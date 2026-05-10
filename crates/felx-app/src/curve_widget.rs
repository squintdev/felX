//! Inline mini curve editor — a compact horizontal strip with keyframe dots
//! and the interpolated curve. Lives below an animated Float parameter in
//! the effects panel. F-036 brings the dedicated graph-view editor with
//! arbitrary bezier-tangent dragging; this widget is intentionally minimal.

use egui::{Color32, Pos2, Sense, Stroke, StrokeKind, Ui, Vec2};
use felx_core::model::{Curve, InterpKind, Rational};

const HIT_RADIUS: f32 = 6.0;
const KEYFRAME_RADIUS: f32 = 3.5;
const WIDGET_HEIGHT: f32 = 56.0;

#[derive(Clone, Debug)]
pub enum CurveAction {
    AddAt {
        time: Rational,
        value: f32,
    },
    Delete {
        index: usize,
    },
    SetInterp {
        index: usize,
        interp: InterpKind,
    },
    Move {
        index: usize,
        time: Rational,
        value: f32,
    },
}

/// Draw the widget. Returns at most one action per frame.
///
/// Layout: the X axis spans `[curve_start, max(curve_end, time)]` so the
/// playhead is always on-screen; the Y axis spans `[min, max]` from the
/// parameter manifest. A vertical line marks the playhead.
pub fn draw_curve_widget(
    ui: &mut Ui,
    salt: &str,
    curve: &Curve<f32>,
    playhead: Rational,
    min: f32,
    max: f32,
) -> Option<CurveAction> {
    let kfs = match curve {
        Curve::Animated(k) if !k.is_empty() => k.clone(),
        _ => return None,
    };

    // Determine X range. Always include t=0 and the playhead so the user can
    // see "before the first keyframe" and the live cursor.
    let mut t_min = 0.0_f64;
    let mut t_max = playhead.as_seconds().max(1.0);
    for k in &kfs {
        let s = k.t.as_seconds();
        t_min = t_min.min(s);
        t_max = t_max.max(s);
    }
    if (t_max - t_min) < 1e-6 {
        t_max = t_min + 1.0;
    }
    let y_lo = min;
    let y_hi = max;
    let y_range = (y_hi - y_lo).max(1e-6);

    let avail = ui.available_width();
    let desired = Vec2::new(avail, WIDGET_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());

    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        2.0,
        Color32::from_gray(28),
        Stroke::new(1.0, Color32::from_gray(60)),
        StrokeKind::Inside,
    );

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

    // Draw the interpolated curve as a polyline. ~64 samples is enough for
    // a thumbnail; the strip is only ~50px tall.
    let samples = 64;
    let mut prev: Option<Pos2> = None;
    for i in 0..=samples {
        let u = i as f64 / samples as f64;
        let secs = t_min + u * (t_max - t_min);
        let den = 1_000_000u32;
        let num = (secs * den as f64).round().max(0.0) as u32;
        let t = Rational::new(num, den);
        let v = sample_curve(&kfs, t);
        let p = Pos2::new(to_x(secs), to_y(v));
        if let Some(prev_p) = prev {
            painter.line_segment(
                [prev_p, p],
                Stroke::new(1.5, Color32::from_rgb(150, 200, 255)),
            );
        }
        prev = Some(p);
    }

    // Playhead marker.
    let ph_x = to_x(playhead.as_seconds());
    painter.line_segment(
        [Pos2::new(ph_x, rect.top()), Pos2::new(ph_x, rect.bottom())],
        Stroke::new(1.0, Color32::from_rgb(255, 200, 80)),
    );

    // Keyframe dots. Hit-test for hover/right-click.
    let mut hovered_index: Option<usize> = None;
    for (i, k) in kfs.iter().enumerate() {
        let p = Pos2::new(to_x(k.t.as_seconds()), to_y(k.v));
        let color = match k.interp {
            InterpKind::Hold => Color32::from_rgb(220, 220, 100),
            InterpKind::Linear => Color32::from_rgb(180, 220, 180),
            InterpKind::EaseIn => Color32::from_rgb(180, 200, 240),
            InterpKind::EaseOut => Color32::from_rgb(220, 180, 240),
            InterpKind::EaseInOut => Color32::from_rgb(240, 180, 200),
        };
        painter.circle_filled(p, KEYFRAME_RADIUS, color);
        painter.circle_stroke(p, KEYFRAME_RADIUS, Stroke::new(1.0, Color32::BLACK));
        if let Some(hover_pos) = response.hover_pos()
            && hover_pos.distance(p) <= HIT_RADIUS
        {
            hovered_index = Some(i);
        }
    }

    // Hover hint at the bottom-left.
    if let Some(idx) = hovered_index {
        let k = &kfs[idx];
        let label = format!("kf {}: t={:.2}s v={:.3}", idx, k.t.as_seconds(), k.v);
        painter.text(
            rect.left_top() + Vec2::new(4.0, 2.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::monospace(10.0),
            Color32::from_gray(200),
        );
    }

    // Drag state machine. Persist the dragging keyframe index across frames
    // via egui's id-keyed memory.
    let drag_id = egui::Id::new(("curve_drag", salt));
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

    let mut action: Option<CurveAction> = None;

    if let Some(idx) = dragging
        && response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let (t, v) = from_xy(pos);
        action = Some(CurveAction::Move {
            index: idx,
            time: t,
            value: v,
        });
    }

    // Right-click context menu on a keyframe: delete + interp picker.
    response.context_menu(|ui| {
        if let Some(idx) = hovered_index {
            ui.label(format!("Keyframe {}", idx));
            ui.separator();
            for kind in [
                InterpKind::Hold,
                InterpKind::Linear,
                InterpKind::EaseIn,
                InterpKind::EaseOut,
                InterpKind::EaseInOut,
            ] {
                if ui
                    .selectable_label(kfs[idx].interp == kind, format!("{:?}", kind))
                    .clicked()
                {
                    action = Some(CurveAction::SetInterp {
                        index: idx,
                        interp: kind,
                    });
                    ui.close();
                }
            }
            ui.separator();
            if kfs.len() > 1 && ui.button("Delete").clicked() {
                action = Some(CurveAction::Delete { index: idx });
                ui.close();
            }
        } else if let Some(pos) = response.hover_pos() {
            let (t, v) = from_xy(pos);
            if ui
                .button(format!("Add keyframe @ {:.2}s = {:.3}", t.as_seconds(), v))
                .clicked()
            {
                action = Some(CurveAction::AddAt { time: t, value: v });
                ui.close();
            }
        }
    });

    // Double-click on empty space → add keyframe at click pos.
    if response.double_clicked()
        && hovered_index.is_none()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let (t, v) = from_xy(pos);
        action = Some(CurveAction::AddAt { time: t, value: v });
    }

    action
}

fn sample_curve(kfs: &[felx_core::model::Keyframe<f32>], time: Rational) -> f32 {
    Curve::Animated(kfs.to_vec()).sample_at_time(time)
}

#[cfg(test)]
mod tests {
    // The widget needs egui::Context to test interactively; this module
    // just relies on cargo build / clippy to catch breakage. The host's
    // panels/effects.rs has tests for the action-application logic.
}
