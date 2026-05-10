//! Layer mask rendering (F-061…F-066).

use felx_core::model::{LayerKind, Mask, MaskMode, Project};
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
fn rectangle_mask_gates_layer_alpha() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 16, 16);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];

    let layer_id = comp.add_layer(
        "red",
        LayerKind::Solid {
            color: [1.0, 0.0, 0.0, 1.0],
        },
        0,
        30,
    );
    comp.layer_mut(layer_id)
        .unwrap()
        .masks
        .push(Mask::rectangle("box", 4.0, 4.0, 8.0, 8.0));

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    // Inside the mask: red shows.
    let inside = *img.get_pixel(8, 8);
    assert!(inside[0] >= 200, "expected red inside mask: {inside:?}");
    // Outside the mask: layer is gated to alpha 0, comp background (black)
    // shows through.
    let outside = *img.get_pixel(0, 0);
    assert!(
        outside[0] <= 8,
        "expected background outside mask: {outside:?}"
    );
}

#[test]
fn ellipse_mask_softens_corners() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 32, 32);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];

    let layer_id = comp.add_layer(
        "blue",
        LayerKind::Solid {
            color: [0.0, 0.0, 1.0, 1.0],
        },
        0,
        30,
    );
    comp.layer_mut(layer_id)
        .unwrap()
        .masks
        .push(Mask::ellipse("oval", 16.0, 16.0, 12.0, 12.0));

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    // Center: blue.
    let center = *img.get_pixel(16, 16);
    assert!(
        center[2] >= 200,
        "ellipse center should show blue: {center:?}"
    );
    // Corner: gated out by the ellipse → background.
    let corner = *img.get_pixel(0, 0);
    assert!(
        corner[2] <= 8,
        "ellipse corner should be background: {corner:?}"
    );
}

#[test]
fn intersect_combines_two_masks_geometrically() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 16, 16);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.background = [0.0, 0.0, 0.0, 1.0];

    let layer_id = comp.add_layer(
        "green",
        LayerKind::Solid {
            color: [0.0, 1.0, 0.0, 1.0],
        },
        0,
        30,
    );
    let layer = comp.layer_mut(layer_id).unwrap();
    layer.masks.push(Mask::rectangle("a", 0.0, 0.0, 12.0, 16.0));
    let mut b = Mask::rectangle("b", 4.0, 0.0, 12.0, 16.0);
    b.mode = MaskMode::Intersect;
    layer.masks.push(b);

    let mut runtime = Compositor::new(renderer);
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    // Overlap region (x=4..=11): green visible.
    let overlap = *img.get_pixel(8, 8);
    assert!(overlap[1] >= 200, "overlap should be green: {overlap:?}");
    // x=2 is in mask A only — intersect should hide it.
    let only_a = *img.get_pixel(2, 8);
    assert!(only_a[1] <= 8, "x=2 should be gated out: {only_a:?}");
    // x=14 is in mask B only — intersect should hide it.
    let only_b = *img.get_pixel(14, 8);
    assert!(only_b[1] <= 8, "x=14 should be gated out: {only_b:?}");
}
