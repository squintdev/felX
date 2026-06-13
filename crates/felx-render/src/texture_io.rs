//! Helpers for moving pixels between CPU and GPU.
//!
//! These wrap the bookkeeping (bytes-per-row alignment, command encoder
//! setup, buffer mapping) so individual effects and tests can stay focused
//! on their actual work.

use crate::Renderer;
use image::RgbaImage;

/// Output texture format for compositor passes — straight `Rgba8Unorm`. We do
/// our own gamma handling per-effect (per ADR `working_space` metadata), so
/// the framebuffer must NOT be the auto-sRGB variant.
pub const COMPOSITOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Upload an [`RgbaImage`] to a fresh GPU texture in `COMPOSITOR_FORMAT`.
pub fn upload_image(renderer: &Renderer, image: &RgbaImage) -> wgpu::Texture {
    let (w, h) = image.dimensions();
    let texture = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("upload_image"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COMPOSITOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    renderer.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    texture
}

/// Read an `Rgba8Unorm` texture back to an [`RgbaImage`].
pub fn download_image(renderer: &Renderer, texture: &wgpu::Texture) -> RgbaImage {
    let w = texture.width();
    let h = texture.height();
    // wgpu requires bytes_per_row to be a multiple of 256.
    let unpadded_bpr = 4 * w;
    let padding = (256 - unpadded_bpr % 256) % 256;
    let padded_bpr = unpadded_bpr + padding;

    let buffer_size = u64::from(padded_bpr) * u64::from(h);
    let buffer = renderer.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("download_image"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("download_image"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bpr),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue().submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    renderer
        .device()
        .poll(wgpu::PollType::Wait)
        .expect("device poll");

    let view = slice.get_mapped_range();

    let mut pixels: Vec<u8> = Vec::with_capacity((unpadded_bpr * h) as usize);
    for row in 0..h as usize {
        let start = row * padded_bpr as usize;
        let end = start + unpadded_bpr as usize;
        pixels.extend_from_slice(&view[start..end]);
    }
    drop(view);
    buffer.unmap();

    RgbaImage::from_raw(w, h, pixels).expect("rgba image from raw bytes")
}

/// Create an empty render-target texture in `COMPOSITOR_FORMAT`.
pub fn create_render_target(renderer: &Renderer, w: u32, h: u32, label: &str) -> wgpu::Texture {
    renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COMPOSITOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}
