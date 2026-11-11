use std::fmt;

use glam::IVec3;

use crate::palette::MaterialId;
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
    filled: u32,
}

impl Chunk {
    /// A chunk with every voxel empty.
    pub fn empty() -> Self {
        Self {
            voxels: vec![None; CHUNK_VOLUME].into_boxed_slice(),
            filled: 0,
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
        if changed {
            self.filled = self.filled + u32::from(voxel.is_some()) - u32::from(slot.is_some());
            *slot = voxel;
        }
        changed
    }

    /// Whether every voxel is empty.
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Number of filled voxels.
    pub fn filled(&self) -> u32 {
        self.filled
    }

    /// Every filled voxel as its position inside the chunk and its Material.
    pub fn iter(&self) -> impl Iterator<Item = (IVec3, MaterialId)> + '_ {
        self.voxels.iter().enumerate().filter_map(|(idx, voxel)| {
            let material = (*voxel)?;
            let idx = i32::try_from(idx).ok()?;
            let local = IVec3::new(
                idx % CHUNK_SIZE,
                idx / (CHUNK_SIZE * CHUNK_SIZE),
                (idx / CHUNK_SIZE) % CHUNK_SIZE,
            );
            Some((local, material))
        })
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk")
            .field("filled", &self.filled)
            .finish_non_exhaustive()
    }
}

fn index(local: IVec3) -> Option<usize> {
    let size = CHUNK_SIZE;
    if local.cmplt(IVec3::ZERO).any() || local.cmpge(IVec3::splat(size)).any() {
        return None;
    }
    usize::try_from(local.x + local.z * size + local.y * size * size).ok()
}
