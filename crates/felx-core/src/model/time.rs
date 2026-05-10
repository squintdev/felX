//! Time and rational arithmetic.
//!
//! Story F-031 will replace [`Rational`] with a more rigorous type that
//! enforces `den != 0` via `NonZeroU32` and provides arithmetic. For now
//! the type is a plain pair, and callers are responsible for non-zero
//! denominators.

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
}
