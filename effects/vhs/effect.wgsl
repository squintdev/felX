// VHS — combined physical-tape artifacts. v1 ships as one effect with
// toggleable sub-features. F-080…F-084 split into atomic chainable passes
// in a follow-up.
//
// Sub-features:
// - Sync wobble: per-scanline horizontal jitter from 1D hash noise.
// - Dropouts: pseudo-random horizontal streaks (white-on-black or
//   black-on-white) at a Poisson-ish density.
// - Tape damage: large-scale Perlin-ish noise multiplier on luma.
// - Transport wobble: slow sinusoidal LFO on brightness + chroma phase.
// - Vertical scroll: a Y-offset the user keyframes for picture-roll bursts.
//
// The signal is converted to YIQ for the chroma-phase rotation, then back to
// RGB for output.

struct Params {
    sync_wobble: f32,
    dropouts_density: f32,
    dropouts_polarity: f32, // 0 = mostly black, 1 = mostly white, 0.5 mixed
    tape_damage: f32,
    transport_brightness: f32,
    transport_chroma_phase: f32,
    transport_freq: f32,
    vertical_scroll: f32,   // 0..1 normalized; wraps
    seed: f32,
    time_seconds: f32,
    src_size: vec2<f32>,
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

fn hash11(x: f32, seed: f32) -> f32 {
    return fract(sin(x * 12.9898 + seed * 78.233) * 43758.5453);
}

fn hash21(p: vec2<f32>, seed: f32) -> f32 {
    return fract(sin(dot(vec3(p.x, p.y, seed), vec3(127.1, 311.7, 74.7))) * 43758.5453);
}

fn rgb_to_yiq(c: vec3<f32>) -> vec3<f32> {
    return vec3(
        0.299 * c.r + 0.587 * c.g + 0.114 * c.b,
        0.596 * c.r - 0.274 * c.g - 0.322 * c.b,
        0.211 * c.r - 0.523 * c.g + 0.312 * c.b,
    );
}

fn yiq_to_rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3(
        c.x + 0.956 * c.y + 0.621 * c.z,
        c.x - 0.272 * c.y - 0.647 * c.z,
        c.x - 1.106 * c.y + 1.703 * c.z,
    );
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var uv = in.uv;

    // Vertical scroll wraps Y.
    uv.y = fract(uv.y + params.vertical_scroll);

    // Sync wobble: per-row horizontal displacement.
    if params.sync_wobble > 0.0 {
        let n = hash11(floor(uv.y * params.src_size.y) + params.seed, params.seed) - 0.5;
        uv.x = uv.x + n * params.sync_wobble * 2.0 / params.src_size.x;
    }

    var s = textureSample(input_tex, input_smp, uv);
    let alpha = s.a;
    let inv_a = select(1.0 / max(alpha, 1e-6), 1.0, alpha == 0.0);
    var rgb = s.rgb * inv_a;
    var yiq = rgb_to_yiq(rgb);

    // Tape damage: low-freq noise on luma (Perlin-ish via hash interpolation).
    if params.tape_damage > 0.0 {
        let big = uv * 6.0;
        let n_lo = hash21(floor(big), params.seed);
        let n_hi = hash21(floor(big) + vec2(1.0, 1.0), params.seed);
        let damage = mix(n_lo, n_hi, fract(big.x) * fract(big.y));
        yiq.x = yiq.x * (1.0 - params.tape_damage * (1.0 - damage));
    }

    // Transport wobble: slow LFO on brightness + chroma phase.
    if params.transport_brightness > 0.0 || params.transport_chroma_phase > 0.0 {
        let lfo = sin(params.time_seconds * params.transport_freq * 6.2831853);
        yiq.x = yiq.x + lfo * params.transport_brightness * 0.1;
        let phase = lfo * params.transport_chroma_phase * 0.523598; // up to ±30°
        let cs = cos(phase);
        let sn = sin(phase);
        let i_new = cs * yiq.y - sn * yiq.z;
        let q_new = sn * yiq.y + cs * yiq.z;
        yiq = vec3(yiq.x, i_new, q_new);
    }

    rgb = yiq_to_rgb(yiq);

    // Dropouts: per-row, occasional horizontal streak. Polarity controls
    // whether streaks are bright or dark.
    if params.dropouts_density > 0.0 {
        let row = floor(uv.y * params.src_size.y);
        let row_hash = hash11(row + params.seed * 13.0, params.seed);
        if row_hash < params.dropouts_density * 0.05 {
            let start_h = hash11(row + params.seed * 17.0, params.seed);
            let end_h = hash11(row + params.seed * 19.0, params.seed);
            let start = min(start_h, end_h);
            let end = max(start_h, end_h);
            if uv.x >= start && uv.x <= end {
                let v = step(0.5, params.dropouts_polarity);
                rgb = vec3(v, v, v);
            }
        }
    }

    rgb = clamp(rgb, vec3(0.0), vec3(1.0));
    return vec4(rgb * alpha, alpha);
}
