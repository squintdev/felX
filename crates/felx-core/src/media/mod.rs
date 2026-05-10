//! Runtime media infrastructure: asset library cache, decode/probe wrappers
//! (added in F-013).

pub mod library;

pub use library::{AssetError, AssetLibrary, AssetMetadata};
