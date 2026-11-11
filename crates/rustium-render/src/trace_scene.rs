//! The GPU copy of the voxel Scene that the path tracer walks: a pool of packed 32³ chunks shared by every Shape,
//! one dense chunk-index grid per Shape, and a list of the visible Models with their transforms.
//!
//! Every Shape is uploaded once, however many Models (Instances) show it; each Model record points at its Shape's
//! grid and carries the Transform that places it in the Scene.
//!
//! This module only uploads; traversal lives in `shaders/pathtracer.wgsl`. A hardware ray-query backend would
//! replace the traversal and build its acceleration structures from the same chunk and Model data.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, UVec3};
use rustium_core::{CHUNK_SIZE, Chunk, ChunkCoord, Scene, ShapeId};
use tracing::warn;

/// 32³ voxels, one byte each, packed four to a `u32`.
const WORDS_PER_CHUNK: usize = 8192;
const BYTES_PER_CHUNK: u64 = WORDS_PER_CHUNK as u64 * 4;
/// Most grid cells one Shape's dense chunk grid may have (16 MiB); a Shape spread wider than this is not traced.
// ponytail: dense grids; swap for hashed grids if Shapes spread over more than ~160 chunks per axis.
const MAX_GRID_CELLS: u64 = 4 * 1024 * 1024;
const MIN_SLOTS: usize = 16;

/// One visible Model as the shader reads it (`ModelRecord` in `shaders/pathtracer.wgsl`).
///
/// `column_*` are the images of the Shape's X, Y and Z axes (the Transform's orientation) and `offset` its
/// translation; like the raster pass they map Shape space to the Scene as `scene = columns * local + offset`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuModel {
    pub(crate) column_x: [f32; 4],
    pub(crate) column_y: [f32; 4],
    pub(crate) column_z: [f32; 4],
    pub(crate) offset: [f32; 4],
    /// xyz: chunk coordinate of the grid's first cell, w: where this Shape's grid starts in the chunk grid buffer.
    pub(crate) grid_min: [i32; 4],
    /// xyz: grid size in chunks.
    pub(crate) grid_size: [u32; 4],
}

const _: () = assert!(size_of::<GpuModel>() == 96);

#[derive(Debug)]
struct Resident {
    slot: u32,
    chunk: Arc<Chunk>,
}

/// Chunk pool, chunk grids, Model list and their bookkeeping.
#[derive(Debug)]
pub(crate) struct TraceScene {
    pool: wgpu::Buffer,
    pool_slots: usize,
    grids: wgpu::Buffer,
    grid_capacity: usize,
    models: wgpu::Buffer,
    model_capacity: usize,
    model_count: u32,
    resident: HashMap<(ShapeId, ChunkCoord), Resident>,
    free: Vec<u32>,
    next_slot: u32,
    /// Bumped whenever a buffer is replaced, so the owner knows to rebuild its bind groups.
    generation: u64,
}

impl TraceScene {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            pool: create_pool(device, MIN_SLOTS),
            pool_slots: MIN_SLOTS,
            grids: create_u32_buffer(device, "trace chunk grids", 4),
            grid_capacity: 4,
            models: create_models(device, 1),
            model_capacity: 1,
            model_count: 0,
            resident: HashMap::new(),
            free: Vec::new(),
            next_slot: 0,
            generation: 0,
        }
    }

    pub(crate) fn pool(&self) -> &wgpu::Buffer {
        &self.pool
    }

    pub(crate) fn grids(&self) -> &wgpu::Buffer {
        &self.grids
    }

    pub(crate) fn models(&self) -> &wgpu::Buffer {
        &self.models
    }

    pub(crate) fn model_count(&self) -> u32 {
        self.model_count
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Brings the GPU copy in line with `scene`: every visible Model, hidden ones skipped. Chunks whose `Arc` did
    /// not change are not uploaded again, and a Shape shown by several Models is uploaded once.
    // ponytail: rebuilds the grids and the Model list on every call; they are small next to the chunk pool.
    pub(crate) fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene) {
        let slot_limit =
            usize::try_from(device.limits().max_storage_buffer_binding_size / BYTES_PER_CHUNK).unwrap_or(usize::MAX);
        let placements = scene.placements();

        // Distinct Shapes in first-use order, and their chunks.
        let mut seen = HashSet::new();
        let mut wanted: Vec<(ShapeId, ChunkCoord, &Arc<Chunk>)> = Vec::new();
        let mut shapes: Vec<ShapeId> = Vec::new();
        for placement in &placements {
            if !seen.insert(placement.shape) {
                continue;
            }
            if let Some(shape) = scene.shape(placement.shape) {
                shapes.push(placement.shape);
                wanted.extend(shape.chunks().map(|(coord, chunk)| (placement.shape, coord, chunk)));
            }
        }
        if wanted.len() > slot_limit {
            warn!(
                chunks = wanted.len(),
                limit = slot_limit,
                "scene has more chunks than the GPU can bind, tracing a subset"
            );
            wanted.truncate(slot_limit);
        }

        // Drop chunks that left the Scene.
        let keep: HashSet<(ShapeId, ChunkCoord)> = wanted.iter().map(|(shape, coord, _)| (*shape, *coord)).collect();
        let stale: Vec<(ShapeId, ChunkCoord)> = self
            .resident
            .keys()
            .filter(|key| !keep.contains(key))
            .copied()
            .collect();
        for key in stale {
            if let Some(resident) = self.resident.remove(&key) {
                self.free.push(resident.slot);
            }
        }

        // Grow the pool when needed; a new buffer starts empty, so every chunk is written again.
        let needed = wanted.len().max(MIN_SLOTS);
        if needed > self.pool_slots {
            self.pool_slots = needed.next_power_of_two().min(slot_limit.max(MIN_SLOTS));
            self.pool = create_pool(device, self.pool_slots);
            self.resident.clear();
            self.free.clear();
            self.next_slot = 0;
            self.generation += 1;
        }

        let mut packed = vec![0_u32; WORDS_PER_CHUNK];
        for (shape, coord, chunk) in &wanted {
            let key = (*shape, *coord);
            if self
                .resident
                .get(&key)
                .is_some_and(|resident| Arc::ptr_eq(&resident.chunk, chunk))
            {
                continue;
            }
            let slot = match self.resident.get(&key) {
                Some(resident) => resident.slot,
                None => self.free.pop().unwrap_or_else(|| {
                    let slot = self.next_slot;
                    self.next_slot += 1;
                    slot
                }),
            };
            pack_chunk(chunk, &mut packed);
            queue.write_buffer(
                &self.pool,
                u64::from(slot) * BYTES_PER_CHUNK,
                bytemuck::cast_slice(&packed),
            );
            self.resident.insert(
                key,
                Resident {
                    slot,
                    chunk: Arc::clone(chunk),
                },
            );
        }

        // One dense grid per Shape, appended into one buffer.
        let mut entries: Vec<u32> = Vec::new();
        let mut grid_of: HashMap<ShapeId, (IVec3, UVec3, usize)> = HashMap::new();
        for shape in shapes {
            let coords: Vec<ChunkCoord> = self
                .resident
                .keys()
                .filter(|(owner, _)| *owner == shape)
                .map(|(_, coord)| *coord)
                .collect();
            let start = entries.len();
            if let Some((min, size)) = build_grid(&mut entries, shape, &coords, &self.resident) {
                grid_of.insert(shape, (min, size, start));
            }
        }
        if entries.len() > self.grid_capacity {
            self.grid_capacity = entries.len().next_power_of_two();
            self.grids = create_u32_buffer(device, "trace chunk grids", self.grid_capacity);
            self.generation += 1;
        }
        if !entries.is_empty() {
            queue.write_buffer(&self.grids, 0, bytemuck::cast_slice(&entries));
        }

        // The Model list: every visible Model whose Shape has a grid.
        let models: Vec<GpuModel> = placements
            .iter()
            .filter_map(|placement| {
                let (min, size, start) = *grid_of.get(&placement.shape)?;
                let [x, y, z] = placement.transform.orientation.columns().map(IVec3::as_vec3);
                let offset = placement.transform.offset.as_vec3();
                Some(GpuModel {
                    column_x: [x.x, x.y, x.z, 0.0],
                    column_y: [y.x, y.y, y.z, 0.0],
                    column_z: [z.x, z.y, z.z, 0.0],
                    offset: [offset.x, offset.y, offset.z, 0.0],
                    grid_min: [min.x, min.y, min.z, i32::try_from(start).unwrap_or(0)],
                    grid_size: [size.x, size.y, size.z, 0],
                })
            })
            .collect();
        if models.len() > self.model_capacity {
            self.model_capacity = models.len().next_power_of_two();
            self.models = create_models(device, self.model_capacity);
            self.generation += 1;
        }
        if !models.is_empty() {
            queue.write_buffer(&self.models, 0, bytemuck::cast_slice(&models));
        }
        self.model_count = u32::try_from(models.len()).unwrap_or(0);
    }
}

/// Appends the chunk grid of `shape` to `entries` and returns its first cell and size, or `None` when the Shape
/// has no chunks or is spread too wide.
fn build_grid(
    entries: &mut Vec<u32>,
    shape: ShapeId,
    coords: &[ChunkCoord],
    resident: &HashMap<(ShapeId, ChunkCoord), Resident>,
) -> Option<(IVec3, UVec3)> {
    let (min, max) = coords.iter().map(|coord| coord.0).fold(None, |bounds, coord| {
        Some(bounds.map_or((coord, coord), |(low, high): (IVec3, IVec3)| {
            (low.min(coord), high.max(coord))
        }))
    })?;
    let size = (max - min + IVec3::ONE).as_uvec3();
    let cells = u64::from(size.x) * u64::from(size.y) * u64::from(size.z);
    if cells > MAX_GRID_CELLS {
        warn!(cells, "a shape is spread too wide for the chunk grid, it is not traced");
        return None;
    }
    let start = entries.len();
    entries.resize(start + usize::try_from(cells).ok()?, 0);
    for coord in coords {
        let cell = (coord.0 - min).as_uvec3();
        let index = cell.x as usize + size.x as usize * (cell.z as usize + size.z as usize * cell.y as usize);
        if let (Some(entry), Some(resident)) = (entries.get_mut(start + index), resident.get(&(shape, *coord))) {
            *entry = resident.slot + 1;
        }
    }
    Some((min, size))
}

fn pack_chunk(chunk: &Chunk, out: &mut [u32]) {
    out.fill(0);
    let mut index = 0_usize;
    // Same order as the shader: x fastest, then z, then y.
    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                if let Some(material) = chunk.get(IVec3::new(x, y, z))
                    && let Some(word) = out.get_mut(index / 4)
                {
                    *word |= u32::from(material.get()) << ((index % 4) * 8);
                }
                index += 1;
            }
        }
    }
}

fn create_pool(device: &wgpu::Device, slots: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trace chunk pool"),
        size: slots as u64 * BYTES_PER_CHUNK,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_u32_buffer(device: &wgpu::Device, label: &str, words: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: words.max(4) as u64 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_models(device: &wgpu::Device, models: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trace models"),
        size: (models.max(1) * size_of::<GpuModel>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use rustium_core::{Brush, Command, Face, HitTarget, MaterialId, Mirror, NodeId, RayHit, VoxelTool, apply};

    use super::*;

    fn place(scene: &mut Scene, at: IVec3, material: MaterialId) {
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        let command = Command::ApplyBrush {
            model,
            tool: VoxelTool::Attach,
            brush: Brush::default(),
            hit: RayHit {
                voxel: at - IVec3::Y,
                face: Face::PosY,
                target: HitTarget::Ground,
            },
            end: None,
            material,
            mirror: Mirror::default(),
        };
        let _ = apply(scene, &command);
    }

    #[test]
    fn packing_puts_the_material_in_the_byte_the_shader_reads() {
        let mut scene = Scene::new();
        let material = MaterialId::new(7).unwrap();
        place(&mut scene, IVec3::new(1, 0, 0), material);
        place(&mut scene, IVec3::new(0, 2, 3), material);
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        let (_, shape) = scene.model_shape(model).unwrap();
        let chunk = shape.chunk(ChunkCoord(IVec3::ZERO)).unwrap();
        let mut words = vec![0_u32; WORDS_PER_CHUNK];
        pack_chunk(chunk, &mut words);
        // (1,0,0): index 1 -> byte 1 of word 0. (0,2,3): index 3*32 + 2*1024 = 2144 -> byte 0 of word 536.
        assert_eq!(words[0], 7 << 8);
        assert_eq!(words[536], 7);
        assert_eq!(words.iter().filter(|word| **word != 0).count(), 2);
        pack_chunk(&Chunk::empty(), &mut words);
        assert!(words.iter().all(|word| *word == 0));
    }

    #[test]
    fn grids_of_two_shapes_are_appended_back_to_back() {
        let mut scene = Scene::new();
        let new_model = Command::NewModel {
            parent: NodeId::ROOT,
            name: "Second".to_owned(),
        };
        apply(&mut scene, &new_model).unwrap();
        let children = scene.node(NodeId::ROOT).unwrap().children().to_vec();
        let a = scene.model_shape(children[0]).unwrap().0;
        let b = scene.model_shape(children[1]).unwrap().0;
        assert_ne!(a, b);
        let chunk = Arc::new(Chunk::empty());
        let resident = |slot| Resident {
            slot,
            chunk: Arc::clone(&chunk),
        };
        let mut map = HashMap::new();
        map.insert((a, ChunkCoord(IVec3::new(2, 0, 0))), resident(0));
        map.insert((a, ChunkCoord(IVec3::new(3, 0, 0))), resident(1));
        map.insert((b, ChunkCoord(IVec3::new(-1, 0, 0))), resident(2));
        let mut entries = Vec::new();
        let first = build_grid(
            &mut entries,
            a,
            &[ChunkCoord(IVec3::new(2, 0, 0)), ChunkCoord(IVec3::new(3, 0, 0))],
            &map,
        );
        assert_eq!(first, Some((IVec3::new(2, 0, 0), UVec3::new(2, 1, 1))));
        let second = build_grid(&mut entries, b, &[ChunkCoord(IVec3::new(-1, 0, 0))], &map);
        assert_eq!(second, Some((IVec3::new(-1, 0, 0), UVec3::ONE)));
        assert_eq!(entries, vec![1, 2, 3], "slot + 1 per cell, one grid after the other");
    }
}
