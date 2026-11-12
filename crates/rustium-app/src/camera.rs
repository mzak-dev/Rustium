use std::f32::consts::{FRAC_PI_2, TAU};

use glam::camera::rh::proj::directx;
use glam::camera::rh::view;
use glam::{Mat3, Mat4, Vec2, Vec3};
use rustium_core::{Face, Ray, Volume};

use crate::settings::OrbitStyle;

/// Perspective or orthographic projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Projection {
    Perspective,
    Orthographic,
}

/// A camera orbiting a target point, in voxel space (Y up).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OrbitCamera {
    pub(crate) target: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) distance: f32,
    pub(crate) projection: Projection,
    pub(crate) field_of_view_degrees: f32,
}

const ORBIT_SPEED: f32 = 0.008;
const MIN_DISTANCE: f32 = 2.0;
const MAX_DISTANCE: f32 = 4000.0;

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::new(0.5, 4.0, 0.5),
            yaw: 0.75,
            pitch: 0.55,
            distance: 48.0,
            projection: Projection::Perspective,
            field_of_view_degrees: 45.0,
        }
    }
}

impl OrbitCamera {
    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        )
    }

    pub(crate) fn eye(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    fn up(&self) -> Vec3 {
        // Past the poles (Free orbit) the camera is upside down.
        if self.pitch.cos() >= 0.0 { Vec3::Y } else { Vec3::NEG_Y }
    }

    pub(crate) fn view_proj(&self, aspect: f32) -> Mat4 {
        let view_matrix = view::look_at_mat4(self.eye(), self.target, self.up());
        let fov = self.field_of_view_degrees.to_radians();
        let projection = match self.projection {
            Projection::Perspective => directx::perspective(fov, aspect, 0.1, MAX_DISTANCE * 2.0),
            Projection::Orthographic => {
                let half_height = self.distance * (fov / 2.0).tan();
                let half_width = half_height * aspect;
                directx::orthographic(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    -MAX_DISTANCE,
                    MAX_DISTANCE * 2.0,
                )
            }
        };
        projection * view_matrix
    }

    /// The ray through `ndc` (normalized device coordinates, y up) for a viewport with `aspect`.
    ///
    /// Built from the camera's axes rather than by inverting the projection, which loses precision far from the
    /// target. The orthographic ray starts on the near plane, behind the eye, because that view also draws what lies
    /// behind the eye; the direction is linear in `ndc`, so blending two rays blends the pointer positions.
    pub(crate) fn ray(&self, ndc: Vec2, aspect: f32) -> Ray {
        let rotation = self.rotation();
        let (right, up, forward) = (rotation.x_axis, rotation.y_axis, -rotation.z_axis);
        let half_height = (self.field_of_view_degrees.to_radians() / 2.0).tan();
        let across = right * (ndc.x * half_height * aspect) + up * (ndc.y * half_height);
        match self.projection {
            Projection::Perspective => Ray {
                origin: self.eye(),
                direction: forward + across,
            },
            Projection::Orthographic => Ray {
                origin: self.eye() + across * self.distance - forward * MAX_DISTANCE,
                direction: forward,
            },
        }
    }

    /// How far a pointer ray must reach to cover everything the view draws.
    pub(crate) const RAY_REACH: f32 = MAX_DISTANCE * 3.0;

    pub(crate) fn orbit(&mut self, delta: Vec2, style: OrbitStyle) {
        self.yaw = (self.yaw - delta.x * ORBIT_SPEED).rem_euclid(TAU);
        self.pitch += delta.y * ORBIT_SPEED;
        match style {
            OrbitStyle::Turntable => self.pitch = self.pitch.clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01),
            OrbitStyle::Free => self.pitch = self.pitch.rem_euclid(TAU),
        }
    }

    /// Fly mode: turns the view in place (the eye stays where it is) by `look` points of pointer travel, then moves
    /// the eye by `step` voxels along the view's right, the world up and the view's forward.
    pub(crate) fn fly(&mut self, look: Vec2, step: Vec3) {
        let eye = self.eye();
        self.yaw = (self.yaw - look.x * ORBIT_SPEED).rem_euclid(TAU);
        self.pitch = (self.pitch + look.y * ORBIT_SPEED).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
        let rotation = self.rotation();
        let eye = eye + rotation.x_axis * step.x + Vec3::Y * step.y - rotation.z_axis * step.z;
        self.target = eye - self.direction() * self.distance;
    }

    /// Moves the target with the pointer; `delta` in points, `viewport_height` in points.
    pub(crate) fn pan(&mut self, delta: Vec2, viewport_height: f32) {
        let units_per_point =
            2.0 * self.distance * (self.field_of_view_degrees.to_radians() / 2.0).tan() / viewport_height.max(1.0);
        let forward = -self.direction();
        let right = forward.cross(self.up()).normalize_or_zero();
        let up = right.cross(forward);
        self.target += (-right * delta.x + up * delta.y) * units_per_point;
    }

    /// Zooms by `steps` wheel notches towards `focus` (a point under the cursor).
    pub(crate) fn zoom_towards(&mut self, steps: f32, focus: Vec3) {
        let factor = 0.85_f32.powf(steps);
        let new_distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        let applied = new_distance / self.distance;
        self.target += (focus - self.target) * (1.0 - applied);
        self.distance = new_distance;
    }

    /// Orbits like [`OrbitCamera::orbit`], but around `pivot` instead of the target: the camera turns about that point,
    /// so it stays exactly where it is on screen and the view never jumps when the pivot is not the target (Around
    /// Cursor, or Around Model after a pan).
    pub(crate) fn orbit_around(&mut self, delta: Vec2, style: OrbitStyle, pivot: Vec3) {
        let before = self.rotation();
        let target = self.target;
        self.orbit(delta, style);
        let turn = self.rotation() * before.transpose();
        self.target = pivot + turn * (target - pivot);
    }

    /// The camera's orientation: columns are right, up and the direction from the target to the eye.
    fn rotation(&self) -> Mat3 {
        let back = self.direction();
        let right = self.up().cross(back).normalize_or_zero();
        Mat3::from_cols(right, back.cross(right), back)
    }

    /// Whether this is the isometric view: orthographic, turned 45° and looking down the cube diagonal.
    pub(crate) fn is_isometric(&self) -> bool {
        let pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
        self.projection == Projection::Orthographic
            && (self.yaw - std::f32::consts::FRAC_PI_4).abs() < 1e-3
            && (self.pitch - pitch).abs() < 1e-3
    }

    /// Orthographic view from the classic isometric angle (45° turn, 35.264° down).
    pub(crate) fn isometric(&mut self) {
        self.projection = Projection::Orthographic;
        self.yaw = std::f32::consts::FRAC_PI_4;
        self.pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
    }

    /// Looks straight at `face` of the model (TOP, FRONT, RIGHT, …).
    pub(crate) fn snap(&mut self, face: Face) {
        let (yaw, pitch) = match face {
            Face::PosY => (self.yaw, FRAC_PI_2 - 0.01),
            Face::NegY => (self.yaw, -FRAC_PI_2 + 0.01),
            Face::PosZ => (0.0, 0.0),
            Face::NegZ => (std::f32::consts::PI, 0.0),
            Face::PosX => (FRAC_PI_2, 0.0),
            Face::NegX => (-FRAC_PI_2, 0.0),
        };
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// Centres the camera on `volume`, or on the origin for an empty Scene.
    pub(crate) fn frame(&mut self, volume: Option<Volume>) {
        let Some(volume) = volume else {
            *self = Self {
                field_of_view_degrees: self.field_of_view_degrees,
                ..Self::default()
            };
            return;
        };
        let min = volume.min.as_vec3();
        let max = (volume.max + 1).as_vec3();
        self.target = (min + max) / 2.0;
        let radius = (max - min).length() / 2.0;
        self.distance =
            (radius / (self.field_of_view_degrees.to_radians() / 2.0).sin() * 1.1).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_ray_points_at_the_target() {
        let camera = OrbitCamera::default();
        let ray = camera.ray(Vec2::ZERO, 16.0 / 9.0);
        let to_target = (camera.target - ray.origin).normalize();
        assert!(ray.direction.normalize().dot(to_target) > 0.9999);
    }

    #[test]
    fn rays_go_through_what_the_projection_draws_there() {
        for projection in [Projection::Perspective, Projection::Orthographic] {
            let mut camera = OrbitCamera {
                projection,
                distance: 900.0,
                ..OrbitCamera::default()
            };
            camera.orbit(Vec2::new(70.0, -30.0), OrbitStyle::Turntable);
            let ndc = Vec2::new(0.6, -0.35);
            let ray = camera.ray(ndc, 16.0 / 9.0);
            for along in [300.0, 4500.0, 7000.0] {
                let point = ray.origin + ray.direction.normalize() * along;
                let landed = screen(&camera, point);
                assert!((landed - ndc).length() < 1e-3, "{projection:?} at {along}: {landed}");
            }
        }
    }

    /// Where `point` lands in normalized device coordinates.
    fn screen(camera: &OrbitCamera, point: Vec3) -> Vec2 {
        camera.view_proj(16.0 / 9.0).project_point3(point).truncate()
    }

    #[test]
    fn orbiting_around_a_pivot_keeps_it_where_it_is_on_screen() {
        for pivot in [Vec3::new(10.0, 2.0, -6.0), Vec3::new(-3.0, 9.0, 4.0)] {
            let mut camera = OrbitCamera::default();
            camera.pan(Vec2::new(40.0, -25.0), 600.0);
            let before = screen(&camera, pivot);
            camera.orbit_around(Vec2::new(60.0, 35.0), OrbitStyle::Turntable, pivot);
            camera.orbit_around(Vec2::new(-20.0, 10.0), OrbitStyle::Turntable, pivot);
            assert!(
                (screen(&camera, pivot) - before).length() < 1e-3,
                "pivot {pivot} stayed put"
            );
        }
    }

    #[test]
    fn orbiting_around_the_target_is_a_plain_orbit() {
        let mut plain = OrbitCamera::default();
        let mut around = plain.clone();
        plain.orbit(Vec2::new(30.0, -12.0), OrbitStyle::Turntable);
        around.orbit_around(Vec2::new(30.0, -12.0), OrbitStyle::Turntable, around.target);
        assert!((plain.eye() - around.eye()).length() < 1e-3);
        assert!((plain.target - around.target).length() < 1e-3);
    }

    #[test]
    fn the_first_orbit_step_barely_moves_an_off_centre_pivot() {
        // The view must not jump when a drag starts: a one-point nudge moves the picture by about a point's worth.
        let mut camera = OrbitCamera::default();
        let pivot = Vec3::new(8.0, 1.0, 3.0);
        let eye = camera.eye();
        camera.orbit_around(Vec2::new(1.0, 0.0), OrbitStyle::Turntable, pivot);
        assert!(
            (camera.eye() - eye).length() < 1.0,
            "eye moved {}",
            (camera.eye() - eye).length()
        );
    }

    #[test]
    fn orthographic_matches_perspective_at_the_target_plane() {
        // A point beside the target, at the target's depth, lands in the same place with either projection, so
        // switching projection keeps the framing of what the camera looks at.
        let mut camera = OrbitCamera::default();
        let right = (camera.target - camera.eye()).normalize().cross(Vec3::Y).normalize();
        let beside = camera.target + right * 6.0;
        let perspective = screen(&camera, beside);
        camera.projection = Projection::Orthographic;
        let orthographic = screen(&camera, beside);
        assert!(
            (perspective - orthographic).length() < 1e-3,
            "{perspective} against {orthographic}"
        );
    }

    #[test]
    fn the_isometric_view_is_recognised() {
        let mut camera = OrbitCamera::default();
        assert!(!camera.is_isometric());
        camera.isometric();
        assert!(camera.is_isometric());
        camera.projection = Projection::Perspective;
        assert!(!camera.is_isometric());
    }

    #[test]
    fn isometric_looks_down_a_cube_diagonal() {
        let mut camera = OrbitCamera::default();
        camera.isometric();
        let direction = (camera.eye() - camera.target).normalize();
        assert!((direction.x - direction.z).abs() < 1e-5 && (direction.x - direction.y).abs() < 1e-5);
    }

    #[test]
    fn flying_turns_in_place_and_moves_forward() {
        let mut camera = OrbitCamera::default();
        let eye = camera.eye();
        camera.fly(Vec2::new(40.0, -10.0), Vec3::ZERO);
        assert!((camera.eye() - eye).length() < 1e-3, "turning keeps the eye");
        let forward = (camera.target - camera.eye()).normalize();
        let before = camera.eye();
        camera.fly(Vec2::ZERO, Vec3::new(0.0, 0.0, 5.0));
        assert!((camera.eye() - (before + forward * 5.0)).length() < 1e-3);
    }

    #[test]
    fn turntable_orbit_never_flips_over_the_pole() {
        let mut camera = OrbitCamera::default();
        camera.orbit(Vec2::new(0.0, 10_000.0), OrbitStyle::Turntable);
        assert!(camera.pitch < FRAC_PI_2);
    }

    #[test]
    fn zoom_keeps_the_focus_point_fixed_on_the_view_axis() {
        let mut camera = OrbitCamera::default();
        let focus = camera.target;
        camera.zoom_towards(2.0, focus);
        assert!((camera.target - focus).length() < 1e-4);
        assert!(camera.distance < 48.0);
    }
}
