//! Transport bar: play/pause, frame counter, scrubber.

use crate::playback::Playhead;
use egui::{Key, RichText, Ui};

#[derive(Clone, Debug)]
pub enum TransportAction {
    Toggle,
    StepForward,
    StepBackward,
    Seek(u32),
}

pub fn show(ui: &mut Ui, playhead: &Playhead) -> Vec<TransportAction> {
    let mut actions = Vec::new();
    let max = playhead.duration_frames().saturating_sub(1);

    ui.horizontal(|ui| {
        let label = if playhead.is_playing() {
            "Pause"
        } else {
            "Play"
        };
        if ui.button(label).clicked() {
            actions.push(TransportAction::Toggle);
        }
        if ui.button("◀").on_hover_text("Step back").clicked() {
            actions.push(TransportAction::StepBackward);
        }
        if ui.button("▶").on_hover_text("Step forward").clicked() {
            actions.push(TransportAction::StepForward);
        }
        ui.separator();

        let mut frame = playhead.current_frame();
        let resp = ui.add(egui::Slider::new(&mut frame, 0..=max).text("frame"));
        if resp.changed() {
            actions.push(TransportAction::Seek(frame));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "{:>5} / {:<5}  @ {:.2} fps",
                    playhead.current_frame(),
                    max,
                    playhead.framerate_fps()
                ))
                .monospace(),
            );
        });
    });

    // Keyboard shortcuts. Read from the egui context (the same ctx is
    // available via ui.ctx()).
    let ctx = ui.ctx();
    ctx.input(|i| {
        if i.key_pressed(Key::Space) {
            actions.push(TransportAction::Toggle);
        }
        if i.key_pressed(Key::ArrowLeft) {
            actions.push(TransportAction::StepBackward);
        }
        if i.key_pressed(Key::ArrowRight) {
            actions.push(TransportAction::StepForward);
        }
    });

    actions
}
