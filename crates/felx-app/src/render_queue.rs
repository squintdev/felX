//! Render queue (F-107).
//!
//! Each entry pairs a `CompId` with the `EncodeOptions` (or sequence /
//! GIF / WAV equivalent) the host should run when the entry is started.
//! Pause / resume / cancel / reorder are surfaced through `QueueAction`s
//! the panel emits and the host applies.
//!
//! The queue is in-memory only; persistence to the project file is a
//! schema follow-up.

#![allow(dead_code)] // panel + actions are public-API surface; full host wiring is the F-108 polish path

use felx_core::model::CompId;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueState {
    Pending,
    Running,
    Paused,
    Done,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
pub enum OutputKind {
    H264,
    H265,
    Prores422,
    Prores4444,
    Gif,
    PngSequence,
    ExrSequence,
    Wav,
}

impl OutputKind {
    pub fn label(&self) -> &'static str {
        match self {
            OutputKind::H264 => "H.264",
            OutputKind::H265 => "H.265",
            OutputKind::Prores422 => "ProRes 422",
            OutputKind::Prores4444 => "ProRes 4444",
            OutputKind::Gif => "GIF",
            OutputKind::PngSequence => "PNG seq",
            OutputKind::ExrSequence => "EXR seq",
            OutputKind::Wav => "WAV",
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueueEntry {
    pub id: u64,
    pub comp_id: CompId,
    pub comp_name: String,
    pub output: OutputKind,
    pub out_path: PathBuf,
    pub state: QueueState,
    pub frames_done: u32,
    pub frames_total: u32,
    pub started: Option<SystemTime>,
    pub finished: Option<SystemTime>,
    pub last_error: Option<String>,
}

impl QueueEntry {
    pub fn pct(&self) -> f32 {
        if self.frames_total == 0 {
            0.0
        } else {
            (self.frames_done as f32 / self.frames_total as f32) * 100.0
        }
    }

    /// Naïve linear ETA based on elapsed-so-far + frames_remaining.
    pub fn eta(&self) -> Option<Duration> {
        let started = self.started?;
        let elapsed = SystemTime::now().duration_since(started).ok()?;
        if self.frames_done == 0 {
            return None;
        }
        let per_frame = elapsed.as_secs_f64() / self.frames_done as f64;
        let remaining = self.frames_total.saturating_sub(self.frames_done);
        Some(Duration::from_secs_f64(per_frame * remaining as f64))
    }
}

#[derive(Default)]
pub struct RenderQueue {
    entries: Vec<QueueEntry>,
    next_id: u64,
}

impl RenderQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(
        &mut self,
        comp_id: CompId,
        comp_name: impl Into<String>,
        output: OutputKind,
        out_path: PathBuf,
        frames_total: u32,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(QueueEntry {
            id,
            comp_id,
            comp_name: comp_name.into(),
            output,
            out_path,
            state: QueueState::Pending,
            frames_done: 0,
            frames_total,
            started: None,
            finished: None,
            last_error: None,
        });
        id
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(e) = self.find_mut(id) {
            e.state = QueueState::Cancelled;
            e.finished = Some(SystemTime::now());
        }
    }

    pub fn pause(&mut self, id: u64) {
        if let Some(e) = self.find_mut(id)
            && matches!(e.state, QueueState::Running | QueueState::Pending)
        {
            e.state = QueueState::Paused;
        }
    }

    pub fn resume(&mut self, id: u64) {
        if let Some(e) = self.find_mut(id)
            && matches!(e.state, QueueState::Paused)
        {
            e.state = QueueState::Pending;
        }
    }

    pub fn move_up(&mut self, id: u64) {
        if let Some(idx) = self.entries.iter().position(|e| e.id == id) {
            if idx > 0 {
                self.entries.swap(idx - 1, idx);
            }
        }
    }

    pub fn move_down(&mut self, id: u64) {
        if let Some(idx) = self.entries.iter().position(|e| e.id == id)
            && idx + 1 < self.entries.len()
        {
            self.entries.swap(idx, idx + 1);
        }
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    pub fn next_pending(&self) -> Option<&QueueEntry> {
        self.entries
            .iter()
            .find(|e| matches!(e.state, QueueState::Pending))
    }

    pub fn mark_running(&mut self, id: u64) {
        if let Some(e) = self.find_mut(id) {
            e.state = QueueState::Running;
            e.started = Some(SystemTime::now());
        }
    }

    pub fn record_progress(&mut self, id: u64, done: u32) {
        if let Some(e) = self.find_mut(id) {
            e.frames_done = done;
        }
    }

    pub fn mark_done(&mut self, id: u64) {
        if let Some(e) = self.find_mut(id) {
            e.state = QueueState::Done;
            e.finished = Some(SystemTime::now());
            e.frames_done = e.frames_total;
        }
    }

    pub fn mark_failed(&mut self, id: u64, err: impl Into<String>) {
        if let Some(e) = self.find_mut(id) {
            e.state = QueueState::Failed;
            e.last_error = Some(err.into());
            e.finished = Some(SystemTime::now());
        }
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut QueueEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }
}

#[derive(Clone, Debug)]
pub enum QueueAction {
    Cancel(u64),
    Pause(u64),
    Resume(u64),
    MoveUp(u64),
    MoveDown(u64),
}

/// Render the queue panel. Returns user actions for the host to apply.
pub fn show(ui: &mut egui::Ui, queue: &RenderQueue) -> Vec<QueueAction> {
    use egui::{Color32, RichText};
    let mut actions = Vec::new();
    ui.heading("Render Queue");
    ui.separator();
    if queue.entries().is_empty() {
        ui.label(
            RichText::new("(queue is empty — add a render from the export menu)")
                .color(Color32::from_gray(120))
                .italics(),
        );
        return actions;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for e in queue.entries() {
            ui.horizontal(|ui| {
                let state_color = match e.state {
                    QueueState::Pending => Color32::from_gray(200),
                    QueueState::Running => Color32::from_rgb(120, 200, 255),
                    QueueState::Paused => Color32::from_rgb(220, 200, 80),
                    QueueState::Done => Color32::from_rgb(120, 255, 120),
                    QueueState::Cancelled => Color32::from_gray(140),
                    QueueState::Failed => Color32::from_rgb(255, 120, 120),
                };
                ui.label(
                    RichText::new(format!("{:?}", e.state))
                        .color(state_color)
                        .small()
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "{}  •  {}  •  {}",
                        e.comp_name,
                        e.output.label(),
                        e.out_path.display()
                    ))
                    .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text("Cancel").clicked() {
                        actions.push(QueueAction::Cancel(e.id));
                    }
                    if matches!(e.state, QueueState::Paused) {
                        if ui.small_button("▶").on_hover_text("Resume").clicked() {
                            actions.push(QueueAction::Resume(e.id));
                        }
                    } else if matches!(e.state, QueueState::Pending | QueueState::Running)
                        && ui.small_button("⏸").on_hover_text("Pause").clicked()
                    {
                        actions.push(QueueAction::Pause(e.id));
                    }
                    if ui.small_button("▼").on_hover_text("Move down").clicked() {
                        actions.push(QueueAction::MoveDown(e.id));
                    }
                    if ui.small_button("▲").on_hover_text("Move up").clicked() {
                        actions.push(QueueAction::MoveUp(e.id));
                    }
                });
            });
            // Progress bar.
            ui.add(
                egui::ProgressBar::new(e.pct() / 100.0)
                    .text(format!("{}/{}", e.frames_done, e.frames_total))
                    .desired_height(6.0),
            );
            if let Some(eta) = e.eta()
                && matches!(e.state, QueueState::Running)
            {
                ui.label(
                    RichText::new(format!("ETA: {:.0}s", eta.as_secs_f32()))
                        .small()
                        .color(Color32::from_gray(160)),
                );
            }
            if let Some(err) = e.last_error.as_deref() {
                ui.label(
                    RichText::new(err)
                        .small()
                        .color(Color32::from_rgb(255, 120, 120)),
                );
            }
            ui.separator();
        }
    });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_assigns_unique_ids() {
        let mut q = RenderQueue::new();
        let a = q.enqueue(CompId(1), "main", OutputKind::H264, "a.mp4".into(), 100);
        let b = q.enqueue(CompId(1), "main", OutputKind::Gif, "a.gif".into(), 100);
        assert_ne!(a, b);
    }

    #[test]
    fn move_up_swaps_with_predecessor() {
        let mut q = RenderQueue::new();
        let a = q.enqueue(CompId(1), "a", OutputKind::H264, "a".into(), 1);
        let b = q.enqueue(CompId(1), "b", OutputKind::H264, "b".into(), 1);
        q.move_up(b);
        assert_eq!(q.entries()[0].id, b);
        assert_eq!(q.entries()[1].id, a);
    }

    #[test]
    fn cancel_marks_state_and_finishes() {
        let mut q = RenderQueue::new();
        let id = q.enqueue(CompId(1), "x", OutputKind::Wav, "x.wav".into(), 0);
        q.cancel(id);
        let e = q.entries().first().unwrap();
        assert_eq!(e.state, QueueState::Cancelled);
        assert!(e.finished.is_some());
    }

    #[test]
    fn next_pending_skips_finished() {
        let mut q = RenderQueue::new();
        let a = q.enqueue(CompId(1), "a", OutputKind::H264, "a".into(), 1);
        let b = q.enqueue(CompId(1), "b", OutputKind::H264, "b".into(), 1);
        q.mark_done(a);
        let next = q.next_pending().unwrap();
        assert_eq!(next.id, b);
    }
}
