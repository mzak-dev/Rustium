//! The Settings modal: sections on the left, rows of controls on the right. Changes apply immediately.

use egui::load::SizedTexture;
use egui::{
    Color32, ComboBox, Context, DragValue, Id, Key, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, vec2,
};
use rustium_mcp::Permission;

use crate::hud::widgets::{self, Look};
use crate::hud::{self, AppCommand, Glass, Hud};
use crate::icons::Icon;
use crate::mcp::{McpStatus, McpTransport, permission_key};
use crate::settings::{Accent, Backend, FrameLimit, Language, Motion, OrbitStyle, RayTracingMode, Settings};
use crate::theme;

/// Sections of the Settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Interface,
    Viewport,
    Input,
    Performance,
    Mcp,
}

impl Section {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Interface,
        Self::Viewport,
        Self::Input,
        Self::Performance,
        Self::Mcp,
    ];

    fn title_key(self) -> &'static str {
        match self {
            Self::General => "section-general",
            Self::Interface => "section-interface",
            Self::Viewport => "section-viewport",
            Self::Input => "section-input",
            Self::Performance => "section-performance",
            Self::Mcp => "section-mcp",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Settings,
            Self::Interface => Icon::Panel,
            Self::Viewport => Icon::Cube,
            Self::Input => Icon::Keyboard,
            Self::Performance => Icon::Bolt,
            Self::Mcp => Icon::Plug,
        }
    }

    /// Row labels, used by the search field.
    fn row_keys(self) -> &'static [&'static str] {
        match self {
            Self::General => &[
                "setting-language",
                "setting-start-screen",
                "setting-default-voxel-size",
                "setting-autosave",
                "setting-autosave-interval",
                "setting-undo-steps",
            ],
            Self::Interface => &[
                "setting-accent",
                "setting-ui-scale",
                "setting-translucent",
                "setting-motion",
                "setting-shortcut-letters",
                "setting-custom-cursors",
                "setting-tooltip-delay",
            ],
            Self::Viewport => &[
                "setting-orbit-style",
                "setting-background",
                "setting-ground-grid",
                "setting-volume-bounds",
                "setting-fov",
                "setting-antialiasing",
            ],
            Self::Input => &["setting-keymap", "setting-invert-zoom", "setting-rebind"],
            Self::Performance => &[
                "setting-device",
                "setting-backend",
                "setting-pt-device",
                "setting-ray-tracing",
                "setting-vsync",
                "setting-frame-limit",
            ],
            Self::Mcp => &[
                "setting-mcp-enabled",
                "setting-mcp-transport",
                "setting-mcp-port",
                "mcp-read-model",
                "mcp-edit-voxels",
                "mcp-edit-palette",
                "mcp-run-scripts",
                "mcp-start-renders",
                "mcp-copy-config",
            ],
        }
    }
}

const CARD: egui::Vec2 = vec2(900.0, 600.0);
/// Space kept between the card and the window edge.
const CARD_GAP: f32 = 12.0;
/// Below this card width the sidebar and footer shrink.
const COMPACT_WIDTH: f32 = 700.0;
const ROW_HEIGHT: f32 = 50.0;

/// The card size that fits the window; the rows scroll when it gets small.
fn card_size(screen: Rect) -> egui::Vec2 {
    vec2(
        CARD.x.min((screen.width() - 2.0 * CARD_GAP).max(240.0)),
        CARD.y.min((screen.height() - 2.0 * CARD_GAP).max(200.0)),
    )
}

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(section) = hud.ui.settings_section else {
        return;
    };
    let before = hud.settings.clone();
    let query = hud.ui.settings_search.trim().to_lowercase();
    ctx.data_mut(|data| data.insert_temp(Id::unique("settings-query"), query));
    let screen = ctx.content_rect();
    let card = Rect::from_center_size(screen.center(), card_size(screen));
    hud.glass.push(Glass {
        rect: screen,
        radius: 0.0,
    });
    egui::Area::new(Id::unique("settings"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (_, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(screen, 0, theme::SCRIM);
            if backdrop.clicked()
                || (!hud.ui.keymap_ui.is_capturing() && ui.input(|input| input.key_pressed(Key::Escape)))
            {
                hud.ui.settings_section = None;
            }
            ui.painter().rect(
                card,
                16,
                theme::PANEL,
                Stroke::new(1.0, theme::BORDER),
                StrokeKind::Inside,
            );
            ui.scope_builder(UiBuilder::new().max_rect(card).sense(Sense::click()), |ui| {
                let compact = card.width() < COMPACT_WIDTH;
                let nav = Rect::from_min_size(card.min, vec2(if compact { 150.0 } else { 212.0 }, card.height()));
                ui.painter().rect_filled(
                    nav,
                    egui::CornerRadius {
                        nw: 16,
                        sw: 16,
                        ne: 0,
                        se: 0,
                    },
                    Color32::from_black_alpha(46),
                );
                ui.painter().vline(
                    nav.right(),
                    nav.y_range(),
                    Stroke::new(1.0, Color32::from_white_alpha(20)),
                );
                navigation(ui, hud, nav.shrink2(vec2(10.0, 14.0)), section);
                let content = Rect::from_min_max(egui::pos2(nav.right(), card.top()), card.max);
                body(ui, hud, content, section, compact);
            });
        });
    if *hud.settings != before {
        hud.send(AppCommand::SettingsChanged);
    }
}

/// Sections whose title, rows or (for Input) shortcut names contain `query`; every section when it is empty.
fn matching_sections(hud: &Hud<'_>, query: &str) -> Vec<Section> {
    Section::ALL
        .into_iter()
        // A page cannot serve MCP, so the browser has no MCP section.
        .filter(|section| crate::DESKTOP || *section != Section::Mcp)
        .filter(|section| {
            query.is_empty()
                || hud.tr(section.title_key()).to_lowercase().contains(query)
                || section
                    .row_keys()
                    .iter()
                    .any(|key| hud.tr(key).to_lowercase().contains(query))
                || (*section == Section::Input && crate::settings_input::any_action_matches(hud, query))
        })
        .collect()
}

fn navigation(ui: &mut Ui, hud: &mut Hud<'_>, rect: Rect, current: Section) {
    // A short card has no room for the logo footer and gives the entries less height.
    let tight = rect.height() < 400.0;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        egui::Frame::new()
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (icon, _) = ui.allocate_exact_size(vec2(14.0, 22.0), Sense::hover());
                    Icon::Search.paint(
                        ui.painter(),
                        Rect::from_center_size(icon.center(), vec2(14.0, 14.0)),
                        theme::SUBTLE,
                    );
                    let hint = hud.tr("settings-search");
                    ui.add(
                        egui::TextEdit::singleline(&mut hud.ui.settings_search)
                            .hint_text(hint)
                            .frame(egui::Frame::NONE)
                            .align(egui::Align2::LEFT_CENTER)
                            .min_size(vec2(0.0, 22.0))
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        ui.add_space(if tight { 4.0 } else { 10.0 });
        let query = hud.ui.settings_search.trim().to_lowercase();
        let matching = matching_sections(hud, &query);
        // Searching jumps to the first section that has a match, where the matching rows are highlighted.
        let mut current = current;
        if let Some(first) = matching.first().filter(|_| !matching.contains(&current)) {
            current = *first;
            hud.ui.settings_section = Some(current);
            ui.ctx().request_repaint();
        }
        if matching.is_empty() {
            ui.add_space(8.0);
            ui.label(widgets::muted(hud.tr("settings-no-results")).size(12.0));
        }
        // A shortcut name typed into the main search also narrows the shortcut list on the Input page.
        let applied_id = Id::unique("settings-filter-applied");
        let applied: String = ui.ctx().data(|data| data.get_temp(applied_id)).unwrap_or_default();
        if applied != query {
            ui.ctx().data_mut(|data| data.insert_temp(applied_id, query.clone()));
            hud.ui.keymap_ui.filter = if crate::settings_input::any_action_matches(hud, &query) {
                query.clone()
            } else {
                String::new()
            };
        }
        for section in matching {
            let (row, response) =
                ui.allocate_exact_size(vec2(rect.width(), if tight { 30.0 } else { 36.0 }), Sense::click());
            let selected = section == current;
            let fill = if selected {
                theme::ACTIVE
            } else if response.hovered() {
                theme::HOVER
            } else {
                Color32::TRANSPARENT
            };
            let color = if selected || response.hovered() {
                Color32::WHITE
            } else {
                theme::MUTED
            };
            ui.painter().rect_filled(row, 8, fill);
            section.icon().paint(
                ui.painter(),
                Rect::from_min_size(row.left_center() + vec2(10.0, -9.0), vec2(18.0, 18.0)),
                color,
            );
            ui.painter().text(
                row.left_center() + vec2(38.0, 0.0),
                egui::Align2::LEFT_CENTER,
                hud.tr(section.title_key()),
                theme::bold(13.0),
                color,
            );
            if response.clicked() {
                hud.ui.settings_section = Some(section);
            }
        }
        if tight {
            return;
        }
        let footer = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 42.0), rect.max);
        ui.painter().hline(
            footer.x_range(),
            footer.top(),
            Stroke::new(1.0, Color32::from_white_alpha(20)),
        );
        let logo = Rect::from_min_size(footer.min + vec2(6.0, 8.0), vec2(34.0, 34.0));
        egui::Image::from_texture(SizedTexture::new(hud.logo.id(), logo.size())).paint_at(ui, logo);
        ui.painter().text(
            logo.right_top() + vec2(10.0, 8.0),
            egui::Align2::LEFT_CENTER,
            hud.tr("app-name"),
            theme::bold(13.0),
            theme::TEXT,
        );
        ui.painter().text(
            logo.right_top() + vec2(10.0, 24.0),
            egui::Align2::LEFT_CENTER,
            env!("CARGO_PKG_VERSION"),
            egui::FontId::monospace(10.0),
            theme::SUBTLE,
        );
    });
}

fn body(ui: &mut Ui, hud: &mut Hud<'_>, rect: Rect, section: Section, compact: bool) {
    let pad = if compact { 16.0 } else { 28.0 };
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), 74.0));
    let footer = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 58.0), rect.max);
    let rows = Rect::from_min_max(
        egui::pos2(rect.left() + pad, header.bottom()),
        egui::pos2(rect.right() - pad, footer.top()),
    );

    ui.scope_builder(UiBuilder::new().max_rect(header.shrink2(vec2(pad, 20.0))), |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width((rect.width() - 2.0 * pad - 44.0).max(60.0));
                ui.label(RichText::new(hud.tr(section.title_key())).font(theme::bold(22.0)));
                ui.add(
                    egui::Label::new(
                        RichText::new(hud.tr(&format!("{}-description", section.title_key())))
                            .size(12.0)
                            .color(theme::MUTED),
                    )
                    .truncate(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::icon_button(ui, Icon::Close, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("window-close"))
                    .clicked()
                {
                    hud.ui.settings_section = None;
                }
            });
        });
    });

    let query = hud.ui.settings_search.trim().to_lowercase();
    ui.scope_builder(UiBuilder::new().max_rect(rows), |ui| {
        if !query.is_empty() && matching_sections(hud, &query).is_empty() {
            ui.add_space(24.0);
            ui.label(widgets::muted(hud.tr("settings-no-results")).size(13.0));
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match section {
                Section::General => general(ui, hud),
                Section::Interface => interface(ui, hud),
                Section::Viewport => viewport(ui, hud),
                Section::Input => input(ui, hud),
                Section::Performance => performance(ui, hud),
                Section::Mcp => mcp(ui, hud),
            });
    });

    ui.painter().hline(
        footer.x_range(),
        footer.top(),
        Stroke::new(1.0, Color32::from_white_alpha(20)),
    );
    ui.scope_builder(UiBuilder::new().max_rect(footer.shrink2(vec2(pad, 12.0))), |ui| {
        ui.horizontal_centered(|ui| {
            let reset = hud.tr("settings-reset-section");
            let reset_width = ui
                .painter()
                .layout_no_wrap(reset.clone(), egui::FontId::proportional(12.0), Color32::WHITE)
                .size()
                .x
                + 32.0;
            if widgets::dialog_button(ui, &reset, vec2(reset_width, 34.0), Look::Plain).clicked() {
                hud.settings.reset_section(section);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let done = vec2(if compact { 84.0 } else { 114.0 }, 34.0);
                if widgets::dialog_button(ui, &hud.tr("settings-done"), done, Look::Accent).clicked() {
                    hud.ui.settings_section = None;
                }
                if !compact {
                    ui.label(
                        RichText::new(hud.tr("settings-apply-now"))
                            .size(11.0)
                            .color(theme::SUBTLE),
                    );
                }
            });
        });
    });
}

/// One settings row: label (and hint) on the left, the control on the right.
///
/// The control is laid out first, so a long hint wraps in the width that is left of it.
pub(crate) fn row(ui: &mut Ui, label: &str, hint: Option<&str>, control: impl FnOnce(&mut Ui)) {
    const GAP: f32 = 16.0;
    const HINT: Color32 = Color32::from_rgb(0x8e, 0x98, 0xa6);
    let area = ui.available_rect_before_wrap();
    // A wide control (or a narrow window) puts the control under the text instead of beside it. The width of the
    // control is only known after it is drawn, so the decision uses the previous frame.
    let width_id = Id::unique(("settings-row-width", label));
    let last_width: f32 = ui.ctx().data(|data| data.get_temp(width_id)).unwrap_or(0.0);
    let stacked = last_width > area.width() * 0.6;

    let painter = ui.painter().clone();
    let text_at = |wrap: f32| {
        let label_galley = painter.layout(label.to_owned(), egui::FontId::proportional(13.0), theme::TEXT, wrap);
        let hint_galley =
            hint.map(|hint| painter.layout(hint.to_owned(), egui::FontId::proportional(11.0), HINT, wrap));
        let height = label_galley.size().y + hint_galley.as_ref().map_or(0.0, |galley| galley.size().y + 2.0);
        (label_galley, hint_galley, height)
    };
    let control_layout = egui::Layout::right_to_left(egui::Align::Center);
    let (label_galley, hint_galley, text_height, control_ui, height, top);
    if stacked {
        (label_galley, hint_galley, text_height) = text_at(area.width());
        let strip = Rect::from_min_size(area.min + vec2(0.0, text_height + 14.0), vec2(area.width(), 34.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(("settings-row", label))
                .max_rect(strip)
                .layout(control_layout),
        );
        control(&mut child);
        control_ui = child;
        height = text_height + 14.0 + 34.0 + 8.0;
        top = area.top() + 7.0;
    } else {
        let strip = Rect::from_min_size(area.min, vec2(area.width(), ROW_HEIGHT));
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(("settings-row", label))
                .max_rect(strip)
                .layout(control_layout),
        );
        control(&mut child);
        let wrap = (child.min_rect().left().min(area.right()) - GAP - area.left()).max(96.0);
        control_ui = child;
        (label_galley, hint_galley, text_height) = text_at(wrap);
        height = ROW_HEIGHT.max(text_height + 14.0);
        top = area.top() + (height - text_height) / 2.0;
    }
    let control_width = control_ui.min_rect().width();
    if (control_width - last_width).abs() > 0.5 {
        ui.ctx().data_mut(|data| data.insert_temp(width_id, control_width));
        ui.ctx().request_repaint();
    }

    let query: String = ui
        .ctx()
        .data(|data| data.get_temp(Id::unique("settings-query")))
        .unwrap_or_default();
    let row_rect = Rect::from_min_size(area.min, vec2(area.width(), height));
    if !query.is_empty() && label.to_lowercase().contains(&query) {
        // Bring the first match of a new search into view once, so scrolling afterwards stays free.
        let seen_id = Id::unique("settings-scrolled-to");
        let seen: String = ui.ctx().data(|data| data.get_temp(seen_id)).unwrap_or_default();
        if seen != query {
            ui.ctx().data_mut(|data| data.insert_temp(seen_id, query.clone()));
            ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
        }
        ui.painter().rect_filled(
            row_rect.shrink2(vec2(-8.0, 3.0)),
            8,
            ui.visuals().selection.bg_fill.gamma_multiply(0.16),
        );
    }
    let label_height = label_galley.size().y;
    ui.painter()
        .galley(egui::pos2(area.left(), top), label_galley, theme::TEXT);
    if let Some(galley) = hint_galley {
        ui.painter()
            .galley(egui::pos2(area.left(), top + label_height + 2.0), galley, HINT);
    }
    ui.painter().hline(
        row_rect.x_range(),
        row_rect.bottom(),
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
    ui.advance_cursor_after_rect(row_rect);
}

/// A number with at most two decimals and no trailing zeros: `25`, `0.3`, `6.25`.
fn trimmed(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A bordered, right-aligned monospace number field like the mockup's value inputs.
pub(crate) fn number_field(ui: &mut Ui, drag: DragValue<'_>) {
    egui::Frame::new()
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 0))
        .show(ui, |ui| {
            ui.set_width(84.0);
            ui.set_height(30.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let style = ui.style_mut();
                style.override_font_id = Some(egui::FontId::monospace(12.0));
                for state in [
                    &mut style.visuals.widgets.inactive,
                    &mut style.visuals.widgets.hovered,
                    &mut style.visuals.widgets.active,
                ] {
                    state.bg_fill = Color32::TRANSPARENT;
                    state.weak_bg_fill = Color32::TRANSPARENT;
                    state.bg_stroke = Stroke::NONE;
                }
                ui.add(drag);
            });
        });
}

fn general(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let settings: &mut Settings = hud.settings;
    row(ui, &i18n.tr("setting-language"), None, |ui| {
        let name = |language| match language {
            Language::English => i18n.tr("language-english"),
            Language::Polish => i18n.tr("language-polish"),
        };
        ComboBox::from_id_salt("language")
            .selected_text(name(settings.language))
            .width(170.0)
            .show_ui(ui, |ui| {
                for language in [Language::English, Language::Polish] {
                    ui.selectable_value(&mut settings.language, language, name(language));
                }
            });
    });
    row(ui, &i18n.tr("setting-start-screen"), None, |ui| {
        widgets::switch(ui, &mut settings.show_start_screen, true);
    });
    row(
        ui,
        &i18n.tr("setting-default-voxel-size"),
        Some(&i18n.tr("setting-default-voxel-size-hint")),
        |ui| {
            number_field(
                ui,
                DragValue::new(&mut settings.default_voxel_size_cm)
                    .range(0.5..=1000.0)
                    .speed(0.25)
                    .custom_formatter(|value, _| trimmed(value))
                    .suffix(" cm"),
            );
        },
    );
    // The browser keeps no recovery copies (see `files`).
    if crate::DESKTOP {
        row(
            ui,
            &i18n.tr("setting-autosave"),
            Some(&i18n.tr("setting-autosave-hint")),
            |ui| {
                widgets::switch(ui, &mut settings.autosave, true);
            },
        );
        row(ui, &i18n.tr("setting-autosave-interval"), None, |ui| {
            number_field(
                ui,
                DragValue::new(&mut settings.autosave_minutes)
                    .range(1..=60)
                    .suffix(" min"),
            );
        });
    }
    row(ui, &i18n.tr("setting-undo-steps"), None, |ui| {
        number_field(ui, DragValue::new(&mut settings.undo_steps).range(10..=2000));
    });
}

fn interface(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let settings: &mut Settings = hud.settings;
    row(
        ui,
        &i18n.tr("setting-accent"),
        Some(&i18n.tr("setting-accent-hint")),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for accent in Accent::ALL.into_iter().rev() {
                let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                let scale = if response.hovered() { 1.12 } else { 1.0 };
                ui.painter()
                    .circle_filled(rect.center(), 11.0 * scale, theme::accent(accent));
                if settings.accent == accent {
                    ui.painter()
                        .circle_stroke(rect.center(), 13.0, Stroke::new(2.0, Color32::WHITE));
                }
                if response.on_hover_text(i18n.tr(accent.message_key())).clicked() {
                    settings.accent = accent;
                }
            }
        },
    );
    // The scale applies on release: zooming while the handle is dragged moves the slider under the pointer.
    let draft = &mut hud.ui.scale_draft;
    row(ui, &i18n.tr("setting-ui-scale"), None, |ui| {
        let mut value = draft.unwrap_or(settings.ui_scale_percent);
        ui.label(widgets::mono(format!("{value}%"), 12.0));
        ui.spacing_mut().slider_width = 168.0;
        ui.visuals_mut().widgets.inactive.bg_fill = Color32::from_white_alpha(40);
        let response = ui.add(
            egui::Slider::new(&mut value, 75..=200)
                .step_by(25.0)
                .trailing_fill(true)
                .show_value(false),
        );
        if response.is_pointer_button_down_on() {
            *draft = Some(value);
        } else {
            *draft = None;
            if value != settings.ui_scale_percent {
                settings.ui_scale_percent = value;
            }
        }
    });
    row(
        ui,
        &i18n.tr("setting-translucent"),
        Some(&i18n.tr("setting-translucent-hint")),
        |ui| {
            widgets::switch(ui, &mut settings.translucent_panels, true);
        },
    );
    row(ui, &i18n.tr("setting-motion"), None, |ui| {
        widgets::segmented(
            ui,
            &mut settings.motion,
            &[
                (Motion::Full, i18n.tr("motion-full")),
                (Motion::Reduced, i18n.tr("motion-reduced")),
                (Motion::Off, i18n.tr("motion-off")),
            ],
            true,
        );
    });
    row(ui, &i18n.tr("setting-shortcut-letters"), None, |ui| {
        widgets::switch(ui, &mut settings.shortcut_letters, true);
    });
    row(
        ui,
        &i18n.tr("setting-custom-cursors"),
        Some(&i18n.tr("setting-custom-cursors-hint")),
        |ui| {
            widgets::switch(ui, &mut settings.custom_cursors, true);
        },
    );
    row(ui, &i18n.tr("setting-tooltip-delay"), None, |ui| {
        number_field(
            ui,
            DragValue::new(&mut settings.tooltip_delay_seconds)
                .range(0.0..=2.0)
                .speed(0.05)
                .suffix(" s")
                .custom_formatter(|value, _| trimmed(value)),
        );
    });
}

fn viewport(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let sample_counts = hud.sample_counts;
    let settings: &mut Settings = hud.settings;
    row(
        ui,
        &i18n.tr("setting-orbit-style"),
        Some(&i18n.tr("setting-orbit-style-hint")),
        |ui| {
            widgets::segmented(
                ui,
                &mut settings.orbit_style,
                &[
                    (OrbitStyle::Turntable, i18n.tr("orbit-turntable")),
                    (OrbitStyle::Free, i18n.tr("orbit-free")),
                ],
                true,
            );
        },
    );
    row(ui, &i18n.tr("setting-background"), None, |ui| {
        let [r, g, b] = settings.background;
        ui.label(widgets::mono(format!("#{r:02X}{g:02X}{b:02X}"), 12.0));
        ui.color_edit_button_srgb(&mut settings.background);
    });
    row(ui, &i18n.tr("setting-ground-grid"), None, |ui| {
        widgets::switch(ui, &mut settings.show_grid, true);
    });
    row(ui, &i18n.tr("setting-volume-bounds"), None, |ui| {
        widgets::switch(ui, &mut settings.show_volume_bounds, true);
    });
    row(ui, &i18n.tr("setting-fov"), None, |ui| {
        number_field(
            ui,
            DragValue::new(&mut settings.field_of_view_degrees)
                .range(20.0..=110.0)
                .speed(0.5)
                .custom_formatter(|value, _| trimmed(value))
                .suffix("°"),
        );
    });
    row(ui, &i18n.tr("setting-antialiasing"), None, |ui| {
        let options: Vec<(u32, String)> = sample_counts
            .iter()
            .map(|count| {
                (
                    *count,
                    if *count == 1 {
                        i18n.tr("antialiasing-off")
                    } else {
                        format!("{count}×")
                    },
                )
            })
            .collect();
        widgets::segmented(ui, &mut settings.msaa_samples, &options, true);
    });
}

fn input(ui: &mut Ui, hud: &mut Hud<'_>) {
    crate::settings_input::show(ui, hud);
}

fn performance(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let adapter = hud.adapter_name;
    let adapters = hud.adapters;
    let settings: &mut Settings = hud.settings;
    let backends: Vec<(Backend, String)> = rustium_render::selectable_backends(adapters)
        .into_iter()
        .map(Backend::from)
        .map(|backend| {
            let name = match backend {
                Backend::Automatic => i18n.tr("backend-automatic"),
                Backend::Vulkan => "Vulkan".to_owned(),
                Backend::DirectX12 => "DirectX 12".to_owned(),
            };
            (backend, name)
        })
        .collect();
    // A saved choice this machine cannot offer (settings copied from another PC) is shown as it is, so it can be
    // changed; startup already fell back to Automatic.
    let mut backends = backends;
    if !backends.iter().any(|(backend, _)| *backend == settings.backend) {
        backends.push((settings.backend, format!("{:?}", settings.backend)));
    }
    let wanted = rustium_render::GraphicsBackend::from(settings.backend);
    let mut devices: Vec<&str> = Vec::new();
    for choice in adapters {
        if wanted.offers(choice) && !devices.contains(&choice.name.as_str()) {
            devices.push(&choice.name);
        }
    }
    // The browser chooses the graphics device and API itself.
    if crate::DESKTOP {
        row(
            ui,
            &i18n.tr("setting-device"),
            Some(&i18n.tr("settings-restart")),
            |ui| {
                let automatic = i18n.tr("backend-automatic");
                let shown = settings.gpu_adapter.clone().unwrap_or_else(|| automatic.clone());
                ComboBox::from_id_salt("gpu-device")
                    .selected_text(shown)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.gpu_adapter, None, automatic);
                        for name in &devices {
                            ui.selectable_value(&mut settings.gpu_adapter, Some((*name).to_owned()), *name);
                        }
                    })
                    .response
                    .on_hover_text(format!("{}: {adapter}", i18n.tr("setting-device-current")));
            },
        );
        row(
            ui,
            &i18n.tr("setting-backend"),
            Some(&i18n.tr("settings-restart")),
            |ui| {
                let before = settings.backend;
                let work_in_progress = i18n.tr("backend-work-in-progress");
                widgets::segmented_with_unavailable(ui, &mut settings.backend, &backends, true, |backend| {
                    let available = rustium_render::GraphicsBackend::from(backend).is_available();
                    (!available).then(|| work_in_progress.clone())
                });
                let wanted = rustium_render::GraphicsBackend::from(settings.backend);
                if settings.backend != before
                    && let Some(name) = &settings.gpu_adapter
                    && !adapters
                        .iter()
                        .any(|choice| choice.name == *name && wanted.offers(choice))
                {
                    settings.gpu_adapter = None;
                }
            },
        );
        row(
            ui,
            &i18n.tr("setting-pt-device"),
            Some(&i18n.tr("setting-pt-device-hint")),
            |ui| {
                ui.add_enabled_ui(false, |ui| {
                    ComboBox::from_id_salt("pt-device")
                        .selected_text(i18n.tr("pt-device-same"))
                        .width(220.0)
                        .show_ui(ui, |_| {});
                })
                .response
                .on_disabled_hover_text(i18n.tr("setting-pt-device-hint"));
            },
        );
    }
    let ray_hint = i18n.tr(if hud.ray_query_supported {
        "setting-ray-tracing-hint"
    } else {
        "setting-ray-tracing-unsupported"
    });
    row(ui, &i18n.tr("setting-ray-tracing"), Some(&ray_hint), |ui| {
        widgets::segmented(
            ui,
            &mut settings.ray_tracing,
            &[
                (RayTracingMode::Automatic, i18n.tr("backend-automatic")),
                (RayTracingMode::Software, i18n.tr("ray-tracing-software")),
                (RayTracingMode::Hardware, i18n.tr("ray-tracing-hardware")),
            ],
            true,
        );
    });
    row(ui, &i18n.tr("setting-vsync"), None, |ui| {
        widgets::switch(ui, &mut settings.vsync, true);
    });
    row(ui, &i18n.tr("setting-frame-limit"), None, |ui| {
        let name = |limit| match limit {
            FrameLimit::MatchDisplay => i18n.tr("frame-limit-display"),
            FrameLimit::Fps60 => "60 FPS".to_owned(),
            FrameLimit::Fps144 => "144 FPS".to_owned(),
            FrameLimit::Unlimited => i18n.tr("frame-limit-unlimited"),
        };
        ComboBox::from_id_salt("frame-limit")
            .selected_text(name(settings.frame_limit))
            .width(170.0)
            .show_ui(ui, |ui| {
                for limit in [
                    FrameLimit::MatchDisplay,
                    FrameLimit::Fps60,
                    FrameLimit::Fps144,
                    FrameLimit::Unlimited,
                ] {
                    ui.selectable_value(&mut settings.frame_limit, limit, name(limit));
                }
            });
    });
}

fn mcp(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let status = hud.mcp_status;
    let status_text = match status {
        McpStatus::Failed(message) => format!("{}: {message}", status.text(i18n)),
        other => other.text(i18n),
    };
    let settings: &mut Settings = hud.settings;
    row(ui, &i18n.tr("setting-mcp-enabled"), Some(&status_text), |ui| {
        widgets::switch(ui, &mut settings.mcp.enabled, true);
    });
    row(
        ui,
        &i18n.tr("setting-mcp-transport"),
        Some(&i18n.tr("setting-mcp-transport-hint")),
        |ui| {
            widgets::segmented(
                ui,
                &mut settings.mcp.transport,
                &[
                    (McpTransport::Stdio, "stdio".to_owned()),
                    (McpTransport::Http, "HTTP".to_owned()),
                ],
                true,
            );
        },
    );
    row(
        ui,
        &i18n.tr("setting-mcp-port"),
        Some(&i18n.tr("setting-mcp-port-hint")),
        |ui| {
            number_field(ui, DragValue::new(&mut settings.mcp.port).range(1024..=65535).speed(1));
        },
    );
    ui.add_space(14.0);
    ui.label(
        RichText::new(i18n.tr("mcp-allowed-tools").to_uppercase())
            .size(10.0)
            .color(theme::SUBTLE),
    );
    ui.add_space(2.0);
    // Two columns of compact rows, like the mockup, so the client setup row stays on screen.
    let column = (ui.available_width() - 28.0) / 2.0;
    for pair in Permission::ALL.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            for permission in pair {
                ui.allocate_ui_with_layout(
                    vec2(column, 38.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_size(vec2(column, 38.0));
                        ui.label(i18n.tr(permission_key(*permission)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            widgets::switch(ui, settings.mcp.permissions.flag_mut(*permission), true);
                        });
                    },
                );
            }
        });
    }
    ui.add_space(6.0);
    let line = ui.available_rect_before_wrap();
    ui.painter().hline(
        line.x_range(),
        line.top(),
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
    let mut copy = false;
    row(
        ui,
        &i18n.tr("mcp-copy-config"),
        Some(&i18n.tr("setting-mcp-copy-hint")),
        |ui| {
            copy = widgets::text_button(ui, &i18n.tr("mcp-copy-button"), 32.0, Look::On, true).clicked();
        },
    );
    if copy {
        hud::copy_client_config(ui.ctx(), hud);
    }
}
