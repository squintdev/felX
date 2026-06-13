//! Preset registry. Loads `presets/*.ron` at app startup.

use felx_core::params::{EffectPreset, PresetError};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const BUILTIN_PRESETS: &[&str] = &[
    "crt_consumer_trinitron",
    "crt_arcade",
    "crt_pc_monitor",
    "vhs_on_crt",
];

#[derive(Debug, Default)]
pub struct PresetRegistry {
    presets: Vec<EffectPreset>,
}

impl PresetRegistry {
    pub fn load_builtins() -> Self {
        let root = presets_root_dir();
        let mut presets = Vec::new();
        for slug in BUILTIN_PRESETS {
            let path = root.join(format!("{slug}.ron"));
            match EffectPreset::load(&path) {
                Ok(p) => {
                    info!(name = %p.name, path = %path.display(), "loaded preset");
                    presets.push(p);
                }
                Err(PresetError::Io(e)) => {
                    warn!(path = %path.display(), error = %e, "preset io error");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "preset parse error");
                }
            }
        }
        Self { presets }
    }

    pub fn iter(&self) -> impl Iterator<Item = &EffectPreset> {
        self.presets.iter()
    }

    /// Currently unused — kept for the upcoming preset-management UI work
    /// that surfaces a "saved presets" panel with counts.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.presets.len()
    }
}

fn presets_root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("presets")
}
