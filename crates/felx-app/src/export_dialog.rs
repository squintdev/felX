//! Export dialog state + worker thread.
//!
//! When the user picks File → Export…, [`ExportDialog`] tracks the chosen
//! format and output path. Hitting "Export" spawns a worker that builds
//! its *own* headless `Compositor` (via `Renderer::new_headless`) so the
//! GUI's wgpu device stays uncontended, then runs the appropriate export
//! pipeline frame by frame. Progress messages flow back through an mpsc
//! channel that the GUI poll loop drains.

use felx_core::model::{CompId, Project};
use felx_media::{EncodeOptions, HwEncoder, RateControl, VideoCodec, WavBitDepth};
use felx_render::audio_export::export_wav;
use felx_render::compositor::Compositor;
use felx_render::gif_export::{GifDither, GifOptions, export_gif};
use felx_render::video_export::export_video;
use felx_render::walker::{
    ExrSequenceOptions, PngSequenceOptions, render_to_exr_sequence, render_to_png_sequence,
};
use felx_render::{Renderer, RendererOptions};
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    H264,
    H265,
    Prores422,
    Prores4444,
    Gif,
    PngSequence,
    ExrSequence,
    Wav,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::H264 => "H.264 / MP4",
            ExportFormat::H265 => "H.265 / MP4",
            ExportFormat::Prores422 => "ProRes 422 / MOV",
            ExportFormat::Prores4444 => "ProRes 4444 / MOV",
            ExportFormat::Gif => "Animated GIF",
            ExportFormat::PngSequence => "PNG sequence",
            ExportFormat::ExrSequence => "EXR sequence",
            ExportFormat::Wav => "WAV audio only",
        }
    }

    pub const ALL: [ExportFormat; 8] = [
        ExportFormat::H264,
        ExportFormat::H265,
        ExportFormat::Prores422,
        ExportFormat::Prores4444,
        ExportFormat::Gif,
        ExportFormat::PngSequence,
        ExportFormat::ExrSequence,
        ExportFormat::Wav,
    ];

    /// File extension the format needs on its output path. ffmpeg picks
    /// the muxer (mp4 vs mov vs etc.) from the extension, so getting this
    /// right is the difference between a clean export and an EINVAL.
    /// Sequence formats return None — the output is a directory.
    pub fn required_extension(self) -> Option<&'static str> {
        match self {
            ExportFormat::H264 | ExportFormat::H265 => Some("mp4"),
            ExportFormat::Prores422 | ExportFormat::Prores4444 => Some("mov"),
            ExportFormat::Gif => Some("gif"),
            ExportFormat::Wav => Some("wav"),
            ExportFormat::PngSequence | ExportFormat::ExrSequence => None,
        }
    }
}

/// Append the format's expected extension if `path` doesn't already
/// carry it (case-insensitive). Returns `(normalized_path, was_changed)`.
/// For video formats we accept either `mp4` / `mov` / `m4v` / `mkv` as
/// valid containers; for everything else we require the exact extension.
pub fn ensure_extension(format: ExportFormat, path: PathBuf) -> (PathBuf, bool) {
    let Some(want) = format.required_extension() else {
        return (path, false);
    };
    let have = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    if matches!(have.as_deref(), Some(e) if e == want) {
        return (path, false);
    }
    // Accept other video containers if the user explicitly typed one.
    let video_compat = matches!(
        format,
        ExportFormat::H264
            | ExportFormat::H265
            | ExportFormat::Prores422
            | ExportFormat::Prores4444
    ) && matches!(
        have.as_deref(),
        Some("mp4") | Some("mov") | Some("m4v") | Some("mkv")
    );
    if video_compat {
        return (path, false);
    }
    let new_name = match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => format!("{name}.{want}"),
        None => format!("export.{want}"),
    };
    let mut new_path = path.clone();
    new_path.set_file_name(new_name);
    (new_path, true)
}

/// User-tunable parameters per export. Live in the dialog and get cloned
/// into the worker on Export.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub out_path: Option<PathBuf>,
    pub crf: u32,
    pub preset: String,
    pub gif_palette: u32,
    pub gif_dither: GifDither,
    pub wav_depth: WavBitDepth,
    /// Threaded through to `RendererOptions::gpu_name_pref` when the
    /// worker spawns its headless renderer. None = automatic selection.
    pub gpu_name_pref: Option<String>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::H264,
            out_path: None,
            crf: 18,
            preset: "medium".to_string(),
            gif_palette: 128,
            gif_dither: GifDither::FloydSteinberg,
            wav_depth: WavBitDepth::Pcm16,
            gpu_name_pref: None,
        }
    }
}

/// Messages the worker thread sends to the GUI poll loop.
#[derive(Clone, Debug)]
pub enum ExportProgress {
    Started { total_frames: u32 },
    Frame { done: u32, total: u32 },
    Done,
    Failed(String),
}

pub struct ExportJob {
    pub rx: mpsc::Receiver<ExportProgress>,
    /// Format used for this run. Retained for the "done" toast and to
    /// help diagnose progress messages in tests.
    #[allow(dead_code)]
    pub format: ExportFormat,
    /// Path the worker is writing to. Same rationale as `format`.
    #[allow(dead_code)]
    pub out_path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct ExportStatus {
    pub done: u32,
    pub total: u32,
    pub finished: bool,
    pub error: Option<String>,
}

impl ExportStatus {
    pub fn pct(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
}

/// Spawn the export worker. Clones the project into a worker thread that
/// builds its own headless renderer + compositor, so the GUI's wgpu
/// device stays uncontended.
pub fn spawn_export(
    project: Project,
    comp_id: CompId,
    mut opts: ExportOptions,
) -> Result<ExportJob, String> {
    let Some(raw_path) = opts.out_path.clone() else {
        return Err("no output path".into());
    };
    // ffmpeg picks the muxer from the file extension. Auto-append the
    // expected one so a user-typed path like "recursiva-felx" with no
    // extension becomes "recursiva-felx.mp4" instead of failing the
    // encoder open with EINVAL.
    let (out_path, changed) = ensure_extension(opts.format, raw_path);
    if changed {
        tracing::info!(path = ?out_path, "appended extension to output path");
    }
    opts.out_path = Some(out_path.clone());
    let format = opts.format;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Err(e) = run_export(project, comp_id, opts, &tx) {
            let _ = tx.send(ExportProgress::Failed(e));
        } else {
            let _ = tx.send(ExportProgress::Done);
        }
    });
    Ok(ExportJob {
        rx,
        format,
        out_path,
    })
}

fn run_export(
    project: Project,
    comp_id: CompId,
    opts: ExportOptions,
    tx: &mpsc::Sender<ExportProgress>,
) -> Result<(), String> {
    let comp = project
        .composition(comp_id)
        .ok_or("comp not found in project")?;
    let (w, h) = (comp.width, comp.height);
    let (fps_num, fps_den) = (comp.framerate.0.num, comp.framerate.0.den);
    let dur = comp.duration_frames;
    let comp_name = comp.name.clone();
    let _ = comp_name; // reserved for status messages
    let out_path = opts.out_path.ok_or_else(|| "no output path".to_string())?;

    let renderer = Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        gpu_name_pref: opts.gpu_name_pref.clone(),
        ..Default::default()
    })
    .map_err(|e| format!("renderer init: {e}"))?;
    let mut compositor = Compositor::new(renderer);

    let _ = tx.send(ExportProgress::Started { total_frames: dur });

    match opts.format {
        ExportFormat::H264
        | ExportFormat::H265
        | ExportFormat::Prores422
        | ExportFormat::Prores4444 => {
            let mut enc_opts = match opts.format {
                ExportFormat::H264 => EncodeOptions::h264_default(w, h, fps_num, fps_den),
                ExportFormat::H265 => EncodeOptions::h265_default(w, h, fps_num, fps_den),
                ExportFormat::Prores422 => EncodeOptions::prores422_default(w, h, fps_num, fps_den),
                ExportFormat::Prores4444 => {
                    EncodeOptions::prores4444_default(w, h, fps_num, fps_den)
                }
                _ => unreachable!(),
            };
            // Override the CRF/preset knobs for the lossy codecs.
            if matches!(opts.format, ExportFormat::H264 | ExportFormat::H265) {
                enc_opts.rate_control = RateControl::Crf;
                enc_opts.crf = opts.crf;
                if !opts.preset.is_empty() {
                    enc_opts.preset = opts.preset.clone();
                }
            }
            enc_opts.hw = HwEncoder::Software;
            let _ = enc_opts.codec; // silence unused-import warnings if codec stays Video*
            let _: VideoCodec = enc_opts.codec;

            export_video(
                &mut compositor,
                &project,
                comp_id,
                &out_path,
                enc_opts,
                |done, total| {
                    let _ = tx.send(ExportProgress::Frame { done, total });
                },
            )
            .map_err(|e| e.to_string())?;
        }
        ExportFormat::Gif => {
            // GIF export already renders the full PNG sequence internally
            // and shells out to ffmpeg; we can't tap per-frame progress
            // mid-way without restructuring. Report frame=0/total at
            // start, then jump to done at end.
            let gif_opts = GifOptions {
                max_palette: opts.gif_palette,
                dither: opts.gif_dither,
                framerate_cap: None,
                loop_forever: true,
                transparency: false,
            };
            export_gif(&mut compositor, &project, comp_id, &out_path, &gif_opts)
                .map_err(|e| format!("gif: {e}"))?;
            let _ = tx.send(ExportProgress::Frame {
                done: dur,
                total: dur,
            });
        }
        ExportFormat::PngSequence => {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("mkdir out: {e}"))?;
            // Render frame-by-frame so we can report progress.
            for frame in 0..dur {
                let pattern = "frame_{frame:05}.png";
                let opts = PngSequenceOptions::new(out_path.clone(), pattern.to_string());
                render_to_png_sequence(&mut compositor, &project, comp_id, frame..frame + 1, &opts)
                    .map_err(|e| format!("png frame {frame}: {e}"))?;
                let _ = tx.send(ExportProgress::Frame {
                    done: frame + 1,
                    total: dur,
                });
            }
        }
        ExportFormat::ExrSequence => {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("mkdir out: {e}"))?;
            for frame in 0..dur {
                let pattern = "frame_{frame:05}.exr";
                let opts = ExrSequenceOptions::new(out_path.clone(), pattern.to_string());
                render_to_exr_sequence(&mut compositor, &project, comp_id, frame..frame + 1, &opts)
                    .map_err(|e| format!("exr frame {frame}: {e}"))?;
                let _ = tx.send(ExportProgress::Frame {
                    done: frame + 1,
                    total: dur,
                });
            }
        }
        ExportFormat::Wav => {
            export_wav(&project, comp_id, &out_path, 48_000, opts.wav_depth)
                .map_err(|e| format!("wav: {e}"))?;
            let _ = tx.send(ExportProgress::Frame {
                done: dur,
                total: dur,
            });
        }
    }
    Ok(())
}
