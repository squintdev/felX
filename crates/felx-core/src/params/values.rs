//! Runtime parameter values, keyed by dotted parameter id.
//!
//! Constructed from an [`EffectManifest`] with each parameter set to its
//! declared default. Mutated as the user edits sliders / colors / etc.
//! Persisted with the project so reopening restores the dialed-in look.

use crate::model::{Curve, Rational};
use crate::params::{EffectManifest, EnumVariant, ParamDecl, ParamKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Float(f32),
    /// Animated float — sampled at the playhead time. Resolves to a
    /// [`ParamValue::Float`] inside [`ParamValues::resolved_at`] before
    /// reaching effect code.
    FloatCurve(Curve<f32>),
    Int(i32),
    Bool(bool),
    Color([f32; 4]),
    Vec2([f32; 2]),
    Enum(String),
    /// Whether the surrounding `OptionalGroup` is enabled.
    GroupEnabled(bool),
}

impl ParamValue {
    pub fn as_float(&self) -> Option<f32> {
        if let ParamValue::Float(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    /// Sample a float-typed value (static or animated) at `time`. Returns
    /// `None` for non-float kinds.
    pub fn as_float_at(&self, time: Rational) -> Option<f32> {
        match self {
            ParamValue::Float(v) => Some(*v),
            ParamValue::FloatCurve(c) => Some(c.sample_at_time(time)),
            _ => None,
        }
    }
    pub fn as_float_curve(&self) -> Option<&Curve<f32>> {
        if let ParamValue::FloatCurve(c) = self {
            Some(c)
        } else {
            None
        }
    }
    pub fn as_int(&self) -> Option<i32> {
        if let ParamValue::Int(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let ParamValue::Bool(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_color(&self) -> Option<[f32; 4]> {
        if let ParamValue::Color(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_vec2(&self) -> Option<[f32; 2]> {
        if let ParamValue::Vec2(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_enum(&self) -> Option<&str> {
        if let ParamValue::Enum(v) = self {
            Some(v.as_str())
        } else {
            None
        }
    }
    pub fn as_group_enabled(&self) -> Option<bool> {
        if let ParamValue::GroupEnabled(v) = self {
            Some(*v)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParamValues {
    /// Dotted-id (`"head_switching.height"`) → value. BTreeMap so the
    /// on-disk RON form has stable ordering for diff readability.
    values: BTreeMap<String, ParamValue>,
}

impl ParamValues {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a fresh values tree from the manifest, with each parameter set
    /// to its declared default.
    pub fn from_manifest(manifest: &EffectManifest) -> Self {
        let mut values = BTreeMap::new();
        seed_defaults(&manifest.parameters, "", &mut values);
        Self { values }
    }

    pub fn get(&self, id: &str) -> Option<&ParamValue> {
        self.values.get(id)
    }

    pub fn set(&mut self, id: impl Into<String>, value: ParamValue) {
        self.values.insert(id.into(), value);
    }

    /// Iterate id → value pairs in stable order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Snapshot all animated values at `time`, returning a copy where every
    /// [`ParamValue::FloatCurve`] entry has been resolved to a static
    /// [`ParamValue::Float`]. Other entries are unchanged. Effect code
    /// reads from the resolved view so it never has to know whether a
    /// parameter was animated.
    pub fn resolved_at(&self, time: Rational) -> ParamValues {
        let values = self
            .values
            .iter()
            .map(|(id, v)| {
                let resolved = match v {
                    ParamValue::FloatCurve(c) => ParamValue::Float(c.sample_at_time(time)),
                    other => other.clone(),
                };
                (id.clone(), resolved)
            })
            .collect();
        ParamValues { values }
    }

    /// Convenience accessors.
    pub fn float(&self, id: &str) -> Option<f32> {
        self.get(id)?.as_float()
    }
    /// Sample a float (static or animated) at `time`.
    pub fn float_at(&self, id: &str, time: Rational) -> Option<f32> {
        self.get(id)?.as_float_at(time)
    }
    pub fn float_curve(&self, id: &str) -> Option<&Curve<f32>> {
        self.get(id)?.as_float_curve()
    }
    pub fn int(&self, id: &str) -> Option<i32> {
        self.get(id)?.as_int()
    }
    pub fn bool(&self, id: &str) -> Option<bool> {
        self.get(id)?.as_bool()
    }
    pub fn color(&self, id: &str) -> Option<[f32; 4]> {
        self.get(id)?.as_color()
    }
    pub fn vec2(&self, id: &str) -> Option<[f32; 2]> {
        self.get(id)?.as_vec2()
    }
    pub fn enum_str(&self, id: &str) -> Option<&str> {
        self.get(id)?.as_enum()
    }
    pub fn group_enabled(&self, id: &str) -> Option<bool> {
        self.get(id)?.as_group_enabled()
    }
}

fn seed_defaults(params: &[ParamDecl], prefix: &str, out: &mut BTreeMap<String, ParamValue>) {
    for p in params {
        let id = if prefix.is_empty() {
            p.id.clone()
        } else {
            format!("{prefix}.{}", p.id)
        };
        match &p.kind {
            ParamKind::Float { default, .. } => {
                out.insert(id, ParamValue::Float(*default));
            }
            ParamKind::Int { default, .. } => {
                out.insert(id, ParamValue::Int(*default));
            }
            ParamKind::Bool { default } => {
                out.insert(id, ParamValue::Bool(*default));
            }
            ParamKind::Color { default } => {
                out.insert(id, ParamValue::Color(*default));
            }
            ParamKind::Vec2 { default } => {
                out.insert(id, ParamValue::Vec2(*default));
            }
            ParamKind::Enum {
                default, variants, ..
            } => {
                let chosen = if variants.iter().any(|v: &EnumVariant| v.id == *default) {
                    default.clone()
                } else {
                    variants.first().map(|v| v.id.clone()).unwrap_or_default()
                };
                out.insert(id, ParamValue::Enum(chosen));
            }
            ParamKind::Group { parameters } => {
                seed_defaults(parameters, &id, out);
            }
            ParamKind::OptionalGroup {
                default_enabled,
                parameters,
            } => {
                out.insert(id.clone(), ParamValue::GroupEnabled(*default_enabled));
                seed_defaults(parameters, &id, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::EffectManifest;

    fn signal_like_manifest() -> EffectManifest {
        EffectManifest::parse(
            r#"(
                id: "x",
                display_name: "X",
                parameters: [
                    (id: "intensity", display_name: "I",
                     kind: Float(default: 0.7, range: (min: 0.0, max: 1.0))),
                    (id: "head_switching", display_name: "HS",
                     kind: OptionalGroup(default_enabled: false, parameters: [
                        (id: "height", display_name: "H",
                         kind: Int(default: 8, range: (min: 0, max: 64))),
                     ])),
                ],
            )"#,
        )
        .unwrap()
    }

    #[test]
    fn from_manifest_seeds_defaults() {
        let m = signal_like_manifest();
        let v = ParamValues::from_manifest(&m);
        assert_eq!(v.float("intensity"), Some(0.7));
        assert_eq!(v.group_enabled("head_switching"), Some(false));
        assert_eq!(v.int("head_switching.height"), Some(8));
    }

    #[test]
    fn set_then_get() {
        let mut v = ParamValues::new();
        v.set("a", ParamValue::Float(0.25));
        assert_eq!(v.float("a"), Some(0.25));
        v.set("a", ParamValue::Float(0.75));
        assert_eq!(v.float("a"), Some(0.75));
    }

    #[test]
    fn type_mismatched_accessor_returns_none() {
        let mut v = ParamValues::new();
        v.set("a", ParamValue::Float(1.0));
        assert!(v.bool("a").is_none());
        assert!(v.int("a").is_none());
    }

    #[test]
    fn round_trips_through_serde() {
        let m = signal_like_manifest();
        let original = ParamValues::from_manifest(&m);
        let ser = ron::ser::to_string_pretty(&original, ron::ser::PrettyConfig::default()).unwrap();
        let parsed: ParamValues = ron::from_str(&ser).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn iter_is_stable_alphabetical() {
        let mut v = ParamValues::new();
        v.set("z", ParamValue::Bool(true));
        v.set("a", ParamValue::Bool(false));
        let ids: Vec<&str> = v.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["a", "z"]);
    }

    use crate::model::{InterpKind, Keyframe};

    #[test]
    fn float_curve_round_trips_through_serde() {
        let mut v = ParamValues::new();
        v.set(
            "g",
            ParamValue::FloatCurve(Curve::Animated(vec![
                Keyframe {
                    t: Rational::new(0, 30),
                    v: 0.0,
                    interp: InterpKind::Linear,
                },
                Keyframe {
                    t: Rational::new(60, 30),
                    v: 1.0,
                    interp: InterpKind::EaseInOut,
                },
            ])),
        );
        let s = ron::ser::to_string_pretty(&v, ron::ser::PrettyConfig::default()).unwrap();
        let back: ParamValues = ron::from_str(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn float_at_samples_curve() {
        let mut v = ParamValues::new();
        v.set(
            "g",
            ParamValue::FloatCurve(Curve::Animated(vec![
                Keyframe {
                    t: Rational::new(0, 30),
                    v: 0.0,
                    interp: InterpKind::Linear,
                },
                Keyframe {
                    t: Rational::new(60, 30),
                    v: 1.0,
                    interp: InterpKind::Linear,
                },
            ])),
        );
        // Midpoint at frame 30 (1.0s on a 30/30 curve).
        let mid = v.float_at("g", Rational::new(30, 30)).unwrap();
        assert!((mid - 0.5).abs() < 1e-6);
        // Static accessor returns None for an animated value.
        assert!(v.float("g").is_none());
    }

    #[test]
    fn resolved_at_collapses_curve_to_static_float() {
        let mut v = ParamValues::new();
        v.set(
            "g",
            ParamValue::FloatCurve(Curve::Animated(vec![
                Keyframe {
                    t: Rational::new(0, 30),
                    v: 0.0,
                    interp: InterpKind::Linear,
                },
                Keyframe {
                    t: Rational::new(60, 30),
                    v: 1.0,
                    interp: InterpKind::Linear,
                },
            ])),
        );
        v.set("h", ParamValue::Bool(true));
        let resolved = v.resolved_at(Rational::new(30, 30));
        assert_eq!(resolved.float("g"), Some(0.5));
        assert_eq!(resolved.bool("h"), Some(true));
    }
}
