//! Top-right island: Undo/Redo, grid and shading, and the window buttons of the frameless window.

use egui::{Align2, Context, vec2};

use super::widgets::{self, Look};
use super::{AppCommand, Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, island};
use crate::camera::Projection;
use crate::icons::Icon;
use crate::theme;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    island(
        ctx,
        hud,
        "top-right",
        Place::Anchor(Align2::RIGHT_TOP, vec2(-MARGIN, MARGIN)),
        Intro {
            delay: 0.12,
            from: vec2(0.0, -16.0),
        },
        theme::island(),
        |ui, hud| {
            ui.set_height(TOP_BAR_HEIGHT - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let can_undo = hud.editor.history.can_undo();
                let can_redo = hud.editor.history.can_redo();
                if widgets::icon_button(ui, Icon::Undo, 32.0, Look::Plain, can_undo)
                    .on_hover_text(hud.tr("undo"))
                    .clicked()
                {
                    hud.editor.undo();
                }
                if widgets::icon_button(ui, Icon::Redo, 32.0, Look::Plain, can_redo)
                    .on_hover_text(hud.tr("redo"))
                    .clicked()
                {
                    hud.editor.redo();
                }
                widgets::divider(ui, 32.0);
                let grid_look = if hud.settings.show_grid { Look::On } else { Look::Plain };
                if widgets::icon_button(ui, Icon::Grid, 32.0, grid_look, true)
                    .on_hover_text(hud.tr("grid"))
                    .clicked()
                {
                    hud.settings.show_grid = !hud.settings.show_grid;
                    hud.send(AppCommand::SettingsChanged);
                }
                let shading = widgets::dropdown_button(ui, &hud.tr("shading-shaded"));
                egui::Popup::menu(&shading).show(|ui| {
                    ui.set_min_width(170.0);
                    let _ = ui.selectable_label(true, hud.tr("shading-shaded"));
                    ui.separator();
                    let camera = &mut *hud.camera;
                    ui.selectable_value(
                        &mut camera.projection,
                        Projection::Perspective,
                        hud.i18n.tr("projection-perspective"),
                    );
                    ui.selectable_value(
                        &mut camera.projection,
                        Projection::Orthographic,
                        hud.i18n.tr("projection-orthographic"),
                    );
                });
                widgets::divider(ui, 32.0);
                if widgets::icon_button(ui, Icon::Minimize, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("window-minimize"))
                    .clicked()
                {
                    hud.send(AppCommand::Minimize);
                }
                if widgets::icon_button(ui, Icon::Maximize, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("window-maximize"))
                    .clicked()
                {
                    hud.send(AppCommand::ToggleMaximize);
                }
                if widgets::icon_button(ui, Icon::Close, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("window-close"))
                    .clicked()
                {
                    hud.send(AppCommand::Quit);
                }
            });
        },
    );
}
