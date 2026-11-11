use glam::IVec3;

use crate::edit::{Edit, EditKind, EditRecorder};
use crate::error::Error;
use crate::palette::MaterialId;
use crate::raycast::{HitTarget, RayHit};
use crate::scene::Scene;
use crate::shape::Volume;

/// Voxel Tools that change voxels with a [`Brush`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxelTool {
    /// Adds voxels against the hit face.
    Attach,
    /// Removes voxels around the hit voxel.
    Erase,
    /// Changes the Material of existing voxels around the hit voxel.
    Paint,
}

/// Footprint of a gesture: a cube `size` voxels wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brush {
    /// Edge length of the cube in voxels.
    pub size: u32,
}

impl Brush {
    /// Largest brush the editor accepts.
    pub const MAX_SIZE: u32 = 64;
}

impl Default for Brush {
    fn default() -> Self {
        Self { size: 1 }
    }
}

/// A request to change the Scene, from the interface, a script or an MCP client.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Applies a voxel Tool with a Brush where a ray hit the Scene.
    ApplyBrush {
        /// Which voxel Tool to use.
        tool: VoxelTool,
        /// Footprint of the gesture.
        brush: Brush,
        /// Where the gesture lands.
        hit: RayHit,
        /// Material for Attach and Paint; ignored by Erase.
        material: MaterialId,
    },
}

/// Applies `command` to `scene` and returns the resulting Edit, or `None` when nothing changed.
///
/// # Errors
///
/// [`Error::UnknownMaterial`] when the Material is not in the Palette, [`Error::BrushSize`] when the brush is
/// empty or larger than [`Brush::MAX_SIZE`].
pub fn apply(scene: &mut Scene, command: &Command) -> Result<Option<Edit>, Error> {
    match *command {
        Command::ApplyBrush {
            tool,
            brush,
            hit,
            material,
        } => apply_brush(scene, tool, brush, hit, material),
    }
}

fn apply_brush(
    scene: &mut Scene,
    tool: VoxelTool,
    brush: Brush,
    hit: RayHit,
    material: MaterialId,
) -> Result<Option<Edit>, Error> {
    if scene.palette().get(material).is_none() {
        return Err(Error::UnknownMaterial(material.get()));
    }
    if brush.size == 0 || brush.size > Brush::MAX_SIZE {
        return Err(Error::BrushSize {
            size: brush.size,
            max: Brush::MAX_SIZE,
        });
    }
    if tool != VoxelTool::Attach && hit.target == HitTarget::Ground {
        return Ok(None);
    }
    let bounds = brush_bounds(tool, brush, hit);
    let mut recorder = EditRecorder::new(&mut scene.shape);
    for_each_in(bounds.min, bounds.max, |position| {
        let current = recorder.get(position);
        match tool {
            VoxelTool::Attach if current.is_none() => recorder.set(position, Some(material)),
            VoxelTool::Erase => recorder.set(position, None),
            VoxelTool::Paint if current.is_some() => recorder.set(position, Some(material)),
            _ => {}
        }
    });
    let kind = match tool {
        VoxelTool::Attach => EditKind::Attach,
        VoxelTool::Erase => EditKind::Erase,
        VoxelTool::Paint => EditKind::Paint,
    };
    Ok(recorder.finish(kind))
}

/// The box of voxel positions a gesture covers: for Attach a cube resting on the hit face and centred across it,
/// for Erase and Paint a cube centred on the hit voxel.
pub fn brush_bounds(tool: VoxelTool, brush: Brush, hit: RayHit) -> Volume {
    // Clamped to MAX_SIZE, so the conversion always succeeds.
    let size = i32::try_from(brush.size.clamp(1, Brush::MAX_SIZE)).unwrap_or(1);
    let low = -(size - 1) / 2;
    let high = size / 2;
    match tool {
        VoxelTool::Attach => {
            let normal = hit.face.normal();
            let base = hit.voxel + normal;
            let axis = hit.face.axis();
            let mut min = IVec3::splat(low);
            let mut max = IVec3::splat(high);
            min[axis] = if normal[axis] > 0 { 0 } else { 1 - size };
            max[axis] = if normal[axis] > 0 { size - 1 } else { 0 };
            Volume {
                min: base + min,
                max: base + max,
            }
        }
        VoxelTool::Erase | VoxelTool::Paint => Volume {
            min: hit.voxel + low,
            max: hit.voxel + high,
        },
    }
}

fn for_each_in(min: IVec3, max: IVec3, mut visit: impl FnMut(IVec3)) {
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                visit(IVec3::new(x, y, z));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::edit::History;
    use crate::face::Face;
    use crate::{CHUNK_SIZE, ChunkCoord};

    fn material(number: u8) -> MaterialId {
        MaterialId::new(number).unwrap()
    }

    fn attach(scene: &mut Scene, at: IVec3, size: u32) -> Option<Edit> {
        let hit = RayHit {
            voxel: at - IVec3::Y,
            face: Face::PosY,
            target: HitTarget::Ground,
        };
        let command = Command::ApplyBrush {
            tool: VoxelTool::Attach,
            brush: Brush { size },
            hit,
            material: material(1),
        };
        apply(scene, &command).unwrap()
    }

    #[test]
    fn attach_on_the_ground_fills_a_cube_above_it() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 3);
        assert_eq!(scene.shape().volume().map(Volume::size), Some(IVec3::splat(3)));
        assert!(scene.shape().get(IVec3::new(0, 2, 0)).is_some());
        assert!(scene.shape().get(IVec3::new(0, -1, 0)).is_none());
    }

    #[test]
    fn volume_grows_when_attaching_outside_it() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 1);
        attach(&mut scene, IVec3::new(40, 0, 0), 1);
        let volume = scene.shape().volume().unwrap();
        assert_eq!(volume.min, IVec3::ZERO);
        assert_eq!(volume.max, IVec3::new(40, 0, 0));
    }

    #[test]
    fn undo_and_redo_restore_the_same_chunks_across_a_chunk_border() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let border = IVec3::new(CHUNK_SIZE - 1, 0, 0);
        history.push(attach(&mut scene, IVec3::ZERO, 1).unwrap());
        let untouched = Arc::clone(scene.shape().chunk(ChunkCoord(IVec3::ZERO)).unwrap());
        history.push(attach(&mut scene, border + IVec3::X, 2).unwrap());
        assert!(scene.shape().get(border + IVec3::X).is_some());

        history.undo(&mut scene);
        assert!(scene.shape().get(border + IVec3::X).is_none());
        assert!(Arc::ptr_eq(
            &untouched,
            scene.shape().chunk(ChunkCoord(IVec3::ZERO)).unwrap()
        ));

        history.redo(&mut scene);
        assert!(scene.shape().get(border + IVec3::X).is_some());
    }

    #[test]
    fn attaching_where_voxels_already_are_records_nothing() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 1);
        assert!(attach(&mut scene, IVec3::ZERO, 1).is_none());
    }

    #[test]
    fn unknown_material_is_rejected() {
        let mut scene = Scene::new();
        let hit = RayHit {
            voxel: IVec3::NEG_Y,
            face: Face::PosY,
            target: HitTarget::Ground,
        };
        let command = Command::ApplyBrush {
            tool: VoxelTool::Attach,
            brush: Brush::default(),
            hit,
            material: material(200),
        };
        assert!(matches!(apply(&mut scene, &command), Err(Error::UnknownMaterial(200))));
    }
}
