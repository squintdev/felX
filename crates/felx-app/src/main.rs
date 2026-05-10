//! analog-felx GUI entry point.
//!
//! Hosts an eframe window, shares its wgpu device with felx-render's
//! [`Compositor`], and shows the rendered comp in a Viewer panel with a
//! Layer panel beside it.
//!
//! M1 scope: still frame, no playback. F-023 brings the playback loop;
//! F-026 wires up the parameter panels.

mod app;
mod panels;

use app::FelxApp;
use eframe::NativeOptions;

fn main() -> eframe::Result<()> {
    felx_core::diagnostics::init_tracing();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "felx-app starting");

    let options = NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("felx")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "felx",
        options,
        Box::new(|cc| Ok(Box::new(FelxApp::new(cc)?))),
    )
}
