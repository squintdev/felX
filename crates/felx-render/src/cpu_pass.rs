//! CPU-pass machinery: download a GPU texture, run a pure-Rust function on
//! the pixels, upload the result. Used by effects that don't fit a fragment
//! shader (Floyd-Steinberg-style sequential algorithms, third-party CPU
//! crates, long-state IIR filters).

use crate::Renderer;
use crate::texture_io::{download_image, upload_image};
use image::RgbaImage;
use tracing::debug_span;

/// Run a CPU function over the contents of `input`, producing a fresh
/// output texture. Tracing spans are emitted for the readback, the CPU
/// work, and the upload so per-frame timing is visible.
pub fn run_cpu_pass<F>(
    renderer: &Renderer,
    input: &wgpu::Texture,
    name: &str,
    mut f: F,
) -> wgpu::Texture
where
    F: FnMut(&mut RgbaImage),
{
    let mut img = {
        let _s = debug_span!("cpu_pass.readback", effect = name).entered();
        download_image(renderer, input)
    };
    {
        let _s = debug_span!("cpu_pass.work", effect = name).entered();
        f(&mut img);
    }
    let _s = debug_span!("cpu_pass.upload", effect = name).entered();
    upload_image(renderer, &img)
}
