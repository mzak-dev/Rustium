# Architecture

Rustium is a Cargo workspace. Dependencies point down: `rustium-app → rustium-render → rustium-core`.
Vocabulary is defined in [GLOSSARY.md](GLOSSARY.md); decisions and their reasons are in [docs/adr](docs/adr).

| Crate | Owns | Must not depend on |
|---|---|---|
| `rustium-core` | Scene, Shape (copy-on-write 32³ chunks), Palette, Commands → Edits, History, raycast, greedy meshing | GPU, windowing, async |
| `rustium-render` | wgpu device and surface, chunk raster with MSAA, ground grid, overlay lines, glass blur and composite | windowing, egui |
| `rustium-app` | the `rustium` binary: winit lifecycle, egui HUD / Settings / start screen, i18n (PL + EN), camera, input, logging | — |
| `rustium-mcp` *(M2)* | MCP server over HTTP and stdio; the only crate with tokio | — |
| `rustium-script` *(M2)* | Luau scripting through mlua | — |

## Data flow of one frame

1. egui runs the HUD (`hud::draw`), the start screen and Settings. Viewport input (picking, brush strokes,
   camera, window drag and resize) is handled by a full-window background area behind the islands.
2. Brush gestures become `rustium_core::Command`s; `apply` returns an `Edit` (before/after `Arc<Chunk>`s).
   A stroke folds its Edits into one History step.
3. Chunks touched by Edits (plus neighbours, for culling and AO) are remeshed on the CPU and uploaded.
4. `ViewportRenderer` draws the scene into an offscreen MSAA target, blurs it at quarter resolution and
   composites it onto the surface through an sRGB view, showing the blur under every glass island.
5. egui paints the interface on top through the non-sRGB view of the same surface texture.

## Coordinates

Everything in core and render is in voxel space (one unit = one voxel, Y up). Metres appear only in the
interface, through the Scene's Voxel size. Interface sizes are logical points; the app converts glass
rects to physical pixels with egui's `pixels_per_point` before handing them to the renderer.

## Runtime plugins

None yet. When they come, the plugin boundary (WASM vs `abi_stable` vs C ABI) is decided and recorded here.
