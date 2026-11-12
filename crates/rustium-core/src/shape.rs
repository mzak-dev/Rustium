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
    /// A voxel on the edge of the Volume was emptied, so the Volume may be too large until it is measured again.
    volume_stale: bool,
}

impl Shape {
    /// A Shape made of the given chunks; empty chunks are dropped and the Volume is measured from the voxels.
    pub(crate) fn from_chunks(chunks: impl IntoIterator<Item = (ChunkCoord, Arc<Chunk>)>) -> Self {
        let mut shape = Self {
            chunks: chunks.into_iter().filter(|(_, chunk)| !chunk.is_empty()).collect(),
            volume: None,
            volume_stale: false,
        };
        let mut volume: Option<Volume> = None;
        for (position, _) in shape.voxels() {
            volume = Some(volume.map_or(Volume::point(position), |total| total.including(position)));
        }
        shape.volume = volume;
        shape
    }

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
        } else if self
            .volume
            .is_some_and(|volume| position.cmpeq(volume.min).any() || position.cmpeq(volume.max).any())
        {
            self.volume_stale = true;
        }
        changed
    }

    /// A copy with the voxels at `cells` emptied, for drawing a Shape while some of its voxels are shown another
    /// way (an edit animation). Cheap: only the touched chunks are copied, and the Volume is left as it is.
    pub fn with_hidden(&self, cells: impl IntoIterator<Item = IVec3>) -> Self {
        let mut copy = self.clone();
        for cell in cells {
            let coord = ChunkCoord::of(cell);
            if let Some(chunk) = copy.chunks.get_mut(&coord) {
                Arc::make_mut(chunk).set(chunk::local_of(cell), None);
            }
        }
        copy
    }

    /// Shrinks the Volume to the voxels after an erase emptied its edge; cheap when nothing on the edge changed.
    pub(crate) fn refresh_volume(&mut self) {
        if std::mem::take(&mut self.volume_stale) {
            self.volume = self.measure_volume();
        }
    }

    /// The box around the filled voxels. The extreme voxel on each side lies in the outermost layer of chunks on that
    /// side, so only those chunks are scanned.
    fn measure_volume(&self) -> Option<Volume> {
        let mut coords = self.chunks.keys().map(|coord| coord.0);
        let first = coords.next()?;
        let (low, high) = coords.fold((first, first), |(low, high), coord| (low.min(coord), high.max(coord)));
        let mut min = IVec3::splat(i32::MAX);
        let mut max = IVec3::splat(i32::MIN);
        for (coord, chunk) in &self.chunks {
            let (at_low, at_high) = (coord.0.cmpeq(low), coord.0.cmpeq(high));
            if !(at_low.any() || at_high.any()) {
                continue;
            }
            for (local, _) in chunk.iter() {
                let position = coord.origin() + local;
                for axis in 0..3 {
                    if at_low.test(axis) {
                        min[axis] = min[axis].min(position[axis]);
                    }
                    if at_high.test(axis) {
                        max[axis] = max[axis].max(position[axis]);
                    }
                }
            }
        }
        Some(Volume { min, max })
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
        self.volume_stale = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CHUNK_SIZE;

    #[test]
    fn erasing_the_edge_shrinks_the_volume() {
        let mut shape = Shape::new();
        let far = IVec3::new(CHUNK_SIZE * 2 + 3, 1, -5);
        for position in [IVec3::ZERO, IVec3::new(4, 2, 1), far] {
            shape.set(position, ColorId::new(1));
        }
        shape.set(far, None);
        shape.refresh_volume();
        assert_eq!(
            shape.volume(),
            Some(Volume {
                min: IVec3::ZERO,
                max: IVec3::new(4, 2, 1)
            })
        );
        shape.set(IVec3::ZERO, None);
        shape.set(IVec3::new(4, 2, 1), None);
        shape.refresh_volume();
        assert_eq!(shape.volume(), None);
    }
}
