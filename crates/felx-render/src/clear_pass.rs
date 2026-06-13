//! Clear an output texture to a single RGBA color via a render pass with
//! `LoadOp::Clear`. Cheaper than `upload_image` for the comp's background
//! fill, and the texture is pool-acquired so subsequent frames reuse it.

use crate::Renderer;

pub fn clear_to(renderer: &Renderer, output: &wgpu::TextureView, color: [f32; 4]) {
    let mut encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("clear_to"),
        });
    {
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear_to.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: color[0] as f64,
                        g: color[1] as f64,
                        b: color[2] as f64,
                        a: color[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
    renderer.queue().submit(Some(encoder.finish()));
}
