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
    comp.background = [1.0, 0.0, 0.0, 1.0]; // red background
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
    // Opacity 0.5 on a white layer with background-fill outside the shape;
    // since the layer covers the whole canvas, every pixel is the white
    // layer at half-opacity (multiplied alpha).
    for p in img.pixels() {
        assert!(p[3] >= 120 && p[3] <= 140, "expected alpha ~128, got {p:?}");
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
