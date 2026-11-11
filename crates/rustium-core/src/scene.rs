use std::collections::HashMap;
use std::sync::Arc;

use crate::ids::{NodeId, ShapeId};
use crate::material_graph::MaterialGraph;
use crate::node::{Graph, Node};
use crate::palette::Palette;
use crate::shape::{Shape, Volume};
use crate::transform::Transform;

/// A Model placed in the Scene, as the renderer and picking see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The Model Node.
    pub node: NodeId,
    /// The Shape it shows.
    pub shape: ShapeId,
    /// Transform from the Shape's cells to the Scene.
    pub transform: Transform,
    /// Position in drawing order, 0 for the Model highest in the tree; where Models overlap, lower ranks win.
    pub rank: u32,
}

/// The document being edited: a tree of Nodes, the Shapes they show, the Palette and the Voxel size.
#[derive(Debug, Clone)]
pub struct Scene {
    pub(crate) graph: Arc<Graph>,
    pub(crate) shapes: Vec<Shape>,
    pub(crate) palette: Palette,
    pub(crate) material_graphs: Vec<MaterialGraph>,
    voxel_size: f32,
}

impl Scene {
    /// Voxel size of a new Scene, in metres.
    pub const DEFAULT_VOXEL_SIZE: f32 = 0.25;

    /// A Scene with the default Palette and one empty Model called "Model".
    pub fn new() -> Self {
        Self::with_voxel_size(Self::DEFAULT_VOXEL_SIZE)
    }

    /// Like [`new`](Self::new) with voxels `voxel_size` metres wide (non-positive sizes fall back to the default).
    pub fn with_voxel_size(voxel_size: f32) -> Self {
        let voxel_size = if voxel_size > 0.0 {
            voxel_size
        } else {
            Self::DEFAULT_VOXEL_SIZE
        };
        let mut scene = Self {
            graph: Arc::new(Graph::new("Scene")),
            shapes: Vec::new(),
            palette: Palette::default(),
            material_graphs: vec![MaterialGraph::default_material()],
            voxel_size,
        };
        let shape = scene.add_shape(Shape::new());
        if let Some(shape) = shape {
            let graph = Arc::make_mut(&mut scene.graph);
            if let Some(model) = graph.push(Node {
                name: "Model".to_owned(),
                parent: None,
                transform: Transform::IDENTITY,
                hidden: false,
                locked: false,
                kind: crate::node::NodeKind::Model { shape },
            }) {
                graph.attach(NodeId::ROOT, 0, model);
            }
        }
        scene
    }

    /// Materials voxels can use.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Material graphs, shared by every Model; index 0 is the Default material.
    pub fn material_graphs(&self) -> &[MaterialGraph] {
        &self.material_graphs
    }

    /// Edge length of one voxel in metres.
    pub fn voxel_size(&self) -> f32 {
        self.voxel_size
    }

    /// The Node `id`, including deleted ones that an undo could bring back.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.graph.get(id)
    }

    /// The Shape `id`.
    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.get(id.index())
    }

    /// The Shape shown by the Model `node`, with its id; `None` for a Group or an unknown Node.
    pub fn model_shape(&self, node: NodeId) -> Option<(ShapeId, &Shape)> {
        let id = self.node(node)?.shape()?;
        Some((id, self.shape(id)?))
    }

    /// Whether `id` is part of the tree (not deleted).
    pub fn is_attached(&self, id: NodeId) -> bool {
        self.graph.is_attached(id)
    }

    /// Whether `ancestor` is `node` itself or one of its ancestors.
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, node: NodeId) -> bool {
        self.graph.is_ancestor_or_self(ancestor, node)
    }

    /// Transform from the local space of `id` to the Scene.
    pub fn world_transform(&self, id: NodeId) -> Transform {
        self.graph.world_transform(id)
    }

    /// Whether `id` or one of its ancestors is hidden.
    pub fn is_hidden(&self, id: NodeId) -> bool {
        self.any_ancestor_or_self(id, Node::is_hidden)
    }

    /// Whether `id` or one of its ancestors is locked.
    pub fn is_locked(&self, id: NodeId) -> bool {
        self.any_ancestor_or_self(id, Node::is_locked)
    }

    fn any_ancestor_or_self(&self, id: NodeId, test: fn(&Node) -> bool) -> bool {
        let mut current = Some(id);
        for _ in 0..=self.graph.len() {
            let Some(node) = current.and_then(|id| self.node(id)) else {
                return false;
            };
            if test(node) {
                return true;
            }
            current = node.parent();
        }
        false
    }

    /// `id` and everything below it in drawing order (parents first, topmost child first).
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        self.graph.descendants(id)
    }

    /// The Nodes from the root down to `id`; empty when `id` is unknown or deleted.
    pub fn path(&self, id: NodeId) -> Vec<NodeId> {
        if !self.is_attached(id) {
            return Vec::new();
        }
        let mut path = vec![id];
        let mut current = id;
        while let Some(parent) = self.node(current).and_then(Node::parent) {
            path.push(parent);
            current = parent;
        }
        path.reverse();
        path
    }

    /// The child of `ancestor` that contains `node`, or `node` itself when it is a direct child.
    pub fn child_toward(&self, ancestor: NodeId, node: NodeId) -> Option<NodeId> {
        let mut current = node;
        loop {
            let parent = self.node(current)?.parent()?;
            if parent == ancestor {
                return Some(current);
            }
            current = parent;
        }
    }

    /// Every visible Model in drawing order (hidden Nodes and their descendants are skipped).
    pub fn placements(&self) -> Vec<Placement> {
        let mut result = Vec::new();
        self.collect_placements(NodeId::ROOT, Transform::IDENTITY, &mut result);
        result
    }

    fn collect_placements(&self, id: NodeId, parent: Transform, out: &mut Vec<Placement>) {
        let Some(node) = self.node(id) else {
            return;
        };
        if node.hidden {
            return;
        }
        let transform = parent.compose(node.transform);
        match node.kind() {
            crate::node::NodeKind::Group { children } => {
                for child in children {
                    self.collect_placements(*child, transform, out);
                }
            }
            crate::node::NodeKind::Model { shape } => {
                out.push(Placement {
                    node: id,
                    shape: *shape,
                    transform,
                    rank: u32::try_from(out.len()).unwrap_or(u32::MAX),
                });
            }
        }
    }

    /// How many Models in the tree show each Shape; Shapes with a count above one are Instances.
    pub fn instance_counts(&self) -> HashMap<ShapeId, usize> {
        let mut counts = HashMap::new();
        for id in self.descendants(NodeId::ROOT) {
            if let Some(shape) = self.node(id).and_then(Node::shape) {
                *counts.entry(shape).or_insert(0) += 1;
            }
        }
        counts
    }

    /// The box around every Voxel under `id`, in Scene space; `None` when there are none.
    pub fn world_volume(&self, id: NodeId) -> Option<Volume> {
        let mut result: Option<Volume> = None;
        for node in self.descendants(id) {
            let Some((_, shape)) = self.model_shape(node) else {
                continue;
            };
            let Some(volume) = shape.volume() else {
                continue;
            };
            let moved = self.world_transform(node).volume(volume);
            result = Some(result.map_or(moved, |total| total.including(moved.min).including(moved.max)));
        }
        result
    }

    /// Number of filled Voxels in the Models under `id`, counting each Instance separately.
    pub fn used_voxels(&self, id: NodeId) -> u64 {
        self.descendants(id)
            .into_iter()
            .filter_map(|node| self.model_shape(node))
            .map(|(_, shape)| shape.used_count())
            .sum()
    }

    /// Adds a Shape to the arena.
    pub(crate) fn add_shape(&mut self, shape: Shape) -> Option<ShapeId> {
        let id = ShapeId::from_index(self.shapes.len())?;
        self.shapes.push(shape);
        Some(id)
    }

    pub(crate) fn shape_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        self.shapes.get_mut(id.index())
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_scene_has_a_root_group_with_one_empty_model() {
        let scene = Scene::new();
        let root = scene.node(NodeId::ROOT).unwrap();
        assert!(root.is_group());
        assert_eq!(root.children().len(), 1);
        let model = root.children()[0];
        assert!(scene.model_shape(model).unwrap().1.volume().is_none());
        assert_eq!(scene.placements().len(), 1);
    }
}
