//! End-to-end encoder test: write 30 frames of synthetic data → decode
//! the file back → verify dimensions, frame count, and that each frame is
//! valid RGBA.

use felx_media::{EncodeOptions, FfmpegDecoder, H264Encoder, HwaccelKind, VideoDecoder, probe};
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
