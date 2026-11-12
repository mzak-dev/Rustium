use bytemuck::{Pod, Zeroable};
use rustium_core::{MaterialGraph, Palette};

use crate::material_compiler::PARAM_CAPACITY;

/// Number of Color slots on the GPU: ids 1..=255, slot 0 stays empty so a voxel's byte indexes it directly.
pub(crate) const COLORS: usize = 256;

/// One Palette Color as the shaders read it (`shaders/material.wgsl`): linear base color and graph index.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuColor {
    /// Linear base color.
    pub albedo: [f32; 3],
    /// Bits 0..16: index of the Color's material graph. Bit 16 is set when the Material does not cast shadows, bit 17
    /// when it has a Volume output (haze dims shadow rays instead of stopping them).
    pub graph: u32,
}

/// The Material does not cast shadows (`COLOR_NO_SHADOW` in `material.wgsl`).
const FLAG_NO_SHADOW: u32 = 1 << 16;
/// The Material has a Volume output (`COLOR_VOLUMETRIC` in `material.wgsl`).
const FLAG_VOLUMETRIC: u32 = 1 << 17;

const _: () = assert!(size_of::<GpuColor>() == 16);

/// The Palette as one uniform buffer shared by the raster pass and the path tracer. The Materials themselves are
/// compiled code plus the `material_params` storage buffer (see [`MaterialBuffers`]).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuMaterialTable {
    /// One entry per Color id.
    pub colors: [GpuColor; COLORS],
}

const _: () = assert!(size_of::<GpuMaterialTable>() == COLORS * 16);

impl GpuMaterialTable {
    /// This table with the Material indices (and their flags) of `live`: the new Colors, but still pointing at
    /// Materials the shader that is running knows.
    #[must_use]
    pub(crate) fn with_graphs_of(mut self, live: &Self) -> Self {
        for (color, live) in self.colors.iter_mut().zip(&live.colors) {
            color.graph = live.graph;
        }
        self
    }

    /// Builds the table for a Scene's `palette`; a Color pointing at a graph that does not exist falls back to the
    /// Default material (0).
    pub fn new(palette: &Palette, graphs: &[MaterialGraph]) -> Self {
        let mut table = Self::zeroed();
        for (id, color) in palette.iter() {
            let graph = if (color.graph as usize) < graphs.len().min(MaterialGraph::MAX_PER_SCENE) {
                color.graph
            } else {
                0
            };
            let mut packed = graph;
            if let Some(found) = graphs.get(graph as usize) {
                if !found.casts_shadow {
                    packed |= FLAG_NO_SHADOW;
                }
                if found.volume_output().is_some() {
                    packed |= FLAG_VOLUMETRIC;
                }
            }
            if let Some(slot) = table.colors.get_mut(usize::from(id.get())) {
                *slot = GpuColor {
                    albedo: color.color.map(srgb_to_linear),
                    graph: packed,
                };
            }
        }
        table
    }
}

/// The two GPU buffers the shaders read Colors and Material constants from. Both renderers bind the same pair, and
/// neither is ever reallocated (the parameter buffer has a fixed capacity), so bind groups stay valid.
#[derive(Debug, Clone)]
pub struct MaterialBuffers {
    colors: wgpu::Buffer,
    params: wgpu::Buffer,
}

impl MaterialBuffers {
    /// Creates empty buffers. They are copy sources too, so [`Self::snapshot`] can duplicate them.
    pub fn new(device: &wgpu::Device) -> Self {
        let colors = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("materials colors uniform"),
            size: size_of::<GpuMaterialTable>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material params storage"),
            size: (PARAM_CAPACITY * size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { colors, params }
    }

    /// New buffers holding a copy of these (for a renderer that must not see later edits).
    #[must_use]
    pub fn snapshot(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let copy = Self::new(device);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("snapshot materials"),
        });
        encoder.copy_buffer_to_buffer(&self.colors, 0, &copy.colors, 0, self.colors.size());
        encoder.copy_buffer_to_buffer(&self.params, 0, &copy.params, 0, self.params.size());
        queue.submit(std::iter::once(encoder.finish()));
        copy
    }

    /// The Palette uniform (binding 1).
    pub fn colors(&self) -> &wgpu::Buffer {
        &self.colors
    }

    /// The Material constants storage buffer (binding 9).
    pub fn params(&self) -> &wgpu::Buffer {
        &self.params
    }

    /// Writes the Palette straight into the color table (for renderers without a background shader build).
    pub fn write_colors(&self, queue: &wgpu::Queue, palette: &Palette, graphs: &[MaterialGraph]) {
        self.write_table(queue, &GpuMaterialTable::new(palette, graphs));
    }

    /// Writes the color table.
    pub fn write_table(&self, queue: &wgpu::Queue, table: &GpuMaterialTable) {
        queue.write_buffer(&self.colors, 0, bytemuck::bytes_of(table));
    }

    /// Writes the Material constants. Only call it with numbers that belong to the shader the pipelines run.
    pub fn write_params(&self, queue: &wgpu::Queue, params: &[f32]) {
        let len = params.len().min(PARAM_CAPACITY);
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&params[..len]));
    }
}

fn srgb_to_linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use rustium_core::Scene;

    use super::*;

    #[test]
    fn colors_decode_to_linear_and_slot_zero_stays_empty() {
        let scene = Scene::new();
        let table = GpuMaterialTable::new(scene.palette(), scene.material_graphs());
        assert_eq!(table.colors[0], GpuColor::zeroed());
        assert!(table.colors[1].albedo[0] > 0.8, "palette entry 1 is light grey");
        assert_eq!(table.colors[scene.palette().len() + 1], GpuColor::zeroed());
    }

    #[test]
    fn a_color_pointing_at_a_missing_graph_falls_back_to_the_default() {
        let mut scene = Scene::new();
        let id = rustium_core::ColorId::new(3).unwrap();
        let create = rustium_core::Command::CreateGraph {
            name: "Glass".to_owned(),
            surface: rustium_core::SurfaceConstants::MATTE,
        };
        rustium_core::apply(&mut scene, &create).unwrap();
        rustium_core::apply(&mut scene, &rustium_core::Command::AssignGraph { id, graph: 1 }).unwrap();
        let table = GpuMaterialTable::new(scene.palette(), scene.material_graphs());
        assert_eq!(table.colors[3].graph, 1);
        let table = GpuMaterialTable::new(scene.palette(), &scene.material_graphs()[..1]);
        assert_eq!(table.colors[3].graph, 0);
    }
}
