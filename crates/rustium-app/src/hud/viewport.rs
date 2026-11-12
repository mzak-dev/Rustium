//! Pointer interaction with the 3D view: picking, Tool gestures, the marquee and camera navigation.

use egui::{Context, Id, Key, LayerId, Order, Pos2, Rect, Response, Stroke, StrokeKind};
use glam::{Vec2 as GlamVec2, Vec3, Vec4};
use rustium_core::{HitTarget, NodeId};

use super::Hud;
use crate::editor::{SelectMode, SelectionEdit, Tool};
use crate::mouse_map::Gesture;

/// Pointer travel (points) below which a press on the marquee is a click.
const CLICK_SLOP: f32 = 4.0;

/// Handles everything that happens in the 3D view once the window frame zones have been ruled out.
pub(super) fn interact(ctx: &Context, hud: &mut Hud<'_>, response: &Response, screen: Rect, pointer: Option<Pos2>) {
    let aspect = screen.width() / screen.height().max(1.0);
    hud.editor.shift = ctx.input(|input| input.modifiers.shift);
    let stroking = hud.editor.is_stroking() || hud.ui.marquee.is_some();
    let ray = pointer.filter(|_| response.hovered() || stroking).map(|position| {
        let ndc = GlamVec2::new(
            (position.x - screen.min.x) / screen.width() * 2.0 - 1.0,
            1.0 - (position.y - screen.min.y) / screen.height() * 2.0,
        );
        hud.camera.ray(ndc, aspect)
    });
    let mut zoom_focus = hud.camera.target;
    match ray {
        Some(ray) => {
            hud.editor.update_hover(ray);
            if let Some(hit) = &hud.editor.view_hit {
                zoom_focus = match hit.world.target {
                    HitTarget::Voxel => hit.world.voxel.as_vec3() + 0.5,
                    HitTarget::Ground => (hit.world.voxel + glam::IVec3::Y).as_vec3() + Vec3::new(0.5, 0.0, 0.5),
                };
            }
        }
        None => hud.editor.clear_hover(),
    }

    if navigate(ctx, hud, response, screen, zoom_focus) {
        hud.editor.clear_hover();
        return;
    }
    if gizmo(ctx, hud, response, screen, pointer) {
        return;
    }
    primary_button(ctx, hud, response, screen, ray, pointer);
    if response.hovered()
        && !hud.editor.is_stroking()
        && let (Some(block), Some(pointer)) = (hud.editor.tool_block(), pointer)
    {
        block_hint(ctx, hud, pointer, &hud.tr(block.message_key()));
    }

    if response.hovered() {
        let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            let direction = if hud.settings.invert_zoom { -1.0 } else { 1.0 };
            hud.camera.zoom_towards(scroll / 60.0 * direction, zoom_focus);
        }
    }
}

/// A move gizmo arrow being dragged: which axis, where the drag started and the arrow on screen at that moment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GizmoDrag {
    axis: usize,
    start: Pos2,
    /// Unit direction of the arrow on screen.
    direction: egui::Vec2,
    /// Screen points per voxel along the arrow.
    per_voxel: f32,
}

/// Length of a gizmo arrow, in points.
const GIZMO_LENGTH: f32 = 72.0;
/// How close the pointer must be to an arrow to grab it, in points.
const GIZMO_GRAB: f32 = 8.0;
const GIZMO_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(0xe8, 0x4a, 0x4a),
    egui::Color32::from_rgb(0x5c, 0xc8, 0x5a),
    egui::Color32::from_rgb(0x4a, 0x8c, 0xf0),
];

/// The move gizmo on the selection, shown with the Cursor and Move: three arrows that drag the selected voxels or
/// Nodes along X, Y or Z by whole voxels. Returns whether it took the pointer this frame.
fn gizmo(ctx: &Context, hud: &mut Hud<'_>, response: &Response, screen: Rect, pointer: Option<Pos2>) -> bool {
    if hud.ui.gizmo.is_some() && !hud.editor.is_stroking() {
        // Escape took the drag back.
        hud.ui.gizmo = None;
    }
    let shown = matches!(hud.editor.tool, Tool::SelectBox | Tool::Move);
    let Some((centre, axes)) = hud.editor.gizmo().filter(|_| shown) else {
        return false;
    };
    if hud.editor.is_stroking() && hud.ui.gizmo.is_none() {
        return false;
    }
    let aspect = screen.width() / screen.height().max(1.0);
    let view_proj = hud.camera.view_proj(aspect);
    let Some(origin) = to_screen(view_proj, screen, centre) else {
        return false;
    };
    // Each arrow on screen: its unit direction and how many points one voxel along it covers.
    let arrows: Vec<Option<(egui::Vec2, f32)>> = axes
        .iter()
        .map(|axis| {
            let along = to_screen(view_proj, screen, centre + *axis)? - origin;
            let per_voxel = along.length();
            (per_voxel > 0.5).then(|| (along / per_voxel, per_voxel))
        })
        .collect();
    let grabbed = hud.ui.gizmo.map(|drag| drag.axis);
    let hovered = pointer
        .filter(|_| grabbed.is_none() && response.hovered())
        .and_then(|at| {
            arrows
                .iter()
                .enumerate()
                .filter_map(|(axis, arrow)| {
                    let (direction, _) = (*arrow)?;
                    let along = (at - origin).dot(direction).clamp(10.0, GIZMO_LENGTH);
                    let distance = (origin + direction * along).distance(at);
                    (distance < GIZMO_GRAB).then_some((axis, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(axis, _)| axis)
        });

    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::unique("move-gizmo")));
    for (axis, arrow) in arrows.iter().enumerate() {
        let Some((direction, _)) = arrow else {
            continue;
        };
        let lit = grabbed == Some(axis) || hovered == Some(axis);
        let color = if lit { egui::Color32::WHITE } else { GIZMO_COLORS[axis] };
        let tip = origin + *direction * GIZMO_LENGTH;
        painter.line_segment(
            [origin + *direction * 10.0, tip],
            Stroke::new(if lit { 3.5 } else { 2.5 }, color),
        );
        let side = direction.rot90() * 5.5;
        painter.add(egui::Shape::convex_polygon(
            vec![tip + *direction * 12.0, tip + side, tip - side],
            color,
            Stroke::NONE,
        ));
    }
    painter.circle_filled(origin, 4.0, egui::Color32::WHITE);

    let (pressed, down) = ctx.input(|input| (input.pointer.primary_pressed(), input.pointer.primary_down()));
    if let (Some(axis), true, Some(start)) = (hovered, pressed, pointer)
        && let Some((direction, per_voxel)) = arrows[axis]
    {
        hud.editor.begin_gizmo();
        hud.ui.gizmo = Some(GizmoDrag {
            axis,
            start,
            direction,
            per_voxel,
        });
        return true;
    }
    let Some(drag) = hud.ui.gizmo else {
        return hovered.is_some();
    };
    if down {
        if let Some(at) = pointer {
            #[expect(clippy::cast_possible_truncation, reason = "a whole number of voxels on screen")]
            let steps = ((at - drag.start).dot(drag.direction) / drag.per_voxel).round() as i32;
            let mut offset = glam::IVec3::ZERO;
            offset[drag.axis] = steps;
            hud.editor.gizmo_to(offset);
        }
    } else {
        hud.editor.release();
        hud.ui.gizmo = None;
    }
    true
}

/// The camera gestures of Settings → Input (orbit, pan, fly), started by their mouse button over the view.
/// Orbit turns around the point that was under the cursor at the press (the camera target over empty space).
/// Returns whether a gesture owns the pointer, so the Tools leave it alone.
fn navigate(ctx: &Context, hud: &mut Hud<'_>, response: &Response, screen: Rect, focus: Vec3) -> bool {
    let map = hud.settings.mouse;
    if hud.ui.nav.is_none()
        && response.hovered()
        && !hud.editor.is_stroking()
        && let Some(gesture) = ctx.input(|input| map.pressed(input))
    {
        hud.ui.nav = Some(gesture);
        hud.ui.orbit_pivot = Some(focus);
    }
    let Some(gesture) = hud.ui.nav else {
        return false;
    };
    if !ctx.input(|input| map.held(input, gesture)) {
        hud.ui.nav = None;
        return false;
    }
    let delta = ctx.input(|input| input.pointer.delta());
    let delta = GlamVec2::new(delta.x, delta.y);
    match gesture {
        Gesture::Orbit => {
            let pivot = hud.ui.orbit_pivot.unwrap_or(hud.camera.target);
            hud.camera.orbit_around(delta, hud.settings.orbit_style, pivot);
        }
        Gesture::Pan => hud.camera.pan(delta, screen.height()),
        Gesture::Fly => fly(ctx, hud, delta),
    }
    true
}

/// Fly mode: the pointer looks around, W A S D move, Q and E go down and up, Shift is faster and the wheel sets the
/// speed. The keys are taken so they do not switch Tools.
fn fly(ctx: &Context, hud: &mut Hud<'_>, look: GlamVec2) {
    let (step, scroll) = ctx.input_mut(|input| {
        for key in [Key::W, Key::A, Key::S, Key::D, Key::Q, Key::E] {
            while input.consume_key(egui::Modifiers::NONE, key) || input.consume_key(egui::Modifiers::SHIFT, key) {}
        }
        let axis = |plus: Key, minus: Key| {
            f32::from(u8::from(input.key_down(plus))) - f32::from(u8::from(input.key_down(minus)))
        };
        let step = Vec3::new(axis(Key::D, Key::A), axis(Key::E, Key::Q), axis(Key::W, Key::S));
        let boost = if input.modifiers.shift { 3.0 } else { 1.0 };
        let scroll = std::mem::take(&mut input.smooth_scroll_delta.y);
        (step * input.stable_dt.min(0.1) * boost, scroll)
    });
    if scroll.abs() > f32::EPSILON {
        hud.ui.fly_speed = (hud.ui.fly_speed * 1.2_f32.powf(scroll / 60.0)).clamp(1.0, 2000.0);
    }
    hud.camera.fly(look, step * hud.ui.fly_speed);
    ctx.request_repaint();
}

fn primary_button(
    ctx: &Context,
    hud: &mut Hud<'_>,
    response: &Response,
    screen: Rect,
    ray: Option<rustium_core::Ray>,
    pointer: Option<Pos2>,
) {
    let (pressed, down, released, extend) = ctx.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
            input.modifiers.shift,
        )
    });
    // The Cursor drags what is selected and draws a marquee anywhere else.
    let layer = hud.editor.focus_is_model() && hud.editor.select_mode == SelectMode::Layer;
    if hud.editor.tool == Tool::SelectBox && !hud.editor.is_stroking() && !layer {
        if pressed && response.hovered() {
            if hud.editor.over_selection() {
                if let Some(ray) = ray {
                    hud.editor.press(ray, extend);
                }
                return;
            }
            hud.ui.marquee = pointer;
        }
        if let (Some(start), Some(now)) = (hud.ui.marquee, pointer) {
            if down {
                draw_marquee(ctx, hud, Rect::from_two_pos(start, now));
            }
            if released || !down {
                hud.ui.marquee = None;
                if start.distance(now) < CLICK_SLOP {
                    if let Some(ray) = ray {
                        hud.editor.press(ray, extend);
                    }
                } else {
                    marquee_select(hud, Rect::from_two_pos(start, now), screen, extend);
                }
            }
        }
        return;
    }

    if pressed
        && response.hovered()
        && let Some(ray) = ray
    {
        hud.editor.press(ray, extend);
        // Pick color also puts the color code on the clipboard.
        if hud.editor.tool == Tool::Pick
            && let Some(color) = hud.editor.scene.palette().get(hud.editor.color())
        {
            let [r, g, b] = color.color;
            ctx.copy_text(super::hotbar::hex(egui::Color32::from_rgb(r, g, b)));
        }
    } else if down
        && hud.editor.is_stroking()
        && let Some(ray) = ray
    {
        hud.editor.drag(ray);
    }
    if released || (!down && hud.editor.is_stroking()) {
        hud.editor.release();
    }
}

/// A short note beside the pointer saying why the Tool would do nothing there.
fn block_hint(ctx: &Context, hud: &Hud<'_>, pointer: Pos2, text: &str) {
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("tool-block-hint")));
    let galley = painter.layout_no_wrap(text.to_owned(), crate::theme::bold(11.0), crate::theme::TEXT);
    let rect = Rect::from_min_size(pointer + egui::vec2(18.0, 14.0), galley.size()).expand2(egui::vec2(7.0, 4.0));
    painter.rect(
        rect,
        6,
        egui::Color32::from_black_alpha(200),
        Stroke::new(1.0, hud.accent().gamma_multiply(0.5)),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + egui::vec2(7.0, 4.0), galley, crate::theme::TEXT);
}

fn draw_marquee(ctx: &Context, hud: &Hud<'_>, rect: Rect) {
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("marquee")));
    let accent = hud.accent();
    painter.rect(
        rect,
        2,
        accent.gamma_multiply(0.12),
        Stroke::new(1.0, accent),
        StrokeKind::Inside,
    );
}

/// Where `point` of the Scene lands on screen, `None` behind the camera.
fn to_screen(view_proj: glam::Mat4, screen: Rect, point: Vec3) -> Option<Pos2> {
    let clip = view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Pos2::new(
        screen.min.x + (ndc.x * 0.5 + 0.5) * screen.width(),
        screen.min.y + (0.5 - ndc.y * 0.5) * screen.height(),
    ))
}

/// Selects what lies inside `rect` (screen points): voxels of the Focus Model, or children of the Focus Group whose
/// centre falls inside.
fn marquee_select(hud: &mut Hud<'_>, rect: Rect, screen: Rect, extend: bool) {
    let aspect = screen.width() / screen.height().max(1.0);
    let view_proj = hud.camera.view_proj(aspect);
    if hud.editor.focus_is_model() {
        let camera = hud.camera.clone();
        hud.editor.marquee_voxels(
            |point| to_screen(view_proj, screen, point).is_some_and(|at| rect.contains(at)),
            |point| {
                let at = to_screen(view_proj, screen, point).unwrap_or(screen.center());
                let ndc = GlamVec2::new(
                    (at.x - screen.min.x) / screen.width() * 2.0 - 1.0,
                    1.0 - (at.y - screen.min.y) / screen.height() * 2.0,
                );
                camera.ray(ndc, aspect)
            },
            extend,
        );
        return;
    }
    let scene = &hud.editor.scene;
    let focus = hud.editor.focus;
    let inside: Vec<NodeId> = scene
        .node(focus)
        .map(|node| node.children().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|child| !scene.is_hidden(*child) && !scene.is_locked(*child))
        .filter(|child| {
            let Some(volume) = scene.world_volume(*child) else {
                return false;
            };
            let centre = (volume.min.as_vec3() + (volume.max + glam::IVec3::ONE).as_vec3()) / 2.0;
            to_screen(view_proj, screen, centre).is_some_and(|point| rect.contains(point))
        })
        .collect();
    hud.editor.select_nodes(
        &inside,
        if extend {
            SelectionEdit::Add
        } else {
            SelectionEdit::Replace
        },
    );
}
