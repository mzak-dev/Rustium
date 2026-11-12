//! Browser entry point of the Rustium voxel editor.
//!
//! `scripts/build-web.sh` builds this crate with `wasm-pack` into `web/pkg`, next to `web/index.html`, which loads it.
//! On start it routes panics and `tracing` to the browser console, then hands over to `rustium_app::web::run`, which
//! draws the editor into the page's `<canvas id="rustium">` with WebGPU. On targets other than `wasm32` the crate is
//! empty.

#[cfg(target_arch = "wasm32")]
mod entry {
    use tracing_subscriber::filter::Targets;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{Layer, fmt};
    use wasm_bindgen::prelude::wasm_bindgen;

    /// Called by the generated JavaScript as soon as the module is instantiated.
    #[wasm_bindgen(start)]
    fn start() {
        console_error_panic_hook::set_once();
        init_logging();
        if let Err(report) = rustium_app::web::run() {
            tracing::error!(error = ?report, "Rustium could not start");
        }
    }

    /// Sends `info` and above (wgpu's internals from `warn`) to the browser console.
    fn init_logging() {
        let filter = Targets::new()
            .with_default(tracing::Level::INFO)
            .with_target("wgpu_hal", tracing::Level::WARN)
            .with_target("wgpu_core", tracing::Level::WARN);
        let console = fmt::layer()
            .with_ansi(false)
            // The browser has no clock `std` can read; the console stamps each line itself.
            .without_time()
            .with_writer(tracing_web::MakeWebConsoleWriter::new())
            .with_filter(filter);
        // A second start in the same page keeps the first subscriber.
        let _ = tracing_subscriber::registry().with(console).try_init();
    }
}
