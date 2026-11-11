//! The temporary "Materials (preview)" section of the Render tab: list the Scene's material graphs, edit their
//! constant Surface values, create one and assign the active Color slot to it.
//!
//! ponytail: a stand-in so M3's material pipeline can be tested by hand. The Effects mode node editor replaces
//! this whole file; delete it then.

use egui::{Color32, Grid, RichText, Sense, Slider, Ui, vec2};
use fluent_bundle::FluentValue;
use rustium_core::SurfaceConstants;

use super::Hud;
use super::widgets::{self, Look};
use crate::theme;

pub(crate) fn show(ui: &mut Ui, hud: &mut Hud<'_>) {
    let graph_count = hud.editor.scene.material_graphs().len();
    let selected = hud.ui.render.preview_graph.min(graph_count.saturating_sub(1));
    let active = hud.editor.material();
    let active_graph = hud
        .editor
        .scene
        .palette()
        .get(active)
        .map_or(0, |color| color.graph as usize);
    let active_color = hud.editor.scene.palette().get(active).map_or(Color32::BLACK, |color| {
        Color32::from_rgb(color.color[0], color.color[1], color.color[2])
    });

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.label(
                RichText::new(hud.tr("mp-title").to_uppercase())
                    .color(theme::SUBTLE)
                    .size(10.0),
            );
            ui.label(RichText::new(hud.tr("mp-hint")).color(theme::SUBTLE).size(11.0));
            ui.add_space(6.0);

            // The active Color and the material it uses.
            ui.horizontal(|ui| {
                let (swatch, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                ui.painter().rect_filled(swatch, 5, active_color);
                let name = hud
                    .editor
                    .scene
                    .material_graphs()
                    .get(active_graph)
                    .map_or("", |graph| graph.name.as_str());
                let text = hud.i18n.tr_args(
                    "mp-active",
                    &[
                        ("n", FluentValue::from(i64::from(active.get()))),
                        ("name", FluentValue::from(name.to_owned())),
                    ],
                );
                ui.label(RichText::new(text).color(theme::SOFT));
            });
            ui.add_space(6.0);

            // One row per graph; click to select.
            let rows: Vec<(String, usize)> = hud
                .editor
                .scene
                .material_graphs()
                .iter()
                .enumerate()
                .map(|(index, graph)| {
                    let users = hud
                        .editor
                        .scene
                        .palette()
                        .iter()
                        .filter(|(_, color)| color.graph as usize == index)
                        .count();
                    (graph.name.clone(), users)
                })
                .collect();
            for (index, (name, users)) in rows.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                let fill = if index == selected {
                    theme::ACTIVE
                } else if response.hovered() {
                    theme::HOVER
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 7, fill);
                ui.painter().text(
                    rect.left_center() + vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(12.0),
                    theme::TEXT,
                );
                ui.painter().text(
                    rect.right_center() - vec2(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    users.to_string(),
                    egui::FontId::monospace(10.0),
                    theme::MUTED,
                );
                if response.clicked() {
                    hud.ui.render.preview_graph = index;
                }
            }
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if widgets::text_button(
                    ui,
                    &hud.tr("mp-new"),
                    28.0,
                    Look::On,
                    graph_count < rustium_core::MaterialGraph::MAX_PER_SCENE,
                )
                .clicked()
                {
                    let index = hud.editor.create_graph();
                    hud.ui.render.preview_graph = index as usize;
                }
                let assign_label = hud.tr("mp-assign");
                if widgets::text_button(ui, &assign_label, 28.0, Look::On, selected != active_graph).clicked() {
                    hud.editor.assign_graph(u32::try_from(selected).unwrap_or(0));
                }
            });
            ui.add_space(8.0);

            constants(ui, hud, selected);
        });

    if !ui.input(|input| input.pointer.any_down()) {
        // A slider drag or picker interaction that ended: its edits become one History step.
        hud.editor.finish_material_edit();
    }
}

fn constants(ui: &mut Ui, hud: &mut Hud<'_>, selected: usize) {
    let Some(original) = hud
        .editor
        .scene
        .material_graphs()
        .get(selected)
        .map(|graph| graph.surface)
    else {
        return;
    };
    if selected == 0 {
        ui.label(
            RichText::new(hud.tr("mp-default-locked"))
                .color(theme::SUBTLE)
                .size(11.0),
        );
        return;
    }
    let mut surface = original;
    ui.spacing_mut().slider_width = 96.0;
    Grid::new("preview-surface")
        .num_columns(2)
        .min_col_width(76.0)
        .spacing(vec2(8.0, 6.0))
        .show(ui, |ui| {
            row(ui, hud.tr("mat-roughness"), &mut surface.roughness, 0.0..=1.0);
            row(ui, hud.tr("mat-metallic"), &mut surface.metallic, 0.0..=1.0);
            ui.label(widgets::muted(hud.tr("mat-emission-color")));
            ui.horizontal(|ui| {
                egui::color_picker::color_edit_button_srgb(ui, &mut surface.emission_color);
            });
            ui.end_row();
            row(
                ui,
                hud.tr("mat-strength"),
                &mut surface.emission_strength,
                0.0..=SurfaceConstants::MAX_EMISSION,
            );
            row(ui, hud.tr("mat-transmission"), &mut surface.transmission, 0.0..=1.0);
            row(
                ui,
                hud.tr("mat-ior"),
                &mut surface.ior,
                SurfaceConstants::IOR_RANGE.0..=SurfaceConstants::IOR_RANGE.1,
            );
        });
    if surface != original {
        hud.editor.edit_graph(u32::try_from(selected).unwrap_or(0), surface);
    }
}

fn row(ui: &mut Ui, label: String, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.label(widgets::muted(label));
    ui.add(Slider::new(value, range).max_decimals(2));
    ui.end_row();
}
