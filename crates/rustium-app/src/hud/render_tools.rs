//! Render mode, top centre and left: Lens and orbit pivot, and the camera and light tools.

use egui::{Align2, Color32, Context, DragValue, FontId, Sense, Stroke, vec2};

use super::widgets::{self, Look};
use super::{Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, centre_budget, fit_key, fit_level, island};
use crate::keymap::Action;
use crate::render_mode::{LENS_RANGE_MM, OrbitAround, RenderTool};
use crate::theme;

/// Top-centre island: name of the active tool, Lens and what the camera orbits around.
pub(crate) fn options(ctx: &Context, hud: &mut Hud<'_>) {
    // Level 1 drops the Tool name, level 2 also the labels.
    let budget = centre_budget(ctx, hud, "top-left", "top-right");
    let key = fit_key(&[
        &hud.ui.render.tool,
        &hud.settings.language,
        &hud.settings.ui_scale_percent,
    ]);
    let level = fit_level(ctx, hud, "render-options", budget, key, 2);
    island(
        ctx,
        hud,
        "render-options",
        Place::Anchor(Align2::CENTER_TOP, vec2(0.0, MARGIN)),
        Intro {
            delay: 0.06,
            from: vec2(0.0, -16.0),
        },
        theme::island().inner_margin(egui::Margin {
            left: 12,
            right: 6,
            top: 4,
            bottom: 4,
        }),
        |ui, hud| {
            ui.set_height(TOP_BAR_HEIGHT - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if level == 0 {
                    tool_chip(ui, hud);
                    widgets::divider(ui, 32.0);
                }
                if level < 2 {
                    ui.label(widgets::muted(hud.tr("lens")));
                }
                ui.add(
                    DragValue::new(&mut hud.ui.render.lens_mm)
                        .range(LENS_RANGE_MM.0..=LENS_RANGE_MM.1)
                        .speed(0.3)
                        .suffix(" mm")
                        .max_decimals(0),
                );
                widgets::divider(ui, 32.0);
                if level < 2 {
                    ui.label(widgets::muted(hud.tr("around")));
                }
                let options = [
                    (OrbitAround::Model, hud.tr("around-model")),
                    (OrbitAround::Cursor, hud.tr("around-cursor")),
                ];
                for (around, label) in options {
                    let look = if hud.ui.render.around == around {
                        Look::On
                    } else {
                        Look::Plain
                    };
                    if widgets::text_button(ui, &label, 32.0, look, true).clicked() {
                        hud.ui.render.around = around;
                    }
                }
            });
        },
    );
}

/// The active tool as a chip: its icon and name on a light background.
fn tool_chip(ui: &mut egui::Ui, hud: &Hud<'_>) {
    let tool = hud.ui.render.tool;
    let galley = ui
        .painter()
        .layout_no_wrap(hud.tr(tool.message_key()), theme::bold(12.0), Color32::WHITE);
    let width = (12.0 + 16.0 + 8.0 + galley.size().x + 14.0).max(112.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 7, theme::ACTIVE);
    tool.icon().paint(
        painter,
        egui::Rect::from_center_size(rect.left_center() + vec2(20.0, 0.0), vec2(16.0, 16.0)),
        Color32::WHITE,
    );
    painter.galley(
        rect.left_center() + vec2(36.0, -galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
}

/// Left island: Orbit, Pan, Dolly, Pick focus, Aim sun and Render region.
pub(crate) fn toolbar(ctx: &Context, hud: &mut Hud<'_>) {
    island(
        ctx,
        hud,
        "render-toolbar",
        Place::Anchor(Align2::LEFT_CENTER, vec2(MARGIN, 0.0)),
        Intro {
            delay: 0.18,
            from: vec2(-16.0, 0.0),
        },
        theme::island(),
        |ui, hud| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.vertical(|ui| {
                for tool in RenderTool::ALL {
                    if tool.starts_group() {
                        let (rect, _) = ui.allocate_exact_size(vec2(44.0, 7.0), egui::Sense::hover());
                        ui.painter().hline(
                            rect.x_range().shrink(8.0),
                            rect.center().y,
                            Stroke::new(1.0, theme::BORDER),
                        );
                    }
                    let look = if hud.ui.render.tool == tool {
                        Look::Accent
                    } else {
                        Look::Plain
                    };
                    let response = widgets::icon_button(ui, tool.icon(), 44.0, look, true);
                    let key = hud.settings.keymap.label(Action::RenderTool(tool));
                    if hud.settings.shortcut_letters {
                        let color = if look == Look::Accent {
                            theme::ON_ACCENT
                        } else {
                            theme::MUTED
                        };
                        ui.painter().text(
                            response.rect.right_bottom() - vec2(4.0, 3.0),
                            Align2::RIGHT_BOTTOM,
                            &key,
                            FontId::monospace(9.0),
                            color.gamma_multiply(0.85),
                        );
                    }
                    widgets::side_tip(&response, &hud.tr(tool.message_key()), &key, &hud.tr(tip_key(tool)));
                    if response.clicked() {
                        hud.ui.render.tool = tool;
                    }
                }
            });
        },
    );
}

/// Message key of the one-line description shown in the tooltip.
fn tip_key(tool: RenderTool) -> &'static str {
    match tool {
        RenderTool::Orbit => "rtip-orbit",
        RenderTool::Pan => "rtip-pan",
        RenderTool::Dolly => "rtip-dolly",
        RenderTool::PickFocus => "rtip-focus",
        RenderTool::AimSun => "rtip-sun",
        RenderTool::Region => "rtip-region",
    }
}
