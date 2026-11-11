use std::collections::HashMap;
use std::sync::Arc;

use glam::IVec3;

use crate::chunk::{self, Chunk, ChunkCoord};
use crate::palette::ColorId;

/// One voxel: empty (`None`) or showing a Color.
pub type Voxel = Option<ColorId>;

/// The box around the voxels of a Shape, with inclusive `min` and `max` corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// Lowest voxel position inside the Volume.
    pub min: IVec3,
    /// Highest voxel position inside the Volume.
    pub max: IVec3,
}

impl Volume {
    /// A Volume holding exactly one voxel.
    pub fn point(position: IVec3) -> Self {
        Self {
            min: position,
            max: position,
        }
    }

    /// Number of voxels along each axis.
    pub fn size(self) -> IVec3 {
        self.max - self.min + IVec3::ONE
    }

    /// The smallest Volume containing both `self` and `position`.
    #[must_use]
    pub fn including(self, position: IVec3) -> Self {
        Self {
            min: self.min.min(position),
            max: self.max.max(position),
        }
    }

    /// Whether `position` lies inside the Volume.
    pub fn contains(self, position: IVec3) -> bool {
        position.cmpge(self.min).all() && position.cmple(self.max).all()
    }
}

/// Unbounded voxel storage made of copy-on-write [`Chunk`]s; empty chunks are not stored.
#[derive(Debug, Clone, Default)]
pub struct Shape {
    chunks: HashMap<ChunkCoord, Arc<Chunk>>,
    volume: Option<Volume>,
}

impl Shape {
    /// An empty Shape.
    pub fn new() -> Self {
        Self::default()
    }

    /// Voxel at `position`.
    pub fn get(&self, position: IVec3) -> Voxel {
        self.chunks
            .get(&ChunkCoord::of(position))
            .and_then(|chunk| chunk.get(chunk::local_of(position)))
    }

    /// The stored chunk at `coord`, if it holds any voxel.
    pub fn chunk(&self, coord: ChunkCoord) -> Option<&Arc<Chunk>> {
        self.chunks.get(&coord)
    }

    /// Every stored chunk.
    pub fn chunks(&self) -> impl Iterator<Item = (ChunkCoord, &Arc<Chunk>)> {
        self.chunks.iter().map(|(coord, chunk)| (*coord, chunk))
    }

    /// Every filled voxel with its position and Color, in no particular order.
    pub fn voxels(&self) -> impl Iterator<Item = (IVec3, ColorId)> + '_ {
        self.chunks()
            .flat_map(|(coord, chunk)| chunk.iter().map(move |(local, color)| (coord.origin() + local, color)))
    }

    /// Number of filled voxels.
    pub fn used_count(&self) -> u64 {
        self.chunks.values().map(|chunk| u64::from(chunk.filled())).sum()
    }

    /// The Volume shown in the editor; `None` until the first voxel is attached.
    pub fn volume(&self) -> Option<Volume> {
        self.volume
    }

    /// Sets one voxel and returns whether it changed; filling a voxel grows the Volume.
    pub(crate) fn set(&mut self, position: IVec3, voxel: Voxel) -> bool {
        if self.get(position) == voxel {
            return false;
        }
        let coord = ChunkCoord::of(position);
        let chunk = self.chunks.entry(coord).or_insert_with(|| Arc::new(Chunk::empty()));
        let changed = Arc::make_mut(chunk).set(chunk::local_of(position), voxel);
        if voxel.is_none() && chunk.is_empty() {
            self.chunks.remove(&coord);
        }
        if voxel.is_some() {
            self.volume = Some(
                self.volume
                    .map_or(Volume::point(position), |volume| volume.including(position)),
            );
        }
        changed
    }

    /// Puts back a chunk exactly as an Edit recorded it.
    pub(crate) fn replace_chunk(&mut self, coord: ChunkCoord, chunk: Option<Arc<Chunk>>) {
        match chunk {
            Some(chunk) => self.chunks.insert(coord, chunk),
            None => self.chunks.remove(&coord),
        };
    }

    pub(crate) fn set_volume(&mut self, volume: Option<Volume>) {
        self.volume = volume;
    }
}
