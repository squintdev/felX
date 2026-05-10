//! Effect implementations. GPU effects ship a WGSL shader under
//! `<workspace>/effects/<id>/effect.wgsl`; the Rust runtime here owns the
//! pipeline and bind-group plumbing. CPU-pass effects are pure Rust.

pub mod cc_toner;
pub mod crt;
pub mod crt_persistence;
pub mod gain;
pub mod invert;
pub mod squint_diffusion;
pub mod signal;
pub mod vhs;
