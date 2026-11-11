//! Left island: the Tools, each with its shortcut letter.

use egui::{Align2, Context, FontId, Stroke, vec2};

use super::widgets::{self, Look};
use super::{Hud, Intro, MARGIN, Place, island};
use crate::editor::Tool;
use crate::theme;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    island(
        ctx,
        hud,
        "toolbar",
        Place::Anchor(Align2::LEFT_CENTER, vec2(MARGIN, 0.0)),
        Intro {
            delay: 0.18,
            from: vec2(-16.0, 0.0),
        },
        theme::island(),
        |ui, hud| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.vertical(|ui| {
                for tool in Tool::ALL {
                    if tool.starts_group() {
                        let (rect, _) = ui.allocate_exact_size(vec2(44.0, 7.0), egui::Sense::hover());
                        ui.painter().hline(
                            rect.x_range().shrink(8.0),
                            rect.center().y,
                            Stroke::new(1.0, theme::BORDER),
                        );
                    }
                    let look = if hud.editor.tool == tool {
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
                            hud.editor.tool = tool;
                        }
                    } else {
                        response.on_disabled_hover_text(format!("{tooltip} · {}", hud.tr("later-milestone")));
                    }
                }
            });
        },
    );
}
