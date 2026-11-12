//! Bottom-centre island: current color, ten Color slots and the Palette popover above them.

use egui::{Align2, Color32, Context, FontId, Id, Sense, Stroke, StrokeKind, vec2};
use fluent_bundle::FluentValue;
use rustium_core::ColorId;

use super::widgets::{self, Look};
use super::{Glass, Hud, Intro, MARGIN, Place, centre_budget, fit_key, fit_level, island};
use crate::icons::Icon;
use crate::keymap::Action;
use crate::settings::Motion;
use crate::theme;

fn color_of(hud: &Hud<'_>, id: ColorId) -> Color32 {
    hud.editor.scene.palette().get(id).map_or(Color32::BLACK, |color| {
        let [r, g, b] = color.color;
        Color32::from_rgb(r, g, b)
    })
}

fn hex(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

/// Level 1 drops the Palette label, level 2 shrinks the slots, level 3 drops the current color.
const MAX_LEVEL: u8 = 3;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let budget = centre_budget(ctx, hud, "hints", "readout");
    let key = fit_key(&[
        &hud.settings.language,
        &hud.settings.ui_scale_percent,
        &hud.editor.slots.len(),
    ]);
    let level = fit_level(ctx, hud, "hotbar", budget, key, MAX_LEVEL);
    let response = island(
        ctx,
        hud,
        "hotbar",
        Place::Anchor(Align2::CENTER_BOTTOM, vec2(0.0, -MARGIN)),
        Intro {
            delay: 0.3,
            from: vec2(0.0, 16.0),
        },
        theme::island().inner_margin(6),
        |ui, hud| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let current = color_of(hud, hud.editor.color());
                if level < MAX_LEVEL {
                    let (area, _) = ui.allocate_exact_size(vec2(54.0, 40.0), Sense::hover());
                    let swatch = egui::Rect::from_center_size(area.center_top() + vec2(0.0, 13.0), vec2(48.0, 24.0));
                    ui.painter().rect(
                        swatch,
                        7,
                        current,
                        Stroke::new(1.0, Color32::from_black_alpha(128)),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        area.center_bottom() - vec2(0.0, 2.0),
                        Align2::CENTER_BOTTOM,
                        hex(current),
                        FontId::monospace(9.0),
                        theme::MUTED,
                    );
                }
                let slot_size = if level >= 2 { 30.0 } else { 36.0 };
                for slot in 0..hud.editor.slots.len() {
                    slot_button(ui, hud, slot, slot_size);
                }
                widgets::divider(ui, 36.0);
                let look = if hud.ui.palette_open { Look::On } else { Look::Plain };
                let count = hud.editor.scene.palette().len().to_string();
                let label = if level >= 1 { String::new() } else { hud.tr("palette") };
                if widgets::icon_text_button(ui, Icon::Palette, &label, &count, look).clicked() {
                    hud.ui.palette_open = !hud.ui.palette_open;
                }
            });
        },
    );
    palette_popover(ctx, hud, response.response.rect);
}

fn slot_button(ui: &mut egui::Ui, hud: &mut Hud<'_>, slot: usize, size: f32) {
    let selected = slot == hud.editor.active_slot;
    let lift = ui
        .ctx()
        .animate_bool_with_time(Id::unique(("slot-lift", slot)), selected, 0.18);
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let rect = rect.translate(vec2(0.0, -6.0 * lift));
    let color = color_of(hud, hud.editor.slots[slot]);
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect.translate(vec2(0.0, 8.0)), 7, Color32::from_black_alpha(90));
        painter.rect(rect.expand(2.0), 9, Color32::WHITE, Stroke::NONE, StrokeKind::Inside);
    }
    painter.rect(
        rect,
        7,
        color,
        Stroke::new(1.0, Color32::from_black_alpha(128)),
        StrokeKind::Inside,
    );
    let number = ((slot + 1) % 10).to_string();
    let key = hud.settings.keymap.label(Action::Slot(u8::try_from(slot).unwrap_or(0)));
    painter.text(
        rect.left_top() + vec2(3.0, 1.0),
        Align2::LEFT_TOP,
        &key,
        FontId::monospace(9.0),
        Color32::WHITE,
    );
    let label = hud.i18n.tr_args(
        "color-slot",
        &[
            ("slot", FluentValue::from(number)),
            ("hex", FluentValue::from(hex(color))),
        ],
    );
    if response.on_hover_text(label).clicked() {
        hud.editor.active_slot = slot;
    }
}

fn palette_popover(ctx: &Context, hud: &mut Hud<'_>, hotbar: egui::Rect) {
    let duration = if hud.settings.motion == Motion::Off { 0.0 } else { 0.2 };
    let openness = ctx.animate_bool_with_time(Id::unique("palette-open"), hud.ui.palette_open, duration);
    if openness < 0.01 {
        return;
    }
    let anchor = egui::pos2(hotbar.center().x, hotbar.top() - 8.0 + 10.0 * (1.0 - openness));
    let response = egui::Area::new(Id::unique("palette-popover"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            ui.multiply_opacity(openness);
            theme::island().inner_margin(10).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(hud.tr("palette")).font(theme::bold(12.0)));
                    ui.label(widgets::mono("default.pal", 11.0).color(theme::MUTED));
                    ui.add_space(24.0);
                    let slot = ((hud.editor.active_slot + 1) % 10).to_string();
                    ui.label(
                        widgets::muted(hud.i18n.tr_args("palette-hint", &[("slot", FluentValue::from(slot))]))
                            .size(11.0),
                    );
                });
                ui.add_space(4.0);
                let ids: Vec<ColorId> = hud.editor.scene.palette().iter().map(|(id, _)| id).collect();
                let current = hud.editor.color();
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(128))
                    .corner_radius(4)
                    .inner_margin(1)
                    .show(ui, |ui| {
                        // Laid out by hand so the cells form one continuous grid.
                        const COLUMNS: usize = 16;
                        const CELL: f32 = 22.0;
                        const STEP: f32 = CELL + 1.0;
                        let rows = ids.len().div_ceil(COLUMNS);
                        let size = vec2(COLUMNS as f32 * STEP - 1.0, rows as f32 * STEP - 1.0);
                        let (area, _) = ui.allocate_exact_size(size, Sense::hover());
                        for (index, id) in ids.iter().enumerate() {
                            let cell = vec2((index % COLUMNS) as f32 * STEP, (index / COLUMNS) as f32 * STEP);
                            let rect = egui::Rect::from_min_size(area.min + cell, vec2(CELL, CELL));
                            let response = ui.interact(rect, Id::unique(("palette-cell", index)), Sense::click());
                            let color = color_of(hud, *id);
                            let hovered = response.hovered();
                            let draw_rect = if hovered { rect.expand(2.0) } else { rect };
                            ui.painter().rect_filled(draw_rect, 0, color);
                            if *id == current || hovered {
                                ui.painter().rect_stroke(
                                    draw_rect,
                                    0,
                                    Stroke::new(2.0, Color32::WHITE),
                                    StrokeKind::Inside,
                                );
                            }
                            if response.on_hover_text(hex(color)).clicked() {
                                let slot = hud.editor.active_slot;
                                hud.editor.slots[slot] = *id;
                            }
                        }
                    });
            });
        });
    hud.glass.push(Glass {
        rect: response.response.rect,
        radius: f32::from(theme::ISLAND_RADIUS),
    });
}
