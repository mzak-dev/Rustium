//! Top-centre island: name of the active Tool and the options that Tool uses (Brush, Size and Mirror axes).
//!
//! The island never overlaps the corner islands: when the window is narrow it drops the Tool name, then the
//! labels, then folds the four Brush buttons into one.

use egui::{Align2, Color32, Context, DragValue, Rect, Sense, Ui, Vec2, vec2};
use rustium_core::{Brush, BrushKind, FillMode};

use super::widgets::{self, Look};
use super::{Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, centre_budget, fit_key, fit_level, island};
use crate::editor::Tool;
use crate::icons::Icon;
use crate::theme;

/// The most compact layout.
const MAX_LEVEL: u8 = 3;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let budget = centre_budget(ctx, hud, "top-left", "top-right");
    let key = fit_key(&[&hud.editor.tool, &hud.settings.language, &hud.settings.ui_scale_percent]);
    let level = fit_level(ctx, hud, "tool-options", budget, key, MAX_LEVEL);
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
            left: 6,
            right: 6,
            top: 4,
            bottom: 4,
        }),
        |ui, hud| {
            ui.set_height(TOP_BAR_HEIGHT - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let tool = hud.editor.tool;
                let ready = hud.editor.tool_ready();
                let name = hud.tr(tool.message_key());
                let color = if ready { theme::TEXT } else { theme::DISABLED };
                let pill = tool_pill(ui, tool.icon(), &name, level == 0, color);
                if !ready {
                    pill.on_hover_text(hud.tr("tool-needs-model"));
                } else if level > 0 {
                    pill.on_hover_text(&name);
                }
                if tool == Tool::Fill {
                    widgets::divider(ui, 32.0);
                    let modes = [
                        (FillMode::Surface, hud.tr("fill-surface")),
                        (FillMode::Volume, hud.tr("fill-volume")),
                    ];
                    widgets::segmented(ui, &mut hud.editor.fill_mode, &modes, true);
                    return;
                }
                // Only the Tools that paint with a Brush have a Brush, a Size and Mirror axes.
                if tool.voxel_tool().is_none() {
                    return;
                }
                widgets::divider(ui, 32.0);
                brush_buttons(ui, hud, level >= MAX_LEVEL);
                widgets::divider(ui, 32.0);
                if level < 2 {
                    ui.label(widgets::muted(hud.tr("size")));
                }
                ui.add(
                    DragValue::new(&mut hud.editor.brush.size)
                        .range(1..=Brush::MAX_SIZE)
                        .speed(0.1),
                )
                .on_hover_text(hud.tr("size"));
                widgets::divider(ui, 32.0);
                if level < 2 {
                    ui.label(widgets::muted(hud.tr("mirror")));
                }
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

/// The four Brush shapes, or just the current one (a click picks the next) when there is no room.
fn brush_buttons(ui: &mut Ui, hud: &mut Hud<'_>, folded: bool) {
    let brushes = [
        (Icon::BrushVoxel, "brush-voxel", BrushKind::Voxel),
        (Icon::BrushLine, "brush-line", BrushKind::Line),
        (Icon::BrushBox, "brush-box", BrushKind::Box),
        (Icon::BrushFace, "brush-face", BrushKind::Face),
    ];
    if folded {
        let index = brushes
            .iter()
            .position(|(_, _, kind)| *kind == hud.editor.brush.kind)
            .unwrap_or(0);
        let (icon, key, _) = brushes[index];
        if widgets::icon_button(ui, icon, 32.0, Look::On, true)
            .on_hover_text(hud.tr(key))
            .clicked()
        {
            hud.editor.brush.kind = brushes[(index + 1) % brushes.len()].2;
        }
        return;
    }
    for (icon, key, kind) in brushes {
        let look = if hud.editor.brush.kind == kind {
            Look::On
        } else {
            Look::Plain
        };
        if widgets::icon_button(ui, icon, 32.0, look, true)
            .on_hover_text(hud.tr(key))
            .clicked()
        {
            hud.editor.brush.kind = kind;
        }
    }
}

/// The highlighted chip with the Tool's icon and, when there is room, its name.
fn tool_pill(ui: &mut Ui, icon: Icon, name: &str, show_name: bool, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(name.to_owned(), theme::bold(12.0), color);
    let width = if show_name {
        8.0 + 18.0 + 8.0 + galley.size().x + 10.0
    } else {
        32.0
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    ui.painter().rect_filled(rect, 7, Color32::from_white_alpha(20));
    let icon_rect = if show_name {
        Rect::from_min_size(rect.left_center() + vec2(8.0, -9.0), Vec2::splat(18.0))
    } else {
        Rect::from_center_size(rect.center(), Vec2::splat(18.0))
    };
    icon.paint(ui.painter(), icon_rect, color);
    if show_name {
        ui.painter()
            .galley(rect.left_center() + vec2(34.0, -galley.size().y / 2.0), galley, color);
    }
    response
}
