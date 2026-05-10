//! Integration tests for video decode + probe.
//!
//! Uses a committed 64×36 / 10fps / 10-frame H.264 mp4 in
//! `tests/fixtures/testsrc_10frames.mp4`. Generated with:
//!
//! ```text
//! ffmpeg -f lavfi -i testsrc=size=64x36:rate=10 \
//!        -frames:v 10 -pix_fmt yuv420p \
//!        -c:v libx264 -preset ultrafast tests/fixtures/testsrc_10frames.mp4
//! ```
//!
//! All tests use software decode so they run on CI without hardware.

use felx_media::{FfmpegDecoder, HwaccelKind, VideoDecoder, probe};
use std::path::PathBuf;
use std::time::Duration;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn probe_reports_dimensions_and_codec() {
    let info = probe(fixture("testsrc_10frames.mp4")).unwrap();
    assert_eq!(info.width, 64);
    assert_eq!(info.height, 36);
    assert_eq!(info.codec, "h264");
    let fps = info.fps();
    assert!((fps - 10.0).abs() < 0.01, "expected 10 fps, got {fps}");
}

#[test]
fn software_decode_returns_first_frame() {
    let mut dec =
        FfmpegDecoder::open(fixture("testsrc_10frames.mp4"), HwaccelKind::Software).unwrap();
    assert_eq!(dec.width(), 64);
    assert_eq!(dec.height(), 36);
    assert_eq!(dec.hwaccel(), HwaccelKind::Software);
    let frame = dec
        .next_frame()
        .unwrap()
        .expect("first frame should decode");
    assert_eq!(frame.width, 64);
    assert_eq!(frame.height, 36);
    assert_eq!(frame.rgba.len(), 64 * 36 * 4);
    // Alpha is 0xff for an opaque YUV-decoded frame.
    assert!(frame.rgba.chunks(4).all(|p| p[3] == 0xff));
}

#[test]
fn software_decode_yields_all_frames_and_terminates() {
    let mut dec =
        FfmpegDecoder::open(fixture("testsrc_10frames.mp4"), HwaccelKind::Software).unwrap();
    let mut count = 0;
    while let Some(_frame) = dec.next_frame().unwrap() {
        count += 1;
        if count > 50 {
            panic!("decoded too many frames; expected 10");
        }
    }
    assert_eq!(count, 10);
}

#[test]
fn auto_hwaccel_falls_back_to_software_when_no_device() {
    // CI runners (and the test harness) may or may not have a hwaccel
    // device available. We just want to confirm Auto either succeeds with
    // software or some hwaccel — never errors out.
    let dec = FfmpegDecoder::open(fixture("testsrc_10frames.mp4"), HwaccelKind::Auto).unwrap();
    let _ = dec.hwaccel();
}

#[test]
fn seek_does_not_error_for_in_range_target() {
    // The fixture is intentionally tiny (10 frames @ 10fps with ultrafast
    // preset → one keyframe at the start). Seek lands on the nearest
    // keyframe, which is the start; we just verify the call succeeds and
    // we can decode after.
    let mut dec =
        FfmpegDecoder::open(fixture("testsrc_10frames.mp4"), HwaccelKind::Software).unwrap();
    dec.seek(Duration::from_millis(500)).unwrap();
    assert!(dec.next_frame().unwrap().is_some());
}

#[test]
fn missing_file_returns_io_error() {
    let res = FfmpegDecoder::open("/no/such/clip.mp4", HwaccelKind::Software);
    assert!(res.is_err());
}

#[test]
fn probe_missing_file_returns_error() {
    let res = probe("/no/such/clip.mp4");
    assert!(res.is_err());
}
