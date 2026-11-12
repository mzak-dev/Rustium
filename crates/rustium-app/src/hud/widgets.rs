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

/// One row of a popup menu: full width, a check mark when `selected`, a subtle fill on hover and when selected.
pub(crate) fn menu_row(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let width = 180.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 6, theme::HOVER);
    } else if selected {
        ui.painter().rect_filled(rect, 6, theme::ACTIVE);
    }
    if selected {
        let origin = rect.left_center() + vec2(10.0, 0.0);
        let check = [vec2(0.0, 0.0), vec2(3.0, 3.5), vec2(8.0, -3.5)].map(|offset| origin + offset);
        ui.painter()
            .add(egui::Shape::line(check.to_vec(), Stroke::new(1.6, theme::TEXT)));
    }
    let color = if selected || response.hovered() {
        Color32::WHITE
    } else {
        theme::TEXT
    };
    ui.painter().text(
        rect.left_center() + vec2(28.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.0),
        color,
    );
    response
}

/// A dialog footer button of a fixed `size`; `Look::Accent` is the filled primary one, anything else is outlined.
pub(crate) fn dialog_button(ui: &mut Ui, text: &str, size: Vec2, look: Look) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let accent = look == Look::Accent;
    let (fill, foreground) = if accent {
        colors(ui, &response, Look::Accent, true)
    } else if response.hovered() {
        (theme::HOVER, Color32::WHITE)
    } else {
        (Color32::TRANSPARENT, theme::TEXT)
    };
    ui.painter().rect_filled(rect, 8, fill);
    if !accent {
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            8,
            Stroke::new(1.0, theme::FIELD_BORDER),
            egui::StrokeKind::Inside,
        );
    }
    let font = if accent {
        theme::bold(13.0)
    } else {
        FontId::proportional(12.0)
    };
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, text, font, foreground);
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

/// A checkbox with its label to the right, like the native control the mockups use: accent fill when checked.
pub(crate) fn checkbox(ui: &mut Ui, on: &mut bool, text: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(11.0), theme::MUTED);
    let (rect, mut response) = ui.allocate_exact_size(vec2(14.0 + 6.0 + galley.size().x, 18.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let accent = ui.visuals().selection.bg_fill;
    let boxed = Rect::from_min_size(rect.left_center() - vec2(0.0, 7.0), Vec2::splat(14.0));
    let painter = ui.painter();
    if *on {
        painter.rect_filled(boxed, 3, accent);
        let point = |(x, y): (f32, f32)| boxed.min + vec2(x, y);
        painter.add(egui::Shape::line(
            vec![point((3.5, 7.2)), point((6.0, 9.8)), point((10.5, 4.6))],
            Stroke::new(1.8, Color32::WHITE),
        ));
    } else {
        painter.rect(
            boxed,
            3,
            theme::FIELD,
            Stroke::new(1.0, Color32::from_rgb(0x6b, 0x75, 0x83)),
            StrokeKind::Inside,
        );
    }
    let color = if response.hovered() { theme::TEXT } else { theme::MUTED };
    painter.galley_with_override_text_color(rect.left_center() + vec2(20.0, -galley.size().y / 2.0), galley, color);
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
            // A right-to-left parent (Settings rows) also flips the layout of `horizontal`.
            let flipped = ui.layout().prefer_right_to_left();
            ui.horizontal(|ui| {
                let ordered: Vec<&(T, String)> = if flipped {
                    options.iter().rev().collect()
                } else {
                    options.iter().collect()
                };
                for (option, label) in ordered.into_iter().map(|pair| (&pair.0, &pair.1)) {
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

/// Widest a side tooltip gets, in points.
const TIP_MAX_WIDTH: f32 = 236.0;
/// Padding inside a side tooltip.
const TIP_PAD: Vec2 = vec2(10.0, 8.0);

/// The tooltip of a vertical toolbar button, like the mockup's `.tip`: a card to the right of the button with the
/// name, a key cap showing the current binding and a one-line description.
///
/// Shows after the Settings tooltip delay while `response` is hovered; call it for every toolbar button.
pub(crate) fn side_tip(response: &Response, name: &str, key: &str, description: &str) {
    if !egui::Tooltip::should_show_tooltip(response, false) {
        return;
    }
    let ctx = &response.ctx;
    let (name_galley, key_galley, description_galley) = ctx.fonts_mut(|fonts| {
        (
            fonts.layout_no_wrap(name.to_owned(), theme::bold(12.0), theme::TEXT),
            fonts.layout_no_wrap(key.to_owned(), FontId::monospace(10.0), theme::TEXT),
            fonts.layout(
                description.to_owned(),
                FontId::proportional(11.0),
                theme::MUTED,
                TIP_MAX_WIDTH - 2.0 * TIP_PAD.x,
            ),
        )
    });
    let key_size = key_galley.size() + vec2(12.0, 2.0);
    let width = (name_galley.size().x + 10.0 + key_size.x)
        .max(description_galley.size().x)
        .max(60.0);
    let row = name_galley.size().y.max(key_size.y);
    let size = vec2(width, row + 4.0 + description_galley.size().y) + 2.0 * TIP_PAD;
    egui::Area::new(egui::Id::unique(("side-tip", response.id)))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .pivot(egui::Align2::LEFT_CENTER)
        .fixed_pos(response.rect.right_center() + vec2(10.0, 0.0))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let painter = ui.painter();
            painter.rect(
                rect,
                9,
                Color32::from_rgb(0x0b, 0x0d, 0x10),
                Stroke::new(1.0, Color32::from_white_alpha(36)),
                StrokeKind::Inside,
            );
            let inner = rect.shrink2(TIP_PAD);
            painter.galley(
                inner.left_top() + vec2(0.0, (row - name_galley.size().y) / 2.0),
                name_galley,
                theme::TEXT,
            );
            let cap_min = inner.right_top() - vec2(key_size.x, 0.0) + vec2(0.0, (row - key_size.y) / 2.0);
            let cap = Rect::from_min_size(cap_min, key_size);
            painter.rect(
                cap,
                5,
                Color32::from_white_alpha(15),
                Stroke::new(1.0, Color32::from_white_alpha(51)),
                StrokeKind::Inside,
            );
            painter.galley(cap.center() - key_galley.size() / 2.0, key_galley, theme::TEXT);
            painter.galley(
                inner.left_top() + vec2(0.0, row + 4.0),
                description_galley,
                theme::MUTED,
            );
        });
}

/// A single-line text field that keeps the typed text in egui memory while it has focus and returns the
/// new text once editing ends (focus lost or Enter) with a value that differs from `current`.
pub(crate) fn text_field(
    ui: &mut Ui,
    salt: egui::IdSalt,
    current: &str,
    enabled: bool,
    hint: Option<&str>,
    width: f32,
) -> Option<String> {
    let id = ui.make_persistent_id(salt);
    let buffer_id = id.with("buffer");
    let focused = ui.ctx().memory(|memory| memory.has_focus(id));
    let mut text = if focused {
        ui.data(|data| data.get_temp::<String>(buffer_id))
            .unwrap_or_else(|| current.to_owned())
    } else {
        current.to_owned()
    };
    let mut edit = egui::TextEdit::singleline(&mut text).id(id).desired_width(width);
    if let Some(hint) = hint {
        edit = edit.hint_text(hint);
    }
    let response = ui.add_enabled(enabled, edit);
    if response.has_focus() {
        ui.data_mut(|data| data.insert_temp(buffer_id, text.clone()));
    }
    if response.lost_focus() {
        ui.data_mut(|data| data.remove_temp::<String>(buffer_id));
        if text != current {
            return Some(text);
        }
    }
    None
}
