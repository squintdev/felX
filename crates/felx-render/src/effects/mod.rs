//! GPU effect implementations. Each effect's WGSL lives in
//! `<workspace>/effects/<id>/effect.wgsl`; the Rust runtime here owns the
//! pipeline and bind-group plumbing.

pub mod gain;
