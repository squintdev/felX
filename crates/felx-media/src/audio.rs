//! Audio decode (F-050) and types (F-051).
//!
//! Decodes any container ffmpeg can read into interleaved stereo `f32` PCM
//! at a project-level master sample rate (default 48 kHz). The decode
//! resamples and channel-mixes during decoding so callers always get a
//! single canonical buffer shape, regardless of source format.

use crate::error::DecodeError;
use ffmpeg_next as ffmpeg;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info};

/// Master sample rate. 48 kHz is the desktop / video standard; resampling
/// every input to this on decode keeps the mixer math trivial.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

/// One decoded audio asset. `pcm` is interleaved f32 PCM at `sample_rate`
/// with `channels` channels. The mixer (F-051) is responsible for any
/// rate / channel conversion to the master bus — keeps the decoder out
/// of swresample, which has historically been the source of misery.
#[derive(Clone, Debug)]
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub channels: u32,
    /// Interleaved f32 PCM in `[-1.0, 1.0]`. Length = frames * channels.
    pub pcm: Vec<f32>,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.pcm.len() / self.channels as usize
        }
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames() as f64 / self.sample_rate.max(1) as f64)
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub duration: Duration,
}

/// Decode an entire audio file. Returns interleaved f32 PCM at the
/// source's native sample rate and channel count. Sample-rate / channel
/// conversion to the master bus is the mixer's job (F-051) — that keeps
/// this path out of swresample, which has been a source of API churn.
///
/// `_target_rate` is reserved for a future master-rate path; ignored in
/// v1.
pub fn decode_file(path: impl AsRef<Path>, _target_rate: u32) -> Result<DecodedAudio, DecodeError> {
    init_ffmpeg();
    let path = path.as_ref();
    let mut ictx = ffmpeg::format::input(path).map_err(DecodeError::Open)?;

    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or(DecodeError::NoAudioStream)?;
    let stream_index = stream.index();
    let codec_params = stream.parameters();

    let mut decoder = ffmpeg::codec::context::Context::from_parameters(codec_params)
        .map_err(DecodeError::Decoder)?
        .decoder()
        .audio()
        .map_err(DecodeError::Decoder)?;

    let in_rate = decoder.rate();
    let in_channels = decoder.ch_layout().channels() as u32;
    let in_format = decoder.format();
    info!(
        path = %path.display(),
        in_rate, in_channels, in_format = ?in_format,
        "decoding audio"
    );

    let mut out: Vec<f32> = Vec::new();
    let mut frame = ffmpeg::frame::Audio::empty();

    let mut drain =
        |decoder: &mut ffmpeg::decoder::Audio, out: &mut Vec<f32>| -> Result<(), DecodeError> {
            loop {
                match decoder.receive_frame(&mut frame) {
                    Ok(()) => append_frame_as_f32(&frame, in_channels, out),
                    Err(ffmpeg::Error::Other { .. }) | Err(ffmpeg::Error::Eof) => break,
                    Err(e) => return Err(DecodeError::Decoder(e)),
                }
            }
            Ok(())
        };

    for item in ictx.packets() {
        match item {
            Ok((s, packet)) => {
                if s.index() != stream_index {
                    continue;
                }
                decoder.send_packet(&packet).map_err(DecodeError::Decoder)?;
                drain(&mut decoder, &mut out)?;
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => return Err(DecodeError::Decoder(e)),
        }
    }
    decoder.send_eof().map_err(DecodeError::Decoder)?;
    drain(&mut decoder, &mut out)?;

    debug!(samples = out.len(), "audio decode complete");
    Ok(DecodedAudio {
        sample_rate: in_rate,
        channels: in_channels,
        pcm: out,
    })
}

/// Convert one decoded frame into interleaved f32 PCM and append to `out`.
/// Handles the common sample formats (s16/s32/f32/f64, packed and planar)
/// natively; falls back to silence on anything unrecognized.
fn append_frame_as_f32(frame: &ffmpeg::frame::Audio, channels: u32, out: &mut Vec<f32>) {
    use ffmpeg::format::Sample;
    use ffmpeg::format::sample::Type as PlanType;
    let n = frame.samples();
    if n == 0 {
        return;
    }
    let ch = channels as usize;
    let total = n * ch;
    out.reserve(total);
    match frame.format() {
        Sample::I16(PlanType::Packed) => {
            let bytes = frame.data(0);
            let nbytes = total * 2;
            if bytes.len() < nbytes {
                return;
            }
            for chunk in bytes[..nbytes].chunks_exact(2) {
                let v = i16::from_le_bytes([chunk[0], chunk[1]]);
                out.push(v as f32 / i16::MAX as f32);
            }
        }
        Sample::I32(PlanType::Packed) => {
            let bytes = frame.data(0);
            let nbytes = total * 4;
            if bytes.len() < nbytes {
                return;
            }
            for chunk in bytes[..nbytes].chunks_exact(4) {
                let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(v as f32 / i32::MAX as f32);
            }
        }
        Sample::F32(PlanType::Packed) => {
            let bytes = frame.data(0);
            let nbytes = total * 4;
            if bytes.len() < nbytes {
                return;
            }
            for chunk in bytes[..nbytes].chunks_exact(4) {
                out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
        }
        Sample::F64(PlanType::Packed) => {
            let bytes = frame.data(0);
            let nbytes = total * 8;
            if bytes.len() < nbytes {
                return;
            }
            for chunk in bytes[..nbytes].chunks_exact(8) {
                let v = f64::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]);
                out.push(v as f32);
            }
        }
        // Planar formats: each channel sits in its own data plane.
        Sample::I16(PlanType::Planar) => {
            for i in 0..n {
                for c in 0..ch {
                    let plane = frame.data(c);
                    let off = i * 2;
                    if off + 1 >= plane.len() {
                        out.push(0.0);
                        continue;
                    }
                    let v = i16::from_le_bytes([plane[off], plane[off + 1]]);
                    out.push(v as f32 / i16::MAX as f32);
                }
            }
        }
        Sample::I32(PlanType::Planar) => {
            for i in 0..n {
                for c in 0..ch {
                    let plane = frame.data(c);
                    let off = i * 4;
                    if off + 3 >= plane.len() {
                        out.push(0.0);
                        continue;
                    }
                    let v = i32::from_le_bytes([
                        plane[off],
                        plane[off + 1],
                        plane[off + 2],
                        plane[off + 3],
                    ]);
                    out.push(v as f32 / i32::MAX as f32);
                }
            }
        }
        Sample::F32(PlanType::Planar) => {
            for i in 0..n {
                for c in 0..ch {
                    let plane = frame.data(c);
                    let off = i * 4;
                    if off + 3 >= plane.len() {
                        out.push(0.0);
                        continue;
                    }
                    out.push(f32::from_le_bytes([
                        plane[off],
                        plane[off + 1],
                        plane[off + 2],
                        plane[off + 3],
                    ]));
                }
            }
        }
        other => {
            tracing::warn!(?other, "unsupported sample format; emitting silence");
            out.extend(std::iter::repeat_n(0.0, total));
        }
    }
}

/// Probe the duration, sample rate, and channel count without decoding.
pub fn probe_audio(path: impl AsRef<Path>) -> Result<AudioInfo, DecodeError> {
    init_ffmpeg();
    let ictx = ffmpeg::format::input(path.as_ref()).map_err(DecodeError::Open)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or(DecodeError::NoAudioStream)?;
    let codec_params = stream.parameters();
    let decoder = ffmpeg::codec::context::Context::from_parameters(codec_params)
        .map_err(DecodeError::Decoder)?
        .decoder()
        .audio()
        .map_err(DecodeError::Decoder)?;
    let dur = if stream.duration() > 0 {
        let tb = stream.time_base();
        let secs = stream.duration() as f64 * f64::from(tb.0) / f64::from(tb.1);
        Duration::from_secs_f64(secs.max(0.0))
    } else {
        Duration::ZERO
    };
    Ok(AudioInfo {
        sample_rate: decoder.rate(),
        channels: decoder.ch_layout().channels() as u32,
        duration: dur,
    })
}

fn init_ffmpeg() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg::init();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-build a 16-bit PCM WAV file at 8 kHz mono with one second of
    /// 440 Hz sine, write it to a temp path, and return that path.
    fn write_test_wav() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("felx-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sine.wav");
        let sample_rate: u32 = 8_000;
        let n_frames: usize = sample_rate as usize;
        // Stereo (the resampler's get2 path requires a layout with a
        // representable mask; mono layouts don't have one in this version
        // of ffmpeg-the-third).
        let mut samples = Vec::with_capacity(n_frames * 2);
        for i in 0..n_frames {
            let t = i as f32 / sample_rate as f32;
            let v = (t * 440.0 * std::f32::consts::TAU).sin();
            let s = (v * 32_000.0) as i16;
            samples.push(s); // L
            samples.push(s); // R
        }
        let data_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let data_len: u32 = data_bytes.len() as u32;
        let block_align: u16 = 4; // 2 channels * 2 bytes
        let byte_rate: u32 = sample_rate * block_align as u32;
        let mut wav: Vec<u8> = Vec::with_capacity(44 + data_bytes.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&data_bytes);
        std::fs::write(&path, wav).unwrap();
        path
    }

    #[test]
    fn decode_one_second_of_sine_at_native_rate() {
        let path = write_test_wav();
        let audio = decode_file(&path, DEFAULT_SAMPLE_RATE).unwrap();
        // v1 returns native rate / channels (resampling is the mixer's job).
        assert_eq!(audio.sample_rate, 8_000);
        assert_eq!(audio.channels, 2);
        let frames = audio.frames();
        assert!(
            (7_900..=8_100).contains(&frames),
            "expected ~8k frames, got {frames}"
        );
        // Verify the signal isn't silence.
        let max = audio.pcm.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(max > 0.5, "expected non-silent decode, max sample {max}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn probe_returns_sensible_metadata() {
        let path = write_test_wav();
        let info = probe_audio(&path).unwrap();
        assert_eq!(info.sample_rate, 8_000);
        assert_eq!(info.channels, 2);
        assert!(info.duration.as_millis() >= 900 && info.duration.as_millis() <= 1_100);
        let _ = std::fs::remove_file(path);
    }
}
