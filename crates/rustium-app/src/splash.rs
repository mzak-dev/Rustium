//! Start screen: New / Open, recent files and the startup progress line.

use egui::load::SizedTexture;
use egui::{Align2, Color32, Context, Id, Key, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};

use crate::hud::widgets::{self, Look};
use crate::hud::{AppCommand, Glass, Hud};
use crate::icons::Icon;
use crate::keymap::Action;
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
/// Space kept between the card and the window edge.
const CARD_GAP: f32 = 12.0;
/// Width of the art on the left; without room for it the card shows only the actions.
const ART_WIDTH: f32 = 360.0;

/// The card size that fits the window.
fn card_size(screen: Rect) -> egui::Vec2 {
    vec2(
        CARD.x.min((screen.width() - 2.0 * CARD_GAP).max(300.0)),
        CARD.y.min((screen.height() - 2.0 * CARD_GAP).max(240.0)),
    )
}

/// An opacity in `0.0..=255.0` as a color channel.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the u8 range first"
)]
fn alpha(value: f32) -> u8 {
    value.clamp(0.0, 255.0).round() as u8
}

fn has_art(card: Rect) -> bool {
    card.width() >= 640.0
}

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    if !hud.ui.splash_open {
        return;
    }
    let now = ctx.input(|input| input.time);
    let opened = *hud.ui.splash_opened_at.get_or_insert(now);
    let elapsed = crate::hud::seconds_between(opened, now);
    let animate = hud.settings.motion == Motion::Full;
    let load = if animate {
        (elapsed / LOAD_SECONDS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if animate {
        ctx.request_repaint();
    }

    let screen = ctx.content_rect();
    let card = Rect::from_center_size(screen.center(), card_size(screen));
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
                if has_art(card) {
                    iso_lines(ui, card);
                    left_side(ui, hud, card, elapsed, animate);
                }
                right_side(ui, hud, card, load);
            });
        });
}

/// The isometric line pattern from the mockup: two families of lines at 26.565 degrees, fading out to the right.
fn iso_lines(ui: &egui::Ui, card: Rect) {
    let painter = ui.painter().with_clip_rect(card.shrink(1.0));
    let fade_from = card.left() + card.width() * 0.30;
    let fade_to = card.left() + card.width() * 0.62;
    let angle = 26.565_f32.to_radians();
    let (sin, cos) = (angle.sin(), angle.cos());
    let width = fade_to - card.left();
    let height = card.height();
    for sign in [1.0_f32, -1.0] {
        // Lines run along (cos, sign * sin) and sit 20 px apart along their normal.
        let normal = vec2(-sign * sin, cos);
        let corner = |x: f32, y: f32| normal.dot(vec2(x, y));
        let (low, high) = [
            corner(0.0, 0.0),
            corner(width, 0.0),
            corner(0.0, height),
            corner(width, height),
        ]
        .into_iter()
        .fold((f32::MAX, f32::MIN), |(low, high), value| {
            (low.min(value), high.max(value))
        });
        let direction = vec2(cos, sign * sin);
        let mut offset = (low / 20.0).floor() * 20.0;
        while offset <= high {
            let origin = card.min + normal * offset;
            // Walk along the line in short pieces so each can take its own fade.
            for step in -40_i32..120 {
                let from = origin + direction * (step as f32 * 10.0);
                let to = from + direction * 10.0;
                let x = f32::midpoint(from.x, to.x);
                let fade = ((fade_to - x) / (fade_to - fade_from)).clamp(0.0, 1.0);
                if fade > 0.0 {
                    painter.line_segment(
                        [from, to],
                        Stroke::new(1.0, Color32::from_white_alpha(alpha(11.5 * fade))),
                    );
                }
            }
            offset += 20.0;
        }
    }
}

/// Glass cubes of the mockup: position in the art, animation delay and whether it is an accent cube.
const CUBES: [(f32, f32, f32, bool); 10] = [
    (300.0, 96.0, 0.2, false),
    (300.0, 71.0, 0.5, false),
    (322.0, 107.0, 0.35, false),
    (40.0, 118.0, 0.65, false),
    (96.0, 372.0, 0.1, false),
    (96.0, 347.0, 0.75, true),
    (74.0, 383.0, 0.25, false),
    (118.0, 383.0, 0.4, false),
    (140.0, 394.0, 0.55, false),
    (286.0, 372.0, 0.85, true),
];

fn glass_cube(painter: &egui::Painter, centre: egui::Pos2, accent: Option<Color32>, opacity: f32) {
    let faces: [([egui::Vec2; 4], f32); 3] = [
        (
            [vec2(0.0, -11.0), vec2(22.0, 0.0), vec2(0.0, 11.0), vec2(-22.0, 0.0)],
            0.08,
        ),
        (
            [vec2(-22.0, 0.0), vec2(0.0, 11.0), vec2(0.0, 36.0), vec2(-22.0, 25.0)],
            0.045,
        ),
        (
            [vec2(22.0, 0.0), vec2(0.0, 11.0), vec2(0.0, 36.0), vec2(22.0, 25.0)],
            0.025,
        ),
    ];
    let accent_fill = [0.55, 0.38, 0.24];
    for (index, (points, glass)) in faces.into_iter().enumerate() {
        let points: Vec<egui::Pos2> = points.iter().map(|point| centre + *point).collect();
        let (fill, stroke) = match accent {
            Some(color) => (
                color.gamma_multiply(accent_fill[index] * opacity),
                color.gamma_multiply(opacity),
            ),
            None => (
                Color32::from_white_alpha(alpha(glass * 255.0 * opacity)),
                Color32::from_white_alpha(alpha(0.16 * 255.0 * opacity)),
            ),
        };
        painter.add(egui::Shape::convex_polygon(points, fill, Stroke::new(1.0, stroke)));
    }
}

fn left_side(ui: &mut egui::Ui, hud: &Hud<'_>, card: Rect, elapsed: f32, animate: bool) {
    let accent = hud.accent();
    let painter = ui.painter().with_clip_rect(card.shrink(1.0));
    for (x, y, delay, accented) in CUBES {
        let progress = if animate {
            ((elapsed - delay) / 0.55).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let eased = crate::hud::ease_out(progress);
        let centre = card.min + vec2(x, y - 24.0 * (1.0 - eased));
        glass_cube(&painter, centre, accented.then_some(accent), eased);
    }
    let float = if animate {
        (elapsed * std::f32::consts::TAU / 6.0).sin() * 4.0 - 4.0
    } else {
        0.0
    };
    // Soft accent glow under the floating logo: stacked translucent ellipses that breathe.
    let glow_centre = card.min + vec2(180.0, 328.0);
    let pulse = if animate {
        0.725 + 0.175 * (elapsed * std::f32::consts::TAU / 6.0).cos()
    } else {
        0.9
    };
    for step in 0..12 {
        let t = step as f32 / 12.0;
        let radius = vec2(110.0, 28.0) * (1.0 - t * 0.85) * (0.86 + 0.14 * pulse / 0.9);
        painter.add(egui::Shape::ellipse_filled(
            glow_centre,
            radius,
            accent.gamma_multiply(0.035 * pulse / 0.9),
        ));
    }
    // The logo fades and scales in once.
    let intro = if animate {
        crate::hud::ease_out(((elapsed - 0.15) / 0.95).clamp(0.0, 1.0))
    } else {
        1.0
    };
    let size = 240.0 * (0.7 + 0.3 * intro);
    let logo = Rect::from_center_size(card.min + vec2(180.0, 204.0 + float), vec2(size, size));
    egui::Image::from_texture(SizedTexture::new(hud.logo.id(), logo.size()))
        .tint(Color32::from_white_alpha(alpha(255.0 * intro)))
        .paint_at(ui, logo);
}

fn right_side(ui: &mut egui::Ui, hud: &mut Hud<'_>, card: Rect, load: f32) {
    let left = if has_art(card) { ART_WIDTH + 12.0 } else { 36.0 };
    let top = if card.height() >= 420.0 { 40.0 } else { 26.0 };
    let content = Rect::from_min_max(card.min + vec2(left, top), card.max - vec2(36.0, 18.0));
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
        // Spacing is explicit so the blocks sit where the mockup puts them.
        ui.spacing_mut().item_spacing.y = 0.0;
        let title = if card.height() >= 420.0 { 46.0 } else { 36.0 };
        ui.label(
            RichText::new(hud.tr("app-name"))
                .font(theme::bold(title))
                .line_height(Some(title))
                .color(theme::TEXT),
        );
        ui.add_space(8.0);
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
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let width = (content.width() - 8.0) / 2.0;
            if action(
                ui,
                Icon::Attach,
                &hud.tr("splash-new"),
                &hud.settings.keymap.label(Action::NewScene),
                width,
                Look::Accent,
                true,
            )
            .clicked()
                || hud
                    .settings
                    .keymap
                    .chord(Action::NewScene)
                    .is_some_and(|chord| ui.input(|input| chord.pressed(input)))
            {
                hud.send(AppCommand::NewScene);
                hud.ui.splash_open = false;
            }
            if action(
                ui,
                Icon::Folder,
                &hud.tr("splash-open"),
                &hud.settings.keymap.label(Action::OpenFile),
                width,
                Look::Plain,
                true,
            )
            .clicked()
                || hud
                    .settings
                    .keymap
                    .chord(Action::OpenFile)
                    .is_some_and(|chord| ui.input(|input| chord.pressed(input)))
            {
                hud.send(AppCommand::Open(None));
            }
        });
        ui.add_space(20.0);
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 8,
                ..egui::Margin::ZERO
            })
            .show(ui, |ui| {
                ui.label(
                    RichText::new(hud.tr("splash-recent").to_uppercase())
                        .size(10.0)
                        .color(theme::SUBTLE),
                );
            });
        ui.add_space(4.0);
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
            let text = hud.tr("splash-show-at-startup");
            if widgets::checkbox(ui, &mut hud.settings.show_start_screen, &text).changed() {
                hud.send(AppCommand::SettingsChanged);
            }
        },
    );
}

/// The recent files: file name in mono, the Scene's root name on the right; missing files are greyed out.
fn recent_list(ui: &mut egui::Ui, hud: &mut Hud<'_>, max_height: f32) {
    if hud.ui.recent.is_empty() {
        if max_height < 18.0 {
            return;
        }
        ui.label(RichText::new(hud.tr("splash-no-recent")).size(12.0).color(theme::MUTED));
        return;
    }
    let mut chosen = None;
    egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for item in &hud.ui.recent {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
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
                rect.left_center() + vec2(34.0, 0.0),
                Align2::LEFT_CENTER,
                file_name,
                egui::FontId::monospace(12.0),
                name_color,
            );
            // Older entries stored the root Node's own name, which says nothing about the content.
            if !item.file.name.is_empty() && !matches!(item.file.name.as_str(), "Scene" | "Scena") {
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
