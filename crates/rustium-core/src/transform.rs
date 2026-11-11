use glam::{IVec3, Vec3};

use crate::face::Face;
use crate::raycast::RayHit;
use crate::shape::Volume;

/// A quarter-turn rotation, a mirror, or both: a signed permutation of the three axes.
///
/// Stored as the images of the X, Y and Z unit vectors, so every Voxel of a rotated Model lands exactly on the
/// Scene grid (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Orientation {
    columns: [IVec3; 3],
}

impl Orientation {
    /// Leaves everything where it is.
    pub const IDENTITY: Self = Self {
        columns: [IVec3::X, IVec3::Y, IVec3::Z],
    };

    /// `quarter_turns` right-handed quarter turns about `axis` (0 = X, 1 = Y, 2 = Z); negative values turn back.
    #[must_use]
    pub fn rotation(axis: usize, quarter_turns: i32) -> Self {
        let first = (axis + 1) % 3;
        let second = (axis + 2) % 3;
        let mut quarter = Self::IDENTITY;
        quarter.columns[first] = unit(second, 1);
        quarter.columns[second] = unit(first, -1);
        let mut result = Self::IDENTITY;
        for _ in 0..quarter_turns.rem_euclid(4) {
            result = quarter.compose(result);
        }
        result
    }

    /// Flips `axis` (0 = X, 1 = Y, 2 = Z).
    #[must_use]
    pub fn mirror(axis: usize) -> Self {
        let mut result = Self::IDENTITY;
        if let Some(column) = result.columns.get_mut(axis) {
            *column = -*column;
        }
        result
    }

    /// The orientation that applies `inner` first and `self` second.
    #[must_use]
    pub fn compose(self, inner: Self) -> Self {
        Self {
            columns: inner.columns.map(|column| self.apply(column)),
        }
    }

    /// The orientation that undoes `self`.
    #[must_use]
    pub fn inverse(self) -> Self {
        let [x, y, z] = self.columns;
        Self {
            columns: [
                IVec3::new(x.x, y.x, z.x),
                IVec3::new(x.y, y.y, z.y),
                IVec3::new(x.z, y.z, z.z),
            ],
        }
    }

    /// Whether the orientation contains an odd number of mirrors, which flips the winding of faces.
    pub fn is_mirrored(self) -> bool {
        let [x, y, z] = self.columns;
        x.dot(y.cross(z)) < 0
    }

    /// Turns the integer vector `vector` (a direction or an offset).
    pub fn apply(self, vector: IVec3) -> IVec3 {
        self.columns[0] * vector.x + self.columns[1] * vector.y + self.columns[2] * vector.z
    }

    /// Turns the vector `vector` (a direction or a point).
    pub fn apply_float(self, vector: Vec3) -> Vec3 {
        self.columns[0].as_vec3() * vector.x
            + self.columns[1].as_vec3() * vector.y
            + self.columns[2].as_vec3() * vector.z
    }

    /// The images of the X, Y and Z unit vectors.
    pub fn columns(self) -> [IVec3; 3] {
        self.columns
    }

    /// Extra shift that keeps a Voxel's lower corner on its cell: -1 on every axis that flips sign.
    fn cell_shift(self) -> IVec3 {
        let [x, y, z] = self.columns;
        let sum = x + y + z;
        -IVec3::new(i32::from(sum.x < 0), i32::from(sum.y < 0), i32::from(sum.z < 0))
    }
}

impl Default for Orientation {
    fn default() -> Self {
        Self::IDENTITY
    }
}

fn unit(axis: usize, sign: i32) -> IVec3 {
    let mut vector = IVec3::ZERO;
    vector[axis] = sign;
    vector
}

/// Where a Node sits relative to its parent: an [`Orientation`] followed by a whole-voxel offset.
///
/// Points move to `orientation(point) + offset`; Voxel cells follow, so a Voxel always lands exactly on a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Transform {
    /// Quarter-turn rotation and mirror, applied first.
    pub orientation: Orientation,
    /// Whole-voxel translation, applied second.
    pub offset: IVec3,
}

impl Transform {
    /// Leaves everything where it is.
    pub const IDENTITY: Self = Self {
        orientation: Orientation::IDENTITY,
        offset: IVec3::ZERO,
    };

    /// A pure translation by `offset` voxels.
    pub fn translation(offset: IVec3) -> Self {
        Self {
            orientation: Orientation::IDENTITY,
            offset,
        }
    }

    /// The Transform that applies `inner` first and `self` second, as a parent's Transform applies a child's.
    #[must_use]
    pub fn compose(self, inner: Self) -> Self {
        Self {
            orientation: self.orientation.compose(inner.orientation),
            offset: self.offset + self.orientation.apply(inner.offset),
        }
    }

    /// The Transform that undoes `self`.
    #[must_use]
    pub fn inverse(self) -> Self {
        let orientation = self.orientation.inverse();
        Self {
            orientation,
            offset: -orientation.apply(self.offset),
        }
    }

    /// The cell that the Voxel at `cell` moves to.
    pub fn cell(self, cell: IVec3) -> IVec3 {
        self.orientation.apply(cell) + self.orientation.cell_shift() + self.offset
    }

    /// The cell whose Voxel moves to `cell`; the exact inverse of [`cell`](Self::cell).
    pub fn inverse_cell(self, cell: IVec3) -> IVec3 {
        self.inverse().cell(cell)
    }

    /// Where the point `point` moves to.
    pub fn point(self, point: Vec3) -> Vec3 {
        self.orientation.apply_float(point) + self.offset.as_vec3()
    }

    /// The box `volume` after the Transform.
    pub fn volume(self, volume: Volume) -> Volume {
        let a = self.cell(volume.min);
        let b = self.cell(volume.max);
        Volume {
            min: a.min(b),
            max: a.max(b),
        }
    }

    /// The face that `face` turns into.
    pub fn face(self, face: Face) -> Face {
        // A signed permutation maps unit axis vectors to unit axis vectors, so the lookup always succeeds.
        Face::from_normal(self.orientation.apply(face.normal())).unwrap_or(face)
    }

    /// A ray hit moved through the Transform.
    pub fn hit(self, hit: RayHit) -> RayHit {
        RayHit {
            voxel: self.cell(hit.voxel),
            face: self.face(hit.face),
            target: hit.target,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn orientation_strategy() -> impl Strategy<Value = Orientation> {
        (0..3_usize, 0..4_i32, 0..3_usize, 0..4_i32, 0..4_usize).prop_map(|(a, ta, b, tb, mirror)| {
            let mut result = Orientation::rotation(a, ta).compose(Orientation::rotation(b, tb));
            if mirror < 3 {
                result = result.compose(Orientation::mirror(mirror));
            }
            result
        })
    }

    fn transform_strategy() -> impl Strategy<Value = Transform> {
        (orientation_strategy(), -50..50_i32, -50..50_i32, -50..50_i32).prop_map(|(orientation, x, y, z)| Transform {
            orientation,
            offset: IVec3::new(x, y, z),
        })
    }

    fn cell_strategy() -> impl Strategy<Value = IVec3> {
        (-40..40_i32, -40..40_i32, -40..40_i32).prop_map(|(x, y, z)| IVec3::new(x, y, z))
    }

    #[test]
    fn quarter_turn_about_y_sends_x_to_minus_z() {
        assert_eq!(Orientation::rotation(1, 1).apply(IVec3::X), IVec3::NEG_Z);
        assert_eq!(Orientation::rotation(1, 4), Orientation::IDENTITY);
    }

    #[test]
    fn mirroring_x_moves_cell_zero_to_cell_minus_one() {
        let transform = Transform {
            orientation: Orientation::mirror(0),
            offset: IVec3::ZERO,
        };
        assert_eq!(transform.cell(IVec3::ZERO), IVec3::NEG_X);
        assert!(transform.orientation.is_mirrored());
    }

    proptest! {
        #[test]
        fn inverse_cell_undoes_cell(transform in transform_strategy(), cell in cell_strategy()) {
            prop_assert_eq!(transform.inverse_cell(transform.cell(cell)), cell);
        }

        #[test]
        fn composing_equals_applying_one_after_the_other(
            outer in transform_strategy(),
            inner in transform_strategy(),
            cell in cell_strategy(),
        ) {
            prop_assert_eq!(outer.compose(inner).cell(cell), outer.cell(inner.cell(cell)));
        }

        #[test]
        fn a_voxel_cell_covers_the_moved_unit_cube(transform in transform_strategy(), cell in cell_strategy()) {
            // The centre of the Voxel must land on the centre of the moved cell.
            let centre = cell.as_vec3() + Vec3::splat(0.5);
            let moved = transform.point(centre);
            let expected = transform.cell(cell).as_vec3() + Vec3::splat(0.5);
            prop_assert!((moved - expected).abs().max_element() < 1e-4);
        }
    }
}
