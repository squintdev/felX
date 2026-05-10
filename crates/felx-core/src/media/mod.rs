//! Runtime media infrastructure: asset library cache, decode/probe wrappers
//! (added in F-013), the audio mixer (F-051), and waveform thumbnails
//! (F-055).

pub mod library;
pub mod mixer;
pub mod waveform;

pub use library::{AssetError, AssetLibrary, AssetMetadata};
pub use mixer::{AudioSource, DEFAULT_MASTER_RATE, MixedBus, mix_window};
pub use waveform::{Waveform, WaveformBin, compute_waveform};
