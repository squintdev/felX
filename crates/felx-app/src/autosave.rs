//! Auto-save (F-113).
//!
//! Every N minutes (default 5) the app writes the current project to a
//! `.felx.autosave` file alongside the project's path. On startup, if an
//! autosave is newer than the main file, the user is prompted to recover.
//!
//! Public-API surface; the host wires the timer + recovery prompt into
//! its update loop. Marked `allow(dead_code)` until the file-open UI
//! lands and gives autosave somewhere to actually run.

#![allow(dead_code)]

use felx_core::model::Project;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct AutoSave {
    pub interval: Duration,
    last_save: Option<Instant>,
}

impl AutoSave {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_save: None,
        }
    }

    pub fn default_interval() -> Self {
        Self::new(Duration::from_secs(5 * 60))
    }

    /// Should the host save now? Updates `last_save` if returning true.
    pub fn should_save_now(&mut self) -> bool {
        let now = Instant::now();
        let due = match self.last_save {
            None => true,
            Some(t) => now.duration_since(t) >= self.interval,
        };
        if due {
            self.last_save = Some(now);
        }
        due
    }
}

/// Autosave file path for `project_path` — same dir, suffix `.autosave`.
pub fn autosave_path(project_path: &Path) -> PathBuf {
    let mut p = project_path.to_path_buf();
    let mut name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project.felx".into());
    name.push_str(".autosave");
    p.set_file_name(name);
    p
}

/// Recovery decision returned at startup. The host shows the prompt UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryStatus {
    /// No autosave found; nothing to recover.
    None,
    /// Autosave exists but the project file is newer — autosave is stale.
    Stale,
    /// Autosave is newer than the project file; offer recovery.
    Available,
}

pub fn check_recovery(project_path: &Path) -> RecoveryStatus {
    let autosave = autosave_path(project_path);
    if !autosave.exists() {
        return RecoveryStatus::None;
    }
    let project_mtime = std::fs::metadata(project_path)
        .and_then(|m| m.modified())
        .ok();
    let autosave_mtime = std::fs::metadata(&autosave).and_then(|m| m.modified()).ok();
    match (project_mtime, autosave_mtime) {
        (Some(p), Some(a)) if a > p => RecoveryStatus::Available,
        (None, Some(_)) => RecoveryStatus::Available,
        _ => RecoveryStatus::Stale,
    }
}

/// Write the project to its autosave path. The autosave file is excluded
/// from project loads — it's a recovery snapshot, not a primary file.
pub fn write_autosave(project: &Project, project_path: &Path) -> Result<(), String> {
    let target = autosave_path(project_path);
    project
        .save(&target)
        .map_err(|e| format!("autosave write {}: {e}", target.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_path_appends_suffix() {
        let p = autosave_path(Path::new("/tmp/proj.felx"));
        assert_eq!(p, PathBuf::from("/tmp/proj.felx.autosave"));
    }

    #[test]
    fn should_save_first_call_returns_true() {
        let mut s = AutoSave::new(Duration::from_secs(60));
        assert!(s.should_save_now());
        assert!(!s.should_save_now()); // too soon
    }

    #[test]
    fn check_recovery_no_file_returns_none() {
        let p = std::env::temp_dir().join(format!("felx-no-such-{}.felx", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(autosave_path(&p));
        assert_eq!(check_recovery(&p), RecoveryStatus::None);
    }
}
