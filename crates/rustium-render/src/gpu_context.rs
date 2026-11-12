use std::sync::Arc;

use tracing::{error, info, warn};

use crate::error::Error;

/// Which graphics API the user asked for in Settings → Performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphicsBackend {
    /// Let wgpu choose the best native backend.
    #[default]
    Automatic,
    /// Vulkan.
    Vulkan,
    /// DirectX 12 (Windows only).
    DirectX12,
}

impl GraphicsBackend {
    /// Whether this backend can be used yet; unavailable ones are shown in Settings but cannot be picked.
    ///
    /// DirectX 12 is off: wgpu compiles its shaders with FXC, which rejects the path tracer shader (dynamically
    /// indexed local arrays written in loops it cannot unroll).
    // TODO(mzak): compile with DXC (wgpu `static-dxc`) and turn DirectX 12 back on.
    pub const fn is_available(self) -> bool {
        !matches!(self, Self::DirectX12)
    }

    /// The wgpu backends this choice opens; Automatic leaves out the unavailable ones.
    pub fn backends(self) -> wgpu::Backends {
        match self {
            Self::Automatic => wgpu::Backends::PRIMARY.difference(wgpu::Backends::DX12),
            Self::Vulkan => wgpu::Backends::VULKAN,
            Self::DirectX12 => wgpu::Backends::DX12,
        }
    }

    /// Whether `choice` is a device this backend opens, so Settings can list it.
    pub fn offers(self, choice: &AdapterChoice) -> bool {
        match self {
            Self::Automatic => choice.backend.is_none_or(Self::is_available),
            _ => choice.backend == Some(self),
        }
    }

    fn of(backend: wgpu::Backend) -> Option<Self> {
        match backend {
            wgpu::Backend::Vulkan => Some(Self::Vulkan),
            wgpu::Backend::Dx12 => Some(Self::DirectX12),
            _ => None,
        }
    }
}

/// GPU choices that take effect when the [`GpuContext`] is created.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuPreferences {
    /// Requested graphics API.
    pub backend: GraphicsBackend,
    /// Name of the graphics device to use; `None` lets wgpu pick the best one.
    pub adapter: Option<String>,
}

/// A graphics device the machine offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterChoice {
    /// Device name as the driver reports it.
    pub name: String,
    /// Graphics API the device is reachable through; `None` for APIs the user cannot pick (Metal, OpenGL).
    pub backend: Option<GraphicsBackend>,
}

/// Lists the graphics devices of every native API on this machine.
pub async fn available_adapters() -> Vec<AdapterChoice> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    descriptor.backends = wgpu::Backends::PRIMARY;
    let instance = wgpu::Instance::new(descriptor);
    let mut choices: Vec<AdapterChoice> = Vec::new();
    for adapter in instance.enumerate_adapters(wgpu::Backends::PRIMARY).await {
        let info = adapter.get_info();
        let choice = AdapterChoice {
            name: info.name,
            backend: GraphicsBackend::of(info.backend),
        };
        if !choices.contains(&choice) {
            choices.push(choice);
        }
    }
    choices
}

/// The graphics APIs worth offering: Automatic, plus every API that has at least one device.
pub fn selectable_backends(adapters: &[AdapterChoice]) -> Vec<GraphicsBackend> {
    let mut list = vec![GraphicsBackend::Automatic];
    for backend in [GraphicsBackend::Vulkan, GraphicsBackend::DirectX12] {
        if adapters.iter().any(|adapter| adapter.backend == Some(backend)) {
            list.push(backend);
        }
    }
    list
}

/// What to try when opening the GPU with `requested` failed: Automatic with no chosen device, or `None`
/// when that is exactly what was requested (nothing left to fall back to).
pub fn startup_fallback(requested: &GpuPreferences) -> Option<GpuPreferences> {
    let automatic = GpuPreferences::default();
    (*requested != automatic).then_some(automatic)
}

/// The wgpu device, queue and window surface.
#[derive(Debug)]
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl GpuContext {
    /// Opens the GPU and configures `target` (the window) at `width` × `height` physical pixels.
    ///
    /// # Errors
    ///
    /// Fails when the surface cannot be created or no adapter or device is available.
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        preferences: GpuPreferences,
    ) -> Result<Self, Error> {
        if !preferences.backend.is_available() {
            return Err(Error::BackendUnavailable(preferences.backend));
        }
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = preferences.backend.backends();
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(target)?;
        let adapter = match &preferences.adapter {
            Some(name) => {
                let found = instance
                    .enumerate_adapters(preferences.backend.backends())
                    .await
                    .into_iter()
                    .find(|adapter| adapter.get_info().name == *name && adapter.is_surface_supported(&surface));
                found.ok_or_else(|| Error::AdapterNotFound(name.clone()))?
            }
            None => {
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: Some(&surface),
                        ..Default::default()
                    })
                    .await?
            }
        };
        let adapter_info = adapter.get_info();
        info!(
            name = %adapter_info.name,
            backend = ?adapter_info.backend,
            kind = ?adapter_info.device_type,
            "graphics adapter"
        );
        let (device, queue) = request_device(&adapter).await?;
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .unwrap_or_else(|| fallback_config(width, height));
        // egui wants a non-sRGB target; the viewport composite draws through an sRGB view of the same texture.
        config.format = config.format.remove_srgb_suffix();
        config.view_formats = vec![config.format.add_srgb_suffix()];
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            config,
        })
    }

    /// The logical device.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The submission queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Pixel format of the window surface (non-sRGB, for the interface).
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// sRGB view format of the surface, used for the viewport composite.
    pub fn surface_srgb_format(&self) -> wgpu::TextureFormat {
        self.config.format.add_srgb_suffix()
    }

    /// Surface size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Name and backend of the adapter in use.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Whether the device was opened with hardware ray query, so [`RayTracing::Hardware`](crate::RayTracing) works.
    pub fn supports_ray_query(&self) -> bool {
        self.device.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
    }

    /// MSAA sample counts `format` supports on this adapter, always including 1.
    pub fn supported_sample_counts(&self, format: wgpu::TextureFormat) -> Vec<u32> {
        let features = self.adapter.get_texture_format_features(format);
        [1, 2, 4, 8]
            .into_iter()
            .filter(|count| *count == 1 || features.flags.sample_count_supported(*count))
            .collect()
    }

    /// Reconfigures the surface for a new window size; zero sizes (minimized) are ignored.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Turns vertical sync on or off.
    pub fn set_vsync(&mut self, vsync: bool) {
        self.config.present_mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        self.surface.configure(&self.device, &self.config);
    }

    /// The next surface texture to draw into, or `None` when this frame should be skipped.
    ///
    /// Suboptimal, outdated and lost surfaces are reconfigured on the spot and the frame is skipped.
    ///
    /// # Errors
    ///
    /// [`Error::SurfaceValidation`] when wgpu reports a validation error.
    pub fn acquire_frame(&mut self) -> Result<Option<wgpu::SurfaceTexture>, Error> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Ok(Some(frame)),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                drop(frame);
                self.surface.configure(&self.device, &self.config);
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => Ok(None),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                warn!("window surface outdated or lost, reconfiguring");
                self.surface.configure(&self.device, &self.config);
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Validation => Err(Error::SurfaceValidation),
        }
    }

    /// Recreates the surface for the same window after the old one was lost for good.
    ///
    /// # Errors
    ///
    /// Fails when the window can no longer provide a surface.
    pub fn recreate_surface(&mut self, target: impl Into<wgpu::SurfaceTarget<'static>>) -> Result<(), Error> {
        self.surface = self.instance.create_surface(target)?;
        self.surface.configure(&self.device, &self.config);
        Ok(())
    }
}

/// Opens the device with the adapter's resolution limits, and with hardware ray query where the adapter has it.
///
/// # Errors
///
/// Fails when the adapter cannot provide a device.
pub async fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), Error> {
    let ray_query = adapter.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY);
    let (required_features, experimental_features) = if ray_query {
        // SAFETY: ray query is experimental in wgpu; Rustium uses it behind the software fallback and only with
        // validated acceleration-structure builds.
        let token = unsafe { wgpu::ExperimentalFeatures::enabled() };
        (wgpu::Features::EXPERIMENTAL_RAY_QUERY, token)
    } else {
        (wgpu::Features::empty(), wgpu::ExperimentalFeatures::disabled())
    };
    let mut required_limits = wgpu::Limits::default().using_resolution(adapter.limits());
    if ray_query {
        // The path tracer binds one top-level acceleration structure; take what the adapter offers for the sizes.
        let offered = adapter.limits();
        required_limits.max_acceleration_structures_per_shader_stage = 1;
        required_limits.max_blas_primitive_count = offered.max_blas_primitive_count;
        required_limits.max_blas_geometry_count = offered.max_blas_geometry_count;
        required_limits.max_tlas_instance_count = offered.max_tlas_instance_count;
    }
    let descriptor = wgpu::DeviceDescriptor {
        label: Some("rustium device"),
        required_features,
        required_limits,
        experimental_features,
        ..Default::default()
    };
    let opened: Result<(wgpu::Device, wgpu::Queue), Error> = match adapter.request_device(&descriptor).await {
        Ok(pair) => Ok(pair),
        Err(error) if ray_query => {
            warn!(%error, "device with ray query failed, retrying without it");
            let plain = wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                ..descriptor
            };
            Ok(adapter.request_device(&plain).await?)
        }
        Err(error) => Err(error.into()),
    };
    let (device, queue) = opened?;
    // By default wgpu panics on an error nobody captured; a failed allocation must not take the editor down.
    device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
        error!(%error, "the GPU reported an error");
    }));
    Ok((device, queue))
}

fn fallback_config(width: u32, height: u32) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: width.max(1),
        height: height.max(1),
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: Vec::new(),
        color_space: wgpu::SurfaceColorSpace::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(name: &str, backend: Option<GraphicsBackend>) -> AdapterChoice {
        AdapterChoice {
            name: name.to_owned(),
            backend,
        }
    }

    #[test]
    fn a_failed_custom_choice_falls_back_to_automatic() {
        let broken = GpuPreferences {
            backend: GraphicsBackend::DirectX12,
            adapter: None,
        };
        assert_eq!(startup_fallback(&broken), Some(GpuPreferences::default()));
        let missing_device = GpuPreferences {
            backend: GraphicsBackend::Automatic,
            adapter: Some("Gone GPU".to_owned()),
        };
        assert_eq!(startup_fallback(&missing_device), Some(GpuPreferences::default()));
    }

    #[test]
    fn automatic_has_nothing_to_fall_back_to() {
        assert_eq!(startup_fallback(&GpuPreferences::default()), None);
    }

    #[test]
    fn only_backends_with_a_device_are_offered() {
        let adapters = [choice("A", Some(GraphicsBackend::Vulkan)), choice("B", None)];
        assert_eq!(
            selectable_backends(&adapters),
            vec![GraphicsBackend::Automatic, GraphicsBackend::Vulkan]
        );
        assert_eq!(selectable_backends(&[]), vec![GraphicsBackend::Automatic]);
    }

    #[test]
    fn directx_12_is_listed_but_automatic_never_opens_it() {
        let adapters = [
            choice("GPU", Some(GraphicsBackend::Vulkan)),
            choice("GPU", Some(GraphicsBackend::DirectX12)),
            choice("Basic Render Driver", Some(GraphicsBackend::DirectX12)),
        ];
        assert_eq!(
            selectable_backends(&adapters),
            vec![
                GraphicsBackend::Automatic,
                GraphicsBackend::Vulkan,
                GraphicsBackend::DirectX12
            ]
        );
        assert!(!GraphicsBackend::DirectX12.is_available());
        assert!(!GraphicsBackend::Automatic.backends().contains(wgpu::Backends::DX12));
        let offered: Vec<&str> = adapters
            .iter()
            .filter(|adapter| GraphicsBackend::Automatic.offers(adapter))
            .map(|adapter| adapter.name.as_str())
            .collect();
        assert_eq!(
            offered,
            ["GPU"],
            "a device only DirectX 12 reaches is not offered under Automatic"
        );
    }
}
