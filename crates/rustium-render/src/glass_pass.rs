use bytemuck::{Pod, Zeroable};

/// A rounded rectangle, in physical pixels, under which the viewport is shown blurred (glass panels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassRect {
    /// Top-left corner.
    pub min: [f32; 2],
    /// Bottom-right corner.
    pub max: [f32; 2],
    /// Corner radius.
    pub radius: f32,
}

const MAX_RECTS: usize = 32;
/// The blur runs at a quarter of the window resolution; cheap, and the softness hides the low resolution.
const BLUR_DIVISOR: u32 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GlassUniform {
    header: [u32; 4],
    rects: [[f32; 4]; MAX_RECTS],
    radii: [[f32; 4]; MAX_RECTS],
}

const _: () = assert!(size_of::<GlassUniform>() == 16 + 2 * 16 * MAX_RECTS);

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BlurParams {
    step: [f32; 4],
}

const _: () = assert!(size_of::<BlurParams>() == 16);

/// Downsamples and blurs the rendered viewport, then composites it onto the surface with blurred glass rects.
#[derive(Debug)]
pub(crate) struct GlassPass {
    blur_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    downsample_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    horizontal_params: wgpu::Buffer,
    vertical_params: wgpu::Buffer,
    glass_buffer: wgpu::Buffer,
    targets: Option<BlurTargets>,
}

#[derive(Debug)]
struct BlurTargets {
    first: wgpu::TextureView,
    second: wgpu::TextureView,
    downsample_group: wgpu::BindGroup,
    horizontal_group: wgpu::BindGroup,
    vertical_group: wgpu::BindGroup,
    composite_group: wgpu::BindGroup,
}

const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl GlassPass {
    pub(crate) fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let blur_source = concat!(
            include_str!("shaders/fullscreen.wgsl"),
            include_str!("shaders/blur.wgsl")
        );
        let composite_source = concat!(
            include_str!("shaders/fullscreen.wgsl"),
            include_str!("shaders/composite.wgsl")
        );
        let blur_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur shader"),
            source: wgpu::ShaderSource::Wgsl(blur_source.into()),
        });
        let composite_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite shader"),
            source: wgpu::ShaderSource::Wgsl(composite_source.into()),
        });

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur bind group layout"),
            entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite bind group layout"),
            entries: &[texture_entry(0), texture_entry(1), sampler_entry(2), uniform_entry(3)],
        });

        let fullscreen_pipeline = |label, layout: &wgpu::BindGroupLayout, module, entry, format| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let downsample_pipeline = fullscreen_pipeline(
            "downsample pipeline",
            &blur_layout,
            &blur_module,
            "fs_downsample",
            BLUR_FORMAT,
        );
        let blur_pipeline = fullscreen_pipeline("blur pipeline", &blur_layout, &blur_module, "fs_blur", BLUR_FORMAT);
        let composite_pipeline = fullscreen_pipeline(
            "composite pipeline",
            &composite_layout,
            &composite_module,
            "fs_composite",
            surface_format,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glass sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params_size = size_of::<BlurParams>() as u64;
        Self {
            blur_layout,
            composite_layout,
            downsample_pipeline,
            blur_pipeline,
            composite_pipeline,
            sampler,
            horizontal_params: uniform("horizontal blur params", params_size),
            vertical_params: uniform("vertical blur params", params_size),
            glass_buffer: uniform("glass rects", size_of::<GlassUniform>() as u64),
            targets: None,
        }
    }

    /// Rebuilds the blur textures and bind groups for a new viewport size or scene texture.
    pub(crate) fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        let width = (size.0 / BLUR_DIVISOR).max(1);
        let height = (size.1 / BLUR_DIVISOR).max(1);
        let texture = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: BLUR_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let first = texture("blur target A");
        let second = texture("blur target B");

        let texel = [1.0 / width as f32, 1.0 / height as f32];
        queue.write_buffer(
            &self.horizontal_params,
            0,
            bytemuck::bytes_of(&BlurParams {
                step: [texel[0], 0.0, 0.0, 0.0],
            }),
        );
        queue.write_buffer(
            &self.vertical_params,
            0,
            bytemuck::bytes_of(&BlurParams {
                step: [0.0, texel[1], 0.0, 0.0],
            }),
        );

        let blur_group = |label, source: &wgpu::TextureView, params: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params.as_entire_binding(),
                    },
                ],
            })
        };
        let downsample_group = blur_group("downsample bind group", scene, &self.horizontal_params);
        let horizontal_group = blur_group("horizontal blur bind group", &first, &self.horizontal_params);
        let vertical_group = blur_group("vertical blur bind group", &second, &self.vertical_params);
        let composite_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bind group"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&first),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.glass_buffer.as_entire_binding(),
                },
            ],
        });
        self.targets = Some(BlurTargets {
            first,
            second,
            downsample_group,
            horizontal_group,
            vertical_group,
            composite_group,
        });
    }

    /// Blurs the scene (when `blur` is on and there are rects) and composites it onto `surface`.
    pub(crate) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        surface: &wgpu::TextureView,
        rects: &[GlassRect],
        blur: bool,
    ) {
        let Some(targets) = &self.targets else {
            return;
        };
        let blur = blur && !rects.is_empty();
        let mut uniform = GlassUniform::zeroed();
        let count = rects.len().min(MAX_RECTS);
        uniform.header = [u32::try_from(count).unwrap_or(0), u32::from(blur), 0, 0];
        for (index, rect) in rects.iter().take(MAX_RECTS).enumerate() {
            uniform.rects[index] = [rect.min[0], rect.min[1], rect.max[0], rect.max[1]];
            uniform.radii[index] = [rect.radius, 0.0, 0.0, 0.0];
        }
        queue.write_buffer(&self.glass_buffer, 0, bytemuck::bytes_of(&uniform));

        if blur {
            let passes = [
                (
                    "downsample pass",
                    &self.downsample_pipeline,
                    &targets.downsample_group,
                    &targets.first,
                ),
                (
                    "horizontal blur pass",
                    &self.blur_pipeline,
                    &targets.horizontal_group,
                    &targets.second,
                ),
                (
                    "vertical blur pass",
                    &self.blur_pipeline,
                    &targets.vertical_group,
                    &targets.first,
                ),
            ];
            for (label, pipeline, group, target) in passes {
                fullscreen_pass(encoder, label, target, pipeline, group);
            }
        }
        fullscreen_pass(
            encoder,
            "composite pass",
            surface,
            &self.composite_pipeline,
            &targets.composite_group,
        );
    }
}

fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
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
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}
