//! CRT Phosphor Persistence (F-079) — first stateful effect.
//!
//! Mixes the current frame with a decayed copy of the previous frame's
//! output, using the [`crate::effect_state::EffectStateRegistry`]
//! ping-pong texture pair (F-070). On a seek the state resets and the
//! trail starts clean — this is exactly what makes scrubbing not turn
//! into smeared mush.

use crate::Renderer;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CrtPersistenceParams {
    pub decay: f32,
    pub tint_r: f32,
    pub tint_g: f32,
    pub tint_b: f32,
}

pub struct CrtPersistence {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

impl CrtPersistence {
    pub fn new(renderer: &Renderer, format: wgpu::TextureFormat) -> Self {
        let shader = renderer
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("crt_persistence.wgsl"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../effects/crt_persistence/effect.wgsl").into(),
                ),
            });
        let sampler = renderer.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("crt_persistence.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = renderer
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("crt_persistence.bgl"),
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
        let pipeline_layout =
            renderer
                .device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("crt_persistence.pl"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });
        let pipeline = renderer
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("crt_persistence.pipeline"),
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

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        current_view: &wgpu::TextureView,
        prev_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
        params: CrtPersistenceParams,
    ) {
        let buf = renderer.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("crt_persistence.uniform"),
            size: std::mem::size_of::<CrtPersistenceParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        renderer
            .queue()
            .write_buffer(&buf, 0, bytemuck::bytes_of(&params));
        let bg = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crt_persistence.bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(current_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(prev_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buf.as_entire_binding(),
                    },
                ],
            });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("crt_persistence.pass"),
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
