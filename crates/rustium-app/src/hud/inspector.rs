//! Right island: Scene, History and MCP tabs in Model mode, Render/Camera/Light/Output in Render mode. Collapses to a
//! small tab on the right edge.

use egui::{Context, Grid, Id, RichText, ScrollArea, Sense, Stroke, vec2};
use rustium_core::EditKind;
use rustium_mcp::Permission;

use super::widgets::{self, Look};
use super::{
    AppCommand, Glass, Hud, INSPECTOR_WIDTH, InspectorTab, Intro, MARGIN, Mode, Place, TOP_BAR_HEIGHT, island,
    render_inspector,
};
use crate::icons::Icon;
use crate::mcp::{McpStatus, McpTransport, permission_key};
use crate::settings::Motion;
use crate::settings_window::Section;
use crate::theme;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let screen = ctx.content_rect();
    let duration = if hud.settings.motion == Motion::Off { 0.0 } else { 0.3 };
    let openness = ctx.animate_bool_with_time(Id::unique("inspector-open"), hud.ui.inspector_open, duration);
    let top = MARGIN + TOP_BAR_HEIGHT + 10.0;
    let height = (screen.height() - 2.0 * top).max(200.0);

    if openness > 0.01 {
        let slide = (1.0 - super::ease_out(openness)) * (INSPECTOR_WIDTH + 16.0);
        let position = egui::pos2(screen.right() - MARGIN - INSPECTOR_WIDTH + slide, top);
        island(
            ctx,
            hud,
            "inspector",
            Place::Fixed(position),
            Intro {
                delay: 0.24,
                from: vec2(16.0, 0.0),
            },
            theme::island().inner_margin(0),
            |ui, hud| {
                ui.multiply_opacity(openness);
                ui.set_width(INSPECTOR_WIDTH);
                ui.set_height(height);
                ui.vertical(|ui| {
                    if hud.ui.mode == Mode::Effects {
                        super::effects_inspector::tabs(ui, hud);
                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| super::effects_inspector::content(ui, hud));
                        return;
                    }
                    if hud.ui.mode == Mode::Render {
                        render_tabs(ui, hud);
                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| render_inspector::content(ui, hud));
                        return;
                    }
                    tabs(ui, hud);
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match hud.ui.inspector_tab {
                            InspectorTab::Scene => scene_tab(ui, hud),
                            InspectorTab::History => history_tab(ui, hud),
                            InspectorTab::Mcp => mcp_tab(ui, hud),
                        });
                });
            },
        );
    }

    if openness < 0.99 {
        let response = egui::Area::new(Id::unique("inspector-reopen"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::pos2(screen.right() - MARGIN - 36.0, screen.center().y - 28.0))
            .show(ctx, |ui| {
                ui.multiply_opacity(1.0 - openness);
                theme::island().inner_margin(0).show(ui, |ui| {
                    let (rect, response) = ui.allocate_exact_size(vec2(36.0, 56.0), Sense::click());
                    let color = if response.hovered() {
                        egui::Color32::WHITE
                    } else {
                        theme::SOFT
                    };
                    Icon::ChevronLeft.paint(ui.painter(), rect.shrink2(vec2(8.0, 18.0)), color);
                    response.on_hover_text(hud.tr("inspector-open"))
                })
            });
        hud.glass.push(Glass {
            rect: response.response.rect,
            radius: f32::from(theme::ISLAND_RADIUS),
        });
        if response.inner.inner.clicked() {
            hud.ui.inspector_open = true;
        }
    }
}

/// The Render-mode tab row: Render, Camera, Light and Output, plus the collapse button.
fn render_tabs(ui: &mut egui::Ui, hud: &mut Hud<'_>) {
    egui::Frame::new().inner_margin(4).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (tab, key) in render_inspector::TABS {
                let look = if hud.ui.render.tab == tab {
                    Look::On
                } else {
                    Look::Plain
                };
                if widgets::text_button(ui, &hud.tr(key), 32.0, look, true).clicked() {
                    hud.ui.render.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::ChevronRight, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("inspector-collapse"))
                    .clicked()
                {
                    hud.ui.inspector_open = false;
                }
            });
        });
    });
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
}

fn tabs(ui: &mut egui::Ui, hud: &mut Hud<'_>) {
    egui::Frame::new().inner_margin(4).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let tabs = [
                (InspectorTab::Scene, "tab-scene"),
                (InspectorTab::History, "tab-history"),
                (InspectorTab::Mcp, "tab-mcp"),
            ];
            for (tab, key) in tabs {
                // A page cannot serve MCP.
                if !crate::DESKTOP && tab == InspectorTab::Mcp {
                    continue;
                }
                let look = if hud.ui.inspector_tab == tab {
                    Look::On
                } else {
                    Look::Plain
                };
                if widgets::text_button(ui, &hud.tr(key), 32.0, look, true).clicked() {
                    hud.ui.inspector_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::ChevronRight, 32.0, Look::Plain, true)
                    .on_hover_text(hud.tr("inspector-collapse"))
                    .clicked()
                {
                    hud.ui.inspector_open = false;
                }
            });
        });
    });
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
}

fn scene_tab(ui: &mut egui::Ui, hud: &mut Hud<'_>) {
    super::scene_tree::show(ui, hud);
}

fn edit_label(hud: &Hud<'_>, kind: EditKind) -> String {
    hud.tr(match kind {
        EditKind::Attach => "edit-attach",
        EditKind::Erase => "edit-erase",
        EditKind::Paint => "edit-paint",
        EditKind::Fill => "edit-fill",
        EditKind::MoveVoxels => "edit-move-voxels",
        EditKind::NewModel => "edit-new-model",
        EditKind::NewGroup => "edit-new-group",
        EditKind::Duplicate => "edit-duplicate",
        EditKind::DuplicateAsInstance => "edit-duplicate-instance",
        EditKind::MakeUnique => "edit-make-unique",
        EditKind::Delete => "edit-delete",
        EditKind::Rename => "edit-rename",
        EditKind::Hide => "edit-hide",
        EditKind::Lock => "edit-lock",
        EditKind::Merge => "edit-merge",
        EditKind::Split => "edit-split",
        EditKind::MoveNode => "edit-move-node",
        EditKind::Transform => "edit-transform",
        EditKind::EditGraph => "edit-graph",
        EditKind::CreateGraph => "create-graph",
        EditKind::AssignGraph => "assign-graph",
        EditKind::DuplicateGraph => "duplicate-graph",
        EditKind::DeleteGraph => "delete-graph",
        EditKind::RenameGraph => "rename-graph",
        EditKind::CastsShadow => "casts-shadow",
        EditKind::AddGraphNode => "add-graph-node",
        EditKind::RemoveGraphNodes => "remove-graph-nodes",
        EditKind::MoveGraphNodes => "move-graph-nodes",
        EditKind::ConnectGraph => "connect-graph",
        EditKind::DisconnectGraph => "disconnect-graph",
        EditKind::SetGraphParam => "set-graph-param",
        EditKind::RenameGraphNode => "rename-graph-node",
        EditKind::BypassGraphNode => "bypass-graph-node",
        EditKind::GraphFrame => "graph-frame",
        EditKind::RenameGraphFrame => "rename-graph-frame",
        EditKind::ResizeGraphFrame => "resize-graph-frame",
        EditKind::AddGraphFrame => "add-graph-frame",
        EditKind::DuplicateGraphNodes => "duplicate-graph-nodes",
        EditKind::SetVoxels => "edit-set-voxels",
        EditKind::SetColor => "edit-set-color",
        EditKind::Script => "edit-script",
    })
}

fn history_tab(ui: &mut egui::Ui, hud: &mut Hud<'_>) {
    let done: Vec<EditKind> = hud.editor.history.done().map(rustium_core::Edit::kind).collect();
    let undone: Vec<EditKind> = hud.editor.history.undone().map(rustium_core::Edit::kind).collect();
    let mut rows = vec![(0, hud.tr("history-new-scene"), true)];
    rows.extend(
        done.iter()
            .enumerate()
            .map(|(index, kind)| (index + 1, edit_label(hud, *kind), true)),
    );
    rows.extend(
        undone
            .iter()
            .enumerate()
            .map(|(index, kind)| (done.len() + index + 1, edit_label(hud, *kind), false)),
    );
    egui::Frame::new().inner_margin(4).show(ui, |ui| {
        for (target, label, applied) in rows {
            let current = target == done.len();
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            let fill = if current {
                theme::ACTIVE
            } else if response.hovered() {
                theme::HOVER
            } else {
                egui::Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 7, fill);
            let color = if applied { theme::SOFT } else { theme::DISABLED };
            ui.painter().text(
                rect.left_center() + vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                target.to_string(),
                egui::FontId::monospace(10.0),
                theme::SUBTLE,
            );
            ui.painter().text(
                rect.left_center() + vec2(28.0, 0.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(12.0),
                color,
            );
            if current {
                ui.painter().text(
                    rect.right_center() - vec2(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    hud.tr("history-now"),
                    egui::FontId::monospace(10.0),
                    theme::SOFT,
                );
            }
            if response.clicked() && !current {
                hud.send(AppCommand::HistoryJump(target));
            }
        }
    });
}

/// The MCP tab: live switch, status, transport, address, permissions, Configure and Copy client config.
fn mcp_tab(ui: &mut egui::Ui, hud: &mut Hud<'_>) {
    let before = hud.settings.mcp;
    let status = hud.mcp_status.clone();
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 14,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                let dot_color = match status {
                    McpStatus::Off => theme::DISABLED,
                    McpStatus::Listening(_) => hud.accent(),
                    McpStatus::Failed(_) => egui::Color32::from_rgb(0xf0, 0x8a, 0x72),
                };
                ui.painter().circle_filled(dot.center(), 4.0, dot_color);
                ui.vertical(|ui| {
                    ui.label(RichText::new(hud.tr("mcp-server")).font(theme::bold(12.0)));
                    ui.label(RichText::new(status.text(hud.i18n)).color(theme::MUTED).size(11.0));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    widgets::switch(ui, &mut hud.settings.mcp.enabled, true);
                });
            });
            if let McpStatus::Failed(message) = &status {
                ui.add_space(6.0);
                ui.label(RichText::new(message).color(theme::SUBTLE).size(11.0));
            }
            ui.add_space(8.0);
            Grid::new("mcp-connection")
                .num_columns(2)
                .min_col_width(72.0)
                .spacing(vec2(8.0, 6.0))
                .show(ui, |ui| {
                    ui.label(widgets::muted(hud.tr("mcp-transport")));
                    widgets::segmented(
                        ui,
                        &mut hud.settings.mcp.transport,
                        &[
                            (McpTransport::Stdio, "stdio".to_owned()),
                            (McpTransport::Http, "HTTP".to_owned()),
                        ],
                        true,
                    );
                    ui.end_row();
                    ui.label(widgets::muted(hud.tr("mcp-address")));
                    let address = match hud.settings.mcp.transport {
                        McpTransport::Http => format!("127.0.0.1:{}", hud.settings.mcp.port),
                        McpTransport::Stdio => "rustium --mcp-stdio".to_owned(),
                    };
                    ui.label(widgets::mono(address, 11.0));
                    ui.end_row();
                });
            ui.add_space(10.0);
            ui.label(
                RichText::new(hud.tr("mcp-allowed-tools").to_uppercase())
                    .color(theme::SUBTLE)
                    .size(10.0),
            );
            for permission in Permission::ALL {
                ui.horizontal(|ui| {
                    ui.set_height(30.0);
                    ui.label(RichText::new(hud.tr(permission_key(permission))).color(theme::SOFT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::switch(ui, hud.settings.mcp.permissions.flag_mut(permission), true);
                    });
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::text_button(ui, &hud.tr("mcp-configure"), 32.0, Look::On, true).clicked() {
                    hud.ui.settings_section = Some(Section::Mcp);
                }
                if widgets::text_button(ui, &hud.tr("mcp-copy-config"), 32.0, Look::On, true).clicked() {
                    super::copy_client_config(ui.ctx(), hud);
                }
            });
        });
    if hud.settings.mcp != before {
        hud.send(AppCommand::SettingsChanged);
    }
}
