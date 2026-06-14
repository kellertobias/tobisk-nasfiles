use std::io::Cursor;

use sha2::{Digest, Sha256};

use super::cache::ThumbError;

const CHAR_WIDTH: u32 = 5;
const CHAR_HEIGHT: u32 = 7;

pub fn render_text_page(name: &str, content: &str, width: u32) -> Result<Vec<u8>, ThumbError> {
    let width = width.clamp(240, 960);
    let height = (width * 3 / 4).max(180);
    let mut img = ::image::RgbImage::from_pixel(width, height, ::image::Rgb([246, 247, 249]));

    fill_rect(&mut img, 0, 0, width, 30, [32, 40, 52]);
    draw_string(&mut img, 12, 10, name, 2, [238, 241, 245], Some(42));

    let scale = 2;
    let line_height = CHAR_HEIGHT * scale + 5;
    let max_chars = ((width.saturating_sub(24)) / ((CHAR_WIDTH + 1) * scale)).max(8) as usize;
    let mut y = 46;

    for line in content.lines().take(18) {
        if y + line_height >= height.saturating_sub(10) {
            break;
        }
        let clean = line.replace('\t', "    ");
        let text = if clean.chars().count() > max_chars {
            let mut s: String = clean.chars().take(max_chars.saturating_sub(3)).collect();
            s.push_str("...");
            s
        } else {
            clean
        };
        draw_string(&mut img, 12, y, &text, scale, [41, 48, 61], Some(max_chars));
        y += line_height;
    }

    encode_jpeg(img)
}

pub fn render_audio_cover(
    title: &str,
    artist: &str,
    seed_text: &str,
    width: u32,
) -> Result<Vec<u8>, ThumbError> {
    let size = width.clamp(240, 960);
    let seed = Sha256::digest(seed_text.as_bytes());
    let a = [seed[0], seed[1], seed[2]];
    let b = [seed[3], seed[4], seed[5]];
    let c = [seed[6], seed[7], seed[8]];
    let mut img = ::image::RgbImage::new(size, size);

    for y in 0..size {
        for x in 0..size {
            let tx = x as f32 / size as f32;
            let ty = y as f32 / size as f32;
            let wave = (((x ^ y) & 31) as f32) / 31.0;
            let color = [
                blend3(a[0], b[0], c[0], tx, ty, wave),
                blend3(a[1], b[1], c[1], tx, ty, wave),
                blend3(a[2], b[2], c[2], tx, ty, wave),
            ];
            img.put_pixel(x, y, ::image::Rgb(color));
        }
    }

    let overlay_h = size / 3;
    fill_rect_alpha(
        &mut img,
        0,
        size - overlay_h,
        size,
        overlay_h,
        [12, 16, 24],
        175,
    );

    let max_chars = ((size.saturating_sub(48)) / 18).max(8) as usize;
    let title = if title.trim().is_empty() {
        "UNTITLED"
    } else {
        title
    };
    let artist = if artist.trim().is_empty() {
        "UNKNOWN ARTIST"
    } else {
        artist
    };

    draw_string(
        &mut img,
        24,
        size - overlay_h + 34,
        title,
        3,
        [255, 255, 255],
        Some(max_chars),
    );
    draw_string(
        &mut img,
        24,
        size - overlay_h + 74,
        artist,
        2,
        [213, 220, 232],
        Some(max_chars + 8),
    );

    encode_jpeg(img)
}

pub fn encode_jpeg(img: ::image::RgbImage) -> Result<Vec<u8>, ThumbError> {
    let mut buf = Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, ::image::ImageFormat::Jpeg)
        .map_err(|e| ThumbError::Image(e.to_string()))?;
    Ok(buf.into_inner())
}

fn blend3(a: u8, b: u8, c: u8, tx: f32, ty: f32, wave: f32) -> u8 {
    let value = a as f32 * (1.0 - tx) + b as f32 * tx;
    let value = value * (1.0 - ty * 0.45) + c as f32 * ty * 0.45;
    let value = value * 0.88 + wave * 32.0;
    value.clamp(24.0, 238.0) as u8
}

fn fill_rect(img: &mut ::image::RgbImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
    let max_x = (x + w).min(img.width());
    let max_y = (y + h).min(img.height());
    for py in y..max_y {
        for px in x..max_x {
            img.put_pixel(px, py, ::image::Rgb(color));
        }
    }
}

fn fill_rect_alpha(
    img: &mut ::image::RgbImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 3],
    alpha: u8,
) {
    let max_x = (x + w).min(img.width());
    let max_y = (y + h).min(img.height());
    for py in y..max_y {
        for px in x..max_x {
            let existing = img.get_pixel(px, py).0;
            let blended = [
                blend_channel(existing[0], color[0], alpha),
                blend_channel(existing[1], color[1], alpha),
                blend_channel(existing[2], color[2], alpha),
            ];
            img.put_pixel(px, py, ::image::Rgb(blended));
        }
    }
}

fn blend_channel(base: u8, overlay: u8, alpha: u8) -> u8 {
    let alpha = alpha as u16;
    (((base as u16 * (255 - alpha)) + (overlay as u16 * alpha)) / 255) as u8
}

pub fn draw_string(
    img: &mut ::image::RgbImage,
    mut x: u32,
    y: u32,
    text: &str,
    scale: u32,
    color: [u8; 3],
    max_chars: Option<usize>,
) {
    let limit = max_chars.unwrap_or(usize::MAX);
    for ch in text.chars().take(limit) {
        draw_char(img, x, y, ch, scale, color);
        x += (CHAR_WIDTH + 1) * scale;
        if x >= img.width().saturating_sub(CHAR_WIDTH * scale) {
            break;
        }
    }
}

fn draw_char(img: &mut ::image::RgbImage, x: u32, y: u32, ch: char, scale: u32, color: [u8; 3]) {
    let glyph = glyph(ch);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..CHAR_WIDTH {
            if bits & (1 << (CHAR_WIDTH - 1 - col)) == 0 {
                continue;
            }
            fill_rect(
                img,
                x + col * scale,
                y + row as u32 * scale,
                scale,
                scale,
                color,
            );
        }
    }
}

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ',' => [0, 0, 0, 0, 0b01100, 0b00100, 0b01000],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        ';' => [0, 0b01100, 0b01100, 0, 0b01100, 0b00100, 0b01000],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '\\' => [
            0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '{' => [
            0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010,
        ],
        '}' => [
            0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000,
        ],
        '<' => [
            0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010,
        ],
        '>' => [
            0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000,
        ],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '*' => [0, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0],
        '"' => [0b01010, 0b01010, 0b01010, 0, 0, 0, 0],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101,
        ],
        '%' => [
            0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011,
        ],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        _ => [
            0b11111, 0b10001, 0b00101, 0b01001, 0b10100, 0b10001, 0b11111,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rendering_produces_jpeg() {
        let bytes = render_text_page("notes.txt", "hello\nworld", 480).unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]));
    }

    #[test]
    fn audio_cover_is_deterministic() {
        let a = render_audio_cover("Title", "Artist", "seed", 480).unwrap();
        let b = render_audio_cover("Title", "Artist", "seed", 480).unwrap();
        assert_eq!(a, b);
    }
}
