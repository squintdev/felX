//! Manifest registry. Loads each effect's `manifest.ron` at app startup so
//! the UI can auto-generate parameter panels and the project file's effect
//! values can be defaulted from the manifest.

use felx_core::params::{EffectManifest, ManifestError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Effects shipped with the binary. As the catalog grows this list grows;
/// later we'll discover effects/<*>/manifest.ron at runtime.
const BUILTIN_EFFECTS: &[&str] = &[
    "gain",
    "invert",
    "cc_toner",
    "signal",
    "squint_diffusion",
    "crt",
    "vhs",
    "crt_persistence",
];

#[derive(Debug, Default)]
pub struct ManifestRegistry {
    by_id: HashMap<String, EffectManifest>,
}

impl ManifestRegistry {
    pub fn load_builtins() -> Self {
        let root = effects_root();
        let mut by_id: HashMap<String, EffectManifest> = HashMap::new();
        for id in BUILTIN_EFFECTS {
            let path = root.join(id).join("manifest.ron");
            match EffectManifest::load(&path) {
                Ok(m) => {
                    info!(id = %m.id, path = %path.display(), "loaded effect manifest");
                    by_id.insert(m.id.clone(), m);
                }
                Err(ManifestError::Io(e)) => {
                    warn!(path = %path.display(), error = %e, "manifest io error — skipping");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "manifest parse error — skipping");
                }
            }
        }
        Self { by_id }
    }

    pub fn get(&self, id: &str) -> Option<&EffectManifest> {
        self.by_id.get(id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }
}

fn effects_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the felx-app crate dir; effects live two levels
    // up at the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("effects")
}
