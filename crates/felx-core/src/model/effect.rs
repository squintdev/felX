//! Effect references on a layer.

use crate::params::ParamValues;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Effect {
    pub id: String,
    pub enabled: bool,
    /// Per-parameter live values, keyed by dotted parameter id. Defaulted
    /// from the effect's manifest at construction time and mutated by the
    /// UI / animation system.
    #[serde(default)]
    pub values: ParamValues,
}

impl Effect {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            enabled: true,
            values: ParamValues::new(),
        }
    }

    /// Construct an effect with its parameter values seeded from the
    /// manifest's declared defaults.
    pub fn from_manifest(manifest: &crate::params::EffectManifest) -> Self {
        Self {
            id: manifest.id.clone(),
            enabled: true,
            values: ParamValues::from_manifest(manifest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::EffectManifest;

    #[test]
    fn effect_new_is_enabled() {
        let e = Effect::new("cc_toner");
        assert_eq!(e.id, "cc_toner");
        assert!(e.enabled);
        assert!(e.values.is_empty());
    }

    #[test]
    fn from_manifest_seeds_defaults() {
        let m = EffectManifest::parse(
            r#"(
                id: "gain",
                display_name: "Gain",
                parameters: [
                    (id: "gain", display_name: "Gain",
                     kind: Float(default: 1.0, range: (min: 0.0, max: 4.0))),
                ],
            )"#,
        )
        .unwrap();
        let e = Effect::from_manifest(&m);
        assert_eq!(e.id, "gain");
        assert_eq!(e.values.float("gain"), Some(1.0));
    }
}
