//! Layer types and the per-kind data they carry.

use crate::model::{AssetId, CompId, Effect, Transform};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    HardLight,
    Lighten,
    Darken,
    Difference,
    Exclusion,
    ColorDodge,
    ColorBurn,
    LinearDodge,
    LinearBurn,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    pub const ALL: [BlendMode; 19] = [
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::Lighten,
        BlendMode::Darken,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::LinearDodge,
        BlendMode::LinearBurn,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    /// Stable index that the BlendPass shader switches on.
    pub fn shader_index(self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Add | BlendMode::LinearDodge => 1,
            BlendMode::Multiply => 2,
            BlendMode::Screen => 3,
            BlendMode::Overlay => 4,
            BlendMode::HardLight => 5,
            BlendMode::Lighten => 6,
            BlendMode::Darken => 7,
            BlendMode::Difference => 8,
            BlendMode::Exclusion => 9,
            BlendMode::ColorDodge => 10,
            BlendMode::ColorBurn => 11,
            BlendMode::LinearBurn => 12,
            // HSL modes and SoftLight fall through to Normal until their
            // shader implementations land in a polish pass.
            BlendMode::SoftLight
            | BlendMode::Hue
            | BlendMode::Saturation
            | BlendMode::Color
            | BlendMode::Luminosity => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Add => "Add",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::HardLight => "Hard Light",
            BlendMode::Lighten => "Lighten",
            BlendMode::Darken => "Darken",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::ColorBurn => "Color Burn",
            BlendMode::LinearDodge => "Linear Dodge",
            BlendMode::LinearBurn => "Linear Burn",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Luminosity => "Luminosity",
        }
    }
}

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
    /// Layer this one is parented to (transform-inherited from). Cycles are
    /// rejected by [`Project::validate`].
    #[serde(default)]
    pub parent: Option<LayerId>,
    /// How this layer composites onto everything below it.
    #[serde(default)]
    pub blend_mode: BlendMode,
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
            parent: None,
            blend_mode: BlendMode::default(),
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
            parent: None,
            blend_mode: BlendMode::default(),
        };
        assert_eq!(l.duration(), 0);
    }
}
