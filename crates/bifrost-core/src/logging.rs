use std::{path::PathBuf, sync::OnceLock};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::error::{BifrostError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogOutput {
    Console,
    File,
}

impl LogOutput {
    pub fn parse(s: &str) -> Vec<LogOutput> {
        s.split(',')
            .filter_map(|part| match part.trim().to_lowercase().as_str() {
                "console" => Some(LogOutput::Console),
                "file" => Some(LogOutput::File),
                "both" => None,
                _ => None,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .chain(if s.to_lowercase().contains("both") {
                vec![LogOutput::Console, LogOutput::File]
            } else {
                vec![]
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub level: String,
    pub outputs: Vec<LogOutput>,
    pub log_dir: PathBuf,
    pub retention_days: u32,
    pub file_prefix: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            outputs: vec![LogOutput::Console, LogOutput::File],
            log_dir: PathBuf::from("."),
            retention_days: 7,
            file_prefix: "bifrost".to_string(),
        }
    }
}

impl LogConfig {
    pub fn new(level: String, log_dir: PathBuf) -> Self {
        Self {
            level,
            log_dir,
            ..Default::default()
        }
    }

    pub fn with_outputs(mut self, outputs: Vec<LogOutput>) -> Self {
        self.outputs = outputs;
        self
    }

    pub fn with_retention_days(mut self, days: u32) -> Self {
        self.retention_days = days;
        self
    }
}

pub struct LogGuard {
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

static DAEMON_LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

fn build_env_filter(level: &str) -> Result<EnvFilter> {
    if std::env::var("RUST_LOG").is_ok() {
        Ok(EnvFilter::from_default_env())
    } else {
        EnvFilter::try_new(level)
            .map_err(|e| BifrostError::Config(format!("Invalid log level '{}': {}", level, e)))
    }
}

fn cleanup_old_logs(log_dir: &std::path::Path, prefix: &str, retention_days: u32) -> Result<()> {
    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("Failed to read log directory for cleanup: {}", e);
            return Ok(());
        }
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);

    for entry in entries.flatten() {
        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with(prefix) && name.ends_with(".log") {
                if let Some(date_str) = extract_date_from_filename(name, prefix) {
                    if let Ok(file_date) = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                    {
                        if file_date < cutoff.date_naive() {
                            tracing::info!("Removing old log file: {}", name);
                            if let Err(e) = std::fs::remove_file(&path) {
                                tracing::warn!("Failed to remove old log file {}: {}", name, e);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn extract_date_from_filename(filename: &str, prefix: &str) -> Option<String> {
    let without_prefix = filename.strip_prefix(prefix)?;
    let without_dot = without_prefix.strip_prefix('.')?;
    let date_part = without_dot.strip_suffix(".log")?;
    Some(date_part.to_string())
}

fn extract_date_from_suffix(filename: &str, base_name: &str) -> Option<String> {
    let prefix = format!("{base_name}.");
    let without_prefix = filename.strip_prefix(&prefix)?;
    Some(without_prefix.to_string())
}

fn start_log_cleanup_thread(log_dir: PathBuf, prefix: String, retention_days: u32) {
    if retention_days == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(24 * 60 * 60));
        let _ = cleanup_old_logs(&log_dir, &prefix, retention_days);
    });
}

pub fn init_logging(level: &str) -> Result<()> {
    let filter = build_env_filter(level)?;

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .try_init()
        .map_err(|e| BifrostError::Config(format!("Failed to initialize logging: {}", e)))?;

    let default_config = LogConfig::default();
    if let Err(e) = cleanup_old_logs(
        &default_config.log_dir,
        &default_config.file_prefix,
        default_config.retention_days,
    ) {
        tracing::warn!("Failed to cleanup old logs: {}", e);
    }
    start_log_cleanup_thread(
        default_config.log_dir,
        default_config.file_prefix,
        default_config.retention_days,
    );

    Ok(())
}

fn rotate_file_if_day_changed(
    log_dir: &std::path::Path,
    base_name: &str,
    today: chrono::NaiveDate,
) {
    let current = log_dir.join(base_name);
    if !current.exists() {
        return;
    }
    let modified_date = current
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).date_naive());
    if let Some(date) = modified_date {
        if date != today {
            let rotated = log_dir.join(format!("{base_name}.{}", date.format("%Y-%m-%d")));
            let _ = std::fs::rename(&current, rotated);
        }
    }
}

fn cleanup_rotated_files(
    log_dir: &std::path::Path,
    base_name: &str,
    retention_days: u32,
) -> Result<()> {
    if retention_days == 0 {
        return Ok(());
    }
    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("Failed to read log directory for cleanup: {}", e);
            return Ok(());
        }
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(date_str) = extract_date_from_suffix(name, base_name) {
                if let Ok(file_date) = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                    if file_date < cutoff.date_naive() {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
    }

    Ok(())
}

pub fn rotate_daemon_err_log(log_dir: &std::path::Path, retention_days: u32) -> Result<()> {
    std::fs::create_dir_all(log_dir).map_err(|e| {
        BifrostError::Config(format!(
            "Failed to create log directory '{}': {}",
            log_dir.display(),
            e
        ))
    })?;
    let today = chrono::Utc::now().date_naive();
    rotate_file_if_day_changed(log_dir, "bifrost.err", today);
    cleanup_rotated_files(log_dir, "bifrost.err", retention_days)?;
    Ok(())
}

pub fn init_logging_with_config(config: &LogConfig) -> Result<LogGuard> {
    let filter = build_env_filter(&config.level)?;

    let has_console = config.outputs.contains(&LogOutput::Console);
    let has_file = config.outputs.contains(&LogOutput::File);

    let mut file_guard: Option<tracing_appender::non_blocking::WorkerGuard> = None;

    if has_file {
        std::fs::create_dir_all(&config.log_dir).map_err(|e| {
            BifrostError::Config(format!(
                "Failed to create log directory '{}': {}",
                config.log_dir.display(),
                e
            ))
        })?;
    }

    match (has_console, has_file) {
        (true, true) => {
            let file_appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix(&config.file_prefix)
                .filename_suffix("log")
                .max_log_files(config.retention_days as usize)
                .build(&config.log_dir)
                .map_err(|e| {
                    BifrostError::Config(format!("Failed to create file appender: {}", e))
                })?;

            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            file_guard = Some(guard);

            let console_layer = fmt::layer()
                .with_target(true)
                .with_file(true)
                .with_line_number(true);

            let file_layer = fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_file(true)
                .with_line_number(true);

            tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .with(file_layer)
                .try_init()
                .map_err(|e| {
                    BifrostError::Config(format!("Failed to initialize logging: {}", e))
                })?;

            if let Err(e) =
                cleanup_old_logs(&config.log_dir, &config.file_prefix, config.retention_days)
            {
                tracing::warn!("Failed to cleanup old logs: {}", e);
            }
            start_log_cleanup_thread(
                config.log_dir.clone(),
                config.file_prefix.clone(),
                config.retention_days,
            );
        }
        (true, false) => {
            let console_layer = fmt::layer()
                .with_target(true)
                .with_file(true)
                .with_line_number(true);

            tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .try_init()
                .map_err(|e| {
                    BifrostError::Config(format!("Failed to initialize logging: {}", e))
                })?;
        }
        (false, true) => {
            let file_appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix(&config.file_prefix)
                .filename_suffix("log")
                .max_log_files(config.retention_days as usize)
                .build(&config.log_dir)
                .map_err(|e| {
                    BifrostError::Config(format!("Failed to create file appender: {}", e))
                })?;

            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            file_guard = Some(guard);

            let file_layer = fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_file(true)
                .with_line_number(true);

            tracing_subscriber::registry()
                .with(filter)
                .with(file_layer)
                .try_init()
                .map_err(|e| {
                    BifrostError::Config(format!("Failed to initialize logging: {}", e))
                })?;

            if let Err(e) =
                cleanup_old_logs(&config.log_dir, &config.file_prefix, config.retention_days)
            {
                tracing::warn!("Failed to cleanup old logs: {}", e);
            }
            start_log_cleanup_thread(
                config.log_dir.clone(),
                config.file_prefix.clone(),
                config.retention_days,
            );
        }
        (false, false) => {
            return Err(BifrostError::Config(
                "At least one log output (console or file) must be specified".to_string(),
            ));
        }
    }

    Ok(LogGuard {
        _file_guard: file_guard,
    })
}

pub fn reinit_logging_for_daemon(
    log_dir: &std::path::Path,
    retention_days: u32,
    level: &str,
) -> Result<()> {
    std::fs::create_dir_all(log_dir).map_err(|e| {
        BifrostError::Config(format!(
            "Failed to create log directory '{}': {}",
            log_dir.display(),
            e
        ))
    })?;

    let filter = build_env_filter(level)?;
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("bifrost")
        .filename_suffix("log")
        .max_log_files(retention_days as usize)
        .build(log_dir)
        .map_err(|e| BifrostError::Config(format!("Failed to create file appender: {}", e)))?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_file(true)
        .with_line_number(true);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .try_init()
        .map_err(|e| {
            BifrostError::Config(format!("Failed to reinitialize logging for daemon: {}", e))
        })?;
    let _ = DAEMON_LOG_GUARD.set(guard);

    let prefix = "bifrost".to_string();
    if let Err(e) = cleanup_old_logs(log_dir, &prefix, retention_days) {
        tracing::warn!("Failed to cleanup old logs: {}", e);
    }
    start_log_cleanup_thread(log_dir.to_path_buf(), prefix, retention_days);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_output_parse() {
        let outputs = LogOutput::parse("console");
        assert!(outputs.contains(&LogOutput::Console));
        assert!(!outputs.contains(&LogOutput::File));

        let outputs = LogOutput::parse("file");
        assert!(!outputs.contains(&LogOutput::Console));
        assert!(outputs.contains(&LogOutput::File));

        let outputs = LogOutput::parse("console,file");
        assert!(outputs.contains(&LogOutput::Console));
        assert!(outputs.contains(&LogOutput::File));

        let outputs = LogOutput::parse("both");
        assert!(outputs.contains(&LogOutput::Console));
        assert!(outputs.contains(&LogOutput::File));
    }

    #[test]
    fn test_extract_date_from_filename() {
        assert_eq!(
            extract_date_from_filename("bifrost.2026-02-22.log", "bifrost"),
            Some("2026-02-22".to_string())
        );
        assert_eq!(
            extract_date_from_filename("bifrost.2026-01-01.log", "bifrost"),
            Some("2026-01-01".to_string())
        );
        assert_eq!(
            extract_date_from_filename("other.2026-02-22.log", "bifrost"),
            None
        );
        assert_eq!(extract_date_from_filename("bifrost.log", "bifrost"), None);
    }
}
