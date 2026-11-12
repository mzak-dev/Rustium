# Architecture

Rustium is a Cargo workspace. Dependencies point down: `rustium-app → rustium-render → rustium-core`; `rustium-app` also uses
`rustium-mcp`, `rustium-script` (`rustium-script → rustium-core`) and `rustium-platform`.
Vocabulary is defined in [GLOSSARY.md](GLOSSARY.md); decisions and their reasons are in [docs/adr](docs/adr).

| Crate | Owns | Must not depend on |
|---|---|---|
| `rustium-core` | Scene graph (Groups, Models, Instances), Shape (copy-on-write 32³ chunks), Palette, material graphs, Commands → Edits, History, raycast and Scene picking, Brush / Fill / Mirror geometry, greedy meshing, the `.ruv`, MagicaVoxel `.vox` and JASC `.pal` readers and writers (pure bytes in, bytes out) | GPU, windowing, async |
| `rustium-render` | wgpu device and surface, chunk raster with MSAA, ground grid, overlay lines, glass blur and composite, the Render-mode path tracer | windowing, egui |
| `rustium-app` | the `rustium` binary: winit lifecycle, egui HUD / Settings / start screen, i18n (PL + EN), camera, input, logging, documents (Open / Save, dirty tracking, autosave and recovery, recent files) | — |
| `rustium-mcp` | in-process MCP server (streamable HTTP on `127.0.0.1:7420`) and the `--mcp-stdio` bridge; the only crate with tokio | the Scene (it depends on no other Rustium crate) |
| `rustium-script` | sandboxed Luau scripting through mlua; a run is one `Edit` | GPU, windowing, async |
| `rustium-platform` | Win32 `WM_NCHITTEST` for the frameless window (Snap Layouts over the maximize button, native title-bar drag and resize edges); a no-op off Windows; the only crate with `unsafe` | everything else |

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
   composites it onto the surface through an sRGB view, showing the blur under every glass island. When a Color of
   the Palette uses a transparent Material, the scene is drawn with dithering and a jittered projection and resolved
   by the temporal pass (see Raster transparency).
5. egui paints the interface on top through the non-sRGB view of the same surface texture.

## Path tracer (Render mode)

Render mode swaps the raster pass for a progressive compute-shader path tracer ([ADR 0001](docs/adr/0001-hybrid-renderer.md)).
Scene upload and traversal are separate: the software DDA and the hardware ray-query backend ([ADR 0013](docs/adr/0013-ray-query-backend.md)) share the upload and all shading.

- **Upload** (`trace_scene.rs`): every Shape of a visible Model is uploaded once into a pooled storage buffer
  (chunks packed to one byte per voxel, `Arc` pointer equality skips unchanged ones), however many Instances show
  it. Each Shape gets a dense chunk-index grid, and a Model list records, per visible Model, its Transform
  (the same axis images and offset the raster uses) and its Shape's grid. Hidden Models are left out and the
  Focus dim is ignored. `PathTracer::upload_scene(&Scene)` is the single entry point; it also updates the
  acceleration structures when the hardware engine is on.
- **Traversal** (`shaders/pathtracer.wgsl`, `trace_model` / `trace_voxels`; software loop in `trace_dda.wgsl`): the ray is moved into each Model's
  Shape space and walked with a two-level DDA. Empty chunks are skipped in one jump, occupied ones are walked
  voxel by voxel; the nearest hit over all Models wins, earlier Models first where they overlap. `trace_voxels` has three modes (`MODE_*`): nearest voxel; *inside*, which
  travels through the voxels of one Material (glass or haze, identified by Material index, so voxels of two Colors with
  the same Material have no face between them) and always ends in a hit on the first voxel of another Material, or on
  air (Color 0, also at a chunk or grid edge); and *shadow*, which skips voxels of Materials that do not cast shadows
  and only dims the ray by the density of haze voxels (`Hit::optical`). Hot paths are marked `HOT PATH` in the shader and in
  `PathTracer::encode`.
- **Ray-query backend** (`ray_scene.rs`, `shaders/trace_rq.wgsl`): `pathtracer.wgsl` calls `trace_scene_hit()` and
  `shadow_transmittance()`, which come from `trace_dda.wgsl` (software) or `trace_rq.wgsl` (hardware); the shader module is
  composed from the shared body plus one of them. Hardware builds one BLAS per Shape of procedural boxes, one per
  occupied chunk, and a TLAS with one instance per visible Model (Transform as the instance matrix, instance index
  = Model index, custom data = where the Shape's boxes start). A candidate box runs the same `trace_voxels` on a
  one-cell view of the chunk grid, so hits are identical. BLASes are rebuilt only when a Shape's chunk set
  changes. `RayTracing` (Auto, Software, Hardware) is the preference, `TraceEngine` the result; `GpuContext`
  requests `EXPERIMENTAL_RAY_QUERY` only when the adapter has it. Rays inside glass or haze use the DDA in both engines; shadow rays walk every chunk box the query reports
  (no early out, haze adds up per chunk) in the hardware engine.
  Settings → Performance sets it, the Render inspector's Engine row shows the active one.
- **Shading**: next-event sun sampling (soft cone), cosine-weighted diffuse bounces, metal, clear-coat, glass and
  haze, emission found by path hits, an analytic sky with presets, and a ground of None, Studio or Shadow catcher.
- **Glass and haze** (`radiance` in `pathtracer.wgsl`): a path carries a *medium* (the Material it travels through, 0 =
  air). Hitting a voxel whose Surface output has transmission starts an interface: exact dielectric Fresnel between
  the current index and the Material's IOR (so IOR 1 reflects nothing), total internal reflection, a roughness-scattered
  microfacet normal for rough glass, and the base color as a tint when entering. Inside the medium the walk reports
  the first voxel of another Material, which is another interface (glass in glass, glass on air) or an opaque surface
  that is shaded from inside. A Material with a Volume output is a homogeneous medium per segment: the free-flight
  distance is sampled from the density at the segment's start (spatially varying density is therefore evaluated where
  a ray enters or scatters, not along the ray), a scatter multiplies the throughput by the scattering colour, adds the
  sun by next-event estimation with a Henyey-Greenstein phase (g = 0.3) and shadow transmittance, and continues with a
  sampled direction. Haze with transmission 1 and IOR 1 is pure fog: its interfaces are straight passes that do not
  count as bounces, and a path that is never bent still sees a transparent background as transparent. Surfaces with
  Volume but no transmission stay opaque (the surface decides). Russian roulette runs after every event. Clay turns
  glass and haze into grey solids; Toon and Wire shade the first hit only; the G-buffer gives glass and haze a white
  albedo so the denoiser does not divide the background by the tint.
- **Casts shadow**: the Colors table carries per-Color flags next to the Material index (bit 16: does not cast shadows,
  bit 17: Material has a Volume output). Shadow rays skip no-shadow voxels, dim by density when crossing haze (opaque
  beyond optical depth 12) and stop at everything else; sun next-event estimation, the shadow catcher and the Toon
  shading use them. Bounce light is not affected: a path that reaches a voxel still shades it.
- **Materials**: a Scene holds Materials as node graphs (`MaterialGraph`, index 0 is the Default material); each
  Palette Color points at one (`Color::graph`). `rustium-core` owns the data: typed sockets (Color / Vector / Float,
  any converts to any), per-input constants, links (one per input, no cycles), frames, Bypass, stable node ids, the
  node library as tables (`GraphNodeKind::spec`) and the starter Materials (`SampleMaterial`). Every change is a
  `Command` (`EditMaterial` with a `GraphEdit`, `AddMaterial`, `DuplicateMaterial`, `DeleteMaterial`, ...) that
  becomes an undoable Edit through the existing Palette-and-graphs snapshot.
  `rustium-render::compile_materials` turns the graphs into WGSL: one `material_N` function per Material and a
  `switch` over the Color's Material inside `evaluate_material`, placed at the marker line at the end of
  `shaders/material.wgsl` (which holds the helper library: noise, cells, edge distance, bevel, hue, ramp).
  **Shape versus numbers:** the source depends only on a graph's shape (nodes, links, Bypass, which sockets hold a
  constant, ramp stop count). Every number lives in the `material_params` storage buffer (binding 7, fixed capacity,
  slots are literals in the source). Editing a number rewrites that buffer; editing the shape rebuilds the raster
  and path tracer pipelines on a worker thread (`MaterialCompiler`): the old pipelines keep drawing, the new ones
  are swapped in together with their numbers, and a shader that fails to compile keeps the old ones and reports
  `CompileStatus::Error` (`ViewportRenderer::material_status`, shown as the "Compiled" chip).
  The Palette is a small uniform (`GpuMaterialTable`, binding 1).
  **Contract:** `fn evaluate_material(color_index: u32, position: vec3<f32>, normal: vec3<f32>) -> Surface`
  in `shaders/material.wgsl` is the only code that reads Color and Material data; both renderers shade through it.
  `Surface` now also carries `volume_density` and `volume_color` (the Volume output) for the M5 haze and glass work,
  and a normal that the Voxel bevel may have bent (the raster shades with it). Inputs the node library needs beyond
  the signature: `position` is in voxels, so the voxel cell, the distance to the voxel edge (Edge wear, Voxel bevel,
  per-voxel Noise) follow from `position` and `normal` alone; the private `material_ao` is set by the raster pass from
  the mesh's per-vertex occlusion and stays 1 in the path tracer, whose bounce light already darkens corners.
  Approximations: the Space option of Noise, Cells and Gradient (Model / World) is stored but both read the Scene
  position. "Casts shadow" only affects the path tracer (the raster viewport has no shadows). Raster shows emission
  unlit.
- **Raster transparency** (`shaders/raster.wgsl`, `taa.rs`, `shaders/taa.wgsl`, ADR 0006): a fragment whose Material
  has transmission or a Volume output is kept with a probability (its coverage: tint, a Fresnel term and the haze
  density as if two voxels thick) and discarded otherwise, by interleaved gradient noise that moves every frame; kept
  fragments write depth, so no sorting is needed. While any Palette Color is transparent the projection is jittered
  (Halton 2,3, 8 frames), the scene resolves into a raw target and the TAA pass blends it with the reprojected history
  (position from depth and last frame's view-projection) clamped to the 5x5 neighbourhood, which keeps what moved from
  ghosting; the history is Rgba16Float, ping-ponged. The history weight is 1/10 while the view changes, shrinks to 1/32
  once the image is still, jumps to 0.35 after an edit or Material change, and the history is dropped on size or MSAA
  change, a camera cut (eye jump over 48 voxels) or when TAA was off. `ViewportRenderer::is_settling` tells the app to
  keep redrawing for ~40 frames after a change. MSAA stays on for edges; overlay lines and the grid are drawn in the
  same pass, so they are resolved with the scene. The Settings mock has only the MSAA row in its Anti-aliasing section,
  so there is no TAA switch.
- **Accumulation**: one pass per frame writes `prev + (sample - prev) / n` into an `Rgba32Float` texture,
  ping-ponging two textures. Any change to camera, scene, Palette, light, size, bounces, region, style or lens
  restarts accumulation (`TraceKey`). The same pass writes a G-buffer on its first sample (normal and distance,
  albedo) from a pixel-centre ray.
- **Styles** (`shaders/trace_style.wgsl`, hooks in `radiance`): Clay swaps every Surface for one grey; Toon and Wire
  shade the first hit only (three light bands, or dark faces with light lines on the edges between voxel faces that
  are not coplanar); Pixel traces one pixel per block and the resolve stage repeats it. A thin lens
  (`TraceFrame::lens`) moves the ray start over the aperture and keeps the focal-plane point.
- **Resolve** (`resolve.rs`, `shaders/resolve.wgsl`): after the trace pass, compute passes turn the accumulation into
  the picture: an edge-aware a-trous denoiser on albedo-demodulated light (weights from normal, plane distance,
  albedo and luminance; skipped for Wire), the Pixel upscale, Toon outlines from the G-buffer, and AOV passes
  (albedo, normals, depth). It runs only when samples arrived or style, denoise or pass changed. `present.wgsl`
  tonemaps (ACES) the resolved texture into the viewport target at the HUD's frame rect; AOV passes bypass the
  tonemap.
- **Output**: `read_image` copies the resolved image back, so saved files match the screen. The app encodes
  tonemapped PNG or linear EXR on a worker thread after a native save dialog (`rfd`). Turntable videos and the Shot
  queue (`render_job.rs`) run on a worker thread with an `OfflineRenderer` of their own (a second `PathTracer` on the
  same device, so the viewport keeps accumulating) and report progress and accept a cancel; frames are piped as raw
  RGB into the `ffmpeg` found on `PATH`, or written as a PNG sequence when there is none.
- `tests/pathtracer.rs` renders tiny scenes headless on a real GPU (skipped without an adapter);
  `RUSTIUM_DUMP_DIR=<dir>` also writes them as PNGs.

Known limits: no light sampling for emissive voxels (small emitters are noisy), haze density is sampled per
  medium segment (not integrated along the ray), glass casts no caustics (a no-shadow Material is simply skipped by
  shadow rays), a dense chunk grid capped at 4M
cells, one sample per pixel per frame. The denoiser is spatial only (no temporal reuse) and a lens
of real size would not blur a voxel model, so the aperture is a scale-free stand-in (`aperture_radius_voxels`).

## Threading and MCP

The Scene is only ever touched by the UI thread (the winit event loop). Everything else talks to it by message:

```
MCP client ──HTTP──▶ rustium-mcp thread ("rustium-mcp", tokio, current-thread runtime)
                         │  Tools::call → Bridge::call(Request)
                         │  1. permission check (SharedPermissions, atomic bitmask)
                         │  2. crossbeam bounded channel ── Envelope{request, reply} ──▶ UI thread
                         │  3. Waker → EventLoopProxy::send_event(UserEvent::Automation)
                         │  4. spawn_blocking(reply.recv_timeout(30 s))     ◀── Reply (JSON or error)
UI thread: user_event → McpHost::drain → automation::handle → Editor::execute_checked → Command → apply → Edit → History
```

- **One sync/async boundary**, `Bridge::call`. Past it the server never sees Scene state; the UI thread builds the
  JSON answer. tokio is used only in `rustium-mcp`, on one named thread; every connection task lives in a
  `JoinSet` owned by the accept loop, and dropping or stopping the `Server` cancels and joins everything.
- **Every change is a Command.** MCP edits (`SetVoxels`, `ApplyBrush`, `SetColor`) and script runs go
  through `rustium_core::apply` and land in `History` as one step each, so they show up live and undo with Ctrl+Z.
  Voxel Commands name a Model by `NodeId` and use that Model's cell space; MCP tools and scripts default to the
  Focus Model and refuse a Group, a hidden Model or a locked Model with a clear error. A script buffers its writes
  and applies one `SetVoxels` at the end, so a failing script changes nothing.
- **Permissions** (Read model, Edit voxels, Edit palette, Run scripts, Start renders) are checked on the server
  side before anything is queued, from atomics the UI updates when Settings change; a refused call returns a tool
  error naming the switch. The queue is bounded (64): a flood gets a "busy" error instead of blocking. Requests
  that carry an `Origin` header (web pages) or a non-loopback `Host` are refused by the HTTP layer.
- **Transports.** The editor serves stateless streamable HTTP with plain JSON responses. `rustium --mcp-stdio` runs
  no window and no logging: it forwards each stdin line to that endpoint (`rustium_mcp::run_stdio_bridge`) and
  answers with a JSON-RPC error if the editor is not running. Both can be used at the same time.
- **Scripts** run on the UI thread inside `rustium-script`: Luau sandbox, no `io`/`os`/`require`, memory limit,
  interrupt budget and wall-clock limit (`Limits`), so a runaway script cannot hang the editor.
- **Tests without a window**: `rustium-mcp/tests/round_trip.rs` drives the server with a fake UI thread;
  `rustium-app/src/mcp.rs` drives a real `Editor` through the stdio bridge, HTTP, the channel and `McpHost::drain`.

## Files

- **Formats** live in `rustium-core` (`ruv.rs`, `vox.rs`, `pal.rs`) and never touch the disk; see ADR 0010. Loading
  validates everything and returns a `FileError`, so a damaged file is a toast, not a crash.
- **IO** in `rustium-app/src/files.rs` runs on one `io-worker` thread; native dialogs run on short-lived threads and
  turn a chosen path into a job. The UI thread only clones the Scene (chunks are shared) and polls outcomes, which
  wake the event loop through a `UserEvent`. Writes go to `<file>.tmp` and are renamed over the target.
- **Dirty** means the Editor revision differs from the one saved; the window title carries an asterisk. New, Open,
  Import and closing ask first through a modal `Prompt`.
- **Autosave** writes `<file>.ruv.recovery` beside the document (or `recovery/untitled.ruv.recovery` in the data
  directory for a Scene that was never saved) every Settings interval while the Scene is dirty. A clean save, a
  clean exit and "Discard" delete it; at startup, and after opening a file, a newer recovery copy is offered.
- **Recent files** are kept in `recent.toml` in the data directory, with the root Node's name as secondary text.

## Coordinates

Everything in core and render is in voxel space (one unit = one voxel, Y up). Metres appear only in the
interface, through the Scene's Voxel size. Interface sizes are logical points; the app converts glass
rects to physical pixels with egui's `pixels_per_point` before handing them to the renderer.

## Runtime plugins

None yet. When they come, the plugin boundary (WASM vs `abi_stable` vs C ABI) is decided and recorded here.
