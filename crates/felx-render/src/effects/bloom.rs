//! Bloom (F-076).
//!
//! Three GPU pipelines orchestrated per frame:
//! 1. **Threshold**: extract pixels above a soft-kneed luminance cutoff.
//! 2. **Separable Gaussian blur**: 9-tap horizontal then vertical, at the
//!    full preview resolution. (The proper multi-level downsample chain
//!    that AE / Unreal use is a perf follow-up — not visually critical
//!    at preview resolutions.)
//! 3. **Additive composite**: original + intensity × blurred-bright.

use crate::Renderer;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ThresholdParams {
    pub threshold: f32,
    pub soft_knee: f32,
    pub _pad0: f32,
    pub _pad1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BlurParams {
    pub direction_x: f32,
    pub direction_y: f32,
    pub radius: f32,
    pub _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CompositeParams {
    pub intensity: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

const VS_AND_THRESHOLD: &str = r#"
struct ThresholdParams { threshold: f32, soft_knee: f32, _pad0: f32, _pad1: f32 };
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;
@group(0) @binding(2) var<uniform> p: ThresholdParams;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

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
    let c = textureSample(src_tex, src_smp, in.uv);
    let lum = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    let knee = max(p.soft_knee, 1e-4);
    let lo = p.threshold - knee;
    let hi = p.threshold + knee;
    let weight = smoothstep(lo, hi, lum);
    return vec4(c.rgb * weight, c.a);
}
"#;

const VS_AND_BLUR: &str = r#"
struct BlurParams { direction_x: f32, direction_y: f32, radius: f32, _pad: f32 };
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;
@group(0) @binding(2) var<uniform> p: BlurParams;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

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
    let dims = vec2<f32>(textureDimensions(src_tex));
    let texel = vec2(1.0 / dims.x, 1.0 / dims.y);
    let dir = vec2(p.direction_x, p.direction_y) * texel;
    var col = textureSample(src_tex, src_smp, in.uv) * 0.227027;
    let weights = array<f32, 4>(0.194594, 0.121622, 0.054054, 0.016216);
    for (var i = 0; i < 4; i = i + 1) {
        let off = dir * f32(i + 1) * p.radius;
        col = col + textureSample(src_tex, src_smp, in.uv + off) * weights[i];
        col = col + textureSample(src_tex, src_smp, in.uv - off) * weights[i];
    }
    return col;
}
"#;

const VS_AND_COMPOSITE: &str = r#"
struct CompositeParams { intensity: f32, _pad0: f32, _pad1: f32, _pad2: f32 };
@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var base_smp: sampler;
@group(0) @binding(2) var bloom_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> p: CompositeParams;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

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
    let base = textureSample(base_tex, base_smp, in.uv);
    let bloom = textureSample(bloom_tex, base_smp, in.uv);
    return vec4(base.rgb + bloom.rgb * p.intensity, base.a);
}
"#;

pub struct Bloom {
    threshold_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    sample_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
}

impl Bloom {
    pub fn new(renderer: &Renderer, format: wgpu::TextureFormat) -> Self {
        let device = renderer.device();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Bind layout used by both threshold and blur (texture + sampler + uniform).
        let sample_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom.sample.bgl"),
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
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom.composite.bgl"),
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
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let threshold_pipeline = make_pipeline(
            renderer,
            format,
            "bloom.threshold",
            VS_AND_THRESHOLD,
            &sample_layout,
        );
        let blur_pipeline =
            make_pipeline(renderer, format, "bloom.blur", VS_AND_BLUR, &sample_layout);
        let composite_pipeline = make_pipeline(
            renderer,
            format,
            "bloom.composite",
            VS_AND_COMPOSITE,
            &composite_layout,
        );

        Self {
            threshold_pipeline,
            blur_pipeline,
            composite_pipeline,
            sampler,
            sample_layout,
            composite_layout,
        }
    }

    pub fn render_threshold(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        input_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
        params: ThresholdParams,
    ) {
        let buf = make_uniform(
            renderer,
            "bloom.threshold.uniform",
            bytemuck::bytes_of(&params),
        );
        let bg = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom.threshold.bg"),
                layout: &self.sample_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buf.as_entire_binding(),
                    },
                ],
            });
        run_pass(
            encoder,
            "bloom.threshold.pass",
            &self.threshold_pipeline,
            &bg,
            out_view,
        );
    }

    pub fn render_blur(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        input_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
        params: BlurParams,
    ) {
        let buf = make_uniform(renderer, "bloom.blur.uniform", bytemuck::bytes_of(&params));
        let bg = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom.blur.bg"),
                layout: &self.sample_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buf.as_entire_binding(),
                    },
                ],
            });
        run_pass(
            encoder,
            "bloom.blur.pass",
            &self.blur_pipeline,
            &bg,
            out_view,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_composite(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        base_view: &wgpu::TextureView,
        bloom_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
        params: CompositeParams,
    ) {
        let buf = make_uniform(
            renderer,
            "bloom.composite.uniform",
            bytemuck::bytes_of(&params),
        );
        let bg = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom.composite.bg"),
                layout: &self.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(base_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(bloom_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buf.as_entire_binding(),
                    },
                ],
            });
        run_pass(
            encoder,
            "bloom.composite.pass",
            &self.composite_pipeline,
            &bg,
            out_view,
        );
    }
}

fn make_pipeline(
    renderer: &Renderer,
    format: wgpu::TextureFormat,
    label: &str,
    wgsl: &str,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let device = renderer.device();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
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
    })
}

fn make_uniform(renderer: &Renderer, label: &str, data: &[u8]) -> wgpu::Buffer {
    let buf = renderer.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: data.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    renderer.queue().write_buffer(&buf, 0, data);
    buf
}

fn run_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    bg: &wgpu::BindGroup,
    out_view: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bg, &[]);
    pass.draw(0..3, 0..1);
}
