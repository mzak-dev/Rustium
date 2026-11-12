# Hybrid renderer: rasterized editing, software path tracer for Render

The viewport rasterizes greedy-meshed chunks while editing because edits stay local (one 32³ chunk remeshes in well under a millisecond) and rasterization works naturally with gizmos, depth, MSAA and many transformed Models. Render mode uses a compute-shader path tracer that steps through voxels with DDA, because it runs on every wgpu GPU and haze (participating media) needs per-voxel stepping anyway. A hardware ray-query backend (BLAS of chunk boxes, TLAS from Models; [ADR 0013](0013-ray-query-backend.md)) is a second backend behind `EXPERIMENTAL_RAY_QUERY`, so the path tracer keeps its scene upload separate from its traversal.

## Considered Options

- Raymarching a brickmap for the viewport too: no meshing, but every overlay must write `frag_depth`, and many Models need a BVH from day one.
- Hardware ray query only: fastest on RTX/RDNA2+, but experimental API, missing on older GPUs, and triangles cannot describe a voxel's interior for haze.
