//! Pre-comp / nested compositions (F-046).

use felx_core::model::{LayerKind, Project};
use felx_render::compositor::{Compositor, CompositorError};
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
fn precomp_passes_inner_pixels_to_outer() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();

    // Inner: a solid magenta layer on its own comp.
    let inner_id = p.add_composition("inner", 8, 8);
    let inner = p.composition_mut(inner_id).unwrap();
    inner.duration_frames = 30;
    inner.background = [0.0, 0.0, 0.0, 1.0];
    inner.add_layer(
        "fill",
        LayerKind::Solid {
            color: [1.0, 0.0, 1.0, 1.0],
        },
        0,
        30,
    );

    // Outer: hosts a Composition layer pointing at the inner comp.
    let outer_id = p.add_composition("outer", 8, 8);
    let outer = p.composition_mut(outer_id).unwrap();
    outer.duration_frames = 30;
    outer.background = [0.0, 0.0, 0.0, 1.0];
    outer.add_layer("nested", LayerKind::Composition { comp: inner_id }, 0, 30);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, outer_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    // Magenta from the inner comp should reach the outer's output.
    assert!(pixel[0] >= 250, "expected red channel high: {pixel:?}");
    assert!(pixel[1] <= 8, "expected green channel low: {pixel:?}");
    assert!(pixel[2] >= 250, "expected blue channel high: {pixel:?}");
}

#[test]
fn precomp_resizes_inner_to_outer_dims() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();

    // Inner: 4x4 cyan solid.
    let inner_id = p.add_composition("inner", 4, 4);
    let inner = p.composition_mut(inner_id).unwrap();
    inner.duration_frames = 30;
    inner.background = [0.0, 0.0, 0.0, 1.0];
    inner.add_layer(
        "fill",
        LayerKind::Solid {
            color: [0.0, 1.0, 1.0, 1.0],
        },
        0,
        30,
    );

    // Outer: 16x16. The pre-comp's 4x4 output must be resized up.
    let outer_id = p.add_composition("outer", 16, 16);
    let outer = p.composition_mut(outer_id).unwrap();
    outer.duration_frames = 30;
    outer.background = [0.0, 0.0, 0.0, 1.0];
    outer.add_layer("nested", LayerKind::Composition { comp: inner_id }, 0, 30);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, outer_id, 0).unwrap();
    assert_eq!(tex.width(), 16);
    assert_eq!(tex.height(), 16);
    let img = download_image(runtime.renderer(), &tex);
    // Sample center to avoid any edge artifacts; should still be cyan.
    let pixel = *img.get_pixel(8, 8);
    assert!(pixel[0] <= 8, "red low: {pixel:?}");
    assert!(pixel[1] >= 250, "green high: {pixel:?}");
    assert!(pixel[2] >= 250, "blue high: {pixel:?}");
}

#[test]
fn precomp_time_offset_picks_a_different_inner_frame() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();

    // Inner: frame 0..=4 are red, frame 5..=9 are blue. Achieve this with
    // two layers whose in/out gate them.
    let inner_id = p.add_composition("inner", 4, 4);
    let inner = p.composition_mut(inner_id).unwrap();
    inner.duration_frames = 10;
    inner.background = [0.0, 0.0, 0.0, 1.0];
    inner.add_layer(
        "red_first",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        5,
    );
    inner.add_layer(
        "blue_second",
        LayerKind::Solid {
            color: [0.0, 0.0, 1.0, 1.0],
        },
        5,
        10,
    );

    // Outer: hosts the inner pre-comp with time_offset_frames = +6.
    // At outer frame 0, inner sees source frame 6 → blue.
    let outer_id = p.add_composition("outer", 4, 4);
    let outer = p.composition_mut(outer_id).unwrap();
    outer.duration_frames = 10;
    outer.background = [0.0, 0.0, 0.0, 1.0];
    let nested_id = outer.add_layer("nested", LayerKind::Composition { comp: inner_id }, 0, 10);
    outer.layer_mut(nested_id).unwrap().time_offset_frames = 6;

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, outer_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.get_pixel(2, 2);
    assert!(
        pixel[2] > pixel[0],
        "with offset=+6, frame 0 of outer should pick up the blue half of inner: {pixel:?}"
    );
}

#[test]
fn precomp_self_reference_returns_cycle_error() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let cid = p.add_composition("self", 8, 8);
    let c = p.composition_mut(cid).unwrap();
    c.duration_frames = 30;
    c.background = [0.0, 0.0, 0.0, 1.0];
    c.add_layer("loop", LayerKind::Composition { comp: cid }, 0, 30);

    let mut runtime = Compositor::new(renderer);
    let err = runtime.render(&p, cid, 0).unwrap_err();
    assert!(
        matches!(err, CompositorError::PrecompCycle(_)),
        "expected PrecompCycle, got {err:?}"
    );
}
