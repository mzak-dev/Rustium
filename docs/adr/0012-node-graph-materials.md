# Materials are node graphs, assigned to palette Colors and compiled to WGSL

A palette entry is now a Color (sRGB plus a reference to one Material), and a Material is a named node graph shared by any number of Colors; voxels still store one byte. Each graph compiles to a WGSL function behind a single `evaluate_material(color_index, position, normal) -> Surface` entry point that both the raster and the path tracer call, so what you see while modelling matches the render and procedural detail stays sharp at any zoom. The node editor is drawn with egui's painter rather than a graph crate.

## Considered Options

- **One graph per palette entry**: no sharing, so "Roof slate" across two shades would be two graphs to keep in sync.
- **GPU bytecode interpreter**: edits without pipeline rebuilds, but every pixel and every path-tracer bounce pays the interpretation cost.
- **Bake-only (CPU to textures)**: simple GPU side, but procedural detail blurs up close and every edit waits for a bake. Baking stays as an export feature.
- **egui-snarl for the editor**: mature, but 0.12 requires egui 0.36 while Rustium is on 0.37; a fork or a downgrade costs more than drawing the mockup's nodes ourselves.

## Consequences

- Changing a graph's shape rebuilds the material pipelines, so the UI shows a "Compiled" status. The compiled WGSL
  contains no numbers: node constants live in a storage buffer, so editing a value only rewrites that buffer and
  needs no rebuild. The rebuild runs on a worker thread and a failed compile keeps the previous pipelines.
- Glass (transmission, IOR) lives on the Surface output and haze on a separate Volume output.
- The core palette types were renamed from `Material`/`MaterialId` to `Color`/`ColorId`; `Material` now means the node graph only (`MaterialGraph` in code).
