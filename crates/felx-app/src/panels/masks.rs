//! Masks panel — sub-section of the layers panel for the selected layer.
//! F-061 list/CRUD + per-mask controls. F-066 shape primitives. F-065's
//! pen tool here is "click-to-add-corner only" — interactive bezier-tangent
//! dragging in the viewport is a polish follow-up.

use egui::{Color32, RichText, Ui};
use felx_core::model::{LayerId, Mask, MaskMode, MaskPath, MaskVertex};

#[derive(Clone, Debug)]
pub enum MaskAction {
    AddRectangle(LayerId),
    AddEllipse(LayerId),
    StartPen(LayerId),
    Delete {
        layer: LayerId,
        index: usize,
    },
    SetMode {
        layer: LayerId,
        index: usize,
        mode: MaskMode,
    },
    SetOpacity {
        layer: LayerId,
        index: usize,
        value: f32,
    },
    SetFeather {
        layer: LayerId,
        index: usize,
        value: f32,
    },
    SetExpansion {
        layer: LayerId,
        index: usize,
        value: f32,
    },
}

pub fn show(
    ui: &mut Ui,
    layer_id: LayerId,
    masks: &[Mask],
    pen_active: bool,
    pen_in_progress: usize,
) -> Vec<MaskAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Masks").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Pen")
                .on_hover_text(
                    "Click on the viewer to drop anchor points; click near the first to close",
                )
                .clicked()
            {
                actions.push(MaskAction::StartPen(layer_id));
            }
            if ui
                .small_button("+ Ellipse")
                .on_hover_text("Add an ellipse mask")
                .clicked()
            {
                actions.push(MaskAction::AddEllipse(layer_id));
            }
            if ui
                .small_button("+ Rect")
                .on_hover_text("Add a rectangle mask")
                .clicked()
            {
                actions.push(MaskAction::AddRectangle(layer_id));
            }
        });
    });

    if pen_active {
        ui.label(
            RichText::new(format!(
                "Pen active — {pen_in_progress} anchor(s); Esc cancels, click near first anchor to close"
            ))
            .small()
            .color(Color32::from_rgb(255, 220, 80)),
        );
    }

    if masks.is_empty() {
        ui.label(
            RichText::new("(no masks)")
                .color(Color32::from_gray(120))
                .small()
                .italics(),
        );
        return actions;
    }

    for (i, m) in masks.iter().enumerate() {
        ui.collapsing(format!("{}: {}", i + 1, &m.name), |ui| {
            ui.horizontal(|ui| {
                let mut mode = m.mode;
                egui::ComboBox::from_id_salt(("mask-mode", layer_id.0, i))
                    .selected_text(mode.label())
                    .show_ui(ui, |ui| {
                        for opt in MaskMode::ALL {
                            if ui.selectable_label(opt == mode, opt.label()).clicked() {
                                mode = opt;
                            }
                        }
                    });
                if mode != m.mode {
                    actions.push(MaskAction::SetMode {
                        layer: layer_id,
                        index: i,
                        mode,
                    });
                }
                if ui.small_button("×").on_hover_text("Delete mask").clicked() {
                    actions.push(MaskAction::Delete {
                        layer: layer_id,
                        index: i,
                    });
                }
            });
            let mut opacity = m.opacity;
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("opacity"))
                .changed()
            {
                actions.push(MaskAction::SetOpacity {
                    layer: layer_id,
                    index: i,
                    value: opacity,
                });
            }
            let mut feather = m.feather;
            if ui
                .add(egui::Slider::new(&mut feather, 0.0..=64.0).text("feather"))
                .changed()
            {
                actions.push(MaskAction::SetFeather {
                    layer: layer_id,
                    index: i,
                    value: feather,
                });
            }
            let mut expansion = m.expansion;
            if ui
                .add(egui::Slider::new(&mut expansion, -32.0..=32.0).text("expansion"))
                .changed()
            {
                actions.push(MaskAction::SetExpansion {
                    layer: layer_id,
                    index: i,
                    value: expansion,
                });
            }
        });
    }

    actions
}

/// In-progress pen-tool state. Held by the host app, not the panel.
#[derive(Clone, Debug, Default)]
pub struct PenState {
    pub layer: Option<LayerId>,
    pub anchors: Vec<MaskVertex>,
}

impl PenState {
    pub fn start(&mut self, layer: LayerId) {
        self.layer = Some(layer);
        self.anchors.clear();
    }
    pub fn cancel(&mut self) {
        self.layer = None;
        self.anchors.clear();
    }
    pub fn add_anchor(&mut self, x: f32, y: f32) {
        self.anchors.push(MaskVertex::corner(x, y));
    }
    pub fn close_into_path(&mut self) -> Option<MaskPath> {
        if self.anchors.len() < 3 {
            return None;
        }
        let path = MaskPath {
            vertices: std::mem::take(&mut self.anchors),
        };
        self.layer = None;
        Some(path)
    }
}
