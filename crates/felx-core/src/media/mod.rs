//! Runtime media infrastructure: asset library cache, decode/probe wrappers
//! (added in F-013), the audio mixer (F-051), waveform thumbnails (F-055),
//! and A/V sync timing math (F-053).

pub mod av_sync;
pub mod library;
pub mod mixer;
pub mod waveform;

pub use av_sync::{AudioClock, SyncDecision, SyncTolerance, decide as av_sync_decide};
pub use library::{AssetError, AssetLibrary, AssetMetadata};
pub use mixer::{AudioSource, DEFAULT_MASTER_RATE, MixedBus, mix_window};
pub use waveform::{Waveform, WaveformBin, compute_waveform};
