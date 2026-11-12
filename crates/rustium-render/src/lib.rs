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
mod material_compiler;
mod material_pipelines;
mod materials;
mod offline;
mod path_tracer;
mod resolve;
mod trace_scene;
mod viewport_renderer;

pub use error::Error;
pub use glass_pass::GlassRect;
pub use gpu_context::{GpuContext, GpuPreferences, GraphicsBackend};
pub use material_compiler::{CompileError, CompiledMaterials, PARAM_CAPACITY, compile_materials};
pub use material_pipelines::CompileStatus;
pub use materials::{GpuColor, GpuMaterialTable, MaterialBuffers};
pub use offline::OfflineRenderer;
pub use path_tracer::{
    Ground, PathTracer, RenderPass, RenderStyle, Sky, ThinLens, TraceFrame, TraceImage, TraceProgress,
};
pub use viewport_renderer::{CameraUniform, LineVertex, ModelDraw, ViewportFrame, ViewportRenderer};
