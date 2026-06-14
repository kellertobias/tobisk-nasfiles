use std::path::{Path, PathBuf};

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use axum::body::Body;
use axum::response::IntoResponse;

use tokio_util::compat::TokioAsyncWriteCompatExt;

/// Collect all files to include in the ZIP from a list of paths.
/// Each path can be a file or directory (walked recursively).
/// Returns a list of (archive_path, fs_path) tuples.
fn collect_entries(paths: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut entries = Vec::new();

    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_str().unwrap_or("unknown").to_string();
                let archive_path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };

                let meta = std::fs::symlink_metadata(&path);
                if let Ok(m) = meta {
                    if m.is_symlink() {
                        tracing::warn!("Skipping symlink in zip: {:?}", path);
                        continue;
                    }
                    if m.is_dir() {
                        walk(&path, &archive_path, out);
                    } else {
                        out.push((archive_path, path));
                    }
                }
            }
        }
    }

    for path in paths {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download")
            .to_string();

        if let Ok(m) = std::fs::symlink_metadata(path) {
            if m.is_symlink() {
                tracing::warn!("Skipping symlink in zip: {:?}", path);
                continue;
            }
            if m.is_dir() {
                walk(path, &name, &mut entries);
            } else {
                entries.push((name, path.clone()));
            }
        }
    }

    entries
}

/// Stream a ZIP archive as an HTTP response.
/// The archive is generated on-the-fly — never buffered fully in memory.
pub async fn stream_zip(
    paths: Vec<PathBuf>,
    archive_name: &str,
) -> Result<impl IntoResponse + use<>, axum::response::Response> {
    use tokio::io::duplex;
    use tokio_util::io::ReaderStream;

    let entries = collect_entries(&paths);

    if entries.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "nothing to download"})),
        )
            .into_response());
    }

    // Create a duplex channel — writer produces ZIP bytes, reader feeds them to the response body
    let (writer, reader) = duplex(64 * 1024); // 64KB buffer

    let file_name_header = format!(
        "attachment; filename=\"{}\"",
        super::sanitize_header_filename(archive_name)
    );

    // Spawn ZIP writer task
    tokio::spawn(async move {
        // async_zip requires futures_lite::AsyncWrite, so wrap with compat
        let compat_writer = writer.compat_write();
        let mut zip = ZipFileWriter::new(compat_writer);

        for (archive_path, fs_path) in entries {
            let metadata = match tokio::fs::metadata(&fs_path).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("skip zip entry {:?}: {e}", fs_path);
                    continue;
                }
            };

            // Skip very large files (> 4GB) to avoid ZIP32 limits
            if metadata.len() > 4_000_000_000 {
                tracing::warn!(
                    "skip zip entry {:?}: too large ({})",
                    fs_path,
                    metadata.len()
                );
                continue;
            }

            // Choose compression — skip for already-compressed formats
            let ext = fs_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let compression = if matches!(
                ext.as_str(),
                "zip"
                    | "gz"
                    | "bz2"
                    | "xz"
                    | "zst"
                    | "jpg"
                    | "jpeg"
                    | "png"
                    | "gif"
                    | "webp"
                    | "mp4"
                    | "mkv"
                    | "avi"
                    | "mov"
                    | "mp3"
                    | "ogg"
                    | "flac"
                    | "aac"
            ) {
                Compression::Stored
            } else {
                Compression::Deflate
            };

            let entry_builder = ZipEntryBuilder::new(archive_path.into(), compression);

            // Read file and write to ZIP
            match tokio::fs::File::open(&fs_path).await {
                Ok(mut file) => {
                    let entry_writer = match zip.write_entry_stream(entry_builder).await {
                        Ok(w) => w,
                        Err(e) => {
                            tracing::warn!("zip write stream error for {:?}: {e}", fs_path);
                            break;
                        }
                    };

                    use tokio_util::compat::FuturesAsyncWriteCompatExt;
                    let mut tokio_writer = entry_writer.compat_write();

                    if let Err(e) = tokio::io::copy(&mut file, &mut tokio_writer).await {
                        tracing::warn!("zip copy error for {:?}: {e}", fs_path);
                        break;
                    }

                    let entry_writer = tokio_writer.into_inner();
                    if let Err(e) = entry_writer.close().await {
                        tracing::warn!("zip entry close error for {:?}: {e}", fs_path);
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("open error for zip entry {:?}: {e}", fs_path);
                }
            }
        }

        if let Err(e) = zip.close().await {
            tracing::warn!("zip close error: {e}");
        }
    });

    let stream = ReaderStream::new(reader);
    let body = Body::from_stream(stream);

    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_string(),
            ),
            (axum::http::header::CONTENT_DISPOSITION, file_name_header),
        ],
        body,
    )
        .into_response())
}
