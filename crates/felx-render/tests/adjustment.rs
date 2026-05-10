//! Adjustment layer (F-060): apply effect stack to flattened layers below.

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
fn adjustment_layer_with_invert_flips_layers_beneath() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];

    // Bottom: red.
    comp.add_layer(
        "red",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        30,
    );
    // Top: an Adjustment layer running the Invert effect.
    let adj = comp.add_layer("adj", LayerKind::Adjustment, 0, 30);
    let mut invert = Effect::new("invert");
    invert.values.set("placeholder", ParamValue::Bool(false));
    comp.push_effect(adj, invert);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.pixels().next().unwrap();
    // Inverted red → cyan.
    assert!(
        pixel[0] <= 8,
        "expected red channel low after invert: {pixel:?}"
    );
    assert!(pixel[1] >= 250, "expected green channel high: {pixel:?}");
    assert!(pixel[2] >= 250, "expected blue channel high: {pixel:?}");
}

#[test]
fn adjustment_layer_with_no_effects_is_passthrough() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    comp.add_layer(
        "green",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );
    // Adjustment with no effects should not change anything beneath it.
    comp.add_layer("noop", LayerKind::Adjustment, 0, 30);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.pixels().next().unwrap();
    assert!(pixel[0] <= 8, "red low: {pixel:?}");
    assert!(pixel[1] >= 250, "green high: {pixel:?}");
    assert!(pixel[2] <= 8, "blue low: {pixel:?}");
}

#[test]
fn adjustment_layer_only_affects_layers_below_it_in_stack() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];

    // Bottom: red. Adjustment with invert. Top: green. The green layer is
    // above the adjustment so it should NOT be inverted.
    comp.add_layer(
        "red_below",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        30,
    );
    let adj = comp.add_layer("adj", LayerKind::Adjustment, 0, 30);
    comp.push_effect(adj, Effect::new("invert"));
    comp.add_layer(
        "green_above",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.pixels().next().unwrap();
    // The top green layer composites over whatever the adjustment produced
    // and dominates (since green is opaque). So we expect green.
    assert!(pixel[1] >= 250, "expected green top to win: {pixel:?}");
}
