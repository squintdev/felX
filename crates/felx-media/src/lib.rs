//! Video decode / encode / probe via rsmpeg + system FFmpeg.

pub mod audio;
pub mod audio_export;
pub mod decode;
pub mod encode;
pub mod error;
pub mod info;
pub mod signal_ntsc;

pub use audio::{AudioInfo, CHANNELS, DEFAULT_SAMPLE_RATE, DecodedAudio, decode_file, probe_audio};
pub use audio_export::{WavBitDepth, write_wav};
pub use decode::{DecodedFrame, FfmpegDecoder, HwaccelKind, VideoDecoder, VideoFrameRgba};
pub use encode::{
    AudioEncodeOptions, EncodeOptions, H264Encoder, HwEncoder, RateControl, VideoCodec,
};
pub use error::DecodeError;
pub use info::{VideoInfo, probe};
