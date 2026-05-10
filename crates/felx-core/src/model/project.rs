//! Top-level project container.

use crate::model::{Asset, AssetId, AssetKind, CompId, Composition};
use std::path::PathBuf;

/// Bumped whenever the on-disk project schema changes incompatibly.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub format_version: u32,
    pub assets: Vec<Asset>,
    pub compositions: Vec<Composition>,
    #[serde(default)]
    next_asset_id: u32,
    #[serde(default)]
    next_comp_id: u32,
}

impl Project {
    pub fn new() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            assets: Vec::new(),
            compositions: Vec::new(),
            next_asset_id: 1,
            next_comp_id: 1,
        }
    }

    pub fn add_asset(&mut self, path: impl Into<PathBuf>, kind: AssetKind) -> AssetId {
        let id = AssetId(self.next_asset_id);
        self.next_asset_id += 1;
        self.assets.push(Asset {
            id,
            path: path.into(),
            kind,
        });
        id
    }

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    pub fn add_composition(&mut self, name: impl Into<String>, width: u32, height: u32) -> CompId {
        let id = CompId(self.next_comp_id);
        self.next_comp_id += 1;
        self.compositions
            .push(Composition::new(id, name, width, height));
        id
    }

    pub fn composition(&self, id: CompId) -> Option<&Composition> {
        self.compositions.iter().find(|c| c.id == id)
    }

    pub fn composition_mut(&mut self, id: CompId) -> Option<&mut Composition> {
        self.compositions.iter_mut().find(|c| c.id == id)
    }

    /// Recompute internal allocators after deserialize, in case the on-disk
    /// format omitted them (older format). Walks compositions to fix their
    /// per-comp `next_layer_id` too.
    pub fn fixup_after_load(&mut self) {
        if self.next_asset_id == 0 {
            self.next_asset_id = self.assets.iter().map(|a| a.id.0).max().unwrap_or(0) + 1;
        }
        if self.next_comp_id == 0 {
            self.next_comp_id = self.compositions.iter().map(|c| c.id.0).max().unwrap_or(0) + 1;
        }
        for comp in &mut self.compositions {
            comp.fixup_after_load();
        }
    }

    /// Walk the project and return all structural invariant violations.
    /// `Ok(())` means the project is internally consistent; this does not
    /// verify that asset files actually exist on disk.
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        for comp in &self.compositions {
            for layer in &comp.layers {
                if layer.in_frame > layer.out_frame {
                    errors.push(ValidationError::LayerInAfterOut {
                        comp: comp.id,
                        layer: layer.id,
                    });
                }
                if layer.out_frame > comp.duration_frames {
                    errors.push(ValidationError::LayerOutBeyondCompDuration {
                        comp: comp.id,
                        layer: layer.id,
                        out_frame: layer.out_frame,
                        comp_duration: comp.duration_frames,
                    });
                }
                match &layer.kind {
                    crate::model::LayerKind::Video { asset }
                    | crate::model::LayerKind::Image { asset }
                    | crate::model::LayerKind::Audio { asset }
                        if self.asset(*asset).is_none() =>
                    {
                        errors.push(ValidationError::UnknownAsset {
                            comp: comp.id,
                            layer: layer.id,
                            asset: *asset,
                        });
                    }
                    crate::model::LayerKind::Composition { comp: target }
                        if self.composition(*target).is_none() =>
                    {
                        errors.push(ValidationError::UnknownComposition {
                            comp: comp.id,
                            layer: layer.id,
                            target: *target,
                        });
                    }
                    crate::model::LayerKind::Composition { comp: target } if *target == comp.id => {
                        errors.push(ValidationError::CompositionSelfReference {
                            comp: comp.id,
                            layer: layer.id,
                        });
                    }
                    _ => {}
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    LayerInAfterOut {
        comp: CompId,
        layer: crate::model::LayerId,
    },
    LayerOutBeyondCompDuration {
        comp: CompId,
        layer: crate::model::LayerId,
        out_frame: u32,
        comp_duration: u32,
    },
    UnknownAsset {
        comp: CompId,
        layer: crate::model::LayerId,
        asset: AssetId,
    },
    UnknownComposition {
        comp: CompId,
        layer: crate::model::LayerId,
        target: CompId,
    },
    CompositionSelfReference {
        comp: CompId,
        layer: crate::model::LayerId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Effect, Framerate, LayerKind};

    fn one_layer_project() -> Project {
        let mut p = Project::new();
        let asset = p.add_asset("media/clip.mp4", AssetKind::Video);
        let comp = p.add_composition("main", 1920, 1080);
        let c = p.composition_mut(comp).unwrap();
        c.framerate = Framerate::FPS_30;
        c.duration_frames = 300;
        let layer = c.add_video("clip", asset);
        c.push_effect(layer, Effect::new("cc_toner"));
        p
    }

    #[test]
    fn project_construction() {
        let p = one_layer_project();
        assert_eq!(p.compositions.len(), 1);
        assert_eq!(p.assets.len(), 1);
        assert_eq!(p.compositions[0].layers.len(), 1);
        assert_eq!(p.compositions[0].layers[0].effects.len(), 1);
    }

    #[test]
    fn valid_project_validates_clean() {
        assert!(one_layer_project().validate().is_ok());
    }

    #[test]
    fn layer_out_beyond_comp_duration_is_an_error() {
        let mut p = Project::new();
        let asset = p.add_asset("media/clip.mp4", AssetKind::Video);
        let comp = p.add_composition("main", 100, 100);
        let c = p.composition_mut(comp).unwrap();
        c.duration_frames = 100;
        c.add_layer(
            "long",
            LayerKind::Video { asset },
            0,
            500, // beyond comp duration
        );
        let errors = p.validate().unwrap_err();
        assert!(matches!(
            errors[0],
            ValidationError::LayerOutBeyondCompDuration { .. }
        ));
    }

    #[test]
    fn layer_in_after_out_is_an_error() {
        let mut p = Project::new();
        let comp = p.add_composition("main", 100, 100);
        let c = p.composition_mut(comp).unwrap();
        c.duration_frames = 100;
        c.add_layer("backwards", LayerKind::Null, 80, 20);
        let errors = p.validate().unwrap_err();
        assert!(matches!(errors[0], ValidationError::LayerInAfterOut { .. }));
    }

    #[test]
    fn unknown_asset_is_an_error() {
        let mut p = Project::new();
        let comp = p.add_composition("main", 100, 100);
        let c = p.composition_mut(comp).unwrap();
        c.duration_frames = 100;
        c.add_layer(
            "ghost",
            LayerKind::Video {
                asset: AssetId(999),
            },
            0,
            100,
        );
        let errors = p.validate().unwrap_err();
        assert!(matches!(errors[0], ValidationError::UnknownAsset { .. }));
    }

    #[test]
    fn composition_self_reference_is_an_error() {
        let mut p = Project::new();
        let comp = p.add_composition("loopy", 100, 100);
        let c = p.composition_mut(comp).unwrap();
        c.duration_frames = 100;
        c.add_layer("self", LayerKind::Composition { comp }, 0, 100);
        let errors = p.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::CompositionSelfReference { .. }))
        );
    }

    #[test]
    fn empty_project_validates() {
        let p = Project::new();
        assert!(p.validate().is_ok());
    }
}
