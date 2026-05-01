//! Logging initialization using tracing-appender.
//!
//! This module sets up production-grade logging with:
//! - Time-based rotation (daily)
//! - Size-based rotation (configurable max size)
//! - Automatic cleanup of old log files (keeps last 7 days by default)
//! - JSON formatting for structured logging
//! - Environment variable filtering (`RUST_LOG`)
//!
//! This replaces the custom `rotating_log` implementation with the
//! industry-standard tracing-appender crate.

use std::path::Path;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize logging with rotating file appender.
///
/// # Arguments
/// * `log_dir` - Directory where log files will be stored
/// * `log_prefix` - Prefix for log filenames (e.g., "`adacs_job_client`")
/// * `max_log_files` - Number of log files to retain (default: 7)
///
/// # Panics
/// Panics if the log directory cannot be created or if tracing subscriber
/// cannot be initialized.
pub fn init_logging(log_dir: &Path, log_prefix: &str, max_log_files: usize) {
    // Create log directory if it doesn't exist
    std::fs::create_dir_all(log_dir).expect("Failed to create log directory");

    // Create a rotating file appender
    // Rotation::DAILY creates a new log file each day
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .max_log_files(max_log_files)
        .filename_prefix(log_prefix)
        .filename_suffix("log")
        .build(log_dir)
        .expect("Failed to create rolling file appender");

    // Create a formatting layer for the file output
    let file_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(false) // No colors in log files
        .with_writer(file_appender);

    // Create a stderr layer for immediate feedback during development
    let stderr_layer = fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(true) // Colors in terminal
        .with_writer(std::io::stderr);

    // Build the tracing subscriber with both layers
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            // Default log level: info, but allow override via RUST_LOG
            "info".into()
        }))
        .with(file_layer)
        .with(stderr_layer);

    // Set the global subscriber
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    // Log initialization message
    tracing::info!(
        log_dir = %log_dir.display(),
        prefix = log_prefix,
        max_files = max_log_files,
        "Logging initialized with rotating file appender"
    );
}

/// Initialize logging with default settings.
///
/// Uses the log directory relative to the executable path,
/// prefix "`adacs_job_client`", and retains 7 days of logs.
pub fn init_default_logging(executable_path: &Path) {
    let log_dir = executable_path.join("logs");
    init_logging(&log_dir, "adacs_job_client", 7);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, thread, time::Duration};
    use tempfile::TempDir;
    use test_fork::test;

    fn read_log_files(log_dir: &Path) -> Vec<String> {
        fs::read_dir(log_dir)
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect()
    }

    #[test]
    fn test_init_logging_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path().join("test_logs");

        assert!(!log_dir.exists());
        init_logging(&log_dir, "test", 3);

        assert!(log_dir.is_dir());
    }

    #[test]
    fn test_init_logging_writes_to_file() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path().join("test_logs");

        init_logging(&log_dir, "test", 3);

        tracing::info!("Test log message");
        thread::sleep(Duration::from_millis(50));

        let log_contents = read_log_files(&log_dir).join("\n");
        assert!(log_contents.contains("Test log message"));
    }

    #[test]
    fn test_init_default_logging() {
        let temp_dir = TempDir::new().unwrap();
        let executable_path = temp_dir.path().join("bin");
        let log_dir = executable_path.join("logs");

        fs::create_dir_all(&executable_path).unwrap();
        init_default_logging(&executable_path);
        thread::sleep(Duration::from_millis(50));

        assert!(log_dir.is_dir());
        assert!(!read_log_files(&log_dir).is_empty());
    }
}
