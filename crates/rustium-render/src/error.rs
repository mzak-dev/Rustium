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
    /// The image is bigger than the GPU can hold.
    #[error(
        "a {width} × {height} image needs {needed} bytes of GPU memory; at most {allowed} bytes and {max_side} pixels per side are allowed"
    )]
    ImageTooLarge {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
        /// Bytes the image's textures take.
        needed: u64,
        /// Bytes allowed for one image.
        allowed: u64,
        /// Largest side the device accepts.
        max_side: u32,
    },
    /// The GPU ran out of memory while allocating the image.
    #[error("the GPU ran out of memory while allocating the image")]
    OutOfMemory,
}
