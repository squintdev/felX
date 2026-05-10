// Signal — analog video signal emulation. v1 implementation: chroma blur,
// ringing, snow, head-switching band. The full ntsc-rs adapter (CPU-pass
// over the ntscrs crate) is planned as a follow-up that adds the rest of
// the artifact set (head switching mid-line jitter, tracking-noise band
// with snow concentrate, FBM noise floors, chroma phase error/delay,
// VHS sub-effect with tape-speed-driven bandwidth, edge wave). For now
// this is enough to give the recognizable "old TV" look.

struct Params {
    chroma_blur: f32,        // 0..1 — fraction of chroma sub-sampling
    ringing_intensity: f32,  // 0..1 — sharpening overshoot
    snow_intensity: f32,     // 0..1 — speckle noise on luma
    composite_noise: f32,    // 0..1 — band-limited noise on luma
    head_switch_height: f32, // pixels at the bottom of frame
    head_switch_shift: f32,  // pixels of horizontal jitter in the band
    seed: f32,
    _pad0: f32,
    src_size: vec2<f32>,
    _pad1: vec2<f32>,
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

fn rgb_to_yiq(rgb: vec3<f32>) -> vec3<f32> {
    return vec3(
        0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b,
        0.596 * rgb.r - 0.274 * rgb.g - 0.322 * rgb.b,
        0.211 * rgb.r - 0.523 * rgb.g + 0.312 * rgb.b,
    );
}

fn yiq_to_rgb(yiq: vec3<f32>) -> vec3<f32> {
    return vec3(
        yiq.x + 0.956 * yiq.y + 0.621 * yiq.z,
        yiq.x - 0.272 * yiq.y - 0.647 * yiq.z,
        yiq.x - 1.106 * yiq.y + 1.703 * yiq.z,
    );
}

// Hash-based pseudo-random in [0, 1). Deterministic given seed + coords.
fn hash21(p: vec2<f32>, seed: f32) -> f32 {
    let q = vec3(p.x, p.y, seed);
    let h = sin(dot(q, vec3(127.1, 311.7, 74.7))) * 43758.5453;
    return fract(h);
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let texel = vec2(1.0 / params.src_size.x, 1.0 / params.src_size.y);

    // Possibly displace this row if it's inside the head-switch band.
    var sample_uv = in.uv;
    let band_norm = params.head_switch_height / params.src_size.y;
    if band_norm > 0.0 && in.uv.y > 1.0 - band_norm {
        let band_pos = (in.uv.y - (1.0 - band_norm)) / band_norm;
        let drift = hash21(vec2(in.uv.y * 47.0, params.seed), params.seed) - 0.5;
        let shift = drift * params.head_switch_shift * (1.0 - band_pos);
        sample_uv.x = sample_uv.x + shift / params.src_size.x;
    }

    let s = textureSample(input_tex, input_smp, sample_uv);
    let alpha = s.a;
    let rgb = s.rgb / max(alpha, 1e-6);

    // Chroma blur: average chroma from a 5-tap horizontal kernel scaled by
    // chroma_blur. At 0, no smoothing; at 1, ~5-pixel chroma low-pass.
    var i_avg: f32 = 0.0;
    var q_avg: f32 = 0.0;
    let radius = i32(round(params.chroma_blur * 4.0));
    let n = 2 * radius + 1;
    for (var k: i32 = -radius; k <= radius; k = k + 1) {
        let off = vec2(f32(k), 0.0) * texel;
        let s2 = textureSample(input_tex, input_smp, sample_uv + off);
        let yiq = rgb_to_yiq(s2.rgb / max(s2.a, 1e-6));
        i_avg = i_avg + yiq.y;
        q_avg = q_avg + yiq.z;
    }
    i_avg = i_avg / f32(n);
    q_avg = q_avg / f32(n);

    let center_yiq = rgb_to_yiq(rgb);
    let smoothed = vec3(center_yiq.x, i_avg, q_avg);

    // Ringing: a horizontal high-pass on luma adds overshoot at sharp
    // transitions. Subtracting the next-pixel luma and adding back a
    // scaled portion gives a sharpened look.
    let next = textureSample(input_tex, input_smp, sample_uv + vec2(texel.x, 0.0));
    let prev = textureSample(input_tex, input_smp, sample_uv - vec2(texel.x, 0.0));
    let next_y = rgb_to_yiq(next.rgb / max(next.a, 1e-6)).x;
    let prev_y = rgb_to_yiq(prev.rgb / max(prev.a, 1e-6)).x;
    let high_pass = smoothed.x - 0.5 * (next_y + prev_y);
    let sharpened_y = smoothed.x + params.ringing_intensity * high_pass;

    var final_yiq = vec3(sharpened_y, smoothed.y, smoothed.z);

    // Composite noise: low-amplitude noise on luma.
    if params.composite_noise > 0.0 {
        let n_l = hash21(in.uv * params.src_size + vec2(params.seed, 0.0), params.seed);
        final_yiq.x = final_yiq.x + (n_l - 0.5) * params.composite_noise * 0.4;
    }

    // Snow: speckle replacing luma at high intensity.
    if params.snow_intensity > 0.0 {
        let n_s = hash21(in.uv * params.src_size * 7.13 + vec2(0.0, params.seed), params.seed);
        if n_s > 1.0 - params.snow_intensity * 0.05 {
            final_yiq.x = 1.0;
        }
    }

    let out_rgb = clamp(yiq_to_rgb(final_yiq), vec3(0.0), vec3(1.0));
    return vec4(out_rgb * alpha, alpha);
}
