//! SquintDiffusion — directional error-diffusion halftoning. v1: horizontal
//! left-to-right scan with a 2..=6 color palette and adjustable error
//! weight. Walks each row independently (rows are perpendicular to the scan
//! and don't share residuals); rayon-friendly when we wire it up.
//!
//! Algorithm per effects.md § SquintDiffusion. Operates on sRGB-encoded
//! values (the CPU pass receives the pixels straight from the compositor's
//! linear buffer; perceptual color matching is the goal so we do the math
//! on the values as-is for v1, matching the approximate look).

use image::RgbaImage;

#[derive(Clone, Debug)]
pub struct DiffusionParams {
    pub error_weight: f32,
    pub alpha: f32,
    pub palette: Vec<[f32; 4]>,
}

impl DiffusionParams {
    pub fn new(error_weight: f32, alpha: f32, palette: Vec<[f32; 4]>) -> Self {
        Self {
            error_weight,
            alpha,
            palette: if palette.is_empty() {
                vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]]
            } else {
                palette
            },
        }
    }
}

/// In-place horizontal-scan error diffusion. Each row carries its own
/// residual; rows are independent.
pub fn diffuse_in_place(img: &mut RgbaImage, params: &DiffusionParams) {
    let w = img.width() as usize;
    let h = img.height() as usize;
    let palette: Vec<[f32; 3]> = params.palette.iter().map(|c| [c[0], c[1], c[2]]).collect();
    if palette.is_empty() {
        return;
    }

    let raw = img.as_mut();
    for y in 0..h {
        let mut residual: [f32; 3] = [0.0, 0.0, 0.0];
        for x in 0..w {
            let off = (y * w + x) * 4;
            let original = [
                raw[off] as f32 / 255.0,
                raw[off + 1] as f32 / 255.0,
                raw[off + 2] as f32 / 255.0,
            ];
            let target = [
                original[0] + residual[0] * params.error_weight,
                original[1] + residual[1] * params.error_weight,
                original[2] + residual[2] * params.error_weight,
            ];
            let nearest = nearest_palette(&palette, target);
            residual = [
                target[0] - nearest[0],
                target[1] - nearest[1],
                target[2] - nearest[2],
            ];
            // Mix with original by `alpha` (1.0 = full effect, 0.0 = original).
            let mixed = [
                nearest[0] * params.alpha + original[0] * (1.0 - params.alpha),
                nearest[1] * params.alpha + original[1] * (1.0 - params.alpha),
                nearest[2] * params.alpha + original[2] * (1.0 - params.alpha),
            ];
            raw[off] = (mixed[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            raw[off + 1] = (mixed[1].clamp(0.0, 1.0) * 255.0).round() as u8;
            raw[off + 2] = (mixed[2].clamp(0.0, 1.0) * 255.0).round() as u8;
            // Alpha unchanged.
        }
    }
}

fn nearest_palette(palette: &[[f32; 3]], c: [f32; 3]) -> [f32; 3] {
    let mut best = palette[0];
    let mut best_dist = sq_dist(palette[0], c);
    for &p in &palette[1..] {
        let d = sq_dist(p, c);
        if d < best_dist {
            best_dist = d;
            best = p;
        }
    }
    best
}

fn sq_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    dr * dr + dg * dg + db * db
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn binary_palette_quantizes_grays() {
        // Mid-gray ramp through a black/white palette should produce a
        // mix of black and white pixels (Floyd-style dithering).
        let mut img: RgbaImage = ImageBuffer::from_pixel(16, 4, Rgba([128, 128, 128, 255]));
        let params = DiffusionParams::new(0.75, 1.0, vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]]);
        diffuse_in_place(&mut img, &params);
        let mut blacks = 0;
        let mut whites = 0;
        for p in img.pixels() {
            if p[0] == 0 {
                blacks += 1;
            } else if p[0] == 255 {
                whites += 1;
            }
        }
        assert!(
            blacks > 0 && whites > 0,
            "expected mix of B+W: {blacks} black, {whites} white"
        );
    }

    #[test]
    fn alpha_zero_is_identity() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(4, 4, Rgba([100, 150, 200, 255]));
        let params = DiffusionParams::new(1.0, 0.0, vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]]);
        diffuse_in_place(&mut img, &params);
        for p in img.pixels() {
            assert!(p[0].abs_diff(100) <= 1);
            assert!(p[1].abs_diff(150) <= 1);
            assert!(p[2].abs_diff(200) <= 1);
        }
    }

    #[test]
    fn alpha_passes_through() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(4, 4, Rgba([200, 200, 200, 64]));
        let params = DiffusionParams::new(1.0, 1.0, vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]]);
        diffuse_in_place(&mut img, &params);
        for p in img.pixels() {
            assert_eq!(p[3], 64);
        }
    }
}
