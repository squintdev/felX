//! End-to-end CPU-pass test: upload → invert → download via run_cpu_pass.

use felx_render::cpu_pass::run_cpu_pass;
use felx_render::effects::invert::invert_in_place;
use felx_render::texture_io::{download_image, upload_image};
use felx_render::{Renderer, RendererOptions};
use felx_test::golden;
use image::{ImageBuffer, Rgba, RgbaImage};

fn try_renderer() -> Option<Renderer> {
    Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        ..Default::default()
    })
    .ok()
}

fn rainbow_8x4() -> RgbaImage {
    let mut img: RgbaImage = ImageBuffer::new(8, 4);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let r = (x * 32) as u8;
        let g = (y * 60) as u8;
        let b = ((x + y) * 24) as u8;
        *p = Rgba([r, g, b, 255]);
    }
    img
}

#[test]
fn invert_via_cpu_pass_matches_golden() {
    let Some(renderer) = try_renderer() else {
        eprintln!("[invert test] no wgpu adapter; skipping");
        return;
    };

    let input = rainbow_8x4();
    let in_tex = upload_image(&renderer, &input);
    let out_tex = run_cpu_pass(&renderer, &in_tex, "invert", invert_in_place);
    let out_img = download_image(&renderer, &out_tex);

    golden!("invert_rainbow_8x4", &out_img, max_diff: 1);
}

#[test]
fn invert_via_cpu_pass_is_idempotent_when_doubled() {
    let Some(renderer) = try_renderer() else {
        return;
    };

    let original = rainbow_8x4();
    let in_tex = upload_image(&renderer, &original);
    let once = run_cpu_pass(&renderer, &in_tex, "invert", invert_in_place);
    let twice = run_cpu_pass(&renderer, &once, "invert", invert_in_place);
    let result = download_image(&renderer, &twice);

    for (a, e) in result.pixels().zip(original.pixels()) {
        for c in 0..4 {
            assert!(
                a[c].abs_diff(e[c]) <= 1,
                "double-invert should restore original; got {a:?} expected {e:?}"
            );
        }
    }
}
