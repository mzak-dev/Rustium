# The path tracer can find hits with hardware ray query over chunk boxes, with the voxel DDA inside each box

Render mode has two interchangeable traversals behind `trace_scene_hit()` and `occluded()` in the path tracer shader: the software two-level DDA, and a wgpu ray query (`EXPERIMENTAL_RAY_QUERY`) over acceleration structures. Each Shape gets one bottom-level structure of procedural boxes, one per occupied chunk; each visible Model is an instance of its Shape's structure with the Model's Transform, so Instances share it. The hardware only reports which chunk boxes a ray crosses; inside a box the same `trace_voxels` DDA finds the voxel. Shading, styles, depth of field, denoising and `evaluate_material` are shared, and hits are voxel-exact in both engines. The device asks for the feature only when the adapter has it, and Settings → Performance picks Automatic (hardware when available), Software or Hardware.

## Considered Options

- **Triangles from the greedy mesh**: the shape ADR 0001 sketched, and what RT cores are fastest at. But every voxel edit rebuilds the BLAS, the hit then needs a second lookup to find the voxel and its Color, and triangles cannot describe a voxel's interior for haze.
- **One box per voxel run or per greedy box**: tighter than a chunk box, so fewer DDA steps per ray, but the box count grows with surface complexity and every edit rebuilds the structure.
- **Hardware only**: simplest shader, but experimental, Vulkan-only today, and missing on many GPUs.

## Consequences

- Painting inside existing chunks costs no rebuild; a structure is rebuilt only when a Shape's set of occupied chunks changes. The top-level structure is rebuilt on every upload (a few instances).
- A ray inside glass still uses the DDA, because it must stay with the Model it is in and stop at the first different voxel. A medium (haze) added to the DDA is therefore available in both engines for free.
- Where two Models overlap exactly, which one wins can differ between engines, since the hardware reports boxes in no fixed order.
- Chunk boxes are slightly larger than their chunk (0.01 voxels) so rays grazing a chunk face are not lost to rounding in the hardware test.
- The traversal is chosen when a `PathTracer` is built; changing the setting rebuilds it and uploads the Scene again.
- On lavapipe (a CPU Vulkan driver that emulates ray query) hardware is slower than the DDA; the tests there prove equal images, not speed. Speed on RT hardware is still to be measured.
