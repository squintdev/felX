//! analog-felx GUI entry point.
//!
//! Hosts an eframe window, shares its wgpu device with felx-render's
//! [`Compositor`], and shows the rendered comp in a Viewer panel with a
//! Layer panel beside it.
//!
//! M1 scope: still frame, no playback. F-023 brings the playback loop;
//! F-026 wires up the parameter panels.

mod app;
mod audio_playback;
mod autosave;
mod crash_reporter;
mod curve_widget;
mod export_dialog;
mod hot_reload;
mod manifests;
mod panels;
mod playback;
mod presets;
mod render_queue;
mod settings;

use app::FelxApp;
use eframe::NativeOptions;
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    felx_core::diagnostics::init_tracing();
    crash_reporter::install();
    let app_settings = settings::Settings::load();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        viewer_gpu = ?app_settings.viewer_gpu,
        export_gpu = ?app_settings.export_gpu,
        "felx-app starting"
    );

    // Honor the viewer-GPU preference by giving eframe a custom adapter
    // selector. Env var still wins via pick_adapter_index.
    let viewer_pref = app_settings.viewer_gpu.clone();
    let selector: eframe::egui_wgpu::NativeAdapterSelectorMethod = Arc::new(
        move |adapters: &[wgpu::Adapter], _surface: Option<&wgpu::Surface<'_>>| {
            choose_viewer_adapter(adapters, viewer_pref.as_deref())
        },
    );
    let wgpu_setup = eframe::egui_wgpu::WgpuSetupCreateNew {
        native_adapter_selector: Some(selector),
        ..Default::default()
    };
    let wgpu_options = eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
        ..Default::default()
    };

    let options = NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        viewport: egui::ViewportBuilder::default()
            .with_title("felx")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "felx",
        options,
        Box::new(move |cc| Ok(Box::new(FelxApp::new(cc, app_settings)?))),
    )
}

/// Adapter-selection logic shared with the headless export path. Same
/// rules: honor FELX_GPU env var, else the saved preference, else
/// prefer DiscreteGpu on a non-GL backend, else any non-GL, else
/// whatever's first.
fn choose_viewer_adapter(
    adapters: &[wgpu::Adapter],
    settings_pref: Option<&str>,
) -> Result<wgpu::Adapter, String> {
    for a in adapters {
        let info = a.get_info();
        tracing::info!(
            name = %info.name,
            backend = ?info.backend,
            device_type = ?info.device_type,
            "viewer adapter available"
        );
    }

    // 1) FELX_GPU env var or saved preference.
    let pref = std::env::var("FELX_GPU")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| settings_pref.map(str::to_string));
    if let Some(want) = pref {
        let lower = want.to_lowercase();
        if let Some(a) = adapters
            .iter()
            .find(|a| a.get_info().name.to_lowercase().contains(&lower))
        {
            tracing::info!(want, "viewer adapter selected by preference");
            return Ok(clone_adapter(adapters, a));
        }
        tracing::warn!(want, "viewer-GPU preference set but nothing matched");
    }

    // 2) Prefer DiscreteGpu on a non-GL backend.
    if let Some(a) = adapters.iter().find(|a| {
        let info = a.get_info();
        matches!(info.device_type, wgpu::DeviceType::DiscreteGpu)
            && !matches!(info.backend, wgpu::Backend::Gl)
    }) {
        return Ok(clone_adapter(adapters, a));
    }

    // 3) Any non-GL adapter.
    if let Some(a) = adapters
        .iter()
        .find(|a| !matches!(a.get_info().backend, wgpu::Backend::Gl))
    {
        return Ok(clone_adapter(adapters, a));
    }

    // 4) Whatever's first.
    adapters
        .first()
        .map(|a| clone_adapter(adapters, a))
        .ok_or_else(|| "no wgpu adapters available".to_string())
}

/// `wgpu::Adapter` doesn't implement `Clone`. The `native_adapter_selector`
/// callback is handed a slice but must return an owned `Adapter`. Take
/// ownership by re-enumerating from a fresh instance and finding the
/// matching one by (name, vendor, device, backend).
fn clone_adapter(adapters: &[wgpu::Adapter], target: &wgpu::Adapter) -> wgpu::Adapter {
    let target_info = target.get_info();
    let _ = adapters; // we re-enumerate to get an owned Adapter
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let mut all = instance.enumerate_adapters(wgpu::Backends::all());
    if let Some(idx) = all.iter().position(|a| {
        let i = a.get_info();
        i.name == target_info.name
            && i.vendor == target_info.vendor
            && i.device == target_info.device
            && i.backend == target_info.backend
    }) {
        return all.swap_remove(idx);
    }
    all.into_iter()
        .next()
        .expect("at least one adapter must exist")
}
