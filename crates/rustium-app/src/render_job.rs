//! Render jobs: a turntable video or the whole Shot queue, rendered frame by frame on a worker thread with a tracer
//! of its own, so the interface keeps drawing and the job can be cancelled.
//!
//! Videos are encoded by piping raw RGB frames into the `ffmpeg` found on `PATH`. Without `ffmpeg` the frames are
//! written as a PNG sequence instead and the outcome says so.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};

use color_eyre::eyre::{Result, WrapErr, eyre};
use rustium_core::Scene;
use rustium_render::{OfflineRenderer, TraceFrame, TraceImage};
use tracing::{error, info, warn};

use crate::render_mode::{ImageFormat, VIDEO_FPS};
use crate::render_output::{self, SaveOutcome};

/// What a job produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputKind {
    /// One image per frame list entry: a still.
    Still(ImageFormat),
    /// All frames in one video.
    Video,
}

/// One file (or video) of a job and the frames that make it up.
#[derive(Debug, Clone)]
pub(crate) struct Output {
    /// File name without extension, e.g. `shot-2`.
    pub(crate) name: String,
    pub(crate) kind: OutputKind,
    pub(crate) frames: Vec<TraceFrame>,
}

/// Where the dialog sends the results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Destination {
    /// A single file picked in a save dialog (a turntable video).
    File,
    /// A folder picked in a folder dialog (the Shot queue).
    Folder,
}

/// Everything a job renders, decided on the UI thread.
#[derive(Debug, Clone)]
pub(crate) struct JobSpec {
    pub(crate) outputs: Vec<Output>,
    pub(crate) destination: Destination,
    /// Linear exposure applied when tonemapping to 8 bit.
    pub(crate) exposure: f32,
    /// Whether stills keep the alpha channel.
    pub(crate) transparent: bool,
}

impl JobSpec {
    fn total_frames(&self) -> u32 {
        let total: usize = self.outputs.iter().map(|output| output.frames.len()).sum();
        u32::try_from(total).unwrap_or(u32::MAX).max(1)
    }
}

/// Progress shared between the worker and the interface.
#[derive(Debug, Default)]
struct Progress {
    total: AtomicU32,
    done: AtomicU32,
    /// How far the current frame is, in thousandths.
    partial: AtomicU32,
}

/// The interface's side of a running job.
#[derive(Debug)]
pub(crate) struct JobHandle {
    progress: Arc<Progress>,
    cancel: Arc<AtomicBool>,
    results: Receiver<SaveOutcome>,
}

impl JobHandle {
    /// How much of the job is done, `0.0..=1.0`.
    pub(crate) fn fraction(&self) -> f32 {
        let total = self.progress.total.load(Ordering::Relaxed).max(1) as f32;
        let done = self.progress.done.load(Ordering::Relaxed) as f32;
        let partial = self.progress.partial.load(Ordering::Relaxed) as f32 / 1000.0;
        ((done + partial) / total).clamp(0.0, 1.0)
    }

    /// Frames finished and frames in total.
    pub(crate) fn frames(&self) -> (u32, u32) {
        (
            self.progress.done.load(Ordering::Relaxed),
            self.progress.total.load(Ordering::Relaxed),
        )
    }

    /// Asks the worker to stop after the current batch of samples.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The job's result once it has ended.
    pub(crate) fn poll(&self) -> Option<SaveOutcome> {
        self.results.try_recv().ok()
    }
}

/// Starts `spec` on a worker thread. `renderer` gets `scene` uploaded on that thread.
pub(crate) fn spawn(spec: JobSpec, mut renderer: OfflineRenderer, scene: Scene) -> JobHandle {
    let progress = Arc::new(Progress::default());
    progress.total.store(spec.total_frames(), Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    let (sender, results) = mpsc::channel();
    let handle = JobHandle {
        progress: Arc::clone(&progress),
        cancel: Arc::clone(&cancel),
        results,
    };
    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("render-job".to_owned())
        .spawn(move || {
            let outcome = run(&spec, &mut renderer, &scene, &progress, &cancel);
            let _ = worker_sender.send(outcome);
        });
    if let Err(error) = spawned {
        error!(%error, "cannot start the render job thread");
        let _ = sender.send(SaveOutcome::Failed(error.to_string()));
    }
    handle
}

/// Asks where to save, then renders. Never panics; every failure becomes an outcome.
fn run(
    spec: &JobSpec,
    renderer: &mut OfflineRenderer,
    scene: &Scene,
    progress: &Progress,
    cancel: &AtomicBool,
) -> SaveOutcome {
    let Some(target) = ask_where(spec) else {
        return SaveOutcome::Cancelled;
    };
    renderer.upload_scene(scene);
    let mut render = |frame: &TraceFrame, report: &mut dyn FnMut(u32) -> bool| {
        renderer
            .render(frame, report)
            .map_err(|error| eyre!("the GPU could not render the frame: {error}"))
    };
    match render_outputs(spec, &target, "ffmpeg", progress, cancel, &mut render) {
        Ok(outcome) => outcome,
        Err(report) => {
            error!(error = ?report, "render job failed");
            SaveOutcome::Failed(format!("{report:#}"))
        }
    }
}

/// The save or folder dialog for `spec`; `None` when the user cancels.
fn ask_where(spec: &JobSpec) -> Option<PathBuf> {
    match spec.destination {
        Destination::File => rfd::FileDialog::new()
            .add_filter("MP4", &["mp4"])
            .set_file_name("turntable.mp4")
            .save_file()
            .map(|path| path.with_extension("mp4")),
        Destination::Folder => rfd::FileDialog::new().pick_folder(),
    }
}

/// Renders every output of `spec` into `target` (a file for [`Destination::File`], else a folder) with `render`, which
/// is handed the frame and a callback for the samples done that returns `false` to cancel.
///
/// # Errors
///
/// A file cannot be written, `ffmpeg` fails, or `render` fails.
fn render_outputs(
    spec: &JobSpec,
    target: &Path,
    ffmpeg: &str,
    progress: &Progress,
    cancel: &AtomicBool,
    render: &mut impl FnMut(&TraceFrame, &mut dyn FnMut(u32) -> bool) -> Result<Option<TraceImage>>,
) -> Result<SaveOutcome> {
    if spec.destination == Destination::Folder {
        fs::create_dir_all(target).wrap_err("cannot create the output folder")?;
    }
    let mut written: Vec<PathBuf> = Vec::new();
    let mut sequences = Vec::new();
    for output in &spec.outputs {
        let path = match spec.destination {
            Destination::File => target.to_path_buf(),
            Destination::Folder => target.join(format!("{}.{}", output.name, extension(&output.kind))),
        };
        let finished = match &output.kind {
            OutputKind::Still(format) => {
                let Some(frame) = output.frames.first() else {
                    continue;
                };
                match render_frame(frame, progress, cancel, render)? {
                    Some(image) => {
                        write_still(&path, &image, *format, spec)?;
                        Finished::File(path)
                    }
                    None => return Ok(SaveOutcome::Cancelled),
                }
            }
            OutputKind::Video => match render_video(&path, output, spec.exposure, ffmpeg, progress, cancel, render)? {
                Some(finished) => finished,
                None => return Ok(SaveOutcome::Cancelled),
            },
        };
        match finished {
            Finished::File(path) => written.push(path),
            Finished::Sequence(directory) => sequences.push(directory),
        }
    }
    info!(
        files = written.len(),
        sequences = sequences.len(),
        "render job finished"
    );
    Ok(match (written.as_slice(), sequences.as_slice()) {
        ([], [directory, ..]) => SaveOutcome::Sequence(if spec.destination == Destination::Folder {
            target.to_path_buf()
        } else {
            directory.clone()
        }),
        ([path], []) if spec.destination == Destination::File => SaveOutcome::Saved(path.clone()),
        _ => SaveOutcome::Batch {
            folder: target.to_path_buf(),
            count: written.len() + sequences.len(),
        },
    })
}

fn extension(kind: &OutputKind) -> &'static str {
    match kind {
        OutputKind::Still(ImageFormat::Exr) => "exr",
        OutputKind::Still(_) => "png",
        OutputKind::Video => "mp4",
    }
}

fn write_still(path: &Path, image: &TraceImage, format: ImageFormat, spec: &JobSpec) -> Result<()> {
    match format {
        ImageFormat::Exr => render_output::write_exr(path, image),
        ImageFormat::Png | ImageFormat::Mp4 => render_output::write_png(path, image, spec.transparent, spec.exposure),
    }
}

/// Renders one frame, reporting its samples to `progress`; `None` when cancelled.
fn render_frame(
    frame: &TraceFrame,
    progress: &Progress,
    cancel: &AtomicBool,
    render: &mut impl FnMut(&TraceFrame, &mut dyn FnMut(u32) -> bool) -> Result<Option<TraceImage>>,
) -> Result<Option<TraceImage>> {
    progress.partial.store(0, Ordering::Relaxed);
    let target = frame.samples.max(1);
    let mut report = |samples: u32| {
        let thousandths = (u64::from(samples) * 1000 / u64::from(target)).min(1000);
        progress
            .partial
            .store(u32::try_from(thousandths).unwrap_or(1000), Ordering::Relaxed);
        !cancel.load(Ordering::Relaxed)
    };
    let image = render(frame, &mut report)?;
    if image.is_some() {
        progress.done.fetch_add(1, Ordering::Relaxed);
        progress.partial.store(0, Ordering::Relaxed);
    }
    Ok(image)
}

/// How a video ended up on disk.
enum Finished {
    File(PathBuf),
    /// `ffmpeg` is missing: the frames are in this folder as PNG files.
    Sequence(PathBuf),
}

fn render_video(
    path: &Path,
    output: &Output,
    exposure: f32,
    ffmpeg: &str,
    progress: &Progress,
    cancel: &AtomicBool,
    render: &mut impl FnMut(&TraceFrame, &mut dyn FnMut(u32) -> bool) -> Result<Option<TraceImage>>,
) -> Result<Option<Finished>> {
    let mut sink: Option<VideoSink> = None;
    for frame in &output.frames {
        let Some(image) = render_frame(frame, progress, cancel, render)? else {
            if let Some(sink) = sink {
                sink.abort();
            }
            return Ok(None);
        };
        if sink.is_none() {
            sink = Some(VideoSink::open(path, (image.width, image.height), ffmpeg)?);
        }
        if let Some(sink) = &mut sink {
            sink.push(&image, exposure)?;
        }
    }
    sink.map(VideoSink::finish).transpose()
}

/// Where video frames go: an `ffmpeg` process, or numbered PNG files when there is none.
enum VideoSink {
    Ffmpeg {
        child: Child,
        input: Option<ChildStdin>,
        path: PathBuf,
    },
    Sequence {
        directory: PathBuf,
        written: u32,
    },
}

impl VideoSink {
    fn open(path: &Path, size: (u32, u32), program: &str) -> Result<Self> {
        let mut command = Command::new(program);
        command
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
            ])
            .args(["-s", &format!("{}x{}", size.0, size.1)])
            .args(["-r", &VIDEO_FPS.to_string(), "-i", "-"])
            // yuv420p needs even sides.
            .args(["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-crf",
                "18",
                "-movflags",
                "+faststart",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        match command.spawn() {
            Ok(mut child) => {
                let input = child.stdin.take();
                Ok(Self::Ffmpeg {
                    child,
                    input,
                    path: path.to_path_buf(),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warn!(program, "ffmpeg not found, writing a PNG sequence instead");
                let stem = path
                    .file_stem()
                    .map_or_else(|| "video".to_owned(), |stem| stem.to_string_lossy().into_owned());
                let directory = path.with_file_name(format!("{stem}_frames"));
                fs::create_dir_all(&directory).wrap_err("cannot create the frame folder")?;
                Ok(Self::Sequence { directory, written: 0 })
            }
            Err(error) => Err(error).wrap_err("cannot start ffmpeg"),
        }
    }

    fn push(&mut self, image: &TraceImage, exposure: f32) -> Result<()> {
        match self {
            Self::Ffmpeg { child, input, .. } => {
                let bytes = render_output::rgb_bytes(image, exposure);
                let Some(pipe) = input else {
                    return Err(eyre!("ffmpeg has no input"));
                };
                if let Err(error) = pipe.write_all(&bytes) {
                    // ffmpeg usually died with a reason on stderr; prefer that to "broken pipe".
                    let reason = take_stderr(child);
                    return Err(eyre!(
                        "ffmpeg stopped: {}",
                        if reason.is_empty() { error.to_string() } else { reason }
                    ));
                }
                Ok(())
            }
            Self::Sequence { directory, written } => {
                *written += 1;
                render_output::write_png(
                    &directory.join(format!("frame_{written:04}.png")),
                    image,
                    false,
                    exposure,
                )
            }
        }
    }

    fn finish(self) -> Result<Finished> {
        match self {
            Self::Ffmpeg { mut child, input, path } => {
                drop(input);
                let status = child.wait().wrap_err("cannot wait for ffmpeg")?;
                if status.success() {
                    Ok(Finished::File(path))
                } else {
                    let reason = take_stderr(&mut child);
                    let _ = fs::remove_file(&path);
                    Err(eyre!("ffmpeg failed ({status}): {reason}"))
                }
            }
            Self::Sequence { directory, .. } => Ok(Finished::Sequence(directory)),
        }
    }

    /// Stops encoding and removes what was written so far.
    fn abort(self) {
        match self {
            Self::Ffmpeg { mut child, input, path } => {
                drop(input);
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(path);
            }
            Self::Sequence { directory, .. } => {
                let _ = fs::remove_dir_all(directory);
            }
        }
    }
}

/// What the child wrote to stderr, trimmed.
fn take_stderr(child: &mut Child) -> String {
    let mut text = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut text);
    }
    text.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use rustium_render::{Ground, RenderPass, RenderStyle, Sky};

    use super::*;

    fn frame() -> TraceFrame {
        TraceFrame {
            inv_view_proj: [[0.0; 4]; 4],
            size: (6, 4),
            samples: 8,
            bounces: 2,
            sun_turn_degrees: 0.0,
            sun_height_degrees: 40.0,
            sky: Sky::Clear,
            ground: Ground::None,
            transparent: false,
            region: None,
            running: true,
            frame_rect: [0.0; 4],
            exposure: 1.0,
            style: RenderStyle::Traced,
            denoise: false,
            lens: None,
            pass: RenderPass::Beauty,
        }
    }

    fn image(frame: &TraceFrame, shade: f32) -> TraceImage {
        let (width, height) = frame.size;
        TraceImage {
            width,
            height,
            samples: frame.samples,
            rgba: [shade, shade * 0.5, 0.2, 1.0].repeat((width * height) as usize),
            display_ready: false,
        }
    }

    #[expect(clippy::unnecessary_wraps, reason = "has the signature of the render callback")]
    fn fake(frame: &TraceFrame, report: &mut dyn FnMut(u32) -> bool) -> Result<Option<TraceImage>> {
        for samples in 1..=frame.samples {
            if !report(samples) {
                return Ok(None);
            }
        }
        Ok(Some(image(frame, 0.4)))
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rustium-job-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn video_spec(frames: usize) -> JobSpec {
        JobSpec {
            outputs: vec![Output {
                name: "turntable".to_owned(),
                kind: OutputKind::Video,
                frames: vec![frame(); frames],
            }],
            destination: Destination::File,
            exposure: 1.0,
            transparent: false,
        }
    }

    fn has_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn without_ffmpeg_the_frames_become_a_png_sequence() {
        let folder = scratch("sequence");
        let spec = video_spec(3);
        let progress = Progress::default();
        let outcome = render_outputs(
            &spec,
            &folder.join("turn.mp4"),
            "rustium-no-such-ffmpeg",
            &progress,
            &AtomicBool::new(false),
            &mut fake,
        )
        .unwrap();
        let frames = folder.join("turn_frames");
        assert!(matches!(outcome, SaveOutcome::Sequence(ref directory) if *directory == frames));
        assert!(frames.join("frame_0001.png").is_file());
        assert!(frames.join("frame_0003.png").is_file());
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn ffmpeg_encodes_a_turntable_to_mp4() {
        if !has_ffmpeg() {
            eprintln!("no ffmpeg, skipping");
            return;
        }
        let folder = scratch("mp4");
        let path = folder.join("turn.mp4");
        // Odd sides must still encode (yuv420p wants even ones).
        let mut spec = video_spec(6);
        for frame in &mut spec.outputs[0].frames {
            frame.size = (33, 21);
        }
        let outcome = render_outputs(
            &spec,
            &path,
            "ffmpeg",
            &Progress::default(),
            &AtomicBool::new(false),
            &mut fake,
        )
        .unwrap();
        assert!(
            matches!(outcome, SaveOutcome::Saved(ref saved) if *saved == path),
            "{outcome:?}"
        );
        assert!(fs::metadata(&path).unwrap().len() > 100, "the video has content");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_queue_writes_one_file_per_shot() {
        let folder = scratch("queue");
        let spec = JobSpec {
            outputs: (1..=3)
                .map(|number| Output {
                    name: format!("shot-{number}"),
                    kind: OutputKind::Still(ImageFormat::Png),
                    frames: vec![frame()],
                })
                .collect(),
            destination: Destination::Folder,
            exposure: 1.0,
            transparent: false,
        };
        let progress = Progress::default();
        progress.total.store(spec.total_frames(), Ordering::Relaxed);
        let outcome = render_outputs(
            &spec,
            &folder.join("out"),
            "ffmpeg",
            &progress,
            &AtomicBool::new(false),
            &mut fake,
        )
        .unwrap();
        assert!(matches!(outcome, SaveOutcome::Batch { count: 3, .. }), "{outcome:?}");
        for number in 1..=3 {
            assert!(folder.join(format!("out/shot-{number}.png")).is_file());
        }
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn cancelling_stops_the_job_and_removes_the_partial_video() {
        let folder = scratch("cancel");
        let spec = video_spec(5);
        let cancel = AtomicBool::new(false);
        let mut rendered = 0;
        let mut render = |frame: &TraceFrame, report: &mut dyn FnMut(u32) -> bool| {
            rendered += 1;
            if rendered == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
            fake(frame, report)
        };
        let outcome = render_outputs(
            &spec,
            &folder.join("turn.mp4"),
            "rustium-no-such-ffmpeg",
            &Progress::default(),
            &cancel,
            &mut render,
        )
        .unwrap();
        assert!(matches!(outcome, SaveOutcome::Cancelled));
        assert!(!folder.join("turn_frames").exists(), "partial frames are removed");
        assert!(rendered < 5, "the job stopped early, rendered {rendered}");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn progress_fraction_counts_finished_and_partial_frames() {
        let (_sender, results) = mpsc::channel();
        let handle = JobHandle {
            progress: Arc::new(Progress::default()),
            cancel: Arc::new(AtomicBool::new(false)),
            results,
        };
        handle.progress.total.store(4, Ordering::Relaxed);
        handle.progress.done.store(1, Ordering::Relaxed);
        handle.progress.partial.store(500, Ordering::Relaxed);
        assert!((handle.fraction() - 0.375).abs() < 1e-6);
        assert!(handle.poll().is_none());
    }

    #[test]
    fn output_file_extensions_follow_the_kind() {
        assert_eq!(extension(&OutputKind::Video), "mp4");
        assert_eq!(extension(&OutputKind::Still(ImageFormat::Exr)), "exr");
        assert_eq!(extension(&OutputKind::Still(ImageFormat::Png)), "png");
    }
}
