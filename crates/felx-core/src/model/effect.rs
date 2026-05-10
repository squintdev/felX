//! Effect references on a layer.
//!
//! Story F-019 adds the parameter-system tree; here an effect carries only
//! its identifier (matching `effects/<id>/manifest.ron`) and an enabled flag.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    pub id: String,
    pub enabled: bool,
}

impl Effect {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_new_is_enabled() {
        let e = Effect::new("cc_toner");
        assert_eq!(e.id, "cc_toner");
        assert!(e.enabled);
    }
}
