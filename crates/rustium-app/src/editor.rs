use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};
use rustium_core::{
    Brush, BrushKind, ChunkCoord, ColorId, Command, Edit, Face, FillMode, GraphEdit, History, HitTarget, Mirror,
    NodeId, Placement, Ray, RayHit, Scene, SceneHit, Selection, Shape, ShapeId, Transparency, Volume, Voxel, VoxelTool,
    apply, brush_bounds, brush_positions, pick, pick_background, pick_scene, select_box, select_by_color,
};
use tracing::warn;

use crate::camera::OrbitCamera;
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

/// Why the current Tool would do nothing at the pointer; the viewport cursor shows it as not allowed and a hint
/// beside the pointer says why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBlock {
    /// The Tool needs a Model in Focus.
    NeedsModel,
    /// Nothing under the pointer that the Tool can act on.
    NoTarget,
    /// Erase, Paint or a Surface Fill over the ground or air: there is no voxel to change.
    NoVoxel,
    /// Move over a voxel that is not selected.
    NotSelected,
    /// Pick color or Select color over the ground: there is no Color to take.
    NoColor,
}

impl ToolBlock {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::NeedsModel => "tool-needs-model",
            Self::NoTarget => "block-no-target",
            Self::NoVoxel => "block-no-voxel",
            Self::NotSelected => "block-not-selected",
            Self::NoColor => "block-no-color",
        }
    }
}

/// A brush gesture in progress; its Edits fold into one History step.
///
/// The stroke aims at the Shape as it was at the press, so it never builds on or digs into what it changed itself,
/// and it remembers the cells it covered: a frame that reaches no new cell does nothing.
#[derive(Debug)]
struct Stroke {
    tool: VoxelTool,
    /// The Focus Model's Shape at the press.
    snapshot: Shape,
    /// Cells the stroke already covered (before Mirror).
    done: HashSet<IVec3>,
    /// The pointer ray of the previous frame, in the Focus's cells; the stroke fills the way between the two.
    last_ray: Ray,
    mirror: Mirror,
    edit: Option<Edit>,
}

/// A Face brush gesture: the press picks the patch, the drag sets how deep it goes, the release applies it.
#[derive(Debug)]
struct FaceDrag {
    tool: VoxelTool,
    anchor: RayHit,
    depth: u32,
    mirror: Mirror,
}

/// When an edit to voxels happened, for the edit animations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditPhase {
    /// A frame of a brush stroke that is still running.
    Stroke,
    /// A whole gesture at once (Line, Box, Face, Fill).
    Commit,
    /// Undo: the change runs backwards.
    Undo,
    /// Redo.
    Redo,
}

/// What the edit animations need to know about changes to the Scene.
#[derive(Debug, Clone)]
pub(crate) enum EditEvent {
    /// Voxels changed: Shape, cell, voxel before and after.
    Voxels {
        phase: EditPhase,
        changes: Vec<(ShapeId, IVec3, Voxel, Voxel)>,
        /// Where the gesture started, in the Shape's cells (the Wave spreads from here).
        origin: IVec3,
        /// Normal of the face the gesture landed on, in the Shape's cells.
        normal: IVec3,
        /// Reflections of the gesture; images of a cell animate together with it.
        mirror: Mirror,
        /// Brush size; larger brushes animate with less amplitude.
        brush: u32,
    },
    /// The brush stroke ended and became a History step.
    StrokeEnd,
    /// The brush stroke was taken back.
    StrokeCancel,
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
    /// The voxel Selection at the press; undo puts it back.
    selection: Selection,
}

#[derive(Debug)]
enum Gesture {
    Stroke(Stroke),
    Shape(ShapeDrag),
    Face(FaceDrag),
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
#[cfg(not(target_arch = "wasm32"))]
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
    /// Where each Model mirrors, as [`Mirror::pivot`] in its cells; the origin column when not set. It stays put
    /// while the user draws, unlike the centre of the Volume.
    mirror_pivots: HashMap<NodeId, IVec3>,
    /// Changes to voxels since the last [`Editor::take_events`], for the edit animations.
    events: Vec<EditEvent>,
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
    /// Which Colors the mesher treats as transparent, and whether Materials changed since it was computed.
    transparency: Transparency,
    transparency_stale: bool,
    /// Increases with every change to the Scene; the path tracer re-uploads when it moves.
    pub(crate) revision: u64,
}

/// Default hotbar: two greys and eight hues from the default Palette (8 greys, then 11 hues × 8 shades).
const DEFAULT_SLOTS: [u8; 10] = [2, 5, 9 + 3, 17 + 3, 33 + 3, 41 + 3, 57 + 3, 73 + 3, 81 + 3, 89 + 3];

/// Furthest the pointer ray reaches, in voxels.
const PICK_DISTANCE: f32 = OrbitCamera::RAY_REACH;

/// Selections whose outer cells number up to this are outlined cell by cell; larger ones get their bounding box.
const OUTLINE_LIMIT: usize = 6000;

/// The preview under the pointer shows up to this many cells one by one; beyond it, the box around them.
const PREVIEW_LIMIT: usize = 20_000;

/// Most samples a stroke takes between two pointer positions.
const STROKE_SAMPLES: i32 = 256;

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
            mirror_pivots: HashMap::new(),
            events: Vec::new(),
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
            transparency: Transparency::OPAQUE,
            transparency_stale: true,
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

    /// Whether a press at the current hover would do nothing, mirroring the checks of [`Editor::press`].
    pub(crate) fn tool_block(&self) -> Option<ToolBlock> {
        if !self.tool_ready() {
            return Some(ToolBlock::NeedsModel);
        }
        let Some(hit) = self.hover.as_ref() else {
            // A click on nothing with Select box clears the selection.
            return (self.tool != Tool::SelectBox).then_some(ToolBlock::NoTarget);
        };
        let local = self.local_hit(hit);
        let on_voxel = local.target == HitTarget::Voxel;
        let usable = |ok: bool, block: ToolBlock| (!ok).then_some(block);
        match self.tool {
            Tool::SelectBox | Tool::Measure | Tool::Attach => None,
            Tool::Erase | Tool::Paint => usable(on_voxel, ToolBlock::NoVoxel),
            Tool::Fill => usable(on_voxel || self.fill_mode == FillMode::Volume, ToolBlock::NoVoxel),
            Tool::Pick => usable(self.color_under_hover().is_some(), ToolBlock::NoColor),
            Tool::SelectColor => usable(self.focus_color_at(&local).is_some(), ToolBlock::NoColor),
            Tool::Move if self.focus_is_model() => {
                if on_voxel {
                    usable(self.selection.contains(local.voxel), ToolBlock::NotSelected)
                } else {
                    Some(ToolBlock::NoTarget)
                }
            }
            Tool::Move => usable(
                hit.node
                    .and_then(|node| self.scene.child_toward(self.focus, node))
                    .is_some(),
                ToolBlock::NoTarget,
            ),
        }
    }

    /// The Color of the Voxel under the pointer, as the Pick color Tool would take it.
    pub(crate) fn color_under_hover(&self) -> Option<ColorId> {
        self.hover.as_ref().and_then(|hit| self.picked_color(hit))
    }

    fn picked_color(&self, hit: &SceneHit) -> Option<ColorId> {
        if hit.world.target != HitTarget::Voxel {
            return None;
        }
        let (_, shape) = self.scene.model_shape(hit.node?)?;
        shape.get(hit.local.voxel)
    }

    /// The Color at a hit in the Focus Model's cells; `None` on the ground, in air or when the Focus is a Group.
    fn focus_color_at(&self, local: &RayHit) -> Option<ColorId> {
        let (_, shape) = self.scene.model_shape(self.focus)?;
        (local.target == HitTarget::Voxel)
            .then(|| shape.get(local.voxel))
            .flatten()
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
        if self.focus_is_model() && self.tool.voxel_tool().is_some() {
            // The voxel Tools preview their cells; the box only stands in when there are too many to show.
            let (anchor, current, tool, brush) = self.voxel_gesture()?;
            if self.preview_cells().is_some() {
                return None;
            }
            let (_, shape) = self.scene.model_shape(self.focus)?;
            let bounds = brush_bounds(shape, tool, brush, anchor, current)?;
            return Some(self.focus_transform().volume(bounds));
        }
        let hit = self.hover.as_ref()?;
        if !self.focus_is_model() && matches!(self.tool, Tool::SelectBox | Tool::Move) {
            let child = self.scene.child_toward(self.focus, hit.node?)?;
            return self.scene.world_volume(child);
        }
        hit.is_voxel().then(|| Volume::point(hit.world.voxel))
    }

    /// The press and current hit, Tool and Brush of the voxel gesture the pointer would make or is making.
    fn voxel_gesture(&self) -> Option<(RayHit, RayHit, VoxelTool, Brush)> {
        match &self.gesture {
            Some(Gesture::Shape(drag)) => Some((drag.anchor, drag.current, drag.tool, self.brush)),
            Some(Gesture::Face(drag)) => Some((
                drag.anchor,
                drag.anchor,
                drag.tool,
                Brush {
                    kind: BrushKind::Face,
                    size: drag.depth,
                },
            )),
            _ => {
                let local = self.local_hit(self.hover.as_ref()?);
                Some((local, local, self.tool.voxel_tool()?, self.brush))
            }
        }
    }

    /// The Scene cells the voxel Tool would change at the pointer, Mirror images included: empty cells for Attach,
    /// filled ones for Erase and Paint. `None` when it is not a voxel Tool or there are too many cells to show.
    pub(crate) fn preview_cells(&self) -> Option<Vec<IVec3>> {
        if !self.tool_ready() {
            return None;
        }
        let (anchor, current, tool, brush) = self.voxel_gesture()?;
        let (_, shape) = self.scene.model_shape(self.focus)?;
        // During a stroke the aim is the Shape at the press, but what changes is decided by the Shape now.
        let aimed = match &self.gesture {
            Some(Gesture::Stroke(stroke)) => &stroke.snapshot,
            _ => shape,
        };
        let cells = brush_positions(aimed, tool, brush, anchor, current);
        if cells.len() > PREVIEW_LIMIT {
            return None;
        }
        let mirror = match &self.gesture {
            Some(Gesture::Stroke(stroke)) => stroke.mirror,
            Some(Gesture::Shape(drag)) => drag.mirror,
            Some(Gesture::Face(drag)) => drag.mirror,
            _ => self.mirror_now(),
        };
        let transform = self.focus_transform();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for cell in cells {
            mirror.for_each_image(cell, |image| {
                let changes = match tool {
                    VoxelTool::Attach => shape.get(image).is_none(),
                    VoxelTool::Erase | VoxelTool::Paint => shape.get(image).is_some(),
                };
                if changes && seen.insert(image) {
                    out.push(transform.cell(image));
                }
            });
        }
        (out.len() <= PREVIEW_LIMIT).then_some(out)
    }

    /// The face under the pointer that Attach, Fill and the Face brush act on, in Scene cells: the cell and face.
    pub(crate) fn hover_face(&self) -> Option<(IVec3, Face)> {
        if !self.tool_ready() || self.tool_block().is_some() {
            return None;
        }
        let shows = matches!(self.tool, Tool::Attach | Tool::Fill)
            || (self.tool.voxel_tool().is_some() && self.brush.kind == BrushKind::Face);
        if !shows || matches!(self.gesture, Some(Gesture::Stroke(_))) {
            return None;
        }
        let hit = match &self.gesture {
            Some(Gesture::Face(drag)) => self.focus_transform().hit(drag.anchor),
            _ => self.hover?.world,
        };
        Some((hit.voxel, hit.face))
    }

    /// The Focus Model's Mirror pivot (see [`Mirror::pivot`]).
    pub(crate) fn mirror_pivot(&self) -> IVec3 {
        self.mirror_pivots.get(&self.focus).copied().unwrap_or(IVec3::ZERO)
    }

    /// Puts the Focus Model's Mirror planes through the centre of its Volume as it is now (the origin when empty).
    pub(crate) fn centre_mirror(&mut self) {
        let volume = self.scene.model_shape(self.focus).and_then(|(_, shape)| shape.volume());
        let pivot = Mirror::around([true; 3], volume).pivot;
        self.mirror_pivots.insert(self.focus, pivot);
    }

    /// The Mirror planes to draw, as quads in the Scene.
    pub(crate) fn mirror_planes(&self) -> Vec<[Vec3; 4]> {
        let Some((_, shape)) = self.scene.model_shape(self.focus) else {
            return Vec::new();
        };
        let mirror = self.mirror_now();
        // Large enough to cover the Model and the plane wherever the pivot is.
        let pivot_cell = mirror.pivot / 2;
        let volume = shape
            .volume()
            .map_or(Volume::point(pivot_cell), |volume| volume.including(pivot_cell))
            .including(pivot_cell - 4)
            .including(pivot_cell + 4);
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

    /// Boxes to outline for the selected voxels and nodes, in Scene cells. Only the outer cells of the voxel
    /// Selection are outlined (the inner ones would be hidden behind them anyway).
    pub(crate) fn selection_boxes(&self) -> Vec<Volume> {
        let transform = self.focus_transform();
        let mut boxes: Vec<Volume> = Vec::new();
        let steps = [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z];
        let outer: Vec<IVec3> = self
            .selection
            .iter()
            .filter(|cell| steps.iter().any(|step| !self.selection.contains(*cell + *step)))
            .take(OUTLINE_LIMIT + 1)
            .collect();
        if outer.len() <= OUTLINE_LIMIT {
            boxes.extend(outer.into_iter().map(|cell| transform.volume(Volume::point(cell))));
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
        if within == self.focus {
            // A stroke aims at the Model as it was at the press; an empty Model offers a background on every side.
            let aimed = match &self.gesture {
                Some(Gesture::Stroke(stroke)) => Some(&stroke.snapshot),
                _ => self.scene.model_shape(self.focus).map(|(_, shape)| shape),
            };
            if let Some(shape) = aimed
                && (matches!(self.gesture, Some(Gesture::Stroke(_))) || shape.volume().is_none())
            {
                let local = aim(shape, self.local_ray(ray));
                let transform = self.focus_transform();
                self.hover = local.map(|local| SceneHit {
                    node: (local.target == HitTarget::Voxel).then_some(self.focus),
                    local,
                    world: transform.hit(local),
                });
            }
        }
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
            Tool::Attach | Tool::Erase | Tool::Paint if self.focus_is_model() => self.begin_brush(&hit, ray),
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
            Gesture::Stroke(stroke) => Gesture::Stroke(self.drag_stroke(stroke, ray)),
            Gesture::Shape(drag) => Gesture::Shape(self.drag_shape(drag, hit, ray)),
            Gesture::Face(drag) => Gesture::Face(self.drag_face(drag, ray)),
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
        let mut selection_before = self.selection.clone();
        let edit = match gesture {
            Gesture::Stroke(stroke) => {
                self.events.push(EditEvent::StrokeEnd);
                stroke.edit
            }
            Gesture::Shape(drag) => self.finish_shape(&drag),
            Gesture::Face(drag) => self.finish_face(&drag),
            Gesture::MoveVoxels(drag) | Gesture::MoveNodes(drag) => {
                selection_before = drag.selection;
                drag.edit
            }
            Gesture::Select(_) => None,
        };
        if let Some(edit) = edit {
            self.push_history(edit, selection_before);
        }
    }

    /// Takes back the gesture in progress as if it never happened: no History step, nothing to redo.
    pub(crate) fn cancel(&mut self) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        let edit = match gesture {
            Gesture::Stroke(stroke) => {
                self.events.push(EditEvent::StrokeCancel);
                stroke.edit
            }
            Gesture::MoveVoxels(drag) | Gesture::MoveNodes(drag) => {
                self.selection = drag.selection;
                drag.edit
            }
            Gesture::Select(drag) => {
                self.selection = drag.base;
                None
            }
            Gesture::Shape(_) | Gesture::Face(_) => None,
        };
        if let Some(edit) = edit {
            self.history.push(edit);
            if let Some(changed) = self
                .history
                .undo(&mut self.scene)
                .map(|edit| edit.changed_chunks().collect::<Vec<_>>())
            {
                self.mark_dirty(changed);
                self.revision += 1;
            }
            self.history.forget_undone();
            self.validate();
        }
    }

    /// Records a finished gesture as a History step, unless it ended where it started.
    fn push_history(&mut self, edit: Edit, selection_before: Selection) {
        if edit.is_noop() {
            return;
        }
        self.history
            .push(edit.with_selection(selection_before, self.selection.clone()));
    }

    /// Changes to voxels since the last call, for the edit animations.
    pub(crate) fn take_events(&mut self) -> Vec<EditEvent> {
        std::mem::take(&mut self.events)
    }

    fn note_voxels(&mut self, phase: EditPhase, edit: &Edit, hit: RayHit, mirror: Mirror) {
        let changes = edit.voxel_changes();
        if changes.is_empty() {
            return;
        }
        self.events.push(EditEvent::Voxels {
            phase,
            changes,
            origin: hit.voxel,
            normal: hit.face.normal(),
            mirror,
            brush: self.brush.size,
        });
    }

    // ---- Tools -------------------------------------------------------------------------------------------

    fn pick_color(&mut self, hit: &SceneHit) {
        if let Some(color) = self.picked_color(hit) {
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

    fn begin_brush(&mut self, hit: &SceneHit, ray: Ray) {
        let Some(tool) = self.tool.voxel_tool() else {
            return;
        };
        let local = self.local_hit(hit);
        let mirror = self.mirror_now();
        match self.brush.kind {
            BrushKind::Line | BrushKind::Box => {
                self.gesture = Some(Gesture::Shape(ShapeDrag {
                    tool,
                    anchor: local,
                    current: local,
                    mirror,
                }));
            }
            BrushKind::Face => {
                self.gesture = Some(Gesture::Face(FaceDrag {
                    tool,
                    anchor: local,
                    depth: self.brush.size,
                    mirror,
                }));
            }
            BrushKind::Voxel => {
                let Some((_, shape)) = self.scene.model_shape(self.focus) else {
                    return;
                };
                let mut stroke = Stroke {
                    tool,
                    snapshot: shape.clone(),
                    done: HashSet::new(),
                    last_ray: self.local_ray(ray),
                    mirror,
                    edit: None,
                };
                self.stroke_at(&mut stroke, local);
                self.gesture = Some(Gesture::Stroke(stroke));
            }
        }
    }

    /// Applies the brush at `hit` unless every cell it covers was already covered by this stroke.
    fn stroke_at(&mut self, stroke: &mut Stroke, hit: RayHit) {
        let cells = brush_positions(&stroke.snapshot, stroke.tool, self.brush, hit, hit);
        if cells.iter().all(|cell| stroke.done.contains(cell)) {
            return;
        }
        stroke.done.extend(cells);
        let edit = self.run_brush(stroke.tool, self.brush, hit, None, stroke.mirror);
        if let Some(edit) = &edit {
            self.note_voxels(EditPhase::Stroke, edit, hit, stroke.mirror);
        }
        stroke.edit = merge(stroke.edit.take(), edit);
    }

    /// Covers the way from the previous pointer position to this one, so a fast stroke leaves no gaps.
    fn drag_stroke(&mut self, mut stroke: Stroke, ray: Ray) -> Stroke {
        let ray = self.local_ray(ray);
        let previous = stroke.last_ray;
        stroke.last_ray = ray;
        let ends = (aim(&stroke.snapshot, previous), aim(&stroke.snapshot, ray));
        // One sample per cell of travel; a pointer that slides off the Model gets a fixed number.
        let samples = match ends {
            (Some(from), Some(to)) => (to.voxel - from.voxel).abs().max_element(),
            _ => 16,
        }
        .clamp(1, STROKE_SAMPLES);
        for step in 1..=samples {
            let t = step as f32 / samples as f32;
            let sample = Ray {
                origin: previous.origin.lerp(ray.origin, t),
                direction: previous.direction.lerp(ray.direction, t),
            };
            if let Some(hit) = aim(&stroke.snapshot, sample) {
                self.stroke_at(&mut stroke, hit);
            }
        }
        stroke
    }

    /// The Face brush depth follows the pointer along the face normal: out of the face for Attach, into it for Erase
    /// and Paint, starting from the Size.
    fn drag_face(&mut self, mut drag: FaceDrag, ray: Ray) -> FaceDrag {
        let ray = self.local_ray(ray);
        let normal = drag.anchor.face.normal().as_vec3();
        let centre = drag.anchor.voxel.as_vec3() + Vec3::splat(0.5) + normal * 0.5;
        // Closest approach between the pointer ray and the line through the face centre along the normal.
        let direction = ray.direction.normalize_or_zero();
        let (b, w) = (normal.dot(direction), centre - ray.origin);
        let denominator = 1.0 - b * b;
        if denominator < 1e-4 {
            return drag;
        }
        let along = (b * direction.dot(w) - normal.dot(w)) / denominator;
        let signed = if drag.tool == VoxelTool::Attach { along } else { -along };
        // The pointer stays inside the outermost layer.
        let depth = (self.brush.size as f32 + signed.floor()).clamp(1.0, Brush::MAX_SIZE as f32);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number within 1..=MAX_SIZE"
        )]
        let depth = depth as u32;
        drag.depth = depth;
        drag
    }

    fn finish_face(&mut self, drag: &FaceDrag) -> Option<Edit> {
        let brush = Brush {
            kind: BrushKind::Face,
            size: drag.depth,
        };
        let edit = self.run_brush(drag.tool, brush, drag.anchor, None, drag.mirror)?;
        self.note_voxels(EditPhase::Commit, &edit, drag.anchor, drag.mirror);
        Some(edit)
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
        let edit = self.run_brush(drag.tool, self.brush, drag.anchor, Some(drag.current), drag.mirror)?;
        self.note_voxels(EditPhase::Commit, &edit, drag.anchor, drag.mirror);
        Some(edit)
    }

    fn fill(&mut self, hit: &SceneHit) {
        let local = self.local_hit(hit);
        let mirror = self.mirror_now();
        let command = Command::Fill {
            model: self.focus,
            mode: self.fill_mode,
            hit: local,
            color: self.color(),
            mirror,
        };
        if let Some(edit) = self.apply_command(&command) {
            self.note_voxels(EditPhase::Commit, &edit, local, mirror);
            self.push_history(edit, self.selection.clone());
        }
    }

    fn select_color(&mut self, hit: &SceneHit, extend: bool) {
        let Some((_, shape)) = self.scene.model_shape(self.focus) else {
            return;
        };
        let Some(color) = self.focus_color_at(&self.local_hit(hit)) else {
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
                selection: self.selection.clone(),
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
            selection: self.selection.clone(),
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
        Mirror {
            axes: self.mirror,
            pivot: self.mirror_pivot(),
        }
    }

    fn run_brush(
        &mut self,
        tool: VoxelTool,
        brush: Brush,
        hit: RayHit,
        end: Option<RayHit>,
        mirror: Mirror,
    ) -> Option<Edit> {
        let command = Command::ApplyBrush {
            model: self.focus,
            tool,
            brush,
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
        self.mark_materials(edit.touches_materials());
    }

    /// Applies a Command from outside the viewport (MCP) as one History step. Unlike [`Editor::execute`] it
    /// reports rejections; returns whether anything changed.
    pub(crate) fn execute_checked(&mut self, command: &Command) -> Result<bool, rustium_core::Error> {
        self.release();
        let Some(edit) = self.apply_checked(command)? else {
            return Ok(false);
        };
        self.push_history(edit, self.selection.clone());
        self.validate();
        Ok(true)
    }

    /// Runs a Luau script on the Model `model` as one History step. A failing script changes nothing.
    #[cfg(not(target_arch = "wasm32"))]
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
        self.push_history(edit, self.selection.clone());
        self.validate();
        created
    }

    pub(crate) fn undo(&mut self) {
        self.step_history(EditPhase::Undo);
    }

    pub(crate) fn redo(&mut self) {
        self.step_history(EditPhase::Redo);
    }

    /// Undo or redo one step; the Selection returns to what it was on that side of the step.
    fn step_history(&mut self, phase: EditPhase) {
        self.release();
        let edit = match phase {
            EditPhase::Undo => self.history.undo(&mut self.scene),
            _ => self.history.redo(&mut self.scene),
        };
        let Some(edit) = edit else {
            // Nothing to undo or redo: the document must not look edited.
            return;
        };
        let changed: Vec<_> = edit.changed_chunks().collect();
        let colors = edit.touches_materials();
        let selection = match phase {
            EditPhase::Undo => edit.selection_before(),
            _ => edit.selection_after(),
        }
        .cloned();
        let mut changes = edit.voxel_changes();
        if phase == EditPhase::Undo {
            for (_, _, before, after) in &mut changes {
                std::mem::swap(before, after);
            }
        }
        self.revision += 1;
        self.mark_materials(colors);
        self.mark_dirty(changed);
        if let Some(selection) = selection {
            self.selection = selection;
        }
        if let Some(&(_, origin, _, _)) = changes.first() {
            self.events.push(EditEvent::Voxels {
                phase,
                changes,
                origin,
                normal: IVec3::Y,
                mirror: Mirror::default(),
                brush: 1,
            });
        }
        self.validate();
    }

    /// Gives the History step in progress another name (a gesture made of several edits, like a duplicate).
    pub(crate) fn relabel_material_edit(&mut self, kind: rustium_core::EditKind) {
        self.material_edit = self.material_edit.take().map(|edit| edit.with_kind(kind));
    }

    /// Takes back the gesture of Material edits in progress as if it never happened: nothing stays in the redo stack
    /// and the revision returns to `revision`, the one before the gesture, so the document does not look edited.
    pub(crate) fn roll_back_material_edit(&mut self, revision: u64) {
        let had_edit = self.material_edit.is_some();
        self.finish_material_edit();
        if had_edit {
            self.undo();
            self.history.forget_undone();
        }
        self.revision = revision;
    }

    /// Ends a Material slider drag: its Edits become one History step.
    pub(crate) fn finish_material_edit(&mut self) {
        if let Some(edit) = self.material_edit.take() {
            self.history.push(edit);
        }
    }

    /// Applies an edit inside Material `index`. With `merge` it joins the History step of the gesture in progress
    /// (a drag or slider), which [`Editor::finish_material_edit`] closes; without it the edit is a step of its own.
    /// Returns whether anything changed.
    ///
    /// # Errors
    ///
    /// The graph rejects the edit (a cycle, a locked Default material, a limit).
    pub(crate) fn edit_material(
        &mut self,
        index: u32,
        edit: GraphEdit,
        merge: bool,
    ) -> Result<bool, rustium_core::Error> {
        if !merge {
            self.finish_material_edit();
        }
        let applied = self.apply_checked(&Command::EditMaterial { index, edit })?;
        let Some(applied) = applied else {
            return Ok(false);
        };
        self.material_edit = Some(match self.material_edit.take() {
            Some(pending) => pending.merge(applied),
            None => applied,
        });
        if !merge {
            self.finish_material_edit();
        }
        Ok(true)
    }

    /// Whether Colors or material graphs changed since the last call, so the GPU copy needs uploading.
    pub(crate) fn take_palette_dirty(&mut self) -> bool {
        std::mem::take(&mut self.palette_dirty)
    }

    fn mark_materials(&mut self, changed: bool) {
        self.palette_dirty |= changed;
        self.transparency_stale |= changed;
    }

    /// The transparent Colors the chunk meshes were built with.
    pub(crate) fn transparency(&self) -> &Transparency {
        &self.transparency
    }

    /// Marks chunks and their 26 neighbours (borders affect culling and AO) for remeshing.
    // ponytail: remeshes every existing neighbour; narrow it to the touched borders if strokes stutter.
    pub(crate) fn mark_dirty(&mut self, chunks: Vec<(ShapeId, ChunkCoord)>) {
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
        if std::mem::take(&mut self.transparency_stale) {
            let transparency = Transparency::of_scene(&self.scene);
            if transparency != self.transparency {
                // Which faces are hidden depends on it, so every chunk is meshed again.
                self.transparency = transparency;
                self.remesh_all = true;
            }
        }
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
        HitTarget::Ground => hit.voxel + hit.face.normal(),
    }
}

/// What a ray in a Model's cells hits in `shape`: a voxel, the ground or, for an empty Shape, the background that
/// faces the camera from any side.
fn aim(shape: &Shape, ray: Ray) -> Option<RayHit> {
    if shape.volume().is_none() {
        return pick_background(ray, PICK_DISTANCE);
    }
    pick(shape, ray, PICK_DISTANCE)
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
    fn undo_and_redo_with_nothing_to_do_leave_the_document_unchanged() {
        let mut editor = Editor::new(0.25, 50);
        let revision = editor.revision;
        editor.undo();
        editor.redo();
        assert_eq!(editor.revision, revision);
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

    /// Presses at the first cell, drags through the others (one frame each) and releases.
    fn stroke(editor: &mut Editor, cells: &[(i32, i32)]) {
        let (first, rest) = cells.split_first().unwrap();
        let ray = down(first.0, first.1);
        editor.update_hover(ray);
        editor.press(ray, false);
        for (x, z) in rest {
            let ray = down(*x, *z);
            editor.update_hover(ray);
            editor.drag(ray);
        }
        editor.release();
    }

    #[test]
    fn an_attach_stroke_climbs_onto_a_step_and_back_down() {
        let mut editor = Editor::new(0.25, 50);
        click(&mut editor, 2, 0);
        stroke(&mut editor, &[(0, 0), (1, 0), (2, 0), (3, 0)]);
        for cell in [IVec3::ZERO, IVec3::X, IVec3::new(2, 1, 0), IVec3::new(3, 0, 0)] {
            assert!(voxel(&editor, cell).is_some(), "{cell}");
        }
        assert!(
            voxel(&editor, IVec3::new(2, 2, 0)).is_none(),
            "the stroke never builds on itself"
        );
    }

    #[test]
    fn a_fast_stroke_fills_the_gap_between_frames() {
        let mut editor = Editor::new(0.25, 50);
        stroke(&mut editor, &[(0, 0), (6, 0)]);
        assert_eq!(used(&editor), 7);
    }

    #[test]
    fn an_erase_stroke_takes_one_layer_however_long_it_lingers() {
        let mut editor = Editor::new(0.25, 50);
        click(&mut editor, 0, 0);
        for y in 1..3 {
            let hit = RayHit {
                voxel: IVec3::new(0, y - 1, 0),
                face: Face::PosY,
                target: HitTarget::Voxel,
            };
            editor.run_brush(VoxelTool::Attach, editor.brush, hit, None, Mirror::default());
        }
        assert_eq!(used(&editor), 3);
        editor.tool = Tool::Erase;
        stroke(&mut editor, &[(0, 0), (0, 0), (0, 0)]);
        assert_eq!(used(&editor), 2);
    }

    #[test]
    fn a_face_drag_sets_the_depth_and_applies_on_release() {
        let mut editor = Editor::new(0.25, 50);
        for x in 0..3 {
            click(&mut editor, x, 0);
        }
        editor.brush.kind = BrushKind::Face;
        let press = down(1, 0);
        editor.update_hover(press);
        editor.press(press, false);
        // A pointer ray from the side, crossing the normal line 2.5 cells above the face.
        let side = Ray {
            origin: Vec3::new(20.0, 3.5, 0.5),
            direction: Vec3::NEG_X,
        };
        editor.update_hover(side);
        editor.drag(side);
        assert_eq!(used(&editor), 3, "nothing changes before the release");
        editor.release();
        assert_eq!(used(&editor), 3 + 3 * 3);
        assert_eq!(editor.history.done().count(), 4);
    }

    #[test]
    fn cancel_takes_a_stroke_back_without_a_history_step() {
        let mut editor = Editor::new(0.25, 50);
        let ray = down(0, 0);
        editor.update_hover(ray);
        editor.press(ray, false);
        editor.cancel();
        assert_eq!(used(&editor), 0);
        assert!(!editor.history.can_undo() && !editor.history.can_redo());
    }

    #[test]
    fn undo_and_redo_bring_the_selection_back() {
        let mut editor = Editor::new(0.25, 50);
        click(&mut editor, 0, 0);
        click(&mut editor, 3, 0);
        editor.tool = Tool::SelectColor;
        click(&mut editor, 0, 0);
        let selected = editor.selection.clone();
        editor.tool = Tool::Move;
        stroke(&mut editor, &[(0, 0), (0, 2)]);
        let moved = editor.selection.clone();
        editor.undo();
        assert_eq!(editor.selection, selected);
        editor.redo();
        assert_eq!(editor.selection, moved);
    }

    #[test]
    fn a_move_dragged_out_and_back_leaves_no_history_step() {
        let mut editor = Editor::new(0.25, 50);
        click(&mut editor, 0, 0);
        editor.tool = Tool::SelectColor;
        click(&mut editor, 0, 0);
        editor.tool = Tool::Move;
        stroke(&mut editor, &[(0, 0), (0, 3), (0, 0)]);
        assert_eq!(editor.history.done().count(), 1);
    }

    #[test]
    fn mirror_stays_put_while_the_model_grows() {
        let mut editor = Editor::new(0.25, 50);
        editor.mirror = [true, false, false];
        click(&mut editor, 3, 0);
        click(&mut editor, 5, 0);
        // The pivot is the origin column: x mirrors to -x.
        assert!(voxel(&editor, IVec3::new(-3, 0, 0)).is_some());
        assert!(voxel(&editor, IVec3::new(-5, 0, 0)).is_some());
    }

    #[test]
    fn an_empty_model_can_be_built_on_from_below() {
        let mut editor = Editor::new(0.25, 50);
        let ray = Ray {
            origin: Vec3::new(1.5, -10.0, 1.5),
            direction: Vec3::Y,
        };
        editor.update_hover(ray);
        editor.press(ray, false);
        editor.release();
        assert!(voxel(&editor, IVec3::new(1, -1, 1)).is_some());
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
        editor.centre_mirror();
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
    fn a_graph_drag_is_one_history_step_and_flags_the_colors() {
        let mut editor = Editor::new(0.25, 50);
        assert!(editor.take_palette_dirty(), "a new editor uploads its colors once");
        assert!(!editor.take_palette_dirty());
        editor
            .execute_checked(&Command::AddMaterial {
                graph: Box::new(rustium_core::MaterialGraph::blank("Test")),
            })
            .unwrap();
        assert_eq!(editor.history.done().count(), 1);
        let surface = editor.scene.material_graphs()[1].surface_output().unwrap().id;
        for _ in 0..4 {
            let edit = GraphEdit::MoveNodes {
                nodes: vec![surface],
                frames: Vec::new(),
                delta: [10.0, 0.0],
            };
            editor.edit_material(1, edit, true).unwrap();
        }
        editor.finish_material_edit();
        assert_eq!(editor.history.done().count(), 2);
        editor.undo();
        assert_eq!(editor.history.done().count(), 1);
        assert!(editor.take_palette_dirty());
    }

    #[test]
    fn a_rolled_back_gesture_leaves_no_redo_step_and_no_new_revision() {
        let mut editor = Editor::new(0.25, 50);
        editor
            .execute_checked(&Command::AddMaterial {
                graph: Box::new(rustium_core::MaterialGraph::blank("Test")),
            })
            .unwrap();
        let revision = editor.revision;
        let done = editor.history.done().count();
        let surface = editor.scene.material_graphs()[1].surface_output().unwrap().id;
        let edit = GraphEdit::MoveNodes {
            nodes: vec![surface],
            frames: Vec::new(),
            delta: [10.0, 0.0],
        };
        editor.edit_material(1, edit, true).unwrap();
        editor.roll_back_material_edit(revision);
        assert_eq!(editor.revision, revision);
        assert_eq!(editor.history.done().count(), done);
        assert!(!editor.history.can_redo(), "a half gesture must not be redoable");
        // Nothing in progress: only the revision is restored.
        editor.roll_back_material_edit(revision);
        assert_eq!(editor.history.done().count(), done);
    }

    #[test]
    fn the_default_material_rejects_graph_edits() {
        let mut editor = Editor::new(0.25, 50);
        let edit = GraphEdit::AddFrame {
            label: String::new(),
            position: [0.0, 0.0],
            size: [10.0, 10.0],
        };
        assert!(editor.edit_material(0, edit, false).is_err());
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
