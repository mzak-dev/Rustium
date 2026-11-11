# One Voxel size per Scene; Transforms snap to the voxel grid

All Models in a Scene share one Voxel size, and Node Transforms are whole-voxel offsets plus quarter-turn rotations and mirrors, so every voxel of every Model sits on one world grid. The interface mockups show a Voxel size per Node (16 m → 1 m → 25 cm → 6.25 cm); we deliberately did not adopt that, because mixed resolutions complicate picking, Fill, overlap and `.vox` exchange for a feature the editor does not need yet. Free (non-grid) Transforms were rejected for the same reason.
