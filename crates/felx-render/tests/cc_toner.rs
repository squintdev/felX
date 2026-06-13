//! Visual + numeric sanity tests for CC Toner.
//!
//! The full algorithm (Rec.601 luminance, segment lookup, per-channel lerp
//! in sRGB-encoded space) is exercised end-to-end through the compositor's
//! cc_toner dispatch path so the SrgbWrap encode/decode legs are included.

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

fn solid_layer_project(color: [f32; 4]) -> (Project, felx_core::model::CompId, Effect) {
    let mut p = Project::new();
    let comp_id = p.add_composition("main", 8, 8);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.duration_frames = 30;
    let layer = comp.add_layer("src", LayerKind::Solid { color }, 0, 30);
    let mut effect = Effect::new("cc_toner");
    // Default tritone: shadows=black, midtones=mid-gray, highlights=white.
    effect
        .values
        .set("tones", ParamValue::Enum("tritone".into()));
    effect
        .values
        .set("highlights", ParamValue::Color([1.0, 1.0, 1.0, 1.0]));
    effect
        .values
        .set("midtones", ParamValue::Color([0.5, 0.5, 0.5, 1.0]));
    effect
        .values
        .set("shadows", ParamValue::Color([0.0, 0.0, 0.0, 1.0]));
    effect
        .values
        .set("brights", ParamValue::Color([0.75, 0.75, 0.75, 1.0]));
    effect
        .values
        .set("darktones", ParamValue::Color([0.25, 0.25, 0.25, 1.0]));
    effect.values.set("blend", ParamValue::Float(0.0));
    let effect_clone = effect.clone();
    comp.push_effect(layer, effect);
    (p, comp_id, effect_clone)
}

#[test]
fn duotone_black_white_on_white_input() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    // Pure white input → after duotone (shadows=black, highlights=white),
    // luminance is 1.0 → maps to highlights → still white-ish.
    let (mut project, comp_id, _) = solid_layer_project([1.0, 1.0, 1.0, 1.0]);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.layers[0].effects[0]
        .values
        .set("tones", ParamValue::Enum("duotone".into()));
    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    for p in img.pixels() {
        assert!(
            p[0] >= 240,
            "duotone of white should stay near white, got R={}",
            p[0]
        );
        assert!(p[1] >= 240);
        assert!(p[2] >= 240);
    }
}

#[test]
fn duotone_black_white_on_black_input() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    let (mut project, comp_id, _) = solid_layer_project([0.0, 0.0, 0.0, 1.0]);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.layers[0].effects[0]
        .values
        .set("tones", ParamValue::Enum("duotone".into()));
    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    for p in img.pixels() {
        assert!(
            p[0] <= 12,
            "duotone of black should stay near black, got R={}",
            p[0]
        );
        assert!(p[1] <= 12);
        assert!(p[2] <= 12);
    }
}

#[test]
fn duotone_red_to_blue_remaps_color() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    // Mid-gray input. Duotone shadows=red, highlights=blue.
    // Mid-gray is luminance ≈ 0.5 → halfway between red and blue → purple-ish.
    let (mut project, comp_id, _) = solid_layer_project([0.5, 0.5, 0.5, 1.0]);
    let comp = project.composition_mut(comp_id).unwrap();
    let eff = &mut comp.layers[0].effects[0];
    eff.values.set("tones", ParamValue::Enum("duotone".into()));
    eff.values
        .set("shadows", ParamValue::Color([1.0, 0.0, 0.0, 1.0]));
    eff.values
        .set("highlights", ParamValue::Color([0.0, 0.0, 1.0, 1.0]));

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let p = img.pixels().next().unwrap();
    // Mid-gray under sRGB encoding has L ≈ 0.735 → closer to highlights
    // (blue) than shadows (red).
    assert!(
        p[2] > p[0],
        "expected blue dominant after toning, got {p:?}"
    );
}

#[test]
fn blend_one_equals_pass_through() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    // Blend=1.0 should be identity (output == input, modulo sRGB round-trip
    // tolerance).
    let (mut project, comp_id, _) = solid_layer_project([0.4, 0.6, 0.8, 1.0]);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.layers[0].effects[0]
        .values
        .set("blend", ParamValue::Float(1.0));
    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let p = img.pixels().next().unwrap();
    let want = [
        (0.4_f32 * 255.0).round() as u8,
        (0.6_f32 * 255.0).round() as u8,
        (0.8_f32 * 255.0).round() as u8,
    ];
    for c in 0..3 {
        assert!(
            p[c].abs_diff(want[c]) <= 3,
            "blend=1 should pass through; channel {c} got {} expected {}",
            p[c],
            want[c]
        );
    }
}

#[test]
fn solid_mode_replaces_input_with_midtones() {
    let Some(renderer) = try_renderer() else {
        return;
    };
    // Solid mode ignores luminance — every pixel becomes midtones. We pick
    // values that are sRGB-encoding-stable (0 and 1 round-trip exactly) so
    // the test isn't sensitive to the encode/decode wrap legs.
    let (mut project, comp_id, _) = solid_layer_project([0.5, 0.5, 0.5, 1.0]);
    let comp = project.composition_mut(comp_id).unwrap();
    let eff = &mut comp.layers[0].effects[0];
    eff.values.set("tones", ParamValue::Enum("solid".into()));
    eff.values
        .set("midtones", ParamValue::Color([1.0, 0.0, 1.0, 1.0]));

    let mut comp_runtime = Compositor::new(renderer);
    let tex = comp_runtime.render(&project, comp_id, 0).unwrap();
    let img = download_image(comp_runtime.renderer(), &tex);
    let p = img.pixels().next().unwrap();
    assert!(p[0] >= 250, "expected R near 255, got {}", p[0]);
    assert!(p[1] <= 8, "expected G near 0, got {}", p[1]);
    assert!(p[2] >= 250, "expected B near 255, got {}", p[2]);
}
