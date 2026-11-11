//! Rustium: a desktop editor for voxel scenes.
//!
//! This binary opens a frameless window, renders the Scene with `rustium-render` and draws the viewport-first
//! interface with egui. Scene changes always go through `rustium_core::Command`s.
#![warn(clippy::panic, clippy::expect_used)]

mod app;
mod camera;
mod editor;
mod hud;
mod i18n;
mod icons;
mod logging;
mod render_mode;
mod render_output;
mod settings;
mod settings_window;
mod splash;
mod theme;

use color_eyre::eyre::{Result, WrapErr};
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    color_eyre::install()?;
    let _log_guard = logging::init().wrap_err("cannot start logging")?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Rustium");
    let event_loop = EventLoop::new().wrap_err("cannot create the event loop")?;
    let mut app = app::App::new(settings::Settings::load());
    event_loop.run_app(&mut app).wrap_err("event loop failed")?;
    app.into_result()
}
