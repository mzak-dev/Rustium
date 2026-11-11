//! Small widgets shared by the HUD islands and the modals, matching the mockup controls.

use egui::{Color32, FontId, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};

use crate::icons::Icon;
use crate::theme;

/// How a button is highlighted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    Plain,
    /// Toggled on (light fill).
    On,
    /// The active Tool or a primary action (accent fill).
    Accent,
}

fn colors(ui: &Ui, response: &Response, look: Look, enabled: bool) -> (Color32, Color32) {
    let accent = ui.visuals().selection.bg_fill;
    if !enabled {
        return (Color32::TRANSPARENT, theme::DISABLED);
    }
    match look {
        Look::Accent => (accent, theme::ON_ACCENT),
        Look::On => (theme::ACTIVE, Color32::WHITE),
        Look::Plain if response.hovered() => (theme::HOVER, Color32::WHITE),
        Look::Plain => (Color32::TRANSPARENT, theme::MUTED),
    }
}

/// A square icon button (32 × 32 in bars, 44 × 44 on the toolbar).
pub(crate) fn icon_button(ui: &mut Ui, icon: Icon, size: f32, look: Look, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let (fill, foreground) = colors(ui, &response, look, enabled);
    let rect = if response.is_pointer_button_down_on() {
        shrink_pressed(rect)
    } else {
        rect
    };
    ui.painter().rect_filled(rect, 7, fill);
    let icon_size = if size >= 40.0 { 22.0 } else { 18.0 };
    icon.paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(icon_size)),
        foreground,
    );
    response
}

/// A text button like the mockup `.hb` (32 points tall, or `height`).
pub(crate) fn text_button(ui: &mut Ui, text: &str, height: f32, look: Look, enabled: bool) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let width = (galley.size().x + 16.0).max(height);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    let (fill, foreground) = colors(ui, &response, look, enabled);
    let text_color = if look == Look::Plain && enabled && !response.hovered() {
        theme::MUTED
    } else {
        foreground
    };
    ui.painter().rect_filled(rect, 7, fill);
    ui.painter()
        .galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, text_color);
    response
}

/// Icon followed by a label, e.g. the Palette button.
pub(crate) fn icon_text_button(ui: &mut Ui, icon: Icon, text: &str, extra: &str, look: Look) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let extra_galley = ui
        .painter()
        .layout_no_wrap(extra.to_owned(), FontId::monospace(10.0), Color32::WHITE);
    let width = 8.0 + 18.0 + 8.0 + galley.size().x + 6.0 + extra_galley.size().x + 10.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
    let (fill, foreground) = colors(ui, &response, look, true);
    ui.painter().rect_filled(rect, 7, fill);
    let icon_rect = Rect::from_min_size(rect.left_center() + vec2(8.0, -9.0), Vec2::splat(18.0));
    icon.paint(ui.painter(), icon_rect, foreground);
    let text_pos = icon_rect.right_center() + vec2(8.0, -galley.size().y / 2.0);
    let extra_pos = text_pos + vec2(galley.size().x + 6.0, (galley.size().y - extra_galley.size().y) / 2.0);
    ui.painter()
        .galley_with_override_text_color(text_pos, galley, foreground);
    ui.painter()
        .galley_with_override_text_color(extra_pos, extra_galley, foreground);
    response
}

fn shrink_pressed(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * 0.93)
}

/// The thin vertical divider between groups in a bar.
pub(crate) fn divider(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, height), Sense::hover());
    let x = rect.center().x;
    ui.painter().line_segment(
        [egui::pos2(x, rect.top() + 6.0), egui::pos2(x, rect.bottom() - 6.0)],
        Stroke::new(1.0, theme::BORDER),
    );
}

/// The pill switch from the mockups.
pub(crate) fn switch(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut response) = ui.allocate_exact_size(vec2(34.0, 20.0), sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let accent = ui.visuals().selection.bg_fill;
    let progress = ui.ctx().animate_bool_with_time(response.id, *on, 0.18);
    let painter = ui.painter();
    let (fill, border) = if *on {
        (accent, accent)
    } else {
        (theme::FIELD, Color32::from_rgb(0x6b, 0x75, 0x83))
    };
    let fill = if enabled { fill } else { fill.gamma_multiply(0.4) };
    painter.rect(rect, 10, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    let knob_x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, progress);
    let knob = if *on { theme::ON_ACCENT } else { theme::MUTED };
    painter.circle_filled(egui::pos2(knob_x, rect.center().y), 7.0, knob);
    response
}

/// A segmented control; returns true when the value changed.
pub(crate) fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, String)],
    enabled: bool,
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
        .corner_radius(8)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, label) in options {
                    let look = if *value == *option { Look::On } else { Look::Plain };
                    if text_button(ui, label, 22.0, look, enabled).clicked() && *value != *option {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Muted label text.
pub(crate) fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::MUTED)
}

/// Monospace text in the soft color.
pub(crate) fn mono(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::monospace(size)).color(theme::SOFT)
}

/// A text button followed by a chevron, opening a dropdown.
pub(crate) fn dropdown_button(ui: &mut Ui, text: &str) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 36.0, 32.0), Sense::click());
    let (fill, foreground) = colors(ui, &response, Look::Plain, true);
    ui.painter().rect_filled(rect, 7, fill);
    let text_pos = rect.left_center() + vec2(8.0, -galley.size().y / 2.0);
    ui.painter()
        .galley_with_override_text_color(text_pos, galley, foreground);
    let chevron = Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), Vec2::splat(14.0));
    Icon::ChevronDown.paint(ui.painter(), chevron, foreground);
    response
}
