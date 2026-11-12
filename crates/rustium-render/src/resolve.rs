//! The resolve stage between the path tracer's accumulation and the picture: the Pixel style's upscale, the
//! edge-aware denoiser and the Toon outlines. Everything the viewport shows and every saved image comes out of the
//! texture this stage writes, so the two always agree.

use bytemuck::{Pod, Zeroable};

use crate::path_tracer::{RenderPass, RenderStyle};

pub(crate) const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const WORKGROUP: u32 = 8;

/// Uniform block of `shaders/resolve.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Params {
    inv_view_proj: [[f32; 4]; 4],
    image: [u32; 4],
    options: [u32; 4],
    /// x: lens radius in voxels (0 is a pinhole), y: focus distance in voxels.
    lens: [f32; 4],
}

const _: () = assert!(size_of::<Params>() == 112);

/// What one resolve needs to know.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolveInput {
    pub(crate) inv_view_proj: [[f32; 4]; 4],
    pub(crate) size: (u32, u32),
    pub(crate) style: RenderStyle,
    pub(crate) denoise: bool,
    pub(crate) pass: RenderPass,
    pub(crate) samples: u32,
    /// Lens radius and focus distance in voxels when depth of field is on.
    pub(crate) lens: Option<(f32, f32)>,
    /// Edge length in pixels of one block of the Pixel style.
    pub(crate) block: u32,
}

impl ResolveInput {
    /// The Wire style keeps its one-pixel lines, which the denoiser would smear; view passes are display colours with
    /// no lighting noise.
    pub(crate) fn denoises(&self) -> bool {
        self.denoise && self.style != RenderStyle::Wire && self.pass == RenderPass::Beauty
    }
}

/// Edge length in pixels of one "big pixel" of the Pixel style for an image `height` pixels tall.
pub(crate) fn pixel_block(height: u32) -> u32 {
    (height / 240).clamp(2, 32)
}

/// Line width of the Toon outline in pixels.
fn outline_width(height: u32) -> u32 {
    (height / 540).clamp(1, 8)
}

#[derive(Debug)]
struct Targets {
    size: (u32, u32),
    resolved: wgpu::Texture,
    /// Demodulate: accumulation `i` into scratch 0.
    demodulate: [wgpu::BindGroup; 2],
    /// A-trous 1, 2 and 4: scratch 0 to 1, 1 to 0, 0 to 1.
    atrous: [wgpu::BindGroup; 3],
    /// Finish from the denoised scratch 1.
    finish_denoised: wgpu::BindGroup,
    /// Finish from accumulation `i`.
    finish_raw: [wgpu::BindGroup; 2],
    /// Show a view pass from the passes texture `i`.
    finish_pass: [wgpu::BindGroup; 2],
}

/// Pipelines and textures of the resolve stage.
#[derive(Debug)]
pub(crate) struct Resolver {
    layout: wgpu::BindGroupLayout,
    demodulate: wgpu::ComputePipeline,
    atrous: [wgpu::ComputePipeline; 3],
    finish: wgpu::ComputePipeline,
    finish_pass: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    targets: Option<Targets>,
}

impl Resolver {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resolve shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/resolve.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resolve bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: IMAGE_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("resolve pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |label, entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        Self {
            layout,
            demodulate: pipeline("resolve demodulate pipeline", "demodulate"),
            atrous: [
                pipeline("resolve a-trous 1 pipeline", "atrous_1"),
                pipeline("resolve a-trous 2 pipeline", "atrous_2"),
                pipeline("resolve a-trous 4 pipeline", "atrous_4"),
            ],
            finish: pipeline("resolve finish pipeline", "finish"),
            finish_pass: pipeline("resolve view pass pipeline", "finish_pass"),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("resolve params"),
                size: size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            targets: None,
        }
    }

    /// The finished picture, once [`Resolver::bind`] has been called.
    pub(crate) fn resolved(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|targets| &targets.resolved)
    }

    /// Creates the scratch and result textures for `size`, binds them to the tracer's textures and returns a view of
    /// the result.
    pub(crate) fn bind(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        accumulation: &[wgpu::Texture; 2],
        geometry: &wgpu::Texture,
        albedo: &wgpu::Texture,
        passes: &[wgpu::Texture; 2],
    ) -> wgpu::TextureView {
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
                format: IMAGE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let scratch = [texture("resolve scratch A"), texture("resolve scratch B")];
        let resolved = texture("resolved image");
        let view = |texture: &wgpu::Texture| texture.create_view(&wgpu::TextureViewDescriptor::default());
        let accumulation_views = accumulation.each_ref().map(view);
        let scratch_views = scratch.each_ref().map(view);
        let (geometry_view, albedo_view, resolved_view) = (view(geometry), view(albedo), view(&resolved));
        let layer = |texture: &wgpu::Texture, layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("resolve view pass layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        };
        let pass_views = passes.each_ref().map(|texture| [layer(texture, 0), layer(texture, 1)]);
        let group = |label,
                     source: &wgpu::TextureView,
                     geometry_view: &wgpu::TextureView,
                     albedo_view: &wgpu::TextureView,
                     destination: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(geometry_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(albedo_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(destination),
                    },
                ],
            })
        };
        self.targets = Some(Targets {
            size,
            demodulate: [0, 1].map(|index| {
                group(
                    "resolve demodulate",
                    &accumulation_views[index],
                    &geometry_view,
                    &albedo_view,
                    &scratch_views[0],
                )
            }),
            atrous: [
                group(
                    "resolve a-trous 1",
                    &scratch_views[0],
                    &geometry_view,
                    &albedo_view,
                    &scratch_views[1],
                ),
                group(
                    "resolve a-trous 2",
                    &scratch_views[1],
                    &geometry_view,
                    &albedo_view,
                    &scratch_views[0],
                ),
                group(
                    "resolve a-trous 4",
                    &scratch_views[0],
                    &geometry_view,
                    &albedo_view,
                    &scratch_views[1],
                ),
            ],
            finish_denoised: group(
                "resolve finish denoised",
                &scratch_views[1],
                &geometry_view,
                &albedo_view,
                &resolved_view,
            ),
            finish_raw: [0, 1].map(|index| {
                group(
                    "resolve finish raw",
                    &accumulation_views[index],
                    &geometry_view,
                    &albedo_view,
                    &resolved_view,
                )
            }),
            finish_pass: [0, 1].map(|index| {
                group(
                    "resolve finish view pass",
                    &pass_views[index][0],
                    &pass_views[index][1],
                    &albedo_view,
                    &resolved_view,
                )
            }),
            resolved,
        });
        resolved_view
    }

    /// Encodes the resolve of accumulation texture `current` into the result texture.
    pub(crate) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        current: usize,
        input: &ResolveInput,
    ) {
        let Some(targets) = self.targets.as_ref().filter(|targets| targets.size == input.size) else {
            return;
        };
        let denoise = input.denoises();
        let params = Params {
            inv_view_proj: input.inv_view_proj,
            image: [input.size.0, input.size.1, input.style.index(), input.block],
            options: [
                u32::from(denoise),
                outline_width(input.size.1),
                input.samples,
                input.pass.index(),
            ],
            lens: input.lens.map_or([0.0; 4], |(radius, focus)| [radius, focus, 0.0, 0.0]),
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let groups = (input.size.0.div_ceil(WORKGROUP), input.size.1.div_ceil(WORKGROUP));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("resolve pass"),
            timestamp_writes: None,
        });
        if denoise {
            pass.set_pipeline(&self.demodulate);
            pass.set_bind_group(0, &targets.demodulate[current], &[]);
            pass.dispatch_workgroups(groups.0, groups.1, 1);
            for (pipeline, group) in self.atrous.iter().zip(&targets.atrous) {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(groups.0, groups.1, 1);
            }
        }
        let (pipeline, group) = if input.pass != RenderPass::Beauty {
            (&self.finish_pass, &targets.finish_pass[current])
        } else if denoise {
            (&self.finish, &targets.finish_denoised)
        } else {
            (&self.finish, &targets.finish_raw[current])
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(groups.0, groups.1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_blocks_grow_with_the_image_but_stay_visible() {
        assert_eq!(pixel_block(96), 2);
        assert_eq!(pixel_block(1080), 4);
        assert_eq!(pixel_block(100_000), 32);
    }

    #[test]
    fn wire_is_never_denoised() {
        let input = ResolveInput {
            inv_view_proj: [[0.0; 4]; 4],
            size: (8, 8),
            style: RenderStyle::Wire,
            denoise: true,
            pass: RenderPass::Beauty,
            samples: 1,
            lens: None,
            block: 2,
        };
        assert!(!input.denoises());
        assert!(
            ResolveInput {
                style: RenderStyle::Clay,
                ..input
            }
            .denoises()
        );
    }
}
