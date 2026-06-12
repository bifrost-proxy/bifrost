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
                "console" => None,
                "file" => Some(LogOutput::File),
                "both" => None,
                _ => None,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .chain(
                if s.to_lowercase().contains("console") || s.to_lowercase().contains("both") {
                    vec![LogOutput::Console, LogOutput::File]
                } else {
                    vec![]
                },
            )
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
            outputs: vec![LogOutput::File],
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
        assert!(outputs.contains(&LogOutput::File));

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

    #[test]
    fn log_output_parse_empty_and_unknown() {
        // Unknown / empty tokens yield no File and no Console.
        assert!(LogOutput::parse("").is_empty());
        assert!(LogOutput::parse("garbage").is_empty());
        // Whitespace + case-insensitivity around "FILE".
        let outputs = LogOutput::parse("  FILE  ");
        assert!(outputs.contains(&LogOutput::File));
        assert!(!outputs.contains(&LogOutput::Console));
    }

    #[test]
    fn log_output_eq_clone_hash() {
        // Exercise derives (PartialEq/Eq/Clone/Hash/Debug).
        let a = LogOutput::File;
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(LogOutput::File, LogOutput::Console);
        let mut set = std::collections::HashSet::new();
        set.insert(LogOutput::File);
        set.insert(LogOutput::File);
        assert_eq!(set.len(), 1);
        assert!(format!("{:?}", LogOutput::Console).contains("Console"));
    }

    #[test]
    fn extract_date_from_suffix_variants() {
        assert_eq!(
            extract_date_from_suffix("bifrost.err.2026-02-22", "bifrost.err"),
            Some("2026-02-22".to_string())
        );
        assert_eq!(extract_date_from_suffix("bifrost.err", "bifrost.err"), None);
        assert_eq!(
            extract_date_from_suffix("other.2026-02-22", "bifrost.err"),
            None
        );
    }

    #[test]
    fn log_config_default_and_builders() {
        let def = LogConfig::default();
        assert_eq!(def.level, "info");
        assert_eq!(def.retention_days, 7);
        assert_eq!(def.file_prefix, "bifrost");
        assert_eq!(def.outputs, vec![LogOutput::File]);

        let cfg = LogConfig::new("debug".to_string(), PathBuf::from("/tmp/x"))
            .with_outputs(vec![LogOutput::Console])
            .with_retention_days(3);
        assert_eq!(cfg.level, "debug");
        assert_eq!(cfg.log_dir, PathBuf::from("/tmp/x"));
        assert_eq!(cfg.outputs, vec![LogOutput::Console]);
        assert_eq!(cfg.retention_days, 3);
        // Debug derive.
        assert!(format!("{cfg:?}").contains("debug"));
    }

    #[test]
    fn build_env_filter_valid_and_invalid() {
        // Ensure RUST_LOG is unset so we exercise the try_new branch.
        let saved = std::env::var("RUST_LOG").ok();
        std::env::remove_var("RUST_LOG");

        assert!(build_env_filter("info").is_ok());
        assert!(build_env_filter("debug").is_ok());
        // A target directive with an invalid level name is rejected by EnvFilter.
        let err = build_env_filter("foo=notalevel");
        assert!(err.is_err());

        // RUST_LOG set -> uses from_default_env, always Ok regardless of level arg.
        std::env::set_var("RUST_LOG", "warn");
        assert!(build_env_filter("anything").is_ok());

        match saved {
            Some(v) => std::env::set_var("RUST_LOG", v),
            None => std::env::remove_var("RUST_LOG"),
        }
    }

    #[test]
    fn cleanup_old_logs_removes_old_keeps_new() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        // Old file (way past retention).
        let old = p.join("bifrost.2000-01-01.log");
        std::fs::write(&old, b"old").unwrap();
        // Recent file (today) should be kept.
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d");
        let recent = p.join(format!("bifrost.{today}.log"));
        std::fs::write(&recent, b"recent").unwrap();
        // Non-matching prefix should be ignored.
        let other = p.join("other.2000-01-01.log");
        std::fs::write(&other, b"other").unwrap();

        cleanup_old_logs(p, "bifrost", 7).unwrap();

        assert!(!old.exists(), "old log should be removed");
        assert!(recent.exists(), "recent log should be kept");
        assert!(other.exists(), "non-matching file should be untouched");
    }

    #[test]
    fn cleanup_old_logs_missing_dir_is_ok() {
        let missing = PathBuf::from("/nonexistent/bifrost/logs/path");
        assert!(cleanup_old_logs(&missing, "bifrost", 7).is_ok());
    }

    #[test]
    fn cleanup_old_logs_removes_old_keeps_recent() {
        let dir = tempfile::tempdir().unwrap();
        // Old dated .log file (well before any retention window).
        let old = dir.path().join("bifrost.2000-01-01.log");
        std::fs::write(&old, b"old").unwrap();
        // Recent dated .log file (today).
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d");
        let recent = dir.path().join(format!("bifrost.{today}.log"));
        std::fs::write(&recent, b"new").unwrap();
        // Unrelated file that must be ignored (wrong prefix/suffix).
        let other = dir.path().join("unrelated.txt");
        std::fs::write(&other, b"keep").unwrap();
        // File with prefix but non-date middle part -> extract returns Some but
        // parse fails, so it is not removed.
        let baddate = dir.path().join("bifrost.notadate.log");
        std::fs::write(&baddate, b"keep").unwrap();

        cleanup_old_logs(dir.path(), "bifrost", 7).unwrap();

        assert!(!old.exists(), "old dated log should be removed");
        assert!(recent.exists(), "recent dated log should remain");
        assert!(other.exists(), "unrelated file should remain");
        assert!(baddate.exists(), "non-date file should remain");
    }

    #[test]
    fn extract_date_from_filename_none_paths() {
        // Wrong prefix.
        assert!(extract_date_from_filename("other.2020-01-01.log", "bifrost").is_none());
        // Missing dot separator after prefix.
        assert!(extract_date_from_filename("bifrost2020.log", "bifrost").is_none());
        // Missing .log suffix.
        assert!(extract_date_from_filename("bifrost.2020-01-01.txt", "bifrost").is_none());
        // Happy path still works.
        assert_eq!(
            extract_date_from_filename("bifrost.2020-01-01.log", "bifrost"),
            Some("2020-01-01".to_string())
        );
    }

    #[test]
    fn cleanup_rotated_files_retention_zero_early_return() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("bifrost.err.2000-01-01");
        std::fs::write(&f, b"x").unwrap();
        cleanup_rotated_files(dir.path(), "bifrost.err", 0).unwrap();
        // Early return -> nothing removed.
        assert!(f.exists());
    }

    #[test]
    fn cleanup_rotated_files_removes_old() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("bifrost.err.2000-01-01");
        std::fs::write(&old, b"x").unwrap();
        let today = chrono::Utc::now().date_naive().format("%Y-%m-%d");
        let recent = dir.path().join(format!("bifrost.err.{today}"));
        std::fs::write(&recent, b"y").unwrap();

        cleanup_rotated_files(dir.path(), "bifrost.err", 7).unwrap();

        assert!(!old.exists());
        assert!(recent.exists());
    }

    #[test]
    fn cleanup_rotated_files_missing_dir_is_ok() {
        let missing = PathBuf::from("/nonexistent/bifrost/rotated/path");
        assert!(cleanup_rotated_files(&missing, "bifrost.err", 7).is_ok());
    }

    #[test]
    fn rotate_file_if_day_changed_no_file_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let today = chrono::Utc::now().date_naive();
        // No current file -> early return, no panic, nothing created.
        rotate_file_if_day_changed(dir.path(), "bifrost.err", today);
        assert!(!dir.path().join("bifrost.err").exists());
    }

    #[test]
    fn rotate_file_if_day_changed_same_day_keeps_file() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("bifrost.err");
        std::fs::write(&current, b"data").unwrap();
        // Today's date matches the freshly-written file's mtime -> not rotated.
        let today = chrono::Utc::now().date_naive();
        rotate_file_if_day_changed(dir.path(), "bifrost.err", today);
        assert!(current.exists());
    }

    #[test]
    fn rotate_file_if_day_changed_rotates_when_day_differs() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("bifrost.err");
        std::fs::write(&current, b"data").unwrap();
        // Pass a future date so the file's mtime (today) != `today` arg -> rotate.
        let future = chrono::Utc::now().date_naive() + chrono::Duration::days(1);
        rotate_file_if_day_changed(dir.path(), "bifrost.err", future);
        // Original moved away.
        assert!(!current.exists());
        // A rotated file with the file's modified date now exists.
        let rotated_count = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("bifrost.err."))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(rotated_count, 1);
    }

    #[test]
    fn rotate_daemon_err_log_creates_dir_and_runs() {
        let base = tempfile::tempdir().unwrap();
        let log_dir = base.path().join("nested").join("logs");
        // Directory does not exist yet -> create_dir_all path.
        assert!(!log_dir.exists());
        rotate_daemon_err_log(&log_dir, 7).unwrap();
        assert!(log_dir.exists());

        // With an existing err log + retention 0 (skips cleanup) still succeeds.
        std::fs::write(log_dir.join("bifrost.err"), b"x").unwrap();
        rotate_daemon_err_log(&log_dir, 0).unwrap();
        assert!(log_dir.exists());
    }

    #[test]
    fn start_log_cleanup_thread_retention_zero_returns() {
        // retention_days == 0 -> early return, no thread spawned, no panic.
        start_log_cleanup_thread(PathBuf::from("/tmp"), "bifrost".to_string(), 0);
    }

    #[test]
    fn init_logging_with_config_no_outputs_errors() {
        let cfg = LogConfig {
            level: "info".to_string(),
            outputs: vec![],
            log_dir: PathBuf::from("."),
            retention_days: 7,
            file_prefix: "bifrost".to_string(),
        };
        let err = init_logging_with_config(&cfg);
        assert!(err.is_err());
        let msg = format!("{}", err.err().unwrap());
        assert!(msg.contains("At least one log output"));
    }

    #[test]
    fn init_logging_with_config_invalid_level_errors() {
        let saved = std::env::var("RUST_LOG").ok();
        std::env::remove_var("RUST_LOG");
        let dir = tempfile::tempdir().unwrap();
        let cfg = LogConfig {
            level: "foo=notalevel".to_string(),
            outputs: vec![LogOutput::File],
            log_dir: dir.path().to_path_buf(),
            retention_days: 7,
            file_prefix: "bifrost".to_string(),
        };
        // Invalid level fails before any subscriber init.
        assert!(init_logging_with_config(&cfg).is_err());
        match saved {
            Some(v) => std::env::set_var("RUST_LOG", v),
            None => std::env::remove_var("RUST_LOG"),
        }
    }
}
