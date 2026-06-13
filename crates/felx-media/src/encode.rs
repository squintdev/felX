//! Video encode. RGBA8 frames in → MP4 / MOV out via ffmpeg-the-third.
//!
//! v1 surface (F-100 + F-101 + F-102):
//! - Codecs: H.264 (libx264 / NVENC / VAAPI / VideoToolbox), H.265
//!   (libx265 / NVENC / VAAPI / VideoToolbox), ProRes 422 / 4444 (prores_ks)
//! - Rate control: CRF / CBR / VBR
//! - Bitrate target + max
//! - Preset (encoder-specific string)
//! - Profile (baseline / main / high; passed as-is to the encoder)
//! - Pixel format (yuv420p default; auto-overridden for ProRes)
//! - Keyframe interval (GOP size)
//! - Hardware encoder selection (auto-fallback to software)
//!
//! Hardware encoders are wired but their full validation needs real
//! hardware on each platform. The software path is what CI exercises.

use crate::error::DecodeError;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::ChannelLayoutMask;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame::audio::Audio as AudioFrame;
use ffmpeg_next::util::frame::video::Video as VideoFrame;
use std::path::Path;
use tracing::{debug, info, warn};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
    /// ProRes 422 (proxy/LT/standard/HQ via `profile`).
    Prores422,
    /// ProRes 4444. Higher bitdepth + alpha.
    Prores4444,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "h265",
            VideoCodec::Prores422 => "prores422",
            VideoCodec::Prores4444 => "prores4444",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    /// Constant Rate Factor (perceptual quality target). x264/x265 only.
    Crf,
    /// Constant bitrate.
    Cbr,
    /// Variable bitrate with a target.
    Vbr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HwEncoder {
    #[default]
    Software,
    Nvenc,
    Vaapi,
    VideoToolbox,
}

impl HwEncoder {
    pub fn label(self) -> &'static str {
        match self {
            HwEncoder::Software => "software",
            HwEncoder::Nvenc => "nvenc",
            HwEncoder::Vaapi => "vaapi",
            HwEncoder::VideoToolbox => "videotoolbox",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub framerate: (u32, u32),
    pub rate_control: RateControl,
    /// CRF value (when `rate_control == Crf`). 18 = visually lossless,
    /// 23 = default, 28 = lower quality. Ignored for CBR/VBR.
    pub crf: u32,
    /// Target bitrate in bits per second. Used by CBR / VBR.
    pub target_bitrate: u64,
    /// Max bitrate (VBR / VBV). 0 = unset.
    pub max_bitrate: u64,
    /// Encoder preset string (`ultrafast`…`veryslow` for x264/x265, `p1`…`p7`
    /// for NVENC, etc.). Passed directly to the encoder.
    pub preset: String,
    /// Profile string (`baseline` / `main` / `high` for H.264 / H.265;
    /// `proxy` / `lt` / `standard` / `hq` / `4444` / `4444xq` for ProRes).
    pub profile: String,
    /// Pixel format. Default `yuv420p`. ProRes overrides to its native fmt.
    pub pixel_format: String,
    /// Keyframe interval in frames. 0 = encoder default.
    pub keyframe_interval: u32,
    /// Hardware encoder selection.
    pub hw: HwEncoder,
}

impl EncodeOptions {
    pub fn h264_default(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            codec: VideoCodec::H264,
            width,
            height,
            framerate: (fps_num, fps_den.max(1)),
            rate_control: RateControl::Crf,
            crf: 18,
            target_bitrate: 0,
            max_bitrate: 0,
            preset: "medium".to_string(),
            profile: "high".to_string(),
            pixel_format: "yuv420p".to_string(),
            keyframe_interval: 0,
            hw: HwEncoder::Software,
        }
    }

    pub fn h265_default(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            codec: VideoCodec::H265,
            // x265 profile names differ from x264 — there is no "high".
            profile: "main".to_string(),
            ..Self::h264_default(width, height, fps_num, fps_den)
        }
    }

    /// ProRes 422 standard profile by default. Profile string maps:
    /// `proxy` `lt` `standard` `hq` (and `4444` / `4444xq` via the 4444
    /// variant codec).
    pub fn prores422_default(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            codec: VideoCodec::Prores422,
            width,
            height,
            framerate: (fps_num, fps_den.max(1)),
            rate_control: RateControl::Vbr, // ProRes is essentially VBR
            crf: 0,
            target_bitrate: 0,
            max_bitrate: 0,
            preset: String::new(),
            profile: "standard".to_string(),
            pixel_format: "yuv422p10le".to_string(),
            keyframe_interval: 1, // ProRes is intra-frame
            hw: HwEncoder::Software,
        }
    }

    pub fn prores4444_default(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            codec: VideoCodec::Prores4444,
            pixel_format: "yuva444p10le".to_string(),
            profile: "4444".to_string(),
            ..Self::prores422_default(width, height, fps_num, fps_den)
        }
    }

    /// Fast, deterministic preset for tests.
    pub fn h264_test(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        let mut o = Self::h264_default(width, height, fps_num, fps_den);
        o.crf = 23;
        o.preset = "ultrafast".to_string();
        o
    }
}

fn codec_id_for(opts: &EncodeOptions) -> ffmpeg::codec::Id {
    match opts.codec {
        VideoCodec::H264 => ffmpeg::codec::Id::H264,
        VideoCodec::H265 => ffmpeg::codec::Id::HEVC,
        VideoCodec::Prores422 | VideoCodec::Prores4444 => ffmpeg::codec::Id::PRORES,
    }
}

fn encoder_name_for(opts: &EncodeOptions) -> Option<&'static str> {
    match (opts.codec, opts.hw) {
        (VideoCodec::H264, HwEncoder::Nvenc) => Some("h264_nvenc"),
        (VideoCodec::H264, HwEncoder::Vaapi) => Some("h264_vaapi"),
        (VideoCodec::H264, HwEncoder::VideoToolbox) => Some("h264_videotoolbox"),
        (VideoCodec::H265, HwEncoder::Nvenc) => Some("hevc_nvenc"),
        (VideoCodec::H265, HwEncoder::Vaapi) => Some("hevc_vaapi"),
        (VideoCodec::H265, HwEncoder::VideoToolbox) => Some("hevc_videotoolbox"),
        // ProRes: always prores_ks — the codec-id default resolves to the
        // Anatoliy encoder, which rejects the named profiles ("standard",
        // "4444", …) our options surface uses. No hardware path.
        (VideoCodec::Prores422 | VideoCodec::Prores4444, _) => Some("prores_ks"),
        _ => None,
    }
}

fn pixel_format_id(name: &str) -> ffmpeg::format::Pixel {
    use ffmpeg::format::Pixel;
    match name {
        "yuv420p" => Pixel::YUV420P,
        "yuv422p" => Pixel::YUV422P,
        "yuv444p" => Pixel::YUV444P,
        "yuv422p10le" => Pixel::YUV422P10LE,
        "yuv444p10le" => Pixel::YUV444P10LE,
        "yuva444p10le" => Pixel::YUVA444P10LE,
        _ => {
            warn!(name, "unknown pixel format; falling back to yuv420p");
            Pixel::YUV420P
        }
    }
}

/// Options for the muxed audio track of a video export. Input samples are
/// interleaved stereo f32 (the mixer's bus format) at `sample_rate`.
/// Codec is derived from the video codec's container conventions:
/// AAC for H.264/H.265 MP4, PCM s16 for ProRes MOV.
#[derive(Clone, Copy, Debug)]
pub struct AudioEncodeOptions {
    pub sample_rate: u32,
}

struct AudioStream {
    encoder: ffmpeg::encoder::Audio,
    stream_index: usize,
    /// Interleaved stereo f32 samples awaiting a full encoder frame.
    pending: Vec<f32>,
    /// Per-channel samples sent so far — the audio pts clock at a
    /// 1/sample_rate time base.
    samples_sent: i64,
    /// Per-channel samples per encoder frame (1024 for AAC; PCM encoders
    /// report 0 = any, mapped to a fixed chunk).
    frame_size: usize,
    sample_rate: u32,
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
    audio: Option<AudioStream>,
}

impl H264Encoder {
    pub fn create(path: impl AsRef<Path>, opts: EncodeOptions) -> Result<Self, DecodeError> {
        Self::create_with_audio(path, opts, None)
    }

    /// Like [`create`](Self::create), additionally muxing an audio track.
    /// Feed samples via [`write_audio_interleaved`](Self::write_audio_interleaved).
    pub fn create_with_audio(
        path: impl AsRef<Path>,
        opts: EncodeOptions,
        audio: Option<AudioEncodeOptions>,
    ) -> Result<Self, DecodeError> {
        ffmpeg::init().map_err(DecodeError::Ffmpeg)?;
        let path = path.as_ref();

        let mut output = ffmpeg::format::output(path)?;

        // Try the requested encoder name first (hw or named software);
        // fall back to the codec-id default if it's not available.
        let codec = if let Some(name) = encoder_name_for(&opts) {
            match ffmpeg::encoder::find_by_name(name) {
                Some(c) => c,
                None => {
                    warn!(
                        name,
                        "requested encoder not available; using codec-id default"
                    );
                    ffmpeg::encoder::find(codec_id_for(&opts))
                        .ok_or_else(|| DecodeError::UnsupportedCodec(opts.codec.label().into()))?
                }
            }
        } else {
            ffmpeg::encoder::find(codec_id_for(&opts))
                .ok_or_else(|| DecodeError::UnsupportedCodec(opts.codec.label().into()))?
        };

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
        let pix_fmt = pixel_format_id(&opts.pixel_format);
        encoder.set_format(pix_fmt);
        encoder.set_time_base(ffmpeg::Rational::new(
            opts.framerate.1 as i32,
            opts.framerate.0 as i32,
        ));
        encoder.set_frame_rate(Some(ffmpeg::Rational::new(
            opts.framerate.0 as i32,
            opts.framerate.1 as i32,
        )));
        if opts.target_bitrate > 0 {
            encoder.set_bit_rate(opts.target_bitrate as usize);
        }
        if opts.max_bitrate > 0 {
            encoder.set_max_bit_rate(opts.max_bitrate as usize);
        }
        if opts.keyframe_interval > 0 {
            encoder.set_gop(opts.keyframe_interval);
        }
        if global_header {
            encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
        }

        let mut codec_opts = ffmpeg::Dictionary::new();
        if !opts.preset.is_empty() {
            codec_opts.set("preset", &opts.preset);
        }
        if !opts.profile.is_empty() {
            codec_opts.set("profile", &opts.profile);
        }
        match opts.rate_control {
            RateControl::Crf => {
                let crf_str = opts.crf.to_string();
                codec_opts.set("crf", &crf_str);
            }
            RateControl::Cbr | RateControl::Vbr => {
                // Encoder-specific rate-control flags. x264/x265 are happy
                // with bit_rate/max_bit_rate; nvenc needs `rc cbr`/`rc vbr`.
                if matches!(opts.hw, HwEncoder::Nvenc) {
                    codec_opts.set(
                        "rc",
                        if matches!(opts.rate_control, RateControl::Cbr) {
                            "cbr"
                        } else {
                            "vbr"
                        },
                    );
                }
            }
        }
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
            pix_fmt,
            opts.width,
            opts.height,
            scaling::Flags::BILINEAR,
        )?;

        // Audio stream must be added before write_header.
        let audio = match audio {
            Some(aopts) => Some(Self::open_audio_stream(
                &mut output,
                &opts,
                aopts,
                global_header,
            )?),
            None => None,
        };

        output.write_header()?;

        let frame_template = VideoFrame::new(pix_fmt, opts.width, opts.height);

        info!(
            path = %path.display(),
            w = opts.width,
            h = opts.height,
            fps = format!("{}/{}", opts.framerate.0, opts.framerate.1),
            codec = opts.codec.label(),
            hw = opts.hw.label(),
            "video encoder open"
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
            audio,
        })
    }

    fn open_audio_stream(
        output: &mut ffmpeg::format::context::Output,
        opts: &EncodeOptions,
        aopts: AudioEncodeOptions,
        global_header: bool,
    ) -> Result<AudioStream, DecodeError> {
        use ffmpeg::format::Sample;
        use ffmpeg::format::sample::Type as PlanType;

        // Container conventions: AAC inside MP4 (H.264/H.265), PCM s16
        // inside MOV (ProRes).
        let (codec_id, sample_format) = match opts.codec {
            VideoCodec::H264 | VideoCodec::H265 => {
                (ffmpeg::codec::Id::AAC, Sample::F32(PlanType::Planar))
            }
            VideoCodec::Prores422 | VideoCodec::Prores4444 => {
                (ffmpeg::codec::Id::PCM_S16LE, Sample::I16(PlanType::Packed))
            }
        };
        let codec = ffmpeg::encoder::find(codec_id)
            .ok_or_else(|| DecodeError::UnsupportedCodec("audio".into()))?;

        let mut stream = output.add_stream(codec)?;
        let stream_index = stream.index();

        let mut encoder = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        let rate = aopts.sample_rate.max(1);
        encoder.set_rate(rate as i32);
        encoder.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
        encoder.set_format(sample_format);
        encoder.set_time_base(ffmpeg::Rational::new(1, rate as i32));
        if global_header {
            encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
        }

        let encoder = encoder.open_as(codec)?;
        let params: ffmpeg::codec::Parameters = (&encoder).into();
        stream.set_parameters(params);
        stream.set_time_base(ffmpeg::Rational::new(1, rate as i32));

        // PCM encoders report frame_size 0 (any size); pick a fixed chunk.
        let frame_size = match encoder.frame_size() {
            0 => 1024,
            n => n as usize,
        };

        info!(
            codec = ?codec_id,
            rate,
            frame_size,
            "audio encoder open"
        );

        Ok(AudioStream {
            encoder,
            stream_index,
            pending: Vec::new(),
            samples_sent: 0,
            frame_size,
            sample_rate: rate,
        })
    }

    /// Queue interleaved stereo f32 samples for the audio track. Complete
    /// encoder frames are sent immediately; a trailing partial frame is
    /// held until [`finish`](Self::finish). No-op when the encoder was
    /// created without an audio track.
    pub fn write_audio_interleaved(&mut self, samples: &[f32]) -> Result<(), DecodeError> {
        if self.finished {
            return Ok(());
        }
        let Some(audio) = self.audio.as_mut() else {
            return Ok(());
        };
        audio.pending.extend_from_slice(samples);
        let chunk_len = audio.frame_size * 2;
        while audio.pending.len() >= chunk_len {
            let chunk: Vec<f32> = audio.pending.drain(..chunk_len).collect();
            send_audio_chunk(audio, &mut self.output, &chunk)?;
        }
        Ok(())
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
    pub fn finish(mut self) -> Result<(), DecodeError> {
        if !self.finished {
            self.encoder.send_eof()?;
            self.drain_packets()?;
            if let Some(audio) = self.audio.as_mut() {
                // Flush the trailing partial frame, then the encoder.
                if !audio.pending.is_empty() {
                    let rest: Vec<f32> = std::mem::take(&mut audio.pending);
                    send_audio_chunk(audio, &mut self.output, &rest)?;
                }
                audio.encoder.send_eof()?;
                drain_audio_packets(audio, &mut self.output)?;
            }
            self.output.write_trailer()?;
            self.finished = true;
            debug!(frames = self.frame_count, "encoder finished");
        }
        let H264Encoder {
            output, encoder, ..
        } = self;
        drop(encoder);
        drop(output);
        Ok(())
    }
}

/// Encode one chunk of interleaved stereo f32 samples. The chunk may be
/// shorter than the encoder's frame size only for the final flush.
fn send_audio_chunk(
    audio: &mut AudioStream,
    output: &mut ffmpeg::format::context::Output,
    interleaved: &[f32],
) -> Result<(), DecodeError> {
    use ffmpeg::format::Sample;
    use ffmpeg::format::sample::Type as PlanType;

    let nsamples = interleaved.len() / 2;
    if nsamples == 0 {
        return Ok(());
    }
    let format = audio.encoder.format();
    let mut frame = AudioFrame::new(format, nsamples, ChannelLayoutMask::STEREO);
    frame.set_rate(audio.sample_rate);
    match format {
        Sample::F32(PlanType::Planar) => {
            let left = frame.plane_mut::<f32>(0);
            for (i, l) in left.iter_mut().enumerate().take(nsamples) {
                *l = interleaved[i * 2];
            }
            let right = frame.plane_mut::<f32>(1);
            for (i, r) in right.iter_mut().enumerate().take(nsamples) {
                *r = interleaved[i * 2 + 1];
            }
        }
        Sample::I16(PlanType::Packed) => {
            // plane_mut's slice length is per-channel samples, too short
            // for packed interleaved data — write through the byte view.
            let data = frame.data_mut(0);
            for (i, v) in interleaved.iter().enumerate() {
                let s = (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                data[i * 2..i * 2 + 2].copy_from_slice(&s.to_le_bytes());
            }
        }
        _ => return Err(DecodeError::Ffmpeg(ffmpeg::Error::InvalidData)),
    }
    frame.set_pts(Some(audio.samples_sent));
    audio.samples_sent += nsamples as i64;
    audio.encoder.send_frame(&frame)?;
    drain_audio_packets(audio, output)
}

fn drain_audio_packets(
    audio: &mut AudioStream,
    output: &mut ffmpeg::format::context::Output,
) -> Result<(), DecodeError> {
    let mut packet = ffmpeg::Packet::empty();
    let enc_tb = ffmpeg::Rational::new(1, audio.sample_rate as i32);
    let stream_tb = output
        .stream(audio.stream_index)
        .map(|s| s.time_base())
        .unwrap_or(enc_tb);
    while audio.encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(audio.stream_index);
        packet.rescale_ts(enc_tb, stream_tb);
        packet.write_interleaved(output)?;
    }
    Ok(())
}
