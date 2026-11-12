# The web version

Rustium also runs in the browser, built from the same crates for `wasm32-unknown-unknown` and drawing with WebGPU. Every push to `main` publishes it at **https://mzak-dev.github.io/Rustium/**. Why it is built this way, and what it leaves out, is in [ADR 0015](adr/0015-web-build.md).

## Browsers

It needs WebGPU: a recent Chrome or Edge, Safari 26 or later, or a Firefox with WebGPU turned on. Without it the page says so and links to the desktop app.

## What differs from the desktop app

| Desktop | Browser |
|---|---|
| Open, import | the browser's file picker |
| Save, Save as, export `.vox` / `.pal`, export the keymap | a download, named after the document |
| Autosave and recovery, recent files | none |
| Settings file | local storage (same TOML) |
| Log files | the browser's developer console |
| MCP server, Luau scripts | none (a page cannot listen on a port; Luau needs a libc) |
| Saved renders, turntable videos, Shot queue | none (they need threads, files and `ffmpeg`); the Render mode preview works |
| Graphics API and device, window buttons | chosen by the browser; the tab is the window |

## Building it locally

```bash
cargo install wasm-pack          # once; it fetches the matching wasm-bindgen and wasm-opt
scripts/build-web.sh             # release build into target/web (scripts/build-web.sh --dev is faster)
python3 -m http.server -d target/web 8000
```

Then open http://localhost:8000/. Opening `index.html` as a file does not work: browsers load WebAssembly modules only over HTTP.

The smoke test that CI runs needs Node and Playwright's Chromium (`npm install playwright && npx playwright install chromium`):

```bash
node web/smoke-test.cjs http://localhost:8000/ /tmp/screenshots
```

It runs WebGPU on SwiftShader, so it works without a GPU, and fails on any console error, which is where the browser's shader compiler reports invalid WGSL.

## Deployment

`.github/workflows/pages.yml` runs on every push to `main` (and by hand): it builds the release version, smoke-tests it and deploys it with `actions/deploy-pages`. One-time setup: in the repository's **Settings → Pages**, set **Source** to **GitHub Actions**. The `ci.yml` job "Web build and smoke test" runs the same build and test on every pull request, so a change that breaks the browser version is caught before it is merged.
