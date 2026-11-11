//! Render mode, bottom: the Render / Pause button with Shots, and the corner hints and status readout.

use std::time::Instant;

use egui::{Align2, Color32, Context, FontId, Id, RichText, Sense, Stroke, StrokeKind, vec2};
use fluent_bundle::FluentValue;

use super::widgets::{self, Look};
use super::{AppCommand, Hud, Intro, MARGIN, Place, corner, island};
use crate::icons::Icon;
use crate::render_mode::{MAX_SHOTS, RenderStatus, format_duration};
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
                render_button(ui, hud);
                widgets::divider(ui, 40.0);
                for index in 0..hud.ui.render.shots.len() {
                    shot_tile(ui, hud, index);
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

fn render_button(ui: &mut egui::Ui, hud: &mut Hud<'_>) {
    let status = hud.ui.render.status();
    let (icon, key) = match status {
        RenderStatus::Rendering => (Icon::Pause, "render-pause"),
        RenderStatus::Paused => (Icon::Play, "render-resume"),
        RenderStatus::Done => (Icon::Play, "render-again"),
    };
    let label = hud.tr(key);
    let galley = ui.painter().layout_no_wrap(label, theme::bold(12.5), theme::ON_ACCENT);
    let width = (14.0 + 18.0 + 8.0 + galley.size().x + 8.0 + 22.0 + 14.0).max(132.0);
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
        egui::Rect::from_center_size(rect.left_center() + vec2(23.0, 0.0), vec2(18.0, 18.0)),
        theme::ON_ACCENT,
    );
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
    if response.on_hover_text(hud.tr("render-button-hint")).clicked() {
        toggle_render(hud);
    }
}

fn shot_tile(ui: &mut egui::Ui, hud: &mut Hud<'_>, index: usize) {
    let selected = index == hud.ui.render.active_shot;
    let lift = ui
        .ctx()
        .animate_bool_with_time(Id::unique(("shot-lift", index)), selected, 0.18);
    let (rect, response) = ui.allocate_exact_size(vec2(56.0, 40.0), Sense::click());
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
    Icon::Cube.paint(
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
    let shot = hud.ui.render.shots.get(index);
    let size = shot.map_or((0, 0), |shot| shot.size);
    let tooltip = hud.i18n.tr_args(
        "shot-tooltip",
        &[
            ("n", FluentValue::from(index + 1)),
            ("size", FluentValue::from(format!("{} × {}", size.0, size.1))),
        ],
    );
    if response.on_hover_text(tooltip).clicked() && !selected {
        hud.ui.render.select_shot(index, hud.camera);
    }
}

/// Bottom corners: mouse hints on the left, status, samples and time left on the right.
pub(crate) fn corners(ctx: &Context, hud: &mut Hud<'_>) {
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
                for (button, key) in [("LMB", "hint-orbit"), ("MMB", "hint-pan"), ("Wheel", "hint-dolly")] {
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
                ui.label(RichText::new(status).font(theme::bold(11.0)).color(theme::TEXT));
                ui.label(widgets::mono(
                    format!("{} / {} spp", render.progress.samples, render.progress.target),
                    11.0,
                ));
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
                ui.label(widgets::mono(eta, 11.0));
            });
        },
    );
}
