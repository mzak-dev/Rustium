//! Modal questions: "save changes?" before a Scene is replaced and "restore unsaved work?" after a crash.

use egui::{Align, Context, Id, Key, Layout, Order, RichText, Sense, Stroke, StrokeKind, UiBuilder, vec2};
use fluent_bundle::FluentValue;

use super::widgets::{self, Look};
use super::{AppCommand, Glass, Hud};
use crate::files::{Pending, RecoveryOffer};
use crate::theme;

/// A question the user has to answer before anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Prompt {
    /// The document has unsaved changes and `Pending` would replace or close it.
    Unsaved(Pending),
    /// A recovery copy from a previous session was found.
    Recovery(RecoveryOffer),
}

const WIDTH: f32 = 420.0;

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(prompt) = hud.ui.prompt.clone() else {
        return;
    };
    let name = if hud.ui.document_name.is_empty() {
        hud.tr("untitled")
    } else {
        hud.ui.document_name.clone()
    };
    let (title, body, primary, secondary) = match &prompt {
        Prompt::Unsaved(_) => (
            hud.tr("unsaved-title"),
            hud.i18n.tr_args("unsaved-body", &[("name", FluentValue::from(name))]),
            ("unsaved-save", AppCommand::PromptSave),
            ("unsaved-discard", AppCommand::PromptDiscard),
        ),
        Prompt::Recovery(offer) => {
            let recovered = offer
                .original
                .as_ref()
                .and_then(|path| path.file_name())
                .map_or_else(|| hud.tr("untitled"), |name| name.to_string_lossy().into_owned());
            (
                hud.tr("recovery-title"),
                hud.i18n
                    .tr_args("recovery-body", &[("name", FluentValue::from(recovered))]),
                ("recovery-restore", AppCommand::RecoveryRestore),
                ("recovery-discard", AppCommand::RecoveryDiscard),
            )
        }
    };
    let cancel = matches!(prompt, Prompt::Unsaved(_));

    let screen = ctx.content_rect();
    hud.glass.push(Glass {
        rect: screen,
        radius: 0.0,
    });
    let mut chosen = None;
    egui::Area::new(Id::unique("prompt"))
        .order(Order::Tooltip)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(screen, 0, theme::SCRIM);
            let card = egui::Rect::from_center_size(screen.center(), vec2(WIDTH, 168.0));
            ui.painter().rect(
                card,
                14,
                theme::PANEL_SOLID,
                Stroke::new(1.0, theme::BORDER),
                StrokeKind::Inside,
            );
            ui.scope_builder(UiBuilder::new().max_rect(card.shrink(20.0)), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(RichText::new(title).font(theme::bold(16.0)).color(theme::TEXT));
                ui.label(RichText::new(body).size(12.5).color(theme::MUTED));
                ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::text_button(ui, &hud.tr(primary.0), 32.0, Look::Accent, true).clicked() {
                            chosen = Some(primary.1.clone());
                        }
                        if widgets::text_button(ui, &hud.tr(secondary.0), 32.0, Look::Plain, true).clicked() {
                            chosen = Some(secondary.1.clone());
                        }
                        if cancel
                            && widgets::text_button(ui, &hud.tr("unsaved-cancel"), 32.0, Look::Plain, true).clicked()
                        {
                            chosen = Some(AppCommand::PromptCancel);
                        }
                    });
                });
            });
        });
    if ctx.input(|input| input.key_pressed(Key::Escape)) {
        // Escape never destroys anything: it dismisses the prompt (the recovery file stays for the next start).
        chosen = Some(AppCommand::PromptCancel);
    } else if ctx.input(|input| input.key_pressed(Key::Enter)) {
        chosen = Some(primary.1.clone());
    }
    if let Some(command) = chosen {
        hud.send(command);
    }
}
