use std::path::Path;

/// Compute MD5 ETag for a file, returned as a hex string.
/// Returns a random-looking fallback if the file can't be read.
pub async fn compute_etag(path: &Path) -> String {
    match tokio::fs::read(path).await {
        Ok(data) => format!("{:x}", md5::compute(&data)),
        Err(_) => {
            // Fall back to a deterministic placeholder based on path
            let placeholder = format!("{}", path.display());
            format!("{:x}", md5::compute(placeholder.as_bytes()))
        }
    }
}
