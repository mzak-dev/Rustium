use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rustium_core::{ChunkCoord, ChunkMesh, MaterialGraph, MeshVertex, Palette, Placement, Scene, ShapeId};
use wgpu::util::DeviceExt;

use crate::error::Error;
use crate::glass_pass::{GlassPass, GlassRect};
use crate::gpu_context::GpuContext;
use crate::material_compiler::{compile_materials, default_compiled};
use crate::material_pipelines::{BuildContext, CompileStatus, MaterialCompiler};
use crate::materials::{GpuMaterialTable, MaterialBuffers, is_transparent};
use crate::offline::OfflineRenderer;
use crate::path_tracer::{PathTracer, RayTracing, TraceEngine, TraceFrame, TraceImage, TraceProgress};
use crate::taa::{self, TaaPass, TaaTargets, TaaUniform};

/// Camera and lighting data shared by every scene shader (`shaders/common.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct CameraUniform {
    /// Column-major view-projection matrix from voxel space to clip space.
    pub view_proj: [[f32; 4]; 4],
    /// Camera position in voxel space (w unused).
    pub eye: [f32; 4],
    /// Unit vector towards the sun (w unused).
    pub sun: [f32; 4],
    /// x: major grid spacing in voxels, y: grid half-extent, zw: grid centre on x and z.
    pub grid: [f32; 4],
}

const _: () = assert!(size_of::<CameraUniform>() == 112);

/// One end of an overlay line (Volume bounds, hover box).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct LineVertex {
    /// Position in voxel space.
    pub position: [f32; 3],
    /// Linear RGBA color.
    pub color: [f32; 4],
}

const _: () = assert!(size_of::<LineVertex>() == 28);

/// One Model to draw this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelDraw {
    /// Which Shape to draw, where and in what drawing order.
    pub placement: Placement,
    /// How strongly the Model is dimmed, from 0 (normal) to 1 (fully dimmed); used for everything outside the Focus.
    pub dim: f32,
}

/// Per-Model data read by the raster vertex shader (`shaders/raster.wgsl`).
///
/// The three columns hold the images of the X, Y and Z axes and `origin` the translation; the `w` components carry
/// the dim factor (`column_x`) and the drawing rank (`column_y`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct InstanceData {
    column_x: [f32; 4],
    column_y: [f32; 4],
    column_z: [f32; 4],
    origin: [f32; 4],
}

const _: () = assert!(size_of::<InstanceData>() == 64);

impl InstanceData {
    fn new(model: &ModelDraw) -> Self {
        let [x, y, z] = model
            .placement
            .transform
            .orientation
            .columns()
            .map(glam::IVec3::as_vec3);
        let origin = model.placement.transform.offset.as_vec3();
        // The rank only needs to separate neighbours in drawing order, so f32 precision is plenty.
        let rank = model.placement.rank as f32;
        Self {
            column_x: [x.x, x.y, x.z, model.dim],
            column_y: [y.x, y.y, y.z, rank],
            column_z: [z.x, z.y, z.z, 0.0],
            origin: [origin.x, origin.y, origin.z, 0.0],
        }
    }
}

/// Everything the viewport needs to draw one frame.
#[derive(Debug, Clone, Copy)]
pub struct ViewportFrame<'frame> {
    /// The Models to draw, in drawing order.
    pub models: &'frame [ModelDraw],
    /// Camera and lighting.
    pub camera: CameraUniform,
    /// Background color as linear RGB.
    pub background: [f32; 3],
    /// Whether to draw the ground grid.
    pub show_grid: bool,
    /// Overlay line list, two vertices per line.
    pub lines: &'frame [LineVertex],
    /// Panels that show the viewport blurred behind them.
    pub glass: &'frame [GlassRect],
    /// Whether glass panels blur (Settings → Interface → Translucent panels).
    pub blur: bool,
    /// When set, the path tracer draws the viewport instead of the raster pass.
    pub trace: Option<TraceFrame>,
}

#[derive(Debug)]
struct GpuChunk {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

#[derive(Debug)]
struct SceneTargets {
    size: (u32, u32),
    multisampled: Option<wgpu::TextureView>,
    color: wgpu::TextureView,
    /// Where the scene pass resolves to while temporal anti-aliasing is on; the TAA pass turns it into `color`.
    raw: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// What the temporal anti-aliasing of the raster viewport remembers between frames.
#[derive(Debug)]
struct TaaState {
    pass: TaaPass,
    /// Frames drawn with TAA so far; moves the jitter and the dither pattern.
    frame: u32,
    /// Frames since the camera or the Scene last changed; the image has settled after [`SETTLE_FRAMES`].
    still_frames: u32,
    /// Whether the last frame was resolved with TAA (so the history holds it).
    was_active: bool,
    /// The view-projection of the last frame, without jitter, and the eye.
    previous: Option<([[f32; 4]; 4], [f32; 4])>,
    /// Edits or Material changes since the last frame: the history is trusted less for one frame.
    edited: bool,
}

/// Weight of the new frame in the history blend while the view changes: about the last ten frames count.
const TAA_BLEND: f32 = 0.1;
/// The same right after an edit: the changed pixels settle fast and stale ones fade.
const TAA_BLEND_AFTER_EDIT: f32 = 0.35;
/// Frames the image needs to settle after a change (the dither pattern repeats every few frames).
const SETTLE_FRAMES: u32 = 40;
/// The smallest weight of the new frame once the image has been still for a while.
const TAA_BLEND_STILL: f32 = 1.0 / 32.0;
/// An eye farther from last frame's eye than this (in voxels) is a camera cut: the history is dropped.
const TAA_CUT_DISTANCE: f32 = 48.0;

#[derive(Debug)]
struct Pipelines {
    raster: wgpu::RenderPipeline,
    raster_mirrored: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
}

const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Draws the voxel Scene, grid and overlays, then composites them with glass blur onto the surface.
#[derive(Debug)]
pub struct ViewportRenderer {
    sample_count: u32,
    camera_buffer: wgpu::Buffer,
    materials: MaterialBuffers,
    compiler: MaterialCompiler,
    scene_group: wgpu::BindGroup,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: Pipelines,
    line_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    chunks: HashMap<ShapeId, HashMap<ChunkCoord, GpuChunk>>,
    targets: Option<SceneTargets>,
    glass: GlassPass,
    tracer: PathTracer,
    /// The traversal the user asked for; applies to the viewport tracer and to offline renderers.
    ray_tracing: RayTracing,
    dither_buffer: wgpu::Buffer,
    taa: TaaState,
    /// Whether a Color of the Palette uses a Material that lets light through (glass, haze).
    has_transparency: bool,
}

impl ViewportRenderer {
    /// Creates the pipelines for `sample_count` MSAA samples (1 = off).
    pub fn new(gpu: &GpuContext, sample_count: u32) -> Self {
        let device = gpu.device();
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let materials = MaterialBuffers::new(device);
        let dither_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dither uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_layout = create_scene_layout(device);
        let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene bind group"),
            layout: &scene_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: materials.colors().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dither_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: materials.params().as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene pipeline layout"),
            bind_group_layouts: &[Some(&scene_layout)],
            immediate_size: 0,
        });
        let initial = default_compiled();
        materials.write_params(gpu.queue(), &initial.params);
        let pipelines = create_pipelines(device, &pipeline_layout, sample_count, &initial.shader_source());
        let line_buffer = create_line_buffer(device, 256);
        let instance_buffer = create_instance_buffer(device, 64);
        let glass = GlassPass::new(device, gpu.surface_srgb_format());
        let tracer = PathTracer::new(device, &materials, SCENE_FORMAT, RayTracing::Auto);
        let compiler = MaterialCompiler::new(materials.clone(), initial.generated);
        Self {
            sample_count,
            camera_buffer,
            materials,
            compiler,
            scene_group,
            pipeline_layout,
            pipelines,
            line_buffer,
            instance_buffer,
            chunks: HashMap::new(),
            targets: None,
            glass,
            tracer,
            ray_tracing: RayTracing::Auto,
            dither_buffer,
            taa: TaaState {
                pass: TaaPass::new(device, SCENE_FORMAT, sample_count > 1),
                frame: 0,
                still_frames: 0,
                was_active: false,
                previous: None,
                edited: false,
            },
            has_transparency: false,
        }
    }

    /// Current MSAA sample count.
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// Switches MSAA; the count must be one the adapter supports (see [`GpuContext::supported_sample_counts`]).
    pub fn set_sample_count(&mut self, gpu: &GpuContext, sample_count: u32) {
        if sample_count == self.sample_count {
            return;
        }
        self.sample_count = sample_count;
        self.pipelines = create_pipelines(
            gpu.device(),
            &self.pipeline_layout,
            sample_count,
            &self.compiler.live_source(),
        );
        let context = self.build_context(gpu);
        self.compiler.restart_pending(&context);
        self.targets = None;
        self.taa.pass = TaaPass::new(gpu.device(), SCENE_FORMAT, sample_count > 1);
        self.taa.was_active = false;
    }

    /// Uploads the Colors (sRGB converted to linear) and compiles the Material graphs for the raster pass and the
    /// path tracer, and restarts path tracing.
    ///
    /// Editing a number inside a graph only rewrites the constants buffer. Changing a graph's shape builds new
    /// pipelines on a worker thread: [`ViewportRenderer::material_status`] reads `Compiling` meanwhile, the previous
    /// pipelines keep drawing, and [`ViewportRenderer::poll_materials`] (also called by `render`) swaps the new ones
    /// in. A graph that does not compile leaves the old pipelines in place and shows [`CompileStatus::Error`].
    pub fn set_materials(&mut self, gpu: &GpuContext, palette: &Palette, graphs: &[MaterialGraph]) {
        let table = GpuMaterialTable::new(palette, graphs);
        self.has_transparency = palette
            .iter()
            .any(|(_, color)| graphs.get(color.graph as usize).is_some_and(is_transparent));
        self.taa.edited = true;
        let context = self.build_context(gpu);
        self.compiler
            .request(gpu.queue(), &context, compile_materials(graphs), &table);
        self.tracer.restart();
    }

    /// Whether the raster image is still settling: glass and haze are dithered and averaged over a few frames, so the
    /// app should keep drawing until this is `false` (it is `false` whenever nothing transparent is on screen).
    pub fn is_settling(&self) -> bool {
        self.has_transparency && self.taa.was_active && self.taa.still_frames < SETTLE_FRAMES
    }

    /// Whether the Material pipelines match the Scene's graphs (the editor's "Compiled" chip).
    pub fn material_status(&self) -> &CompileStatus {
        self.compiler.status()
    }

    /// Swaps in Material pipelines whose background build has finished; returns whether it did. Cheap when idle.
    pub fn poll_materials(&mut self, gpu: &GpuContext) -> bool {
        let Some(built) = self.compiler.poll(gpu.queue()) else {
            return false;
        };
        self.pipelines.raster = built.raster;
        self.pipelines.raster_mirrored = built.raster_mirrored;
        self.tracer.set_compute_pipeline(built.compute);
        true
    }

    fn build_context(&self, gpu: &GpuContext) -> BuildContext {
        BuildContext {
            device: gpu.device().clone(),
            raster_layout: self.pipeline_layout.clone(),
            compute_layout: self.tracer.compute_pipeline_layout().clone(),
            sample_count: self.sample_count,
            engine: self.tracer.engine(),
        }
    }

    /// The traversal the viewport's path tracer runs.
    pub fn trace_engine(&self) -> TraceEngine {
        self.tracer.engine()
    }

    /// Picks the path tracer's traversal. Returns `true` when that replaced the tracer, which then holds no Scene:
    /// call [`ViewportRenderer::upload_trace_scene`] again. The new tracer builds its pipeline from the live Material
    /// source, and a build in flight for the old engine is restarted.
    pub fn set_ray_tracing(&mut self, gpu: &GpuContext, ray_tracing: RayTracing) -> bool {
        self.ray_tracing = ray_tracing;
        if ray_tracing.resolve(gpu.device()) == self.tracer.engine() {
            return false;
        }
        self.tracer = PathTracer::new(gpu.device(), &self.materials, SCENE_FORMAT, ray_tracing);
        self.tracer
            .set_material_source(gpu.device(), &self.compiler.live_source());
        let context = self.build_context(gpu);
        self.compiler.restart_pending(&context);
        true
    }

    /// Uploads every visible Model's Shape for the path tracer and restarts it. Cheap for unchanged chunks.
    pub fn upload_trace_scene(&mut self, gpu: &GpuContext, scene: &Scene) {
        self.tracer.upload_scene(gpu.device(), gpu.queue(), scene);
    }

    /// A path tracer of its own for saving images and video frames on a worker thread. It gets a snapshot of the
    /// Colors and Material constants and the shader the viewport runs right now (later Material edits do not reach
    /// it), and holds no Scene until [`OfflineRenderer::upload_scene`] is called.
    pub fn offline_renderer(&self, gpu: &GpuContext) -> OfflineRenderer {
        let materials = self.materials.snapshot(gpu.device(), gpu.queue());
        let mut tracer = PathTracer::new(gpu.device(), &materials, SCENE_FORMAT, self.ray_tracing);
        tracer.set_material_source(gpu.device(), &self.compiler.live_source());
        OfflineRenderer::new(gpu.device().clone(), gpu.queue().clone(), tracer)
    }

    /// Throws away the accumulated path-traced samples and starts over.
    pub fn restart_trace(&mut self) {
        self.tracer.restart();
    }

    /// How many samples per pixel the path tracer has accumulated.
    pub fn trace_progress(&self) -> TraceProgress {
        self.tracer.progress()
    }

    /// Copies the accumulated path-traced image to the CPU.
    ///
    /// # Errors
    ///
    /// [`Error::Readback`] when nothing has been traced yet or the GPU read fails.
    pub fn read_trace_image(&self, gpu: &GpuContext) -> Result<TraceImage, Error> {
        self.tracer.read_image(gpu.device(), gpu.queue())
    }

    /// Replaces the GPU mesh of one chunk of `shape`; an empty mesh removes it.
    // ponytail: new buffers per upload; switch to a pooled allocator if strokes show allocation stalls.
    pub fn upload_chunk(&mut self, gpu: &GpuContext, shape: ShapeId, coord: ChunkCoord, mesh: &ChunkMesh) {
        let Ok(index_count) = u32::try_from(mesh.indices.len()) else {
            return;
        };
        self.taa.edited = true;
        if index_count == 0 {
            if let Some(chunks) = self.chunks.get_mut(&shape) {
                chunks.remove(&coord);
            }
            return;
        }
        let device = gpu.device();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk vertices"),
            contents: bytemuck::cast_slice::<MeshVertex, u8>(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.chunks.entry(shape).or_default().insert(
            coord,
            GpuChunk {
                vertices,
                indices,
                index_count,
            },
        );
    }

    /// Drops every chunk mesh.
    pub fn clear_chunks(&mut self) {
        self.taa.edited = true;
        self.chunks.clear();
    }

    /// Draws `frame` and composites it onto `surface`; the interface is drawn afterwards with `LoadOp::Load`.
    pub fn render(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        surface: &wgpu::TextureView,
        frame: &ViewportFrame<'_>,
    ) {
        self.poll_materials(gpu);
        let size = gpu.size();
        if self.targets.as_ref().is_none_or(|targets| targets.size != size) {
            let targets = create_targets(gpu.device(), size, self.sample_count);
            self.glass.resize(gpu.device(), gpu.queue(), &targets.color, size);
            self.targets = Some(targets);
        }
        if self.has_transparency && !self.taa.pass.has_history_for(size) {
            self.taa.pass.resize(gpu.device(), size);
            self.taa.was_active = false;
        }
        let Some(targets) = &self.targets else {
            return;
        };
        let queue = gpu.queue();
        let temporal = self.has_transparency && frame.trace.is_none();
        let mut camera = frame.camera;
        let mut taa_uniform = None;
        if temporal {
            let state = &mut self.taa;
            let cut = state.previous.is_none_or(|(_, eye)| {
                let moved = glam::Vec3::from_array([eye[0], eye[1], eye[2]])
                    - glam::Vec3::from_array([frame.camera.eye[0], frame.camera.eye[1], frame.camera.eye[2]]);
                moved.length() > TAA_CUT_DISTANCE
            });
            let history = state.was_active && !cut;
            let changed = state
                .previous
                .is_none_or(|(view_proj, _)| view_proj != frame.camera.view_proj)
                || state.edited;
            state.still_frames = if changed {
                0
            } else {
                state.still_frames.saturating_add(1)
            };
            // While nothing moves the memory grows (1/n down to 1/32), so the dither averages out completely.
            let blend = if state.edited {
                TAA_BLEND_AFTER_EDIT
            } else {
                (1.0 / (state.still_frames as f32 + 3.0)).clamp(TAA_BLEND_STILL, TAA_BLEND)
            };
            let unjittered = glam::Mat4::from_cols_array_2d(&frame.camera.view_proj);
            camera.view_proj = taa::jittered(frame.camera.view_proj, taa::jitter(state.frame), size);
            taa_uniform = Some(TaaUniform {
                prev_view_proj: state
                    .previous
                    .map_or(frame.camera.view_proj, |(view_proj, _)| view_proj),
                inv_view_proj: unjittered.inverse().to_cols_array_2d(),
                settings: [blend, f32::from(u8::from(!history)), size.0 as f32, size.1 as f32],
            });
            state.previous = Some((frame.camera.view_proj, frame.camera.eye));
            state.edited = false;
        } else {
            self.taa.was_active = false;
            self.taa.previous = None;
        }
        // The dither pattern changes every frame only while the temporal pass averages the frames.
        let dither_frame = if temporal { (self.taa.frame % 4096) as f32 } else { 0.0 };
        queue.write_buffer(
            &self.dither_buffer,
            0,
            bytemuck::bytes_of(&[dither_frame, 0.0_f32, 0.0, 0.0]),
        );
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
        let line_bytes = bytemuck::cast_slice::<LineVertex, u8>(frame.lines);
        if line_bytes.len() as u64 > self.line_buffer.size() {
            self.line_buffer = create_line_buffer(gpu.device(), frame.lines.len().next_power_of_two());
        }
        queue.write_buffer(&self.line_buffer, 0, line_bytes);
        let instances: Vec<InstanceData> = frame.models.iter().map(InstanceData::new).collect();
        if instances.len() * size_of::<InstanceData>() > usize::try_from(self.instance_buffer.size()).unwrap_or(0) {
            self.instance_buffer = create_instance_buffer(gpu.device(), instances.len().next_power_of_two());
        }
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));

        if let Some(trace) = &frame.trace {
            self.tracer.encode(gpu.device(), queue, encoder, &targets.color, trace);
            self.glass.encode(queue, encoder, surface, frame.glass, frame.blur);
            return;
        }
        // With temporal anti-aliasing the scene goes to `raw` first and the TAA pass writes `color`.
        let scene_color = if temporal { &targets.raw } else { &targets.color };
        let (view, resolve_target) = match &targets.multisampled {
            Some(multisampled) => (multisampled, Some(scene_color)),
            None => (scene_color, None),
        };
        let [r, g, b] = frame.background.map(f64::from);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: if temporal {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.scene_group, &[]);
            pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            for (index, model) in frame.models.iter().enumerate() {
                let (Some(chunks), Ok(index)) = (self.chunks.get(&model.placement.shape), u32::try_from(index)) else {
                    continue;
                };
                // A mirror flips the winding of every face, so mirrored Models need the opposite front face.
                let pipeline = if model.placement.transform.orientation.is_mirrored() {
                    &self.pipelines.raster_mirrored
                } else {
                    &self.pipelines.raster
                };
                pass.set_pipeline(pipeline);
                for chunk in chunks.values() {
                    pass.set_vertex_buffer(0, chunk.vertices.slice(..));
                    pass.set_index_buffer(chunk.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..chunk.index_count, 0, index..index + 1);
                }
            }
            if frame.show_grid {
                pass.set_pipeline(&self.pipelines.grid);
                pass.draw(0..6, 0..1);
            }
            if let Ok(line_count) = u32::try_from(frame.lines.len())
                && line_count > 0
            {
                pass.set_pipeline(&self.pipelines.lines);
                pass.set_vertex_buffer(0, self.line_buffer.slice(..));
                pass.draw(0..line_count, 0..1);
            }
        }
        if let Some(uniform) = &taa_uniform {
            self.taa.pass.encode(
                gpu.device(),
                queue,
                encoder,
                &TaaTargets {
                    raw: &targets.raw,
                    depth: &targets.depth,
                    output: &targets.color,
                },
                uniform,
            );
            self.taa.frame = self.taa.frame.wrapping_add(1);
            self.taa.was_active = true;
        }
        self.glass.encode(queue, encoder, surface, frame.glass, frame.blur);
    }
}

fn create_line_buffer(device: &wgpu::Device, vertices: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay lines"),
        size: (vertices.max(2) * size_of::<LineVertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_instance_buffer(device: &wgpu::Device, instances: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model instances"),
        size: (instances.max(1) * size_of::<InstanceData>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_targets(device: &wgpu::Device, size: (u32, u32), sample_count: u32) -> SceneTargets {
    let extent = wgpu::Extent3d {
        width: size.0.max(1),
        height: size.1.max(1),
        depth_or_array_layers: 1,
    };
    let texture = |label, format, samples, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    let color = texture(
        "scene color",
        SCENE_FORMAT,
        1,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let raw = texture(
        "scene color raw",
        SCENE_FORMAT,
        1,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let multisampled = (sample_count > 1).then(|| {
        texture(
            "scene color msaa",
            SCENE_FORMAT,
            sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
    });
    let depth = texture(
        "scene depth",
        DEPTH_FORMAT,
        sample_count,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    SceneTargets {
        size,
        multisampled,
        color,
        raw,
        depth,
    }
}

/// Group 0 of the raster pipelines: camera (0), Palette colors (1), the dither frame (2) and Material constants (9).
pub(crate) fn create_scene_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("scene bind group layout"),
        entries: &[
            uniform_entry(0),
            uniform_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 9,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

struct PipelineSpec<'spec> {
    label: &'spec str,
    module: &'spec wgpu::ShaderModule,
    buffers: &'spec [Option<wgpu::VertexBufferLayout<'spec>>],
    topology: wgpu::PrimitiveTopology,
    cull_mode: Option<wgpu::Face>,
    front_face: wgpu::FrontFace,
    depth_write: bool,
    blend: Option<wgpu::BlendState>,
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    sample_count: u32,
    spec: &PipelineSpec<'_>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: spec.module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: spec.buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: spec.topology,
            cull_mode: spec.cull_mode,
            front_face: spec.front_face,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(spec.depth_write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module: spec.module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_FORMAT,
                blend: spec.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The full WGSL of the raster shader: material module, camera uniform, then the shader proper.
pub(crate) fn raster_shader_source(material_source: &str) -> String {
    format!(
        "{material_source}{}{}",
        include_str!("shaders/common.wgsl"),
        include_str!("shaders/raster.wgsl")
    )
}

/// The two pipelines that run `evaluate_material` (normal and mirrored winding). They are rebuilt whenever the
/// Material graphs change shape.
pub(crate) fn create_raster_pair(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    sample_count: u32,
    material_source: &str,
) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("raster shader"),
        source: wgpu::ShaderSource::Wgsl(raster_shader_source(material_source).into()),
    });
    let mesh_layout = wgpu::VertexBufferLayout {
        array_stride: size_of::<MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
    };
    let instance_layout = wgpu::VertexBufferLayout {
        array_stride: size_of::<InstanceData>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4],
    };
    let raster = |label, front_face| {
        build_pipeline(
            device,
            layout,
            sample_count,
            &PipelineSpec {
                label,
                module: &module,
                buffers: &[Some(mesh_layout.clone()), Some(instance_layout.clone())],
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                front_face,
                depth_write: true,
                blend: None,
            },
        )
    };
    (
        raster("raster pipeline", wgpu::FrontFace::Ccw),
        raster("mirrored raster pipeline", wgpu::FrontFace::Cw),
    )
}

fn create_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    sample_count: u32,
    material_source: &str,
) -> Pipelines {
    // The grid and the overlay lines never shade voxels, so they do not carry the material module.
    let module = |label, source: &str| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(format!("{}{source}", include_str!("shaders/common.wgsl")).into()),
        })
    };
    let grid_module = module("grid shader", include_str!("shaders/grid.wgsl"));
    let lines_module = module("lines shader", include_str!("shaders/lines.wgsl"));
    let line_layout = wgpu::VertexBufferLayout {
        array_stride: size_of::<LineVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
    };
    let (raster, raster_mirrored) = create_raster_pair(device, layout, sample_count, material_source);
    Pipelines {
        raster,
        raster_mirrored,
        grid: build_pipeline(
            device,
            layout,
            sample_count,
            &PipelineSpec {
                label: "grid pipeline",
                module: &grid_module,
                buffers: &[],
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                front_face: wgpu::FrontFace::Ccw,
                depth_write: false,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            },
        ),
        lines: build_pipeline(
            device,
            layout,
            sample_count,
            &PipelineSpec {
                label: "lines pipeline",
                module: &lines_module,
                buffers: &[Some(line_layout)],
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                front_face: wgpu::FrontFace::Ccw,
                depth_write: false,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            },
        ),
    }
}
