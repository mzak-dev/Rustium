//! Progressive compute-shader path tracer (ADR 0001): traverses the chunk grid uploaded by [`TraceScene`],
//! accumulates samples into an `Rgba32Float` texture and tonemaps the result into the viewport.

use std::sync::mpsc;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use tracing::warn;

use crate::error::Error;
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
}

const _: () = assert!(size_of::<Params>() == 64 + 9 * 16);

/// Uniform block of `shaders/present.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PresentParams {
    frame: [f32; 4],
    image: [f32; 4],
    letterbox: [f32; 4],
}

const _: () = assert!(size_of::<PresentParams>() == 48);

#[derive(Debug)]
struct Accumulation {
    size: (u32, u32),
    textures: [wgpu::Texture; 2],
    /// `compute_groups[i]` reads texture `i` and writes texture `1 - i`.
    compute_groups: [wgpu::BindGroup; 2],
    present_groups: [wgpu::BindGroup; 2],
}

/// The compute pipeline, its accumulation textures and the GPU scene it walks.
#[derive(Debug)]
pub struct PathTracer {
    scene: TraceScene,
    materials: wgpu::Buffer,
    compute_layout: wgpu::BindGroupLayout,
    compute_pipeline: wgpu::ComputePipeline,
    present_layout: wgpu::BindGroupLayout,
    present_pipeline: wgpu::RenderPipeline,
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
    /// Builds the pipelines; `materials` is the shared Palette and Material uniform and `target_format` the format of
    /// the view [`PathTracer::encode`] draws into.
    pub fn new(device: &wgpu::Device, materials: &wgpu::Buffer, target_format: wgpu::TextureFormat) -> Self {
        let compute_source = format!(
            "{}{}",
            include_str!("shaders/material.wgsl"),
            include_str!("shaders/pathtracer.wgsl")
        );
        let compute_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("path tracer shader"),
            source: wgpu::ShaderSource::Wgsl(compute_source.into()),
        });
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
        let uniform = wgpu::BufferBindingType::Uniform;
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("path tracer bind group layout"),
            entries: &[
                buffer_entry(0, uniform),
                buffer_entry(1, uniform),
                buffer_entry(2, storage),
                buffer_entry(3, storage),
                buffer_entry(6, storage),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: ACCUM_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let compute_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("path tracer pipeline layout"),
            bind_group_layouts: &[Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("path tracer pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

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
            materials: materials.clone(),
            compute_layout,
            compute_pipeline,
            present_layout,
            present_pipeline,
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
        self.restart();
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
    /// Accumulates at most one pass of samples and draws the tonemapped image into `target`.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        frame: &TraceFrame,
    ) {
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
            self.bound_generation = self.scene.generation();
            self.restart();
        } else if self.bound_generation != self.scene.generation() {
            self.rebuild_groups(device);
        }
        let key = frame.key();
        if self.key != Some(key) {
            self.key = Some(key);
            self.samples_done = 0;
            self.generation += 1;
        }
        self.target_samples = frame.samples.max(1);

        let region = frame.region.map_or([0, 0, size.0, size.1], |region| {
            [
                region[0].min(size.0),
                region[1].min(size.1),
                region[2].min(size.0),
                region[3].min(size.1),
            ]
        });
        let passes = (frame.running || self.samples_done == 0) && self.samples_done < self.target_samples;
        let count = u32::from(passes);
        let (zenith, horizon, sun_color, sun_radius) = frame.sky.look();
        let sun = sun_direction(frame.sun_turn_degrees, frame.sun_height_degrees);
        let params = Params {
            inv_view_proj: frame.inv_view_proj,
            eye: [0.0; 4],
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
        };
        queue.write_buffer(&self.present_buffer, 0, bytemuck::bytes_of(&present));

        let Some(accumulation) = &self.accumulation else {
            return;
        };
        if passes {
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
        pass.set_bind_group(0, &accumulation.present_groups[self.current], &[]);
        pass.draw(0..3, 0..1);
    }

    /// Copies the accumulated image to the CPU, blocking until the GPU has finished.
    ///
    /// # Errors
    ///
    /// [`Error::Readback`] when nothing has been rendered yet or the GPU read fails.
    pub fn read_image(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<TraceImage, Error> {
        let accumulation = self.accumulation.as_ref().ok_or(Error::Readback)?;
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
            accumulation.textures[self.current].as_image_copy(),
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
        })
    }

    fn create_accumulation(&self, device: &wgpu::Device, size: (u32, u32)) -> Accumulation {
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
        let (compute_groups, present_groups) = self.groups_for(device, &textures);
        Accumulation {
            size,
            textures,
            compute_groups,
            present_groups,
        }
    }

    fn rebuild_groups(&mut self, device: &wgpu::Device) {
        if let Some(accumulation) = self.accumulation.take() {
            let (compute_groups, present_groups) = self.groups_for(device, &accumulation.textures);
            self.accumulation = Some(Accumulation {
                compute_groups,
                present_groups,
                ..accumulation
            });
        }
        self.bound_generation = self.scene.generation();
    }

    fn groups_for(
        &self,
        device: &wgpu::Device,
        textures: &[wgpu::Texture; 2],
    ) -> ([wgpu::BindGroup; 2], [wgpu::BindGroup; 2]) {
        let views = textures
            .each_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let compute = |read: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("path tracer bind group"),
                layout: &self.compute_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.materials.as_entire_binding(),
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
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&views[read]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&views[1 - read]),
                    },
                ],
            })
        };
        let present = |index: usize| {
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
                        resource: wgpu::BindingResource::TextureView(&views[index]),
                    },
                ],
            })
        };
        ([compute(0), compute(1)], [present(0), present(1)])
    }
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
    }
}
