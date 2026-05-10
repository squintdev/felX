//! CC Toner — multi-tone color mapping. See effects.md for the full algorithm
//! breakdown. Operates in sRGB-encoded space; the compositor wraps the pass.

use crate::Renderer;
use bytemuck::{Pod, Zeroable};

pub const EMBEDDED_SHADER_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../effects/cc_toner/effect.wgsl"
));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TonesMode {
    Solid,
    Duotone,
    Tritone,
    Quadtone,
    Pentone,
}

impl TonesMode {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "solid" => TonesMode::Solid,
            "duotone" => TonesMode::Duotone,
            "tritone" => TonesMode::Tritone,
            "quadtone" => TonesMode::Quadtone,
            "pentone" => TonesMode::Pentone,
            _ => return None,
        })
    }

    pub fn n_stops(self) -> u32 {
        match self {
            TonesMode::Solid => 1,
            TonesMode::Duotone => 2,
            TonesMode::Tritone => 3,
            TonesMode::Quadtone => 4,
            TonesMode::Pentone => 5,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CcTonerParams {
    /// Active stops, in shadows→highlights order. Unused slots are still
    /// padded; the shader only reads `n_stops` of them.
    pub stops: [[f32; 4]; 5],
    pub n_stops: u32,
    pub blend: f32,
    _pad0: f32,
    _pad1: f32,
}

impl CcTonerParams {
    /// Build params from the 5 named slot colors and the mode. Slot ordering
    /// per mode (from effects.md):
    /// - Solid:    Midtones
    /// - Duotone:  Shadows, Highlights
    /// - Tritone:  Shadows, Midtones, Highlights
    /// - Quadtone: Shadows, Darktones, Brights, Highlights
    /// - Pentone:  Shadows, Darktones, Midtones, Brights, Highlights
    pub fn pack(
        mode: TonesMode,
        highlights: [f32; 4],
        brights: [f32; 4],
        midtones: [f32; 4],
        darktones: [f32; 4],
        shadows: [f32; 4],
        blend: f32,
    ) -> Self {
        let mut stops = [[0.0_f32; 4]; 5];
        match mode {
            TonesMode::Solid => stops[0] = midtones,
            TonesMode::Duotone => {
                stops[0] = shadows;
                stops[1] = highlights;
            }
            TonesMode::Tritone => {
                stops[0] = shadows;
                stops[1] = midtones;
                stops[2] = highlights;
            }
            TonesMode::Quadtone => {
                stops[0] = shadows;
                stops[1] = darktones;
                stops[2] = brights;
                stops[3] = highlights;
            }
            TonesMode::Pentone => {
                stops[0] = shadows;
                stops[1] = darktones;
                stops[2] = midtones;
                stops[3] = brights;
                stops[4] = highlights;
            }
        }
        Self {
            stops,
            n_stops: mode.n_stops(),
            blend,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }
}

pub struct CcToner {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
}

impl CcToner {
    pub fn new(renderer: &Renderer, output_format: wgpu::TextureFormat) -> Self {
        let device = renderer.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cc_toner.wgsl"),
            source: wgpu::ShaderSource::Wgsl(EMBEDDED_SHADER_SRC.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cc_toner.bgl"),
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
            label: Some("cc_toner.pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cc_toner.rp"),
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
            label: Some("cc_toner.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cc_toner.uniforms"),
            size: std::mem::size_of::<CcTonerParams>() as u64,
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
        params: CcTonerParams,
    ) {
        renderer
            .queue()
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));
        let bind_group = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cc_toner.bg"),
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
            label: Some("cc_toner.pass"),
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
