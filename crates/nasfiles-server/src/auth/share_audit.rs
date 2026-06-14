#![allow(clippy::collapsible_if)]
use crate::config;
use crate::state::AppState;
use openidconnect::{OAuth2TokenResponse, RefreshToken};
use serde_json::Value;

pub fn spawn_daily_share_audit(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Sleep for 24h initially
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        interval.tick().await; // First tick fires immediately

        loop {
            interval.tick().await;

            tracing::info!("Starting daily share audit scan");

            // Query distinct users with active shares
            let user_ids_res: Result<Vec<(String,)>, sqlx::Error> = sqlx::query_as(
                "SELECT DISTINCT owner_user_id FROM shares WHERE revoked_at IS NULL",
            )
            .fetch_all(&state.pool)
            .await;

            let user_ids = match user_ids_res {
                Ok(u) => u.into_iter().map(|(id,)| id).collect::<Vec<_>>(),
                Err(e) => {
                    tracing::error!("Failed to fetch active share owners: {}", e);
                    continue;
                }
            };

            if user_ids.is_empty() {
                tracing::info!("No active shares to audit");
                continue;
            }

            let oidc_state = match super::oidc::OIDC_CLIENT.get() {
                Some(s) => s,
                None => {
                    tracing::warn!("OIDC not configured, skipping daily share audit");
                    continue;
                }
            };

            let http_client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default();

            for user_id in user_ids {
                // Fetch user data
                let user_row: Result<Option<(String, Option<String>)>, sqlx::Error> =
                    sqlx::query_as(
                        "SELECT external_id, oidc_refresh_token FROM users WHERE id = $1",
                    )
                    .bind(&user_id)
                    .fetch_optional(&state.pool)
                    .await;

                let (_external_id, enc_refresh_token) = match user_row {
                    Ok(Some((ext, Some(enc_rt)))) => (ext, enc_rt),
                    _ => {
                        // Missing user or no refresh token -> revoke all shares
                        tracing::warn!(
                            "User {} missing or has no refresh token, revoking active shares",
                            user_id
                        );
                        revoke_all_active_shares(&state.pool, &user_id).await;
                        continue;
                    }
                };

                let refresh_token = match super::oidc::decrypt_token(
                    &enc_refresh_token,
                    &state.config.session_secret,
                ) {
                    Ok(rt) => rt,
                    Err(_) => {
                        tracing::warn!(
                            "Failed to decrypt refresh token for user {}, revoking active shares",
                            user_id
                        );
                        revoke_all_active_shares(&state.pool, &user_id).await;
                        continue;
                    }
                };

                // Exchange refresh token
                let rt = RefreshToken::new(refresh_token);
                let token_req = match oidc_state.client.exchange_refresh_token(&rt) {
                    Ok(req) => req,
                    Err(_) => {
                        tracing::warn!(
                            "Refresh token exchange failed for user {}, revoking active shares",
                            user_id
                        );
                        let _ = sqlx::query("UPDATE users SET oidc_access_token = NULL, oidc_refresh_token = NULL WHERE id = $1")
                            .bind(&user_id)
                            .execute(&state.pool)
                            .await;
                        revoke_all_active_shares(&state.pool, &user_id).await;
                        continue;
                    }
                };
                let token_resp = token_req.request_async(&http_client).await;

                let (access_token, _new_enc_refresh) = match token_resp {
                    Ok(resp) => {
                        let at = resp.access_token().secret().clone();
                        let new_rt = resp.refresh_token().map(|t| t.secret().clone());
                        let enc_rt = new_rt.as_deref().and_then(|t| {
                            super::oidc::encrypt_token(t, &state.config.session_secret).ok()
                        });
                        let enc_at = super::oidc::encrypt_token(&at, &state.config.session_secret)
                            .unwrap_or_default();

                        // Update DB
                        let _ = sqlx::query("UPDATE users SET oidc_access_token = $1, oidc_refresh_token = COALESCE($2, oidc_refresh_token) WHERE id = $3")
                            .bind(&enc_at)
                            .bind(&enc_rt)
                            .bind(&user_id)
                            .execute(&state.pool)
                            .await;

                        (at, enc_rt)
                    }
                    Err(_) => {
                        tracing::warn!(
                            "Refresh token exchange failed for user {}, revoking active shares",
                            user_id
                        );
                        let _ = sqlx::query("UPDATE users SET oidc_access_token = NULL, oidc_refresh_token = NULL WHERE id = $1")
                            .bind(&user_id)
                            .execute(&state.pool)
                            .await;
                        revoke_all_active_shares(&state.pool, &user_id).await;
                        continue;
                    }
                };

                // Get userinfo
                let resp = match http_client
                    .get(oidc_state.userinfo_url.clone())
                    .bearer_auth(&access_token)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            "Daily share audit skipped for user {} due to network error: {}",
                            user_id,
                            e
                        );
                        continue; // Skip, don't revoke
                    }
                };

                if resp.status() == 404 {
                    tracing::warn!("User {} not found in IdP, revoking active shares", user_id);
                    revoke_all_active_shares(&state.pool, &user_id).await;
                    continue;
                } else if !resp.status().is_success() {
                    tracing::warn!(
                        "IdP returned {} for user {}, skipping",
                        resp.status(),
                        user_id
                    );
                    continue; // Skip, don't revoke
                }

                let userinfo_json = match resp.json::<Value>().await {
                    Ok(json) => json,
                    Err(_) => {
                        tracing::warn!("Failed to parse userinfo for user {}, skipping", user_id);
                        continue;
                    }
                };

                let groups = super::oidc::extract_claim_array(
                    &userinfo_json,
                    &state.config.sso_groups_claim,
                )
                .unwrap_or_default();

                let folder_permissions = config::compute_folder_permissions(&state.config, &groups);

                // Find shares where the user no longer has capability
                let active_shares: Result<Vec<(String, String)>, sqlx::Error> = sqlx::query_as(
                    "SELECT id, root_key FROM shares WHERE owner_user_id = $1 AND revoked_at IS NULL"
                )
                .bind(&user_id)
                .fetch_all(&state.pool)
                .await;

                if let Ok(shares) = active_shares {
                    let now = chrono::Utc::now().timestamp_millis();
                    for (share_id, root_key) in shares {
                        let caps = folder_permissions
                            .get(&root_key)
                            .copied()
                            .unwrap_or_default();
                        if !caps.share && !caps.read {
                            let _ = sqlx::query("UPDATE shares SET revoked_at = $1 WHERE id = $2")
                                .bind(now)
                                .bind(&share_id)
                                .execute(&state.pool)
                                .await;

                            tracing::info!(
                                "Revoked share {} for user {} (lost access to root_key {})",
                                share_id,
                                user_id,
                                root_key
                            );
                        }
                    }
                }
            }

            tracing::info!("Daily share audit scan complete");
        }
    })
}

async fn revoke_all_active_shares(pool: &sqlx::AnyPool, user_id: &str) {
    let now = chrono::Utc::now().timestamp_millis();
    let res = sqlx::query(
        "UPDATE shares SET revoked_at = $1 WHERE owner_user_id = $2 AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(user_id)
    .execute(pool)
    .await;
    if let Ok(result) = res {
        if result.rows_affected() > 0 {
            tracing::info!(
                "Revoked {} active shares for user {}",
                result.rows_affected(),
                user_id
            );
        }
    }
}
