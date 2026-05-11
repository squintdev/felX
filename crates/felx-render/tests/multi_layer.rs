//! Multi-layer composition tests (F-040).

use felx_core::model::{LayerKind, Project};
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
fn two_solid_layers_top_layer_is_visible() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    // Bottom: red. Top: green. Top should win.
    comp.add_layer(
        "bottom",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.add_layer(
        "top",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    assert!(
        pixel[1] > pixel[0],
        "top green layer should show: {pixel:?}"
    );
    assert!(pixel[1] > pixel[2]);
}

#[test]
fn layer_in_out_range_excludes_layers() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    // Layer A is visible only at frames 0-9.
    comp.add_layer(
        "early",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        10,
    );
    // Layer B is visible at frames 10-29.
    comp.add_layer(
        "late",
        LayerKind::Solid {
            color: [0.0, 0.0, 1.0, 1.0],
        },
        10,
        30,
    );

    let mut comp_runtime = Compositor::new(renderer);

    // Frame 0: red.
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let p0 = *download_image(comp_runtime.renderer(), &tex)
        .pixels()
        .next()
        .unwrap();
    assert!(p0[0] > p0[2], "frame 0 should be red-dominant: {p0:?}");

    // Frame 15: blue.
    let tex = comp_runtime.render(&p, comp_id, 15).unwrap();
    let p15 = *download_image(comp_runtime.renderer(), &tex)
        .pixels()
        .next()
        .unwrap();
    assert!(p15[2] > p15[0], "frame 15 should be blue-dominant: {p15:?}");
}

#[test]
fn comp_background_shows_when_no_layers_cover() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.5, 1.0, 1.0]; // sky blue

    // One null layer just so the comp has something visible (the
    // compositor's NoVisibleLayer guard would otherwise fire).
    comp.add_layer("ghost", LayerKind::Null, 0, 30);

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let pixel = img.pixels().next().unwrap();
    // Sky blue background.
    assert!(
        pixel[2] >= 250,
        "expected blue from background, got {pixel:?}"
    );
    assert!(pixel[1] >= 120 && pixel[1] <= 140);
    assert!(pixel[0] <= 8);
}

#[test]
fn audio_layer_does_not_break_visual_render() {
    // Regression: a Video import auto-adds an Audio layer that points at
    // the same file. The Audio layer must not contribute to the visual
    // stack (it's the host audio mixer's job) or trip the compositor's
    // "unsupported Audio" branch and abort the render.
    use felx_core::model::{AssetId, AssetKind};
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let asset = p.add_asset("/nonexistent/clip.mp4", AssetKind::Audio);
    assert_eq!(asset, AssetId(1));
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    // Visual layer first, audio layer on top.
    comp.add_layer(
        "red",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.add_layer("audio_only", LayerKind::Audio { asset }, 0, 30);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime
        .render(&p, comp_id, 0)
        .expect("audio layer must not break the render");
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.pixels().next().unwrap();
    assert!(pixel[0] >= 250, "red should still show: {pixel:?}");
}
