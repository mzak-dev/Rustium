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
use crate::settings::{Accent, Backend, FrameLimit, Language, Motion, OrbitStyle, Settings};
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
            Self::Interface => Icon::Palette,
            Self::Viewport => Icon::Cube,
            Self::Input => Icon::Grid,
            Self::Performance => Icon::Attach,
            Self::Mcp => Icon::Folder,
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

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(section) = hud.ui.settings_section else {
        return;
    };
    let before = hud.settings.clone();
    let screen = ctx.content_rect();
    let card = Rect::from_center_size(screen.center(), CARD);
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
            if backdrop.clicked() || ui.input(|input| input.key_pressed(Key::Escape)) {
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
                let nav = Rect::from_min_size(card.min, vec2(212.0, card.height()));
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
                body(ui, hud, content, section);
            });
        });
    if *hud.settings != before {
        hud.send(AppCommand::SettingsChanged);
    }
}

fn navigation(ui: &mut Ui, hud: &mut Hud<'_>, rect: Rect, current: Section) {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        egui::Frame::new()
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (icon, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    Icon::Search.paint(ui.painter(), icon, theme::SUBTLE);
                    let hint = hud.tr("settings-search");
                    ui.add(
                        egui::TextEdit::singleline(&mut hud.ui.settings_search)
                            .hint_text(hint)
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        ui.add_space(10.0);
        let query = hud.ui.settings_search.to_lowercase();
        for section in Section::ALL {
            let matches = query.is_empty()
                || hud.tr(section.title_key()).to_lowercase().contains(&query)
                || section
                    .row_keys()
                    .iter()
                    .any(|key| hud.tr(key).to_lowercase().contains(&query));
            if !matches {
                continue;
            }
            let (row, response) = ui.allocate_exact_size(vec2(rect.width(), 36.0), Sense::click());
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

fn body(ui: &mut Ui, hud: &mut Hud<'_>, rect: Rect, section: Section) {
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), 74.0));
    let footer = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 58.0), rect.max);
    let rows = Rect::from_min_max(
        egui::pos2(rect.left() + 28.0, header.bottom()),
        egui::pos2(rect.right() - 28.0, footer.top()),
    );

    ui.scope_builder(UiBuilder::new().max_rect(header.shrink2(vec2(28.0, 20.0))), |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(hud.tr(section.title_key())).font(theme::bold(22.0)));
                ui.label(
                    RichText::new(hud.tr(&format!("{}-description", section.title_key())))
                        .size(12.0)
                        .color(theme::MUTED),
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

    ui.scope_builder(UiBuilder::new().max_rect(rows), |ui| {
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
    ui.scope_builder(UiBuilder::new().max_rect(footer.shrink2(vec2(28.0, 13.0))), |ui| {
        ui.horizontal_centered(|ui| {
            if widgets::text_button(ui, &hud.tr("settings-reset-section"), 32.0, Look::On, true).clicked() {
                hud.settings.reset_section(section);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::text_button(ui, &hud.tr("settings-done"), 32.0, Look::Accent, true).clicked() {
                    hud.ui.settings_section = None;
                }
                ui.label(
                    RichText::new(hud.tr("settings-apply-now"))
                        .size(11.0)
                        .color(theme::SUBTLE),
                );
            });
        });
    });
}

/// One settings row: label (and hint) on the left, the control on the right.
fn row(ui: &mut Ui, label: &str, hint: Option<&str>, control: impl FnOnce(&mut Ui)) {
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.set_min_height(50.0);
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(label).size(13.0));
            if let Some(hint) = hint {
                ui.label(
                    RichText::new(hint)
                        .size(11.0)
                        .color(Color32::from_rgb(0x8e, 0x98, 0xa6)),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
    let bottom = ui.cursor().top();
    let width = ui.available_width();
    let _ = top;
    ui.painter().hline(
        ui.cursor().left()..=ui.cursor().left() + width,
        bottom,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
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
            ui.add(
                DragValue::new(&mut settings.default_voxel_size_cm)
                    .range(0.5..=1000.0)
                    .speed(0.25)
                    .suffix(" cm"),
            );
        },
    );
    row(
        ui,
        &i18n.tr("setting-autosave"),
        Some(&i18n.tr("setting-autosave-hint")),
        |ui| {
            widgets::switch(ui, &mut settings.autosave, true);
        },
    );
    row(ui, &i18n.tr("setting-autosave-interval"), None, |ui| {
        ui.add(
            DragValue::new(&mut settings.autosave_minutes)
                .range(1..=60)
                .suffix(" min"),
        );
    });
    row(ui, &i18n.tr("setting-undo-steps"), None, |ui| {
        ui.add(DragValue::new(&mut settings.undo_steps).range(10..=2000));
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
    row(ui, &i18n.tr("setting-ui-scale"), None, |ui| {
        ui.label(widgets::mono(format!("{}%", settings.ui_scale_percent), 12.0));
        ui.add(
            egui::Slider::new(&mut settings.ui_scale_percent, 75..=200)
                .step_by(25.0)
                .show_value(false),
        );
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
    row(ui, &i18n.tr("setting-tooltip-delay"), None, |ui| {
        ui.add(
            DragValue::new(&mut settings.tooltip_delay_seconds)
                .range(0.0..=2.0)
                .speed(0.05)
                .suffix(" s")
                .max_decimals(2),
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
        ui.color_edit_button_srgb(&mut settings.background);
        let [r, g, b] = settings.background;
        ui.label(widgets::mono(format!("#{r:02X}{g:02X}{b:02X}"), 12.0));
    });
    row(ui, &i18n.tr("setting-ground-grid"), None, |ui| {
        widgets::switch(ui, &mut settings.show_grid, true);
    });
    row(ui, &i18n.tr("setting-volume-bounds"), None, |ui| {
        widgets::switch(ui, &mut settings.show_volume_bounds, true);
    });
    row(ui, &i18n.tr("setting-fov"), None, |ui| {
        ui.add(
            DragValue::new(&mut settings.field_of_view_degrees)
                .range(20.0..=110.0)
                .speed(0.5)
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
    let i18n = hud.i18n;
    let settings: &mut Settings = hud.settings;
    row(ui, &i18n.tr("setting-keymap"), None, |ui| {
        ui.add_enabled(
            false,
            egui::Button::new(i18n.tr("keymap-default")).min_size(vec2(170.0, 30.0)),
        );
    });
    let bindings = [
        (i18n.tr("tool-attach"), "B"),
        (i18n.tr("tool-erase"), "E"),
        (i18n.tr("tool-paint"), "P"),
        (i18n.tr("tool-fill"), "F"),
        (i18n.tr("tool-pick"), "I"),
        (i18n.tr("tool-move"), "G"),
        (i18n.tr("hint-tool-wheel"), "RMB"),
        (i18n.tr("hint-orbit"), "MMB"),
        (i18n.tr("binding-color-slots"), "1 – 0"),
        (i18n.tr("binding-search"), "Ctrl K"),
    ];
    egui::Grid::new("bindings")
        .num_columns(2)
        .spacing(vec2(28.0, 0.0))
        .show(ui, |ui| {
            for pair in bindings.chunks(2) {
                for (label, key) in pair {
                    ui.horizontal(|ui| {
                        ui.set_min_size(vec2(270.0, 36.0));
                        ui.label(label.as_str());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::Frame::new()
                                .fill(Color32::from_white_alpha(15))
                                .stroke(Stroke::new(1.0, Color32::from_white_alpha(46)))
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(8, 3))
                                .show(ui, |ui| ui.label(widgets::mono(*key, 11.0).color(theme::TEXT)));
                        });
                    });
                }
                ui.end_row();
            }
        });
    row(ui, &i18n.tr("setting-invert-zoom"), None, |ui| {
        widgets::switch(ui, &mut settings.invert_zoom, true);
    });
    row(
        ui,
        &i18n.tr("setting-rebind"),
        Some(&i18n.tr("setting-rebind-hint")),
        |_| {},
    );
}

fn performance(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let adapter = hud.adapter_name;
    let settings: &mut Settings = hud.settings;
    row(ui, &i18n.tr("setting-device"), None, |ui| {
        ui.label(widgets::mono(adapter, 11.0));
    });
    row(
        ui,
        &i18n.tr("setting-backend"),
        Some(&i18n.tr("settings-restart")),
        |ui| {
            widgets::segmented(
                ui,
                &mut settings.backend,
                &[
                    (Backend::Automatic, i18n.tr("backend-automatic")),
                    (Backend::Vulkan, "Vulkan".to_owned()),
                    (Backend::DirectX12, "DirectX 12".to_owned()),
                ],
                true,
            );
        },
    );
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
            ui.add(DragValue::new(&mut settings.mcp.port).range(1024..=65535).speed(1));
        },
    );
    for permission in Permission::ALL {
        row(ui, &i18n.tr(permission_key(permission)), None, |ui| {
            widgets::switch(ui, settings.mcp.permissions.flag_mut(permission), true);
        });
    }
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
