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
    Delete(LayerId),
    MoveUp(LayerId),
    MoveDown(LayerId),
}

pub fn show(ui: &mut Ui, comp: &Composition, selected: Option<LayerId>) -> Vec<LayerAction> {
    let mut actions = Vec::new();

    ui.horizontal(|ui| {
        ui.heading("Layers");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("+ Solid").clicked() {
                actions.push(LayerAction::AddSolid);
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
