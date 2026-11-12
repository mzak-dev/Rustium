//! Gathers the cursor context of the current mode and draws the cursor (or sets the closest system one).

use std::time::Duration;

use egui::{Context, CursorIcon, Pos2};

use super::{Hud, Mode, effects_canvas, render_viewport};
use crate::cursor::{self, CursorSpec, Look, ModelCtx, PointerInput, RenderCtx};

/// Seconds the zoom badge stays after the last wheel tick.
const ZOOM_BADGE_SECONDS: f64 = 0.35;

/// Cursor bookkeeping across frames and between the viewport input and the end of the frame.
#[derive(Debug, Default)]
pub(crate) struct CursorUi {
    /// Set by the viewport input when the pointer is over the 3D view, node canvas or render frame.
    pub(super) pointer: Option<Pos2>,
    /// The time until which the wheel just turned counts as zooming.
    zoom_until: f64,
}

/// Draws the custom cursor, or sets the system one, when the pointer is over the viewport. Call it last in the frame so
/// the state of this frame's gestures is what the cursor shows.
pub(super) fn update(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(pointer) = hud.ui.cursor.pointer.take() else {
        return;
    };
    if hud.ui.modal_open() || hud.ui.tool_wheel.is_some() {
        return;
    }
    let (primary_down, middle_down, shift, scrolled, now) = ctx.input(|input| {
        (
            input.pointer.primary_down(),
            input.pointer.middle_down(),
            input.modifiers.shift,
            input.smooth_scroll_delta.y.abs() > f32::EPSILON,
            input.time,
        )
    });
    if scrolled {
        hud.ui.cursor.zoom_until = now + ZOOM_BADGE_SECONDS;
    }
    let zooming = now < hud.ui.cursor.zoom_until;
    if zooming {
        ctx.request_repaint_after(Duration::from_secs_f64(ZOOM_BADGE_SECONDS));
    }
    let raw = PointerInput {
        primary_down,
        middle_down,
        shift,
        zooming,
    };
    let spec = match hud.ui.mode {
        Mode::Model => Some(cursor::resolve_model(&model_context(hud, raw))),
        Mode::Render => cursor::resolve_render(RenderCtx {
            tool: hud.ui.render.tool,
            in_frame: render_viewport::frame_contains(hud, pointer),
            input: raw,
        }),
        Mode::Effects => Some(cursor::resolve_effects(&effects_canvas::cursor_context(
            ctx, hud, pointer, raw,
        ))),
    };
    let Some(spec) = spec else {
        return;
    };
    apply(ctx, hud, &spec, pointer);
}

fn apply(ctx: &Context, hud: &Hud<'_>, spec: &CursorSpec, pointer: Pos2) {
    if hud.settings.custom_cursors && !spec.kind.is_native() {
        ctx.set_cursor_icon(CursorIcon::None);
        let look = Look {
            accent: hud.accent(),
            motion: hud.settings.motion,
        };
        cursor::paint(ctx, spec, pointer, look);
    } else {
        ctx.set_cursor_icon(cursor::system_icon(spec));
    }
}

fn model_context(hud: &Hud<'_>, input: PointerInput) -> ModelCtx {
    let editor = &*hud.editor;
    let measuring = editor.measurement.start.is_some() && editor.measurement.end.is_none();
    let distance = editor
        .distance()
        .filter(|_| measuring)
        .map(|(_, _, distance)| format!("{:.1} vox · {:.2} m", distance.voxels, distance.metres));
    let swatch = editor
        .color_under_hover()
        .and_then(|id| editor.scene.palette().get(id))
        .map(|color| color.color);
    ModelCtx {
        tool: editor.tool,
        brush: editor.brush.kind,
        fill_mode: editor.fill_mode,
        block: editor.tool_block(),
        input,
        measuring,
        swatch,
        distance,
    }
}
