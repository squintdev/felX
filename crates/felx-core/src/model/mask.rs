//! Per-layer bezier-path masks (F-061 … F-066).
//!
//! A [`Mask`] is a closed bezier path that gates the layer's alpha. Multiple
//! masks combine via [`MaskMode`] (Add / Subtract / Intersect / Difference).
//!
//! v1 scope:
//! - Closed paths only. Open-path masks (strokes) are post-MVP.
//! - Vertex count is fixed across keyframes. Bezier interpolation between
//!   topologically different paths is post-MVP.
//! - Stored in layer-local UV-pixel coordinates relative to the comp's
//!   render dims. F-046 / F-047 transforms apply *outside* the mask, so
//!   masks survive scale/rotation changes naturally.

use crate::model::{Curve, Lerp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    #[default]
    Add,
    Subtract,
    Intersect,
    Difference,
}

impl MaskMode {
    pub const ALL: [MaskMode; 4] = [
        MaskMode::Add,
        MaskMode::Subtract,
        MaskMode::Intersect,
        MaskMode::Difference,
    ];
    pub fn label(self) -> &'static str {
        match self {
            MaskMode::Add => "Add",
            MaskMode::Subtract => "Subtract",
            MaskMode::Intersect => "Intersect",
            MaskMode::Difference => "Difference",
        }
    }
}

/// One control point on a closed bezier mask path. `in_tan` and `out_tan`
/// are tangent handles relative to the anchor (not absolute positions); a
/// zero tangent on both sides yields a corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskVertex {
    pub anchor: [f32; 2],
    pub in_tan: [f32; 2],
    pub out_tan: [f32; 2],
}

impl MaskVertex {
    pub fn corner(x: f32, y: f32) -> Self {
        Self {
            anchor: [x, y],
            in_tan: [0.0, 0.0],
            out_tan: [0.0, 0.0],
        }
    }
}

impl Lerp for MaskVertex {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            anchor: self.anchor.lerp(&other.anchor, t),
            in_tan: self.in_tan.lerp(&other.in_tan, t),
            out_tan: self.out_tan.lerp(&other.out_tan, t),
        }
    }
}

/// Closed bezier path. Vertex count is fixed across the lifetime of a mask
/// (per the keyframing constraint above).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MaskPath {
    pub vertices: Vec<MaskVertex>,
}

impl MaskPath {
    pub fn rectangle(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            vertices: vec![
                MaskVertex::corner(x, y),
                MaskVertex::corner(x + w, y),
                MaskVertex::corner(x + w, y + h),
                MaskVertex::corner(x, y + h),
            ],
        }
    }

    /// Approximate ellipse with 4 cubic bezier segments using the standard
    /// 0.5522847498 control-point ratio.
    pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Self {
        const K: f32 = 0.552_284_8;
        let kx = rx * K;
        let ky = ry * K;
        // 4 anchors at top/right/bottom/left, with tangent handles forming
        // the bezier curves between them.
        Self {
            vertices: vec![
                MaskVertex {
                    anchor: [cx, cy - ry],
                    in_tan: [-kx, 0.0],
                    out_tan: [kx, 0.0],
                },
                MaskVertex {
                    anchor: [cx + rx, cy],
                    in_tan: [0.0, -ky],
                    out_tan: [0.0, ky],
                },
                MaskVertex {
                    anchor: [cx, cy + ry],
                    in_tan: [kx, 0.0],
                    out_tan: [-kx, 0.0],
                },
                MaskVertex {
                    anchor: [cx - rx, cy],
                    in_tan: [0.0, ky],
                    out_tan: [0.0, -ky],
                },
            ],
        }
    }
}

impl Lerp for MaskPath {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        // Vertex count must match; if it doesn't, fall back to whichever
        // path is closer in time. This matches the F-064 v1 constraint.
        if self.vertices.len() != other.vertices.len() {
            return if t < 0.5 { self.clone() } else { other.clone() };
        }
        Self {
            vertices: self
                .vertices
                .iter()
                .zip(other.vertices.iter())
                .map(|(a, b)| a.lerp(b, t))
                .collect(),
        }
    }
}

/// A single mask on a layer. Path can be animated; opacity, expansion, and
/// feather are scalar but not currently keyframeable (post-MVP polish).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub name: String,
    #[serde(default)]
    pub mode: MaskMode,
    /// Per-mask opacity in 0..=1. Multiplies the rasterized alpha.
    #[serde(default = "default_one")]
    pub opacity: f32,
    /// Inset/outset in mask-space pixels. Negative shrinks, positive grows.
    /// Approximated in v1 by sampling a thresholded distance field — exact
    /// for axis-aligned simple shapes, conservative for complex ones.
    #[serde(default)]
    pub expansion: f32,
    /// Edge softening in mask-space pixels.
    #[serde(default)]
    pub feather: f32,
    pub path: Curve<MaskPath>,
}

fn default_one() -> f32 {
    1.0
}

impl Mask {
    pub fn rectangle(name: impl Into<String>, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            name: name.into(),
            mode: MaskMode::default(),
            opacity: 1.0,
            expansion: 0.0,
            feather: 0.0,
            path: Curve::Static(MaskPath::rectangle(x, y, w, h)),
        }
    }
    pub fn ellipse(name: impl Into<String>, cx: f32, cy: f32, rx: f32, ry: f32) -> Self {
        Self {
            name: name.into(),
            mode: MaskMode::default(),
            opacity: 1.0,
            expansion: 0.0,
            feather: 0.0,
            path: Curve::Static(MaskPath::ellipse(cx, cy, rx, ry)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_has_four_corners() {
        let r = MaskPath::rectangle(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.vertices.len(), 4);
        assert_eq!(r.vertices[0].anchor, [10.0, 20.0]);
        assert_eq!(r.vertices[2].anchor, [110.0, 70.0]);
        assert!(r.vertices.iter().all(|v| v.in_tan == [0.0, 0.0]));
    }

    #[test]
    fn ellipse_has_four_anchors_with_tangents() {
        let e = MaskPath::ellipse(50.0, 50.0, 30.0, 20.0);
        assert_eq!(e.vertices.len(), 4);
        // Top anchor at (50, 30); horizontal tangents.
        assert_eq!(e.vertices[0].anchor, [50.0, 30.0]);
        assert!(e.vertices[0].out_tan[0] > 0.0);
        assert_eq!(e.vertices[0].out_tan[1], 0.0);
    }

    #[test]
    fn path_lerp_interpolates_vertex_anchors() {
        let a = MaskPath::rectangle(0.0, 0.0, 100.0, 100.0);
        let b = MaskPath::rectangle(50.0, 50.0, 100.0, 100.0);
        let mid = a.lerp(&b, 0.5);
        assert_eq!(mid.vertices[0].anchor, [25.0, 25.0]);
    }

    #[test]
    fn path_lerp_with_mismatched_vertex_count_snaps() {
        let a = MaskPath {
            vertices: vec![MaskVertex::corner(0.0, 0.0); 3],
        };
        let b = MaskPath {
            vertices: vec![MaskVertex::corner(10.0, 10.0); 4],
        };
        // t < 0.5 → a, t >= 0.5 → b.
        assert_eq!(a.lerp(&b, 0.2).vertices.len(), 3);
        assert_eq!(a.lerp(&b, 0.8).vertices.len(), 4);
    }

    #[test]
    fn mask_round_trips_through_serde() {
        let m = Mask::rectangle("box", 0.0, 0.0, 10.0, 10.0);
        let s = ron::ser::to_string_pretty(&m, ron::ser::PrettyConfig::default()).unwrap();
        let back: Mask = ron::from_str(&s).unwrap();
        assert_eq!(m, back);
    }
}
