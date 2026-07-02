use std::path::Path;

use super::cache::ThumbError;
use super::render;

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

/// Rasterize an in-memory SVG (e.g. an embedded frontend asset with no path
/// on disk) onto a solid-colored square canvas and encode it as JPEG.
///
/// Used for static social-preview images (the app icon) where there is no
/// source file to read from the filesystem.
pub fn render_bytes_to_jpeg(
    svg_bytes: &[u8],
    canvas: u32,
    background: [u8; 3],
) -> Result<Vec<u8>, ThumbError> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_bytes, &options)
        .map_err(|e| ThumbError::Svg(format!("invalid SVG: {e}")))?;

    let size = tree.size();
    let scale = (canvas as f32 * 0.6 / size.width()).min(canvas as f32 * 0.6 / size.height());
    let icon_w = (size.width() * scale).round().max(1.0) as u32;
    let icon_h = (size.height() * scale).round().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(icon_w, icon_h)
        .ok_or_else(|| ThumbError::Svg("failed to allocate SVG pixmap".into()))?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut img = ::image::RgbImage::from_pixel(canvas, canvas, ::image::Rgb(background));
    let off_x = (canvas.saturating_sub(icon_w)) / 2;
    let off_y = (canvas.saturating_sub(icon_h)) / 2;

    // tiny_skia pixmaps are premultiplied alpha, so the source contribution
    // is already scaled by alpha — only the background needs attenuating.
    for y in 0..icon_h {
        for x in 0..icon_w {
            let Some(p) = pixmap.pixel(x, y) else {
                continue;
            };
            let a = p.alpha() as u16;
            if a == 0 {
                continue;
            }
            let bg = img.get_pixel(off_x + x, off_y + y).0;
            let blended = [
                (p.red() as u16 + (bg[0] as u16 * (255 - a)) / 255).min(255) as u8,
                (p.green() as u16 + (bg[1] as u16 * (255 - a)) / 255).min(255) as u8,
                (p.blue() as u16 + (bg[2] as u16 * (255 - a)) / 255).min(255) as u8,
            ];
            img.put_pixel(off_x + x, off_y + y, ::image::Rgb(blended));
        }
    }

    render::encode_jpeg(img)
}
