use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use glam::IVec3;

use crate::chunk::{Chunk, ChunkCoord};
use crate::ids::{NodeId, ShapeId};
use crate::node::Graph;
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
    /// A Fill changed voxels.
    Fill,
    /// Selected voxels moved inside their Model.
    MoveVoxels,
    /// A Model was created.
    NewModel,
    /// A Group was created.
    NewGroup,
    /// A Node was copied with its own Shapes.
    Duplicate,
    /// A Node was copied sharing its Shapes.
    DuplicateAsInstance,
    /// A Model got a Shape of its own.
    MakeUnique,
    /// A Node was deleted.
    Delete,
    /// A Node was renamed.
    Rename,
    /// A Node was hidden or shown.
    Hide,
    /// A Node was locked or unlocked.
    Lock,
    /// Models were merged into one.
    Merge,
    /// A Selection became a new Model.
    Split,
    /// A Node moved to another place in the tree.
    MoveNode,
    /// A Node's Transform changed.
    Transform,
}

/// One named, undoable change: the chunks it touched before and after, the Volumes, and a snapshot of the node
/// graph when the graph changed.
#[derive(Debug, Clone)]
pub struct Edit {
    kind: EditKind,
    graph: Option<GraphChange>,
    chunks: Vec<ChunkChange>,
    volumes: Vec<VolumeChange>,
    created: Vec<NodeId>,
}

#[derive(Debug, Clone)]
struct GraphChange {
    before: Arc<Graph>,
    after: Arc<Graph>,
}

#[derive(Debug, Clone)]
struct ChunkChange {
    shape: ShapeId,
    coord: ChunkCoord,
    before: Option<Arc<Chunk>>,
    after: Option<Arc<Chunk>>,
}

#[derive(Debug, Clone)]
struct VolumeChange {
    shape: ShapeId,
    before: Option<Volume>,
    after: Option<Volume>,
}

impl Edit {
    /// What the Edit did.
    pub fn kind(&self) -> EditKind {
        self.kind
    }

    /// Chunks whose voxels the Edit changed, with the Shape they belong to.
    pub fn changed_chunks(&self) -> impl Iterator<Item = (ShapeId, ChunkCoord)> + '_ {
        self.chunks.iter().map(|change| (change.shape, change.coord))
    }

    /// Whether the Edit changed the node graph (the tree, names, Transforms, flags).
    pub fn graph_changed(&self) -> bool {
        self.graph.is_some()
    }

    /// Nodes the Edit created.
    pub fn created_nodes(&self) -> &[NodeId] {
        &self.created
    }

    /// Folds a later Edit into this one so a whole gesture undoes as a single step.
    #[must_use]
    pub fn merge(mut self, later: Self) -> Self {
        for change in later.chunks {
            match self
                .chunks
                .iter_mut()
                .find(|existing| existing.shape == change.shape && existing.coord == change.coord)
            {
                Some(existing) => existing.after = change.after,
                None => self.chunks.push(change),
            }
        }
        for change in later.volumes {
            match self.volumes.iter_mut().find(|existing| existing.shape == change.shape) {
                Some(existing) => existing.after = change.after,
                None => self.volumes.push(change),
            }
        }
        self.graph = match (self.graph.take(), later.graph) {
            (Some(first), Some(next)) => Some(GraphChange {
                before: first.before,
                after: next.after,
            }),
            (first, next) => first.or(next),
        };
        self.created.extend(later.created);
        self
    }

    fn restore(&self, scene: &mut Scene, direction: Direction) {
        for change in &self.chunks {
            let chunk = match direction {
                Direction::Back => change.before.as_ref(),
                Direction::Forward => change.after.as_ref(),
            };
            if let Some(shape) = scene.shape_mut(change.shape) {
                shape.replace_chunk(change.coord, chunk.map(Arc::clone));
            }
        }
        for change in &self.volumes {
            let volume = match direction {
                Direction::Back => change.before,
                Direction::Forward => change.after,
            };
            if let Some(shape) = scene.shape_mut(change.shape) {
                shape.set_volume(volume);
            }
        }
        if let Some(change) = &self.graph {
            scene.graph = match direction {
                Direction::Back => Arc::clone(&change.before),
                Direction::Forward => Arc::clone(&change.after),
            };
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Back,
    Forward,
}

/// Records the first state of everything it changes, so the change can become an [`Edit`].
#[derive(Debug)]
pub(crate) struct Recorder<'scene> {
    scene: &'scene mut Scene,
    graph_before: Option<Arc<Graph>>,
    chunks_before: HashMap<(ShapeId, ChunkCoord), Option<Arc<Chunk>>>,
    volumes_before: HashMap<ShapeId, Option<Volume>>,
    created: Vec<NodeId>,
}

impl<'scene> Recorder<'scene> {
    pub(crate) fn new(scene: &'scene mut Scene) -> Self {
        Self {
            scene,
            graph_before: None,
            chunks_before: HashMap::new(),
            volumes_before: HashMap::new(),
            created: Vec::new(),
        }
    }

    pub(crate) fn scene(&self) -> &Scene {
        self.scene
    }

    pub(crate) fn voxel(&self, shape: ShapeId, position: IVec3) -> Voxel {
        self.scene.shape(shape).and_then(|shape| shape.get(position))
    }

    pub(crate) fn set(&mut self, shape_id: ShapeId, position: IVec3, voxel: Voxel) {
        let Some(shape) = self.scene.shape_mut(shape_id) else {
            return;
        };
        if shape.get(position) == voxel {
            return;
        }
        let coord = ChunkCoord::of(position);
        self.volumes_before.entry(shape_id).or_insert_with(|| shape.volume());
        self.chunks_before
            .entry((shape_id, coord))
            .or_insert_with(|| shape.chunk(coord).map(Arc::clone));
        shape.set(position, voxel);
    }

    /// The node graph, ready to be changed; the state before the first call is what undo restores.
    pub(crate) fn graph(&mut self) -> &mut Graph {
        if self.graph_before.is_none() {
            self.graph_before = Some(Arc::clone(&self.scene.graph));
        }
        Arc::make_mut(&mut self.scene.graph)
    }

    /// Adds a Shape; Shapes are never removed, so an Edit that undoes the Model using it leaves it unused.
    pub(crate) fn add_shape(&mut self, shape: Shape) -> Option<ShapeId> {
        self.scene.add_shape(shape)
    }

    pub(crate) fn mark_created(&mut self, node: NodeId) {
        self.created.push(node);
    }

    /// The finished Edit, or `None` when nothing changed.
    pub(crate) fn finish(self, kind: EditKind) -> Option<Edit> {
        let graph = self.graph_before.map(|before| GraphChange {
            before,
            after: Arc::clone(&self.scene.graph),
        });
        let scene = self.scene;
        let chunks: Vec<ChunkChange> = self
            .chunks_before
            .into_iter()
            .map(|((shape, coord), before)| ChunkChange {
                shape,
                coord,
                before,
                after: scene.shape(shape).and_then(|shape| shape.chunk(coord)).map(Arc::clone),
            })
            .collect();
        let volumes: Vec<VolumeChange> = self
            .volumes_before
            .into_iter()
            .map(|(shape, before)| VolumeChange {
                shape,
                before,
                after: scene.shape(shape).and_then(Shape::volume),
            })
            .collect();
        if graph.is_none() && chunks.is_empty() {
            return None;
        }
        Some(Edit {
            kind,
            graph,
            chunks,
            volumes,
            created: self.created,
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
