# Native `.ruv` format plus our own `.vox` reader and writer

Scenes are saved as `.ruv`: a magic and version header followed by an lz4-compressed postcard encoding of serde types, which keeps the format private to Rustium and versioned explicitly. MagicaVoxel `.vox` is supported by our own parser and writer instead of a crate, converting between `.vox` Z-up and Rustium's Y-up and splitting Shapes larger than 256³ on export. `.vox` cannot be the native format because it limits Models to 256³ and lacks our glass and haze Material properties.
