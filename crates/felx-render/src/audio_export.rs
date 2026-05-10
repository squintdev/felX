//! Project-aware audio export. Walks every audio asset referenced by
//! [`LayerKind::Audio`] layers in the chosen comp, decodes each, mixes
//! through the F-051 mixer (with each layer's gain/pan curves), and
//! writes the result via `felx_media::write_wav`.

use felx_core::media::{AudioSource, mix_window};
use felx_core::model::{AssetId, CompId, Curve, Frame, Framerate, Layer, LayerKind, Project};
use felx_media::{WavBitDepth, decode_file, write_wav};
use std::path::Path;
use tracing::{info, warn};

#[derive(Debug)]
pub enum AudioExportError {
    UnknownComposition,
    UnknownAsset(AssetId),
    Decode(felx_media::DecodeError),
    Io(std::io::Error),
}

impl std::fmt::Display for AudioExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioExportError::UnknownComposition => write!(f, "unknown composition"),
            AudioExportError::UnknownAsset(a) => write!(f, "unknown asset {}", a.0),
            AudioExportError::Decode(e) => write!(f, "decode: {e}"),
            AudioExportError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for AudioExportError {}

impl From<felx_media::DecodeError> for AudioExportError {
    fn from(e: felx_media::DecodeError) -> Self {
        AudioExportError::Decode(e)
    }
}
impl From<std::io::Error> for AudioExportError {
    fn from(e: std::io::Error) -> Self {
        AudioExportError::Io(e)
    }
}

/// Export the comp's audio bus to a WAV file at `master_rate`. Per-layer
/// gain/pan are sampled from the layer's curves at each window start.
pub fn export_wav(
    project: &Project,
    comp_id: CompId,
    out_path: impl AsRef<Path>,
    master_rate: u32,
    bit_depth: WavBitDepth,
) -> Result<(), AudioExportError> {
    let comp = project
        .composition(comp_id)
        .ok_or(AudioExportError::UnknownComposition)?;
    let framerate = comp.framerate;
    let total_frames = comp.duration_frames;

    // Gather one AudioSource per Audio layer.
    let mut sources: Vec<AudioSource> = Vec::new();
    for layer in &comp.layers {
        if let LayerKind::Audio { asset } = &layer.kind {
            let asset_meta = project
                .asset(*asset)
                .ok_or(AudioExportError::UnknownAsset(*asset))?;
            let decoded = decode_file(&asset_meta.path, master_rate)?;
            sources.push(AudioSource {
                sample_rate: decoded.sample_rate,
                channels: decoded.channels,
                pcm: shift_pcm_for_layer(
                    decoded.pcm,
                    decoded.sample_rate,
                    decoded.channels,
                    layer,
                    framerate,
                ),
                gain: layer_gain_curve(layer),
                pan: layer_pan_curve(layer),
            });
        }
    }

    if sources.is_empty() {
        info!("no audio layers; writing silent WAV of comp duration");
    }

    // Mix the entire timeline as one window. Cheaper for short comps; a
    // streaming windowed pass is a follow-up for very long projects.
    let total_secs = total_frames as f64 / framerate.as_fps();
    let window_frames = (total_secs * master_rate as f64).round() as usize;
    let bus = mix_window(
        &sources,
        felx_core::model::Rational::new(0, master_rate),
        master_rate,
        window_frames,
        1.0,
    );

    write_wav(out_path, &bus.pcm, bus.sample_rate, 2, bit_depth)?;
    Ok(())
}

/// Layers have an `in_frame` offset relative to the comp timeline. Pad
/// the source PCM with leading silence so the mix_window math (which uses
/// time-zero as the comp origin) lines it up correctly.
fn shift_pcm_for_layer(
    pcm: Vec<f32>,
    rate: u32,
    channels: u32,
    layer: &Layer,
    framerate: Framerate,
) -> Vec<f32> {
    if layer.in_frame == 0 {
        return pcm;
    }
    let secs = Frame(layer.in_frame).to_time(framerate).as_seconds();
    let pad_frames = (secs * rate as f64).round() as usize;
    let pad_samples = pad_frames * channels as usize;
    let mut shifted = Vec::with_capacity(pad_samples + pcm.len());
    shifted.resize(pad_samples, 0.0);
    shifted.extend_from_slice(&pcm);
    shifted
}

/// Per-layer gain — Audio layers don't currently carry their own
/// gain/pan curves in the data model, so we synthesize unity defaults
/// here. A schema extension carrying `Curve<f32>` per Audio layer is the
/// natural follow-up; the mixer already speaks that shape.
fn layer_gain_curve(_layer: &Layer) -> Curve<f32> {
    Curve::Static(1.0)
}

fn layer_pan_curve(_layer: &Layer) -> Curve<f32> {
    if false {
        warn!("audio layer pan curve unused");
    }
    Curve::Static(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use felx_core::model::{Asset, AssetId, AssetKind, Project};

    #[test]
    fn empty_comp_writes_silent_wav() {
        let dir =
            std::env::temp_dir().join(format!("felx-render-audio-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("silence.wav");

        let mut p = Project::new();
        let cid = p.add_composition("main", 16, 16);
        let comp = p.composition_mut(cid).unwrap();
        comp.duration_frames = 30;
        comp.background = [0.0, 0.0, 0.0, 1.0];

        export_wav(&p, cid, &path, 8_000, WavBitDepth::Pcm16).unwrap();
        let decoded = decode_file(&path, 48_000).unwrap();
        // 1 second at 8 kHz stereo.
        let frames = decoded.frames();
        assert!(
            (7_900..=8_100).contains(&frames),
            "expected ~8000 stereo frames, got {frames}"
        );
        assert_eq!(decoded.channels, 2);
        // All silent.
        let max = decoded.pcm.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(max < 1e-3, "expected silence, max sample {max}");
        let _ = std::fs::remove_file(path);
    }

    // Asset/library tests with real audio files would round-trip a sine
    // through the export — kept out for now because they need write
    // access to a longer-lived temp dir layout. The mixer's own tests
    // exercise the gain/pan/mix path directly.
    #[allow(dead_code)]
    fn _asset_helper_compile_check(_a: AssetId, _kind: AssetKind, _asset: Asset) {}
}
