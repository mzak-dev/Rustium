//! Top-right island: Undo/Redo, grid and shading, and the window buttons of the frameless window.

use egui::{Align2, Context, vec2};
use rustium_render::RenderPass;

use super::widgets::{self, Look};
use super::{AppCommand, Hud, Intro, MARGIN, Mode, Place, TOP_BAR_HEIGHT, island};
use crate::camera::Projection;
use crate::icons::Icon;
use crate::keymap::Action;
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
                    .on_hover_text(format!("{}{}", hud.tr("undo"), hud.settings.keymap.hint(Action::Undo)))
                    .clicked()
                {
                    hud.editor.undo();
                }
                if widgets::icon_button(ui, Icon::Redo, 32.0, Look::Plain, can_redo)
                    .on_hover_text(format!("{}{}", hud.tr("redo"), hud.settings.keymap.hint(Action::Redo)))
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
                } else if hud.ui.mode == Mode::Effects {
                    let look = if hud.ui.effects.snap { Look::On } else { Look::Plain };
                    if widgets::icon_button(ui, Icon::Grid, 32.0, look, true)
                        .on_hover_text(hud.tr("snap-nodes"))
                        .clicked()
                    {
                        hud.ui.effects.snap = !hud.ui.effects.snap;
                    }
                } else {
                    let grid_look = if hud.settings.show_grid { Look::On } else { Look::Plain };
                    let grid = widgets::icon_button(ui, Icon::Grid, 32.0, grid_look, true);
                    if widgets::tip(grid, &hud.tr("grid"), &hud.tr("grid-tip")).clicked() {
                        hud.settings.show_grid = !hud.settings.show_grid;
                        hud.send(AppCommand::SettingsChanged);
                    }
                    widgets::divider(ui, 32.0);
                    for (projection, icon, key) in [
                        (Projection::Perspective, Icon::Perspective, "projection-perspective"),
                        (Projection::Orthographic, Icon::Orthographic, "projection-orthographic"),
                    ] {
                        let look = if hud.camera.projection == projection {
                            Look::On
                        } else {
                            Look::Plain
                        };
                        let button = widgets::icon_button(ui, icon, 32.0, look, true);
                        let tip = format!("{key}-tip");
                        if widgets::tip(button, &hud.tr(key), &hud.tr(&tip)).clicked() {
                            hud.camera.projection = projection;
                        }
                    }
                }
                // In the browser the page is the window: the tab has its own buttons.
                if crate::DESKTOP {
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
                }
            });
        },
    );
}
