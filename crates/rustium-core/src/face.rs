use glam::IVec3;

/// One of the six sides of a voxel, named after the direction it faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Faces +X.
    PosX,
    /// Faces -X.
    NegX,
    /// Faces +Y (up).
    PosY,
    /// Faces -Y (down).
    NegY,
    /// Faces +Z.
    PosZ,
    /// Faces -Z.
    NegZ,
}

impl Face {
    /// All faces, in the order of [`Face::index`].
    pub const ALL: [Self; 6] = [Self::PosX, Self::NegX, Self::PosY, Self::NegY, Self::PosZ, Self::NegZ];

    /// Unit vector pointing out of this face.
    pub fn normal(self) -> IVec3 {
        match self {
            Self::PosX => IVec3::X,
            Self::NegX => IVec3::NEG_X,
            Self::PosY => IVec3::Y,
            Self::NegY => IVec3::NEG_Y,
            Self::PosZ => IVec3::Z,
            Self::NegZ => IVec3::NEG_Z,
        }
    }

    /// Axis the face is perpendicular to: 0 = X, 1 = Y, 2 = Z.
    pub fn axis(self) -> usize {
        match self {
            Self::PosX | Self::NegX => 0,
            Self::PosY | Self::NegY => 1,
            Self::PosZ | Self::NegZ => 2,
        }
    }

    /// Whether the face points along the positive direction of its axis.
    pub fn is_positive(self) -> bool {
        matches!(self, Self::PosX | Self::PosY | Self::PosZ)
    }

    /// Stable index 0..6, used to pack the face into GPU vertices.
    pub fn index(self) -> u32 {
        match self {
            Self::PosX => 0,
            Self::NegX => 1,
            Self::PosY => 2,
            Self::NegY => 3,
            Self::PosZ => 4,
            Self::NegZ => 5,
        }
    }

    /// The face whose [`normal`](Self::normal) is `normal`; `None` unless it is one of the six unit axis vectors.
    pub fn from_normal(normal: IVec3) -> Option<Self> {
        Self::ALL.into_iter().find(|face| face.normal() == normal)
    }

    /// The face on `axis` that a ray moving with `step` (+1 or -1) enters through.
    pub(crate) fn entered_by_step(axis: usize, step: i32) -> Self {
        match (axis, step > 0) {
            (0, true) => Self::NegX,
            (0, false) => Self::PosX,
            (1, true) => Self::NegY,
            (1, false) => Self::PosY,
            (_, true) => Self::NegZ,
            (_, false) => Self::PosZ,
        }
    }
}
