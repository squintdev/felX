//! wgpu-based render pipeline for analog-felx.
//!
//! [`Renderer`] owns or borrows a wgpu `Device` and `Queue`. The headless
//! constructor is for the CLI render path, integration tests, and the
//! visual-regression harness; the borrowed constructor is for hosting under
//! `eframe` (per ADR 0002), which owns the device for the live preview.

mod renderer;

pub use renderer::*;
