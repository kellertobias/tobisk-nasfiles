use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use super::{cache::ThumbError, process};

/// Check if ffmpeg is available on the system.
pub async fn is_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn ffprobe_is_available() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Generate a JPEG thumbnail from a video file using ffmpeg.
///
/// Probes duration, tries up to three candidate frames from early in the clip,
/// and returns the first frame that is not effectively black.
pub async fn generate(source_path: &Path, width: u32) -> Result<Option<Vec<u8>>, ThumbError> {
    let duration_ms = probe_duration_ms(source_path).await.unwrap_or(None);
    for offset in candidate_offsets(duration_ms) {
        let Some(bytes) = extract_frame(source_path, offset, width).await? else {
            continue;
        };
        if !is_black_frame(&bytes) {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

async fn probe_duration_ms(source_path: &Path) -> Result<Option<u64>, ThumbError> {
    let mut command = Command::new("ffprobe");
    command
        .arg("-v")
        .arg("error")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg(source_path);
    let Some(output) =
        process::output_with_timeout(command, Duration::from_secs(8), "ffprobe", source_path)
            .await?
    else {
        return Ok(None);
    };

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }

    let parsed: ProbeOutput =
        serde_json::from_slice(&output.stdout).map_err(|e| ThumbError::Video(e.to_string()))?;
    Ok(parsed
        .format
        .and_then(|format| parse_duration_ms(format.duration.as_deref())))
}

pub fn candidate_offsets(duration_ms: Option<u64>) -> Vec<f64> {
    let Some(duration_ms) = duration_ms.filter(|d| *d > 0) else {
        return vec![1.0, 3.0, 5.0];
    };

    let duration_s = duration_ms as f64 / 1000.0;
    let base = 30.0_f64.min(duration_s * 0.20).max(0.1);
    let max_time = (duration_s - 0.1).max(0.1);
    let mut offsets = Vec::new();

    for multiplier in 1..=3 {
        let offset = (base * multiplier as f64).min(max_time);
        let rounded = (offset * 100.0).round() / 100.0;
        if !offsets
            .iter()
            .any(|existing: &f64| (*existing - rounded).abs() < 0.01)
        {
            offsets.push(rounded);
        }
    }

    offsets
}

async fn extract_frame(
    source_path: &Path,
    offset_seconds: f64,
    width: u32,
) -> Result<Option<Vec<u8>>, ThumbError> {
    let width = width.clamp(240, 1280);
    let scale_filter = format!(
        "scale=w='min({width},iw)':h=-2:force_original_aspect_ratio=decrease,scale=w='min(1280,iw)':h='min(720,ih)':force_original_aspect_ratio=decrease,format=yuvj420p"
    );

    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(format!("{offset_seconds:.2}"))
        .arg("-i")
        .arg(source_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-sn")
        .arg("-dn")
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(&scale_filter)
        .arg("-f")
        .arg("image2")
        .arg("-c:v")
        .arg("mjpeg")
        .arg("-q:v")
        .arg("4")
        .arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Some(out) =
        process::output_with_timeout(command, Duration::from_secs(20), "ffmpeg", source_path)
            .await?
    else {
        return Ok(None);
    };

    if out.status.success() && !out.stdout.is_empty() {
        Ok(Some(out.stdout))
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!(
            "ffmpeg thumbnail failed for {} at {offset_seconds:.2}s: status={} stderr={}",
            source_path.display(),
            out.status,
            stderr.trim()
        );
        Ok(None)
    }
}

pub fn is_black_frame(bytes: &[u8]) -> bool {
    let Ok(img) = ::image::load_from_memory(bytes) else {
        return false;
    };
    let rgb = img.to_rgb8();
    let mut sampled = 0_u64;
    let mut non_black = 0_u64;
    let step_x = (rgb.width() / 80).max(1);
    let step_y = (rgb.height() / 45).max(1);

    for y in (0..rgb.height()).step_by(step_y as usize) {
        for x in (0..rgb.width()).step_by(step_x as usize) {
            sampled += 1;
            let [r, g, b] = rgb.get_pixel(x, y).0;
            let luminance = (0.2126 * r as f32) + (0.7152 * g as f32) + (0.0722 * b as f32);
            if luminance > 12.0 {
                non_black += 1;
            }
        }
    }

    sampled > 0 && (non_black as f64 / sampled as f64) < 0.01
}

#[derive(Deserialize)]
struct ProbeOutput {
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

fn parse_duration_ms(duration: Option<&str>) -> Option<u64> {
    let seconds: f64 = duration?.parse().ok()?;
    if seconds.is_finite() && seconds >= 0.0 {
        Some((seconds * 1000.0).round() as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_candidate_offsets() {
        assert_eq!(candidate_offsets(Some(10_000)), vec![2.0, 4.0, 6.0]);
        assert_eq!(candidate_offsets(Some(300_000)), vec![30.0, 60.0, 90.0]);
        assert_eq!(candidate_offsets(None), vec![1.0, 3.0, 5.0]);
    }

    #[test]
    fn detects_black_frame() {
        let img = ::image::RgbImage::from_pixel(64, 64, ::image::Rgb([0, 0, 0]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(img)
            .write_to(&mut cursor, ::image::ImageFormat::Jpeg)
            .unwrap();
        assert!(is_black_frame(&cursor.into_inner()));
    }

    #[test]
    fn accepts_non_black_frame() {
        let img = ::image::RgbImage::from_pixel(64, 64, ::image::Rgb([120, 120, 120]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(img)
            .write_to(&mut cursor, ::image::ImageFormat::Jpeg)
            .unwrap();
        assert!(!is_black_frame(&cursor.into_inner()));
    }
}
