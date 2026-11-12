//! Render mode, right island: the Render, Camera, Light and Output tabs.

use egui::{Color32, DragValue, FontId, Grid, Response, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use rustium_render::{Ground, RenderStyle, Sky, TraceEngine};

use super::widgets::{self, Look};
use super::{AppCommand, Hud};
use crate::icons::Icon;
use crate::render_mode::{
    APERTURE_RANGE, Backdrop, EXPOSURE_RANGE, ImageFormat, RenderTab, SIDE_RANGE, TURNTABLE_RANGE, ViewProjection,
};
use crate::settings::RayTracingMode;
use crate::theme;

pub(crate) const TABS: [(RenderTab, &str); 4] = [
    (RenderTab::Render, "rtab-render"),
    (RenderTab::Camera, "rtab-camera"),
    (RenderTab::Light, "rtab-light"),
    (RenderTab::Output, "rtab-output"),
];

pub(crate) fn content(ui: &mut Ui, hud: &mut Hud<'_>) {
    match hud.ui.render.tab {
        RenderTab::Render => render_tab(ui, hud),
        RenderTab::Camera => camera_tab(ui, hud),
        RenderTab::Light => light_tab(ui, hud),
        RenderTab::Output => output_tab(ui, hud),
    }
}

fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(text.to_uppercase()).color(theme::SUBTLE).size(10.0));
    });
}

/// Width of the label column of a property grid.
const LABEL_WIDTH: f32 = 92.0;
/// Colour of a message that says why something cannot be done.
const WARNING: Color32 = Color32::from_rgb(0xf0, 0x8a, 0x72);

/// A grid of labelled rows; `add_rows` gets the width the value column leaves, so fields can fill it.
fn properties(ui: &mut Ui, id: &str, add_rows: impl FnOnce(&mut Ui, f32)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            let field_width = (ui.available_width() - LABEL_WIDTH - 8.0).max(60.0);
            Grid::new(id)
                .num_columns(2)
                .min_col_width(LABEL_WIDTH)
                .spacing(vec2(8.0, 7.0))
                .show(ui, |ui| add_rows(ui, field_width));
        });
}

/// A bordered number field in the mono font, `width` wide, like the mockup's `.fld`.
fn number(ui: &mut Ui, width: f32, enabled: bool, drag: DragValue<'_>) -> Response {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            widget.bg_fill = theme::FIELD;
            widget.weak_bg_fill = theme::FIELD;
            widget.bg_stroke = Stroke::new(1.0, theme::FIELD_BORDER);
            widget.corner_radius = egui::CornerRadius::same(7);
        }
        ui.style_mut().override_font_id = Some(FontId::monospace(11.5));
        ui.add_enabled_ui(enabled, |ui| ui.add_sized([width, 26.0], drag)).inner
    })
    .inner
}

/// A full-width dropdown field with a chevron, like the mockup's `.selb`; open its menu with
/// [`egui::Popup::menu`] on the response.
fn select(ui: &mut Ui, width: f32, text: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let painter = ui.painter();
    painter.rect(
        rect,
        7,
        theme::FIELD,
        Stroke::new(
            1.0,
            if response.hovered() {
                theme::MUTED
            } else {
                theme::FIELD_BORDER
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.left_center() + vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.0),
        theme::TEXT,
    );
    Icon::ChevronDown.paint(
        painter,
        egui::Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), vec2(11.0, 11.0)),
        theme::MUTED,
    );
    response
}

/// One isometric cube thumbnail of the Style row (28 × 28 view box).
struct StyleLook {
    key: &'static str,
    hint: &'static str,
    faces: [Option<Color32>; 3],
    outline: Color32,
}

fn style_looks() -> [StyleLook; 5] {
    let color = |r, g, b| Some(Color32::from_rgb(r, g, b));
    let stroke = |r, g, b| Color32::from_rgb(r, g, b);
    [
        StyleLook {
            key: "style-traced",
            hint: "style-traced-hint",
            faces: [
                color(0xd6, 0xcc, 0xba),
                color(0xa8, 0x9c, 0x86),
                color(0x7c, 0x72, 0x60),
            ],
            outline: stroke(0xa8, 0x9c, 0x86),
        },
        StyleLook {
            key: "style-clay",
            hint: "style-clay-hint",
            faces: [
                color(0xb4, 0xb4, 0xb4),
                color(0x8e, 0x8e, 0x8e),
                color(0x6c, 0x6c, 0x6c),
            ],
            outline: stroke(0x8e, 0x8e, 0x8e),
        },
        StyleLook {
            key: "style-toon",
            hint: "style-toon-hint",
            faces: [
                color(0xd6, 0xcc, 0xba),
                color(0xd6, 0xcc, 0xba),
                color(0xa8, 0x9c, 0x86),
            ],
            outline: stroke(0x0b, 0x0d, 0x10),
        },
        StyleLook {
            key: "style-pixel",
            hint: "style-pixel-hint",
            faces: [
                color(0xf0, 0xe0, 0xb0),
                color(0xb0, 0x80, 0x40),
                color(0x60, 0x40, 0x20),
            ],
            outline: stroke(0xb0, 0x80, 0x40),
        },
        StyleLook {
            key: "style-wire",
            hint: "style-wire-hint",
            faces: [None, None, None],
            outline: stroke(0xc8, 0xcf, 0xd9),
        },
    ]
}

fn style_row(ui: &mut Ui, hud: &mut Hud<'_>) {
    let accent = hud.accent();
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 0,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let width = (ui.available_width() - 4.0 * 2.0) / 5.0;
            ui.horizontal(|ui| {
                for (index, look) in style_looks().iter().enumerate() {
                    let style = RenderStyle::ALL[index];
                    let selected = hud.ui.render.style == style;
                    let (rect, response) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
                    let painter = ui.painter();
                    if selected {
                        painter.rect(
                            rect,
                            8,
                            Color32::from_white_alpha(15),
                            Stroke::new(1.0, accent),
                            egui::StrokeKind::Inside,
                        );
                    } else if response.hovered() {
                        painter.rect_filled(rect, 8, theme::HOVER);
                    }
                    let dim = 1.0;
                    let origin = pos2(rect.center().x - 14.0, rect.top() + 4.0);
                    let point = |x: f32, y: f32| origin + vec2(x, y);
                    let outline = Stroke::new(1.4, look.outline.gamma_multiply(dim));
                    let faces = [
                        [point(14.0, 4.0), point(24.0, 9.0), point(14.0, 14.0), point(4.0, 9.0)],
                        [point(4.0, 9.0), point(14.0, 14.0), point(14.0, 24.0), point(4.0, 19.0)],
                        [
                            point(24.0, 9.0),
                            point(14.0, 14.0),
                            point(14.0, 24.0),
                            point(24.0, 19.0),
                        ],
                    ];
                    for (face, fill) in faces.into_iter().zip(look.faces) {
                        painter.add(Shape::convex_polygon(
                            face.to_vec(),
                            fill.map_or(Color32::TRANSPARENT, |fill| fill.gamma_multiply(dim)),
                            outline,
                        ));
                    }
                    let name = hud.tr(look.key);
                    let text_color = if selected { Color32::WHITE } else { theme::DISABLED };
                    painter.text(
                        rect.center_bottom() - vec2(0.0, 3.0),
                        egui::Align2::CENTER_BOTTOM,
                        name,
                        egui::FontId::proportional(10.0),
                        text_color,
                    );
                    if response.on_hover_text(hud.tr(look.hint)).clicked() && !selected {
                        hud.ui.render.style = style;
                    }
                }
            });
        });
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
}

fn render_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    caption(ui, &hud.tr("style"));
    style_row(ui, hud);
    let labels = [
        hud.tr("engine"),
        hud.tr("samples"),
        hud.tr("bounces"),
        hud.tr("denoise"),
    ];
    let active = hud.tr(match hud.trace_engine {
        TraceEngine::Software => "engine-active-software",
        TraceEngine::Hardware => "engine-active-hardware",
    });
    let automatic = hud.tr("backend-automatic");
    let options = [
        (RayTracingMode::Automatic, automatic.clone(), true),
        (RayTracingMode::Software, hud.tr("ray-tracing-software"), true),
        (
            RayTracingMode::Hardware,
            hud.tr("ray-tracing-hardware"),
            hud.ray_query_supported,
        ),
    ];
    let mode = hud.settings.ray_tracing;
    let engine = match mode {
        RayTracingMode::Automatic => format!("{automatic} \u{b7} {active}"),
        _ => options
            .iter()
            .find(|(candidate, _, _)| *candidate == mode)
            .map_or_else(String::new, |(_, name, _)| name.clone()),
    };
    let denoise_hint = hud.tr("denoise-hint");
    let mut chosen = mode;
    properties(ui, "render-tab", |ui, width| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        let button = select(ui, width, &engine);
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(180.0);
            for (candidate, name, enabled) in &options {
                ui.add_enabled_ui(*enabled, |ui| ui.selectable_value(&mut chosen, *candidate, name));
            }
        });
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        number(
            ui,
            width,
            true,
            DragValue::new(&mut render.samples).range(1..=65_536).speed(4.0),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        number(
            ui,
            width,
            true,
            DragValue::new(&mut render.bounces).range(1..=32).speed(0.1),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        widgets::switch(ui, &mut render.denoise, true).on_hover_text(denoise_hint);
        ui.end_row();
    });
    if chosen != mode {
        hud.settings.ray_tracing = chosen;
        hud.send(AppCommand::SettingsChanged);
    }
}

fn camera_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let labels = [
        hud.tr("projection"),
        hud.tr("lens"),
        hud.tr("exposure"),
        hud.tr("dof"),
        hud.tr("aperture"),
        hud.tr("focus"),
    ];
    let options = [
        (ViewProjection::Perspective, hud.tr("proj-persp")),
        (ViewProjection::Orthographic, hud.tr("proj-ortho")),
        (ViewProjection::Isometric, hud.tr("proj-iso")),
    ];
    let focus_hint = hud.tr("focus-hint");
    let voxel_size = hud.editor.scene.voxel_size();
    let mut focus = hud.ui.render.focus_metres(hud.camera, voxel_size);
    let mut focus_changed = false;
    let mut projection = hud.ui.render.projection;
    properties(ui, "camera-tab", |ui, width| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        widgets::segmented(ui, &mut projection, &options, true);
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        number(
            ui,
            width,
            true,
            DragValue::new(&mut render.lens_mm)
                .range(crate::render_mode::LENS_RANGE_MM.0..=crate::render_mode::LENS_RANGE_MM.1)
                .speed(0.3)
                .suffix(" mm")
                .max_decimals(0),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        number(
            ui,
            width,
            true,
            DragValue::new(&mut render.exposure_ev)
                .range(EXPOSURE_RANGE.0..=EXPOSURE_RANGE.1)
                .speed(0.02)
                .suffix(" EV")
                .fixed_decimals(1),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        widgets::switch(ui, &mut render.dof, true);
        ui.end_row();
        ui.label(widgets::muted(&labels[4]));
        number(
            ui,
            width,
            render.dof,
            DragValue::new(&mut render.aperture)
                .range(APERTURE_RANGE.0..=APERTURE_RANGE.1)
                .speed(0.05)
                .prefix("f/")
                .fixed_decimals(1),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[5]));
        let response = number(
            ui,
            width,
            render.dof,
            DragValue::new(&mut focus)
                .range(0.05..=10_000.0)
                .speed(0.02)
                .suffix(" m")
                .fixed_decimals(2),
        )
        .on_hover_text(focus_hint);
        focus_changed = response.changed();
        ui.end_row();
    });
    if focus_changed {
        hud.ui.render.focus_m = Some(focus);
    }
    if projection != hud.ui.render.projection {
        hud.ui.render.apply_projection(hud.camera, projection);
    }
}

fn light_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let labels = [
        hud.tr("sun-turn"),
        hud.tr("sun-height"),
        hud.tr("sky"),
        hud.tr("ground"),
    ];
    let ground_options = [
        (Ground::None, hud.tr("ground-none")),
        (Ground::Studio, hud.tr("ground-studio")),
        (Ground::Shadow, hud.tr("ground-shadow")),
    ];
    let sky_names: Vec<(Sky, String)> = Sky::ALL.iter().map(|sky| (*sky, hud.tr(sky_key(*sky)))).collect();
    let mut sky = hud.ui.render.sky;
    let mut ground = hud.ui.render.ground;
    properties(ui, "light-tab", |ui, width| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        number(
            ui,
            width,
            true,
            DragValue::new(&mut render.sun_turn_degrees)
                .range(0.0..=360.0)
                .speed(0.5)
                .suffix("°")
                .max_decimals(0),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        number(
            ui,
            width,
            true,
            DragValue::new(&mut render.sun_height_degrees)
                .range(0.0..=90.0)
                .speed(0.3)
                .suffix("°")
                .max_decimals(0),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        let current = sky_names
            .iter()
            .find(|(candidate, _)| *candidate == sky)
            .map_or("", |(_, name)| name.as_str());
        let button = select(ui, width, current);
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(140.0);
            for (candidate, name) in &sky_names {
                ui.selectable_value(&mut sky, *candidate, name);
            }
        });
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        widgets::segmented(ui, &mut ground, &ground_options, true);
        ui.end_row();
    });
    hud.ui.render.sky = sky;
    hud.ui.render.ground = ground;
}

fn sky_key(sky: Sky) -> &'static str {
    match sky {
        Sky::Clear => "sky-clear",
        Sky::Overcast => "sky-overcast",
        Sky::Sunset => "sky-sunset",
        Sky::Night => "sky-night",
    }
}

/// "72 frames" with the plural of the current language.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the count is a whole number within the turntable range"
)]
fn frames_label(i18n: &crate::i18n::I18n, count: f64) -> String {
    i18n.tr_args(
        "frames-count",
        &[("count", fluent_bundle::FluentValue::from(count.max(0.0).round() as u32))],
    )
}

fn output_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let labels = [
        hud.tr("size"),
        hud.tr("format"),
        hud.tr("background"),
        hud.tr("turntable"),
    ];
    let backdrop_options = [
        (Backdrop::Keep, hud.tr("background-keep")),
        (Backdrop::Transparent, hud.tr("background-transparent")),
    ];
    let format_options = [
        (ImageFormat::Png, "PNG".to_owned()),
        (ImageFormat::Exr, "EXR".to_owned()),
        (ImageFormat::Mp4, "MP4".to_owned()),
    ];
    let mut format = hud.ui.render.format;
    let mut backdrop = hud.ui.render.backdrop;
    let video = format == ImageFormat::Mp4;
    let video_no_alpha = hud.tr("video-no-alpha");
    let video_only = hud.tr("turntable-video-only");
    let problem = hud.ui.render.size_problem(hud.ui.render.size);
    let problem_text = problem.map(|problem| {
        let size = hud.ui.render.size;
        hud.i18n.tr_args(
            "size-too-large",
            &[
                ("width", fluent_bundle::FluentValue::from(size.0)),
                ("height", fluent_bundle::FluentValue::from(size.1)),
                (
                    "needed",
                    fluent_bundle::FluentValue::from(format!("{:.1}", problem.needed as f64 / f64::from(1u32 << 30))),
                ),
                (
                    "allowed",
                    fluent_bundle::FluentValue::from(format!("{:.1}", problem.allowed as f64 / f64::from(1u32 << 30))),
                ),
                ("side", fluent_bundle::FluentValue::from(problem.max_side)),
            ],
        )
    });
    let i18n = hud.i18n;
    let frames_format = |value: f64, _range: std::ops::RangeInclusive<usize>| frames_label(i18n, value);
    properties(ui, "output-tab", |ui, width| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        let half = (width - 4.0) / 2.0;
        let size_before = render.size;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            number(
                ui,
                half,
                true,
                DragValue::new(&mut render.size.0)
                    .range(SIDE_RANGE.0..=SIDE_RANGE.1)
                    .speed(4.0),
            );
            number(
                ui,
                half,
                true,
                DragValue::new(&mut render.size.1)
                    .range(SIDE_RANGE.0..=SIDE_RANGE.1)
                    .speed(4.0),
            );
        });
        if render.size != size_before {
            // A message about the old size (too large, saved) no longer describes this one.
            render.message = None;
        }
        ui.end_row();
        if let Some(text) = &problem_text {
            ui.label("");
            ui.add(egui::Label::new(RichText::new(text).color(WARNING).size(11.0)).wrap());
            ui.end_row();
        }
        ui.label(widgets::muted(&labels[1]));
        widgets::segmented(ui, &mut format, &format_options, true);
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        if video {
            // A video has no alpha channel: it is always rendered on the background.
            let mut shown = Backdrop::Keep;
            ui.scope(|ui| widgets::segmented(ui, &mut shown, &backdrop_options, false))
                .response
                .on_hover_text(&video_no_alpha);
        } else {
            widgets::segmented(ui, &mut backdrop, &backdrop_options, true);
        }
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        number(
            ui,
            width,
            video,
            DragValue::new(&mut render.turntable_frames)
                .range(TURNTABLE_RANGE.0..=TURNTABLE_RANGE.1)
                .speed(0.5)
                .custom_formatter(frames_format)
                .custom_parser(|text| {
                    text.trim()
                        .split(|c: char| !c.is_ascii_digit())
                        .find(|part| !part.is_empty())
                        .and_then(|digits| digits.parse().ok())
                }),
        )
        .on_disabled_hover_text(&video_only);
        ui.end_row();
    });
    hud.ui.render.format = format;
    hud.ui.render.backdrop = backdrop;

    let running = hud.ui.render.job.is_some();
    let too_large = problem.is_some();
    let save_label = hud.tr(if video { "save-video" } else { "save-image" });
    let save_hint = hud.tr(if video { "save-video-hint" } else { "save-image-hint" });
    let queue_label = hud.tr("queue-shots");
    let queue_hint = hud.tr("queue-shots-hint");
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        let half = (ui.available_width() - 12.0 - 4.0) / 2.0;
        let button = |ui: &mut Ui, label: &str, enabled: bool| {
            let sense = if enabled { Sense::click() } else { Sense::hover() };
            let (rect, response) = ui.allocate_exact_size(vec2(half, 32.0), sense);
            let fill = if !enabled {
                Color32::TRANSPARENT
            } else if response.hovered() {
                theme::HOVER
            } else {
                theme::ACTIVE
            };
            ui.painter().rect_filled(rect, 7, fill);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                FontId::proportional(12.0),
                if enabled { Color32::WHITE } else { theme::DISABLED },
            );
            response
        };
        if button(ui, &save_label, !running && !too_large)
            .on_hover_text(save_hint)
            .clicked()
        {
            hud.send(AppCommand::SaveImage);
        }
        if button(ui, &queue_label, !running).on_hover_text(queue_hint).clicked() {
            hud.send(AppCommand::QueueShots);
        }
    });
    job_progress(ui, hud);
    if let Some(message) = &hud.ui.render.message {
        ui.add_space(8.0);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 0))
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(message).color(theme::SOFT).size(11.0)).wrap());
            });
    }
}

/// The progress bar and Cancel button of a running turntable or queue job.
fn job_progress(ui: &mut Ui, hud: &mut Hud<'_>) {
    let Some(job) = &hud.ui.render.job else {
        return;
    };
    let fraction = job.fraction();
    let (done, total) = job.frames();
    let status = hud.i18n.tr_args(
        "job-progress",
        &[
            ("done", fluent_bundle::FluentValue::from(done)),
            ("total", fluent_bundle::FluentValue::from(total)),
        ],
    );
    let cancel_label = hud.tr("job-cancel");
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(status).color(theme::SOFT).size(11.0));
    });
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let width = ui.available_width() - 12.0 - 72.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width.max(40.0), 6.0), Sense::hover());
        ui.painter().rect_filled(rect, 3, Color32::from_white_alpha(26));
        let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
        ui.painter().rect_filled(filled, 3, hud.accent());
        if widgets::text_button(ui, &cancel_label, 24.0, Look::Plain, true).clicked() {
            hud.send(AppCommand::CancelJob);
        }
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
}
