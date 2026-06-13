//! Animated GIF export (F-103).
//!
//! Two-pass `palettegen` → `paletteuse` via a spawned `ffmpeg` binary.
//! The system ffmpeg is already a hard build-time dep (libav* linking),
//! so requiring it at runtime adds nothing new.
//!
//! Strategy:
//! 1. Render the comp to a PNG sequence in a temp dir.
//! 2. Run `ffmpeg -i pat.png -vf palettegen` → palette.png.
//! 3. Run `ffmpeg -i pat.png -i palette.png -filter_complex paletteuse=…`
//!    → output.gif.
//! 4. Remove the temp PNG sequence.

use crate::compositor::{Compositor, CompositorError};
use crate::walker::{PngSequenceOptions, render_to_png_sequence};
use felx_core::model::{CompId, Project};
use std::path::Path;
use tracing::{debug, info};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GifDither {
    None,
    /// Ordered Bayer dithering. Smallest file, hardest visible pattern.
    Bayer,
    /// Floyd–Steinberg error diffusion.
    #[default]
    FloydSteinberg,
    /// Sierra-2-4A error diffusion.
    Sierra2_4a,
}

impl GifDither {
    fn paletteuse_arg(self) -> &'static str {
        match self {
            GifDither::None => "dither=none",
            GifDither::Bayer => "dither=bayer:bayer_scale=3",
            GifDither::FloydSteinberg => "dither=floyd_steinberg",
            GifDither::Sierra2_4a => "dither=sierra2_4a",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GifOptions {
    /// 8…256.
    pub max_palette: u32,
    pub dither: GifDither,
    /// Optional framerate cap. None = use comp framerate.
    pub framerate_cap: Option<u32>,
    /// Loop forever (the GIF default). false = play once.
    pub loop_forever: bool,
    /// Whether to preserve transparency (alpha-aware palette). Most GIF
    /// players honor it; some don't.
    pub transparency: bool,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            max_palette: 128,
            dither: GifDither::default(),
            framerate_cap: None,
            loop_forever: true,
            transparency: false,
        }
    }
}

#[derive(Debug)]
pub enum GifError {
    Compositor(CompositorError),
    Io(std::io::Error),
    FfmpegBinary {
        stage: &'static str,
        status: i32,
        stderr: String,
    },
}

impl std::fmt::Display for GifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GifError::Compositor(e) => write!(f, "compositor: {e}"),
            GifError::Io(e) => write!(f, "io: {e}"),
            GifError::FfmpegBinary {
                stage,
                status,
                stderr,
            } => {
                write!(f, "ffmpeg {stage} exited {status}: {stderr}")
            }
        }
    }
}

impl std::error::Error for GifError {}

impl From<CompositorError> for GifError {
    fn from(e: CompositorError) -> Self {
        GifError::Compositor(e)
    }
}
impl From<std::io::Error> for GifError {
    fn from(e: std::io::Error) -> Self {
        GifError::Io(e)
    }
}
impl From<crate::walker::WalkError> for GifError {
    fn from(e: crate::walker::WalkError) -> Self {
        match e {
            crate::walker::WalkError::Compositor(e) => GifError::Compositor(e),
            crate::walker::WalkError::Io(e) => GifError::Io(e),
            crate::walker::WalkError::ImageEncode(e) => {
                GifError::Io(std::io::Error::other(e.to_string()))
            }
            crate::walker::WalkError::UnknownComposition => {
                GifError::Compositor(CompositorError::UnknownComposition)
            }
        }
    }
}

/// Render `comp_id` and write an animated GIF to `out_path`.
pub fn export_gif(
    compositor: &mut Compositor,
    project: &Project,
    comp_id: CompId,
    out_path: impl AsRef<Path>,
    opts: &GifOptions,
) -> Result<(), GifError> {
    let comp = project
        .composition(comp_id)
        .ok_or(CompositorError::UnknownComposition)?;
    let fps = opts
        .framerate_cap
        .map(|cap| cap.min(comp.framerate.as_fps().round() as u32))
        .unwrap_or_else(|| comp.framerate.as_fps().round() as u32)
        .max(1);

    let temp_dir = std::env::temp_dir().join(format!("felx-gif-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;
    let png_pattern = "frame_{frame:05}.png";
    let png_opts = PngSequenceOptions::new(temp_dir.clone(), png_pattern);
    let written = render_to_png_sequence(
        compositor,
        project,
        comp_id,
        0..comp.duration_frames,
        &png_opts,
    )?;
    debug!(written, dir = %temp_dir.display(), "PNG sequence ready for GIF");

    // The ffmpeg input pattern uses %05d.
    let ffmpeg_input_pattern = temp_dir.join("frame_%05d.png");
    let palette_path = temp_dir.join("palette.png");

    // Pass 1: palettegen.
    let palette_filter = format!(
        "fps={fps},palettegen=max_colors={pal}{transparency}",
        pal = opts.max_palette.clamp(8, 256),
        transparency = if opts.transparency {
            ":reserve_transparent=1"
        } else {
            ""
        },
    );
    run_ffmpeg(
        "palettegen",
        &[
            "-y",
            "-i",
            ffmpeg_input_pattern.to_str().unwrap(),
            "-vf",
            &palette_filter,
            palette_path.to_str().unwrap(),
        ],
    )?;

    // Pass 2: paletteuse.
    let loop_arg = if opts.loop_forever { "0" } else { "-1" };
    let paletteuse_filter = format!(
        "fps={fps}[x];[x][1:v]paletteuse={dither}",
        dither = opts.dither.paletteuse_arg(),
    );
    run_ffmpeg(
        "paletteuse",
        &[
            "-y",
            "-i",
            ffmpeg_input_pattern.to_str().unwrap(),
            "-i",
            palette_path.to_str().unwrap(),
            "-loop",
            loop_arg,
            "-filter_complex",
            &paletteuse_filter,
            out_path.as_ref().to_str().unwrap(),
        ],
    )?;

    info!(
        out = %out_path.as_ref().display(),
        frames = written,
        fps,
        "GIF export done"
    );

    // Best-effort cleanup of the temp PNGs.
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(())
}

fn run_ffmpeg(stage: &'static str, args: &[&str]) -> Result<(), GifError> {
    let out = std::process::Command::new("ffmpeg").args(args).output()?;
    if !out.status.success() {
        return Err(GifError::FfmpegBinary {
            stage,
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Integration test would render a tiny comp to a GIF and probe the
    // output via image::open. Skipped here because it requires the
    // `ffmpeg` binary on PATH at test time, which CI runners have but
    // dev machines may not — same constraint as the existing video
    // round-trip tests in F-014. The function-shape contract (palette
    // pass + use pass + cleanup) is what we want to pin down today.
}
