//! Effects mode: the Material node editor. This file holds its state, the shared edit helpers, the keyboard
//! shortcuts and the popups (add node, frame label, color picker); the canvas, the islands and the inspector live
//! in the sibling `effects_*` files.

use egui::{Align2, Color32, Context, Id, Key, Modifiers, Order, Pos2, Rect, Vec2, vec2};
use rustium_core::{
    FrameId, GraphEdit, GraphNodeId, GraphNodeKind, InputRef, MaterialGraph, NodeCategory, OutputRef, ParamValue,
    SocketType,
};

use super::effects_canvas::{self, Target};
use super::{Glass, Hud, Toast, effects_bars};
use crate::keymap::Action;
use crate::theme;

/// The Tool picked in the left island. Add node, Frame and Fit are one-shot actions and not Tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GraphTool {
    Select,
    Pan,
    Cut,
}

/// Tabs of the Effects inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectsTab {
    Node,
    Library,
    Material,
}

/// What the preview island shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewShape {
    /// The open Scene.
    Model,
    /// A voxel cube painted with the Material.
    Cube,
    /// A voxel ball painted with the Material.
    Ball,
}

/// What the pointer is dragging on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Gesture {
    /// Moving the selected nodes (and frames); `anchor` is the primary node's position when the drag began.
    Move {
        nodes: Vec<GraphNodeId>,
        frames: Vec<FrameId>,
        anchor: Vec2,
        raw: Vec2,
        applied: Vec2,
    },
    /// Rubber band selection; `start` in screen points.
    Box { start: Pos2, additive: bool },
    /// A wire being pulled from a socket.
    Wire { from: WireEnd },
    /// The line of the Cut links Tool, in screen points.
    Cut { points: Vec<Pos2> },
    /// Resizing a frame from its corner.
    Resize { frame: FrameId },
    /// Scrubbing a number on a node.
    Scrub {
        node: GraphNodeId,
        target: Target,
        start: f32,
        travelled: f32,
        second: bool,
    },
}

/// The socket a wire started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WireEnd {
    Output(OutputRef),
    Input(InputRef),
}

/// The search popup of the Add node action.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AddPopup {
    /// Where it opened, in screen points; new nodes land at the same place.
    pub(crate) at: Pos2,
    pub(crate) query: String,
    pub(crate) fresh: bool,
}

/// Interface state of Effects mode; not part of the document.
#[derive(Debug)]
pub(crate) struct EffectsState {
    /// Index of the Material being edited.
    pub(crate) graph: usize,
    pub(crate) tool: GraphTool,
    pub(crate) tab: EffectsTab,
    pub(crate) snap: bool,
    pub(crate) preview: PreviewShape,
    /// Screen offset of the graph origin from the canvas corner.
    pub(crate) pan: Vec2,
    pub(crate) zoom: f32,
    pub(crate) selection: Vec<GraphNodeId>,
    pub(crate) frame_selection: Option<FrameId>,
    pub(crate) gesture: Option<Gesture>,
    pub(crate) add_popup: Option<AddPopup>,
    pub(crate) library_query: String,
    /// A node kind being dragged out of the Library.
    pub(crate) library_drag: Option<GraphNodeKind>,
    /// A frame whose label is being typed.
    pub(crate) frame_label: Option<(FrameId, String, bool)>,
    /// A color being picked for a node input or parameter.
    pub(crate) color_popup: Option<(GraphNodeId, Target, Pos2)>,
    /// Frames the color popup has been open; a press in its first frame does not close it.
    pub(crate) color_age: u8,
    /// The Material the view was last fitted to.
    pub(crate) fitted: Option<usize>,
    /// Whether the Palette picker of "Assign to more palette colors" is open.
    pub(crate) assign_open: bool,
    /// Pointer position in graph space, for placing new nodes.
    pub(crate) cursor: Vec2,
}

impl EffectsState {
    pub(crate) fn new() -> Self {
        Self {
            graph: 0,
            tool: GraphTool::Select,
            tab: EffectsTab::Node,
            snap: true,
            preview: PreviewShape::Model,
            pan: Vec2::ZERO,
            zoom: 1.0,
            selection: Vec::new(),
            frame_selection: None,
            gesture: None,
            add_popup: None,
            library_query: String::new(),
            library_drag: None,
            frame_label: None,
            color_popup: None,
            color_age: 0,
            fitted: None,
            assign_open: false,
            cursor: Vec2::ZERO,
        }
    }

    /// Switches the edited Material and forgets everything that referred to the old one.
    pub(crate) fn select_graph(&mut self, graph: usize) {
        if self.graph != graph {
            self.graph = graph;
            self.selection.clear();
            self.frame_selection = None;
            self.gesture = None;
            self.add_popup = None;
            self.frame_label = None;
            self.color_popup = None;
            self.fitted = None;
        }
    }
}

/// Grid step nodes snap to, in graph units.
pub(crate) const GRID: f32 = 20.0;

/// The Material being edited, if the index is still valid.
pub(crate) fn current<'hud>(hud: &'hud Hud<'_>) -> Option<&'hud MaterialGraph> {
    hud.editor.scene.material_graphs().get(hud.ui.effects.graph)
}

/// Whether the edited Material is the built-in Default, which cannot be changed.
pub(crate) fn locked(hud: &Hud<'_>) -> bool {
    hud.ui.effects.graph == 0
}

fn graph_index(hud: &Hud<'_>) -> u32 {
    u32::try_from(hud.ui.effects.graph).unwrap_or(0)
}

/// Applies a graph edit to the edited Material. Errors become a toast. `merge` joins the gesture in progress.
pub(crate) fn graph_edit(hud: &mut Hud<'_>, edit: GraphEdit, merge: bool) -> bool {
    let index = graph_index(hud);
    match hud.editor.edit_material(index, edit, merge) {
        Ok(changed) => changed,
        Err(error) => {
            hud.ui.toast = Some(Toast::new(error.to_string(), true));
            false
        }
    }
}

/// Applies a Command that is a History step of its own; errors become a toast.
pub(crate) fn command(hud: &mut Hud<'_>, command: &rustium_core::Command) -> bool {
    match hud.editor.execute_checked(command) {
        Ok(changed) => changed,
        Err(error) => {
            hud.ui.toast = Some(Toast::new(error.to_string(), true));
            false
        }
    }
}

/// Adds a node at `position` (graph space) and selects it.
pub(crate) fn add_node(hud: &mut Hud<'_>, kind: GraphNodeKind, position: Vec2) {
    let Some(id) = current(hud).map(MaterialGraph::next_node_id) else {
        return;
    };
    let position = if hud.ui.effects.snap {
        [snap(position.x), snap(position.y)]
    } else {
        [position.x, position.y]
    };
    if graph_edit(hud, GraphEdit::AddNode { kind, position }, false) {
        hud.ui.effects.selection = vec![id];
        hud.ui.effects.frame_selection = None;
        hud.ui.effects.tab = EffectsTab::Node;
    }
}

pub(crate) fn snap(value: f32) -> f32 {
    (value / GRID).round() * GRID
}

/// Removes the selected frame, or the selected nodes (the Surface output stays).
pub(crate) fn delete_selection(hud: &mut Hud<'_>) {
    if locked(hud) {
        return;
    }
    if let Some(frame) = hud.ui.effects.frame_selection.take() {
        graph_edit(hud, GraphEdit::RemoveFrame { frame }, false);
        return;
    }
    let Some(graph) = current(hud) else {
        return;
    };
    let nodes: Vec<GraphNodeId> = hud
        .ui
        .effects
        .selection
        .iter()
        .copied()
        .filter(|id| {
            graph
                .node(*id)
                .is_some_and(|node| node.kind != GraphNodeKind::SurfaceOutput)
        })
        .collect();
    let protected = nodes.len() < hud.ui.effects.selection.len();
    if nodes.is_empty() {
        if protected {
            hud.ui.toast = Some(Toast::new(hud.tr("effects-surface-protected"), true));
        }
        return;
    }
    if graph_edit(hud, GraphEdit::RemoveNodes { nodes }, false) {
        hud.ui.effects.selection.clear();
    }
}

/// Copies the selected nodes (without the outputs) next to the originals, with their values and the links
/// between them, as one History step.
pub(crate) fn duplicate_selection(hud: &mut Hud<'_>) {
    if locked(hud) {
        return;
    }
    let Some(graph) = current(hud) else {
        return;
    };
    let picked: Vec<_> = hud
        .ui
        .effects
        .selection
        .iter()
        .filter_map(|id| graph.node(*id))
        .filter(|node| !node.kind.is_output())
        .cloned()
        .collect();
    if picked.is_empty() {
        return;
    }
    let first = graph.next_node_id().get();
    let links: Vec<_> = graph.links().to_vec();
    let mut edits = Vec::new();
    let mut mapping = Vec::new();
    for (offset, node) in picked.iter().enumerate() {
        let id = GraphNodeId::from_raw(first + u32::try_from(offset).unwrap_or(0));
        mapping.push((node.id, id));
        edits.push(GraphEdit::AddNode {
            kind: node.kind,
            position: [node.position[0] + 40.0, node.position[1] + 40.0],
        });
        let fresh = rustium_core::GraphNode::new(id, node.kind, node.position);
        if !node.name.is_empty() {
            edits.push(GraphEdit::RenameNode {
                node: id,
                name: node.name.clone(),
            });
        }
        if node.bypass {
            edits.push(GraphEdit::SetBypass { node: id, bypass: true });
        }
        for (index, (value, default)) in node.params.iter().zip(&fresh.params).enumerate() {
            if value != default {
                edits.push(GraphEdit::SetParam {
                    node: id,
                    param: u8::try_from(index).unwrap_or(0),
                    value: value.clone(),
                });
            }
        }
        for (index, (value, default)) in node.inputs.iter().zip(&fresh.inputs).enumerate() {
            if value != default {
                edits.push(GraphEdit::SetInput {
                    node: id,
                    input: u8::try_from(index).unwrap_or(0),
                    value: *value,
                });
            }
        }
    }
    let map = |old: GraphNodeId| mapping.iter().find(|(from, _)| *from == old).map(|(_, to)| *to);
    for link in links {
        if let (Some(from), Some(to)) = (map(link.from.node), map(link.to.node)) {
            edits.push(GraphEdit::Connect {
                from: OutputRef {
                    node: from,
                    socket: link.from.socket,
                },
                to: InputRef {
                    node: to,
                    socket: link.to.socket,
                },
            });
        }
    }
    hud.editor.finish_material_edit();
    let total = edits.len();
    let mut applied = 0;
    for edit in edits {
        if !graph_edit(hud, edit, true) {
            break;
        }
        applied += 1;
    }
    hud.editor.finish_material_edit();
    if applied < total {
        // A partial copy is worse than none: take back what was applied (the first edit adds a node, so a
        // failure before any edit leaves nothing to undo).
        if applied > 0 {
            hud.editor.undo();
        }
        return;
    }
    hud.ui.effects.selection = mapping.into_iter().map(|(_, id)| id).collect();
    hud.ui.effects.frame_selection = None;
}

/// Wraps the selected nodes in a new labelled frame and starts editing its label.
pub(crate) fn frame_selection(hud: &mut Hud<'_>) {
    if locked(hud) {
        return;
    }
    let Some(graph) = current(hud) else {
        return;
    };
    let mut bounds: Option<Rect> = None;
    for id in &hud.ui.effects.selection {
        if let Some(node) = graph.node(*id) {
            let rect = effects_canvas::geometry(node).rect;
            bounds = Some(bounds.map_or(rect, |bounds| bounds.union(rect)));
        }
    }
    let Some(bounds) = bounds else {
        hud.ui.toast = Some(Toast::new(hud.tr("effects-frame-needs-nodes"), false));
        return;
    };
    let id = graph.next_frame_id();
    let label = hud.tr("effects-frame-default");
    let position = [bounds.min.x - 24.0, bounds.min.y - 40.0];
    let size = [bounds.width() + 48.0, bounds.height() + 40.0 + 28.0];
    if graph_edit(
        hud,
        GraphEdit::AddFrame {
            label: label.clone(),
            position,
            size,
        },
        false,
    ) {
        hud.ui.effects.frame_selection = Some(id);
        hud.ui.effects.selection.clear();
        hud.ui.effects.frame_label = Some((id, label, true));
    }
}

/// Zooms and pans so every node and frame is visible inside the free part of the canvas.
pub(crate) fn fit(hud: &mut Hud<'_>, free: Rect) {
    let Some(graph) = current(hud) else {
        return;
    };
    let mut bounds: Option<Rect> = None;
    for node in graph.nodes() {
        let rect = effects_canvas::geometry(node).rect;
        bounds = Some(bounds.map_or(rect, |bounds| bounds.union(rect)));
    }
    for frame in graph.frames() {
        let rect = Rect::from_min_size(
            egui::pos2(frame.position[0], frame.position[1]),
            vec2(frame.size[0], frame.size[1]),
        );
        bounds = Some(bounds.map_or(rect, |bounds| bounds.union(rect)));
    }
    let Some(bounds) = bounds else {
        return;
    };
    let bounds = bounds.expand(24.0);
    let zoom = (free.width() / bounds.width())
        .min(free.height() / bounds.height())
        .clamp(effects_canvas::MIN_ZOOM, 1.25);
    let effects = &mut hud.ui.effects;
    effects.zoom = zoom;
    // `pan` is relative to the canvas corner, which is the screen corner.
    effects.pan = free.center().to_vec2() - bounds.center().to_vec2() * zoom;
}

/// The part of the window the graph may use without hiding behind islands.
pub(crate) fn free_area(screen: Rect, inspector_open: bool) -> Rect {
    let right = if inspector_open {
        super::INSPECTOR_WIDTH + 2.0 * super::MARGIN
    } else {
        0.0
    };
    Rect::from_min_max(
        egui::pos2(screen.left() + 76.0, screen.top() + 70.0),
        egui::pos2(screen.right() - right - 12.0, screen.bottom() - 92.0),
    )
}

/// Draws the Effects HUD and handles its keyboard.
pub(crate) fn draw(ctx: &Context, hud: &mut Hud<'_>) {
    let count = hud.editor.scene.material_graphs().len();
    let effects = &mut hud.ui.effects;
    if effects.graph >= count {
        effects.graph = count.saturating_sub(1);
        effects.selection.clear();
        effects.frame_selection = None;
        effects.fitted = None;
    }
    if let Some(graph) = hud.editor.scene.material_graphs().get(hud.ui.effects.graph) {
        let nodes: Vec<GraphNodeId> = graph.nodes().iter().map(|node| node.id).collect();
        hud.ui.effects.selection.retain(|id| nodes.contains(id));
        if let Some(frame) = hud.ui.effects.frame_selection
            && graph.frame(frame).is_none()
        {
            hud.ui.effects.frame_selection = None;
        }
    }
    let screen = ctx.content_rect();
    if hud.ui.effects.fitted != Some(hud.ui.effects.graph) {
        let free = free_area(screen, hud.ui.inspector_open);
        fit(hud, free);
        hud.ui.effects.fitted = Some(hud.ui.effects.graph);
    }

    effects_bars::top_centre(ctx, hud);
    effects_bars::tools(ctx, hud);
    effects_bars::preview(ctx, hud);
    effects_bars::hotbar(ctx, hud);
    effects_bars::corners(ctx, hud);
    popups(ctx, hud);
    if !ctx.input(|input| input.pointer.any_down()) && hud.ui.effects.gesture.is_none() {
        hud.editor.finish_material_edit();
    }
}

impl GraphTool {
    /// Every Tool, in the order of the left island.
    pub(crate) const ALL: [Self; 3] = [Self::Select, Self::Pan, Self::Cut];

    /// Default key.
    pub(crate) fn key(self) -> Key {
        match self {
            Self::Select => Key::V,
            Self::Pan => Key::H,
            Self::Cut => Key::X,
        }
    }

    /// Name in the keymap file.
    pub(crate) fn action_id(self) -> &'static str {
        match self {
            Self::Select => "graph-select",
            Self::Pan => "graph-pan",
            Self::Cut => "graph-cut",
        }
    }

    /// Message key of the Tool's name.
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Select => "gt-select",
            Self::Pan => "gt-pan",
            Self::Cut => "gt-cut",
        }
    }
}

/// Runs a keymap action of Effects mode.
pub(crate) fn shortcut(ctx: &Context, hud: &mut Hud<'_>, action: Action) {
    let screen = ctx.content_rect();
    match action {
        Action::GraphTool(tool) => hud.ui.effects.tool = tool,
        Action::FrameNodes => frame_selection(hud),
        Action::FitGraph => {
            let free = free_area(screen, hud.ui.inspector_open);
            fit(hud, free);
        }
        Action::AddNode => open_add_popup(ctx, hud),
        Action::DeleteGraphSelection => delete_selection(hud),
        Action::DuplicateNodes => duplicate_selection(hud),
        Action::SelectAllNodes => {
            let ids: Vec<GraphNodeId> = current(hud)
                .map(|graph| graph.nodes().iter().map(|node| node.id).collect())
                .unwrap_or_default();
            hud.ui.effects.selection = ids;
            hud.ui.effects.frame_selection = None;
        }
        Action::ClearNodeSelection => {
            let effects = &mut hud.ui.effects;
            effects.gesture = None;
            effects.selection.clear();
            effects.frame_selection = None;
        }
        Action::MaterialSlot(slot) => {
            let index = usize::from(slot);
            if index < hud.editor.scene.material_graphs().len() {
                hud.ui.effects.select_graph(index);
            }
        }
        _ => {}
    }
}

/// Backspace also deletes, whatever the keymap says.
pub(crate) fn extra_keys(ctx: &Context, hud: &mut Hud<'_>) {
    if !ctx.egui_wants_keyboard_input() && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Backspace)) {
        delete_selection(hud);
    }
}

/// Opens the Add node popup at the pointer, or at the middle of the canvas when the pointer is elsewhere.
pub(crate) fn open_add_popup(ctx: &Context, hud: &mut Hud<'_>) {
    let screen = ctx.content_rect();
    let at = ctx
        .input(|input| input.pointer.hover_pos())
        .filter(|position| screen.contains(*position))
        .unwrap_or_else(|| screen.center());
    hud.ui.effects.add_popup = Some(AddPopup {
        at,
        query: String::new(),
        fresh: true,
    });
}

/// Converts a screen position to graph space.
pub(crate) fn to_graph(hud: &Hud<'_>, position: Pos2) -> Vec2 {
    (position.to_vec2() - hud.ui.effects.pan) / hud.ui.effects.zoom
}

/// Kinds that match `query`, in library order; an output that the graph already has is left out.
pub(crate) fn matching_kinds(hud: &Hud<'_>, query: &str) -> Vec<GraphNodeKind> {
    let query = query.trim().to_lowercase();
    let graph = current(hud);
    GraphNodeKind::ALL
        .into_iter()
        .filter(|kind| {
            let taken =
                kind.is_output() && graph.is_some_and(|graph| graph.nodes().iter().any(|node| node.kind == *kind));
            !taken && (query.is_empty() || kind_label(hud, *kind).to_lowercase().contains(&query))
        })
        .collect()
}

/// Lowercase words joined by dashes: the key suffix of a translatable core text.
pub(crate) fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// A name from `rustium-core` (node, socket, parameter or option), translated when the language files know it.
pub(crate) fn core_text(hud: &Hud<'_>, text: &str) -> String {
    hud.i18n.tr_or(&format!("gn-{}", slug(text)), text)
}

pub(crate) fn kind_label(hud: &Hud<'_>, kind: GraphNodeKind) -> String {
    core_text(hud, kind.spec().label)
}

pub(crate) fn kind_description(hud: &Hud<'_>, kind: GraphNodeKind) -> String {
    let spec = kind.spec();
    hud.i18n.tr_or(&format!("gnd-{}", slug(spec.label)), spec.description)
}

pub(crate) fn category_label(hud: &Hud<'_>, category: NodeCategory) -> String {
    core_text(hud, category.label())
}

pub(crate) fn rgb(color: [u8; 3]) -> Color32 {
    Color32::from_rgb(color[0], color[1], color[2])
}

pub(crate) fn wire_color(ty: SocketType) -> Color32 {
    rgb(ty.wire_color())
}

/// sRGB `0..=1` floats to a display color.
pub(crate) fn unit_color(color: [f32; 3]) -> Color32 {
    let [r, g, b] = unit_to_bytes(color);
    Color32::from_rgb(r, g, b)
}

/// sRGB `0..=1` floats to 8-bit channels.
pub(crate) fn unit_to_bytes(color: [f32; 3]) -> [u8; 3] {
    color.map(|value| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the value is clamped to 0..=255 before the cast"
        )]
        let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        byte
    })
}

/// 8-bit channels to sRGB `0..=1` floats.
pub(crate) fn bytes_to_unit(color: [u8; 3]) -> [f32; 3] {
    color.map(|channel| f32::from(channel) / 255.0)
}

fn popups(ctx: &Context, hud: &mut Hud<'_>) {
    add_popup(ctx, hud);
    frame_label_popup(ctx, hud);
    color_popup(ctx, hud);
}

fn add_popup(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(mut popup) = hud.ui.effects.add_popup.take() else {
        return;
    };
    // A press in the frame that opened the popup (a fast click) is not a click outside it.
    let opened_now = popup.fresh;
    let kinds = matching_kinds(hud, &popup.query);
    let mut chosen = None;
    let mut keep = true;
    let response = egui::Area::new(Id::unique("effects-add-popup"))
        .order(Order::Foreground)
        .fixed_pos(popup.at)
        .constrain(true)
        .show(ctx, |ui| {
            theme::island().fill(theme::PANEL_SOLID).inner_margin(8).show(ui, |ui| {
                ui.set_width(230.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut popup.query)
                        .hint_text(hud.tr("search-nodes"))
                        .desired_width(f32::INFINITY),
                );
                if popup.fresh {
                    edit.request_focus();
                    popup.fresh = false;
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for category in NodeCategory::ALL {
                        let in_category: Vec<GraphNodeKind> = kinds
                            .iter()
                            .copied()
                            .filter(|kind| kind.spec().category == category)
                            .collect();
                        if in_category.is_empty() {
                            continue;
                        }
                        ui.label(
                            egui::RichText::new(category_label(hud, category).to_uppercase())
                                .color(theme::SUBTLE)
                                .size(10.0),
                        );
                        for kind in in_category {
                            if library_row(ui, hud, kind).clicked() {
                                chosen = Some(kind);
                            }
                        }
                    }
                });
                if ui.input(|input| input.key_pressed(Key::Enter)) {
                    chosen = chosen.or_else(|| kinds.first().copied());
                }
                if ui.input(|input| input.key_pressed(Key::Escape)) {
                    keep = false;
                }
            });
        });
    hud.glass.push(Glass {
        rect: response.response.rect,
        radius: f32::from(theme::ISLAND_RADIUS),
    });
    let outside = !opened_now
        && ctx.input(|input| input.pointer.any_pressed())
        && ctx
            .input(|input| input.pointer.interact_pos())
            .is_some_and(|position| !response.response.rect.contains(position));
    if let Some(kind) = chosen {
        let at = to_graph(hud, popup.at);
        add_node(hud, kind, at);
    } else if keep && !outside {
        hud.ui.effects.add_popup = Some(popup);
    }
}

/// One row of a node list: category dot and name.
pub(crate) fn library_row(ui: &mut egui::Ui, hud: &Hud<'_>, kind: GraphNodeKind) -> egui::Response {
    let spec = kind.spec();
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click_and_drag());
    if response.hovered() {
        ui.painter().rect_filled(rect, 7, theme::HOVER);
    }
    let dot = Rect::from_center_size(rect.left_center() + vec2(14.0, 0.0), Vec2::splat(8.0));
    ui.painter().rect_filled(dot, 2, rgb(spec.category.dot_color()));
    ui.painter().text(
        rect.left_center() + vec2(28.0, 0.0),
        Align2::LEFT_CENTER,
        kind_label(hud, kind),
        egui::FontId::proportional(12.0),
        if response.hovered() {
            Color32::WHITE
        } else {
            theme::SOFT
        },
    );
    response.on_hover_text(kind_description(hud, kind))
}

fn frame_label_popup(ctx: &Context, hud: &mut Hud<'_>) {
    let Some((frame, mut text, mut fresh)) = hud.ui.effects.frame_label.take() else {
        return;
    };
    let Some(found) = current(hud).and_then(|graph| graph.frame(frame)).cloned() else {
        return;
    };
    let at = effects_canvas::frame_label_position(hud, &found);
    let mut done = false;
    let mut cancel = false;
    egui::Area::new(Id::unique("effects-frame-label"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(150.0));
            if fresh {
                edit.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit.id) {
                    let end = egui::text::CCursor::new(text.chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                    state.store(ui.ctx(), edit.id);
                }
                fresh = false;
            }
            if edit.lost_focus() {
                done = true;
                cancel = ui.input(|input| input.key_pressed(Key::Escape));
            }
        });
    if done {
        if !cancel {
            graph_edit(
                hud,
                GraphEdit::SetFrame {
                    frame,
                    label: text,
                    position: found.position,
                    size: found.size,
                },
                false,
            );
        }
    } else {
        hud.ui.effects.frame_label = Some((frame, text, fresh));
    }
}

fn color_popup(ctx: &Context, hud: &mut Hud<'_>) {
    let Some((node, target, at)) = hud.ui.effects.color_popup else {
        return;
    };
    let Some(current_color) = current(hud)
        .and_then(|graph| graph.node(node))
        .and_then(|found| match target {
            Target::Input(index) => match found.inputs.get(usize::from(index)) {
                Some(rustium_core::InputValue::Color(color)) => Some(*color),
                _ => None,
            },
            Target::Param(index) => match found.params.get(usize::from(index)) {
                Some(ParamValue::Color(color)) => Some(*color),
                _ => None,
            },
        })
    else {
        hud.ui.effects.color_popup = None;
        return;
    };
    let mut color = unit_color(current_color);
    let response = egui::Area::new(Id::unique("effects-color-popup"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .constrain(true)
        .show(ctx, |ui| {
            theme::island().fill(theme::PANEL_SOLID).inner_margin(8).show(ui, |ui| {
                egui::color_picker::color_picker_color32(ui, &mut color, egui::color_picker::Alpha::Opaque);
            });
        });
    hud.glass.push(Glass {
        rect: response.response.rect,
        radius: f32::from(theme::ISLAND_RADIUS),
    });
    let picked = [color.r(), color.g(), color.b()];
    if picked != unit_to_bytes(current_color) && !locked(hud) {
        set_color(hud, node, target, bytes_to_unit(picked), true);
    }
    let young = hud.ui.effects.color_age == 0;
    hud.ui.effects.color_age = hud.ui.effects.color_age.saturating_add(1);
    let outside = !young
        && ctx.input(|input| input.pointer.any_pressed())
        && ctx
            .input(|input| input.pointer.interact_pos())
            .is_some_and(|position| !response.response.rect.contains(position));
    if outside || ctx.input(|input| input.key_pressed(Key::Escape)) {
        hud.ui.effects.color_popup = None;
        hud.editor.finish_material_edit();
    }
}

/// Sets a color input or parameter.
pub(crate) fn set_color(hud: &mut Hud<'_>, node: GraphNodeId, target: Target, color: [f32; 3], merge: bool) {
    let edit = match target {
        Target::Input(input) => GraphEdit::SetInput {
            node,
            input,
            value: rustium_core::InputValue::Color(color),
        },
        Target::Param(param) => GraphEdit::SetParam {
            node,
            param,
            value: ParamValue::Color(color),
        },
    };
    graph_edit(hud, edit, merge);
}
