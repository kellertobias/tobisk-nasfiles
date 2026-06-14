use std::path::Path;

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::io::AsyncReadExt;

/// Serve a file with HTTP Range support.
/// Handles single-range requests with proper 206 Partial Content responses.
pub async fn serve_file(path: &Path, headers: &HeaderMap) -> Result<Response, StreamError> {
    if !path.is_file() {
        return Err(StreamError::NotFound);
    }

    let metadata = tokio::fs::metadata(path).await.map_err(StreamError::Io)?;
    let file_size = metadata.len();

    // Detect content type from extension
    let content_type = mime_guess::from_path(path)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Determine content disposition
    let disposition = if is_inline_safe(&content_type) {
        "inline"
    } else {
        "attachment"
    };
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    // Check for Range header
    if let Some(range_header) = headers.get(header::RANGE) {
        let range_str = range_header.to_str().map_err(|_| StreamError::BadRange)?;
        if let Some((start, end)) = parse_range(range_str, file_size) {
            let length = end - start + 1;

            let file = tokio::fs::File::open(path).await.map_err(StreamError::Io)?;
            let mut file = tokio::io::BufReader::new(file);

            // Seek to start position
            tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start))
                .await
                .map_err(StreamError::Io)?;

            // Read the range
            let stream = tokio_util::io::ReaderStream::new(file.take(length));
            let body = Body::from_stream(stream);

            let filename = super::sanitize_header_filename(filename);

            return Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, &content_type)
                .header(header::CONTENT_LENGTH, length)
                .header(header::CONTENT_ENCODING, "identity")
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{file_size}"),
                )
                .header(header::ACCEPT_RANGES, "bytes")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("{disposition}; filename=\"{filename}\""),
                )
                .header("X-Content-Type-Options", "nosniff")
                .body(body)
                .map_err(|e| StreamError::Response(e.to_string()));
        } else {
            return Err(StreamError::BadRange);
        }
    }

    // Full file response
    let file = tokio::fs::File::open(path).await.map_err(StreamError::Io)?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let filename = super::sanitize_header_filename(filename);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &content_type)
        .header(header::CONTENT_LENGTH, file_size)
        .header(header::CONTENT_ENCODING, "identity")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_DISPOSITION,
            format!("{disposition}; filename=\"{filename}\""),
        )
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .map_err(|e| StreamError::Response(e.to_string()))
}

/// Parse a "bytes=start-end" range header. Returns (start, end) inclusive.
fn parse_range(range: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range.splitn(2, '-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start: u64;
    let end: u64;

    if parts[0].is_empty() {
        // Suffix range: -500 means last 500 bytes
        let suffix_len: u64 = parts[1].parse().ok()?;
        if suffix_len > file_size {
            start = 0;
        } else {
            start = file_size - suffix_len;
        }
        end = file_size - 1;
    } else {
        start = parts[0].parse().ok()?;
        if parts[1].is_empty() {
            end = file_size - 1;
        } else {
            end = parts[1].parse().ok()?;
        }
    }

    if start > end || start >= file_size {
        return None;
    }

    // Clamp end to file size
    let end = end.min(file_size - 1);

    Some((start, end))
}

/// Check if a content type is safe to serve inline.
fn is_inline_safe(content_type: &str) -> bool {
    (content_type.starts_with("image/") && content_type != "image/svg+xml")
        || content_type.starts_with("video/")
        || content_type.starts_with("audio/")
        || content_type == "application/pdf"
        || content_type.starts_with("text/plain")
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("file not found")]
    NotFound,
    #[error("invalid range")]
    BadRange,
    #[error("io error: {0}")]
    Io(std::io::Error),
    #[error("response error: {0}")]
    Response(String),
}

impl IntoResponse for StreamError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            StreamError::NotFound => (StatusCode::NOT_FOUND, "file not found"),
            StreamError::BadRange => (StatusCode::RANGE_NOT_SATISFIABLE, "invalid range"),
            StreamError::Io(_) | StreamError::Response(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        };
        (status, axum::Json(serde_json::json!({"error": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_normal() {
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
    }

    #[test]
    fn test_parse_range_open_end() {
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn test_parse_range_suffix() {
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
    }

    #[test]
    fn test_parse_range_start_beyond_size() {
        assert_eq!(parse_range("bytes=2000-", 1000), None);
    }

    #[test]
    fn test_parse_range_start_greater_than_end() {
        assert_eq!(parse_range("bytes=500-100", 1000), None);
    }

    #[test]
    fn test_parse_range_clamp_end() {
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 999)));
    }

    #[test]
    fn test_svg_is_not_inline_safe() {
        assert!(!is_inline_safe("image/svg+xml"));
        assert!(is_inline_safe("image/png"));
    }
}
