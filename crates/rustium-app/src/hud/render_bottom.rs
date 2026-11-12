//! Render mode, bottom: the Render / Pause button with Shots, and the corner hints and status readout.

use egui::{Align2, Color32, Context, FontId, Id, RichText, Sense, Stroke, StrokeKind, vec2};
use fluent_bundle::FluentValue;
use web_time::Instant;

use super::widgets::{self, Look};
use super::{AppCommand, Hud, Intro, MARGIN, Place, centre_budget, corner, corner_budget, fit_key, fit_level, island};
use crate::icons::Icon;
use crate::render_mode::{MAX_SHOTS, RenderStatus, RenderTool, format_duration};
use crate::theme;

/// Starts, pauses or restarts the render (the Render button and F5).
pub(crate) fn toggle_render(hud: &mut Hud<'_>) {
    if hud.ui.render.status() == RenderStatus::Done {
        hud.ui.render.running = true;
        hud.send(AppCommand::RestartRender);
    } else {
        hud.ui.render.running = !hud.ui.render.running;
    }
}

/// Bottom-centre island: Render / Pause, the Shots and Add shot.
pub(crate) fn shots(ctx: &Context, hud: &mut Hud<'_>) {
    // Level 1 shows only the icon on the Render button, level 2 also makes the Shot tiles narrower.
    let budget = centre_budget(ctx, hud, "render-hints", "render-readout");
    let status = hud.ui.render.status();
    let key = fit_key(&[
        &hud.settings.language,
        &hud.settings.ui_scale_percent,
        &status,
        &hud.ui.render.shots.len(),
    ]);
    let level = fit_level(ctx, hud, "render-shots", budget, key, 2);
    island(
        ctx,
        hud,
        "render-shots",
        Place::Anchor(Align2::CENTER_BOTTOM, vec2(0.0, -MARGIN)),
        Intro {
            delay: 0.3,
            from: vec2(0.0, 16.0),
        },
        theme::island().inner_margin(6),
        |ui, hud| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                render_button(ui, hud, level >= 1);
                widgets::divider(ui, 40.0);
                for index in 0..hud.ui.render.shots.len() {
                    shot_tile(ui, hud, index, if level >= 2 { 40.0 } else { 56.0 });
                }
                let can_add = hud.ui.render.shots.len() < MAX_SHOTS;
                let add = widgets::icon_button(ui, Icon::Plus, 40.0, Look::Plain, can_add);
                if can_add {
                    if add.on_hover_text(hud.tr("add-shot")).clicked() {
                        hud.ui.render.add_shot(hud.camera);
                    }
                } else {
                    add.on_disabled_hover_text(hud.tr("shots-full"));
                }
            });
        },
    );
}

fn render_button(ui: &mut egui::Ui, hud: &mut Hud<'_>, icon_only: bool) {
    let status = hud.ui.render.status();
    let (icon, key) = match status {
        RenderStatus::Rendering => (Icon::Pause, "render-pause"),
        RenderStatus::Paused => (Icon::Play, "render-resume"),
        RenderStatus::Done => (Icon::Play, "render-again"),
    };
    let label = hud.tr(key);
    let galley = ui.painter().layout_no_wrap(label, theme::bold(12.5), theme::ON_ACCENT);
    let width = if icon_only {
        44.0
    } else {
        (14.0 + 18.0 + 8.0 + galley.size().x + 8.0 + 22.0 + 14.0).max(132.0)
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    let accent = hud.accent();
    let fill = if response.hovered() {
        accent.gamma_multiply(1.1)
    } else {
        accent
    };
    let rect = if response.is_pointer_button_down_on() {
        egui::Rect::from_center_size(rect.center(), rect.size() * 0.94)
    } else {
        rect
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 8, fill);
    icon.paint(
        painter,
        egui::Rect::from_center_size(
            if icon_only {
                rect.center()
            } else {
                rect.left_center() + vec2(23.0, 0.0)
            },
            vec2(18.0, 18.0),
        ),
        theme::ON_ACCENT,
    );
    if !icon_only {
        draw_label(painter, rect, galley);
    }
    if response.on_hover_text(hud.tr("render-button-hint")).clicked() {
        toggle_render(hud);
    }
}

fn draw_label(painter: &egui::Painter, rect: egui::Rect, galley: std::sync::Arc<egui::Galley>) {
    let text_pos = rect.left_center() + vec2(40.0, -galley.size().y / 2.0);
    let text_width = galley.size().x;
    painter.galley(text_pos, galley, theme::ON_ACCENT);
    painter.text(
        egui::pos2(text_pos.x + text_width + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        "F5",
        FontId::monospace(10.0),
        theme::ON_ACCENT,
    );
}

/// The name shown for Shot `index`: the one the user gave it, else "Shot n".
pub(crate) fn shot_label(hud: &Hud<'_>, index: usize) -> String {
    hud.ui
        .render
        .shots
        .get(index)
        .and_then(|shot| shot.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(
            || hud.i18n.tr_args("shot-name", &[("n", FluentValue::from(index + 1))]),
            str::to_owned,
        )
}

fn shot_tile(ui: &mut egui::Ui, hud: &mut Hud<'_>, index: usize, width: f32) {
    let selected = index == hud.ui.render.active_shot;
    let modified = hud.ui.render.shot_modified(index, hud.camera);
    let lift = ui
        .ctx()
        .animate_bool_with_time(Id::unique(("shot-lift", index)), selected, 0.18);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    let rect = rect.translate(vec2(0.0, -6.0 * lift));
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect.translate(vec2(0.0, 8.0)), 7, Color32::from_black_alpha(90));
        painter.rect(rect.expand(2.0), 9, Color32::WHITE, Stroke::NONE, StrokeKind::Inside);
    }
    painter.rect(
        rect,
        7,
        Color32::from_rgb(0xbf, 0xc3, 0xc7),
        Stroke::new(1.0, Color32::from_black_alpha(128)),
        StrokeKind::Inside,
    );
    let dark = Color32::from_rgb(0x2a, 0x2d, 0x32);
    let icon = hud.ui.render.shots.get(index).map_or(Icon::Cube, |shot| shot.icon);
    icon.paint(
        painter,
        egui::Rect::from_center_size(rect.center() + vec2(4.0, 2.0), vec2(18.0, 18.0)),
        dark,
    );
    painter.text(
        rect.left_top() + vec2(4.0, 2.0),
        Align2::LEFT_TOP,
        (index + 1).to_string(),
        FontId::monospace(9.0),
        Color32::from_rgb(0x1a, 0x1d, 0x22),
    );
    if modified {
        painter.circle_filled(rect.right_top() + vec2(-6.0, 6.0), 3.0, hud.accent());
    }
    let size = hud.ui.render.shots.get(index).map_or((0, 0), |shot| shot.size);
    let mut tooltip = hud.i18n.tr_args(
        "shot-tooltip",
        &[
            ("name", FluentValue::from(shot_label(hud, index))),
            ("size", FluentValue::from(format!("{} × {}", size.0, size.1))),
        ],
    );
    if modified {
        tooltip.push('\n');
        tooltip.push_str(&hud.tr("shot-modified"));
    }
    let response = response.on_hover_text(tooltip);
    if response.clicked() && (!selected || modified) {
        hud.ui.render.select_shot(index, hud.camera);
    }
    shot_menu(&response, hud, index, modified);
}

/// Right-click menu of a Shot tile: name, update from the live view, delete.
fn shot_menu(response: &egui::Response, hud: &mut Hud<'_>, index: usize, modified: bool) {
    let rename_label = hud.tr("shot-rename");
    let update_label = hud.tr("shot-update");
    let delete_label = hud.tr("shot-delete");
    let unchanged_hint = hud.tr("shot-update-unchanged");
    response.context_menu(|ui| {
        ui.set_min_width(190.0);
        ui.label(RichText::new(rename_label).color(theme::SUBTLE).size(10.0));
        // The field starts with the name the tile shows, so "Shot 2" can be edited rather than typed from nothing.
        let mut name = shot_label(hud, index);
        let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(170.0));
        if edit.changed() {
            hud.ui.render.rename_shot(index, &name);
        }
        if edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            ui.close();
        }
        ui.separator();
        // Another Shot can be overwritten with what is on screen; the active one only when it differs from its save.
        let unchanged = index == hud.ui.render.active_shot && !modified;
        let update = ui.add_enabled(!unchanged, egui::Button::new(update_label));
        if unchanged {
            update.on_disabled_hover_text(unchanged_hint);
        } else if update.clicked() {
            hud.ui.render.update_shot(index, hud.camera);
            ui.close();
        }
        if ui
            .add_enabled(hud.ui.render.shots.len() > 1, egui::Button::new(delete_label))
            .clicked()
        {
            hud.ui.render.delete_shot(index, hud.camera);
            ui.close();
        }
    });
}

/// A cell `width` wide whose content sits at the left or right edge.
fn fixed_cell(ui: &mut egui::Ui, width: f32, align: egui::Align, content: impl FnOnce(&mut egui::Ui)) {
    let layout = if align == egui::Align::RIGHT {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.allocate_ui_with_layout(vec2(width, 16.0), layout, content);
}

/// Bottom corners: mouse hints on the left, status, samples and time left on the right.
pub(crate) fn corners(ctx: &Context, hud: &mut Hud<'_>) {
    // Level 1 drops one hint and the time left, level 2 keeps one hint and only the status.
    let budget = corner_budget(ctx);
    let key = fit_key(&[&hud.settings.language, &hud.settings.ui_scale_percent]);
    let hints_level = fit_level(ctx, hud, "render-hints", budget, key, 2);
    let readout_level = fit_level(ctx, hud, "render-readout", budget, key, 2);
    let hints = theme::island()
        .corner_radius(corner(0, 12, 0, 0))
        .inner_margin(egui::Margin {
            left: 12,
            right: 14,
            top: 0,
            bottom: 0,
        });
    island(
        ctx,
        hud,
        "render-hints",
        Place::Anchor(Align2::LEFT_BOTTOM, vec2(0.0, 0.0)),
        Intro {
            delay: 0.36,
            from: vec2(0.0, 16.0),
        },
        hints,
        |ui, hud| {
            ui.set_height(34.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let left = match hud.ui.render.tool {
                    RenderTool::Orbit => "hint-orbit",
                    RenderTool::Pan => "hint-pan",
                    RenderTool::Dolly => "hint-dolly",
                    RenderTool::PickFocus => "hint-pick-focus",
                    RenderTool::AimSun => "hint-aim-sun",
                    RenderTool::Region => "hint-draw-region",
                };
                let all = [
                    ("LMB".to_owned(), left),
                    ("RMB".to_owned(), "hint-tool-wheel"),
                    ("MMB".to_owned(), "hint-pan"),
                    (hud.tr("hint-wheel"), "hint-dolly"),
                ];
                for (button, key) in all.into_iter().take(4 - usize::from(hints_level)) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(widgets::mono(button, 11.0).color(theme::TEXT));
                        ui.label(RichText::new(hud.tr(key)).color(theme::MUTED).size(11.0));
                    });
                }
            });
        },
    );

    let readout = theme::island()
        .corner_radius(corner(12, 0, 0, 0))
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 0,
            bottom: 0,
        });
    island(
        ctx,
        hud,
        "render-readout",
        Place::Anchor(Align2::RIGHT_BOTTOM, vec2(0.0, 0.0)),
        Intro {
            delay: 0.42,
            from: vec2(0.0, 16.0),
        },
        readout,
        |ui, hud| {
            ui.set_height(34.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let render = &hud.ui.render;
                let status = match render.status() {
                    RenderStatus::Rendering => hud.tr("status-rendering"),
                    RenderStatus::Paused => hud.tr("status-paused"),
                    RenderStatus::Done => hud.tr("status-done"),
                };
                // Fixed-width cells: the island keeps its width while the numbers change.
                fixed_cell(ui, 78.0, egui::Align::LEFT, |ui| {
                    ui.label(RichText::new(status).font(theme::bold(11.0)).color(theme::TEXT));
                });
                if readout_level < 2 {
                    // More samples than asked (the target was lowered) read as finished, not as "101 / 64".
                    let done = render.progress.samples.min(render.progress.target);
                    fixed_cell(ui, 100.0, egui::Align::RIGHT, |ui| {
                        ui.label(widgets::mono(format!("{done} / {} spp", render.progress.target), 11.0));
                    });
                }
                let eta = match render.status() {
                    RenderStatus::Done => hud.tr("eta-finished"),
                    _ => render.eta(Instant::now()).map_or_else(
                        || "–".to_owned(),
                        |eta| {
                            hud.i18n
                                .tr_args("eta-left", &[("time", FluentValue::from(format_duration(eta)))])
                        },
                    ),
                };
                if readout_level == 0 {
                    fixed_cell(ui, 112.0, egui::Align::RIGHT, |ui| {
                        ui.label(widgets::mono(eta, 11.0));
                    });
                }
            });
        },
    );
}
