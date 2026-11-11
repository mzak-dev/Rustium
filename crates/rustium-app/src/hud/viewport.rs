//! Pointer interaction with the 3D view: picking, Tool gestures, the marquee, the Tool wheel and camera navigation.

use egui::{Align2, Context, Id, LayerId, Order, PointerButton, Pos2, Rect, Response, Sense, Stroke, StrokeKind, vec2};
use glam::{Vec2 as GlamVec2, Vec3, Vec4};
use rustium_core::{HitTarget, NodeId};

use super::Hud;
use super::widgets::{self, Look};
use crate::editor::{SelectionEdit, Tool};
use crate::theme;

/// Radius of the ring the Tool wheel places its buttons on, in points.
const WHEEL_RADIUS: f32 = 82.0;
const WHEEL_BUTTON: f32 = 44.0;
/// Pointer travel (points) below which a press on the marquee is a click.
const CLICK_SLOP: f32 = 4.0;

/// Handles everything that happens in the 3D view once the window frame zones have been ruled out.
pub(super) fn interact(ctx: &Context, hud: &mut Hud<'_>, response: &Response, screen: Rect, pointer: Option<Pos2>) {
    if let Some(centre) = hud.ui.tool_wheel {
        hud.editor.clear_hover();
        tool_wheel(ctx, hud, centre);
        return;
    }

    let aspect = screen.width() / screen.height().max(1.0);
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

    primary_button(ctx, hud, response, screen, ray, pointer);

    if response.secondary_clicked() && !hud.editor.is_stroking() {
        hud.ui.tool_wheel = pointer;
    }

    // Camera: middle drag orbits, Shift + middle drag pans, the wheel zooms towards the cursor.
    if response.dragged_by(PointerButton::Middle) {
        let delta = response.drag_delta();
        if ctx.input(|input| input.modifiers.shift) {
            hud.camera.pan(GlamVec2::new(delta.x, delta.y), screen.height());
        } else {
            hud.camera
                .orbit(GlamVec2::new(delta.x, delta.y), hud.settings.orbit_style);
        }
    }
    if response.hovered() {
        let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            let direction = if hud.settings.invert_zoom { -1.0 } else { 1.0 };
            hud.camera.zoom_towards(scroll / 60.0 * direction, zoom_focus);
        }
    }
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
    let marquee_tool = hud.editor.tool == Tool::SelectBox && !hud.editor.focus_is_model();

    if marquee_tool {
        if pressed && response.hovered() {
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

/// Selects the children of the Focus whose centre falls inside `rect` (screen points).
fn marquee_select(hud: &mut Hud<'_>, rect: Rect, screen: Rect, extend: bool) {
    let aspect = screen.width() / screen.height().max(1.0);
    let view_proj = hud.camera.view_proj(aspect);
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
            let clip = view_proj * Vec4::new(centre.x, centre.y, centre.z, 1.0);
            if clip.w <= 0.0 {
                return false;
            }
            let ndc = clip.truncate() / clip.w;
            let point = Pos2::new(
                screen.min.x + (ndc.x * 0.5 + 0.5) * screen.width(),
                screen.min.y + (0.5 - ndc.y * 0.5) * screen.height(),
            );
            rect.contains(point)
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

/// The radial Tool menu opened with a right click.
fn tool_wheel(ctx: &Context, hud: &mut Hud<'_>, centre: Pos2) {
    let screen = ctx.content_rect();
    let margin = WHEEL_RADIUS + WHEEL_BUTTON;
    let centre = Pos2::new(
        centre.x.clamp(
            screen.min.x + margin,
            (screen.max.x - margin).max(screen.min.x + margin),
        ),
        centre.y.clamp(
            screen.min.y + margin,
            (screen.max.y - margin).max(screen.min.y + margin),
        ),
    );
    let mut chosen = None;
    egui::Area::new(Id::unique("tool-wheel"))
        .order(Order::Foreground)
        .fixed_pos(centre - vec2(margin, margin))
        .show(ctx, |ui| {
            let (_, _) = ui.allocate_exact_size(vec2(margin * 2.0, margin * 2.0), Sense::hover());
            ui.painter()
                .circle_filled(centre, WHEEL_RADIUS + WHEEL_BUTTON / 2.0 + 8.0, theme::PANEL);
            ui.painter().circle_stroke(
                centre,
                WHEEL_RADIUS + WHEEL_BUTTON / 2.0 + 8.0,
                Stroke::new(1.0, theme::BORDER),
            );
            ui.painter().text(
                centre,
                Align2::CENTER_CENTER,
                hud.tr(hud.editor.tool.message_key()),
                theme::bold(12.0),
                theme::TEXT,
            );
            for (index, tool) in Tool::ALL.into_iter().enumerate() {
                let angle =
                    -std::f32::consts::FRAC_PI_2 + index as f32 / Tool::ALL.len() as f32 * std::f32::consts::TAU;
                let position = centre + vec2(angle.cos(), angle.sin()) * WHEEL_RADIUS;
                let look = if hud.editor.tool == tool {
                    Look::Accent
                } else {
                    Look::Plain
                };
                let rect = Rect::from_center_size(position, vec2(WHEEL_BUTTON, WHEEL_BUTTON));
                let button = ui.put(rect, |ui: &mut egui::Ui| {
                    widgets::icon_button(ui, tool.icon(), WHEEL_BUTTON, look, true)
                });
                if button.on_hover_text(hud.tr(tool.message_key())).clicked() {
                    chosen = Some(tool);
                }
            }
        });
    if let Some(tool) = chosen {
        hud.editor.tool = tool;
        hud.ui.tool_wheel = None;
        return;
    }
    let outside = ctx.input(|input| {
        let away = input
            .pointer
            .interact_pos()
            .is_none_or(|position| position.distance(centre) > margin);
        (input.pointer.any_pressed() && away) || input.key_pressed(egui::Key::Escape)
    });
    if outside {
        hud.ui.tool_wheel = None;
    }
}
