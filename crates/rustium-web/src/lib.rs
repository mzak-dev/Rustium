//! Browser entry point of the Rustium voxel editor.
//!
//! `scripts/build-web.sh` builds this crate with `wasm-pack` into `target/web/pkg`, next to a copy of `web/index.html`,
//! which loads it. On start it routes panics to the browser console, then hands over to `rustium_app::web::run`, which
//! sets up logging and draws the editor into the page's `<canvas id="rustium">` with WebGPU. On targets other than
//! `wasm32` the crate is empty.

/// Called by the generated JavaScript as soon as the module is instantiated.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
    if let Err(report) = rustium_app::web::run() {
        // Logging may be what failed, so this goes to the console directly.
        web_sys::console::error_1(&format!("Rustium could not start: {report:#}").into());
    }
}
