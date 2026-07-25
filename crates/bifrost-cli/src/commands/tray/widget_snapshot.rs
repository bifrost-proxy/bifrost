use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

const APP_GROUP_IDENTIFIER: &str = "group.com.bifrost.desktop";
const SNAPSHOT_FILE_NAME: &str = "status.json";
const SNAPSHOT_SCHEMA_VERSION: u8 = 1;
const SNAPSHOT_PUBLISH_INTERVAL: Duration = Duration::from_secs(5);
const WIDGET_RELOAD_INTERVAL: Duration = Duration::from_secs(60);
const GROUP_CONTAINER_OVERRIDE_ENV: &str = "BIFROST_WIDGET_GROUP_CONTAINER";
const WIDGET_EXTENSION_IDENTIFIER: &str = "com.bifrost.desktop.status-widget";
const WIDGET_RELOADER_EXECUTABLE: &str = "bifrost-widget-reloader";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetMetrics {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_percent: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetProxyStatus {
    On,
    Off,
    Checking,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetStatusSnapshot {
    pub schema_version: u8,
    pub sampled_at_ms: u64,
    pub cpu_percent: Option<f32>,
    pub memory_percent: Option<f32>,
    pub disk_percent: Option<f32>,
    pub proxy_status: WidgetProxyStatus,
}

impl WidgetStatusSnapshot {
    pub fn from_metrics(
        sampled_at_ms: u64,
        metrics: WidgetMetrics,
        proxy_status: WidgetProxyStatus,
    ) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            sampled_at_ms,
            cpu_percent: normalized_percent(metrics.cpu_percent),
            memory_percent: usage_percent(metrics.memory_used_bytes, metrics.memory_total_bytes),
            disk_percent: metrics.disk_used_percent.and_then(normalized_percent),
            proxy_status,
        }
    }
}

#[derive(Debug)]
pub struct WidgetSnapshotPublisher {
    output_dirs: Vec<PathBuf>,
    reloader_path: Option<PathBuf>,
    last_publish_at: Option<Instant>,
    last_reload_at: Option<Instant>,
    last_proxy_status: Option<WidgetProxyStatus>,
}

impl WidgetSnapshotPublisher {
    pub fn new() -> Self {
        Self {
            output_dirs: widget_snapshot_output_dirs(),
            reloader_path: widget_reloader_path(),
            last_publish_at: None,
            last_reload_at: None,
            last_proxy_status: None,
        }
    }

    #[cfg(test)]
    fn for_output_dir(output_dir: PathBuf) -> Self {
        Self::for_output_dirs(vec![output_dir])
    }

    #[cfg(test)]
    fn for_output_dirs(output_dirs: Vec<PathBuf>) -> Self {
        Self {
            output_dirs,
            reloader_path: None,
            last_publish_at: None,
            last_reload_at: None,
            last_proxy_status: None,
        }
    }

    pub fn publish_if_needed(
        &mut self,
        now: Instant,
        sampled_at_ms: u64,
        metrics: WidgetMetrics,
        proxy_status: WidgetProxyStatus,
    ) -> Result<bool, String> {
        if self.output_dirs.is_empty() {
            return Ok(false);
        }
        if !should_publish(
            self.last_publish_at,
            self.last_proxy_status,
            now,
            proxy_status,
        ) {
            return Ok(false);
        }

        let snapshot = WidgetStatusSnapshot::from_metrics(sampled_at_ms, metrics, proxy_status);
        let mut published = false;
        let mut errors = Vec::new();
        for output_dir in &self.output_dirs {
            match write_snapshot_atomically(output_dir, &snapshot) {
                Ok(()) => published = true,
                Err(error) => errors.push(error),
            }
        }
        if !published {
            return Err(errors.join("; "));
        }
        if !errors.is_empty() {
            tracing::debug!(
                errors = ?errors,
                "macOS status widget snapshot was only published to some destinations"
            );
        }
        let reload_needed = should_reload_widget(
            self.last_reload_at,
            self.last_proxy_status,
            now,
            proxy_status,
        );
        self.last_publish_at = Some(now);
        self.last_proxy_status = Some(proxy_status);
        if reload_needed {
            if let Some(reloader_path) = &self.reloader_path {
                match request_widget_reload(reloader_path) {
                    Ok(()) => self.last_reload_at = Some(now),
                    Err(error) => tracing::debug!(
                        %error,
                        "failed to request an immediate macOS status widget reload"
                    ),
                }
            }
        }
        Ok(true)
    }
}

fn normalized_percent(value: f32) -> Option<f32> {
    value.is_finite().then(|| value.clamp(0.0, 100.0))
}

fn usage_percent(used: u64, total: u64) -> Option<f32> {
    if total == 0 {
        return None;
    }
    normalized_percent((used as f64 * 100.0 / total as f64) as f32)
}

fn should_publish(
    last_publish_at: Option<Instant>,
    last_proxy_status: Option<WidgetProxyStatus>,
    now: Instant,
    proxy_status: WidgetProxyStatus,
) -> bool {
    last_publish_at.is_none()
        || last_proxy_status != Some(proxy_status)
        || last_publish_at
            .is_some_and(|last| now.saturating_duration_since(last) >= SNAPSHOT_PUBLISH_INTERVAL)
}

fn should_reload_widget(
    last_reload_at: Option<Instant>,
    last_proxy_status: Option<WidgetProxyStatus>,
    now: Instant,
    proxy_status: WidgetProxyStatus,
) -> bool {
    last_reload_at.is_none()
        || last_proxy_status != Some(proxy_status)
        || last_reload_at
            .is_some_and(|last| now.saturating_duration_since(last) >= WIDGET_RELOAD_INTERVAL)
}

#[cfg(target_os = "macos")]
fn widget_reloader_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .map(|directory| directory.join(WIDGET_RELOADER_EXECUTABLE))
        .filter(|path| path.is_file())
}

#[cfg(not(target_os = "macos"))]
fn widget_reloader_path() -> Option<PathBuf> {
    None
}

fn request_widget_reload(reloader_path: &Path) -> Result<(), String> {
    let mut child = Command::new(reloader_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            format!(
                "failed to launch widget reload helper {}: {error}",
                reloader_path.display()
            )
        })?;
    let helper = reloader_path.to_path_buf();
    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::debug!(
            helper = %helper.display(),
            %status,
            "macOS status widget reload helper exited unsuccessfully"
        ),
        Err(error) => tracing::debug!(
            helper = %helper.display(),
            %error,
            "failed to reap macOS status widget reload helper"
        ),
    });
    Ok(())
}

fn widget_group_container_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(GROUP_CONTAINER_OVERRIDE_ENV) {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }

    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| {
            home.join("Library")
                .join("Group Containers")
                .join(APP_GROUP_IDENTIFIER)
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn widget_snapshot_output_dirs() -> Vec<PathBuf> {
    let mut output_dirs = widget_group_container_path()
        .into_iter()
        .collect::<Vec<_>>();

    #[cfg(target_os = "macos")]
    if std::env::var_os(GROUP_CONTAINER_OVERRIDE_ENV).is_none() {
        if let Some(home) = dirs::home_dir() {
            output_dirs.push(
                home.join("Library")
                    .join("Containers")
                    .join(WIDGET_EXTENSION_IDENTIFIER)
                    .join("Data")
                    .join("Library")
                    .join("Application Support")
                    .join("Bifrost"),
            );
        }
    }

    output_dirs
}

fn write_snapshot_atomically(
    output_dir: &Path,
    snapshot: &WidgetStatusSnapshot,
) -> Result<(), String> {
    fs::create_dir_all(output_dir).map_err(|error| {
        format!(
            "failed to create widget group container {}: {error}",
            output_dir.display()
        )
    })?;
    let destination = output_dir.join(SNAPSHOT_FILE_NAME);
    let temporary = output_dir.join(format!(
        ".{SNAPSHOT_FILE_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let content = serde_json::to_vec(snapshot)
        .map_err(|error| format!("failed to encode widget snapshot: {error}"))?;

    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&content)?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "failed to publish widget snapshot {}: {error}",
            destination.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> WidgetMetrics {
        WidgetMetrics {
            cpu_percent: 24.5,
            memory_used_bytes: 6,
            memory_total_bytes: 8,
            disk_used_percent: Some(52.25),
        }
    }

    #[test]
    fn snapshot_normalizes_percentages_and_preserves_unknown_values() {
        let snapshot = WidgetStatusSnapshot::from_metrics(
            123,
            WidgetMetrics {
                cpu_percent: 120.0,
                memory_used_bytes: 9,
                memory_total_bytes: 8,
                disk_used_percent: Some(f32::NAN),
            },
            WidgetProxyStatus::On,
        );

        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.sampled_at_ms, 123);
        assert_eq!(snapshot.cpu_percent, Some(100.0));
        assert_eq!(snapshot.memory_percent, Some(100.0));
        assert_eq!(snapshot.disk_percent, None);
        assert_eq!(snapshot.proxy_status, WidgetProxyStatus::On);
    }

    #[test]
    fn snapshot_reports_unknown_memory_when_total_is_zero() {
        let snapshot = WidgetStatusSnapshot::from_metrics(
            456,
            WidgetMetrics {
                memory_total_bytes: 0,
                ..metrics()
            },
            WidgetProxyStatus::Checking,
        );

        assert_eq!(snapshot.memory_percent, None);
        assert_eq!(snapshot.proxy_status, WidgetProxyStatus::Checking);
    }

    #[test]
    fn publisher_throttles_regular_updates_but_publishes_proxy_changes() {
        let temp = tempfile::tempdir().unwrap();
        let mut publisher = WidgetSnapshotPublisher::for_output_dir(temp.path().to_path_buf());
        let start = Instant::now();

        assert!(publisher
            .publish_if_needed(start, 1, metrics(), WidgetProxyStatus::Off)
            .unwrap());
        assert!(!publisher
            .publish_if_needed(
                start + Duration::from_secs(4),
                2,
                metrics(),
                WidgetProxyStatus::Off,
            )
            .unwrap());
        assert!(publisher
            .publish_if_needed(
                start + Duration::from_secs(4),
                3,
                metrics(),
                WidgetProxyStatus::On,
            )
            .unwrap());
        assert!(publisher
            .publish_if_needed(
                start + Duration::from_secs(9),
                4,
                metrics(),
                WidgetProxyStatus::On,
            )
            .unwrap());
    }

    #[test]
    fn widget_reload_is_minute_throttled_but_proxy_changes_are_immediate() {
        let start = Instant::now();

        assert!(should_reload_widget(
            None,
            None,
            start,
            WidgetProxyStatus::Off
        ));
        assert!(!should_reload_widget(
            Some(start),
            Some(WidgetProxyStatus::Off),
            start + Duration::from_secs(59),
            WidgetProxyStatus::Off,
        ));
        assert!(should_reload_widget(
            Some(start),
            Some(WidgetProxyStatus::Off),
            start + Duration::from_secs(10),
            WidgetProxyStatus::On,
        ));
        assert!(should_reload_widget(
            Some(start),
            Some(WidgetProxyStatus::On),
            start + Duration::from_secs(60),
            WidgetProxyStatus::On,
        ));
    }

    #[test]
    fn missing_widget_reload_helper_returns_an_error_without_panicking() {
        let temp = tempfile::tempdir().unwrap();
        let error = request_widget_reload(&temp.path().join("missing-helper")).unwrap_err();

        assert!(error.contains("failed to launch widget reload helper"));
    }

    #[test]
    fn publisher_writes_atomic_json_without_leaving_temp_files() {
        let temp = tempfile::tempdir().unwrap();
        let mut publisher = WidgetSnapshotPublisher::for_output_dir(temp.path().to_path_buf());

        assert!(publisher
            .publish_if_needed(
                Instant::now(),
                1_780_000_000_000,
                metrics(),
                WidgetProxyStatus::Unsupported,
            )
            .unwrap());

        let content = fs::read_to_string(temp.path().join(SNAPSHOT_FILE_NAME)).unwrap();
        let decoded: WidgetStatusSnapshot = serde_json::from_str(&content).unwrap();
        assert_eq!(decoded.sampled_at_ms, 1_780_000_000_000);
        assert_eq!(decoded.cpu_percent, Some(24.5));
        assert_eq!(decoded.memory_percent, Some(75.0));
        assert_eq!(decoded.disk_percent, Some(52.25));
        assert_eq!(decoded.proxy_status, WidgetProxyStatus::Unsupported);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn publisher_mirrors_the_same_snapshot_to_every_output_directory() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut publisher = WidgetSnapshotPublisher::for_output_dirs(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);

        assert!(publisher
            .publish_if_needed(
                Instant::now(),
                1_780_000_000_000,
                metrics(),
                WidgetProxyStatus::On,
            )
            .unwrap());

        let first_content = fs::read(first.path().join(SNAPSHOT_FILE_NAME)).unwrap();
        let second_content = fs::read(second.path().join(SNAPSHOT_FILE_NAME)).unwrap();
        assert_eq!(first_content, second_content);
    }

    #[test]
    fn publisher_succeeds_when_at_least_one_output_directory_is_writable() {
        let invalid_parent = tempfile::NamedTempFile::new().unwrap();
        let valid = tempfile::tempdir().unwrap();
        let mut publisher = WidgetSnapshotPublisher::for_output_dirs(vec![
            invalid_parent.path().join("not-a-directory"),
            valid.path().to_path_buf(),
        ]);

        assert!(publisher
            .publish_if_needed(
                Instant::now(),
                1_780_000_000_000,
                metrics(),
                WidgetProxyStatus::Off,
            )
            .unwrap());
        assert!(valid.path().join(SNAPSHOT_FILE_NAME).is_file());
    }
}
