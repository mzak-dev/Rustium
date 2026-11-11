use crate::palette::Palette;
use crate::shape::Shape;

/// The document being edited: the voxels, the Palette and the Voxel size.
#[derive(Debug, Clone)]
pub struct Scene {
    pub(crate) shape: Shape,
    palette: Palette,
    voxel_size: f32,
}

impl Scene {
    /// Voxel size of a new Scene, in metres.
    pub const DEFAULT_VOXEL_SIZE: f32 = 0.25;

    /// An empty Scene with the default Palette.
    pub fn new() -> Self {
        Self::with_voxel_size(Self::DEFAULT_VOXEL_SIZE)
    }

    /// An empty Scene whose voxels are `voxel_size` metres wide (non-positive sizes fall back to the default).
    pub fn with_voxel_size(voxel_size: f32) -> Self {
        let voxel_size = if voxel_size > 0.0 {
            voxel_size
        } else {
            Self::DEFAULT_VOXEL_SIZE
        };
        Self {
            shape: Shape::new(),
            palette: Palette::default(),
            voxel_size,
        }
    }

    /// The voxels of the Scene.
    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    /// Materials voxels can use.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Edge length of one voxel in metres.
    pub fn voxel_size(&self) -> f32 {
        self.voxel_size
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}
