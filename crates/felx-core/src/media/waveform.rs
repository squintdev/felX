//! Per-asset audio waveform thumbnail (F-055).
//!
//! Reduces a long PCM buffer to a fixed number of bins; each bin records
//! min and max sample amplitude across its time window. Suitable for the
//! per-track waveform thumbnail in the timeline. Computed once on import
//! and cached alongside the asset metadata.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaveformBin {
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waveform {
    pub bins: Vec<WaveformBin>,
}

/// Compute a waveform with `target_bins` bins from interleaved PCM data.
/// Channels are summed-and-averaged into mono before binning.
pub fn compute_waveform(pcm: &[f32], channels: u32, target_bins: usize) -> Waveform {
    let target_bins = target_bins.max(1);
    let ch = channels.max(1) as usize;
    let frames = pcm.len() / ch;
    if frames == 0 {
        return Waveform {
            bins: vec![WaveformBin::default(); target_bins],
        };
    }
    let frames_per_bin = frames.div_ceil(target_bins);
    let mut bins = Vec::with_capacity(target_bins);
    for bin_idx in 0..target_bins {
        let start_frame = bin_idx * frames_per_bin;
        let end_frame = ((bin_idx + 1) * frames_per_bin).min(frames);
        if start_frame >= end_frame {
            bins.push(WaveformBin::default());
            continue;
        }
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for f in start_frame..end_frame {
            let mut sum = 0.0_f32;
            for c in 0..ch {
                let i = f * ch + c;
                sum += pcm.get(i).copied().unwrap_or(0.0);
            }
            let mono = sum / ch as f32;
            if mono < min {
                min = mono;
            }
            if mono > max {
                max = mono;
            }
        }
        if !min.is_finite() {
            min = 0.0;
        }
        if !max.is_finite() {
            max = 0.0;
        }
        bins.push(WaveformBin { min, max });
    }
    Waveform { bins }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pcm_yields_zeroed_bins() {
        let w = compute_waveform(&[], 2, 16);
        assert_eq!(w.bins.len(), 16);
        assert!(w.bins.iter().all(|b| *b == WaveformBin::default()));
    }

    #[test]
    fn full_scale_sine_hits_one_and_negative_one() {
        let n = 8000;
        let mut pcm = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / n as f32;
            let v = (t * 4.0 * std::f32::consts::TAU).sin();
            pcm.push(v); // L
            pcm.push(v); // R
        }
        let w = compute_waveform(&pcm, 2, 32);
        // Each bin should contain peaks near ±1.
        let max_max = w.bins.iter().map(|b| b.max).fold(0.0_f32, f32::max);
        let min_min = w.bins.iter().map(|b| b.min).fold(0.0_f32, f32::min);
        assert!(max_max > 0.9);
        assert!(min_min < -0.9);
    }

    #[test]
    fn binning_respects_target_count() {
        let pcm: Vec<f32> = (0..1000).map(|i| i as f32 * 0.001).collect();
        let w = compute_waveform(&pcm, 1, 50);
        assert_eq!(w.bins.len(), 50);
    }

    #[test]
    fn dc_signal_min_equals_max() {
        let pcm: Vec<f32> = vec![0.5; 1000];
        let w = compute_waveform(&pcm, 1, 10);
        for b in &w.bins {
            assert!((b.min - 0.5).abs() < 1e-6);
            assert!((b.max - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn channels_average_into_mono() {
        // L = +1, R = -1 → mono = 0
        let pcm: Vec<f32> = (0..1000).flat_map(|_| [1.0_f32, -1.0]).collect();
        let w = compute_waveform(&pcm, 2, 4);
        for b in &w.bins {
            assert!(b.min.abs() < 1e-6);
            assert!(b.max.abs() < 1e-6);
        }
    }
}
