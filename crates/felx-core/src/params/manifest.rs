//! Manifest types and (de)serialization.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level manifest, one per effect. On disk: `effects/<id>/manifest.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectManifest {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub working_space: WorkingSpace,
    #[serde(default)]
    pub pass: PassKind,
    pub parameters: Vec<ParamDecl>,
}

/// Color-space framing the compositor wraps the effect's pass with. Per ADR
/// semantics, sRGB-tagged effects (e.g. CC Toner) get an sRGB encode → pass
/// → decode cycle so their math runs in gamma-encoded space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkingSpace {
    #[default]
    Linear,
    #[serde(rename = "srgb")]
    SRgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassKind {
    #[default]
    Gpu,
    Cpu,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDecl {
    pub id: String,
    pub display_name: String,
    pub kind: ParamKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamKind {
    Float {
        default: f32,
        range: FloatRange,
    },
    Int {
        default: i32,
        range: IntRange,
    },
    Bool {
        default: bool,
    },
    Color {
        default: [f32; 4],
    },
    Vec2 {
        default: [f32; 2],
    },
    Enum {
        variants: Vec<EnumVariant>,
        default: String,
    },
    Group {
        parameters: Vec<ParamDecl>,
    },
    OptionalGroup {
        default_enabled: bool,
        parameters: Vec<ParamDecl>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloatRange {
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntRange {
    pub min: i32,
    pub max: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariant {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug)]
pub enum ManifestError {
    Io(std::io::Error),
    Parse(ron::de::SpannedError),
    DuplicateParameterId(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "io: {e}"),
            ManifestError::Parse(e) => write!(f, "parse: {e}"),
            ManifestError::DuplicateParameterId(id) => {
                write!(f, "duplicate parameter id: {id}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<std::io::Error> for ManifestError {
    fn from(e: std::io::Error) -> Self {
        ManifestError::Io(e)
    }
}

impl From<ron::de::SpannedError> for ManifestError {
    fn from(e: ron::de::SpannedError) -> Self {
        ManifestError::Parse(e)
    }
}

impl EffectManifest {
    /// Parse a manifest file from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path)?;
        let m: EffectManifest = ron::from_str(&text)?;
        m.validate()?;
        Ok(m)
    }

    /// Parse a manifest from a string. Useful for tests and inline fixtures.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let m: EffectManifest = ron::from_str(text)?;
        m.validate()?;
        Ok(m)
    }

    /// Reject manifests with duplicate parameter IDs at the same level.
    pub fn validate(&self) -> Result<(), ManifestError> {
        validate_param_list(&self.parameters)
    }

    /// Recursively iterate every parameter id in dotted-path form
    /// ("group.subgroup.id"), in declaration order.
    pub fn ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect_ids(&self.parameters, "", &mut out);
        out
    }
}

fn validate_param_list(params: &[ParamDecl]) -> Result<(), ManifestError> {
    let mut seen = std::collections::HashSet::new();
    for p in params {
        if !seen.insert(&p.id) {
            return Err(ManifestError::DuplicateParameterId(p.id.clone()));
        }
        match &p.kind {
            ParamKind::Group { parameters } => validate_param_list(parameters)?,
            ParamKind::OptionalGroup { parameters, .. } => validate_param_list(parameters)?,
            _ => {}
        }
    }
    Ok(())
}

fn collect_ids(params: &[ParamDecl], prefix: &str, out: &mut Vec<String>) {
    for p in params {
        let path = if prefix.is_empty() {
            p.id.clone()
        } else {
            format!("{prefix}.{}", p.id)
        };
        out.push(path.clone());
        match &p.kind {
            ParamKind::Group { parameters } => collect_ids(parameters, &path, out),
            ParamKind::OptionalGroup { parameters, .. } => collect_ids(parameters, &path, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_manifest_text() -> &'static str {
        r#"(
            id: "gain",
            display_name: "Gain",
            category: "Color",
            working_space: linear,
            pass: gpu,
            parameters: [
                (
                    id: "gain",
                    display_name: "Gain",
                    kind: Float(default: 1.0, range: (min: 0.0, max: 4.0)),
                ),
            ],
        )"#
    }

    fn signal_like_manifest_text() -> &'static str {
        r#"(
            id: "signal",
            display_name: "Signal",
            category: "Stylize",
            working_space: srgb,
            pass: cpu,
            parameters: [
                (
                    id: "intensity",
                    display_name: "Intensity",
                    kind: Float(default: 0.7, range: (min: 0.0, max: 1.0)),
                ),
                (
                    id: "head_switching",
                    display_name: "Head switching",
                    kind: OptionalGroup(default_enabled: false, parameters: [
                        (
                            id: "height",
                            display_name: "Height (px)",
                            kind: Int(default: 8, range: (min: 0, max: 64)),
                        ),
                        (
                            id: "horiz_shift",
                            display_name: "Horizontal shift",
                            kind: Float(default: 4.0, range: (min: 0.0, max: 32.0)),
                        ),
                    ]),
                ),
            ],
        )"#
    }

    #[test]
    fn parses_gain_manifest() {
        let m = EffectManifest::parse(gain_manifest_text()).unwrap();
        assert_eq!(m.id, "gain");
        assert_eq!(m.working_space, WorkingSpace::Linear);
        assert_eq!(m.pass, PassKind::Gpu);
        assert_eq!(m.parameters.len(), 1);
    }

    #[test]
    fn parses_optional_group() {
        let m = EffectManifest::parse(signal_like_manifest_text()).unwrap();
        assert_eq!(m.parameters.len(), 2);
        match &m.parameters[1].kind {
            ParamKind::OptionalGroup {
                default_enabled,
                parameters,
            } => {
                assert!(!default_enabled);
                assert_eq!(parameters.len(), 2);
            }
            other => panic!("expected OptionalGroup, got {other:?}"),
        }
    }

    #[test]
    fn ids_uses_dotted_paths_for_nested_params() {
        let m = EffectManifest::parse(signal_like_manifest_text()).unwrap();
        let ids = m.ids();
        assert!(ids.contains(&"intensity".to_string()));
        assert!(ids.contains(&"head_switching".to_string()));
        assert!(ids.contains(&"head_switching.height".to_string()));
        assert!(ids.contains(&"head_switching.horiz_shift".to_string()));
    }

    #[test]
    fn duplicate_parameter_id_rejected() {
        let text = r#"(
            id: "x",
            display_name: "X",
            parameters: [
                (id: "a", display_name: "A", kind: Bool(default: true)),
                (id: "a", display_name: "Also A", kind: Bool(default: false)),
            ],
        )"#;
        let err = EffectManifest::parse(text).unwrap_err();
        assert!(matches!(err, ManifestError::DuplicateParameterId(_)));
    }

    #[test]
    fn round_trip_through_json_preserves_shape() {
        let original = EffectManifest::parse(signal_like_manifest_text()).unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let from_json: EffectManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(original, from_json);
    }

    #[test]
    fn working_space_variants_serialize_snake_case() {
        let m = EffectManifest::parse(signal_like_manifest_text()).unwrap();
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains("\"srgb\""),
            "expected snake_case 'srgb' in JSON, got {json}"
        );
    }

    #[test]
    fn loads_from_file() {
        let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../effects/gain/manifest.ron");
        let m = EffectManifest::load(manifest_path).unwrap();
        assert_eq!(m.id, "gain");
        assert_eq!(m.parameters.len(), 1);
    }

    #[test]
    fn missing_file_returns_io_error() {
        let err = EffectManifest::load("/no/such/manifest.ron").unwrap_err();
        assert!(matches!(err, ManifestError::Io(_)));
    }
}
