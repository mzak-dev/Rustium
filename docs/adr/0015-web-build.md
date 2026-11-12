# The editor also runs in the browser, on WebGPU, as a single-threaded build of the same crates

The browser version is the desktop editor compiled for `wasm32-unknown-unknown`: `rustium-core`, `rustium-render` and `rustium-app` build for both targets, and a thin `rustium-web` crate (built with `wasm-pack`) installs the panic hook and console logging and calls `rustium_app::web::run`, which runs the same winit `App` on a `<canvas>`. It needs WebGPU; there is no WebGL fallback, because the path tracer is a compute shader writing `rgba32float` storage textures. Every push to `main` builds it, smoke-tests it in headless Chromium and publishes it on GitHub Pages ([docs/web.md](../web.md)).

## Considered Options

- **eframe instead of the own winit loop**: eframe's web backend is more polished (text input, IME, clipboard), but the editor owns its frame loop, its surface and the glass composite; moving to eframe would rewrite the app layer for both targets.
- **WebGL2 fallback**: wider support, but no compute shaders and no storage textures, so Render mode and the Effects preview would not exist; the editor would be a different product in those browsers.
- **Threads through `SharedArrayBuffer`**: needs cross-origin isolation headers (COOP/COEP), which GitHub Pages cannot send.
- **Same crates, `cfg(target_arch = "wasm32")` where a platform is missing (chosen)**: one code base and one interface; the differences are small and listed below.

## Consequences

- No threads in the browser. Where the desktop uses a worker, the web build does the work inline: file jobs run when they are submitted; Material pipelines are created at once and their error scope is awaited on the event loop (`wasm-bindgen-futures`) instead of blocking with `pollster`. Code that blocks on the GPU (the offline renderer, `read_image`) is compiled for native targets only, so it cannot be called by accident.
- The GPU arrives asynchronously: the canvas is created in `resumed` and the editor is assembled when `UserEvent::GpuReady` arrives. The canvas's first `Resized` lands before that, so the surface is resized once the editor exists.
- Desktop only, kept out with `cfg` or hidden behind `crate::DESKTOP`: files on disk (opening and importing use the browser's file picker, saving and exporting download the file), autosave and recovery, the recent files list, log files (the browser console instead), the MCP server (`rustium-mcp` builds only its permission types for wasm), Luau scripts (Luau is C++ and needs a libc), render jobs (saved images, turntables, the Shot queue), the window buttons, title bar and resize edges, and the graphics API and device pickers. Settings live in local storage, in the same TOML as the settings file.
- `std::time::Instant` and `SystemTime::now` panic in the browser, so timing code uses `web-time` (which is `std::time` on native targets).
- Browsers check WGSL more strictly than naga: Tint enforces the uniformity rules, so an implicit-derivative `textureSample` after a branch on a per-pixel value is an error there and not on the desktop. The browser smoke test in CI catches such shaders.
- The release build is about 8 MB of wasm, 3.3 MB gzipped. `wasm-opt` must be told the WebAssembly features rustc enables by default, or it rejects the module (`[package.metadata.wasm-pack.profile.release]` in `crates/rustium-web/Cargo.toml`).
