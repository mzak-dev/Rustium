//! Colors, fonts and frames of the viewport-first HUD (Final mockups).

use std::sync::Arc;

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Stroke, TextStyle};

use crate::settings::{Accent, Motion, Settings};

pub(crate) const TEXT: Color32 = Color32::from_rgb(0xe6, 0xea, 0xf0);
pub(crate) const SOFT: Color32 = Color32::from_rgb(0xc8, 0xcf, 0xd9);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0xa6, 0xaf, 0xbc);
pub(crate) const SUBTLE: Color32 = Color32::from_rgb(0x85, 0x90, 0xa0);
pub(crate) const DISABLED: Color32 = Color32::from_rgb(0x5e, 0x68, 0x75);
pub(crate) const ON_ACCENT: Color32 = Color32::from_rgb(0x1f, 0x0d, 0x05);
pub(crate) const PANEL: Color32 = Color32::from_rgba_unmultiplied_const(17, 19, 23, 189);
pub(crate) const PANEL_SOLID: Color32 = Color32::from_rgb(0x14, 0x16, 0x1a);
pub(crate) const BORDER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 26);
pub(crate) const HOVER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 20);
pub(crate) const ACTIVE: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 33);
pub(crate) const FIELD: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 89);
pub(crate) const FIELD_BORDER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 31);
pub(crate) const DIVIDER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 18);
pub(crate) const SCRIM: Color32 = Color32::from_rgba_unmultiplied_const(8, 9, 11, 158);

/// Corner radius of floating islands, in points.
pub(crate) const ISLAND_RADIUS: u8 = 12;

/// Font family name for bold Lato (headings, active labels).
pub(crate) const BOLD: &str = "bold";

pub(crate) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub(crate) fn accent(accent: Accent) -> Color32 {
    let [r, g, b] = accent.rgb();
    Color32::from_rgb(r, g, b)
}

/// The proportional (Lato, plus a bold family for headings) and monospace (`JetBrains Mono`) families from the mockups.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "lato".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Lato-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "lato-bold".to_owned(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Lato-Bold.ttf"))),
    );
    fonts.font_data.insert(
        "jetbrains-mono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "lato".to_owned());
    fonts.families.insert(
        FontFamily::Name(BOLD.into()),
        vec!["lato-bold".to_owned(), "lato".to_owned()],
    );
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".to_owned());
    ctx.set_fonts(fonts);
}

/// Smallest window, in points at 100 % scale, that the HUD islands are laid out for.
const MIN_POINTS: (f32, f32) = (960.0, 640.0);

/// The interface scale to use: the chosen percentage, lowered so the window still holds [`MIN_POINTS`] points.
///
/// `physical` is the window's inner size in pixels and `native` the display scale factor.
pub(crate) fn effective_zoom(percent: u32, physical: (u32, u32), native: f32) -> f32 {
    let wanted = percent as f32 / 100.0;
    let native = native.max(0.1);
    let fit = (physical.0 as f32 / native / MIN_POINTS.0).min(physical.1 as f32 / native / MIN_POINTS.1);
    // A window that is not laid out yet reports a zero size; leave the scale alone then.
    if fit <= 0.0 { wanted } else { wanted.min(fit.max(1.0)) }
}

/// Applies colors, sizes and timing from `settings` to egui's style.
pub(crate) fn apply_style(ctx: &egui::Context, settings: &Settings) {
    let accent = accent(settings.accent);
    ctx.set_zoom_factor(settings.ui_scale_percent as f32 / 100.0);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(10.5)),
            (TextStyle::Body, FontId::proportional(12.0)),
            (TextStyle::Button, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(11.0)),
            (TextStyle::Heading, FontId::proportional(22.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(4.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size.y = 26.0;
        style.interaction.tooltip_delay = settings.tooltip_delay_seconds;
        style.animation_time = match settings.motion {
            Motion::Full => 0.16,
            Motion::Reduced => 0.08,
            Motion::Off => 0.0,
        };

        let visuals = &mut style.visuals;
        *visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(TEXT);
        visuals.panel_fill = Color32::TRANSPARENT;
        visuals.window_fill = PANEL_SOLID;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(16);
        visuals.menu_corner_radius = CornerRadius::same(10);
        visuals.popup_shadow = shadow();
        visuals.extreme_bg_color = FIELD;
        visuals.selection.bg_fill = accent;
        visuals.selection.stroke = Stroke::new(1.0, ON_ACCENT);
        visuals.hyperlink_color = MUTED;
        let widgets = &mut visuals.widgets;
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            state.corner_radius = CornerRadius::same(7);
            state.bg_stroke = Stroke::NONE;
        }
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_fill = FIELD;
        widgets.inactive.fg_stroke = Stroke::new(1.5, MUTED);
        widgets.hovered.weak_bg_fill = HOVER;
        widgets.hovered.bg_fill = HOVER;
        widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        widgets.active.weak_bg_fill = ACTIVE;
        widgets.active.bg_fill = ACTIVE;
        widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        widgets.open.weak_bg_fill = ACTIVE;
    });
}

fn shadow() -> Shadow {
    Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(89),
    }
}

/// The translucent rounded frame every floating island uses.
pub(crate) fn island() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(ISLAND_RADIUS)
        .inner_margin(4)
        .shadow(shadow())
}

#[cfg(test)]
mod tests {
    use super::effective_zoom;

    #[test]
    fn a_large_scale_is_lowered_until_the_window_fits_the_layout() {
        assert!((effective_zoom(200, (960, 640), 1.0) - 1.0).abs() < 1e-6);
        assert!((effective_zoom(150, (1920, 1200), 1.0) - 1.5).abs() < 1e-6);
        assert!((effective_zoom(200, (1440, 900), 1.0) - 1.40625).abs() < 1e-4);
    }

    #[test]
    fn a_small_scale_is_kept_and_an_empty_window_is_ignored() {
        assert!((effective_zoom(80, (960, 640), 1.0) - 0.8).abs() < 1e-6);
        assert!((effective_zoom(150, (0, 0), 1.0) - 1.5).abs() < 1e-6);
    }
}
