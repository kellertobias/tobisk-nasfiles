use std::path::Path;

use super::cache::ThumbError;

/// Render an SVG to PNG bytes without exposing the original XML to the browser.
///
/// `usvg` resolves the SVG structure in-process and `resvg` rasterizes it into
/// a bounded pixmap. External resource loading is not configured here.
pub async fn generate(
    source_path: &Path,
    width: u32,
    max_svg_width: u32,
    max_svg_height: u32,
) -> Result<Option<Vec<u8>>, ThumbError> {
    let path = source_path.to_path_buf();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ThumbError> {
        let svg = std::fs::read(&path).map_err(|e| ThumbError::Svg(e.to_string()))?;
        let options = usvg::Options::default();
        let tree = usvg::Tree::from_data(&svg, &options)
            .map_err(|e| ThumbError::Svg(format!("invalid SVG: {e}")))?;

        let size = tree.size();
        if size.width() > max_svg_width as f32 || size.height() > max_svg_height as f32 {
            return Err(ThumbError::TooLarge {
                size: size.width().max(size.height()) as u64,
                limit: max_svg_width.max(max_svg_height) as u64,
            });
        }

        let scale = (width as f32 / size.width()).min(width as f32 / size.height());
        let target_width = (size.width() * scale).ceil().max(1.0) as u32;
        let target_height = (size.height() * scale).ceil().max(1.0) as u32;

        let mut pixmap = tiny_skia::Pixmap::new(target_width, target_height)
            .ok_or_else(|| ThumbError::Svg("failed to allocate SVG pixmap".into()))?;

        pixmap.fill(tiny_skia::Color::WHITE);
        let transform = tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        pixmap
            .encode_png()
            .map_err(|e| ThumbError::Svg(format!("PNG encode error: {e}")))
    })
    .await
    .map_err(|e| ThumbError::Svg(format!("task join error: {e}")))?;

    result.map(Some)
}
