use sqlx::AnyPool;

/// Log an access event for a share.
///
/// Actions: "open", "download", "upload", "list", "auth_fail"
pub async fn log_access(
    pool: &AnyPool,
    share_id: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
    action: &str,
    path: Option<&str>,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();

    sqlx::query(
        r#"INSERT INTO share_access_log (id, share_id, occurred_at, ip, user_agent, action, path)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(&id)
    .bind(share_id)
    .bind(now)
    .bind(ip)
    .bind(user_agent)
    .bind(action)
    .bind(path)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get the access log for a share (most recent first).
pub async fn get_access_log(
    pool: &AnyPool,
    share_id: &str,
    limit: i64,
) -> Result<Vec<AccessLogEntry>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AccessLogEntry>(
        r#"SELECT id, share_id, occurred_at, ip, user_agent, action, path
           FROM share_access_log
           WHERE share_id = $1
           ORDER BY occurred_at DESC
           LIMIT $2"#,
    )
    .bind(share_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct AccessLogEntry {
    pub id: String,
    pub share_id: String,
    pub occurred_at: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub action: String,
    pub path: Option<String>,
}
