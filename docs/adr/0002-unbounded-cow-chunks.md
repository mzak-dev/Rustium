# Unbounded Shapes in copy-on-write chunks; undo as chunk snapshots

A Shape is a hash map from chunk coordinate to `Arc<Chunk>` (32³ voxels); empty chunks are dropped, so Shapes have no size limit. The Volume shown in the interface is only the box around the voxels and grows on Attach. Every Edit stores the before/after `Arc` of the chunks it touched, so undo, redo, Duplicate and Instances all reuse unchanged chunks for free, and one mechanism covers every Tool instead of a hand-written inverse per Tool.

## Consequences

- Memory per undo step is proportional to touched chunks (32 KiB each), so History has a step limit (Settings, default 200).
- Fill in 3D must be bounded by the Volume, otherwise it would never end.
