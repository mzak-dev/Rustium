//! The running app's side of files: New / Open / Save, the unsaved-changes flow, imports and exports, autosave,
//! recovery and the recent files list.

use std::path::{Path, PathBuf};

use fluent_bundle::FluentValue;
use rustium_core::NodeId;
use tracing::{debug, info, warn};
use web_time::{Duration, Instant};

use super::Running;
use crate::files::{self, IoJob, IoOutcome, Operation, Origin, Pending, SaveJob, SaveStart};
use crate::hud::{Prompt, Toast};
use crate::recent;
use crate::settings::Settings;

impl Running {
    /// Whether the Scene differs from what is on disk.
    pub(super) fn is_dirty(&self) -> bool {
        self.doc.is_dirty(self.editor.revision)
    }

    /// Asks about unsaved changes when there are any, then does `action`. Returns `true` when the app should quit.
    pub(super) fn request(&mut self, action: Pending, settings: &Settings) -> bool {
        if self.is_dirty() {
            if let Some(Prompt::Recovery(offer)) = self.ui.prompt.take() {
                self.dismiss_recovery(offer);
            }
            self.ui.prompt = Some(Prompt::Unsaved(action));
            return false;
        }
        self.perform(action, settings)
    }

    /// Does `action` now. Returns `true` when the app should quit.
    pub(super) fn perform(&mut self, action: Pending, settings: &Settings) -> bool {
        match action {
            Pending::New => self.new_document(settings),
            Pending::Open(Some(path)) => {
                self.accept_loss();
                self.io.submit(IoJob::Load { path });
            }
            Pending::Open(None) => {
                self.accept_loss();
                self.io
                    .pick_open(self.i18n.tr("dialog-open"), self.i18n.tr("file-type-scene"));
            }
            Pending::ImportVox => {
                self.accept_loss();
                self.io.pick_import_vox(self.i18n.tr("dialog-import-vox"));
            }
            Pending::Import(path) => {
                self.accept_loss();
                self.io.submit(IoJob::ImportVox { path });
            }
            Pending::Quit => {
                // Synchronous on purpose: the process is about to end and a queued job would never run. A recovery
                // copy still queued or being written must not come back after the delete.
                self.io.delete_for_quit(&self.recovery_files_to_delete());
                return true;
            }
        }
        false
    }

    /// Dismissing a recovery offer keeps the copy for the next start, but the document that is open now (a never-saved
    /// one writes and deletes the same file name) must not be able to destroy it.
    fn dismiss_recovery(&mut self, offer: files::RecoveryOffer) {
        if offer.original.is_none() && offer.recovery == files::recovery_path(None) {
            self.io.submit(IoJob::Shelve(offer.recovery));
        }
    }

    /// Remembers that the user has dealt with the changes made so far (saved or discarded them), so a file that
    /// arrives later replaces the Scene without asking again, unless it was edited in the meantime.
    fn accept_loss(&mut self) {
        self.accepted_loss = Some((self.doc.id(), self.editor.revision));
    }

    /// Whether the Scene has unsaved changes the user has not been asked about since asking for a file.
    fn edited_since_accepting(&self) -> bool {
        self.is_dirty() && self.accepted_loss != Some((self.doc.id(), self.editor.revision))
    }

    /// The recovery files of the open document, except one that is still offered to the user: a never-saved document
    /// shares its file name with the offered copy of an earlier session.
    fn recovery_files_to_delete(&self) -> Vec<PathBuf> {
        let mut files = files::recovery_files(self.doc.path.as_deref());
        if let Some(Prompt::Recovery(offer)) = &self.ui.prompt {
            files.retain(|file| *file != offer.recovery);
        }
        files
    }

    fn new_document(&mut self, settings: &Settings) {
        self.io.submit(IoJob::Delete(self.recovery_files_to_delete()));
        self.editor
            .new_scene(settings.default_voxel_size_cm / 100.0, settings.undo_steps as usize);
        self.viewport.clear_chunks();
        self.preview.invalidate();
        self.trace_revision = None;
        self.camera.frame(None);
        self.ui.collapsed.clear();
        self.ui.renaming = None;
        self.doc.reset(None, self.editor.revision, true);
    }

    /// Saves to the document's file, or asks for one. `then` runs once the save has finished.
    pub(super) fn save(&mut self, then: Option<Pending>, force_dialog: bool) {
        if self.doc.saving {
            return;
        }
        let job = SaveJob {
            scene: Box::new(self.editor.scene.clone()),
            document: self.doc.id(),
            revision: self.editor.revision,
            then,
            obsolete: files::recovery_files(self.doc.path.as_deref()),
        };
        self.doc.saving = true;
        if let (Some(path), false) = (self.doc.path.clone(), force_dialog) {
            self.io.submit(IoJob::Save {
                path,
                scene: job.scene,
                document: job.document,
                revision: job.revision,
                then: job.then,
                obsolete: job.obsolete,
            });
            return;
        }
        let root = self
            .editor
            .scene
            .node(NodeId::ROOT)
            .map_or_else(String::new, |root| root.name().to_owned());
        let file_name = self
            .doc
            .file_name()
            .unwrap_or_else(|| format!("{}.{}", if root.is_empty() { "scene" } else { &root }, files::EXTENSION));
        let start = SaveStart {
            file_name,
            directory: self.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf),
        };
        self.io
            .pick_save(self.i18n.tr("dialog-save"), self.i18n.tr("file-type-scene"), start, job);
    }

    pub(super) fn export_vox(&mut self) {
        let root = self
            .editor
            .scene
            .node(NodeId::ROOT)
            .map_or_else(|| "scene".to_owned(), |root| root.name().to_owned());
        let start = SaveStart {
            file_name: root,
            directory: self.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf),
        };
        self.io.pick_export_vox(
            self.i18n.tr("dialog-export-vox"),
            start,
            Box::new(self.editor.scene.clone()),
        );
    }

    pub(super) fn export_palette(&mut self) {
        let colors = self
            .editor
            .scene
            .palette()
            .iter()
            .map(|(_, color)| color.color)
            .collect();
        let start = SaveStart {
            file_name: "palette".to_owned(),
            directory: self.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf),
        };
        self.io
            .pick_export_palette(self.i18n.tr("dialog-export-palette"), start, colors);
    }

    /// Answers of the prompts. Returns `true` when the app should quit.
    pub(super) fn answer_prompt(&mut self, answer: PromptAnswer, settings: &Settings) -> bool {
        let Some(prompt) = self.ui.prompt.take() else {
            return false;
        };
        match (prompt, answer) {
            (Prompt::Unsaved(action), PromptAnswer::Primary) => {
                if self.doc.saving {
                    // A save is still running; saving again now would drop `action`. It continues when that save
                    // has finished.
                    self.doc.after_save = Some(action);
                } else {
                    self.save(Some(action), false);
                }
            }
            (Prompt::Unsaved(action), PromptAnswer::Secondary) => return self.perform(action, settings),
            (Prompt::Recovery(offer), PromptAnswer::Primary) => {
                let set_aside = offer.recovery != files::recovery_path(offer.original.as_deref());
                let recovery = offer.recovery.clone();
                self.io.submit(IoJob::LoadRecovery(offer));
                if set_aside {
                    // The restored Scene is unsaved work again and is autosaved under the standard name.
                    self.io.submit(IoJob::Delete(vec![recovery]));
                }
            }
            (Prompt::Recovery(offer), PromptAnswer::Secondary) => self.io.submit(IoJob::Delete(vec![offer.recovery])),
            (Prompt::Recovery(offer), PromptAnswer::Cancel) => self.dismiss_recovery(offer),
            (_, PromptAnswer::Cancel) => {}
        }
        false
    }

    /// Writes a recovery copy when the interval has passed and the Scene changed since the last one.
    pub(super) fn autosave_tick(&mut self, settings: &Settings) {
        // The browser could only hand a recovery copy out as a download.
        if !crate::DESKTOP
            || !settings.autosave
            || self.doc.saving
            || matches!(self.ui.prompt, Some(Prompt::Recovery(_)))
        {
            return;
        }
        let now = Instant::now();
        let revision = self.editor.revision;
        if self.doc.autosave_due(revision, now, autosave_interval(settings)) {
            debug!(revision, "writing the recovery copy");
            self.io.submit(IoJob::Autosave {
                path: self.doc.recovery_path(),
                scene: Box::new(self.editor.scene.clone()),
                revision,
            });
            self.doc.note_autosave_started(revision, now);
        }
    }

    /// When the event loop must wake up for the next autosave, if one is pending.
    pub(super) fn next_autosave(&self, settings: &Settings) -> Option<Instant> {
        let waiting = crate::DESKTOP
            && settings.autosave
            && !self.doc.saving
            && !matches!(self.ui.prompt, Some(Prompt::Recovery(_)))
            && self.is_dirty()
            && self.doc.autosaved_revision() != Some(self.editor.revision);
        waiting.then(|| self.doc.next_autosave(autosave_interval(settings)))
    }

    /// Handles what the IO thread reports. Returns `true` when the app should quit.
    pub(super) fn handle_io(&mut self, settings: &Settings) -> bool {
        let mut quit = false;
        for outcome in self.io.poll() {
            match outcome {
                IoOutcome::Opened {
                    path,
                    scene,
                    origin,
                    recovery,
                    source,
                } => {
                    if origin != Origin::Recovery && self.edited_since_accepting() {
                        // The Scene was edited while the file was being chosen or read: ask before replacing it.
                        let name = path
                            .as_deref()
                            .or(source.as_deref())
                            .map(display_name)
                            .unwrap_or_default();
                        let again = match origin {
                            Origin::File => path.map(|path| Pending::Open(Some(path))),
                            _ => source.map(Pending::Import),
                        };
                        match (again, &self.ui.prompt) {
                            (Some(again), None) => self.ui.prompt = Some(Prompt::Unsaved(again)),
                            _ => {
                                // Another question is open; the file is not dropped silently.
                                self.say("file-open-busy", &[("name", FluentValue::from(name))], false);
                            }
                        }
                        continue;
                    }
                    self.accepted_loss = None;
                    self.install_scene(path, *scene, origin, recovery, settings);
                }
                IoOutcome::Saved {
                    path,
                    document,
                    revision,
                    name,
                    then,
                } => {
                    self.remember(&path, &name);
                    self.say("file-saved", &[("name", FluentValue::from(display_name(&path)))], false);
                    if document == self.doc.id() {
                        self.doc.mark_saved(path, revision);
                        if let Some(action) = then {
                            self.doc.after_save = None;
                            quit |= self.perform(action, settings);
                        } else if let Some(action) = self.doc.after_save.take() {
                            // Edits made while the save ran are asked about again.
                            quit |= self.request(action, settings);
                        }
                    } else {
                        // The file was written, but another document is open now: it must not take this path.
                        info!(path = %path.display(), "a save finished after its document was closed");
                    }
                }
                IoOutcome::Exported(path) => {
                    self.say(
                        "file-exported",
                        &[("name", FluentValue::from(display_name(&path)))],
                        false,
                    );
                }
                IoOutcome::PaletteLoaded(colors) => {
                    let applied = self.editor.import_palette(&colors);
                    self.say(
                        "palette-imported",
                        &[("count", FluentValue::from(applied.to_string()))],
                        false,
                    );
                }
                IoOutcome::Autosaved(revision) => debug!(revision, "recovery copy written"),
                IoOutcome::RecentChecked(items) => self.ui.recent = items,
                IoOutcome::Cancelled(document) => self.doc.save_ended(document),
                IoOutcome::Failed {
                    operation,
                    path,
                    message,
                    document,
                } => self.report_failure(operation, &path, &message, document),
            }
        }
        quit
    }

    fn report_failure(&mut self, operation: Operation, path: &Path, message: &str, document: Option<u64>) {
        let key = match operation {
            Operation::Open => "file-error-open",
            Operation::Save => "file-error-save",
            Operation::Import => "file-error-import",
            Operation::Export => "file-error-export",
            Operation::Palette => "file-error-palette",
            // Not a reason to interrupt, but the user should know the safety net is missing.
            Operation::Autosave => {
                self.doc.note_autosave_failed();
                "file-error-autosave"
            }
        };
        if operation == Operation::Save {
            self.doc.save_ended(document);
        }
        if operation == Operation::Open && !path.is_file() {
            // A file that is gone does not belong in the recent list any more.
            self.forget(path);
        }
        self.say(
            key,
            &[
                ("name", FluentValue::from(display_name(path))),
                ("reason", FluentValue::from(message.to_owned())),
            ],
            true,
        );
    }

    /// Replaces the Scene with one that was just read.
    fn install_scene(
        &mut self,
        path: Option<PathBuf>,
        scene: rustium_core::Scene,
        origin: Origin,
        recovery: Option<files::RecoveryOffer>,
        settings: &Settings,
    ) {
        if origin != Origin::Recovery {
            // The old document is gone; keep a recovery copy that is about to be offered for the new one.
            let offered = recovery.as_ref().map(|offer| offer.recovery.clone());
            let stale: Vec<PathBuf> = files::recovery_files(self.doc.path.as_deref())
                .into_iter()
                .filter(|file| Some(file) != offered.as_ref())
                .collect();
            self.io.submit(IoJob::Delete(stale));
        }
        let root_name = crate::files::scene_summary(&scene);
        self.editor.replace_scene(scene, settings.undo_steps as usize);
        self.viewport.clear_chunks();
        self.preview.invalidate();
        self.trace_revision = None;
        self.camera.frame(self.editor.scene.world_volume(NodeId::ROOT));
        self.ui.collapsed.clear();
        self.ui.renaming = None;
        self.ui.splash_open = false;
        if let (Origin::File, Some(path)) = (origin, &path) {
            self.remember(path, &root_name);
        }
        self.doc.reset(path, self.editor.revision, origin == Origin::File);
        info!(?origin, "scene installed");
        if let Some(offer) = recovery {
            self.ui.prompt = Some(Prompt::Recovery(offer));
        }
    }

    fn remember(&mut self, path: &Path, name: &str) {
        recent::remember(&mut self.recent, path, name);
        self.persist_recent();
    }

    fn forget(&mut self, path: &Path) {
        self.recent.retain(|file| file.path != path);
        self.persist_recent();
    }

    fn persist_recent(&mut self) {
        self.io.submit(IoJob::SaveRecent(self.recent.clone()));
        self.io.submit(IoJob::CheckRecent(self.recent.clone()));
    }

    /// Refreshes which recent files still exist when the start screen opens.
    pub(super) fn refresh_recent_on_splash(&mut self) {
        if self.ui.splash_open && !self.splash_was_open {
            self.io.submit(IoJob::CheckRecent(self.recent.clone()));
        }
        self.splash_was_open = self.ui.splash_open;
    }

    fn say(&mut self, key: &str, args: &[(&str, FluentValue<'_>)], error: bool) {
        if error {
            warn!(key, "showing an error to the user");
        }
        self.ui.toast = Some(Toast::new(self.i18n.tr_args(key, args), error));
    }

    /// Keeps the window title in step with the document.
    pub(super) fn update_title(&mut self) {
        let name = self.doc.file_name().unwrap_or_else(|| self.i18n.tr("untitled"));
        let title = format!("{name}{} - Rustium", if self.is_dirty() { "*" } else { "" });
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
        }
        self.ui.document_name = self.doc.file_name().unwrap_or_default();
    }
}

/// How the user answered a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptAnswer {
    /// Save / Restore.
    Primary,
    /// Don't save / Discard.
    Secondary,
    Cancel,
}

fn autosave_interval(settings: &Settings) -> Duration {
    Duration::from_secs(u64::from(settings.autosave_minutes.max(1)) * 60)
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
