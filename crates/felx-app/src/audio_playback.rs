//! cpal-backed preview playback (F-052).
//!
//! Pushes interleaved stereo f32 samples through the system default audio
//! output device. The cpal stream callback drains from a mutex-guarded
//! ring buffer and zero-fills any underrun (no audio queued ⇒ silence).
//!
//! Hardware testing required to fully validate the audio device path —
//! the ring-buffer logic is unit-tested in isolation, but the cpal stream
//! callback only fires when an audio device is actually present.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use felx_core::media::AudioClock;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

#[derive(Default)]
struct RingBuffer {
    samples: Vec<f32>,
}

impl RingBuffer {
    fn enqueue(&mut self, src: &[f32]) {
        self.samples.extend_from_slice(src);
    }

    /// Drain `n` samples into `dst`, zero-filling on underrun. Returns the
    /// number of *real* (non-zero-filled) samples drained.
    fn drain_into(&mut self, dst: &mut [f32]) -> usize {
        let n = dst.len();
        let take = n.min(self.samples.len());
        for (i, s) in self.samples.drain(..take).enumerate() {
            dst[i] = s;
        }
        if take < n {
            for slot in dst.iter_mut().skip(take) {
                *slot = 0.0;
            }
        }
        take
    }

    fn len(&self) -> usize {
        self.samples.len()
    }
}

#[allow(dead_code)] // public API surface; full host integration is the F-053 polish path
pub struct AudioPlayback {
    _stream: cpal::Stream,
    buffer: Arc<Mutex<RingBuffer>>,
    sample_rate: u32,
    clock: Arc<AudioClock>,
}

#[derive(Debug)]
pub enum PlaybackError {
    NoOutputDevice,
    NoConfig,
    Build(cpal::BuildStreamError),
    Play(cpal::PlayStreamError),
}

impl std::fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaybackError::NoOutputDevice => write!(f, "no audio output device"),
            PlaybackError::NoConfig => write!(f, "no compatible output config"),
            PlaybackError::Build(e) => write!(f, "build stream: {e}"),
            PlaybackError::Play(e) => write!(f, "play stream: {e}"),
        }
    }
}

impl std::error::Error for PlaybackError {}

#[allow(dead_code)] // see struct-level note
impl AudioPlayback {
    /// Open the system default output device, build a stereo f32 stream,
    /// and start playing. The returned handle owns the cpal Stream;
    /// dropping it stops audio output.
    pub fn new(clock: Arc<AudioClock>) -> Result<Self, PlaybackError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(PlaybackError::NoOutputDevice)?;
        let config = device
            .default_output_config()
            .map_err(|_| PlaybackError::NoConfig)?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        info!(
            sample_rate,
            channels,
            format = ?config.sample_format(),
            "audio output device opened"
        );

        let buffer = Arc::new(Mutex::new(RingBuffer::default()));
        let buffer_for_cb = Arc::clone(&buffer);
        let clock_for_cb = Arc::clone(&clock);
        let stream_config: cpal::StreamConfig = config.into();
        let device_channels = channels as usize;

        let err_fn = |err| warn!(?err, "audio stream error");
        let stream = device
            .build_output_stream::<f32, _, _>(
                &stream_config,
                move |out: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    if let Ok(mut buf) = buffer_for_cb.lock() {
                        // We assume queued audio is always interleaved stereo.
                        // If the device opened at >2 channels, route L/R into
                        // the first two and zero the rest. Anything fancier
                        // (5.1 upmix etc.) is post-MVP.
                        if device_channels == 2 {
                            let real = buf.drain_into(out);
                            clock_for_cb.add_samples((real / 2) as i64);
                        } else {
                            let frames = out.len() / device_channels;
                            let mut stereo_buf = vec![0.0_f32; frames * 2];
                            let real = buf.drain_into(&mut stereo_buf);
                            for f in 0..frames {
                                let l = stereo_buf[f * 2];
                                let r = stereo_buf[f * 2 + 1];
                                for c in 0..device_channels {
                                    out[f * device_channels + c] = match c {
                                        0 => l,
                                        1 => r,
                                        _ => 0.0,
                                    };
                                }
                            }
                            clock_for_cb.add_samples((real / 2) as i64);
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(PlaybackError::Build)?;
        stream.play().map_err(PlaybackError::Play)?;
        Ok(Self {
            _stream: stream,
            buffer,
            sample_rate,
            clock,
        })
    }

    pub fn enqueue(&self, samples: &[f32]) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.enqueue(samples);
        }
    }

    pub fn queued(&self) -> usize {
        self.buffer.lock().map(|b| b.len()).unwrap_or(0)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn clock(&self) -> &Arc<AudioClock> {
        &self.clock
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_enqueue_then_drain() {
        let mut b = RingBuffer::default();
        b.enqueue(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = [0.0_f32; 3];
        let real = b.drain_into(&mut out);
        assert_eq!(real, 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn ring_buffer_underrun_zero_fills() {
        let mut b = RingBuffer::default();
        b.enqueue(&[1.0, 2.0]);
        let mut out = [0.0_f32; 5];
        let real = b.drain_into(&mut out);
        assert_eq!(real, 2);
        assert_eq!(out, [1.0, 2.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn ring_buffer_empty_drain_is_silent() {
        let mut b = RingBuffer::default();
        let mut out = [9.0_f32; 4];
        let real = b.drain_into(&mut out);
        assert_eq!(real, 0);
        assert_eq!(out, [0.0; 4]);
    }
}
