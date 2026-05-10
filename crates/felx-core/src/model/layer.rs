//! Layer types and the per-kind data they carry.

use crate::model::{AssetId, CompId, Effect, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LayerId(pub u32);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    /// First frame on the parent comp's timeline at which this layer is
    /// visible.
    pub in_frame: u32,
    /// One past the last frame. `out_frame == in_frame` means a zero-length
    /// layer, which is valid (just invisible).
    pub out_frame: u32,
    pub transform: Transform,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayerKind {
    Video { asset: AssetId },
    Image { asset: AssetId },
    Audio { asset: AssetId },
    Solid { color: [f32; 4] },
    Null,
    Adjustment,
    Composition { comp: CompId },
}

impl Layer {
    /// Duration on the parent comp's timeline, in frames.
    pub fn duration(&self) -> u32 {
        self.out_frame.saturating_sub(self.in_frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_duration_is_out_minus_in() {
        let l = Layer {
            id: LayerId(1),
            name: "test".into(),
            kind: LayerKind::Solid {
                color: [0.0, 0.0, 0.0, 1.0],
            },
            in_frame: 30,
            out_frame: 90,
            transform: Transform::default(),
            effects: vec![],
        };
        assert_eq!(l.duration(), 60);
    }

    #[test]
    fn zero_length_layer_is_valid() {
        let l = Layer {
            id: LayerId(1),
            name: "marker".into(),
            kind: LayerKind::Null,
            in_frame: 30,
            out_frame: 30,
            transform: Transform::default(),
            effects: vec![],
        };
        assert_eq!(l.duration(), 0);
    }
}
