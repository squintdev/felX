//! Video decode / encode / probe via rsmpeg + system FFmpeg.

pub mod decode;
pub mod encode;
pub mod error;
pub mod info;

pub use decode::{DecodedFrame, FfmpegDecoder, HwaccelKind, VideoDecoder, VideoFrameRgba};
pub use encode::{EncodeOptions, H264Encoder};
pub use error::DecodeError;
pub use info::{VideoInfo, probe};
