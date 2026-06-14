use std::path::Path;

use super::{cache::ThumbError, render};

const MAX_TEXT_BYTES: u64 = 64 * 1024;

pub async fn generate(source_path: &Path, width: u32) -> Result<Option<Vec<u8>>, ThumbError> {
    let name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("text");
    let mut file = tokio::fs::File::open(source_path)
        .await
        .map_err(|e| ThumbError::Io(e.to_string()))?;
    let mut limited = tokio::io::AsyncReadExt::take(&mut file, MAX_TEXT_BYTES);
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut limited, &mut bytes)
        .await
        .map_err(|e| ThumbError::Io(e.to_string()))?;
    let content = String::from_utf8_lossy(&bytes);
    render::render_text_page(name, &content, width).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn renders_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        tokio::fs::write(&path, "one\ntwo\nthree").await.unwrap();
        let bytes = generate(&path, 480).await.unwrap().unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]));
    }
}
