//! Asset probing — read codec / duration / dimensions / framerate without
//! decoding any frames. Used by the asset library to populate metadata.

use crate::error::DecodeError;
use ffmpeg_next as ffmpeg;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: Duration,
    /// Average framerate as (num, den).
    pub framerate: (u32, u32),
    /// e.g. "h264", "hevc", "vp9", "prores"
    pub codec: String,
}

impl VideoInfo {
    pub fn fps(&self) -> f64 {
        if self.framerate.1 == 0 {
            0.0
        } else {
            self.framerate.0 as f64 / self.framerate.1 as f64
        }
    }
}

pub fn probe(path: impl AsRef<Path>) -> Result<VideoInfo, DecodeError> {
    ffmpeg::init().map_err(DecodeError::Ffmpeg)?;
    let path = path.as_ref();
    let input = ffmpeg::format::input(path)?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| DecodeError::NoVideoStream(path.to_path_buf()))?;

    let codec_params = stream.parameters();
    let codec_ctx = ffmpeg::codec::Context::from_parameters(codec_params)?;
    let video = codec_ctx.decoder().video()?;

    let codec_name = stream.parameters().id().name().to_string();

    let avg_fr = stream.avg_frame_rate();
    let framerate = (
        avg_fr.numerator() as u32,
        avg_fr.denominator().max(1) as u32,
    );

    // Stream duration is in stream-timebase units; convert via stream.time_base().
    let stream_duration = stream.duration();
    let tb = stream.time_base();
    let secs = if tb.denominator() == 0 {
        0.0
    } else {
        stream_duration as f64 * tb.numerator() as f64 / tb.denominator() as f64
    };

    Ok(VideoInfo {
        width: video.width(),
        height: video.height(),
        duration: Duration::from_secs_f64(secs.max(0.0)),
        framerate,
        codec: codec_name,
    })
}
