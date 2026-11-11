//! Start screen: New / Open, recent files and the startup progress line.

use egui::load::SizedTexture;
use egui::{Align2, Color32, Context, Id, Key, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};

use crate::hud::widgets::{self, Look};
use crate::hud::{AppCommand, Glass, Hud};
use crate::icons::Icon;
use crate::settings::Motion;
use crate::theme;

const CARD: egui::Vec2 = vec2(760.0, 460.0);
const STEPS: [&str; 4] = [
    "splash-step-gpu",
    "splash-step-palettes",
    "splash-step-workspace",
    "splash-step-ready",
];
const LOAD_SECONDS: f32 = 2.8;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    if !hud.ui.splash_open {
        return;
    }
    let now = ctx.input(|input| input.time);
    let opened = *hud.ui.splash_opened_at.get_or_insert(now);
    let elapsed = crate::hud::seconds_between(opened, now);
    let animate = hud.settings.motion != Motion::Off;
    let load = if animate {
        (elapsed / LOAD_SECONDS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if animate {
        ctx.request_repaint();
    }

    let screen = ctx.content_rect();
    let card = Rect::from_center_size(screen.center(), CARD);
    hud.glass.push(Glass {
        rect: screen,
        radius: 0.0,
    });
    egui::Area::new(Id::unique("splash"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (_, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(screen, 0, theme::SCRIM);
            if backdrop.clicked() || ui.input(|input| input.key_pressed(Key::Escape)) {
                hud.ui.splash_open = false;
            }
            ui.painter().rect(
                card,
                16,
                theme::PANEL_SOLID,
                Stroke::new(1.0, theme::BORDER),
                StrokeKind::Inside,
            );
            ui.scope_builder(UiBuilder::new().max_rect(card).sense(Sense::click()), |ui| {
                left_side(ui, hud, card, elapsed, animate);
                right_side(ui, hud, card, load);
            });
        });
}

fn left_side(ui: &mut egui::Ui, hud: &Hud<'_>, card: Rect, elapsed: f32, animate: bool) {
    let float = if animate {
        (elapsed * std::f32::consts::TAU / 6.0).sin() * 4.0 - 4.0
    } else {
        0.0
    };
    // Soft accent glow under the floating logo: stacked translucent ellipses.
    let glow_centre = card.min + vec2(180.0, 328.0);
    let accent = hud.accent();
    for step in 0..12 {
        let t = step as f32 / 12.0;
        let radius = vec2(110.0, 28.0) * (1.0 - t * 0.85);
        ui.painter().add(egui::Shape::ellipse_filled(
            glow_centre,
            radius,
            accent.gamma_multiply(0.035),
        ));
    }
    let logo = Rect::from_min_size(card.min + vec2(60.0, 84.0 + float), vec2(240.0, 240.0));
    egui::Image::from_texture(SizedTexture::new(hud.logo.id(), logo.size())).paint_at(ui, logo);
}

fn right_side(ui: &mut egui::Ui, hud: &mut Hud<'_>, card: Rect, load: f32) {
    let content = Rect::from_min_max(card.min + vec2(372.0, 44.0), card.max - vec2(36.0, 18.0));
    let close = Rect::from_min_size(pos2(card.right() - 40.0, card.top() + 12.0), vec2(28.0, 28.0));
    let close_response = ui.interact(close, Id::unique("splash-close"), Sense::click());
    let close_color = if close_response.hovered() {
        Color32::WHITE
    } else {
        theme::MUTED
    };
    Icon::Close.paint(ui.painter(), close.shrink(6.0), close_color);
    if close_response.on_hover_text(hud.tr("splash-close")).clicked() {
        hud.ui.splash_open = false;
    }

    ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            RichText::new(hud.tr("app-name"))
                .font(theme::bold(46.0))
                .color(theme::TEXT),
        );
        ui.horizontal(|ui| {
            ui.label(RichText::new(hud.tr("splash-subtitle")).color(theme::MUTED).size(13.0));
            egui::Frame::new()
                .stroke(Stroke::new(1.0, Color32::from_white_alpha(36)))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.label(widgets::mono(env!("CARGO_PKG_VERSION"), 11.0).color(theme::MUTED))
                });
        });
        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let width = (content.width() - 8.0) / 2.0;
            if action(
                ui,
                Icon::Attach,
                &hud.tr("splash-new"),
                "Ctrl N",
                width,
                Look::Accent,
                true,
            )
            .clicked()
                || ui.input(|input| input.modifiers.command && input.key_pressed(Key::N))
            {
                hud.send(AppCommand::NewScene);
                hud.ui.splash_open = false;
            }
            if action(
                ui,
                Icon::Folder,
                &hud.tr("splash-open"),
                "Ctrl O",
                width,
                Look::Plain,
                true,
            )
            .clicked()
                || ui.input(|input| input.modifiers.command && input.key_pressed(Key::O))
            {
                hud.send(AppCommand::Open(None));
            }
        });
        ui.add_space(18.0);
        ui.label(
            RichText::new(hud.tr("splash-recent").to_uppercase())
                .size(10.0)
                .color(theme::SUBTLE),
        );
        let list_height = (content.bottom() - 40.0 - ui.cursor().top()).max(0.0);
        recent_list(ui, hud, list_height);
    });

    // Startup progress and the start-screen preference along the bottom.
    let bar = Rect::from_min_size(
        pos2(content.left(), content.bottom() - 30.0),
        vec2(content.width(), 2.0),
    );
    ui.painter().rect_filled(bar, 1, Color32::from_white_alpha(26));
    ui.painter().rect_filled(
        Rect::from_min_size(bar.min, vec2(bar.width() * load, 2.0)),
        1,
        hud.accent(),
    );
    // Thresholds follow the mockup keyframes (30 %, 55 %, 88 %).
    let step = STEPS[match load {
        l if l >= 0.88 => 3,
        l if l >= 0.55 => 2,
        l if l >= 0.3 => 1,
        _ => 0,
    }];
    let status_color = if load >= 1.0 { theme::TEXT } else { theme::MUTED };
    ui.painter().text(
        pos2(content.left(), content.bottom() - 10.0),
        Align2::LEFT_CENTER,
        hud.tr(step),
        egui::FontId::monospace(11.0),
        status_color,
    );
    let toggle = Rect::from_min_size(
        pos2(content.right() - 190.0, content.bottom() - 20.0),
        vec2(190.0, 20.0),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(toggle)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        |ui| {
            if widgets::switch(ui, &mut hud.settings.show_start_screen, true).changed() {
                hud.send(AppCommand::SettingsChanged);
            }
            ui.label(
                RichText::new(hud.tr("splash-show-at-startup"))
                    .size(11.0)
                    .color(theme::MUTED),
            );
        },
    );
}

/// The recent files: file name in mono, the Scene's root name on the right; missing files are greyed out.
fn recent_list(ui: &mut egui::Ui, hud: &mut Hud<'_>, max_height: f32) {
    if hud.ui.recent.is_empty() {
        ui.label(RichText::new(hud.tr("splash-no-recent")).size(12.0).color(theme::MUTED));
        return;
    }
    let mut chosen = None;
    egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for item in &hud.ui.recent {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            let (name_color, note_color) = if item.exists {
                (theme::TEXT, theme::SUBTLE)
            } else {
                (theme::DISABLED, theme::DISABLED)
            };
            if response.hovered() {
                ui.painter().rect_filled(rect, 7, theme::HOVER);
            }
            Icon::Cube.paint(
                ui.painter(),
                Rect::from_min_size(rect.left_center() + vec2(8.0, -8.0), vec2(16.0, 16.0)),
                note_color,
            );
            let file_name = item
                .file
                .path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            ui.painter().text(
                rect.left_center() + vec2(32.0, 0.0),
                Align2::LEFT_CENTER,
                file_name,
                egui::FontId::monospace(12.0),
                name_color,
            );
            if !item.file.name.is_empty() {
                ui.painter().text(
                    rect.right_center() - vec2(8.0, 0.0),
                    Align2::RIGHT_CENTER,
                    &item.file.name,
                    egui::FontId::proportional(11.0),
                    note_color,
                );
            }
            let response = response.on_hover_text(if item.exists {
                item.file.path.display().to_string()
            } else {
                hud.tr("splash-recent-missing")
            });
            if response.clicked() {
                chosen = Some(item.file.path.clone());
            }
        }
    });
    if let Some(path) = chosen {
        hud.send(AppCommand::Open(Some(path)));
    }
}

fn action(
    ui: &mut egui::Ui,
    icon: Icon,
    text: &str,
    shortcut: &str,
    width: f32,
    look: Look,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), sense);
    let accent = ui.visuals().selection.bg_fill;
    let (fill, stroke, color) = match (look, enabled, response.hovered()) {
        (_, false, _) => (
            Color32::from_white_alpha(8),
            Color32::from_white_alpha(20),
            theme::DISABLED,
        ),
        (Look::Accent, true, hovered) => (
            if hovered { accent.gamma_multiply(1.08) } else { accent },
            accent,
            theme::ON_ACCENT,
        ),
        (_, true, true) => (
            Color32::from_white_alpha(28),
            Color32::from_white_alpha(31),
            Color32::WHITE,
        ),
        (_, true, false) => (
            Color32::from_white_alpha(13),
            Color32::from_white_alpha(31),
            theme::TEXT,
        ),
    };
    ui.painter()
        .rect(rect, 9, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    icon.paint(
        ui.painter(),
        Rect::from_min_size(rect.left_center() + vec2(12.0, -8.0), vec2(16.0, 16.0)),
        color,
    );
    ui.painter().text(
        rect.left_center() + vec2(38.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        theme::bold(13.0),
        color,
    );
    ui.painter().text(
        rect.right_center() - vec2(12.0, 0.0),
        Align2::RIGHT_CENTER,
        shortcut,
        egui::FontId::monospace(10.0),
        color.gamma_multiply(0.8),
    );
    response
}
