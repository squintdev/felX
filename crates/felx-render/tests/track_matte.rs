//! Track matte tests (F-043 alpha, F-044 luma).

use felx_core::model::{LayerKind, Project, TrackMatteMode};
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
fn alpha_matte_with_opaque_source_lets_target_through() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 4, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [1.0, 0.0, 0.0, 1.0]; // red

    // Vec[0]: target — green solid, uses Vec[1] as alpha matte.
    let target = comp.add_layer(
        "target",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );
    // Vec[1]: source — fully opaque white. Source layer's alpha is the
    // gating; opaque alpha → target shows through fully.
    comp.add_layer(
        "matte",
        LayerKind::Solid {
            color: [1.0, 1.0, 1.0, 1.0],
        },
        0,
        30,
    );
    comp.layer_mut(target).unwrap().track_matte = Some(TrackMatteMode::Alpha);

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    // Target (green) should be visible — not the red background or the
    // (suppressed) white matte source.
    assert!(pixel[1] >= 240, "expected green target, got {pixel:?}");
}

#[test]
fn alpha_inverted_matte_with_opaque_source_hides_target() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 4, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [1.0, 0.0, 0.0, 1.0]; // red

    let target = comp.add_layer(
        "target",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.add_layer(
        "matte",
        LayerKind::Solid {
            color: [1.0, 1.0, 1.0, 1.0],
        },
        0,
        30,
    );
    comp.layer_mut(target).unwrap().track_matte = Some(TrackMatteMode::AlphaInverted);

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    // Inverted: opaque source means hidden target → red background shows.
    assert!(pixel[0] >= 240, "expected red background, got {pixel:?}");
    assert!(pixel[1] <= 16);
}

#[test]
fn luma_matte_with_white_source_lets_target_through() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 4, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [1.0, 0.0, 0.0, 1.0];

    let target = comp.add_layer(
        "target",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.add_layer(
        "matte",
        LayerKind::Solid {
            color: [1.0, 1.0, 1.0, 1.0],
        },
        0,
        30,
    );
    comp.layer_mut(target).unwrap().track_matte = Some(TrackMatteMode::Luma);

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    assert!(pixel[1] >= 240, "expected green target, got {pixel:?}");
}

#[test]
fn luma_matte_with_black_source_hides_target() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 4, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [1.0, 0.0, 0.0, 1.0];

    let target = comp.add_layer(
        "target",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.add_layer(
        "matte",
        LayerKind::Solid {
            color: [0.0, 0.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.layer_mut(target).unwrap().track_matte = Some(TrackMatteMode::Luma);

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    // Black source has zero luminance → target hidden → red background.
    assert!(pixel[0] >= 240, "expected red background, got {pixel:?}");
    assert!(pixel[1] <= 16);
}
