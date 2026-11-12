//! Temporal anti-aliasing for the raster viewport: the resolve pass that turns the dithered, jittered scene color of
//! the last frames into one smooth image (ADR 0006). The scene pass lives in `viewport_renderer.rs`.

use bytemuck::{Pod, Zeroable};

/// Format of the history the pass keeps between frames; wider than the 8-bit scene color so that blending many frames
/// does not stall on rounding.
const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Sub-pixel offsets of the projection, in pixels, one per frame: a Halton (2, 3) sequence.
const JITTER_FRAMES: u32 = 8;

/// The jitter of frame `frame`, in pixels, within `-0.5..0.5`.
pub(crate) fn jitter(frame: u32) -> [f32; 2] {
    let halton = |mut index: u32, base: u32| {
        let (mut fraction, mut result) = (1.0_f32, 0.0_f32);
        while index > 0 {
            fraction /= base as f32;
            result += fraction * (index % base) as f32;
            index /= base;
        }
        result
    };
    let index = frame % JITTER_FRAMES + 1;
    [halton(index, 2) - 0.5, halton(index, 3) - 0.5]
}

/// `view_proj` moved by `jitter` pixels on an image of `size`: a point's clip position gets `x += jx * w`.
pub(crate) fn jittered(view_proj: [[f32; 4]; 4], jitter: [f32; 2], size: (u32, u32)) -> [[f32; 4]; 4] {
    let (jx, jy) = (
        2.0 * jitter[0] / size.0.max(1) as f32,
        -2.0 * jitter[1] / size.1.max(1) as f32,
    );
    let mut out = view_proj;
    for column in &mut out {
        column[0] += jx * column[3];
        column[1] += jy * column[3];
    }
    out
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct TaaUniform {
    pub(crate) prev_view_proj: [[f32; 4]; 4],
    pub(crate) inv_view_proj: [[f32; 4]; 4],
    /// x: weight of the new frame, y: 1 = ignore the history, zw: image size.
    pub(crate) settings: [f32; 4],
}

const _: () = assert!(size_of::<TaaUniform>() == 144);

/// The images one resolve reads and writes.
pub(crate) struct TaaTargets<'view> {
    /// The scene as drawn this frame (dithered, jittered), `SCENE_FORMAT`.
    pub(crate) raw: &'view wgpu::TextureView,
    /// The scene's depth buffer, single-sample or multisampled to match the pass.
    pub(crate) depth: &'view wgpu::TextureView,
    /// Where the smooth image goes, `SCENE_FORMAT`.
    pub(crate) output: &'view wgpu::TextureView,
}

#[derive(Debug)]
struct History {
    size: (u32, u32),
    views: [wgpu::TextureView; 2],
}

/// The resolve pipeline and the two history images it alternates between.
#[derive(Debug)]
pub(crate) struct TaaPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    history: Option<History>,
    /// Which history image holds the last frame.
    current: usize,
}

impl TaaPass {
    /// Builds the pass for a scene color of `output_format`; `depth_multisampled` says whether the scene pass uses
    /// MSAA (its depth buffer is then a multisampled texture).
    pub(crate) fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat, depth_multisampled: bool) -> Self {
        let texture_entry = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1, float, false),
                texture_entry(2, wgpu::TextureSampleType::Depth, depth_multisampled),
                texture_entry(3, float, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let depth_type = if depth_multisampled {
            "texture_depth_multisampled_2d"
        } else {
            "texture_depth_2d"
        };
        let source = format!(
            "{}{}",
            include_str!("shaders/fullscreen.wgsl"),
            include_str!("shaders/taa.wgsl").replace("DEPTH_TEXTURE", depth_type)
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("taa shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("taa pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let target = |format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("taa pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_taa"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[target(output_format), target(HISTORY_FORMAT)],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("taa uniform"),
            size: size_of::<TaaUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("taa history sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            layout,
            pipeline,
            uniform,
            sampler,
            history: None,
            current: 0,
        }
    }

    /// Whether the history images have the size of the image. A pass without matching history must be encoded with
    /// the history ignored.
    pub(crate) fn has_history_for(&self, size: (u32, u32)) -> bool {
        self.history.as_ref().is_some_and(|history| history.size == size)
    }

    /// (Re)creates the history images for an image of `size`; their content is undefined until the next resolve.
    pub(crate) fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        let extent = wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        };
        let view = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HISTORY_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        self.history = Some(History {
            size,
            views: [view("taa history a"), view("taa history b")],
        });
        self.current = 0;
    }

    // HOT PATH: runs every frame while transparent Materials are on screen.
    /// Resolves `targets.raw` against the history into `targets.output`. `uniform.settings.y` must be 1 when
    /// [`TaaPass::has_history_for`] is false.
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &TaaTargets<'_>,
        uniform: &TaaUniform,
    ) {
        let Some(history) = &self.history else {
            return;
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(uniform));
        let read = self.current;
        let write = 1 - self.current;
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("taa bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(targets.raw),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(targets.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&history.views[read]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let attachment = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("taa pass"),
                color_attachments: &[attachment(targets.output), attachment(&history.views[write])],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.current = write;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_context::GraphicsBackend;

    const SIZE: u32 = 64;
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    #[test]
    fn the_jitter_stays_within_a_pixel_and_repeats() {
        for frame in 0..JITTER_FRAMES {
            let [x, y] = jitter(frame);
            assert!((-0.5..0.5).contains(&x) && (-0.5..0.5).contains(&y), "{x} {y}");
            assert_eq!(jitter(frame), jitter(frame + JITTER_FRAMES));
        }
        assert_ne!(jitter(0), jitter(1));
    }

    #[test]
    fn jittering_moves_the_clip_position_by_the_offset_times_w() {
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        let moved = glam::Mat4::from_cols_array_2d(&jittered(identity, [0.5, 0.25], (100, 100)));
        let clip = moved * glam::Vec4::new(0.0, 0.0, 0.5, 1.0);
        assert!((clip.x - 0.01).abs() < 1e-6 && (clip.y + 0.005).abs() < 1e-6, "{clip}");
    }

    /// A static image whose pixels flicker between two values every frame, as dithered glass does, settles to its
    /// mean; a frame with the history ignored passes through unchanged.
    #[test]
    fn a_flickering_image_settles_towards_its_mean() {
        // The backends the app opens, so a machine with only DirectX 12 skips instead of failing.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = GraphicsBackend::Automatic.backends();
        let instance = wgpu::Instance::new(descriptor);
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) else {
            return;
        };
        let extent = wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let raw = texture(
            "test raw",
            FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let depth = texture(
            "test depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let output = texture(
            "test output",
            FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let raw_view = raw.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let mut pass = TaaPass::new(&device, FORMAT, false);
        pass.resize(&device, (SIZE, SIZE));
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        let mut seed = 12_345_u32;
        let mut noise = || {
            seed = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
            (seed >> 16) & 1 == 1
        };
        let mut spread = Vec::new();
        for frame in 0..24 {
            // Half of the pixels show the glass (200), the others what is behind it (40).
            let mut bytes = Vec::new();
            for _ in 0..SIZE * SIZE {
                let value = if noise() { 200 } else { 40 };
                bytes.extend([value, value, value, 255]);
            }
            queue.write_texture(
                raw.as_image_copy(),
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: Some(SIZE),
                },
                extent,
            );
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
            // The depth buffer is cleared to the far plane by this empty pass.
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test depth clear"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
            let uniform = TaaUniform {
                prev_view_proj: identity,
                inv_view_proj: identity,
                settings: [0.1, f32::from(u8::from(frame == 0)), SIZE as f32, SIZE as f32],
            };
            pass.encode(
                &device,
                &queue,
                &mut encoder,
                &TaaTargets {
                    raw: &raw_view,
                    depth: &depth_view,
                    output: &output_view,
                },
                &uniform,
            );
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("test readback"),
                size: u64::from(SIZE * SIZE * 4),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIZE * 4),
                        rows_per_image: Some(SIZE),
                    },
                },
                extent,
            );
            queue.submit(std::iter::once(encoder.finish()));
            readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let data = readback.slice(..).get_mapped_range().unwrap();
            let values: Vec<f32> = data.chunks(4).map(|texel| f32::from(texel[0])).collect();
            let mean = values.iter().sum::<f32>() / values.len() as f32;
            let variance = values.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / values.len() as f32;
            spread.push(variance.sqrt());
        }
        // The first frame had no history and shows the raw flicker; later frames settle.
        assert!(spread[0] > 60.0, "the raw flicker is wide: {}", spread[0]);
        let settled = spread[23];
        assert!(settled < spread[0] * 0.4, "settled {settled} against raw {}", spread[0]);
    }
}
