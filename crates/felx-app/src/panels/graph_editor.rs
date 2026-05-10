//! Graph editor — bottom panel showing all animated Float parameters of the
//! selected layer's effect stack, one row per parameter. Supports drag-to-
//! retime/revalue, shift-click multi-select, Delete-key bulk remove, and a
//! right-click interp picker applied to the whole selection.
//!
//! Arbitrary in/out bezier tangent editing is deferred — the keyframe data
//! model uses [`InterpKind`] presets (Hold / Linear / EaseIn / EaseOut /
//! EaseInOut), not free tangents. That schema change would ripple through
//! the project file format and is out of scope here.

use crate::panels::effects::EffectsAction;
use egui::{Color32, Key, Pos2, Sense, Stroke, StrokeKind, Ui, Vec2};
use felx_core::model::{Curve, InterpKind, Keyframe, Layer, Rational};
use felx_core::params::{ParamDecl, ParamKind, ParamValue, ParamValues};
use std::collections::HashSet;

const HIT_RADIUS: f32 = 7.0;
const KEYFRAME_RADIUS: f32 = 4.5;
const ROW_HEIGHT: f32 = 96.0;
const ROW_SPACING: f32 = 8.0;
const LABEL_WIDTH: f32 = 160.0;

/// (effect_index, dotted_param_id, keyframe_index)
type SelKey = (usize, String, usize);

#[derive(Clone, Debug, Default)]
struct GraphSelection {
    keys: HashSet<SelKey>,
}

pub fn show(
    ui: &mut Ui,
    layer: Option<&Layer>,
    manifests: &crate::manifests::ManifestRegistry,
    playhead: Rational,
    duration_secs: f64,
) -> Vec<EffectsAction> {
    let mut actions: Vec<EffectsAction> = Vec::new();

    let Some(layer) = layer else {
        ui.label(
            egui::RichText::new("(select a layer to see its animated parameters)")
                .color(Color32::from_gray(120))
                .italics(),
        );
        return actions;
    };

    let rows = collect_animated_rows(layer, manifests);
    if rows.is_empty() {
        ui.label(
            egui::RichText::new(
                "(no animated parameters — toggle the ⏱ stopwatch on a Float \
                 slider in the Effects panel to start animating)",
            )
            .color(Color32::from_gray(120))
            .italics(),
        );
        return actions;
    }

    let sel_id = egui::Id::new("graph_editor_sel");
    let mut selection: GraphSelection = ui
        .ctx()
        .memory(|m| m.data.get_temp(sel_id).unwrap_or_default());
    let shift_held = ui.input(|i| i.modifiers.shift);
    let delete_pressed = ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));

    for row in &rows {
        actions.extend(draw_row(
            ui,
            row,
            playhead,
            duration_secs,
            shift_held,
            &mut selection,
        ));
    }

    // Bulk delete via Delete / Backspace.
    if delete_pressed && !selection.keys.is_empty() {
        actions.extend(bulk_delete(&selection, &rows));
        selection.keys.clear();
    }

    ui.ctx()
        .memory_mut(|m| m.data.insert_temp(sel_id, selection));
    actions
}

#[derive(Clone)]
struct AnimatedRow {
    effect_index: usize,
    effect_label: String,
    param_id: String,
    param_label: String,
    min: f32,
    max: f32,
    curve: Curve<f32>,
}

fn collect_animated_rows(
    layer: &Layer,
    manifests: &crate::manifests::ManifestRegistry,
) -> Vec<AnimatedRow> {
    let mut out = Vec::new();
    for (idx, eff) in layer.effects.iter().enumerate() {
        let manifest = match manifests.get(&eff.id) {
            Some(m) => m,
            None => continue,
        };
        collect_from_params(
            idx,
            &eff.id,
            &manifest.parameters,
            "",
            &eff.values,
            &mut out,
        );
    }
    out
}

fn collect_from_params(
    effect_index: usize,
    effect_label: &str,
    params: &[ParamDecl],
    prefix: &str,
    values: &ParamValues,
    out: &mut Vec<AnimatedRow>,
) {
    for p in params {
        let id = if prefix.is_empty() {
            p.id.clone()
        } else {
            format!("{prefix}.{}", p.id)
        };
        match &p.kind {
            ParamKind::Float { range, .. } => {
                if let Some(curve) = values.float_curve(&id) {
                    out.push(AnimatedRow {
                        effect_index,
                        effect_label: effect_label.to_string(),
                        param_id: id.clone(),
                        param_label: p.display_name.clone(),
                        min: range.min,
                        max: range.max,
                        curve: curve.clone(),
                    });
                }
            }
            ParamKind::Group { parameters } => {
                collect_from_params(effect_index, effect_label, parameters, &id, values, out);
            }
            ParamKind::OptionalGroup { parameters, .. }
                if values.group_enabled(&id).unwrap_or(false) =>
            {
                collect_from_params(effect_index, effect_label, parameters, &id, values, out);
            }
            _ => {}
        }
    }
}

fn draw_row(
    ui: &mut Ui,
    row: &AnimatedRow,
    playhead: Rational,
    duration_secs: f64,
    shift_held: bool,
    selection: &mut GraphSelection,
) -> Vec<EffectsAction> {
    let mut actions = Vec::new();

    let kfs: Vec<Keyframe<f32>> = match &row.curve {
        Curve::Animated(k) => k.clone(),
        Curve::Static(v) => vec![Keyframe {
            t: Rational::new(0, 30),
            v: *v,
            interp: InterpKind::Linear,
        }],
    };

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(LABEL_WIDTH, ROW_HEIGHT),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(
                    egui::RichText::new(&row.effect_label)
                        .small()
                        .color(Color32::from_gray(150)),
                );
                ui.label(egui::RichText::new(&row.param_label).strong());
                ui.label(
                    egui::RichText::new(format!("{:.2} … {:.2}", row.min, row.max))
                        .small()
                        .color(Color32::from_gray(150)),
                );
            },
        );

        let avail = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(avail, ROW_HEIGHT), Sense::click_and_drag());

        let painter = ui.painter_at(rect);
        painter.rect(
            rect,
            2.0,
            Color32::from_gray(22),
            Stroke::new(1.0, Color32::from_gray(60)),
            StrokeKind::Inside,
        );

        let t_min = 0.0_f64;
        let t_max = duration_secs.max(playhead.as_seconds()).max(1.0);
        let y_lo = row.min;
        let y_hi = row.max;
        let y_range = (y_hi - y_lo).max(1e-6);

        let to_x = |t: f64| -> f32 {
            let u = ((t - t_min) / (t_max - t_min)) as f32;
            rect.left() + u.clamp(0.0, 1.0) * rect.width()
        };
        let to_y = |v: f32| -> f32 {
            let u = ((v - y_lo) / y_range).clamp(0.0, 1.0);
            rect.bottom() - u * rect.height()
        };
        let from_xy = |p: Pos2| -> (Rational, f32) {
            let u_x = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let u_y = ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0);
            let secs = t_min + (u_x as f64) * (t_max - t_min);
            let den: u32 = 1_000_000;
            let num = (secs * den as f64).round().max(0.0) as u32;
            let value = y_lo + u_y * y_range;
            (Rational::new(num, den), value)
        };

        // Curve polyline.
        let samples = 128;
        let mut prev: Option<Pos2> = None;
        for i in 0..=samples {
            let u = i as f64 / samples as f64;
            let secs = t_min + u * (t_max - t_min);
            let den = 1_000_000u32;
            let num = (secs * den as f64).round().max(0.0) as u32;
            let t = Rational::new(num, den);
            let v = Curve::Animated(kfs.clone()).sample_at_time(t);
            let p = Pos2::new(to_x(secs), to_y(v));
            if let Some(prev_p) = prev {
                painter.line_segment(
                    [prev_p, p],
                    Stroke::new(1.5, Color32::from_rgb(140, 200, 255)),
                );
            }
            prev = Some(p);
        }

        // Playhead.
        let ph_x = to_x(playhead.as_seconds());
        painter.line_segment(
            [Pos2::new(ph_x, rect.top()), Pos2::new(ph_x, rect.bottom())],
            Stroke::new(1.0, Color32::from_rgb(255, 200, 80)),
        );

        // Hover hit-test.
        let mut hovered_index: Option<usize> = None;
        for (i, k) in kfs.iter().enumerate() {
            let p = Pos2::new(to_x(k.t.as_seconds()), to_y(k.v));
            if let Some(hp) = response.hover_pos()
                && hp.distance(p) <= HIT_RADIUS
            {
                hovered_index = Some(i);
            }
        }

        // Drag state per row.
        let drag_id = egui::Id::new(("graph_drag", row.effect_index, row.param_id.clone()));
        let mut dragging: Option<usize> = ui.ctx().memory(|m| m.data.get_temp(drag_id));

        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            for (i, k) in kfs.iter().enumerate() {
                let p = Pos2::new(to_x(k.t.as_seconds()), to_y(k.v));
                if pos.distance(p) <= HIT_RADIUS {
                    dragging = Some(i);
                    ui.ctx().memory_mut(|m| m.data.insert_temp(drag_id, i));
                    break;
                }
            }
        }
        if response.drag_stopped() {
            dragging = None;
            ui.ctx().memory_mut(|m| m.data.remove::<usize>(drag_id));
        }

        // Click selection (single or shift-add).
        if response.clicked()
            && let Some(idx) = hovered_index
        {
            let key: SelKey = (row.effect_index, row.param_id.clone(), idx);
            if shift_held {
                if !selection.keys.insert(key.clone()) {
                    selection.keys.remove(&key);
                }
            } else {
                selection.keys.clear();
                selection.keys.insert(key);
            }
        }

        // Drag → emit Move action; replace the entire curve since a re-sort
        // may shift indices.
        if let Some(idx) = dragging
            && response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let (t, v) = from_xy(pos);
            let mut new_kfs = kfs.clone();
            if let Some(k) = new_kfs.get_mut(idx) {
                k.t = t;
                k.v = v;
            }
            new_kfs.sort_by(|a, b| {
                a.t.as_seconds()
                    .partial_cmp(&b.t.as_seconds())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            actions.push(EffectsAction::SetValue {
                effect_index: row.effect_index,
                id: row.param_id.clone(),
                value: ParamValue::FloatCurve(Curve::Animated(new_kfs)),
            });
        }

        // Draw dots last so they're on top.
        for (i, k) in kfs.iter().enumerate() {
            let p = Pos2::new(to_x(k.t.as_seconds()), to_y(k.v));
            let key: SelKey = (row.effect_index, row.param_id.clone(), i);
            let is_sel = selection.keys.contains(&key);
            let base = match k.interp {
                InterpKind::Hold => Color32::from_rgb(220, 220, 100),
                InterpKind::Linear => Color32::from_rgb(180, 220, 180),
                InterpKind::EaseIn => Color32::from_rgb(180, 200, 240),
                InterpKind::EaseOut => Color32::from_rgb(220, 180, 240),
                InterpKind::EaseInOut => Color32::from_rgb(240, 180, 200),
            };
            painter.circle_filled(p, KEYFRAME_RADIUS, base);
            let outline = if is_sel {
                Stroke::new(2.0, Color32::from_rgb(255, 220, 80))
            } else {
                Stroke::new(1.0, Color32::BLACK)
            };
            painter.circle_stroke(p, KEYFRAME_RADIUS, outline);
        }

        // Right-click: interp picker on the selection (or just the hovered
        // keyframe if nothing is selected).
        let mut effective: Vec<SelKey> = selection
            .keys
            .iter()
            .filter(|(eidx, pid, _)| *eidx == row.effect_index && pid == &row.param_id)
            .cloned()
            .collect();
        if effective.is_empty()
            && let Some(i) = hovered_index
        {
            effective.push((row.effect_index, row.param_id.clone(), i));
        }

        response.context_menu(|ui| {
            if effective.is_empty() {
                ui.label("(click a keyframe first)");
                return;
            }
            ui.label(format!("{} keyframe(s)", effective.len()));
            ui.separator();
            for kind in [
                InterpKind::Hold,
                InterpKind::Linear,
                InterpKind::EaseIn,
                InterpKind::EaseOut,
                InterpKind::EaseInOut,
            ] {
                if ui.button(format!("{:?}", kind)).clicked() {
                    let mut new_kfs = kfs.clone();
                    for (_, _, i) in &effective {
                        if let Some(k) = new_kfs.get_mut(*i) {
                            k.interp = kind;
                        }
                    }
                    actions.push(EffectsAction::SetValue {
                        effect_index: row.effect_index,
                        id: row.param_id.clone(),
                        value: ParamValue::FloatCurve(Curve::Animated(new_kfs)),
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Delete").clicked() && kfs.len() > effective.len() {
                let to_remove: HashSet<usize> = effective.iter().map(|(_, _, i)| *i).collect();
                let new_kfs: Vec<Keyframe<f32>> = kfs
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !to_remove.contains(i))
                    .map(|(_, k)| k.clone())
                    .collect();
                actions.push(EffectsAction::SetValue {
                    effect_index: row.effect_index,
                    id: row.param_id.clone(),
                    value: ParamValue::FloatCurve(Curve::Animated(new_kfs)),
                });
                ui.close();
            }
        });

        // Double-click empty space to add a keyframe.
        if response.double_clicked()
            && hovered_index.is_none()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let (t, v) = from_xy(pos);
            let mut new_kfs = kfs.clone();
            new_kfs.push(Keyframe {
                t,
                v,
                interp: InterpKind::Linear,
            });
            new_kfs.sort_by(|a, b| {
                a.t.as_seconds()
                    .partial_cmp(&b.t.as_seconds())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            actions.push(EffectsAction::SetValue {
                effect_index: row.effect_index,
                id: row.param_id.clone(),
                value: ParamValue::FloatCurve(Curve::Animated(new_kfs)),
            });
        }
    });
    ui.add_space(ROW_SPACING);

    actions
}

/// Bulk-delete: collapse all selected keyframes by parameter, emit one
/// SetValue per affected curve. Always preserves at least one keyframe.
fn bulk_delete(sel: &GraphSelection, rows: &[AnimatedRow]) -> Vec<EffectsAction> {
    use std::collections::HashMap;
    let mut by_param: HashMap<(usize, String), Vec<usize>> = HashMap::new();
    for (eidx, pid, kf_idx) in &sel.keys {
        by_param
            .entry((*eidx, pid.clone()))
            .or_default()
            .push(*kf_idx);
    }
    let mut out = Vec::new();
    for ((eidx, pid), indices) in by_param {
        let row = rows
            .iter()
            .find(|r| r.effect_index == eidx && r.param_id == pid);
        let Some(row) = row else { continue };
        let kfs = match &row.curve {
            Curve::Animated(k) => k,
            _ => continue,
        };
        let to_remove: HashSet<usize> = indices.into_iter().collect();
        // Always preserve at least one keyframe.
        let remaining: Vec<Keyframe<f32>> = kfs
            .iter()
            .enumerate()
            .filter(|(i, _)| !to_remove.contains(i))
            .map(|(_, k)| k.clone())
            .collect();
        let final_kfs = if remaining.is_empty() {
            vec![kfs[0].clone()]
        } else {
            remaining
        };
        out.push(EffectsAction::SetValue {
            effect_index: eidx,
            id: pid,
            value: ParamValue::FloatCurve(Curve::Animated(final_kfs)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(effect_index: usize, param: &str, kfs: Vec<Keyframe<f32>>) -> AnimatedRow {
        AnimatedRow {
            effect_index,
            effect_label: "x".to_string(),
            param_id: param.to_string(),
            param_label: "X".to_string(),
            min: 0.0,
            max: 1.0,
            curve: Curve::Animated(kfs),
        }
    }

    #[test]
    fn bulk_delete_preserves_at_least_one_keyframe() {
        let r = row(
            0,
            "g",
            vec![Keyframe {
                t: Rational::new(0, 30),
                v: 0.0,
                interp: InterpKind::Linear,
            }],
        );
        let mut sel = GraphSelection::default();
        sel.keys.insert((0, "g".to_string(), 0));
        let actions = bulk_delete(&sel, std::slice::from_ref(&r));
        assert_eq!(actions.len(), 1);
        if let EffectsAction::SetValue {
            value: ParamValue::FloatCurve(Curve::Animated(kfs)),
            ..
        } = &actions[0]
        {
            assert_eq!(kfs.len(), 1, "must keep at least one keyframe");
        } else {
            panic!("expected SetValue with FloatCurve");
        }
    }

    #[test]
    fn bulk_delete_removes_selected_indices() {
        let r = row(
            0,
            "g",
            vec![
                Keyframe {
                    t: Rational::new(0, 30),
                    v: 0.0,
                    interp: InterpKind::Linear,
                },
                Keyframe {
                    t: Rational::new(30, 30),
                    v: 0.5,
                    interp: InterpKind::Linear,
                },
                Keyframe {
                    t: Rational::new(60, 30),
                    v: 1.0,
                    interp: InterpKind::Linear,
                },
            ],
        );
        let mut sel = GraphSelection::default();
        sel.keys.insert((0, "g".to_string(), 1));
        let actions = bulk_delete(&sel, &[r]);
        if let EffectsAction::SetValue {
            value: ParamValue::FloatCurve(Curve::Animated(kfs)),
            ..
        } = &actions[0]
        {
            assert_eq!(kfs.len(), 2);
            assert_eq!(kfs[0].v, 0.0);
            assert_eq!(kfs[1].v, 1.0);
        } else {
            panic!()
        }
    }
}
