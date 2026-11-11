use std::f32::consts::{FRAC_PI_2, TAU};

use glam::camera::rh::proj::directx;
use glam::camera::rh::view;
use glam::{Mat4, Vec2, Vec3};
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
    pub(crate) fn ray(&self, ndc: Vec2, aspect: f32) -> Ray {
        let inverse = self.view_proj(aspect).inverse();
        let near = inverse.project_point3(ndc.extend(0.0));
        let far = inverse.project_point3(ndc.extend(1.0));
        Ray {
            origin: near,
            direction: far - near,
        }
    }

    pub(crate) fn orbit(&mut self, delta: Vec2, style: OrbitStyle) {
        self.yaw = (self.yaw - delta.x * ORBIT_SPEED).rem_euclid(TAU);
        self.pitch += delta.y * ORBIT_SPEED;
        match style {
            OrbitStyle::Turntable => self.pitch = self.pitch.clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01),
            OrbitStyle::Free => self.pitch = self.pitch.rem_euclid(TAU),
        }
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

    /// Makes `point` the orbit centre without moving the eye.
    pub(crate) fn retarget(&mut self, point: Vec3) {
        let offset = self.eye() - point;
        let distance = offset.length();
        if distance < f32::EPSILON {
            return;
        }
        let direction = offset / distance;
        self.target = point;
        self.distance = distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.pitch = direction.y.clamp(-1.0, 1.0).asin();
        self.yaw = direction.x.atan2(direction.z).rem_euclid(TAU);
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
    fn retarget_keeps_the_eye_where_it_was() {
        let mut camera = OrbitCamera::default();
        let eye = camera.eye();
        camera.retarget(Vec3::new(10.0, 2.0, -6.0));
        assert!((camera.eye() - eye).length() < 1e-3);
        assert!((camera.target - Vec3::new(10.0, 2.0, -6.0)).length() < 1e-6);
    }

    #[test]
    fn isometric_looks_down_a_cube_diagonal() {
        let mut camera = OrbitCamera::default();
        camera.isometric();
        let direction = (camera.eye() - camera.target).normalize();
        assert!((direction.x - direction.z).abs() < 1e-5 && (direction.x - direction.y).abs() < 1e-5);
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
