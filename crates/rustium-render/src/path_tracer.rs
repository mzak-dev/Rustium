//! Progressive compute-shader path tracer (ADR 0001): traverses the chunk grid uploaded by [`TraceScene`],
//! accumulates samples into an `Rgba32Float` texture and tonemaps the result into the viewport.

use std::sync::mpsc;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use rustium_core::MaterialGraph;
use tracing::warn;

use crate::error::Error;
use crate::material_compiler::{CompileError, compile_materials, default_compiled};
use crate::materials::MaterialBuffers;
use crate::ray_scene::RayScene;
use crate::resolve::{self, ResolveInput, Resolver};
use crate::trace_scene::TraceScene;

const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const WORKGROUP: u32 = 8;

/// Analytic sky presets (Light tab); the sun colour and softness come with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sky {
    /// Blue sky, warm sun.
    Clear,
    /// Grey, soft light.
    Overcast,
    /// Orange horizon, low red sun.
    Sunset,
    /// Dark blue, a dim cold light.
    Night,
}

impl Sky {
    /// Every preset, in menu order.
    pub const ALL: [Self; 4] = [Self::Clear, Self::Overcast, Self::Sunset, Self::Night];

    /// Linear zenith colour, horizon colour, sun radiance and sun angular radius (radians).
    fn look(self) -> ([f32; 3], [f32; 3], [f32; 3], f32) {
        match self {
            Self::Clear => ([0.16, 0.34, 0.78], [0.62, 0.74, 0.90], [2.2, 2.0, 1.75], 0.035),
            Self::Overcast => ([0.42, 0.45, 0.50], [0.62, 0.64, 0.67], [0.45, 0.45, 0.45], 0.30),
            Self::Sunset => ([0.10, 0.18, 0.42], [0.95, 0.48, 0.24], [3.0, 1.45, 0.6], 0.05),
            Self::Night => ([0.008, 0.014, 0.04], [0.03, 0.045, 0.09], [0.25, 0.32, 0.6], 0.04),
        }
    }
}

/// Which traversal finds the nearest hit and shadow rays; settable in Settings → Performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RayTracing {
    /// Hardware ray query when the adapter has it, the software DDA otherwise.
    #[default]
    Auto,
    /// The software two-level DDA, which runs on every GPU.
    Software,
    /// Hardware ray query (acceleration structures); falls back to software where the adapter lacks it.
    Hardware,
}

/// The traversal a [`PathTracer`] actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceEngine {
    /// Two-level DDA in the compute shader.
    Software,
    /// Hardware ray query over chunk boxes, voxel DDA inside each box.
    Hardware,
}

impl RayTracing {
    /// The engine this preference gives on `device`: hardware only where the device was opened with ray query.
    pub fn resolve(self, device: &wgpu::Device) -> TraceEngine {
        let capable = device.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY);
        if self == Self::Hardware && !capable {
            warn!("hardware ray query requested but the device lacks it, using the software tracer");
        }
        if capable && self != Self::Software {
            TraceEngine::Hardware
        } else {
            TraceEngine::Software
        }
    }
}

/// What the ground below the model looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// No ground: rays going down see the sky.
    None,
    /// A light matte floor that catches shadows and bounce light.
    Studio,
    /// Invisible floor that only shows the shadow the model casts.
    Shadow,
}

/// How the image is shaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStyle {
    /// Full path tracing with the Colors' material graphs.
    Traced,
    /// Every surface the same matte grey, lit by the sun and sky.
    Clay,
    /// Sun light in three flat bands with dark outlines on silhouettes and creases.
    Toon,
    /// Rendered at a coarse resolution and enlarged without smoothing.
    Pixel,
    /// Light lines on the edges between voxel faces over dark faces.
    Wire,
}

impl RenderStyle {
    /// Every style, in the order of the Style row.
    pub const ALL: [Self; 5] = [Self::Traced, Self::Clay, Self::Toon, Self::Pixel, Self::Wire];

    /// The number the shaders use for this style.
    pub(crate) fn index(self) -> u32 {
        match self {
            Self::Traced => 0,
            Self::Clay => 1,
            Self::Toon => 2,
            Self::Pixel => 3,
            Self::Wire => 4,
        }
    }
}

/// Which layer of the render is shown and saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPass {
    /// The finished picture.
    Beauty,
    /// The Color of the first surface each pixel sees, without lighting.
    Albedo,
    /// Surface normals as colours (`xyz * 0.5 + 0.5`).
    Normal,
    /// Distance from the camera, near is bright.
    Depth,
}

impl RenderPass {
    /// Every pass, in menu order.
    pub const ALL: [Self; 4] = [Self::Beauty, Self::Albedo, Self::Normal, Self::Depth];

    pub(crate) fn index(self) -> u32 {
        match self {
            Self::Beauty => 0,
            Self::Albedo => 1,
            Self::Normal => 2,
            Self::Depth => 3,
        }
    }
}

/// A thin lens for depth of field. Lengths are in voxels, the unit of the Scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThinLens {
    /// Camera position.
    pub eye: [f32; 3],
    /// Radius of the aperture; zero is a pinhole.
    pub radius: f32,
    /// Distance from the camera to the plane that is in focus, along the view direction.
    pub focus_distance: f32,
}

/// Everything the path tracer needs for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceFrame {
    /// Column-major clip-to-voxel-space matrix of the camera.
    pub inv_view_proj: [[f32; 4]; 4],
    /// Image size in pixels; the accumulation texture has exactly this size.
    pub size: (u32, u32),
    /// Samples per pixel to accumulate before stopping.
    pub samples: u32,
    /// Most bounces per path.
    pub bounces: u32,
    /// Sun azimuth in degrees.
    pub sun_turn_degrees: f32,
    /// Sun elevation above the horizon in degrees.
    pub sun_height_degrees: f32,
    /// Sky preset.
    pub sky: Sky,
    /// Ground option.
    pub ground: Ground,
    /// Whether pixels that see only the sky get zero coverage.
    pub transparent: bool,
    /// Pixel rectangle `[x0, y0, x1, y1)` to render; `None` renders the whole image.
    pub region: Option<[u32; 4]>,
    /// Whether to keep accumulating; paused frames only redraw.
    pub running: bool,
    /// Where the image is shown on the target, `[x, y, width, height]` in target pixels.
    pub frame_rect: [f32; 4],
    /// Linear exposure multiplier applied before tonemapping.
    pub exposure: f32,
    /// How the image is shaded.
    pub style: RenderStyle,
    /// Whether the accumulated image is denoised for display and readback.
    pub denoise: bool,
    /// Depth of field; `None` renders with a pinhole.
    pub lens: Option<ThinLens>,
    /// Which layer is shown. Passes other than [`RenderPass::Beauty`] are already display-ready colours: they are
    /// shown without exposure and tonemapping, and [`TraceImage::display_ready`] says so for saving.
    pub pass: RenderPass,
}

/// The part of a [`TraceFrame`] that changes what is rendered; any difference restarts accumulation.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TraceKey {
    inv_view_proj: [[f32; 4]; 4],
    size: (u32, u32),
    bounces: u32,
    sun: [f32; 2],
    sky: Sky,
    ground: Ground,
    transparent: bool,
    region: Option<[u32; 4]>,
    style: RenderStyle,
    lens: Option<ThinLens>,
}

impl TraceFrame {
    fn key(&self) -> TraceKey {
        TraceKey {
            inv_view_proj: self.inv_view_proj,
            size: self.size,
            bounces: self.bounces,
            sun: [self.sun_turn_degrees, self.sun_height_degrees],
            sky: self.sky,
            ground: self.ground,
            transparent: self.transparent,
            region: self.region,
            style: self.style,
            lens: self.lens,
        }
    }

    fn resolve_input(&self, samples: u32) -> ResolveInput {
        ResolveInput {
            inv_view_proj: self.inv_view_proj,
            size: self.size,
            style: self.style,
            denoise: self.denoise,
            pass: self.pass,
            samples,
        }
    }
}

/// How far accumulation has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceProgress {
    /// Samples per pixel accumulated so far.
    pub samples: u32,
    /// Samples per pixel the current frame asks for.
    pub target: u32,
    /// Increases every time accumulation restarts.
    pub generation: u64,
}

/// The accumulated image copied back to the CPU.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel the image holds.
    pub samples: u32,
    /// Linear RGBA, premultiplied alpha, row-major from the top-left, `width * height * 4` floats.
    pub rgba: Vec<f32>,
    /// Whether the colours are an AOV pass that must be saved as they are, without exposure and tonemapping.
    pub display_ready: bool,
}

/// Uniform block of `shaders/pathtracer.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    sky_zenith: [f32; 4],
    sky_horizon: [f32; 4],
    image: [u32; 4],
    region: [u32; 4],
    options: [u32; 4],
    ground: [f32; 4],
    style: [u32; 4],
    lens: [f32; 4],
}

const _: () = assert!(size_of::<Params>() == 64 + 11 * 16);

/// Uniform block of `shaders/present.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PresentParams {
    frame: [f32; 4],
    image: [f32; 4],
    letterbox: [f32; 4],
    /// x: 1 when the image is display-ready and skips exposure and tonemapping.
    options: [f32; 4],
}

const _: () = assert!(size_of::<PresentParams>() == 64);

/// First-hit data the trace pass writes for the resolve stage.
#[derive(Debug)]
struct GBuffer {
    geometry: wgpu::Texture,
    albedo: wgpu::Texture,
}

#[derive(Debug)]
struct Accumulation {
    size: (u32, u32),
    textures: [wgpu::Texture; 2],
    gbuffer: GBuffer,
    /// `compute_groups[i]` reads texture `i` and writes texture `1 - i`.
    compute_groups: [wgpu::BindGroup; 2],
    present_group: wgpu::BindGroup,
}

/// The inputs of the resolve stage that are not samples.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ResolveKey {
    size: (u32, u32),
    style: RenderStyle,
    denoise: bool,
    pass: RenderPass,
}

/// The compute pipeline, its accumulation textures and the GPU scene it walks.
#[derive(Debug)]
pub struct PathTracer {
    scene: TraceScene,
    /// Acceleration structures; present exactly when the engine is [`TraceEngine::Hardware`].
    rays: Option<RayScene>,
    materials: MaterialBuffers,
    compute_layout: wgpu::BindGroupLayout,
    compute_pipeline_layout: wgpu::PipelineLayout,
    compute_pipeline: wgpu::ComputePipeline,
    present_layout: wgpu::BindGroupLayout,
    present_pipeline: wgpu::RenderPipeline,
    resolver: Resolver,
    /// What the resolved texture was last produced from; a difference means it is stale.
    resolved_for: Option<ResolveKey>,
    params_buffer: wgpu::Buffer,
    present_buffer: wgpu::Buffer,
    accumulation: Option<Accumulation>,
    /// Scene buffer generation the bind groups were built against.
    bound_generation: u64,
    key: Option<TraceKey>,
    /// Index of the texture holding the latest result.
    current: usize,
    samples_done: u32,
    target_samples: u32,
    generation: u64,
}

impl PathTracer {
    /// Builds the pipelines; `materials` are the buffers shared with the raster pass and `target_format` the format
    /// of the view [`PathTracer::encode`] draws into. The compute pipeline starts out with only the Default material
    /// compiled; call [`PathTracer::set_materials`] (or let the [`ViewportRenderer`](crate::ViewportRenderer) do it).
    pub fn new(
        device: &wgpu::Device,
        materials: &MaterialBuffers,
        target_format: wgpu::TextureFormat,
        ray_tracing: RayTracing,
    ) -> Self {
        let engine = ray_tracing.resolve(device);
        let hardware = engine == TraceEngine::Hardware;
        let present_source = format!(
            "{}{}",
            include_str!("shaders/fullscreen.wgsl"),
            include_str!("shaders/present.wgsl")
        );
        let present_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("path tracer present shader"),
            source: wgpu::ShaderSource::Wgsl(present_source.into()),
        });

        let buffer_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: ACCUM_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let uniform = wgpu::BufferBindingType::Uniform;
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        let mut compute_entries = vec![
            buffer_entry(0, uniform),
            buffer_entry(1, uniform),
            buffer_entry(2, storage),
            buffer_entry(3, storage),
            buffer_entry(6, storage),
            buffer_entry(9, storage),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            storage_texture_entry(5),
            storage_texture_entry(7),
            storage_texture_entry(8),
        ];
        if hardware {
            compute_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::AccelerationStructure { vertex_return: false },
                count: None,
            });
            compute_entries.push(buffer_entry(11, storage));
        }
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("path tracer bind group layout"),
            entries: &compute_entries,
        });
        let compute_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("path tracer pipeline layout"),
            bind_group_layouts: &[Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute_pipeline = create_compute_pipeline(
            device,
            &compute_pipeline_layout,
            &default_compiled().shader_source(),
            engine,
        );

        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("path tracer present bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let present_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("path tracer present pipeline layout"),
            bind_group_layouts: &[Some(&present_layout)],
            immediate_size: 0,
        });
        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("path tracer present pipeline"),
            layout: Some(&present_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &present_module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &present_module,
                entry_point: Some("fs_present"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            scene: TraceScene::new(device),
            rays: hardware.then(|| RayScene::new(device)),
            materials: materials.clone(),
            compute_layout,
            compute_pipeline_layout,
            compute_pipeline,
            present_layout,
            present_pipeline,
            resolver: Resolver::new(device),
            resolved_for: None,
            params_buffer: uniform_buffer("path tracer params", size_of::<Params>()),
            present_buffer: uniform_buffer("path tracer present params", size_of::<PresentParams>()),
            accumulation: None,
            bound_generation: u64::MAX,
            key: None,
            current: 0,
            samples_done: 0,
            target_samples: 0,
            generation: 0,
        }
    }

    /// Uploads every visible Model of `scene` and restarts accumulation. Kept apart from tracing so another backend can
    /// consume the same chunk data.
    pub fn upload_scene(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &rustium_core::Scene) {
        self.scene.upload(device, queue, scene);
        if let Some(rays) = &mut self.rays {
            rays.update(device, queue, &self.scene);
        }
        self.restart();
    }

    /// The traversal this tracer runs.
    pub fn engine(&self) -> TraceEngine {
        if self.rays.is_some() {
            TraceEngine::Hardware
        } else {
            TraceEngine::Software
        }
    }

    /// Counts every replacement of a buffer or acceleration structure the bind groups point at.
    fn scene_generation(&self) -> u64 {
        self.scene.generation() + self.rays.as_ref().map_or(0, RayScene::generation)
    }

    /// The layout the compute pipeline is built against (for rebuilding it on a worker thread).
    pub(crate) fn compute_pipeline_layout(&self) -> &wgpu::PipelineLayout {
        &self.compute_pipeline_layout
    }

    /// Rebuilds the compute pipeline from a complete material module (see `MaterialCompiler::live_source`).
    pub(crate) fn set_material_source(&mut self, device: &wgpu::Device, material_source: &str) {
        self.compute_pipeline =
            create_compute_pipeline(device, &self.compute_pipeline_layout, material_source, self.engine());
    }

    /// Swaps in a compute pipeline built from newly compiled Materials and restarts accumulation.
    pub(crate) fn set_compute_pipeline(&mut self, pipeline: wgpu::ComputePipeline) {
        self.compute_pipeline = pipeline;
        self.restart();
    }

    /// Compiles `graphs`, uploads their constants and swaps in the new pipeline, waiting for the driver. The viewport
    /// does this in the background instead; this is for tools and tests that have no frame to protect.
    ///
    /// # Errors
    ///
    /// [`CompileError`] when the graphs do not compile to valid WGSL.
    pub fn set_materials(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        graphs: &[MaterialGraph],
    ) -> Result<(), CompileError> {
        let compiled = compile_materials(graphs)?;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = create_compute_pipeline(
            device,
            &self.compute_pipeline_layout,
            &compiled.shader_source(),
            self.engine(),
        );
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(CompileError::Shader(error.to_string()));
        }
        self.materials.write_params(queue, &compiled.params);
        self.set_compute_pipeline(pipeline);
        Ok(())
    }

    /// Throws away accumulated samples; call after the Palette changed.
    pub fn restart(&mut self) {
        self.samples_done = 0;
        self.key = None;
        self.generation += 1;
    }

    /// How far accumulation has come.
    pub fn progress(&self) -> TraceProgress {
        TraceProgress {
            samples: self.samples_done,
            target: self.target_samples,
            generation: self.generation,
        }
    }

    // HOT PATH: runs every frame while Render mode is open. Everything else in this file runs on change only.
    /// Accumulates at most one pass of samples and draws the resolved image into `target`.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        frame: &TraceFrame,
    ) {
        let frame = self.prepare(device, frame);
        let passes = (frame.running || self.samples_done == 0) && self.samples_done < self.target_samples;
        let count = u32::from(passes);
        self.write_params(queue, &frame, count);
        if passes {
            self.encode_trace_pass(encoder, frame.size, count);
        }
        self.encode_resolve(queue, encoder, &frame, passes);
        let Some(accumulation) = &self.accumulation else {
            return;
        };
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("path tracer present pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        pass.set_pipeline(&self.present_pipeline);
        pass.set_bind_group(0, &accumulation.present_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Renders `frame` to completion without drawing anything, for saved images and video frames. Samples go in
    /// small batches; `progress` gets the samples done after each one and returns `false` to cancel. The accumulation
    /// is shared with [`PathTracer::encode`], so use a tracer of its own for this.
    ///
    /// # Errors
    ///
    /// [`Error::Readback`] when the image cannot be copied back from the GPU. A cancelled render is `Ok(None)`.
    pub fn render_to_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &TraceFrame,
        mut progress: impl FnMut(u32) -> bool,
    ) -> Result<Option<TraceImage>, Error> {
        /// Samples traced per submission: a few at a time keeps one GPU job short.
        const BATCH: u32 = 4;
        let frame = TraceFrame {
            running: true,
            ..self.prepare(device, frame)
        };
        self.samples_done = 0;
        while self.samples_done < self.target_samples {
            let count = BATCH.min(self.target_samples - self.samples_done);
            self.write_params(queue, &frame, count);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("path tracer offline encoder"),
            });
            self.encode_trace_pass(&mut encoder, frame.size, count);
            queue.submit(std::iter::once(encoder.finish()));
            if let Err(error) = device.poll(wgpu::PollType::wait_indefinitely()) {
                warn!(%error, "waiting for the GPU failed");
                return Err(Error::Readback);
            }
            if !progress(self.samples_done) {
                return Ok(None);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("path tracer resolve encoder"),
        });
        self.encode_resolve(queue, &mut encoder, &frame, true);
        queue.submit(std::iter::once(encoder.finish()));
        self.read_image(device, queue).map(Some)
    }

    /// Sizes the accumulation to the frame, restarts it when anything that changes the picture changed and returns the
    /// frame with its size clamped to what the device can hold.
    fn prepare(&mut self, device: &wgpu::Device, frame: &TraceFrame) -> TraceFrame {
        let max_dimension = device.limits().max_texture_dimension_2d;
        let size = (
            frame.size.0.clamp(1, max_dimension),
            frame.size.1.clamp(1, max_dimension),
        );
        let frame = TraceFrame { size, ..*frame };
        if self
            .accumulation
            .as_ref()
            .is_none_or(|accumulation| accumulation.size != size)
        {
            self.accumulation = Some(self.create_accumulation(device, size));
            self.bound_generation = self.scene_generation();
            self.resolved_for = None;
            self.restart();
        } else if self.bound_generation != self.scene_generation() {
            self.rebuild_groups(device);
        }
        let key = frame.key();
        if self.key != Some(key) {
            self.key = Some(key);
            self.samples_done = 0;
            self.generation += 1;
        }
        self.target_samples = frame.samples.max(1);
        frame
    }

    /// Uploads the uniforms of the trace pass (`count` samples) and of the present pass.
    fn write_params(&self, queue: &wgpu::Queue, frame: &TraceFrame, count: u32) {
        let size = frame.size;
        let region = frame.region.map_or([0, 0, size.0, size.1], |region| {
            [
                region[0].min(size.0),
                region[1].min(size.1),
                region[2].min(size.0),
                region[3].min(size.1),
            ]
        });
        let (zenith, horizon, sun_color, sun_radius) = frame.sky.look();
        let sun = sun_direction(frame.sun_turn_degrees, frame.sun_height_degrees);
        let lens = frame.lens.unwrap_or(ThinLens {
            eye: [0.0; 3],
            radius: 0.0,
            focus_distance: 1.0,
        });
        let params = Params {
            inv_view_proj: frame.inv_view_proj,
            eye: [lens.eye[0], lens.eye[1], lens.eye[2], 0.0],
            sun_dir: [sun.x, sun.y, sun.z, sun_radius],
            sun_color: [sun_color[0], sun_color[1], sun_color[2], 0.0],
            sky_zenith: [zenith[0], zenith[1], zenith[2], 0.0],
            sky_horizon: [horizon[0], horizon[1], horizon[2], 0.0],
            image: [size.0, size.1, self.samples_done, frame.bounces],
            region,
            options: [
                match frame.ground {
                    Ground::None => 0,
                    Ground::Studio => 1,
                    Ground::Shadow => 2,
                },
                u32::from(frame.transparent),
                count,
                self.scene.model_count(),
            ],
            ground: [0.45, 0.45, 0.44, 0.0],
            style: [
                frame.style.index(),
                resolve::pixel_block(size.1),
                u32::from(self.samples_done == 0),
                0,
            ],
            lens: [
                lens.radius,
                lens.focus_distance,
                f32::from(u8::from(frame.lens.is_some_and(|lens| lens.radius > 0.0))),
                0.0,
            ],
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
        let present = PresentParams {
            frame: frame.frame_rect,
            image: [
                size.0 as f32,
                size.1 as f32,
                frame.exposure,
                f32::from(u8::from(frame.transparent)),
            ],
            letterbox: [0.006, 0.007, 0.009, 1.0],
            options: [f32::from(u8::from(frame.pass != RenderPass::Beauty)), 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.present_buffer, 0, bytemuck::bytes_of(&present));
    }

    /// Adds `count` samples per pixel to the accumulation.
    fn encode_trace_pass(&mut self, encoder: &mut wgpu::CommandEncoder, size: (u32, u32), count: u32) {
        let Some(accumulation) = &self.accumulation else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("path tracer pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.compute_pipeline);
        pass.set_bind_group(0, &accumulation.compute_groups[self.current], &[]);
        pass.dispatch_workgroups(size.0.div_ceil(WORKGROUP), size.1.div_ceil(WORKGROUP), 1);
        drop(pass);
        self.current = 1 - self.current;
        self.samples_done += count;
    }

    /// Refreshes the resolved image when new samples arrived or the style or denoise setting changed.
    fn encode_resolve(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &TraceFrame,
        fresh: bool,
    ) {
        let key = ResolveKey {
            size: frame.size,
            style: frame.style,
            denoise: frame.denoise,
            pass: frame.pass,
        };
        if !fresh && self.resolved_for == Some(key) {
            return;
        }
        self.resolved_for = Some(key);
        self.resolver
            .encode(queue, encoder, self.current, &frame.resolve_input(self.samples_done));
    }

    /// Copies the resolved image (denoised and styled as the last frame asked) to the CPU, blocking until the GPU has
    /// finished.
    ///
    /// # Errors
    ///
    /// [`Error::Readback`] when nothing has been rendered yet or the GPU read fails.
    pub fn read_image(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<TraceImage, Error> {
        let accumulation = self.accumulation.as_ref().ok_or(Error::Readback)?;
        let resolved = self.resolver.resolved().ok_or(Error::Readback)?;
        if self.samples_done == 0 {
            return Err(Error::Readback);
        }
        let (width, height) = accumulation.size;
        let row_bytes = width * 16;
        let padded = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path tracer readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("path tracer readback encoder"),
        });
        encoder.copy_texture_to_buffer(
            resolved.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));
        let (sender, receiver) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        if let Err(error) = device.poll(wgpu::PollType::wait_indefinitely()) {
            warn!(%error, "waiting for the readback failed");
            return Err(Error::Readback);
        }
        receiver
            .recv()
            .map_err(|_| Error::Readback)?
            .map_err(|_| Error::Readback)?;
        let mapped = buffer.slice(..).get_mapped_range().map_err(|_| Error::Readback)?;
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for row in mapped.chunks(padded as usize).take(height as usize) {
            let texels: &[f32] = bytemuck::cast_slice(row.get(..row_bytes as usize).ok_or(Error::Readback)?);
            rgba.extend_from_slice(texels);
        }
        drop(mapped);
        buffer.unmap();
        Ok(TraceImage {
            width,
            height,
            samples: self.samples_done,
            rgba,
            display_ready: self.resolved_for.is_some_and(|key| key.pass != RenderPass::Beauty),
        })
    }

    fn create_accumulation(&mut self, device: &wgpu::Device, size: (u32, u32)) -> Accumulation {
        let texture = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: ACCUM_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let textures = [
            texture("path tracer accumulation A"),
            texture("path tracer accumulation B"),
        ];
        let gbuffer = GBuffer {
            geometry: texture("path tracer G-buffer geometry"),
            albedo: texture("path tracer G-buffer albedo"),
        };
        let resolved = self
            .resolver
            .bind(device, size, &textures, &gbuffer.geometry, &gbuffer.albedo);
        let compute_groups = self.compute_groups_for(device, &textures, &gbuffer);
        let present_group = self.present_group(device, &resolved);
        Accumulation {
            size,
            textures,
            gbuffer,
            compute_groups,
            present_group,
        }
    }

    fn rebuild_groups(&mut self, device: &wgpu::Device) {
        if let Some(accumulation) = self.accumulation.take() {
            let compute_groups = self.compute_groups_for(device, &accumulation.textures, &accumulation.gbuffer);
            self.accumulation = Some(Accumulation {
                compute_groups,
                ..accumulation
            });
        }
        self.bound_generation = self.scene_generation();
    }

    fn compute_groups_for(
        &self,
        device: &wgpu::Device,
        textures: &[wgpu::Texture; 2],
        gbuffer: &GBuffer,
    ) -> [wgpu::BindGroup; 2] {
        let views = textures
            .each_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let geometry_view = gbuffer.geometry.create_view(&wgpu::TextureViewDescriptor::default());
        let albedo_view = gbuffer.albedo.create_view(&wgpu::TextureViewDescriptor::default());
        let compute = |read: usize| {
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.materials.colors().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.scene.pool().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.scene.grids().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.scene.models().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: self.materials.params().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&views[read]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&views[1 - read]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&geometry_view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&albedo_view),
                },
            ];
            if let Some(rays) = &self.rays {
                entries.push(wgpu::BindGroupEntry {
                    binding: 10,
                    resource: rays.tlas().as_binding(),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: 11,
                    resource: rays.chunk_boxes().as_entire_binding(),
                });
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("path tracer bind group"),
                layout: &self.compute_layout,
                entries: &entries,
            })
        };
        [compute(0), compute(1)]
    }

    /// The bind group that shows the resolved image.
    fn present_group(&self, device: &wgpu::Device, resolved: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("path tracer present bind group"),
            layout: &self.present_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.present_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(resolved),
                },
            ],
        })
    }
}

/// The full WGSL of the path tracer: material module, the tracer, and the traversal of `engine`. The one place the
/// shader is composed, so the Material rebuild and the engine switch cannot disagree.
pub(crate) fn compute_shader_source(material_source: &str, engine: TraceEngine) -> String {
    let (prefix, traversal) = match engine {
        TraceEngine::Software => ("", include_str!("shaders/trace_dda.wgsl")),
        // `enable` directives must come before any declaration.
        TraceEngine::Hardware => ("enable wgpu_ray_query;\n", include_str!("shaders/trace_rq.wgsl")),
    };
    format!(
        "{prefix}{material_source}{}{}{traversal}",
        include_str!("shaders/pathtracer.wgsl"),
        include_str!("shaders/trace_style.wgsl")
    )
}

pub(crate) fn create_compute_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    material_source: &str,
    engine: TraceEngine,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("path tracer shader"),
        source: wgpu::ShaderSource::Wgsl(compute_shader_source(material_source, engine).into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("path tracer pipeline"),
        layout: Some(layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

/// Unit vector towards the sun for an azimuth (`turn`) and elevation (`height`) in degrees.
fn sun_direction(turn_degrees: f32, height_degrees: f32) -> Vec3 {
    let turn = turn_degrees.to_radians();
    let height = height_degrees.clamp(0.0, 90.0).to_radians();
    Vec3::new(height.cos() * turn.sin(), height.sin(), height.cos() * turn.cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_height_zero_is_on_the_horizon_and_ninety_is_overhead() {
        assert!(sun_direction(135.0, 0.0).y.abs() < 1e-6);
        assert!((sun_direction(0.0, 90.0).y - 1.0).abs() < 1e-6);
        assert!((sun_direction(10.0, 42.0).length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn only_rendering_inputs_restart_accumulation() {
        let frame = TraceFrame {
            inv_view_proj: [[0.0; 4]; 4],
            size: (64, 64),
            samples: 128,
            bounces: 4,
            sun_turn_degrees: 135.0,
            sun_height_degrees: 42.0,
            sky: Sky::Clear,
            ground: Ground::Studio,
            transparent: false,
            region: None,
            running: true,
            frame_rect: [0.0; 4],
            exposure: 1.0,
            style: RenderStyle::Traced,
            denoise: false,
            lens: None,
            pass: RenderPass::Beauty,
        };
        let paused = TraceFrame {
            running: false,
            samples: 512,
            exposure: 2.0,
            frame_rect: [1.0, 2.0, 3.0, 4.0],
            ..frame
        };
        assert_eq!(frame.key(), paused.key());
        let brighter_sun = TraceFrame {
            sun_height_degrees: 50.0,
            ..frame
        };
        assert_ne!(frame.key(), brighter_sun.key());
        let denoised = TraceFrame { denoise: true, ..frame };
        assert_eq!(frame.key(), denoised.key(), "denoising only changes the resolve stage");
        let clay = TraceFrame {
            style: RenderStyle::Clay,
            ..frame
        };
        assert_ne!(frame.key(), clay.key());
    }
}
