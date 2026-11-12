//! Rustium: an editor for voxel scenes.
//!
//! The editor opens a frameless window (a `<canvas>` in the browser), renders the Scene with `rustium-render` and
//! draws the viewport-first interface with egui. Scene changes always go through `rustium_core::Command`s.
//!
//! [`run_native`] starts the desktop editor; the browser build in `crates/rustium-web` calls `web::run` (only on
//! `wasm32`). The desktop has a few things the browser cannot offer, kept out of the web build with
//! `cfg(target_arch = "wasm32")`: files on disk with autosave and recovery, log files, the MCP server, Luau scripts and
//! render jobs (saved images, turntable videos and the Shot queue).
#![warn(clippy::panic, clippy::expect_used)]

mod app;
#[cfg(not(target_arch = "wasm32"))]
mod automation;
mod camera;
mod cursor;
mod edit_anim;
mod editor;
mod effects_preview;
mod files;
mod hud;
mod i18n;
mod icons;
mod keymap;
mod logging;
mod mcp;
mod mouse_map;
mod recent;
// The browser has no render jobs; its module only keeps the interface's job slot compiling.
#[cfg_attr(target_arch = "wasm32", path = "render_job/web.rs")]
mod render_job;
mod render_mode;
mod render_output;
mod settings;
mod settings_input;
mod settings_window;
mod splash;
mod theme;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// Whether the editor runs as a desktop application, with a window of its own (title bar, resize edges, window
/// buttons), files on disk and the desktop-only features listed above. `false` in the browser.
pub(crate) const DESKTOP: bool = !cfg!(target_arch = "wasm32");

#[cfg(not(target_arch = "wasm32"))]
pub use native::run_native;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use color_eyre::eyre::{Result, WrapErr};
    use winit::event_loop::EventLoop;

    use crate::{app, logging, mcp, settings};

    /// Runs the desktop editor until its window closes.
    ///
    /// With `--mcp-stdio` it runs no window and forwards MCP messages between stdin/stdout and the running editor
    /// instead. Any other argument that does not start with `--` names a file to open at startup.
    ///
    /// # Errors
    ///
    /// Fails when logging, the event loop or the GPU cannot start, or when the editor stopped on a fatal error.
    pub fn run_native() -> Result<()> {
        color_eyre::install()?;
        if std::env::args().any(|argument| argument == "--mcp-stdio") {
            // Bridge mode keeps stdout clean for the MCP protocol: no window, no logging.
            return mcp::run_stdio_bridge();
        }
        let _log_guard = logging::init().wrap_err("cannot start logging")?;
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Rustium");
        let event_loop = EventLoop::with_user_event()
            .build()
            .wrap_err("cannot create the event loop")?;
        // A file named on the command line (a file association, or `rustium scene.ruv`) is opened at startup.
        let open_path = std::env::args_os()
            .skip(1)
            .find(|argument| !argument.to_string_lossy().starts_with("--"))
            .map(std::path::PathBuf::from);
        let mut app = app::App::new(settings::Settings::load(), event_loop.create_proxy(), open_path);
        event_loop.run_app(&mut app).wrap_err("event loop failed")?;
        app.into_result()
    }
}
