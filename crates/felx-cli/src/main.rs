fn main() {
    felx_core::diagnostics::init_tracing();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "felx starting");
}
