//! Settings → Input: the shortcut list with search, click-to-rebind, conflict resolution, reset, and keymap
//! import and export through native file dialogs.
//!
//! Dialogs run on a worker thread so the interface keeps drawing; the result comes back over a channel.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;
use std::{fs, thread};

use color_eyre::eyre::WrapErr;
use egui::{Color32, ComboBox, Context, RichText, Stroke, Ui, vec2};
use fluent_bundle::FluentValue;
use tracing::warn;

use crate::hud::widgets::{self, Look};
use crate::hud::{Hud, Toast};
use crate::i18n::I18n;
use crate::keymap::{Action, Capture, Chord, Group, Keymap};
use crate::theme;

/// Colour of a chip whose key is shared with another shortcut.
const CONFLICT: Color32 = Color32::from_rgb(0xe5, 0x5c, 0x5c);

/// A rebind that clashes with other shortcuts and waits for the user's decision.
#[derive(Debug)]
struct Pending {
    action: Action,
    chord: Chord,
    others: Vec<Action>,
}

/// How a keymap file dialog ended.
#[derive(Debug)]
enum FileOutcome {
    Imported { keymap: Keymap, skipped: usize },
    Exported(PathBuf),
    Failed { import: bool, reason: String },
    Cancelled,
}

/// Interface state of the Input section.
#[derive(Debug, Default)]
pub(crate) struct KeymapUi {
    /// The shortcut waiting for the next key press.
    capturing: Option<Action>,
    pending: Option<Pending>,
    filter: String,
    job: Option<Receiver<FileOutcome>>,
}

impl KeymapUi {
    /// Whether a key press is being captured, so Escape must not close Settings.
    pub(crate) fn is_capturing(&self) -> bool {
        self.capturing.is_some()
    }
}

pub(crate) fn show(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    poll_job(ui.ctx(), hud);
    handle_capture(ui.ctx(), hud);

    super::settings_window::row(ui, &i18n.tr("setting-keymap"), None, |ui| {
        let custom = !hud.settings.keymap.is_default();
        let selected = if custom { "keymap-custom" } else { "keymap-default" };
        ComboBox::from_id_salt("keymap-preset")
            .selected_text(i18n.tr(selected))
            .width(170.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(!custom, i18n.tr("keymap-default")).clicked() {
                    hud.settings.keymap.reset();
                    hud.ui.keymap_ui.pending = None;
                }
                if custom {
                    ui.add_enabled(false, egui::Button::selectable(true, i18n.tr("keymap-custom")));
                }
            });
    });

    ui.add_space(8.0);
    ui.add(
        egui::TextEdit::singleline(&mut hud.ui.keymap_ui.filter)
            .hint_text(i18n.tr("keymap-search"))
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);

    let query = hud.ui.keymap_ui.filter.trim().to_lowercase();
    let mut shown = 0;
    for group in Group::ALL {
        let actions: Vec<Action> = Action::all()
            .iter()
            .copied()
            .filter(|action| action.group() == group)
            .filter(|action| {
                query.is_empty()
                    || action_name(i18n, *action).to_lowercase().contains(&query)
                    || hud.settings.keymap.label(*action).to_lowercase().contains(&query)
            })
            .collect();
        if actions.is_empty() {
            continue;
        }
        shown += actions.len();
        ui.label(
            RichText::new(i18n.tr(group.message_key()).to_uppercase())
                .size(10.0)
                .color(theme::SUBTLE),
        );
        ui.add_space(2.0);
        egui::Grid::new(("bindings", group.message_key()))
            .num_columns(2)
            .spacing(vec2(28.0, 0.0))
            .show(ui, |ui| {
                for pair in actions.chunks(2) {
                    for action in pair {
                        binding(ui, hud, *action);
                    }
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
    }
    if query.is_empty() {
        mouse_bindings(ui, i18n);
    } else if shown == 0 {
        ui.label(widgets::muted(i18n.tr("keymap-no-match")).size(12.0));
    }

    pending_banner(ui, hud);
    let any_conflict = Action::all()
        .iter()
        .any(|action| hud.settings.keymap.has_conflict(*action));
    if any_conflict {
        ui.label(
            RichText::new(i18n.tr("keymap-conflicts-left"))
                .size(11.0)
                .color(CONFLICT),
        );
    }

    ui.add_space(6.0);
    super::settings_window::row(ui, &i18n.tr("setting-invert-zoom"), None, |ui| {
        widgets::switch(ui, &mut hud.settings.invert_zoom, true);
    });
    let mut import = false;
    let mut export = false;
    super::settings_window::row(
        ui,
        &i18n.tr("setting-rebind"),
        Some(&i18n.tr("setting-rebind-hint")),
        |ui| {
            let idle = hud.ui.keymap_ui.job.is_none();
            import = widgets::text_button(ui, &i18n.tr("keymap-import"), 32.0, Look::Plain, idle).clicked();
            export = widgets::text_button(ui, &i18n.tr("keymap-export"), 32.0, Look::Plain, idle).clicked();
        },
    );
    if import {
        start_job(hud, None);
    } else if export {
        start_job(hud, Some(hud.settings.keymap.to_file_text()));
    }
}

/// Name of a shortcut in the current language.
fn action_name(i18n: &I18n, action: Action) -> String {
    match action.slot_number() {
        Some(number) => i18n.tr_args(action.message_key(), &[("slot", FluentValue::from(i64::from(number)))]),
        None => i18n.tr(action.message_key()),
    }
}

/// One label and its key chip; clicking the chip waits for a new key, right-click unbinds.
fn binding(ui: &mut Ui, hud: &mut Hud<'_>, action: Action) {
    let i18n = hud.i18n;
    let capturing = hud.ui.keymap_ui.capturing == Some(action);
    let conflict = hud.settings.keymap.has_conflict(action);
    ui.horizontal(|ui| {
        ui.set_min_size(vec2(270.0, 36.0));
        ui.label(action_name(i18n, action));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = if capturing {
                i18n.tr("keymap-press")
            } else {
                hud.settings.keymap.label(action)
            };
            let (fill, stroke) = if capturing {
                (
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35),
                    ui.visuals().selection.bg_fill,
                )
            } else if conflict {
                (Color32::from_white_alpha(15), CONFLICT)
            } else {
                (Color32::from_white_alpha(15), Color32::from_white_alpha(46))
            };
            let chip = egui::Button::new(widgets::mono(text, 11.0).color(theme::TEXT))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(6)
                .min_size(vec2(56.0, 24.0));
            let response = ui.add(chip).on_hover_text(i18n.tr("keymap-chip-hint"));
            if response.clicked() {
                hud.ui.keymap_ui.capturing = if capturing { None } else { Some(action) };
                hud.ui.keymap_ui.pending = None;
            } else if response.secondary_clicked() {
                hud.settings.keymap.set(action, None);
                hud.ui.keymap_ui.capturing = None;
            }
        });
    });
}

/// The mouse bindings are fixed and only listed.
fn mouse_bindings(ui: &mut Ui, i18n: &I18n) {
    ui.label(
        RichText::new(i18n.tr("binding-group-mouse").to_uppercase())
            .size(10.0)
            .color(theme::SUBTLE),
    );
    ui.add_space(2.0);
    egui::Grid::new("mouse-bindings")
        .num_columns(2)
        .spacing(vec2(28.0, 0.0))
        .show(ui, |ui| {
            for (label, key) in [("hint-tool-wheel", "RMB"), ("hint-orbit", "MMB")] {
                ui.horizontal(|ui| {
                    ui.set_min_size(vec2(270.0, 36.0));
                    ui.label(i18n.tr(label));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::new()
                            .fill(Color32::from_white_alpha(15))
                            .stroke(Stroke::new(1.0, Color32::from_white_alpha(46)))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| ui.label(widgets::mono(key, 11.0).color(theme::TEXT)));
                    });
                });
            }
            ui.end_row();
        });
    ui.add_space(8.0);
}

/// Applies the key pressed while a chip waits for one.
fn handle_capture(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(action) = hud.ui.keymap_ui.capturing else {
        return;
    };
    match ctx.input(Keymap::capture) {
        None => {}
        Some(Capture::Cancel) => hud.ui.keymap_ui.capturing = None,
        Some(Capture::Chord(chord)) => {
            hud.ui.keymap_ui.capturing = None;
            let others = hud.settings.keymap.conflicts(action, chord);
            if others.is_empty() {
                hud.settings.keymap.set(action, Some(chord));
            } else {
                hud.ui.keymap_ui.pending = Some(Pending { action, chord, others });
            }
        }
    }
}

/// The warning for a rebind that clashes, with the ways to resolve it.
fn pending_banner(ui: &mut Ui, hud: &mut Hud<'_>) {
    let i18n = hud.i18n;
    let Some(pending) = &hud.ui.keymap_ui.pending else {
        return;
    };
    let (action, chord) = (pending.action, pending.chord);
    let others = pending
        .others
        .iter()
        .map(|other| action_name(i18n, *other))
        .collect::<Vec<_>>()
        .join(", ");
    let message = i18n.tr_args(
        "keymap-conflict",
        &[
            ("chord", FluentValue::from(chord.to_string())),
            ("others", FluentValue::from(others)),
        ],
    );
    let mut decision = None;
    egui::Frame::new()
        .fill(CONFLICT.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, CONFLICT))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.label(RichText::new(message).size(12.0));
            ui.horizontal(|ui| {
                if widgets::text_button(ui, &i18n.tr("keymap-swap"), 28.0, Look::On, true).clicked() {
                    decision = Some(true);
                }
                if widgets::text_button(ui, &i18n.tr("keymap-unbind-other"), 28.0, Look::Plain, true).clicked() {
                    decision = Some(false);
                }
                if widgets::text_button(ui, &i18n.tr("keymap-cancel"), 28.0, Look::Plain, true).clicked() {
                    hud.ui.keymap_ui.pending = None;
                }
            });
        });
    match decision {
        Some(true) => hud.settings.keymap.swap(action, chord),
        Some(false) => hud.settings.keymap.take_over(action, chord),
        None => return,
    }
    hud.ui.keymap_ui.pending = None;
}

/// Opens the import (`export == None`) or export dialog on a worker thread.
fn start_job(hud: &mut Hud<'_>, export: Option<String>) {
    let (sender, receiver) = mpsc::channel();
    let name = if export.is_some() {
        "keymap-export"
    } else {
        "keymap-import"
    };
    let spawned = thread::Builder::new().name(name.to_owned()).spawn(move || {
        let outcome = match export {
            Some(text) => export_dialog(&text),
            None => import_dialog(),
        };
        // The window may have closed meanwhile; nobody is left to read the answer.
        let _ = sender.send(outcome);
    });
    match spawned {
        Ok(_) => hud.ui.keymap_ui.job = Some(receiver),
        Err(error) => {
            warn!(%error, "cannot start the keymap dialog");
            hud.ui.toast = Some(Toast::new(hud.tr("keymap-dialog-failed"), true));
        }
    }
}

fn import_dialog() -> FileOutcome {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Rustium keymap", &["toml"])
        .pick_file()
    else {
        return FileOutcome::Cancelled;
    };
    let result = fs::read_to_string(&path)
        .wrap_err_with(|| format!("cannot read {}", path.display()))
        .and_then(|text| Keymap::from_file_text(&text));
    match result {
        Ok((keymap, skipped)) => FileOutcome::Imported { keymap, skipped },
        Err(report) => FileOutcome::Failed {
            import: true,
            reason: format!("{report:#}"),
        },
    }
}

fn export_dialog(text: &str) -> FileOutcome {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Rustium keymap", &["toml"])
        .set_file_name("rustium-keymap.toml")
        .save_file()
    else {
        return FileOutcome::Cancelled;
    };
    match fs::write(&path, text) {
        Ok(()) => FileOutcome::Exported(path),
        Err(error) => FileOutcome::Failed {
            import: false,
            reason: format!("{}: {error}", path.display()),
        },
    }
}

/// Picks up the answer of a finished file dialog.
fn poll_job(ctx: &Context, hud: &mut Hud<'_>) {
    let Some(receiver) = &hud.ui.keymap_ui.job else {
        return;
    };
    let outcome = match receiver.try_recv() {
        Ok(outcome) => outcome,
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        Err(TryRecvError::Disconnected) => FileOutcome::Cancelled,
    };
    hud.ui.keymap_ui.job = None;
    let toast = match outcome {
        FileOutcome::Imported { keymap, skipped } => {
            hud.settings.keymap = keymap;
            hud.ui.keymap_ui.pending = None;
            if skipped == 0 {
                Some((hud.tr("keymap-imported"), false))
            } else {
                let count = i64::try_from(skipped).unwrap_or(i64::MAX);
                let text = hud
                    .i18n
                    .tr_args("keymap-imported-skipped", &[("count", FluentValue::from(count))]);
                Some((text, false))
            }
        }
        FileOutcome::Exported(path) => {
            let text = hud.i18n.tr_args(
                "keymap-exported",
                &[("path", FluentValue::from(path.display().to_string()))],
            );
            Some((text, false))
        }
        FileOutcome::Failed { import, reason } => {
            warn!(%reason, import, "keymap file failed");
            let key = if import {
                "keymap-import-failed"
            } else {
                "keymap-export-failed"
            };
            Some((hud.i18n.tr_args(key, &[("reason", FluentValue::from(reason))]), true))
        }
        FileOutcome::Cancelled => None,
    };
    if let Some((text, error)) = toast {
        hud.ui.toast = Some(Toast::new(text, error));
    }
}
