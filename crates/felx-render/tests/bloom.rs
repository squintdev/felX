//! Bloom effect (F-076) — threshold + separable Gaussian + composite.

use felx_core::model::{Effect, LayerKind, Project};
use felx_core::params::ParamValue;
use felx_render::compositor::Compositor;
use felx_render::texture_io::download_image;
use felx_render::{Renderer, RendererOptions};

fn try_renderer() -> Option<Renderer> {
    Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        ..Default::default()
    })
    .ok()
}

#[test]
fn bloom_brightens_a_bright_image() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 16, 16);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];

    // Bright white solid → bloom should brighten further (additive).
    let layer_id = comp.add_layer(
        "bright",
        LayerKind::Solid {
            color: [1.0, 1.0, 1.0, 1.0],
        },
        0,
        30,
    );
    let mut bloom = Effect::new("bloom");
    bloom.values.set("threshold", ParamValue::Float(0.5));
    bloom.values.set("intensity", ParamValue::Float(1.0));
    bloom.values.set("radius", ParamValue::Float(2.0));
    bloom.values.set("soft_knee", ParamValue::Float(0.1));
    comp.push_effect(layer_id, bloom);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.get_pixel(8, 8);
    // Solid white is already 255 — additive bloom on top stays clamped at
    // 255. The point of this test is just that bloom didn't darken anything.
    assert_eq!(pixel[0], 255);
    assert_eq!(pixel[1], 255);
    assert_eq!(pixel[2], 255);
}

#[test]
fn bloom_passes_dark_image_through_with_no_glow() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 16, 16);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    // Dim grey, well below threshold.
    let layer_id = comp.add_layer(
        "dim",
        LayerKind::Solid {
            color: [0.2, 0.2, 0.2, 1.0],
        },
        0,
        30,
    );
    let mut bloom = Effect::new("bloom");
    bloom.values.set("threshold", ParamValue::Float(0.9));
    bloom.values.set("intensity", ParamValue::Float(2.0));
    bloom.values.set("radius", ParamValue::Float(4.0));
    bloom.values.set("soft_knee", ParamValue::Float(0.05));
    comp.push_effect(layer_id, bloom);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.get_pixel(8, 8);
    // Output should still be ~0.2*255 = ~51, untouched by bloom.
    assert!(
        (45..=58).contains(&pixel[0]),
        "expected dim grey to pass through, got {pixel:?}"
    );
}
