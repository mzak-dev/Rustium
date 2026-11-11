use std::collections::HashSet;

use glam::{IVec3, Vec3};
use rustium_core::{
    Brush, BrushKind, ChunkCoord, ColorId, Command, Edit, Face, FillMode, History, HitTarget, Mirror, NodeId,
    Placement, Ray, RayHit, Scene, SceneHit, Selection, ShapeId, SurfaceConstants, Volume, VoxelTool, apply,
    brush_bounds, pick_scene, select_box, select_by_color,
};
use tracing::warn;

use crate::icons::Icon;

/// The Tools on the left toolbar, in mockup order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    SelectBox,
    SelectColor,
    Attach,
    Erase,
    Paint,
    Fill,
    Pick,
    Move,
    Measure,
}

impl Tool {
    pub(crate) const ALL: [Self; 9] = [
        Self::SelectBox,
        Self::SelectColor,
        Self::Attach,
        Self::Erase,
        Self::Paint,
        Self::Fill,
        Self::Pick,
        Self::Move,
        Self::Measure,
    ];

    pub(crate) fn key(self) -> egui::Key {
        match self {
            Self::SelectBox => egui::Key::V,
            Self::SelectColor => egui::Key::W,
            Self::Attach => egui::Key::B,
            Self::Erase => egui::Key::E,
            Self::Paint => egui::Key::P,
            Self::Fill => egui::Key::F,
            Self::Pick => egui::Key::I,
            Self::Move => egui::Key::G,
            Self::Measure => egui::Key::M,
        }
    }

    pub(crate) fn icon(self) -> Icon {
        match self {
            Self::SelectBox => Icon::SelectBox,
            Self::SelectColor => Icon::SelectColor,
            Self::Attach => Icon::Attach,
            Self::Erase => Icon::Erase,
            Self::Paint => Icon::Paint,
            Self::Fill => Icon::Fill,
            Self::Pick => Icon::Pick,
            Self::Move => Icon::Move,
            Self::Measure => Icon::Measure,
        }
    }

    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::SelectBox => "tool-select-box",
            Self::SelectColor => "tool-select-color",
            Self::Attach => "tool-attach",
            Self::Erase => "tool-erase",
            Self::Paint => "tool-paint",
            Self::Fill => "tool-fill",
            Self::Pick => "tool-pick",
            Self::Move => "tool-move",
            Self::Measure => "tool-measure",
        }
    }

    /// Tools drawn with a divider above them (groups: select | edit | transform).
    pub(crate) fn starts_group(self) -> bool {
        matches!(self, Self::Attach | Self::Move)
    }

    pub(crate) fn voxel_tool(self) -> Option<VoxelTool> {
        match self {
            Self::Attach => Some(VoxelTool::Attach),
            Self::Erase => Some(VoxelTool::Erase),
            Self::Paint => Some(VoxelTool::Paint),
            _ => None,
        }
    }

    /// Tools that only make sense on the Voxels of one Model, so they need a Model in Focus.
    pub(crate) fn needs_model(self) -> bool {
        matches!(
            self,
            Self::Attach | Self::Erase | Self::Paint | Self::Fill | Self::SelectColor
        )
    }
}

/// A brush gesture in progress; its Edits fold into one History step.
#[derive(Debug)]
struct Stroke {
    tool: VoxelTool,
    /// Attach strokes stay on the plane of the first face so they do not stack towards the camera.
    face: Face,
    plane: i32,
    last_hit: RayHit,
    mirror: Mirror,
    edit: Option<Edit>,
}

/// A Line or Box gesture: it previews while dragging and is applied on release.
#[derive(Debug)]
struct ShapeDrag {
    tool: VoxelTool,
    anchor: RayHit,
    current: RayHit,
    mirror: Mirror,
}

/// A Select box gesture in a Model.
#[derive(Debug)]
struct SelectDrag {
    anchor: IVec3,
    base: Selection,
}

/// A Move gesture: the pointer slides over the plane of the grabbed face and the grabbed things follow whole voxels.
#[derive(Debug)]
struct MoveDrag {
    axis: usize,
    coordinate: f32,
    last: IVec3,
    edit: Option<Edit>,
}

#[derive(Debug)]
enum Gesture {
    Stroke(Stroke),
    Shape(ShapeDrag),
    Select(SelectDrag),
    MoveVoxels(MoveDrag),
    MoveNodes(MoveDrag),
}

/// How a click or a marquee changes the set of selected Nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionEdit {
    /// Selects only the given Nodes.
    Replace,
    /// Adds the given Nodes.
    Add,
    /// Adds Nodes that are not selected and removes the ones that are.
    Toggle,
}

/// The Measure Tool's two end points, in Scene cells.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Measurement {
    pub(crate) start: Option<IVec3>,
    pub(crate) end: Option<IVec3>,
}

/// Distance between the two Measure points.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Distance {
    pub(crate) delta: IVec3,
    pub(crate) voxels: f32,
    pub(crate) metres: f32,
}

/// What a finished script run did.
#[derive(Debug)]
pub(crate) struct ScriptRun {
    pub(crate) changed: bool,
    pub(crate) log: Vec<String>,
}

/// The document plus everything about how the user is editing it.
#[derive(Debug)]
pub(crate) struct Editor {
    pub(crate) scene: Scene,
    pub(crate) history: History,
    pub(crate) tool: Tool,
    pub(crate) brush: Brush,
    pub(crate) fill_mode: FillMode,
    pub(crate) mirror: [bool; 3],
    pub(crate) slots: [ColorId; 10],
    pub(crate) active_slot: usize,
    /// The Node the user has stepped into.
    pub(crate) focus: NodeId,
    /// Voxels of the Focus Model that Select tools picked.
    pub(crate) selection: Selection,
    /// Nodes selected in the Scene tree or with Select box inside a Group.
    pub(crate) node_selection: Vec<NodeId>,
    /// What the pointer is over, limited to the Focus.
    pub(crate) hover: Option<SceneHit>,
    /// What the pointer is over anywhere in the Scene, for zooming towards the cursor.
    pub(crate) view_hit: Option<SceneHit>,
    pub(crate) measurement: Measurement,
    gesture: Option<Gesture>,
    dirty: HashSet<(ShapeId, ChunkCoord)>,
    meshed: HashSet<ShapeId>,
    remesh_all: bool,
    /// Material graph edits of the current slider drag, folded into one History step when the drag ends.
    material_edit: Option<Edit>,
    palette_dirty: bool,
    /// Increases with every change to the Scene; the path tracer re-uploads when it moves.
    pub(crate) revision: u64,
}

/// Default hotbar: two greys and eight hues from the default Palette (8 greys, then 11 hues × 8 shades).
const DEFAULT_SLOTS: [u8; 10] = [2, 5, 9 + 3, 17 + 3, 33 + 3, 41 + 3, 57 + 3, 73 + 3, 81 + 3, 89 + 3];

/// Furthest the pointer ray reaches, in voxels.
const PICK_DISTANCE: f32 = 4000.0;

/// Selections up to this size are outlined voxel by voxel; larger ones only get their bounding box.
const OUTLINE_LIMIT: usize = 1500;

impl Editor {
    pub(crate) fn new(voxel_size: f32, undo_steps: usize) -> Self {
        Self::with_scene(Scene::with_voxel_size(voxel_size), undo_steps)
    }

    /// An Editor on `scene` with a fresh History and default tool state.
    fn with_scene(scene: Scene, undo_steps: usize) -> Self {
        let slots = DEFAULT_SLOTS.map(|number| {
            ColorId::new(number)
                .filter(|id| scene.palette().get(*id).is_some())
                .unwrap_or(ColorId::FIRST)
        });
        let focus = scene
            .node(NodeId::ROOT)
            .and_then(|root| root.children().first().copied())
            .unwrap_or(NodeId::ROOT);
        Self {
            scene,
            history: History::new(undo_steps),
            tool: Tool::Attach,
            brush: Brush::default(),
            fill_mode: FillMode::default(),
            mirror: [false; 3],
            slots,
            active_slot: 2,
            focus,
            selection: Selection::new(),
            node_selection: Vec::new(),
            hover: None,
            view_hit: None,
            measurement: Measurement::default(),
            gesture: None,
            dirty: HashSet::new(),
            meshed: HashSet::new(),
            remesh_all: true,
            material_edit: None,
            palette_dirty: true,
            revision: 0,
        }
    }

    /// Starts a new Scene with one empty Model and clears History.
    pub(crate) fn new_scene(&mut self, voxel_size: f32, undo_steps: usize) {
        self.replace_scene(Scene::with_voxel_size(voxel_size), undo_steps);
    }

    /// Swaps in another Scene (a new or a loaded one), clears History and keeps the tool settings. The revision keeps
    /// counting up so that a stored revision never matches the new Scene by accident.
    pub(crate) fn replace_scene(&mut self, scene: Scene, undo_steps: usize) {
        let slots = self.slots;
        let (tool, brush, fill_mode, mirror) = (self.tool, self.brush, self.fill_mode, self.mirror);
        let revision = self.revision + 1;
        *self = Self::with_scene(scene, undo_steps);
        self.revision = revision;
        self.slots = slots.map(|id| {
            if self.scene.palette().get(id).is_some() {
                id
            } else {
                ColorId::FIRST
            }
        });
        (self.tool, self.brush, self.fill_mode, self.mirror) = (tool, brush, fill_mode, mirror);
    }

    /// Sets the Palette entries to `colors` in one History step; entries beyond the Palette's size are ignored.
    /// Returns how many Colors were applied.
    pub(crate) fn import_palette(&mut self, colors: &[[u8; 3]]) -> usize {
        self.release();
        let mut merged: Option<Edit> = None;
        let mut applied = 0;
        for (index, color) in colors.iter().enumerate() {
            let Some(id) = u8::try_from(index + 1).ok().and_then(ColorId::new) else {
                break;
            };
            if self.scene.palette().get(id).is_none() {
                break;
            }
            applied += 1;
            if let Some(edit) = self.apply_command(&Command::SetColor { id, color: *color }) {
                merged = Some(match merged.take() {
                    Some(pending) => pending.merge(edit),
                    None => edit,
                });
            }
        }
        if let Some(edit) = merged {
            self.history.push(edit);
        }
        applied
    }

    pub(crate) fn color(&self) -> ColorId {
        self.slots[self.active_slot.min(self.slots.len() - 1)]
    }

    /// Whether the Focus is a Model, where voxel Tools work.
    pub(crate) fn focus_is_model(&self) -> bool {
        self.scene.node(self.focus).is_some_and(|node| !node.is_group())
    }

    /// Whether the current Tool can do anything in the current Focus.
    pub(crate) fn tool_ready(&self) -> bool {
        self.focus_is_model() || !self.tool.needs_model()
    }

    /// Transform from the Focus's cells to the Scene.
    fn focus_transform(&self) -> rustium_core::Transform {
        self.scene.world_transform(self.focus)
    }

    /// The hit in the cell space of the Focus.
    fn local_hit(&self, hit: &SceneHit) -> RayHit {
        self.focus_transform().inverse().hit(hit.world)
    }

    fn local_ray(&self, ray: Ray) -> Ray {
        let inverse = self.focus_transform().inverse();
        Ray {
            origin: inverse.point(ray.origin),
            direction: inverse.orientation.apply_float(ray.direction),
        }
    }

    // ---- Focus -------------------------------------------------------------------------------------------

    /// Steps into `node` (a Group or an unlocked, visible Model); returns whether the Focus changed.
    pub(crate) fn step_in(&mut self, node: NodeId) -> bool {
        let Some(entry) = self.scene.node(node) else {
            return false;
        };
        let blocked = !entry.is_group() && (self.scene.is_locked(node) || self.scene.is_hidden(node));
        if blocked || !self.scene.is_attached(node) || node == self.focus {
            return false;
        }
        self.release();
        self.focus = node;
        self.selection = Selection::new();
        self.node_selection.clear();
        true
    }

    /// Steps out to the parent of the Focus.
    pub(crate) fn step_out(&mut self) -> bool {
        let Some(parent) = self.scene.node(self.focus).and_then(rustium_core::Node::parent) else {
            return false;
        };
        self.release();
        let previous = self.focus;
        self.focus = parent;
        self.selection = Selection::new();
        self.node_selection = vec![previous];
        true
    }

    /// Puts the Focus back in the tree after an undo removed its Node, and drops stale selections.
    pub(crate) fn validate(&mut self) {
        let scene = &self.scene;
        if !scene.is_attached(self.focus) {
            self.focus = NodeId::ROOT;
            self.selection = Selection::new();
        }
        self.node_selection.retain(|node| scene.is_attached(*node));
        let focus_unusable = scene
            .node(self.focus)
            .is_some_and(|node| !node.is_group() && (scene.is_locked(self.focus) || scene.is_hidden(self.focus)));
        if focus_unusable {
            self.focus = scene
                .node(self.focus)
                .and_then(rustium_core::Node::parent)
                .unwrap_or(NodeId::ROOT);
            self.selection = Selection::new();
        }
    }

    // ---- Overlays ----------------------------------------------------------------------------------------

    /// The Scene cell the pointer is over (for a ground hit, the cell above the ground).
    pub(crate) fn hover_cell(&self) -> Option<IVec3> {
        self.hover.as_ref().map(|hit| world_cell(hit.world))
    }

    /// The box the current Tool would act on at the pointer, in Scene cells.
    pub(crate) fn hover_bounds(&self) -> Option<Volume> {
        if !self.tool_ready() {
            return None;
        }
        if let Some(Gesture::Select(drag)) = &self.gesture {
            let current = self.hover.as_ref().map(|hit| select_cell(self.local_hit(hit)))?;
            let volume = Volume::point(drag.anchor).including(current);
            return Some(self.focus_transform().volume(volume));
        }
        let hit = self.hover.as_ref()?;
        let focus_is_model = self.focus_is_model();
        if focus_is_model && let Some(tool) = self.tool.voxel_tool() {
            let local = self.local_hit(hit);
            let (anchor, current) = match &self.gesture {
                Some(Gesture::Shape(drag)) => (drag.anchor, drag.current),
                _ => (local, local),
            };
            let (_, shape) = self.scene.model_shape(self.focus)?;
            let bounds = brush_bounds(shape, tool, self.brush, anchor, current)?;
            return Some(self.focus_transform().volume(bounds));
        }
        if !focus_is_model && matches!(self.tool, Tool::SelectBox | Tool::Move) {
            let child = self.scene.child_toward(self.focus, hit.node?)?;
            return self.scene.world_volume(child);
        }
        hit.is_voxel().then(|| Volume::point(hit.world.voxel))
    }

    /// Bounds of the Shape-drag preview, if a Line or Box gesture is running; used to avoid highlighting twice.
    pub(crate) fn mirror_planes(&self) -> Vec<[Vec3; 4]> {
        let Some((_, shape)) = self.scene.model_shape(self.focus) else {
            return Vec::new();
        };
        let mirror = Mirror::around(self.mirror, shape.volume());
        let volume = shape.volume().unwrap_or_else(|| Volume::point(IVec3::ZERO));
        let (low, high) = (volume.min.as_vec3() - 2.0, (volume.max + IVec3::ONE).as_vec3() + 2.0);
        let transform = self.focus_transform();
        let mut planes = Vec::new();
        for axis in (0..3).filter(|axis| self.mirror[*axis]) {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |a: f32, b: f32| {
                let mut point = Vec3::ZERO;
                point[axis] = mirror.plane(axis);
                point[u] = a;
                point[v] = b;
                transform.point(point)
            };
            planes.push([
                corner(low[u], low[v]),
                corner(high[u], low[v]),
                corner(high[u], high[v]),
                corner(low[u], high[v]),
            ]);
        }
        planes
    }

    /// Boxes to outline for the selected voxels and nodes, in Scene cells.
    pub(crate) fn selection_boxes(&self) -> Vec<Volume> {
        let transform = self.focus_transform();
        let mut boxes: Vec<Volume> = Vec::new();
        if self.selection.len() <= OUTLINE_LIMIT {
            boxes.extend(self.selection.iter().map(|cell| transform.volume(Volume::point(cell))));
        } else if let Some(bounds) = self.selection.bounds() {
            boxes.push(transform.volume(bounds));
        }
        boxes.extend(
            self.node_selection
                .iter()
                .filter_map(|node| self.scene.world_volume(*node)),
        );
        boxes
    }

    /// The Measure Tool's end points and their distance; the second point follows the pointer until it is set.
    pub(crate) fn distance(&self) -> Option<(IVec3, IVec3, Distance)> {
        let start = self.measurement.start?;
        let end = self.measurement.end.or_else(|| self.hover_cell())?;
        let delta = end - start;
        let voxels = delta.as_vec3().length();
        Some((
            start,
            end,
            Distance {
                delta,
                voxels,
                metres: voxels * self.scene.voxel_size(),
            },
        ))
    }

    // ---- Pointer -----------------------------------------------------------------------------------------

    /// Finds what the pointer ray is over.
    pub(crate) fn update_hover(&mut self, ray: Ray) {
        let within = if self.tool == Tool::Measure {
            NodeId::ROOT
        } else {
            self.focus
        };
        self.hover = pick_scene(&self.scene, ray, PICK_DISTANCE, within);
        self.view_hit = if within == NodeId::ROOT {
            self.hover
        } else {
            pick_scene(&self.scene, ray, PICK_DISTANCE, NodeId::ROOT)
        };
    }

    pub(crate) fn clear_hover(&mut self) {
        self.hover = None;
        self.view_hit = None;
    }

    pub(crate) fn is_stroking(&self) -> bool {
        self.gesture.is_some()
    }

    /// Primary button pressed over the viewport; `ray` is the pointer ray and `extend` is Shift.
    pub(crate) fn press(&mut self, ray: Ray, extend: bool) {
        let Some(hit) = self.hover else {
            if matches!(self.tool, Tool::SelectBox | Tool::SelectColor) && self.focus_is_model() && !extend {
                self.selection = Selection::new();
            }
            return;
        };
        match self.tool {
            Tool::Pick => self.pick_color(&hit),
            Tool::Measure => self.measure(&hit),
            Tool::Attach | Tool::Erase | Tool::Paint if self.focus_is_model() => self.begin_brush(&hit),
            Tool::Fill if self.focus_is_model() => self.fill(&hit),
            Tool::SelectColor if self.focus_is_model() => self.select_color(&hit, extend),
            Tool::SelectBox if self.focus_is_model() => {
                let anchor = select_cell(self.local_hit(&hit));
                let base = if extend {
                    self.selection.clone()
                } else {
                    Selection::new()
                };
                self.gesture = Some(Gesture::Select(SelectDrag { anchor, base }));
                self.drag(ray);
            }
            Tool::SelectBox if !self.focus_is_model() => self.click_node(&hit, extend),
            Tool::Move => self.begin_move(&hit),
            _ => {}
        }
    }

    /// Pointer moved while the primary button is held.
    pub(crate) fn drag(&mut self, ray: Ray) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        let hit = self.hover.as_ref().map(|hit| self.local_hit(hit));
        self.gesture = Some(match gesture {
            Gesture::Stroke(stroke) => Gesture::Stroke(self.drag_stroke(stroke, hit)),
            Gesture::Shape(drag) => Gesture::Shape(self.drag_shape(drag, hit, ray)),
            Gesture::Select(drag) => {
                if let (Some(hit), Some((_, shape))) = (hit, self.scene.model_shape(self.focus)) {
                    let boxed = select_box(shape, drag.anchor, select_cell(hit));
                    self.selection = if drag.base.is_empty() {
                        boxed
                    } else {
                        drag.base.union(&boxed)
                    };
                }
                Gesture::Select(drag)
            }
            Gesture::MoveVoxels(drag) => Gesture::MoveVoxels(self.drag_move(drag, ray, true)),
            Gesture::MoveNodes(drag) => Gesture::MoveNodes(self.drag_move(drag, ray, false)),
        });
    }

    /// Primary button released: the gesture becomes one History step.
    pub(crate) fn release(&mut self) {
        self.finish_material_edit();
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        let edit = match gesture {
            Gesture::Stroke(stroke) => stroke.edit,
            Gesture::Shape(drag) => self.finish_shape(&drag),
            Gesture::MoveVoxels(drag) | Gesture::MoveNodes(drag) => drag.edit,
            Gesture::Select(_) => None,
        };
        if let Some(edit) = edit {
            self.history.push(edit);
        }
    }

    // ---- Tools -------------------------------------------------------------------------------------------

    fn pick_color(&mut self, hit: &SceneHit) {
        if let (HitTarget::Voxel, Some(node)) = (hit.world.target, hit.node)
            && let Some((_, shape)) = self.scene.model_shape(node)
            && let Some(color) = shape.get(hit.local.voxel)
        {
            self.slots[self.active_slot] = color;
        }
    }

    fn measure(&mut self, hit: &SceneHit) {
        let cell = world_cell(hit.world);
        self.measurement = match (self.measurement.start, self.measurement.end) {
            (Some(start), None) => Measurement {
                start: Some(start),
                end: Some(cell),
            },
            _ => Measurement {
                start: Some(cell),
                end: None,
            },
        };
    }

    fn begin_brush(&mut self, hit: &SceneHit) {
        let Some(tool) = self.tool.voxel_tool() else {
            return;
        };
        let local = self.local_hit(hit);
        let mirror = self.mirror_now();
        if matches!(self.brush.kind, BrushKind::Line | BrushKind::Box) {
            self.gesture = Some(Gesture::Shape(ShapeDrag {
                tool,
                anchor: local,
                current: local,
                mirror,
            }));
            return;
        }
        let edit = self.run_brush(tool, local, None, mirror);
        self.gesture = Some(Gesture::Stroke(Stroke {
            tool,
            face: local.face,
            plane: local.voxel[local.face.axis()],
            last_hit: local,
            mirror,
            edit,
        }));
    }

    fn drag_stroke(&mut self, mut stroke: Stroke, hit: Option<RayHit>) -> Stroke {
        let Some(hit) = hit else {
            return stroke;
        };
        if hit == stroke.last_hit {
            return stroke;
        }
        if stroke.tool == VoxelTool::Attach && (hit.face != stroke.face || hit.voxel[hit.face.axis()] != stroke.plane) {
            return stroke;
        }
        let edit = self.run_brush(stroke.tool, hit, None, stroke.mirror);
        stroke.last_hit = hit;
        stroke.edit = merge(stroke.edit.take(), edit);
        stroke
    }

    fn drag_shape(&mut self, mut drag: ShapeDrag, hit: Option<RayHit>, ray: Ray) -> ShapeDrag {
        match self.brush.kind {
            BrushKind::Box => {
                // The rectangle lives on the plane of the pressed face, so follow the pointer over that plane.
                let (axis, coordinate) = plane_of(drag.anchor);
                if let Some(cell) = plane_cell(self.local_ray(ray), axis, coordinate) {
                    let mut voxel = cell;
                    voxel[axis] = drag.anchor.voxel[axis];
                    drag.current = RayHit {
                        voxel,
                        face: drag.anchor.face,
                        target: HitTarget::Voxel,
                    };
                }
            }
            _ => {
                if let Some(hit) = hit {
                    drag.current = hit;
                }
            }
        }
        drag
    }

    fn finish_shape(&mut self, drag: &ShapeDrag) -> Option<Edit> {
        self.run_brush(drag.tool, drag.anchor, Some(drag.current), drag.mirror)
    }

    fn fill(&mut self, hit: &SceneHit) {
        let command = Command::Fill {
            model: self.focus,
            mode: self.fill_mode,
            hit: self.local_hit(hit),
            color: self.color(),
            mirror: self.mirror_now(),
        };
        if let Some(edit) = self.apply_command(&command) {
            self.history.push(edit);
        }
    }

    fn select_color(&mut self, hit: &SceneHit, extend: bool) {
        let Some((_, shape)) = self.scene.model_shape(self.focus) else {
            return;
        };
        let local = self.local_hit(hit);
        let Some(color) = (local.target == HitTarget::Voxel)
            .then(|| shape.get(local.voxel))
            .flatten()
        else {
            return;
        };
        let picked = select_by_color(shape, color);
        self.selection = if extend { self.selection.union(&picked) } else { picked };
    }

    /// Changes which Nodes are selected.
    pub(crate) fn select_nodes(&mut self, nodes: &[NodeId], edit: SelectionEdit) {
        match edit {
            SelectionEdit::Replace => self.node_selection = nodes.to_vec(),
            SelectionEdit::Add => {
                for node in nodes {
                    if !self.node_selection.contains(node) {
                        self.node_selection.push(*node);
                    }
                }
            }
            SelectionEdit::Toggle => {
                for node in nodes {
                    match self.node_selection.iter().position(|selected| selected == node) {
                        Some(position) => {
                            self.node_selection.remove(position);
                        }
                        None => self.node_selection.push(*node),
                    }
                }
            }
        }
    }

    /// Select box in a Group: a click selects the child Node under the pointer.
    pub(crate) fn click_node(&mut self, hit: &SceneHit, extend: bool) {
        let child = hit.node.and_then(|node| self.scene.child_toward(self.focus, node));
        match (child, extend) {
            (Some(child), true) => self.select_nodes(&[child], SelectionEdit::Toggle),
            (Some(child), false) => self.select_nodes(&[child], SelectionEdit::Replace),
            (None, false) => self.node_selection.clear(),
            (None, true) => {}
        }
    }

    fn begin_move(&mut self, hit: &SceneHit) {
        let local = self.local_hit(hit);
        let (axis, coordinate) = plane_of(local);
        if self.focus_is_model() {
            if local.target != HitTarget::Voxel || !self.selection.contains(local.voxel) {
                return;
            }
            self.gesture = Some(Gesture::MoveVoxels(MoveDrag {
                axis,
                coordinate,
                last: plane_flat(local.voxel, axis),
                edit: None,
            }));
            return;
        }
        let Some(child) = hit.node.and_then(|node| self.scene.child_toward(self.focus, node)) else {
            return;
        };
        if !self.node_selection.contains(&child) {
            self.node_selection = vec![child];
        }
        self.gesture = Some(Gesture::MoveNodes(MoveDrag {
            axis,
            coordinate,
            last: plane_flat(local.voxel, axis),
            edit: None,
        }));
    }

    fn drag_move(&mut self, mut drag: MoveDrag, ray: Ray, voxels: bool) -> MoveDrag {
        let Some(cell) = plane_cell(self.local_ray(ray), drag.axis, drag.coordinate) else {
            return drag;
        };
        let delta = cell - drag.last;
        if delta == IVec3::ZERO {
            return drag;
        }
        drag.last = cell;
        let edit = if voxels {
            let command = Command::MoveVoxels {
                model: self.focus,
                selection: self.selection.clone(),
                offset: delta,
            };
            let edit = self.apply_command(&command);
            if edit.is_some() {
                self.selection = self.selection.translated(delta);
            }
            edit
        } else {
            self.move_nodes(delta)
        };
        drag.edit = merge(drag.edit.take(), edit);
        drag
    }

    fn move_nodes(&mut self, delta: IVec3) -> Option<Edit> {
        let mut merged = None;
        for node in self.node_selection.clone() {
            let Some(mut transform) = self.scene.node(node).map(rustium_core::Node::transform) else {
                continue;
            };
            transform.offset += delta;
            let command = Command::SetTransform { node, transform };
            merged = merge(merged, self.apply_command(&command));
        }
        merged
    }

    // ---- Commands ----------------------------------------------------------------------------------------

    fn mirror_now(&self) -> Mirror {
        let volume = self.scene.model_shape(self.focus).and_then(|(_, shape)| shape.volume());
        Mirror::around(self.mirror, volume)
    }

    fn run_brush(&mut self, tool: VoxelTool, hit: RayHit, end: Option<RayHit>, mirror: Mirror) -> Option<Edit> {
        let command = Command::ApplyBrush {
            model: self.focus,
            tool,
            brush: self.brush,
            hit,
            end,
            color: self.color(),
            mirror,
        };
        self.apply_command(&command)
    }

    /// Applies a Command and marks the touched chunks for remeshing; the caller decides about History.
    fn apply_command(&mut self, command: &Command) -> Option<Edit> {
        match self.apply_checked(command) {
            Ok(edit) => edit,
            Err(error) => {
                warn!(%error, "command rejected");
                None
            }
        }
    }

    /// Like [`Editor::apply_command`], but reports why a Command was rejected.
    fn apply_checked(&mut self, command: &Command) -> Result<Option<Edit>, rustium_core::Error> {
        let edit = apply(&mut self.scene, command)?;
        if let Some(edit) = &edit {
            self.note_applied(edit);
        }
        Ok(edit)
    }

    /// Bookkeeping for an Edit that is already applied to the Scene: remesh, re-upload, bump the revision.
    fn note_applied(&mut self, edit: &Edit) {
        let changed: Vec<(ShapeId, ChunkCoord)> = edit.changed_chunks().collect();
        self.mark_dirty(changed);
        self.revision += 1;
        self.palette_dirty |= edit.touches_materials();
    }

    /// Applies a Command from outside the viewport (MCP) as one History step. Unlike [`Editor::execute`] it
    /// reports rejections; returns whether anything changed.
    pub(crate) fn execute_checked(&mut self, command: &Command) -> Result<bool, rustium_core::Error> {
        self.release();
        let Some(edit) = self.apply_checked(command)? else {
            return Ok(false);
        };
        self.history.push(edit);
        self.validate();
        Ok(true)
    }

    /// Runs a Luau script on the Model `model` as one History step. A failing script changes nothing.
    pub(crate) fn run_script(
        &mut self,
        model: NodeId,
        source: &str,
        limits: &rustium_script::Limits,
    ) -> Result<ScriptRun, rustium_script::Error> {
        self.release();
        let output = rustium_script::run(&mut self.scene, model, source, limits)?;
        let changed = output.edit.is_some();
        if let Some(edit) = output.edit {
            self.note_applied(&edit);
            self.history.push(edit);
            self.validate();
        }
        Ok(ScriptRun {
            changed,
            log: output.log,
        })
    }

    /// Applies a Command as its own History step; returns the Nodes it created.
    pub(crate) fn execute(&mut self, command: &Command) -> Vec<NodeId> {
        self.release();
        let Some(edit) = self.apply_command(command) else {
            return Vec::new();
        };
        let created = edit.created_nodes().to_vec();
        self.history.push(edit);
        self.validate();
        created
    }

    pub(crate) fn undo(&mut self) {
        self.release();
        let (changed, colors) = self
            .history
            .undo(&mut self.scene)
            .map(|edit| (edit.changed_chunks().collect::<Vec<_>>(), edit.touches_materials()))
            .unwrap_or_default();
        self.revision += 1;
        self.palette_dirty |= colors;
        self.mark_dirty(changed);
        self.selection = Selection::new();
        self.validate();
    }

    pub(crate) fn redo(&mut self) {
        self.release();
        let (changed, colors) = self
            .history
            .redo(&mut self.scene)
            .map(|edit| (edit.changed_chunks().collect::<Vec<_>>(), edit.touches_materials()))
            .unwrap_or_default();
        self.revision += 1;
        self.palette_dirty |= colors;
        self.mark_dirty(changed);
        self.selection = Selection::new();
        self.validate();
    }

    /// Ends a Material slider drag: its Edits become one History step.
    pub(crate) fn finish_material_edit(&mut self) {
        if let Some(edit) = self.material_edit.take() {
            self.history.push(edit);
        }
    }

    /// Changes the Surface constants of material graph `index`; consecutive calls (a slider drag) merge into one
    /// History step until [`Editor::finish_material_edit`].
    pub(crate) fn edit_graph(&mut self, index: u32, surface: SurfaceConstants) {
        self.apply_material_command(&Command::EditGraph { index, surface });
    }

    /// Creates a matte material graph and returns its index.
    pub(crate) fn create_graph(&mut self) -> u32 {
        self.finish_material_edit();
        let index = u32::try_from(self.scene.material_graphs().len()).unwrap_or(u32::MAX);
        let command = Command::CreateGraph {
            name: format!("Material {index}"),
            surface: SurfaceConstants::MATTE,
        };
        self.apply_material_command(&command);
        self.finish_material_edit();
        index
    }

    /// Points the active Color slot's Palette entry at material graph `graph`.
    pub(crate) fn assign_graph(&mut self, graph: u32) {
        self.finish_material_edit();
        self.apply_material_command(&Command::AssignGraph {
            id: self.color(),
            graph,
        });
        self.finish_material_edit();
    }

    fn apply_material_command(&mut self, command: &Command) {
        if let Some(edit) = self.apply_command(command) {
            self.material_edit = Some(match self.material_edit.take() {
                Some(pending) => pending.merge(edit),
                None => edit,
            });
        }
    }

    /// Whether Colors or material graphs changed since the last call, so the GPU copy needs uploading.
    pub(crate) fn take_palette_dirty(&mut self) -> bool {
        std::mem::take(&mut self.palette_dirty)
    }

    /// Marks chunks and their 26 neighbours (borders affect culling and AO) for remeshing.
    // ponytail: remeshes every existing neighbour; narrow it to the touched borders if strokes stutter.
    fn mark_dirty(&mut self, chunks: Vec<(ShapeId, ChunkCoord)>) {
        for (shape, coord) in chunks {
            for z in -1..=1 {
                for y in -1..=1 {
                    for x in -1..=1 {
                        let neighbour = ChunkCoord(coord.0 + IVec3::new(x, y, z));
                        let exists = self
                            .scene
                            .shape(shape)
                            .is_some_and(|shape| shape.chunk(neighbour).is_some());
                        if neighbour == coord || exists {
                            self.dirty.insert((shape, neighbour));
                        }
                    }
                }
            }
        }
    }

    /// Chunks to remesh since the last call, and whether every GPU chunk must be dropped first.
    ///
    /// Shapes shown for the first time (a duplicate, or a Model shown again) are meshed completely.
    pub(crate) fn take_dirty(&mut self, placements: &[Placement]) -> (bool, Vec<(ShapeId, ChunkCoord)>) {
        let reset = std::mem::take(&mut self.remesh_all);
        if reset {
            self.dirty.clear();
            self.meshed.clear();
        }
        for placement in placements {
            if self.meshed.insert(placement.shape)
                && let Some(shape) = self.scene.shape(placement.shape)
            {
                self.dirty
                    .extend(shape.chunks().map(|(coord, _)| (placement.shape, coord)));
            }
        }
        (reset, self.dirty.drain().collect())
    }
}

fn merge(first: Option<Edit>, next: Option<Edit>) -> Option<Edit> {
    match (first, next) {
        (Some(first), Some(next)) => Some(first.merge(next)),
        (first, next) => first.or(next),
    }
}

/// The Scene cell a hit points at: the voxel itself, or for the ground the cell above it.
fn world_cell(hit: RayHit) -> IVec3 {
    match hit.target {
        HitTarget::Voxel => hit.voxel,
        HitTarget::Ground => hit.voxel + IVec3::Y,
    }
}

/// The cell a Select box corner snaps to.
fn select_cell(hit: RayHit) -> IVec3 {
    world_cell(hit)
}

/// Axis and coordinate of the plane of the hit face, in cell space.
fn plane_of(hit: RayHit) -> (usize, f32) {
    let axis = hit.face.axis();
    (axis, (hit.voxel[axis] + i32::from(hit.face.is_positive())) as f32)
}

/// `cell` with the axis component cleared, so differences stay on the drag plane.
fn plane_flat(mut cell: IVec3, axis: usize) -> IVec3 {
    cell[axis] = 0;
    cell
}

/// The cell where `ray` crosses the plane `axis = coordinate`, with the axis component cleared.
fn plane_cell(ray: Ray, axis: usize, coordinate: f32) -> Option<IVec3> {
    let slope = ray.direction[axis];
    if slope.abs() < 1e-6 {
        return None;
    }
    let distance = (coordinate - ray.origin[axis]) / slope;
    if !(0.0..=PICK_DISTANCE * 4.0).contains(&distance) {
        return None;
    }
    let point = ray.origin + ray.direction * distance;
    Some(plane_flat(point.floor().as_ivec3(), axis))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(x: i32, z: i32) -> Ray {
        Ray {
            origin: Vec3::new(x as f32 + 0.5, 30.0, z as f32 + 0.5),
            direction: Vec3::NEG_Y,
        }
    }

    fn click(editor: &mut Editor, x: i32, z: i32) {
        let ray = down(x, z);
        editor.update_hover(ray);
        editor.press(ray, false);
        editor.release();
    }

    fn voxel(editor: &Editor, cell: IVec3) -> Option<ColorId> {
        editor
            .scene
            .model_shape(editor.focus)
            .and_then(|(_, shape)| shape.get(cell))
    }

    fn used(editor: &Editor) -> u64 {
        editor
            .scene
            .model_shape(editor.focus)
            .map_or(0, |(_, shape)| shape.used_count())
    }

    #[test]
    fn a_stroke_is_one_history_step() {
        let mut editor = Editor::new(0.25, 50);
        let ray = down(0, 0);
        editor.update_hover(ray);
        editor.press(ray, false);
        for x in 1..=2 {
            let ray = down(x, 0);
            editor.update_hover(ray);
            editor.drag(ray);
        }
        editor.release();
        assert_eq!(editor.history.done().count(), 1);
        assert_eq!(used(&editor), 3);
        editor.undo();
        assert_eq!(used(&editor), 0);
    }

    #[test]
    fn attach_strokes_ignore_voxels_they_just_placed() {
        let mut editor = Editor::new(0.25, 50);
        let ray = down(0, 0);
        editor.update_hover(ray);
        editor.press(ray, false);
        // The ray now hits the new voxel's top face one plane higher; the stroke must not stack on it.
        editor.update_hover(ray);
        editor.drag(ray);
        editor.release();
        assert!(voxel(&editor, IVec3::Y).is_none());
    }

    #[test]
    fn pick_color_puts_the_voxel_color_in_the_active_slot() {
        let mut editor = Editor::new(0.25, 50);
        editor.active_slot = 0;
        let placed = editor.color();
        click(&mut editor, 0, 0);
        editor.active_slot = 5;
        editor.tool = Tool::Pick;
        click(&mut editor, 0, 0);
        assert_eq!(editor.slots[5], placed);
    }

    #[test]
    fn a_line_is_previewed_while_dragging_and_applied_once_on_release() {
        let mut editor = Editor::new(0.25, 50);
        editor.brush.kind = BrushKind::Line;
        let ray = down(0, 0);
        editor.update_hover(ray);
        editor.press(ray, false);
        let ray = down(6, 0);
        editor.update_hover(ray);
        editor.drag(ray);
        assert_eq!(used(&editor), 0);
        editor.release();
        assert_eq!(used(&editor), 7);
        assert_eq!(editor.history.done().count(), 1);
    }

    #[test]
    fn mirrored_strokes_undo_as_one_step() {
        let mut editor = Editor::new(0.25, 50);
        click(&mut editor, 0, 0);
        click(&mut editor, 8, 0);
        editor.mirror = [true, false, false];
        click(&mut editor, 2, 0);
        assert!(voxel(&editor, IVec3::new(2, 0, 0)).is_some());
        assert!(voxel(&editor, IVec3::new(6, 0, 0)).is_some());
        editor.undo();
        assert!(voxel(&editor, IVec3::new(6, 0, 0)).is_none());
    }

    #[test]
    fn select_by_color_then_move_shifts_only_the_selection() {
        let mut editor = Editor::new(0.25, 50);
        click(&mut editor, 0, 0);
        editor.active_slot = 5;
        click(&mut editor, 3, 0);
        editor.tool = Tool::SelectColor;
        click(&mut editor, 0, 0);
        assert_eq!(editor.selection.len(), 1);

        editor.tool = Tool::Move;
        let ray = down(0, 0);
        editor.update_hover(ray);
        editor.press(ray, false);
        let ray = down(0, 2);
        editor.update_hover(ray);
        editor.drag(ray);
        editor.release();
        assert!(voxel(&editor, IVec3::ZERO).is_none());
        assert!(voxel(&editor, IVec3::new(0, 1, 0)).is_none());
        assert!(voxel(&editor, IVec3::new(0, 0, 2)).is_some());
        assert!(voxel(&editor, IVec3::new(3, 0, 0)).is_some());
    }

    #[test]
    fn stepping_in_and_out_follows_the_tree_and_undo_cannot_strand_the_focus() {
        let mut editor = Editor::new(0.25, 50);
        let model = editor.focus;
        assert!(editor.step_out());
        assert_eq!(editor.focus, NodeId::ROOT);
        assert!(!editor.tool_ready() || !editor.tool.needs_model());
        let created = editor.execute(&Command::NewModel {
            parent: NodeId::ROOT,
            name: "Other".to_owned(),
        });
        assert!(editor.step_in(created[0]));
        editor.undo();
        assert_eq!(editor.focus, NodeId::ROOT);
        assert!(editor.step_in(model));
    }

    #[test]
    fn measure_reports_the_distance_between_two_cells() {
        let mut editor = Editor::new(0.5, 50);
        editor.tool = Tool::Measure;
        click(&mut editor, 0, 0);
        click(&mut editor, 3, 4);
        let (_, _, distance) = editor.distance().unwrap();
        assert_eq!(distance.delta, IVec3::new(3, 0, 4));
        assert!((distance.voxels - 5.0).abs() < 1e-4);
        assert!((distance.metres - 2.5).abs() < 1e-4);
    }

    #[test]
    fn a_slider_drag_is_one_history_step_and_flags_the_colors() {
        let mut editor = Editor::new(0.25, 50);
        assert!(editor.take_palette_dirty(), "a new editor uploads its colors once");
        assert!(!editor.take_palette_dirty());
        let index = editor.create_graph();
        assert_eq!(index, 1);
        assert_eq!(editor.history.done().count(), 1);
        for step in 1..=4_u8 {
            let surface = SurfaceConstants {
                roughness: f32::from(step) / 8.0,
                ..SurfaceConstants::MATTE
            };
            editor.edit_graph(index, surface);
        }
        assert!(editor.take_palette_dirty());
        editor.finish_material_edit();
        assert_eq!(editor.history.done().count(), 2);
        editor.undo();
        assert_eq!(
            editor.scene.material_graphs()[1].surface_constants(),
            SurfaceConstants::MATTE
        );
        assert!(editor.take_palette_dirty(), "undo changes the colors too");
    }

    #[test]
    fn assigning_a_graph_changes_the_active_color_only() {
        let mut editor = Editor::new(0.25, 50);
        let index = editor.create_graph();
        let active = editor.color();
        editor.assign_graph(index);
        assert_eq!(editor.scene.palette().get(active).unwrap().graph, index);
        let users = editor
            .scene
            .palette()
            .iter()
            .filter(|(_, color)| color.graph == index)
            .count();
        assert_eq!(users, 1);
        assert_eq!(editor.history.done().count(), 2);
    }

    #[test]
    fn every_scene_change_moves_the_revision() {
        let mut editor = Editor::new(0.25, 50);
        let before = editor.revision;
        let model = editor.focus;
        editor.execute(&Command::SetHidden {
            node: model,
            hidden: true,
        });
        assert!(
            editor.revision > before,
            "hiding a Model changes what the tracer must upload"
        );
        let hidden = editor.revision;
        editor.undo();
        assert!(editor.revision > hidden);
    }
}
