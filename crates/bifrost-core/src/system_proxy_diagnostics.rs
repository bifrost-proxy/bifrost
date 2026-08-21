use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::{lock_contended_error, FileExt};
use serde::{Deserialize, Serialize};

use crate::{BifrostError, ProxyBackup, ResourcePressureLevel, Result};

const OWNER_STATE_FILE: &str = "system_proxy_owner_state.json";
const DIAGNOSTICS_LOCK_FILE: &str = ".system_proxy_diagnostics.lock";
const EVENT_FILE: &str = "system_proxy_events.jsonl";
const EVENT_ROTATE_BYTES: u64 = 10 * 1024 * 1024;
const EVENT_ROTATIONS: usize = 3;
const DIAGNOSTICS_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const DIAGNOSTICS_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemProxyOwnerState {
    pub schema_version: u32,
    pub ownership_generation: Option<String>,
    pub pid: Option<u32>,
    pub started_at_ms: Option<u64>,
    pub runtime_start_mode: Option<String>,
    pub restartable_runtime: bool,
    pub listener_addr: Option<String>,
    pub health_port: Option<u16>,
    pub expected_proxy: Option<ProxyBackup>,
    pub helper_pid: Option<u32>,
    pub helper_started_at_ms: Option<u64>,
    pub helper_last_heartbeat_at: Option<String>,
    pub recovery_mode: Option<String>,
    pub recovery_grace_secs: Option<u64>,
    pub phase: Option<String>,
    pub pressure: ResourcePressureLevel,
    pub last_action: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemProxyLifecycleEvent {
    pub schema_version: u32,
    pub ts: String,
    pub event: String,
    pub component: String,
    pub trigger: Option<String>,
    pub decision: Option<String>,
    pub error: Option<String>,
    pub old_pid: Option<u32>,
    pub new_pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<i32>,
    pub admin_probe_ms: Option<u64>,
    pub data_plane_probe_ms: Option<u64>,
    pub health_lane_probe_ms: Option<u64>,
    pub scheduler_heartbeat_age_ms: Option<u64>,
    pub rss_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
    pub fd_count: Option<u64>,
    pub fd_limit: Option<u64>,
    pub active_connections: Option<u64>,
    pub queue_depth: Option<u64>,
    pub queue_capacity: Option<u64>,
    pub ownership_generation: Option<String>,
    pub system_proxy_action: Option<String>,
    pub recovery_elapsed_ms: Option<u64>,
}

impl SystemProxyLifecycleEvent {
    pub fn new(event: impl Into<String>, component: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            ts: chrono::Utc::now().to_rfc3339(),
            event: event.into(),
            component: component.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug)]
struct DiagnosticsLock {
    file: File,
}

impl Drop for DiagnosticsLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn acquire_lock(data_dir: &Path) -> Result<DiagnosticsLock> {
    acquire_lock_with_timeout(data_dir, DIAGNOSTICS_LOCK_TIMEOUT)
}

fn acquire_lock_with_timeout(data_dir: &Path, timeout: Duration) -> Result<DiagnosticsLock> {
    fs::create_dir_all(data_dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data_dir.join(DIAGNOSTICS_LOCK_FILE))?;
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(DiagnosticsLock { file }),
            Err(error) if error.raw_os_error() == lock_contended_error().raw_os_error() => {
                if started.elapsed() >= timeout {
                    return Err(BifrostError::Config(format!(
                        "timed out after {} ms waiting for lifecycle diagnostics lock",
                        timeout.as_millis()
                    )));
                }
                std::thread::sleep(std::cmp::min(
                    DIAGNOSTICS_LOCK_POLL_INTERVAL,
                    timeout.saturating_sub(started.elapsed()),
                ));
            }
            Err(error) => return Err(BifrostError::Io(error)),
        }
    }
}

fn owner_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(OWNER_STATE_FILE)
}

fn event_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join(EVENT_FILE)
}

pub fn read_system_proxy_owner_state(data_dir: &Path) -> Result<Option<SystemProxyOwnerState>> {
    let path = owner_state_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| BifrostError::Config(format!("invalid system proxy owner state: {error}")))
}

pub fn update_system_proxy_owner_state<F>(
    data_dir: &Path,
    update: F,
) -> Result<SystemProxyOwnerState>
where
    F: FnOnce(&mut SystemProxyOwnerState),
{
    let _lock = acquire_lock(data_dir)?;
    let path = owner_state_path(data_dir);
    let mut state = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?).unwrap_or_default()
    } else {
        SystemProxyOwnerState::default()
    };
    state.schema_version = 1;
    update(&mut state);
    state.updated_at = Some(chrono::Utc::now().to_rfc3339());
    let parent = path.parent().unwrap_or(data_dir);
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, &state)
        .map_err(|error| BifrostError::Config(format!("serialize owner state: {error}")))?;
    temp.as_file_mut().sync_all()?;
    temp.persist(&path)
        .map_err(|error| BifrostError::Io(error.error))?;
    Ok(state)
}

pub fn append_system_proxy_event(data_dir: &Path, event: &SystemProxyLifecycleEvent) -> Result<()> {
    let _lock = acquire_lock(data_dir)?;
    let path = event_path(data_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_events(&path)?;
    let mut line = serde_json::to_vec(event)
        .map_err(|error| BifrostError::Config(format!("serialize lifecycle event: {error}")))?;
    line.push(b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&line)?;
    file.flush()?;
    Ok(())
}

fn rotate_events(path: &Path) -> Result<()> {
    if path.metadata().map(|metadata| metadata.len()).unwrap_or(0) < EVENT_ROTATE_BYTES {
        return Ok(());
    }
    for index in (1..=EVENT_ROTATIONS).rev() {
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            PathBuf::from(format!("{}.{}", path.display(), index - 1))
        };
        let target = PathBuf::from(format!("{}.{}", path.display(), index));
        if source.exists() {
            if target.exists() {
                fs::remove_file(&target)?;
            }
            fs::rename(source, target)?;
        }
    }
    Ok(())
}

pub fn read_recent_system_proxy_events(
    data_dir: &Path,
    limit: usize,
) -> Result<Vec<SystemProxyLifecycleEvent>> {
    let path = event_path(data_dir);
    if !path.exists() || limit == 0 {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if let Ok(event) = serde_json::from_str(&line) {
            events.push(event);
            if events.len() > limit {
                events.remove(0);
            }
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_state_updates_atomically_and_events_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        update_system_proxy_owner_state(temp.path(), |state| {
            state.pid = Some(42);
            state.phase = Some("running".into());
        })
        .unwrap();
        let state = read_system_proxy_owner_state(temp.path()).unwrap().unwrap();
        assert_eq!(state.pid, Some(42));
        assert_eq!(state.phase.as_deref(), Some("running"));

        let mut event = SystemProxyLifecycleEvent::new("runtime_started", "test");
        event.new_pid = Some(42);
        append_system_proxy_event(temp.path(), &event).unwrap();
        let events = read_recent_system_proxy_events(temp.path(), 10).unwrap();
        assert_eq!(events, vec![event]);

        for index in 1..=2 {
            let event = SystemProxyLifecycleEvent::new(format!("event_{index}"), "test");
            append_system_proxy_event(temp.path(), &event).unwrap();
        }
        let recent = read_recent_system_proxy_events(temp.path(), 2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].event, "event_1");
        assert_eq!(recent[1].event, "event_2");
    }

    #[test]
    fn diagnostics_lock_wait_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let _held = acquire_lock(temp.path()).unwrap();
        let started = Instant::now();
        let error = acquire_lock_with_timeout(temp.path(), Duration::from_millis(40))
            .expect_err("a contended diagnostics lock must time out");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(error.to_string().contains("timed out after 40 ms"));
    }

    #[test]
    fn lifecycle_events_rotate_before_append() {
        let temp = tempfile::tempdir().unwrap();
        let path = event_path(temp.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = File::create(&path).unwrap();
        file.set_len(EVENT_ROTATE_BYTES).unwrap();
        fs::write(format!("{}.2", path.display()), "older").unwrap();
        fs::write(format!("{}.3", path.display()), "oldest").unwrap();

        let event = SystemProxyLifecycleEvent::new("runtime_recovered", "test");
        append_system_proxy_event(temp.path(), &event).unwrap();

        assert!(PathBuf::from(format!("{}.1", path.display())).exists());
        assert_eq!(
            read_recent_system_proxy_events(temp.path(), 10).unwrap(),
            vec![event]
        );
    }
}
