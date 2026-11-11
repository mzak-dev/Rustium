//! State and pure logic of Render mode: Shots, lens, light and output settings, and the [`TraceFrame`] the
//! renderer needs. The interface for it lives in `hud/render_*.rs`.

use std::time::{Duration, Instant};

use rustium_render::{Ground, Sky, TraceFrame, TraceProgress};

use crate::camera::{OrbitCamera, Projection};
use crate::icons::Icon;

/// Most Shots a Scene holds.
pub(crate) const MAX_SHOTS: usize = 5;
/// Height of the virtual sensor; Lens (mm) becomes a vertical field of view through it.
const SENSOR_HEIGHT_MM: f32 = 24.0;
/// Default exposure compensation: -1 EV puts the studio backdrop near a light grey instead of white.
const DEFAULT_EXPOSURE_EV: f32 = -1.0;
/// Range of the Lens field.
pub(crate) const LENS_RANGE_MM: (f32, f32) = (8.0, 300.0);
/// Range of an output side in pixels.
pub(crate) const SIDE_RANGE: (u32, u32) = (16, 8192);

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

    /// Focus picking belongs to depth of field, which arrives with the effects milestone.
    pub(crate) fn is_available(self) -> bool {
        self != Self::PickFocus
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
}

/// What the render is doing, for the status readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderStatus {
    Rendering,
    Paused,
    Done,
}

/// Everything Render mode remembers.
#[derive(Debug)]
pub(crate) struct RenderState {
    pub(crate) tool: RenderTool,
    pub(crate) tab: RenderTab,
    pub(crate) around: OrbitAround,
    pub(crate) samples: u32,
    pub(crate) bounces: u32,
    pub(crate) lens_mm: f32,
    pub(crate) projection: ViewProjection,
    pub(crate) sun_turn_degrees: f32,
    pub(crate) sun_height_degrees: f32,
    pub(crate) sky: Sky,
    pub(crate) ground: Ground,
    pub(crate) size: (u32, u32),
    pub(crate) format: ImageFormat,
    pub(crate) backdrop: Backdrop,
    pub(crate) shots: Vec<Shot>,
    pub(crate) active_shot: usize,
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
    /// Material graph selected in the temporary Materials preview section.
    pub(crate) preview_graph: usize,
    started: Instant,
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
            }],
            active_shot: 0,
            region: None,
            region_anchor: None,
            running: true,
            guides: true,
            exposure_ev: DEFAULT_EXPOSURE_EV,
            frame_rect: None,
            progress: TraceProgress {
                samples: 0,
                target: 1,
                generation: 0,
            },
            message: None,
            preview_graph: 0,
            started: Instant::now(),
            generation: 0,
        }
    }

    /// Called when Render mode opens: the live view becomes the active Shot.
    pub(crate) fn enter(&mut self, camera: &OrbitCamera) {
        self.projection = if camera.projection == Projection::Perspective {
            ViewProjection::Perspective
        } else {
            ViewProjection::Orthographic
        };
        self.sync_active_shot(camera);
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

    /// Keeps the active Shot equal to the live view.
    pub(crate) fn sync_active_shot(&mut self, camera: &OrbitCamera) {
        let (lens_mm, size) = (self.lens_mm, self.size);
        if let Some(shot) = self.shots.get_mut(self.active_shot) {
            shot.camera.clone_from(camera);
            shot.lens_mm = lens_mm;
            shot.size = size;
        }
    }

    /// Adds a Shot from the current view and makes it active; `false` when all [`MAX_SHOTS`] are used.
    pub(crate) fn add_shot(&mut self, camera: &OrbitCamera) -> bool {
        if self.shots.len() >= MAX_SHOTS {
            return false;
        }
        self.sync_active_shot(camera);
        self.shots.push(Shot {
            camera: camera.clone(),
            lens_mm: self.lens_mm,
            size: self.size,
        });
        self.active_shot = self.shots.len() - 1;
        true
    }

    /// Makes Shot `index` the live view.
    pub(crate) fn select_shot(&mut self, index: usize, camera: &mut OrbitCamera) {
        self.sync_active_shot(camera);
        let Some(shot) = self.shots.get(index) else {
            return;
        };
        camera.clone_from(&shot.camera);
        self.lens_mm = shot.lens_mm;
        self.size = shot.size;
        self.active_shot = index;
        self.projection = if camera.projection == Projection::Perspective {
            ViewProjection::Perspective
        } else {
            ViewProjection::Orthographic
        };
    }

    /// The pixel rectangle of the render region, if one is set and not degenerate.
    pub(crate) fn pixel_region(&self) -> Option<[u32; 4]> {
        let [x0, y0, x1, y1] = self.region?;
        let region = [
            fraction_to_pixels(x0.min(x1), self.size.0),
            fraction_to_pixels(y0.min(y1), self.size.1),
            fraction_to_pixels(x0.max(x1), self.size.0),
            fraction_to_pixels(y0.max(y1), self.size.1),
        ];
        (region[2] > region[0] && region[3] > region[1]).then_some(region)
    }

    /// The renderer's input for this frame; `scale` converts HUD points to physical pixels.
    pub(crate) fn trace_frame(&self, camera: &OrbitCamera, scale: f32) -> Option<TraceFrame> {
        let [x, y, width, height] = self.frame_rect?;
        let view_proj = self.render_camera(camera).view_proj(self.aspect());
        Some(TraceFrame {
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            size: self.size,
            samples: self.samples,
            bounces: self.bounces,
            sun_turn_degrees: self.sun_turn_degrees,
            sun_height_degrees: self.sun_height_degrees,
            sky: self.sky,
            ground: self.ground,
            transparent: self.backdrop == Backdrop::Transparent,
            region: self.pixel_region(),
            running: self.running,
            frame_rect: [x * scale, y * scale, width * scale, height * scale],
            exposure: self.exposure(),
        })
    }

    /// Records the renderer's progress; a restart resets the clock the ETA runs on.
    pub(crate) fn update_progress(&mut self, progress: TraceProgress, now: Instant) {
        if progress.generation != self.generation {
            self.generation = progress.generation;
            self.started = now;
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

    /// Whether the renderer still wants frames.
    pub(crate) fn is_accumulating(&self) -> bool {
        self.running && self.progress.samples < self.progress.target
    }

    /// Time left at the pace so far, or `None` before the first sample or when finished.
    pub(crate) fn eta(&self, now: Instant) -> Option<Duration> {
        let done = self.progress.samples;
        if done == 0 || done >= self.progress.target {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.started);
        Some(elapsed.mul_f64(f64::from(self.progress.target - done) / f64::from(done)))
    }

    pub(crate) fn fraction_done(&self) -> f32 {
        (self.progress.samples as f32 / self.progress.target.max(1) as f32).clamp(0.0, 1.0)
    }
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

/// `mm:ss` for a duration, capped at 99:59.
pub(crate) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs().min(99 * 60 + 59);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
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
    fn shots_store_camera_and_size_and_restore_them() {
        let (mut render, mut camera) = state();
        assert!(render.add_shot(&camera));
        camera.yaw = 2.0;
        render.size = (1080, 1080);
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
    fn the_live_view_updates_the_active_shot_before_switching() {
        let (mut render, mut camera) = state();
        render.add_shot(&camera);
        camera.pitch = 1.0;
        render.select_shot(0, &mut camera);
        render.select_shot(1, &mut camera);
        assert!(
            (camera.pitch - 1.0).abs() < 1e-6,
            "shot 2 remembered the edit made while it was active"
        );
    }

    #[test]
    fn region_fractions_become_ordered_pixels() {
        let (mut render, _) = state();
        render.region = Some([0.75, 0.5, 0.25, 0.0]);
        assert_eq!(render.pixel_region(), Some([480, 0, 1440, 540]));
        render.region = Some([0.5, 0.5, 0.5, 0.9]);
        assert_eq!(render.pixel_region(), None, "a zero-width region is ignored");
    }

    #[test]
    fn the_centre_pixel_looks_at_the_camera_target() {
        let (mut render, camera) = state();
        render.frame_rect = Some([10.0, 20.0, 640.0, 360.0]);
        let frame = render.trace_frame(&camera, 2.0).unwrap();
        assert_eq!(frame.frame_rect, [20.0, 40.0, 1280.0, 720.0]);
        let inverse = glam::Mat4::from_cols_array_2d(&frame.inv_view_proj);
        let near = inverse.project_point3(glam::Vec3::new(0.0, 0.0, 0.0));
        let far = inverse.project_point3(glam::Vec3::new(0.0, 0.0, 1.0));
        let to_target = (camera.target - near).normalize();
        assert!((far - near).normalize().dot(to_target) > 0.9999);
    }

    #[test]
    fn eta_scales_with_the_remaining_samples() {
        let (mut render, _) = state();
        let start = Instant::now();
        render.update_progress(
            TraceProgress {
                samples: 0,
                target: 100,
                generation: 1,
            },
            start,
        );
        assert_eq!(render.eta(start), None);
        render.update_progress(
            TraceProgress {
                samples: 25,
                target: 100,
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
    fn durations_format_as_minutes_and_seconds() {
        assert_eq!(format_duration(Duration::from_secs(75)), "01:15");
        assert_eq!(format_duration(Duration::from_secs(100_000)), "99:59");
    }
}
