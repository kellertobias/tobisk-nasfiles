use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cache::{ThumbError, ThumbFormat};

#[cfg(not(target_os = "macos"))]
const XATTR_PREFIX_LINUX: &str = "user.nasfiles.thumb";
#[cfg(target_os = "macos")]
const XATTR_PREFIX_MACOS: &str = "com.nasfiles.thumb";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThumbnailMeta {
    pub version: u32,
    pub key: String,
    pub format: String,
    pub width: u32,
    pub source_mtime_ms: i64,
    pub source_size: u64,
}

pub struct ThumbnailStorage {
    cache_dir: PathBuf,
}

impl ThumbnailStorage {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    pub async fn read(
        &self,
        source_path: &Path,
        key: &str,
        format: ThumbFormat,
        expected: &ThumbnailMeta,
    ) -> Result<Option<Vec<u8>>, ThumbError> {
        if let Some(bytes) = read_xattr(source_path, key, expected) {
            return Ok(Some(bytes));
        }

        let cache_path = self.cache_path(key, format);
        if !cache_path.exists() {
            return Ok(None);
        }

        let bytes = tokio::fs::read(&cache_path)
            .await
            .map_err(|e| ThumbError::Io(e.to_string()))?;
        Ok(Some(bytes))
    }

    pub async fn write(
        &self,
        source_path: &Path,
        key: &str,
        format: ThumbFormat,
        meta: &ThumbnailMeta,
        bytes: &[u8],
    ) -> Result<(), ThumbError> {
        if write_xattr(source_path, key, meta, bytes).is_ok() {
            return Ok(());
        }

        let cache_path = self.cache_path(key, format);
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ThumbError::Io(e.to_string()))?;
        }
        tokio::fs::write(&cache_path, bytes)
            .await
            .map_err(|e| ThumbError::Io(e.to_string()))
    }

    fn cache_path(&self, key: &str, format: ThumbFormat) -> PathBuf {
        let a = &key[0..2];
        let b = &key[2..4];
        self.cache_dir
            .join(a)
            .join(b)
            .join(format!("{}.{}", key, format.extension()))
    }
}

fn xattr_prefix() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        XATTR_PREFIX_MACOS
    }
    #[cfg(not(target_os = "macos"))]
    {
        XATTR_PREFIX_LINUX
    }
}

fn xattr_names(key: &str) -> (String, String) {
    let short_key = &key[..key.len().min(24)];
    (
        format!("{}.{}.meta", xattr_prefix(), short_key),
        format!("{}.{}.bytes", xattr_prefix(), short_key),
    )
}

fn read_xattr(source_path: &Path, key: &str, expected: &ThumbnailMeta) -> Option<Vec<u8>> {
    let (meta_name, bytes_name) = xattr_names(key);
    let meta_bytes = platform_xattr::get(source_path, &meta_name).ok()??;
    let meta = serde_json::from_slice::<ThumbnailMeta>(&meta_bytes).ok()?;
    if &meta != expected {
        return None;
    }
    platform_xattr::get(source_path, &bytes_name).ok()?
}

fn write_xattr(
    source_path: &Path,
    key: &str,
    meta: &ThumbnailMeta,
    bytes: &[u8],
) -> std::io::Result<()> {
    let (meta_name, bytes_name) = xattr_names(key);
    let meta_bytes = serde_json::to_vec(meta).map_err(std::io::Error::other)?;
    platform_xattr::set(source_path, &meta_name, &meta_bytes)?;
    platform_xattr::set(source_path, &bytes_name, bytes)
}

#[cfg(unix)]
mod platform_xattr {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub fn get(path: &Path, name: &str) -> io::Result<Option<Vec<u8>>> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let name = CString::new(name).map_err(io::Error::other)?;

        #[cfg(target_os = "macos")]
        unsafe {
            let size = libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0);
            if size < 0 {
                return Ok(None);
            }
            let mut buf = vec![0_u8; size as usize];
            let read = libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                0,
            );
            if read < 0 {
                return Ok(None);
            }
            buf.truncate(read as usize);
            Ok(Some(buf))
        }

        #[cfg(not(target_os = "macos"))]
        unsafe {
            let size = libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0);
            if size < 0 {
                return Ok(None);
            }
            let mut buf = vec![0_u8; size as usize];
            let read = libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
            );
            if read < 0 {
                return Ok(None);
            }
            buf.truncate(read as usize);
            Ok(Some(buf))
        }
    }

    pub fn set(path: &Path, name: &str, value: &[u8]) -> io::Result<()> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let name = CString::new(name).map_err(io::Error::other)?;

        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };

        #[cfg(not(target_os = "macos"))]
        let result = unsafe {
            libc::setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };

        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(not(unix))]
mod platform_xattr {
    use std::io;
    use std::path::Path;

    pub fn get(_path: &Path, _name: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    pub fn set(_path: &Path, _name: &str, _value: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "xattrs are not supported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xattr_names_are_scoped_by_key() {
        let (meta, bytes) = xattr_names("abcdef0123456789abcdef012345");
        assert!(meta.contains("abcdef0123456789abcdef01"));
        assert!(bytes.ends_with(".bytes"));
    }
}
