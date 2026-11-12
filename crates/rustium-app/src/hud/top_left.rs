//! Top-left island: logo (opens the start screen), main menu and Mode switch. Dragging it moves the window.

use egui::load::SizedTexture;
use egui::{Align2, Context, Image, PopupCloseBehavior, vec2};

use super::widgets::{self, Look};
use super::{AppCommand, Hud, Intro, MARGIN, Mode, Place, TOP_BAR_HEIGHT, island};
use crate::icons::Icon;
use crate::keymap::Action;
use crate::settings_window::Section;
use crate::theme;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let response = island(
        ctx,
        hud,
        "top-left",
        Place::Anchor(Align2::LEFT_TOP, vec2(MARGIN, MARGIN)),
        Intro {
            delay: 0.0,
            from: vec2(0.0, -16.0),
        },
        theme::island(),
        |ui, hud| {
            ui.set_height(TOP_BAR_HEIGHT - 8.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let logo = SizedTexture::new(hud.logo.id(), vec2(32.0, 32.0));
                let brand = ui
                    .add(
                        egui::Button::image_and_text(
                            Image::from_texture(logo),
                            egui::RichText::new(hud.tr("app-name")).font(theme::bold(13.0)),
                        )
                        .frame(false),
                    )
                    .on_hover_text(hud.tr("start-screen"));
                if brand.clicked() {
                    hud.ui.splash_open = true;
                    hud.ui.splash_opened_at = None;
                }
                let menu_button =
                    widgets::icon_button(ui, Icon::Menu, 32.0, Look::Plain, true).on_hover_text(hud.tr("main-menu"));
                main_menu(&menu_button, hud);
                widgets::divider(ui, 32.0);
                let current = hud.ui.mode;
                let look = |mode| if current == mode { Look::On } else { Look::Plain };
                if widgets::text_button(ui, &hud.tr("mode-model"), 32.0, look(Mode::Model), true).clicked() {
                    hud.ui.mode = Mode::Model;
                }
                widgets::text_button(ui, &hud.tr("mode-effects"), 32.0, Look::Plain, false)
                    .on_hover_text(hud.tr("planned"));
                if widgets::text_button(ui, &hud.tr("mode-render"), 32.0, look(Mode::Render), true).clicked()
                    && hud.ui.mode != Mode::Render
                {
                    hud.editor.release();
                    hud.ui.render.enter(hud.camera);
                    hud.ui.mode = Mode::Render;
                }
            });
        },
    );
    // Empty parts of the island act as the title bar.
    let drag = response.response;
    if drag.double_clicked() {
        hud.send(AppCommand::ToggleMaximize);
    } else if drag.drag_started() {
        hud.send(AppCommand::DragWindow);
    }
}

fn main_menu(button: &egui::Response, hud: &mut Hud<'_>) {
    egui::Popup::menu(button)
        .close_behavior(PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(220.0);
            let item = |ui: &mut egui::Ui, label: String, shortcut: &str, enabled: bool| {
                ui.add_enabled(
                    enabled,
                    egui::Button::new(label)
                        .shortcut_text(shortcut)
                        .frame(false)
                        .min_size(vec2(220.0, 28.0)),
                )
            };
            if item(
                ui,
                hud.tr("menu-new"),
                &hud.settings.keymap.label(Action::NewScene),
                true,
            )
            .clicked()
            {
                hud.send(AppCommand::NewScene);
            }
            if item(
                ui,
                hud.tr("menu-open"),
                &hud.settings.keymap.label(Action::OpenFile),
                true,
            )
            .clicked()
            {
                hud.send(AppCommand::Open(None));
            }
            if item(
                ui,
                hud.tr("menu-save"),
                &hud.settings.keymap.label(Action::SaveFile),
                true,
            )
            .clicked()
            {
                hud.send(AppCommand::Save);
            }
            if item(
                ui,
                hud.tr("menu-save-as"),
                &hud.settings.keymap.label(Action::SaveFileAs),
                true,
            )
            .clicked()
            {
                hud.send(AppCommand::SaveAs);
            }
            ui.separator();
            if item(ui, hud.tr("menu-import-vox"), "", true).clicked() {
                hud.send(AppCommand::ImportVox);
            }
            if item(ui, hud.tr("menu-import-palette"), "", true).clicked() {
                hud.send(AppCommand::ImportPalette);
            }
            if item(ui, hud.tr("menu-export-vox"), "", true).clicked() {
                hud.send(AppCommand::ExportVox);
            }
            if item(ui, hud.tr("menu-export-palette"), "", true).clicked() {
                hud.send(AppCommand::ExportPalette);
            }
            ui.separator();
            if item(ui, hud.tr("menu-run-script"), "", true).clicked() {
                hud.send(AppCommand::RunScript);
            }
            ui.separator();
            if item(
                ui,
                hud.tr("menu-settings"),
                &hud.settings.keymap.label(Action::OpenSettings),
                true,
            )
            .clicked()
            {
                hud.ui.settings_section = Some(Section::General);
            }
            if item(ui, hud.tr("menu-start-screen"), "", true).clicked() {
                hud.ui.splash_open = true;
                hud.ui.splash_opened_at = None;
            }
            ui.separator();
            if item(ui, hud.tr("menu-quit"), "Alt F4", true).clicked() {
                hud.send(AppCommand::Quit);
            }
        });
}
