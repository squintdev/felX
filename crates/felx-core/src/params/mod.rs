//! Effect parameter manifests. Each effect ships a `manifest.ron` declaring
//! its parameters; the runtime parses it into [`EffectManifest`] for the UI
//! generator (F-026), the project-file value tree (F-030), and the
//! compositor's per-frame uniform buffers.
//!
//! Parameter IDs are stable across UI renames: changing `display_name` does
//! not break projects that reference `id`.

mod manifest;
mod values;

pub use manifest::*;
pub use values::*;
