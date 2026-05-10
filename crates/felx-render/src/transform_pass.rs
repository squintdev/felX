//! Layer transform pass. Takes a layer's rendered texture and places it
//! on the composition's canvas at the layer's transform (position /
//! anchor / scale / rotation / opacity).
//!
//! Inverse-mapping fragment shader: for each output pixel, walks the
//! transform backward to find the source pixel; samples + opacity multiplies
//! + handles out-of-bounds with the comp's background fill.
//!
//! v1 treats the input texture as covering the comp's full canvas at scale
//! 1.0; multi-layer compositing (F-040) and per-asset native dims arrive
//! later.

use crate::Renderer;
use bytemuck::{Pod, Zeroable};

const SHADER: &str = r#"
struct Params {
    position: vec2<f32>,
    anchor: vec2<f32>,
    scale: vec2<f32>,
    rot_cos: f32,
    rot_sin: f32,
    opacity: f32,
    _pad0: f32,
    src_size: vec2<f32>,
    out_size: vec2<f32>,
    // 16-byte alignment for vec4 — padding pushes us from offset 56 to 64.
    _pad1: vec2<f32>,
    background: vec4<f32>,
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
    // Output pixel coordinate in comp pixels (top-left origin).
    let p = in.uv * params.out_size;
    // Translate
    let dx = p.x - params.position.x;
    let dy = p.y - params.position.y;
    // Inverse-rotate. Forward is clockwise (+rot); inverse is counter-clockwise.
    let rx = params.rot_cos * dx + params.rot_sin * dy;
    let ry = -params.rot_sin * dx + params.rot_cos * dy;
    // Inverse-scale.
    let sx = select(0.0, rx / params.scale.x, params.scale.x != 0.0);
    let sy = select(0.0, ry / params.scale.y, params.scale.y != 0.0);
    // Add anchor → source pixel.
    let su = sx + params.anchor.x;
    let sv = sy + params.anchor.y;
    // Normalize to UV.
    let uv = vec2(su / params.src_size.x, sv / params.src_size.y);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return params.background;
    }
    let s = textureSample(input_tex, input_smp, uv);
    return vec4(s.rgb * params.opacity, s.a * params.opacity);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TransformParams {
    pub position: [f32; 2],
    pub anchor: [f32; 2],
    pub scale: [f32; 2],
    pub rot_cos: f32,
    pub rot_sin: f32,
    pub opacity: f32,
    _pad0: f32,
    pub src_size: [f32; 2],
    pub out_size: [f32; 2],
    /// Padding to land `background` on a 16-byte alignment per WGSL std140
    /// uniform layout rules.
    _pad1: [f32; 2],
    pub background: [f32; 4],
}

impl TransformParams {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        position: [f32; 2],
        anchor: [f32; 2],
        scale: [f32; 2],
        rotation_deg: f32,
        opacity: f32,
        src_size: [f32; 2],
        out_size: [f32; 2],
        background: [f32; 4],
    ) -> Self {
        let r = rotation_deg.to_radians();
        Self {
            position,
            anchor,
            scale,
            rot_cos: r.cos(),
            rot_sin: r.sin(),
            opacity,
            _pad0: 0.0,
            src_size,
            out_size,
            _pad1: [0.0; 2],
            background,
        }
    }

    /// Identity: input texture is placed at comp origin with no scale /
    /// rotation, full opacity, transparent black background fill.
    pub fn identity(comp_w: u32, comp_h: u32) -> Self {
        Self::build(
            [0.0, 0.0],
            [0.0, 0.0],
            [1.0, 1.0],
            0.0,
            1.0,
            [comp_w as f32, comp_h as f32],
            [comp_w as f32, comp_h as f32],
            [0.0, 0.0, 0.0, 0.0],
        )
    }
}

pub struct TransformPass {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
}

impl TransformPass {
    pub fn new(renderer: &Renderer, output_format: wgpu::TextureFormat) -> Self {
        let device = renderer.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform_pass.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transform.bgl"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transform.pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transform.rp"),
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
            label: Some("transform.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform.uniforms"),
            size: std::mem::size_of::<TransformParams>() as u64,
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
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        params: TransformParams,
    ) {
        renderer
            .queue()
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));
        let bind_group = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("transform.bg"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                ],
            });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("transform.pass"),
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
