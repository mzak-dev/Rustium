//! State and pure logic of Render mode: Shots, lens, light and output settings, and the [`TraceFrame`] the
//! renderer needs. The interface for it lives in `hud/render_*.rs`.

use std::time::{Duration, Instant};

use glam::Vec3;
use rustium_render::{Ground, RenderPass, RenderStyle, Sky, ThinLens, TraceFrame, TraceProgress};

use crate::camera::{OrbitCamera, Projection};
use crate::icons::Icon;
use crate::render_job::JobHandle;

/// Most Shots a Scene holds.
pub(crate) const MAX_SHOTS: usize = 5;
/// Height of the virtual sensor; Lens (mm) becomes a vertical field of view through it.
const SENSOR_HEIGHT_MM: f32 = 24.0;
/// Default exposure compensation: -1 EV puts the studio backdrop near a light grey instead of white.
const DEFAULT_EXPOSURE_EV: f32 = -1.0;
/// Range of the Lens field.
pub(crate) const LENS_RANGE_MM: (f32, f32) = (8.0, 300.0);
/// Most pixels the interactive preview traces. The picture is shown at most this big and scaled up by the display;
/// the saved image is always rendered at the full output size by the offline renderer.
const PREVIEW_MAX_PIXELS: f32 = 1_300_000.0;
/// Smallest side of the interactive preview.
const PREVIEW_MIN_SIDE: u32 = 16;
/// Range of an output side in pixels.
pub(crate) const SIDE_RANGE: (u32, u32) = (16, 32768);
/// What a GPU that has not been asked yet is assumed to allow: the side of an image and the bytes its textures take.
const DEFAULT_IMAGE_LIMITS: (u32, u64) = (8192, 3 << 30);

/// Why an output size cannot be rendered on this GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SizeProblem {
    /// GPU memory the image needs, in bytes.
    pub(crate) needed: u64,
    /// GPU memory one image may take, in bytes.
    pub(crate) allowed: u64,
    /// The longest side a texture may have.
    pub(crate) max_side: u32,
}
/// Range of the aperture as an f-number.
pub(crate) const APERTURE_RANGE: (f32, f32) = (1.0, 32.0);
/// Range of the exposure compensation in stops.
pub(crate) const EXPOSURE_RANGE: (f32, f32) = (-6.0, 6.0);
/// Range of the number of frames of a turntable.
pub(crate) const TURNTABLE_RANGE: (u32, u32) = (8, 720);
/// Frame rate of rendered videos.
pub(crate) const VIDEO_FPS: u32 = 24;

/// Which Render tool the left toolbar has active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderTool {
    Orbit,
    Pan,
    Dolly,
    PickFocus,
    AimSun,
    Region,
}

impl RenderTool {
    pub(crate) const ALL: [Self; 6] = [
        Self::Orbit,
        Self::Pan,
        Self::Dolly,
        Self::PickFocus,
        Self::AimSun,
        Self::Region,
    ];

    pub(crate) fn key(self) -> egui::Key {
        match self {
            Self::Orbit => egui::Key::O,
            Self::Pan => egui::Key::H,
            Self::Dolly => egui::Key::Z,
            Self::PickFocus => egui::Key::F,
            Self::AimSun => egui::Key::L,
            Self::Region => egui::Key::R,
        }
    }

    pub(crate) fn icon(self) -> Icon {
        match self {
            Self::Orbit => Icon::Orbit,
            Self::Pan => Icon::Move,
            Self::Dolly => Icon::Dolly,
            Self::PickFocus => Icon::Focus,
            Self::AimSun => Icon::Sun,
            Self::Region => Icon::SelectBox,
        }
    }

    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Orbit => "rtool-orbit",
            Self::Pan => "rtool-pan",
            Self::Dolly => "rtool-dolly",
            Self::PickFocus => "rtool-focus",
            Self::AimSun => "rtool-sun",
            Self::Region => "rtool-region",
        }
    }

    /// Tools drawn with a divider above them.
    pub(crate) fn starts_group(self) -> bool {
        matches!(self, Self::PickFocus | Self::Region)
    }
}

/// Tabs of the Render-mode inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderTab {
    Render,
    Camera,
    Light,
    Output,
}

/// What the camera orbits around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrbitAround {
    Model,
    Cursor,
}

/// File format of Save image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Exr,
    /// A turntable video; needs `ffmpeg`.
    Mp4,
}

/// What shows behind the model in the saved image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backdrop {
    Keep,
    Transparent,
}

/// Camera projection choices of the Camera tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewProjection {
    Perspective,
    Orthographic,
    Isometric,
}

/// A saved camera and output size.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shot {
    pub(crate) camera: OrbitCamera,
    pub(crate) lens_mm: f32,
    pub(crate) size: (u32, u32),
    /// Name given by the user; `None` shows "Shot n".
    pub(crate) name: Option<String>,
    /// The icon of the Shot's tile; it stays with the Shot when others are deleted.
    pub(crate) icon: Icon,
}

/// The icons Shot tiles take, in the order new Shots pick them.
pub(crate) const SHOT_ICONS: [Icon; 5] = [Icon::Cube, Icon::Fit, Icon::Palette, Icon::Grid, Icon::Eye];

/// What the render is doing, for the status readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderStatus {
    Rendering,
    Paused,
    Done,
}

/// Everything Render mode remembers.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent HUD toggles, not a state machine"
)]
pub(crate) struct RenderState {
    pub(crate) tool: RenderTool,
    pub(crate) tab: RenderTab,
    pub(crate) around: OrbitAround,
    pub(crate) samples: u32,
    pub(crate) bounces: u32,
    pub(crate) lens_mm: f32,
    pub(crate) projection: ViewProjection,
    pub(crate) style: RenderStyle,
    pub(crate) denoise: bool,
    /// Which layer of the render the frame shows and Save image writes.
    pub(crate) pass: RenderPass,
    pub(crate) dof: bool,
    /// Aperture as an f-number (smaller is a stronger blur).
    pub(crate) aperture: f32,
    /// Distance to the focal plane in metres; `None` follows the distance to the orbit target.
    pub(crate) focus_m: Option<f32>,
    /// Where the Focus tool last picked, as fractions of the image, for the reticle.
    pub(crate) focus_marker: Option<[f32; 2]>,
    /// Frames of a turntable video: one full turn of the camera around the target.
    pub(crate) turntable_frames: u32,
    /// The Save video or Queue job running on a worker thread.
    pub(crate) job: Option<JobHandle>,
    /// The longest image side and most GPU bytes an image may take on this GPU.
    pub(crate) image_limits: (u32, u64),
    /// Locale key of the message for a failed job, by what the job was (still, video, queue).
    pub(crate) failure_key: &'static str,
    pub(crate) sun_turn_degrees: f32,
    pub(crate) sun_height_degrees: f32,
    pub(crate) sky: Sky,
    pub(crate) ground: Ground,
    pub(crate) size: (u32, u32),
    pub(crate) format: ImageFormat,
    pub(crate) backdrop: Backdrop,
    pub(crate) shots: Vec<Shot>,
    pub(crate) active_shot: usize,
    /// Whether the user has added, renamed, updated or deleted a Shot. Until then the one default Shot is just the
    /// working view, and the queue renders what is on screen.
    pub(crate) shots_touched: bool,
    /// What the camera orbits around during the current drag, fixed when the button went down.
    pub(crate) orbit_pivot: Option<Vec3>,
    /// Region to render as fractions of the image, `[x0, y0, x1, y1]`.
    pub(crate) region: Option<[f32; 4]>,
    /// Where a Render region drag started, as fractions of the image.
    pub(crate) region_anchor: Option<[f32; 2]>,
    pub(crate) running: bool,
    pub(crate) guides: bool,
    /// Exposure compensation in stops, applied before tonemapping (preview and saved PNG).
    pub(crate) exposure_ev: f32,
    /// Where the HUD put the image this frame, in points: `[x, y, width, height]`.
    pub(crate) frame_rect: Option<[f32; 4]>,
    pub(crate) progress: TraceProgress,
    /// Result of the last Save image, already translated.
    pub(crate) message: Option<String>,
    started: Instant,
    /// Time the render spent paused since `started`, and since when it is paused now.
    paused_total: Duration,
    paused_since: Option<Instant>,
    generation: u64,
}

impl RenderState {
    pub(crate) fn new(camera: &OrbitCamera) -> Self {
        let lens_mm = 35.0;
        let size = (1920, 1080);
        Self {
            tool: RenderTool::Orbit,
            tab: RenderTab::Render,
            around: OrbitAround::Model,
            samples: 1024,
            bounces: 6,
            lens_mm,
            projection: ViewProjection::Perspective,
            style: RenderStyle::Traced,
            denoise: true,
            pass: RenderPass::Beauty,
            dof: false,
            aperture: 2.8,
            focus_m: None,
            focus_marker: None,
            turntable_frames: 72,
            job: None,
            image_limits: DEFAULT_IMAGE_LIMITS,
            failure_key: "save-failed",
            sun_turn_degrees: 135.0,
            sun_height_degrees: 42.0,
            sky: Sky::Clear,
            ground: Ground::Studio,
            size,
            format: ImageFormat::Png,
            backdrop: Backdrop::Keep,
            shots: vec![Shot {
                camera: camera.clone(),
                lens_mm,
                size,
                name: None,
                icon: SHOT_ICONS[0],
            }],
            active_shot: 0,
            shots_touched: false,
            orbit_pivot: None,
            region: None,
            region_anchor: None,
            running: true,
            guides: true,
            exposure_ev: DEFAULT_EXPOSURE_EV,
            frame_rect: None,
            progress: TraceProgress {
                samples: 0,
                target: 1,
                partial: 0.0,
                generation: 0,
            },
            message: None,
            started: Instant::now(),
            paused_total: Duration::ZERO,
            paused_since: None,
            generation: 0,
        }
    }

    /// Called when Render mode opens: until the user manages Shots themselves, the live view is Shot 1.
    pub(crate) fn enter(&mut self, camera: &OrbitCamera) {
        self.projection = projection_of(camera);
        if !self.shots_touched {
            self.sync_active_shot(camera);
        }
        self.message = None;
    }

    /// The camera the path tracer renders from: the modelling camera with the Lens applied.
    pub(crate) fn render_camera(&self, camera: &OrbitCamera) -> OrbitCamera {
        OrbitCamera {
            field_of_view_degrees: lens_to_fov_degrees(self.lens_mm),
            ..camera.clone()
        }
    }

    /// Linear exposure multiplier for the tonemapper.
    pub(crate) fn exposure(&self) -> f32 {
        self.exposure_ev.exp2()
    }

    pub(crate) fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }

    /// Applies the Camera tab's projection choice to `camera`.
    pub(crate) fn apply_projection(&mut self, camera: &mut OrbitCamera, projection: ViewProjection) {
        self.projection = projection;
        match projection {
            ViewProjection::Perspective => camera.projection = Projection::Perspective,
            ViewProjection::Orthographic => camera.projection = Projection::Orthographic,
            ViewProjection::Isometric => camera.isometric(),
        }
    }

    /// Stores the live view, lens and output size in the active Shot.
    fn sync_active_shot(&mut self, camera: &OrbitCamera) {
        let (lens_mm, size) = (self.lens_mm, self.size);
        if let Some(shot) = self.shots.get_mut(self.active_shot) {
            shot.camera.clone_from(camera);
            shot.lens_mm = lens_mm;
            shot.size = size;
        }
    }

    /// Whether the live view, lens or output size differ from what Shot `index` saved. Only the active Shot can.
    pub(crate) fn shot_modified(&self, index: usize, camera: &OrbitCamera) -> bool {
        index == self.active_shot
            && self.shots.get(index).is_some_and(|shot| {
                shot.camera != *camera || (shot.lens_mm - self.lens_mm).abs() > 1e-4 || shot.size != self.size
            })
    }

    /// Replaces Shot `index`'s saved view with the live one and makes it the active Shot. The Shot that was active
    /// keeps what it saved.
    pub(crate) fn update_shot(&mut self, index: usize, camera: &OrbitCamera) {
        if index >= self.shots.len() {
            return;
        }
        self.shots_touched = true;
        self.active_shot = index;
        self.sync_active_shot(camera);
    }

    /// The icon for a new Shot: the first one no Shot uses yet.
    fn free_shot_icon(&self) -> Icon {
        SHOT_ICONS
            .into_iter()
            .find(|icon| self.shots.iter().all(|shot| shot.icon != *icon))
            .unwrap_or(SHOT_ICONS[self.shots.len() % SHOT_ICONS.len()])
    }

    /// Adds a Shot from the current view and makes it active; `false` when all [`MAX_SHOTS`] are used. The Shots that
    /// exist keep what they saved.
    pub(crate) fn add_shot(&mut self, camera: &OrbitCamera) -> bool {
        if self.shots.len() >= MAX_SHOTS {
            return false;
        }
        self.shots_touched = true;
        let icon = self.free_shot_icon();
        self.shots.push(Shot {
            camera: camera.clone(),
            lens_mm: self.lens_mm,
            size: self.size,
            name: None,
            icon,
        });
        self.active_shot = self.shots.len() - 1;
        true
    }

    /// Removes Shot `index`; the last Shot cannot be removed. When the active Shot goes, its neighbour becomes the live
    /// view.
    pub(crate) fn delete_shot(&mut self, index: usize, camera: &mut OrbitCamera) {
        if self.shots.len() <= 1 || index >= self.shots.len() {
            return;
        }
        self.shots_touched = true;
        self.shots.remove(index);
        if index < self.active_shot {
            self.active_shot -= 1;
        } else if index == self.active_shot {
            let next = index.min(self.shots.len() - 1);
            self.active_shot = next;
            self.load_shot(next, camera);
        }
    }

    /// Names Shot `index`; an empty or blank name goes back to "Shot n". Spaces are kept so the name can be typed.
    ///
    /// Naming a Shot is not managing Shots: the queue keeps rendering the live view until a Shot is added, updated or
    /// deleted.
    pub(crate) fn rename_shot(&mut self, index: usize, name: &str) {
        if let Some(shot) = self.shots.get_mut(index) {
            shot.name = (!name.trim().is_empty()).then(|| name.chars().take(40).collect());
        }
    }

    /// The Shots the queue renders. Until the user manages Shots, the one default Shot is the working view, so the
    /// queue renders what is on screen; afterwards every Shot renders as saved.
    pub(crate) fn queued_shots(&self, camera: &OrbitCamera) -> Vec<Shot> {
        if self.shots_touched {
            return self.shots.clone();
        }
        vec![Shot {
            camera: camera.clone(),
            lens_mm: self.lens_mm,
            size: self.size,
            name: self.shots.get(self.active_shot).and_then(|shot| shot.name.clone()),
            icon: self.shots.get(self.active_shot).map_or(SHOT_ICONS[0], |shot| shot.icon),
        }]
    }

    /// Makes Shot `index` the live view. Changes made to the live view since the Shot was saved are dropped.
    pub(crate) fn select_shot(&mut self, index: usize, camera: &mut OrbitCamera) {
        if index >= self.shots.len() {
            return;
        }
        self.active_shot = index;
        self.load_shot(index, camera);
    }

    fn load_shot(&mut self, index: usize, camera: &mut OrbitCamera) {
        let Some(shot) = self.shots.get(index) else {
            return;
        };
        camera.clone_from(&shot.camera);
        self.lens_mm = shot.lens_mm;
        self.size = shot.size;
        self.projection = projection_of(camera);
    }

    /// The pixel rectangle of the render region in an image of `size`, if one is set and not degenerate.
    pub(crate) fn pixel_region(&self, size: (u32, u32)) -> Option<[u32; 4]> {
        let [x0, y0, x1, y1] = self.region?;
        let region = [
            fraction_to_pixels(x0.min(x1), size.0),
            fraction_to_pixels(y0.min(y1), size.1),
            fraction_to_pixels(x0.max(x1), size.0),
            fraction_to_pixels(y0.max(y1), size.1),
        ];
        (region[2] > region[0] && region[3] > region[1]).then_some(region)
    }

    /// The size the interactive preview traces for a frame shown `shown` physical pixels big: what is on screen, at
    /// the output's aspect, capped at [`PREVIEW_MAX_PIXELS`] and never above the output size. Saved images are
    /// rendered at the output size by the offline renderer instead.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the sizes are positive and at most the output side"
    )]
    pub(crate) fn preview_size(&self, shown: (f32, f32)) -> (u32, u32) {
        let aspect = self.aspect();
        let width = shown.0.max(1.0);
        let shrink = (PREVIEW_MAX_PIXELS / (width * (width / aspect)).max(1.0))
            .sqrt()
            .min(1.0);
        let preview_width =
            ((width * shrink).round() as u32).clamp(PREVIEW_MIN_SIDE, self.size.0.max(PREVIEW_MIN_SIDE));
        let preview_height =
            ((preview_width as f32 / aspect).round() as u32).clamp(PREVIEW_MIN_SIDE, self.size.1.max(PREVIEW_MIN_SIDE));
        (preview_width, preview_height)
    }

    /// The renderer's input for this frame, traced at the preview size; `scale` converts HUD points to physical pixels
    /// and `voxel_size` is the Scene's voxel edge in metres.
    pub(crate) fn trace_frame(&self, camera: &OrbitCamera, scale: f32, voxel_size: f32) -> Option<TraceFrame> {
        let [x, y, width, height] = self.frame_rect?;
        let preview = self.preview_size((width * scale, height * scale));
        Some(TraceFrame {
            region: self.pixel_region(preview),
            running: self.running,
            frame_rect: [x * scale, y * scale, width * scale, height * scale],
            ..self.frame_for(camera, self.lens_mm, preview, voxel_size)
        })
    }

    /// A frame that renders `camera` through a lens of `lens_mm` into an image of `size`, with every other setting
    /// from here. Used for the live view, Shots and turntable frames alike.
    pub(crate) fn frame_for(
        &self,
        camera: &OrbitCamera,
        lens_mm: f32,
        size: (u32, u32),
        voxel_size: f32,
    ) -> TraceFrame {
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let lensed = OrbitCamera {
            field_of_view_degrees: lens_to_fov_degrees(lens_mm),
            ..camera.clone()
        };
        let focus_voxels = self
            .focus_m
            .map_or(camera.distance, |metres| metres / voxel_size.max(1e-6));
        TraceFrame {
            inv_view_proj: lensed.view_proj(aspect).inverse().to_cols_array_2d(),
            size,
            samples: self.samples,
            bounces: self.bounces,
            sun_turn_degrees: self.sun_turn_degrees,
            sun_height_degrees: self.sun_height_degrees,
            sky: self.sky,
            ground: self.ground,
            transparent: self.backdrop == Backdrop::Transparent,
            region: None,
            running: true,
            frame_rect: [0.0, 0.0, size.0 as f32, size.1 as f32],
            exposure: self.exposure(),
            style: self.style,
            denoise: self.denoise,
            lens: self.dof.then(|| ThinLens {
                eye: camera.eye().to_array(),
                radius: aperture_radius_voxels(lens_mm, self.aperture, focus_voxels),
                focus_distance: focus_voxels,
            }),
            pass: self.pass,
        }
    }

    /// The frames of a turntable around `camera`'s target: one full turn in `turntable_frames` steps. The background
    /// is never transparent because video has no alpha.
    pub(crate) fn turntable_frames(
        &self,
        camera: &OrbitCamera,
        lens_mm: f32,
        size: (u32, u32),
        voxel_size: f32,
    ) -> Vec<TraceFrame> {
        let count = self.turntable_frames.clamp(TURNTABLE_RANGE.0, TURNTABLE_RANGE.1);
        (0..count)
            .map(|index| {
                let mut turned = camera.clone();
                turned.yaw += std::f32::consts::TAU * index as f32 / count as f32;
                TraceFrame {
                    transparent: false,
                    ..self.frame_for(&turned, lens_mm, size, voxel_size)
                }
            })
            .collect()
    }

    /// Distance from the camera to the focal plane in metres, as the Camera tab shows it.
    pub(crate) fn focus_metres(&self, camera: &OrbitCamera, voxel_size: f32) -> f32 {
        self.focus_m.unwrap_or(camera.distance * voxel_size)
    }

    /// Records the renderer's progress; a restart resets the clock the ETA runs on, and time spent paused does not
    /// count towards it.
    pub(crate) fn update_progress(&mut self, progress: TraceProgress, now: Instant) {
        if progress.generation != self.generation {
            self.generation = progress.generation;
            self.started = now;
            self.paused_total = Duration::ZERO;
            self.paused_since = None;
        }
        match (self.running, self.paused_since) {
            (false, None) => self.paused_since = Some(now),
            (true, Some(since)) => {
                self.paused_total += now.saturating_duration_since(since);
                self.paused_since = None;
            }
            _ => {}
        }
        self.progress = progress;
    }

    pub(crate) fn status(&self) -> RenderStatus {
        if self.progress.samples >= self.progress.target {
            RenderStatus::Done
        } else if self.running {
            RenderStatus::Rendering
        } else {
            RenderStatus::Paused
        }
    }

    /// Why an image of `size` cannot be rendered on this GPU, or `None` when it can.
    pub(crate) fn size_problem(&self, size: (u32, u32)) -> Option<SizeProblem> {
        let (max_side, allowed) = self.image_limits;
        let needed = rustium_render::trace_image_bytes(size);
        (size.0 > max_side || size.1 > max_side || needed > allowed).then_some(SizeProblem {
            needed,
            allowed,
            max_side,
        })
    }

    /// Whether the renderer still wants frames.
    pub(crate) fn is_accumulating(&self) -> bool {
        self.running && self.progress.samples < self.progress.target
    }

    /// Time left at the pace so far, or `None` before anything is traced or when finished. A sample that is still
    /// being swept band by band counts by the part that is done, so a slow GPU shows an estimate before the first
    /// full sample.
    pub(crate) fn eta(&self, now: Instant) -> Option<Duration> {
        let done = f64::from(self.progress.samples) + f64::from(self.progress.partial);
        if done <= 0.0 || self.progress.samples >= self.progress.target {
            return None;
        }
        let paused = self.paused_total
            + self
                .paused_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        let elapsed = now.saturating_duration_since(self.started).saturating_sub(paused);
        if elapsed.is_zero() {
            // Paused before any time was spent rendering: there is no pace to estimate from.
            return None;
        }
        Some(elapsed.mul_f64((f64::from(self.progress.target) - done) / done))
    }

    pub(crate) fn fraction_done(&self) -> f32 {
        (self.progress.samples as f32 / self.progress.target.max(1) as f32).clamp(0.0, 1.0)
    }
}

/// Radius of the lens opening in voxels for a focal length, an f-number and a focus distance in voxels.
///
/// Real lenses are far too small next to a voxel model to blur anything, so this is a scale-free stand-in: at 35 mm
/// the opening radius is an eighth of the focus distance divided by the f-number, so f/2.8 blurs a model of about a
/// third of that distance in depth by a visible amount, and f/16 hardly at all.
pub(crate) fn aperture_radius_voxels(lens_mm: f32, f_number: f32, focus_voxels: f32) -> f32 {
    lens_mm / 35.0 * focus_voxels / (8.0 * f_number.clamp(APERTURE_RANGE.0, APERTURE_RANGE.1))
}

/// A fraction of an image side as a pixel position.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is clamped to 0..=1 and rounded, so the result is at most `side`"
)]
fn fraction_to_pixels(fraction: f32, side: u32) -> u32 {
    (fraction.clamp(0.0, 1.0) * side as f32).round() as u32
}

/// Vertical field of view for a lens of `lens_mm` on the virtual sensor.
pub(crate) fn lens_to_fov_degrees(lens_mm: f32) -> f32 {
    2.0 * (SENSOR_HEIGHT_MM / 2.0 / lens_mm.clamp(LENS_RANGE_MM.0, LENS_RANGE_MM.1))
        .atan()
        .to_degrees()
}

/// `mm:ss` for a duration, `h:mm:ss` from one hour on.
pub(crate) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

/// The Camera tab's projection choice that matches `camera`: the isometric view is the orthographic one from the
/// classic angle.
pub(crate) fn projection_of(camera: &OrbitCamera) -> ViewProjection {
    match camera.projection {
        Projection::Perspective => ViewProjection::Perspective,
        Projection::Orthographic if camera.is_isometric() => ViewProjection::Isometric,
        Projection::Orthographic => ViewProjection::Orthographic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (RenderState, OrbitCamera) {
        let camera = OrbitCamera::default();
        (RenderState::new(&camera), camera)
    }

    #[test]
    fn default_exposure_is_minus_one_stop() {
        let (render, _) = state();
        assert!((render.exposure() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_35mm_lens_sees_about_38_degrees() {
        assert!((lens_to_fov_degrees(35.0) - 37.85).abs() < 0.1);
        assert!(lens_to_fov_degrees(14.0) > lens_to_fov_degrees(85.0));
    }

    #[test]
    fn naming_the_default_shot_keeps_the_queue_on_the_live_view() {
        let (mut render, mut camera) = state();
        render.rename_shot(0, "Hero");
        assert!(!render.shots_touched);
        camera.yaw = 2.0;
        let queued = render.queued_shots(&camera);
        assert_eq!(queued.len(), 1);
        assert!((queued[0].camera.yaw - 2.0).abs() < 1e-6, "the orbited view is queued");
        assert_eq!(queued[0].name.as_deref(), Some("Hero"));
    }

    #[test]
    fn shots_store_camera_and_size_and_restore_them() {
        let (mut render, mut camera) = state();
        assert!(render.add_shot(&camera));
        camera.yaw = 2.0;
        render.size = (1080, 1080);
        render.update_shot(1, &camera);
        render.select_shot(0, &mut camera);
        assert!((camera.yaw - OrbitCamera::default().yaw).abs() < 1e-6);
        assert_eq!(render.size, (1920, 1080), "shot 1 kept its own size");
        render.select_shot(1, &mut camera);
        assert_eq!(render.size, (1080, 1080));
        assert!((camera.yaw - 2.0).abs() < 1e-6);
        assert_eq!(render.active_shot, 1);
    }

    #[test]
    fn the_shot_list_stops_at_five() {
        let (mut render, camera) = state();
        while render.add_shot(&camera) {}
        assert_eq!(render.shots.len(), MAX_SHOTS);
    }

    #[test]
    fn region_fractions_become_ordered_pixels() {
        let (mut render, _) = state();
        render.region = Some([0.75, 0.5, 0.25, 0.0]);
        assert_eq!(render.pixel_region(render.size), Some([480, 0, 1440, 540]));
        assert_eq!(
            render.pixel_region((960, 540)),
            Some([240, 0, 720, 270]),
            "the region scales to the preview size"
        );
        render.region = Some([0.5, 0.5, 0.5, 0.9]);
        assert_eq!(render.pixel_region(render.size), None, "a zero-width region is ignored");
    }

    #[test]
    fn the_centre_pixel_looks_at_the_camera_target() {
        let (mut render, camera) = state();
        render.frame_rect = Some([10.0, 20.0, 640.0, 360.0]);
        let frame = render.trace_frame(&camera, 2.0, 0.1).unwrap();
        assert_eq!(frame.frame_rect, [20.0, 40.0, 1280.0, 720.0]);
        let inverse = glam::Mat4::from_cols_array_2d(&frame.inv_view_proj);
        let near = inverse.project_point3(glam::Vec3::new(0.0, 0.0, 0.0));
        let far = inverse.project_point3(glam::Vec3::new(0.0, 0.0, 1.0));
        let to_target = (camera.target - near).normalize();
        assert!((far - near).normalize().dot(to_target) > 0.9999);
    }

    #[test]
    fn the_preview_follows_the_display_and_never_the_output() {
        let (mut render, _) = state();
        // Shown at 1030 x 580 physical pixels, the preview traces about that, not 1920 x 1080.
        let preview = render.preview_size((1030.0, 579.4));
        assert!((i64::from(preview.0) - 1030).abs() <= 1 && (i64::from(preview.1) - 579).abs() <= 1);
        // A large display is capped so the trace stays affordable, at the output's aspect.
        let capped = render.preview_size((3840.0, 2160.0));
        assert!(capped.0 * capped.1 <= 1_350_000 && (capped.0 as f32 / capped.1 as f32 - 16.0 / 9.0).abs() < 0.02);
        // A small output is never enlarged for the preview.
        render.size = (320, 180);
        assert_eq!(render.preview_size((1030.0, 579.0)), (320, 180));
    }

    #[test]
    fn the_region_is_set_in_preview_pixels() {
        let (mut render, camera) = state();
        render.frame_rect = Some([0.0, 0.0, 800.0, 450.0]);
        render.region = Some([0.0, 0.0, 0.5, 1.0]);
        let frame = render.trace_frame(&camera, 1.0, 0.1).unwrap();
        assert_eq!(frame.size, (800, 450));
        assert_eq!(frame.region, Some([0, 0, 400, 450]));
    }

    #[test]
    fn eta_scales_with_the_remaining_samples() {
        let (mut render, _) = state();
        let start = Instant::now();
        render.update_progress(
            TraceProgress {
                samples: 0,
                target: 100,
                partial: 0.0,
                generation: 1,
            },
            start,
        );
        assert_eq!(render.eta(start), None);
        render.update_progress(
            TraceProgress {
                samples: 25,
                target: 100,
                partial: 0.0,
                generation: 1,
            },
            start,
        );
        let eta = render.eta(start + Duration::from_secs(10)).unwrap();
        assert_eq!(eta.as_secs(), 30);
        assert_eq!(render.status(), RenderStatus::Rendering);
        render.running = false;
        assert_eq!(render.status(), RenderStatus::Paused);
    }

    #[test]
    fn a_wider_aperture_blurs_more() {
        assert!(aperture_radius_voxels(35.0, 1.4, 50.0) > aperture_radius_voxels(35.0, 2.8, 50.0));
        assert!(aperture_radius_voxels(70.0, 2.8, 50.0) > aperture_radius_voxels(35.0, 2.8, 50.0));
        assert!(aperture_radius_voxels(35.0, 2.8, 100.0) > aperture_radius_voxels(35.0, 2.8, 50.0));
    }

    #[test]
    fn depth_of_field_needs_the_switch_and_converts_the_focus_to_voxels() {
        let (mut render, camera) = state();
        assert!(render.frame_for(&camera, 35.0, (64, 64), 0.1).lens.is_none());
        render.dof = true;
        render.focus_m = Some(4.2);
        let lens = render.frame_for(&camera, 35.0, (64, 64), 0.1).lens.unwrap();
        assert!((lens.focus_distance - 42.0).abs() < 1e-3);
        assert!((glam::Vec3::from(lens.eye) - camera.eye()).length() < 1e-4);
        render.focus_m = None;
        let auto = render.frame_for(&camera, 35.0, (64, 64), 0.1).lens.unwrap();
        assert!(
            (auto.focus_distance - camera.distance).abs() < 1e-3,
            "follows the orbit distance"
        );
    }

    #[test]
    fn a_turntable_makes_one_full_opaque_turn() {
        let (mut render, camera) = state();
        render.turntable_frames = 8;
        render.backdrop = Backdrop::Transparent;
        let frames = render.turntable_frames(&camera, 35.0, (64, 36), 0.1);
        assert_eq!(frames.len(), 8);
        assert!(frames.iter().all(|frame| !frame.transparent && frame.size == (64, 36)));
        assert_ne!(frames[0].inv_view_proj, frames[1].inv_view_proj);
        // A turn of 8 steps of 45 degrees: frame 0 and frame 4 look from opposite sides.
        let eye = |frame: &TraceFrame| {
            glam::Mat4::from_cols_array_2d(&frame.inv_view_proj).project_point3(glam::Vec3::new(0.0, 0.0, 0.0))
        };
        assert!(eye(&frames[0]).distance(eye(&frames[4])) > camera.distance);
    }

    #[test]
    fn durations_format_as_minutes_and_seconds_then_hours() {
        assert_eq!(format_duration(Duration::from_secs(75)), "01:15");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59:59");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(format_duration(Duration::from_secs(100_000)), "27:46:40");
    }

    #[test]
    fn a_sample_in_flight_gives_an_estimate_before_it_is_finished() {
        let (mut render, _) = state();
        let start = Instant::now();
        render.update_progress(
            TraceProgress {
                samples: 0,
                target: 10,
                partial: 0.5,
                generation: 1,
            },
            start,
        );
        // Half a sample in 5 s: 9.5 samples to go at 10 s per sample.
        assert_eq!(render.eta(start + Duration::from_secs(5)).unwrap().as_secs(), 95);
    }

    #[test]
    fn paused_time_does_not_count_towards_the_eta() {
        let (mut render, _) = state();
        let start = Instant::now();
        let progress = |samples| TraceProgress {
            samples,
            target: 100,
            partial: 0.0,
            generation: 1,
        };
        render.update_progress(progress(0), start);
        render.update_progress(progress(10), start + Duration::from_secs(10));
        // Paused for a minute between second 11 and 71.
        render.running = false;
        render.update_progress(progress(10), start + Duration::from_secs(11));
        render.running = true;
        render.update_progress(progress(10), start + Duration::from_secs(71));
        let eta = render.eta(start + Duration::from_secs(71)).unwrap();
        // 11 s of running time for 10 samples, 90 to go.
        assert_eq!(eta.as_secs(), 99, "the paused minute is left out");
    }

    #[test]
    fn shots_are_saved_views_that_the_live_view_does_not_overwrite() {
        let (mut render, mut camera) = state();
        let original = camera.clone();
        assert!(render.add_shot(&camera));
        camera.yaw += 1.0;
        assert!(render.shot_modified(1, &camera), "the active shot notices the change");
        assert!(!render.shot_modified(0, &camera));
        assert_eq!(render.shots[0].camera, original, "Shot 1 kept what it saved");
        render.select_shot(0, &mut camera);
        assert_eq!(camera, original);
        // Adding after moving stores the new view as a new shot and leaves the others alone.
        camera.yaw += 0.5;
        assert!(render.add_shot(&camera));
        assert_eq!(render.shots.len(), 3);
        assert_eq!(render.shots[0].camera, original);
        assert_ne!(render.shots[2].camera, render.shots[0].camera);
    }

    #[test]
    fn shots_can_be_updated_renamed_and_deleted() {
        let (mut render, mut camera) = state();
        render.add_shot(&camera);
        render.add_shot(&camera);
        camera.pitch = 0.2;
        render.update_shot(2, &camera);
        assert_eq!(render.shots[2].camera, camera);
        assert!(!render.shot_modified(2, &camera));
        render.rename_shot(1, "Front ");
        assert_eq!(render.shots[1].name.as_deref(), Some("Front "));
        render.rename_shot(1, "  ");
        assert_eq!(render.shots[1].name, None, "a blank name goes back to the default");
        render.delete_shot(1, &mut camera);
        assert_eq!(render.shots.len(), 2);
        assert_eq!(render.active_shot, 1, "the active shot keeps its place");
        render.delete_shot(1, &mut camera);
        render.delete_shot(0, &mut camera);
        assert_eq!(render.shots.len(), 1, "the last shot stays");
    }

    #[test]
    fn another_shot_can_be_overwritten_with_the_live_view() {
        let (mut render, mut camera) = state();
        render.add_shot(&camera);
        render.select_shot(0, &mut camera);
        camera.yaw += 0.7;
        render.update_shot(1, &camera);
        assert_eq!(render.shots[1].camera, camera, "Shot 2 takes the live view");
        assert_eq!(render.active_shot, 1, "and becomes the active Shot");
        assert_ne!(render.shots[0].camera, camera, "Shot 1 keeps what it saved");
    }

    #[test]
    fn a_shot_keeps_its_icon_when_others_are_deleted() {
        let (mut render, mut camera) = state();
        for _ in 0..4 {
            render.add_shot(&camera);
        }
        let icons: Vec<_> = render.shots.iter().map(|shot| shot.icon).collect();
        assert_eq!(icons, SHOT_ICONS, "every Shot starts with its own icon");
        render.delete_shot(3, &mut camera);
        assert_eq!(render.shots[3].icon, SHOT_ICONS[4], "the fifth Shot keeps its icon");
        render.add_shot(&camera);
        assert_eq!(render.shots[4].icon, SHOT_ICONS[3], "a new Shot takes the free icon");
    }

    #[test]
    fn the_queue_renders_the_screen_until_shots_are_managed() {
        let (mut render, mut camera) = state();
        camera.yaw += 1.0;
        assert_eq!(render.queued_shots(&camera).len(), 1);
        assert_eq!(render.queued_shots(&camera)[0].camera, camera);
        render.add_shot(&camera);
        camera.yaw += 1.0;
        let queued = render.queued_shots(&camera);
        assert_eq!(queued.len(), 2);
        assert_ne!(queued[1].camera, camera, "saved shots render as saved");
    }

    #[test]
    fn entering_render_mode_recognises_the_isometric_view() {
        let (mut render, mut camera) = state();
        camera.isometric();
        render.enter(&camera);
        assert_eq!(render.projection, ViewProjection::Isometric);
        camera.yaw = 1.0;
        render.enter(&camera);
        assert_eq!(render.projection, ViewProjection::Orthographic);
    }
}
