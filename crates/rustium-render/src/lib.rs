//! wgpu renderer for the Rustium viewport.
//!
//! [`GpuContext`] owns the device and the window surface. [`ViewportRenderer`] draws the voxel Scene
//! (greedy-meshed chunks from `rustium-core`), the ground grid and overlay lines into an offscreen target with
//! optional MSAA, blurs it for glass panels and composites the result onto the surface, leaving room for the
//! interface to be drawn on top.
#![warn(clippy::panic, clippy::expect_used)]

mod error;
mod glass_pass;
mod gpu_context;
mod viewport_renderer;

pub use error::Error;
pub use glass_pass::GlassRect;
pub use gpu_context::{GpuContext, GpuPreferences, GraphicsBackend};
pub use viewport_renderer::{CameraUniform, LineVertex, ModelDraw, ViewportFrame, ViewportRenderer};
