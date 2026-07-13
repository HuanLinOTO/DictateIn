use std::fs;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::paths::AppPaths;

pub fn initialize() -> Result<WorkerGuard> {
    let paths = AppPaths::discover()?;
    paths.ensure_directories()?;
    let application_root = paths.root.clone();
    let log_directory = paths.logs;
    fs::create_dir_all(&log_directory)?;
    remove_expired_logs(&log_directory)?;
    let file_appender = tracing_appender::rolling::daily(&log_directory, "dictate-in.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "dictate_in=info".into());

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_writer(file_writer),
        )
        .init();
    tracing::info!(root = %application_root.display(), "logging initialized");
    Ok(guard)
}

fn remove_expired_logs(directory: &std::path::Path) -> Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(7 * 24 * 60 * 60))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let is_application_log = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("dictate-in.log"));
        if is_application_log
            && entry
                .metadata()?
                .modified()
                .is_ok_and(|modified| modified < cutoff)
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
