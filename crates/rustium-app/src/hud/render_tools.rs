//! Render mode, top centre and left: Lens and orbit pivot, and the camera and light tools.

use egui::{Align2, Context, DragValue, FontId, RichText, Stroke, vec2};

use super::widgets::{self, Look};
use super::{Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, island};
use crate::render_mode::{LENS_RANGE_MM, OrbitAround, RenderTool};
use crate::theme;

/// Top-centre island: name of the active tool, Lens and what the camera orbits around.
pub(crate) fn options(ctx: &Context, hud: &mut Hud<'_>) {
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
                let name = hud.tr(hud.ui.render.tool.message_key());
                ui.add_sized(
                    [112.0, 32.0],
                    egui::Label::new(RichText::new(name).font(theme::bold(12.0))),
                );
                widgets::divider(ui, 32.0);
                ui.label(widgets::muted(hud.tr("lens")));
                ui.add(
                    DragValue::new(&mut hud.ui.render.lens_mm)
                        .range(LENS_RANGE_MM.0..=LENS_RANGE_MM.1)
                        .speed(0.3)
                        .suffix(" mm")
                        .max_decimals(0),
                );
                widgets::divider(ui, 32.0);
                ui.label(widgets::muted(hud.tr("around")));
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
                    let available = tool.is_available();
                    let response = widgets::icon_button(ui, tool.icon(), 44.0, look, available);
                    let key = format!("{:?}", tool.key());
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
                    let tooltip = format!("{} ({key})", hud.tr(tool.message_key()));
                    if available {
                        if response.on_hover_text(tooltip).clicked() {
                            hud.ui.render.tool = tool;
                        }
                    } else {
                        response.on_disabled_hover_text(format!("{tooltip} · {}", hud.tr("later-milestone")));
                    }
                }
            });
        },
    );
}
