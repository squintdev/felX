// Gain — multiplies RGB by a scalar. Alpha pass-through.

struct Params {
    gain: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
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
    // Fullscreen triangle: vertices (-1,-1), (3,-1), (-1,3) → covers the
    // viewport with one triangle, no vertex buffer needed.
    let x = f32((idx & 1u) << 2u) - 1.0;
    let y = f32((idx & 2u) << 1u) - 1.0;
    var out: VsOut;
    out.clip = vec4(x, y, 0.0, 1.0);
    // Map clip-space [-1, 1] → texture UV [0, 1] with Y flipped (textures
    // are top-left origin; clip is bottom-left).
    out.uv = vec2((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(input_tex, input_smp, in.uv);
    return vec4(s.rgb * params.gain, s.a);
}
