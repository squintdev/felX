//! Layer transform tests via the compositor.

use felx_core::model::{Curve, Project};
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

fn solid_project_white_layer() -> (Project, felx_core::model::CompId) {
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 32, 16);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    // Default: opaque red comp background so off-canvas regions show red.
    comp.background = [1.0, 0.0, 0.0, 1.0];
    comp.add_solid("white", [1.0, 1.0, 1.0, 1.0]);
    (p, comp_id)
}

#[test]
fn identity_transform_does_not_change_output() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (project, comp_id) = solid_project_white_layer();
    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    // Identity: the white layer fills the comp.
    for p in img.pixels() {
        assert_eq!(*p, image::Rgba([255, 255, 255, 255]));
    }
}

#[test]
fn translation_off_canvas_shows_background() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (mut project, comp_id) = solid_project_white_layer();
    // Translate the layer entirely off-canvas (by 1000px right). The comp
    // background (red) should fill instead.
    let comp = project.composition_mut(comp_id).unwrap();
    comp.layers[0].transform.position = Curve::Static([1000.0, 0.0]);
    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    for p in img.pixels() {
        // Red background — RGBA = (255, 0, 0, 255).
        assert!(p[0] >= 250, "expected red background, got {p:?}");
        assert!(p[1] <= 8);
        assert!(p[2] <= 8);
    }
}

#[test]
fn opacity_half_blends_layer_with_background() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (mut project, comp_id) = solid_project_white_layer();
    let comp = project.composition_mut(comp_id).unwrap();
    comp.layers[0].transform.opacity = Curve::Static(0.5);
    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    // White layer at 50% opacity over an opaque red background. Final
    // alpha is 1.0 (background is opaque); RGB blends to ~(0.75, 0.5, 0.5)
    // = (191, 128, 128). We just sanity-check that some white showed up
    // on top of the red.
    for p in img.pixels() {
        assert!(p[1] >= 100, "green channel low — layer didn't blend, {p:?}");
        assert!(p[2] >= 100, "blue channel low — layer didn't blend, {p:?}");
    }
}

#[test]
fn scale_zero_is_handled_gracefully() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (mut project, comp_id) = solid_project_white_layer();
    let comp = project.composition_mut(comp_id).unwrap();
    comp.layers[0].transform.scale = Curve::Static([0.0, 0.0]);
    let mut comp_runtime = Compositor::new(renderer);
    // Should not panic / crash; the shader's `select` falls through to 0,0.
    let _ = comp_runtime.render(&project, comp_id, 0).unwrap();
}
