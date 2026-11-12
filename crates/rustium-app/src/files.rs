//! Files on disk: the document's path and dirty state, the IO worker thread, native dialogs, autosave and recovery.
//!
//! Every read and write runs on the `io-worker` thread (dialogs on short-lived threads of their own), so the
//! interface keeps drawing. Jobs go in through a channel, outcomes come back through another and wake the event loop.
//! A Scene is cloned on the UI thread (shapes and chunks are shared, so that is cheap) and encoded on the worker.

use std::ffi::OsString;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use rustium_core::{
    FileError, NodeId, PAL_MAX_COLORS, Scene, decode_ruv, encode_ruv, export_vox, import_vox, parse_pal, write_pal,
};
use tracing::{error, info, warn};

use crate::recent::{self, RecentFile, RecentItem};
use crate::settings::data_dir;

/// Extension of Rustium scene files.
pub(crate) const EXTENSION: &str = "ruv";

/// What to do once the user has dealt with unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pending {
    /// Start an empty Scene.
    New,
    /// Open this file, or ask for one.
    Open(Option<PathBuf>),
    /// Ask for a `.vox` file and open it as a new Scene.
    ImportVox,
    /// Open this `.vox` file as a new Scene.
    Import(PathBuf),
    /// Leave the application.
    Quit,
}

/// A recovery copy that can be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryOffer {
    /// The recovery file itself.
    pub(crate) recovery: PathBuf,
    /// The document it belongs to; `None` for a Scene that was never saved.
    pub(crate) original: Option<PathBuf>,
}

/// How a Scene came to be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A `.ruv` file chosen by the user.
    File,
    /// A `.vox` import: a new Scene without a path.
    Import,
    /// A recovery file: unsaved work that stays unsaved.
    Recovery,
}

/// Which kind of operation failed, for the message shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Open,
    Save,
    Import,
    Export,
    Palette,
    Autosave,
}

/// Work for the IO thread.
#[derive(Debug)]
pub(crate) enum IoJob {
    Load {
        path: PathBuf,
    },
    LoadRecovery(RecoveryOffer),
    /// Set a recovery copy aside, see [`shelve_recovery`].
    Shelve(PathBuf),
    ImportVox {
        path: PathBuf,
    },
    Save {
        path: PathBuf,
        scene: Box<Scene>,
        /// The document the Scene was taken from, see [`Document::id`].
        document: u64,
        revision: u64,
        then: Option<Pending>,
        /// Recovery files that become obsolete once the save succeeds.
        obsolete: Vec<PathBuf>,
    },
    Autosave {
        path: PathBuf,
        scene: Box<Scene>,
        revision: u64,
    },
    ExportVox {
        path: PathBuf,
        scene: Box<Scene>,
    },
    ExportPalette {
        path: PathBuf,
        colors: Vec<[u8; 3]>,
    },
    ImportPalette {
        path: PathBuf,
    },
    Delete(Vec<PathBuf>),
    SaveRecent(Vec<RecentFile>),
    CheckRecent(Vec<RecentFile>),
}

/// What the IO thread reports back.
#[derive(Debug)]
pub(crate) enum IoOutcome {
    Opened {
        path: Option<PathBuf>,
        scene: Box<Scene>,
        origin: Origin,
        /// A newer recovery copy next to the file that was opened.
        recovery: Option<RecoveryOffer>,
        /// The file an import was read from, so it can be asked about again.
        source: Option<PathBuf>,
    },
    Saved {
        path: PathBuf,
        /// The document the saved Scene belonged to; the current one may be another by now.
        document: u64,
        revision: u64,
        name: String,
        then: Option<Pending>,
    },
    Exported(PathBuf),
    PaletteLoaded(Vec<[u8; 3]>),
    Autosaved(u64),
    RecentChecked(Vec<RecentItem>),
    /// A dialog was dismissed; carries the document of a Save dialog.
    Cancelled(Option<u64>),
    Failed {
        operation: Operation,
        path: PathBuf,
        message: String,
    },
}

/// Handle to the IO thread.
pub(crate) struct Io {
    jobs: Sender<IoJob>,
    outcome_sender: Sender<IoOutcome>,
    outcomes: Receiver<IoOutcome>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Set once the app is quitting: no recovery copy may be written any more.
    quitting: Arc<AtomicBool>,
    /// Held while a recovery copy is written, so quitting can wait for a write in flight.
    autosave_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for Io {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io").finish_non_exhaustive()
    }
}

impl Io {
    /// Starts the IO thread; `wake` is called after every outcome so the event loop redraws.
    pub(crate) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (jobs, job_receiver) = crossbeam_channel::unbounded::<IoJob>();
        let (outcome_sender, outcomes) = crossbeam_channel::unbounded::<IoOutcome>();
        let worker_outcomes = outcome_sender.clone();
        let worker_wake = Arc::clone(&wake);
        let quitting = Arc::new(AtomicBool::new(false));
        let autosave_lock = Arc::new(Mutex::new(()));
        let worker_quitting = Arc::clone(&quitting);
        let worker_lock = Arc::clone(&autosave_lock);
        let spawned = std::thread::Builder::new().name("io-worker".to_owned()).spawn(move || {
            for job in job_receiver {
                let outcome = if let IoJob::Autosave { .. } = job {
                    let _guard = worker_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if worker_quitting.load(Ordering::SeqCst) {
                        None
                    } else {
                        run(job)
                    }
                } else {
                    run(job)
                };
                if let Some(outcome) = outcome {
                    // A closed channel means the application is shutting down.
                    let _ = worker_outcomes.send(outcome);
                    worker_wake();
                }
            }
        });
        if let Err(error) = spawned {
            error!(%error, "cannot start the IO thread");
        }
        Self {
            jobs,
            outcome_sender,
            outcomes,
            wake,
            quitting,
            autosave_lock,
        }
    }

    /// Stops recovery copies for good and deletes `paths`, waiting for a copy that is being written, so that nothing
    /// re-creates them while the app exits.
    pub(crate) fn delete_for_quit(&self, paths: &[PathBuf]) {
        self.quitting.store(true, Ordering::SeqCst);
        let _guard = self
            .autosave_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for path in paths {
            let _ = fs::remove_file(path);
        }
    }

    /// Queues a job.
    pub(crate) fn submit(&self, job: IoJob) {
        if self.jobs.send(job).is_err() {
            error!("the IO thread is gone");
        }
    }

    /// Takes the outcomes that arrived since the last call.
    pub(crate) fn poll(&self) -> Vec<IoOutcome> {
        self.outcomes.try_iter().collect()
    }

    /// Runs a native dialog on its own thread; a chosen file becomes a job, a dismissed dialog becomes
    /// [`IoOutcome::Cancelled`] (with `document` when it was a Save dialog for that document).
    fn dialog(&self, document: Option<u64>, pick: impl FnOnce() -> Option<IoJob> + Send + 'static) {
        let jobs = self.jobs.clone();
        let outcomes = self.outcome_sender.clone();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("file-dialog".to_owned())
            .spawn(move || {
                if let Some(job) = pick() {
                    let _ = jobs.send(job);
                } else {
                    let _ = outcomes.send(IoOutcome::Cancelled(document));
                    wake();
                }
            });
        if let Err(error) = spawned {
            error!(%error, "cannot start the dialog thread");
        }
    }

    /// Asks for a `.ruv` file to open.
    pub(crate) fn pick_open(&self, title: String, scene_label: String) {
        self.dialog(None, move || {
            let path = rfd::FileDialog::new()
                .set_title(title)
                .add_filter(scene_label, &[EXTENSION])
                .pick_file()?;
            Some(IoJob::Load { path })
        });
    }

    /// Asks for a `.vox` file to import.
    pub(crate) fn pick_import_vox(&self, title: String) {
        self.dialog(None, move || {
            let path = rfd::FileDialog::new()
                .set_title(title)
                .add_filter("MagicaVoxel", &["vox"])
                .pick_file()?;
            Some(IoJob::ImportVox { path })
        });
    }

    /// Asks for a palette file to import.
    pub(crate) fn pick_import_palette(&self, title: String) {
        self.dialog(None, move || {
            let path = rfd::FileDialog::new()
                .set_title(title)
                .add_filter("Palette", &["pal", "png"])
                .pick_file()?;
            Some(IoJob::ImportPalette { path })
        });
    }

    /// Asks where to save the Scene.
    pub(crate) fn pick_save(&self, title: String, scene_label: String, start: SaveStart, job: SaveJob) {
        let document = job.document;
        self.dialog(Some(document), move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title(title)
                .add_filter(scene_label, &[EXTENSION])
                .set_file_name(start.file_name);
            if let Some(directory) = start.directory {
                dialog = dialog.set_directory(directory);
            }
            let path = with_default_extension(dialog.save_file()?, EXTENSION);
            Some(job.into_job(path))
        });
    }

    /// Asks where to export a `.vox` file.
    pub(crate) fn pick_export_vox(&self, title: String, start: SaveStart, scene: Box<Scene>) {
        self.dialog(None, move || {
            let path = save_dialog(title, "MagicaVoxel", "vox", &start)?;
            Some(IoJob::ExportVox { path, scene })
        });
    }

    /// Asks where to export a palette file.
    pub(crate) fn pick_export_palette(&self, title: String, start: SaveStart, colors: Vec<[u8; 3]>) {
        self.dialog(None, move || {
            let path = save_dialog(title, "Palette", "pal", &start)?;
            Some(IoJob::ExportPalette { path, colors })
        });
    }
}

fn save_dialog(title: String, label: &str, extension: &str, start: &SaveStart) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title(title)
        .add_filter(label, &[extension])
        .set_file_name(Path::new(&start.file_name).with_extension(extension).to_string_lossy());
    if let Some(directory) = &start.directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.save_file().map(|path| with_default_extension(path, extension))
}

/// Where a save dialog starts.
#[derive(Debug, Clone)]
pub(crate) struct SaveStart {
    pub(crate) file_name: String,
    pub(crate) directory: Option<PathBuf>,
}

/// What a "Save as" does once a path is chosen.
#[derive(Debug)]
pub(crate) struct SaveJob {
    pub(crate) scene: Box<Scene>,
    pub(crate) document: u64,
    pub(crate) revision: u64,
    pub(crate) then: Option<Pending>,
    pub(crate) obsolete: Vec<PathBuf>,
}

impl SaveJob {
    fn into_job(self, path: PathBuf) -> IoJob {
        IoJob::Save {
            path,
            scene: self.scene,
            document: self.document,
            revision: self.revision,
            then: self.then,
            obsolete: self.obsolete,
        }
    }
}

fn with_default_extension(path: PathBuf, extension: &str) -> PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension(extension)
    }
}

/// Where the recovery copy of a document lives: beside the file, or in the data directory for a Scene that was never
/// saved.
pub(crate) fn recovery_path(document: Option<&Path>) -> PathBuf {
    match document {
        Some(path) => {
            let mut name: OsString = path.as_os_str().to_owned();
            name.push(".recovery");
            PathBuf::from(name)
        }
        None => untitled_recovery_dir().join("untitled.ruv.recovery"),
    }
}

/// The directory of the recovery copies of Scenes that were never saved.
fn untitled_recovery_dir() -> PathBuf {
    data_dir().join("recovery")
}

/// The newest recovery copy of a never-saved Scene in `dir`: the live one or one that was set aside by
/// [`shelve_recovery`].
fn newest_untitled_recovery(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("untitled") && name.ends_with(".ruv.recovery")
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()
        .map(|(_, path)| path)
}

/// Moves the recovery copy at `recovery` aside under a name with the current time, so that the unsaved Scene that is
/// open now (and writes the standard file) cannot overwrite or delete a copy the user has not decided about. The
/// copy is still offered at the next start. Returns the new path.
///
/// # Errors
///
/// The error of the failed rename.
pub(crate) fn shelve_recovery(recovery: &Path) -> std::io::Result<PathBuf> {
    let dir = recovery.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut target = dir.join(format!("untitled-{seconds}.ruv.recovery"));
    let mut number = 1;
    while target.exists() {
        target = dir.join(format!("untitled-{seconds}-{number}.ruv.recovery"));
        number += 1;
    }
    fs::rename(recovery, &target)?;
    Ok(target)
}

/// A recovery copy for `document` that is newer than the document itself (or the document is gone).
pub(crate) fn find_recovery(document: Option<&Path>) -> Option<RecoveryOffer> {
    let recovery = match document {
        Some(_) => recovery_path(document),
        None => newest_untitled_recovery(&untitled_recovery_dir())?,
    };
    let recovered = fs::metadata(&recovery).ok()?.modified().ok()?;
    let saved: Option<SystemTime> = document.and_then(|path| fs::metadata(path).ok()?.modified().ok());
    if saved.is_some_and(|saved| saved >= recovered) {
        return None;
    }
    Some(RecoveryOffer {
        recovery,
        original: document.map(Path::to_path_buf),
    })
}

/// The open document: where it lives and whether it differs from disk.
#[derive(Debug)]
pub(crate) struct Document {
    /// Tells this document from every earlier one, so a late file outcome can be matched to its document.
    id: u64,
    pub(crate) path: Option<PathBuf>,
    /// The Editor revision that matches the file; `None` when the Scene has no clean state (imported, recovered).
    saved_revision: Option<u64>,
    autosaved_revision: Option<u64>,
    last_autosave: Instant,
    /// A save is running; a second autosave or save waits for it.
    pub(crate) saving: bool,
}

impl Document {
    /// A clean, unsaved Scene at `revision`.
    pub(crate) fn new(revision: u64) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            path: None,
            saved_revision: Some(revision),
            autosaved_revision: None,
            last_autosave: Instant::now(),
            saving: false,
        }
    }

    /// Starts tracking a different Scene.
    pub(crate) fn reset(&mut self, path: Option<PathBuf>, revision: u64, clean: bool) {
        *self = Self::new(revision);
        self.path = path;
        if !clean {
            self.saved_revision = None;
        }
    }

    /// Identifies this document among all that were open in this run.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// A recovery copy could not be written: try again at the next interval.
    pub(crate) fn note_autosave_failed(&mut self) {
        self.autosaved_revision = None;
    }

    pub(crate) fn is_dirty(&self, revision: u64) -> bool {
        self.saved_revision != Some(revision)
    }

    pub(crate) fn mark_saved(&mut self, path: PathBuf, revision: u64) {
        self.path = Some(path);
        self.saved_revision = Some(revision);
        self.autosaved_revision = None;
        self.saving = false;
    }

    /// The file name without its folder, if the document has been saved.
    pub(crate) fn file_name(&self) -> Option<String> {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Whether an autosave is due at `now`.
    pub(crate) fn autosave_due(&self, revision: u64, now: Instant, interval: Duration) -> bool {
        self.is_dirty(revision)
            && self.autosaved_revision != Some(revision)
            && now.duration_since(self.last_autosave) >= interval
    }

    /// When the next autosave could fire.
    pub(crate) fn next_autosave(&self, interval: Duration) -> Instant {
        self.last_autosave + interval
    }

    pub(crate) fn note_autosave_started(&mut self, revision: u64, now: Instant) {
        self.autosaved_revision = Some(revision);
        self.last_autosave = now;
    }

    pub(crate) fn autosaved_revision(&self) -> Option<u64> {
        self.autosaved_revision
    }

    pub(crate) fn recovery_path(&self) -> PathBuf {
        recovery_path(self.path.as_deref())
    }
}

/// Writes `bytes` to a temporary file beside `path` and renames it over `path`, so a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut temporary: OsString = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        // The data must be on disk before the rename makes it the document.
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    // Persist the rename itself where the platform lets a directory be opened (not on Windows).
    #[cfg(unix)]
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty())
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn root_name(scene: &Scene) -> String {
    scene
        .node(NodeId::ROOT)
        .map_or_else(String::new, |root| root.name().to_owned())
}

fn failure(operation: Operation, path: &Path, error: impl std::fmt::Display) -> IoOutcome {
    warn!(?operation, path = %path.display(), %error, "file operation failed");
    IoOutcome::Failed {
        operation,
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Executes one job; `None` means there is nothing to report.
fn run(job: IoJob) -> Option<IoOutcome> {
    Some(match job {
        IoJob::Load { path } => match read_scene(&path) {
            Ok(scene) => {
                info!(path = %path.display(), "opened scene");
                let recovery = find_recovery(Some(&path));
                IoOutcome::Opened {
                    path: Some(path),
                    scene: Box::new(scene),
                    origin: Origin::File,
                    recovery,
                    source: None,
                }
            }
            Err(error) => failure(Operation::Open, &path, error),
        },
        IoJob::LoadRecovery(offer) => match read_scene(&offer.recovery) {
            Ok(scene) => IoOutcome::Opened {
                path: offer.original,
                scene: Box::new(scene),
                origin: Origin::Recovery,
                recovery: None,
                source: None,
            },
            Err(error) => {
                // A damaged recovery file would be offered again at every start.
                let _ = fs::remove_file(&offer.recovery);
                failure(Operation::Open, &offer.recovery, error)
            }
        },
        IoJob::ImportVox { path } => {
            let result = fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| import_vox(&bytes, &file_stem(&path)).map_err(|error| error.to_string()));
            match result {
                Ok(scene) => IoOutcome::Opened {
                    path: None,
                    scene: Box::new(scene),
                    origin: Origin::Import,
                    recovery: None,
                    source: Some(path),
                },
                Err(error) => failure(Operation::Import, &path, error),
            }
        }
        IoJob::Save {
            path,
            scene,
            document,
            revision,
            then,
            obsolete,
        } => match encode_ruv(&scene)
            .map_err(|error| error.to_string())
            .and_then(|bytes| write_atomic(&path, &bytes).map_err(|error| error.to_string()))
        {
            Ok(()) => {
                info!(path = %path.display(), "saved scene");
                for recovery in obsolete.iter().chain(&[recovery_path(Some(&path))]) {
                    let _ = fs::remove_file(recovery);
                }
                IoOutcome::Saved {
                    name: root_name(&scene),
                    path,
                    document,
                    revision,
                    then,
                }
            }
            Err(error) => failure(Operation::Save, &path, error),
        },
        IoJob::Autosave { path, scene, revision } => match encode_ruv(&scene)
            .map_err(|error| error.to_string())
            .and_then(|bytes| write_atomic(&path, &bytes).map_err(|error| error.to_string()))
        {
            Ok(()) => IoOutcome::Autosaved(revision),
            Err(error) => failure(Operation::Autosave, &path, error),
        },
        IoJob::ExportVox { path, scene } => match export_vox(&scene)
            .map_err(|error| error.to_string())
            .and_then(|bytes| write_atomic(&path, &bytes).map_err(|error| error.to_string()))
        {
            Ok(()) => IoOutcome::Exported(path),
            Err(error) => failure(Operation::Export, &path, error),
        },
        IoJob::ExportPalette { path, colors } => match write_atomic(&path, write_pal(&colors).as_bytes()) {
            Ok(()) => IoOutcome::Exported(path),
            Err(error) => failure(Operation::Export, &path, error),
        },
        IoJob::ImportPalette { path } => match read_palette(&path) {
            Ok(colors) => IoOutcome::PaletteLoaded(colors),
            Err(error) => failure(Operation::Palette, &path, error),
        },
        IoJob::Shelve(path) => {
            match shelve_recovery(&path) {
                Ok(target) => info!(path = %target.display(), "recovery copy set aside"),
                Err(error) => warn!(path = %path.display(), %error, "could not set the recovery copy aside"),
            }
            return None;
        }
        IoJob::Delete(paths) => {
            for path in paths {
                let _ = fs::remove_file(path);
            }
            return None;
        }
        IoJob::SaveRecent(files) => {
            recent::save(&files);
            return None;
        }
        IoJob::CheckRecent(files) => IoOutcome::RecentChecked(recent::check(files)),
    })
}

fn read_scene(path: &Path) -> Result<Scene, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    decode_ruv(&bytes).map_err(|error| error.to_string())
}

/// Reads a JASC-PAL file, or the pixels of a PNG (row by row, at most 256 of them).
fn read_palette(path: &Path) -> Result<Vec<[u8; 3]>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let is_png = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if is_png {
        palette_from_png(&bytes)
    } else {
        let text = String::from_utf8_lossy(&bytes);
        parse_pal(&text).map_err(|error: FileError| error.to_string())
    }
}

fn palette_from_png(bytes: &[u8]) -> Result<Vec<[u8; 3]>, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("the image is too large")?];
    let info = reader.next_frame(&mut buffer).map_err(|error| error.to_string())?;
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("indexed images are not supported".to_owned()),
    };
    let data = buffer.get(..info.buffer_size()).unwrap_or(&buffer);
    let colors: Vec<[u8; 3]> = data
        .chunks_exact(channels)
        .take(PAL_MAX_COLORS)
        .map(|pixel| {
            if channels < 3 {
                [pixel[0]; 3]
            } else {
                [pixel[0], pixel[1], pixel[2]]
            }
        })
        .collect();
    if colors.is_empty() {
        return Err("the image has no pixels".to_owned());
    }
    Ok(colors)
}

/// The recovery file of a document; the one of the unsaved Scene when `document` is `None`. Other documents'
/// recovery copies are never touched, because they may be work the user has not decided about yet.
pub(crate) fn recovery_files(document: Option<&Path>) -> Vec<PathBuf> {
    vec![recovery_path(document)]
}

#[cfg(test)]
mod tests {
    use rustium_core::{Command, apply};

    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rustium-files-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_saved_scene_loads_back_and_the_recovery_file_is_removed() {
        let dir = directory("save");
        let path = dir.join("hut.ruv");
        let recovery = recovery_path(Some(&path));
        fs::write(&recovery, b"stale").unwrap();
        let mut scene = Scene::new();
        apply(
            &mut scene,
            &Command::Rename {
                node: NodeId::ROOT,
                name: "Hut".to_owned(),
            },
        )
        .unwrap();
        let outcome = run(IoJob::Save {
            path: path.clone(),
            scene: Box::new(scene),
            document: 1,
            revision: 7,
            then: Some(Pending::New),
            obsolete: Vec::new(),
        });
        assert!(matches!(
            outcome,
            Some(IoOutcome::Saved { revision: 7, then: Some(Pending::New), ref name, .. }) if name == "Hut"
        ));
        assert!(!recovery.exists());
        let outcome = run(IoJob::Load { path });
        let Some(IoOutcome::Opened { scene, origin, .. }) = outcome else {
            unreachable!("the file should load");
        };
        assert_eq!(origin, Origin::File);
        assert_eq!(root_name(&scene), "Hut");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_file_is_a_failure_not_a_panic() {
        let dir = directory("corrupt");
        let path = dir.join("broken.ruv");
        fs::write(&path, b"RUSTIUMV\x01\x00\x00\x00junk").unwrap();
        let outcome = run(IoJob::Load { path });
        assert!(matches!(
            outcome,
            Some(IoOutcome::Failed {
                operation: Operation::Open,
                ..
            })
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_recovery_copy_is_offered_only_when_it_is_newer() {
        let dir = directory("recovery");
        let path = dir.join("ship.ruv");
        fs::write(&path, b"x").unwrap();
        assert!(find_recovery(Some(&path)).is_none());
        std::thread::sleep(Duration::from_millis(20));
        fs::write(recovery_path(Some(&path)), b"y").unwrap();
        let offer = find_recovery(Some(&path)).unwrap();
        assert_eq!(offer.original.as_deref(), Some(path.as_path()));
        // The document itself missing means only the recovery copy is left.
        fs::remove_file(&path).unwrap();
        assert!(find_recovery(Some(&path)).is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn autosave_writes_a_loadable_recovery_file() {
        let dir = directory("autosave");
        let path = dir.join("nested").join("a.ruv.recovery");
        let outcome = run(IoJob::Autosave {
            path: path.clone(),
            scene: Box::new(Scene::new()),
            revision: 3,
        });
        assert!(matches!(outcome, Some(IoOutcome::Autosaved(3))));
        assert!(read_scene(&path).is_ok());
        assert!(!dir.join("nested").join("a.ruv.recovery.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn palettes_round_trip_through_pal_and_png() {
        let dir = directory("palette");
        let colors = vec![[1, 2, 3], [250, 251, 252]];
        let path = dir.join("p.pal");
        assert!(matches!(
            run(IoJob::ExportPalette {
                path: path.clone(),
                colors: colors.clone()
            }),
            Some(IoOutcome::Exported(_))
        ));
        assert_eq!(read_palette(&path).unwrap(), colors);

        let png_path = dir.join("p.png");
        let file = fs::File::create(&png_path).unwrap();
        let mut encoder = png::Encoder::new(file, 2, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[1, 2, 3, 250, 251, 252])
            .unwrap();
        assert_eq!(read_palette(&png_path).unwrap(), colors);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_shelved_recovery_copy_is_found_again_and_survives_the_live_one() {
        let dir = directory("shelve");
        let live = dir.join("untitled.ruv.recovery");
        fs::write(&live, b"offered").unwrap();
        let shelved = shelve_recovery(&live).unwrap();
        assert!(!live.exists(), "the standard name is free for the new document");
        assert_eq!(fs::read(&shelved).unwrap(), b"offered");
        assert_eq!(newest_untitled_recovery(&dir), Some(shelved.clone()));
        // The new document autosaves under the standard name; the shelved copy is not touched.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&live, b"new work").unwrap();
        assert_eq!(fs::read(&shelved).unwrap(), b"offered");
        assert_eq!(newest_untitled_recovery(&dir), Some(live.clone()));
        fs::remove_file(&live).unwrap();
        assert_eq!(newest_untitled_recovery(&dir), Some(shelved.clone()));
        // Two copies set aside in the same second get different names.
        fs::write(&live, b"second").unwrap();
        let second = shelve_recovery(&live).unwrap();
        assert_ne!(second, shelved);
        assert_eq!(fs::read(&shelved).unwrap(), b"offered");
    }

    #[test]
    fn recovery_files_never_include_another_documents_copy() {
        let path = PathBuf::from("a.ruv");
        assert_eq!(recovery_files(Some(&path)), vec![recovery_path(Some(&path))]);
        assert!(!recovery_files(Some(&path)).contains(&recovery_path(None)));
    }

    #[test]
    fn every_document_gets_its_own_id_even_after_a_reset() {
        let mut document = Document::new(1);
        let first = document.id();
        document.reset(None, 2, true);
        assert_ne!(document.id(), first);
    }

    #[test]
    fn a_failed_autosave_is_tried_again() {
        let mut document = Document::new(1);
        let start = document.last_autosave;
        let interval = Duration::from_secs(60);
        document.note_autosave_started(2, start);
        document.note_autosave_failed();
        assert!(document.autosave_due(2, start + interval, interval));
    }

    #[test]
    fn writing_replaces_the_file_and_leaves_no_temporary_one() {
        let dir = directory("atomic");
        let path = dir.join("a.bin");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!dir.join("a.bin.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dirty_tracking_follows_the_revision() {
        let mut document = Document::new(4);
        assert!(!document.is_dirty(4));
        assert!(document.is_dirty(5));
        document.reset(None, 9, false);
        assert!(document.is_dirty(9), "an imported scene starts unsaved");
        document.mark_saved(PathBuf::from("a.ruv"), 9);
        assert!(!document.is_dirty(9));
        assert_eq!(document.file_name().as_deref(), Some("a.ruv"));
    }

    #[test]
    fn autosave_waits_for_the_interval_and_for_changes() {
        let mut document = Document::new(1);
        let start = document.last_autosave;
        let interval = Duration::from_secs(60);
        assert!(
            !document.autosave_due(1, start + interval, interval),
            "clean documents are not saved"
        );
        assert!(!document.autosave_due(2, start, interval), "too early");
        assert!(document.autosave_due(2, start + interval, interval));
        document.note_autosave_started(2, start + interval);
        assert!(
            !document.autosave_due(2, start + interval * 2, interval),
            "nothing changed since"
        );
        assert!(document.autosave_due(3, start + interval * 2, interval));
    }
}
