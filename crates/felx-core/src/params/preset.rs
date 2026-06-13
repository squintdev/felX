//! Effect presets — saved chains of effects with parameter values, applied
//! to a layer in one click. Live in `presets/<name>.ron` at the workspace
//! root; users can save their own presets the same way.

use crate::params::{ParamValue, ParamValues};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectPreset {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub effects: Vec<PresetEffect>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetEffect {
    pub id: String,
    #[serde(default)]
    pub values: ParamValues,
}

#[derive(Debug)]
pub enum PresetError {
    Io(std::io::Error),
    Parse(ron::de::SpannedError),
}

impl std::fmt::Display for PresetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetError::Io(e) => write!(f, "io: {e}"),
            PresetError::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}

impl std::error::Error for PresetError {}

impl From<std::io::Error> for PresetError {
    fn from(e: std::io::Error) -> Self {
        PresetError::Io(e)
    }
}
impl From<ron::de::SpannedError> for PresetError {
    fn from(e: ron::de::SpannedError) -> Self {
        PresetError::Parse(e)
    }
}

impl EffectPreset {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PresetError> {
        let text = std::fs::read_to_string(path)?;
        Ok(ron::from_str(&text)?)
    }

    pub fn parse(text: &str) -> Result<Self, PresetError> {
        Ok(ron::from_str(text)?)
    }
}

/// Convenience builder for programmatic preset construction (mostly used
/// by tests and by the four built-in presets at fixture-time).
pub fn preset_effect(id: impl Into<String>, values: &[(&str, ParamValue)]) -> PresetEffect {
    let mut pv = ParamValues::new();
    for (k, v) in values {
        pv.set((*k).to_string(), v.clone());
    }
    PresetEffect {
        id: id.into(),
        values: pv,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_preset() {
        let p = EffectPreset::parse(
            r#"(
                name: "Test",
                effects: [
                    (id: "gain"),
                    (id: "cc_toner"),
                ],
            )"#,
        )
        .unwrap();
        assert_eq!(p.name, "Test");
        assert_eq!(p.effects.len(), 2);
        assert_eq!(p.effects[0].id, "gain");
    }

    #[test]
    fn round_trip_through_serde() {
        let original = EffectPreset {
            name: "X".into(),
            description: "y".into(),
            effects: vec![preset_effect("gain", &[("gain", ParamValue::Float(0.5))])],
        };
        let s = ron::ser::to_string_pretty(&original, ron::ser::PrettyConfig::default()).unwrap();
        let back: EffectPreset = ron::from_str(&s).unwrap();
        assert_eq!(original, back);
    }
}
