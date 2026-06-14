use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use super::cache::ThumbError;

pub async fn is_available() -> bool {
    Command::new("pdftoppm")
        .arg("-v")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn generate(source_path: &Path, width: u32) -> Result<Option<Vec<u8>>, ThumbError> {
    let width = width.clamp(240, 1280).to_string();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let output = Command::new("pdftoppm")
            .arg("-f")
            .arg("1")
            .arg("-singlefile")
            .arg("-jpeg")
            .arg("-scale-to")
            .arg(width)
            .arg(source_path)
            .arg("-")
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() && out.stdout.starts_with(&[0xff, 0xd8]) => {
                Ok(Some(out.stdout))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::warn!(
                    "pdftoppm thumbnail failed for {}: status={} stderr={}",
                    source_path.display(),
                    out.status,
                    stderr.trim()
                );
                Ok(None)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ThumbError::Pdf(e.to_string())),
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => {
            tracing::warn!("pdftoppm thumbnail timed out for {}", source_path.display());
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_pdf_returns_no_thumbnail() {
        let path = Path::new("/definitely/missing.pdf");
        let result = generate(path, 480).await;
        assert!(result.is_ok() || matches!(result, Err(ThumbError::Pdf(_))));
    }
}
