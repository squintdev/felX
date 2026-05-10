//! Effects panel — shown for the currently selected layer. Auto-generates
//! controls from each effect's manifest.

use crate::manifests::ManifestRegistry;
use egui::{CollapsingHeader, Color32, RichText, Ui};
use felx_core::model::Layer;
use felx_core::params::{EffectManifest, ParamDecl, ParamKind, ParamValue, ParamValues};

/// Edits emitted by the panel: the host applies these to the project on
/// the next frame.
#[derive(Clone, Debug)]
pub enum EffectsAction {
    /// Replace `effect_index`'s parameter `id` with `value`.
    SetValue {
        effect_index: usize,
        id: String,
        value: ParamValue,
    },
    /// Toggle `effect_index`'s enabled flag.
    ToggleEnabled { effect_index: usize, enabled: bool },
}

pub fn show(ui: &mut Ui, registry: &ManifestRegistry, layer: Option<&Layer>) -> Vec<EffectsAction> {
    let mut actions = Vec::new();

    ui.heading("Effects");
    ui.separator();

    let Some(layer) = layer else {
        ui.label(
            RichText::new("(select a layer)")
                .color(Color32::from_gray(120))
                .italics(),
        );
        return actions;
    };

    if layer.effects.is_empty() {
        ui.label(
            RichText::new("(no effects on this layer)")
                .color(Color32::from_gray(120))
                .italics(),
        );
        return actions;
    }

    for (idx, eff) in layer.effects.iter().enumerate() {
        let header_label = registry
            .get(&eff.id)
            .map(|m| m.display_name.as_str())
            .unwrap_or(eff.id.as_str());

        CollapsingHeader::new(header_label)
            .id_salt(("effect-header", idx))
            .default_open(true)
            .show(ui, |ui| {
                let mut enabled = eff.enabled;
                if ui.checkbox(&mut enabled, "enabled").changed() {
                    actions.push(EffectsAction::ToggleEnabled {
                        effect_index: idx,
                        enabled,
                    });
                }

                let Some(manifest) = registry.get(&eff.id) else {
                    ui.label(
                        RichText::new(format!("(no manifest registered for '{}')", eff.id))
                            .color(Color32::from_gray(120))
                            .italics(),
                    );
                    return;
                };

                render_param_list(ui, idx, &manifest.parameters, "", &eff.values, &mut actions);
            });
    }

    actions
}

fn render_param_list(
    ui: &mut Ui,
    effect_index: usize,
    params: &[ParamDecl],
    prefix: &str,
    values: &ParamValues,
    actions: &mut Vec<EffectsAction>,
) {
    for p in params {
        let id = if prefix.is_empty() {
            p.id.clone()
        } else {
            format!("{prefix}.{}", p.id)
        };
        render_param(ui, effect_index, &id, p, values, actions);
    }
}

fn render_param(
    ui: &mut Ui,
    effect_index: usize,
    id: &str,
    decl: &ParamDecl,
    values: &ParamValues,
    actions: &mut Vec<EffectsAction>,
) {
    match &decl.kind {
        ParamKind::Float { default, range } => {
            let mut v = values.float(id).unwrap_or(*default);
            if ui
                .add(
                    egui::Slider::new(&mut v, range.min..=range.max)
                        .text(decl.display_name.as_str()),
                )
                .changed()
            {
                actions.push(EffectsAction::SetValue {
                    effect_index,
                    id: id.to_string(),
                    value: ParamValue::Float(v),
                });
            }
        }
        ParamKind::Int { default, range } => {
            let mut v = values.int(id).unwrap_or(*default);
            if ui
                .add(
                    egui::Slider::new(&mut v, range.min..=range.max)
                        .text(decl.display_name.as_str()),
                )
                .changed()
            {
                actions.push(EffectsAction::SetValue {
                    effect_index,
                    id: id.to_string(),
                    value: ParamValue::Int(v),
                });
            }
        }
        ParamKind::Bool { default } => {
            let mut v = values.bool(id).unwrap_or(*default);
            if ui.checkbox(&mut v, decl.display_name.as_str()).changed() {
                actions.push(EffectsAction::SetValue {
                    effect_index,
                    id: id.to_string(),
                    value: ParamValue::Bool(v),
                });
            }
        }
        ParamKind::Color { default } => {
            let mut v = values.color(id).unwrap_or(*default);
            ui.horizontal(|ui| {
                ui.label(decl.display_name.as_str());
                if ui.color_edit_button_rgba_premultiplied(&mut v).changed() {
                    actions.push(EffectsAction::SetValue {
                        effect_index,
                        id: id.to_string(),
                        value: ParamValue::Color(v),
                    });
                }
            });
        }
        ParamKind::Vec2 { default } => {
            let mut v = values.vec2(id).unwrap_or(*default);
            ui.horizontal(|ui| {
                ui.label(decl.display_name.as_str());
                let mut changed = false;
                changed |= ui.add(egui::DragValue::new(&mut v[0]).speed(0.1)).changed();
                changed |= ui.add(egui::DragValue::new(&mut v[1]).speed(0.1)).changed();
                if changed {
                    actions.push(EffectsAction::SetValue {
                        effect_index,
                        id: id.to_string(),
                        value: ParamValue::Vec2(v),
                    });
                }
            });
        }
        ParamKind::Enum { variants, default } => {
            let current = values.enum_str(id).unwrap_or(default).to_string();
            let label = variants
                .iter()
                .find(|v| v.id == current)
                .map(|v| v.display_name.as_str())
                .unwrap_or(current.as_str());
            egui::ComboBox::from_label(decl.display_name.as_str())
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for variant in variants {
                        if ui
                            .selectable_label(variant.id == current, variant.display_name.as_str())
                            .clicked()
                            && variant.id != current
                        {
                            actions.push(EffectsAction::SetValue {
                                effect_index,
                                id: id.to_string(),
                                value: ParamValue::Enum(variant.id.clone()),
                            });
                        }
                    }
                });
        }
        ParamKind::Group { parameters } => {
            CollapsingHeader::new(decl.display_name.as_str())
                .id_salt(("group", effect_index, id))
                .default_open(true)
                .show(ui, |ui| {
                    render_param_list(ui, effect_index, parameters, id, values, actions);
                });
        }
        ParamKind::OptionalGroup {
            default_enabled,
            parameters,
        } => {
            let group_enabled_id = id;
            let mut enabled = values
                .group_enabled(group_enabled_id)
                .unwrap_or(*default_enabled);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut enabled, decl.display_name.as_str())
                    .changed()
                {
                    actions.push(EffectsAction::SetValue {
                        effect_index,
                        id: group_enabled_id.to_string(),
                        value: ParamValue::GroupEnabled(enabled),
                    });
                }
            });
            if enabled {
                ui.indent(id, |ui| {
                    render_param_list(ui, effect_index, parameters, id, values, actions);
                });
            }
        }
    }
}

// Suppress an unused-import lint when the host doesn't reference EffectManifest
// directly through this module.
#[allow(dead_code)]
fn _force_use_manifest(_: &EffectManifest) {}
