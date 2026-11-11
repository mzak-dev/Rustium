use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use glam::IVec3;

use crate::chunk::{Chunk, ChunkCoord};
use crate::scene::Scene;
use crate::shape::{Shape, Volume, Voxel};

/// What kind of change an [`Edit`] made; the interface turns it into a History label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Voxels were added.
    Attach,
    /// Voxels were removed.
    Erase,
    /// Voxels changed Material.
    Paint,
}

/// One named, undoable change: the chunks it touched before and after, plus the Volume.
#[derive(Debug, Clone)]
pub struct Edit {
    kind: EditKind,
    chunks: Vec<ChunkChange>,
    volume_before: Option<Volume>,
    volume_after: Option<Volume>,
}

#[derive(Debug, Clone)]
struct ChunkChange {
    coord: ChunkCoord,
    before: Option<Arc<Chunk>>,
    after: Option<Arc<Chunk>>,
}

impl Edit {
    /// What the Edit did.
    pub fn kind(&self) -> EditKind {
        self.kind
    }

    /// Chunks whose voxels the Edit changed.
    pub fn changed_chunks(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.chunks.iter().map(|change| change.coord)
    }

    /// Folds a later Edit into this one so a whole brush stroke undoes as a single step.
    #[must_use]
    pub fn merge(mut self, later: Self) -> Self {
        for change in later.chunks {
            match self.chunks.iter_mut().find(|existing| existing.coord == change.coord) {
                Some(existing) => existing.after = change.after,
                None => self.chunks.push(change),
            }
        }
        self.volume_after = later.volume_after;
        self
    }

    fn restore(&self, scene: &mut Scene, direction: Direction) {
        for change in &self.chunks {
            let chunk = match direction {
                Direction::Back => change.before.as_ref(),
                Direction::Forward => change.after.as_ref(),
            };
            scene.shape.replace_chunk(change.coord, chunk.map(Arc::clone));
        }
        let volume = match direction {
            Direction::Back => self.volume_before,
            Direction::Forward => self.volume_after,
        };
        scene.shape.set_volume(volume);
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Back,
    Forward,
}

/// Records the first state of every chunk it changes, so the change can become an [`Edit`].
#[derive(Debug)]
pub(crate) struct EditRecorder<'shape> {
    shape: &'shape mut Shape,
    before: HashMap<ChunkCoord, Option<Arc<Chunk>>>,
    volume_before: Option<Volume>,
}

impl<'shape> EditRecorder<'shape> {
    pub(crate) fn new(shape: &'shape mut Shape) -> Self {
        let volume_before = shape.volume();
        Self {
            shape,
            before: HashMap::new(),
            volume_before,
        }
    }

    pub(crate) fn get(&self, position: IVec3) -> Voxel {
        self.shape.get(position)
    }

    pub(crate) fn set(&mut self, position: IVec3, voxel: Voxel) {
        if self.shape.get(position) == voxel {
            return;
        }
        let coord = ChunkCoord::of(position);
        if !self.before.contains_key(&coord) {
            self.before.insert(coord, self.shape.chunk(coord).map(Arc::clone));
        }
        self.shape.set(position, voxel);
    }

    /// The finished Edit, or `None` when nothing changed.
    pub(crate) fn finish(self, kind: EditKind) -> Option<Edit> {
        if self.before.is_empty() {
            return None;
        }
        let chunks = self
            .before
            .into_iter()
            .map(|(coord, before)| ChunkChange {
                coord,
                before,
                after: self.shape.chunk(coord).map(Arc::clone),
            })
            .collect();
        Some(Edit {
            kind,
            chunks,
            volume_before: self.volume_before,
            volume_after: self.shape.volume(),
        })
    }
}

/// Undo and redo stacks of [`Edit`]s, keeping at most `limit` undo steps.
#[derive(Debug, Clone)]
pub struct History {
    done: VecDeque<Edit>,
    undone: Vec<Edit>,
    limit: usize,
}

impl History {
    /// Empty History that remembers at most `limit` Edits.
    pub fn new(limit: usize) -> Self {
        Self {
            done: VecDeque::new(),
            undone: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Changes how many Edits are kept, dropping the oldest ones if needed.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
        self.trim();
    }

    /// Records an Edit that was just applied; this clears the redo stack.
    pub fn push(&mut self, edit: Edit) {
        self.undone.clear();
        self.done.push_back(edit);
        self.trim();
    }

    /// Reverts the latest Edit and returns it.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<&Edit> {
        let edit = self.done.pop_back()?;
        edit.restore(scene, Direction::Back);
        self.undone.push(edit);
        self.undone.last()
    }

    /// Re-applies the latest undone Edit and returns it.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<&Edit> {
        let edit = self.undone.pop()?;
        edit.restore(scene, Direction::Forward);
        self.done.push_back(edit);
        self.done.back()
    }

    /// Applied Edits, oldest first.
    pub fn done(&self) -> impl Iterator<Item = &Edit> {
        self.done.iter()
    }

    /// Undone Edits in the order redo would apply them.
    pub fn undone(&self) -> impl Iterator<Item = &Edit> {
        self.undone.iter().rev()
    }

    /// Whether there is an Edit to undo.
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    /// Whether there is an Edit to redo.
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    fn trim(&mut self) {
        while self.done.len() > self.limit {
            self.done.pop_front();
        }
    }
}
