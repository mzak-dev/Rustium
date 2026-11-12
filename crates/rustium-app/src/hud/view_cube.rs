//! The view cube next to the inspector: drag to orbit, click a face to look straight at it.

use egui::{Color32, Context, Id, Pos2, Sense, Shape, Stroke, pos2, vec2};
use glam::camera::rh::view;
use glam::{Vec2 as GlamVec2, Vec3};
use rustium_core::Face;

use super::{Hud, INSPECTOR_WIDTH, MARGIN, TOP_BAR_HEIGHT};
use crate::settings::Motion;

const SIZE: egui::Vec2 = vec2(76.0, 72.0);
const SCALE: f32 = 36.0;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let screen = ctx.content_rect();
    let duration = if hud.settings.motion == Motion::Off { 0.0 } else { 0.3 };
    let openness = ctx.animate_bool_with_time(Id::unique("view-cube-shift"), hud.ui.inspector_open, duration);
    let right_gap = 16.0 + openness * (INSPECTOR_WIDTH + MARGIN - 4.0);
    let position = pos2(screen.right() - right_gap - SIZE.x, MARGIN + TOP_BAR_HEIGHT + 10.0);

    egui::Area::new(Id::unique("view-cube"))
        .order(egui::Order::Middle)
        .fixed_pos(position)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(SIZE, Sense::click_and_drag());
            let response = response.on_hover_text(hud.tr("view-cube"));
            let camera = &*hud.camera;
            let direction = (camera.eye() - camera.target).normalize_or_zero();
            let up = if camera.pitch.cos() >= 0.0 {
                Vec3::Y
            } else {
                Vec3::NEG_Y
            };
            let rotation = view::look_at_mat4(direction, Vec3::ZERO, up);
            let centre = rect.center();
            let project = |point: Vec3| -> Pos2 {
                let p = rotation.transform_point3(point);
                centre + vec2(p.x, -p.y) * SCALE
            };

            // Visible faces, far to near.
            let mut faces: Vec<(Face, f32, [Pos2; 4])> = Face::ALL
                .iter()
                .filter_map(|face| {
                    let normal = face.normal().as_vec3();
                    let facing = rotation.transform_vector3(normal).z;
                    (facing > 0.02).then(|| {
                        let axis = face.axis();
                        let u = unit((axis + 1) % 3) * 0.5;
                        let v = unit((axis + 2) % 3) * 0.5;
                        let middle = normal * 0.5;
                        let corners = [middle - u - v, middle + u - v, middle + u + v, middle - u + v].map(&project);
                        (*face, facing, corners)
                    })
                })
                .collect();
            faces.sort_by(|a, b| a.1.total_cmp(&b.1));

            let pointer = response.hover_pos();
            let hovered_face = pointer.and_then(|point| {
                faces
                    .iter()
                    .rev()
                    .find(|(_, _, quad)| contains(quad, point))
                    .map(|f| f.0)
            });
            let accent = hud.accent();
            for (face, _, quad) in &faces {
                let base = match face.axis() {
                    1 => Color32::from_rgb(0x3b, 0x40, 0x48),
                    2 => Color32::from_rgb(0x2d, 0x31, 0x38),
                    _ => Color32::from_rgb(0x24, 0x28, 0x2e),
                };
                let fill = if Some(*face) == hovered_face {
                    base.lerp_to_gamma(accent, 0.35)
                } else {
                    base
                };
                ui.painter().add(Shape::convex_polygon(
                    quad.to_vec(),
                    fill,
                    Stroke::new(1.0, Color32::from_rgb(0x7b, 0x83, 0x8e)),
                ));
                let label_centre = quad.iter().fold(egui::Vec2::ZERO, |sum, p| sum + p.to_vec2()) / 4.0;
                let area = (quad[1] - quad[0]).length() * (quad[3] - quad[0]).length();
                if area > 120.0 {
                    let colour = if face.axis() == 1 {
                        Color32::WHITE
                    } else {
                        crate::theme::SOFT
                    };
                    let text = hud.tr(label_key(*face));
                    // Shrink the label until it fits the width of the face.
                    let left = quad.iter().map(|p| p.x).fold(f32::MAX, f32::min);
                    let right = quad.iter().map(|p| p.x).fold(f32::MIN, f32::max);
                    let measured = ui
                        .painter()
                        .layout_no_wrap(text.clone(), crate::theme::bold(7.0), colour);
                    let size = (7.0 * (right - left) * 0.8 / measured.size().x.max(1.0)).min(7.0);
                    if size >= 4.5 {
                        ui.painter().text(
                            label_centre.to_pos2(),
                            egui::Align2::CENTER_CENTER,
                            text,
                            crate::theme::bold(size),
                            colour,
                        );
                    }
                }
            }

            if response.dragged() {
                let delta = response.drag_delta();
                hud.camera
                    .orbit(GlamVec2::new(delta.x, delta.y) * 1.5, hud.settings.orbit_style);
            } else if response.clicked()
                && let Some(face) = hovered_face
            {
                super::snap(hud, face);
            }
        });
}

fn unit(axis: usize) -> Vec3 {
    let mut vector = Vec3::ZERO;
    vector[axis] = 1.0;
    vector
}

fn contains(quad: &[Pos2; 4], point: Pos2) -> bool {
    let sign = |a: Pos2, b: Pos2| (b - a).x * (point - a).y - (b - a).y * (point - a).x;
    let signs = [
        sign(quad[0], quad[1]),
        sign(quad[1], quad[2]),
        sign(quad[2], quad[3]),
        sign(quad[3], quad[0]),
    ];
    signs.iter().all(|s| *s >= 0.0) || signs.iter().all(|s| *s <= 0.0)
}

fn label_key(face: Face) -> &'static str {
    match face {
        Face::PosY => "cube-top",
        Face::NegY => "cube-bottom",
        Face::PosZ => "cube-front",
        Face::NegZ => "cube-back",
        Face::PosX => "cube-right",
        Face::NegX => "cube-left",
    }
}
