//! Top-centre island: name of the active Tool, Brush, Size and Mirror axes.

use egui::{Align2, Context, DragValue, RichText, vec2};
use rustium_core::Brush;

use super::widgets::{self, Look};
use super::{Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, island};
use crate::icons::Icon;
use crate::theme;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    island(
        ctx,
        hud,
        "tool-options",
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
                let name = hud.tr(hud.editor.tool.message_key());
                ui.add_sized(
                    [92.0, 32.0],
                    egui::Label::new(RichText::new(name).font(theme::bold(12.0))),
                );
                widgets::divider(ui, 32.0);
                let later = hud.tr("later-milestone");
                for (icon, key, available) in [
                    (Icon::BrushVoxel, "brush-voxel", true),
                    (Icon::BrushLine, "brush-line", false),
                    (Icon::BrushBox, "brush-box", false),
                    (Icon::BrushFace, "brush-face", false),
                ] {
                    let look = if available { Look::On } else { Look::Plain };
                    let response = widgets::icon_button(ui, icon, 32.0, look, available).on_hover_text(hud.tr(key));
                    if !available {
                        response.on_disabled_hover_text(&later);
                    }
                }
                widgets::divider(ui, 32.0);
                ui.label(widgets::muted(hud.tr("size")));
                ui.add(
                    DragValue::new(&mut hud.editor.brush.size)
                        .range(1..=Brush::MAX_SIZE)
                        .speed(0.1),
                );
                widgets::divider(ui, 32.0);
                ui.label(widgets::muted(hud.tr("mirror")));
                for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                    let look = if hud.editor.mirror[axis] { Look::On } else { Look::Plain };
                    widgets::text_button(ui, label, 32.0, look, false).on_disabled_hover_text(&later);
                }
            });
        },
    );
}
