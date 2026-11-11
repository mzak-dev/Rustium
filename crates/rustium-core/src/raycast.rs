use glam::{IVec3, Vec3};

use crate::face::Face;
use crate::shape::Shape;

/// A ray in voxel space (one unit = one voxel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Where the ray starts.
    pub origin: Vec3,
    /// Direction of travel; does not need to be normalized.
    pub direction: Vec3,
}

/// What a ray hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    /// A filled voxel.
    Voxel,
    /// The ground plane at y = 0; `voxel` is the empty cell just below it.
    Ground,
}

/// Where a ray hit the Scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayHit {
    /// The voxel that was hit (for the ground: the cell below the hit point).
    pub voxel: IVec3,
    /// Face of `voxel` the ray entered through.
    pub face: Face,
    /// Whether a voxel or the ground was hit.
    pub target: HitTarget,
}

/// Finds the first voxel `ray` hits within `max_distance`, falling back to the ground plane y = 0.
pub fn pick(shape: &Shape, ray: Ray, max_distance: f32) -> Option<RayHit> {
    let direction = ray.direction.normalize_or_zero();
    if direction == Vec3::ZERO {
        return None;
    }
    let ray = Ray {
        origin: ray.origin,
        direction,
    };
    let voxel_hit = raycast_voxels(shape, ray, max_distance);
    let ground_hit = raycast_ground(ray, max_distance);
    match (voxel_hit, ground_hit) {
        (Some((voxel, voxel_distance)), Some((ground, ground_distance))) => {
            Some(if voxel_distance <= ground_distance {
                voxel
            } else {
                ground
            })
        }
        (Some((hit, _)), None) | (None, Some((hit, _))) => Some(hit),
        (None, None) => None,
    }
}

pub(crate) fn raycast_ground(ray: Ray, max_distance: f32) -> Option<(RayHit, f32)> {
    if ray.direction.y >= 0.0 || ray.origin.y <= 0.0 {
        return None;
    }
    let distance = ray.origin.y / -ray.direction.y;
    if distance > max_distance {
        return None;
    }
    let point = ray.origin + ray.direction * distance;
    let cell = point.floor().as_ivec3();
    let voxel = IVec3::new(cell.x, -1, cell.z);
    Some((
        RayHit {
            voxel,
            face: Face::PosY,
            target: HitTarget::Ground,
        },
        distance,
    ))
}

/// Amanatides–Woo voxel traversal, clipped to the Volume so empty space costs nothing.
// HOT PATH: runs on every pointer move.
pub(crate) fn raycast_voxels(shape: &Shape, ray: Ray, max_distance: f32) -> Option<(RayHit, f32)> {
    let volume = shape.volume()?;
    let low = volume.min.as_vec3();
    let high = (volume.max + IVec3::ONE).as_vec3();
    let inverse = ray.direction.recip();
    let t0 = (low - ray.origin) * inverse;
    let t1 = (high - ray.origin) * inverse;
    let t_near = t0.min(t1);
    let t_far = t0.max(t1);
    let enter = t_near.max_element();
    let exit = t_far.min_element().min(max_distance);
    let start = enter.max(0.0);
    if start > exit {
        return None;
    }

    let start_point = ray.origin + ray.direction * start;
    let mut voxel = start_point.floor().as_ivec3().clamp(volume.min, volume.max);
    let step = IVec3::new(sign(ray.direction.x), sign(ray.direction.y), sign(ray.direction.z));
    let delta = inverse.abs();
    let next_boundary = |axis: usize, voxel: IVec3| -> f32 {
        let boundary = if step[axis] > 0 { voxel[axis] + 1 } else { voxel[axis] };
        if step[axis] == 0 {
            f32::INFINITY
        } else {
            (boundary as f32 - ray.origin[axis]) * inverse[axis]
        }
    };
    let mut t_max = Vec3::new(
        next_boundary(0, voxel),
        next_boundary(1, voxel),
        next_boundary(2, voxel),
    );
    let entry_axis = (0..3).max_by(|a, b| t_near[*a].total_cmp(&t_near[*b])).unwrap_or(1);
    let mut face = Face::entered_by_step(entry_axis, step[entry_axis]);
    let mut distance = start;

    loop {
        if shape.get(voxel).is_some() {
            return Some((
                RayHit {
                    voxel,
                    face,
                    target: HitTarget::Voxel,
                },
                distance,
            ));
        }
        let axis = if t_max.x < t_max.y && t_max.x < t_max.z {
            0
        } else if t_max.y < t_max.z {
            1
        } else {
            2
        };
        distance = t_max[axis];
        if distance > exit {
            return None;
        }
        voxel[axis] += step[axis];
        t_max[axis] += delta[axis];
        face = Face::entered_by_step(axis, step[axis]);
    }
}

fn sign(value: f32) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CHUNK_SIZE;
    use crate::palette::ColorId;

    fn shape_with(positions: &[IVec3]) -> Shape {
        let mut shape = Shape::new();
        for position in positions {
            shape.set(*position, ColorId::new(1));
        }
        shape
    }

    #[test]
    fn ray_from_above_hits_the_top_face() {
        let shape = shape_with(&[IVec3::new(3, 0, 3)]);
        let ray = Ray {
            origin: Vec3::new(3.5, 20.0, 3.5),
            direction: Vec3::NEG_Y,
        };
        let hit = pick(&shape, ray, 100.0).unwrap();
        assert_eq!(
            (hit.voxel, hit.face, hit.target),
            (IVec3::new(3, 0, 3), Face::PosY, HitTarget::Voxel)
        );
    }

    #[test]
    fn ray_crossing_a_chunk_border_hits_the_far_voxel() {
        let far = IVec3::new(CHUNK_SIZE + 5, 0, 0);
        let shape = shape_with(&[IVec3::ZERO, far]);
        let ray = Ray {
            origin: Vec3::new(1.5, 0.5, 0.5),
            direction: Vec3::X,
        };
        let hit = pick(&shape, ray, 100.0).unwrap();
        assert_eq!((hit.voxel, hit.face), (far, Face::NegX));
    }

    #[test]
    fn empty_shape_falls_back_to_the_ground() {
        let shape = Shape::new();
        let ray = Ray {
            origin: Vec3::new(2.5, 5.0, -1.5),
            direction: Vec3::NEG_Y,
        };
        let hit = pick(&shape, ray, 100.0).unwrap();
        assert_eq!((hit.voxel, hit.target), (IVec3::new(2, -1, -2), HitTarget::Ground));
    }

    #[test]
    fn ray_pointing_away_hits_nothing() {
        let shape = shape_with(&[IVec3::ZERO]);
        let ray = Ray {
            origin: Vec3::new(0.5, 5.0, 0.5),
            direction: Vec3::Y,
        };
        assert_eq!(pick(&shape, ray, 100.0), None);
    }
}
