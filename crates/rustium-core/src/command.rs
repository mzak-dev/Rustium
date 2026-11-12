use glam::IVec3;

use crate::edit::{Edit, EditKind, Recorder};
use crate::error::Error;
use crate::ids::{NodeId, ShapeId};
use crate::material_graph::{GraphEdit, MaterialGraph, SurfaceConstants};
use crate::palette::ColorId;
use crate::raycast::RayHit;
use crate::scene::Scene;
use crate::scene_ops;
use crate::selection::Selection;
use crate::shape::Voxel;
use crate::tools::{self, Brush, FillMode, Mirror};
use crate::transform::Transform;

/// Longest Material name kept, in characters.
const MATERIAL_NAME_MAX: usize = 64;

/// Voxel Tools that change voxels with a [`Brush`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxelTool {
    /// Adds voxels against the hit face.
    Attach,
    /// Removes voxels around the hit voxel.
    Erase,
    /// Changes the Color of existing voxels around the hit voxel.
    Paint,
}

/// Farthest a voxel may lie from the origin along any axis.
pub const COORD_LIMIT: i32 = 1 << 24;

/// Most voxels one [`Command::SetVoxels`] may change.
pub const MAX_BATCH: usize = 4_000_000;

/// A request to change the Scene, from the interface, a script or an MCP client.
///
/// Hits are in the cell coordinates of the Model the Command names (not in Scene space).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Applies a voxel Tool with a Brush to a Model, where a ray hit it.
    ApplyBrush {
        /// The Model to edit; it must be visible and unlocked.
        model: NodeId,
        /// Which voxel Tool to use.
        tool: VoxelTool,
        /// Footprint of the gesture.
        brush: Brush,
        /// Where the gesture lands, or where it starts for Line and Box.
        hit: RayHit,
        /// Where a Line or Box gesture ends; `None` means the same as `hit`.
        end: Option<RayHit>,
        /// Color for Attach and Paint; ignored by Erase.
        color: ColorId,
        /// Reflections applied to every changed cell.
        mirror: Mirror,
    },
    /// Fills a face or an enclosed space of a Model.
    Fill {
        /// The Model to edit; it must be visible and unlocked.
        model: NodeId,
        /// Surface or 3D fill.
        mode: FillMode,
        /// The clicked voxel face.
        hit: RayHit,
        /// Color to fill with.
        color: ColorId,
        /// Reflections applied to every changed cell.
        mirror: Mirror,
    },
    /// Moves the selected voxels inside their Model; the Volume grows to hold them.
    MoveVoxels {
        /// The Model to edit; it must be visible and unlocked.
        model: NodeId,
        /// The voxels to move.
        selection: Selection,
        /// Distance in voxels along each axis of the Model.
        offset: IVec3,
    },
    /// Adds an empty Model at the top of a Group.
    NewModel {
        /// The Group to add to.
        parent: NodeId,
        /// Name of the new Model.
        name: String,
    },
    /// Adds an empty Group at the top of a Group and moves `adopt` into it, keeping their place in the Scene.
    NewGroup {
        /// The Group to add to.
        parent: NodeId,
        /// Name of the new Group.
        name: String,
        /// Nodes that become its children.
        adopt: Vec<NodeId>,
    },
    /// Copies a Node, with Shapes of its own, next to the original.
    Duplicate {
        /// The Node to copy.
        node: NodeId,
    },
    /// Copies a Node next to the original, sharing its Shapes so edits show in both.
    DuplicateAsInstance {
        /// The Node to copy.
        node: NodeId,
    },
    /// Gives every Model of a Node that shares a Shape a Shape of its own.
    MakeUnique {
        /// The Model, or a Group whose Models become unique.
        node: NodeId,
    },
    /// Removes a Node and everything below it.
    Delete {
        /// The Node to remove.
        node: NodeId,
    },
    /// Changes the name of a Node.
    Rename {
        /// The Node to rename.
        node: NodeId,
        /// The new name; blank names are ignored.
        name: String,
    },
    /// Hides or shows a Node and everything below it.
    SetHidden {
        /// The Node to change.
        node: NodeId,
        /// Whether it is hidden afterwards.
        hidden: bool,
    },
    /// Locks or unlocks a Node and everything below it.
    SetLocked {
        /// The Node to change.
        node: NodeId,
        /// Whether it is locked afterwards.
        locked: bool,
    },
    /// Merges Models into the first of them in tree order; Groups stand for the Models below them.
    Merge {
        /// The Nodes to merge.
        nodes: Vec<NodeId>,
    },
    /// Moves the Selection of a Model into a new Model above it.
    Split {
        /// The Model to split.
        model: NodeId,
        /// The voxels that leave it.
        selection: Selection,
        /// Name of the new Model.
        name: String,
    },
    /// Moves a Node to another place in the tree, keeping its place in the Scene.
    MoveNode {
        /// The Node to move.
        node: NodeId,
        /// The Group it moves into.
        parent: NodeId,
        /// Position among the children of `parent` once the Node has left its old place.
        index: usize,
    },
    /// Sets where a Node sits relative to its parent.
    SetTransform {
        /// The Node to change.
        node: NodeId,
        /// Its new Transform.
        transform: Transform,
    },
    /// Creates a material graph with the given Surface constants (clamped); it gets the next free index.
    CreateGraph {
        /// Name shown in the interface.
        name: String,
        /// The Surface output's constants.
        surface: SurfaceConstants,
    },
    /// Replaces the Surface constants of a material graph (clamped). The Default material (0) is locked.
    EditGraph {
        /// Index of the graph.
        index: u32,
        /// Its new constants.
        surface: SurfaceConstants,
    },
    /// Points a Palette entry at a material graph.
    AssignGraph {
        /// The Palette entry.
        id: ColorId,
        /// Index of the graph; 0 is the Default material.
        graph: u32,
    },
    /// Adds a finished Material (for example [`SampleMaterial::build`](crate::SampleMaterial::build) or
    /// [`MaterialGraph::blank`]); it gets the next free index. The graph must pass [`MaterialGraph::validate`].
    AddMaterial {
        /// The Material to add.
        graph: Box<MaterialGraph>,
    },
    /// Copies a Material (the copy is named "NAME copy") and appends it.
    DuplicateMaterial {
        /// Index of the Material to copy.
        index: u32,
    },
    /// Deletes a Material. Colors that used it fall back to the Default material and later indices shift down by
    /// one.
    DeleteMaterial {
        /// Index of the Material; the Default material (0) cannot be deleted.
        index: u32,
    },
    /// Renames a Material.
    RenameMaterial {
        /// Index of the Material.
        index: u32,
        /// The new name.
        name: String,
    },
    /// Turns "Casts shadow" of a Material on or off.
    SetCastsShadow {
        /// Index of the Material.
        index: u32,
        /// Whether voxels with this Material block light.
        casts_shadow: bool,
    },
    /// Points several Palette entries at a Material in one step.
    AssignGraphToColors {
        /// The Palette entries.
        ids: Vec<ColorId>,
        /// Index of the Material; 0 is the Default material.
        graph: u32,
    },
    /// Changes the inside of a Material's node graph. The Default material (0) is locked.
    EditMaterial {
        /// Index of the Material.
        index: u32,
        /// What to change.
        edit: GraphEdit,
    },
    /// Sets or erases voxels of a Model at explicit cell positions (`None` erases). The batch is
    /// all-or-nothing: an invalid entry rejects the whole Command.
    SetVoxels {
        /// The Model whose Shape changes.
        model: NodeId,
        /// Position (in the Model's cells) and new content of every voxel to change, applied in order.
        voxels: Vec<(IVec3, Voxel)>,
    },
    /// Changes the base color of one Palette entry.
    SetColor {
        /// Palette entry to change.
        id: ColorId,
        /// New base color as 8-bit sRGB.
        color: [u8; 3],
    },
}

/// Applies `command` to `scene` and returns the resulting Edit, or `None` when nothing changed.
///
/// # Errors
///
/// [`Error::UnknownColor`] when the Color is not in the Palette, [`Error::BrushSize`] when the brush is
/// empty or larger than [`Brush::MAX_SIZE`], [`Error::FillTooLarge`] for an oversized 3D Fill, and the Node errors
/// ([`Error::UnknownNode`], [`Error::NotAModel`], [`Error::NotAGroup`], [`Error::ModelUnavailable`],
/// [`Error::RootProtected`], [`Error::MoveIntoSelf`]) when a Command names a Node it cannot be applied to, and
/// [`Error::TooManyGraphs`], [`Error::UnknownGraph`] and [`Error::DefaultMaterialLocked`] for the material graph
/// Commands.
pub fn apply(scene: &mut Scene, command: &Command) -> Result<Option<Edit>, Error> {
    match command {
        Command::ApplyBrush {
            model,
            tool,
            brush,
            hit,
            end,
            color,
            mirror,
        } => apply_brush(
            scene,
            *model,
            *tool,
            *brush,
            (*hit, end.unwrap_or(*hit)),
            *color,
            *mirror,
        ),
        Command::Fill {
            model,
            mode,
            hit,
            color,
            mirror,
        } => apply_fill(scene, *model, *mode, *hit, *color, *mirror),
        Command::MoveVoxels {
            model,
            selection,
            offset,
        } => move_voxels(scene, *model, selection, *offset),
        Command::NewModel { parent, name } => scene_ops::new_model(scene, *parent, name),
        Command::NewGroup { parent, name, adopt } => scene_ops::new_group(scene, *parent, name, adopt),
        Command::Duplicate { node } => scene_ops::duplicate(scene, *node, false),
        Command::DuplicateAsInstance { node } => scene_ops::duplicate(scene, *node, true),
        Command::MakeUnique { node } => scene_ops::make_unique(scene, *node),
        Command::Delete { node } => scene_ops::delete(scene, *node),
        Command::Rename { node, name } => scene_ops::rename(scene, *node, name),
        Command::SetHidden { node, hidden } => scene_ops::set_hidden(scene, *node, *hidden),
        Command::SetLocked { node, locked } => scene_ops::set_locked(scene, *node, *locked),
        Command::Merge { nodes } => scene_ops::merge(scene, nodes),
        Command::Split { model, selection, name } => scene_ops::split(scene, *model, selection, name),
        Command::MoveNode { node, parent, index } => scene_ops::move_node(scene, *node, *parent, *index),
        Command::SetTransform { node, transform } => scene_ops::set_transform(scene, *node, *transform),
        Command::CreateGraph { name, surface } => create_graph(scene, name, *surface),
        Command::EditGraph { index, surface } => edit_graph(scene, *index, *surface),
        Command::AssignGraph { id, graph } => assign_graph(scene, *id, *graph),
        Command::AddMaterial { graph } => add_material(scene, graph),
        Command::DuplicateMaterial { index } => duplicate_material(scene, *index),
        Command::DeleteMaterial { index } => delete_material(scene, *index),
        Command::RenameMaterial { index, name } => rename_material(scene, *index, name),
        Command::SetCastsShadow { index, casts_shadow } => set_casts_shadow(scene, *index, *casts_shadow),
        Command::AssignGraphToColors { ids, graph } => assign_graph_to_colors(scene, ids, *graph),
        Command::EditMaterial { index, edit } => edit_material(scene, *index, edit),
        Command::SetVoxels { model, voxels } => apply_set_voxels(scene, *model, voxels),
        Command::SetColor { id, color } => apply_set_color(scene, *id, *color),
    }
}

fn check_position(position: IVec3) -> Result<(), Error> {
    if position.abs().max_element() > COORD_LIMIT {
        return Err(Error::OutOfRange {
            position,
            limit: COORD_LIMIT,
        });
    }
    Ok(())
}

fn apply_set_voxels(scene: &mut Scene, model: NodeId, voxels: &[(IVec3, Voxel)]) -> Result<Option<Edit>, Error> {
    let shape = scene_ops::editable_shape(scene, model)?;
    if voxels.len() > MAX_BATCH {
        return Err(Error::TooManyVoxels {
            len: voxels.len(),
            max: MAX_BATCH,
        });
    }
    for (position, voxel) in voxels {
        check_position(*position)?;
        if let Some(color) = voxel {
            check_color(scene, *color)?;
        }
    }
    let mut rec = Recorder::new(scene);
    for (position, voxel) in voxels {
        rec.set(shape, *position, *voxel);
    }
    Ok(rec.finish(EditKind::SetVoxels))
}

fn apply_set_color(scene: &mut Scene, id: ColorId, color: [u8; 3]) -> Result<Option<Edit>, Error> {
    check_color(scene, id)?;
    let before = snapshot(scene);
    if !scene.palette.set_color(id, color) {
        return Ok(None);
    }
    Ok(Some(Edit::materials_change(EditKind::SetColor, scene, before)))
}

fn snapshot(scene: &Scene) -> (crate::Palette, Vec<MaterialGraph>) {
    (scene.palette.clone(), scene.material_graphs.clone())
}

fn create_graph(scene: &mut Scene, name: &str, surface: SurfaceConstants) -> Result<Option<Edit>, Error> {
    add_material(scene, &MaterialGraph::from_constants(name, surface))
}

fn add_material(scene: &mut Scene, graph: &MaterialGraph) -> Result<Option<Edit>, Error> {
    if scene.material_graphs.len() >= MaterialGraph::MAX_PER_SCENE {
        return Err(Error::TooManyGraphs(scene.material_graphs.len()));
    }
    graph.validate()?;
    let before = snapshot(scene);
    scene.material_graphs.push(graph.clone());
    Ok(Some(Edit::materials_change(EditKind::CreateGraph, scene, before)))
}

fn duplicate_material(scene: &mut Scene, index: u32) -> Result<Option<Edit>, Error> {
    let mut copy = graph_at(scene, index)?.clone();
    copy.name = format!("{} copy", copy.name);
    add_material(scene, &copy).map(|edit| edit.map(|e| e.with_kind(EditKind::DuplicateGraph)))
}

fn delete_material(scene: &mut Scene, index: u32) -> Result<Option<Edit>, Error> {
    if index == 0 {
        return Err(Error::DefaultMaterialLocked);
    }
    graph_at(scene, index)?;
    let before = snapshot(scene);
    scene.material_graphs.remove(slot(index));
    for id in (1..=u8::MAX).filter_map(ColorId::new) {
        if let Some(color) = scene.palette.get_mut(id) {
            if color.graph == index {
                color.graph = 0;
            } else if color.graph > index {
                color.graph -= 1;
            }
        }
    }
    Ok(Some(Edit::materials_change(EditKind::DeleteGraph, scene, before)))
}

fn rename_material(scene: &mut Scene, index: u32, name: &str) -> Result<Option<Edit>, Error> {
    let name: String = name.trim().chars().take(MATERIAL_NAME_MAX).collect();
    let before = snapshot(scene);
    let graph = unlocked_graph(scene, index)?;
    if graph.name == name {
        return Ok(None);
    }
    graph.name = name;
    Ok(Some(Edit::materials_change(EditKind::RenameGraph, scene, before)))
}

fn set_casts_shadow(scene: &mut Scene, index: u32, casts_shadow: bool) -> Result<Option<Edit>, Error> {
    let before = snapshot(scene);
    let graph = unlocked_graph(scene, index)?;
    if graph.casts_shadow == casts_shadow {
        return Ok(None);
    }
    graph.casts_shadow = casts_shadow;
    Ok(Some(Edit::materials_change(EditKind::CastsShadow, scene, before)))
}

fn edit_material(scene: &mut Scene, index: u32, edit: &GraphEdit) -> Result<Option<Edit>, Error> {
    let before = snapshot(scene);
    let graph = unlocked_graph(scene, index)?;
    if !graph.apply_edit(edit)? {
        return Ok(None);
    }
    Ok(Some(Edit::materials_change(edit.kind(), scene, before)))
}

fn assign_graph_to_colors(scene: &mut Scene, ids: &[ColorId], graph: u32) -> Result<Option<Edit>, Error> {
    graph_at(scene, graph)?;
    for id in ids {
        check_color(scene, *id)?;
    }
    let before = snapshot(scene);
    let mut changed = false;
    for id in ids {
        if let Some(color) = scene.palette.get_mut(*id)
            && color.graph != graph
        {
            color.graph = graph;
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some(Edit::materials_change(EditKind::AssignGraph, scene, before)))
}

fn slot(index: u32) -> usize {
    usize::try_from(index).unwrap_or(usize::MAX)
}

fn graph_at(scene: &Scene, index: u32) -> Result<&MaterialGraph, Error> {
    scene.material_graphs.get(slot(index)).ok_or(Error::UnknownGraph(index))
}

/// The graph at `index`, unless it is the locked Default material.
fn unlocked_graph(scene: &mut Scene, index: u32) -> Result<&mut MaterialGraph, Error> {
    if index == 0 {
        return Err(Error::DefaultMaterialLocked);
    }
    scene
        .material_graphs
        .get_mut(slot(index))
        .ok_or(Error::UnknownGraph(index))
}

fn edit_graph(scene: &mut Scene, index: u32, surface: SurfaceConstants) -> Result<Option<Edit>, Error> {
    let before = snapshot(scene);
    if !unlocked_graph(scene, index)?.set_surface_constants(surface) {
        return Ok(None);
    }
    Ok(Some(Edit::materials_change(EditKind::EditGraph, scene, before)))
}

fn assign_graph(scene: &mut Scene, id: ColorId, graph: u32) -> Result<Option<Edit>, Error> {
    if usize::try_from(graph).map_or(true, |slot| slot >= scene.material_graphs.len()) {
        return Err(Error::UnknownGraph(graph));
    }
    let before = snapshot(scene);
    let color = scene.palette.get_mut(id).ok_or(Error::UnknownColor(id.get()))?;
    if color.graph == graph {
        return Ok(None);
    }
    color.graph = graph;
    Ok(Some(Edit::materials_change(EditKind::AssignGraph, scene, before)))
}

/// What to do with each cell a gesture reaches.
#[derive(Debug, Clone, Copy)]
enum Op {
    /// Fill the cell when it is empty.
    Place(ColorId),
    /// Empty the cell.
    Remove,
    /// Change the Color when the cell is filled.
    Recolor(ColorId),
}

fn check_color(scene: &Scene, color: ColorId) -> Result<(), Error> {
    if scene.palette().get(color).is_none() {
        return Err(Error::UnknownColor(color.get()));
    }
    Ok(())
}

fn apply_to_cells(rec: &mut Recorder<'_>, shape: ShapeId, op: Op, cells: &[IVec3], mirror: Mirror) {
    for cell in cells {
        mirror.for_each_image(*cell, |image| {
            let current = rec.voxel(shape, image);
            match op {
                Op::Place(color) if current.is_none() => rec.set(shape, image, Some(color)),
                Op::Remove => rec.set(shape, image, None),
                Op::Recolor(color) if current.is_some() => rec.set(shape, image, Some(color)),
                Op::Place(_) | Op::Recolor(_) => {}
            }
        });
    }
}

fn apply_brush(
    scene: &mut Scene,
    model: NodeId,
    tool: VoxelTool,
    brush: Brush,
    (from, to): (RayHit, RayHit),
    color: ColorId,
    mirror: Mirror,
) -> Result<Option<Edit>, Error> {
    let shape = scene_ops::editable_shape(scene, model)?;
    check_color(scene, color)?;
    check_position(from.voxel)?;
    check_position(to.voxel)?;
    if brush.size == 0 || brush.size > Brush::MAX_SIZE {
        return Err(Error::BrushSize {
            size: brush.size,
            max: Brush::MAX_SIZE,
        });
    }
    let cells = {
        let shape = scene.shape(shape).ok_or(Error::UnknownNode(model))?;
        tools::brush_positions(shape, tool, brush, from, to)
    };
    let (op, kind) = match tool {
        VoxelTool::Attach => (Op::Place(color), EditKind::Attach),
        VoxelTool::Erase => (Op::Remove, EditKind::Erase),
        VoxelTool::Paint => (Op::Recolor(color), EditKind::Paint),
    };
    let mut rec = Recorder::new(scene);
    apply_to_cells(&mut rec, shape, op, &cells, mirror);
    Ok(rec.finish(kind))
}

fn apply_fill(
    scene: &mut Scene,
    model: NodeId,
    mode: FillMode,
    hit: RayHit,
    color: ColorId,
    mirror: Mirror,
) -> Result<Option<Edit>, Error> {
    let shape = scene_ops::editable_shape(scene, model)?;
    check_color(scene, color)?;
    let (cells, op) = {
        let shape = scene.shape(shape).ok_or(Error::UnknownNode(model))?;
        match mode {
            FillMode::Surface => (tools::face_region(shape, hit), Op::Recolor(color)),
            FillMode::Volume => (tools::fill_volume(shape, hit)?, Op::Place(color)),
        }
    };
    let mut rec = Recorder::new(scene);
    apply_to_cells(&mut rec, shape, op, &cells, mirror);
    Ok(rec.finish(EditKind::Fill))
}

fn move_voxels(scene: &mut Scene, model: NodeId, selection: &Selection, offset: IVec3) -> Result<Option<Edit>, Error> {
    let shape = scene_ops::editable_shape(scene, model)?;
    if offset == IVec3::ZERO {
        return Ok(None);
    }
    let moving: Vec<_> = {
        let source = scene.shape(shape).ok_or(Error::UnknownNode(model))?;
        selection
            .iter()
            .filter_map(|cell| source.get(cell).map(|color| (cell, color)))
            .collect()
    };
    let mut rec = Recorder::new(scene);
    for (cell, _) in &moving {
        rec.set(shape, *cell, None);
    }
    for (cell, color) in &moving {
        rec.set(shape, *cell + offset, Some(*color));
    }
    Ok(rec.finish(EditKind::MoveVoxels))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::edit::History;
    use crate::face::Face;
    use crate::raycast::HitTarget;
    use crate::tools::BrushKind;
    use crate::{CHUNK_SIZE, ChunkCoord};

    fn color_id(number: u8) -> ColorId {
        ColorId::new(number).unwrap()
    }

    fn first_model(scene: &Scene) -> NodeId {
        scene.node(NodeId::ROOT).unwrap().children()[0]
    }

    fn ground_hit(at: IVec3) -> RayHit {
        RayHit {
            voxel: at - IVec3::Y,
            face: Face::PosY,
            target: HitTarget::Ground,
        }
    }

    fn attach_command(model: NodeId, at: IVec3, size: u32) -> Command {
        Command::ApplyBrush {
            model,
            tool: VoxelTool::Attach,
            brush: Brush {
                kind: BrushKind::Voxel,
                size,
            },
            hit: ground_hit(at),
            end: None,
            color: color_id(1),
            mirror: Mirror::default(),
        }
    }

    fn attach(scene: &mut Scene, at: IVec3, size: u32) -> Option<Edit> {
        let model = first_model(scene);
        apply(scene, &attach_command(model, at, size)).unwrap()
    }

    fn shape_of(scene: &Scene, node: NodeId) -> &crate::Shape {
        scene.model_shape(node).unwrap().1
    }

    #[test]
    fn attach_on_the_ground_fills_a_cube_above_it() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 3);
        let shape = shape_of(&scene, first_model(&scene));
        assert_eq!(shape.volume().map(crate::Volume::size), Some(IVec3::splat(3)));
        assert!(shape.get(IVec3::new(0, 2, 0)).is_some());
        assert!(shape.get(IVec3::new(0, -1, 0)).is_none());
    }

    #[test]
    fn volume_grows_when_attaching_outside_it() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 1);
        attach(&mut scene, IVec3::new(40, 0, 0), 1);
        let volume = shape_of(&scene, first_model(&scene)).volume().unwrap();
        assert_eq!(volume.min, IVec3::ZERO);
        assert_eq!(volume.max, IVec3::new(40, 0, 0));
    }

    #[test]
    fn undo_and_redo_restore_the_same_chunks_across_a_chunk_border() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let model = first_model(&scene);
        let border = IVec3::new(CHUNK_SIZE - 1, 0, 0);
        history.push(attach(&mut scene, IVec3::ZERO, 1).unwrap());
        let untouched = Arc::clone(shape_of(&scene, model).chunk(ChunkCoord(IVec3::ZERO)).unwrap());
        history.push(attach(&mut scene, border + IVec3::X, 2).unwrap());
        assert!(shape_of(&scene, model).get(border + IVec3::X).is_some());

        history.undo(&mut scene);
        assert!(shape_of(&scene, model).get(border + IVec3::X).is_none());
        assert!(Arc::ptr_eq(
            &untouched,
            shape_of(&scene, model).chunk(ChunkCoord(IVec3::ZERO)).unwrap()
        ));

        history.redo(&mut scene);
        assert!(shape_of(&scene, model).get(border + IVec3::X).is_some());
    }

    #[test]
    fn attaching_where_voxels_already_are_records_nothing() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 1);
        assert!(attach(&mut scene, IVec3::ZERO, 1).is_none());
    }

    #[test]
    fn set_voxels_is_one_undoable_edit_across_chunks() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        let mut history = History::new(10);
        let far = IVec3::new(100, -50, 7);
        let command = Command::SetVoxels {
            model,
            voxels: vec![
                (IVec3::ZERO, Some(color_id(3))),
                (far, Some(color_id(4))),
                (IVec3::new(1, 0, 0), None),
            ],
        };
        history.push(apply(&mut scene, &command).unwrap().unwrap());
        let shape = |scene: &Scene| scene.model_shape(model).unwrap().1.clone();
        assert_eq!(shape(&scene).get(far), Some(color_id(4)));
        assert_eq!(history.done().count(), 1);
        history.undo(&mut scene);
        assert!(shape(&scene).get(far).is_none());
        assert!(shape(&scene).get(IVec3::ZERO).is_none());
        assert_eq!(shape(&scene).volume(), None);
        history.redo(&mut scene);
        assert_eq!(shape(&scene).get(IVec3::ZERO), Some(color_id(3)));
    }

    #[test]
    fn set_voxels_with_one_bad_entry_changes_nothing() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        let command = Command::SetVoxels {
            model,
            voxels: vec![(IVec3::ZERO, Some(color_id(1))), (IVec3::ONE, Some(color_id(200)))],
        };
        assert!(matches!(apply(&mut scene, &command), Err(Error::UnknownColor(200))));
        assert!(scene.model_shape(model).unwrap().1.get(IVec3::ZERO).is_none());
        let far = Command::SetVoxels {
            model,
            voxels: vec![(IVec3::new(COORD_LIMIT + 1, 0, 0), Some(color_id(1)))],
        };
        assert!(matches!(apply(&mut scene, &far), Err(Error::OutOfRange { .. })));
    }

    #[test]
    fn set_voxels_needs_a_visible_unlocked_model() {
        let mut scene = Scene::new();
        let command = Command::SetVoxels {
            model: NodeId::ROOT,
            voxels: vec![(IVec3::ZERO, Some(color_id(1)))],
        };
        assert!(matches!(apply(&mut scene, &command), Err(Error::NotAModel(_))));
    }

    #[test]
    fn setting_what_is_already_there_records_nothing() {
        let mut scene = Scene::new();
        let command = Command::SetVoxels {
            model: first_model(&scene),
            voxels: vec![(IVec3::ZERO, None)],
        };
        assert!(apply(&mut scene, &command).unwrap().is_none());
    }

    #[test]
    fn palette_color_change_undoes_and_redoes() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let id = color_id(2);
        let original = scene.palette().get(id).unwrap().color;
        let command = Command::SetColor { id, color: [1, 2, 3] };
        let edit = apply(&mut scene, &command).unwrap().unwrap();
        assert!(edit.touches_materials());
        history.push(edit);
        assert_eq!(scene.palette().get(id).unwrap().color, [1, 2, 3]);
        history.undo(&mut scene);
        assert_eq!(scene.palette().get(id).unwrap().color, original);
        history.redo(&mut scene);
        assert_eq!(scene.palette().get(id).unwrap().color, [1, 2, 3]);
        assert!(apply(&mut scene, &command).unwrap().is_none());
    }

    #[test]
    fn merged_edits_undo_voxels_and_palette_together() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        let mut history = History::new(10);
        let voxels = Command::SetVoxels {
            model,
            voxels: vec![(IVec3::ZERO, Some(color_id(1)))],
        };
        let color = Command::SetColor {
            id: color_id(1),
            color: [9, 9, 9],
        };
        let first = apply(&mut scene, &voxels).unwrap().unwrap();
        let second = apply(&mut scene, &color).unwrap().unwrap();
        history.push(first.merge(second));
        history.undo(&mut scene);
        assert!(scene.model_shape(model).unwrap().1.get(IVec3::ZERO).is_none());
        assert_ne!(scene.palette().get(color_id(1)).unwrap().color, [9, 9, 9]);
    }

    #[test]
    fn unknown_color_is_rejected() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        let mut command = attach_command(model, IVec3::ZERO, 1);
        if let Command::ApplyBrush { color, .. } = &mut command {
            *color = self::color_id(200);
        }
        assert!(matches!(apply(&mut scene, &command), Err(Error::UnknownColor(200))));
    }

    #[test]
    fn hidden_or_locked_models_reject_voxel_tools() {
        let mut scene = Scene::new();
        let model = first_model(&scene);
        apply(
            &mut scene,
            &Command::SetLocked {
                node: model,
                locked: true,
            },
        )
        .unwrap();
        let result = apply(&mut scene, &attach_command(model, IVec3::ZERO, 1));
        assert!(matches!(result, Err(Error::ModelUnavailable(_))));
    }

    #[test]
    fn mirrored_attach_lands_in_the_same_edit_and_undoes_together() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let model = first_model(&scene);
        history.push(attach(&mut scene, IVec3::new(9, 0, 0), 1).unwrap());
        history.push(attach(&mut scene, IVec3::ZERO, 1).unwrap());
        let volume = shape_of(&scene, model).volume();
        let mut command = attach_command(model, IVec3::new(2, 0, 0), 1);
        if let Command::ApplyBrush { mirror, .. } = &mut command {
            *mirror = Mirror::around([true, false, false], volume);
        }
        let edit = apply(&mut scene, &command).unwrap().unwrap();
        history.push(edit);
        let shape = shape_of(&scene, model);
        assert!(shape.get(IVec3::new(2, 0, 0)).is_some());
        assert!(shape.get(IVec3::new(7, 0, 0)).is_some());
        history.undo(&mut scene);
        let shape = shape_of(&scene, model);
        assert!(shape.get(IVec3::new(2, 0, 0)).is_none());
        assert!(shape.get(IVec3::new(7, 0, 0)).is_none());
    }

    #[test]
    fn surface_fill_repaints_the_connected_face_and_nothing_else() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 3);
        let model = first_model(&scene);
        let command = Command::Fill {
            model,
            mode: FillMode::Surface,
            hit: RayHit {
                voxel: IVec3::new(1, 2, 1),
                face: Face::PosY,
                target: HitTarget::Voxel,
            },
            color: color_id(40),
            mirror: Mirror::default(),
        };
        apply(&mut scene, &command).unwrap().unwrap();
        let shape = shape_of(&scene, model);
        assert_eq!(shape.get(IVec3::new(0, 2, 1)), Some(color_id(40)));
        assert_eq!(shape.get(IVec3::new(0, 1, 1)), Some(color_id(1)));
    }

    #[test]
    fn volume_fill_never_leaves_the_volume() {
        let mut scene = Scene::new();
        attach(&mut scene, IVec3::ZERO, 1);
        attach(&mut scene, IVec3::new(3, 0, 3), 3);
        let model = first_model(&scene);
        let before = shape_of(&scene, model).volume().unwrap();
        let command = Command::Fill {
            model,
            mode: FillMode::Volume,
            hit: RayHit {
                voxel: IVec3::ZERO,
                face: Face::PosY,
                target: HitTarget::Voxel,
            },
            color: color_id(7),
            mirror: Mirror::default(),
        };
        apply(&mut scene, &command).unwrap().unwrap();
        let shape = shape_of(&scene, model);
        assert_eq!(shape.volume(), Some(before));
        assert_eq!(
            shape.used_count(),
            u64::try_from(before.size().element_product()).unwrap()
        );
    }

    proptest::proptest! {
        #[test]
        fn undoing_every_edit_empties_the_shape_and_redoing_restores_it(
            strokes in proptest::collection::vec((-40..40_i32, 0..6_i32, -40..40_i32, 1..4_u32), 1..10),
        ) {
            let mut scene = Scene::new();
            let mut history = History::new(64);
            let model = first_model(&scene);
            for (x, y, z, size) in strokes {
                if let Some(edit) = apply(&mut scene, &attach_command(model, IVec3::new(x, y, z), size)).unwrap() {
                    history.push(edit);
                }
            }
            let filled = shape_of(&scene, model).used_count();
            while history.can_undo() {
                history.undo(&mut scene);
            }
            proptest::prop_assert_eq!(shape_of(&scene, model).used_count(), 0);
            while history.can_redo() {
                history.redo(&mut scene);
            }
            proptest::prop_assert_eq!(shape_of(&scene, model).used_count(), filled);
        }

        #[test]
        fn mirrored_attach_always_produces_a_symmetric_shape(
            strokes in proptest::collection::vec((-30..30_i32, 0..6_i32, -30..30_i32, 1..4_u32), 1..8),
        ) {
            let mut scene = Scene::new();
            let model = first_model(&scene);
            let volume = crate::Volume { min: IVec3::new(-50, 0, -50), max: IVec3::new(50, 10, 50) };
            for (x, y, z, size) in strokes {
                let mut command = attach_command(model, IVec3::new(x, y, z), size);
                if let Command::ApplyBrush { mirror, .. } = &mut command {
                    *mirror = Mirror::around([true, false, false], Some(volume));
                }
                apply(&mut scene, &command).unwrap();
            }
            let shape = shape_of(&scene, model);
            for (cell, _) in shape.voxels() {
                proptest::prop_assert!(shape.get(IVec3::new(-cell.x, cell.y, cell.z)).is_some());
            }
        }
    }

    #[test]
    fn moving_voxels_grows_the_volume_and_undoes() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        attach(&mut scene, IVec3::ZERO, 2);
        let model = first_model(&scene);
        let selection: Selection = [IVec3::ZERO].into_iter().collect();
        let command = Command::MoveVoxels {
            model,
            selection,
            offset: IVec3::new(10, 0, 0),
        };
        history.push(apply(&mut scene, &command).unwrap().unwrap());
        let shape = shape_of(&scene, model);
        assert!(shape.get(IVec3::ZERO).is_none() && shape.get(IVec3::new(10, 0, 0)).is_some());
        assert_eq!(shape.volume().unwrap().max.x, 10);
        history.undo(&mut scene);
        let shape = shape_of(&scene, model);
        assert!(shape.get(IVec3::ZERO).is_some());
        assert_eq!(shape.volume().unwrap().max.x, 1);
    }
}

#[cfg(test)]
mod material_graph_tests {
    use super::*;
    use crate::edit::History;

    fn color_id(number: u8) -> ColorId {
        ColorId::new(number).unwrap()
    }

    fn glow() -> SurfaceConstants {
        SurfaceConstants {
            emission_strength: 6.0,
            roughness: 2.0,
            ..SurfaceConstants::MATTE
        }
    }

    #[test]
    fn new_scenes_start_with_the_default_material_and_colors_use_it() {
        let scene = Scene::new();
        assert_eq!(scene.material_graphs().len(), 1);
        assert_eq!(scene.material_graphs()[0], MaterialGraph::default_material());
        assert!(scene.palette().iter().all(|(_, color)| color.graph == 0));
    }

    #[test]
    fn create_edit_and_assign_undo_in_order() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let create = Command::CreateGraph {
            name: "Lamp".to_owned(),
            surface: SurfaceConstants::MATTE,
        };
        history.push(apply(&mut scene, &create).unwrap().unwrap());
        history.push(
            apply(
                &mut scene,
                &Command::EditGraph {
                    index: 1,
                    surface: glow(),
                },
            )
            .unwrap()
            .unwrap(),
        );
        let assign = Command::AssignGraph {
            id: color_id(5),
            graph: 1,
        };
        let edit = apply(&mut scene, &assign).unwrap().unwrap();
        assert!(edit.touches_materials());
        history.push(edit);
        assert_eq!(scene.palette().get(color_id(5)).unwrap().graph, 1);
        assert!(
            (scene.material_graphs()[1].surface_constants().roughness - 1.0).abs() < f32::EPSILON,
            "constants are clamped"
        );

        history.undo(&mut scene);
        assert_eq!(scene.palette().get(color_id(5)).unwrap().graph, 0);
        history.undo(&mut scene);
        assert!(scene.material_graphs()[1].surface_constants().emission_strength.abs() < f32::EPSILON);
        history.undo(&mut scene);
        assert_eq!(scene.material_graphs().len(), 1);
        history.redo(&mut scene);
        assert_eq!(scene.material_graphs()[1].name, "Lamp");
    }

    #[test]
    fn a_slider_drag_of_merged_edits_undoes_in_one_step() {
        let mut scene = Scene::new();
        let mut history = History::new(10);
        let create = Command::CreateGraph {
            name: "Slate".to_owned(),
            surface: SurfaceConstants::MATTE,
        };
        apply(&mut scene, &create).unwrap();
        let mut merged: Option<Edit> = None;
        for step in 1..=3_u8 {
            let surface = SurfaceConstants {
                metallic: f32::from(step) / 4.0,
                ..SurfaceConstants::MATTE
            };
            let edit = apply(&mut scene, &Command::EditGraph { index: 1, surface })
                .unwrap()
                .unwrap();
            merged = Some(match merged {
                Some(first) => first.merge(edit),
                None => edit,
            });
        }
        history.push(merged.unwrap());
        history.undo(&mut scene);
        assert!(scene.material_graphs()[1].surface_constants().metallic.abs() < f32::EPSILON);
        assert!(!history.can_undo());
    }

    #[test]
    fn graph_commands_reject_bad_targets() {
        let mut scene = Scene::new();
        let edit_default = Command::EditGraph {
            index: 0,
            surface: glow(),
        };
        assert!(matches!(
            apply(&mut scene, &edit_default),
            Err(Error::DefaultMaterialLocked)
        ));
        let edit_missing = Command::EditGraph {
            index: 9,
            surface: glow(),
        };
        assert!(matches!(apply(&mut scene, &edit_missing), Err(Error::UnknownGraph(9))));
        let assign_missing = Command::AssignGraph {
            id: color_id(1),
            graph: 3,
        };
        assert!(matches!(
            apply(&mut scene, &assign_missing),
            Err(Error::UnknownGraph(3))
        ));
        let assign_unknown_color = Command::AssignGraph {
            id: color_id(200),
            graph: 0,
        };
        assert!(matches!(
            apply(&mut scene, &assign_unknown_color),
            Err(Error::UnknownColor(200))
        ));
    }

    #[test]
    fn assigning_the_same_graph_records_nothing() {
        let mut scene = Scene::new();
        let assign = Command::AssignGraph {
            id: color_id(1),
            graph: 0,
        };
        assert!(apply(&mut scene, &assign).unwrap().is_none());
    }

    #[test]
    fn graph_creation_stops_at_the_limit() {
        let mut scene = Scene::new();
        let create = Command::CreateGraph {
            name: "Filler".to_owned(),
            surface: SurfaceConstants::MATTE,
        };
        for _ in 1..MaterialGraph::MAX_PER_SCENE {
            apply(&mut scene, &create).unwrap();
        }
        assert!(matches!(apply(&mut scene, &create), Err(Error::TooManyGraphs(_))));
    }

    #[test]
    fn material_commands_undo_and_redo_with_labels() {
        use crate::material_graph::{GraphEdit, GraphNodeKind, SampleMaterial};

        let mut scene = Scene::new();
        let mut history = History::new(20);
        let mut run = |scene: &mut Scene, command: Command| {
            let edit = apply(scene, &command).unwrap().unwrap();
            let kind = edit.kind();
            history.push(edit);
            kind
        };
        let slate = SampleMaterial::RoofSlate.build();
        assert_eq!(
            run(&mut scene, Command::AddMaterial { graph: Box::new(slate) }),
            EditKind::CreateGraph
        );
        let add = GraphEdit::AddNode {
            kind: GraphNodeKind::Cells,
            position: [0.0, 0.0],
        };
        assert_eq!(
            run(&mut scene, Command::EditMaterial { index: 1, edit: add }),
            EditKind::AddGraphNode
        );
        assert_eq!(scene.material_graphs()[1].nodes().len(), 7);
        let ids = vec![color_id(2), color_id(3)];
        assert_eq!(
            run(&mut scene, Command::AssignGraphToColors { ids, graph: 1 }),
            EditKind::AssignGraph
        );
        assert_eq!(scene.colors_using(1), vec![color_id(2), color_id(3)]);
        assert_eq!(
            run(
                &mut scene,
                Command::RenameMaterial {
                    index: 1,
                    name: "Slate".to_owned()
                }
            ),
            EditKind::RenameGraph
        );
        assert_eq!(
            run(
                &mut scene,
                Command::SetCastsShadow {
                    index: 1,
                    casts_shadow: false
                }
            ),
            EditKind::CastsShadow
        );
        assert_eq!(
            run(&mut scene, Command::DuplicateMaterial { index: 1 }),
            EditKind::DuplicateGraph
        );
        assert_eq!(scene.material_graphs()[2].name, "Slate copy");

        // Deleting material 1 sends its colors to the default and shifts the copy down.
        assert_eq!(
            run(
                &mut scene,
                Command::AssignGraphToColors {
                    ids: vec![color_id(4)],
                    graph: 2
                }
            ),
            EditKind::AssignGraph
        );
        assert_eq!(
            run(&mut scene, Command::DeleteMaterial { index: 1 }),
            EditKind::DeleteGraph
        );
        assert_eq!(scene.palette().get(color_id(2)).unwrap().graph, 0);
        assert_eq!(scene.palette().get(color_id(4)).unwrap().graph, 1);
        assert_eq!(scene.material_graphs().len(), 2);

        while history.can_undo() {
            history.undo(&mut scene);
        }
        assert_eq!(scene.material_graphs().len(), 1);
        while history.can_redo() {
            history.redo(&mut scene);
        }
        assert_eq!(scene.material_graphs()[1].name, "Slate copy");
    }

    #[test]
    fn material_commands_reject_bad_input_and_leave_the_scene_alone() {
        use crate::material_graph::{GraphEdit, GraphError, GraphNodeKind};

        let mut scene = Scene::new();
        let default_edit = Command::EditMaterial {
            index: 0,
            edit: GraphEdit::AddNode {
                kind: GraphNodeKind::Noise,
                position: [0.0; 2],
            },
        };
        assert!(matches!(
            apply(&mut scene, &default_edit),
            Err(Error::DefaultMaterialLocked)
        ));
        assert!(matches!(
            apply(&mut scene, &Command::DeleteMaterial { index: 0 }),
            Err(Error::DefaultMaterialLocked)
        ));
        assert!(matches!(
            apply(&mut scene, &Command::DuplicateMaterial { index: 4 }),
            Err(Error::UnknownGraph(4))
        ));
        apply(
            &mut scene,
            &Command::AddMaterial {
                graph: Box::new(MaterialGraph::blank("A")),
            },
        )
        .unwrap();
        let second_surface = Command::EditMaterial {
            index: 1,
            edit: GraphEdit::AddNode {
                kind: GraphNodeKind::SurfaceOutput,
                position: [0.0; 2],
            },
        };
        assert!(matches!(
            apply(&mut scene, &second_surface),
            Err(Error::Graph(GraphError::SecondOutput(_)))
        ));
        let unlinked = Command::AddMaterial {
            graph: Box::new(serde_json::from_str(r#"{"name":"x","casts_shadow":true,"nodes":[],"links":[],"frames":[],"next_node":0,"next_frame":0}"#).unwrap()),
        };
        assert!(matches!(
            apply(&mut scene, &unlinked),
            Err(Error::Graph(GraphError::NoSurface))
        ));
        assert_eq!(scene.material_graphs().len(), 2);
    }
}
