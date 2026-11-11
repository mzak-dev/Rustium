//! Render mode, right island: the Render, Camera, Light and Output tabs.

use egui::{Color32, DragValue, Grid, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use rustium_render::{Ground, Sky};

use super::widgets::{self, Look};
use super::{AppCommand, Hud};
use crate::icons::Icon;
use crate::render_mode::{Backdrop, ImageFormat, RenderTab, SIDE_RANGE, ViewProjection};
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
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(text.to_uppercase()).color(theme::SUBTLE).size(10.0));
    });
}

fn properties(ui: &mut Ui, id: &str, add_rows: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            Grid::new(id)
                .num_columns(2)
                .min_col_width(76.0)
                .spacing(vec2(8.0, 7.0))
                .show(ui, add_rows);
        });
}

/// A read-only field with a dropdown chevron, like the mockup's Engine and Sky selectors.
fn static_select(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(11.0, 11.0), Sense::hover());
                    Icon::ChevronDown.paint(ui.painter(), rect, theme::MUTED);
                });
            });
        });
}

fn disabled_field(ui: &mut Ui, text: &str, tooltip: String) {
    ui.add_enabled_ui(false, |ui| {
        egui::Frame::new()
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, theme::FIELD_BORDER))
            .corner_radius(7)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| ui.label(widgets::mono(text, 11.0)));
    })
    .response
    .on_disabled_hover_text(tooltip);
}

/// One isometric cube thumbnail of the Style row (28 × 28 view box).
struct StyleLook {
    key: &'static str,
    faces: [Option<Color32>; 3],
    outline: Color32,
}

fn style_looks() -> [StyleLook; 5] {
    let color = |r, g, b| Some(Color32::from_rgb(r, g, b));
    let stroke = |r, g, b| Color32::from_rgb(r, g, b);
    [
        StyleLook {
            key: "style-traced",
            faces: [
                color(0xd6, 0xcc, 0xba),
                color(0xa8, 0x9c, 0x86),
                color(0x7c, 0x72, 0x60),
            ],
            outline: stroke(0xa8, 0x9c, 0x86),
        },
        StyleLook {
            key: "style-clay",
            faces: [
                color(0xb4, 0xb4, 0xb4),
                color(0x8e, 0x8e, 0x8e),
                color(0x6c, 0x6c, 0x6c),
            ],
            outline: stroke(0x8e, 0x8e, 0x8e),
        },
        StyleLook {
            key: "style-toon",
            faces: [
                color(0xd6, 0xcc, 0xba),
                color(0xd6, 0xcc, 0xba),
                color(0xa8, 0x9c, 0x86),
            ],
            outline: stroke(0x0b, 0x0d, 0x10),
        },
        StyleLook {
            key: "style-pixel",
            faces: [
                color(0xf0, 0xe0, 0xb0),
                color(0xb0, 0x80, 0x40),
                color(0x60, 0x40, 0x20),
            ],
            outline: stroke(0xb0, 0x80, 0x40),
        },
        StyleLook {
            key: "style-wire",
            faces: [None, None, None],
            outline: stroke(0xc8, 0xcf, 0xd9),
        },
    ]
}

fn style_row(ui: &mut Ui, hud: &Hud<'_>) {
    let accent = hud.accent();
    let later = hud.tr("later-milestone");
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 6,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let width = (ui.available_width() - 4.0 * 2.0) / 5.0;
            ui.horizontal(|ui| {
                for (index, look) in style_looks().iter().enumerate() {
                    let selected = index == 0;
                    let sense = if selected { Sense::click() } else { Sense::hover() };
                    let (rect, response) = ui.allocate_exact_size(vec2(width, 50.0), sense);
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
                    let dim = if selected { 1.0 } else { 0.45 };
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
                    if !selected {
                        response.on_hover_text(later.clone());
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
    let later = hud.tr("later-milestone");
    let labels = [
        hud.tr("engine"),
        hud.tr("samples"),
        hud.tr("bounces"),
        hud.tr("denoise"),
    ];
    let engine = hud.tr("engine-path-tracing");
    properties(ui, "render-tab", |ui| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        static_select(ui, &engine);
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        ui.add(DragValue::new(&mut render.samples).range(1..=65_536).speed(4.0));
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        ui.add(DragValue::new(&mut render.bounces).range(1..=32).speed(0.1));
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        let mut on = false;
        widgets::switch(ui, &mut on, false).on_disabled_hover_text(later);
        ui.end_row();
    });
    super::materials_preview::show(ui, hud);
}

fn camera_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let later = hud.tr("later-milestone");
    let labels = [
        hud.tr("projection"),
        hud.tr("lens"),
        hud.tr("dof"),
        hud.tr("aperture"),
        hud.tr("focus"),
    ];
    let options = [
        (ViewProjection::Perspective, hud.tr("proj-persp")),
        (ViewProjection::Orthographic, hud.tr("proj-ortho")),
        (ViewProjection::Isometric, hud.tr("proj-iso")),
    ];
    let mut projection = hud.ui.render.projection;
    properties(ui, "camera-tab", |ui| {
        ui.label(widgets::muted(&labels[0]));
        widgets::segmented(ui, &mut projection, &options, true);
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        ui.add(
            DragValue::new(&mut hud.ui.render.lens_mm)
                .range(crate::render_mode::LENS_RANGE_MM.0..=crate::render_mode::LENS_RANGE_MM.1)
                .speed(0.3)
                .suffix(" mm")
                .max_decimals(0),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        let mut off = false;
        widgets::switch(ui, &mut off, false).on_disabled_hover_text(later.clone());
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        disabled_field(ui, "f/2.8", later.clone());
        ui.end_row();
        ui.label(widgets::muted(&labels[4]));
        disabled_field(ui, "4.20 m", later);
        ui.end_row();
    });
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
    properties(ui, "light-tab", |ui| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        ui.add(
            DragValue::new(&mut render.sun_turn_degrees)
                .range(0.0..=360.0)
                .speed(0.5)
                .suffix("°")
                .max_decimals(0),
        );
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        ui.add(
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
        let button = widgets::dropdown_button(ui, current);
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

fn output_tab(ui: &mut Ui, hud: &mut Hud<'_>) {
    let later = hud.tr("later-milestone");
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
    ];
    let mut format = hud.ui.render.format;
    let mut backdrop = hud.ui.render.backdrop;
    properties(ui, "output-tab", |ui| {
        let render = &mut hud.ui.render;
        ui.label(widgets::muted(&labels[0]));
        ui.horizontal(|ui| {
            ui.add(
                DragValue::new(&mut render.size.0)
                    .range(SIDE_RANGE.0..=SIDE_RANGE.1)
                    .speed(4.0),
            );
            ui.label(widgets::muted("×"));
            ui.add(
                DragValue::new(&mut render.size.1)
                    .range(SIDE_RANGE.0..=SIDE_RANGE.1)
                    .speed(4.0),
            );
        });
        ui.end_row();
        ui.label(widgets::muted(&labels[1]));
        ui.horizontal(|ui| {
            widgets::segmented(ui, &mut format, &format_options, true);
            widgets::text_button(ui, "MP4", 26.0, Look::Plain, false).on_disabled_hover_text(later.clone());
        });
        ui.end_row();
        ui.label(widgets::muted(&labels[2]));
        widgets::segmented(ui, &mut backdrop, &backdrop_options, true);
        ui.end_row();
        ui.label(widgets::muted(&labels[3]));
        disabled_field(ui, "72", later.clone());
        ui.end_row();
    });
    hud.ui.render.format = format;
    hud.ui.render.backdrop = backdrop;

    let ready = hud.ui.render.progress.samples > 0;
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        let half = (ui.available_width() - 12.0 - 4.0) / 2.0;
        ui.scope(|ui| {
            ui.set_width(half);
            let save = widgets::text_button(ui, &hud.tr("save-image"), 32.0, Look::On, ready);
            if save.clicked() {
                hud.send(AppCommand::SaveImage);
            }
        });
        ui.scope(|ui| {
            ui.set_width(half);
            widgets::text_button(ui, &hud.tr("queue-shots"), 32.0, Look::On, false).on_disabled_hover_text(later);
        });
    });
    if let Some(message) = &hud.ui.render.message {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(message).color(theme::SOFT).size(11.0));
        });
    }
}
