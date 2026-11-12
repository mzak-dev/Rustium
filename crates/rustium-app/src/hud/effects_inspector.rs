//! The Effects-mode inspector: Node, Library and Material tabs. It is drawn inside the shared inspector island.

use egui::{Align2, Color32, DragValue, Grid, Id, Order, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use fluent_bundle::FluentValue;
use rustium_core::{
    ColorId, Command, GraphEdit, GraphNode, GraphNodeId, GraphNodeKind, InputValue, ParamKind, ParamValue, RAMP_STOPS,
    RampStop,
};

use super::effects::{
    self, EffectsTab, category_label, core_text, current, graph_edit, kind_description, kind_label, locked, rgb,
};
use super::effects_bars::delete_material;
use super::effects_canvas::paint_ramp;
use super::widgets::{self, Look};
use super::{Hud, effects_canvas};
use crate::icons::Icon;
use crate::theme;

const TABS: [(EffectsTab, &str); 3] = [
    (EffectsTab::Node, "tab-node"),
    (EffectsTab::Library, "tab-library"),
    (EffectsTab::Material, "tab-material"),
];

/// The tab row with the collapse button.
pub(crate) fn tabs(ui: &mut Ui, hud: &mut Hud<'_>) {
    egui::Frame::new().inner_margin(4).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (tab, key) in TABS {
                let look = if hud.ui.effects.tab == tab {
                    Look::On
                } else {
                    Look::Plain
                };
                if widgets::text_button(ui, &hud.tr(key), 32.0, look, true).clicked() {
                    if hud.ui.effects.tab != tab {
                        // The assign grid belongs to the Material tab and must not stay open behind another one.
                        hud.ui.effects.assign_open = false;
                    }
                    hud.ui.effects.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::ChevronRight, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("inspector-collapse"))
                    .clicked()
                {
                    hud.ui.inspector_open = false;
                }
            });
        });
    });
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
}

/// The body of the selected tab.
pub(crate) fn content(ui: &mut Ui, hud: &mut Hud<'_>) {
    match hud.ui.effects.tab {
        EffectsTab::Node => node_tab(ui, hud),
        EffectsTab::Library => library_tab(ui, hud),
        EffectsTab::Material => material_tab(ui, hud),
    }
}

fn note(ui: &mut Ui, text: String) {
    ui.label(RichText::new(text).color(theme::SUBTLE).size(11.0));
}

fn padded<R>(ui: &mut Ui, top: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top,
            bottom: 8,
        })
        .show(ui, add)
        .inner
}

// ---- Node tab ----------------------------------------------------------------------------------------------

fn node_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let selection = hud.ui.effects.selection.clone();
    match selection.as_slice() {
        [] => {
            padded(ui, 14, |ui| {
                note(ui, hud.tr("effects-no-selection"));
            });
        }
        [single] => {
            let Some(node) = current(hud).and_then(|graph| graph.node(*single)).cloned() else {
                return;
            };
            single_node(ui, hud, &node);
        }
        many => {
            padded(ui, 14, |ui| {
                let count = hud.i18n.tr_args(
                    "effects-selected-count",
                    &[("count", FluentValue::from(i64::try_from(many.len()).unwrap_or(0)))],
                );
                ui.label(RichText::new(count).font(theme::bold(13.0)).color(theme::TEXT));
            });
            node_actions(ui, hud, None);
        }
    }
}

fn single_node(ui: &mut Ui, hud: &mut Hud<'_>, node: &GraphNode) {
    let spec = node.kind.spec();
    let editable = !locked(hud);
    padded(ui, 14, |ui| {
        ui.horizontal(|ui| {
            let color = if node.kind == GraphNodeKind::SurfaceOutput {
                hud.accent()
            } else {
                rgb(spec.category.dot_color())
            };
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().rect_filled(dot, 2, color);
            ui.label(
                RichText::new(if node.name.is_empty() {
                    kind_label(hud, node.kind)
                } else {
                    node.name.clone()
                })
                .font(theme::bold(15.0))
                .color(theme::TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(category_label(hud, spec.category))
                        .color(theme::SUBTLE)
                        .size(11.0),
                );
            });
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(kind_description(hud, node.kind))
                .color(theme::MUTED)
                .size(11.0),
        );
    });
    ui.add_enabled_ui(editable, |ui| {
        padded(ui, 0, |ui| {
            let mut edits: Vec<GraphEdit> = Vec::new();
            Grid::new(("node-grid", node.id.get()))
                .num_columns(2)
                .min_col_width(ROW_LABEL_WIDTH)
                .spacing(vec2(8.0, 7.0))
                .show(ui, |ui| {
                    row_label(ui, hud.tr("effects-node-name"));
                    if let Some(name) = widgets::text_field(
                        ui,
                        egui::IdSalt::new(("node-name", node.id.get())),
                        &node.name,
                        true,
                        Some(&kind_label(hud, node.kind)),
                        f32::INFINITY,
                    ) {
                        edits.push(GraphEdit::RenameNode { node: node.id, name });
                    }
                    ui.end_row();
                    for (index, input) in spec.inputs.iter().enumerate() {
                        let linked = current(hud)
                            .and_then(|graph| {
                                graph.link_to(rustium_core::InputRef {
                                    node: node.id,
                                    socket: u8::try_from(index).unwrap_or(0),
                                })
                            })
                            .is_some();
                        let Some(value) = node.inputs.get(index) else {
                            continue;
                        };
                        if let Some(edit) = input_row(ui, hud, node.id, index, input.name, input.range, value, linked) {
                            edits.push(edit);
                        }
                    }
                    for (index, param) in spec.params.iter().enumerate() {
                        let Some(value) = node.params.get(index) else {
                            continue;
                        };
                        if let Some(edit) = param_row(ui, hud, node.id, index, param.name, &param.kind, value) {
                            edits.push(edit);
                        }
                    }
                });
            if node.kind == GraphNodeKind::Noise {
                note(ui, hud.tr("effects-per-voxel-hint"));
            }
            for edit in edits {
                let merge = !matches!(edit, GraphEdit::RenameNode { .. });
                graph_edit(hud, edit, merge);
            }
        });
    });
    node_actions(ui, hud, Some(node));
}

#[expect(
    clippy::too_many_arguments,
    reason = "one row of the node grid needs the whole socket description"
)]
fn input_row(
    ui: &mut Ui,
    hud: &Hud<'_>,
    node: GraphNodeId,
    index: usize,
    name: &str,
    range: (f32, f32),
    value: &InputValue,
    linked: bool,
) -> Option<GraphEdit> {
    let input = u8::try_from(index).unwrap_or(0);
    let label = core_text(hud, name);
    match value {
        InputValue::Float(current) if !linked => {
            row_label(ui, label);
            let mut number = *current;
            let changed = number_field(ui, &mut number, range, 2);
            ui.end_row();
            changed.then_some(GraphEdit::SetInput {
                node,
                input,
                value: InputValue::Float(number),
            })
        }
        InputValue::Color(current) if !linked => {
            row_label(ui, label);
            let mut rgb8 = effects::unit_to_bytes(*current);
            let changed = color_button(ui, &mut rgb8);
            ui.end_row();
            changed.then_some(GraphEdit::SetInput {
                node,
                input,
                value: InputValue::Color(effects::bytes_to_unit(rgb8)),
            })
        }
        _ => {
            row_label(ui, label);
            let text = if linked {
                hud.tr("effects-linked")
            } else {
                hud.tr("effects-auto")
            };
            ui.label(RichText::new(text).color(theme::SUBTLE).size(11.0));
            ui.end_row();
            None
        }
    }
}

fn param_row(
    ui: &mut Ui,
    hud: &Hud<'_>,
    node: GraphNodeId,
    index: usize,
    name: &str,
    kind: &ParamKind,
    value: &ParamValue,
) -> Option<GraphEdit> {
    let param = u8::try_from(index).unwrap_or(0);
    row_label(ui, core_text(hud, name));
    let new = match (kind, value) {
        (ParamKind::Float { min, max, .. }, ParamValue::Float(current)) => {
            let mut number = *current;
            number_field(ui, &mut number, (*min, *max), 2).then_some(ParamValue::Float(number))
        }
        (ParamKind::Int { min, max, .. }, ParamValue::Int(current)) => {
            let mut number = f64::from(*current);
            let changed = number_box(ui, &mut number, (f64::from(*min), f64::from(*max)), 0, 0.2);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the field is clamped to the parameter's i32 range"
            )]
            let int = number.round() as i32;
            changed.then_some(ParamValue::Int(int))
        }
        (ParamKind::Bool { .. }, ParamValue::Bool(current)) => {
            let mut on = *current;
            widgets::switch(ui, &mut on, true)
                .changed()
                .then_some(ParamValue::Bool(on))
        }
        (ParamKind::Choice { options, .. }, ParamValue::Choice(current)) => {
            let mut chosen = *current;
            let labels: Vec<(u32, String)> = options
                .iter()
                .enumerate()
                .map(|(i, option)| (u32::try_from(i).unwrap_or(0), core_text(hud, option)))
                .collect();
            if labels.len() <= 3 {
                widgets::segmented(ui, &mut chosen, &labels, true).then_some(ParamValue::Choice(chosen))
            } else {
                let selected = labels
                    .get(usize::try_from(*current).unwrap_or(0))
                    .map_or_else(String::new, |(_, label)| label.clone());
                let mut changed = false;
                egui::ComboBox::from_id_salt(("choice", node.get(), index))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (id, label) in &labels {
                            changed |= ui.selectable_value(&mut chosen, *id, label).changed();
                        }
                    });
                changed.then_some(ParamValue::Choice(chosen))
            }
        }
        (ParamKind::Range { min, max, .. }, ParamValue::Range([low, high])) => {
            let (mut a, mut b) = (f64::from(*low), f64::from(*high));
            let bounds = (f64::from(*min), f64::from(*max));
            let changed = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let half = ((ui.available_width() - 4.0) / 2.0).max(40.0);
                    let first = ui
                        .allocate_ui(vec2(half, 24.0), |ui| number_box(ui, &mut a, bounds, 2, 0.01))
                        .inner;
                    let second = ui
                        .allocate_ui(vec2(half, 24.0), |ui| number_box(ui, &mut b, bounds, 2, 0.01))
                        .inner;
                    first || second
                })
                .inner;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the fields stay inside the parameter's f32 range"
            )]
            let pair = [a as f32, b as f32];
            changed.then_some(ParamValue::Range(pair))
        }
        (ParamKind::Color { .. }, ParamValue::Color(current)) => {
            let mut rgb8 = effects::unit_to_bytes(*current);
            color_button(ui, &mut rgb8).then(|| ParamValue::Color(effects::bytes_to_unit(rgb8)))
        }
        (ParamKind::Stops, ParamValue::Stops(stops)) => {
            let edited = ramp_editor(ui, hud, stops);
            ui.end_row();
            return edited.map(|stops| GraphEdit::SetParam {
                node,
                param,
                value: ParamValue::Stops(stops),
            });
        }
        _ => None,
    };
    ui.end_row();
    new.map(|value| GraphEdit::SetParam { node, param, value })
}

/// Edits the stops of a Color ramp: a strip, then a row per stop (position, color, remove) and an add button.
fn ramp_editor(ui: &mut Ui, hud: &Hud<'_>, stops: &[RampStop]) -> Option<Vec<RampStop>> {
    let mut edited = stops.to_vec();
    let mut changed = false;
    ui.vertical(|ui| {
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 16.0), Sense::hover());
        paint_ramp(ui.painter(), bar, stops, 1.0);
        let mut remove = None;
        let count = edited.len();
        for (index, stop) in edited.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        DragValue::new(&mut stop.position)
                            .range(0.0..=1.0)
                            .speed(0.005)
                            .max_decimals(2),
                    )
                    .changed();
                let mut rgb8 = effects::unit_to_bytes(stop.color);
                if color_button(ui, &mut rgb8) {
                    stop.color = effects::bytes_to_unit(rgb8);
                    changed = true;
                }
                let can_remove = count > *RAMP_STOPS.start();
                if widgets::icon_button(ui, Icon::Close, 24.0, Look::Plain, can_remove)
                    .on_hover_text(hud.tr("effects-stop-remove"))
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            edited.remove(index);
            changed = true;
        }
        let can_add = edited.len() < *RAMP_STOPS.end();
        if widgets::text_button(ui, &hud.tr("effects-stop-add"), 24.0, Look::On, can_add).clicked() {
            let color = effects_canvas::ramp_color(&edited, 0.5);
            edited.push(RampStop {
                position: 0.5,
                color: [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0),
            });
            edited.sort_by(|a, b| a.position.total_cmp(&b.position));
            changed = true;
        }
    });
    changed.then_some(edited)
}

/// Bypass, Duplicate and Delete.
fn node_actions(ui: &mut Ui, hud: &mut Hud<'_>, node: Option<&GraphNode>) {
    let editable = !locked(hud);
    let output = node.is_some_and(|node| node.kind.is_output());
    let surface = node.is_some_and(|node| node.kind == GraphNodeKind::SurfaceOutput);
    padded(ui, 4, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = (ui.available_width() - 8.0) / 3.0;
            if let Some(node) = node {
                let look = if node.bypass { Look::Accent } else { Look::On };
                let can = editable && !output;
                if sized_button(ui, &hud.tr("effects-bypass"), width, look, can).clicked() {
                    graph_edit(
                        hud,
                        GraphEdit::SetBypass {
                            node: node.id,
                            bypass: !node.bypass,
                        },
                        false,
                    );
                }
            }
            if sized_button(ui, &hud.tr("effects-duplicate"), width, Look::On, editable && !output).clicked() {
                effects::duplicate_selection(hud);
            }
            let response = sized_button(ui, &hud.tr("effects-delete"), width, Look::On, editable && !surface);
            if response.clicked() {
                effects::delete_selection(hud);
            }
        });
        if surface {
            note(ui, hud.tr("effects-surface-protected"));
        }
        if !editable {
            note(ui, hud.tr("material-default-locked"));
        }
    });
}

fn sized_button(ui: &mut Ui, text: &str, width: f32, look: Look, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), sense);
    let (fill, foreground) = match (enabled, look) {
        (false, _) => (Color32::TRANSPARENT, theme::DISABLED),
        (true, Look::Accent) => (ui.visuals().selection.bg_fill, theme::ON_ACCENT),
        (true, _) if response.hovered() => (theme::ACTIVE, Color32::WHITE),
        (true, _) => (theme::HOVER, Color32::WHITE),
    };
    ui.painter().rect_filled(rect, 7, fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(12.0),
        foreground,
    );
    response
}

// ---- Library tab -------------------------------------------------------------------------------------------

fn library_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    padded(ui, 10, |ui| {
        let hint = hud.tr("search-nodes");
        ui.add(
            egui::TextEdit::singleline(&mut hud.ui.effects.library_query)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    });
    let query = hud.ui.effects.library_query.clone();
    let kinds = effects::matching_kinds(hud, &query, true);
    let inspector = ui.clip_rect();
    for (category, in_category) in effects::library_groups(&kinds) {
        ui.add_space(6.0);
        padded(ui, 0, |ui| {
            ui.label(
                RichText::new(category_label(hud, category).to_uppercase())
                    .color(theme::SUBTLE)
                    .size(10.0),
            );
        });
        for kind in in_category {
            let response = effects::library_row(ui, hud, kind);
            if response.drag_started() {
                hud.ui.effects.library_drag = Some(kind);
            }
            if response.clicked() && !locked(hud) {
                let screen = ui.ctx().content_rect();
                let free = effects::free_area(screen, hud.ui.inspector_open);
                let count = current(hud).map_or(0.0, |graph| graph.nodes().len() as f32);
                let at = effects::to_graph(hud, free.center()) + vec2(count % 5.0 * 20.0, count % 5.0 * 20.0);
                effects::add_node(hud, kind, at);
            }
        }
    }
    drag_ghost(ui, hud, inspector);
}

/// Shows the node kind being dragged out of the Library and drops it on the canvas when released.
fn drag_ghost(ui: &mut Ui, hud: &mut Hud<'_>, inspector: egui::Rect) {
    let Some(kind) = hud.ui.effects.library_drag else {
        return;
    };
    let ctx = ui.ctx().clone();
    let pointer = ctx.input(|input| input.pointer.interact_pos());
    if ctx.input(|input| input.pointer.any_down()) {
        if let Some(pointer) = pointer {
            egui::Area::new(Id::unique("library-ghost"))
                .order(Order::Tooltip)
                .fixed_pos(pointer + vec2(12.0, 8.0))
                .interactable(false)
                .show(&ctx, |ui| {
                    theme::island().inner_margin(6).show(ui, |ui| {
                        ui.label(RichText::new(kind_label(hud, kind)).color(theme::TEXT));
                    });
                });
        }
    } else {
        hud.ui.effects.library_drag = None;
        if let Some(pointer) = pointer
            && !inspector.contains(pointer)
            && !locked(hud)
        {
            let at = effects::to_graph(hud, pointer);
            effects::add_node(hud, kind, at);
        }
    }
}

// ---- Material tab ------------------------------------------------------------------------------------------

fn material_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let Some(graph) = current(hud) else {
        return;
    };
    let index = u32::try_from(hud.ui.effects.graph).unwrap_or(0);
    let name = effects::material_name(hud, hud.ui.effects.graph, graph);
    let casts = graph.casts_shadow;
    let editable = !locked(hud);
    let users = hud.editor.scene.colors_using(index);

    padded(ui, 12, |ui| {
        Grid::new("material-grid")
            .num_columns(2)
            .min_col_width(82.0)
            .spacing(vec2(8.0, 7.0))
            .show(ui, |ui| {
                ui.label(widgets::muted(hud.tr("material-name")));
                if let Some(text) = widgets::text_field(
                    ui,
                    egui::IdSalt::new(("material-name", index)),
                    &name,
                    editable,
                    None,
                    f32::INFINITY,
                ) && !text.trim().is_empty()
                {
                    effects::command(hud, &Command::RenameMaterial { index, name: text });
                }
                ui.end_row();

                ui.label(widgets::muted(hud.tr("effects-used-by")));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // One line: as many chips as fit beside the "+" button, then a "+N" count.
                    let room = ui.available_width() - if editable { 28.0 } else { 0.0 };
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a small positive count of chips"
                    )]
                    let slots = (room / 26.0).floor().max(1.0) as usize;
                    let shown = if users.len() > slots { slots - 1 } else { users.len() };
                    for id in users.iter().take(shown) {
                        if let Some(color) = hud.editor.scene.palette().get(*id) {
                            let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                            ui.painter().rect(
                                rect,
                                6,
                                rgb(color.color),
                                Stroke::new(1.0, Color32::from_black_alpha(128)),
                                StrokeKind::Inside,
                            );
                            response.on_hover_text(format!(
                                "#{:02X}{:02X}{:02X}",
                                color.color[0], color.color[1], color.color[2]
                            ));
                        }
                    }
                    if users.len() > shown {
                        ui.label(widgets::mono(format!("+{}", users.len() - shown), 11.0));
                    }
                    if editable {
                        let assign = widgets::icon_button(ui, Icon::Plus, 24.0, Look::Plain, true)
                            .on_hover_text(hud.tr("material-assign-more"));
                        if assign.clicked() {
                            hud.ui.effects.assign_open = !hud.ui.effects.assign_open;
                        }
                    }
                });
                ui.end_row();

                ui.label(widgets::muted(hud.tr("material-casts-shadow")));
                let mut on = casts;
                if widgets::switch(ui, &mut on, editable).changed() {
                    effects::command(
                        hud,
                        &Command::SetCastsShadow {
                            index,
                            casts_shadow: on,
                        },
                    );
                }
                ui.end_row();
            });
    });
    if hud.ui.effects.assign_open && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        hud.ui.effects.assign_open = false;
    }
    if hud.ui.effects.assign_open && editable {
        assign_grid(ui, hud, index);
    }
    padded(ui, 0, |ui| {
        note(ui, hud.tr("material-hint"));
    });
    padded(ui, 4, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = (ui.available_width() - 4.0) / 2.0;
            if sized_button(ui, &hud.tr("effects-duplicate"), width, Look::On, true).clicked()
                && effects::command(hud, &effects::duplicate_command(hud, index))
            {
                let last = hud.editor.scene.material_graphs().len().saturating_sub(1);
                hud.ui.effects.select_graph(last);
            }
            if sized_button(ui, &hud.tr("material-delete"), width, Look::On, editable).clicked() {
                delete_material(hud, hud.ui.effects.graph);
            }
        });
        ui.add_space(2.0);
        let width = ui.available_width();
        sized_button(ui, &hud.tr("effects-bake"), width, Look::Plain, false);
        if editable {
            note(ui, hud.tr("effects-bake-planned"));
        } else {
            note(ui, hud.tr("material-default-locked"));
        }
    });
}

/// A boxed monospace number field: drag to scrub, click to type.
fn number_field(ui: &mut Ui, value: &mut f32, range: (f32, f32), decimals: usize) -> bool {
    let mut wide = f64::from(*value);
    let speed = f64::from(range.1 - range.0).max(0.01) / 200.0;
    let changed = number_box(ui, &mut wide, (f64::from(range.0), f64::from(range.1)), decimals, speed);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value came from an f32 and stays in its range"
    )]
    {
        *value = wide as f32;
    }
    changed
}

fn number_box(ui: &mut Ui, value: &mut f64, range: (f64, f64), decimals: usize, speed: f64) -> bool {
    egui::Frame::new()
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            let visuals = ui.visuals_mut();
            for state in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
            ] {
                state.weak_bg_fill = Color32::TRANSPARENT;
                state.bg_fill = Color32::TRANSPARENT;
                state.bg_stroke = Stroke::NONE;
            }
            // The focused field draws its own box; the frame around it is the only one.
            ui.visuals_mut().extreme_bg_color = Color32::TRANSPARENT;
            ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
            ui.add_sized(
                vec2(ui.available_width().max(40.0), 18.0),
                DragValue::new(value)
                    .range(range.0..=range.1)
                    .speed(speed)
                    .fixed_decimals(decimals),
            )
            .changed()
        })
        .inner
}

/// "Assign to more palette colors": the Palette; a click moves a Color to this Material or back to the Default.
const ASSIGN_COLUMNS: usize = 16;
const ASSIGN_GAP: f32 = 2.0;

fn assign_grid(ui: &mut Ui, hud: &mut Hud<'_>, graph: u32) {
    padded(ui, 0, |ui| {
        note(ui, hud.tr("material-assign-hint"));
        ui.add_space(4.0);
        let ids: Vec<ColorId> = hud.editor.scene.palette().iter().map(|(id, _)| id).collect();
        let cell =
            ((ui.available_width() - ASSIGN_GAP * (ASSIGN_COLUMNS as f32 - 1.0)) / ASSIGN_COLUMNS as f32).floor();
        let rows = ids.len().div_ceil(ASSIGN_COLUMNS);
        let (area, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), rows as f32 * (cell + ASSIGN_GAP) - ASSIGN_GAP),
            Sense::hover(),
        );
        for (slot, id) in ids.into_iter().enumerate() {
            let Some(color) = hud.editor.scene.palette().get(id).copied() else {
                continue;
            };
            let (column, row) = (slot % ASSIGN_COLUMNS, slot / ASSIGN_COLUMNS);
            let rect = egui::Rect::from_min_size(
                area.min + vec2(column as f32 * (cell + ASSIGN_GAP), row as f32 * (cell + ASSIGN_GAP)),
                vec2(cell, cell),
            );
            let response = ui.interact(
                rect,
                ui.make_persistent_id(egui::IdSalt::new(("assign", graph, id.get()))),
                Sense::click(),
            );
            ui.painter().rect_filled(rect, 2, rgb(color.color));
            if color.graph == graph {
                ui.painter()
                    .rect_stroke(rect, 2, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
            }
            if response.clicked() {
                let target = if color.graph == graph { 0 } else { graph };
                effects::command(
                    hud,
                    &Command::AssignGraphToColors {
                        ids: vec![id],
                        graph: target,
                    },
                );
            }
        }
    });
}

/// Width of the label column in the node grid, the same for every node so the fields line up.
const ROW_LABEL_WIDTH: f32 = 96.0;

/// A muted row label in a column of fixed width; long names are cut short.
fn row_label(ui: &mut Ui, text: impl Into<String>) {
    ui.allocate_ui_with_layout(
        vec2(ROW_LABEL_WIDTH, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(widgets::muted(text)).truncate());
        },
    );
}

/// A swatch that opens the HUD color picker; returns whether the color changed.
fn color_button(ui: &mut Ui, rgb8: &mut [u8; 3]) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    ui.painter().rect(
        rect,
        5,
        rgb(*rgb8),
        Stroke::new(1.0, Color32::from_white_alpha(77)),
        StrokeKind::Inside,
    );
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&response).gap(6.0).show(|ui| {
        let mut color = rgb(*rgb8);
        if effects::color_picker(ui, &mut color) {
            *rgb8 = [color.r(), color.g(), color.b()];
            changed = true;
        }
    });
    changed
}
