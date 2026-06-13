//! Video layer end-to-end (F-013 follow-up).
//!
//! Generates a tiny H.264 .mp4 via the existing felx-media encoder, then
//! renders it as a Video layer through the compositor and confirms the
//! resulting pixels carry signal.

use felx_core::model::{AssetKind, LayerKind, Project};
use felx_media::{EncodeOptions, H264Encoder};
use felx_render::compositor::Compositor;
use felx_render::texture_io::download_image;
use felx_render::{Renderer, RendererOptions};
use std::path::PathBuf;

fn try_renderer() -> Option<Renderer> {
    Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        ..Default::default()
    })
    .ok()
}

/// Produce a 4-frame 16x16 H.264 .mp4 with each frame a different solid
/// color. Returns the path of the generated file.
fn make_test_video() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("felx-video-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("test.mp4");
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let opts = EncodeOptions::h264_test(16, 16, 30, 1);
    let mut enc = H264Encoder::create(&path, opts).ok()?;
    let colors: [[u8; 4]; 4] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
    ];
    for color in &colors {
        let mut frame = Vec::with_capacity(16 * 16 * 4);
        for _ in 0..(16 * 16) {
            frame.extend_from_slice(color);
        }
        enc.write_rgba(&frame).ok()?;
    }
    enc.finish().ok()?;
    Some(path)
}

#[test]
fn video_layer_renders_through_compositor() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let Some(video_path) = make_test_video() else {
        eprintln!("skipping: H.264 encoder not available");
        return;
    };

    let mut p = Project::new();
    let asset = p.add_asset(video_path.clone(), AssetKind::Video);
    let comp_id = p.add_composition("main", 16, 16);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 4;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    comp.add_layer("clip", LayerKind::Video { asset }, 0, 4);

    let mut runtime = Compositor::new(renderer);
    // Frame 0 — should decode to red-ish.
    let tex = runtime.render(&p, comp_id, 0).unwrap();
    let img = download_image(runtime.renderer(), &tex);
    let pixel = *img.get_pixel(8, 8);
    // YUV420P round-trip won't be exactly (255,0,0) — just confirm
    // red dominates, signal is present, and alpha is opaque.
    assert!(
        pixel[0] > pixel[1] && pixel[0] > pixel[2],
        "frame 0 should decode red-dominant, got {pixel:?}"
    );
    assert!(pixel[3] >= 250, "alpha should be opaque: {pixel:?}");

    let _ = std::fs::remove_file(video_path);
}
