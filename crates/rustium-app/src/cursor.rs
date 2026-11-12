//! Context-aware viewport cursors.
//!
//! Over the 3D view, the node canvas and the render frame the pointer is drawn by Rustium: a hotspot glyph (crosshair,
//! arrow or a centred icon) plus a small badge for the context. The context-to-cursor mapping is pure (`resolve_*`
//! take a small context struct), so the same logic drives both the drawn cursor and the system-cursor fallback of
//! Settings → Interface → Custom cursors. Gathering the contexts from the HUD lives in `hud::cursors`.

use egui::{Align2, Color32, Context, CursorIcon, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Stroke, Vec2, vec2};
use rustium_core::{BrushKind, FillMode};

use crate::editor::{Tool, ToolBlock};
use crate::hud::GraphTool;
use crate::icons::Icon;
use crate::render_mode::RenderTool;
use crate::settings::Motion;
use crate::theme;

/// The thing a cursor stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorKind {
    SelectColor,
    Attach,
    Erase,
    Paint,
    FillSurface,
    FillVolume,
    Pick,
    Move,
    Measure,
    Orbit,
    Pan,
    Zoom,
    Dolly,
    Focus,
    Sun,
    Region,
    /// Node canvas: the plain pointer.
    Arrow,
    /// Node canvas: over a node or frame header, or dragging nodes.
    NodeMove,
    /// Node canvas: over a socket.
    Connect,
    /// Node canvas: dragging a link.
    Link,
    Cut,
    BoxSelect,
    /// Frame corner: the system resize cursor.
    Resize,
    /// A number field: the system horizontal resize cursor.
    Scrub,
    /// A toggle, cycle or color field: the system pointing hand.
    Click,
}

impl CursorKind {
    /// Kinds the system draws even when custom cursors are on.
    pub(crate) fn is_native(self) -> bool {
        matches!(self, Self::Resize | Self::Scrub | Self::Click)
    }
}

/// How the cursor looks within its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorState {
    /// The action would work.
    Normal,
    /// The button is down, or the gesture is running.
    Pressed,
    /// The action would do nothing here.
    Forbidden,
}

/// A resolved cursor: what to draw and the small extras around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CursorSpec {
    pub(crate) kind: CursorKind,
    pub(crate) state: CursorState,
    /// Shift adds to the selection.
    pub(crate) extend: bool,
    /// Brush shape hint of Attach, Erase and Paint.
    pub(crate) brush: Option<BrushKind>,
    /// A color the badge shows: the picked voxel's Color or the socket type.
    pub(crate) tint: Option<[u8; 3]>,
    /// Live text next to the cursor, e.g. the Measure distance.
    pub(crate) label: Option<String>,
}

impl CursorSpec {
    pub(crate) fn new(kind: CursorKind, state: CursorState) -> Self {
        Self {
            kind,
            state,
            extend: false,
            brush: None,
            tint: None,
            label: None,
        }
    }
}

/// Buttons, modifier and wheel state the cursor reacts to.
#[expect(clippy::struct_excessive_bools, reason = "independent input flags")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PointerInput {
    pub(crate) primary_down: bool,
    pub(crate) middle_down: bool,
    pub(crate) shift: bool,
    /// The wheel was turned a moment ago.
    pub(crate) zooming: bool,
}

// ---- Model mode --------------------------------------------------------------------------------------------

/// Everything that decides the cursor over the 3D view.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelCtx {
    pub(crate) tool: Tool,
    pub(crate) brush: BrushKind,
    pub(crate) fill_mode: FillMode,
    /// Why the Tool would do nothing here, from the Editor.
    pub(crate) block: Option<ToolBlock>,
    pub(crate) input: PointerInput,
    /// The Measure Tool has a first point and follows the pointer.
    pub(crate) measuring: bool,
    /// Color of the voxel under the pointer, for Pick color.
    pub(crate) swatch: Option<[u8; 3]>,
    /// Distance text of the Measure Tool.
    pub(crate) distance: Option<String>,
    /// The Cursor is over something selected that it would drag.
    pub(crate) over_selection: bool,
}

/// The cursor over the 3D view.
pub(crate) fn resolve_model(ctx: &ModelCtx) -> CursorSpec {
    if ctx.input.middle_down {
        let kind = if ctx.input.shift {
            CursorKind::Pan
        } else {
            CursorKind::Orbit
        };
        return CursorSpec::new(kind, CursorState::Pressed);
    }
    if ctx.input.zooming {
        return CursorSpec::new(CursorKind::Zoom, CursorState::Normal);
    }
    let kind = match ctx.tool {
        Tool::SelectBox if ctx.over_selection => CursorKind::Move,
        Tool::SelectBox if ctx.input.primary_down => CursorKind::BoxSelect,
        Tool::SelectBox => CursorKind::Arrow,
        Tool::SelectColor => CursorKind::SelectColor,
        Tool::Attach => CursorKind::Attach,
        Tool::Erase => CursorKind::Erase,
        Tool::Paint => CursorKind::Paint,
        Tool::Fill => match ctx.fill_mode {
            FillMode::Surface => CursorKind::FillSurface,
            FillMode::Volume => CursorKind::FillVolume,
        },
        Tool::Pick => CursorKind::Pick,
        Tool::Move => CursorKind::Move,
        Tool::Measure => CursorKind::Measure,
    };
    let state = if ctx.block.is_some() {
        CursorState::Forbidden
    } else if ctx.input.primary_down {
        CursorState::Pressed
    } else {
        CursorState::Normal
    };
    let mut spec = CursorSpec::new(kind, state);
    spec.extend = ctx.input.shift && matches!(ctx.tool, Tool::SelectBox | Tool::SelectColor);
    spec.brush = matches!(ctx.tool, Tool::Attach | Tool::Erase | Tool::Paint).then_some(ctx.brush);
    if ctx.tool == Tool::Pick && state != CursorState::Forbidden {
        spec.tint = ctx.swatch;
    }
    if ctx.tool == Tool::Measure && ctx.measuring {
        spec.label.clone_from(&ctx.distance);
    }
    spec
}

// ---- Render mode -------------------------------------------------------------------------------------------

/// Everything that decides the cursor over the render frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderCtx {
    pub(crate) tool: RenderTool,
    /// The pointer is inside the 16:9 frame.
    pub(crate) in_frame: bool,
    pub(crate) input: PointerInput,
}

/// The cursor over the render frame; `None` outside it (the system cursor stays).
pub(crate) fn resolve_render(ctx: RenderCtx) -> Option<CursorSpec> {
    let dragging = ctx.input.primary_down || ctx.input.middle_down;
    if !ctx.in_frame && !dragging {
        return None;
    }
    if ctx.input.middle_down {
        return Some(CursorSpec::new(CursorKind::Pan, CursorState::Pressed));
    }
    if ctx.input.zooming {
        return Some(CursorSpec::new(CursorKind::Zoom, CursorState::Normal));
    }
    let kind = match ctx.tool {
        RenderTool::Orbit => CursorKind::Orbit,
        RenderTool::Pan => CursorKind::Pan,
        RenderTool::Dolly => CursorKind::Dolly,
        RenderTool::PickFocus => CursorKind::Focus,
        RenderTool::AimSun => CursorKind::Sun,
        RenderTool::Region => CursorKind::Region,
    };
    let state = if ctx.input.primary_down {
        CursorState::Pressed
    } else {
        CursorState::Normal
    };
    Some(CursorSpec::new(kind, state))
}

// ---- Effects mode ------------------------------------------------------------------------------------------

/// What lies under the pointer on the node canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanvasHit {
    Empty,
    /// A socket and its type color.
    Socket([u8; 3]),
    /// A node or frame header.
    Grab,
    FrameCorner,
    /// A number field.
    Scrub,
    /// A toggle, cycle or color field.
    Click,
}

/// What the pointer is dragging on the node canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanvasDrag {
    None,
    Move,
    Box,
    /// A link from a socket of this color; `target_ok` says whether the socket under the pointer would accept it
    /// (`None` over empty canvas).
    Wire {
        tint: [u8; 3],
        target_ok: Option<bool>,
    },
    Cut,
    Resize,
    Scrub,
}

/// Everything that decides the cursor over the node canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectsCtx {
    pub(crate) tool: GraphTool,
    /// Space is held: pan like the Pan tool.
    pub(crate) space: bool,
    pub(crate) input: PointerInput,
    /// The edited Material is the built-in Default, which cannot be changed.
    pub(crate) locked: bool,
    pub(crate) under: CanvasHit,
    pub(crate) drag: CanvasDrag,
}

/// The cursor over the node canvas.
pub(crate) fn resolve_effects(ctx: &EffectsCtx) -> CursorSpec {
    if ctx.input.middle_down || ((ctx.tool == GraphTool::Pan || ctx.space) && ctx.input.primary_down) {
        return CursorSpec::new(CursorKind::Pan, CursorState::Pressed);
    }
    if ctx.tool == GraphTool::Pan || ctx.space {
        return CursorSpec::new(CursorKind::Pan, CursorState::Normal);
    }
    match ctx.drag {
        CanvasDrag::Move => return CursorSpec::new(CursorKind::NodeMove, CursorState::Pressed),
        CanvasDrag::Box => return CursorSpec::new(CursorKind::BoxSelect, CursorState::Pressed),
        CanvasDrag::Cut => return CursorSpec::new(CursorKind::Cut, CursorState::Pressed),
        CanvasDrag::Resize => return CursorSpec::new(CursorKind::Resize, CursorState::Pressed),
        CanvasDrag::Scrub => return CursorSpec::new(CursorKind::Scrub, CursorState::Pressed),
        CanvasDrag::Wire { tint, target_ok } => {
            let state = if target_ok == Some(false) {
                CursorState::Forbidden
            } else {
                CursorState::Pressed
            };
            let mut spec = CursorSpec::new(CursorKind::Link, state);
            spec.tint = Some(tint);
            return spec;
        }
        CanvasDrag::None => {}
    }
    if ctx.input.zooming {
        return CursorSpec::new(CursorKind::Zoom, CursorState::Normal);
    }
    if ctx.tool == GraphTool::Cut {
        let state = if ctx.input.primary_down {
            CursorState::Pressed
        } else {
            CursorState::Normal
        };
        return CursorSpec::new(CursorKind::Cut, state);
    }
    match ctx.under {
        CanvasHit::Socket(tint) => {
            let state = if ctx.locked {
                CursorState::Forbidden
            } else {
                CursorState::Normal
            };
            let mut spec = CursorSpec::new(CursorKind::Connect, state);
            spec.tint = Some(tint);
            spec
        }
        CanvasHit::Grab if !ctx.locked => CursorSpec::new(CursorKind::NodeMove, CursorState::Normal),
        CanvasHit::FrameCorner if !ctx.locked => CursorSpec::new(CursorKind::Resize, CursorState::Normal),
        CanvasHit::Scrub if !ctx.locked => CursorSpec::new(CursorKind::Scrub, CursorState::Normal),
        CanvasHit::Click if !ctx.locked => CursorSpec::new(CursorKind::Click, CursorState::Normal),
        _ => CursorSpec::new(CursorKind::Arrow, CursorState::Normal),
    }
}

// ---- System fallback ---------------------------------------------------------------------------------------

/// The closest system cursor, used when custom cursors are off and for the native kinds.
pub(crate) fn system_icon(spec: &CursorSpec) -> CursorIcon {
    if spec.state == CursorState::Forbidden {
        return CursorIcon::NotAllowed;
    }
    let pressed = spec.state == CursorState::Pressed;
    match spec.kind {
        CursorKind::Attach
        | CursorKind::Erase
        | CursorKind::Paint
        | CursorKind::Pick
        | CursorKind::Measure
        | CursorKind::Focus
        | CursorKind::Region
        | CursorKind::Connect
        | CursorKind::BoxSelect
        | CursorKind::Cut => CursorIcon::Crosshair,
        CursorKind::SelectColor | CursorKind::FillSurface | CursorKind::FillVolume => CursorIcon::Cell,
        CursorKind::Orbit | CursorKind::Sun => CursorIcon::AllScroll,
        CursorKind::Move | CursorKind::Pan | CursorKind::NodeMove if pressed => CursorIcon::Grabbing,
        CursorKind::Move => CursorIcon::Move,
        CursorKind::Pan | CursorKind::NodeMove => CursorIcon::Grab,
        CursorKind::Zoom | CursorKind::Dolly => CursorIcon::ZoomIn,
        CursorKind::Arrow => CursorIcon::Default,
        CursorKind::Link => CursorIcon::Alias,
        CursorKind::Resize => CursorIcon::ResizeNwSe,
        CursorKind::Scrub => CursorIcon::ResizeHorizontal,
        CursorKind::Click => CursorIcon::PointingHand,
    }
}

// ---- Drawing -----------------------------------------------------------------------------------------------

/// The part of a cursor that sits on the hotspot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    Crosshair,
    Arrow,
    /// An icon centred on the pointer.
    Icon(Icon),
}

fn body(spec: &CursorSpec) -> Body {
    let pressed = spec.state == CursorState::Pressed;
    match spec.kind {
        CursorKind::Arrow => Body::Arrow,
        CursorKind::Orbit => Body::Icon(Icon::Orbit),
        CursorKind::Pan | CursorKind::NodeMove | CursorKind::Move if pressed => Body::Icon(Icon::Grab),
        CursorKind::Pan | CursorKind::NodeMove => Body::Icon(Icon::Hand),
        CursorKind::Move => Body::Icon(Icon::Move),
        CursorKind::Zoom | CursorKind::Dolly => Body::Icon(Icon::Dolly),
        CursorKind::Focus => Body::Icon(Icon::Focus),
        CursorKind::Sun => Body::Icon(Icon::Sun),
        _ => Body::Crosshair,
    }
}

/// The context icon next to the hotspot.
fn badge_icon(kind: CursorKind) -> Option<Icon> {
    match kind {
        CursorKind::Region => Some(Icon::SelectBox),
        CursorKind::SelectColor => Some(Icon::SelectColor),
        CursorKind::Attach => Some(Icon::Attach),
        CursorKind::Erase => Some(Icon::Erase),
        CursorKind::Paint => Some(Icon::Paint),
        CursorKind::FillSurface | CursorKind::FillVolume => Some(Icon::Fill),
        CursorKind::Pick => Some(Icon::Pick),
        CursorKind::Measure => Some(Icon::Measure),
        CursorKind::Cut => Some(Icon::Scissors),
        _ => None,
    }
}

fn brush_icon(brush: BrushKind) -> Icon {
    match brush {
        BrushKind::Voxel => Icon::BrushVoxel,
        BrushKind::Line => Icon::BrushLine,
        BrushKind::Box => Icon::BrushBox,
        BrushKind::Face => Icon::BrushFace,
    }
}

const DANGER: Color32 = Color32::from_rgb(0xff, 0x6b, 0x5b);
const OUTLINE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 215);
/// Badge centre relative to the hotspot, and its radius.
const BADGE_OFFSET: Vec2 = vec2(15.0, 15.0);
const BADGE_RADIUS: f32 = 9.5;

/// Per-viewer look settings for drawing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Look {
    pub(crate) accent: Color32,
    pub(crate) motion: Motion,
}

/// Draws `spec` with its hotspot at `pos` on a layer above everything else.
pub(crate) fn paint(ctx: &Context, spec: &CursorSpec, pos: Pos2, look: Look) {
    let target = if spec.state == CursorState::Pressed { 0.88 } else { 1.0 };
    // Only Full motion eases the press; otherwise the filled badge alone says "pressed".
    let scale = if look.motion == Motion::Full {
        ctx.animate_value_with_time(Id::unique("rustium-cursor-scale"), target, 0.09)
    } else {
        1.0
    };
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("rustium-cursor")));
    let dim = if spec.state == CursorState::Forbidden {
        0.55
    } else {
        1.0
    };
    let light = theme::TEXT.gamma_multiply(dim);
    let at = |offset: Vec2| pos + offset * scale;

    match body(spec) {
        Body::Crosshair => crosshair(&painter, pos, scale, light),
        Body::Arrow => {
            // The icon's tip sits at (5, 3) of its 24-unit box.
            let size = 22.0 * scale;
            let centre = pos + vec2(12.0 - 5.0, 12.0 - 3.0) * size / 24.0;
            Icon::Pointer.paint_outlined(
                &painter,
                Rect::from_center_size(centre, Vec2::splat(size)),
                light,
                OUTLINE,
            );
        }
        Body::Icon(icon) => {
            let size = 24.0 * scale;
            icon.paint_outlined(&painter, Rect::from_center_size(pos, Vec2::splat(size)), light, OUTLINE);
        }
    }

    let centred = matches!(body(spec), Body::Icon(_));
    let badge_centre = at(if centred { vec2(16.0, 16.0) } else { BADGE_OFFSET });
    paint_badge(&painter, spec, badge_centre, scale, look);

    if spec.extend {
        Icon::Plus.paint_outlined(
            &painter,
            Rect::from_center_size(at(vec2(11.0, -11.0)), Vec2::splat(10.0 * scale)),
            theme::TEXT,
            OUTLINE,
        );
    }
    if let Some(brush) = spec.brush {
        Icon::paint_outlined(
            brush_icon(brush),
            &painter,
            Rect::from_center_size(badge_centre + vec2(10.0, 10.0) * scale, Vec2::splat(9.0 * scale)),
            theme::SOFT.gamma_multiply(dim),
            OUTLINE,
        );
    }
    if let Some(label) = &spec.label {
        paint_label(&painter, label, badge_centre + vec2(BADGE_RADIUS + 5.0, 0.0) * scale);
    }
}

fn crosshair(painter: &Painter, pos: Pos2, scale: f32, light: Color32) {
    let arms = [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)];
    for (color, width) in [(OUTLINE, 3.4), (light, 1.4)] {
        for arm in arms {
            painter.line_segment(
                [pos + arm * 3.5 * scale, pos + arm * 9.5 * scale],
                Stroke::new(width, color),
            );
        }
    }
    painter.circle_filled(pos, 2.0 * scale, OUTLINE);
    painter.circle_filled(pos, 1.0 * scale, light);
}

fn paint_badge(painter: &Painter, spec: &CursorSpec, centre: Pos2, scale: f32, look: Look) {
    let radius = BADGE_RADIUS * scale;
    let icon_box = Vec2::splat(13.0 * scale);
    let forbidden = spec.state == CursorState::Forbidden;
    if forbidden {
        disc(
            painter,
            centre,
            radius,
            theme::PANEL_SOLID.gamma_multiply(0.9),
            Color32::TRANSPARENT,
        );
        Icon::Forbidden.paint(painter, Rect::from_center_size(centre, icon_box * 1.15), DANGER);
        return;
    }
    let pressed = spec.state == CursorState::Pressed;
    // A tint replaces the disc fill with the picked Color; sockets get a plain dot instead of an icon.
    let socket = matches!(spec.kind, CursorKind::Connect | CursorKind::Link);
    if socket {
        let [r, g, b] = spec.tint.unwrap_or([0xa6, 0xaf, 0xbc]);
        let dot = Color32::from_rgb(r, g, b);
        let size = if spec.kind == CursorKind::Link { 5.5 } else { 4.5 };
        painter.circle_filled(centre, (size + 1.5) * scale, OUTLINE);
        painter.circle_filled(centre, size * scale, dot);
        if spec.kind == CursorKind::Link {
            painter.circle_stroke(centre, (size + 3.0) * scale, Stroke::new(1.0, dot));
        }
        return;
    }
    let Some(icon) = badge_icon(spec.kind) else {
        return;
    };
    let (fill, ink) = match spec.tint {
        Some([r, g, b]) => {
            let fill = Color32::from_rgb(r, g, b);
            (fill, readable_on(fill))
        }
        None if pressed => (look.accent, theme::ON_ACCENT),
        None => (theme::PANEL_SOLID.gamma_multiply(0.92), theme::TEXT),
    };
    disc(
        painter,
        centre,
        radius,
        fill,
        if pressed { look.accent } else { theme::BORDER },
    );
    icon.paint(painter, Rect::from_center_size(centre, icon_box), ink);
}

/// A disc with a crisp dark outer ring.
fn disc(painter: &Painter, centre: Pos2, radius: f32, fill: Color32, ring: Color32) {
    painter.circle_filled(centre, radius + 1.0, OUTLINE);
    painter.circle_filled(centre, radius, fill);
    painter.circle_stroke(centre, radius - 0.5, Stroke::new(1.0, ring));
}

/// Black or white ink, whichever reads better on `fill`.
fn readable_on(fill: Color32) -> Color32 {
    let luma = 0.299 * f32::from(fill.r()) + 0.587 * f32::from(fill.g()) + 0.114 * f32::from(fill.b());
    if luma > 140.0 {
        Color32::from_rgb(0x14, 0x16, 0x1a)
    } else {
        Color32::WHITE
    }
}

fn paint_label(painter: &Painter, text: &str, left_centre: Pos2) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::monospace(11.0), theme::TEXT);
    let rect = Rect::from_min_size(
        left_centre - vec2(0.0, galley.size().y / 2.0 + 2.0),
        galley.size() + vec2(10.0, 4.0),
    );
    painter.rect(
        rect,
        6,
        theme::PANEL_SOLID.gamma_multiply(0.92),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Outside,
    );
    painter.text(
        rect.left_center() + vec2(5.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        FontId::monospace(11.0),
        theme::TEXT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down() -> PointerInput {
        PointerInput {
            primary_down: true,
            ..PointerInput::default()
        }
    }

    fn model() -> ModelCtx {
        ModelCtx {
            tool: Tool::Attach,
            brush: BrushKind::Voxel,
            fill_mode: FillMode::Surface,
            block: None,
            input: PointerInput::default(),
            measuring: false,
            swatch: None,
            distance: None,
            over_selection: false,
        }
    }

    #[test]
    fn every_model_tool_has_its_own_kind() {
        let mut kinds = Vec::new();
        for tool in Tool::ALL {
            let spec = resolve_model(&ModelCtx { tool, ..model() });
            assert_eq!(spec.state, CursorState::Normal);
            assert!(!kinds.contains(&spec.kind), "{tool:?} repeats a kind");
            kinds.push(spec.kind);
        }
    }

    #[test]
    fn fill_variant_follows_the_mode() {
        let fill = ModelCtx {
            tool: Tool::Fill,
            ..model()
        };
        assert_eq!(resolve_model(&fill).kind, CursorKind::FillSurface);
        let volume = ModelCtx {
            fill_mode: FillMode::Volume,
            ..fill
        };
        assert_eq!(resolve_model(&volume).kind, CursorKind::FillVolume);
    }

    #[test]
    fn blocked_tool_is_forbidden_even_while_pressed() {
        for block in [ToolBlock::NeedsModel, ToolBlock::NoTarget] {
            let spec = resolve_model(&ModelCtx {
                tool: Tool::Erase,
                block: Some(block),
                input: down(),
                ..model()
            });
            assert_eq!(spec.state, CursorState::Forbidden);
            assert_eq!(system_icon(&spec), CursorIcon::NotAllowed);
        }
    }

    #[test]
    fn pressed_and_brush_hint() {
        let spec = resolve_model(&ModelCtx {
            tool: Tool::Paint,
            brush: BrushKind::Box,
            input: down(),
            ..model()
        });
        assert_eq!(spec.state, CursorState::Pressed);
        assert_eq!(spec.brush, Some(BrushKind::Box));
        let pick = resolve_model(&ModelCtx {
            tool: Tool::Pick,
            ..model()
        });
        assert_eq!(pick.brush, None);
    }

    #[test]
    fn pick_shows_the_hovered_swatch_unless_forbidden() {
        let pick = ModelCtx {
            tool: Tool::Pick,
            swatch: Some([1, 2, 3]),
            ..model()
        };
        assert_eq!(resolve_model(&pick).tint, Some([1, 2, 3]));
        let blocked = ModelCtx {
            block: Some(ToolBlock::NoTarget),
            ..pick
        };
        assert_eq!(resolve_model(&blocked).tint, None);
    }

    #[test]
    fn shift_extends_only_selection_tools() {
        let select = resolve_model(&ModelCtx {
            tool: Tool::SelectBox,
            input: PointerInput {
                shift: true,
                ..PointerInput::default()
            },
            ..model()
        });
        assert!(select.extend);
        assert!(
            !resolve_model(&ModelCtx {
                input: PointerInput {
                    shift: true,
                    ..PointerInput::default()
                },
                ..model()
            })
            .extend
        );
    }

    #[test]
    fn measure_label_appears_only_while_measuring() {
        let measure = ModelCtx {
            tool: Tool::Measure,
            distance: Some("3.0 vox".to_owned()),
            ..model()
        };
        assert_eq!(resolve_model(&measure).label, None);
        let measuring = ModelCtx {
            measuring: true,
            ..measure
        };
        assert_eq!(resolve_model(&measuring).label.as_deref(), Some("3.0 vox"));
    }

    #[test]
    fn navigation_overrides_the_tool() {
        let orbit = resolve_model(&ModelCtx {
            input: PointerInput {
                middle_down: true,
                ..PointerInput::default()
            },
            ..model()
        });
        assert_eq!(orbit.kind, CursorKind::Orbit);
        let pan = resolve_model(&ModelCtx {
            input: PointerInput {
                middle_down: true,
                shift: true,
                ..PointerInput::default()
            },
            ..model()
        });
        assert_eq!((pan.kind, pan.state), (CursorKind::Pan, CursorState::Pressed));
        assert_eq!(system_icon(&pan), CursorIcon::Grabbing);
        let zoom = resolve_model(&ModelCtx {
            input: PointerInput {
                zooming: true,
                ..PointerInput::default()
            },
            ..model()
        });
        assert_eq!(zoom.kind, CursorKind::Zoom);
    }

    fn render(tool: RenderTool) -> RenderCtx {
        RenderCtx {
            tool,
            in_frame: true,
            input: PointerInput::default(),
        }
    }

    #[test]
    fn render_tools_map_and_outside_the_frame_is_system() {
        let kinds: Vec<CursorKind> = RenderTool::ALL
            .into_iter()
            .map(|tool| resolve_render(render(tool)).map(|spec| spec.kind).unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                CursorKind::Orbit,
                CursorKind::Pan,
                CursorKind::Dolly,
                CursorKind::Focus,
                CursorKind::Sun,
                CursorKind::Region
            ]
        );
        let outside = RenderCtx {
            in_frame: false,
            ..render(RenderTool::Orbit)
        };
        assert_eq!(resolve_render(outside), None);
        let dragging = RenderCtx {
            input: down(),
            ..outside
        };
        assert_eq!(resolve_render(dragging).unwrap().state, CursorState::Pressed);
    }

    fn effects() -> EffectsCtx {
        EffectsCtx {
            tool: GraphTool::Select,
            space: false,
            input: PointerInput::default(),
            locked: false,
            under: CanvasHit::Empty,
            drag: CanvasDrag::None,
        }
    }

    #[test]
    fn effects_hover_targets() {
        assert_eq!(resolve_effects(&effects()).kind, CursorKind::Arrow);
        let socket = resolve_effects(&EffectsCtx {
            under: CanvasHit::Socket([9, 8, 7]),
            ..effects()
        });
        assert_eq!((socket.kind, socket.tint), (CursorKind::Connect, Some([9, 8, 7])));
        let grab = resolve_effects(&EffectsCtx {
            under: CanvasHit::Grab,
            ..effects()
        });
        assert_eq!(grab.kind, CursorKind::NodeMove);
        assert!(CursorKind::Resize.is_native());
        let corner = resolve_effects(&EffectsCtx {
            under: CanvasHit::FrameCorner,
            ..effects()
        });
        assert_eq!(corner.kind, CursorKind::Resize);
    }

    #[test]
    fn effects_gestures_and_tools() {
        let moving = resolve_effects(&EffectsCtx {
            drag: CanvasDrag::Move,
            ..effects()
        });
        assert_eq!(
            (moving.kind, moving.state),
            (CursorKind::NodeMove, CursorState::Pressed)
        );
        let pan = resolve_effects(&EffectsCtx {
            tool: GraphTool::Pan,
            ..effects()
        });
        assert_eq!((pan.kind, pan.state), (CursorKind::Pan, CursorState::Normal));
        let space = resolve_effects(&EffectsCtx {
            space: true,
            input: down(),
            ..effects()
        });
        assert_eq!((space.kind, space.state), (CursorKind::Pan, CursorState::Pressed));
        let cut = resolve_effects(&EffectsCtx {
            tool: GraphTool::Cut,
            ..effects()
        });
        assert_eq!(cut.kind, CursorKind::Cut);
        let boxed = resolve_effects(&EffectsCtx {
            drag: CanvasDrag::Box,
            ..effects()
        });
        assert_eq!(boxed.kind, CursorKind::BoxSelect);
    }

    #[test]
    fn dragging_a_link_reports_incompatible_targets() {
        let wire = |target_ok| {
            resolve_effects(&EffectsCtx {
                drag: CanvasDrag::Wire {
                    tint: [1, 1, 1],
                    target_ok,
                },
                ..effects()
            })
        };
        assert_eq!(wire(None).state, CursorState::Pressed);
        assert_eq!(wire(Some(true)).state, CursorState::Pressed);
        assert_eq!(wire(Some(false)).state, CursorState::Forbidden);
        assert_eq!(wire(None).tint, Some([1, 1, 1]));
    }

    #[test]
    fn locked_material_forbids_wiring_and_hides_grabs() {
        let socket = resolve_effects(&EffectsCtx {
            locked: true,
            under: CanvasHit::Socket([0, 0, 0]),
            ..effects()
        });
        assert_eq!(socket.state, CursorState::Forbidden);
        let node = resolve_effects(&EffectsCtx {
            locked: true,
            under: CanvasHit::Grab,
            ..effects()
        });
        assert_eq!(node.kind, CursorKind::Arrow);
    }

    #[test]
    fn every_kind_has_a_system_icon() {
        for tool in Tool::ALL {
            let spec = resolve_model(&ModelCtx { tool, ..model() });
            assert_ne!(system_icon(&spec), CursorIcon::NotAllowed);
        }
    }
}
