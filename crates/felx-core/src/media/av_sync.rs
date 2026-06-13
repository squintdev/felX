//! A/V sync (F-053) — pure timing math.
//!
//! The audio playback thread advances a sample counter as it consumes
//! samples from the bus. The video preview loop reads that counter,
//! converts to a video frame, and snaps the playhead if it has drifted
//! more than [`SyncTolerance::tolerance_frames`] from where audio thinks
//! we are.

use crate::model::{Framerate, Rational};
use std::sync::atomic::{AtomicI64, Ordering};

/// Shared sample-clock counter. The audio side bumps it; the video side
/// reads it.
#[derive(Debug, Default)]
pub struct AudioClock {
    /// Total samples consumed by the audio output device since the
    /// playhead was last seek-reset. Atomic so the audio thread can write
    /// while the UI thread reads.
    samples: AtomicI64,
    sample_rate: AtomicI64,
}

impl AudioClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            samples: AtomicI64::new(0),
            sample_rate: AtomicI64::new(sample_rate as i64),
        }
    }

    pub fn add_samples(&self, delta: i64) {
        self.samples.fetch_add(delta, Ordering::Release);
    }

    pub fn reset_to(&self, frame: u32, framerate: Framerate, sample_rate: u32) {
        let rate = sample_rate as i64;
        self.sample_rate.store(rate, Ordering::Release);
        let secs = Rational::new(frame, 1).as_seconds() / framerate.as_fps();
        let samples = (secs * rate as f64).round() as i64;
        self.samples.store(samples, Ordering::Release);
    }

    pub fn samples(&self) -> i64 {
        self.samples.load(Ordering::Acquire)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Acquire) as u32
    }

    /// Frame index the audio clock currently believes we're playing.
    pub fn frame_at(&self, framerate: Framerate) -> i64 {
        let rate = self.sample_rate().max(1) as f64;
        let secs = self.samples() as f64 / rate;
        (secs * framerate.as_fps()).round() as i64
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SyncTolerance {
    /// If the video frame counter drifts more than this many frames from
    /// the audio clock, snap it back. AE / Premiere both ship with about
    /// 1 frame of tolerance at typical framerates.
    pub tolerance_frames: i64,
}

impl Default for SyncTolerance {
    fn default() -> Self {
        Self {
            tolerance_frames: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncDecision {
    Hold,
    SnapTo(u32),
}

/// Decide whether the video playhead should snap to the audio clock.
pub fn decide(
    video_frame: u32,
    audio_clock: &AudioClock,
    framerate: Framerate,
    tolerance: SyncTolerance,
) -> SyncDecision {
    let audio_frame = audio_clock.frame_at(framerate);
    if audio_frame < 0 {
        return SyncDecision::Hold;
    }
    let drift = (video_frame as i64 - audio_frame).abs();
    if drift <= tolerance.tolerance_frames {
        SyncDecision::Hold
    } else {
        SyncDecision::SnapTo(audio_frame.max(0) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_clock_reports_frame_zero() {
        let clk = AudioClock::new(48_000);
        assert_eq!(clk.frame_at(Framerate::FPS_30), 0);
    }

    #[test]
    fn samples_accumulate_into_the_correct_frame() {
        let clk = AudioClock::new(48_000);
        // 1 second of samples = 30 frames at 30fps.
        clk.add_samples(48_000);
        assert_eq!(clk.frame_at(Framerate::FPS_30), 30);
    }

    #[test]
    fn no_drift_returns_hold() {
        let clk = AudioClock::new(48_000);
        clk.add_samples(48_000); // 30 frames at 30fps
        let d = decide(30, &clk, Framerate::FPS_30, SyncTolerance::default());
        assert_eq!(d, SyncDecision::Hold);
    }

    #[test]
    fn one_frame_drift_within_tolerance() {
        let clk = AudioClock::new(48_000);
        clk.add_samples(48_000); // 30 frames
        let d = decide(31, &clk, Framerate::FPS_30, SyncTolerance::default());
        assert_eq!(d, SyncDecision::Hold);
    }

    #[test]
    fn larger_drift_triggers_snap() {
        let clk = AudioClock::new(48_000);
        clk.add_samples(48_000); // 30 frames
        let d = decide(50, &clk, Framerate::FPS_30, SyncTolerance::default());
        assert_eq!(d, SyncDecision::SnapTo(30));
    }

    #[test]
    fn reset_to_seeks_clock_to_a_frame() {
        let clk = AudioClock::new(48_000);
        clk.reset_to(60, Framerate::FPS_30, 48_000);
        // 60 frames at 30fps = 2s = 96k samples.
        assert!((clk.samples() - 96_000).abs() < 5);
    }
}
