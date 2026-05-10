//! Visual regression test for the Gain effect.

use felx_render::effects::gain::{Gain, GainParams};
use felx_render::texture_io::{
    COMPOSITOR_FORMAT, create_render_target, download_image, upload_image,
};
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

fn red_ramp_8x4() -> RgbaImage {
    // Horizontal ramp 0..255 in R, alpha=255.
    let mut img: RgbaImage = ImageBuffer::new(8, 4);
    for (x, _y, p) in img.enumerate_pixels_mut() {
        let r = (x as f32 / 7.0 * 255.0).round() as u8;
        *p = Rgba([r, 0, 0, 255]);
    }
    img
}

#[test]
fn gain_half_matches_golden() {
    let Some(renderer) = try_renderer() else {
        eprintln!("[gain test] no wgpu adapter; skipping");
        return;
    };

    let input = red_ramp_8x4();
    let in_tex = upload_image(&renderer, &input);
    let out_tex = create_render_target(&renderer, 8, 4, "gain-output");

    let in_view = in_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let gain = Gain::new(&renderer, COMPOSITOR_FORMAT);
    let mut encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gain-test"),
        });
    gain.render(
        &renderer,
        &mut encoder,
        &in_view,
        &out_view,
        GainParams::new(0.5),
    );
    renderer.queue().submit(Some(encoder.finish()));

    let out_img = download_image(&renderer, &out_tex);
    golden!("gain_half_red_ramp_8x4", &out_img, max_diff: 2);
}

#[test]
fn gain_one_is_pass_through() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let input = red_ramp_8x4();
    let in_tex = upload_image(&renderer, &input);
    let out_tex = create_render_target(&renderer, 8, 4, "gain-output");
    let in_view = in_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let gain = Gain::new(&renderer, COMPOSITOR_FORMAT);
    let mut encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gain-test"),
        });
    gain.render(
        &renderer,
        &mut encoder,
        &in_view,
        &out_view,
        GainParams::new(1.0),
    );
    renderer.queue().submit(Some(encoder.finish()));

    let out_img = download_image(&renderer, &out_tex);
    // Within the per-channel tolerance, output equals input.
    for (a, e) in out_img.pixels().zip(input.pixels()) {
        for c in 0..4 {
            assert!(
                a[c].abs_diff(e[c]) <= 2,
                "gain=1 should be pass-through; got {a:?} expected {e:?}"
            );
        }
    }
}
