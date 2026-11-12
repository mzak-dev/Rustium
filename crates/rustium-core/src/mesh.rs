#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "loop counters and run lengths never exceed 34"
)]

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

use crate::chunk::{CHUNK_SIZE, Chunk, ChunkCoord};
use crate::face::Face;
use crate::material_graph::MaterialGraph;
use crate::palette::ColorId;
use crate::scene::Scene;
use crate::shape::Shape;

/// One vertex of a chunk mesh, laid out for the GPU.
///
/// `packed` holds the face index (bits 0–2), ambient occlusion 0..=3 (bits 3–4, 3 = fully lit) and the
/// Color number (bits 8–15).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    /// Corner position in voxel space.
    pub position: [f32; 3],
    /// Face, ambient occlusion and Color, see the type docs.
    pub packed: u32,
}

const _: () = assert!(size_of::<MeshVertex>() == 16);

/// Triangles for one chunk; reuse it between calls to avoid allocations.
#[derive(Debug, Clone, Default)]
pub struct ChunkMesh {
    /// Quad corners, four per quad.
    pub vertices: Vec<MeshVertex>,
    /// Two triangles per quad.
    pub indices: Vec<u32>,
}

/// Which Colors let light through, for [`mesh_chunk`].
///
/// A transparent Color belongs to the group of its Material. Faces between two voxels of the same group are
/// culled; an opaque voxel next to a transparent one keeps its face, and so do two different transparent groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transparency {
    /// Indexed by the Color number: 0 for an opaque Color, otherwise the Material index plus one.
    groups: [u32; 256],
}

impl Default for Transparency {
    fn default() -> Self {
        Self::OPAQUE
    }
}

impl Transparency {
    /// Every Color is opaque.
    pub const OPAQUE: Self = Self { groups: [0; 256] };

    /// The table for `scene`: a Color is transparent when its Material is (see
    /// [`MaterialGraph::is_transparent`](crate::MaterialGraph::is_transparent)).
    pub fn of_scene(scene: &Scene) -> Self {
        let graphs = scene.material_graphs();
        let mut table = Self::OPAQUE;
        for (id, color) in scene.palette().iter() {
            // A Color pointing at a graph that does not exist falls back to the Default material.
            let graph = if (color.graph as usize) < graphs.len() {
                color.graph
            } else {
                0
            };
            if graphs.get(graph as usize).is_some_and(MaterialGraph::is_transparent) {
                table.set(id, Some(graph));
            }
        }
        table
    }

    /// Marks `color` as transparent with the Material `graph`, or opaque with `None`.
    pub fn set(&mut self, color: ColorId, graph: Option<u32>) {
        if let Some(group) = self.groups.get_mut(usize::from(color.get())) {
            *group = graph.map_or(0, |graph| graph.saturating_add(1));
        }
    }

    /// 0 for an opaque Color (or an empty voxel), otherwise a number identifying the Color's Material.
    fn group(&self, color: u8) -> u32 {
        self.groups.get(usize::from(color)).copied().unwrap_or(0)
    }
}

const SIZE: usize = 32;
const PADDED: usize = SIZE + 2;
const PADDED_I32: i32 = CHUNK_SIZE + 2;

/// Builds the greedy mesh of chunk `coord` into `mesh`, culling faces hidden by neighbouring voxels.
///
/// A face is hidden by an opaque neighbour, or by a transparent neighbour of the same Material (see
/// [`Transparency`]). Transparent voxels do not darken their neighbours' ambient occlusion.
///
/// Neighbouring chunks are read for face culling and ambient occlusion, so a chunk must be remeshed when a
/// voxel on the border of a neighbour changes.
///
/// Binary greedy meshing: occupancy lives in `u64` column bitmasks (one bit per voxel along an axis, with the
/// one-voxel border), so the exposed faces of a whole column fall out of two shifts, and runs of equal faces
/// are merged with bit scans instead of per-voxel comparisons.
// HOT PATH: runs for every changed chunk during a gesture; no allocations beyond growing `mesh`.
pub fn mesh_chunk(shape: &Shape, coord: ChunkCoord, transparency: &Transparency, mesh: &mut ChunkMesh) {
    mesh.vertices.clear();
    mesh.indices.clear();
    if shape.chunk(coord).is_none() {
        return;
    }
    let padded = PaddedChunk::new(shape, coord);
    let columns = Columns::new(&padded, transparency);
    let origin = coord.origin().as_vec3();
    // `rows[depth * SIZE + v]` has bit `u` set when the voxel at (u, v) of that slice has an exposed face.
    let mut rows = [0_u32; SIZE * SIZE];
    let mut keys = [0_u32; SIZE * SIZE];

    for face in Face::ALL {
        let axis = face.axis();
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;
        let normal = face.normal();
        let u_dir = unit(u_axis);
        let v_dir = unit(v_axis);

        rows.fill(0);
        for v in 0..SIZE {
            for u in 0..SIZE {
                let (all, glass) = columns.column(axis, u, v);
                let opaque = all & !glass;
                let shift = |bits: u64| if face.is_positive() { bits >> 1 } else { bits << 1 };
                // Next to nothing, or an opaque voxel next to glass.
                let mut exposed = (all & !shift(all)) | (opaque & shift(glass));
                if columns.mixed_glass {
                    // Glass next to glass of another Material shows both faces.
                    let mut pairs = glass & shift(glass) & INNER_BITS;
                    while pairs != 0 {
                        let bit = pairs.trailing_zeros() as usize;
                        pairs &= pairs - 1;
                        let mut position = IVec3::ZERO;
                        position[axis] = bit as i32 - 1;
                        position[u_axis] = u as i32;
                        position[v_axis] = v as i32;
                        let own = transparency.group(padded.get(position));
                        if own != transparency.group(padded.get(position + normal)) {
                            exposed |= 1 << bit;
                        }
                    }
                }
                let mut bits = exposed & INNER_BITS;
                while bits != 0 {
                    let depth = bits.trailing_zeros() as usize - 1;
                    bits &= bits - 1;
                    if let Some(row) = rows.get_mut(depth * SIZE + v) {
                        *row |= 1 << u;
                    }
                }
            }
        }

        for depth in 0..SIZE {
            let Some(slice) = rows.get_mut(depth * SIZE..(depth + 1) * SIZE) else {
                continue;
            };
            // Key of every exposed face: Color + 4 AO values.
            for (v, &row) in slice.iter().enumerate() {
                let mut bits = row;
                while bits != 0 {
                    let u = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let mut position = IVec3::ZERO;
                    position[axis] = depth as i32;
                    position[u_axis] = u as i32;
                    position[v_axis] = v as i32;
                    if let Some(key) = keys.get_mut(v * SIZE + u) {
                        *key = face_key(&padded, transparency, position, normal, u_dir, v_dir);
                    }
                }
            }

            // Merge equal neighbours into rectangles.
            for v in 0..SIZE {
                while let Some(&row) = slice.get(v) {
                    if row == 0 {
                        break;
                    }
                    let u = row.trailing_zeros() as usize;
                    let key = keys.get(v * SIZE + u).copied().unwrap_or(0);
                    let run = (!(row >> u)).trailing_zeros() as usize;
                    let mut width = 1;
                    while width < run && keys.get(v * SIZE + u + width) == Some(&key) {
                        width += 1;
                    }
                    let span = (((1_u64 << width) - 1) as u32) << u;
                    let mut height = 1;
                    'grow: while v + height < SIZE {
                        if slice.get(v + height).is_none_or(|next| next & span != span) {
                            break;
                        }
                        for du in 0..width {
                            if keys.get((v + height) * SIZE + u + du) != Some(&key) {
                                break 'grow;
                            }
                        }
                        height += 1;
                    }
                    for next in slice.iter_mut().skip(v).take(height) {
                        *next &= !span;
                    }
                    let mut corner = IVec3::ZERO;
                    corner[axis] = depth as i32 + i32::from(face.is_positive());
                    corner[u_axis] = u as i32;
                    corner[v_axis] = v as i32;
                    push_quad(
                        mesh,
                        origin + corner.as_vec3(),
                        u_dir * width as i32,
                        v_dir * height as i32,
                        face,
                        key,
                    );
                }
            }
        }
    }
}

/// Bits 1..=32 of a column: the chunk's own voxels, without the border.
const INNER_BITS: u64 = ((1_u64 << SIZE) - 1) << 1;

/// Merge key of the exposed face of the voxel at `position`: Color (bits 0–7), four AO values (8–15) and a
/// marker bit so a key is never zero.
fn face_key(
    padded: &PaddedChunk,
    transparency: &Transparency,
    position: IVec3,
    normal: IVec3,
    u_dir: IVec3,
    v_dir: IVec3,
) -> u32 {
    let material = padded.get(position);
    let occludes = |at: IVec3| {
        let voxel = padded.get(at);
        voxel != 0 && transparency.group(voxel) == 0
    };
    let ao = |su: i32, sv: i32| -> u32 {
        let outside = position + normal;
        let side_u = occludes(outside + u_dir * su);
        let side_v = occludes(outside + v_dir * sv);
        let corner = occludes(outside + u_dir * su + v_dir * sv);
        if side_u && side_v {
            0
        } else {
            3 - u32::from(side_u) - u32::from(side_v) - u32::from(corner)
        }
    };
    let ao = ao(-1, -1) | ao(1, -1) << 2 | ao(1, 1) << 4 | ao(-1, 1) << 6;
    u32::from(material) | ao << 8 | 1 << 16
}

/// Occupancy of the padded chunk as one `u64` per voxel column and axis; bit `i` is padded coordinate `i`.
struct Columns {
    /// `[axis * PADDED² + padded v * PADDED + padded u]`, with u and v the two axes following `axis` cyclically.
    bits: Box<[u64]>,
    /// The same layout for the transparent voxels only.
    glass: Box<[u64]>,
    /// Whether voxels of more than one transparent Material are present.
    mixed_glass: bool,
}

impl Columns {
    // HOT PATH: one pass over the padded voxels.
    fn new(padded: &PaddedChunk, transparency: &Transparency) -> Self {
        let mut bits = vec![0_u64; 3 * PADDED * PADDED].into_boxed_slice();
        let mut glass = bits.clone();
        let mut first_group = 0;
        let mut mixed_glass = false;
        for (y, plane) in padded.voxels.as_chunks::<{ PADDED * PADDED }>().0.iter().enumerate() {
            for (z, line) in plane.as_chunks::<PADDED>().0.iter().enumerate() {
                for (x, &voxel) in line.iter().enumerate() {
                    if voxel == 0 {
                        continue;
                    }
                    // Axis 0 (X): u = Y, v = Z. Axis 1 (Y): u = Z, v = X. Axis 2 (Z): u = X, v = Y.
                    let group = transparency.group(voxel);
                    if group != 0 {
                        if first_group == 0 {
                            first_group = group;
                        }
                        mixed_glass |= group != first_group;
                    }
                    for (axis, index, bit) in [(0, z * PADDED + y, x), (1, x * PADDED + z, y), (2, y * PADDED + x, z)] {
                        let slot = axis * PADDED * PADDED + index;
                        if let Some(column) = bits.get_mut(slot) {
                            *column |= 1 << bit;
                        }
                        if group != 0
                            && let Some(column) = glass.get_mut(slot)
                        {
                            *column |= 1 << bit;
                        }
                    }
                }
            }
        }
        Self {
            bits,
            glass,
            mixed_glass,
        }
    }

    /// The occupied and the transparent column along `axis` at chunk-local `(u, v)`.
    fn column(&self, axis: usize, u: usize, v: usize) -> (u64, u64) {
        let slot = axis * PADDED * PADDED + (v + 1) * PADDED + u + 1;
        (
            self.bits.get(slot).copied().unwrap_or(0),
            self.glass.get(slot).copied().unwrap_or(0),
        )
    }
}

fn push_quad(mesh: &mut ChunkMesh, corner: Vec3, u_span: IVec3, v_span: IVec3, face: Face, key: u32) {
    let Ok(base) = u32::try_from(mesh.vertices.len()) else {
        debug_assert!(false, "chunk mesh exceeds u32 vertex indices");
        return;
    };
    let color = key & 0xff;
    let ao = [(key >> 8) & 3, (key >> 10) & 3, (key >> 12) & 3, (key >> 14) & 3];
    let u_span = u_span.as_vec3();
    let v_span = v_span.as_vec3();
    let corners = [corner, corner + u_span, corner + u_span + v_span, corner + v_span];
    for (position, ao) in corners.iter().zip(ao) {
        mesh.vertices.push(MeshVertex {
            position: position.to_array(),
            packed: face.index() | ao << 3 | color << 8,
        });
    }
    // Corners run counter-clockwise seen from the positive side; flip for negative faces. Split along the
    // diagonal with less occlusion difference so AO interpolates without a crease.
    let flip_diagonal = ao[0] + ao[2] < ao[1] + ao[3];
    let triangles: [u32; 6] = match (face.is_positive(), flip_diagonal) {
        (true, false) => [0, 1, 2, 0, 2, 3],
        (true, true) => [1, 2, 3, 1, 3, 0],
        (false, false) => [0, 2, 1, 0, 3, 2],
        (false, true) => [1, 3, 2, 1, 0, 3],
    };
    mesh.indices.extend(triangles.iter().map(|index| base + index));
}

fn unit(axis: usize) -> IVec3 {
    let mut vector = IVec3::ZERO;
    vector[axis] = 1;
    vector
}

/// The chunk plus a one-voxel border from its 26 neighbours, as raw Color numbers (0 = empty).
struct PaddedChunk {
    voxels: Box<[u8]>,
}

impl PaddedChunk {
    fn new(shape: &Shape, coord: ChunkCoord) -> Self {
        let mut neighbours: [Option<&Arc<Chunk>>; 27] = [None; 27];
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let offset = IVec3::new(x, y, z);
                    if let Some(slot) = neighbour_slot(offset + IVec3::ONE) {
                        neighbours[slot] = shape.chunk(ChunkCoord(coord.0 + offset));
                    }
                }
            }
        }
        let mut voxels = vec![0_u8; PADDED * PADDED * PADDED].into_boxed_slice();
        for y in -1..=CHUNK_SIZE {
            for z in -1..=CHUNK_SIZE {
                for x in -1..=CHUNK_SIZE {
                    let local = IVec3::new(x, y, z);
                    let chunk_offset = local.div_euclid(IVec3::splat(CHUNK_SIZE)) + IVec3::ONE;
                    let neighbour = neighbour_slot(chunk_offset).and_then(|slot| neighbours[slot]);
                    let Some(chunk) = neighbour else { continue };
                    let voxel = chunk.get(local.rem_euclid(IVec3::splat(CHUNK_SIZE)));
                    if let (Some(color), Some(slot)) =
                        (voxel, padded_index(local).and_then(|index| voxels.get_mut(index)))
                    {
                        *slot = color.get();
                    }
                }
            }
        }
        Self { voxels }
    }

    fn get(&self, local: IVec3) -> u8 {
        padded_index(local)
            .and_then(|index| self.voxels.get(index).copied())
            .unwrap_or(0)
    }
}

fn neighbour_slot(offset: IVec3) -> Option<usize> {
    usize::try_from(offset.x + offset.y * 3 + offset.z * 9).ok()
}

fn padded_index(local: IVec3) -> Option<usize> {
    let p = local + IVec3::ONE;
    let size = PADDED_I32;
    if p.cmplt(IVec3::ZERO).any() || p.cmpge(IVec3::splat(size)).any() {
        return None;
    }
    usize::try_from(p.x + p.z * size + p.y * size * size).ok()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::palette::ColorId;

    fn exposed_faces(shape: &Shape, coord: ChunkCoord, transparency: &Transparency) -> u32 {
        let mut count = 0;
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let position = coord.origin() + IVec3::new(x, y, z);
                    let Some(own) = shape.get(position).map(|color| transparency.group(color.get())) else {
                        continue;
                    };
                    for face in Face::ALL {
                        let visible = match shape.get(position + face.normal()) {
                            None => true,
                            Some(next) => {
                                let other = transparency.group(next.get());
                                other != 0 && (own == 0 || own != other)
                            }
                        };
                        count += u32::from(visible);
                    }
                }
            }
        }
        count
    }

    fn quad_area(mesh: &ChunkMesh) -> f32 {
        mesh.vertices
            .chunks(4)
            .map(|quad| {
                let [a, b, _, d] = [quad[0], quad[1], quad[2], quad[3]].map(|v| Vec3::from_array(v.position));
                (b - a).cross(d - a).length()
            })
            .sum()
    }

    #[test]
    fn single_voxel_has_six_quads() {
        let mut shape = Shape::new();
        shape.set(IVec3::new(4, 4, 4), ColorId::new(1));
        let mut mesh = ChunkMesh::default();
        mesh_chunk(&shape, ChunkCoord(IVec3::ZERO), &Transparency::OPAQUE, &mut mesh);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn flat_slab_merges_into_one_quad_per_side() {
        let mut shape = Shape::new();
        for x in 0..8 {
            for z in 0..8 {
                shape.set(IVec3::new(x, 0, z), ColorId::new(1));
            }
        }
        let mut mesh = ChunkMesh::default();
        mesh_chunk(&shape, ChunkCoord(IVec3::ZERO), &Transparency::OPAQUE, &mut mesh);
        assert_eq!(mesh.vertices.len() / 4, 6);
    }

    proptest! {
        #[test]
        fn greedy_mesh_covers_exactly_the_exposed_faces(
            voxels in prop::collection::vec((0..40_i32, 0..6_i32, 0..6_i32, 1..6_u8), 1..300),
            glass in prop::collection::vec(prop::option::of(0..3_u32), 6),
        ) {
            let mut transparency = Transparency::OPAQUE;
            for (number, group) in (1..=6_u8).zip(glass) {
                if let Some(color) = ColorId::new(number) {
                    transparency.set(color, group);
                }
            }
            let mut shape = Shape::new();
            for (x, y, z, color) in voxels {
                shape.set(IVec3::new(x - 4, y, z), ColorId::new(color));
            }
            let mut mesh = ChunkMesh::default();
            for coord in [ChunkCoord(IVec3::ZERO), ChunkCoord(IVec3::new(1, 0, 0)), ChunkCoord(IVec3::new(-1, 0, 0))] {
                mesh_chunk(&shape, coord, &transparency, &mut mesh);
                prop_assert!((quad_area(&mesh) - exposed_faces(&shape, coord, &transparency) as f32).abs() < 1e-3);
            }
        }
    }

    fn face_count(shape: &Shape, transparency: &Transparency) -> usize {
        let mut mesh = ChunkMesh::default();
        mesh_chunk(shape, ChunkCoord(IVec3::ZERO), transparency, &mut mesh);
        mesh.vertices.len() / 4
    }

    #[test]
    fn opaque_floor_under_glass_keeps_its_face() {
        let mut shape = Shape::new();
        shape.set(IVec3::new(2, 0, 2), ColorId::new(1));
        shape.set(IVec3::new(2, 1, 2), ColorId::new(2));
        let mut glass = Transparency::OPAQUE;
        glass.set(ColorId::new(2).unwrap(), Some(1));
        // Opaque: both hide the shared face (5 + 5). Glass above: the floor's top is kept, the glass bottom is not.
        assert_eq!(face_count(&shape, &Transparency::OPAQUE), 10);
        assert_eq!(face_count(&shape, &glass), 11);
    }

    #[test]
    fn glass_of_one_material_is_one_body_and_two_materials_show_both_faces() {
        let mut shape = Shape::new();
        shape.set(IVec3::new(2, 0, 2), ColorId::new(1));
        shape.set(IVec3::new(3, 0, 2), ColorId::new(2));
        let mut same = Transparency::OPAQUE;
        same.set(ColorId::new(1).unwrap(), Some(1));
        same.set(ColorId::new(2).unwrap(), Some(1));
        let mut different = same.clone();
        different.set(ColorId::new(2).unwrap(), Some(2));
        assert_eq!(face_count(&shape, &same), 10, "the shared face is culled");
        assert_eq!(
            face_count(&shape, &different),
            12,
            "the shared face shows on both sides"
        );
    }

    #[test]
    fn glass_does_not_darken_neighbours() {
        let mut shape = Shape::new();
        shape.set(IVec3::new(2, 0, 2), ColorId::new(1));
        shape.set(IVec3::new(3, 1, 2), ColorId::new(2));
        let mut mesh = ChunkMesh::default();
        let mut glass = Transparency::OPAQUE;
        glass.set(ColorId::new(2).unwrap(), Some(1));
        mesh_chunk(&shape, ChunkCoord(IVec3::ZERO), &glass, &mut mesh);
        let occluded = mesh
            .vertices
            .iter()
            .filter(|vertex| (vertex.packed >> 8) & 0xff == 1 && (vertex.packed >> 3) & 3 != 3)
            .count();
        assert_eq!(occluded, 0, "the opaque voxel has no occluded corner");
    }
}
