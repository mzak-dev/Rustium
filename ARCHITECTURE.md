# Architecture

Rustium is a Cargo workspace. Dependencies point down: `rustium-app → rustium-render → rustium-core`.
Vocabulary is defined in [GLOSSARY.md](GLOSSARY.md); decisions and their reasons are in [docs/adr](docs/adr).

| Crate | Owns | Must not depend on |
|---|---|---|
| `rustium-core` | Scene graph (Groups, Models, Instances), Shape (copy-on-write 32³ chunks), Palette, material graphs, Commands → Edits, History, raycast and Scene picking, Brush / Fill / Mirror geometry, greedy meshing | GPU, windowing, async |
| `rustium-render` | wgpu device and surface, chunk raster with MSAA, ground grid, overlay lines, glass blur and composite, the Render-mode path tracer | windowing, egui |
| `rustium-app` | the `rustium` binary: winit lifecycle, egui HUD / Settings / start screen, i18n (PL + EN), camera, input, logging | — |
| `rustium-mcp` *(M2)* | MCP server over HTTP and stdio; the only crate with tokio | — |
| `rustium-script` *(M2)* | Luau scripting through mlua | — |

## Data flow of one frame

1. egui runs the HUD (`hud::draw`), the start screen and Settings. Viewport input (picking, brush strokes,
   camera, window drag and resize) is handled by a full-window background area behind the islands.
2. Brush gestures become `rustium_core::Command`s; `apply` returns an `Edit` (before/after `Arc<Chunk>`s, plus a snapshot of the node graph when the tree
   changed). A gesture folds its Edits into one History step. Tools act inside the Focus: voxel Tools on the
   Model in Focus, Select box and Move on the child Nodes of a Group in Focus.
3. Chunks touched by Edits (plus neighbours, for culling and AO) are remeshed on the CPU and uploaded per Shape.
   Each visible Model is drawn as one instance of its Shape's chunks with its Transform and a dim factor
   (everything outside the Focus is dimmed).
4. `ViewportRenderer` draws the scene into an offscreen MSAA target, blurs it at quarter resolution and
   composites it onto the surface through an sRGB view, showing the blur under every glass island.
5. egui paints the interface on top through the non-sRGB view of the same surface texture.

## Path tracer (Render mode)

Render mode swaps the raster pass for a progressive compute-shader path tracer ([ADR 0001](docs/adr/0001-hybrid-renderer.md)).
Scene upload and traversal are separate so a ray-query backend can reuse the upload.

- **Upload** (`trace_scene.rs`): every Shape of a visible Model is uploaded once into a pooled storage buffer
  (chunks packed to one byte per voxel, `Arc` pointer equality skips unchanged ones), however many Instances show
  it. Each Shape gets a dense chunk-index grid, and a Model list records, per visible Model, its Transform
  (the same axis images and offset the raster uses) and its Shape's grid. Hidden Models are left out and the
  Focus dim is ignored. `PathTracer::upload_scene(&Scene)` is the single entry point a ray-query backend would
  reuse.
- **Traversal** (`shaders/pathtracer.wgsl`, `trace_model` / `trace_voxels`): the ray is moved into each Model's
  Shape space and walked with a two-level DDA. Empty chunks are skipped in one jump, occupied ones are walked
  voxel by voxel; the nearest hit over all Models wins, earlier Models first where they overlap. With `inside` set, the same walk travels through glass or haze voxels
  and stops at the first different voxel. Hot paths are marked `HOT PATH` in the shader and in
  `PathTracer::encode`.
- **Shading**: next-event sun sampling (soft cone), cosine-weighted diffuse bounces, metal, clear-coat, glass
  (Fresnel + refraction, tinted by the base color), emission found by path hits, an analytic sky with presets, and
  a ground of None, Studio or Shadow catcher.
- **Materials**: a Scene holds material graphs (index 0 is the Default material); each Palette Color points at one
  (`Material::graph`). Today a graph is constant Surface parameters. `GpuMaterialTable` uploads one record per
  graph and one entry per Color into a single uniform buffer shared by the raster pass and the path tracer.
  **Contract:** `fn evaluate_material(color_index: u32, position: vec3<f32>, normal: vec3<f32>) -> Surface`
  in `shaders/material.wgsl` is the only code that reads material data; both renderers shade through it. The
  Effects milestone replaces its body with WGSL generated from the node graphs. Raster shows emission unlit and
  treats transmission as opaque until M4.
- **Accumulation**: one pass per frame writes `prev + (sample - prev) / n` into an `Rgba32Float` texture,
  ping-ponging two textures; `present.wgsl` tonemaps (ACES) into the viewport target at the HUD's frame rect.
  Any change to camera, scene, Palette, light, size, bounces or region restarts accumulation (`TraceKey`).
- **Output**: `read_image` copies the accumulation back; the app encodes tonemapped PNG or linear EXR on a worker
  thread after a native save dialog (`rfd`).
- `tests/pathtracer.rs` renders tiny scenes headless on a real GPU (skipped without an adapter);
  `RUSTIUM_DUMP_DIR=<dir>` also writes them as PNGs.

Known limits: no light sampling for emissive voxels (small emitters are noisy), a dense chunk grid capped at 4M
cells, no denoiser, one sample per pixel per frame.

## Coordinates

Everything in core and render is in voxel space (one unit = one voxel, Y up). Metres appear only in the
interface, through the Scene's Voxel size. Interface sizes are logical points; the app converts glass
rects to physical pixels with egui's `pixels_per_point` before handing them to the renderer.

## Runtime plugins

None yet. When they come, the plugin boundary (WASM vs `abi_stable` vs C ABI) is decided and recorded here.
