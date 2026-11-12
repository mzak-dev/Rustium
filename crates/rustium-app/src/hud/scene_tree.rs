//! The Scene tab of the inspector: breadcrumb, Node tree with Instance counters, flags, rename and drag to
//! reparent, and the properties of the selected Node.

use egui::{Align2, Color32, DragValue, FontId, Grid, RichText, Sense, Stroke, TextEdit, Ui, vec2};
use fluent_bundle::FluentValue;
use glam::IVec3;
use rustium_core::{Command, NodeId, Orientation, Transform};

use super::Hud;
use super::widgets::{self, Look};
use crate::editor::SelectionEdit;
use crate::icons::Icon;
use crate::theme;

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 14.0;

/// One visible row of the tree, copied out of the Scene so the interface can change the Scene while drawing.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat snapshot of one tree row; each flag is drawn differently"
)]
struct Row {
    id: NodeId,
    parent: Option<NodeId>,
    depth: usize,
    name: String,
    is_group: bool,
    has_children: bool,
    expanded: bool,
    hidden: bool,
    locked: bool,
    /// Hidden or locked through an ancestor.
    inherited_off: bool,
    instances: usize,
    selected: bool,
    focused: bool,
}

/// Where a dragged Node lands relative to the row it is dropped on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropSpot {
    Before,
    Into,
    After,
}

pub(super) fn show(ui: &mut Ui, hud: &mut Hud<'_>) {
    breadcrumb(ui, hud);
    toolbar(ui, hud);
    let separator = ui.available_rect_before_wrap();
    ui.painter()
        .hline(separator.x_range(), separator.top(), Stroke::new(1.0, theme::BORDER));
    tree(ui, hud);
    let separator = ui.available_rect_before_wrap();
    ui.painter().hline(
        separator.x_range(),
        separator.top() + 6.0,
        Stroke::new(1.0, theme::BORDER),
    );
    ui.add_space(12.0);
    properties(ui, hud);
}

fn breadcrumb(ui: &mut Ui, hud: &mut Hud<'_>) {
    let path = hud.editor.scene.path(hud.editor.focus);
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 10,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (index, node) in path.iter().enumerate() {
                    let last = index + 1 == path.len();
                    if index > 0 {
                        ui.label(RichText::new("/").color(theme::DISABLED).size(11.0));
                    }
                    let name = node_label(hud, *node);
                    if last {
                        ui.label(RichText::new(name).font(theme::bold(11.0)))
                            .on_hover_text(hud.tr("breadcrumb-hint"));
                    } else if ui
                        .add(egui::Label::new(RichText::new(name).color(theme::MUTED).size(11.0)).sense(Sense::click()))
                        .on_hover_text(hud.tr("breadcrumb-hint"))
                        .clicked()
                    {
                        hud.editor.step_in(*node);
                    }
                }
            });
        });
}

fn node_label(hud: &Hud<'_>, node: NodeId) -> String {
    if node == NodeId::ROOT {
        return hud.tr("scene-root");
    }
    hud.editor
        .scene
        .node(node)
        .map_or_else(String::new, |node| node.name().to_owned())
}

fn toolbar(ui: &mut Ui, hud: &mut Hud<'_>) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let parent = group_for_new_nodes(hud);
                if widgets::text_button(ui, &hud.tr("scene-new-model"), 26.0, Look::Plain, true).clicked() {
                    let name = hud.tr("node-new-model-name");
                    let created = hud.editor.execute(&Command::NewModel { parent, name });
                    hud.editor.select_nodes(&created, SelectionEdit::Replace);
                }
                if widgets::text_button(ui, &hud.tr("scene-new-group"), 26.0, Look::Plain, true).clicked() {
                    let name = hud.tr("node-new-group-name");
                    let adopt = hud.editor.node_selection.clone();
                    let created = hud.editor.execute(&Command::NewGroup { parent, name, adopt });
                    hud.editor.select_nodes(&created, SelectionEdit::Replace);
                }
            });
        });
}

/// The Group new Nodes go into: the Focus when it is a Group, otherwise the Focus's parent.
fn group_for_new_nodes(hud: &Hud<'_>) -> NodeId {
    let scene = &hud.editor.scene;
    let focus = hud.editor.focus;
    match scene.node(focus) {
        Some(node) if node.is_group() => focus,
        Some(node) => node.parent().unwrap_or(NodeId::ROOT),
        None => NodeId::ROOT,
    }
}

fn collect_rows(hud: &Hud<'_>) -> Vec<Row> {
    let editor = &hud.editor;
    let counts = editor.scene.instance_counts();
    let mut rows = Vec::new();
    let mut stack = vec![(NodeId::ROOT, 0_usize)];
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = editor.scene.node(id) else {
            continue;
        };
        let expanded = !hud.ui.collapsed.contains(&id);
        let own_off = node.is_hidden() || node.is_locked();
        let effective_off = editor.scene.is_hidden(id) || editor.scene.is_locked(id);
        rows.push(Row {
            id,
            parent: node.parent(),
            depth,
            name: if id == NodeId::ROOT {
                hud.tr("scene-root")
            } else {
                node.name().to_owned()
            },
            is_group: node.is_group(),
            has_children: !node.children().is_empty(),
            expanded,
            hidden: node.is_hidden(),
            locked: node.is_locked(),
            inherited_off: effective_off && !own_off,
            instances: node.shape().and_then(|shape| counts.get(&shape).copied()).unwrap_or(0),
            selected: editor.node_selection.contains(&id),
            focused: editor.focus == id,
        });
        if expanded {
            stack.extend(node.children().iter().rev().map(|child| (*child, depth + 1)));
        }
    }
    rows
}

fn tree(ui: &mut Ui, hud: &mut Hud<'_>) {
    let rows = collect_rows(hud);
    let mut pending: Option<Command> = None;
    let mut renaming = hud.ui.renaming.take();
    egui::ScrollArea::vertical()
        .id_salt("scene-tree")
        .max_height(300.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Frame::new().inner_margin(4).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for row in &rows {
                    row_ui(ui, hud, row, &rows, &mut renaming, &mut pending);
                }
            });
        });
    hud.ui.renaming = renaming;
    if let Some(command) = pending {
        let created = hud.editor.execute(&command);
        if !created.is_empty() {
            hud.editor.select_nodes(&created, SelectionEdit::Replace);
        }
    }
}

fn row_ui(
    ui: &mut Ui,
    hud: &mut Hud<'_>,
    row: &Row,
    rows: &[Row],
    renaming: &mut Option<(NodeId, String)>,
    pending: &mut Option<Command>,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click_and_drag());
    let is_root = row.id == NodeId::ROOT;
    if !is_root {
        response.dnd_set_drag_payload(row.id);
    }
    let fill = if row.selected {
        theme::ACTIVE
    } else if response.hovered() {
        theme::HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 7, fill);
    if row.focused {
        let bar = egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 5.0), vec2(2.5, ROW_HEIGHT - 10.0));
        ui.painter().rect_filled(bar, 1, hud.accent());
    }

    let dimmed = row.hidden || row.inherited_off;
    let text_color = if dimmed { theme::DISABLED } else { theme::TEXT };
    let mut x = rect.left() + 8.0 + row.depth as f32 * INDENT;
    let centre_y = rect.center().y;

    // Expand / collapse chevron.
    if row.is_group && row.has_children {
        let chevron = egui::Rect::from_center_size(egui::pos2(x + 6.0, centre_y), vec2(14.0, 14.0));
        let click = ui.interact(chevron, response.id.with("chevron"), Sense::click());
        let icon = if row.expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        icon.paint(
            ui.painter(),
            chevron,
            if click.hovered() { theme::TEXT } else { theme::MUTED },
        );
        if click.clicked() {
            if row.expanded {
                hud.ui.collapsed.insert(row.id);
            } else {
                hud.ui.collapsed.remove(&row.id);
            }
        }
    }
    x += 16.0;
    let icon = if row.is_group { Icon::Folder } else { Icon::Cube };
    icon.paint(
        ui.painter(),
        egui::Rect::from_center_size(egui::pos2(x + 7.0, centre_y), vec2(14.0, 14.0)),
        if dimmed { theme::DISABLED } else { theme::SOFT },
    );
    x += 22.0;

    // Right-hand controls: lock, eye and the Instance counter.
    let mut right = rect.right() - 6.0;
    if !is_root {
        for (kind, on) in [(Flag::Lock, row.locked), (Flag::Hide, row.hidden)] {
            let button = egui::Rect::from_center_size(egui::pos2(right - 8.0, centre_y), vec2(16.0, 16.0));
            let click = ui.interact(button, response.id.with(kind as u8), Sense::click());
            let icon = match (kind, on) {
                (Flag::Lock, true) => Icon::Lock,
                (Flag::Lock, false) => Icon::Unlock,
                (Flag::Hide, true) => Icon::EyeOff,
                (Flag::Hide, false) => Icon::Eye,
            };
            let color = match (on, click.hovered()) {
                (_, true) => theme::TEXT,
                (true, false) => theme::SOFT,
                (false, false) => theme::DISABLED,
            };
            icon.paint(ui.painter(), button, color);
            if click.clicked() {
                *pending = Some(match kind {
                    Flag::Lock => Command::SetLocked {
                        node: row.id,
                        locked: !on,
                    },
                    Flag::Hide => Command::SetHidden {
                        node: row.id,
                        hidden: !on,
                    },
                });
            }
            right -= 20.0;
        }
    }
    if row.instances > 1 {
        let galley = ui.painter().text(
            egui::pos2(right - 2.0, centre_y),
            Align2::RIGHT_CENTER,
            format!("×{}", row.instances),
            FontId::monospace(10.0),
            hud.accent(),
        );
        right -= galley.width() + 6.0;
    }

    // The name, or the rename field.
    let name_rect = egui::Rect::from_min_max(
        egui::pos2(x, rect.top() + 2.0),
        egui::pos2((right - 4.0).max(x + 20.0), rect.bottom() - 2.0),
    );
    let is_renaming = renaming.as_ref().is_some_and(|(node, _)| *node == row.id);
    if is_renaming {
        rename_field(ui, hud, row.id, name_rect, renaming, pending);
    } else {
        let font = if row.focused {
            theme::bold(12.0)
        } else {
            FontId::proportional(12.0)
        };
        ui.painter().with_clip_rect(name_rect).text(
            egui::pos2(x, centre_y),
            Align2::LEFT_CENTER,
            &row.name,
            font,
            text_color,
        );
    }

    // Selection, step in and context menu.
    if response.clicked() && !is_root {
        let (command, shift) = ui.input(|input| (input.modifiers.command, input.modifiers.shift));
        let edit = if command {
            SelectionEdit::Toggle
        } else if shift {
            SelectionEdit::Add
        } else {
            SelectionEdit::Replace
        };
        hud.editor.select_nodes(&[row.id], edit);
    }
    if response.double_clicked() {
        hud.editor.step_in(row.id);
    }
    response.context_menu(|ui| context_menu(ui, hud, row, renaming, pending));

    drop_target(ui, hud, row, rows, &response, pending);
}

#[derive(Debug, Clone, Copy)]
enum Flag {
    Lock,
    Hide,
}

fn rename_field(
    ui: &mut Ui,
    hud: &mut Hud<'_>,
    node: NodeId,
    rect: egui::Rect,
    renaming: &mut Option<(NodeId, String)>,
    pending: &mut Option<Command>,
) {
    let Some((_, text)) = renaming.as_mut() else {
        return;
    };
    let edit = ui.put(
        rect,
        TextEdit::singleline(text)
            .font(FontId::proportional(12.0))
            .desired_width(rect.width()),
    );
    if !edit.has_focus() && !edit.lost_focus() {
        edit.request_focus();
    }
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    if edit.lost_focus() && !escape {
        let name = text.clone();
        *pending = Some(Command::Rename { node, name });
        *renaming = None;
    } else if escape || edit.lost_focus() {
        *renaming = None;
    }
    let _ = hud;
}

fn context_menu(
    ui: &mut Ui,
    hud: &mut Hud<'_>,
    row: &Row,
    renaming: &mut Option<(NodeId, String)>,
    pending: &mut Option<Command>,
) {
    if !hud.editor.node_selection.contains(&row.id) && row.id != NodeId::ROOT {
        hud.editor.select_nodes(&[row.id], SelectionEdit::Replace);
    }
    let is_root = row.id == NodeId::ROOT;
    let selected = hud.editor.node_selection.clone();
    let item =
        |ui: &mut Ui, label: String, enabled: bool| ui.add_enabled(enabled, egui::Button::new(label).frame(false));
    if item(ui, hud.tr("node-step-in"), true).clicked() {
        hud.editor.step_in(row.id);
        ui.close();
    }
    if item(ui, hud.tr("node-rename"), !is_root).clicked() {
        *renaming = Some((row.id, row.name.clone()));
        ui.close();
    }
    ui.separator();
    if item(ui, hud.tr("node-duplicate"), !is_root).clicked() {
        *pending = Some(Command::Duplicate { node: row.id });
        ui.close();
    }
    if item(ui, hud.tr("node-duplicate-instance"), !is_root).clicked() {
        *pending = Some(Command::DuplicateAsInstance { node: row.id });
        ui.close();
    }
    if item(
        ui,
        hud.tr("node-make-unique"),
        !is_root && (row.is_group || row.instances > 1),
    )
    .clicked()
    {
        *pending = Some(Command::MakeUnique { node: row.id });
        ui.close();
    }
    ui.separator();
    if item(
        ui,
        hud.tr("node-merge"),
        selected.len() >= 2 || row.is_group && !is_root,
    )
    .clicked()
    {
        let nodes = if selected.len() >= 2 { selected } else { vec![row.id] };
        *pending = Some(Command::Merge { nodes });
        ui.close();
    }
    let can_split = hud.editor.focus == row.id && !row.is_group && !hud.editor.selection.is_empty();
    if item(ui, hud.tr("node-split"), can_split).clicked() {
        let name = hud
            .i18n
            .tr_args("node-split-name", &[("name", FluentValue::from(row.name.clone()))]);
        *pending = Some(Command::Split {
            model: row.id,
            selection: hud.editor.selection.clone(),
            name,
        });
        ui.close();
    }
    ui.separator();
    if item(ui, hud.tr("node-delete"), !is_root).clicked() {
        *pending = Some(Command::Delete { node: row.id });
        ui.close();
    }
}

/// Highlights the drop spot while a Node is dragged over `row` and moves the Node when it is released.
fn drop_target(
    ui: &mut Ui,
    hud: &Hud<'_>,
    row: &Row,
    rows: &[Row],
    response: &egui::Response,
    pending: &mut Option<Command>,
) {
    let Some(dragged) = response.dnd_hover_payload::<NodeId>() else {
        return;
    };
    let dragged = *dragged;
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    // A Node cannot move onto itself or into its own subtree.
    if hud.editor.scene.is_ancestor_or_self(dragged, row.id) {
        return;
    }
    let spot = spot_for(row, (pointer.y - response.rect.top()) / ROW_HEIGHT);
    let (parent, index) = match spot {
        DropSpot::Into => (row.id, 0),
        DropSpot::Before | DropSpot::After => {
            let Some(parent) = row.parent else {
                return;
            };
            let siblings: Vec<NodeId> = hud
                .editor
                .scene
                .node(parent)
                .map(|node| {
                    node.children()
                        .iter()
                        .copied()
                        .filter(|child| *child != dragged)
                        .collect()
                })
                .unwrap_or_default();
            let position = siblings.iter().position(|child| *child == row.id).unwrap_or(0);
            (parent, position + usize::from(spot == DropSpot::After))
        }
    };
    let _ = rows;
    let accent = hud.accent();
    let painter = ui.painter();
    match spot {
        DropSpot::Into => {
            painter.rect_stroke(response.rect, 7, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
        }
        DropSpot::Before => {
            painter.hline(response.rect.x_range(), response.rect.top(), Stroke::new(2.0, accent));
        }
        DropSpot::After => {
            painter.hline(
                response.rect.x_range(),
                response.rect.bottom(),
                Stroke::new(2.0, accent),
            );
        }
    }
    if response.dnd_release_payload::<NodeId>().is_some() {
        *pending = Some(Command::MoveNode {
            node: dragged,
            parent,
            index,
        });
    }
}

/// Which part of a row the pointer is over, as a fraction of its height from the top.
fn spot_for(row: &Row, fraction: f32) -> DropSpot {
    if row.id == NodeId::ROOT {
        return DropSpot::Into;
    }
    if row.is_group {
        if fraction < 0.25 {
            DropSpot::Before
        } else if fraction > 0.75 {
            DropSpot::After
        } else {
            DropSpot::Into
        }
    } else if fraction < 0.5 {
        DropSpot::Before
    } else {
        DropSpot::After
    }
}

fn format_length(metres: f32) -> String {
    if metres >= 1.0 {
        format!("{metres:.2} m")
    } else {
        format!("{:.2} cm", metres * 100.0).replace(".00", "")
    }
}

/// Name, transform, size and usage of the selected Node (or the Focus when nothing is selected).
fn properties(ui: &mut Ui, hud: &mut Hud<'_>) {
    let node = hud
        .editor
        .node_selection
        .last()
        .copied()
        .filter(|node| hud.editor.scene.is_attached(*node))
        .unwrap_or(hud.editor.focus);
    let scene = &hud.editor.scene;
    let voxel_size = scene.voxel_size();
    let Some(entry) = scene.node(node) else {
        return;
    };
    let name = node_label(hud, node);
    let is_root = node == NodeId::ROOT;
    let volume = match scene.model_shape(node) {
        Some((_, shape)) => shape.volume(),
        None => scene.world_volume(node),
    };
    let world = scene.world_volume(node);
    let used = scene.used_voxels(node);
    let transform = entry.transform();
    let instances = entry
        .shape()
        .and_then(|shape| scene.instance_counts().get(&shape).copied())
        .unwrap_or(0);
    let mut command = None;

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            Grid::new("node-properties")
                .num_columns(2)
                .min_col_width(72.0)
                .spacing(vec2(8.0, 8.0))
                .show(ui, |ui| {
                    ui.label(widgets::muted(hud.tr("prop-name")));
                    if let Some(text) = widgets::text_field(
                        ui,
                        egui::IdSalt::new(("node-name", node.get())),
                        &name,
                        !is_root,
                        None,
                        150.0,
                    ) {
                        command = Some(Command::Rename { node, name: text });
                    }
                    ui.end_row();

                    ui.label(widgets::muted(hud.tr("prop-voxel-size")));
                    ui.label(widgets::mono(format_length(voxel_size), 11.0));
                    ui.end_row();

                    ui.label(widgets::muted(hud.tr("prop-size")));
                    match volume {
                        Some(volume) => {
                            let size = volume.size();
                            ui.label(widgets::mono(format!("{} × {} × {}", size.x, size.y, size.z), 11.0));
                        }
                        None => {
                            ui.label(widgets::muted(hud.tr("prop-empty")));
                        }
                    }
                    ui.end_row();

                    ui.label(widgets::muted(hud.tr("prop-world-size")));
                    match world {
                        Some(world) => {
                            let size = world.size().as_vec3() * voxel_size;
                            ui.label(widgets::mono(
                                format!("{:.2} × {:.2} × {:.2} m", size.x, size.y, size.z),
                                11.0,
                            ));
                        }
                        None => {
                            ui.label(widgets::muted("—"));
                        }
                    }
                    ui.end_row();

                    ui.label(widgets::muted(hud.tr("prop-used")));
                    ui.label(widgets::mono(used.to_string(), 11.0));
                    ui.end_row();

                    if instances > 1 {
                        ui.label(widgets::muted(hud.tr("prop-instances")));
                        ui.label(widgets::mono(format!("×{instances}"), 11.0));
                        ui.end_row();
                    }

                    if !is_root {
                        ui.label(widgets::muted(hud.tr("prop-position")));
                        ui.horizontal(|ui| {
                            let mut offset = transform.offset;
                            let mut changed = false;
                            for axis in 0..3 {
                                changed |= ui.add(DragValue::new(&mut offset[axis]).speed(0.2)).changed();
                            }
                            if changed {
                                command = Some(Command::SetTransform {
                                    node,
                                    transform: Transform { offset, ..transform },
                                });
                            }
                        });
                        ui.end_row();

                        ui.label(widgets::muted(hud.tr("prop-orientation")));
                        ui.horizontal(|ui| {
                            if widgets::text_button(ui, &hud.tr("prop-rotate"), 24.0, Look::Plain, true).clicked() {
                                command = turned(&hud.editor.scene, node, Orientation::rotation(1, 1))
                                    .map(|transform| Command::SetTransform { node, transform });
                            }
                            if widgets::text_button(ui, &hud.tr("prop-flip"), 24.0, Look::Plain, true).clicked() {
                                command = turned(&hud.editor.scene, node, Orientation::mirror(0))
                                    .map(|transform| Command::SetTransform { node, transform });
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    if let Some(command) = command {
        hud.editor.execute(&command);
    }
}

/// The Transform of `node` after turning it by `turn` in its parent's space around the centre of its Voxels, so it
/// stays where it is instead of swinging around its origin.
fn turned(scene: &rustium_core::Scene, node: NodeId, turn: Orientation) -> Option<Transform> {
    let current = scene.node(node)?.transform();
    let mut next = Transform {
        orientation: turn.compose(current.orientation),
        offset: turn.apply(current.offset),
    };
    let before = parent_space_volume(scene, node, current)?;
    let after = parent_space_volume(scene, node, next)?;
    let shift = (before.min + before.max - after.min - after.max) / 2;
    next.offset += shift;
    Some(next)
}

/// The box around the Voxels of `node` in its parent's space if its Transform were `transform`.
fn parent_space_volume(
    scene: &rustium_core::Scene,
    node: NodeId,
    transform: Transform,
) -> Option<rustium_core::Volume> {
    let inverse = scene.world_transform(node).inverse();
    let mut result: Option<rustium_core::Volume> = None;
    for model in scene.descendants(node) {
        let Some((_, shape)) = scene.model_shape(model) else {
            continue;
        };
        let Some(volume) = shape.volume() else {
            continue;
        };
        let moved = transform
            .compose(inverse.compose(scene.world_transform(model)))
            .volume(volume);
        result = Some(result.map_or(moved, |total| total.including(moved.min).including(moved.max)));
    }
    result.or(Some(rustium_core::Volume::point(IVec3::ZERO)))
}
