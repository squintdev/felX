//! Filesystem watcher for `effects/<id>/effect.wgsl` changes.
//!
//! The watcher runs on a background thread (via `notify`) and forwards
//! debounced change events to the app over a `mpsc::channel`. The app
//! polls the channel each frame, attempts to recompile the affected
//! effect, and surfaces any compile error in a non-fatal overlay.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[derive(Clone, Debug)]
pub enum HotReloadEvent {
    /// `effects/<id>/effect.wgsl` was modified.
    WgslChanged { effect_id: String, path: PathBuf },
}

pub struct HotReloadWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<HotReloadEvent>,
}

impl HotReloadWatcher {
    pub fn new(effects_root: PathBuf) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut last_seen = std::collections::HashMap::<PathBuf, Instant>::new();
        let debounce = Duration::from_millis(150);

        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = ?e, "hot-reload watcher error");
                        return;
                    }
                };
                let modify = matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
                if !modify {
                    return;
                }
                for path in event.paths {
                    if path.extension().and_then(|s| s.to_str()) != Some("wgsl") {
                        continue;
                    }
                    let now = Instant::now();
                    if let Some(prev) = last_seen.get(&path)
                        && now.duration_since(*prev) < debounce
                    {
                        continue;
                    }
                    last_seen.insert(path.clone(), now);
                    if let Some(id) = effect_id_from_path(&path) {
                        let _ = tx.send(HotReloadEvent::WgslChanged {
                            effect_id: id,
                            path,
                        });
                    }
                }
            })?;
        watcher.watch(&effects_root, RecursiveMode::Recursive)?;
        info!(path = %effects_root.display(), "watching effects directory");
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Drain pending events without blocking.
    pub fn drain(&self) -> Vec<HotReloadEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

/// `.../effects/<id>/effect.wgsl` → `id`.
fn effect_id_from_path(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    parent
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_effect_id_from_typical_path() {
        let p = Path::new("effects/gain/effect.wgsl");
        assert_eq!(effect_id_from_path(p).as_deref(), Some("gain"));
    }

    #[test]
    fn handles_absolute_path() {
        let p = Path::new("/home/u/proj/effects/cc_toner/effect.wgsl");
        assert_eq!(effect_id_from_path(p).as_deref(), Some("cc_toner"));
    }

    #[test]
    fn no_parent_returns_none() {
        // bare filename has an empty parent → no enclosing directory to use
        // as the effect id.
        assert!(effect_id_from_path(Path::new("effect.wgsl")).is_none());
    }
}
