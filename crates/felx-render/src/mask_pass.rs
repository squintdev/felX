//! Mask rasterization (F-062) and application to layer alpha.
//!
//! v1 path is CPU-side: tessellate each cubic-bezier segment to a polyline,
//! fill the polygon via scanline + even-odd rule into an `Rgba8Unorm`
//! single-channel mask, optionally box-blur for [`Mask::feather`], and
//! multiply against the layer texture's alpha via a tiny WGSL pass.
//!
//! This is plenty fast at preview resolutions; a GPU SDF rasterizer is a
//! perf follow-up if real-time mask scrubbing on 4K plates becomes a thing.

use crate::Renderer;
use crate::texture_io::{COMPOSITOR_FORMAT, upload_image};
use felx_core::model::{Mask, MaskMode, MaskPath};
use image::{ImageBuffer, Rgba, RgbaImage};

const TESS_STEPS: usize = 24;

/// Rasterize one path into an `RgbaImage` whose RGB channels are 0 and
/// whose alpha encodes the mask (255 inside, 0 outside, intermediate at
/// the feathered border).
pub fn rasterize_path(path: &MaskPath, w: u32, h: u32) -> RgbaImage {
    let mut img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba([0, 0, 0, 0]));
    if path.vertices.len() < 3 {
        return img;
    }
    let polyline = tessellate(path);
    fill_polygon_even_odd(&mut img, &polyline);
    img
}

/// Build the full single-channel mask for a layer at (w, h): combine every
/// mask via its [`MaskMode`], apply per-mask opacity, expansion, and
/// feather. Returns RGBA where alpha is the result.
pub fn rasterize_masks(
    masks: &[Mask],
    w: u32,
    h: u32,
    time: felx_core::model::Rational,
) -> RgbaImage {
    // Start fully opaque so a mask with no entries doesn't gate anything.
    // (Caller decides whether to skip the multiply pass entirely if the
    // layer has no masks.)
    let mut accum: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba([0, 0, 0, 255]));

    for (i, mask) in masks.iter().enumerate() {
        let path = mask.path.sample_at_time(time);
        let mut layer_mask = rasterize_path(&path, w, h);
        if mask.expansion != 0.0 {
            apply_expansion(&mut layer_mask, mask.expansion);
        }
        if mask.feather > 0.0 {
            box_blur_alpha(&mut layer_mask, mask.feather);
        }
        if (mask.opacity - 1.0).abs() > f32::EPSILON {
            for p in layer_mask.pixels_mut() {
                let a = p[3] as f32 / 255.0;
                p[3] = (a * mask.opacity * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        // Combine into accumulator.
        if i == 0 && matches!(mask.mode, MaskMode::Add) {
            // First mask is the basis: replace the all-opaque accumulator
            // with this mask. (Add-on-top-of-opaque would be no-op.)
            accum = layer_mask;
        } else {
            combine_alpha(&mut accum, &layer_mask, mask.mode);
        }
    }
    accum
}

fn tessellate(path: &MaskPath) -> Vec<[f32; 2]> {
    let n = path.vertices.len();
    let mut points = Vec::with_capacity(n * TESS_STEPS);
    for i in 0..n {
        let a = &path.vertices[i];
        let b = &path.vertices[(i + 1) % n];
        let p0 = a.anchor;
        let p1 = [a.anchor[0] + a.out_tan[0], a.anchor[1] + a.out_tan[1]];
        let p2 = [b.anchor[0] + b.in_tan[0], b.anchor[1] + b.in_tan[1]];
        let p3 = b.anchor;
        // For a corner-to-corner segment with zero tangents, fall back to a
        // straight line — no need to oversample.
        let is_line = a.out_tan == [0.0, 0.0] && b.in_tan == [0.0, 0.0];
        let steps = if is_line { 1 } else { TESS_STEPS };
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            points.push(cubic_bezier(p0, p1, p2, p3, t));
        }
    }
    points
}

fn cubic_bezier(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let b0 = u * u * u;
    let b1 = 3.0 * u * u * t;
    let b2 = 3.0 * u * t * t;
    let b3 = t * t * t;
    [
        b0 * p0[0] + b1 * p1[0] + b2 * p2[0] + b3 * p3[0],
        b0 * p0[1] + b1 * p1[1] + b2 * p2[1] + b3 * p3[1],
    ]
}

/// Scanline polygon fill, even-odd rule. Writes alpha=255 inside the
/// polygon, alpha=0 outside.
fn fill_polygon_even_odd(img: &mut RgbaImage, polyline: &[[f32; 2]]) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let n = polyline.len();
    if n < 3 {
        return;
    }
    for y in 0..h {
        let yf = y as f32 + 0.5;
        let mut crossings: Vec<f32> = Vec::with_capacity(n);
        for i in 0..n {
            let a = polyline[i];
            let b = polyline[(i + 1) % n];
            let (y1, y2) = (a[1], b[1]);
            // Standard "intersect at y if one endpoint is below and the other
            // is above" rule, with the half-open [y1, y2) trick to avoid
            // double-counting horizontal edges at vertices.
            let crosses = (y1 <= yf && yf < y2) || (y2 <= yf && yf < y1);
            if crosses {
                let t = (yf - y1) / (y2 - y1);
                let x = a[0] + t * (b[0] - a[0]);
                crossings.push(x);
            }
        }
        crossings.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut i = 0;
        while i + 1 < crossings.len() {
            let x_lo = crossings[i].ceil() as i32;
            let x_hi = crossings[i + 1].floor() as i32;
            for x in x_lo.max(0)..=(x_hi.min(w - 1)) {
                img.put_pixel(x as u32, y as u32, Rgba([0, 0, 0, 255]));
            }
            i += 2;
        }
    }
}

/// Approximate expansion by a per-pixel max (grow) or min (shrink) over a
/// radius equal to |expansion|. Sufficient for axis-aligned simple shapes;
/// a true SDF would do better at sharp corners.
fn apply_expansion(img: &mut RgbaImage, expansion: f32) {
    let r = expansion.abs().round() as i32;
    if r == 0 {
        return;
    }
    let grow = expansion > 0.0;
    let w = img.width() as i32;
    let h = img.height() as i32;
    let src = img.clone();
    for y in 0..h {
        for x in 0..w {
            let mut acc = if grow { 0u8 } else { 255u8 };
            for dy in -r..=r {
                for dx in -r..=r {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || nx >= w || ny < 0 || ny >= h {
                        continue;
                    }
                    let s = src.get_pixel(nx as u32, ny as u32)[3];
                    if grow {
                        acc = acc.max(s);
                    } else {
                        acc = acc.min(s);
                    }
                }
            }
            let mut p = *img.get_pixel(x as u32, y as u32);
            p[3] = acc;
            img.put_pixel(x as u32, y as u32, p);
        }
    }
}

/// Box-blur the alpha channel, n iterations approximate a Gaussian.
/// Radius derived from feather pixels (rounded). Keeps RGB at 0.
fn box_blur_alpha(img: &mut RgbaImage, feather: f32) {
    let r = feather.round().max(1.0) as i32;
    let iters = 3;
    let w = img.width() as i32;
    let h = img.height() as i32;
    for _ in 0..iters {
        // Horizontal pass.
        let src = img.clone();
        for y in 0..h {
            for x in 0..w {
                let mut sum: i32 = 0;
                let mut cnt: i32 = 0;
                for dx in -r..=r {
                    let nx = x + dx;
                    if nx < 0 || nx >= w {
                        continue;
                    }
                    sum += src.get_pixel(nx as u32, y as u32)[3] as i32;
                    cnt += 1;
                }
                let avg = if cnt > 0 { (sum / cnt) as u8 } else { 0 };
                let mut p = *img.get_pixel(x as u32, y as u32);
                p[3] = avg;
                img.put_pixel(x as u32, y as u32, p);
            }
        }
        // Vertical pass.
        let src = img.clone();
        for y in 0..h {
            for x in 0..w {
                let mut sum: i32 = 0;
                let mut cnt: i32 = 0;
                for dy in -r..=r {
                    let ny = y + dy;
                    if ny < 0 || ny >= h {
                        continue;
                    }
                    sum += src.get_pixel(x as u32, ny as u32)[3] as i32;
                    cnt += 1;
                }
                let avg = if cnt > 0 { (sum / cnt) as u8 } else { 0 };
                let mut p = *img.get_pixel(x as u32, y as u32);
                p[3] = avg;
                img.put_pixel(x as u32, y as u32, p);
            }
        }
    }
}

fn combine_alpha(dst: &mut RgbaImage, src: &RgbaImage, mode: MaskMode) {
    for (d, s) in dst.pixels_mut().zip(src.pixels()) {
        let a_d = d[3] as f32 / 255.0;
        let a_s = s[3] as f32 / 255.0;
        let a = match mode {
            MaskMode::Add => (a_d + a_s).min(1.0),
            MaskMode::Subtract => (a_d - a_s).max(0.0),
            MaskMode::Intersect => a_d * a_s,
            MaskMode::Difference => (a_d - a_s).abs(),
        };
        d[3] = (a * 255.0).round() as u8;
    }
}

/// GPU pass: multiply input.rgba by mask.a (alpha).
pub struct MaskApply {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

impl MaskApply {
    pub fn new(renderer: &Renderer, format: wgpu::TextureFormat) -> Self {
        let shader = renderer
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("mask_apply.wgsl"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
        let sampler = renderer.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mask_apply.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = renderer
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mask_apply.bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout =
            renderer
                .device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("mask_apply.pl"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });
        let pipeline = renderer
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mask_apply.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        Self {
            pipeline,
            sampler,
            layout,
        }
    }

    pub fn apply(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        input_view: &wgpu::TextureView,
        mask_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
    ) {
        let bg = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mask_apply.bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(mask_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mask_apply.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var mask_tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

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
    let src = textureSample(input_tex, smp, in.uv);
    let m = textureSample(mask_tex, smp, in.uv).a;
    return vec4(src.rgb * m, src.a * m);
}
"#;

/// Convenience: rasterize masks → upload to a GPU texture suitable for
/// [`MaskApply::apply`].
pub fn upload_mask(renderer: &Renderer, mask: &RgbaImage) -> wgpu::Texture {
    upload_image(renderer, mask)
}

/// Format used for the per-layer mask texture. RGBA so we can reuse
/// `upload_image` and `MaskApply`'s bind layout. Only the alpha matters.
pub const MASK_FORMAT: wgpu::TextureFormat = COMPOSITOR_FORMAT;

#[cfg(test)]
mod tests {
    use super::*;
    use felx_core::model::{MaskPath, MaskVertex};

    #[test]
    fn rectangle_path_fills_interior() {
        let path = MaskPath::rectangle(2.0, 2.0, 6.0, 6.0);
        let img = rasterize_path(&path, 10, 10);
        // Center should be inside (alpha 255).
        assert_eq!(img.get_pixel(5, 5)[3], 255);
        // Outside corner should be 0.
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        assert_eq!(img.get_pixel(9, 9)[3], 0);
    }

    #[test]
    fn empty_path_does_not_panic() {
        let path = MaskPath {
            vertices: vec![MaskVertex::corner(0.0, 0.0)],
        };
        let img = rasterize_path(&path, 4, 4);
        assert!(img.pixels().all(|p| p[3] == 0));
    }

    #[test]
    fn two_adds_combine_to_union() {
        let masks = vec![
            Mask::rectangle("a", 0.0, 0.0, 5.0, 10.0),
            Mask::rectangle("b", 5.0, 0.0, 5.0, 10.0),
        ];
        let img = rasterize_masks(&masks, 10, 10, felx_core::model::Rational::new(0, 30));
        // Both halves should be opaque.
        assert_eq!(img.get_pixel(2, 5)[3], 255);
        assert_eq!(img.get_pixel(7, 5)[3], 255);
    }

    #[test]
    fn intersect_keeps_only_overlap() {
        let mut masks = vec![
            Mask::rectangle("a", 0.0, 0.0, 6.0, 10.0),
            Mask::rectangle("b", 4.0, 0.0, 6.0, 10.0),
        ];
        masks[1].mode = MaskMode::Intersect;
        let img = rasterize_masks(&masks, 10, 10, felx_core::model::Rational::new(0, 30));
        // Overlap (x=4..=5) opaque; outside (x=0, x=9) transparent.
        assert_eq!(img.get_pixel(5, 5)[3], 255);
        assert_eq!(img.get_pixel(0, 5)[3], 0);
        assert_eq!(img.get_pixel(9, 5)[3], 0);
    }

    #[test]
    fn feather_softens_edge() {
        // 16x16 canvas, 12x12 rectangle, feather=1 — center stays opaque
        // and the blur still spreads alpha into surrounding pixels.
        let mut m = Mask::rectangle("a", 2.0, 2.0, 12.0, 12.0);
        m.feather = 1.0;
        let img = rasterize_masks(&[m], 16, 16, felx_core::model::Rational::new(0, 30));
        let center = img.get_pixel(8, 8)[3];
        assert!(center >= 200, "interior alpha should stay high: {center}");
        let edge = img.get_pixel(1, 8)[3];
        assert!(edge > 0 && edge < 255, "expected feathered edge: {edge}");
    }
}
