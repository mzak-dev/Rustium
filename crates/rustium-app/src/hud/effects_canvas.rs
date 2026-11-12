//! The node canvas of Effects mode: layout of nodes, painting (dotted grid, frames, wires, nodes) and the pointer
//! gestures (pan, zoom, select, move, connect, cut, frame, scrub).
//!
//! The canvas covers the whole window. Painting is done with plain `egui` shapes, one pass per band around the
//! preview island, because the preview island is a hole through which the 3D viewport shows.

use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Key, PointerButton, Pos2, Rect, Response, Shape, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};
use rustium_core::{
    Frame, GraphEdit, GraphNode, GraphNodeId, GraphNodeKind, InputRef, InputValue, MaterialGraph, OutputRef, ParamKind,
    ParamValue, RampStop,
};

use super::Hud;
use super::effects::{
    self, EffectsTab, Gesture, GraphTool, WireEnd, core_text, graph_edit, kind_label, locked, rgb, snap, unit_color,
    wire_color,
};
use crate::cursor::{CanvasDrag, CanvasHit, EffectsCtx, PointerInput};
use crate::theme;

pub(crate) const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.0;
const NODE_WIDTH: f32 = 184.0;
const OUTPUT_WIDTH: f32 = 190.0;
const HEADER: f32 = 30.0;
const ROW: f32 = 26.0;
const BOTTOM: f32 = 6.0;
const FRAME_HEADER: f32 = 26.0;
const SOCKET_RADIUS: f32 = 6.0;

/// Which value of a node a widget changes: an unlinked input or a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Input(u8),
    Param(u8),
}

/// What a row of a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowKind {
    Output(u8),
    Input(u8),
    Param(u8),
}

/// One row of a node, in graph space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Row {
    pub(crate) kind: RowKind,
    pub(crate) rect: Rect,
}

/// Where a node and its rows are, in graph space.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Geo {
    pub(crate) rect: Rect,
    pub(crate) rows: Vec<Row>,
}

/// How a widget on a node reacts.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Widget {
    /// A number scrubbed by dragging.
    Scrub {
        min: f32,
        max: f32,
        whole: bool,
    },
    /// A pair of numbers; the pointer's half of the row picks the end.
    Pair {
        min: f32,
        max: f32,
    },
    Toggle,
    Cycle,
    Color,
}

/// Lays out one node: header, then output rows, input rows and parameter rows.
pub(crate) fn geometry(node: &GraphNode) -> Geo {
    let spec = node.kind.spec();
    let width = if node.kind == GraphNodeKind::SurfaceOutput {
        OUTPUT_WIDTH
    } else {
        NODE_WIDTH
    };
    let left = node.position[0];
    let mut top = node.position[1] + HEADER;
    let mut rows = Vec::new();
    let mut push = |kind: RowKind, top: &mut f32| {
        rows.push(Row {
            kind,
            rect: Rect::from_min_size(pos2(left, *top), vec2(width, ROW)),
        });
        *top += ROW;
    };
    for index in 0..spec.outputs.len() {
        push(RowKind::Output(index_u8(index)), &mut top);
    }
    for index in 0..spec.inputs.len() {
        push(RowKind::Input(index_u8(index)), &mut top);
    }
    for (index, param) in spec.params.iter().enumerate() {
        if param.inline {
            push(RowKind::Param(index_u8(index)), &mut top);
        }
    }
    Geo {
        rect: Rect::from_min_max(pos2(left, node.position[1]), pos2(left + width, top + BOTTOM)),
        rows,
    }
}

fn index_u8(index: usize) -> u8 {
    u8::try_from(index).unwrap_or(u8::MAX)
}

fn socket_point(geo: &Geo, row: &Row) -> Pos2 {
    match row.kind {
        RowKind::Output(_) => pos2(geo.rect.right(), row.rect.center().y),
        _ => pos2(geo.rect.left(), row.rect.center().y),
    }
}

fn output_point(node: &GraphNode, socket: u8) -> Option<Pos2> {
    let geo = geometry(node);
    geo.rows
        .iter()
        .find(|row| row.kind == RowKind::Output(socket))
        .map(|row| socket_point(&geo, row))
}

fn input_point(node: &GraphNode, socket: u8) -> Option<Pos2> {
    let geo = geometry(node);
    geo.rows
        .iter()
        .find(|row| row.kind == RowKind::Input(socket))
        .map(|row| socket_point(&geo, row))
}

fn widget_rect(row: &Row) -> Rect {
    row.rect.shrink2(vec2(10.0, 3.0))
}

/// The widget a row carries, if the row has one: constants of unlinked inputs and all parameters.
fn widget_of(node: &GraphNode, graph: &MaterialGraph, row: &Row) -> Option<(Target, Widget)> {
    let spec = node.kind.spec();
    match row.kind {
        RowKind::Input(index) => {
            let input = spec.inputs.get(usize::from(index))?;
            if graph
                .link_to(InputRef {
                    node: node.id,
                    socket: index,
                })
                .is_some()
            {
                return None;
            }
            match node.inputs.get(usize::from(index))? {
                InputValue::Float(_) => Some((
                    Target::Input(index),
                    Widget::Scrub {
                        min: input.range.0,
                        max: input.range.1,
                        whole: false,
                    },
                )),
                InputValue::Color(_) => Some((Target::Input(index), Widget::Color)),
                _ => None,
            }
        }
        RowKind::Param(index) => {
            let param = spec.params.get(usize::from(index))?;
            let widget = match param.kind {
                ParamKind::Float { min, max, .. } => Widget::Scrub { min, max, whole: false },
                #[expect(clippy::cast_precision_loss, reason = "parameter limits are small whole numbers")]
                ParamKind::Int { min, max, .. } => Widget::Scrub {
                    min: min as f32,
                    max: max as f32,
                    whole: true,
                },
                ParamKind::Bool { .. } => Widget::Toggle,
                ParamKind::Choice { .. } => Widget::Cycle,
                ParamKind::Range { min, max, .. } => Widget::Pair { min, max },
                ParamKind::Color { .. } => Widget::Color,
                ParamKind::Stops => return None,
            };
            Some((Target::Param(index), widget))
        }
        RowKind::Output(_) => None,
    }
}

// ---- View --------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct View {
    pan: Vec2,
    zoom: f32,
}

impl View {
    fn of(hud: &Hud<'_>) -> Self {
        Self {
            pan: hud.ui.effects.pan,
            zoom: hud.ui.effects.zoom,
        }
    }

    fn point(self, graph: Pos2) -> Pos2 {
        pos2(graph.x * self.zoom + self.pan.x, graph.y * self.zoom + self.pan.y)
    }

    fn rect(self, graph: Rect) -> Rect {
        Rect::from_min_max(self.point(graph.min), self.point(graph.max))
    }

    fn back(self, screen: Pos2) -> Pos2 {
        pos2((screen.x - self.pan.x) / self.zoom, (screen.y - self.pan.y) / self.zoom)
    }
}

fn radius(value: f32) -> CornerRadius {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "corner radii are small positive numbers"
    )]
    CornerRadius::same(value.round().clamp(0.0, 255.0) as u8)
}

fn frame_rect(frame: &Frame) -> Rect {
    Rect::from_min_size(
        pos2(frame.position[0], frame.position[1]),
        vec2(frame.size[0], frame.size[1]),
    )
}

/// Where the label of `frame` sits on screen (for the label editor).
pub(crate) fn frame_label_position(hud: &Hud<'_>, frame: &Frame) -> Pos2 {
    View::of(hud).point(pos2(frame.position[0] + 10.0, frame.position[1] + 3.0))
}

/// A wire as points: a horizontal-tangent cubic curve from an output to an input, in screen points.
fn wire_points(from: Pos2, to: Pos2, zoom: f32) -> Vec<Pos2> {
    let reach = ((to.x - from.x).abs() * 0.5).max(40.0 * zoom);
    let control_a = from + vec2(reach, 0.0);
    let control_b = to - vec2(reach, 0.0);
    (0..=24_u8)
        .map(|step| {
            let t = f32::from(step) / 24.0;
            let u = 1.0 - t;
            let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let x = weights[0] * from.x + weights[1] * control_a.x + weights[2] * control_b.x + weights[3] * to.x;
            let y = weights[0] * from.y + weights[1] * control_a.y + weights[2] * control_b.y + weights[3] * to.y;
            pos2(x, y)
        })
        .collect()
}

// ---- Painting ----------------------------------------------------------------------------------------------

const CANVAS: Color32 = Color32::from_rgb(0x15, 0x17, 0x1b);

/// The four rectangles of `outer` that surround `hole`.
fn bands(outer: Rect, hole: Rect) -> [Rect; 4] {
    let hole = hole.intersect(outer);
    [
        Rect::from_min_max(outer.min, pos2(outer.max.x, hole.min.y)),
        Rect::from_min_max(pos2(outer.min.x, hole.max.y), outer.max),
        Rect::from_min_max(pos2(outer.min.x, hole.min.y), pos2(hole.min.x, hole.max.y)),
        Rect::from_min_max(pos2(hole.max.x, hole.min.y), pos2(outer.max.x, hole.max.y)),
    ]
}

/// Paints the whole canvas except `hole`, where the 3D preview shows through.
pub(crate) fn paint(ui: &Ui, hud: &Hud<'_>, canvas: Rect, hole: Rect) {
    let bands = bands(canvas, hole);
    let painter = ui.painter();
    for band in bands {
        if band.width() > 0.0 && band.height() > 0.0 {
            painter.rect_filled(band, 0, CANVAS);
        }
    }
    let view = View::of(hud);
    // Dotted grid: one dot per 24 graph units, thinned out when zoomed far away.
    let mut step = 24.0 * view.zoom;
    while step < 14.0 {
        step *= 2.0;
    }
    let dot = Color32::from_white_alpha(26);
    let start_x = canvas.left() + (view.pan.x - canvas.left()).rem_euclid(step);
    let start_y = canvas.top() + (view.pan.y - canvas.top()).rem_euclid(step);
    let mut y = start_y;
    while y < canvas.bottom() {
        let mut x = start_x;
        while x < canvas.right() {
            let point = pos2(x, y);
            if !hole.contains(point) {
                painter.circle_filled(point, 1.0, dot);
            }
            x += step;
        }
        y += step;
    }
    let pointer = ui.input(|input| input.pointer.hover_pos());
    for band in bands {
        if band.width() > 0.0 && band.height() > 0.0 {
            let clipped = painter.with_clip_rect(band);
            paint_graph(&clipped, hud, pointer);
        }
    }
}

fn paint_graph(painter: &egui::Painter, hud: &Hud<'_>, pointer: Option<Pos2>) {
    let Some(graph) = effects::current(hud) else {
        return;
    };
    let view = View::of(hud);
    let accent = hud.accent();
    let state = &hud.ui.effects;
    for frame in graph.frames() {
        paint_frame(
            painter,
            hud,
            view,
            frame,
            state.frame_selection == Some(frame.id),
            accent,
        );
    }
    for link in graph.links() {
        let (Some(from_node), Some(to_node)) = (graph.node(link.from.node), graph.node(link.to.node)) else {
            continue;
        };
        let (Some(from), Some(to)) = (
            output_point(from_node, link.from.socket),
            input_point(to_node, link.to.socket),
        ) else {
            continue;
        };
        let spec = from_node.kind.spec();
        let color = spec
            .outputs
            .get(usize::from(link.from.socket))
            .map_or(Color32::GRAY, |output| wire_color(output.ty));
        let bypassed = from_node.bypass || to_node.bypass;
        let color = if bypassed { color.gamma_multiply(0.45) } else { color };
        let points = wire_points(view.point(from), view.point(to), view.zoom);
        painter.add(Shape::line(
            points.clone(),
            Stroke::new((2.0 * view.zoom).max(1.2), color),
        ));
        // The links of the selected nodes carry flowing dashes, in the direction the values travel.
        if !bypassed && (state.selection.contains(&link.from.node) || state.selection.contains(&link.to.node)) {
            let time = super::seconds_between(0.0, painter.ctx().input(|input| input.time));
            let dash = 6.0 * view.zoom;
            let flow = Stroke::new((2.0 * view.zoom).max(1.2), Color32::from_white_alpha(170));
            painter.extend(Shape::dashed_line_with_offset(
                &points,
                flow,
                &[dash],
                &[dash * 2.0],
                (-time * 36.0 * view.zoom).rem_euclid(dash * 3.0),
            ));
            painter.ctx().request_repaint();
        }
    }
    for node in graph.nodes() {
        paint_node(painter, hud, graph, view, node, pointer);
    }
    overlay(painter, hud, graph, view, pointer, accent);
}

fn paint_frame(painter: &egui::Painter, hud: &Hud<'_>, view: View, frame: &Frame, selected: bool, accent: Color32) {
    let rect = view.rect(frame_rect(frame));
    painter.rect_filled(rect, radius(12.0 * view.zoom), Color32::from_white_alpha(6));
    let stroke = if selected {
        Stroke::new(1.5, accent)
    } else {
        Stroke::new(1.0, Color32::from_white_alpha(41))
    };
    // A dashed border, as in the mock-up.
    dashed_rect(painter, rect, stroke, 6.0);
    let editing = hud
        .ui
        .effects
        .frame_label
        .as_ref()
        .is_some_and(|(id, ..)| *id == frame.id);
    if !editing {
        painter.text(
            view.point(pos2(frame.position[0] + 12.0, frame.position[1] + 8.0)),
            Align2::LEFT_TOP,
            &frame.label,
            FontId::proportional((11.0 * view.zoom).max(6.0)),
            theme::MUTED,
        );
    }
    let handle = Rect::from_center_size(rect.right_bottom() - vec2(8.0, 8.0), Vec2::splat(10.0));
    painter.line_segment(
        [
            handle.right_top() + vec2(0.0, 2.0),
            handle.left_bottom() + vec2(2.0, 0.0),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(60)),
    );
}

fn dashed_rect(painter: &egui::Painter, rect: Rect, stroke: Stroke, dash: f32) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    for pair in corners.windows(2) {
        painter.extend(Shape::dashed_line(pair, stroke, dash, dash * 0.8));
    }
}

fn paint_node(
    painter: &egui::Painter,
    hud: &Hud<'_>,
    graph: &MaterialGraph,
    view: View,
    node: &GraphNode,
    pointer: Option<Pos2>,
) {
    let spec = node.kind.spec();
    let geo = geometry(node);
    let rect = view.rect(geo.rect);
    let zoom = view.zoom;
    let selected = hud.ui.effects.selection.contains(&node.id);
    let accent = hud.accent();
    let fade = |color: Color32| if node.bypass { color.gamma_multiply(0.5) } else { color };
    let round = radius(10.0 * zoom);

    painter.rect_filled(
        rect.translate(vec2(0.0, 8.0 * zoom)).expand(-2.0 * zoom),
        round,
        Color32::from_black_alpha(70),
    );
    painter.rect_filled(rect, round, fade(Color32::from_rgba_unmultiplied(22, 25, 30, 245)));
    let border = if selected {
        Stroke::new(1.5, accent)
    } else {
        Stroke::new(1.0, Color32::from_white_alpha(36))
    };
    painter.rect_stroke(rect, round, border, StrokeKind::Inside);

    // Header.
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER * zoom));
    if node.kind == GraphNodeKind::SurfaceOutput {
        painter.rect_filled(
            header,
            CornerRadius {
                nw: radius(9.0 * zoom).nw,
                ne: radius(9.0 * zoom).nw,
                sw: 0,
                se: 0,
            },
            accent.gamma_multiply(0.22),
        );
    }
    painter.hline(
        header.x_range(),
        header.bottom(),
        Stroke::new(1.0, Color32::from_white_alpha(20)),
    );
    let dot_size = 8.0 * zoom;
    let dot_color = if node.kind == GraphNodeKind::SurfaceOutput {
        accent
    } else {
        rgb(spec.category.dot_color())
    };
    painter.rect_filled(
        Rect::from_center_size(header.left_center() + vec2(14.0 * zoom, 0.0), Vec2::splat(dot_size)),
        radius(2.0 * zoom),
        fade(dot_color),
    );
    let title = if node.name.is_empty() {
        kind_label(hud, node.kind)
    } else {
        node.name.clone()
    };
    let title_end = header.left()
        + 25.0 * zoom
        + painter
            .layout_no_wrap(title.clone(), text_font(12.0, zoom, true), Color32::WHITE)
            .size()
            .x;
    painter.text(
        header.left_center() + vec2(25.0 * zoom, 0.0),
        Align2::LEFT_CENTER,
        title,
        text_font(12.0, zoom, true),
        fade(theme::TEXT),
    );
    let tag = if node.bypass {
        hud.tr("effects-bypass")
    } else {
        effects::category_label(hud, spec.category)
    };
    let tag_color = if node.bypass { accent } else { theme::SUBTLE };
    let tag_start = header.right()
        - 10.0 * zoom
        - painter
            .layout_no_wrap(tag.clone(), text_font(10.0, zoom, false), Color32::WHITE)
            .size()
            .x;
    // A long title keeps the header to itself: the category is already shown by the dot's color.
    let tag = if tag_start - title_end < 8.0 * zoom && !node.bypass {
        String::new()
    } else {
        tag
    };
    painter.text(
        header.right_center() - vec2(10.0 * zoom, 0.0),
        Align2::RIGHT_CENTER,
        tag,
        text_font(10.0, zoom, false),
        tag_color,
    );

    for row in &geo.rows {
        paint_row(painter, hud, graph, view, node, &geo, row, pointer, &fade);
    }
}

fn text_font(size: f32, zoom: f32, bold: bool) -> FontId {
    let size = (size * zoom).max(5.0);
    if bold {
        theme::bold(size)
    } else {
        FontId::proportional(size)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "paints one row from the shared drawing context"
)]
fn paint_row(
    painter: &egui::Painter,
    hud: &Hud<'_>,
    graph: &MaterialGraph,
    view: View,
    node: &GraphNode,
    geo: &Geo,
    row: &Row,
    pointer: Option<Pos2>,
    fade: &dyn Fn(Color32) -> Color32,
) {
    let spec = node.kind.spec();
    let zoom = view.zoom;
    let rect = view.rect(row.rect);
    let text_color = fade(theme::SOFT);
    let font = text_font(11.0, zoom, false);
    let mono = FontId::monospace((10.5 * zoom).max(5.0));
    let socket_color = match row.kind {
        RowKind::Output(index) => spec.outputs.get(usize::from(index)).map(|output| wire_color(output.ty)),
        RowKind::Input(index) => spec.inputs.get(usize::from(index)).map(|input| wire_color(input.ty)),
        RowKind::Param(_) => None,
    };
    if let Some(color) = socket_color {
        let center = view.point(socket_point(geo, row));
        let hovered = pointer.is_some_and(|p| p.distance(center) < SOCKET_RADIUS * zoom * 1.6 + 3.0);
        let size = SOCKET_RADIUS * zoom * if hovered { 1.35 } else { 1.0 };
        painter.circle(
            center,
            size,
            fade(color),
            Stroke::new((2.0 * zoom).max(1.0), Color32::from_rgb(0x16, 0x19, 0x1e)),
        );
    }
    match row.kind {
        RowKind::Output(index) => {
            let name = spec.outputs.get(usize::from(index)).map_or("", |output| output.name);
            painter.text(
                rect.right_center() - vec2(10.0 * zoom, 0.0),
                Align2::RIGHT_CENTER,
                core_text(hud, name),
                font,
                text_color,
            );
        }
        RowKind::Input(index) => {
            let name = spec.inputs.get(usize::from(index)).map_or("", |input| input.name);
            let label = core_text(hud, name);
            match widget_of(node, graph, row) {
                Some((target, widget)) => {
                    let field = view.rect(widget_rect(row));
                    painter.rect_filled(field, radius(5.0 * zoom), Color32::from_white_alpha(18));
                    painter.text(
                        field.left_center() + vec2(6.0 * zoom, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        font,
                        text_color,
                    );
                    paint_value(painter, hud, node, target, widget, field, &mono, zoom);
                }
                None => {
                    painter.text(
                        rect.left_center() + vec2(10.0 * zoom, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        font,
                        text_color,
                    );
                }
            }
        }
        RowKind::Param(index) => {
            let Some(param) = spec.params.get(usize::from(index)) else {
                return;
            };
            let field = view.rect(widget_rect(row));
            painter.rect_filled(field, radius(5.0 * zoom), Color32::from_white_alpha(18));
            painter.text(
                field.left_center() + vec2(6.0 * zoom, 0.0),
                Align2::LEFT_CENTER,
                core_text(hud, param.name),
                font,
                text_color,
            );
            match widget_of(node, graph, row) {
                Some((target, widget)) => paint_value(painter, hud, node, target, widget, field, &mono, zoom),
                None => {
                    if let Some(ParamValue::Stops(stops)) = node.params.get(usize::from(index)) {
                        let bar = Rect::from_min_max(
                            pos2(field.left() + field.width() * 0.4, field.top() + 3.0 * zoom),
                            pos2(field.right() - 4.0 * zoom, field.bottom() - 3.0 * zoom),
                        );
                        paint_ramp(painter, bar, stops, zoom);
                    }
                }
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "paints one value from the shared drawing context"
)]
fn paint_value(
    painter: &egui::Painter,
    hud: &Hud<'_>,
    node: &GraphNode,
    target: Target,
    widget: Widget,
    field: Rect,
    mono: &FontId,
    zoom: f32,
) {
    let right = field.right_center() - vec2(6.0 * zoom, 0.0);
    let value_text = |text: String| {
        painter.text(right, Align2::RIGHT_CENTER, text, mono.clone(), theme::TEXT);
    };
    let swatch = |color: [f32; 3]| {
        let size = vec2(34.0 * zoom, 12.0 * zoom);
        let rect = Rect::from_center_size(right - vec2(size.x / 2.0, 0.0), size);
        painter.rect(
            rect,
            radius(3.0 * zoom),
            unit_color(color),
            Stroke::new(1.0, Color32::from_white_alpha(77)),
            StrokeKind::Inside,
        );
    };
    match (target, widget) {
        (Target::Input(index), Widget::Scrub { .. }) => {
            if let Some(InputValue::Float(value)) = node.inputs.get(usize::from(index)) {
                value_text(format!("{value:.2}"));
            }
        }
        (Target::Input(index), Widget::Color) => {
            if let Some(InputValue::Color(color)) = node.inputs.get(usize::from(index)) {
                swatch(*color);
            }
        }
        (Target::Param(index), widget) => {
            let spec = node.kind.spec().params.get(usize::from(index));
            match (node.params.get(usize::from(index)), widget) {
                (Some(ParamValue::Float(value)), _) => value_text(format!("{value:.2}")),
                (Some(ParamValue::Int(value)), _) => value_text(value.to_string()),
                (Some(ParamValue::Bool(on)), _) => {
                    let track = Rect::from_center_size(right - vec2(11.0 * zoom, 0.0), vec2(22.0, 12.0) * zoom);
                    let accent = hud.accent();
                    painter.rect(
                        track,
                        radius(6.0 * zoom),
                        if *on { accent } else { Color32::from_black_alpha(90) },
                        Stroke::new(
                            1.0,
                            if *on {
                                accent
                            } else {
                                Color32::from_rgb(0x5e, 0x68, 0x75)
                            },
                        ),
                        StrokeKind::Inside,
                    );
                    let knob_x = if *on {
                        track.right() - 6.0 * zoom
                    } else {
                        track.left() + 6.0 * zoom
                    };
                    painter.circle_filled(
                        pos2(knob_x, track.center().y),
                        4.0 * zoom,
                        if *on { theme::ON_ACCENT } else { theme::MUTED },
                    );
                }
                (Some(ParamValue::Choice(choice)), _) => {
                    if let Some(ParamKind::Choice { options, .. }) = spec.map(|spec| spec.kind) {
                        let name = options
                            .get(usize::try_from(*choice).unwrap_or(0))
                            .copied()
                            .unwrap_or("");
                        value_text(core_text(hud, name));
                    }
                }
                (Some(ParamValue::Range([low, high])), _) => value_text(format!("{low:.2} – {high:.2}")),
                (Some(ParamValue::Color(color)), _) => swatch(*color),
                _ => {}
            }
        }
        _ => {}
    }
}

/// Color of a ramp at `t`, interpolating between its stops in sRGB.
pub(crate) fn ramp_color(stops: &[RampStop], t: f32) -> Color32 {
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return Color32::BLACK;
    };
    if t <= first.position {
        return unit_color(first.color);
    }
    for pair in stops.windows(2) {
        if t <= pair[1].position {
            let span = (pair[1].position - pair[0].position).max(1.0e-6);
            let mix = ((t - pair[0].position) / span).clamp(0.0, 1.0);
            let color = [0, 1, 2].map(|c| pair[0].color[c] + (pair[1].color[c] - pair[0].color[c]) * mix);
            return unit_color(color);
        }
    }
    unit_color(last.color)
}

/// Draws a color ramp as a strip of thin slices.
pub(crate) fn paint_ramp(painter: &egui::Painter, bar: Rect, stops: &[RampStop], zoom: f32) {
    let slices = 28_u8;
    for slice in 0..slices {
        let a = f32::from(slice) / f32::from(slices);
        let b = f32::from(slice + 1) / f32::from(slices);
        let slice_rect = Rect::from_min_max(
            pos2(bar.left() + bar.width() * a, bar.top()),
            pos2(bar.left() + bar.width() * b + 0.5, bar.bottom()),
        );
        painter.rect_filled(slice_rect, 0, ramp_color(stops, f32::midpoint(a, b)));
    }
    painter.rect_stroke(
        bar,
        radius(3.0 * zoom),
        Stroke::new(1.0, Color32::from_white_alpha(60)),
        StrokeKind::Inside,
    );
}

fn overlay(
    painter: &egui::Painter,
    hud: &Hud<'_>,
    graph: &MaterialGraph,
    view: View,
    pointer: Option<Pos2>,
    accent: Color32,
) {
    let Some(gesture) = &hud.ui.effects.gesture else {
        return;
    };
    match gesture {
        Gesture::Box { start, .. } => {
            if let Some(pointer) = pointer {
                let rect = Rect::from_two_pos(*start, pointer);
                painter.rect(
                    rect,
                    2,
                    accent.gamma_multiply(0.12),
                    Stroke::new(1.0, accent),
                    StrokeKind::Inside,
                );
            }
        }
        Gesture::Cut { points } => {
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(1.5, Color32::from_rgb(0xff, 0x6b, 0x5b)),
            ));
        }
        Gesture::Wire { from } => {
            let Some(pointer) = pointer else {
                return;
            };
            let (anchor, ty, reverse) = match from {
                WireEnd::Output(output) => {
                    let Some(node) = graph.node(output.node) else {
                        return;
                    };
                    let Some(point) = output_point(node, output.socket) else {
                        return;
                    };
                    let ty = node.kind.spec().outputs.get(usize::from(output.socket)).map(|o| o.ty);
                    (point, ty, false)
                }
                WireEnd::Input(input) => {
                    let Some(node) = graph.node(input.node) else {
                        return;
                    };
                    let Some(point) = input_point(node, input.socket) else {
                        return;
                    };
                    let ty = node.kind.spec().inputs.get(usize::from(input.socket)).map(|i| i.ty);
                    (point, ty, true)
                }
            };
            let color = ty.map_or(Color32::GRAY, wire_color);
            let anchor = view.point(anchor);
            let points = if reverse {
                wire_points(pointer, anchor, view.zoom)
            } else {
                wire_points(anchor, pointer, view.zoom)
            };
            painter.add(Shape::line(points, Stroke::new((2.0 * view.zoom).max(1.2), color)));
        }
        _ => {}
    }
}

// ---- Input -------------------------------------------------------------------------------------------------

/// What lies under a point.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hit {
    Socket(WireEnd),
    Widget(GraphNodeId, Target, Widget, Rect),
    Node(GraphNodeId),
    FrameHeader(rustium_core::FrameId),
    FrameCorner(rustium_core::FrameId),
    FrameBody(rustium_core::FrameId),
    Empty,
}

fn hit_test(hud: &Hud<'_>, position: Pos2) -> Hit {
    let Some(graph) = effects::current(hud) else {
        return Hit::Empty;
    };
    let view = View::of(hud);
    let reach = (SOCKET_RADIUS * view.zoom * 1.6).max(8.0);
    for node in graph.nodes().iter().rev() {
        let geo = geometry(node);
        for row in &geo.rows {
            let center = view.point(socket_point(&geo, row));
            let end = match row.kind {
                RowKind::Output(socket) => Some(WireEnd::Output(OutputRef { node: node.id, socket })),
                RowKind::Input(socket) => Some(WireEnd::Input(InputRef { node: node.id, socket })),
                RowKind::Param(_) => None,
            };
            if let Some(end) = end
                && center.distance(position) <= reach
            {
                return Hit::Socket(end);
            }
        }
    }
    for node in graph.nodes().iter().rev() {
        let geo = geometry(node);
        if !view.rect(geo.rect).contains(position) {
            continue;
        }
        for row in &geo.rows {
            let field = view.rect(widget_rect(row));
            if field.contains(position)
                && let Some((target, widget)) = widget_of(node, graph, row)
            {
                return Hit::Widget(node.id, target, widget, field);
            }
        }
        return Hit::Node(node.id);
    }
    let graph_point = view.back(position);
    for frame in graph.frames().iter().rev() {
        let rect = frame_rect(frame);
        if !rect.contains(graph_point) {
            continue;
        }
        if (rect.right_bottom() - graph_point).abs().max_elem() < 14.0 {
            return Hit::FrameCorner(frame.id);
        }
        if graph_point.y - rect.top() < FRAME_HEADER {
            return Hit::FrameHeader(frame.id);
        }
        return Hit::FrameBody(frame.id);
    }
    Hit::Empty
}

/// Handles everything the pointer and wheel do on the canvas.
pub(crate) fn input(ctx: &Context, hud: &mut Hud<'_>, response: &Response, pointer: Option<Pos2>) {
    if let Some(position) = pointer {
        hud.ui.effects.cursor = effects::to_graph(hud, position);
    }
    zoom_input(ctx, hud, response, pointer);
    // Pressing the canvas makes the area the focused widget, which would swallow the keyboard shortcuts.
    if response.clicked() || response.drag_started() {
        ctx.memory_mut(egui::Memory::stop_text_input);
    }

    let space = ctx.input(|input| input.key_down(Key::Space)) && !ctx.egui_wants_keyboard_input();
    let pan_tool = hud.ui.effects.tool == GraphTool::Pan || space;
    if response.dragged_by(PointerButton::Middle) || (pan_tool && response.dragged_by(PointerButton::Primary)) {
        hud.ui.effects.pan += response.drag_delta();
    }

    let origin = ctx.input(|input| input.pointer.press_origin()).or(pointer);
    if response.drag_started_by(PointerButton::Primary)
        && !pan_tool
        && let Some(origin) = origin
    {
        begin(ctx, hud, origin);
    }
    if response.dragged_by(PointerButton::Primary)
        && let Some(position) = pointer
    {
        // egui only starts a drag after a few pixels; count them, so the node stays under the pointer.
        let delta = match origin {
            Some(origin) if response.drag_started() => position - origin,
            _ => response.drag_delta(),
        };
        update(ctx, hud, delta, position);
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        end(ctx, hud, pointer);
    }
    if response.clicked_by(PointerButton::Primary)
        && let Some(position) = response.interact_pointer_pos()
    {
        click(ctx, hud, position);
    }
    if response.double_clicked_by(PointerButton::Primary)
        && let Some(position) = response.interact_pointer_pos()
        && let Hit::FrameHeader(id) | Hit::FrameBody(id) = hit_test(hud, position)
        && !locked(hud)
        && let Some(label) = effects::current(hud)
            .and_then(|graph| graph.frame(id))
            .map(|f| f.label.clone())
    {
        hud.ui.effects.frame_label = Some((id, label, true));
        hud.ui.effects.frame_selection = Some(id);
    }
}

/// The cursor context at `pointer`: what is under it, what is being dragged and which Tool is active.
pub(super) fn cursor_context(ctx: &Context, hud: &Hud<'_>, pointer: Pos2, input: PointerInput) -> EffectsCtx {
    let space = ctx.input(|input| input.key_down(Key::Space));
    let hit = hit_test(hud, pointer);
    let under = match hit {
        Hit::Socket(end) => CanvasHit::Socket(socket_tint(hud, end)),
        Hit::Widget(_, _, Widget::Scrub { .. } | Widget::Pair { .. }, _) => CanvasHit::Scrub,
        Hit::Widget(..) => CanvasHit::Click,
        Hit::Node(_) | Hit::FrameHeader(_) => CanvasHit::Grab,
        Hit::FrameCorner(_) => CanvasHit::FrameCorner,
        Hit::FrameBody(_) | Hit::Empty => CanvasHit::Empty,
    };
    let drag = match &hud.ui.effects.gesture {
        None => CanvasDrag::None,
        Some(Gesture::Move { .. }) => CanvasDrag::Move,
        Some(Gesture::Box { .. }) => CanvasDrag::Box,
        Some(Gesture::Cut { .. }) => CanvasDrag::Cut,
        Some(Gesture::Resize { .. }) => CanvasDrag::Resize,
        Some(Gesture::Scrub { .. }) => CanvasDrag::Scrub,
        Some(Gesture::Wire { from }) => CanvasDrag::Wire {
            tint: socket_tint(hud, *from),
            target_ok: match hit {
                Hit::Socket(end) => Some(wire_accepts(hud, *from, end)),
                _ => None,
            },
        },
    };
    EffectsCtx {
        tool: hud.ui.effects.tool,
        space: space && !ctx.egui_wants_keyboard_input(),
        input,
        locked: locked(hud),
        under,
        drag,
    }
}

/// The wire color of the type of the socket `end`.
fn socket_tint(hud: &Hud<'_>, end: WireEnd) -> [u8; 3] {
    let ty = effects::current(hud).and_then(|graph| match end {
        WireEnd::Output(output) => graph.node(output.node).and_then(|node| {
            node.kind
                .spec()
                .outputs
                .get(usize::from(output.socket))
                .map(|socket| socket.ty)
        }),
        WireEnd::Input(input) => graph.node(input.node).and_then(|node| {
            node.kind
                .spec()
                .inputs
                .get(usize::from(input.socket))
                .map(|socket| socket.ty)
        }),
    });
    ty.map_or([0xa6, 0xaf, 0xbc], rustium_core::SocketType::wire_color)
}

/// Whether dropping a wire started at `from` on the socket `onto` would connect: opposite sides of two different
/// nodes, and no cycle.
fn wire_accepts(hud: &Hud<'_>, from: WireEnd, onto: WireEnd) -> bool {
    let ((WireEnd::Output(output), WireEnd::Input(input)) | (WireEnd::Input(input), WireEnd::Output(output))) =
        (from, onto)
    else {
        return false;
    };
    let Some(graph) = effects::current(hud) else {
        return false;
    };
    if output.node == input.node {
        return false;
    }
    // A cycle appears when the node that would read the link already feeds the node that would write it.
    let mut stack = vec![input.node];
    let mut seen = vec![input.node];
    while let Some(node) = stack.pop() {
        for link in graph.links().iter().filter(|link| link.from.node == node) {
            if link.to.node == output.node {
                return false;
            }
            if !seen.contains(&link.to.node) {
                seen.push(link.to.node);
                stack.push(link.to.node);
            }
        }
    }
    true
}

fn zoom_input(ctx: &Context, hud: &mut Hud<'_>, response: &Response, pointer: Option<Pos2>) {
    if !response.hovered() {
        return;
    }
    let (scroll, pinch) = ctx.input(|input| (input.smooth_scroll_delta.y, input.zoom_delta()));
    let factor = (scroll * 0.0015).exp() * pinch;
    if (factor - 1.0).abs() < 1.0e-4 {
        return;
    }
    let effects = &mut hud.ui.effects;
    let new_zoom = (effects.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let about = pointer.unwrap_or_else(|| ctx.content_rect().center());
    let ratio = new_zoom / effects.zoom;
    effects.pan = about.to_vec2() - (about.to_vec2() - effects.pan) * ratio;
    effects.zoom = new_zoom;
}

fn begin(ctx: &Context, hud: &mut Hud<'_>, origin: Pos2) {
    hud.editor.finish_material_edit();
    let shift = ctx.input(|input| input.modifiers.shift);
    if hud.ui.effects.tool == GraphTool::Cut {
        hud.ui.effects.gesture = Some(Gesture::Cut { points: vec![origin] });
        return;
    }
    let locked = locked(hud);
    let hit = hit_test(hud, origin);
    let gesture = match hit {
        Hit::Socket(end) if !locked => start_wire(hud, end),
        Hit::Widget(node, target, widget, field) if !locked => {
            select_for_drag(hud, node, shift);
            start_scrub(hud, node, target, widget, field, origin)
        }
        Hit::Socket(WireEnd::Output(OutputRef { node, .. }) | WireEnd::Input(InputRef { node, .. }))
        | Hit::Widget(node, ..)
        | Hit::Node(node) => {
            select_for_drag(hud, node, shift);
            if locked { None } else { start_move(hud, node) }
        }
        Hit::FrameHeader(id) if !locked => {
            hud.ui.effects.frame_selection = Some(id);
            hud.ui.effects.selection.clear();
            start_frame_move(hud, id)
        }
        Hit::FrameCorner(id) if !locked => {
            hud.ui.effects.frame_selection = Some(id);
            Some(Gesture::Resize { frame: id })
        }
        Hit::FrameHeader(_) | Hit::FrameCorner(_) | Hit::FrameBody(_) | Hit::Empty => Some(Gesture::Box {
            start: origin,
            additive: shift,
        }),
    };
    hud.ui.effects.gesture = gesture;
}

fn select_for_drag(hud: &mut Hud<'_>, node: GraphNodeId, shift: bool) {
    let effects = &mut hud.ui.effects;
    effects.frame_selection = None;
    effects.tab = EffectsTab::Node;
    if !effects.selection.contains(&node) {
        if !shift {
            effects.selection.clear();
        }
        effects.selection.push(node);
    }
}

fn start_move(hud: &Hud<'_>, node: GraphNodeId) -> Option<Gesture> {
    let graph = effects::current(hud)?;
    let anchor = graph.node(node)?.position;
    Some(Gesture::Move {
        nodes: hud.ui.effects.selection.clone(),
        frames: Vec::new(),
        anchor: vec2(anchor[0], anchor[1]),
        raw: Vec2::ZERO,
        applied: Vec2::ZERO,
    })
}

fn start_frame_move(hud: &Hud<'_>, id: rustium_core::FrameId) -> Option<Gesture> {
    let graph = effects::current(hud)?;
    let frame = graph.frame(id)?;
    let rect = frame_rect(frame);
    let nodes = graph
        .nodes()
        .iter()
        .filter(|node| rect.contains_rect(geometry(node).rect))
        .map(|node| node.id)
        .collect();
    Some(Gesture::Move {
        nodes,
        frames: vec![id],
        anchor: vec2(frame.position[0], frame.position[1]),
        raw: Vec2::ZERO,
        applied: Vec2::ZERO,
    })
}

fn start_wire(hud: &mut Hud<'_>, end: WireEnd) -> Option<Gesture> {
    match end {
        WireEnd::Output(_) => Some(Gesture::Wire { from: end }),
        WireEnd::Input(input) => {
            let link = effects::current(hud)?.link_to(input).copied();
            match link {
                // Pulling a connected input detaches it and carries the wire away from its source.
                Some(link) => {
                    graph_edit(hud, GraphEdit::Disconnect { to: input }, true);
                    Some(Gesture::Wire {
                        from: WireEnd::Output(link.from),
                    })
                }
                None => Some(Gesture::Wire { from: end }),
            }
        }
    }
}

fn start_scrub(
    hud: &Hud<'_>,
    node: GraphNodeId,
    target: Target,
    widget: Widget,
    field: Rect,
    origin: Pos2,
) -> Option<Gesture> {
    let found = effects::current(hud)?.node(node)?;
    let second = matches!(widget, Widget::Pair { .. }) && origin.x > field.center().x;
    let start = match (
        target,
        found.params.get(usize::from(param_index(target))),
        found.inputs.get(usize::from(input_index(target))),
    ) {
        (Target::Param(_), Some(ParamValue::Float(value)), _)
        | (Target::Input(_), _, Some(InputValue::Float(value))) => *value,
        (Target::Param(_), Some(ParamValue::Int(value)), _) => scrub_int(*value),
        (Target::Param(_), Some(ParamValue::Range(pair)), _) => pair[usize::from(second)],
        _ => return None,
    };
    match widget {
        Widget::Scrub { .. } | Widget::Pair { .. } => Some(Gesture::Scrub {
            node,
            target,
            start,
            travelled: 0.0,
            second,
        }),
        _ => None,
    }
}

#[expect(clippy::cast_precision_loss, reason = "parameter values are small whole numbers")]
fn scrub_int(value: i32) -> f32 {
    value as f32
}

fn param_index(target: Target) -> u8 {
    match target {
        Target::Param(index) => index,
        Target::Input(_) => u8::MAX,
    }
}

fn input_index(target: Target) -> u8 {
    match target {
        Target::Input(index) => index,
        Target::Param(_) => u8::MAX,
    }
}

fn update(ctx: &Context, hud: &mut Hud<'_>, delta: Vec2, position: Pos2) {
    let Some(gesture) = hud.ui.effects.gesture.take() else {
        return;
    };
    let zoom = hud.ui.effects.zoom;
    let snap_on = hud.ui.effects.snap != ctx.input(|input| input.modifiers.alt);
    let next = match gesture {
        Gesture::Move {
            nodes,
            frames,
            anchor,
            raw,
            applied,
        } => {
            let raw = raw + delta / zoom;
            let wanted = anchor + raw;
            let wanted = if snap_on {
                vec2(snap(wanted.x), snap(wanted.y))
            } else {
                wanted
            };
            let total = wanted - anchor;
            let step = total - applied;
            let mut applied = applied;
            if step.length() > f32::EPSILON {
                let edit = GraphEdit::MoveNodes {
                    nodes: nodes.clone(),
                    frames: frames.clone(),
                    delta: [step.x, step.y],
                };
                if graph_edit(hud, edit, true) {
                    applied = total;
                }
            }
            Gesture::Move {
                nodes,
                frames,
                anchor,
                raw,
                applied,
            }
        }
        Gesture::Cut { mut points } => {
            if points.last().is_none_or(|last| last.distance(position) > 2.0) {
                points.push(position);
            }
            Gesture::Cut { points }
        }
        Gesture::Resize { frame } => {
            let found = effects::current(hud).and_then(|graph| graph.frame(frame)).cloned();
            if let Some(found) = found {
                let size = [
                    (found.size[0] + delta.x / zoom).max(80.0),
                    (found.size[1] + delta.y / zoom).max(60.0),
                ];
                graph_edit(
                    hud,
                    GraphEdit::SetFrame {
                        frame,
                        label: found.label,
                        position: found.position,
                        size,
                    },
                    true,
                );
            }
            Gesture::Resize { frame }
        }
        Gesture::Scrub {
            node,
            target,
            start,
            travelled,
            second,
        } => {
            let travelled = travelled + delta.x;
            scrub(hud, node, target, start, travelled, second);
            Gesture::Scrub {
                node,
                target,
                start,
                travelled,
                second,
            }
        }
        other => other,
    };
    hud.ui.effects.gesture = Some(next);
}

fn scrub(hud: &mut Hud<'_>, node: GraphNodeId, target: Target, start: f32, travelled: f32, second: bool) {
    let Some(found) = effects::current(hud).and_then(|graph| graph.node(node)) else {
        return;
    };
    let spec = found.kind.spec();
    let (min, max, whole) = match target {
        Target::Input(index) => spec
            .inputs
            .get(usize::from(index))
            .map_or((0.0, 1.0, false), |input| (input.range.0, input.range.1, false)),
        Target::Param(index) => match spec.params.get(usize::from(index)).map(|param| param.kind) {
            Some(ParamKind::Float { min, max, .. } | ParamKind::Range { min, max, .. }) => (min, max, false),
            Some(ParamKind::Int { min, max, .. }) => (scrub_int(min), scrub_int(max), true),
            _ => return,
        },
    };
    let per_point = if whole { 0.1 } else { (max - min).min(64.0) / 200.0 };
    let value = (start + travelled * per_point).clamp(min, max);
    let value = if whole { value.round() } else { value };
    let current = found.params.clone();
    let edit = match target {
        Target::Input(input) => GraphEdit::SetInput {
            node,
            input,
            value: InputValue::Float(value),
        },
        Target::Param(param) => {
            let new = match current.get(usize::from(param)) {
                Some(ParamValue::Float(_)) => ParamValue::Float(value),
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the value is rounded and clamped to the parameter's range"
                )]
                Some(ParamValue::Int(_)) => ParamValue::Int(value as i32),
                Some(ParamValue::Range([low, high])) => {
                    if second {
                        ParamValue::Range([*low, value.max(*low)])
                    } else {
                        ParamValue::Range([value.min(*high), *high])
                    }
                }
                _ => return,
            };
            GraphEdit::SetParam {
                node,
                param,
                value: new,
            }
        }
    };
    graph_edit(hud, edit, true);
}

fn end(ctx: &Context, hud: &mut Hud<'_>, pointer: Option<Pos2>) {
    let Some(gesture) = hud.ui.effects.gesture.take() else {
        return;
    };
    match gesture {
        Gesture::Box { start, additive } => {
            let Some(pointer) = pointer else {
                return;
            };
            let area = Rect::from_two_pos(start, pointer);
            let view = View::of(hud);
            let inside: Vec<GraphNodeId> = effects::current(hud)
                .map(|graph| {
                    graph
                        .nodes()
                        .iter()
                        .filter(|node| view.rect(geometry(node).rect).intersects(area))
                        .map(|node| node.id)
                        .collect()
                })
                .unwrap_or_default();
            let effects = &mut hud.ui.effects;
            effects.frame_selection = None;
            if !additive {
                effects.selection.clear();
            }
            for id in inside {
                if !effects.selection.contains(&id) {
                    effects.selection.push(id);
                }
            }
        }
        Gesture::Wire { from } => {
            if let Some(pointer) = pointer {
                connect_wire(hud, from, pointer);
            }
        }
        Gesture::Cut { points } => cut(hud, &points),
        Gesture::Move { .. } | Gesture::Resize { .. } | Gesture::Scrub { .. } => {}
    }
    let _ = ctx;
    hud.editor.finish_material_edit();
}

fn connect_wire(hud: &mut Hud<'_>, from: WireEnd, pointer: Pos2) {
    let hit = hit_test(hud, pointer);
    let edit = match (from, hit) {
        (WireEnd::Output(output), Hit::Socket(WireEnd::Input(input)))
        | (WireEnd::Input(input), Hit::Socket(WireEnd::Output(output))) => GraphEdit::Connect {
            from: output,
            to: input,
        },
        _ => return,
    };
    graph_edit(hud, edit, true);
}

fn cut(hud: &mut Hud<'_>, path: &[Pos2]) {
    if path.len() < 2 {
        return;
    }
    let Some(graph) = effects::current(hud) else {
        return;
    };
    let view = View::of(hud);
    let mut doomed = Vec::new();
    for link in graph.links() {
        let (Some(from_node), Some(to_node)) = (graph.node(link.from.node), graph.node(link.to.node)) else {
            continue;
        };
        let (Some(from), Some(to)) = (
            output_point(from_node, link.from.socket),
            input_point(to_node, link.to.socket),
        ) else {
            continue;
        };
        let wire = wire_points(view.point(from), view.point(to), view.zoom);
        let crosses = wire.windows(2).any(|piece| {
            path.windows(2)
                .any(|cut| segments_cross(piece[0], piece[1], cut[0], cut[1]))
        });
        if crosses {
            doomed.push(link.to);
        }
    }
    if doomed.is_empty() {
        return;
    }
    for to in doomed {
        graph_edit(hud, GraphEdit::Disconnect { to }, true);
    }
    hud.editor.finish_material_edit();
}

fn segments_cross(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> bool {
    let side = |p: Pos2, q: Pos2, r: Pos2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

fn click(ctx: &Context, hud: &mut Hud<'_>, position: Pos2) {
    let shift = ctx.input(|input| input.modifiers.shift);
    let locked = locked(hud);
    match hit_test(hud, position) {
        Hit::Widget(node, target, widget, field) => {
            select_for_drag(hud, node, shift);
            if !locked {
                click_widget(hud, node, target, widget, field);
            }
        }
        Hit::Node(node)
        | Hit::Socket(WireEnd::Output(OutputRef { node, .. }) | WireEnd::Input(InputRef { node, .. })) => {
            let effects = &mut hud.ui.effects;
            effects.frame_selection = None;
            effects.tab = EffectsTab::Node;
            if shift {
                if let Some(index) = effects.selection.iter().position(|id| *id == node) {
                    effects.selection.remove(index);
                } else {
                    effects.selection.push(node);
                }
            } else {
                effects.selection = vec![node];
            }
        }
        Hit::FrameHeader(id) | Hit::FrameBody(id) | Hit::FrameCorner(id) => {
            let effects = &mut hud.ui.effects;
            effects.frame_selection = Some(id);
            effects.selection.clear();
        }
        Hit::Empty => {
            let effects = &mut hud.ui.effects;
            effects.selection.clear();
            effects.frame_selection = None;
        }
    }
}

fn click_widget(hud: &mut Hud<'_>, node: GraphNodeId, target: Target, widget: Widget, field: Rect) {
    let Some(found) = effects::current(hud).and_then(|graph| graph.node(node)) else {
        return;
    };
    match (target, widget) {
        (Target::Param(param), Widget::Toggle) => {
            if let Some(ParamValue::Bool(on)) = found.params.get(usize::from(param)) {
                let value = ParamValue::Bool(!on);
                graph_edit(hud, GraphEdit::SetParam { node, param, value }, false);
            }
        }
        (Target::Param(param), Widget::Cycle) => {
            let options = match found.kind.spec().params.get(usize::from(param)).map(|p| p.kind) {
                Some(ParamKind::Choice { options, .. }) => options.len(),
                _ => return,
            };
            if let Some(ParamValue::Choice(current)) = found.params.get(usize::from(param)) {
                let next = (usize::try_from(*current).unwrap_or(0) + 1) % options.max(1);
                let value = ParamValue::Choice(u32::try_from(next).unwrap_or(0));
                graph_edit(hud, GraphEdit::SetParam { node, param, value }, false);
            }
        }
        (target, Widget::Color) => {
            hud.ui.effects.color_popup = Some((node, target, field.left_bottom() + vec2(0.0, 6.0)));
            hud.ui.effects.color_age = 0;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use rustium_core::{GraphNodeKind, SampleMaterial};

    use super::*;

    #[test]
    fn the_roof_slate_frame_encloses_the_nodes_it_groups() {
        let graph = SampleMaterial::RoofSlate.build();
        let frame = graph.frames().first().expect("the sample has a frame");
        let frame = frame_rect(frame);
        for node in graph.nodes() {
            if matches!(
                node.kind,
                GraphNodeKind::Noise | GraphNodeKind::Mix | GraphNodeKind::Remap
            ) {
                assert!(frame.contains_rect(geometry(node).rect), "{:?} sticks out", node.kind);
            }
        }
    }
}
