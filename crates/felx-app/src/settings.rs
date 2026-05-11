//! Persistent app settings — currently just GPU selection per purpose
//! (viewer / export). Lives at `~/.felx/settings.ron`.
//!
//! Resolution order at startup:
//!   1. `FELX_GPU` / `FELX_BACKEND` env vars — explicit override
//!   2. Saved `Settings::viewer_gpu` / `export_gpu` (substring match)
//!   3. Built-in default (prefer DiscreteGpu on a non-GL backend)
//!
//! Viewer-GPU changes only take effect on next launch (eframe builds
//! the wgpu device at `run_native` time). Export-GPU changes apply
//! immediately to the next export.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Substring match on adapter name. `None` = automatic.
    #[serde(default)]
    pub viewer_gpu: Option<String>,
    #[serde(default)]
    pub export_gpu: Option<String>,
}

impl Settings {
    /// Load from `~/.felx/settings.ron`. Returns `Default` on any error
    /// (file missing, parse failure, no home dir) — silent so a fresh
    /// install works without forcing the user through a setup screen.
    pub fn load() -> Self {
        let path = settings_path();
        let bytes = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        ron::from_str(&bytes).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let s = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| format!("serialize: {e}"))?;
        std::fs::write(&path, s).map_err(|e| format!("write: {e}"))?;
        Ok(())
    }
}

fn settings_path() -> PathBuf {
    home_dir().join(".felx").join("settings.ron")
}

fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h);
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        return PathBuf::from(p);
    }
    std::env::temp_dir()
}

/// One adapter the GUI knows about — built once at startup, used to
/// populate the Settings dialog dropdowns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterChoice {
    pub name: String,
    pub backend: String,
    pub device_type: String,
}

impl AdapterChoice {
    pub fn label(&self) -> String {
        format!(
            "{}  •  {}  •  {}",
            self.name, self.backend, self.device_type
        )
    }
}

pub fn list_adapters() -> Vec<AdapterChoice> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    instance
        .enumerate_adapters(wgpu::Backends::all())
        .into_iter()
        .map(|a| {
            let info = a.get_info();
            AdapterChoice {
                name: info.name,
                backend: format!("{:?}", info.backend),
                device_type: format!("{:?}", info.device_type),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_ron() {
        let s = Settings {
            viewer_gpu: Some("nvidia".into()),
            export_gpu: None,
        };
        let text = ron::ser::to_string_pretty(&s, ron::ser::PrettyConfig::default()).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn settings_default_when_file_missing_is_silent() {
        // Unset HOME so settings_path lands in temp dir under a likely-
        // nonexistent name; load() should return Default without panic.
        let saved = std::env::var("HOME").ok();
        unsafe {
            std::env::remove_var("HOME");
            std::env::set_var(
                "HOME",
                std::env::temp_dir().join(format!("felx-no-such-{}", std::process::id())),
            );
        }
        let loaded = Settings::load();
        assert_eq!(loaded, Settings::default());
        if let Some(h) = saved {
            unsafe {
                std::env::set_var("HOME", h);
            }
        }
    }
}
