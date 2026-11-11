use bytemuck::{Pod, Zeroable};
use rustium_core::{MaterialGraph, Palette};

/// Number of Color slots on the GPU: ids 1..=255, slot 0 stays empty so a voxel's byte indexes it directly.
pub(crate) const COLORS: usize = 256;
/// Number of material graph records on the GPU.
pub(crate) const SURFACES: usize = MaterialGraph::MAX_PER_SCENE;

/// One Palette Color as the shaders read it (`shaders/material.wgsl`): linear base color and graph index.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuColor {
    /// Linear base color.
    pub albedo: [f32; 3],
    /// Index of the Color's material graph.
    pub graph: u32,
}

const _: () = assert!(size_of::<GpuColor>() == 16);

/// The constant Surface parameters of one material graph.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuSurface {
    /// rgb: linear emission color, a: emission strength (`0` means no glow).
    pub emission: [f32; 4],
    /// x: roughness, y: metallic, z: transmission, w: index of refraction.
    pub params: [f32; 4],
}

const _: () = assert!(size_of::<GpuSurface>() == 32);

/// Everything `evaluate_material` reads, as one uniform buffer shared by the raster pass and the path tracer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuMaterialTable {
    /// One entry per Color id.
    pub colors: [GpuColor; COLORS],
    /// One record per material graph.
    pub surfaces: [GpuSurface; SURFACES],
}

const _: () = assert!(size_of::<GpuMaterialTable>() == COLORS * 16 + SURFACES * 32);

impl GpuMaterialTable {
    /// Builds the table for a Scene's `palette` and `graphs`; unused slots are zeroed and a Color pointing at a
    /// missing graph falls back to the Default material (0).
    pub fn new(palette: &Palette, graphs: &[MaterialGraph]) -> Self {
        let mut table = Self::zeroed();
        for (slot, graph) in table.surfaces.iter_mut().zip(graphs) {
            let surface = &graph.surface;
            let [r, g, b] = surface.emission_color.map(srgb_to_linear);
            *slot = GpuSurface {
                emission: [r, g, b, surface.emission_strength],
                params: [surface.roughness, surface.metallic, surface.transmission, surface.ior],
            };
        }
        for (id, color) in palette.iter() {
            let graph = if (color.graph as usize) < graphs.len().min(SURFACES) {
                color.graph
            } else {
                0
            };
            if let Some(slot) = table.colors.get_mut(usize::from(id.get())) {
                *slot = GpuColor {
                    albedo: color.color.map(srgb_to_linear),
                    graph,
                };
            }
        }
        table
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
    use rustium_core::{Scene, SurfaceConstants};

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
    fn surface_constants_land_in_the_expected_lanes() {
        let graphs = [
            MaterialGraph::default_material(),
            MaterialGraph {
                name: "Lamp".to_owned(),
                surface: SurfaceConstants {
                    roughness: 0.25,
                    metallic: 1.0,
                    emission_color: [255, 255, 255],
                    emission_strength: 4.0,
                    transmission: 0.5,
                    ior: 1.33,
                },
            },
        ];
        let table = GpuMaterialTable::new(&Palette::default(), &graphs);
        assert_eq!(table.surfaces[1].params, [0.25, 1.0, 0.5, 1.33]);
        assert!((table.surfaces[1].emission[0] - 1.0).abs() < 1e-6);
        assert!((table.surfaces[1].emission[3] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn a_color_pointing_at_a_missing_graph_falls_back_to_the_default() {
        let mut scene = Scene::new();
        let id = rustium_core::MaterialId::new(3).unwrap();
        let create = rustium_core::Command::CreateGraph {
            name: "Glass".to_owned(),
            surface: SurfaceConstants::MATTE,
        };
        rustium_core::apply(&mut scene, &create).unwrap();
        rustium_core::apply(&mut scene, &rustium_core::Command::AssignGraph { id, graph: 1 }).unwrap();
        let table = GpuMaterialTable::new(scene.palette(), scene.material_graphs());
        assert_eq!(table.colors[3].graph, 1);
        let table = GpuMaterialTable::new(scene.palette(), &scene.material_graphs()[..1]);
        assert_eq!(table.colors[3].graph, 0);
    }
}
