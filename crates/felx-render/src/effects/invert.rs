//! Invert effect — pure-Rust CPU pass. RGB inverted, alpha pass-through.

use image::RgbaImage;

pub const EFFECT_ID: &str = "invert";

/// In-place pixel mutation. Trivially parallelizable; we keep it serial for
/// now since the compositor's per-effect threading model is still TBD.
pub fn invert_in_place(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        p[0] = 255 - p[0];
        p[1] = 255 - p[1];
        p[2] = 255 - p[2];
        // alpha unchanged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn inverts_each_channel() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(2, 2, Rgba([10, 20, 30, 200]));
        invert_in_place(&mut img);
        for p in img.pixels() {
            assert_eq!(p[0], 245);
            assert_eq!(p[1], 235);
            assert_eq!(p[2], 225);
            assert_eq!(p[3], 200);
        }
    }

    #[test]
    fn double_invert_is_identity() {
        let original: RgbaImage = ImageBuffer::from_pixel(4, 4, Rgba([42, 99, 200, 128]));
        let mut working = original.clone();
        invert_in_place(&mut working);
        invert_in_place(&mut working);
        assert_eq!(original.as_raw(), working.as_raw());
    }
}
