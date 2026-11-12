//! The radial Tool wheel opened with a right click, shared by Model, Render and Effects mode.

use egui::{Align2, Context, Id, Order, Pos2, Rect, Sense, Stroke, vec2};

use super::effects_bars::{TOOLS, run_tool};
use super::widgets::{self, Look};
use super::{Hud, Mode};
use crate::editor::Tool;
use crate::icons::Icon;
use crate::render_mode::RenderTool;
use crate::theme;

/// Radius of the ring the wheel places its buttons on, in points.
const WHEEL_RADIUS: f32 = 82.0;
const WHEEL_BUTTON: f32 = 44.0;

/// One button of the wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WheelItem {
    Model(Tool),
    Render(RenderTool),
    /// An entry of the Effects tools island (a Tool or a one-shot action), by index.
    Graph(usize),
}

/// The buttons of the wheel in `mode`, clockwise from the top.
pub(super) fn items(mode: Mode) -> Vec<WheelItem> {
    match mode {
        Mode::Model => Tool::ALL.into_iter().map(WheelItem::Model).collect(),
        Mode::Render => RenderTool::ALL.into_iter().map(WheelItem::Render).collect(),
        Mode::Effects => (0..TOOLS.len()).map(WheelItem::Graph).collect(),
    }
}

/// Icon, message key of the name, and whether the item is the active Tool.
fn describe(hud: &Hud<'_>, item: WheelItem) -> Option<(Icon, &'static str, bool)> {
    Some(match item {
        WheelItem::Model(tool) => (tool.icon(), tool.message_key(), hud.editor.tool == tool),
        WheelItem::Render(tool) => (tool.icon(), tool.message_key(), hud.ui.render.tool == tool),
        WheelItem::Graph(index) => {
            let info = TOOLS.get(index)?;
            (info.icon, info.key, info.tool == Some(hud.ui.effects.tool))
        }
    })
}

fn choose(ctx: &Context, hud: &mut Hud<'_>, item: WheelItem, centre: Pos2) {
    match item {
        WheelItem::Model(tool) => hud.editor.tool = tool,
        WheelItem::Render(tool) => hud.ui.render.tool = tool,
        WheelItem::Graph(index) => {
            if let Some(info) = TOOLS.get(index) {
                run_tool(ctx, hud, info);
                // The node search opens where the wheel was.
                if let Some(popup) = hud.ui.effects.add_popup.as_mut()
                    && popup.fresh
                {
                    popup.at = centre;
                }
            }
        }
    }
}

/// Draws the wheel around `centre` and handles its buttons, outside clicks and Escape.
pub(super) fn show(ctx: &Context, hud: &mut Hud<'_>, centre: Pos2) {
    let screen = ctx.content_rect();
    let margin = WHEEL_RADIUS + WHEEL_BUTTON;
    let centre = Pos2::new(
        centre.x.clamp(
            screen.min.x + margin,
            (screen.max.x - margin).max(screen.min.x + margin),
        ),
        centre.y.clamp(
            screen.min.y + margin,
            (screen.max.y - margin).max(screen.min.y + margin),
        ),
    );
    let items = items(hud.ui.mode);
    let active_name = items
        .iter()
        .filter_map(|item| describe(hud, *item))
        .find(|(_, _, active)| *active)
        .map(|(_, key, _)| hud.tr(key))
        .unwrap_or_default();
    let mut chosen = None;
    egui::Area::new(Id::unique("tool-wheel"))
        .order(Order::Foreground)
        .fixed_pos(centre - vec2(margin, margin))
        .show(ctx, |ui| {
            let (_, _) = ui.allocate_exact_size(vec2(margin * 2.0, margin * 2.0), Sense::hover());
            ui.painter()
                .circle_filled(centre, WHEEL_RADIUS + WHEEL_BUTTON / 2.0 + 8.0, theme::PANEL);
            ui.painter().circle_stroke(
                centre,
                WHEEL_RADIUS + WHEEL_BUTTON / 2.0 + 8.0,
                Stroke::new(1.0, theme::BORDER),
            );
            ui.painter().text(
                centre,
                Align2::CENTER_CENTER,
                active_name,
                theme::bold(12.0),
                theme::TEXT,
            );
            for (index, item) in items.iter().enumerate() {
                let Some((icon, key, active)) = describe(hud, *item) else {
                    continue;
                };
                let angle = -std::f32::consts::FRAC_PI_2 + index as f32 / items.len() as f32 * std::f32::consts::TAU;
                let position = centre + vec2(angle.cos(), angle.sin()) * WHEEL_RADIUS;
                let look = if active { Look::Accent } else { Look::Plain };
                let rect = Rect::from_center_size(position, vec2(WHEEL_BUTTON, WHEEL_BUTTON));
                let button = ui.put(rect, |ui: &mut egui::Ui| {
                    widgets::icon_button(ui, icon, WHEEL_BUTTON, look, true)
                });
                if button.on_hover_text(hud.tr(key)).clicked() {
                    chosen = Some(*item);
                }
            }
        });
    if let Some(item) = chosen {
        hud.ui.tool_wheel = None;
        choose(ctx, hud, item, centre);
        return;
    }
    let outside = ctx.input(|input| {
        let away = input
            .pointer
            .interact_pos()
            .is_none_or(|position| position.distance(centre) > margin);
        (input.pointer.any_pressed() && away) || input.key_pressed(egui::Key::Escape)
    });
    if outside {
        hud.ui.tool_wheel = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_wheel_lists_every_tool() {
        let items = items(Mode::Model);
        assert_eq!(items.len(), Tool::ALL.len());
        assert_eq!(items[0], WheelItem::Model(Tool::SelectBox));
    }

    #[test]
    fn render_wheel_lists_the_render_tools() {
        let expected: Vec<WheelItem> = RenderTool::ALL.into_iter().map(WheelItem::Render).collect();
        assert_eq!(items(Mode::Render), expected);
        assert_eq!(expected.len(), 6);
    }

    #[test]
    fn effects_wheel_is_select_pan_add_cut_frame_fit() {
        let keys: Vec<&str> = items(Mode::Effects)
            .into_iter()
            .filter_map(|item| match item {
                WheelItem::Graph(index) => TOOLS.get(index).map(|info| info.key),
                _ => None,
            })
            .collect();
        assert_eq!(keys, ["gt-select", "gt-pan", "gt-add", "gt-cut", "gt-frame", "gt-fit"]);
    }
}
