//! End-to-end smoke test: a programmatic image matches a committed golden.

use felx_test::golden;
use image::{ImageBuffer, Rgba, RgbaImage};

fn solid_red() -> RgbaImage {
    ImageBuffer::from_pixel(4, 4, Rgba([255, 0, 0, 255]))
}

#[test]
fn solid_red_4x4_matches_golden() {
    golden!("solid_red_4x4", &solid_red());
}
