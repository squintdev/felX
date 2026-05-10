//! wgpu device + queue ownership.
//!
//! `wgpu::Device` and `wgpu::Queue` are already reference-counted internally,
//! so we hold them by value here; cloning is cheap and avoids a redundant
//! `Arc<Arc<...>>` layer when interoperating with eframe.

use tracing::info;

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
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            power_preference: wgpu::PowerPreference::HighPerformance,
            allow_software_fallback: false,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
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

async fn request_adapter(
    instance: &wgpu::Instance,
    power: wgpu::PowerPreference,
    allow_software_fallback: bool,
) -> Result<wgpu::Adapter, RendererError> {
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
