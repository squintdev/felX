//! Logging and diagnostics initialization.
//!
//! Binaries call [`init_tracing`] once at startup. The `RUST_LOG` environment
//! variable controls verbosity; if unset, the filter defaults to `felx=info`.

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const DEFAULT_FILTER: &str = "felx=info";

/// Initialize the global `tracing` subscriber.
///
/// Idempotent in practice: a second call is a no-op because
/// `tracing_subscriber::registry().init()` returns an error when a global
/// subscriber is already installed, and we swallow it.
pub fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_writer(std::io::stderr),
        )
        .try_init();
}
