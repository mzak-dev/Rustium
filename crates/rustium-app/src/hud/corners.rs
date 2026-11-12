//! Bottom corners: mouse hints on the left, cursor position and model size on the right.

use egui::{Align2, Context, RichText, vec2};
use fluent_bundle::FluentValue;

use super::{Hud, Intro, Place, corner, corner_budget, fit_key, fit_level, island, widgets};
use crate::editor::Tool;
use crate::theme;

/// Level 1 drops the Pan hint and the cursor position, level 2 keeps two hints and only the model size.
const MAX_LEVEL: u8 = 2;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let budget = corner_budget(ctx);
    let key = fit_key(&[&hud.settings.language, &hud.settings.ui_scale_percent, &hud.editor.tool]);
    let hints_level = fit_level(ctx, hud, "hints", budget, key, MAX_LEVEL);
    let readout_level = fit_level(ctx, hud, "readout", budget, key, MAX_LEVEL);
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
        "hints",
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
                let mouse = hud.settings.mouse;
                let all = [
                    ("LMB".to_owned(), "hint-place"),
                    (format!("{} WASD", mouse.fly), "hint-fly"),
                    (mouse.orbit.to_string(), "hint-orbit"),
                    (mouse.pan.to_string(), "hint-pan"),
                ];
                let shown = match hints_level {
                    0 => 4,
                    1 => 3,
                    _ => 2,
                };
                for (button, key) in all.into_iter().take(shown) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(widgets::mono(button.as_str(), 11.0).color(theme::TEXT));
                        ui.label(RichText::new(hud.tr(key)).color(theme::MUTED).size(11.0));
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
        "readout",
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
                if readout_level == 0 {
                    let position = hud.editor.hover_cell();
                    let position = position.map_or_else(
                        || "X –  Y –  Z –".to_owned(),
                        |p| format!("X {}  Y {}  Z {}", p.x, p.y, p.z),
                    );
                    ui.label(widgets::mono(position, 11.0));
                }
                if let Some((_, _, distance)) = hud.editor.distance().filter(|_| hud.editor.tool == Tool::Measure) {
                    let delta = distance.delta;
                    ui.label(widgets::mono(
                        format!(
                            "Δ {} {} {}  ·  {:.2} vox  ·  {:.2} m",
                            delta.x, delta.y, delta.z, distance.voxels, distance.metres
                        ),
                        11.0,
                    ));
                } else if let Some(volume) = hud.editor.scene.world_volume(hud.editor.focus) {
                    let size = volume.size();
                    ui.label(widgets::mono(format!("{} × {} × {}", size.x, size.y, size.z), 11.0));
                }
                if readout_level < 2 {
                    let centimetres = hud.editor.scene.voxel_size() * 100.0;
                    let size = format!("{} cm", (centimetres * 100.0).round() / 100.0);
                    ui.label(widgets::mono(
                        hud.i18n.tr_args("per-voxel", &[("size", FluentValue::from(size))]),
                        11.0,
                    ));
                }
            });
        },
    );
}
