/// Why a [`Command`](crate::Command) could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Command names a Material the Palette does not have.
    #[error("material {0} is not in the palette")]
    UnknownMaterial(u8),
    /// The brush is empty or larger than the editor allows.
    #[error("brush size {size} is outside 1..={max}")]
    BrushSize {
        /// Requested size in voxels.
        size: u32,
        /// Largest allowed size in voxels.
        max: u32,
    },
}
