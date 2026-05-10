//! wgpu-based render pipeline for analog-felx.
//!
//! [`Renderer`] owns or borrows a wgpu `Device` and `Queue`. The headless
//! constructor is for the CLI render path, integration tests, and the
//! visual-regression harness; the borrowed constructor is for hosting under
//! `eframe` (per ADR 0002), which owns the device for the live preview.

pub mod blend_pass;
pub mod clear_pass;
pub mod compositor;
pub mod cpu_pass;
pub mod effect_state;
pub mod effects;
pub mod frame_cache;
pub mod mask_pass;
pub mod matte_pass;
mod renderer;
pub mod srgb_wrap;
pub mod texture_io;
pub mod transform_pass;
pub mod walker;

pub use renderer::*;
