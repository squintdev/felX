//! Audio mixer (F-051).
//!
//! Sums per-layer audio buffers (with per-layer keyframed gain + pan)
//! into a stereo master bus at the project master sample rate, then
//! applies master gain. Keyframed values are sampled at the start of
//! each output buffer — for typical comp framerates this is plenty
//! precise; per-sample evaluation is overkill.
//!
//! Source buffers are nearest-neighbor / linear-interpolated to the
//! master rate as needed (decoders return native rate per F-050). A
//! polyphase resampler is a quality follow-up.

use crate::model::{Curve, Lerp, Rational};

pub const DEFAULT_MASTER_RATE: u32 = 48_000;

/// Per-layer audio source feeding the mixer. Keyframed gain (linear,
/// 0..∞) and pan (-1 = full left, 0 = center, +1 = full right).
#[derive(Clone, Debug)]
pub struct AudioSource {
    pub sample_rate: u32,
    pub channels: u32,
    /// Interleaved f32 PCM, length = frames * channels.
    pub pcm: Vec<f32>,
    pub gain: Curve<f32>,
    pub pan: Curve<f32>,
}

/// Master bus output. Stereo interleaved f32 at `sample_rate`.
#[derive(Clone, Debug)]
pub struct MixedBus {
    pub sample_rate: u32,
    pub pcm: Vec<f32>,
}

impl MixedBus {
    pub fn frames(&self) -> usize {
        self.pcm.len() / 2
    }
}

/// Mix a window of audio. Time-zero corresponds to the first sample
/// emitted; `window_frames` is the number of stereo frames at the master
/// rate to produce. Each source contributes to the window per its own
/// gain/pan curves sampled at the window-start time.
///
/// `master_gain_at_window_start` should be the project master-gain curve
/// sampled at `time_start`; the call is left to the host so this function
/// stays free of the project model.
pub fn mix_window(
    sources: &[AudioSource],
    time_start: Rational,
    master_rate: u32,
    window_frames: usize,
    master_gain_at_window_start: f32,
) -> MixedBus {
    let mut out: Vec<f32> = vec![0.0; window_frames * 2];
    let secs_per_frame = 1.0 / master_rate as f64;
    for src in sources {
        let gain = src.gain.sample_at_time(time_start).max(0.0);
        let pan = src.pan.sample_at_time(time_start).clamp(-1.0, 1.0);
        if gain == 0.0 || src.pcm.is_empty() {
            continue;
        }
        let (gl, gr) = pan_law(pan);
        let g_left = gain * gl;
        let g_right = gain * gr;

        // Resample (linear) and channel-mix on the fly.
        let src_frames = if src.channels == 0 {
            0
        } else {
            src.pcm.len() / src.channels as usize
        };
        let rate_ratio = src.sample_rate as f64 / master_rate as f64;
        let start_offset_secs = time_start.as_seconds();
        let start_offset_src = start_offset_secs * src.sample_rate as f64;
        for i in 0..window_frames {
            let src_pos = start_offset_src + (i as f64) * rate_ratio;
            if src_pos < 0.0 || src_pos >= src_frames as f64 {
                continue;
            }
            let i0 = src_pos.floor() as usize;
            let i1 = (i0 + 1).min(src_frames.saturating_sub(1));
            let frac = (src_pos - i0 as f64) as f32;
            let (sl, sr) = sample_stereo(&src.pcm, src.channels, i0, i1, frac);
            out[i * 2] += sl * g_left;
            out[i * 2 + 1] += sr * g_right;
        }
        let _ = secs_per_frame;
    }
    if (master_gain_at_window_start - 1.0).abs() > f32::EPSILON {
        for s in &mut out {
            *s *= master_gain_at_window_start;
        }
    }
    MixedBus {
        sample_rate: master_rate,
        pcm: out,
    }
}

fn sample_stereo(pcm: &[f32], channels: u32, i0: usize, i1: usize, frac: f32) -> (f32, f32) {
    let ch = channels as usize;
    let read = |frame: usize| -> (f32, f32) {
        let base = frame * ch;
        match channels {
            1 => {
                let v = pcm.get(base).copied().unwrap_or(0.0);
                (v, v)
            }
            2 => (
                pcm.get(base).copied().unwrap_or(0.0),
                pcm.get(base + 1).copied().unwrap_or(0.0),
            ),
            _ => {
                // For >2 channels, mix into stereo by averaging odd/even
                // channel indices. Sufficient for v1 — proper surround
                // downmix is post-MVP.
                let mut l = 0.0;
                let mut r = 0.0;
                let mut nl = 0;
                let mut nr = 0;
                for c in 0..ch {
                    let v = pcm.get(base + c).copied().unwrap_or(0.0);
                    if c % 2 == 0 {
                        l += v;
                        nl += 1;
                    } else {
                        r += v;
                        nr += 1;
                    }
                }
                (l / nl.max(1) as f32, r / nr.max(1) as f32)
            }
        }
    };
    let (l0, r0) = read(i0);
    let (l1, r1) = read(i1);
    (l0.lerp(&l1, frac), r0.lerp(&r1, frac))
}

/// Equal-power pan law: at center (-1..1 -> 0), L = R = 1/sqrt(2).
/// At ±1, the corresponding side is 1.0 and the other is 0.
fn pan_law(pan: f32) -> (f32, f32) {
    let p = (pan + 1.0) * 0.5; // 0..=1
    let theta = p * std::f32::consts::FRAC_PI_2;
    (theta.cos(), theta.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rational;

    fn src(sample_rate: u32, channels: u32, pcm: Vec<f32>, gain: f32, pan: f32) -> AudioSource {
        AudioSource {
            sample_rate,
            channels,
            pcm,
            gain: Curve::Static(gain),
            pan: Curve::Static(pan),
        }
    }

    #[test]
    fn single_centered_source_produces_equal_l_r() {
        // 1 second of stereo unity at 48k.
        let pcm: Vec<f32> = (0..48_000).flat_map(|_| [1.0_f32, 1.0]).collect();
        let bus = mix_window(
            &[src(48_000, 2, pcm, 1.0, 0.0)],
            Rational::new(0, 48_000),
            48_000,
            64,
            1.0,
        );
        for i in 0..64 {
            let l = bus.pcm[i * 2];
            let r = bus.pcm[i * 2 + 1];
            assert!((l - r).abs() < 1e-5, "L/R mismatch at {i}: {l} vs {r}");
            // Equal-power center pan reduces by 1/sqrt(2) ≈ 0.707.
            assert!(
                (l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
                "center pan should attenuate to ~0.707, got {l}"
            );
        }
    }

    #[test]
    fn full_left_pan_silences_right() {
        let pcm: Vec<f32> = vec![1.0; 256];
        let bus = mix_window(
            &[src(48_000, 1, pcm, 1.0, -1.0)],
            Rational::new(0, 48_000),
            48_000,
            64,
            1.0,
        );
        for i in 0..64 {
            let l = bus.pcm[i * 2];
            let r = bus.pcm[i * 2 + 1];
            assert!(l > 0.95, "left channel should be ~unity: {l}");
            assert!(r.abs() < 1e-5, "right channel should be silent: {r}");
        }
    }

    #[test]
    fn gain_zero_contributes_silence() {
        let pcm: Vec<f32> = vec![1.0; 256];
        let bus = mix_window(
            &[src(48_000, 2, pcm, 0.0, 0.0)],
            Rational::new(0, 48_000),
            48_000,
            32,
            1.0,
        );
        assert!(bus.pcm.iter().all(|s| s.abs() < 1e-9));
    }

    #[test]
    fn two_sources_sum() {
        let a = src(48_000, 2, vec![1.0; 128], 0.5, 0.0);
        let b = src(48_000, 2, vec![1.0; 128], 0.5, 0.0);
        let bus = mix_window(&[a, b], Rational::new(0, 48_000), 48_000, 32, 1.0);
        // Each source at 0.5 gain * center pan (0.707) → ~0.353.
        // Two sources sum → ~0.707.
        for i in 0..32 {
            let l = bus.pcm[i * 2];
            assert!(
                (l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-2,
                "summed level should be ~0.707, got {l}"
            );
        }
    }

    #[test]
    fn rate_conversion_matches_known_duration() {
        // 1-second 8 kHz mono buffer summed at 48 kHz master should produce
        // 48k * (window/48k) = window stereo frames covering source range.
        let n = 8_000;
        let pcm: Vec<f32> = (0..n).map(|_| 1.0).collect();
        let bus = mix_window(
            &[src(8_000, 1, pcm, 1.0, 0.0)],
            Rational::new(0, 48_000),
            48_000,
            48_000, // ask for 1 sec of master output
            1.0,
        );
        // The full source is 1 sec, so all 48k master frames should have
        // signal (modulo edge sampling).
        let nonzero = bus.pcm.iter().filter(|s| s.abs() > 1e-3).count();
        assert!(
            nonzero > 90_000,
            "expected most master samples to carry signal, got {nonzero}"
        );
    }

    #[test]
    fn master_gain_scales_output() {
        let pcm: Vec<f32> = vec![1.0; 256];
        let bus = mix_window(
            &[src(48_000, 1, pcm, 1.0, -1.0)],
            Rational::new(0, 48_000),
            48_000,
            32,
            0.5,
        );
        for i in 0..32 {
            let l = bus.pcm[i * 2];
            assert!(
                (l - 0.5).abs() < 1e-3,
                "master gain 0.5 should scale to ~0.5, got {l}"
            );
        }
    }
}
