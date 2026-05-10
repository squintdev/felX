//! Layer types and the per-kind data they carry.

use crate::model::{AssetId, CompId, Effect, Mask, Transform};

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
    /// Use the layer immediately above this one in the stack as a track
    /// matte (gating alpha source). The source layer becomes invisible.
    #[serde(default)]
    pub track_matte: Option<TrackMatteMode>,
    /// Constant time offset applied to the source clock. Affects only
    /// time-driven layer kinds (Composition, Video). +N → source plays N
    /// frames later relative to the layer's `in_frame`.
    #[serde(default)]
    pub time_offset_frames: i32,
    /// Constant speed multiplier on the source clock. 1.0 is identity,
    /// 0.5 plays at half speed, 2.0 at double speed, -1.0 reverses. Full
    /// keyframed time-remap (per-frame curve) is post-MVP.
    #[serde(default = "default_time_scale")]
    pub time_scale: f32,
    /// Per-layer bezier-path masks (F-061). Combined left-to-right via
    /// each mask's [`MaskMode`].
    #[serde(default)]
    pub masks: Vec<Mask>,
}

fn default_time_scale() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackMatteMode {
    Alpha,
    AlphaInverted,
    Luma,
    LumaInverted,
}

impl TrackMatteMode {
    pub fn shader_index(self) -> u32 {
        match self {
            TrackMatteMode::Alpha => 0,
            TrackMatteMode::AlphaInverted => 1,
            TrackMatteMode::Luma => 2,
            TrackMatteMode::LumaInverted => 3,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TrackMatteMode::Alpha => "Alpha",
            TrackMatteMode::AlphaInverted => "Alpha Inverted",
            TrackMatteMode::Luma => "Luma",
            TrackMatteMode::LumaInverted => "Luma Inverted",
        }
    }
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

    /// Map a comp-timeline frame to the source clock for this layer.
    /// Returns 0 when the layer has identity time-remap (the common case).
    /// Negative results are clamped to 0 — the source can't play "before
    /// it starts".
    pub fn source_frame_for(&self, comp_frame: u32) -> u32 {
        if self.time_offset_frames == 0 && (self.time_scale - 1.0).abs() < f32::EPSILON {
            return comp_frame.saturating_sub(self.in_frame);
        }
        let local = comp_frame as i64 - self.in_frame as i64;
        let scaled = (local as f64 * self.time_scale as f64).round() as i64;
        let with_offset = scaled + self.time_offset_frames as i64;
        if with_offset <= 0 {
            0
        } else {
            with_offset as u32
        }
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
            track_matte: None,
            time_offset_frames: 0,
            time_scale: 1.0,
            masks: vec![],
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
            track_matte: None,
            time_offset_frames: 0,
            time_scale: 1.0,
            masks: vec![],
        };
        assert_eq!(l.duration(), 0);
    }

    fn layer_with(in_f: u32, out_f: u32, offset: i32, scale: f32) -> Layer {
        Layer {
            id: LayerId(1),
            name: "x".into(),
            kind: LayerKind::Null,
            in_frame: in_f,
            out_frame: out_f,
            transform: Transform::default(),
            effects: vec![],
            parent: None,
            blend_mode: BlendMode::default(),
            track_matte: None,
            time_offset_frames: offset,
            time_scale: scale,
            masks: vec![],
        }
    }

    #[test]
    fn source_frame_identity_subtracts_in_frame() {
        let l = layer_with(10, 100, 0, 1.0);
        assert_eq!(l.source_frame_for(10), 0);
        assert_eq!(l.source_frame_for(50), 40);
        // Before in_frame: clamp to 0 (the layer isn't visible there anyway,
        // but the function shouldn't underflow).
        assert_eq!(l.source_frame_for(5), 0);
    }

    #[test]
    fn source_frame_with_positive_offset() {
        let l = layer_with(0, 100, 5, 1.0);
        // comp 0 → local 0 → scaled 0 → +5 = 5
        assert_eq!(l.source_frame_for(0), 5);
        // comp 10 → local 10 → +5 = 15
        assert_eq!(l.source_frame_for(10), 15);
    }

    #[test]
    fn source_frame_with_half_speed() {
        let l = layer_with(0, 100, 0, 0.5);
        // comp 0 → 0; comp 10 → 5; comp 100 → 50
        assert_eq!(l.source_frame_for(0), 0);
        assert_eq!(l.source_frame_for(10), 5);
        assert_eq!(l.source_frame_for(100), 50);
    }

    #[test]
    fn source_frame_with_double_speed() {
        let l = layer_with(0, 100, 0, 2.0);
        assert_eq!(l.source_frame_for(0), 0);
        assert_eq!(l.source_frame_for(10), 20);
        assert_eq!(l.source_frame_for(50), 100);
    }

    #[test]
    fn source_frame_with_negative_scale_reverses() {
        // Reverse playback: scale = -1, offset = 60 means comp frame 0
        // shows source frame 60, comp frame 60 shows source frame 0.
        let l = layer_with(0, 60, 60, -1.0);
        assert_eq!(l.source_frame_for(0), 60);
        assert_eq!(l.source_frame_for(30), 30);
        assert_eq!(l.source_frame_for(60), 0);
    }
}
