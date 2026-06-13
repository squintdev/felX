//! Per-layer blend pass. Composites a layer texture over an accumulator
//! using a selectable blend mode. F-040 ships with `Normal` (alpha-over);
//! F-042 extends the shader with the rest of AE's blend mode set.

use crate::Renderer;
use bytemuck::{Pod, Zeroable};

const SHADER: &str = r#"
struct Params {
    mode: u32,
    opacity: f32,
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var bg_tex: texture_2d<f32>;
@group(0) @binding(1) var bg_smp: sampler;
@group(0) @binding(2) var fg_tex: texture_2d<f32>;
@group(0) @binding(3) var fg_smp: sampler;
@group(0) @binding(4) var<uniform> params: Params;

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

fn overlay_channel(b: f32, f: f32) -> f32 {
    if b < 0.5 { return 2.0 * b * f; }
    return 1.0 - 2.0 * (1.0 - b) * (1.0 - f);
}
fn hard_light_channel(b: f32, f: f32) -> f32 {
    if f < 0.5 { return 2.0 * b * f; }
    return 1.0 - 2.0 * (1.0 - b) * (1.0 - f);
}
fn color_dodge_channel(b: f32, f: f32) -> f32 {
    if f >= 1.0 { return 1.0; }
    return min(1.0, b / (1.0 - f));
}
fn color_burn_channel(b: f32, f: f32) -> f32 {
    if f <= 0.0 { return 0.0; }
    return 1.0 - min(1.0, (1.0 - b) / f);
}

fn blend_color(mode: u32, b: vec3<f32>, f: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 0u: { return f; }
        case 1u: { return min(b + f, vec3(1.0)); }
        case 2u: { return b * f; }
        case 3u: { return 1.0 - (1.0 - b) * (1.0 - f); }
        case 4u: {
            return vec3(
                overlay_channel(b.r, f.r),
                overlay_channel(b.g, f.g),
                overlay_channel(b.b, f.b),
            );
        }
        case 5u: {
            return vec3(
                hard_light_channel(b.r, f.r),
                hard_light_channel(b.g, f.g),
                hard_light_channel(b.b, f.b),
            );
        }
        case 6u: { return max(b, f); }
        case 7u: { return min(b, f); }
        case 8u: { return abs(b - f); }
        case 9u: { return b + f - 2.0 * b * f; }
        case 10u: {
            return vec3(
                color_dodge_channel(b.r, f.r),
                color_dodge_channel(b.g, f.g),
                color_dodge_channel(b.b, f.b),
            );
        }
        case 11u: {
            return vec3(
                color_burn_channel(b.r, f.r),
                color_burn_channel(b.g, f.g),
                color_burn_channel(b.b, f.b),
            );
        }
        case 12u: { return max(b + f - 1.0, vec3(0.0)); }
        default: { return f; }
    }
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let bg = textureSample(bg_tex, bg_smp, in.uv);
    var fg = textureSample(fg_tex, fg_smp, in.uv);
    fg = fg * params.opacity;

    let bg_rgb = bg.rgb / max(bg.a, 1e-6);
    let fg_rgb = fg.rgb / max(fg.a, 1e-6);
    let blended = blend_color(params.mode, bg_rgb, fg_rgb);

    let one_minus_fa = 1.0 - fg.a;
    let out_a = fg.a + bg.a * one_minus_fa;
    let out_rgb = blended * fg.a + bg_rgb * bg.a * one_minus_fa;
    return vec4(out_rgb, out_a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BlendParams {
    pub mode: u32,
    pub opacity: f32,
    _pad0: f32,
    _pad1: f32,
}

impl BlendParams {
    pub fn normal(opacity: f32) -> Self {
        Self::with_mode(0, opacity)
    }

    pub fn with_mode(mode: u32, opacity: f32) -> Self {
        Self {
            mode,
            opacity,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }
}

pub struct BlendPass {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
}

impl BlendPass {
    pub fn new(renderer: &Renderer, output_format: wgpu::TextureFormat) -> Self {
        let device = renderer.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blend.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend.bgl"),
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
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blend.pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blend.rp"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blend.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blend.uniforms"),
            size: std::mem::size_of::<BlendParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
        }
    }

    pub fn render(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        bg: &wgpu::TextureView,
        fg: &wgpu::TextureView,
        output: &wgpu::TextureView,
        params: BlendParams,
    ) {
        renderer
            .queue()
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));
        let bind_group = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blend.bg"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(bg),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(fg),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                ],
            });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blend.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
