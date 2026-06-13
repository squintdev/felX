//! End-to-end test for the headless render walker.

use felx_core::model::{LayerKind, Project};
use felx_render::compositor::Compositor;
use felx_render::walker::{
    PngSequenceOptions, render_full_comp_to_png_sequence, render_to_png_sequence,
};
use felx_render::{Renderer, RendererOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("felx-walker-{label}-{pid}-{n}"));
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

fn ten_frame_solid_project() -> (Project, felx_core::model::CompId) {
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 4);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 10;
    comp.add_layer(
        "bg",
        LayerKind::Solid {
            color: [0.5, 0.5, 0.5, 1.0],
        },
        0,
        10,
    );
    (p, comp_id)
}

#[test]
fn writes_one_png_per_frame() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut comp_runtime = Compositor::new(renderer);
    let (project, comp_id) = ten_frame_solid_project();
    let dir = scratch_dir("seq");
    let opts = PngSequenceOptions::new(&dir, "f_{frame:04}.png");
    let n = render_to_png_sequence(&mut comp_runtime, &project, comp_id, 0..5, &opts).unwrap();
    assert_eq!(n, 5);
    for i in 0..5 {
        let p = dir.join(format!("f_{:04}.png", i));
        assert!(p.exists(), "expected {} to exist", p.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn full_comp_walks_every_frame() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut comp_runtime = Compositor::new(renderer);
    let (project, comp_id) = ten_frame_solid_project();
    let dir = scratch_dir("full");
    let n = render_full_comp_to_png_sequence(
        &mut comp_runtime,
        &project,
        comp_id,
        &dir,
        "frame_{frame:05}.png",
    )
    .unwrap();
    assert_eq!(n, 10);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_range_writes_zero_frames() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let mut comp_runtime = Compositor::new(renderer);
    let (project, comp_id) = ten_frame_solid_project();
    let dir = scratch_dir("empty");
    let opts = PngSequenceOptions::new(&dir, "{frame:03}.png");
    let n = render_to_png_sequence(&mut comp_runtime, &project, comp_id, 5..5, &opts).unwrap();
    assert_eq!(n, 0);
    let _ = std::fs::remove_dir_all(&dir);
}
