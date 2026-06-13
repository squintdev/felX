// CRT — combined display simulation. Each sub-feature is independently
// scalable so users can dial in a 1990s consumer-Trinitron, 80s arcade
// monitor, or 90s PC-monitor look without reaching for 7 separate effects.
//
// Atomic chainable passes (per ADR-0002) come as F-073…F-079a; this v1
// covers the recognizable artifact set in one shader pass:
// - Screen curvature (barrel distortion)
// - Scanlines (per-row dimming)
// - Shadow mask (RGB phosphor sub-pattern)
// - RGB convergence (per-channel UV offset, growing toward corners)
// - Vignette (corner darkening)
// - Edge fade to black past the curved screen boundary
//
// Phosphor persistence and full bloom are deferred (both need multi-frame
// state or multi-pass internals; M3 ships chainable single-pass v1).

struct Params {
    curvature: vec2<f32>,
    scanline_intensity: f32,
    scanline_thickness: f32,
    mask_intensity: f32,
    mask_size: f32,        // pixels per phosphor cell
    mask_type: u32,        // 0 = dot trio, 1 = aperture grille, 2 = slot mask
    convergence_radial: f32,
    vignette_intensity: f32,
    vignette_softness: f32,
    src_size: vec2<f32>,
    _pad0: vec2<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_smp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    let x = f32((idx & 1u) << 2u) - 1.0;
    let y = f32((idx & 2u) << 1u) - 1.0;
    var out: VsOut;
    out.clip = vec4(x, y, 0.0, 1.0);
    out.uv = vec2((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

fn curve_uv(uv: vec2<f32>, curvature: vec2<f32>) -> vec2<f32> {
    // Standard `uv' = uv + curvature * uv * (uv·uv - 1)` model. Centered
    // around (0, 0) in NDC-like space → re-center.
    let p = uv * 2.0 - 1.0;
    let pp = p * p;
    let dx = curvature.x * p.x * (pp.y - 1.0);
    let dy = curvature.y * p.y * (pp.x - 1.0);
    let curved = vec2(p.x + dx, p.y + dy);
    return curved * 0.5 + 0.5;
}

fn shadow_mask_factor(p: vec2<f32>, mask_type: u32, size: f32, intensity: f32) -> vec3<f32> {
    let cell = floor(p / max(size, 1.0));
    let local = (p / max(size, 1.0)) - cell;
    var rgb_factor = vec3(1.0, 1.0, 1.0);
    if mask_type == 1u {
        // Aperture grille: vertical RGB stripes.
        let phase = i32(floor(local.x * 3.0)) % 3;
        if phase == 0 { rgb_factor = vec3(1.0, 0.5, 0.5); }
        else if phase == 1 { rgb_factor = vec3(0.5, 1.0, 0.5); }
        else { rgb_factor = vec3(0.5, 0.5, 1.0); }
    } else if mask_type == 2u {
        // Slot mask: vertical stripes broken into staggered slots.
        let row_offset = f32(i32(cell.y) % 2) * 0.5;
        let phase = i32(floor((local.x + row_offset) * 3.0)) % 3;
        if phase == 0 { rgb_factor = vec3(1.0, 0.5, 0.5); }
        else if phase == 1 { rgb_factor = vec3(0.5, 1.0, 0.5); }
        else { rgb_factor = vec3(0.5, 0.5, 1.0); }
    } else {
        // Dot trio: 3-cell horizontal pattern with row-offset every other row.
        let row_offset = f32(i32(cell.y) % 2) * 0.5;
        let phase = i32(floor((local.x + row_offset) * 3.0)) % 3;
        if phase == 0 { rgb_factor = vec3(1.0, 0.4, 0.4); }
        else if phase == 1 { rgb_factor = vec3(0.4, 1.0, 0.4); }
        else { rgb_factor = vec3(0.4, 0.4, 1.0); }
    }
    return mix(vec3(1.0), rgb_factor, intensity);
}

fn scanline_factor(uv: vec2<f32>, intensity: f32, thickness: f32) -> f32 {
    if intensity <= 0.0 {
        return 1.0;
    }
    let line = sin(uv.y * params.src_size.y * 3.14159);
    let band = pow(abs(line), max(thickness, 0.05));
    return mix(1.0, band, intensity);
}

fn vignette_factor(uv: vec2<f32>, intensity: f32, softness: f32) -> f32 {
    if intensity <= 0.0 {
        return 1.0;
    }
    let p = uv * 2.0 - 1.0;
    let r = length(p);
    let edge = smoothstep(1.0 - max(softness, 0.01), 1.0, r);
    return 1.0 - intensity * edge;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let uv = curve_uv(in.uv, params.curvature);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }

    // RGB convergence: per-channel offset that grows with corner distance.
    let center = uv - 0.5;
    let dist = length(center);
    let conv_amount = params.convergence_radial * dist;
    let r_off = vec2(conv_amount, 0.0) / params.src_size;
    let b_off = vec2(-conv_amount, 0.0) / params.src_size;
    let r_sample = textureSample(input_tex, input_smp, uv + r_off);
    let g_sample = textureSample(input_tex, input_smp, uv);
    let b_sample = textureSample(input_tex, input_smp, uv - b_off);
    let a = g_sample.a;
    let inv_a = select(1.0 / max(a, 1e-6), 1.0, a == 0.0);
    var rgb = vec3(
        r_sample.r * inv_a,
        g_sample.g * inv_a,
        b_sample.b * inv_a,
    );

    // Shadow mask (in pixel space).
    let mask = shadow_mask_factor(uv * params.src_size, params.mask_type,
                                  params.mask_size, params.mask_intensity);
    rgb = rgb * mask;

    // Scanlines.
    rgb = rgb * scanline_factor(uv, params.scanline_intensity, params.scanline_thickness);

    // Vignette.
    rgb = rgb * vignette_factor(uv, params.vignette_intensity, params.vignette_softness);

    rgb = clamp(rgb, vec3(0.0), vec3(1.5));
    return vec4(rgb * a, a);
}
