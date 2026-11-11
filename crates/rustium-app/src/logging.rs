use std::str::FromStr;

use color_eyre::eyre::{Result, WrapErr};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

use crate::settings;

/// Daily rotating log files (7 kept) in the local data dir, plus the console in debug builds.
///
/// The filter defaults to `info` and can be overridden with `RUST_LOG` (e.g. `rustium=debug,wgpu=warn`).
pub(crate) fn init() -> Result<WorkerGuard> {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| Targets::from_str(&value).ok())
        .unwrap_or_else(|| {
            Targets::new()
                .with_default(tracing::Level::INFO)
                .with_target("wgpu_hal", tracing::Level::WARN)
        });
    let directory = settings::data_dir().join("logs");
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
