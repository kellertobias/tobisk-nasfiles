use std::path::{Path, PathBuf};

use async_zip::tokio::read::fs::ZipFileReader;
use futures_lite::io::AsyncReadExt;

use super::{cache::ThumbError, image as img_thumb};

const MAX_EPUB_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

pub async fn generate(
    source_path: &Path,
    width: u32,
    max_image_width: u32,
    max_image_height: u32,
    max_alloc: u64,
) -> Result<Option<Vec<u8>>, ThumbError> {
    let reader = ZipFileReader::new(source_path)
        .await
        .map_err(|e| ThumbError::Epub(e.to_string()))?;
    let Some(container) = read_entry_text(&reader, "META-INF/container.xml").await? else {
        return Ok(None);
    };
    let Some(opf_path) = rootfile_path(&container) else {
        return Ok(None);
    };
    let Some(opf) = read_entry_text(&reader, &opf_path).await? else {
        return Ok(None);
    };
    let Some(cover_href) = cover_href(&opf) else {
        return Ok(None);
    };
    let cover_path = resolve_epub_path(&opf_path, &cover_href);
    let Some(cover_bytes) = read_entry_bytes(&reader, &cover_path).await? else {
        return Ok(None);
    };

    img_thumb::generate_from_bytes(
        cover_bytes,
        width,
        max_image_width,
        max_image_height,
        max_alloc,
    )
    .await
}

async fn read_entry_text(
    reader: &ZipFileReader,
    entry_path: &str,
) -> Result<Option<String>, ThumbError> {
    let Some(bytes) = read_entry_bytes(reader, entry_path).await? else {
        return Ok(None);
    };
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

async fn read_entry_bytes(
    reader: &ZipFileReader,
    entry_path: &str,
) -> Result<Option<Vec<u8>>, ThumbError> {
    let Some(index) = find_entry(reader, entry_path) else {
        return Ok(None);
    };
    let entry = &reader.file().entries()[index];
    if entry.uncompressed_size() > MAX_EPUB_ENTRY_BYTES {
        return Ok(None);
    }
    let mut entry_reader = reader
        .reader_without_entry(index)
        .await
        .map_err(|e| ThumbError::Epub(e.to_string()))?;
    let mut bytes = Vec::with_capacity(entry.uncompressed_size().min(1024 * 1024) as usize);
    entry_reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| ThumbError::Epub(e.to_string()))?;
    Ok(Some(bytes))
}

fn find_entry(reader: &ZipFileReader, entry_path: &str) -> Option<usize> {
    let normalized = entry_path.trim_start_matches('/').replace('\\', "/");
    reader.file().entries().iter().position(|entry| {
        entry
            .filename()
            .as_str()
            .map(|name| name.trim_start_matches('/').replace('\\', "/") == normalized)
            .unwrap_or(false)
    })
}

pub fn rootfile_path(container_xml: &str) -> Option<String> {
    let rootfile_start = container_xml.find("<rootfile")?;
    let rootfile = &container_xml[rootfile_start..];
    attr_value(rootfile, "full-path")
}

pub fn cover_href(opf: &str) -> Option<String> {
    if let Some(cover_id) = meta_cover_id(opf)
        && let Some(href) = manifest_href_for_id(opf, &cover_id)
    {
        return Some(href);
    }

    manifest_href_with_property(opf, "cover-image").or_else(|| {
        manifest_image_hrefs(opf)
            .into_iter()
            .find(|href| href.to_lowercase().contains("cover"))
    })
}

fn meta_cover_id(opf: &str) -> Option<String> {
    let mut rest = opf;
    while let Some(index) = rest.find("<meta") {
        rest = &rest[index + 5..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if attr_value(tag, "name")
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("cover"))
        {
            return attr_value(tag, "content");
        }
        rest = &rest[end..];
    }
    None
}

fn manifest_href_for_id(opf: &str, id: &str) -> Option<String> {
    manifest_items(opf).into_iter().find_map(|item| {
        if attr_value(&item, "id").as_deref() == Some(id) {
            attr_value(&item, "href")
        } else {
            None
        }
    })
}

fn manifest_href_with_property(opf: &str, property: &str) -> Option<String> {
    manifest_items(opf).into_iter().find_map(|item| {
        let properties = attr_value(&item, "properties").unwrap_or_default();
        if properties.split_whitespace().any(|p| p == property) {
            attr_value(&item, "href")
        } else {
            None
        }
    })
}

fn manifest_image_hrefs(opf: &str) -> Vec<String> {
    manifest_items(opf)
        .into_iter()
        .filter(|item| {
            attr_value(item, "media-type")
                .is_some_and(|media_type| media_type.starts_with("image/"))
        })
        .filter_map(|item| attr_value(&item, "href"))
        .collect()
}

fn manifest_items(opf: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = opf;
    while let Some(index) = rest.find("<item") {
        rest = &rest[index + 5..];
        let end = rest.find('>').unwrap_or(rest.len());
        items.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    items
}

fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=");
    let start = tag.find(&needle)? + needle.len();
    let quote = tag[start..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value_start = start + quote.len_utf8();
    let end = tag[value_start..].find(quote)? + value_start;
    Some(html_unescape(&tag[value_start..end]))
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn resolve_epub_path(opf_path: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let base = Path::new(opf_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut path = PathBuf::from(base);
    for component in href.replace('\\', "/").split('/') {
        match component {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            part => path.push(part),
        }
    }
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_container_rootfile() {
        let xml = r#"<container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#;
        assert_eq!(rootfile_path(xml), Some("OPS/package.opf".to_string()));
    }

    #[test]
    fn finds_cover_by_meta_id() {
        let opf = r#"
            <metadata><meta name="cover" content="cover-id"/></metadata>
            <manifest><item id="cover-id" href="images/cover.jpg" media-type="image/jpeg"/></manifest>
        "#;
        assert_eq!(cover_href(opf), Some("images/cover.jpg".to_string()));
    }

    #[test]
    fn resolves_cover_relative_to_opf() {
        assert_eq!(
            resolve_epub_path("OPS/package.opf", "images/cover.jpg"),
            "OPS/images/cover.jpg"
        );
    }
}
