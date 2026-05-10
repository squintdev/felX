//! Time and rational arithmetic.
//!
//! [`Rational`] holds `num / den` as `u32 / u32` representing seconds. Time
//! arithmetic in the engine — sampling animation curves, advancing
//! playheads, computing media offsets — works through these integer-pair
//! types rather than floats so long timelines don't accumulate drift.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    pub fn as_seconds(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Convert this time to a frame index at `framerate`. Integer division —
    /// sub-frame remainders are dropped.
    pub fn to_frame(self, framerate: Framerate) -> Frame {
        // frame = time_seconds * fps = (num/den) * (fps_num/fps_den)
        //       = (num * fps_num) / (den * fps_den)
        let n = (self.num as u64) * (framerate.0.num as u64);
        let d = (self.den as u64) * (framerate.0.den as u64);
        Frame((n / d) as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Framerate(pub Rational);

impl Framerate {
    pub const fn new(num: u32, den: u32) -> Self {
        Self(Rational::new(num, den))
    }

    pub const FPS_24: Framerate = Framerate(Rational { num: 24, den: 1 });
    pub const FPS_30: Framerate = Framerate(Rational { num: 30, den: 1 });
    pub const FPS_60: Framerate = Framerate(Rational { num: 60, den: 1 });

    pub fn as_fps(self) -> f64 {
        self.0.as_seconds()
    }
}

impl Default for Framerate {
    fn default() -> Self {
        Self::FPS_30
    }
}

/// Frame index relative to the start of a composition's timeline.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Frame(pub u32);

impl Frame {
    /// Convert a frame index at `framerate` to an exact time on the
    /// timebase. `frame * (1 / framerate)` in rational arithmetic so long
    /// timelines don't accumulate float drift.
    pub fn to_time(self, framerate: Framerate) -> Rational {
        let fps_num = framerate.0.num as u64;
        let fps_den = framerate.0.den as u64;
        // time_seconds = frame / fps = frame * fps_den / fps_num.
        let num = (self.0 as u64) * fps_den;
        Rational {
            num: num as u32,
            den: fps_num as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_as_seconds() {
        assert_eq!(Rational::new(1, 2).as_seconds(), 0.5);
        assert_eq!(Rational::new(30, 1).as_seconds(), 30.0);
    }

    #[test]
    fn framerate_constants() {
        assert_eq!(Framerate::FPS_30.as_fps(), 30.0);
        assert_eq!(Framerate::FPS_24.as_fps(), 24.0);
        assert_eq!(Framerate::FPS_60.as_fps(), 60.0);
    }

    #[test]
    fn ten_hours_at_60fps_round_trip_is_exact() {
        // 10 * 3600 * 60 = 2_160_000 frames.
        let f = Frame(2_160_000);
        let t = f.to_time(Framerate::FPS_60);
        // 2_160_000 frames / 60 fps = 36_000 seconds (10 hours).
        assert_eq!(t.as_seconds(), 36_000.0);
        let back = t.to_frame(Framerate::FPS_60);
        assert_eq!(back, f, "round-trip drifted: {f:?} → {back:?}");
    }

    #[test]
    fn fractional_framerate_round_trip_is_exact() {
        let fps = Framerate::new(24000, 1001); // 23.976 (NTSC film cadence)
        let f = Frame(60_000);
        let t = f.to_time(fps);
        let back = t.to_frame(fps);
        assert_eq!(back, f);
    }

    #[test]
    fn frame_zero_is_time_zero() {
        let t = Frame(0).to_time(Framerate::FPS_30);
        assert_eq!(t.as_seconds(), 0.0);
    }

    #[test]
    fn frame_to_time_30fps_is_thirtieths() {
        let t = Frame(15).to_time(Framerate::FPS_30);
        assert_eq!(t, Rational { num: 15, den: 30 });
        assert_eq!(t.as_seconds(), 0.5);
    }
}
