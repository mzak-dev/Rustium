//! Top-right island: Undo/Redo, grid and shading, and the window buttons of the frameless window.

use egui::{Align2, Context, vec2};
use rustium_render::RenderPass;

use super::widgets::{self, Look};
use super::{AppCommand, Hud, Intro, MARGIN, Mode, Place, TOP_BAR_HEIGHT, island};
use crate::camera::Projection;
use crate::icons::Icon;
use crate::theme;

fn pass_key(pass: RenderPass) -> &'static str {
    match pass {
        RenderPass::Beauty => "pass-beauty",
        RenderPass::Albedo => "pass-albedo",
        RenderPass::Normal => "pass-normal",
        RenderPass::Depth => "pass-depth",
    }
}

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
                if hud.ui.mode == Mode::Render {
                    let look = if hud.ui.render.guides { Look::On } else { Look::Plain };
                    if widgets::icon_button(ui, Icon::Guides, 32.0, look, true)
                        .on_hover_text(hud.tr("frame-guides"))
                        .clicked()
                    {
                        hud.ui.render.guides = !hud.ui.render.guides;
                    }
                    let pass_name = hud.tr(pass_key(hud.ui.render.pass));
                    let pass_button = widgets::dropdown_button(ui, &pass_name).on_hover_text(hud.tr("view-pass"));
                    egui::Popup::menu(&pass_button).show(|ui| {
                        ui.set_min_width(140.0);
                        for pass in RenderPass::ALL {
                            ui.selectable_value(&mut hud.ui.render.pass, pass, hud.i18n.tr(pass_key(pass)));
                        }
                    });
                } else {
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
                }
                widgets::divider(ui, 32.0);
                if widgets::icon_button(ui, Icon::Minimize, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("window-minimize"))
                    .clicked()
                {
                    hud.send(AppCommand::Minimize);
                }
                let maximize = widgets::icon_button(ui, Icon::Maximize, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("window-maximize"));
                hud.ui.maximize_button = Some(maximize.rect);
                if maximize.clicked() {
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
