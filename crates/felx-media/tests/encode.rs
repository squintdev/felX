//! End-to-end encoder test: write 30 frames of synthetic data → decode
//! the file back → verify dimensions, frame count, and that each frame is
//! valid RGBA.

use felx_media::{
    AudioEncodeOptions, EncodeOptions, FfmpegDecoder, H264Encoder, HwaccelKind, VideoDecoder,
    decode_file, probe,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("felx-encode-{pid}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn synth_frame(w: u32, h: u32, frame_idx: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let r = ((x + frame_idx) & 0xff) as u8;
            let g = (y & 0xff) as u8;
            let b = ((x ^ y ^ frame_idx) & 0xff) as u8;
            buf.extend_from_slice(&[r, g, b, 255]);
        }
    }
    buf
}

#[test]
fn encode_then_decode_round_trip() {
    let dir = scratch_dir();
    let path = dir.join("out.mp4");

    let w = 64u32;
    let h = 36u32;
    let frame_count = 30u32;
    let opts = EncodeOptions::h264_default(w, h, 30, 1);

    let mut enc = H264Encoder::create(&path, opts).unwrap();
    for i in 0..frame_count {
        enc.write_rgba(&synth_frame(w, h, i)).unwrap();
    }
    enc.finish().unwrap();

    assert!(path.exists());
    assert!(path.metadata().unwrap().len() > 0);

    let info = probe(&path).unwrap();
    assert_eq!(info.width, w);
    assert_eq!(info.height, h);
    assert_eq!(info.codec, "h264");

    let mut dec = FfmpegDecoder::open(&path, HwaccelKind::Software).unwrap();
    let mut count = 0;
    while let Some(frame) = dec.next_frame().unwrap() {
        assert_eq!(frame.width, w);
        assert_eq!(frame.height, h);
        assert_eq!(frame.rgba.len(), (w * h * 4) as usize);
        count += 1;
        if count > 100 {
            break;
        }
    }
    assert_eq!(count, frame_count);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn encode_with_audio_round_trips_tone() {
    let dir = scratch_dir();
    let path = dir.join("out_audio.mp4");

    let w = 64u32;
    let h = 36u32;
    let frame_count = 30u32; // 1 second at 30 fps
    let rate = 48_000u32;
    let opts = EncodeOptions::h264_test(w, h, 30, 1);

    let mut enc =
        H264Encoder::create_with_audio(&path, opts, Some(AudioEncodeOptions { sample_rate: rate }))
            .unwrap();

    // 440 Hz sine, 1 second, interleaved stereo.
    let total_samples = rate as usize;
    let tone: Vec<f32> = (0..total_samples)
        .flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
            [v, v]
        })
        .collect();

    // Interleave audio with video frames the way the export loop does.
    let mut cursor = 0usize;
    for i in 0..frame_count {
        enc.write_rgba(&synth_frame(w, h, i)).unwrap();
        let end = (((i + 1) as usize * total_samples) / frame_count as usize) * 2;
        enc.write_audio_interleaved(&tone[cursor..end]).unwrap();
        cursor = end;
    }
    enc.finish().unwrap();

    // The container must hold a decodable audio stream of ~1s.
    let decoded = decode_file(&path, rate).unwrap();
    assert_eq!(decoded.channels, 2);
    let frames = decoded.frames();
    assert!(
        (rate as usize - 4096..=rate as usize + 4096).contains(&frames),
        "expected ~{rate} audio frames, got {frames}"
    );
    // Tone survives the AAC round trip: strong signal, ~440 zero crossings.
    let mono: Vec<f32> = decoded
        .pcm
        .chunks_exact(2)
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();
    let peak = mono.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.25, "expected audible tone, peak {peak}");
    let crossings = mono
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    assert!(
        (380..=500).contains(&crossings),
        "expected ~440 rising zero crossings, got {crossings}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rgba_size_mismatch_returns_error() {
    let dir = scratch_dir();
    let path = dir.join("out.mp4");
    let opts = EncodeOptions::h264_default(64, 36, 30, 1);
    let mut enc = H264Encoder::create(&path, opts).unwrap();
    let too_small = vec![0u8; 64 * 36 * 4 - 1];
    assert!(enc.write_rgba(&too_small).is_err());
    let _ = enc.finish();
    let _ = std::fs::remove_dir_all(&dir);
}
