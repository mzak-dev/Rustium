//! A path tracer that renders finished images on a worker thread, apart from the one that draws the viewport.

use rustium_core::Scene;

use crate::error::Error;
use crate::path_tracer::{PathTracer, TraceFrame, TraceImage};

/// Renders [`TraceFrame`]s to CPU images without touching the viewport's accumulation. Built by
/// `ViewportRenderer::offline_renderer` and then moved to a worker thread.
#[derive(Debug)]
pub struct OfflineRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    tracer: PathTracer,
}

impl OfflineRenderer {
    pub(crate) fn new(device: wgpu::Device, queue: wgpu::Queue, tracer: PathTracer) -> Self {
        Self { device, queue, tracer }
    }

    /// Uploads every visible Model of `scene`; call again after the Scene changed.
    pub fn upload_scene(&mut self, scene: &Scene) {
        self.tracer.upload_scene(&self.device, &self.queue, scene);
    }

    /// Renders `frame` to completion. `progress` gets the samples per pixel done so far and returns `false` to cancel,
    /// which makes the result `Ok(None)`.
    ///
    /// # Errors
    ///
    /// [`Error::Readback`] when the GPU fails to hand the image back.
    pub fn render(
        &mut self,
        frame: &TraceFrame,
        progress: impl FnMut(u32) -> bool,
    ) -> Result<Option<TraceImage>, Error> {
        self.tracer.render_to_image(&self.device, &self.queue, frame, progress)
    }
}
