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
            .on_hover_text("Import a video file as a new Video layer")
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
