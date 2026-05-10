//! Verify sRGB encode/decode round-trips to the input within tolerance, and
//! that the encoded values match the analytic OETF for a known mid-gray.

use felx_render::srgb_wrap::SrgbWrap;
use felx_render::texture_io::{
    COMPOSITOR_FORMAT, create_render_target, download_image, upload_image,
};
use felx_render::{Renderer, RendererOptions};
use image::{ImageBuffer, Rgba, RgbaImage};

fn try_renderer() -> Option<Renderer> {
    Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        ..Default::default()
    })
    .ok()
}

fn solid(rgba: [u8; 4]) -> RgbaImage {
    ImageBuffer::from_pixel(8, 8, Rgba(rgba))
}

#[test]
fn encode_then_decode_is_near_identity() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let wrap = SrgbWrap::new(&renderer, COMPOSITOR_FORMAT);

    // Mid-gray (linear).
    let input = solid([128, 64, 200, 255]);
    let in_tex = upload_image(&renderer, &input);
    let mid = create_render_target(&renderer, 8, 8, "srgb-mid");
    let out = create_render_target(&renderer, 8, 8, "srgb-out");

    wrap.encode(
        &renderer,
        &in_tex.create_view(&wgpu::TextureViewDescriptor::default()),
        &mid.create_view(&wgpu::TextureViewDescriptor::default()),
    );
    wrap.decode(
        &renderer,
        &mid.create_view(&wgpu::TextureViewDescriptor::default()),
        &out.create_view(&wgpu::TextureViewDescriptor::default()),
    );

    let result = download_image(&renderer, &out);
    for (a, b) in result.pixels().zip(input.pixels()) {
        for c in 0..4 {
            assert!(
                a[c].abs_diff(b[c]) <= 2,
                "round-trip drift too large: got {a:?} expected {b:?}"
            );
        }
    }
}

#[test]
fn encode_lifts_linear_midgray() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let wrap = SrgbWrap::new(&renderer, COMPOSITOR_FORMAT);

    // Linear 0.5 should encode to ~0.735 (sRGB), well above the input.
    // 0.5 * 255 = 128.
    let input = solid([128, 128, 128, 255]);
    let in_tex = upload_image(&renderer, &input);
    let out = create_render_target(&renderer, 8, 8, "srgb-encoded");
    wrap.encode(
        &renderer,
        &in_tex.create_view(&wgpu::TextureViewDescriptor::default()),
        &out.create_view(&wgpu::TextureViewDescriptor::default()),
    );
    let result = download_image(&renderer, &out);
    let p = result.pixels().next().unwrap();
    // Expected: 1.055 * 0.5^(1/2.4) - 0.055 ≈ 0.7354 → 187/255.
    assert!(
        p[0] >= 180 && p[0] <= 195,
        "expected encoded R near 187, got {}",
        p[0]
    );
    assert_eq!(p[3], 255, "alpha should be unchanged");
}
