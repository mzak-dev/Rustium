//! The live preview of Effects mode.
//!
//! The raster viewport draws into a small rectangle of the window (the hole in the node canvas, see
//! `hud::effects_canvas`) by squeezing the projection into that rectangle. "Model" shows the open Scene with its own
//! Colors; "Cube" and "Ball" show a small voxel shape painted with one reserved Palette Color that is pointed at the
//! edited Material in the GPU copy of the Palette (the document is not touched).

use glam::{IVec3, Mat4, Vec3, Vec4};
use rustium_core::{ChunkCoord, ChunkMesh, ColorId, Command, NodeId, Placement, Scene, ShapeId, Transform, mesh_chunk};
use rustium_render::{CameraUniform, ModelDraw};
use tracing::warn;

use crate::camera::OrbitCamera;
use crate::hud::PreviewShape;

/// Shape id the preview meshes are uploaded under; no Scene reaches this many shapes.
const PREVIEW_SHAPE: u32 = 0x00ff_ff00;
/// Edge of the cube and diameter of the ball, in voxels.
const SIZE: i32 = 12;
/// The Palette Color the preview shapes use. Palettes have at least this many entries when it is clamped to length.
const PREVIEW_COLOR_NUMBER: usize = 96;

/// Cached meshes of the Cube and Ball and what the GPU Palette currently shows.
#[derive(Debug, Default)]
pub(crate) struct Preview {
    shape: Option<PreviewShape>,
    meshes: Vec<(ChunkCoord, ChunkMesh)>,
    /// The shape whose meshes the GPU holds now, so they are uploaded once rather than every frame.
    uploaded: Option<PreviewShape>,
    /// The Material the GPU Palette points the preview Color at, while a Cube or Ball shows.
    pub(crate) applied_graph: Option<usize>,
}

impl Preview {
    /// The Palette Color the preview shapes are painted with.
    fn color(scene: &Scene) -> Option<ColorId> {
        let number = scene.palette().len().min(PREVIEW_COLOR_NUMBER);
        ColorId::new(u8::try_from(number).ok()?)
    }

    /// A copy of `scene` whose preview Color uses Material `graph`; only its Palette is meant to be read.
    pub(crate) fn scene_with_material(scene: &Scene, graph: usize) -> Scene {
        let mut copy = scene.clone();
        if let Some(id) = Self::color(scene) {
            let commands = [
                Command::SetColor {
                    id,
                    color: [0xc8, 0xcc, 0xd2],
                },
                Command::AssignGraph {
                    id,
                    graph: u32::try_from(graph).unwrap_or(0),
                },
            ];
            for command in &commands {
                if let Err(error) = rustium_core::apply(&mut copy, command) {
                    warn!(%error, "cannot prepare the material preview");
                }
            }
        }
        copy
    }

    /// Meshes of `shape` for upload, building them on first use.
    pub(crate) fn meshes(&mut self, shape: PreviewShape, scene: &Scene) -> (ShapeId, &[(ChunkCoord, ChunkMesh)]) {
        if self.shape != Some(shape) {
            self.meshes = build(shape, scene);
            self.shape = Some(shape);
        }
        (ShapeId::from_raw(PREVIEW_SHAPE), &self.meshes)
    }

    /// The meshes of `shape` when they still have to go to the GPU; `None` once they are there.
    pub(crate) fn take_upload(
        &mut self,
        shape: PreviewShape,
        scene: &Scene,
    ) -> Option<(ShapeId, &[(ChunkCoord, ChunkMesh)])> {
        if self.uploaded == Some(shape) {
            return None;
        }
        self.uploaded = Some(shape);
        Some(self.meshes(shape, scene))
    }

    /// Forgets what the GPU holds (its chunks were cleared, or the preview went away).
    pub(crate) fn invalidate(&mut self) {
        self.uploaded = None;
    }

    /// The single Model draw of a Cube or Ball preview.
    pub(crate) fn draws() -> Vec<ModelDraw> {
        vec![ModelDraw {
            placement: Placement {
                node: NodeId::ROOT,
                shape: ShapeId::from_raw(PREVIEW_SHAPE),
                transform: Transform::IDENTITY,
                rank: 0,
            },
            dim: 0.0,
        }]
    }
}

fn build(shape: PreviewShape, scene: &Scene) -> Vec<(ChunkCoord, ChunkMesh)> {
    let (Some(color), true) = (Preview::color(scene), shape != PreviewShape::Model) else {
        return Vec::new();
    };
    let mut scratch = Scene::new();
    let model = match rustium_core::apply(
        &mut scratch,
        &Command::NewModel {
            parent: NodeId::ROOT,
            name: "preview".to_owned(),
        },
    ) {
        Ok(Some(edit)) => edit.created_nodes().first().copied(),
        _ => None,
    };
    let Some(model) = model else {
        return Vec::new();
    };
    // The scratch Scene has the default Palette, whose last entries are at least `color`'s number.
    let mut voxels = Vec::new();
    let centre = (SIZE as f32 - 1.0) / 2.0;
    for z in 0..SIZE {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let inside = match shape {
                    PreviewShape::Ball => {
                        let offset = Vec3::new(x as f32 - centre, y as f32 - centre, z as f32 - centre);
                        offset.length() <= SIZE as f32 / 2.0
                    }
                    _ => true,
                };
                if inside {
                    voxels.push((IVec3::new(x, y, z), Some(color)));
                }
            }
        }
    }
    if let Err(error) = rustium_core::apply(&mut scratch, &Command::SetVoxels { model, voxels }) {
        warn!(%error, "cannot build the preview shape");
        return Vec::new();
    }
    let Some((_, built)) = scratch.model_shape(model) else {
        return Vec::new();
    };
    let coords: Vec<ChunkCoord> = built.chunks().map(|(coord, _)| coord).collect();
    coords
        .into_iter()
        .map(|coord| {
            let mut mesh = ChunkMesh::default();
            mesh_chunk(built, coord, &mut mesh);
            (coord, mesh)
        })
        .collect()
}

/// A camera that frames `scene` (Model) or the Cube and Ball, looking from the front-right above.
pub(crate) fn camera_for(shape: PreviewShape, scene: &Scene) -> OrbitCamera {
    let (centre, radius) = match shape {
        PreviewShape::Model => scene
            .world_volume(NodeId::ROOT)
            .map_or((Vec3::new(0.5, 4.0, 0.5), 8.0), |volume| {
                let min = volume.min.as_vec3();
                let max = (volume.max + IVec3::ONE).as_vec3();
                ((min + max) / 2.0, ((max - min).length() / 2.0).max(2.0))
            }),
        _ => (Vec3::splat(SIZE as f32 / 2.0), SIZE as f32 * 0.9),
    };
    let mut camera = OrbitCamera::default();
    camera.target = centre;
    camera.yaw = 0.75;
    camera.pitch = 0.5;
    camera.distance = radius / (camera.field_of_view_degrees.to_radians() / 2.0).sin() * 1.05;
    camera
}

/// Camera data for drawing `camera` into the window rectangle `rect` (points) of a window `screen` points big.
pub(crate) fn uniform(camera: &OrbitCamera, rect: egui::Rect, screen: egui::Vec2) -> CameraUniform {
    let aspect = rect.width() / rect.height().max(1.0);
    let centre = rect.center();
    let scale_x = rect.width() / screen.x.max(1.0);
    let scale_y = rect.height() / screen.y.max(1.0);
    let shift_x = centre.x / screen.x.max(1.0) * 2.0 - 1.0;
    let shift_y = 1.0 - centre.y / screen.y.max(1.0) * 2.0;
    // Clip x and y are scaled into the rectangle; the translation is multiplied by w so it survives the divide.
    let squeeze = Mat4::from_cols(
        Vec4::new(scale_x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, scale_y, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(shift_x, shift_y, 0.0, 1.0),
    );
    CameraUniform {
        view_proj: (squeeze * camera.view_proj(aspect)).to_cols_array_2d(),
        eye: camera.eye().extend(1.0).to_array(),
        sun: Vec3::new(0.45, 0.8, 0.35).normalize().extend(0.0).to_array(),
        grid: [1.0, 1.0, 0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_shape_is_uploaded_once_until_invalidated() {
        let scene = Scene::new();
        let mut preview = Preview::default();
        assert!(preview.take_upload(PreviewShape::Cube, &scene).is_some());
        assert!(preview.take_upload(PreviewShape::Cube, &scene).is_none());
        assert!(preview.take_upload(PreviewShape::Ball, &scene).is_some());
        preview.invalidate();
        assert!(preview.take_upload(PreviewShape::Ball, &scene).is_some());
    }
}
