use std::collections::HashSet;

use glam::IVec3;
use rustium_core::{
    Brush, ChunkCoord, Command, Edit, Face, History, HitTarget, MaterialId, RayHit, Scene, Volume, VoxelTool, apply,
    brush_bounds,
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

    /// Whether this milestone implements the Tool; the others are shown disabled.
    pub(crate) fn is_available(self) -> bool {
        matches!(self, Self::Attach | Self::Erase | Self::Paint | Self::Pick)
    }

    pub(crate) fn voxel_tool(self) -> Option<VoxelTool> {
        match self {
            Self::Attach => Some(VoxelTool::Attach),
            Self::Erase => Some(VoxelTool::Erase),
            Self::Paint => Some(VoxelTool::Paint),
            _ => None,
        }
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
    edit: Option<Edit>,
}

/// The document plus everything about how the user is editing it.
#[derive(Debug)]
pub(crate) struct Editor {
    pub(crate) scene: Scene,
    pub(crate) history: History,
    pub(crate) tool: Tool,
    pub(crate) brush: Brush,
    pub(crate) mirror: [bool; 3],
    pub(crate) slots: [MaterialId; 10],
    pub(crate) active_slot: usize,
    pub(crate) hover: Option<RayHit>,
    stroke: Option<Stroke>,
    dirty: HashSet<ChunkCoord>,
    remesh_all: bool,
}

/// Default hotbar: two greys and eight hues from the default Palette (8 greys, then 11 hues × 8 shades).
const DEFAULT_SLOTS: [u8; 10] = [2, 5, 9 + 3, 17 + 3, 33 + 3, 41 + 3, 57 + 3, 73 + 3, 81 + 3, 89 + 3];

impl Editor {
    pub(crate) fn new(voxel_size: f32, undo_steps: usize) -> Self {
        let scene = Scene::with_voxel_size(voxel_size);
        let slots = DEFAULT_SLOTS.map(|number| {
            MaterialId::new(number)
                .filter(|id| scene.palette().get(*id).is_some())
                .unwrap_or(MaterialId::FIRST)
        });
        Self {
            scene,
            history: History::new(undo_steps),
            tool: Tool::Attach,
            brush: Brush::default(),
            mirror: [false; 3],
            slots,
            active_slot: 2,
            hover: None,
            stroke: None,
            dirty: HashSet::new(),
            remesh_all: true,
        }
    }

    /// Starts a new, empty Scene and clears History.
    pub(crate) fn new_scene(&mut self, voxel_size: f32, undo_steps: usize) {
        let slots = self.slots;
        *self = Self::new(voxel_size, undo_steps);
        self.slots = slots;
    }

    pub(crate) fn material(&self) -> MaterialId {
        self.slots[self.active_slot.min(self.slots.len() - 1)]
    }

    /// The voxels the current Tool would change at the hovered spot, for the hover box.
    pub(crate) fn hover_bounds(&self) -> Option<Volume> {
        let hit = self.hover?;
        match self.tool.voxel_tool() {
            Some(tool) if tool != VoxelTool::Attach && hit.target == HitTarget::Ground => None,
            Some(tool) => Some(brush_bounds(tool, self.brush, hit)),
            None if hit.target == HitTarget::Voxel => Some(Volume::point(hit.voxel)),
            None => None,
        }
    }

    pub(crate) fn is_stroking(&self) -> bool {
        self.stroke.is_some()
    }

    /// Primary button pressed over the viewport.
    pub(crate) fn press(&mut self, hit: RayHit) {
        if self.tool == Tool::Pick {
            if let (HitTarget::Voxel, Some(material)) = (hit.target, self.scene.shape().get(hit.voxel)) {
                self.slots[self.active_slot] = material;
            }
            return;
        }
        let Some(tool) = self.tool.voxel_tool() else {
            return;
        };
        let face = hit.face;
        let plane = hit.voxel[face.axis()];
        let edit = self.run(tool, hit);
        self.stroke = Some(Stroke {
            tool,
            face,
            plane,
            last_hit: hit,
            edit,
        });
    }

    /// Pointer moved while the primary button is held.
    pub(crate) fn drag(&mut self, hit: RayHit) {
        let Some(stroke) = &self.stroke else {
            return;
        };
        if hit == stroke.last_hit {
            return;
        }
        let tool = stroke.tool;
        if tool == VoxelTool::Attach && (hit.face != stroke.face || hit.voxel[hit.face.axis()] != stroke.plane) {
            return;
        }
        let edit = self.run(tool, hit);
        if let Some(stroke) = &mut self.stroke {
            stroke.last_hit = hit;
            stroke.edit = match (stroke.edit.take(), edit) {
                (Some(first), Some(next)) => Some(first.merge(next)),
                (first, next) => first.or(next),
            };
        }
    }

    /// Primary button released: the stroke becomes one History step.
    pub(crate) fn release(&mut self) {
        if let Some(edit) = self.stroke.take().and_then(|stroke| stroke.edit) {
            self.history.push(edit);
        }
    }

    pub(crate) fn undo(&mut self) {
        self.release();
        let changed: Vec<ChunkCoord> = self
            .history
            .undo(&mut self.scene)
            .map(|edit| edit.changed_chunks().collect())
            .unwrap_or_default();
        self.mark_dirty(changed);
    }

    pub(crate) fn redo(&mut self) {
        self.release();
        let changed: Vec<ChunkCoord> = self
            .history
            .redo(&mut self.scene)
            .map(|edit| edit.changed_chunks().collect())
            .unwrap_or_default();
        self.mark_dirty(changed);
    }

    fn run(&mut self, tool: VoxelTool, hit: RayHit) -> Option<Edit> {
        let command = Command::ApplyBrush {
            tool,
            brush: self.brush,
            hit,
            material: self.material(),
        };
        match apply(&mut self.scene, &command) {
            Ok(Some(edit)) => {
                let changed: Vec<ChunkCoord> = edit.changed_chunks().collect();
                self.mark_dirty(changed);
                Some(edit)
            }
            Ok(None) => None,
            Err(error) => {
                warn!(%error, "command rejected");
                None
            }
        }
    }

    /// Marks chunks and their 26 neighbours (borders affect culling and AO) for remeshing.
    // ponytail: remeshes every existing neighbour; narrow it to the touched borders if strokes stutter.
    fn mark_dirty(&mut self, chunks: Vec<ChunkCoord>) {
        for coord in chunks {
            for z in -1..=1 {
                for y in -1..=1 {
                    for x in -1..=1 {
                        let neighbour = ChunkCoord(coord.0 + IVec3::new(x, y, z));
                        if neighbour == coord || self.scene.shape().chunk(neighbour).is_some() {
                            self.dirty.insert(neighbour);
                        }
                    }
                }
            }
        }
    }

    /// Chunks to remesh since the last call, and whether every GPU chunk must be dropped first.
    pub(crate) fn take_dirty(&mut self) -> (bool, Vec<ChunkCoord>) {
        let reset = std::mem::take(&mut self.remesh_all);
        if reset {
            self.dirty.clear();
            return (true, self.scene.shape().chunks().map(|(coord, _)| coord).collect());
        }
        (false, self.dirty.drain().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ground(x: i32, z: i32) -> RayHit {
        RayHit {
            voxel: IVec3::new(x, -1, z),
            face: Face::PosY,
            target: HitTarget::Ground,
        }
    }

    #[test]
    fn a_stroke_is_one_history_step() {
        let mut editor = Editor::new(0.25, 50);
        editor.press(ground(0, 0));
        editor.drag(ground(1, 0));
        editor.drag(ground(2, 0));
        editor.release();
        assert_eq!(editor.history.done().count(), 1);
        editor.undo();
        assert!(editor.scene.shape().get(IVec3::ZERO).is_none());
    }

    #[test]
    fn attach_strokes_ignore_voxels_they_just_placed() {
        let mut editor = Editor::new(0.25, 50);
        editor.press(ground(0, 0));
        // The ray now hits the new voxel's top face one plane higher; the stroke must not stack on it.
        editor.drag(RayHit {
            voxel: IVec3::ZERO,
            face: Face::PosY,
            target: HitTarget::Voxel,
        });
        editor.release();
        assert!(editor.scene.shape().get(IVec3::Y).is_none());
    }

    #[test]
    fn pick_color_puts_the_voxel_material_in_the_active_slot() {
        let mut editor = Editor::new(0.25, 50);
        editor.active_slot = 0;
        let placed = editor.material();
        editor.press(ground(0, 0));
        editor.release();
        editor.active_slot = 5;
        editor.tool = Tool::Pick;
        editor.press(RayHit {
            voxel: IVec3::ZERO,
            face: Face::PosY,
            target: HitTarget::Voxel,
        });
        assert_eq!(editor.slots[5], placed);
    }
}
