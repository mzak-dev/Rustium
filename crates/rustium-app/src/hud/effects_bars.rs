//! The islands of Effects mode around the node canvas: effect type and Material (top centre), graph tools (left),
//! preview, Materials hotbar (bottom centre) and the hints and status corners.

use egui::{
    Align2, Color32, Context, FontId, Id, Order, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
    pos2, vec2,
};
use fluent_bundle::FluentValue;
use rustium_core::{Command, GraphEdit, InputRef, MaterialGraph, SampleMaterial};
use rustium_render::CompileStatus;

use super::effects::{self, EffectsTab, GraphTool, PreviewShape, rgb};
use super::widgets::{self, Look};
use super::{
    Glass, Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, centre_budget, corner, corner_budget, fit_key, fit_level, island,
};
use crate::icons::Icon;
use crate::theme;

/// Width of the preview island in points.
pub(crate) const PREVIEW_WIDTH: f32 = 252.0;
const PREVIEW_HEADER: f32 = 40.0;
const PREVIEW_BODY: f32 = 148.0;
/// Gap between the preview island and the bottom edge of the window.
const PREVIEW_BOTTOM: f32 = 46.0;

/// Where the preview island sits (whole island, header included).
pub(crate) fn preview_island_rect(screen: Rect) -> Rect {
    let height = PREVIEW_HEADER + PREVIEW_BODY;
    Rect::from_min_size(
        pos2(screen.left() + MARGIN, screen.bottom() - PREVIEW_BOTTOM - height),
        vec2(PREVIEW_WIDTH, height),
    )
}

/// The part of the preview island where the 3D view shows through.
pub(crate) fn preview_view_rect(screen: Rect) -> Rect {
    let island = preview_island_rect(screen);
    Rect::from_min_max(pos2(island.left(), island.top() + PREVIEW_HEADER), island.max)
}

/// The most compact layouts of the islands that share the window's centre.
const TOP_MAX_LEVEL: u8 = 3;
const HOTBAR_MAX_LEVEL: u8 = 4;
/// The hints island keeps three hints, then two, then one.
const HINTS_MAX_LEVEL: u8 = 2;

pub(crate) fn top_centre(ctx: &Context, hud: &mut Hud<'_>) {
    let budget = centre_budget(ctx, hud, "top-left", "top-right");
    let name = effects::current(hud).map(|graph| graph.name.clone());
    let users = hud
        .editor
        .scene
        .colors_using(u32::try_from(hud.ui.effects.graph).unwrap_or(0))
        .len();
    let key = fit_key(&[
        &hud.settings.language,
        &hud.settings.ui_scale_percent,
        &hud.ui.effects.graph,
        &name,
        &users,
    ]);
    // 0: everything. 1: no "Used by". 2: also no inactive tab names. 3: only the Material.
    let level = fit_level(ctx, hud, "effects-top", budget, key, TOP_MAX_LEVEL);
    island(
        ctx,
        hud,
        "effects-top",
        Place::Anchor(Align2::CENTER_TOP, vec2(0.0, MARGIN)),
        Intro {
            delay: 0.06,
            from: vec2(0.0, -16.0),
        },
        theme::island().inner_margin(egui::Margin {
            left: 4,
            right: 10,
            top: 4,
            bottom: 4,
        }),
        |ui, hud| {
            ui.set_height(TOP_BAR_HEIGHT - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if level < 3 {
                    widgets::text_button(ui, &hud.tr("effects-materials"), 32.0, Look::On, true);
                }
                if level < 2 {
                    widgets::text_button(ui, &hud.tr("effects-textures"), 32.0, Look::Plain, false)
                        .on_hover_text(hud.tr("planned"));
                    widgets::text_button(ui, &hud.tr("effects-scripts"), 32.0, Look::Plain, false)
                        .on_hover_text(hud.tr("planned"));
                }
                if level < 3 {
                    widgets::divider(ui, 32.0);
                }
                material_menu(ui, hud);
                if level < 1 {
                    widgets::divider(ui, 32.0);
                    used_by(ui, hud, 18.0, 6);
                }
            });
        },
    );
}

fn material_menu(ui: &mut Ui, hud: &mut Hud<'_>) {
    let selected = hud.ui.effects.graph;
    let name = effects::current(hud).map_or_else(String::new, |graph| effects::material_name(hud, selected, graph));
    let button = widgets::dropdown_button(ui, &name);
    let names: Vec<String> = hud
        .editor
        .scene
        .material_graphs()
        .iter()
        .enumerate()
        .map(|(index, graph)| effects::material_name(hud, index, graph))
        .collect();
    egui::Popup::menu(&button).show(|ui| {
        ui.set_min_width(180.0);
        for (index, name) in names.iter().enumerate() {
            if ui.selectable_label(index == selected, name).clicked() {
                hud.ui.effects.select_graph(index);
            }
        }
    });
}

/// "Used by" swatches of the edited Material and the count.
fn used_by(ui: &mut Ui, hud: &Hud<'_>, size: f32, limit: usize) {
    let index = u32::try_from(hud.ui.effects.graph).unwrap_or(0);
    let ids = hud.editor.scene.colors_using(index);
    ui.label(widgets::muted(hud.tr("effects-used-by")));
    for id in ids.iter().take(limit) {
        let Some(color) = hud.editor.scene.palette().get(*id) else {
            continue;
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        ui.painter().rect(
            rect,
            5,
            rgb(color.color),
            Stroke::new(1.0, Color32::from_black_alpha(128)),
            StrokeKind::Inside,
        );
        response.on_hover_text(format!(
            "#{:02X}{:02X}{:02X}",
            color.color[0], color.color[1], color.color[2]
        ));
    }
    let count = hud.i18n.tr_args(
        "effects-colors-count",
        &[("count", FluentValue::from(i64::try_from(ids.len()).unwrap_or(0)))],
    );
    ui.label(widgets::mono(count, 11.0).color(theme::MUTED));
}

/// Name, key, description key and icon of a graph Tool or action.
pub(super) struct ToolInfo {
    pub(super) icon: Icon,
    pub(super) key: &'static str,
    key_label: &'static str,
    letter: bool,
    pub(super) tool: Option<GraphTool>,
    gap: bool,
}

pub(super) const TOOLS: [ToolInfo; 6] = [
    ToolInfo {
        icon: Icon::Pointer,
        key: "gt-select",
        key_label: "V",
        letter: true,
        tool: Some(GraphTool::Select),
        gap: false,
    },
    ToolInfo {
        icon: Icon::Move,
        key: "gt-pan",
        key_label: "H",
        letter: true,
        tool: Some(GraphTool::Pan),
        gap: false,
    },
    ToolInfo {
        icon: Icon::AddNode,
        key: "gt-add",
        key_label: "Tab",
        letter: false,
        tool: None,
        gap: true,
    },
    ToolInfo {
        icon: Icon::Close,
        key: "gt-cut",
        key_label: "X",
        letter: true,
        tool: Some(GraphTool::Cut),
        gap: false,
    },
    ToolInfo {
        icon: Icon::SelectBox,
        key: "gt-frame",
        key_label: "N",
        letter: true,
        tool: None,
        gap: false,
    },
    ToolInfo {
        icon: Icon::Fit,
        key: "gt-fit",
        key_label: "Home",
        letter: false,
        tool: None,
        gap: true,
    },
];

/// The tools sit at the middle of the left edge, moved up when the window is too short to clear the Preview island
/// (but never into the top islands).
fn tools_place(ctx: &Context, hud: &Hud<'_>) -> Place {
    let screen = ctx.content_rect();
    let height = hud.ui.island_rects.get("graph-tools").map_or(300.0, egui::Rect::height);
    let highest = screen.top() + MARGIN + TOP_BAR_HEIGHT + 8.0;
    let lowest = preview_island_rect(screen).top() - 8.0 - height;
    let centred = screen.center().y - height / 2.0;
    Place::Fixed(pos2(screen.left() + MARGIN, centred.min(lowest).max(highest)))
}

pub(crate) fn tools(ctx: &Context, hud: &mut Hud<'_>) {
    island(
        ctx,
        hud,
        "graph-tools",
        tools_place(ctx, hud),
        Intro {
            delay: 0.18,
            from: vec2(-16.0, 0.0),
        },
        theme::island(),
        |ui, hud| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.vertical(|ui| {
                for info in &TOOLS {
                    if info.gap {
                        let (rect, _) = ui.allocate_exact_size(vec2(44.0, 7.0), Sense::hover());
                        ui.painter().hline(
                            rect.x_range().shrink(8.0),
                            rect.center().y,
                            Stroke::new(1.0, theme::BORDER),
                        );
                    }
                    let active = info.tool == Some(hud.ui.effects.tool);
                    let look = if active { Look::Accent } else { Look::Plain };
                    let response = widgets::icon_button(ui, info.icon, 44.0, look, true);
                    if info.letter && hud.settings.shortcut_letters {
                        let color = if active { theme::ON_ACCENT } else { theme::MUTED };
                        ui.painter().text(
                            response.rect.right_bottom() - vec2(4.0, 3.0),
                            Align2::RIGHT_BOTTOM,
                            info.key_label,
                            FontId::monospace(9.0),
                            color.gamma_multiply(0.85),
                        );
                    }
                    let name = hud.tr(info.key);
                    let description = hud.tr(&format!("{}-desc", info.key));
                    widgets::side_tip(&response, &name, info.key_label, &description);
                    if response.clicked() {
                        run_tool(ui.ctx(), hud, info);
                    }
                }
            });
        },
    );
}

pub(super) fn run_tool(ctx: &Context, hud: &mut Hud<'_>, info: &ToolInfo) {
    if let Some(tool) = info.tool {
        hud.ui.effects.tool = tool;
        return;
    }
    match info.key {
        "gt-add" => {
            let screen = ctx.content_rect();
            let free = effects::free_area(screen, hud.ui.inspector_open);
            if effects::locked(hud) {
                return;
            }
            hud.ui.effects.add_popup = Some(effects::AddPopup {
                selected: 0,
                at: free.center(),
                query: String::new(),
                fresh: true,
            });
        }
        "gt-frame" => effects::frame_selection(hud),
        _ => {
            let free = effects::free_area(ctx.content_rect(), hud.ui.inspector_open);
            effects::fit(hud, free);
        }
    }
}

/// The preview island: a header with the shape switch; the 3D view is drawn by the viewport behind the hole.
pub(crate) fn preview(ctx: &Context, hud: &mut Hud<'_>) {
    let screen = ctx.content_rect();
    let rect = preview_island_rect(screen);
    let empty =
        hud.ui.effects.preview == PreviewShape::Model && hud.editor.scene.used_voxels(rustium_core::NodeId::ROOT) == 0;
    let hint = hud.tr("preview-empty");
    egui::Area::new(Id::unique("effects-preview"))
        .order(Order::Middle)
        .fixed_pos(rect.min)
        .sense(Sense::click_and_drag())
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let painter = ui.painter();
            let header = Rect::from_min_size(rect.min, vec2(rect.width(), PREVIEW_HEADER));
            let round = f32::from(theme::ISLAND_RADIUS);
            painter.rect_filled(
                header,
                corner(12, 12, 0, 0),
                Color32::from_rgba_unmultiplied(17, 19, 23, 224),
            );
            painter.hline(header.x_range(), header.bottom(), Stroke::new(1.0, theme::BORDER));
            // The viewport shows through the body, which is square: round its bottom corners with patches.
            for (point, x_sign) in [(rect.left_bottom(), 1.0), (rect.right_bottom(), -1.0)] {
                corner_patch(painter, point, round, x_sign);
            }
            painter.rect_stroke(rect, 12, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
            painter.text(
                header.left_center() + vec2(12.0, 0.0),
                Align2::LEFT_CENTER,
                hud.tr("effects-preview"),
                theme::bold(12.0),
                theme::TEXT,
            );
            let options = [
                (PreviewShape::Model, hud.tr("preview-model")),
                (PreviewShape::Cube, hud.tr("preview-cube")),
                (PreviewShape::Ball, hud.tr("preview-ball")),
            ];
            let font = FontId::proportional(12.0);
            let buttons: f32 = options
                .iter()
                .map(|(_, label)| {
                    (painter
                        .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                        + 16.0)
                        .max(22.0)
                })
                .sum();
            // Buttons, the two gaps between them, the control's inner margin and its border.
            let width = buttons + 4.0 + 4.0 + 2.0;
            let area = Rect::from_min_size(
                pos2(header.right() - 4.0 - width, header.top() + 4.0),
                vec2(width, PREVIEW_HEADER - 8.0),
            );
            let mut inner = ui.new_child(
                UiBuilder::new()
                    .max_rect(area)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            widgets::segmented(&mut inner, &mut hud.ui.effects.preview, &options, true);
            if empty {
                let body = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
                let painter = ui.painter_at(body);
                let galley = painter.layout(hint, FontId::proportional(11.0), theme::MUTED, body.width() - 32.0);
                painter.galley(body.center() - galley.size() / 2.0, galley, theme::MUTED);
            }
        });
    // Only the header blurs what is behind it; the body is the live 3D view.
    hud.glass.push(Glass {
        rect: Rect::from_min_size(rect.min, vec2(rect.width(), PREVIEW_HEADER)),
        radius: 0.0,
    });
}

/// Fills the part of the square corner at `corner_point` that lies outside a circle arc of `radius`, so the square
/// 3D view looks rounded. `x_sign` is 1 for a left corner and -1 for a right one.
fn corner_patch(painter: &egui::Painter, corner_point: Pos2, radius: f32, x_sign: f32) {
    let center = corner_point + vec2(x_sign * radius, -radius);
    let mut points = vec![corner_point, corner_point + vec2(0.0, -radius)];
    for step in 0..=8_u8 {
        let angle = f32::from(step) / 8.0 * std::f32::consts::FRAC_PI_2;
        points.push(center + vec2(-x_sign * radius * angle.cos(), radius * angle.sin()));
    }
    points.push(corner_point + vec2(x_sign * radius, 0.0));
    painter.add(Shape::convex_polygon(
        points,
        Color32::from_rgb(0x15, 0x17, 0x1b),
        Stroke::NONE,
    ));
}

/// Light, mid and dark colours of the ball that stands for Material `index` in the hotbar, derived from the graph's
/// Surface constants and the first Palette Color that uses it (so renaming a Material changes nothing).
fn swatch(hud: &Hud<'_>, index: usize, graph: &MaterialGraph) -> [Color32; 3] {
    let surface = graph.surface_constants();
    let used = hud
        .editor
        .scene
        .colors_using(u32::try_from(index).unwrap_or(0))
        .into_iter()
        .find_map(|id| hud.editor.scene.palette().get(id).map(|color| color.color));
    let mut mid = used.unwrap_or([0x9a, 0xa1, 0xab]).map(f32::from);
    let mut light = 1.35_f32;
    let mut dark = 0.45_f32;
    if surface.metallic > 0.5 {
        light = 1.7;
        dark = 0.3;
    }
    if surface.transmission > 0.0 {
        // Glass reads pale and bluish.
        let pale = [0xb8_u8, 0xd4, 0xe0].map(f32::from);
        for (channel, target) in mid.iter_mut().zip(pale) {
            *channel += (target - *channel) * surface.transmission * 0.7;
        }
    }
    if surface.emission_strength > 0.0 {
        // A linked Emission (Voxel color in the samples) glows in the Palette Color, not in the unlinked constant.
        let linked = graph.surface_output().is_some_and(|node| {
            graph
                .link_to(InputRef {
                    node: node.id,
                    socket: 4,
                })
                .is_some()
        });
        if !linked {
            mid = surface.emission_color.map(f32::from);
        }
        light = 1.25;
        dark = 0.75;
    }
    let scale = |factor: f32| {
        mid.map(|channel| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the result is clamped to a byte"
            )]
            let value = (channel * factor).clamp(0.0, 255.0) as u8;
            value
        })
    };
    [rgb(scale(light)), rgb(scale(1.0)), rgb(scale(dark))]
}

const HOTBAR_VISIBLE: usize = 12;

pub(crate) fn hotbar(ctx: &Context, hud: &mut Hud<'_>) {
    let budget = centre_budget(ctx, hud, "effects-hints", "effects-readout");
    let count = hud.editor.scene.material_graphs().len();
    let key = fit_key(&[
        &hud.settings.language,
        &hud.settings.ui_scale_percent,
        &count,
        &hud.ui.effects.graph,
    ]);
    // 0: everything. 1: no Material info. 2: at most six Materials. 3: at most three. 4: only the first.
    let level = fit_level(ctx, hud, "materials-hotbar", budget, key, HOTBAR_MAX_LEVEL);
    let visible = match level {
        0 | 1 => HOTBAR_VISIBLE,
        2 => 6,
        3 => 3,
        _ => 1,
    };
    island(
        ctx,
        hud,
        "materials-hotbar",
        Place::Anchor(Align2::CENTER_BOTTOM, vec2(0.0, -MARGIN)),
        Intro {
            delay: 0.3,
            from: vec2(0.0, 16.0),
        },
        theme::island().inner_margin(egui::Margin::symmetric(8, 6)),
        |ui, hud| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if level == 0 {
                    info_block(ui, hud);
                    widgets::divider(ui, 38.0);
                }
                for index in 0..count.min(visible) {
                    material_button(ui, hud, index);
                }
                if count > visible {
                    let more = widgets::text_button(ui, &format!("+{}", count - visible), 38.0, Look::Plain, true);
                    egui::Popup::menu(&more).show(|ui| {
                        for index in visible..count {
                            let name = hud
                                .editor
                                .scene
                                .material_graphs()
                                .get(index)
                                .map_or_else(String::new, |graph| effects::material_name(hud, index, graph));
                            if ui.selectable_label(index == hud.ui.effects.graph, name).clicked() {
                                hud.ui.effects.select_graph(index);
                            }
                        }
                    });
                }
                widgets::divider(ui, 38.0);
                new_button(ui, hud);
            });
        },
    );
}

fn info_block(ui: &mut Ui, hud: &Hud<'_>) {
    let Some(graph) = effects::current(hud) else {
        return;
    };
    let name = effects::material_name(hud, hud.ui.effects.graph, graph);
    ui.allocate_ui(vec2(112.0, 38.0), |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(egui::RichText::new(name).font(theme::bold(12.0)).color(theme::TEXT)).truncate());
            let info = hud.i18n.tr_args(
                "effects-material-info",
                &[(
                    "count",
                    FluentValue::from(i64::try_from(graph.nodes().len()).unwrap_or(0)),
                )],
            );
            ui.label(widgets::mono(info, 10.0).color(theme::MUTED));
        });
    });
}

fn material_button(ui: &mut Ui, hud: &mut Hud<'_>, index: usize) {
    let Some(graph) = hud.editor.scene.material_graphs().get(index).cloned() else {
        return;
    };
    let selected = index == hud.ui.effects.graph;
    let lift = ui
        .ctx()
        .animate_bool_with_time(Id::unique(("material-lift", index)), selected, 0.18);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    let rect = rect.translate(vec2(0.0, -6.0 * lift));
    let [light, mid, dark] = swatch(hud, index, &graph);
    let painter = ui.painter();
    if selected {
        painter.circle_filled(rect.center() + vec2(0.0, 8.0), 19.0, Color32::from_black_alpha(90));
        painter.circle_filled(rect.center(), 21.0, Color32::WHITE);
    }
    // A ball: three concentric discs approximate the radial gradient of the mock-up.
    let center = rect.center();
    painter.circle_filled(center, 19.0, dark);
    painter.circle_filled(center - vec2(1.5, 1.5), 15.0, mid);
    painter.circle_filled(center - vec2(5.0, 5.5), 8.0, light);
    painter.circle_stroke(center, 19.0, Stroke::new(1.0, Color32::from_black_alpha(150)));
    let number = ((index + 1) % 10).to_string();
    let corner = rect.left_top() + vec2(1.0, 1.0);
    painter.circle_filled(corner + vec2(4.0, 5.0), 7.0, Color32::from_black_alpha(170));
    painter.text(
        corner + vec2(4.0, 5.0),
        Align2::CENTER_CENTER,
        number,
        FontId::monospace(9.0),
        Color32::WHITE,
    );
    let label = effects::material_name(hud, index, &graph);
    let response = response.on_hover_text(label);
    if response.clicked() {
        hud.ui.effects.select_graph(index);
    }
    response.context_menu(|ui| {
        if ui.button(hud.tr("effects-duplicate")).clicked() {
            let index = u32::try_from(index).unwrap_or(0);
            if effects::command(hud, &effects::duplicate_command(hud, index)) {
                let last = hud.editor.scene.material_graphs().len().saturating_sub(1);
                hud.ui.effects.select_graph(last);
            }
            ui.close();
        }
        let deletable = index != 0;
        if ui
            .add_enabled(deletable, egui::Button::new(hud.tr("material-delete")))
            .clicked()
        {
            delete_material(hud, index);
            ui.close();
        }
    });
}

/// Deletes Material `index` (never the Default) and selects its neighbour.
pub(crate) fn delete_material(hud: &mut Hud<'_>, index: usize) {
    if index == 0 {
        return;
    }
    let raw = u32::try_from(index).unwrap_or(0);
    if effects::command(hud, &Command::DeleteMaterial { index: raw }) {
        hud.ui.effects.select_graph(index.saturating_sub(1));
        hud.ui.effects.fitted = None;
    }
}

fn new_button(ui: &mut Ui, hud: &mut Hud<'_>) {
    let label = hud.tr("effects-new");
    let galley = ui
        .painter()
        .layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 44.0, 38.0), Sense::click());
    let fill = if response.hovered() {
        theme::HOVER
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if response.hovered() {
        Color32::WHITE
    } else {
        theme::MUTED
    };
    ui.painter().rect_filled(rect, 7, fill);
    Icon::Plus.paint(
        ui.painter(),
        Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), Vec2::splat(18.0)),
        foreground,
    );
    ui.painter().galley_with_override_text_color(
        rect.left_center() + vec2(32.0, -galley.size().y / 2.0),
        galley,
        foreground,
    );
    let full = hud.editor.scene.material_graphs().len() >= MaterialGraph::MAX_PER_SCENE;
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(180.0);
        if ui
            .add_enabled(!full, egui::Button::new(hud.tr("effects-new-blank")))
            .clicked()
        {
            add_material(hud, MaterialGraph::blank(&hud.i18n.tr("effects-new-name")));
            ui.close();
        }
        ui.separator();
        ui.label(widgets::muted(hud.tr("effects-new-sample")).size(11.0));
        for sample in SampleMaterial::ALL {
            let label = effects::core_text(hud, sample.label());
            if ui.add_enabled(!full, egui::Button::new(&label)).clicked() {
                let mut graph = sample.build();
                graph.name = label;
                translate_frames(hud, &mut graph);
                add_material(hud, graph);
                ui.close();
            }
        }
    });
}

/// Gives the frames of a freshly built sample their translated labels.
fn translate_frames(hud: &Hud<'_>, graph: &mut MaterialGraph) {
    let edits: Vec<GraphEdit> = graph
        .frames()
        .iter()
        .map(|frame| GraphEdit::SetFrame {
            frame: frame.id,
            label: effects::core_text(hud, &frame.label),
            position: frame.position,
            size: frame.size,
        })
        .collect();
    for edit in edits {
        // A label that cannot be applied keeps the English text.
        let _ = graph.apply_edit(&edit);
    }
}

/// Adds a Material to the Scene and starts editing it.
pub(crate) fn add_material(hud: &mut Hud<'_>, graph: MaterialGraph) {
    if effects::command(hud, &Command::AddMaterial { graph: Box::new(graph) }) {
        let last = hud.editor.scene.material_graphs().len().saturating_sub(1);
        hud.ui.effects.select_graph(last);
        hud.ui.effects.tab = EffectsTab::Material;
    }
}

pub(crate) fn corners(ctx: &Context, hud: &mut Hud<'_>) {
    let budget = corner_budget(ctx);
    let key = fit_key(&[&hud.settings.language, &hud.settings.ui_scale_percent]);
    let hints_level = fit_level(ctx, hud, "effects-hints", budget, key, HINTS_MAX_LEVEL);
    let hints = theme::island()
        .corner_radius(corner(0, 12, 0, 0))
        .inner_margin(egui::Margin {
            left: 12,
            right: 14,
            top: 0,
            bottom: 0,
        });
    island(
        ctx,
        hud,
        "effects-hints",
        Place::Anchor(Align2::LEFT_BOTTOM, vec2(0.0, 0.0)),
        Intro {
            delay: 0.36,
            from: vec2(0.0, 16.0),
        },
        hints,
        |ui, hud| {
            ui.set_height(34.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let shown = match hints_level {
                    0 => 3,
                    1 => 2,
                    _ => 1,
                };
                for (button, key) in [
                    ("Tab", "hint-add-node"),
                    ("LMB", "hint-connect"),
                    ("RMB", "hint-tool-wheel"),
                    ("MMB", "hint-pan-graph"),
                ]
                .into_iter()
                .take(shown)
                {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(widgets::mono(button, 11.0).color(theme::TEXT));
                        ui.label(egui::RichText::new(hud.tr(key)).color(theme::MUTED).size(11.0));
                    });
                }
            });
        },
    );

    let readout = theme::island()
        .corner_radius(corner(12, 0, 0, 0))
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 0,
            bottom: 0,
        });
    island(
        ctx,
        hud,
        "effects-readout",
        Place::Anchor(Align2::RIGHT_BOTTOM, vec2(0.0, 0.0)),
        Intro {
            delay: 0.42,
            from: vec2(0.0, 16.0),
        },
        readout,
        |ui, hud| {
            ui.set_height(34.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                status_chip(ui, hud);
                let (nodes, links) =
                    effects::current(hud).map_or((0, 0), |graph| (graph.nodes().len(), graph.links().len()));
                let nodes = hud.i18n.tr_args(
                    "effects-nodes-count",
                    &[("count", FluentValue::from(i64::try_from(nodes).unwrap_or(0)))],
                );
                let links = hud.i18n.tr_args(
                    "effects-links-count",
                    &[("count", FluentValue::from(i64::try_from(links).unwrap_or(0)))],
                );
                ui.label(widgets::mono(nodes, 11.0).color(theme::SOFT));
                ui.label(widgets::mono(links, 11.0).color(theme::SOFT));
            });
        },
    );
}

fn status_chip(ui: &mut Ui, hud: &Hud<'_>) {
    let (key, color, detail) = match hud.material_status {
        CompileStatus::Compiled => ("status-compiled", Color32::from_rgb(0x6f, 0xb3, 0x6a), None),
        CompileStatus::Compiling => ("status-compiling", Color32::from_rgb(0xe6, 0xc3, 0x5c), None),
        CompileStatus::Error(message) => (
            "status-error",
            Color32::from_rgb(0xe0, 0x5a, 0x4a),
            Some(message.clone()),
        ),
    };
    let text = hud.tr(key);
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, color);
            ui.label(egui::RichText::new(text).color(theme::TEXT).size(12.0))
        })
        .response;
    if let Some(message) = detail {
        response.on_hover_text(message);
    }
}
