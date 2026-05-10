//! sRGB encode / decode wrap passes used to host effects that operate in
//! gamma-encoded space (per ADR per-effect `working_space` metadata).
//!
//! The compositor's textures are nominally linear-light. Effects whose
//! manifest declares `working_space = srgb` (CC Toner is the canonical
//! example — its visual character depends on doing the lerp on
//! gamma-encoded values) need their input encoded with the sRGB OETF first
//! and the output decoded back with the EOTF.
//!
//! The two wrap passes are tiny full-screen-triangle fragment shaders that
//! do the per-channel OETF/EOTF in WGSL.

use crate::Renderer;

const ENCODE_SRC: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_smp: sampler;

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    let x = f32((idx & 1u) << 2u) - 1.0;
    let y = f32((idx & 2u) << 1u) - 1.0;
    var out: VsOut;
    out.clip = vec4(x, y, 0.0, 1.0);
    out.uv = vec2((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

fn lin_to_srgb_channel(c: f32) -> f32 {
    if c <= 0.0031308 {
        return 12.92 * c;
    } else {
        return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(input_tex, input_smp, in.uv);
    return vec4(
        lin_to_srgb_channel(s.r),
        lin_to_srgb_channel(s.g),
        lin_to_srgb_channel(s.b),
        s.a,
    );
}
"#;

const DECODE_SRC: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_smp: sampler;

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    let x = f32((idx & 1u) << 2u) - 1.0;
    let y = f32((idx & 2u) << 1u) - 1.0;
    var out: VsOut;
    out.clip = vec4(x, y, 0.0, 1.0);
    out.uv = vec2((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

fn srgb_to_lin_channel(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(input_tex, input_smp, in.uv);
    return vec4(
        srgb_to_lin_channel(s.r),
        srgb_to_lin_channel(s.g),
        srgb_to_lin_channel(s.b),
        s.a,
    );
}
"#;

pub struct SrgbWrap {
    encode_pipeline: wgpu::RenderPipeline,
    decode_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    output_format: wgpu::TextureFormat,
}

impl SrgbWrap {
    pub fn new(renderer: &Renderer, output_format: wgpu::TextureFormat) -> Self {
        let device = renderer.device();
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("srgb-wrap.bgl"),
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("srgb-wrap.pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let encode_pipeline = build_pipeline(
            device,
            &pipeline_layout,
            output_format,
            ENCODE_SRC,
            "srgb-encode",
        );
        let decode_pipeline = build_pipeline(
            device,
            &pipeline_layout,
            output_format,
            DECODE_SRC,
            "srgb-decode",
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("srgb-wrap.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            encode_pipeline,
            decode_pipeline,
            bind_group_layout,
            sampler,
            output_format,
        }
    }

    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    /// Run the `linear → sRGB-encoded` pass.
    pub fn encode(
        &self,
        renderer: &Renderer,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        self.run(renderer, input, output, true);
    }

    /// Run the `sRGB-encoded → linear` pass.
    pub fn decode(
        &self,
        renderer: &Renderer,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        self.run(renderer, input, output, false);
    }

    fn run(
        &self,
        renderer: &Renderer,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        is_encode: bool,
    ) {
        let bind_group = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(if is_encode {
                    "srgb-encode.bg"
                } else {
                    "srgb-decode.bg"
                }),
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
                ],
            });
        let mut encoder =
            renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some(if is_encode {
                        "srgb-encode"
                    } else {
                        "srgb-decode"
                    }),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if is_encode {
                    "srgb-encode.pass"
                } else {
                    "srgb-decode.pass"
                }),
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
            pass.set_pipeline(if is_encode {
                &self.encode_pipeline
            } else {
                &self.decode_pipeline
            });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        renderer.queue().submit(Some(encoder.finish()));
    }
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    source: &str,
    label: &'static str,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
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
                format,
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
    })
}
