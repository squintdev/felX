// CRT Phosphor Persistence (F-079).
// Inputs: current-frame texture and the previous frame's output (state.read).
// Output: max(current, decay * previous_tinted) — phosphor decay style.

struct Params {
    decay: f32,
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
};

@group(0) @binding(0) var current_tex: texture_2d<f32>;
@group(0) @binding(1) var prev_tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    let x = f32((idx & 1u) << 2u) - 1.0;
    let y = f32((idx & 2u) << 1u) - 1.0;
    var o: VsOut;
    o.clip = vec4(x, y, 0.0, 1.0);
    o.uv = vec2((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let cur = textureSample(current_tex, smp, in.uv);
    let prev = textureSample(prev_tex, smp, in.uv);
    let tint = vec3<f32>(params.tint_r, params.tint_g, params.tint_b);
    let decayed = prev.rgb * params.decay * tint;
    let out_rgb = max(cur.rgb, decayed);
    return vec4(out_rgb, max(cur.a, prev.a * params.decay));
}
