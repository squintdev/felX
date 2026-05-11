//! The eframe `App` impl. Owns the project, the compositor, and the egui
//! texture handle that mirrors the compositor's output for display.

use crate::hot_reload::{HotReloadEvent, HotReloadWatcher};
use crate::manifests::ManifestRegistry;
use crate::panels::effects::{self, EffectsAction};
use crate::panels::graph_editor;
use crate::panels::layers::{self, LayerAction};
use crate::panels::masks::{self, MaskAction, PenState};
use crate::panels::transport::{self, TransportAction};
use crate::playback::Playhead;
use crate::presets::PresetRegistry;
use eframe::egui_wgpu::RenderState;
use eframe::{App, CreationContext, Frame};
use egui::{CentralPanel, Color32, Context, Sense, SidePanel, TextureId, TopBottomPanel, Vec2};
use felx_core::model::{AssetKind, CompId, Effect, Frame as FelxFrame, LayerId, Project};
use felx_render::compositor::{Compositor, CompositorError, PreviewScale};
use felx_render::effects::gain::Gain;
use felx_render::texture_io::COMPOSITOR_FORMAT;
use felx_render::{AdapterInfo, Renderer};
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

pub struct FelxApp {
    project: Project,
    comp_id: CompId,
    playhead: Playhead,
    compositor: Compositor,
    preview_scale: PreviewScale,
    selected_layer: Option<LayerId>,
    manifests: ManifestRegistry,
    presets: PresetRegistry,
    /// Filesystem watcher for `effects/<id>/effect.wgsl`. None if the
    /// effects dir couldn't be located (running from a non-source layout).
    hot_reload: Option<HotReloadWatcher>,
    /// Most recent shader-compile error message, if any. Cleared on a
    /// successful reload. Displayed as a non-fatal overlay.
    shader_error: Option<String>,
    /// Texture currently registered with egui's wgpu renderer. Replaced
    /// every time the compositor produces a new output texture.
    egui_texture: Option<TextureId>,
    /// Set any time the compositor needs to re-render (layer or parameter
    /// edit, scrub, playback advance, hot-reload). Cleared by
    /// [`ensure_frame_rendered`].
    render_dirty: bool,
    /// Graph editor visibility. Off by default to avoid eating screen real
    /// estate before the user has any animated parameters.
    graph_editor_open: bool,
    /// Optional region-of-interest for the preview. Normalized comp-space
    /// (0..1, 0..1, with origin at top-left). When `Some`, the viewer paints
    /// only this sub-rectangle stretched to fill the viewer area. Per-effect
    /// region-aware rendering — actually skipping pixels outside the region
    /// inside the compositor — is a perf follow-up.
    render_region: Option<RegionRect>,
    /// Mid-drag state for the shift-drag region selector. Normalized.
    region_drag_anchor: Option<egui::Vec2>,
    /// Pen-tool in-progress state for mask drawing (F-065). Click on the
    /// viewer drops corner anchors; clicking near the first one closes the
    /// path and creates a new mask on the selected layer.
    pen: PenState,
    /// Help window: which effect's README is currently displayed (None =
    /// help window closed).
    help_open_for: Option<String>,
    /// Path of the .felx file the project came from, if any. `None` means
    /// the in-memory default project — Save behaves like Save As.
    current_project_path: Option<PathBuf>,
    /// cpal output stream + ring buffer (F-052). Lazily constructed on
    /// the first play to avoid grabbing an audio device on launch.
    audio_playback: Option<crate::audio_playback::AudioPlayback>,
    /// Pre-decoded audio sources, one per LayerKind::Audio in the comp.
    /// Built when [`audio_bus_dirty`] is true and the user starts playing.
    audio_sources: Vec<felx_core::media::AudioSource>,
    /// Set when the layer list / asset list changes — the next play tick
    /// rebuilds [`audio_sources`] from the current project state.
    audio_bus_dirty: bool,
    /// Where in the comp timeline the audio playhead is, in master-rate
    /// frames since play started.
    audio_emit_cursor: u64,
    /// Comp frame at which the current play span began. Combined with
    /// `audio_emit_cursor` to compute absolute mix time.
    audio_play_start_frame: u32,
    /// In-flight rfd file dialog. We run `pick_file` on a background
    /// thread so the egui update loop keeps pumping — otherwise the OS
    /// flags the window as "not responding" while the user is browsing.
    pending_file: Option<PendingFile>,
    /// Export modal visibility.
    export_dialog_open: bool,
    /// Live form values for the export modal.
    export_options: crate::export_dialog::ExportOptions,
    /// In-flight export worker (encoder + frame loop on a thread).
    export_job: Option<crate::export_dialog::ExportJob>,
    /// Latest progress for the running export.
    export_status: crate::export_dialog::ExportStatus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingFileKind {
    ImportImage,
    ImportVideo,
    ImportAudio,
    OpenProject,
    SaveProjectAs,
    /// File picker for the Export dialog's "Output" field. The chosen
    /// path lands on `export_options.out_path` rather than performing any
    /// project mutation.
    ExportOutput,
}

struct PendingFile {
    kind: PendingFileKind,
    rx: std::sync::mpsc::Receiver<Option<PathBuf>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionRect {
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
}

impl RegionRect {
    pub fn from_corners(a: egui::Vec2, b: egui::Vec2) -> Self {
        Self {
            u_min: a.x.min(b.x).clamp(0.0, 1.0),
            v_min: a.y.min(b.y).clamp(0.0, 1.0),
            u_max: a.x.max(b.x).clamp(0.0, 1.0),
            v_max: a.y.max(b.y).clamp(0.0, 1.0),
        }
    }
    pub fn is_degenerate(&self) -> bool {
        (self.u_max - self.u_min) < 0.01 || (self.v_max - self.v_min) < 0.01
    }
}

#[derive(Debug)]
pub enum AppInitError {
    NoWgpuRenderState,
}

impl std::fmt::Display for AppInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppInitError::NoWgpuRenderState => write!(
                f,
                "eframe did not provide a wgpu render state — was the wgpu \
                 feature enabled and the wgpu renderer selected?"
            ),
        }
    }
}

impl std::error::Error for AppInitError {}

impl FelxApp {
    pub fn new(cc: &CreationContext<'_>) -> Result<Self, AppInitError> {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or(AppInitError::NoWgpuRenderState)?;
        let renderer = build_renderer(render_state);
        let compositor = Compositor::new(renderer);
        let manifests = ManifestRegistry::load_builtins();
        let presets = PresetRegistry::load_builtins();
        let (project, comp_id) = default_project(&manifests);
        let comp = project.composition(comp_id).expect("comp exists");
        let playhead = Playhead::new(comp.framerate.as_fps(), comp.duration_frames);

        let effects_root = effects_root_dir();
        let hot_reload = match HotReloadWatcher::new(effects_root.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                warn!(error = %e, path = %effects_root.display(), "hot-reload disabled");
                None
            }
        };

        info!(
            comp = comp_id.0,
            manifests = manifests.len(),
            hot_reload = hot_reload.is_some(),
            "felx-app initialized"
        );
        Ok(Self {
            project,
            comp_id,
            playhead,
            compositor,
            preview_scale: PreviewScale::default(),
            selected_layer: None,
            manifests,
            presets,
            hot_reload,
            shader_error: None,
            egui_texture: None,
            render_dirty: true,
            graph_editor_open: false,
            render_region: None,
            region_drag_anchor: None,
            pen: PenState::default(),
            help_open_for: None,
            current_project_path: None,
            audio_playback: None,
            audio_sources: Vec::new(),
            audio_bus_dirty: true,
            audio_emit_cursor: 0,
            audio_play_start_frame: 0,
            pending_file: None,
            export_dialog_open: false,
            export_options: crate::export_dialog::ExportOptions::default(),
            export_job: None,
            export_status: crate::export_dialog::ExportStatus::default(),
        })
    }

    /// Spawn a background thread that runs the rfd file dialog and sends
    /// the chosen path (or `None` if the user cancelled) back through a
    /// channel. The GUI poll loop picks it up via [`poll_pending_file`].
    fn start_file_pick(&mut self, kind: PendingFileKind) {
        if self.pending_file.is_some() {
            return; // one dialog at a time
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = match kind {
                PendingFileKind::ImportImage => rfd::FileDialog::new()
                    .add_filter(
                        "Images",
                        &["png", "jpg", "jpeg", "exr", "bmp", "tif", "tiff"],
                    )
                    .set_title("Import image")
                    .pick_file(),
                PendingFileKind::ImportVideo => rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi", "m4v"])
                    .set_title("Import video")
                    .pick_file(),
                PendingFileKind::ImportAudio => rfd::FileDialog::new()
                    .add_filter(
                        "Audio",
                        &["wav", "mp3", "flac", "ogg", "aac", "m4a", "opus"],
                    )
                    .set_title("Import audio")
                    .pick_file(),
                PendingFileKind::OpenProject => rfd::FileDialog::new()
                    .add_filter("felx project", &["felx"])
                    .set_title("Open project")
                    .pick_file(),
                PendingFileKind::SaveProjectAs => rfd::FileDialog::new()
                    .add_filter("felx project", &["felx"])
                    .set_title("Save project")
                    .save_file(),
                PendingFileKind::ExportOutput => {
                    rfd::FileDialog::new().set_title("Export to…").save_file()
                }
            };
            let _ = tx.send(result);
        });
        self.pending_file = Some(PendingFile { kind, rx });
    }

    /// Poll the in-flight file pick (if any). Returns true if a result
    /// was consumed this tick — caller can use that to flag a repaint.
    fn poll_pending_file(&mut self) -> bool {
        let Some(pending) = self.pending_file.as_ref() else {
            return false;
        };
        let result = match pending.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending_file = None;
                return true;
            }
        };
        let kind = pending.kind;
        self.pending_file = None;
        let Some(path) = result else {
            return true; // user cancelled
        };
        match kind {
            PendingFileKind::ImportImage => self.complete_import(path, AssetKind::Image),
            PendingFileKind::ImportVideo => self.complete_import(path, AssetKind::Video),
            PendingFileKind::ImportAudio => self.complete_import(path, AssetKind::Audio),
            PendingFileKind::OpenProject => self.complete_open_project(path),
            PendingFileKind::SaveProjectAs => self.complete_save_project(path),
            PendingFileKind::ExportOutput => {
                self.export_options.out_path = Some(path);
            }
        }
        true
    }

    /// Render the Export modal. Closes either when the user cancels or
    /// when an export starts. The progress bar window stays open
    /// (separate state) while the worker runs.
    fn show_export_dialog(&mut self, ctx: &egui::Context) {
        use crate::export_dialog::ExportFormat;
        if !self.export_dialog_open {
            return;
        }
        let mut start = false;
        let mut pick_path = false;
        let mut cancel = false;
        let mut window_open = true;
        egui::Window::new("Export")
            .open(&mut window_open)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                let comp = self.project.composition(self.comp_id).expect("comp exists");
                ui.label(format!(
                    "{} — {}×{} @ {:.2} fps, {} frames",
                    comp.name,
                    comp.width,
                    comp.height,
                    comp.framerate.as_fps(),
                    comp.duration_frames
                ));
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Format");
                    let current = self.export_options.format;
                    egui::ComboBox::from_id_salt("export-format")
                        .selected_text(current.label())
                        .show_ui(ui, |ui| {
                            for f in ExportFormat::ALL {
                                if ui.selectable_label(f == current, f.label()).clicked() {
                                    self.export_options.format = f;
                                    // Reset output path when format changes — the file
                                    // extension is wrong now.
                                    self.export_options.out_path = None;
                                }
                            }
                        });
                });

                // Per-format extra controls.
                match self.export_options.format {
                    ExportFormat::H264 | ExportFormat::H265 => {
                        ui.horizontal(|ui| {
                            ui.label("CRF");
                            ui.add(
                                egui::Slider::new(&mut self.export_options.crf, 0..=51)
                                    .text("(lower = higher quality)"),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Preset");
                            egui::ComboBox::from_id_salt("export-preset")
                                .selected_text(&self.export_options.preset)
                                .show_ui(ui, |ui| {
                                    for p in [
                                        "ultrafast",
                                        "superfast",
                                        "veryfast",
                                        "faster",
                                        "fast",
                                        "medium",
                                        "slow",
                                        "slower",
                                        "veryslow",
                                    ] {
                                        if ui
                                            .selectable_label(self.export_options.preset == p, p)
                                            .clicked()
                                        {
                                            self.export_options.preset = p.to_string();
                                        }
                                    }
                                });
                        });
                    }
                    ExportFormat::Gif => {
                        ui.horizontal(|ui| {
                            ui.label("Palette");
                            ui.add(
                                egui::Slider::new(&mut self.export_options.gif_palette, 8..=256)
                                    .text("colors"),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Dither");
                            use felx_render::gif_export::GifDither;
                            for (label, value) in [
                                ("None", GifDither::None),
                                ("Bayer", GifDither::Bayer),
                                ("Floyd–Steinberg", GifDither::FloydSteinberg),
                                ("Sierra2-4A", GifDither::Sierra2_4a),
                            ] {
                                if ui
                                    .selectable_label(
                                        self.export_options.gif_dither == value,
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.export_options.gif_dither = value;
                                }
                            }
                        });
                    }
                    ExportFormat::Wav => {
                        ui.horizontal(|ui| {
                            ui.label("Bit depth");
                            use felx_media::WavBitDepth;
                            for (label, value) in [
                                ("16-bit PCM", WavBitDepth::Pcm16),
                                ("24-bit PCM", WavBitDepth::Pcm24),
                                ("32-bit float", WavBitDepth::Float32),
                            ] {
                                if ui
                                    .selectable_label(self.export_options.wav_depth == value, label)
                                    .clicked()
                                {
                                    self.export_options.wav_depth = value;
                                }
                            }
                        });
                    }
                    ExportFormat::Prores422
                    | ExportFormat::Prores4444
                    | ExportFormat::PngSequence
                    | ExportFormat::ExrSequence => {}
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Output");
                    let display = self
                        .export_options
                        .out_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(none — click \"Choose…\")".into());
                    ui.label(
                        egui::RichText::new(display)
                            .color(Color32::from_gray(180))
                            .monospace(),
                    );
                    if ui.button("Choose…").clicked() {
                        pick_path = true;
                    }
                });
                if let Some(ext) = self.export_options.format.required_extension() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Filename should end in .{ext} (auto-appended if missing)"
                        ))
                        .small()
                        .color(Color32::from_gray(140)),
                    );
                }

                ui.separator();
                let ready = self.export_options.out_path.is_some();
                ui.horizontal(|ui| {
                    if ui.add_enabled(ready, egui::Button::new("Export")).clicked() {
                        start = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if !ready {
                        ui.label(
                            egui::RichText::new("(pick an output path first)")
                                .small()
                                .color(Color32::from_gray(150)),
                        );
                    }
                });
            });
        if pick_path {
            self.start_file_pick(PendingFileKind::ExportOutput);
        }
        if start {
            self.export_dialog_open = false;
            self.start_export();
        } else if cancel || !window_open {
            self.export_dialog_open = false;
        }
    }

    fn start_export(&mut self) {
        let job = crate::export_dialog::spawn_export(
            self.project.clone(),
            self.comp_id,
            self.export_options.clone(),
        );
        match job {
            Ok(job) => {
                self.export_status = crate::export_dialog::ExportStatus::default();
                self.export_job = Some(job);
                info!("export started");
            }
            Err(e) => {
                warn!(error = %e, "export failed to start");
                self.export_status = crate::export_dialog::ExportStatus {
                    done: 0,
                    total: 0,
                    finished: true,
                    error: Some(e),
                };
            }
        }
    }

    /// Drain export progress messages. Run every update tick.
    fn poll_export(&mut self) {
        use crate::export_dialog::ExportProgress;
        let Some(job) = self.export_job.as_ref() else {
            return;
        };
        loop {
            match job.rx.try_recv() {
                Ok(ExportProgress::Started { total_frames }) => {
                    self.export_status.total = total_frames;
                }
                Ok(ExportProgress::Frame { done, total }) => {
                    self.export_status.done = done;
                    self.export_status.total = total;
                }
                Ok(ExportProgress::Done) => {
                    self.export_status.finished = true;
                    self.export_status.error = None;
                    self.export_job = None;
                    info!("export complete");
                    break;
                }
                Ok(ExportProgress::Failed(e)) => {
                    self.export_status.finished = true;
                    self.export_status.error = Some(e);
                    self.export_job = None;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if !self.export_status.finished {
                        self.export_status.finished = true;
                        self.export_status.error =
                            Some("export worker disconnected unexpectedly".into());
                    }
                    self.export_job = None;
                    break;
                }
            }
        }
    }

    /// Floating progress window for an in-flight or just-finished export.
    fn show_export_progress(&mut self, ctx: &egui::Context) {
        if self.export_job.is_none() && self.export_status.finished {
            // Show a one-shot "done" toast until the user dismisses it.
            let mut keep = true;
            egui::Window::new("Export — done")
                .open(&mut keep)
                .resizable(false)
                .show(ctx, |ui| {
                    if let Some(err) = self.export_status.error.as_deref() {
                        ui.colored_label(Color32::from_rgb(255, 120, 120), err);
                    } else {
                        ui.label(
                            egui::RichText::new("Export complete.")
                                .color(Color32::from_rgb(120, 220, 140)),
                        );
                    }
                });
            if !keep {
                self.export_status = crate::export_dialog::ExportStatus::default();
            }
            return;
        }
        if self.export_job.is_none() {
            return;
        }
        egui::Window::new("Export — running")
            .resizable(false)
            .collapsible(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.add(
                    egui::ProgressBar::new(self.export_status.pct()).text(format!(
                        "{}/{}",
                        self.export_status.done, self.export_status.total
                    )),
                );
                ui.label(
                    egui::RichText::new("(export runs on a worker thread; you can keep editing)")
                        .small()
                        .color(Color32::from_gray(150)),
                );
            });
    }

    /// Decode every `LayerKind::Audio` asset in the current comp and build
    /// the mixer's source list. Called when the audio layer set changes
    /// or before the first play. The decoded buffers are cached so
    /// playback doesn't redo the decode on every loop iteration.
    fn rebuild_audio_sources(&mut self) {
        use felx_core::media::AudioSource;
        use felx_core::model::{Curve, Frame as FelxFrame, LayerKind};
        let comp = match self.project.composition(self.comp_id) {
            Some(c) => c,
            None => return,
        };
        let framerate = comp.framerate;
        let mut sources: Vec<AudioSource> = Vec::new();
        for layer in &comp.layers {
            let LayerKind::Audio { asset } = &layer.kind else {
                continue;
            };
            let Some(asset_meta) = self.project.asset(*asset) else {
                continue;
            };
            let decoded = match felx_media::decode_file(&asset_meta.path, 48_000) {
                Ok(d) => d,
                Err(e) => {
                    warn!(path = ?asset_meta.path, error = %e, "audio decode failed");
                    continue;
                }
            };
            // Pad with leading silence so the layer's in_frame aligns with
            // the mix window's time-zero (which is the comp's frame 0).
            let in_secs = FelxFrame(layer.in_frame).to_time(framerate).as_seconds();
            let pad_frames = (in_secs * decoded.sample_rate as f64).round() as usize;
            let pad_samples = pad_frames * decoded.channels as usize;
            let mut pcm = Vec::with_capacity(pad_samples + decoded.pcm.len());
            pcm.resize(pad_samples, 0.0);
            pcm.extend_from_slice(&decoded.pcm);
            sources.push(AudioSource {
                sample_rate: decoded.sample_rate,
                channels: decoded.channels,
                pcm,
                gain: Curve::Static(1.0),
                pan: Curve::Static(0.0),
            });
        }
        info!(layers = sources.len(), "audio sources rebuilt");
        self.audio_sources = sources;
        self.audio_bus_dirty = false;
    }

    /// Per-update tick — keep the audio output buffer fed while playing.
    /// Drops the playback (and therefore the cpal stream) on pause / stop
    /// so we don't burn an audio device when idle.
    fn drive_audio(&mut self) {
        if !self.playhead.is_playing() {
            // Pause / stop: drop the stream, reset cursor.
            if self.audio_playback.is_some() {
                self.audio_playback = None;
            }
            return;
        }

        // First play tick after a (re)start: rebuild sources if dirty,
        // anchor the audio cursor to the current playhead frame.
        if self.audio_playback.is_none() {
            if self.audio_bus_dirty {
                self.rebuild_audio_sources();
            }
            let clock = std::sync::Arc::new(felx_core::media::AudioClock::new(48_000));
            match crate::audio_playback::AudioPlayback::new(clock) {
                Ok(p) => {
                    self.audio_playback = Some(p);
                    self.audio_play_start_frame = self.playhead.current_frame();
                    self.audio_emit_cursor = 0;
                }
                Err(e) => {
                    warn!(error = %e, "audio playback init failed; rendering will be silent");
                    return;
                }
            }
        }

        if self.audio_sources.is_empty() {
            return;
        }

        let comp = self.project.composition(self.comp_id).expect("comp");
        let framerate = comp.framerate;
        let master_rate = 48_000u32;
        let chunk_frames = 4096usize;
        // Keep ~6 chunks ahead so a missed update tick doesn't underrun.
        let target_queue_samples = chunk_frames * 2 * 6;

        let Some(playback) = self.audio_playback.as_ref() else {
            return;
        };
        while playback.queued() < target_queue_samples {
            // Compute the time at which this chunk starts. Audio cursor is
            // master-rate frames since play start; comp time = play-start
            // frame + cursor / master_rate (in seconds), expressed as the
            // mixer's Rational time.
            let secs_since_start = self.audio_emit_cursor as f64 / master_rate as f64;
            let start_frame_secs = FelxFrame(self.audio_play_start_frame)
                .to_time(framerate)
                .as_seconds();
            let abs_secs = start_frame_secs + secs_since_start;
            let den: u32 = master_rate;
            let num = (abs_secs * den as f64).round().max(0.0) as u32;
            let time = felx_core::model::Rational::new(num, den);

            // Stop emitting once we've passed the comp duration so
            // playback doesn't loop the master-bus reads indefinitely.
            let comp_duration_secs = comp.duration_frames as f64 / framerate.as_fps();
            if abs_secs >= comp_duration_secs {
                break;
            }

            let mixed = felx_core::media::mix_window(
                &self.audio_sources,
                time,
                master_rate,
                chunk_frames,
                1.0,
            );
            playback.enqueue(&mixed.pcm);
            self.audio_emit_cursor += chunk_frames as u64;
        }
    }

    /// Open a file dialog and import a media file as a new layer of the
    /// matching kind. The asset is registered against the project, then a
    /// layer is added to the current comp with sensible defaults: full
    /// duration for image/video, comp-aligned in/out for audio.
    /// Kick off an import file dialog on a background thread. The actual
    /// asset registration happens later via [`complete_import`] once the
    /// dialog returns. Doing it asynchronously is what keeps the eframe
    /// update loop pumping while the user browses for a file.
    fn import_media(&mut self, kind: AssetKind) {
        let pending = match kind {
            AssetKind::Image => PendingFileKind::ImportImage,
            AssetKind::Video => PendingFileKind::ImportVideo,
            AssetKind::Audio => PendingFileKind::ImportAudio,
        };
        self.start_file_pick(pending);
    }

    fn complete_import(&mut self, path: PathBuf, kind: AssetKind) {
        let asset_id = self.project.add_asset(path.clone(), kind);

        // If we're importing a video, probe for an audio stream and add a
        // parallel Audio asset + layer when one's present. Imported clips
        // with sound then just work — no second "Import Audio" step.
        let extra_audio_asset = if matches!(kind, AssetKind::Video) {
            match felx_media::probe_audio(&path) {
                Ok(_) => {
                    info!(path = ?path, "video has audio stream; adding Audio layer");
                    Some(self.project.add_asset(path.clone(), AssetKind::Audio))
                }
                Err(_) => None,
            }
        } else {
            None
        };

        let Some(comp) = self.project.composition_mut(self.comp_id) else {
            return;
        };
        let dur = comp.duration_frames;
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported".into());
        let layer_kind = match kind {
            AssetKind::Image => felx_core::model::LayerKind::Image { asset: asset_id },
            AssetKind::Video => felx_core::model::LayerKind::Video { asset: asset_id },
            AssetKind::Audio => felx_core::model::LayerKind::Audio { asset: asset_id },
        };
        let id = comp.add_layer(label.clone(), layer_kind, 0, dur);
        if let Some(audio_asset) = extra_audio_asset {
            comp.add_layer(
                format!("{label} (audio)"),
                felx_core::model::LayerKind::Audio { asset: audio_asset },
                0,
                dur,
            );
        }
        self.selected_layer = Some(id);
        self.render_dirty = true;
        self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
        self.audio_bus_dirty = true;
        info!(asset = ?path, kind = ?kind, "imported media");
    }

    fn open_project_dialog(&mut self) {
        self.start_file_pick(PendingFileKind::OpenProject);
    }

    fn complete_open_project(&mut self, path: PathBuf) {
        match Project::load(&path) {
            Ok(p) => {
                let comp_id = p.compositions.first().map(|c| c.id).unwrap_or(self.comp_id);
                let comp = p.composition(comp_id).expect("comp from loaded project");
                let playhead =
                    crate::playback::Playhead::new(comp.framerate.as_fps(), comp.duration_frames);
                self.project = p;
                self.comp_id = comp_id;
                self.playhead = playhead;
                self.selected_layer = None;
                self.current_project_path = Some(path);
                self.render_dirty = true;
                self.compositor.cache_mut().clear();
                self.audio_bus_dirty = true;
                self.audio_playback = None;
                info!(path = ?self.current_project_path, "project loaded");
            }
            Err(e) => {
                warn!(error = %e, "project load failed");
            }
        }
    }

    /// Save the project. If we know its file path, write straight back
    /// (no dialog, no blocking). Otherwise spawn the Save-As dialog.
    fn save_project_dialog(&mut self) {
        match &self.current_project_path {
            Some(p) => {
                let p = p.clone();
                self.complete_save_project(p);
            }
            None => self.start_file_pick(PendingFileKind::SaveProjectAs),
        }
    }

    fn complete_save_project(&mut self, path: PathBuf) {
        match self.project.save(&path) {
            Ok(()) => {
                self.current_project_path = Some(path.clone());
                info!(path = ?path, "project saved");
            }
            Err(e) => {
                warn!(error = %e, "project save failed");
            }
        }
    }

    fn process_hot_reload(&mut self) {
        let Some(watcher) = self.hot_reload.as_ref() else {
            return;
        };
        let events = watcher.drain();
        for ev in events {
            match ev {
                HotReloadEvent::WgslChanged { effect_id, path } => {
                    self.reload_effect(&effect_id, &path);
                }
            }
        }
    }

    fn reload_effect(&mut self, effect_id: &str, path: &Path) {
        match effect_id {
            "gain" => {
                let wgsl = match std::fs::read_to_string(path) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(error = %e, path = %path.display(), "shader read failed");
                        return;
                    }
                };
                match Gain::try_with_shader(self.compositor.renderer(), COMPOSITOR_FORMAT, &wgsl) {
                    Ok(gain) => {
                        self.compositor.replace_gain(gain);
                        self.compositor.cache_mut().clear();
                        self.render_dirty = true;
                        self.shader_error = None;
                        info!(effect_id, "shader reloaded");
                    }
                    Err(msg) => {
                        warn!(effect_id, error = %msg, "shader compile failed");
                        self.shader_error = Some(format!("[{effect_id}] {msg}"));
                    }
                }
            }
            "invert" => {
                // CPU effects don't have a shader to reload. Log once per
                // session — `notify` may also fire on save events for the
                // manifest, so we just shrug.
                tracing::debug!(effect_id, "ignoring hot-reload for CPU effect");
            }
            other => {
                tracing::debug!(effect_id = other, "no hot-reload handler");
            }
        }
    }

    fn apply_transport_actions(&mut self, actions: Vec<TransportAction>) {
        if actions.is_empty() {
            return;
        }
        let mut moved = false;
        for action in actions {
            match action {
                TransportAction::Toggle => self.playhead.toggle(),
                TransportAction::StepForward => {
                    self.playhead.step_forward();
                    moved = true;
                }
                TransportAction::StepBackward => {
                    self.playhead.step_backward();
                    moved = true;
                }
                TransportAction::Seek(f) => {
                    self.playhead.seek(f);
                    moved = true;
                }
                TransportAction::SetPreviewScale(s) => {
                    self.preview_scale = s;
                    moved = true;
                }
                TransportAction::ToggleGraphEditor => {
                    self.graph_editor_open = !self.graph_editor_open;
                }
            }
        }
        if moved {
            self.render_dirty = true;
            // Any seek / step / scrub drops the audio stream so the next
            // play tick rebuilds from the new playhead. Otherwise the
            // queue would keep playing the *old* timeline position.
            self.audio_playback = None;
        }
    }

    fn apply_preset(&mut self, preset_index: usize) {
        let Some(preset) = self.presets.iter().nth(preset_index).cloned() else {
            return;
        };
        let Some(layer_id) = self.selected_layer else {
            warn!("apply_preset: no layer selected");
            return;
        };
        let Some(comp) = self.project.composition_mut(self.comp_id) else {
            return;
        };
        let Some(layer) = comp.layer_mut(layer_id) else {
            return;
        };
        for pe in &preset.effects {
            let mut effect = match self.manifests.get(&pe.id) {
                Some(m) => felx_core::model::Effect::from_manifest(m),
                None => felx_core::model::Effect::new(pe.id.clone()),
            };
            for (id, value) in pe.values.iter() {
                effect.values.set(id.to_string(), value.clone());
            }
            layer.effects.push(effect);
        }
        info!(preset = %preset.name, "preset applied");
        self.render_dirty = true;
        self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
    }

    fn apply_effects_actions(&mut self, actions: Vec<EffectsAction>) {
        if actions.is_empty() {
            return;
        }
        let Some(layer_id) = self.selected_layer else {
            return;
        };
        // Resolve any AddEffect actions first by looking up manifests on
        // the *immutable* registry, then enter the mutable comp scope.
        let mut new_effects: Vec<felx_core::model::Effect> = Vec::new();
        let mut remaining: Vec<EffectsAction> = Vec::new();
        for action in actions {
            if let EffectsAction::AddEffect { effect_id } = &action {
                let effect = match self.manifests.get(effect_id) {
                    Some(m) => felx_core::model::Effect::from_manifest(m),
                    None => felx_core::model::Effect::new(effect_id.clone()),
                };
                new_effects.push(effect);
            } else {
                remaining.push(action);
            }
        }
        let Some(comp) = self.project.composition_mut(self.comp_id) else {
            return;
        };
        let Some(layer) = comp.layer_mut(layer_id) else {
            return;
        };
        for eff in new_effects {
            layer.effects.push(eff);
        }
        for action in remaining {
            match action {
                EffectsAction::SetValue {
                    effect_index,
                    id,
                    value,
                } => {
                    if let Some(eff) = layer.effects.get_mut(effect_index) {
                        eff.values.set(id, value);
                    }
                }
                EffectsAction::ToggleEnabled {
                    effect_index,
                    enabled,
                } => {
                    if let Some(eff) = layer.effects.get_mut(effect_index) {
                        eff.enabled = enabled;
                    }
                }
                EffectsAction::RemoveEffect { effect_index } => {
                    if effect_index < layer.effects.len() {
                        layer.effects.remove(effect_index);
                    }
                }
                EffectsAction::MoveUp { effect_index } => {
                    if effect_index > 0 && effect_index < layer.effects.len() {
                        layer.effects.swap(effect_index - 1, effect_index);
                    }
                }
                EffectsAction::MoveDown { effect_index } => {
                    if effect_index + 1 < layer.effects.len() {
                        layer.effects.swap(effect_index, effect_index + 1);
                    }
                }
                EffectsAction::AddEffect { .. } => {
                    // Already handled in the first pass above.
                }
            }
        }
        self.render_dirty = true;
        self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
    }

    fn apply_mask_actions(&mut self, actions: Vec<MaskAction>) {
        if actions.is_empty() {
            return;
        }
        let mut dirty = false;
        for action in actions {
            match action {
                MaskAction::AddRectangle(layer_id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        let w = comp.width as f32;
                        let h = comp.height as f32;
                        if let Some(layer) = comp.layer_mut(layer_id) {
                            layer.masks.push(felx_core::model::Mask::rectangle(
                                format!("Mask {}", layer.masks.len() + 1),
                                w * 0.25,
                                h * 0.25,
                                w * 0.5,
                                h * 0.5,
                            ));
                            dirty = true;
                        }
                    }
                }
                MaskAction::AddEllipse(layer_id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        let w = comp.width as f32;
                        let h = comp.height as f32;
                        if let Some(layer) = comp.layer_mut(layer_id) {
                            layer.masks.push(felx_core::model::Mask::ellipse(
                                format!("Mask {}", layer.masks.len() + 1),
                                w * 0.5,
                                h * 0.5,
                                w * 0.25,
                                h * 0.25,
                            ));
                            dirty = true;
                        }
                    }
                }
                MaskAction::StartPen(layer_id) => {
                    self.pen.start(layer_id);
                }
                MaskAction::Delete { layer, index } => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(l) = comp.layer_mut(layer)
                        && index < l.masks.len()
                    {
                        l.masks.remove(index);
                        dirty = true;
                    }
                }
                MaskAction::SetMode { layer, index, mode } => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(l) = comp.layer_mut(layer)
                        && let Some(m) = l.masks.get_mut(index)
                    {
                        m.mode = mode;
                        dirty = true;
                    }
                }
                MaskAction::SetOpacity {
                    layer,
                    index,
                    value,
                } => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(l) = comp.layer_mut(layer)
                        && let Some(m) = l.masks.get_mut(index)
                    {
                        m.opacity = value;
                        dirty = true;
                    }
                }
                MaskAction::SetFeather {
                    layer,
                    index,
                    value,
                } => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(l) = comp.layer_mut(layer)
                        && let Some(m) = l.masks.get_mut(index)
                    {
                        m.feather = value;
                        dirty = true;
                    }
                }
                MaskAction::SetExpansion {
                    layer,
                    index,
                    value,
                } => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(l) = comp.layer_mut(layer)
                        && let Some(m) = l.masks.get_mut(index)
                    {
                        m.expansion = value;
                        dirty = true;
                    }
                }
            }
        }
        if dirty {
            self.render_dirty = true;
            self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
        }
    }

    fn apply_layer_actions(&mut self, actions: Vec<LayerAction>) {
        if actions.is_empty() {
            return;
        }
        let dirty = !actions.is_empty();
        for action in actions {
            match action {
                LayerAction::Select(id) => self.selected_layer = id,
                LayerAction::AddSolid => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        let id = comp.add_solid("Solid", [0.5, 0.5, 0.5, 1.0]);
                        self.selected_layer = Some(id);
                    }
                }
                LayerAction::AddAdjustment => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        let dur = comp.duration_frames;
                        let id = comp.add_layer(
                            "Adjustment",
                            felx_core::model::LayerKind::Adjustment,
                            0,
                            dur,
                        );
                        self.selected_layer = Some(id);
                    }
                }
                LayerAction::ImportImage => self.import_media(AssetKind::Image),
                LayerAction::ImportVideo => self.import_media(AssetKind::Video),
                LayerAction::ImportAudio => self.import_media(AssetKind::Audio),
                LayerAction::Delete(id) => {
                    let was_audio = self
                        .project
                        .composition(self.comp_id)
                        .and_then(|c| c.layer(id))
                        .map(|l| matches!(l.kind, felx_core::model::LayerKind::Audio { .. }))
                        .unwrap_or(false);
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.remove_layer(id);
                    }
                    if self.selected_layer == Some(id) {
                        self.selected_layer = None;
                    }
                    if was_audio {
                        self.audio_bus_dirty = true;
                        self.audio_playback = None;
                    }
                }
                // The layers panel renders `comp.layers.iter().rev()` so the
                // top panel row = last Vec entry = the layer rendered last
                // = visually on top. "▲ Move up" means the user wants the
                // layer higher in the panel (= visually on top), which is
                // toward the *end* of the Vec — i.e. Composition::
                // move_layer_down. Flipped likewise for ▼.
                LayerAction::MoveUp(id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.move_layer_down(id);
                    }
                }
                LayerAction::MoveDown(id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.move_layer_up(id);
                    }
                }
                LayerAction::SetTimeOffset(id, offset) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(layer) = comp.layer_mut(id)
                    {
                        layer.time_offset_frames = offset;
                    }
                }
                LayerAction::SetTimeScale(id, scale) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id)
                        && let Some(layer) = comp.layer_mut(id)
                    {
                        layer.time_scale = scale;
                    }
                }
                LayerAction::SetCompWidth(w) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.width = w;
                    }
                }
                LayerAction::SetCompHeight(h) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.height = h;
                    }
                }
                LayerAction::SetCompFramerate(num, den) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.framerate = felx_core::model::Framerate::new(num, den);
                        self.playhead.set_framerate_fps(comp.framerate.as_fps());
                    }
                }
                LayerAction::SetCompDurationFrames(d) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.duration_frames = d.max(1);
                        // Push any layer's out_frame that exceeds the new
                        // duration back to the new tail; otherwise they'd
                        // become invisible without warning.
                        for layer in comp.layers.iter_mut() {
                            if layer.out_frame > comp.duration_frames {
                                layer.out_frame = comp.duration_frames;
                            }
                        }
                        self.playhead.set_duration_frames(comp.duration_frames);
                    }
                }
                LayerAction::SetCompBackground(rgba) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.background = rgba;
                    }
                }
            }
        }
        if dirty {
            self.render_dirty = true;
            self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
        }
    }

    fn ensure_frame_rendered(&mut self, render_state: &RenderState) {
        if !self.render_dirty && self.egui_texture.is_some() {
            return;
        }
        let frame = self.playhead.current_frame();
        let texture = match self.compositor.render_cached_at(
            &self.project,
            self.comp_id,
            frame,
            self.preview_scale,
        ) {
            Ok(t) => t,
            Err(CompositorError::NoVisibleLayer) => {
                // Empty playhead; show a placeholder later. For now leave
                // texture unset.
                return;
            }
            Err(e) => {
                error!(error = %e, "compositor render failed");
                return;
            }
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = render_state.renderer.write();
        let id =
            renderer.register_native_texture(&render_state.device, &view, wgpu::FilterMode::Linear);
        if let Some(old) = self.egui_texture.replace(id) {
            renderer.free_texture(&old);
        }
        self.render_dirty = false;
    }

    fn comp_aspect(&self) -> f32 {
        let comp = self.project.composition(self.comp_id).expect("comp exists");
        comp.width as f32 / comp.height as f32
    }
}

impl App for FelxApp {
    fn update(&mut self, ctx: &Context, frame: &mut Frame) {
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        let render_state = render_state.clone();

        self.process_hot_reload();

        // Advance the playhead off real elapsed time before drawing the UI
        // so the transport bar shows the new frame.
        if self.playhead.tick() {
            self.render_dirty = true;
        }

        // Audio: keep the output buffer fed while playing; drop the stream
        // when paused. Cheap when there are no Audio layers.
        self.drive_audio();

        // Drain any completed file dialog. Spawned on a background thread
        // so the egui update loop never blocks waiting for the user to
        // pick a file. While outstanding we tick at ~10 Hz so the channel
        // is polled even when nothing else would dirty the UI.
        if self.poll_pending_file() {
            self.render_dirty = true;
        }
        if self.pending_file.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Top menu strip: file ops, preset selector, help.
        let mut chosen_preset: Option<usize> = None;
        let mut chosen_help: Option<String> = None;
        let mut do_open = false;
        let mut do_save = false;
        let mut do_save_as = false;
        let mut do_export = false;
        TopBottomPanel::top("menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open project…").clicked() {
                        do_open = true;
                        ui.close();
                    }
                    if ui.button("Save project").clicked() {
                        do_save = true;
                        ui.close();
                    }
                    if ui.button("Save project as…").clicked() {
                        do_save_as = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Export…").clicked() {
                        do_export = true;
                        ui.close();
                    }
                });
                ui.separator();
                ui.label("Presets:");
                for (i, p) in self.presets.iter().enumerate() {
                    if ui.button(&p.name).on_hover_text(&p.description).clicked() {
                        chosen_preset = Some(i);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("Help", |ui| {
                        ui.label(egui::RichText::new("Effect docs").strong());
                        ui.separator();
                        for id in EFFECT_DOC_IDS {
                            if ui.button(*id).clicked() {
                                chosen_help = Some((*id).to_string());
                                ui.close();
                            }
                        }
                    });
                });
            });
        });
        if do_open {
            self.open_project_dialog();
        }
        if do_save {
            self.save_project_dialog();
        }
        if do_save_as {
            self.current_project_path = None;
            self.save_project_dialog();
        }
        if do_export {
            // Seed a sensible default filename from the comp name so the
            // file picker proposes something useful when the user clicks
            // Choose…
            let comp_name = self
                .project
                .composition(self.comp_id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "export".into());
            if self.export_options.out_path.is_none() {
                self.export_options.out_path = None; // user picks via dialog
                let _ = comp_name; // hint for default filename — used by export thread later
            }
            self.export_dialog_open = true;
        }
        if let Some(i) = chosen_preset {
            self.apply_preset(i);
        }
        if let Some(id) = chosen_help {
            self.help_open_for = Some(id);
        }

        // Export dialog + running-job state.
        self.show_export_dialog(ctx);
        self.poll_export();
        self.show_export_progress(ctx);
        if self.export_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Help window: load the effect's README on demand.
        if let Some(id) = self.help_open_for.clone() {
            let mut still_open = true;
            egui::Window::new(format!("Help — {id}"))
                .open(&mut still_open)
                .resizable(true)
                .default_size([500.0, 400.0])
                .show(ctx, |ui| {
                    let path = effects_root_dir().join(&id).join("README.md");
                    let body = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| format!("(could not read {}: {e})", path.display()));
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(egui::RichText::new(body).monospace());
                    });
                });
            if !still_open {
                self.help_open_for = None;
            }
        }

        let transport_actions = TopBottomPanel::bottom("transport")
            .show(ctx, |ui| {
                transport::show(
                    ui,
                    &self.playhead,
                    self.preview_scale,
                    self.graph_editor_open,
                )
            })
            .inner;
        self.apply_transport_actions(transport_actions);

        if self.graph_editor_open {
            let comp_for_graph = self.project.composition(self.comp_id).expect("comp exists");
            let duration_secs =
                comp_for_graph.duration_frames as f64 / comp_for_graph.framerate.as_fps();
            let graph_time =
                FelxFrame(self.playhead.current_frame()).to_time(comp_for_graph.framerate);
            let selected_layer = self
                .selected_layer
                .and_then(|id| comp_for_graph.layers.iter().find(|l| l.id == id));
            let graph_actions = TopBottomPanel::bottom("graph_editor")
                .resizable(true)
                .default_height(220.0)
                .min_height(120.0)
                .show(ctx, |ui| {
                    graph_editor::show(
                        ui,
                        selected_layer,
                        &self.manifests,
                        graph_time,
                        duration_secs,
                    )
                })
                .inner;
            self.apply_effects_actions(graph_actions);
        }

        let (layer_actions, mask_actions) = SidePanel::left("layers")
            .resizable(true)
            .default_width(220.0)
            .min_width(180.0)
            .show(ctx, |ui| {
                let comp = self.project.composition(self.comp_id).expect("comp exists");
                let layer_actions = layers::show(ui, comp, self.selected_layer);
                let mut mask_actions: Vec<MaskAction> = Vec::new();
                if let Some(sel_id) = self.selected_layer
                    && let Some(layer) = comp.layer(sel_id)
                {
                    ui.separator();
                    mask_actions = masks::show(
                        ui,
                        sel_id,
                        &layer.masks,
                        self.pen.layer == Some(sel_id),
                        self.pen.anchors.len(),
                    );
                }
                (layer_actions, mask_actions)
            })
            .inner;
        self.apply_layer_actions(layer_actions);
        self.apply_mask_actions(mask_actions);

        let effects_actions = SidePanel::right("effects")
            .resizable(true)
            .default_width(280.0)
            .min_width(220.0)
            .show(ctx, |ui| {
                let comp = self.project.composition(self.comp_id).expect("comp exists");
                let selected_layer = self
                    .selected_layer
                    .and_then(|id| comp.layers.iter().find(|l| l.id == id));
                let time = FelxFrame(self.playhead.current_frame()).to_time(comp.framerate);
                effects::show(ui, &self.manifests, selected_layer, time)
            })
            .inner;
        self.apply_effects_actions(effects_actions);

        self.ensure_frame_rendered(&render_state);

        // Keep the loop running while playing so tick() fires regularly.
        if let Some(after) = self.playhead.repaint_after() {
            ctx.request_repaint_after(after);
        }

        CentralPanel::default()
            .frame(egui::Frame::default().fill(Color32::from_gray(15)))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                let aspect = self.comp_aspect();
                let size = fit_aspect(avail, aspect);
                let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());

                // Region-of-interest (F-048): shift-drag to set, Esc clears.
                let shift_held = ui.input(|i| i.modifiers.shift);
                let esc_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if esc_pressed {
                    self.render_region = None;
                    self.region_drag_anchor = None;
                    if self.pen.layer.is_some() {
                        self.pen.cancel();
                    }
                }

                // Pen tool (F-065): plain clicks while a pen session is
                // active drop corner anchors. Click near the first anchor
                // to close the path and commit a new mask. Tangent-handle
                // dragging is a polish follow-up.
                if let Some(pen_layer) = self.pen.layer
                    && response.clicked()
                    && !shift_held
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    let comp_for_pen = self.project.composition(self.comp_id).expect("comp exists");
                    let comp_w = comp_for_pen.width as f32;
                    let comp_h = comp_for_pen.height as f32;
                    let u = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                    let v = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                    let cx = u * comp_w;
                    let cy = v * comp_h;
                    let close_threshold_px = 12.0;
                    let close_threshold_comp = close_threshold_px * (comp_w / rect.width());
                    let should_close = self.pen.anchors.len() >= 3 && {
                        let first = self.pen.anchors[0].anchor;
                        let dx = first[0] - cx;
                        let dy = first[1] - cy;
                        (dx * dx + dy * dy).sqrt() <= close_threshold_comp
                    };
                    if should_close {
                        if let Some(path) = self.pen.close_into_path()
                            && let Some(comp) = self.project.composition_mut(self.comp_id)
                            && let Some(layer) = comp.layer_mut(pen_layer)
                        {
                            let mask = felx_core::model::Mask {
                                name: format!("Pen mask {}", layer.masks.len() + 1),
                                mode: felx_core::model::MaskMode::default(),
                                opacity: 1.0,
                                expansion: 0.0,
                                feather: 0.0,
                                path: felx_core::model::Curve::Static(path),
                            };
                            layer.masks.push(mask);
                            self.render_dirty = true;
                            self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
                        }
                    } else {
                        self.pen.add_anchor(cx, cy);
                    }
                }
                if response.drag_started() && shift_held {
                    if let Some(pos) = response.interact_pointer_pos() {
                        self.region_drag_anchor = Some(viewer_to_norm(rect, pos));
                    }
                } else if response.dragged() && self.region_drag_anchor.is_some() {
                    if let (Some(pos), Some(anchor)) =
                        (response.interact_pointer_pos(), self.region_drag_anchor)
                    {
                        let now = viewer_to_norm(rect, pos);
                        let region = RegionRect::from_corners(anchor, now);
                        if !region.is_degenerate() {
                            self.render_region = Some(region);
                        }
                    }
                } else if response.drag_stopped() {
                    self.region_drag_anchor = None;
                }

                if let Some(id) = self.egui_texture {
                    let painter = ui.painter_at(rect);
                    let uv = match self.render_region {
                        Some(r) => egui::Rect::from_min_max(
                            egui::pos2(r.u_min, r.v_min),
                            egui::pos2(r.u_max, r.v_max),
                        ),
                        None => {
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
                        }
                    };
                    painter.image(id, rect, uv, Color32::WHITE);

                    // Region overlay: subtle dimming outside the region and a
                    // bright outline around it. Only meaningful when set.
                    if let Some(r) = self.render_region {
                        let stroke = egui::Stroke::new(1.5, Color32::from_rgb(255, 220, 80));
                        painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
                        let label = format!(
                            "ROI {:.0}×{:.0}% · Esc to clear",
                            (r.u_max - r.u_min) * 100.0,
                            (r.v_max - r.v_min) * 100.0
                        );
                        painter.text(
                            rect.left_top() + egui::vec2(8.0, 8.0),
                            egui::Align2::LEFT_TOP,
                            label,
                            egui::FontId::monospace(11.0),
                            Color32::from_rgb(255, 220, 80),
                        );
                    } else if shift_held {
                        // Subtle hint: shift-drag draws a region.
                        painter.text(
                            rect.right_top() + egui::vec2(-8.0, 8.0),
                            egui::Align2::RIGHT_TOP,
                            "shift-drag to set ROI",
                            egui::FontId::monospace(11.0),
                            Color32::from_gray(200),
                        );
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("(no frame)")
                                .color(Color32::GRAY)
                                .italics(),
                        );
                    });
                }

                // Pen-tool overlay: draw anchors and the polyline so far.
                if self.pen.layer.is_some() && !self.pen.anchors.is_empty() {
                    let comp_for_pen = self.project.composition(self.comp_id).expect("comp exists");
                    let comp_w = comp_for_pen.width as f32;
                    let comp_h = comp_for_pen.height as f32;
                    let painter = ui.painter_at(rect);
                    let to_screen = |x: f32, y: f32| -> egui::Pos2 {
                        egui::pos2(
                            rect.left() + (x / comp_w) * rect.width(),
                            rect.top() + (y / comp_h) * rect.height(),
                        )
                    };
                    let mut prev: Option<egui::Pos2> = None;
                    for v in &self.pen.anchors {
                        let p = to_screen(v.anchor[0], v.anchor[1]);
                        if let Some(prev_p) = prev {
                            painter.line_segment(
                                [prev_p, p],
                                egui::Stroke::new(1.0, Color32::from_rgb(255, 220, 80)),
                            );
                        }
                        painter.circle_filled(p, 4.0, Color32::from_rgb(255, 220, 80));
                        painter.circle_stroke(p, 4.0, egui::Stroke::new(1.0, Color32::BLACK));
                        prev = Some(p);
                    }
                    if let Some(first) = self.pen.anchors.first() {
                        let p_first = to_screen(first.anchor[0], first.anchor[1]);
                        painter.circle_stroke(
                            p_first,
                            8.0,
                            egui::Stroke::new(1.0, Color32::from_rgb(255, 220, 80)),
                        );
                    }
                }

                if let Some(err) = self.shader_error.as_deref() {
                    let overlay_rect = egui::Rect::from_two_pos(
                        rect.left_top() + egui::vec2(8.0, 8.0),
                        rect.right_top() + egui::vec2(-8.0, 8.0 + 64.0),
                    );
                    let painter = ui.painter_at(overlay_rect);
                    painter.rect_filled(overlay_rect, 4.0, Color32::from_black_alpha(220));
                    painter.text(
                        overlay_rect.left_top() + egui::vec2(8.0, 6.0),
                        egui::Align2::LEFT_TOP,
                        format!("⚠ shader compile error\n{err}"),
                        egui::FontId::monospace(11.0),
                        Color32::from_rgb(255, 180, 100),
                    );
                }
            });
    }
}

/// Effect IDs whose README.md is shown in the Help menu (F-090). Update
/// when adding new effects.
const EFFECT_DOC_IDS: &[&str] = &[
    "gain",
    "invert",
    "cc_toner",
    "signal",
    "squint_diffusion",
    "crt",
    "vhs",
    "crt_persistence",
    "bloom",
];

/// Locate the workspace `effects/` directory. CARGO_MANIFEST_DIR points at
/// `crates/felx-app/`; the effects live two levels up.
fn effects_root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("effects")
}

fn build_renderer(render_state: &RenderState) -> Renderer {
    let info = AdapterInfo::from(render_state.adapter.get_info());
    Renderer::from_borrowed(
        render_state.device.clone(),
        render_state.queue.clone(),
        info,
    )
}

/// Default placeholder project until file-open lands. A 1280x720 / 30fps
/// comp with a slate-blue solid layer and a Gain effect (defaulted from
/// the manifest if loaded, otherwise the bare `Effect::new` default).
fn default_project(manifests: &ManifestRegistry) -> (Project, CompId) {
    let mut project = Project::new();
    let comp_id = project.add_composition("preview", 1280, 720);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.duration_frames = 600;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    let layer = comp.add_solid("background", [0.18, 0.22, 0.32, 1.0]);
    let gain_effect = manifests
        .get("gain")
        .map(Effect::from_manifest)
        .unwrap_or_else(|| Effect::new("gain"));
    comp.push_effect(layer, gain_effect);
    (project, comp_id)
}

/// Convert a screen position inside the viewer rect to normalized comp UV
/// (0..1 origin top-left).
fn viewer_to_norm(rect: egui::Rect, pos: egui::Pos2) -> egui::Vec2 {
    egui::vec2(
        ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
    )
}

/// Largest box fitting `avail` while preserving `aspect` (= w/h).
fn fit_aspect(avail: Vec2, aspect: f32) -> Vec2 {
    if avail.x <= 0.0 || avail.y <= 0.0 || aspect <= 0.0 {
        return Vec2::ZERO;
    }
    if avail.x / avail.y > aspect {
        Vec2::new(avail.y * aspect, avail.y)
    } else {
        Vec2::new(avail.x, avail.x / aspect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_aspect_handles_wider_avail() {
        // avail wider than aspect → fit by height.
        let s = fit_aspect(Vec2::new(800.0, 400.0), 1.0);
        assert!((s.x - 400.0).abs() < 0.001);
        assert!((s.y - 400.0).abs() < 0.001);
    }

    #[test]
    fn fit_aspect_handles_taller_avail() {
        // avail taller than aspect → fit by width.
        let s = fit_aspect(Vec2::new(400.0, 800.0), 1.0);
        assert!((s.x - 400.0).abs() < 0.001);
        assert!((s.y - 400.0).abs() < 0.001);
    }

    #[test]
    fn fit_aspect_widescreen_in_square_box() {
        // 16:9 in a square: should be width-limited.
        let s = fit_aspect(Vec2::new(800.0, 800.0), 16.0 / 9.0);
        assert!((s.x - 800.0).abs() < 0.001);
        assert!((s.y - 450.0).abs() < 0.001);
    }

    #[test]
    fn fit_aspect_zero_inputs_return_zero() {
        assert_eq!(fit_aspect(Vec2::ZERO, 1.0), Vec2::ZERO);
        assert_eq!(fit_aspect(Vec2::new(100.0, 0.0), 1.0), Vec2::ZERO);
        assert_eq!(fit_aspect(Vec2::new(100.0, 100.0), 0.0), Vec2::ZERO);
    }
}
