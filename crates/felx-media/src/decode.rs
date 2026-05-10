//! Video decoder. Software path is the baseline; hardware acceleration via
//! VAAPI / NVDEC / VideoToolbox / D3D11VA selects the first available device
//! at open time and falls back to software if none is present.
//!
//! v1 always returns frames as `RGBA8` in CPU memory (after `sws_scale` from
//! the decoder's native format, typically YUV420P or NV12). Zero-copy
//! GPU upload via wgpu HAL is post-MVP.

use crate::error::DecodeError;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame::video::Video as VideoFrame;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Hardware decode device class. `Software` means no acceleration; `Auto`
/// asks the decoder to pick the best available, falling back to software.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwaccelKind {
    Auto,
    Software,
    Vaapi,
    Nvdec,
    VideoToolbox,
    D3d11va,
    Dxva2,
}

impl HwaccelKind {
    fn av_device_type(self) -> Option<ffmpeg::ffi::AVHWDeviceType> {
        use ffmpeg::ffi::AVHWDeviceType;
        match self {
            HwaccelKind::Auto | HwaccelKind::Software => None,
            HwaccelKind::Vaapi => Some(AVHWDeviceType::VAAPI),
            HwaccelKind::Nvdec => Some(AVHWDeviceType::CUDA),
            HwaccelKind::VideoToolbox => Some(AVHWDeviceType::VIDEOTOOLBOX),
            HwaccelKind::D3d11va => Some(AVHWDeviceType::D3D11VA),
            HwaccelKind::Dxva2 => Some(AVHWDeviceType::DXVA2),
        }
    }

    /// Platform-preferred device order. Returns the kinds to try (other than
    /// Software) in priority order for the current OS.
    fn platform_preferred() -> &'static [HwaccelKind] {
        #[cfg(target_os = "linux")]
        {
            &[HwaccelKind::Vaapi, HwaccelKind::Nvdec]
        }
        #[cfg(target_os = "macos")]
        {
            &[HwaccelKind::VideoToolbox]
        }
        #[cfg(target_os = "windows")]
        {
            &[HwaccelKind::D3d11va, HwaccelKind::Nvdec, HwaccelKind::Dxva2]
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            &[]
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HwaccelKind::Auto => "auto",
            HwaccelKind::Software => "software",
            HwaccelKind::Vaapi => "vaapi",
            HwaccelKind::Nvdec => "nvdec",
            HwaccelKind::VideoToolbox => "videotoolbox",
            HwaccelKind::D3d11va => "d3d11va",
            HwaccelKind::Dxva2 => "dxva2",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecodedFrame {
    /// Presentation time relative to the start of the file.
    pub pts: Duration,
    pub width: u32,
    pub height: u32,
    /// RGBA8, tightly packed (no row padding). Length = `width * height * 4`.
    pub rgba: Vec<u8>,
}

pub type VideoFrameRgba = DecodedFrame;

pub trait VideoDecoder {
    fn open(path: impl AsRef<Path>, hwaccel: HwaccelKind) -> Result<Self, DecodeError>
    where
        Self: Sized;
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn fps(&self) -> f64;
    fn hwaccel(&self) -> HwaccelKind;
    fn seek(&mut self, time: Duration) -> Result<(), DecodeError>;
    fn next_frame(&mut self) -> Result<Option<DecodedFrame>, DecodeError>;
}

pub struct FfmpegDecoder {
    /// Held for diagnostics; surfaced via Display on errors.
    #[allow(dead_code)]
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    stream_index: usize,
    /// (num, den) seconds-per-tick for the video stream's PTS values.
    time_base: (i32, i32),
    scaler: Option<scaling::Context>,
    hwaccel: HwaccelKind,
}

impl FfmpegDecoder {
    fn build_scaler(&mut self) -> Result<&mut scaling::Context, DecodeError> {
        if self.scaler.is_none() {
            let ctx = scaling::Context::get(
                self.decoder.format(),
                self.decoder.width(),
                self.decoder.height(),
                ffmpeg::format::Pixel::RGBA,
                self.decoder.width(),
                self.decoder.height(),
                scaling::Flags::BILINEAR,
            )?;
            self.scaler = Some(ctx);
        }
        Ok(self.scaler.as_mut().unwrap())
    }

    fn pts_to_duration(&self, pts: i64) -> Duration {
        if pts < 0 || self.time_base.1 == 0 {
            return Duration::ZERO;
        }
        let secs = pts as f64 * self.time_base.0 as f64 / self.time_base.1 as f64;
        Duration::from_secs_f64(secs.max(0.0))
    }
}

fn try_attach_hwaccel(
    codec_ctx: &mut ffmpeg::codec::context::Context,
    requested: HwaccelKind,
) -> HwaccelKind {
    let candidates: Vec<HwaccelKind> = match requested {
        HwaccelKind::Software => return HwaccelKind::Software,
        HwaccelKind::Auto => HwaccelKind::platform_preferred().to_vec(),
        explicit => vec![explicit],
    };

    for kind in candidates {
        let Some(av_type) = kind.av_device_type() else {
            continue;
        };
        // Probe by attempting to create the device. SAFETY: rsmpeg/ffmpeg-next
        // does not provide a safe wrapper for av_hwdevice_ctx_create on all
        // platforms; we drop into FFI. The created buffer is reffed and
        // attached to the codec context; FFmpeg releases it when the codec
        // context is freed.
        unsafe {
            let mut hw_device_ctx: *mut ffmpeg::ffi::AVBufferRef = std::ptr::null_mut();
            let rc = ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut hw_device_ctx,
                av_type,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            );
            if rc < 0 || hw_device_ctx.is_null() {
                debug!(kind = kind.label(), rc, "hwaccel device create failed");
                continue;
            }
            let raw = codec_ctx.as_mut_ptr();
            (*raw).hw_device_ctx = ffmpeg::ffi::av_buffer_ref(hw_device_ctx);
            ffmpeg::ffi::av_buffer_unref(&mut hw_device_ctx);
        }
        info!(kind = kind.label(), "hwaccel device attached");
        return kind;
    }
    info!("no hwaccel device available; using software decode");
    HwaccelKind::Software
}

impl VideoDecoder for FfmpegDecoder {
    fn open(path: impl AsRef<Path>, hwaccel: HwaccelKind) -> Result<Self, DecodeError> {
        ffmpeg::init().map_err(DecodeError::Ffmpeg)?;
        let path_buf = path.as_ref().to_path_buf();
        let input = ffmpeg::format::input(&path_buf)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| DecodeError::NoVideoStream(path_buf.clone()))?;
        let stream_index = stream.index();
        let tb = stream.time_base();
        let time_base = (tb.numerator(), tb.denominator());

        let codec_params = stream.parameters();
        let mut codec_ctx = ffmpeg::codec::Context::from_parameters(codec_params)?;
        let actual_hwaccel = try_attach_hwaccel(&mut codec_ctx, hwaccel);
        let decoder = codec_ctx.decoder().video()?;

        Ok(Self {
            path: path_buf,
            input,
            decoder,
            stream_index,
            time_base,
            scaler: None,
            hwaccel: actual_hwaccel,
        })
    }

    fn width(&self) -> u32 {
        self.decoder.width()
    }

    fn height(&self) -> u32 {
        self.decoder.height()
    }

    fn fps(&self) -> f64 {
        // Reach back through the input streams to read the avg framerate;
        // the decoder doesn't carry it directly.
        if let Some(s) = self.input.stream(self.stream_index) {
            let r = s.avg_frame_rate();
            if r.denominator() != 0 {
                return r.numerator() as f64 / r.denominator() as f64;
            }
        }
        0.0
    }

    fn hwaccel(&self) -> HwaccelKind {
        self.hwaccel
    }

    fn seek(&mut self, time: Duration) -> Result<(), DecodeError> {
        let target_seconds = time.as_secs_f64();
        let ts = (target_seconds * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        // Seek to a window around the target. AVSEEK_FLAG_BACKWARD via the
        // ffmpeg-next helper picks the nearest keyframe at or before the
        // target; we then rely on the decoder loop to walk forward.
        match self.input.seek(ts, i64::MIN..i64::MAX) {
            Ok(_) => {
                self.decoder.flush();
                Ok(())
            }
            Err(_) => Err(DecodeError::SeekFailed { target_seconds }),
        }
    }

    fn next_frame(&mut self) -> Result<Option<DecodedFrame>, DecodeError> {
        let mut decoded = VideoFrame::empty();

        loop {
            // Drain anything already buffered.
            match self.decoder.receive_frame(&mut decoded) {
                Ok(_) => {
                    let hw_unwrapped: Option<VideoFrame> = if self.hwaccel != HwaccelKind::Software
                    {
                        // Pull GPU-resident frame to system memory.
                        let mut sw = VideoFrame::empty();
                        let rc = unsafe {
                            ffmpeg::ffi::av_hwframe_transfer_data(
                                sw.as_mut_ptr(),
                                decoded.as_ptr(),
                                0,
                            )
                        };
                        if rc < 0 {
                            warn!(rc, "hwframe_transfer_data failed; using raw frame");
                            None
                        } else {
                            Some(sw)
                        }
                    } else {
                        None
                    };
                    let frame_to_scale = hw_unwrapped.as_ref().unwrap_or(&decoded);

                    let pts = decoded.pts().unwrap_or(0);
                    let scaler = self.build_scaler()?;
                    let mut rgba = VideoFrame::empty();
                    scaler.run(frame_to_scale, &mut rgba)?;

                    let w = rgba.width();
                    let h = rgba.height();
                    let stride = rgba.stride(0);
                    let raw = rgba.data(0);
                    let row_bytes = (w * 4) as usize;
                    let mut tight = Vec::with_capacity(row_bytes * h as usize);
                    for y in 0..h as usize {
                        let off = y * stride;
                        tight.extend_from_slice(&raw[off..off + row_bytes]);
                    }
                    return Ok(Some(DecodedFrame {
                        pts: self.pts_to_duration(pts),
                        width: w,
                        height: h,
                        rgba: tight,
                    }));
                }
                Err(ffmpeg::Error::Other { .. }) | Err(ffmpeg::Error::Eof) => {
                    // Need more input or we're done — fall through to packet
                    // read; EOF will be picked up by send_eof + receive_frame
                    // below.
                }
                Err(_e) => {
                    // Anything else: try to feed more packets and retry.
                }
            }

            // Read the next packet for our stream.
            let next = self.input.packets().next();
            match next {
                Some(Ok((stream, packet))) => {
                    if stream.index() == self.stream_index {
                        self.decoder.send_packet(&packet)?;
                    }
                }
                // Some implementations return Some(Err(Eof)) at the end of
                // the file rather than None — handle both as "drain decoder".
                Some(Err(ffmpeg::Error::Eof)) | None => {
                    // send_eof returns Eof itself if it's already been
                    // signalled; on the second loop iteration that's expected.
                    let _ = self.decoder.send_eof();
                    match self.decoder.receive_frame(&mut decoded) {
                        Ok(_) => continue,
                        Err(_) => return Ok(None),
                    }
                }
                Some(Err(e)) => return Err(DecodeError::Ffmpeg(e)),
            }
        }
    }
}
