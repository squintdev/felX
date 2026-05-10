//! Compositions hold an ordered list of layers and define the output canvas
//! dimensions, framerate, duration, and background color.

use crate::model::{AssetId, Effect, Framerate, Layer, LayerId, LayerKind, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CompId(pub u32);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Composition {
    pub id: CompId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub framerate: Framerate,
    /// Duration in frames at the comp's framerate.
    pub duration_frames: u32,
    /// Background color shown where no layer is opaque. RGBA, linear-light.
    pub background: [f32; 4],
    pub layers: Vec<Layer>,
    #[serde(default)]
    next_layer_id: u32,
}

impl Composition {
    pub fn new(id: CompId, name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            id,
            name: name.into(),
            width,
            height,
            framerate: Framerate::default(),
            duration_frames: 0,
            background: [0.0, 0.0, 0.0, 1.0],
            layers: Vec::new(),
            next_layer_id: 1,
        }
    }

    /// Add a layer. Returns its assigned [`LayerId`].
    pub fn add_layer(
        &mut self,
        name: impl Into<String>,
        kind: LayerKind,
        in_frame: u32,
        out_frame: u32,
    ) -> LayerId {
        let id = LayerId(self.next_layer_id);
        self.next_layer_id += 1;
        self.layers.push(Layer {
            id,
            name: name.into(),
            kind,
            in_frame,
            out_frame,
            transform: Transform::default(),
            effects: Vec::new(),
            parent: None,
            blend_mode: crate::model::BlendMode::default(),
        });
        id
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Convenience for tests / programmatic project construction: add a
    /// solid-color layer spanning the whole comp duration.
    pub fn add_solid(&mut self, name: impl Into<String>, color: [f32; 4]) -> LayerId {
        let dur = self.duration_frames;
        self.add_layer(name, LayerKind::Solid { color }, 0, dur)
    }

    /// Convenience: add a video layer referencing an asset over the full
    /// comp duration.
    pub fn add_video(&mut self, name: impl Into<String>, asset: AssetId) -> LayerId {
        let dur = self.duration_frames;
        self.add_layer(name, LayerKind::Video { asset }, 0, dur)
    }

    pub fn push_effect(&mut self, layer: LayerId, effect: Effect) {
        if let Some(l) = self.layer_mut(layer) {
            l.effects.push(effect);
        }
    }

    /// Remove a layer by id. Returns the removed layer if found.
    pub fn remove_layer(&mut self, id: LayerId) -> Option<Layer> {
        let idx = self.layers.iter().position(|l| l.id == id)?;
        Some(self.layers.remove(idx))
    }

    /// Move the layer with `id` up by one (toward index 0). No-op if it's
    /// already first or doesn't exist. Returns true if a swap happened.
    pub fn move_layer_up(&mut self, id: LayerId) -> bool {
        match self.layers.iter().position(|l| l.id == id) {
            Some(0) | None => false,
            Some(i) => {
                self.layers.swap(i - 1, i);
                true
            }
        }
    }

    /// Move the layer with `id` down by one (toward the end). No-op if it's
    /// already last or doesn't exist. Returns true if a swap happened.
    pub fn move_layer_down(&mut self, id: LayerId) -> bool {
        match self.layers.iter().position(|l| l.id == id) {
            Some(i) if i + 1 < self.layers.len() => {
                self.layers.swap(i, i + 1);
                true
            }
            _ => false,
        }
    }

    /// Recompute `next_layer_id` from existing layer IDs. Called after
    /// deserialize for files that didn't persist the allocator.
    pub fn fixup_after_load(&mut self) {
        if self.next_layer_id == 0 {
            self.next_layer_id = self.layers.iter().map(|l| l.id.0).max().unwrap_or(0) + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AssetId;

    #[test]
    fn add_layer_assigns_unique_ids() {
        let mut c = Composition::new(CompId(1), "main", 1920, 1080);
        c.duration_frames = 600;
        let a = c.add_layer("a", LayerKind::Null, 0, 100);
        let b = c.add_layer("b", LayerKind::Null, 0, 200);
        assert_ne!(a, b);
        assert_eq!(c.layers.len(), 2);
    }

    #[test]
    fn layer_lookup_by_id() {
        let mut c = Composition::new(CompId(1), "main", 100, 100);
        c.duration_frames = 60;
        let id = c.add_layer("solo", LayerKind::Null, 0, 60);
        assert!(c.layer(id).is_some());
        assert!(c.layer(LayerId(999)).is_none());
    }

    #[test]
    fn convenience_constructors() {
        let mut c = Composition::new(CompId(2), "main", 100, 100);
        c.duration_frames = 30;
        let _solid = c.add_solid("bg", [0.1, 0.1, 0.1, 1.0]);
        let _vid = c.add_video("clip", AssetId(7));
        assert_eq!(c.layers.len(), 2);
        assert_eq!(c.layers[0].in_frame, 0);
        assert_eq!(c.layers[0].out_frame, 30);
    }

    #[test]
    fn remove_layer_drops_only_that_layer() {
        let mut c = Composition::new(CompId(1), "main", 10, 10);
        c.duration_frames = 30;
        let a = c.add_layer("a", LayerKind::Null, 0, 30);
        let b = c.add_layer("b", LayerKind::Null, 0, 30);
        let removed = c.remove_layer(a).unwrap();
        assert_eq!(removed.id, a);
        assert_eq!(c.layers.len(), 1);
        assert_eq!(c.layers[0].id, b);
        assert!(c.remove_layer(LayerId(999)).is_none());
    }

    #[test]
    fn move_layer_up_swaps_with_predecessor() {
        let mut c = Composition::new(CompId(1), "main", 10, 10);
        c.duration_frames = 30;
        let a = c.add_layer("a", LayerKind::Null, 0, 30);
        let b = c.add_layer("b", LayerKind::Null, 0, 30);
        assert_eq!(c.layers[0].id, a);
        assert_eq!(c.layers[1].id, b);
        assert!(c.move_layer_up(b));
        assert_eq!(c.layers[0].id, b);
        assert_eq!(c.layers[1].id, a);
        // Moving the now-top layer up is a no-op.
        assert!(!c.move_layer_up(b));
    }

    #[test]
    fn move_layer_down_swaps_with_successor() {
        let mut c = Composition::new(CompId(1), "main", 10, 10);
        c.duration_frames = 30;
        let a = c.add_layer("a", LayerKind::Null, 0, 30);
        let b = c.add_layer("b", LayerKind::Null, 0, 30);
        assert!(c.move_layer_down(a));
        assert_eq!(c.layers[0].id, b);
        assert_eq!(c.layers[1].id, a);
        assert!(!c.move_layer_down(a));
    }
}
