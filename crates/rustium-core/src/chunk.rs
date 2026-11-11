use std::fmt;

use glam::IVec3;

use crate::shape::Voxel;

/// Edge length of a [`Chunk`] in voxels.
pub const CHUNK_SIZE: i32 = 32;

/// Number of voxels in one chunk.
pub(crate) const CHUNK_VOLUME: usize = 32 * 32 * 32;

/// Position of a [`Chunk`] in chunk units (voxel position divided by [`CHUNK_SIZE`], rounded down).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkCoord(pub IVec3);

impl ChunkCoord {
    /// The chunk containing the voxel at `position`.
    pub fn of(position: IVec3) -> Self {
        Self(position.div_euclid(IVec3::splat(CHUNK_SIZE)))
    }

    /// Position of this chunk's first voxel.
    pub fn origin(self) -> IVec3 {
        self.0 * CHUNK_SIZE
    }
}

/// Position of `position` inside its chunk, each component in `0..CHUNK_SIZE`.
pub(crate) fn local_of(position: IVec3) -> IVec3 {
    position.rem_euclid(IVec3::splat(CHUNK_SIZE))
}

/// A dense 32³ block of voxels; [`Shape`](crate::Shape)s share chunks through `Arc` until one is edited.
#[derive(Clone, PartialEq, Eq)]
pub struct Chunk {
    voxels: Box<[Voxel]>,
}

impl Chunk {
    /// A chunk with every voxel empty.
    pub fn empty() -> Self {
        Self {
            voxels: vec![None; CHUNK_VOLUME].into_boxed_slice(),
        }
    }

    /// Voxel at `local` (each component in `0..CHUNK_SIZE`); outside that range the voxel reads as empty.
    pub fn get(&self, local: IVec3) -> Voxel {
        index(local).and_then(|idx| self.voxels.get(idx).copied().flatten())
    }

    /// Sets the voxel at `local` and returns whether it changed.
    pub(crate) fn set(&mut self, local: IVec3, voxel: Voxel) -> bool {
        let Some(slot) = index(local).and_then(|idx| self.voxels.get_mut(idx)) else {
            debug_assert!(false, "local position {local} is outside the chunk");
            return false;
        };
        let changed = *slot != voxel;
        *slot = voxel;
        changed
    }

    /// Whether every voxel is empty.
    pub fn is_empty(&self) -> bool {
        self.voxels.iter().all(Option::is_none)
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let filled = self.voxels.iter().filter(|voxel| voxel.is_some()).count();
        f.debug_struct("Chunk").field("filled", &filled).finish()
    }
}

fn index(local: IVec3) -> Option<usize> {
    let size = CHUNK_SIZE;
    if local.cmplt(IVec3::ZERO).any() || local.cmpge(IVec3::splat(size)).any() {
        return None;
    }
    usize::try_from(local.x + local.z * size + local.y * size * size).ok()
}
