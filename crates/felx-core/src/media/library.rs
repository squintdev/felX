//! Path-keyed runtime metadata cache for media assets.
//!
//! `AssetLibrary` is the runtime companion to the project file's path-only
//! [`crate::model::Asset`] references. It does not persist; it caches
//! filesystem and (eventually) media-probe metadata so the UI can show
//! asset info without re-statting on every render or paint.
//!
//! Per F-013 this will gain `MediaInfo` (codec / duration / dimensions /
//! sample rate / channels) populated by the ffmpeg probe path. For now only
//! filesystem metadata (size, mtime) is captured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetMetadata {
    pub size_bytes: u64,
    pub modified_at: SystemTime,
}

#[derive(Debug)]
pub enum AssetError {
    NotFound(PathBuf),
    NotAFile(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetError::NotFound(p) => write!(f, "asset not found: {}", p.display()),
            AssetError::NotAFile(p) => write!(f, "asset is not a regular file: {}", p.display()),
            AssetError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AssetError::Io(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct AssetLibrary {
    entries: HashMap<PathBuf, AssetMetadata>,
}

impl AssetLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read filesystem metadata for `path` and store it in the cache.
    /// Replaces any existing entry for the same path.
    ///
    /// Returns the freshly cached metadata. Errors do not poison the cache.
    pub fn refresh(&mut self, path: impl AsRef<Path>) -> Result<&AssetMetadata, AssetError> {
        let path = path.as_ref();
        let canonical = path.to_path_buf();

        let meta = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AssetError::NotFound(canonical));
            }
            Err(e) => return Err(AssetError::Io(e)),
        };

        if !meta.is_file() {
            return Err(AssetError::NotAFile(canonical));
        }

        let modified_at = meta.modified().map_err(AssetError::Io)?;
        let entry = AssetMetadata {
            size_bytes: meta.len(),
            modified_at,
        };
        self.entries.insert(canonical.clone(), entry);
        Ok(self.entries.get(&canonical).expect("just inserted"))
    }

    /// Look up cached metadata. Does not touch the filesystem.
    pub fn metadata(&self, path: impl AsRef<Path>) -> Option<&AssetMetadata> {
        self.entries.get(path.as_ref())
    }

    /// Remove a single path's cache entry.
    pub fn invalidate(&mut self, path: impl AsRef<Path>) {
        self.entries.remove(path.as_ref());
    }

    /// Drop every cached entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("felx-asset-lib-{pid}-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn refresh_populates_metadata() {
        let dir = scratch_dir();
        let path = dir.join("clip.mp4");
        std::fs::write(&path, b"hello world").unwrap();

        let mut lib = AssetLibrary::new();
        let meta = lib.refresh(&path).unwrap().clone();
        assert_eq!(meta.size_bytes, 11);
        assert!(lib.metadata(&path).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_missing_file_returns_not_found() {
        let mut lib = AssetLibrary::new();
        let err = lib.refresh("/no/such/path/xyz.mp4").unwrap_err();
        assert!(matches!(err, AssetError::NotFound(_)));
    }

    #[test]
    fn refresh_directory_returns_not_a_file() {
        let dir = scratch_dir();
        let mut lib = AssetLibrary::new();
        let err = lib.refresh(&dir).unwrap_err();
        assert!(matches!(err, AssetError::NotAFile(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_is_none_for_unknown_paths() {
        let lib = AssetLibrary::new();
        assert!(lib.metadata("/no/such/file").is_none());
    }

    #[test]
    fn adding_a_second_asset_does_not_invalidate_the_first() {
        let dir = scratch_dir();
        let a = dir.join("a.mp4");
        let b = dir.join("b.mp4");
        std::fs::write(&a, b"AAAA").unwrap();
        std::fs::write(&b, b"BBBBBBBB").unwrap();

        let mut lib = AssetLibrary::new();
        let m_a_initial = lib.refresh(&a).unwrap().clone();
        // Add a second file; the first must still be present and unchanged.
        let _ = lib.refresh(&b).unwrap();
        let m_a_after = lib.metadata(&a).unwrap().clone();
        assert_eq!(m_a_initial, m_a_after);
        assert_eq!(lib.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalidate_removes_a_single_entry() {
        let dir = scratch_dir();
        let a = dir.join("a.mp4");
        let b = dir.join("b.mp4");
        std::fs::write(&a, b"A").unwrap();
        std::fs::write(&b, b"B").unwrap();

        let mut lib = AssetLibrary::new();
        lib.refresh(&a).unwrap();
        lib.refresh(&b).unwrap();
        assert_eq!(lib.len(), 2);

        lib.invalidate(&a);
        assert!(lib.metadata(&a).is_none());
        assert!(lib.metadata(&b).is_some());
        assert_eq!(lib.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_replaces_stale_metadata() {
        let dir = scratch_dir();
        let path = dir.join("file.bin");
        std::fs::write(&path, b"short").unwrap();

        let mut lib = AssetLibrary::new();
        let m1 = lib.refresh(&path).unwrap().clone();

        // Sleep a tick so mtime can change; then rewrite with a longer body.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, b"a much longer body of bytes").unwrap();
        let m2 = lib.refresh(&path).unwrap().clone();

        assert!(m2.size_bytes > m1.size_bytes);
        assert_eq!(lib.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
