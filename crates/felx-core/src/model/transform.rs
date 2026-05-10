//! 2D affine transform components, each keyframeable.

use crate::model::Curve;

#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    /// Position in composition pixels, relative to the comp origin.
    pub position: Curve<[f32; 2]>,
    /// Anchor point in layer-local pixels — pivot for scale and rotation.
    pub anchor: Curve<[f32; 2]>,
    /// Scale factor per axis. 1.0 is identity.
    pub scale: Curve<[f32; 2]>,
    /// Rotation in degrees, clockwise.
    pub rotation: Curve<f32>,
    /// Opacity in `0.0..=1.0`.
    pub opacity: Curve<f32>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Curve::Static([0.0, 0.0]),
            anchor: Curve::Static([0.0, 0.0]),
            scale: Curve::Static([1.0, 1.0]),
            rotation: Curve::Static(0.0),
            opacity: Curve::Static(1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let t = Transform::default();
        assert_eq!(t.position.sample_at(0), [0.0, 0.0]);
        assert_eq!(t.anchor.sample_at(0), [0.0, 0.0]);
        assert_eq!(t.scale.sample_at(0), [1.0, 1.0]);
        assert_eq!(t.rotation.sample_at(0), 0.0);
        assert_eq!(t.opacity.sample_at(0), 1.0);
    }
}
