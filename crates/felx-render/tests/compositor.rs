//! End-to-end test for the single-layer compositor: build a project, point
//! a layer at an on-disk PNG, attach Gain (or Invert), render, golden compare.

use felx_core::model::{AssetKind, Effect, LayerKind, Project};
use felx_render::compositor::Compositor;
use felx_render::texture_io::download_image;
use felx_render::{Renderer, RendererOptions};
use felx_test::golden;
use image::{ImageBuffer, Rgba, RgbaImage};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("felx-comp-{label}-{pid}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn try_renderer() -> Option<Renderer> {
    Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        ..Default::default()
    })
    .ok()
}

fn write_red_ramp_png(path: &std::path::Path) {
    let mut img: RgbaImage = ImageBuffer::new(8, 4);
    for (x, _y, p) in img.enumerate_pixels_mut() {
        let r = (x * 32) as u8;
        *p = Rgba([r, 0, 0, 255]);
    }
    img.save(path).unwrap();
}

#[test]
fn renders_image_layer_with_gain_effect() {
    let Some(renderer) = try_renderer() else {
        eprintln!("[compositor test] no wgpu adapter; skipping");
        return;
    };

    let dir = scratch_dir("image-gain");
    let png_path = dir.join("ramp.png");
    write_red_ramp_png(&png_path);

    let mut project = Project::new();
    let asset = project.add_asset(&png_path, AssetKind::Image);
    let comp_id = project.add_composition("main", 8, 4);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    let layer = comp.add_layer("ramp", LayerKind::Image { asset }, 0, 30);
    comp.push_effect(layer, Effect::new("gain"));

    let mut compositor = Compositor::new(renderer);
    let out_tex = compositor.render(&project, comp_id, 0).unwrap();
    let out_img = download_image(compositor.renderer(), &out_tex);

    golden!("compositor_image_gain_8x4", &out_img, max_diff: 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn renders_solid_layer_no_effects() {
    let Some(renderer) = try_renderer() else {
        return;
    };

    let mut project = Project::new();
    let comp_id = project.add_composition("main", 4, 4);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    comp.add_layer(
        "bg",
        LayerKind::Solid {
            color: [0.5, 0.25, 0.75, 1.0],
        },
        0,
        30,
    );

    let mut compositor = Compositor::new(renderer);
    let out_tex = compositor.render(&project, comp_id, 0).unwrap();
    let out_img = download_image(compositor.renderer(), &out_tex);

    let expected = Rgba([128_u8, 64, 191, 255]);
    for p in out_img.pixels() {
        for c in 0..4 {
            assert!(
                p[c].abs_diff(expected[c]) <= 2,
                "expected ~{expected:?}, got {p:?}"
            );
        }
    }
}

#[test]
fn pool_reuses_textures_across_frames() {
    let Some(renderer) = try_renderer() else {
        return;
    };

    let dir = scratch_dir("pool-reuse");
    let png_path = dir.join("ramp.png");
    write_red_ramp_png(&png_path);

    let mut project = Project::new();
    let asset = project.add_asset(&png_path, AssetKind::Image);
    let comp_id = project.add_composition("main", 8, 4);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    let layer = comp.add_layer("ramp", LayerKind::Image { asset }, 0, 30);
    comp.push_effect(layer, Effect::new("gain"));

    let mut compositor = Compositor::new(renderer);

    // Render the same comp at the same dims a bunch of times; the pool
    // should bound out at a small number of textures rather than growing
    // unbounded.
    for f in 0..16 {
        let _ = compositor.render(&project, comp_id, f).unwrap();
    }
    let pool_size = compositor.pool().len();
    assert!(
        pool_size < 16,
        "pool grew unexpectedly large after 16 frames: {pool_size}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_visible_layer_returns_error() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut project = Project::new();
    let comp_id = project.add_composition("main", 4, 4);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.duration_frames = 100;
    comp.add_layer("brief", LayerKind::Null, 0, 10);

    let mut compositor = Compositor::new(renderer);
    let result = compositor.render(&project, comp_id, 50);
    assert!(matches!(
        result,
        Err(felx_render::compositor::CompositorError::NoVisibleLayer)
    ));
}
