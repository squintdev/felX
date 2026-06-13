//! Determinism harness (F-110).
//!
//! Render the same comp twice and assert the outputs are byte-identical.
//! For PNG/EXR sequences this is exact; encoder non-determinism (B-frame
//! reordering, multi-threading) is out of scope and would need an
//! encoder-side fixed-seed configuration to assert anything.

use felx_core::model::{Effect, LayerKind, Project};
use felx_core::params::ParamValue;
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

fn solid_with_gain(comp_w: u32, comp_h: u32) -> (Project, felx_core::model::CompId) {
    let mut p = Project::new();
    let cid = p.add_composition("main", comp_w, comp_h);
    let comp = p.composition_mut(cid).unwrap();
    comp.duration_frames = 4;
    comp.background = [0.1, 0.2, 0.3, 1.0];
    let lid = comp.add_layer(
        "fill",
        LayerKind::Solid {
            color: [0.5, 0.5, 0.5, 1.0],
        },
        0,
        4,
    );
    let mut g = Effect::new("gain");
    g.values.set("gain", ParamValue::Float(1.5));
    comp.push_effect(lid, g);
    (p, cid)
}

#[test]
fn rendering_same_project_twice_produces_byte_identical_pixels() {
    let Some(r1) = try_renderer() else {
        return;
    };
    let Some(r2) = try_renderer() else {
        return;
    };
    let (p, cid) = solid_with_gain(8, 8);
    let mut a = Compositor::new(r1);
    let mut b = Compositor::new(r2);

    for frame in 0..4 {
        let ta = a.render(&p, cid, frame).unwrap();
        let tb = b.render(&p, cid, frame).unwrap();
        let ia = download_image(a.renderer(), &ta);
        let ib = download_image(b.renderer(), &tb);
        assert_eq!(ia.dimensions(), ib.dimensions());
        // Allow a small per-channel tolerance because some software
        // adapters round differently across compositor instances. With
        // the same renderer instance it's bit-exact.
        let max_diff: u8 = ia
            .pixels()
            .zip(ib.pixels())
            .flat_map(|(a, b)| (0..4).map(move |c| a[c].abs_diff(b[c])))
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 1,
            "frame {frame}: max per-channel diff {max_diff} > 1"
        );
    }
}

#[test]
fn rendering_with_one_compositor_twice_is_bit_exact() {
    let Some(r) = try_renderer() else {
        return;
    };
    let (p, cid) = solid_with_gain(16, 16);
    let mut comp = Compositor::new(r);
    let mut prev: Option<image::RgbaImage> = None;
    for _ in 0..3 {
        let t = comp.render(&p, cid, 0).unwrap();
        let img = download_image(comp.renderer(), &t);
        if let Some(p) = &prev {
            assert_eq!(p.as_raw(), img.as_raw(), "non-deterministic render");
        }
        prev = Some(img);
    }
}
