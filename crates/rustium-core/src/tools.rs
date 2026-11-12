//! Geometry of the voxel Tools: which cells a Brush covers, what Fill reaches and how Mirror reflects them.

use std::collections::{HashSet, VecDeque};

use glam::IVec3;

use crate::command::VoxelTool;
use crate::error::Error;
use crate::raycast::{HitTarget, RayHit};
use crate::shape::{Shape, Volume};

/// Footprint family of a Brush gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrushKind {
    /// A cube `size` voxels wide at the pointer.
    #[default]
    Voxel,
    /// A line from the press to the release, `size` voxels thick.
    Line,
    /// A rectangle on the pressed face between the press and the release, `size` voxels deep.
    Box,
    /// The whole connected face of the same Color under the pointer, `size` voxels deep.
    Face,
}

/// The footprint of an Attach, Erase or Paint gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brush {
    /// Which footprint.
    pub kind: BrushKind,
    /// Edge length or thickness in voxels.
    pub size: u32,
}

impl Brush {
    /// Largest brush the editor accepts.
    pub const MAX_SIZE: u32 = 64;
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            kind: BrushKind::Voxel,
            size: 1,
        }
    }
}

/// Which kind of Fill to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillMode {
    /// Repaints the connected face of the same Color on the plane of the hit face.
    #[default]
    Surface,
    /// Fills the connected empty space in front of the hit face, never leaving the Volume.
    Volume,
}

/// Reflects every gesture across the centre planes of the Focus Model's Volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mirror {
    /// Which of X, Y and Z are mirrored.
    pub axes: [bool; 3],
    /// Sum of the Volume's `min` and `max`: a cell `p` reflects to `pivot - p` on each mirrored axis.
    pub pivot: IVec3,
}

impl Mirror {
    /// Mirrors across the centre planes of `volume` (the origin when the Shape is still empty).
    pub fn around(axes: [bool; 3], volume: Option<Volume>) -> Self {
        Self {
            axes,
            pivot: volume.map_or(IVec3::ZERO, |volume| volume.min + volume.max),
        }
    }

    /// Whether any axis is mirrored.
    pub fn is_active(self) -> bool {
        self.axes.iter().any(|axis| *axis)
    }

    /// Position of the mirror plane on `axis`, in point coordinates.
    pub fn plane(self, axis: usize) -> f32 {
        (self.pivot[axis] + 1) as f32 / 2.0
    }

    /// Calls `visit` with `cell` and each of its reflections.
    pub fn for_each_image(self, cell: IVec3, mut visit: impl FnMut(IVec3)) {
        let active = u8::from(self.axes[0]) | (u8::from(self.axes[1]) << 1) | (u8::from(self.axes[2]) << 2);
        for mask in 0..8_u8 {
            if mask & !active != 0 {
                continue;
            }
            let mut image = cell;
            for axis in 0..3 {
                if mask & (1 << axis) != 0 {
                    image[axis] = self.pivot[axis] - cell[axis];
                }
            }
            visit(image);
        }
    }
}

/// The cell a gesture starts or ends at: for Attach the empty cell in front of the hit face, otherwise the hit voxel.
pub(crate) fn gesture_cell(tool: VoxelTool, hit: RayHit) -> IVec3 {
    match tool {
        VoxelTool::Attach => hit.voxel + hit.face.normal(),
        VoxelTool::Erase | VoxelTool::Paint => hit.voxel,
    }
}

/// Lowest and highest offset of a cube `size` wide centred on a cell.
fn cube_span(size: u32) -> (i32, i32) {
    // Clamped to MAX_SIZE, so the conversion always succeeds.
    let size = i32::try_from(size.clamp(1, Brush::MAX_SIZE)).unwrap_or(1);
    (-(size - 1) / 2, size / 2)
}

/// The box of voxel positions a Voxel brush covers: a cube centred across the hit face that rests on it, outside
/// the face for Attach and inside it, from the surface in, for Erase and Paint. Only the side of the face changes
/// between the Tools, so a cube of any size reaches the same depth whichever way the face points.
fn voxel_bounds(tool: VoxelTool, size: u32, hit: RayHit) -> Volume {
    let (low, high) = cube_span(size);
    let size = i32::try_from(size.clamp(1, Brush::MAX_SIZE)).unwrap_or(1);
    let base = gesture_cell(tool, hit);
    let axis = hit.face.axis();
    // The direction the cube grows in along the face axis: away from the face for Attach, into the solid otherwise.
    let outward = hit.face.normal()[axis] > 0;
    let grows_up = outward == (tool == VoxelTool::Attach);
    let mut min = IVec3::splat(low);
    let mut max = IVec3::splat(high);
    min[axis] = if grows_up { 0 } else { 1 - size };
    max[axis] = if grows_up { size - 1 } else { 0 };
    Volume {
        min: base + min,
        max: base + max,
    }
}

/// Every cell a gesture covers, from the press at `from` to the release at `to` (equal for Voxel and Face), before
/// Mirror; the Tool then changes the ones it can (Attach only fills empty cells, Paint only recolors filled ones).
pub fn brush_positions(shape: &Shape, tool: VoxelTool, brush: Brush, from: RayHit, to: RayHit) -> Vec<IVec3> {
    if tool != VoxelTool::Attach && from.target == HitTarget::Ground {
        return Vec::new();
    }
    match brush.kind {
        BrushKind::Voxel => cells_in(voxel_bounds(tool, brush.size, from)),
        BrushKind::Line => line_positions(tool, brush.size, from, to),
        BrushKind::Box => box_positions(tool, brush.size, from, to),
        BrushKind::Face => face_positions(shape, tool, brush.size, from),
    }
}

/// The box around the cells a gesture would cover, for the hover outline; `None` when it covers nothing.
pub fn brush_bounds(shape: &Shape, tool: VoxelTool, brush: Brush, from: RayHit, to: RayHit) -> Option<Volume> {
    if tool != VoxelTool::Attach && from.target == HitTarget::Ground {
        return None;
    }
    if brush.kind == BrushKind::Voxel {
        return Some(voxel_bounds(tool, brush.size, from));
    }
    bounds_of(&brush_positions(shape, tool, brush, from, to))
}

fn bounds_of(positions: &[IVec3]) -> Option<Volume> {
    let (first, rest) = positions.split_first()?;
    Some(
        rest.iter()
            .fold(Volume::point(*first), |volume, cell| volume.including(*cell)),
    )
}

fn cells_in(volume: Volume) -> Vec<IVec3> {
    let size = volume.size();
    let capacity = usize::try_from(size.x.max(0) * size.y.max(0) * size.z.max(0)).unwrap_or(0);
    let mut cells = Vec::with_capacity(capacity);
    for y in volume.min.y..=volume.max.y {
        for z in volume.min.z..=volume.max.z {
            for x in volume.min.x..=volume.max.x {
                cells.push(IVec3::new(x, y, z));
            }
        }
    }
    cells
}

fn line_positions(tool: VoxelTool, size: u32, from: RayHit, to: RayHit) -> Vec<IVec3> {
    let start = gesture_cell(tool, from);
    let end = gesture_cell(tool, to);
    let (low, high) = cube_span(size);
    let steps = (end - start).abs().max_element();
    let mut seen = HashSet::new();
    let mut cells = Vec::new();
    for step in 0..=steps {
        let t = if steps == 0 { 0.0 } else { step as f32 / steps as f32 };
        let centre = (start.as_vec3() + (end - start).as_vec3() * t).round().as_ivec3();
        for dy in low..=high {
            for dz in low..=high {
                for dx in low..=high {
                    let cell = centre + IVec3::new(dx, dy, dz);
                    if seen.insert(cell) {
                        cells.push(cell);
                    }
                }
            }
        }
    }
    cells
}

fn box_positions(tool: VoxelTool, size: u32, from: RayHit, to: RayHit) -> Vec<IVec3> {
    let axis = from.face.axis();
    let a = gesture_cell(tool, from);
    let mut b = gesture_cell(tool, to);
    b[axis] = a[axis];
    // Attach grows away from the face; Erase and Paint dig into the object.
    let direction = match tool {
        VoxelTool::Attach => from.face.normal()[axis],
        VoxelTool::Erase | VoxelTool::Paint => -from.face.normal()[axis],
    };
    let depth = i32::try_from(size.clamp(1, Brush::MAX_SIZE)).unwrap_or(1);
    let (min, max) = (a.min(b), a.max(b));
    let mut cells = Vec::new();
    for layer in 0..depth {
        let mut layer_min = min;
        let mut layer_max = max;
        layer_min[axis] = a[axis] + direction * layer;
        layer_max[axis] = layer_min[axis];
        cells.extend(cells_in(Volume {
            min: layer_min,
            max: layer_max,
        }));
    }
    cells
}

fn face_positions(shape: &Shape, tool: VoxelTool, size: u32, hit: RayHit) -> Vec<IVec3> {
    let region = face_region(shape, hit);
    let normal = hit.face.normal();
    let depth = i32::try_from(size.clamp(1, Brush::MAX_SIZE)).unwrap_or(1);
    let mut cells = Vec::with_capacity(region.len() * usize::try_from(depth).unwrap_or(1));
    for layer in 0..depth {
        for cell in &region {
            cells.push(match tool {
                VoxelTool::Attach => *cell + normal * (layer + 1),
                VoxelTool::Erase | VoxelTool::Paint => *cell - normal * layer,
            });
        }
    }
    cells
}

/// The connected patch of voxels of the hit voxel's Color that lie on its plane and show a face in the same
/// direction; empty unless the hit is a voxel.
pub(crate) fn face_region(shape: &Shape, hit: RayHit) -> Vec<IVec3> {
    if hit.target != HitTarget::Voxel {
        return Vec::new();
    }
    let Some(color) = shape.get(hit.voxel) else {
        return Vec::new();
    };
    let normal = hit.face.normal();
    let axis = hit.face.axis();
    let steps = [(axis + 1) % 3, (axis + 2) % 3]
        .into_iter()
        .flat_map(|other| {
            let mut step = IVec3::ZERO;
            step[other] = 1;
            [step, -step]
        })
        .collect::<Vec<_>>();
    let exposed = |cell: IVec3| shape.get(cell) == Some(color) && shape.get(cell + normal).is_none();
    if !exposed(hit.voxel) {
        return Vec::new();
    }
    let mut seen = HashSet::from([hit.voxel]);
    let mut queue = VecDeque::from([hit.voxel]);
    let mut region = Vec::new();
    while let Some(cell) = queue.pop_front() {
        region.push(cell);
        for step in &steps {
            let next = cell + *step;
            if exposed(next) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    region
}

/// Largest number of cells a 3D Fill may visit before it is refused.
pub const FILL_LIMIT: usize = 2_000_000;

/// The empty cells connected to the cell in front of `hit` without leaving the Shape's Volume.
///
/// # Errors
///
/// [`Error::FillTooLarge`] when the region has more than [`FILL_LIMIT`] cells.
pub(crate) fn fill_volume(shape: &Shape, hit: RayHit) -> Result<Vec<IVec3>, Error> {
    let Some(volume) = shape.volume() else {
        return Ok(Vec::new());
    };
    let start = hit.voxel + hit.face.normal();
    let open = |cell: IVec3| volume.contains(cell) && shape.get(cell).is_none();
    if !open(start) {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    let mut region = Vec::new();
    while let Some(cell) = queue.pop_front() {
        if region.len() >= FILL_LIMIT {
            return Err(Error::FillTooLarge { limit: FILL_LIMIT });
        }
        region.push(cell);
        for step in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
            let next = cell + step;
            if open(next) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    Ok(region)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Face;
    use crate::palette::ColorId;

    fn voxel(x: i32, y: i32, z: i32) -> IVec3 {
        IVec3::new(x, y, z)
    }

    fn top_hit(cell: IVec3) -> RayHit {
        RayHit {
            voxel: cell,
            face: Face::PosY,
            target: HitTarget::Voxel,
        }
    }

    fn slab(width: i32) -> Shape {
        let mut shape = Shape::new();
        let color = ColorId::new(1);
        for x in 0..width {
            for z in 0..width {
                shape.set(voxel(x, 0, z), color);
            }
        }
        shape
    }

    #[test]
    fn mirror_reflects_across_the_centre_of_the_volume() {
        let mirror = Mirror::around(
            [true, false, false],
            Some(Volume {
                min: voxel(0, 0, 0),
                max: voxel(9, 4, 4),
            }),
        );
        let mut images = Vec::new();
        mirror.for_each_image(voxel(2, 1, 1), |cell| images.push(cell));
        assert_eq!(images, vec![voxel(2, 1, 1), voxel(7, 1, 1)]);
    }

    #[test]
    fn mirroring_two_axes_gives_four_images() {
        let mirror = Mirror::around([true, true, false], Some(Volume::point(IVec3::ZERO)));
        let mut count = 0;
        mirror.for_each_image(voxel(3, 3, 3), |_| count += 1);
        assert_eq!(count, 4);
    }

    #[test]
    fn erase_cubes_reach_into_the_solid_the_same_from_either_side() {
        for (face, inward) in [(Face::PosY, -1), (Face::NegY, 1), (Face::PosX, 0), (Face::NegZ, 0)] {
            let hit = RayHit {
                voxel: IVec3::ZERO,
                face,
                target: HitTarget::Voxel,
            };
            let bounds = voxel_bounds(VoxelTool::Erase, 2, hit);
            let axis = face.axis();
            assert_eq!(bounds.size(), IVec3::splat(2), "{face:?}");
            assert_eq!(
                bounds.min[axis] + bounds.max[axis],
                -face.normal()[axis],
                "{face:?} digs inward"
            );
            if inward != 0 {
                assert_eq!(bounds.min.y + bounds.max.y, inward);
            }
        }
    }

    #[test]
    fn line_brush_covers_both_ends() {
        let shape = Shape::new();
        let from = top_hit(voxel(0, -1, 0));
        let to = top_hit(voxel(5, -1, 3));
        let cells = brush_positions(
            &shape,
            VoxelTool::Attach,
            Brush {
                kind: BrushKind::Line,
                size: 1,
            },
            from,
            to,
        );
        assert!(cells.contains(&voxel(0, 0, 0)) && cells.contains(&voxel(5, 0, 3)));
        assert_eq!(cells.len(), 6);
    }

    #[test]
    fn box_brush_fills_the_rectangle_on_the_face_plane() {
        let shape = Shape::new();
        let from = top_hit(voxel(0, -1, 0));
        let to = top_hit(voxel(3, 7, 2));
        let brush = Brush {
            kind: BrushKind::Box,
            size: 2,
        };
        let cells = brush_positions(&shape, VoxelTool::Attach, brush, from, to);
        assert_eq!(cells.len(), 4 * 3 * 2);
        assert!(cells.iter().all(|cell| (0..=1).contains(&cell.y)));
    }

    #[test]
    fn face_brush_selects_the_connected_exposed_patch_only() {
        let mut shape = slab(4);
        shape.set(voxel(1, 1, 1), ColorId::new(1));
        let region = face_region(&shape, top_hit(voxel(0, 0, 0)));
        // The patch is the 4×4 slab minus the cell under the bump.
        assert_eq!(region.len(), 15);
        assert!(!region.contains(&voxel(1, 0, 1)));
    }

    #[test]
    fn fill_volume_stays_inside_the_volume() {
        let mut shape = slab(5);
        shape.set(voxel(4, 3, 4), ColorId::new(1));
        let region = fill_volume(&shape, top_hit(voxel(0, 0, 0))).unwrap();
        let volume = shape.volume().unwrap();
        // Everything above the slab up to the highest voxel: 5 × 3 × 5 minus the bump.
        assert_eq!(region.len(), 5 * 3 * 5 - 1);
        assert!(region.iter().all(|cell| volume.contains(*cell)));
    }

    #[test]
    fn fill_volume_of_a_sealed_room_stops_at_the_walls() {
        let mut shape = Shape::new();
        let color = ColorId::new(1);
        for x in 0..5 {
            for y in 0..5 {
                for z in 0..5 {
                    let edge = [x, y, z].iter().any(|v| *v == 0 || *v == 4);
                    if edge {
                        shape.set(voxel(x, y, z), color);
                    }
                }
            }
        }
        let region = fill_volume(&shape, top_hit(voxel(0, 0, 0))).unwrap();
        // The hit voxel is a wall cell; the cell above it is a wall too, so nothing is reachable from there.
        assert_eq!(region.len(), 0);
        let inside = fill_volume(&shape, top_hit(voxel(2, 0, 2))).unwrap();
        assert_eq!(inside.len(), 27);
    }
}
