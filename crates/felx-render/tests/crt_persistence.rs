//! CRT phosphor persistence (F-079) — uses the F-070 effect-state infra.

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

/// Build a comp where a red flash on frame 0 should leave a fading trail
/// in subsequent frames thanks to persistence on an Adjustment layer that
/// sees the flattened-below result. Adjustment layer is the right host for
/// cross-layer state — per-layer effects only see their own layer's input.
fn build_comp_with_persistence(decay: f32) -> (Project, felx_core::model::CompId) {
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 4, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    // Bottom: red on frame 0, gone after.
    comp.add_layer(
        "red_one_frame",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        1,
    );
    // Top: an adjustment layer running persistence on whatever's below.
    let adj = comp.add_layer("persistence_adj", LayerKind::Adjustment, 0, 30);
    let mut persistence = Effect::new("crt_persistence");
    persistence.values.set("decay", ParamValue::Float(decay));
    persistence.values.set("tint_r", ParamValue::Float(1.0));
    persistence.values.set("tint_g", ParamValue::Float(1.0));
    persistence.values.set("tint_b", ParamValue::Float(1.0));
    comp.push_effect(adj, persistence);
    (p, comp_id)
}

#[test]
fn persistence_carries_red_into_subsequent_frame() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (p, comp_id) = build_comp_with_persistence(0.85);
    let mut runtime = Compositor::new(renderer);
    // Frame 0: accumulator after red layer = red. Adjustment runs
    // persistence: state.read is black (reset), input is red, output = max(red, decay*black) = red.
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    assert!(
        img.get_pixel(2, 2)[0] >= 200,
        "frame 0 should be red: {:?}",
        img.get_pixel(2, 2)
    );
    // Frame 1: red layer gone → accumulator = comp background (black).
    // Adjustment runs persistence: state.read = previous output (red),
    // input = black, output = max(black, decay*red) = ~0.85 red.
    let tex = runtime.render(&p, comp_id, 1).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.get_pixel(2, 2);
    assert!(
        pixel[0] >= 180,
        "expected red trail to persist into frame 1: {pixel:?}"
    );
}

#[test]
fn seek_resets_persistence_state() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (p, comp_id) = build_comp_with_persistence(0.95);
    let mut runtime = Compositor::new(renderer);
    // Render frame 0 then jump to frame 5 — non-monotonic = reset.
    let _ = runtime.render(&p, comp_id, 0).unwrap();
    let tex = runtime.render(&p, comp_id, 5).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.get_pixel(2, 2);
    // After reset, state.read is black; input is black (red layer gone);
    // output is black.
    assert!(pixel[0] < 16, "expected reset to clear trail: {pixel:?}");
}
