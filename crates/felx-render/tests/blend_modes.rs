//! Blend mode behavior verified through the multi-layer compositor.

use felx_core::model::{BlendMode, LayerKind, Project};
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

fn two_solid_comp(
    bottom: [f32; 4],
    top: [f32; 4],
    top_mode: BlendMode,
) -> (Project, felx_core::model::CompId) {
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 4, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    comp.add_layer("bot", LayerKind::Solid { color: bottom }, 0, 30);
    let top_id = comp.add_layer("top", LayerKind::Solid { color: top }, 0, 30);
    comp.layer_mut(top_id).unwrap().blend_mode = top_mode;
    (p, comp_id)
}

fn pixel_at_frame(
    project: &Project,
    comp_id: felx_core::model::CompId,
    renderer: Renderer,
) -> [u8; 4] {
    let mut comp = Compositor::new(renderer);
    let tex = comp.render(project, comp_id, 0).unwrap();
    let img = download_image(comp.renderer(), &tex);
    let p = img.pixels().next().unwrap();
    [p[0], p[1], p[2], p[3]]
}

#[test]
fn multiply_red_with_green_is_zero() {
    let Some(r) = try_renderer() else {
        return;
    };
    let (p, c) = two_solid_comp(
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        BlendMode::Multiply,
    );
    let pixel = pixel_at_frame(&p, c, r);
    assert!(pixel[0] <= 8);
    assert!(pixel[1] <= 8);
    assert!(pixel[2] <= 8);
}

#[test]
fn add_red_and_green_is_yellow() {
    let Some(r) = try_renderer() else {
        return;
    };
    let (p, c) = two_solid_comp(
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        BlendMode::Add,
    );
    let pixel = pixel_at_frame(&p, c, r);
    assert!(pixel[0] >= 250 && pixel[1] >= 250);
}

#[test]
fn screen_red_with_green_is_yellow() {
    let Some(r) = try_renderer() else {
        return;
    };
    let (p, c) = two_solid_comp(
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        BlendMode::Screen,
    );
    let pixel = pixel_at_frame(&p, c, r);
    assert!(pixel[0] >= 250 && pixel[1] >= 250);
    assert!(pixel[2] <= 8);
}

#[test]
fn difference_same_color_is_black() {
    let Some(r) = try_renderer() else {
        return;
    };
    let (p, c) = two_solid_comp(
        [0.5, 0.5, 0.5, 1.0],
        [0.5, 0.5, 0.5, 1.0],
        BlendMode::Difference,
    );
    let pixel = pixel_at_frame(&p, c, r);
    assert!(pixel[0] <= 8);
    assert!(pixel[1] <= 8);
    assert!(pixel[2] <= 8);
}

#[test]
fn lighten_takes_brighter_channel() {
    let Some(r) = try_renderer() else {
        return;
    };
    let (p, c) = two_solid_comp(
        [1.0, 0.2, 0.3, 1.0],
        [0.4, 0.9, 0.1, 1.0],
        BlendMode::Lighten,
    );
    let pixel = pixel_at_frame(&p, c, r);
    // Per-channel max → R from bg (1.0), G from top (0.9), B from bg (0.3).
    assert!(pixel[0] >= 250);
    assert!(pixel[1] >= 220 && pixel[1] <= 240);
    assert!(pixel[2] <= 90);
}
