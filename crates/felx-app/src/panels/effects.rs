//! Effects panel — shown for the currently selected layer. Auto-generates
//! controls from each effect's manifest.

use crate::curve_widget::{CurveAction, draw_curve_widget};
use crate::manifests::ManifestRegistry;
use egui::{CollapsingHeader, Color32, RichText, Ui};
use felx_core::model::{Curve, InterpKind, Keyframe, Layer, Rational};
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
    /// Append a new effect (built from its manifest defaults) to the layer.
    AddEffect { effect_id: String },
    /// Remove the effect at `effect_index`.
    RemoveEffect { effect_index: usize },
    /// Reorder: move `effect_index` up (earlier in the chain).
    MoveUp { effect_index: usize },
    /// Reorder: move `effect_index` down (later in the chain).
    MoveDown { effect_index: usize },
}

pub fn show(
    ui: &mut Ui,
    registry: &ManifestRegistry,
    layer: Option<&Layer>,
    time: Rational,
) -> Vec<EffectsAction> {
    let mut actions = Vec::new();

    ui.horizontal(|ui| {
        ui.heading("Effects");
        if layer.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_add_effect_menu(ui, registry, &mut actions);
            });
        }
    });
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
            RichText::new("(no effects on this layer — use \"+ Effect\" above to add one)")
                .color(Color32::from_gray(120))
                .italics(),
        );
        return actions;
    }

    let n = layer.effects.len();
    for (idx, eff) in layer.effects.iter().enumerate() {
        let header_label = registry
            .get(&eff.id)
            .map(|m| m.display_name.as_str())
            .unwrap_or(eff.id.as_str());

        ui.horizontal(|ui| {
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

                    render_param_list(
                        ui,
                        idx,
                        &manifest.parameters,
                        "",
                        &eff.values,
                        time,
                        &mut actions,
                    );
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Remove this effect")
                    .clicked()
                {
                    actions.push(EffectsAction::RemoveEffect { effect_index: idx });
                }
                if idx + 1 < n
                    && ui
                        .small_button("▼")
                        .on_hover_text("Move later in chain")
                        .clicked()
                {
                    actions.push(EffectsAction::MoveDown { effect_index: idx });
                }
                if idx > 0
                    && ui
                        .small_button("▲")
                        .on_hover_text("Move earlier in chain")
                        .clicked()
                {
                    actions.push(EffectsAction::MoveUp { effect_index: idx });
                }
            });
        });
    }

    actions
}

fn render_add_effect_menu(
    ui: &mut Ui,
    registry: &ManifestRegistry,
    actions: &mut Vec<EffectsAction>,
) {
    ui.menu_button("+ Effect", |ui| {
        if registry.len() == 0 {
            ui.label(
                RichText::new("(no manifests loaded)")
                    .color(Color32::from_gray(120))
                    .italics(),
            );
            return;
        }
        for manifest in registry.iter_ordered() {
            let label = format!("{}  —  {}", manifest.display_name, manifest.category);
            if ui.button(label).clicked() {
                actions.push(EffectsAction::AddEffect {
                    effect_id: manifest.id.clone(),
                });
                ui.close();
            }
        }
    });
}

fn render_param_list(
    ui: &mut Ui,
    effect_index: usize,
    params: &[ParamDecl],
    prefix: &str,
    values: &ParamValues,
    time: Rational,
    actions: &mut Vec<EffectsAction>,
) {
    for p in params {
        let id = if prefix.is_empty() {
            p.id.clone()
        } else {
            format!("{prefix}.{}", p.id)
        };
        render_param(ui, effect_index, &id, p, values, time, actions);
    }
}

fn render_param(
    ui: &mut Ui,
    effect_index: usize,
    id: &str,
    decl: &ParamDecl,
    values: &ParamValues,
    time: Rational,
    actions: &mut Vec<EffectsAction>,
) {
    match &decl.kind {
        ParamKind::Float { default, range } => {
            render_float_param(
                ui,
                effect_index,
                id,
                decl,
                *default,
                range.min,
                range.max,
                values,
                time,
                actions,
            );
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
                    render_param_list(ui, effect_index, parameters, id, values, time, actions);
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
                    render_param_list(ui, effect_index, parameters, id, values, time, actions);
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_float_param(
    ui: &mut Ui,
    effect_index: usize,
    id: &str,
    decl: &ParamDecl,
    default: f32,
    min: f32,
    max: f32,
    values: &ParamValues,
    time: Rational,
    actions: &mut Vec<EffectsAction>,
) {
    let stored = values.get(id);
    let curve_view = stored.and_then(|v| v.as_float_curve());
    let is_animated = curve_view.is_some();
    let current = match stored {
        Some(ParamValue::Float(f)) => *f,
        Some(ParamValue::FloatCurve(c)) => c.sample_at_time(time),
        _ => default,
    };

    ui.horizontal(|ui| {
        // Stopwatch toggle: convert static ↔ animated. When converting to
        // animated we seed the curve with a single keyframe at the playhead.
        let stopwatch_label = if is_animated { "⏱" } else { "○" };
        let stopwatch_resp = ui
            .small_button(stopwatch_label)
            .on_hover_text(if is_animated {
                "remove animation (revert to static)"
            } else {
                "animate this parameter (add a keyframe)"
            });
        if stopwatch_resp.clicked() {
            let new_value = if is_animated {
                ParamValue::Float(current)
            } else {
                ParamValue::FloatCurve(Curve::Animated(vec![Keyframe {
                    t: time,
                    v: current,
                    interp: InterpKind::Linear,
                }]))
            };
            actions.push(EffectsAction::SetValue {
                effect_index,
                id: id.to_string(),
                value: new_value,
            });
        }

        let mut v = current;
        let slider = ui.add(egui::Slider::new(&mut v, min..=max).text(decl.display_name.as_str()));
        if slider.changed() {
            // For an animated curve, dragging the slider sets the value at
            // the playhead — adding or replacing a keyframe at `time`.
            let new_value = if let Some(c) = curve_view {
                ParamValue::FloatCurve(set_keyframe_at(c, time, v))
            } else {
                ParamValue::Float(v)
            };
            actions.push(EffectsAction::SetValue {
                effect_index,
                id: id.to_string(),
                value: new_value,
            });
        }
    });

    if let Some(c) = curve_view {
        let widget_id = ui
            .id()
            .with(("curve_widget", effect_index, id))
            .value()
            .to_string();
        let action = draw_curve_widget(ui, &widget_id, c, time, min, max);
        if let Some(act) = action {
            let new_curve = apply_curve_action(c, act);
            actions.push(EffectsAction::SetValue {
                effect_index,
                id: id.to_string(),
                value: ParamValue::FloatCurve(new_curve),
            });
        }
    }
}

fn set_keyframe_at(curve: &Curve<f32>, time: Rational, value: f32) -> Curve<f32> {
    let mut kfs = match curve {
        Curve::Static(v) => vec![Keyframe {
            t: time,
            v: *v,
            interp: InterpKind::Linear,
        }],
        Curve::Animated(kfs) => kfs.clone(),
    };
    let target_secs = time.as_seconds();
    if let Some(existing) = kfs
        .iter_mut()
        .find(|k| (k.t.as_seconds() - target_secs).abs() < 1e-9)
    {
        existing.v = value;
    } else {
        kfs.push(Keyframe {
            t: time,
            v: value,
            interp: InterpKind::Linear,
        });
        kfs.sort_by(|a, b| {
            a.t.as_seconds()
                .partial_cmp(&b.t.as_seconds())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    Curve::Animated(kfs)
}

fn apply_curve_action(curve: &Curve<f32>, action: CurveAction) -> Curve<f32> {
    let mut kfs: Vec<Keyframe<f32>> = match curve {
        Curve::Static(v) => vec![Keyframe {
            t: Rational::new(0, 30),
            v: *v,
            interp: InterpKind::Linear,
        }],
        Curve::Animated(k) => k.clone(),
    };
    match action {
        CurveAction::AddAt { time, value } => {
            kfs.push(Keyframe {
                t: time,
                v: value,
                interp: InterpKind::Linear,
            });
            kfs.sort_by(|a, b| {
                a.t.as_seconds()
                    .partial_cmp(&b.t.as_seconds())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        CurveAction::Delete { index } => {
            if kfs.len() > 1 && index < kfs.len() {
                kfs.remove(index);
            }
        }
        CurveAction::SetInterp { index, interp } => {
            if let Some(k) = kfs.get_mut(index) {
                k.interp = interp;
            }
        }
        CurveAction::Move { index, time, value } => {
            if let Some(k) = kfs.get_mut(index) {
                k.t = time;
                k.v = value;
            }
            kfs.sort_by(|a, b| {
                a.t.as_seconds()
                    .partial_cmp(&b.t.as_seconds())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
    Curve::Animated(kfs)
}

// Suppress an unused-import lint when the host doesn't reference EffectManifest
// directly through this module.
#[allow(dead_code)]
fn _force_use_manifest(_: &EffectManifest) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keyframe_replaces_value_at_existing_time() {
        let c = Curve::Animated(vec![
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
        ]);
        let updated = set_keyframe_at(&c, Rational::new(0, 30), 0.5);
        let Curve::Animated(kfs) = updated else {
            panic!()
        };
        assert_eq!(kfs.len(), 2);
        assert_eq!(kfs[0].v, 0.5);
    }

    #[test]
    fn set_keyframe_inserts_new_time_in_sorted_order() {
        let c = Curve::Animated(vec![
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
        ]);
        let updated = set_keyframe_at(&c, Rational::new(30, 30), 0.7);
        let Curve::Animated(kfs) = updated else {
            panic!()
        };
        assert_eq!(kfs.len(), 3);
        assert_eq!(kfs[1].t, Rational::new(30, 30));
        assert_eq!(kfs[1].v, 0.7);
    }

    #[test]
    fn delete_keyframe_keeps_at_least_one() {
        let c = Curve::Animated(vec![Keyframe {
            t: Rational::new(0, 30),
            v: 0.0,
            interp: InterpKind::Linear,
        }]);
        let updated = apply_curve_action(&c, CurveAction::Delete { index: 0 });
        let Curve::Animated(kfs) = updated else {
            panic!()
        };
        assert_eq!(kfs.len(), 1, "must not delete the last keyframe");
    }
}
