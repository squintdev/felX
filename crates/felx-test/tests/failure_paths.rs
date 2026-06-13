//! Tests for the harness's error paths: pixel mismatch and missing golden.
//! Use a per-test scratch directory so committed goldens are not touched.

use felx_test::compare_golden;
use image::{ImageBuffer, Rgba, RgbaImage};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("felx-test-{label}-{pid}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn solid(rgba: [u8; 4]) -> RgbaImage {
    ImageBuffer::from_pixel(4, 4, Rgba(rgba))
}

#[test]
fn mismatch_writes_diff_artifacts_and_panics() {
    let temp = scratch_dir("mismatch");
    let golden_dir = temp.join("tests/golden");
    std::fs::create_dir_all(&golden_dir).unwrap();
    solid([255, 0, 0, 255])
        .save(golden_dir.join("scratch.png"))
        .unwrap();

    let result = std::panic::catch_unwind(|| {
        compare_golden(temp.to_str().unwrap(), "scratch", &solid([0, 0, 255, 255]));
    });
    assert!(result.is_err(), "expected panic on pixel mismatch");

    let diffs = temp.join("target/visual-diffs");
    assert!(
        diffs.join("scratch.actual.png").exists(),
        "actual.png missing under {}",
        diffs.display()
    );
    assert!(
        diffs.join("scratch.expected.png").exists(),
        "expected.png missing"
    );
    assert!(diffs.join("scratch.diff.png").exists(), "diff.png missing");

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn missing_golden_panics_after_creating_one() {
    let temp = scratch_dir("missing");

    let result = std::panic::catch_unwind(|| {
        compare_golden(temp.to_str().unwrap(), "new_one", &solid([0, 255, 0, 255]));
    });
    assert!(
        result.is_err(),
        "first run with missing golden should panic"
    );

    let created = temp.join("tests/golden/new_one.png");
    assert!(
        created.exists(),
        "golden should have been created at {}",
        created.display()
    );

    let _ = std::fs::remove_dir_all(&temp);
}
