//! Shared video-with-audio export loop used by the CLI render runner and
//! the GUI export worker. Renders the comp frame-by-frame through the
//! compositor, encodes via [`H264Encoder`], and — when the comp has Audio
//! layers — mixes the audio bus once up front and feeds it to the muxed
//! audio track interleaved with the video frames (bounded muxer buffering).

use crate::audio_export::{AudioExportError, mix_comp_audio};
use crate::compositor::{Compositor, CompositorError};
use crate::texture_io::download_image;
use felx_core::model::{CompId, Project};
use felx_media::{AudioEncodeOptions, DecodeError, EncodeOptions, H264Encoder};
use std::path::Path;
use tracing::info;

/// Master rate for the muxed audio track. Matches the WAV export default.
pub const EXPORT_AUDIO_RATE: u32 = 48_000;

#[derive(Debug)]
pub enum VideoExportError {
    UnknownComposition,
    Compositor(CompositorError),
    Audio(AudioExportError),
    Encode(DecodeError),
}

impl std::fmt::Display for VideoExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoExportError::UnknownComposition => write!(f, "unknown composition"),
            VideoExportError::Compositor(e) => write!(f, "compositor: {e}"),
            VideoExportError::Audio(e) => write!(f, "audio: {e}"),
            VideoExportError::Encode(e) => write!(f, "encode: {e}"),
        }
    }
}

impl std::error::Error for VideoExportError {}

impl From<CompositorError> for VideoExportError {
    fn from(e: CompositorError) -> Self {
        VideoExportError::Compositor(e)
    }
}
impl From<AudioExportError> for VideoExportError {
    fn from(e: AudioExportError) -> Self {
        VideoExportError::Audio(e)
    }
}
impl From<DecodeError> for VideoExportError {
    fn from(e: DecodeError) -> Self {
        VideoExportError::Encode(e)
    }
}

/// Render every frame of `comp_id` and encode to `out_path` with `opts`,
/// muxing the comp's audio bus when Audio layers exist. `progress` is
/// called after each encoded frame with `(done, total)`.
pub fn export_video(
    compositor: &mut Compositor,
    project: &Project,
    comp_id: CompId,
    out_path: &Path,
    opts: EncodeOptions,
    mut progress: impl FnMut(u32, u32),
) -> Result<(), VideoExportError> {
    let comp = project
        .composition(comp_id)
        .ok_or(VideoExportError::UnknownComposition)?;
    let duration = comp.duration_frames;
    let fps = comp.framerate.as_fps().max(1e-6);

    let audio_pcm = mix_comp_audio(project, comp_id, EXPORT_AUDIO_RATE)?;
    let audio_opts = audio_pcm.as_ref().map(|_| AudioEncodeOptions {
        sample_rate: EXPORT_AUDIO_RATE,
    });
    if audio_pcm.is_some() {
        info!(rate = EXPORT_AUDIO_RATE, "muxing comp audio into export");
    }

    let mut enc = H264Encoder::create_with_audio(out_path, opts, audio_opts)?;

    // Interleave: after each video frame, feed audio samples up to that
    // frame's end time so the muxer never has to buffer one stream deeply.
    let mut audio_cursor = 0usize; // interleaved f32 index
    for frame in 0..duration {
        let tex = compositor.render_cached(project, comp_id, frame)?;
        let img = download_image(compositor.renderer(), &tex);
        enc.write_rgba(img.as_raw())?;

        if let Some(pcm) = &audio_pcm {
            let end_secs = (frame + 1) as f64 / fps;
            let end = ((end_secs * EXPORT_AUDIO_RATE as f64).round() as usize * 2).min(pcm.len());
            if end > audio_cursor {
                enc.write_audio_interleaved(&pcm[audio_cursor..end])?;
                audio_cursor = end;
            }
        }
        progress(frame + 1, duration);
    }
    if let Some(pcm) = &audio_pcm
        && audio_cursor < pcm.len()
    {
        enc.write_audio_interleaved(&pcm[audio_cursor..])?;
    }
    enc.finish()?;
    Ok(())
}
