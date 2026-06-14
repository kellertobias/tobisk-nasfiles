use std::io::Cursor;
use std::path::Path;

use super::cache::ThumbError;

/// Generate a JPEG thumbnail from an image file.
///
/// Decodes the image, resizes to `width` px (preserving aspect ratio),
/// and encodes as JPEG quality 80.
///
/// Runs in `spawn_blocking` to avoid blocking the async runtime.
pub async fn generate(
    source_path: &Path,
    width: u32,
    max_image_width: u32,
    max_image_height: u32,
    max_alloc: u64,
) -> Result<Option<Vec<u8>>, ThumbError> {
    let path = source_path.to_path_buf();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ThumbError> {
        let mut reader =
            ::image::ImageReader::open(&path).map_err(|e| ThumbError::Image(e.to_string()))?;
        let mut limits = ::image::Limits::default();
        limits.max_image_width = Some(max_image_width);
        limits.max_image_height = Some(max_image_height);
        limits.max_alloc = Some(max_alloc);
        reader.limits(limits);
        let img = reader
            .decode()
            .map_err(|e| ThumbError::Image(e.to_string()))?;

        resize_and_encode(img, width)
    })
    .await
    .map_err(|e| ThumbError::Image(format!("task join error: {e}")))?;

    result.map(Some)
}

pub async fn generate_from_bytes(
    bytes: Vec<u8>,
    width: u32,
    max_image_width: u32,
    max_image_height: u32,
    max_alloc: u64,
) -> Result<Option<Vec<u8>>, ThumbError> {
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ThumbError> {
        let cursor = Cursor::new(bytes);
        let mut reader = ::image::ImageReader::new(cursor)
            .with_guessed_format()
            .map_err(|e| ThumbError::Image(format!("failed to guess image format: {e}")))?;
        let mut limits = ::image::Limits::default();
        limits.max_image_width = Some(max_image_width);
        limits.max_image_height = Some(max_image_height);
        limits.max_alloc = Some(max_alloc);
        reader.limits(limits);
        let img = reader
            .decode()
            .map_err(|e| ThumbError::Image(e.to_string()))?;

        resize_and_encode(img, width)
    })
    .await
    .map_err(|e| ThumbError::Image(format!("task join error: {e}")))?;

    result.map(Some)
}

fn resize_and_encode(img: ::image::DynamicImage, width: u32) -> Result<Vec<u8>, ThumbError> {
    let thumb = img.thumbnail(width, width).to_rgb8();
    let mut buf = Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgb8(thumb)
        .write_to(&mut buf, ::image::ImageFormat::Jpeg)
        .map_err(|e| ThumbError::Image(e.to_string()))?;
    Ok(buf.into_inner())
}
