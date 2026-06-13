//! Transport state: playhead, play/pause, frame stepping, loop-at-end.
//!
//! Pure data + arithmetic. The app drives this from its `update()` loop
//! and reads `current_frame` to feed the compositor.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
}

pub struct Playhead {
    state: PlayState,
    current_frame: u32,
    duration_frames: u32,
    framerate_fps: f64,
    last_tick: Instant,
    /// Sub-frame time accumulated between integer frame advances.
    accumulator: f64,
}

impl Playhead {
    pub fn new(framerate_fps: f64, duration_frames: u32) -> Self {
        Self {
            state: PlayState::Stopped,
            current_frame: 0,
            duration_frames,
            framerate_fps,
            last_tick: Instant::now(),
            accumulator: 0.0,
        }
    }

    pub fn current_frame(&self) -> u32 {
        self.current_frame
    }

    pub fn duration_frames(&self) -> u32 {
        self.duration_frames
    }

    /// Used when project framerate/duration changes (e.g. after load).
    #[allow(dead_code)]
    pub fn set_duration_frames(&mut self, d: u32) {
        self.duration_frames = d.max(1);
        if self.current_frame >= self.duration_frames {
            self.current_frame = self.duration_frames.saturating_sub(1);
        }
    }

    pub fn framerate_fps(&self) -> f64 {
        self.framerate_fps
    }

    /// Used when project framerate/duration changes (e.g. after load).
    #[allow(dead_code)]
    pub fn set_framerate_fps(&mut self, fps: f64) {
        self.framerate_fps = fps.max(1e-3);
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.state, PlayState::Playing)
    }

    pub fn play(&mut self) {
        self.state = PlayState::Playing;
        self.last_tick = Instant::now();
        self.accumulator = 0.0;
    }

    pub fn pause(&mut self) {
        self.state = PlayState::Stopped;
    }

    pub fn toggle(&mut self) {
        match self.state {
            PlayState::Playing => self.pause(),
            PlayState::Stopped => self.play(),
        }
    }

    pub fn seek(&mut self, frame: u32) {
        let max = self.duration_frames.saturating_sub(1);
        self.current_frame = frame.min(max);
        self.accumulator = 0.0;
    }

    pub fn step_forward(&mut self) {
        let max = self.duration_frames.saturating_sub(1);
        if self.current_frame < max {
            self.current_frame += 1;
        } else {
            self.current_frame = 0;
        }
        self.accumulator = 0.0;
    }

    pub fn step_backward(&mut self) {
        if self.current_frame > 0 {
            self.current_frame -= 1;
        } else {
            self.current_frame = self.duration_frames.saturating_sub(1);
        }
        self.accumulator = 0.0;
    }

    /// Advance the frame counter based on real elapsed time. Call once per
    /// `update()`. Returns true if the playhead moved (caller invalidates
    /// the rendered frame).
    pub fn tick(&mut self) -> bool {
        if !self.is_playing() {
            self.last_tick = Instant::now();
            return false;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;

        self.accumulator += elapsed * self.framerate_fps;
        if self.accumulator < 1.0 {
            return false;
        }
        let advance_by = self.accumulator as u32;
        self.accumulator -= advance_by as f64;
        let max = self.duration_frames.saturating_sub(1);
        if max == 0 {
            return false;
        }
        let new_frame = (self.current_frame + advance_by) % self.duration_frames;
        if new_frame == self.current_frame {
            return false;
        }
        self.current_frame = new_frame;
        true
    }

    /// Suggest a repaint delay in seconds when playing — half a frame for
    /// some scheduling slack.
    pub fn repaint_after(&self) -> Option<Duration> {
        if !self.is_playing() {
            return None;
        }
        Some(Duration::from_secs_f64(0.5 / self.framerate_fps.max(1.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn starts_stopped_at_frame_zero() {
        let p = Playhead::new(30.0, 100);
        assert!(!p.is_playing());
        assert_eq!(p.current_frame(), 0);
    }

    #[test]
    fn step_forward_advances_and_wraps() {
        let mut p = Playhead::new(30.0, 3);
        p.step_forward();
        assert_eq!(p.current_frame(), 1);
        p.step_forward();
        p.step_forward();
        assert_eq!(p.current_frame(), 0); // wrapped (max=2 → 0)
    }

    #[test]
    fn step_backward_wraps_to_end() {
        let mut p = Playhead::new(30.0, 3);
        p.step_backward();
        assert_eq!(p.current_frame(), 2);
    }

    #[test]
    fn seek_clamps_to_max() {
        let mut p = Playhead::new(30.0, 10);
        p.seek(99);
        assert_eq!(p.current_frame(), 9);
    }

    #[test]
    fn tick_when_stopped_does_not_advance() {
        let mut p = Playhead::new(30.0, 100);
        sleep(Duration::from_millis(50));
        assert!(!p.tick());
        assert_eq!(p.current_frame(), 0);
    }

    #[test]
    fn tick_when_playing_advances_proportionally() {
        let mut p = Playhead::new(60.0, 100);
        p.play();
        sleep(Duration::from_millis(40)); // ~2.4 frames at 60fps
        let advanced = p.tick();
        assert!(advanced, "expected the playhead to advance");
        assert!(p.current_frame() >= 1, "got frame {}", p.current_frame());
    }

    #[test]
    fn toggle_flips_state() {
        let mut p = Playhead::new(30.0, 30);
        assert!(!p.is_playing());
        p.toggle();
        assert!(p.is_playing());
        p.toggle();
        assert!(!p.is_playing());
    }
}
