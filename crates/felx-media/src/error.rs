//! Decode / probe errors.

use std::path::PathBuf;

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    Ffmpeg(ffmpeg_next::Error),
    NoVideoStream(PathBuf),
    UnsupportedCodec(String),
    SeekFailed { target_seconds: f64 },
    HwaccelInitFailed { kind: &'static str, detail: String },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "io: {e}"),
            DecodeError::Ffmpeg(e) => write!(f, "ffmpeg: {e}"),
            DecodeError::NoVideoStream(p) => write!(f, "no video stream in {}", p.display()),
            DecodeError::UnsupportedCodec(c) => write!(f, "unsupported codec: {c}"),
            DecodeError::SeekFailed { target_seconds } => {
                write!(f, "seek to {target_seconds}s failed")
            }
            DecodeError::HwaccelInitFailed { kind, detail } => {
                write!(f, "hwaccel init failed for {kind}: {detail}")
            }
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Io(e) => Some(e),
            DecodeError::Ffmpeg(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

impl From<ffmpeg_next::Error> for DecodeError {
    fn from(e: ffmpeg_next::Error) -> Self {
        DecodeError::Ffmpeg(e)
    }
}
