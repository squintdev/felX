//! Audio export (F-054, F-106).
//!
//! v1 ships a hand-rolled WAV writer for the audio-only path. The
//! "muxed with video" half of F-054 is blocked on F-100 (the full H.264
//! encoder controls rewrite) — it'll plug into the same WAV writer once
//! F-100's encoder exposes a parallel audio-stream API.

use std::io::Write;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavBitDepth {
    Pcm16,
    Pcm24,
    Float32,
}

impl WavBitDepth {
    pub fn bits(self) -> u16 {
        match self {
            WavBitDepth::Pcm16 => 16,
            WavBitDepth::Pcm24 => 24,
            WavBitDepth::Float32 => 32,
        }
    }
    /// 1 = PCM integer, 3 = IEEE float.
    pub fn format_tag(self) -> u16 {
        match self {
            WavBitDepth::Pcm16 | WavBitDepth::Pcm24 => 1,
            WavBitDepth::Float32 => 3,
        }
    }
}

/// Write interleaved PCM samples to a WAV file.
pub fn write_wav(
    path: impl AsRef<Path>,
    pcm: &[f32],
    sample_rate: u32,
    channels: u16,
    bit_depth: WavBitDepth,
) -> std::io::Result<()> {
    let bits = bit_depth.bits();
    let bytes_per_sample = (bits / 8) as u32;
    let block_align = (bytes_per_sample * channels as u32) as u16;
    let byte_rate = sample_rate * block_align as u32;

    let data_bytes: Vec<u8> = match bit_depth {
        WavBitDepth::Pcm16 => pcm
            .iter()
            .flat_map(|&s| {
                let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                v.to_le_bytes()
            })
            .collect(),
        WavBitDepth::Pcm24 => {
            let mut out = Vec::with_capacity(pcm.len() * 3);
            for &s in pcm {
                let v = (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                let bytes = v.to_le_bytes();
                out.extend_from_slice(&bytes[..3]);
            }
            out
        }
        WavBitDepth::Float32 => pcm.iter().flat_map(|s| s.to_le_bytes()).collect(),
    };
    let data_len = data_bytes.len() as u32;

    let mut f = std::fs::File::create(path.as_ref())?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&bit_depth.format_tag().to_le_bytes())?;
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    f.write_all(&data_bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_wav(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("felx-audio-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn round_trip_16bit_stereo_via_decoder() {
        let n = 4_000;
        let mut pcm = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / n as f32;
            let v = (t * 8.0 * std::f32::consts::TAU).sin() * 0.5;
            pcm.push(v);
            pcm.push(v);
        }
        let path = tmp_wav("sine16.wav");
        write_wav(&path, &pcm, 44_100, 2, WavBitDepth::Pcm16).unwrap();
        let decoded = crate::audio::decode_file(&path, 48_000).unwrap();
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 2);
        let frames = decoded.frames();
        assert!(
            (n as i64 - frames as i64).abs() < 16,
            "frame count mismatch: wrote {n}, decoded {frames}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn round_trip_float32_mono_via_decoder() {
        let n = 8_000;
        let pcm: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                (t * 4.0 * std::f32::consts::TAU).sin() * 0.7
            })
            .collect();
        let path = tmp_wav("sine_f32.wav");
        write_wav(&path, &pcm, 22_050, 1, WavBitDepth::Float32).unwrap();
        let decoded = crate::audio::decode_file(&path, 48_000).unwrap();
        assert_eq!(decoded.sample_rate, 22_050);
        assert_eq!(decoded.channels, 1);
        // Spot-check a few mid samples — should be very close to the source.
        let original_mid = pcm[n / 2];
        let decoded_mid = decoded.pcm[n / 2];
        assert!(
            (original_mid - decoded_mid).abs() < 1e-3,
            "f32 round-trip drift: {original_mid} vs {decoded_mid}"
        );
        let _ = std::fs::remove_file(path);
    }
}
