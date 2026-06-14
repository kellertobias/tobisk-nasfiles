use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::Response,
};

/// Embedded frontend assets from the Vite build output.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

/// Serve embedded static assets with SPA fallback.
/// - If the requested path exists as an embedded file, serve it.
/// - Otherwise, serve index.html for client-side routing.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try to find the exact file
    if let Some(file) = Assets::get(path) {
        let content_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CACHE_CONTROL, cache_control(path))
            .body(Body::from(file.data.into_owned()))
            .unwrap();
    }

    // SPA fallback: serve index.html
    match Assets::get("index.html") {
        Some(index) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(index.data.into_owned()))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(
                "frontend not built — run `npm run build` in web/",
            ))
            .unwrap(),
    }
}

/// Cache control based on file type.
/// Hashed assets (Vite output) get long-lived caching. HTML is never cached.
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        // Vite content-hashed assets
        "public, max-age=31536000, immutable"
    } else if path.ends_with(".html") {
        "no-cache"
    } else {
        "public, max-age=3600"
    }
}
