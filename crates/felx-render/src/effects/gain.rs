//! Gain effect — multiplies RGB by a scalar. First GPU effect; establishes
//! the fullscreen-pass pattern other effects will follow.

use crate::Renderer;
use bytemuck::{Pod, Zeroable};

/// Embedded copy of `effects/gain/effect.wgsl` at build time. Used as the
/// default and as the fallback when hot-reload can't find the file on disk.
pub const EMBEDDED_SHADER_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../effects/gain/effect.wgsl"
));

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GainParams {
    pub gain: f32,
    _pad: [f32; 3],
}

impl GainParams {
    pub fn new(gain: f32) -> Self {
        Self {
            gain,
            _pad: [0.0; 3],
        }
    }
}

pub struct Gain {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    output_format: wgpu::TextureFormat,
}

impl Gain {
    pub fn new(renderer: &Renderer, output_format: wgpu::TextureFormat) -> Self {
        Self::with_shader(renderer, output_format, EMBEDDED_SHADER_SRC)
    }

    /// Build with a custom WGSL source (e.g. read from disk for hot-reload).
    /// Panics if the shader fails to compile; for non-panicking compile use
    /// [`Self::try_with_shader`].
    pub fn with_shader(
        renderer: &Renderer,
        output_format: wgpu::TextureFormat,
        wgsl: &str,
    ) -> Self {
        match Self::try_with_shader(renderer, output_format, wgsl) {
            Ok(gain) => gain,
            Err(e) => panic!("gain shader compile failed: {e}"),
        }
    }

    /// Build with a custom WGSL source, returning the compile error on
    /// failure. Used by hot-reload to surface errors without crashing.
    pub fn try_with_shader(
        renderer: &Renderer,
        output_format: wgpu::TextureFormat,
        wgsl: &str,
    ) -> Result<Self, String> {
        let device = renderer.device();

        // Capture validation errors from the shader compile by pushing an
        // error scope around module creation.
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gain.wgsl"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        if let Some(err) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("{err}"));
        }

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gain.bgl"),
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
            label: Some("gain.pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gain.rp"),
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
            label: Some("gain.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gain.uniforms"),
            size: std::mem::size_of::<GainParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            output_format,
        })
    }

    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    /// Run one Gain pass: read from `input`, write to `output`.
    pub fn render(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        params: GainParams,
    ) {
        renderer
            .queue()
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        let bind_group = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gain.bg"),
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
            label: Some("gain.pass"),
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
