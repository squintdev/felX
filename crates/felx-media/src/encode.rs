//! Video encode. RGBA8 frames in → H.264 MP4 out via ffmpeg-the-third.
//!
//! v1 defaults are tuned for editing-quality intermediate output: H.264
//! `medium` preset, CRF 18, yuv420p, baseline-yuv compatibility. F-100
//! exposes the full encoder-controls surface (CBR/VBR/CRF, bitrate caps,
//! presets, profiles) on top of this.

use crate::error::DecodeError;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame::video::Video as VideoFrame;
use std::path::Path;
use tracing::{debug, info};

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    pub width: u32,
    pub height: u32,
    pub framerate: (u32, u32),
    pub crf: u32,
    pub preset: String,
}

impl EncodeOptions {
    pub fn h264_default(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            width,
            height,
            framerate: (fps_num, fps_den.max(1)),
            crf: 18,
            preset: "medium".to_string(),
        }
    }

    /// Fast, deterministic preset for tests: ultrafast, no B-frames so EOF
    /// flush drops nothing.
    pub fn h264_test(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            width,
            height,
            framerate: (fps_num, fps_den.max(1)),
            crf: 23,
            preset: "ultrafast".to_string(),
        }
    }
}

pub struct H264Encoder {
    output: ffmpeg::format::context::Output,
    encoder: ffmpeg::encoder::Video,
    scaler: scaling::Context,
    stream_index: usize,
    frame_count: i64,
    frame_template: VideoFrame,
    finished: bool,
    fps_num: u32,
    fps_den: u32,
}

impl H264Encoder {
    pub fn create(path: impl AsRef<Path>, opts: EncodeOptions) -> Result<Self, DecodeError> {
        ffmpeg::init().map_err(DecodeError::Ffmpeg)?;
        let path = path.as_ref();

        let mut output = ffmpeg::format::output(path)?;
        let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::H264).ok_or_else(|| {
            DecodeError::UnsupportedCodec("h264 encoder not available in libavcodec".into())
        })?;

        let global_header = output
            .format()
            .flags()
            .contains(ffmpeg::format::Flags::GLOBAL_HEADER);

        let mut stream = output.add_stream(codec)?;
        let stream_index = stream.index();

        let mut encoder = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        encoder.set_width(opts.width);
        encoder.set_height(opts.height);
        encoder.set_format(ffmpeg::format::Pixel::YUV420P);
        encoder.set_time_base(ffmpeg::Rational::new(
            opts.framerate.1 as i32,
            opts.framerate.0 as i32,
        ));
        encoder.set_frame_rate(Some(ffmpeg::Rational::new(
            opts.framerate.0 as i32,
            opts.framerate.1 as i32,
        )));
        if global_header {
            encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
        }

        let mut codec_opts = ffmpeg::Dictionary::new();
        codec_opts.set("preset", &opts.preset);
        let crf_str = opts.crf.to_string();
        codec_opts.set("crf", &crf_str);
        // No B-frames keeps the encoder's input/output frame counts equal,
        // which is what we want for editing-quality intermediates and what
        // makes the round-trip test deterministic.
        codec_opts.set("bf", "0");

        let encoder = encoder.open_as_with(codec, codec_opts)?;
        let params: ffmpeg::codec::Parameters = (&encoder).into();
        stream.set_parameters(params);
        stream.set_time_base(ffmpeg::Rational::new(
            opts.framerate.1 as i32,
            opts.framerate.0 as i32,
        ));

        let scaler = scaling::Context::get(
            ffmpeg::format::Pixel::RGBA,
            opts.width,
            opts.height,
            ffmpeg::format::Pixel::YUV420P,
            opts.width,
            opts.height,
            scaling::Flags::BILINEAR,
        )?;

        output.write_header()?;

        let frame_template =
            VideoFrame::new(ffmpeg::format::Pixel::YUV420P, opts.width, opts.height);

        info!(
            path = %path.display(),
            w = opts.width,
            h = opts.height,
            fps = format!("{}/{}", opts.framerate.0, opts.framerate.1),
            "h264 encoder open"
        );

        Ok(Self {
            output,
            encoder,
            scaler,
            stream_index,
            frame_count: 0,
            frame_template,
            finished: false,
            fps_num: opts.framerate.0,
            fps_den: opts.framerate.1,
        })
    }

    /// Push one RGBA8 frame. The slice must be `width * height * 4` bytes.
    pub fn write_rgba(&mut self, rgba: &[u8]) -> Result<(), DecodeError> {
        if self.finished {
            return Ok(());
        }
        let w = self.encoder.width();
        let h = self.encoder.height();
        let expected = (w as usize) * (h as usize) * 4;
        if rgba.len() != expected {
            return Err(DecodeError::Ffmpeg(ffmpeg::Error::InvalidData));
        }

        // Wrap the RGBA buffer as a VideoFrame.
        let mut src = VideoFrame::new(ffmpeg::format::Pixel::RGBA, w, h);
        let stride = src.stride(0);
        let row_bytes = (w as usize) * 4;
        {
            let dst = src.data_mut(0);
            for y in 0..h as usize {
                let off = y * stride;
                dst[off..off + row_bytes]
                    .copy_from_slice(&rgba[y * row_bytes..(y + 1) * row_bytes]);
            }
        }
        let mut yuv = self.frame_template.clone();
        self.scaler.run(&src, &mut yuv)?;
        yuv.set_pts(Some(self.frame_count));
        self.frame_count += 1;
        self.encoder.send_frame(&yuv)?;
        self.drain_packets()?;
        Ok(())
    }

    fn drain_packets(&mut self) -> Result<(), DecodeError> {
        let mut packet = ffmpeg::Packet::empty();
        let stream_tb = self
            .output
            .stream(self.stream_index)
            .map(|s| s.time_base())
            .unwrap_or_else(|| ffmpeg::Rational::new(self.fps_den as i32, self.fps_num as i32));
        let enc_tb = ffmpeg::Rational::new(self.fps_den as i32, self.fps_num as i32);
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream_index);
            packet.rescale_ts(enc_tb, stream_tb);
            packet.write_interleaved(&mut self.output)?;
        }
        Ok(())
    }

    /// Flush any buffered frames and finalize the container. Idempotent.
    /// Drops the underlying [`format::context::Output`] before returning so
    /// the destructor's `avio_close` runs and the bytes hit disk.
    pub fn finish(mut self) -> Result<(), DecodeError> {
        if !self.finished {
            self.encoder.send_eof()?;
            self.drain_packets()?;
            self.output.write_trailer()?;
            self.finished = true;
            debug!(frames = self.frame_count, "h264 encoder finished");
        }
        // Explicit drop to make the file-close ordering obvious. Output's
        // Destructor calls avio_close which flushes; it would happen on the
        // function-return drop anyway, but doing it here documents the intent.
        let H264Encoder {
            output, encoder, ..
        } = self;
        drop(encoder);
        drop(output);
        Ok(())
    }
}
