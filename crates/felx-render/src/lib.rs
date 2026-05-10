//! wgpu-based render pipeline for analog-felx.
//!
//! [`Renderer`] owns or borrows a wgpu `Device` and `Queue`. The headless
//! constructor is for the CLI render path, integration tests, and the
//! visual-regression harness; the borrowed constructor is for hosting under
//! `eframe` (per ADR 0002), which owns the device for the live preview.

pub mod compositor;
pub mod cpu_pass;
pub mod effects;
pub mod frame_cache;
mod renderer;
pub mod texture_io;

pub use renderer::*;
