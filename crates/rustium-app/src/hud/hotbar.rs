//! Bottom-centre island: current color, ten Color slots and the Palette popover above them.

use egui::{Align2, Color32, Context, FontId, Id, Sense, Stroke, StrokeKind, vec2};
use fluent_bundle::FluentValue;
use rustium_core::{ColorId, Command, MaterialGraph};

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

pub(crate) fn hex(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

/// A pasted `#RRGGBB`: finds or adds that Color, puts it in the active slot and, in Effects mode, gives it the
/// Material being edited.
pub(crate) fn paste_hex(hud: &mut Hud<'_>, text: &str) -> bool {
    let Some(color) = super::effects::parse_hex(text) else {
        return false;
    };
    let Some(id) = hud.editor.color_for([color.r(), color.g(), color.b()]) else {
        return false;
    };
    let slot = hud.editor.active_slot;
    if let Some(entry) = hud.editor.slots.get_mut(slot) {
        *entry = id;
    }
    let graph = u32::try_from(hud.ui.effects.graph).unwrap_or(0);
    let assigned = hud.ui.mode == super::Mode::Effects
        && graph != 0
        && super::effects::command(hud, &Command::AssignGraphToColors { ids: vec![id], graph });
    let key = if assigned {
        "color-pasted-assigned"
    } else {
        "color-pasted"
    };
    let text = hud.i18n.tr_args(key, &[("hex", FluentValue::from(hex(color)))]);
    hud.ui.toast = Some(super::Toast::new(text, false));
    true
}

/// The right-click menu of a Color: its Material in Effects mode, copy its hex, select its voxels.
fn color_menu(ui: &mut egui::Ui, hud: &mut Hud<'_>, id: ColorId) {
    let Some(entry) = hud.editor.scene.palette().get(id).copied() else {
        return;
    };
    let key = if entry.graph == 0 {
        "color-add-material"
    } else {
        "color-edit-material"
    };
    if ui.button(hud.tr(key)).clicked() {
        hud.editor.release();
        if entry.graph == 0 {
            super::effects_bars::add_material(hud, MaterialGraph::blank(&hud.i18n.tr("effects-new-name")));
            let graph = u32::try_from(hud.ui.effects.graph).unwrap_or(0);
            if graph != 0 {
                super::effects::command(hud, &Command::AssignGraphToColors { ids: vec![id], graph });
            }
        } else {
            hud.ui.effects.select_graph(usize::try_from(entry.graph).unwrap_or(0));
            hud.ui.effects.tab = super::effects::EffectsTab::Material;
        }
        hud.ui.mode = super::Mode::Effects;
        ui.close();
    }
    let [r, g, b] = entry.color;
    ui.menu_button(hud.tr("color-edit"), |ui| {
        let mut color = Color32::from_rgb(r, g, b);
        if super::effects::color_picker(ui, &mut color) {
            hud.editor.set_color_live(id, [color.r(), color.g(), color.b()]);
        }
        if !ui.input(|input| input.pointer.any_down()) {
            hud.editor.finish_material_edit();
        }
    });
    if ui.button(hud.tr("color-copy-hex")).clicked() {
        ui.ctx().copy_text(hex(Color32::from_rgb(r, g, b)));
        ui.close();
    }
    ui.separator();
    let count = hud.editor.scene.palette().len();
    let previous = id.get().checked_sub(1).and_then(ColorId::new);
    let next = ColorId::new(id.get().saturating_add(1)).filter(|next| usize::from(next.get()) <= count);
    if ui
        .add_enabled(previous.is_some(), egui::Button::new(hud.tr("color-move-left")))
        .clicked()
        && let Some(previous) = previous
    {
        hud.editor.swap_colors(id, previous);
        ui.close();
    }
    if ui
        .add_enabled(next.is_some(), egui::Button::new(hud.tr("color-move-right")))
        .clicked()
        && let Some(next) = next
    {
        hud.editor.swap_colors(id, next);
        ui.close();
    }
    if ui
        .add_enabled(count > 1, egui::Button::new(hud.tr("color-delete")))
        .clicked()
    {
        hud.editor.delete_color(id);
        ui.close();
    }
    ui.separator();
    let model = hud.editor.scene.model_shape(hud.editor.focus).map(|(_, shape)| shape);
    if ui
        .add_enabled(model.is_some(), egui::Button::new(hud.tr("color-select-voxels")))
        .clicked()
    {
        if let Some(shape) = model {
            hud.editor.selection = rustium_core::select_by_color(shape, id);
            hud.editor.tool = crate::editor::Tool::SelectBox;
        }
        ui.close();
    }
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
    let response = response.on_hover_text(label);
    if response.clicked() {
        hud.editor.active_slot = slot;
    }
    let id = hud.editor.slots[slot];
    response.context_menu(|ui| color_menu(ui, hud, id));
}

fn palette_popover(ctx: &Context, hud: &mut Hud<'_>, hotbar: egui::Rect) {
    // Escape closes the popover; the HUD consumes the key afterwards so it does not also step out of the Focus.
    if hud.ui.palette_open && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        hud.ui.palette_open = false;
    }
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
                    ui.add_space(24.0);
                    let slot = ((hud.editor.active_slot + 1) % 10).to_string();
                    ui.label(
                        widgets::muted(hud.i18n.tr_args("palette-hint", &[("slot", FluentValue::from(slot))]))
                            .size(11.0),
                    );
                    let new = widgets::icon_button(ui, Icon::NewFile, 24.0, Look::Plain, true);
                    if widgets::tip(new, &hud.tr("palette-new"), &hud.tr("palette-new-tip")).clicked() {
                        hud.editor.new_palette();
                    }
                    let import = widgets::icon_button(ui, Icon::Folder, 24.0, Look::Plain, true);
                    if widgets::tip(import, &hud.tr("menu-import-palette"), &hud.tr("palette-import-tip")).clicked() {
                        hud.send(super::AppCommand::ImportPalette);
                    }
                    let export = widgets::icon_button(ui, Icon::Download, 24.0, Look::Plain, true);
                    if widgets::tip(export, &hud.tr("menu-export-palette"), &hud.tr("palette-export-tip")).clicked() {
                        hud.send(super::AppCommand::ExportPalette);
                    }
                    // A copy of the current color, ready to edit with a right click.
                    let room = hud.editor.scene.palette().len() < rustium_core::Palette::MAX_LEN;
                    let add = widgets::icon_button(ui, Icon::Plus, 24.0, Look::Plain, room);
                    if widgets::tip(add, &hud.tr("palette-add"), &hud.tr("palette-add-tip")).clicked() {
                        let current = hud.editor.color();
                        if let Some(color) = hud.editor.scene.palette().get(current).map(|entry| entry.color)
                            && hud
                                .editor
                                .execute_checked(&Command::AddColor { color })
                                .unwrap_or(false)
                            && let Some(id) = u8::try_from(hud.editor.scene.palette().len())
                                .ok()
                                .and_then(ColorId::new)
                        {
                            let slot = hud.editor.active_slot;
                            hud.editor.slots[slot] = id;
                        }
                    }
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
                            let response = response.on_hover_text(hex(color));
                            if response.clicked() {
                                let slot = hud.editor.active_slot;
                                if let Some(entry) = hud.editor.slots.get_mut(slot) {
                                    *entry = *id;
                                }
                            }
                            response.context_menu(|ui| color_menu(ui, hud, *id));
                        }
                    });
            });
        });
    hud.glass.push(Glass {
        rect: response.response.rect,
        radius: f32::from(theme::ISLAND_RADIUS),
    });
}
