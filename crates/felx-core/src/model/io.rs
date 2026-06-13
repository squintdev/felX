//! Project file I/O. RON format on disk; the file extension convention is
//! `.felx`. Asset paths are stored relative to the project file's parent
//! directory when possible, and resolved back to absolute on load.

use crate::model::{FORMAT_VERSION, Project};
use std::path::Path;

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Parse(ron::de::SpannedError),
    UnsupportedFormatVersion { found: u32, max_supported: u32 },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "io: {e}"),
            LoadError::Parse(e) => write!(f, "parse: {e}"),
            LoadError::UnsupportedFormatVersion {
                found,
                max_supported,
            } => write!(
                f,
                "unsupported project format version {found} (max supported: {max_supported})"
            ),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io(e) => Some(e),
            LoadError::Parse(e) => Some(e),
            LoadError::UnsupportedFormatVersion { .. } => None,
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        LoadError::Io(e)
    }
}

impl From<ron::de::SpannedError> for LoadError {
    fn from(e: ron::de::SpannedError) -> Self {
        LoadError::Parse(e)
    }
}

#[derive(Debug)]
pub enum SaveError {
    Io(std::io::Error),
    Serialize(ron::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "io: {e}"),
            SaveError::Serialize(e) => write!(f, "serialize: {e}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SaveError::Io(e) => Some(e),
            SaveError::Serialize(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

impl From<ron::Error> for SaveError {
    fn from(e: ron::Error) -> Self {
        SaveError::Serialize(e)
    }
}

impl Project {
    /// Serialize to RON and write to `path`. Asset paths under the project
    /// file's parent directory are stored relative to it; assets outside
    /// the parent stay absolute. The in-memory `self` is not mutated.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), SaveError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or(Path::new(""));
        let mut to_save = self.clone();
        for asset in &mut to_save.assets {
            if let Ok(rel) = asset.path.strip_prefix(parent) {
                asset.path = rel.to_path_buf();
            }
        }
        let pretty = ron::ser::to_string_pretty(
            &to_save,
            ron::ser::PrettyConfig::default().struct_names(true),
        )?;
        std::fs::write(path, pretty)?;
        tracing::debug!(path = %path.display(), "project saved");
        Ok(())
    }

    /// Read a project file from `path`. Asset paths in the file are
    /// resolved relative to the file's parent directory.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or(Path::new(""));
        let text = std::fs::read_to_string(path)?;
        let mut p: Project = ron::from_str(&text)?;

        if p.format_version > FORMAT_VERSION {
            return Err(LoadError::UnsupportedFormatVersion {
                found: p.format_version,
                max_supported: FORMAT_VERSION,
            });
        }

        for asset in &mut p.assets {
            if asset.path.is_relative() && !parent.as_os_str().is_empty() {
                asset.path = parent.join(&asset.path);
            }
        }

        p.fixup_after_load();

        tracing::debug!(path = %path.display(), "project loaded");
        Ok(p)
    }
}
