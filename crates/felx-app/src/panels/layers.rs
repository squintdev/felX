//! Layer panel: list, select, add solid, delete, move up/down.

use egui::{Color32, RichText, ScrollArea, Ui};
use felx_core::model::{Composition, LayerId, LayerKind};

/// Edit operations the layer panel emits, applied by the host app on the
/// composition. We return them as values so the panel can render with an
/// immutable comp borrow and the app can mutate after.
#[derive(Clone, Debug)]
pub enum LayerAction {
    Select(Option<LayerId>),
    AddSolid,
    AddAdjustment,
    /// Open a file dialog and import the chosen file as an Image asset +
    /// Image layer.
    ImportImage,
    /// Open a file dialog and import the chosen file as a Video asset +
    /// Video layer.
    ImportVideo,
    /// Open a file dialog and import the chosen file as an Audio asset +
    /// Audio layer.
    ImportAudio,
    Delete(LayerId),
    MoveUp(LayerId),
    MoveDown(LayerId),
    SetTimeOffset(LayerId, i32),
    SetTimeScale(LayerId, f32),
    /// Edit the active composition's settings.
    SetCompWidth(u32),
    SetCompHeight(u32),
    /// Framerate as (numerator, denominator). 30/1 = 30fps, 24000/1001 = 23.976.
    SetCompFramerate(u32, u32),
    SetCompDurationFrames(u32),
    SetCompBackground([f32; 4]),
}

pub fn show(ui: &mut Ui, comp: &Composition, selected: Option<LayerId>) -> Vec<LayerAction> {
    let mut actions = Vec::new();

    ui.horizontal(|ui| {
        ui.heading("Layers");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("+ Adj")
                .on_hover_text("Add Adjustment layer")
                .clicked()
            {
                actions.push(LayerAction::AddAdjustment);
            }
            if ui.button("+ Solid").clicked() {
                actions.push(LayerAction::AddSolid);
            }
        });
    });
    ui.horizontal(|ui| {
        if ui
            .small_button("📁 Image")
            .on_hover_text("Import an image file as a new Image layer")
            .clicked()
        {
            actions.push(LayerAction::ImportImage);
        }
        if ui
            .small_button("📁 Video")
            .on_hover_text("Import a video file (Audio layer is added too if it has sound)")
            .clicked()
        {
            actions.push(LayerAction::ImportVideo);
        }
        if ui
            .small_button("📁 Audio")
            .on_hover_text("Import an audio file as a new Audio layer")
            .clicked()
        {
            actions.push(LayerAction::ImportAudio);
        }
    });
    ui.separator();

    // Composition properties. Lets the user change the canvas / framerate /
    // duration without having to hand-edit the .felx.
    ui.collapsing("Composition", |ui| {
        let mut w = comp.width;
        let mut h = comp.height;
        let mut dur = comp.duration_frames;
        let mut fps_num = comp.framerate.0.num;
        let mut fps_den = comp.framerate.0.den;
        let mut bg = comp.background;

        ui.horizontal(|ui| {
            ui.label("width");
            if ui
                .add(egui::DragValue::new(&mut w).speed(2.0).range(1..=8192))
                .changed()
            {
                actions.push(LayerAction::SetCompWidth(w));
            }
            ui.label("height");
            if ui
                .add(egui::DragValue::new(&mut h).speed(2.0).range(1..=8192))
                .changed()
            {
                actions.push(LayerAction::SetCompHeight(h));
            }
        });
        ui.horizontal(|ui| {
            ui.label("fps");
            let mut changed = false;
            changed |= ui
                .add(
                    egui::DragValue::new(&mut fps_num)
                        .speed(0.1)
                        .range(1..=240000),
                )
                .on_hover_text("numerator")
                .changed();
            ui.label("/");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut fps_den)
                        .speed(0.1)
                        .range(1..=1001),
                )
                .on_hover_text("denominator (1 for whole-number fps, 1001 for NTSC fractional)")
                .changed();
            if changed {
                actions.push(LayerAction::SetCompFramerate(fps_num, fps_den));
            }
        });
        ui.horizontal(|ui| {
            ui.label("duration (frames)");
            if ui
                .add(
                    egui::DragValue::new(&mut dur)
                        .speed(1.0)
                        .range(1..=u32::MAX),
                )
                .changed()
            {
                actions.push(LayerAction::SetCompDurationFrames(dur));
            }
            let secs = dur as f64 / (fps_num as f64 / fps_den as f64);
            ui.label(
                RichText::new(format!("≈ {:.2}s", secs))
                    .color(Color32::from_gray(160))
                    .small(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("background");
            if ui.color_edit_button_rgba_premultiplied(&mut bg).changed() {
                actions.push(LayerAction::SetCompBackground(bg));
            }
        });
    });
    ui.separator();

    if comp.layers.is_empty() {
        ui.label(
            RichText::new("(no layers)")
                .color(Color32::from_gray(120))
                .italics(),
        );
        return actions;
    }

    // Selected-layer properties strip (time remap). Only meaningful for
    // time-driven kinds; for static kinds we still show the controls so
    // users can tell they're available, but they're functionally no-ops.
    if let Some(sel_id) = selected
        && let Some(layer) = comp.layer(sel_id)
    {
        ui.collapsing("Time remap", |ui| {
            let mut offset = layer.time_offset_frames;
            let mut scale = layer.time_scale;
            ui.horizontal(|ui| {
                ui.label("offset (frames)");
                if ui
                    .add(egui::DragValue::new(&mut offset).speed(0.5))
                    .changed()
                {
                    actions.push(LayerAction::SetTimeOffset(sel_id, offset));
                }
            });
            ui.horizontal(|ui| {
                ui.label("scale");
                if ui
                    .add(
                        egui::DragValue::new(&mut scale)
                            .speed(0.05)
                            .range(-8.0..=8.0),
                    )
                    .changed()
                {
                    actions.push(LayerAction::SetTimeScale(sel_id, scale));
                }
            });
            if !matches!(
                layer.kind,
                LayerKind::Composition { .. } | LayerKind::Video { .. }
            ) {
                ui.label(
                    RichText::new("(only affects Composition / Video layers)")
                        .color(Color32::from_gray(120))
                        .small()
                        .italics(),
                );
            }
        });
        ui.separator();
    }

    ScrollArea::vertical().show(ui, |ui| {
        // Top of list = top of stack visually; the model orders bottom→top
        // (last layer in Vec composites last / on top of others). Render in
        // reverse so the topmost layer appears first in the panel.
        for layer in comp.layers.iter().rev() {
            let is_selected = selected == Some(layer.id);
            let row = ui.horizontal(|ui| {
                let label = format!("{}  {}", kind_glyph(&layer.kind), layer.name);
                if ui
                    .selectable_label(is_selected, RichText::new(label).monospace())
                    .clicked()
                {
                    actions.push(LayerAction::Select(if is_selected {
                        None
                    } else {
                        Some(layer.id)
                    }));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text("Delete layer").clicked() {
                        actions.push(LayerAction::Delete(layer.id));
                    }
                    if ui.small_button("▼").on_hover_text("Move down").clicked() {
                        actions.push(LayerAction::MoveDown(layer.id));
                    }
                    if ui.small_button("▲").on_hover_text("Move up").clicked() {
                        actions.push(LayerAction::MoveUp(layer.id));
                    }
                });
            });
            let _ = row.response;
        }
    });

    actions
}

fn kind_glyph(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Video { .. } => "▶",
        LayerKind::Image { .. } => "◇",
        LayerKind::Audio { .. } => "♪",
        LayerKind::Solid { .. } => "■",
        LayerKind::Null => "○",
        LayerKind::Adjustment => "⚙",
        LayerKind::Composition { .. } => "⊞",
    }
}
