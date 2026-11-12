//! Hardware ray-query backend of the path tracer (ADR 0013): acceleration structures built from the data
//! [`TraceScene`] already uploaded.
//!
//! Every Shape becomes one bottom-level structure (BLAS) of procedural boxes, one per occupied chunk. Every visible
//! Model becomes an instance of its Shape's BLAS in the top-level structure (TLAS), placed by the Model's Transform.
//! The hardware only finds which chunk boxes a ray crosses; the shader then walks the voxels of that chunk with the
//! same DDA as the software backend, so materials and hits stay voxel-exact.
//!
//! Boxes, not greedy-meshed quads, because a box per chunk is a few hundred primitives for a whole model, builds in
//! microseconds, and only changes when a chunk appears or disappears: painting inside existing chunks costs no
//! rebuild. Triangles would need a rebuild on every voxel edit and cannot describe a voxel's interior for haze.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use rustium_core::{CHUNK_SIZE, ChunkCoord, ShapeId};
use wgpu::util::DeviceExt;

use crate::trace_scene::TraceScene;

/// Bytes of one procedural box: two `vec3<f32>` corners (the smallest stride wgpu accepts).
const BOX_STRIDE: u64 = wgpu::AABB_GEOMETRY_MIN_STRIDE;
/// How far a box reaches past its chunk, so rays grazing a chunk face are not lost to rounding in the hardware test.
const BOX_MARGIN: f32 = 0.01;
const MIN_INSTANCES: usize = 4;

/// One chunk box as the hardware reads it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct GpuBox {
    min: [f32; 3],
    max: [f32; 3],
}

const _: () = assert!(size_of::<GpuBox>() as u64 == BOX_STRIDE);

/// The BLAS of one Shape and the chunk set it was built for.
#[derive(Debug)]
struct ShapeStructure {
    /// Occupied chunks, in the order of the boxes.
    chunks: Vec<ChunkCoord>,
    blas: wgpu::Blas,
    boxes: wgpu::Buffer,
}

/// Acceleration structures for the visible Models of a Scene.
#[derive(Debug)]
pub(crate) struct RayScene {
    shapes: HashMap<ShapeId, ShapeStructure>,
    tlas: wgpu::Tlas,
    tlas_capacity: usize,
    /// `vec4<i32>` per box (chunk coordinate), all Shapes one after another.
    chunk_boxes: wgpu::Buffer,
    chunk_boxes_capacity: usize,
    /// Bumped whenever the TLAS or the box buffer is replaced, so the owner knows to rebuild its bind groups.
    generation: u64,
}

impl RayScene {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            shapes: HashMap::new(),
            tlas: create_tlas(device, MIN_INSTANCES),
            tlas_capacity: MIN_INSTANCES,
            chunk_boxes: create_chunk_boxes(device, 1),
            chunk_boxes_capacity: 1,
            generation: 0,
        }
    }

    pub(crate) fn tlas(&self) -> &wgpu::Tlas {
        &self.tlas
    }

    pub(crate) fn chunk_boxes(&self) -> &wgpu::Buffer {
        &self.chunk_boxes
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Brings the structures in line with the Model list `trace` holds after its upload. Shapes whose set of occupied
    /// chunks did not change keep their BLAS; the TLAS is rebuilt every time (a few instances).
    pub(crate) fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, trace: &TraceScene) {
        let placed = trace.placed();

        // BLAS: build for new Shapes and Shapes whose chunk set changed, drop the ones that left.
        let mut wanted: Vec<ShapeId> = Vec::new();
        let mut seen = HashSet::new();
        for (shape, _) in placed {
            if seen.insert(*shape) {
                wanted.push(*shape);
            }
        }
        self.shapes.retain(|shape, _| seen.contains(shape));
        let mut rebuilt: Vec<ShapeId> = Vec::new();
        for shape in &wanted {
            let chunks = trace.shape_chunks(*shape);
            if self.shapes.get(shape).is_some_and(|built| built.chunks == chunks) {
                continue;
            }
            self.shapes.insert(*shape, create_structure(device, chunks));
            rebuilt.push(*shape);
        }

        // Box table: where each Shape's boxes start, and the chunk coordinate of every box.
        let mut table: Vec<[i32; 4]> = Vec::new();
        let mut base_of: HashMap<ShapeId, u32> = HashMap::new();
        for shape in &wanted {
            let Some(built) = self.shapes.get(shape) else {
                continue;
            };
            base_of.insert(*shape, u32::try_from(table.len()).unwrap_or(0));
            table.extend(built.chunks.iter().map(|coord| [coord.0.x, coord.0.y, coord.0.z, 0]));
        }
        if table.len() > self.chunk_boxes_capacity {
            self.chunk_boxes_capacity = table.len().next_power_of_two();
            self.chunk_boxes = create_chunk_boxes(device, self.chunk_boxes_capacity);
            self.generation += 1;
        }
        if !table.is_empty() {
            queue.write_buffer(&self.chunk_boxes, 0, bytemuck::cast_slice(&table));
        }

        // TLAS: one instance per Model, in Model list order (the shader takes `instance_index` as the Model index).
        if placed.len() > self.tlas_capacity {
            self.tlas_capacity = placed.len().next_power_of_two();
            self.tlas = create_tlas(device, self.tlas_capacity);
            self.generation += 1;
        }
        for slot in 0..self.tlas_capacity {
            let instance = placed.get(slot).and_then(|(shape, record)| {
                let built = self.shapes.get(shape)?;
                let [cx, cy, cz, offset] = [record.column_x, record.column_y, record.column_z, record.offset];
                // Row-major 3×4: scene = columns * local + offset.
                let transform = [
                    cx[0], cy[0], cz[0], offset[0], //
                    cx[1], cy[1], cz[1], offset[1], //
                    cx[2], cy[2], cz[2], offset[2],
                ];
                Some(wgpu::TlasInstance::new(
                    &built.blas,
                    transform,
                    base_of.get(shape).copied().unwrap_or(0),
                    0xff,
                ))
            });
            self.tlas[slot] = instance;
        }

        let sizes: Vec<wgpu::BlasAABBGeometrySizeDescriptor> = rebuilt
            .iter()
            .map(|shape| wgpu::BlasAABBGeometrySizeDescriptor {
                primitive_count: self
                    .shapes
                    .get(shape)
                    .map_or(0, |built| u32::try_from(built.chunks.len()).unwrap_or(0)),
                flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
            })
            .collect();
        let entries: Vec<wgpu::BlasBuildEntry<'_>> = rebuilt
            .iter()
            .zip(&sizes)
            .filter_map(|(shape, size)| {
                let built = self.shapes.get(shape)?;
                Some(wgpu::BlasBuildEntry {
                    blas: &built.blas,
                    geometry: wgpu::BlasGeometries::AabbGeometries(vec![wgpu::BlasAabbGeometry {
                        size,
                        stride: BOX_STRIDE,
                        aabb_buffer: &built.boxes,
                        primitive_offset: 0,
                    }]),
                })
            })
            .collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("acceleration structure build"),
        });
        encoder.build_acceleration_structures(entries.iter(), std::iter::once(&self.tlas));
        queue.submit(std::iter::once(encoder.finish()));
    }
}

fn create_structure(device: &wgpu::Device, chunks: Vec<ChunkCoord>) -> ShapeStructure {
    let extent = CHUNK_SIZE as f32;
    let boxes: Vec<GpuBox> = chunks
        .iter()
        .map(|coord| {
            let min = coord.0.as_vec3() * extent;
            GpuBox {
                min: (min - BOX_MARGIN).to_array(),
                max: (min + extent + BOX_MARGIN).to_array(),
            }
        })
        .collect();
    // Buffers cannot be empty: a Shape without chunks gets one unused box.
    let mut bytes = bytemuck::cast_slice::<GpuBox, u8>(&boxes).to_vec();
    bytes.resize(boxes.len().max(1) * size_of::<GpuBox>(), 0);
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk boxes"),
        contents: &bytes,
        usage: wgpu::BufferUsages::BLAS_INPUT,
    });
    let blas = device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some("shape chunk boxes"),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::AABBs {
            descriptors: vec![wgpu::BlasAABBGeometrySizeDescriptor {
                primitive_count: u32::try_from(chunks.len()).unwrap_or(0),
                flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
            }],
        },
    );
    ShapeStructure {
        chunks,
        blas,
        boxes: buffer,
    }
}

fn create_tlas(device: &wgpu::Device, instances: usize) -> wgpu::Tlas {
    device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("scene instances"),
        max_instances: u32::try_from(instances).unwrap_or(u32::MAX),
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    })
}

fn create_chunk_boxes(device: &wgpu::Device, boxes: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trace chunk boxes"),
        size: boxes.max(1) as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
