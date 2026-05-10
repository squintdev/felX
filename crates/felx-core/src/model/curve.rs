//! Value-or-keyframed-curve. M2's F-030 fills in the [`Curve::Animated`]
//! variant; F-035 / F-036 add the editor UI on top.

use serde::{Deserialize, Serialize};

use crate::model::time::Rational;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Curve<T> {
    Static(T),
    Animated(Vec<Keyframe<T>>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe<T> {
    pub t: Rational,
    pub v: T,
    /// How to interpolate from this keyframe to the *next* one. The last
    /// keyframe's interp is irrelevant (the curve clamps after it).
    pub interp: InterpKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterpKind {
    /// Hold the keyframe's value until exactly the next keyframe's time.
    Hold,
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// Linear-interpolation trait used by [`Curve::sample_at_time`]. The
/// generic Curve<T> requires `T: Lerp + Clone` for sampling; impls are
/// provided for the common parameter value types.
pub trait Lerp {
    fn lerp(&self, other: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        self + (other - self) * t
    }
}

impl Lerp for [f32; 2] {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        [
            self[0] + (other[0] - self[0]) * t,
            self[1] + (other[1] - self[1]) * t,
        ]
    }
}

impl Lerp for [f32; 4] {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        [
            self[0] + (other[0] - self[0]) * t,
            self[1] + (other[1] - self[1]) * t,
            self[2] + (other[2] - self[2]) * t,
            self[3] + (other[3] - self[3]) * t,
        ]
    }
}

impl Lerp for i32 {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        let a = *self as f32;
        let b = *other as f32;
        (a + (b - a) * t).round() as i32
    }
}

impl<T: Clone> Curve<T> {
    /// Sample at a frame index. Only valid for `Static`; for animated
    /// curves the caller must call [`Self::sample_at_time`] with the
    /// composition's framerate-converted time.
    ///
    /// For animated curves this returns the first keyframe's value as a
    /// best-effort fallback.
    pub fn sample_at(&self, _frame: u32) -> T {
        match self {
            Curve::Static(v) => v.clone(),
            Curve::Animated(kfs) => kfs
                .first()
                .map(|k| k.v.clone())
                .expect("animated curve must have at least one keyframe"),
        }
    }
}

impl<T: Lerp + Clone> Curve<T> {
    /// Sample at a specific time on the comp's timebase.
    pub fn sample_at_time(&self, time: Rational) -> T {
        match self {
            Curve::Static(v) => v.clone(),
            Curve::Animated(kfs) if kfs.is_empty() => {
                panic!("animated curve must have at least one keyframe")
            }
            Curve::Animated(kfs) => sample_animated(kfs, time),
        }
    }
}

impl<T: Default> Default for Curve<T> {
    fn default() -> Self {
        Curve::Static(T::default())
    }
}

fn sample_animated<T: Lerp + Clone>(kfs: &[Keyframe<T>], time: Rational) -> T {
    let target = time.as_seconds();
    // Clamp before first / after last.
    if target <= kfs[0].t.as_seconds() {
        return kfs[0].v.clone();
    }
    if target >= kfs.last().unwrap().t.as_seconds() {
        return kfs.last().unwrap().v.clone();
    }
    // Find the segment where target falls.
    for window in kfs.windows(2) {
        let a = &window[0];
        let b = &window[1];
        let ta = a.t.as_seconds();
        let tb = b.t.as_seconds();
        if target >= ta && target <= tb {
            if (tb - ta).abs() < 1e-12 {
                return b.v.clone();
            }
            let u = ((target - ta) / (tb - ta)) as f32;
            return interpolate(&a.v, &b.v, u, a.interp);
        }
    }
    // Should be unreachable; the clamps above cover before-first and
    // after-last, and the loop covers everything in between.
    kfs.last().unwrap().v.clone()
}

fn interpolate<T: Lerp + Clone>(a: &T, b: &T, u: f32, kind: InterpKind) -> T {
    match kind {
        InterpKind::Hold => a.clone(),
        InterpKind::Linear => a.lerp(b, u),
        InterpKind::EaseIn => a.lerp(b, ease_in(u)),
        InterpKind::EaseOut => a.lerp(b, ease_out(u)),
        InterpKind::EaseInOut => a.lerp(b, ease_in_out(u)),
    }
}

/// Cubic ease-in: starts slow, ends linear-ish.
fn ease_in(u: f32) -> f32 {
    u * u * u
}

/// Cubic ease-out: starts fast, decelerates.
fn ease_out(u: f32) -> f32 {
    let inv = 1.0 - u;
    1.0 - inv * inv * inv
}

/// Smoothstep — symmetric ease-in-out. Matches AE's "easy ease" closely
/// enough for the common cases.
fn ease_in_out(u: f32) -> f32 {
    u * u * (3.0 - 2.0 * u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rat(num: u32, den: u32) -> Rational {
        Rational::new(num, den)
    }

    #[test]
    fn static_curve_samples_constant() {
        let c = Curve::Static(0.5_f32);
        assert_eq!(c.sample_at_time(rat(0, 1)), 0.5);
        assert_eq!(c.sample_at_time(rat(100, 1)), 0.5);
    }

    #[test]
    fn linear_two_keyframes_returns_midpoint() {
        let c = Curve::Animated(vec![
            Keyframe {
                t: rat(0, 1),
                v: 0.0_f32,
                interp: InterpKind::Linear,
            },
            Keyframe {
                t: rat(2, 1),
                v: 1.0_f32,
                interp: InterpKind::Linear,
            },
        ]);
        // Midpoint at t=1s.
        assert!((c.sample_at_time(rat(1, 1)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hold_keyframe_stays_at_first_value_until_next() {
        let c = Curve::Animated(vec![
            Keyframe {
                t: rat(0, 1),
                v: 1.0_f32,
                interp: InterpKind::Hold,
            },
            Keyframe {
                t: rat(2, 1),
                v: 5.0_f32,
                interp: InterpKind::Linear,
            },
        ]);
        assert_eq!(c.sample_at_time(rat(0, 1)), 1.0);
        assert_eq!(c.sample_at_time(rat(1, 1)), 1.0);
        // Exactly at 2s lands on k2 via the clamp / segment-end logic.
        assert_eq!(c.sample_at_time(rat(2, 1)), 5.0);
    }

    #[test]
    fn out_of_range_before_first_clamps_to_first() {
        let c = Curve::Animated(vec![
            Keyframe {
                t: rat(2, 1),
                v: 7.0_f32,
                interp: InterpKind::Linear,
            },
            Keyframe {
                t: rat(4, 1),
                v: 9.0_f32,
                interp: InterpKind::Linear,
            },
        ]);
        assert_eq!(c.sample_at_time(rat(0, 1)), 7.0);
    }

    #[test]
    fn out_of_range_after_last_clamps_to_last() {
        let c = Curve::Animated(vec![
            Keyframe {
                t: rat(0, 1),
                v: 7.0_f32,
                interp: InterpKind::Linear,
            },
            Keyframe {
                t: rat(2, 1),
                v: 9.0_f32,
                interp: InterpKind::Linear,
            },
        ]);
        assert_eq!(c.sample_at_time(rat(10, 1)), 9.0);
    }

    #[test]
    fn ease_in_out_at_midpoint_is_half() {
        let c = Curve::Animated(vec![
            Keyframe {
                t: rat(0, 1),
                v: 0.0_f32,
                interp: InterpKind::EaseInOut,
            },
            Keyframe {
                t: rat(2, 1),
                v: 1.0_f32,
                interp: InterpKind::Linear,
            },
        ]);
        assert!((c.sample_at_time(rat(1, 1)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ease_in_starts_slow() {
        let c = Curve::Animated(vec![
            Keyframe {
                t: rat(0, 1),
                v: 0.0_f32,
                interp: InterpKind::EaseIn,
            },
            Keyframe {
                t: rat(10, 1),
                v: 1.0_f32,
                interp: InterpKind::Linear,
            },
        ]);
        // Quarter-way through time, value should be much less than 0.25
        // (cubic ease-in: 0.25^3 = 0.015625).
        let v = c.sample_at_time(rat(25, 10));
        assert!((v - 0.015625).abs() < 1e-3);
    }

    #[test]
    fn lerp_color_components_independently() {
        let a = [0.0_f32, 0.0, 1.0, 1.0];
        let b = [1.0_f32, 1.0, 0.0, 1.0];
        let mid = a.lerp(&b, 0.5);
        assert!((mid[0] - 0.5).abs() < 1e-6);
        assert!((mid[1] - 0.5).abs() < 1e-6);
        assert!((mid[2] - 0.5).abs() < 1e-6);
        assert!((mid[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_int_rounds() {
        assert_eq!(0_i32.lerp(&10, 0.5), 5);
        assert_eq!(0_i32.lerp(&10, 0.07), 1);
    }

    #[test]
    fn round_trips_through_serde() {
        let c: Curve<f32> = Curve::Animated(vec![
            Keyframe {
                t: rat(0, 1),
                v: 0.0,
                interp: InterpKind::EaseInOut,
            },
            Keyframe {
                t: rat(60, 1),
                v: 1.0,
                interp: InterpKind::Linear,
            },
        ]);
        let s = ron::ser::to_string_pretty(&c, ron::ser::PrettyConfig::default()).unwrap();
        let back: Curve<f32> = ron::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}
