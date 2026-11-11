//! Top-centre island: name of the active Tool, Brush, Size and Mirror axes.

use egui::{Align2, Context, DragValue, RichText, vec2};
use rustium_core::{Brush, BrushKind, FillMode};

use super::widgets::{self, Look};
use super::{Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, island};
use crate::editor::Tool;
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
                let ready = hud.editor.tool_ready();
                let color = if ready { theme::TEXT } else { theme::DISABLED };
                let label = ui.add_sized(
                    [92.0, 32.0],
                    egui::Label::new(RichText::new(name).font(theme::bold(12.0)).color(color)),
                );
                if !ready {
                    label.on_hover_text(hud.tr("tool-needs-model"));
                }
                widgets::divider(ui, 32.0);
                if hud.editor.tool == Tool::Fill {
                    let modes = [
                        (FillMode::Surface, hud.tr("fill-surface")),
                        (FillMode::Volume, hud.tr("fill-volume")),
                    ];
                    widgets::segmented(ui, &mut hud.editor.fill_mode, &modes, true);
                } else {
                    let brushes_enabled = hud.editor.tool.voxel_tool().is_some();
                    for (icon, key, kind) in [
                        (Icon::BrushVoxel, "brush-voxel", BrushKind::Voxel),
                        (Icon::BrushLine, "brush-line", BrushKind::Line),
                        (Icon::BrushBox, "brush-box", BrushKind::Box),
                        (Icon::BrushFace, "brush-face", BrushKind::Face),
                    ] {
                        let look = if brushes_enabled && hud.editor.brush.kind == kind {
                            Look::On
                        } else {
                            Look::Plain
                        };
                        let response =
                            widgets::icon_button(ui, icon, 32.0, look, brushes_enabled).on_hover_text(hud.tr(key));
                        if response.clicked() {
                            hud.editor.brush.kind = kind;
                        }
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
                    if widgets::text_button(ui, label, 32.0, look, true)
                        .on_hover_text(hud.tr("mirror-axes"))
                        .clicked()
                    {
                        hud.editor.mirror[axis] = !hud.editor.mirror[axis];
                    }
                }
            });
        },
    );
}
