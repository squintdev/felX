// CC Toner — multi-tone color mapping based on Rec.601 luminance.
//
// Designed to run in sRGB-encoded space (the compositor wraps this pass
// with felx_render::srgb_wrap). The luminance, segment lookup, and
// per-channel lerp all happen on the encoded values, which matches the
// recognizable "CC Toner look".

struct Params {
    stops: array<vec4<f32>, 5>,
    n_stops: u32,
    blend: f32,
    _pad0: f32,
    _pad1: f32,
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

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(input_tex, input_smp, in.uv);
    let alpha = s.a;
    let inv_a = select(1.0 / max(alpha, 1e-6), 1.0, alpha == 0.0);
    let rgb = s.rgb * inv_a;

    // Rec.601 luminance. Inferred from the original CC Toner's vintage; see
    // effects.md notes.
    let l = clamp(dot(rgb, vec3(0.299, 0.587, 0.114)), 0.0, 1.0);

    var toned: vec3<f32>;
    if params.n_stops <= 1u {
        toned = params.stops[0].rgb;
    } else {
        let scaled = l * f32(params.n_stops - 1u);
        let idx = u32(floor(scaled));
        let next_idx = min(idx + 1u, params.n_stops - 1u);
        let frac = scaled - f32(idx);
        let a = params.stops[idx].rgb;
        let b = params.stops[next_idx].rgb;
        toned = mix(a, b, frac);
    }

    let blended = mix(toned, rgb, params.blend);
    return vec4(blended * alpha, alpha);
}
