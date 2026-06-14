use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use super::{cache::ThumbError, image as img_thumb, render};

pub async fn generate(
    source_path: &Path,
    width: u32,
    max_image_width: u32,
    max_image_height: u32,
    max_alloc: u64,
) -> Result<Option<Vec<u8>>, ThumbError> {
    if let Some(cover) = extract_embedded_cover(source_path).await? {
        match img_thumb::generate_from_bytes(
            cover,
            width,
            max_image_width,
            max_image_height,
            max_alloc,
        )
        .await
        {
            Ok(Some(bytes)) => return Ok(Some(bytes)),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "failed to decode embedded audio cover for {}: {e}",
                    source_path.display()
                );
            }
        }
    }

    let Some(tags) = probe_tags(source_path).await? else {
        return Ok(None);
    };

    let title = tag_value(&tags, &["title", "TITLE"]).unwrap_or_else(|| {
        source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string()
    });
    let artist = tag_value(&tags, &["artist", "ARTIST", "album_artist", "ALBUMARTIST"])
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let seed = format!("{}:{}:{}", source_path.display(), title, artist);
    render::render_audio_cover(&title, &artist, &seed, width).map(Some)
}

async fn extract_embedded_cover(source_path: &Path) -> Result<Option<Vec<u8>>, ThumbError> {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let output = Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(source_path)
            .arg("-map")
            .arg("0:v:0")
            .arg("-an")
            .arg("-sn")
            .arg("-dn")
            .arg("-frames:v")
            .arg("1")
            .arg("-f")
            .arg("image2")
            .arg("pipe:1")
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() && !out.stdout.is_empty() => Ok(Some(out.stdout)),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ThumbError::Audio(e.to_string())),
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Ok(None),
    }
}

async fn probe_tags(source_path: &Path) -> Result<Option<HashMap<String, String>>, ThumbError> {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let output = Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-print_format")
            .arg("json")
            .arg("-show_format")
            .arg(source_path)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() && !out.stdout.is_empty() => {
                let parsed: ProbeOutput = serde_json::from_slice(&out.stdout)
                    .map_err(|e| ThumbError::Audio(e.to_string()))?;
                Ok(parsed.format.and_then(|format| format.tags))
            }
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ThumbError::Audio(e.to_string())),
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Ok(None),
    }
}

fn tag_value(tags: &HashMap<String, String>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        tags.get(*name)
            .or_else(|| tags.get(&name.to_lowercase()))
            .or_else(|| tags.get(&name.to_uppercase()))
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

#[derive(Deserialize)]
struct ProbeOutput {
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    tags: Option<HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tags_case_insensitively() {
        let mut tags = HashMap::new();
        tags.insert("TITLE".to_string(), "Song".to_string());
        assert_eq!(tag_value(&tags, &["title"]), Some("Song".to_string()));
    }
}
