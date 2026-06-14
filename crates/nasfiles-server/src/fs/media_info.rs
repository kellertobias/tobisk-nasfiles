use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nasfiles_core::models::MediaInfo;
use serde::Deserialize;
use tokio::process::Command;

use crate::thumb::cache::ThumbnailCache;

pub async fn get_or_probe(
    cache_dir: &Path,
    source_path: &Path,
    root_kind: &str,
    root_key: &str,
    relative_path: &str,
) -> Result<Option<MediaInfo>, MediaInfoError> {
    if !is_media(source_path) {
        return Ok(None);
    }

    let metadata = tokio::fs::metadata(source_path)
        .await
        .map_err(|e| MediaInfoError::Io(e.to_string()))?;

    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let cache_root_kind = format!("media-info-v3:{root_kind}");
    let key = ThumbnailCache::cache_key(
        &cache_root_kind,
        root_key,
        relative_path,
        mtime_ms,
        metadata.len(),
    );
    let cache_path = cache_path(cache_dir, &key);

    if cache_path.exists() {
        let bytes = tokio::fs::read(&cache_path)
            .await
            .map_err(|e| MediaInfoError::Io(e.to_string()))?;
        let info =
            serde_json::from_slice(&bytes).map_err(|e| MediaInfoError::Parse(e.to_string()))?;
        return Ok(Some(info));
    }

    let Some(info) = probe(source_path).await? else {
        return Ok(None);
    };

    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| MediaInfoError::Io(e.to_string()))?;
    }

    let bytes = serde_json::to_vec(&info).map_err(|e| MediaInfoError::Parse(e.to_string()))?;
    tokio::fs::write(cache_path, bytes)
        .await
        .map_err(|e| MediaInfoError::Io(e.to_string()))?;

    Ok(Some(info))
}

fn cache_path(cache_dir: &Path, key: &str) -> PathBuf {
    cache_dir
        .join("media-info")
        .join(&key[0..2])
        .join(&key[2..4])
        .join(format!("{key}.json"))
}

fn is_media(path: &Path) -> bool {
    mime_guess::from_path(path).first().is_some_and(|mime| {
        let essence = mime.essence_str();
        essence.starts_with("video/") || essence.starts_with("audio/")
    })
}

async fn probe(source_path: &Path) -> Result<Option<MediaInfo>, MediaInfoError> {
    let output = tokio::time::timeout(Duration::from_secs(10), async {
        Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-print_format")
            .arg("json")
            .arg("-show_format")
            .arg("-show_streams")
            .arg(source_path)
            .output()
            .await
    })
    .await
    .map_err(|_| MediaInfoError::Timeout)?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            MediaInfoError::FfprobeUnavailable
        } else {
            MediaInfoError::Io(e.to_string())
        }
    })?;

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }

    let probe: ProbeOutput =
        serde_json::from_slice(&output.stdout).map_err(|e| MediaInfoError::Parse(e.to_string()))?;
    Ok(Some(probe.into_media_info()))
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    mime_codec_string: Option<String>,
    bit_rate: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<String>,
    tags: Option<ProbeTags>,
}

#[derive(Deserialize)]
struct ProbeTags {
    language: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

impl ProbeOutput {
    fn into_media_info(self) -> MediaInfo {
        let video = self
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"));
        let audio = self
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("audio"));
        let audio_languages = audio_languages(&self.streams);
        let format = self.format.as_ref();

        let duration_ms = format
            .and_then(|format| parse_duration_ms(format.duration.as_deref()))
            .or_else(|| {
                self.streams
                    .iter()
                    .find_map(|stream| parse_duration_ms(stream.duration.as_deref()))
            });
        let bitrate_bps = format
            .and_then(|format| parse_u64(format.bit_rate.as_deref()))
            .or_else(|| {
                let total: u64 = self
                    .streams
                    .iter()
                    .filter_map(|stream| parse_u64(stream.bit_rate.as_deref()))
                    .sum();
                (total > 0).then_some(total)
            });

        MediaInfo {
            duration_ms,
            width: video.and_then(|stream| stream.width),
            height: video.and_then(|stream| stream.height),
            video_codec: video.and_then(|stream| stream.codec_name.clone()),
            audio_codec: audio.and_then(|stream| stream.codec_name.clone()),
            bitrate_bps,
            format_name: format.and_then(|format| format.format_name.clone()),
            video_mime_codec: video
                .and_then(|stream| stream.mime_codec_string.clone())
                .or_else(|| video.and_then(|stream| codec_name_to_mime_codec(&stream.codec_name))),
            audio_mime_codec: audio
                .and_then(|stream| stream.mime_codec_string.clone())
                .or_else(|| audio.and_then(|stream| codec_name_to_mime_codec(&stream.codec_name))),
            audio_languages,
        }
    }
}

fn parse_u64(value: Option<&str>) -> Option<u64> {
    value?.parse().ok()
}

fn codec_name_to_mime_codec(codec_name: &Option<String>) -> Option<String> {
    let codec_name = codec_name.as_deref()?;
    let codec = match codec_name {
        "aac" => "mp4a.40.2",
        "alac" => "alac",
        "av1" => "av01",
        "h264" => "avc1",
        "hevc" | "h265" => "hvc1",
        "mp3" => "mp3",
        "opus" => "opus",
        "vp8" => "vp8",
        "vp9" => "vp9",
        _ => return None,
    };

    Some(codec.to_string())
}

fn parse_duration_ms(duration: Option<&str>) -> Option<u64> {
    let seconds: f64 = duration?.parse().ok()?;
    if seconds.is_finite() && seconds >= 0.0 {
        Some((seconds * 1000.0).round() as u64)
    } else {
        None
    }
}

fn audio_languages(streams: &[ProbeStream]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut languages = Vec::new();

    for stream in streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
    {
        let Some(language) = stream
            .tags
            .as_ref()
            .and_then(|tags| normalize_language(tags.language.as_deref()))
        else {
            continue;
        };

        if seen.insert(language.clone()) {
            languages.push(language);
        }
    }

    languages
}

fn normalize_language(language: Option<&str>) -> Option<String> {
    let language = language?.trim().to_lowercase();
    if language.is_empty() || language == "und" {
        return None;
    }

    let normalized = match language.as_str() {
        "deu" | "ger" => "de",
        "eng" => "en",
        "fra" | "fre" => "fr",
        "spa" => "es",
        "ita" => "it",
        "por" => "pt",
        "nld" | "dut" => "nl",
        "jpn" => "ja",
        "kor" => "ko",
        "zho" | "chi" => "zh",
        _ => language.as_str(),
    };

    Some(normalized.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum MediaInfoError {
    #[error("ffprobe is not available")]
    FfprobeUnavailable,
    #[error("media info probe timed out")]
    Timeout,
    #[error("io error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duration_ms() {
        assert_eq!(parse_duration_ms(Some("12.345")), Some(12_345));
        assert_eq!(parse_duration_ms(Some("0.004")), Some(4));
        assert_eq!(parse_duration_ms(Some("nan")), None);
        assert_eq!(parse_duration_ms(None), None);
    }

    #[test]
    fn normalizes_audio_languages() {
        assert_eq!(normalize_language(Some("deu")), Some("de".to_string()));
        assert_eq!(normalize_language(Some("eng")), Some("en".to_string()));
        assert_eq!(normalize_language(Some("fre")), Some("fr".to_string()));
        assert_eq!(normalize_language(Some("und")), None);
    }

    #[test]
    fn collects_unique_audio_languages() {
        let streams = vec![
            ProbeStream {
                codec_type: Some("audio".to_string()),
                codec_name: Some("aac".to_string()),
                mime_codec_string: Some("mp4a.40.2".to_string()),
                bit_rate: Some("128000".to_string()),
                width: None,
                height: None,
                duration: None,
                tags: Some(ProbeTags {
                    language: Some("deu".to_string()),
                }),
            },
            ProbeStream {
                codec_type: Some("audio".to_string()),
                codec_name: Some("aac".to_string()),
                mime_codec_string: None,
                bit_rate: None,
                width: None,
                height: None,
                duration: None,
                tags: Some(ProbeTags {
                    language: Some("eng".to_string()),
                }),
            },
            ProbeStream {
                codec_type: Some("audio".to_string()),
                codec_name: Some("aac".to_string()),
                mime_codec_string: None,
                bit_rate: None,
                width: None,
                height: None,
                duration: None,
                tags: Some(ProbeTags {
                    language: Some("de".to_string()),
                }),
            },
        ];

        assert_eq!(audio_languages(&streams), vec!["de", "en"]);
    }

    #[test]
    fn extracts_playback_metadata() {
        let probe = ProbeOutput {
            streams: vec![
                ProbeStream {
                    codec_type: Some("video".to_string()),
                    codec_name: Some("h264".to_string()),
                    mime_codec_string: Some("avc1.4d401f".to_string()),
                    bit_rate: Some("1000000".to_string()),
                    width: Some(1280),
                    height: Some(720),
                    duration: None,
                    tags: None,
                },
                ProbeStream {
                    codec_type: Some("audio".to_string()),
                    codec_name: Some("aac".to_string()),
                    mime_codec_string: Some("mp4a.40.2".to_string()),
                    bit_rate: Some("128000".to_string()),
                    width: None,
                    height: None,
                    duration: None,
                    tags: None,
                },
            ],
            format: Some(ProbeFormat {
                format_name: Some("mov,mp4,m4a,3gp,3g2,mj2".to_string()),
                duration: Some("12.5".to_string()),
                bit_rate: Some("1128000".to_string()),
            }),
        };

        let info = probe.into_media_info();

        assert_eq!(info.duration_ms, Some(12_500));
        assert_eq!(info.bitrate_bps, Some(1_128_000));
        assert_eq!(info.format_name.as_deref(), Some("mov,mp4,m4a,3gp,3g2,mj2"));
        assert_eq!(info.video_mime_codec.as_deref(), Some("avc1.4d401f"));
        assert_eq!(info.audio_mime_codec.as_deref(), Some("mp4a.40.2"));
    }
}
