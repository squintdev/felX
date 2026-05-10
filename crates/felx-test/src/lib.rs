//! Visual regression test harness for analog-felx.
//!
//! Each effect or render path declares a small set of reference renders
//! (input + parameters → expected PNG). Tests use the [`golden!`] macro to
//! compare actual output against a committed PNG under
//! `<consumer-crate>/tests/golden/<name>.png`.
//!
//! Threshold: per-channel max-diff defaults to [`DEFAULT_MAX_DIFF`] (1/255).
//! Override with `golden!(name, &img, max_diff: 3)`.
//!
//! On mismatch, three PNGs are written to `<workspace>/target/visual-diffs/`:
//! `<name>.actual.png`, `<name>.expected.png`, `<name>.diff.png` (the diff
//! is the per-channel absolute difference, scaled ×16 for visibility).
//!
//! Set `FELX_UPDATE_GOLDEN=1` to overwrite the golden with the actual image.
//! Missing goldens are auto-created on first run, but the test still fails
//! that run so the human commits the file before relying on it.

use image::{ImageBuffer, Rgba, RgbaImage};
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_DIFF: u8 = 1;
pub const UPDATE_ENV_VAR: &str = "FELX_UPDATE_GOLDEN";

#[derive(Debug)]
pub struct DiffReport {
    pub max_observed: u8,
    pub channels_over_threshold: usize,
    pub total_channels: usize,
}

#[derive(Debug)]
pub enum DiffError {
    DimensionMismatch {
        expected: (u32, u32),
        actual: (u32, u32),
    },
    PixelMismatch(DiffReport),
    GoldenMissing(PathBuf),
    Io(std::io::Error),
    Image(image::ImageError),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "dimension mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            DiffError::PixelMismatch(r) => write!(
                f,
                "pixel mismatch: {} of {} channels exceed threshold (max diff observed: {})",
                r.channels_over_threshold, r.total_channels, r.max_observed
            ),
            DiffError::GoldenMissing(p) => write!(f, "golden missing at {}", p.display()),
            DiffError::Io(e) => write!(f, "io: {e}"),
            DiffError::Image(e) => write!(f, "image: {e}"),
        }
    }
}

impl std::error::Error for DiffError {}

/// Pure pixel comparison. No file I/O. Use this in unit tests of the harness
/// itself.
pub fn diff(actual: &RgbaImage, expected: &RgbaImage, max_diff: u8) -> Result<(), DiffError> {
    if actual.dimensions() != expected.dimensions() {
        return Err(DiffError::DimensionMismatch {
            expected: expected.dimensions(),
            actual: actual.dimensions(),
        });
    }

    let mut max_observed: u8 = 0;
    let mut over: usize = 0;
    let mut total: usize = 0;
    for (a, e) in actual.pixels().zip(expected.pixels()) {
        for c in 0..4 {
            total += 1;
            let d = a[c].abs_diff(e[c]);
            if d > max_observed {
                max_observed = d;
            }
            if d > max_diff {
                over += 1;
            }
        }
    }

    if over > 0 {
        return Err(DiffError::PixelMismatch(DiffReport {
            max_observed,
            channels_over_threshold: over,
            total_channels: total,
        }));
    }
    Ok(())
}

/// Compare `actual` against the golden image stored under
/// `<crate_dir>/tests/golden/<name>.png`. Panics on mismatch. Use the
/// [`golden!`] macro at call sites so `CARGO_MANIFEST_DIR` is captured.
pub fn compare_golden(crate_dir: &str, name: &str, actual: &RgbaImage) {
    compare_golden_with(crate_dir, name, actual, DEFAULT_MAX_DIFF);
}

pub fn compare_golden_with(crate_dir: &str, name: &str, actual: &RgbaImage, max_diff: u8) {
    let golden_dir = PathBuf::from(crate_dir).join("tests/golden");
    let golden_path = golden_dir.join(format!("{name}.png"));
    let update = std::env::var(UPDATE_ENV_VAR).as_deref() == Ok("1");

    if update {
        std::fs::create_dir_all(&golden_dir).expect("create golden dir");
        actual.save(&golden_path).expect("write golden");
        eprintln!("[felx-test] updated golden: {}", golden_path.display());
        return;
    }

    if !golden_path.exists() {
        std::fs::create_dir_all(&golden_dir).expect("create golden dir");
        actual.save(&golden_path).expect("write golden");
        panic!(
            "[felx-test] golden missing; created {} from this run.\n\
             Review and commit it, then re-run the test to verify.",
            golden_path.display()
        );
    }

    let expected = match image::open(&golden_path) {
        Ok(img) => img.to_rgba8(),
        Err(e) => panic!(
            "[felx-test] failed to load golden {}: {e}",
            golden_path.display()
        ),
    };

    if let Err(err) = diff(actual, &expected, max_diff) {
        write_diff_artifacts(crate_dir, name, actual, &expected);
        panic!("[felx-test] golden '{name}' {err}");
    }
}

fn write_diff_artifacts(crate_dir: &str, name: &str, actual: &RgbaImage, expected: &RgbaImage) {
    let target = workspace_target_dir(crate_dir).join("visual-diffs");
    if std::fs::create_dir_all(&target).is_err() {
        return;
    }
    let _ = actual.save(target.join(format!("{name}.actual.png")));
    let _ = expected.save(target.join(format!("{name}.expected.png")));

    if expected.dimensions() == actual.dimensions() {
        let mut diff_img: RgbaImage = ImageBuffer::new(actual.width(), actual.height());
        for (x, y, p) in actual.enumerate_pixels() {
            let e = expected.get_pixel(x, y);
            let mut out = [0u8; 4];
            for c in 0..4 {
                out[c] = p[c].abs_diff(e[c]).saturating_mul(16);
            }
            out[3] = 255;
            diff_img.put_pixel(x, y, Rgba(out));
        }
        let _ = diff_img.save(target.join(format!("{name}.diff.png")));
    }
    eprintln!("[felx-test] visual-diffs written to: {}", target.display());
}

/// Walk up from `crate_dir` to find the workspace root (where `Cargo.lock`
/// lives) and return its `target/` subdir.
fn workspace_target_dir(crate_dir: &str) -> PathBuf {
    let mut p: &Path = Path::new(crate_dir);
    loop {
        if p.join("Cargo.lock").exists() {
            return p.join("target");
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => return PathBuf::from(crate_dir).join("target"),
        }
    }
}

/// Compare an `RgbaImage` against a committed golden PNG.
///
/// ```ignore
/// use felx_test::golden;
/// let img = my_render();
/// golden!("my_render_smoke", &img);
/// golden!("my_render_lenient", &img, max_diff: 3);
/// ```
#[macro_export]
macro_rules! golden {
    ($name:expr, $actual:expr) => {
        $crate::compare_golden(env!("CARGO_MANIFEST_DIR"), $name, $actual)
    };
    ($name:expr, $actual:expr, max_diff: $max:expr) => {
        $crate::compare_golden_with(env!("CARGO_MANIFEST_DIR"), $name, $actual, $max)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        ImageBuffer::from_pixel(w, h, Rgba(rgba))
    }

    #[test]
    fn diff_identical_passes() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [10, 20, 30, 255]);
        assert!(diff(&a, &b, 0).is_ok());
    }

    #[test]
    fn diff_within_threshold_passes() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [11, 21, 31, 255]);
        assert!(diff(&a, &b, 1).is_ok());
    }

    #[test]
    fn diff_exceeding_threshold_fails() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [10, 25, 30, 255]);
        match diff(&a, &b, 1) {
            Err(DiffError::PixelMismatch(r)) => {
                assert_eq!(r.max_observed, 5);
                assert!(r.channels_over_threshold > 0);
            }
            other => panic!("expected PixelMismatch, got {other:?}"),
        }
    }

    #[test]
    fn diff_dimension_mismatch_fails() {
        let a = solid(4, 4, [0, 0, 0, 255]);
        let b = solid(5, 4, [0, 0, 0, 255]);
        assert!(matches!(
            diff(&a, &b, 0),
            Err(DiffError::DimensionMismatch { .. })
        ));
    }
}
