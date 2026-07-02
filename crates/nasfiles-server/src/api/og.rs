use std::sync::OnceLock;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::shares::access;
use crate::state::AppState;
use crate::thumb::{cache::ThumbnailRequest, kind, render, svg};

const APP_ICON_BACKGROUND: [u8; 3] = [26, 26, 46];
const OG_IMAGE_SIZE: u32 = 1200;

/// GET /api/public/og/app-icon.jpg — a static, branded social-preview image
/// used whenever a share link can't (or shouldn't) expose a real preview:
/// password-protected shares, expired/revoked shares, and non-share routes.
pub async fn app_icon() -> Response {
    image_response("image/jpeg", app_icon_bytes())
}

/// GET /api/public/shares/{token}/og-image — the `og:image` for a passwordless,
/// unexpired share. Serves the shared file's real thumbnail when one is
/// supported and downloads are allowed; otherwise falls back to a generic
/// branded "shared item" card, or the app icon if the share isn't in a state
/// that should expose a preview at all.
pub async fn share_og_image(
    State(state): State<AppState>,
    Path(raw_token): Path<String>,
) -> Response {
    let Ok(share) = access::resolve_share_unchecked(&state.pool, &raw_token).await else {
        return image_response("image/jpeg", app_icon_bytes());
    };

    let now = chrono::Utc::now().timestamp_millis();
    let is_expired = share.revoked_at.is_some() || share.expires_at.is_some_and(|exp| now > exp);
    if is_expired || share.password_hash.is_some() {
        return image_response("image/jpeg", app_icon_bytes());
    }

    if !share.is_directory
        && share.allow_download
        && !state.config.no_server_side_execution
        && let Some(thumb) = generate_thumbnail(&state, &share).await
    {
        return image_response(thumb.content_type, thumb.bytes);
    }

    let subtitle = if share.is_directory {
        "Shared Folder"
    } else {
        "Shared File"
    };
    match render::render_audio_cover(share.display_name(), subtitle, &share.id, OG_IMAGE_SIZE) {
        Ok(bytes) => image_response("image/jpeg", bytes),
        Err(_) => image_response("image/jpeg", app_icon_bytes()),
    }
}

async fn generate_thumbnail(
    state: &AppState,
    share: &crate::shares::model::Share,
) -> Option<crate::thumb::cache::Thumbnail> {
    let cache = state.thumb_cache.as_ref()?;
    let resolved = access::resolve_share_path(&state.pool, &state.config, share, "")
        .await
        .ok()?;
    if !kind::supports_thumbnail_path(&resolved, true) {
        return None;
    }

    cache
        .get_or_generate(
            ThumbnailRequest {
                source_path: &resolved,
                root_kind: &share.root_kind,
                root_key: &share.root_key,
                relative_path: &share.relative_path,
                width: OG_IMAGE_SIZE,
                requested_format: None,
            },
            &state.config,
        )
        .await
        .ok()
        .flatten()
}

fn app_icon_bytes() -> Vec<u8> {
    static APP_ICON: OnceLock<Vec<u8>> = OnceLock::new();
    APP_ICON.get_or_init(build_app_icon).clone()
}

fn build_app_icon() -> Vec<u8> {
    if let Some(bytes) = crate::assets::favicon_svg_bytes()
        && let Ok(jpeg) = svg::render_bytes_to_jpeg(&bytes, 630, APP_ICON_BACKGROUND)
    {
        return jpeg;
    }
    // Guaranteed-success fallback if the embedded SVG is ever missing or fails to parse.
    let img = ::image::RgbImage::from_pixel(630, 630, ::image::Rgb(APP_ICON_BACKGROUND));
    render::encode_jpeg(img).unwrap_or_default()
}

fn image_response(content_type: &'static str, bytes: Vec<u8>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
