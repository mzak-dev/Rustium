# One byte per voxel, indexing a Scene-wide Palette of 255 Colors

A voxel is a `u8`: 0 is empty and 1–255 index the Scene's Palette. This keeps chunks at 32 KiB, uploads cheaply to the GPU, matches `.vox`, and recoloring a Color recolors every voxel using it. We rejected RGBA per voxel (4× memory, no shared palette entries) and `u16` indices (2× memory for more Colors than voxel art needs).
