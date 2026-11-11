//! Commands that change the node graph: creating, copying, deleting and rearranging Nodes.

use std::collections::HashSet;

use glam::IVec3;

use crate::edit::{Edit, EditKind, Recorder};
use crate::error::Error;
use crate::ids::{NodeId, ShapeId};
use crate::node::{Node, NodeKind};
use crate::palette::MaterialId;
use crate::scene::Scene;
use crate::selection::Selection;
use crate::shape::Shape;
use crate::transform::Transform;

/// The Node `id` when it is part of the tree.
fn require_node(scene: &Scene, id: NodeId) -> Result<&Node, Error> {
    match scene.node(id) {
        Some(node) if scene.is_attached(id) => Ok(node),
        _ => Err(Error::UnknownNode(id)),
    }
}

fn require_group(scene: &Scene, id: NodeId) -> Result<(), Error> {
    if require_node(scene, id)?.is_group() {
        Ok(())
    } else {
        Err(Error::NotAGroup(id))
    }
}

/// The Shape of the Model `id`, which must be part of the tree, visible and unlocked.
pub(crate) fn editable_shape(scene: &Scene, id: NodeId) -> Result<ShapeId, Error> {
    let node = require_node(scene, id)?;
    let shape = node.shape().ok_or(Error::NotAModel(id))?;
    if scene.is_hidden(id) || scene.is_locked(id) {
        return Err(Error::ModelUnavailable(id));
    }
    Ok(shape)
}

fn require_not_root(id: NodeId) -> Result<(), Error> {
    if id == NodeId::ROOT {
        Err(Error::RootProtected)
    } else {
        Ok(())
    }
}

fn add_node(rec: &mut Recorder<'_>, parent: NodeId, index: usize, node: Node) -> Result<NodeId, Error> {
    let graph = rec.graph();
    let id = graph.push(node).ok_or(Error::TooManyNodes)?;
    graph.attach(parent, index, id);
    Ok(id)
}

fn blank(name: &str, kind: NodeKind) -> Node {
    Node {
        name: name.to_owned(),
        parent: None,
        transform: Transform::IDENTITY,
        hidden: false,
        locked: false,
        kind,
    }
}

pub(crate) fn new_model(scene: &mut Scene, parent: NodeId, name: &str) -> Result<Option<Edit>, Error> {
    require_group(scene, parent)?;
    let mut rec = Recorder::new(scene);
    let shape = rec.add_shape(Shape::new()).ok_or(Error::TooManyNodes)?;
    let id = add_node(&mut rec, parent, 0, blank(name, NodeKind::Model { shape }))?;
    rec.mark_created(id);
    Ok(rec.finish(EditKind::NewModel))
}

pub(crate) fn new_group(
    scene: &mut Scene,
    parent: NodeId,
    name: &str,
    adopt: &[NodeId],
) -> Result<Option<Edit>, Error> {
    require_group(scene, parent)?;
    for node in adopt {
        require_node(scene, *node)?;
        require_not_root(*node)?;
        if scene.is_ancestor_or_self(*node, parent) {
            return Err(Error::MoveIntoSelf(*node));
        }
    }
    let mut rec = Recorder::new(scene);
    let group = add_node(
        &mut rec,
        parent,
        0,
        blank(name, NodeKind::Group { children: Vec::new() }),
    )?;
    rec.mark_created(group);
    for node in adopt {
        reparent(&mut rec, *node, group, usize::MAX);
    }
    Ok(rec.finish(EditKind::NewGroup))
}

/// Moves `node` to `index` of `parent` keeping its place in the Scene; callers have validated the move.
fn reparent(rec: &mut Recorder<'_>, node: NodeId, parent: NodeId, index: usize) {
    let world = rec.scene().world_transform(node);
    let parent_world = rec.scene().world_transform(parent);
    let local = parent_world.inverse().compose(world);
    let graph = rec.graph();
    graph.detach(node);
    if let Some(entry) = graph.get_mut(node) {
        entry.transform = local;
    }
    graph.attach(parent, index, node);
}

pub(crate) fn move_node(scene: &mut Scene, node: NodeId, parent: NodeId, index: usize) -> Result<Option<Edit>, Error> {
    require_node(scene, node)?;
    require_not_root(node)?;
    require_group(scene, parent)?;
    if scene.is_ancestor_or_self(node, parent) {
        return Err(Error::MoveIntoSelf(node));
    }
    let mut rec = Recorder::new(scene);
    reparent(&mut rec, node, parent, index);
    Ok(rec.finish(EditKind::MoveNode))
}

pub(crate) fn duplicate(scene: &mut Scene, node: NodeId, share_shapes: bool) -> Result<Option<Edit>, Error> {
    let original = require_node(scene, node)?;
    require_not_root(node)?;
    let parent = original.parent().ok_or(Error::UnknownNode(node))?;
    let position = scene
        .node(parent)
        .and_then(|parent| parent.children().iter().position(|child| *child == node))
        .map_or(0, |index| index + 1);
    // Place the copy beside the original, along the world X axis, so it does not hide it.
    let width = scene.world_volume(node).map_or(0, |volume| volume.size().x);
    let shift = if width > 0 {
        scene
            .world_transform(parent)
            .orientation
            .inverse()
            .apply(IVec3::new(width + 1, 0, 0))
    } else {
        IVec3::ZERO
    };
    let mut rec = Recorder::new(scene);
    let copy = copy_subtree(&mut rec, node, share_shapes)?;
    let graph = rec.graph();
    if let Some(entry) = graph.get_mut(copy) {
        entry.transform.offset += shift;
    }
    graph.attach(parent, position, copy);
    rec.mark_created(copy);
    let kind = if share_shapes {
        EditKind::DuplicateAsInstance
    } else {
        EditKind::Duplicate
    };
    Ok(rec.finish(kind))
}

/// Copies `source` and everything below it as detached Nodes; Shapes are copied or shared.
fn copy_subtree(rec: &mut Recorder<'_>, source: NodeId, share_shapes: bool) -> Result<NodeId, Error> {
    let original = rec.scene().node(source).cloned().ok_or(Error::UnknownNode(source))?;
    match original.kind {
        NodeKind::Model { shape } => {
            let shape = if share_shapes {
                shape
            } else {
                let copy = rec.scene().shape(shape).cloned().unwrap_or_default();
                rec.add_shape(copy).ok_or(Error::TooManyNodes)?
            };
            let node = Node {
                parent: None,
                kind: NodeKind::Model { shape },
                ..original
            };
            rec.graph().push(node).ok_or(Error::TooManyNodes)
        }
        NodeKind::Group { children } => {
            let node = Node {
                parent: None,
                kind: NodeKind::Group { children: Vec::new() },
                ..original
            };
            let group = rec.graph().push(node).ok_or(Error::TooManyNodes)?;
            for child in children {
                let copy = copy_subtree(rec, child, share_shapes)?;
                rec.graph().attach(group, usize::MAX, copy);
            }
            Ok(group)
        }
    }
}

/// Gives the Model `model` a Shape of its own when it shares one; returns the Shape it ends up with.
fn ensure_unique(rec: &mut Recorder<'_>, model: NodeId) -> Result<ShapeId, Error> {
    let shape_id = rec
        .scene()
        .node(model)
        .and_then(Node::shape)
        .ok_or(Error::NotAModel(model))?;
    let shared = rec.scene().instance_counts().get(&shape_id).copied().unwrap_or(0) > 1;
    if !shared {
        return Ok(shape_id);
    }
    let copy = rec.scene().shape(shape_id).cloned().unwrap_or_default();
    let unique = rec.add_shape(copy).ok_or(Error::TooManyNodes)?;
    if let Some(node) = rec.graph().get_mut(model) {
        node.kind = NodeKind::Model { shape: unique };
    }
    Ok(unique)
}

pub(crate) fn make_unique(scene: &mut Scene, node: NodeId) -> Result<Option<Edit>, Error> {
    require_node(scene, node)?;
    let models: Vec<NodeId> = scene
        .descendants(node)
        .into_iter()
        .filter(|id| scene.node(*id).is_some_and(|entry| !entry.is_group()))
        .collect();
    let mut rec = Recorder::new(scene);
    for model in models {
        ensure_unique(&mut rec, model)?;
    }
    Ok(rec.finish(EditKind::MakeUnique))
}

pub(crate) fn delete(scene: &mut Scene, node: NodeId) -> Result<Option<Edit>, Error> {
    require_node(scene, node)?;
    require_not_root(node)?;
    let mut rec = Recorder::new(scene);
    rec.graph().detach(node);
    Ok(rec.finish(EditKind::Delete))
}

pub(crate) fn rename(scene: &mut Scene, node: NodeId, name: &str) -> Result<Option<Edit>, Error> {
    let current = require_node(scene, node)?;
    let name = name.trim();
    if name.is_empty() || name == current.name() {
        return Ok(None);
    }
    let mut rec = Recorder::new(scene);
    if let Some(entry) = rec.graph().get_mut(node) {
        name.clone_into(&mut entry.name);
    }
    Ok(rec.finish(EditKind::Rename))
}

pub(crate) fn set_hidden(scene: &mut Scene, node: NodeId, hidden: bool) -> Result<Option<Edit>, Error> {
    if require_node(scene, node)?.is_hidden() == hidden {
        return Ok(None);
    }
    let mut rec = Recorder::new(scene);
    if let Some(entry) = rec.graph().get_mut(node) {
        entry.hidden = hidden;
    }
    Ok(rec.finish(EditKind::Hide))
}

pub(crate) fn set_locked(scene: &mut Scene, node: NodeId, locked: bool) -> Result<Option<Edit>, Error> {
    if require_node(scene, node)?.is_locked() == locked {
        return Ok(None);
    }
    let mut rec = Recorder::new(scene);
    if let Some(entry) = rec.graph().get_mut(node) {
        entry.locked = locked;
    }
    Ok(rec.finish(EditKind::Lock))
}

pub(crate) fn set_transform(scene: &mut Scene, node: NodeId, transform: Transform) -> Result<Option<Edit>, Error> {
    if require_node(scene, node)?.transform() == transform {
        return Ok(None);
    }
    require_not_root(node)?;
    let mut rec = Recorder::new(scene);
    if let Some(entry) = rec.graph().get_mut(node) {
        entry.transform = transform;
    }
    Ok(rec.finish(EditKind::Transform))
}

/// Merges the Models in `nodes` (Groups stand for the Models below them) into the first of them in tree order.
pub(crate) fn merge(scene: &mut Scene, nodes: &[NodeId]) -> Result<Option<Edit>, Error> {
    let mut wanted = HashSet::new();
    for node in nodes {
        require_node(scene, *node)?;
        require_not_root(*node)?;
        wanted.extend(scene.descendants(*node));
    }
    let models: Vec<NodeId> = scene
        .descendants(NodeId::ROOT)
        .into_iter()
        .filter(|id| wanted.contains(id) && scene.node(*id).is_some_and(|entry| !entry.is_group()))
        .collect();
    let Some((&target, sources)) = models.split_first() else {
        return Ok(None);
    };
    if sources.is_empty() {
        return Ok(None);
    }
    let into_target = scene.world_transform(target).inverse();
    let mut moved: Vec<(IVec3, MaterialId)> = Vec::new();
    for source in sources {
        let Some((_, shape)) = scene.model_shape(*source) else {
            continue;
        };
        let transform = into_target.compose(scene.world_transform(*source));
        moved.extend(shape.voxels().map(|(cell, material)| (transform.cell(cell), material)));
    }
    let mut rec = Recorder::new(scene);
    let shape = ensure_unique(&mut rec, target)?;
    // Earlier Models are higher in the tree and win where they overlap.
    for (cell, material) in moved {
        if rec.voxel(shape, cell).is_none() {
            rec.set(shape, cell, Some(material));
        }
    }
    for node in nodes.iter().chain(sources) {
        if rec.scene().is_attached(*node) && !rec.scene().is_ancestor_or_self(*node, target) {
            rec.graph().detach(*node);
        }
    }
    Ok(rec.finish(EditKind::Merge))
}

/// Moves the Selection out of `model` into a new Model that sits right above it with the same Transform.
pub(crate) fn split(
    scene: &mut Scene,
    model: NodeId,
    selection: &Selection,
    name: &str,
) -> Result<Option<Edit>, Error> {
    let shape = editable_shape(scene, model)?;
    let parent = scene.node(model).and_then(Node::parent).ok_or(Error::RootProtected)?;
    let cells: Vec<_> = {
        let source = scene.shape(shape).ok_or(Error::UnknownNode(model))?;
        selection
            .iter()
            .filter_map(|cell| source.get(cell).map(|material| (cell, material)))
            .collect()
    };
    if cells.is_empty() {
        return Ok(None);
    }
    let index = scene
        .node(parent)
        .and_then(|entry| entry.children().iter().position(|child| *child == model))
        .unwrap_or(0);
    let transform = scene.node(model).map(Node::transform).unwrap_or_default();
    let mut rec = Recorder::new(scene);
    let source = ensure_unique(&mut rec, model)?;
    let target = rec.add_shape(Shape::new()).ok_or(Error::TooManyNodes)?;
    for (cell, material) in cells {
        rec.set(target, cell, Some(material));
        rec.set(source, cell, None);
    }
    let mut node = blank(name, NodeKind::Model { shape: target });
    node.transform = transform;
    let id = add_node(&mut rec, parent, index, node)?;
    rec.mark_created(id);
    Ok(rec.finish(EditKind::Split))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, VoxelTool, apply};
    use crate::edit::History;
    use crate::error::Error;
    use crate::face::Face;
    use crate::raycast::{HitTarget, RayHit};
    use crate::tools::{Brush, Mirror};
    use crate::transform::Orientation;

    fn cube(scene: &mut Scene, model: NodeId, at: IVec3) -> Edit {
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
            material: MaterialId::FIRST,
            mirror: Mirror::default(),
        };
        apply(scene, &command).unwrap().unwrap()
    }

    fn run(scene: &mut Scene, history: &mut History, command: &Command) -> Edit {
        let edit = apply(scene, command).unwrap().unwrap();
        history.push(edit.clone());
        edit
    }

    fn first_model(scene: &Scene) -> NodeId {
        scene.node(NodeId::ROOT).unwrap().children()[0]
    }

    fn children(scene: &Scene, node: NodeId) -> Vec<NodeId> {
        scene.node(node).unwrap().children().to_vec()
    }

    fn voxels(scene: &Scene, node: NodeId) -> u64 {
        scene.model_shape(node).unwrap().1.used_count()
    }

    fn new_group(name: &str, adopt: Vec<NodeId>) -> Command {
        Command::NewGroup {
            parent: NodeId::ROOT,
            name: name.to_owned(),
            adopt,
        }
    }

    #[test]
    fn graph_edits_round_trip_through_undo_and_redo() {
        let mut scene = Scene::new();
        let mut history = History::new(50);
        let model = first_model(&scene);
        let rename = Command::Rename {
            node: model,
            name: "Tree".to_owned(),
        };
        run(&mut scene, &mut history, &rename);
        let edit = run(&mut scene, &mut history, &new_group("G", vec![model]));
        let group = edit.created_nodes()[0];
        run(
            &mut scene,
            &mut history,
            &Command::SetHidden {
                node: group,
                hidden: true,
            },
        );
        assert_eq!(scene.node(model).unwrap().parent(), Some(group));
        assert_eq!(scene.placements().len(), 0);

        while history.can_undo() {
            history.undo(&mut scene);
        }
        assert_eq!(children(&scene, NodeId::ROOT), vec![model]);
        assert_eq!(scene.node(model).unwrap().name(), "Model");
        assert!(!scene.is_attached(group));

        while history.can_redo() {
            history.redo(&mut scene);
        }
        assert_eq!(scene.node(model).unwrap().parent(), Some(group));
        assert_eq!(scene.node(model).unwrap().name(), "Tree");
        assert!(scene.is_hidden(model));
    }

    #[test]
    fn editing_one_instance_edits_the_other_and_make_unique_separates_them() {
        let mut scene = Scene::new();
        let mut history = History::new(50);
        let model = first_model(&scene);
        cube(&mut scene, model, IVec3::ZERO);
        let edit = run(&mut scene, &mut history, &Command::DuplicateAsInstance { node: model });
        let twin = edit.created_nodes()[0];
        assert_eq!(scene.node(twin).unwrap().shape(), scene.node(model).unwrap().shape());
        let shape = scene.node(model).unwrap().shape().unwrap();
        assert_eq!(scene.instance_counts().get(&shape), Some(&2));

        cube(&mut scene, model, IVec3::new(0, 1, 0));
        assert_eq!(voxels(&scene, twin), 2);

        run(&mut scene, &mut history, &Command::MakeUnique { node: twin });
        cube(&mut scene, model, IVec3::new(0, 2, 0));
        assert_eq!((voxels(&scene, model), voxels(&scene, twin)), (3, 2));

        history.undo(&mut scene);
        assert_eq!(scene.node(twin).unwrap().shape(), scene.node(model).unwrap().shape());
    }

    #[test]
    fn duplicate_copies_voxels_and_leaves_room_beside_the_original() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        cube(&mut scene, model, IVec3::ZERO);
        let edit = apply(&mut scene, &Command::Duplicate { node: model }).unwrap().unwrap();
        let copy = edit.created_nodes()[0];
        assert_ne!(scene.node(copy).unwrap().shape(), scene.node(model).unwrap().shape());
        assert_eq!(voxels(&scene, copy), 1);
        assert_eq!(scene.node(copy).unwrap().transform().offset, IVec3::new(2, 0, 0));
        cube(&mut scene, model, IVec3::new(0, 1, 0));
        assert_eq!(voxels(&scene, copy), 1);
    }

    #[test]
    fn deleting_and_undoing_brings_the_subtree_back() {
        let mut scene = Scene::new();
        let mut history = History::new(50);
        let model = first_model(&scene);
        run(&mut scene, &mut history, &Command::Delete { node: model });
        assert!(!scene.is_attached(model));
        history.undo(&mut scene);
        assert!(scene.is_attached(model));
        let root = apply(&mut scene, &Command::Delete { node: NodeId::ROOT });
        assert!(matches!(root, Err(Error::RootProtected)));
    }

    #[test]
    fn merge_pulls_voxels_through_the_transforms_and_the_upper_model_wins() {
        let mut scene = Scene::new();
        let mut history = History::new(50);
        let lower = first_model(&scene);
        cube(&mut scene, lower, IVec3::ZERO);
        let new_model = Command::NewModel {
            parent: NodeId::ROOT,
            name: "Upper".to_owned(),
        };
        let upper = run(&mut scene, &mut history, &new_model).created_nodes()[0];
        cube(&mut scene, upper, IVec3::new(5, 0, 0));
        let shifted = Transform {
            orientation: Orientation::IDENTITY,
            offset: IVec3::new(-5, 0, 0),
        };
        run(
            &mut scene,
            &mut history,
            &Command::SetTransform {
                node: lower,
                transform: shifted,
            },
        );
        // `upper` is first in tree order and covers world cell (5, 0, 0); `lower` covers (-5, 0, 0).
        run(
            &mut scene,
            &mut history,
            &Command::Merge {
                nodes: vec![lower, upper],
            },
        );
        assert_eq!(children(&scene, NodeId::ROOT), vec![upper]);
        let shape = scene.model_shape(upper).unwrap().1;
        assert!(shape.get(IVec3::new(5, 0, 0)).is_some());
        assert!(shape.get(IVec3::new(-5, 0, 0)).is_some());
        history.undo(&mut scene);
        assert_eq!(children(&scene, NodeId::ROOT).len(), 2);
        assert_eq!(voxels(&scene, upper), 1);
    }

    #[test]
    fn split_moves_the_selection_into_a_new_model_in_the_same_place() {
        let mut scene = Scene::new();
        let mut history = History::new(50);
        let model = first_model(&scene);
        cube(&mut scene, model, IVec3::ZERO);
        cube(&mut scene, model, IVec3::new(4, 0, 0));
        let selection: Selection = [IVec3::new(4, 0, 0)].into_iter().collect();
        let split = Command::Split {
            model,
            selection,
            name: "Part".to_owned(),
        };
        let part = run(&mut scene, &mut history, &split).created_nodes()[0];
        assert_eq!((voxels(&scene, model), voxels(&scene, part)), (1, 1));
        assert_eq!(children(&scene, NodeId::ROOT), vec![part, model]);
        history.undo(&mut scene);
        assert_eq!(voxels(&scene, model), 2);
        assert_eq!(children(&scene, NodeId::ROOT), vec![model]);
    }

    #[test]
    fn moving_a_node_keeps_its_place_in_the_scene_and_refuses_cycles() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        let group = apply(&mut scene, &new_group("G", vec![]))
            .unwrap()
            .unwrap()
            .created_nodes()[0];
        let shift = Transform::translation(IVec3::new(7, 0, 0));
        apply(
            &mut scene,
            &Command::SetTransform {
                node: group,
                transform: shift,
            },
        )
        .unwrap();
        apply(
            &mut scene,
            &Command::MoveNode {
                node: model,
                parent: group,
                index: 0,
            },
        )
        .unwrap();
        assert_eq!(scene.world_transform(model), Transform::IDENTITY);
        assert_eq!(scene.node(model).unwrap().transform().offset, IVec3::new(-7, 0, 0));
        let cycle = apply(
            &mut scene,
            &Command::MoveNode {
                node: group,
                parent: group,
                index: 0,
            },
        );
        assert!(matches!(cycle, Err(Error::MoveIntoSelf(_))));
    }
}
