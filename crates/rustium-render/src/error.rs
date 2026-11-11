/// Errors raised while setting up or using the GPU.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The window surface could not be created.
    #[error("cannot create a surface for the window")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    /// No GPU adapter can present to the window.
    #[error("no compatible graphics adapter found")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    /// The adapter refused to create a device.
    #[error("cannot open the graphics device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// The surface reported a validation error while acquiring a frame.
    #[error("the window surface failed validation")]
    SurfaceValidation,
    /// The traced image could not be copied back from the GPU, or nothing has been traced yet.
    #[error("cannot read the rendered image back from the GPU")]
    Readback,
}
