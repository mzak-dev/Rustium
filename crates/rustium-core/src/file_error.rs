//! Errors of the file formats: `.ruv`, `.vox` and `.pal`.

/// Why a file could not be read or written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FileError {
    /// The file does not start with the expected signature.
    #[error("this is not a Rustium scene file")]
    BadMagic,
    /// The file was written by a newer version of the editor.
    #[error("the file has format version {found}, but this editor reads up to version {supported}")]
    UnsupportedVersion {
        /// Version stored in the file.
        found: u32,
        /// Newest version this editor reads.
        supported: u32,
    },
    /// The file ends before its data does.
    #[error("the file is truncated")]
    Truncated,
    /// The data is present but does not describe a valid Scene.
    #[error("the file is damaged: {0}")]
    Corrupt(String),
    /// A MagicaVoxel file could not be read.
    #[error("invalid .vox file: {0}")]
    Vox(String),
    /// A palette file could not be read.
    #[error("invalid .pal file: {0}")]
    Palette(String),
    /// The Scene does not fit the target format.
    #[error("the scene is too large for this format: {0}")]
    TooLarge(String),
}
