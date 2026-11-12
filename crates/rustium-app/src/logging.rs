#[cfg(not(target_arch = "wasm32"))]
use std::str::FromStr;

use color_eyre::eyre::{Result, WrapErr};
#[cfg(not(target_arch = "wasm32"))]
use tracing_appender::non_blocking::WorkerGuard;
#[cfg(not(target_arch = "wasm32"))]
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

#[cfg(not(target_arch = "wasm32"))]
use crate::settings;

/// `info` and above, with wgpu's backend layer from `warn`.
fn default_filter() -> Targets {
    Targets::new()
        .with_default(tracing::Level::INFO)
        .with_target("wgpu_hal", tracing::Level::WARN)
}

/// Daily rotating log files (7 kept) in the local data dir, plus the console in debug builds.
///
/// The filter defaults to `info` and can be overridden with `RUST_LOG` (e.g. `rustium=debug,wgpu=warn`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn init() -> Result<WorkerGuard> {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| Targets::from_str(&value).ok())
        .unwrap_or_else(default_filter);
    let directory = settings::data_dir().join("logs");
    // The appender prunes old files while it is built; without the directory it prints an error on the first start.
    std::fs::create_dir_all(&directory)
        .wrap_err_with(|| format!("cannot create log directory {}", directory.display()))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("rustium")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&directory)
        .wrap_err_with(|| format!("cannot open log directory {}", directory.display()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(filter.clone());
    let console_layer = cfg!(debug_assertions).then(|| fmt::layer().with_filter(filter));
    tracing_subscriber::registry()
        .with(file_layer)
        .with(console_layer)
        .with(tracing_error::ErrorLayer::default())
        .try_init()
        .wrap_err("a tracing subscriber is already installed")?;
    Ok(guard)
}

/// The browser's developer console, with the default filter.
#[cfg(target_arch = "wasm32")]
pub(crate) fn init() -> Result<()> {
    let console = fmt::layer()
        .with_ansi(false)
        // The browser has no clock `std` can read; the console stamps each line itself.
        .without_time()
        .with_writer(tracing_web::MakeWebConsoleWriter::new())
        .with_filter(default_filter());
    tracing_subscriber::registry()
        .with(console)
        .with(tracing_error::ErrorLayer::default())
        .try_init()
        .wrap_err("a tracing subscriber is already installed")
}
