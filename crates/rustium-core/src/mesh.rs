use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

use crate::chunk::{CHUNK_SIZE, Chunk, ChunkCoord};
use crate::face::Face;
use crate::shape::Shape;

/// One vertex of a chunk mesh, laid out for the GPU.
///
/// `packed` holds the face index (bits 0–2), ambient occlusion 0..=3 (bits 3–4, 3 = fully lit) and the
/// Material number (bits 8–15).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    /// Corner position in voxel space.
    pub position: [f32; 3],
    /// Face, ambient occlusion and Material, see the type docs.
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

const SIZE: usize = 32;
const PADDED: usize = SIZE + 2;
const PADDED_I32: i32 = CHUNK_SIZE + 2;

/// Builds the greedy mesh of chunk `coord` into `mesh`, culling faces hidden by neighbouring voxels.
///
/// Neighbouring chunks are read for face culling and ambient occlusion, so a chunk must be remeshed when a
/// voxel on the border of a neighbour changes.
// HOT PATH: runs for every changed chunk during a gesture; no allocations beyond growing `mesh`.
// ponytail: classic slice-mask greedy meshing; switch to binary greedy meshing (u64 column masks) if
// profiling shows remeshing over ~1 ms per chunk.
pub fn mesh_chunk(shape: &Shape, coord: ChunkCoord, mesh: &mut ChunkMesh) {
    mesh.vertices.clear();
    mesh.indices.clear();
    if shape.chunk(coord).is_none() {
        return;
    }
    let padded = PaddedChunk::new(shape, coord);
    let origin = coord.origin().as_vec3();
    let mut mask = [0_u32; SIZE * SIZE];

    for face in Face::ALL {
        let axis = face.axis();
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;
        let normal = face.normal();
        let u_dir = unit(u_axis);
        let v_dir = unit(v_axis);

        for depth in 0..CHUNK_SIZE {
            // Build the mask of visible faces in this slice; each entry encodes Material + 4 AO values.
            for v in 0..CHUNK_SIZE {
                for u in 0..CHUNK_SIZE {
                    let mut position = IVec3::ZERO;
                    position[axis] = depth;
                    position[u_axis] = u;
                    position[v_axis] = v;
                    let material = padded.get(position);
                    let key = if material != 0 && padded.get(position + normal) == 0 {
                        let ao = |su: i32, sv: i32| -> u32 {
                            let outside = position + normal;
                            let side_u = padded.get(outside + u_dir * su) != 0;
                            let side_v = padded.get(outside + v_dir * sv) != 0;
                            let corner = padded.get(outside + u_dir * su + v_dir * sv) != 0;
                            if side_u && side_v {
                                0
                            } else {
                                3 - u32::from(side_u) - u32::from(side_v) - u32::from(corner)
                            }
                        };
                        let ao = ao(-1, -1) | ao(1, -1) << 2 | ao(1, 1) << 4 | ao(-1, 1) << 6;
                        u32::from(material) | ao << 8 | 1 << 16
                    } else {
                        0
                    };
                    mask[slot(u, v)] = key;
                }
            }

            // Merge equal neighbours into rectangles.
            for v in 0..CHUNK_SIZE {
                let mut u = 0;
                while u < CHUNK_SIZE {
                    let key = mask[slot(u, v)];
                    if key == 0 {
                        u += 1;
                        continue;
                    }
                    let mut width = 1;
                    while u + width < CHUNK_SIZE && mask[slot(u + width, v)] == key {
                        width += 1;
                    }
                    let mut height = 1;
                    'grow: while v + height < CHUNK_SIZE {
                        for du in 0..width {
                            if mask[slot(u + du, v + height)] != key {
                                break 'grow;
                            }
                        }
                        height += 1;
                    }
                    for dv in 0..height {
                        for du in 0..width {
                            mask[slot(u + du, v + dv)] = 0;
                        }
                    }
                    let mut corner = IVec3::ZERO;
                    corner[axis] = depth + i32::from(face.is_positive());
                    corner[u_axis] = u;
                    corner[v_axis] = v;
                    push_quad(
                        mesh,
                        origin + corner.as_vec3(),
                        u_dir * width,
                        v_dir * height,
                        face,
                        key,
                    );
                    u += width;
                }
            }
        }
    }
}

fn push_quad(mesh: &mut ChunkMesh, corner: Vec3, u_span: IVec3, v_span: IVec3, face: Face, key: u32) {
    let Ok(base) = u32::try_from(mesh.vertices.len()) else {
        debug_assert!(false, "chunk mesh exceeds u32 vertex indices");
        return;
    };
    let material = key & 0xff;
    let ao = [(key >> 8) & 3, (key >> 10) & 3, (key >> 12) & 3, (key >> 14) & 3];
    let u_span = u_span.as_vec3();
    let v_span = v_span.as_vec3();
    let corners = [corner, corner + u_span, corner + u_span + v_span, corner + v_span];
    for (position, ao) in corners.iter().zip(ao) {
        mesh.vertices.push(MeshVertex {
            position: position.to_array(),
            packed: face.index() | ao << 3 | material << 8,
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

fn slot(u: i32, v: i32) -> usize {
    usize::try_from(u + v * CHUNK_SIZE).unwrap_or(0)
}

/// The chunk plus a one-voxel border from its 26 neighbours, as raw Material numbers (0 = empty).
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
                    if let (Some(material), Some(slot)) =
                        (voxel, padded_index(local).and_then(|index| voxels.get_mut(index)))
                    {
                        *slot = material.get();
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
    use crate::palette::MaterialId;

    fn exposed_faces(shape: &Shape, coord: ChunkCoord) -> u32 {
        let mut count = 0;
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let position = coord.origin() + IVec3::new(x, y, z);
                    if shape.get(position).is_none() {
                        continue;
                    }
                    for face in Face::ALL {
                        if shape.get(position + face.normal()).is_none() {
                            count += 1;
                        }
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
        shape.set(IVec3::new(4, 4, 4), MaterialId::new(1));
        let mut mesh = ChunkMesh::default();
        mesh_chunk(&shape, ChunkCoord(IVec3::ZERO), &mut mesh);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn flat_slab_merges_into_one_quad_per_side() {
        let mut shape = Shape::new();
        for x in 0..8 {
            for z in 0..8 {
                shape.set(IVec3::new(x, 0, z), MaterialId::new(1));
            }
        }
        let mut mesh = ChunkMesh::default();
        mesh_chunk(&shape, ChunkCoord(IVec3::ZERO), &mut mesh);
        assert_eq!(mesh.vertices.len() / 4, 6);
    }

    proptest! {
        #[test]
        fn greedy_mesh_covers_exactly_the_exposed_faces(
            voxels in prop::collection::vec((0..40_i32, 0..6_i32, 0..6_i32, 1..4_u8), 1..300)
        ) {
            let mut shape = Shape::new();
            for (x, y, z, material) in voxels {
                shape.set(IVec3::new(x - 4, y, z), MaterialId::new(material));
            }
            let mut mesh = ChunkMesh::default();
            for coord in [ChunkCoord(IVec3::ZERO), ChunkCoord(IVec3::new(1, 0, 0)), ChunkCoord(IVec3::new(-1, 0, 0))] {
                mesh_chunk(&shape, coord, &mut mesh);
                prop_assert!((quad_area(&mesh) - exposed_faces(&shape, coord) as f32).abs() < 1e-3);
            }
        }
    }
}
