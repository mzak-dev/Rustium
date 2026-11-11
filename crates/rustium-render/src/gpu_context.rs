use tracing::{info, warn};

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

/// GPU choices that take effect when the [`GpuContext`] is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GpuPreferences {
    /// Requested graphics API.
    pub backend: GraphicsBackend,
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
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = match preferences.backend {
            GraphicsBackend::Automatic => wgpu::Backends::PRIMARY,
            GraphicsBackend::Vulkan => wgpu::Backends::VULKAN,
            GraphicsBackend::DirectX12 => wgpu::Backends::DX12,
        };
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(target)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let adapter_info = adapter.get_info();
        info!(
            name = %adapter_info.name,
            backend = ?adapter_info.backend,
            kind = ?adapter_info.device_type,
            "graphics adapter"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rustium device"),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await?;
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
