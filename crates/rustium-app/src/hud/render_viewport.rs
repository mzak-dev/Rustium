//! Render mode, the viewport itself: where the 16:9 frame sits, its labels, guides and progress bar, and what the
//! pointer and keys do over it.

use egui::{Align2, Color32, Context, FontId, PointerButton, Pos2, Rect, Response, Stroke, StrokeKind, Ui, pos2, vec2};
use glam::{Vec2 as GlamVec2, Vec3};
use rustium_core::{HitTarget, NodeId, pick_scene};

use super::{Hud, INSPECTOR_WIDTH, MARGIN, render_bottom};
use crate::keymap::Action;
use crate::render_mode::{OrbitAround, RenderTool};
use crate::theme;

/// Space the frame keeps clear: left toolbar, top bar, bottom shots island.
const AREA_LEFT: f32 = 84.0;
const AREA_TOP: f32 = 66.0;
const AREA_BOTTOM: f32 = 100.0;
/// Height of the label row above the frame and the progress row below it.
const LABEL_HEIGHT: f32 = 18.0;
const PROGRESS_HEIGHT: f32 = 10.0;

/// Fits the output aspect into the free area between the islands and records it for the renderer.
pub(crate) fn layout(ctx: &Context, hud: &mut Hud<'_>) {
    let screen = ctx.content_rect();
    let right = if hud.ui.inspector_open {
        INSPECTOR_WIDTH + MARGIN + 18.0
    } else {
        AREA_LEFT
    };
    let area = Rect::from_min_max(
        pos2(screen.left() + AREA_LEFT, screen.top() + AREA_TOP),
        pos2(screen.right() - right, screen.bottom() - AREA_BOTTOM),
    );
    let max_height = area.height() - LABEL_HEIGHT - PROGRESS_HEIGHT;
    if area.width() < 80.0 || max_height < 45.0 {
        hud.ui.render.frame_rect = None;
        return;
    }
    let aspect = hud.ui.render.aspect();
    let width = area.width().min(max_height * aspect);
    let height = width / aspect;
    let block_top = area.center().y - (height + LABEL_HEIGHT + PROGRESS_HEIGHT) / 2.0;
    let left = area.center().x - width / 2.0;
    hud.ui.render.frame_rect = Some([left, block_top + LABEL_HEIGHT, width, height]);
}

fn frame_of(hud: &Hud<'_>) -> Option<Rect> {
    let [x, y, width, height] = hud.ui.render.frame_rect?;
    Some(Rect::from_min_size(pos2(x, y), vec2(width, height)))
}

/// Paints the frame's labels, outline, guides, region and progress bar over the traced image.
pub(crate) fn paint(ui: &Ui, hud: &Hud<'_>) {
    let Some(frame) = frame_of(hud) else {
        return;
    };
    let render = &hud.ui.render;
    let painter = ui.painter();
    let shot = hud.i18n.tr_args(
        "shot-name",
        &[("n", fluent_bundle::FluentValue::from(render.active_shot + 1))],
    );
    painter.text(
        frame.left_top() - vec2(0.0, 7.0),
        Align2::LEFT_BOTTOM,
        format!("{shot} · {:.0} mm", render.lens_mm),
        FontId::proportional(11.0),
        theme::MUTED,
    );
    painter.text(
        frame.right_top() - vec2(0.0, 7.0),
        Align2::RIGHT_BOTTOM,
        format!("{} × {}", render.size.0, render.size.1),
        FontId::monospace(11.0),
        theme::MUTED,
    );
    painter.rect_stroke(
        frame,
        4,
        Stroke::new(1.0, Color32::from_white_alpha(41)),
        StrokeKind::Outside,
    );

    if render.guides {
        let line = Stroke::new(1.0, Color32::from_white_alpha(46));
        for step in [1.0 / 3.0, 2.0 / 3.0] {
            let x = frame.left() + frame.width() * step;
            let y = frame.top() + frame.height() * step;
            painter.vline(x, frame.y_range(), line);
            painter.hline(frame.x_range(), y, line);
        }
    }

    if let Some([x0, y0, x1, y1]) = render.region {
        let at = |fx: f32, fy: f32| pos2(frame.left() + frame.width() * fx, frame.top() + frame.height() * fy);
        let region = Rect::from_two_pos(at(x0, y0), at(x1, y1));
        let dim = Color32::from_black_alpha(120);
        for outside in [
            Rect::from_min_max(frame.min, pos2(frame.right(), region.top())),
            Rect::from_min_max(pos2(frame.left(), region.bottom()), frame.max),
            Rect::from_min_max(pos2(frame.left(), region.top()), pos2(region.left(), region.bottom())),
            Rect::from_min_max(pos2(region.right(), region.top()), pos2(frame.right(), region.bottom())),
        ] {
            painter.rect_filled(outside, 0, dim);
        }
        painter.rect_stroke(region, 0, Stroke::new(1.5, hud.accent()), StrokeKind::Inside);
    }

    if render.dof
        && render.tool == RenderTool::PickFocus
        && let Some([fx, fy]) = render.focus_marker
    {
        let centre = pos2(frame.left() + frame.width() * fx, frame.top() + frame.height() * fy);
        let stroke = Stroke::new(1.5, hud.accent());
        painter.circle_stroke(centre, 9.0, stroke);
        for (from, to) in [
            (vec2(-15.0, 0.0), vec2(-9.0, 0.0)),
            (vec2(9.0, 0.0), vec2(15.0, 0.0)),
            (vec2(0.0, -15.0), vec2(0.0, -9.0)),
            (vec2(0.0, 9.0), vec2(0.0, 15.0)),
        ] {
            painter.line_segment([centre + from, centre + to], stroke);
        }
    }

    let bar = Rect::from_min_size(pos2(frame.left(), frame.bottom() + 7.0), vec2(frame.width(), 3.0));
    painter.rect_filled(bar, 2, Color32::from_white_alpha(26));
    let filled = Rect::from_min_size(bar.min, vec2(bar.width() * render.fraction_done(), bar.height()));
    painter.rect_filled(filled, 2, hud.accent());
}

/// Pointer handling over the frame: tool drags, middle-drag pan and wheel dolly.
pub(crate) fn input(ctx: &Context, hud: &mut Hud<'_>, response: &Response, pointer: Option<Pos2>) {
    hud.editor.clear_hover();
    let frame = frame_of(hud);

    if response.hovered() {
        let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            let direction = if hud.settings.invert_zoom { -1.0 } else { 1.0 };
            let target = hud.camera.target;
            hud.camera.zoom_towards(scroll / 60.0 * direction, target);
        }
    }
    let height = frame.map_or(600.0, |frame| frame.height());
    if response.dragged_by(PointerButton::Middle) {
        pan(hud, response.drag_delta(), height);
    }

    if response.drag_started_by(PointerButton::Primary) {
        match hud.ui.render.tool {
            RenderTool::Orbit => choose_pivot(hud, pointer, frame),
            RenderTool::PickFocus => pick_focus(hud, pointer, frame),
            RenderTool::Region => {
                hud.ui.render.region_anchor = pointer.zip(frame).map(|(position, frame)| fraction(frame, position));
            }
            _ => {}
        }
    }
    if response.dragged_by(PointerButton::Primary) {
        let delta = response.drag_delta();
        match hud.ui.render.tool {
            RenderTool::Orbit => hud
                .camera
                .orbit(GlamVec2::new(delta.x, delta.y), hud.settings.orbit_style),
            RenderTool::Pan => pan(hud, delta, height),
            RenderTool::Dolly => {
                let target = hud.camera.target;
                hud.camera.zoom_towards(-delta.y / 40.0, target);
            }
            RenderTool::AimSun => {
                let render = &mut hud.ui.render;
                render.sun_turn_degrees = (render.sun_turn_degrees + delta.x * 0.4).rem_euclid(360.0);
                render.sun_height_degrees = (render.sun_height_degrees - delta.y * 0.3).clamp(0.0, 90.0);
            }
            RenderTool::Region => {
                if let (Some(anchor), Some(position), Some(frame)) = (hud.ui.render.region_anchor, pointer, frame) {
                    let now = fraction(frame, position);
                    hud.ui.render.region = Some([anchor[0], anchor[1], now[0], now[1]]);
                }
            }
            RenderTool::PickFocus => pick_focus(hud, pointer, frame),
        }
    }
    if response.clicked_by(PointerButton::Primary) && hud.ui.render.tool == RenderTool::PickFocus {
        pick_focus(hud, pointer, frame);
    }
    if response.drag_stopped_by(PointerButton::Primary) && hud.ui.render.tool == RenderTool::Region {
        hud.ui.render.region_anchor = None;
        if hud.ui.render.pixel_region().is_none_or(|region| {
            let [width, height] = [region[2] - region[0], region[3] - region[1]];
            width < 8 || height < 8
        }) {
            hud.ui.render.region = None;
        }
    }
}

/// Position of `point` inside `frame` as fractions, clamped to the frame.
fn fraction(frame: Rect, point: Pos2) -> [f32; 2] {
    [
        ((point.x - frame.left()) / frame.width()).clamp(0.0, 1.0),
        ((point.y - frame.top()) / frame.height()).clamp(0.0, 1.0),
    ]
}

/// Pans with the lens of the render camera, so the model follows the pointer.
fn pan(hud: &mut Hud<'_>, delta: egui::Vec2, viewport_height: f32) {
    let mut camera = hud.ui.render.render_camera(hud.camera);
    camera.pan(GlamVec2::new(delta.x, delta.y), viewport_height);
    hud.camera.target = camera.target;
}

/// Sets the focus distance to the depth of the point under the pointer and turns depth of field on. Dragging
/// scrubs the focus over the model.
fn pick_focus(hud: &mut Hud<'_>, pointer: Option<Pos2>, frame: Option<Rect>) {
    let Some((position, frame)) = pointer.zip(frame) else {
        return;
    };
    let Some(point) = point_under(hud, frame, position) else {
        return;
    };
    let camera = &*hud.camera;
    let forward = (camera.target - camera.eye()).normalize_or_zero();
    let depth = (point - camera.eye()).dot(forward);
    if depth <= 0.0 {
        return;
    }
    let render = &mut hud.ui.render;
    render.focus_m = Some(depth * hud.editor.scene.voxel_size());
    render.dof = true;
    render.focus_marker = Some(fraction(frame, position));
}

/// Orbit pivot at the start of a drag: the model's centre, or the point under the cursor.
fn choose_pivot(hud: &mut Hud<'_>, pointer: Option<Pos2>, frame: Option<Rect>) {
    let pivot = match hud.ui.render.around {
        OrbitAround::Model => hud
            .editor
            .scene
            .world_volume(NodeId::ROOT)
            .map(|volume| (volume.min.as_vec3() + (volume.max + 1).as_vec3()) / 2.0),
        OrbitAround::Cursor => pointer
            .zip(frame)
            .and_then(|(position, frame)| point_under(hud, frame, position)),
    };
    if let Some(pivot) = pivot {
        hud.camera.retarget(pivot);
    }
}

/// The scene point under `position`, seen through the render camera.
fn point_under(hud: &Hud<'_>, frame: Rect, position: Pos2) -> Option<Vec3> {
    if !frame.contains(position) {
        return None;
    }
    let ndc = GlamVec2::new(
        (position.x - frame.left()) / frame.width() * 2.0 - 1.0,
        1.0 - (position.y - frame.top()) / frame.height() * 2.0,
    );
    let ray = hud.ui.render.render_camera(hud.camera).ray(ndc, hud.ui.render.aspect());
    let hit = pick_scene(&hud.editor.scene, ray, 4000.0, NodeId::ROOT)?.world;
    Some(match hit.target {
        HitTarget::Voxel => hit.voxel.as_vec3() + 0.5,
        HitTarget::Ground => {
            let direction = ray.direction.normalize_or_zero();
            if direction.y.abs() < 1e-6 {
                return None;
            }
            ray.origin + direction * (-ray.origin.y / direction.y)
        }
    })
}

/// Applies one Render-mode shortcut from the keymap.
pub(crate) fn shortcut(hud: &mut Hud<'_>, action: Action) {
    match action {
        Action::RenderTool(tool) => hud.ui.render.tool = tool,
        Action::RenderStart => render_bottom::toggle_render(hud),
        Action::ClearRenderRegion => hud.ui.render.region = None,
        _ => {}
    }
}
