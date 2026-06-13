//! wgpu device + queue ownership.
//!
//! `wgpu::Device` and `wgpu::Queue` are already reference-counted internally,
//! so we hold them by value here; cloning is cheap and avoids a redundant
//! `Arc<Arc<...>>` layer when interoperating with eframe.

use tracing::{info, warn};

#[derive(Clone, Debug)]
pub struct AdapterInfo {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub backend: wgpu::Backend,
    pub adapter_type: wgpu::DeviceType,
}

impl From<wgpu::AdapterInfo> for AdapterInfo {
    fn from(i: wgpu::AdapterInfo) -> Self {
        Self {
            name: i.name,
            vendor: i.vendor,
            device: i.device,
            backend: i.backend,
            adapter_type: i.device_type,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RendererOptions {
    pub power_preference: wgpu::PowerPreference,
    /// Allow falling back to a software adapter (e.g. lavapipe) when no
    /// hardware adapter is available. Honors `FELX_SOFTWARE_GPU=1`.
    pub allow_software_fallback: bool,
    pub required_features: wgpu::Features,
    pub required_limits: wgpu::Limits,
    /// Optional case-insensitive substring match on the adapter name.
    /// Used by the GUI to honor a saved Export-GPU preference. `None` =
    /// fall back to env var (`FELX_GPU`) then automatic discrete-prefer
    /// logic.
    pub gpu_name_pref: Option<String>,
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            power_preference: wgpu::PowerPreference::HighPerformance,
            allow_software_fallback: false,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            gpu_name_pref: None,
        }
    }
}

#[derive(Debug)]
pub enum RendererError {
    NoCompatibleAdapter,
    DeviceRequest(wgpu::RequestDeviceError),
    AdapterRequest(wgpu::RequestAdapterError),
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoCompatibleAdapter => write!(f, "no compatible wgpu adapter"),
            RendererError::DeviceRequest(e) => write!(f, "wgpu device request failed: {e}"),
            RendererError::AdapterRequest(e) => write!(f, "wgpu adapter request failed: {e}"),
        }
    }
}

impl std::error::Error for RendererError {}

/// Owns or borrows a wgpu device + queue.
#[derive(Clone)]
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: AdapterInfo,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl Renderer {
    /// Block on initialization. For CLI / tests / the visual-regression
    /// harness — anywhere outside of an async runtime.
    pub fn new_headless(opts: RendererOptions) -> Result<Self, RendererError> {
        pollster::block_on(Self::new_headless_async(opts))
    }

    pub async fn new_headless_async(mut opts: RendererOptions) -> Result<Self, RendererError> {
        if std::env::var("FELX_SOFTWARE_GPU").as_deref() == Ok("1") {
            opts.allow_software_fallback = true;
        }

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = request_adapter(
            &instance,
            opts.power_preference,
            opts.allow_software_fallback,
            opts.gpu_name_pref.as_deref(),
        )
        .await?;

        let info: AdapterInfo = adapter.get_info().into();
        info!(
            name = %info.name,
            backend = ?info.backend,
            device_type = ?info.adapter_type,
            "wgpu adapter selected"
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("felx-render"),
                required_features: opts.required_features,
                required_limits: opts.required_limits,
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(RendererError::DeviceRequest)?;

        Ok(Self {
            device,
            queue,
            info,
        })
    }

    /// Wrap a borrowed device + queue, e.g. from eframe's wgpu render state.
    /// `wgpu::Device` and `wgpu::Queue` are cheap to clone (internally
    /// reference-counted), so this takes them by value.
    pub fn from_borrowed(device: wgpu::Device, queue: wgpu::Queue, info: AdapterInfo) -> Self {
        Self {
            device,
            queue,
            info,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.info
    }
}

/// Pick the most-capable wgpu adapter we can find, with overrides:
///
/// 1. `FELX_GPU=<substring>` — case-insensitive substring match on the
///    adapter name. `FELX_GPU=nvidia` selects the NVIDIA card, `intel`
///    forces the iGPU, etc.
/// 2. `FELX_BACKEND=<vulkan|metal|dx12|gl>` — force a specific backend.
/// 3. Else: prefer a discrete GPU on a non-GL backend (Vulkan / Metal /
///    DX12). Falls back to whatever `HighPerformance` returns.
///
/// Without these overrides, `wgpu::Instance::request_adapter` on Linux
/// PRIME setups (Intel iGPU + NVIDIA dGPU) often returns the Intel
/// adapter via Mesa's GL backend because GL adapters enumerate first.
/// Enumerating ourselves fixes that.
async fn request_adapter(
    instance: &wgpu::Instance,
    power: wgpu::PowerPreference,
    allow_software_fallback: bool,
    settings_pref: Option<&str>,
) -> Result<wgpu::Adapter, RendererError> {
    // Log every adapter so users can see what's available and what we
    // picked from. Cheap and high-signal for "why is it using my iGPU?"
    // diagnoses.
    let all: Vec<wgpu::Adapter> = instance.enumerate_adapters(wgpu::Backends::all());
    for a in &all {
        let info = a.get_info();
        info!(
            name = %info.name,
            backend = ?info.backend,
            device_type = ?info.device_type,
            "wgpu adapter available"
        );
    }

    // 1) Override by name substring. FELX_GPU env var first, then the
    // caller-supplied preference (which the GUI threads through from its
    // saved Settings).
    let pref = std::env::var("FELX_GPU")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| settings_pref.map(str::to_string));
    if let Some(want) = pref {
        let want_lower = want.to_lowercase();
        if let Some(a) = all
            .iter()
            .find(|a| a.get_info().name.to_lowercase().contains(&want_lower))
        {
            info!(want, "GPU preference matched");
            return Ok(clone_via_re_enumerate(instance, a.get_info()));
        }
        warn!(
            want,
            "GPU preference set but no adapter matched; falling back"
        );
    }

    // 2) Explicit backend override.
    let backend_override =
        std::env::var("FELX_BACKEND")
            .ok()
            .and_then(|s| match s.to_lowercase().as_str() {
                "vulkan" => Some(wgpu::Backend::Vulkan),
                "metal" => Some(wgpu::Backend::Metal),
                "dx12" | "d3d12" => Some(wgpu::Backend::Dx12),
                "gl" | "opengl" => Some(wgpu::Backend::Gl),
                _ => None,
            });
    if let Some(want_backend) = backend_override
        && let Some(a) = all.iter().find(|a| a.get_info().backend == want_backend)
    {
        return Ok(clone_via_re_enumerate(instance, a.get_info()));
    }

    // 3) Prefer DiscreteGpu on a non-GL backend.
    let preferred = all.iter().find(|a| {
        let info = a.get_info();
        matches!(info.device_type, wgpu::DeviceType::DiscreteGpu)
            && !matches!(info.backend, wgpu::Backend::Gl)
    });
    if let Some(a) = preferred {
        return Ok(clone_via_re_enumerate(instance, a.get_info()));
    }

    // 4) Any non-GL adapter (avoid Mesa's GL when Vulkan is available).
    let any_non_gl = all
        .iter()
        .find(|a| !matches!(a.get_info().backend, wgpu::Backend::Gl));
    if let Some(a) = any_non_gl {
        return Ok(clone_via_re_enumerate(instance, a.get_info()));
    }

    // 5) Fall back to wgpu's built-in HighPerformance selection.
    match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
    {
        Ok(a) => Ok(a),
        Err(_) if allow_software_fallback => instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: true,
                compatible_surface: None,
            })
            .await
            .map_err(|_| RendererError::NoCompatibleAdapter),
        Err(_) => Err(RendererError::NoCompatibleAdapter),
    }
}

/// `wgpu::Adapter` doesn't impl `Clone`. Once we've identified the
/// adapter we want by inspecting the enumerated set, re-enumerate and
/// take the matching one by ownership. Slightly wasteful but the cost
/// is one Vulkan / DX12 instance scan, which is microseconds.
fn clone_via_re_enumerate(instance: &wgpu::Instance, target: wgpu::AdapterInfo) -> wgpu::Adapter {
    let mut adapters = instance.enumerate_adapters(wgpu::Backends::all());
    if let Some(idx) = adapters.iter().position(|a| {
        let i = a.get_info();
        i.name == target.name
            && i.vendor == target.vendor
            && i.device == target.device
            && i.backend == target.backend
    }) {
        return adapters.swap_remove(idx);
    }
    // Should not happen — we just enumerated the same backends.
    adapters
        .into_iter()
        .next()
        .expect("at least one adapter must exist")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tries a real adapter first; if that's not available (no GPU on the
    /// host, headless test environment), retries with software fallback.
    fn try_renderer() -> Option<Renderer> {
        let opts = RendererOptions {
            allow_software_fallback: true,
            ..Default::default()
        };
        Renderer::new_headless(opts).ok()
    }

    #[test]
    fn renderer_initializes_with_software_fallback() {
        let Some(r) = try_renderer() else {
            // No adapter at all (very rare; software fallback should always
            // work). Skip rather than fail spuriously.
            eprintln!("[felx-render] no adapter; skipping test");
            return;
        };
        let info = r.adapter_info();
        assert!(!info.name.is_empty(), "adapter name should not be empty");
    }

    #[test]
    fn device_can_create_a_buffer() {
        let Some(r) = try_renderer() else {
            return;
        };
        let buf = r.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("smoke-test"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        assert_eq!(buf.size(), 256);
    }
}
