use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bifrost_admin::{MetricsCollector, SharedAsyncTrafficWriter};
use bifrost_core::{
    append_system_proxy_event, publish_resource_pressure, update_system_proxy_owner_state,
    PressureInputs, ResourcePressureController, ResourcePressureLevel, RuntimeHealthSnapshot,
    SystemProxyLifecycleEvent,
};
use sysinfo::{Pid, ProcessesToUpdate, System};

pub const RUNTIME_CONNECTION_LIMIT: u64 = 10_000;
const SCHEDULER_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

struct RuntimeHealthShared {
    scheduler_heartbeat_at_ms: AtomicU64,
    active_connections: AtomicU64,
    queue_depth: AtomicU64,
    queue_capacity: AtomicU64,
    stop: AtomicBool,
}

#[derive(Clone)]
pub struct RuntimeHealthReporter {
    shared: Arc<RuntimeHealthShared>,
}

impl RuntimeHealthReporter {
    pub fn heartbeat(&self) {
        self.shared
            .scheduler_heartbeat_at_ms
            .store(epoch_ms(), Ordering::Release);
    }

    pub fn update_load(
        &self,
        metrics: &Arc<MetricsCollector>,
        traffic_writer: &SharedAsyncTrafficWriter,
    ) {
        self.shared
            .active_connections
            .store(metrics.get_active_connections(), Ordering::Release);
        self.shared
            .queue_depth
            .store(traffic_writer.queue_depth() as u64, Ordering::Release);
        self.shared
            .queue_capacity
            .store(traffic_writer.queue_capacity() as u64, Ordering::Release);
    }
}

pub struct RuntimeHealthLane {
    port: u16,
    shared: Arc<RuntimeHealthShared>,
    thread: Option<JoinHandle<()>>,
}

impl RuntimeHealthLane {
    pub fn start(data_dir: PathBuf) -> bifrost_core::Result<(Self, RuntimeHealthReporter)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let shared = Arc::new(RuntimeHealthShared {
            scheduler_heartbeat_at_ms: AtomicU64::new(epoch_ms()),
            active_connections: AtomicU64::new(0),
            queue_depth: AtomicU64::new(0),
            queue_capacity: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("bifrost-runtime-health".to_string())
            .spawn(move || run_health_lane(listener, thread_shared, data_dir))?;
        Ok((
            Self {
                port,
                shared: Arc::clone(&shared),
                thread: Some(thread),
            },
            RuntimeHealthReporter { shared },
        ))
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for RuntimeHealthLane {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        publish_resource_pressure(ResourcePressureLevel::Normal);
    }
}

pub fn spawn_scheduler_heartbeat_task(
    reporter: RuntimeHealthReporter,
    metrics: Arc<MetricsCollector>,
    traffic_writer: SharedAsyncTrafficWriter,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SCHEDULER_HEARTBEAT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            reporter.heartbeat();
            reporter.update_load(&metrics, &traffic_writer);
        }
    })
}

fn run_health_lane(listener: TcpListener, shared: Arc<RuntimeHealthShared>, data_dir: PathBuf) {
    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new_all();
    let mut controller = ResourcePressureController::default();
    let mut snapshot = RuntimeHealthSnapshot {
        pid: std::process::id(),
        connection_limit: RUNTIME_CONNECTION_LIMIT,
        ..RuntimeHealthSnapshot::default()
    };
    let mut last_sample = std::time::Instant::now() - RESOURCE_SAMPLE_INTERVAL;

    while !shared.stop.load(Ordering::Acquire) {
        if last_sample.elapsed() >= RESOURCE_SAMPLE_INTERVAL {
            system.refresh_memory();
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
            snapshot.scheduler_heartbeat_age_ms =
                epoch_ms().saturating_sub(shared.scheduler_heartbeat_at_ms.load(Ordering::Acquire));
            snapshot.active_connections = shared.active_connections.load(Ordering::Acquire);
            snapshot.queue_depth = shared.queue_depth.load(Ordering::Acquire);
            snapshot.queue_capacity = shared.queue_capacity.load(Ordering::Acquire);
            if let Some(process) = system.process(pid) {
                snapshot.rss_bytes = process.memory();
                snapshot.cpu_percent = process.cpu_usage();
            }
            snapshot.fd_count = current_fd_count();
            snapshot.fd_limit = current_fd_limit();

            let observed = pressure_override().unwrap_or_else(|| {
                controller.observe(PressureInputs {
                    rss_bytes: snapshot.rss_bytes,
                    // Available memory includes reclaimable pages on supported
                    // platforms; using it avoids treating file cache as hard
                    // pressure while still reacting to genuine exhaustion.
                    system_used_memory_bytes: system
                        .total_memory()
                        .saturating_sub(system.available_memory()),
                    total_memory_bytes: system.total_memory(),
                    fd_count: snapshot.fd_count,
                    fd_limit: snapshot.fd_limit,
                    active_connections: snapshot.active_connections,
                    connection_limit: snapshot.connection_limit,
                    queue_depth: snapshot.queue_depth,
                    queue_capacity: snapshot.queue_capacity,
                    scheduler_heartbeat_age_ms: snapshot.scheduler_heartbeat_age_ms,
                })
            });
            snapshot.pressure = observed;
            let previous = publish_resource_pressure(observed);
            if previous != observed {
                record_pressure_transition(&data_dir, previous, observed, &snapshot);
            }
            last_sample = std::time::Instant::now();
        }

        match listener.accept() {
            Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                snapshot.scheduler_heartbeat_age_ms = epoch_ms()
                    .saturating_sub(shared.scheduler_heartbeat_at_ms.load(Ordering::Acquire));
                let _ = respond(&mut stream, &snapshot);
            }
            Ok((_stream, _)) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                tracing::warn!(error = %error, "runtime health listener accept failed");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn record_pressure_transition(
    data_dir: &std::path::Path,
    previous: ResourcePressureLevel,
    observed: ResourcePressureLevel,
    snapshot: &RuntimeHealthSnapshot,
) {
    let mut event = SystemProxyLifecycleEvent::new("resource_pressure_changed", "runtime_health");
    event.decision = Some(format!("{previous:?}_to_{observed:?}").to_ascii_lowercase());
    populate_event_metrics(&mut event, snapshot);
    if let Err(error) = append_system_proxy_event(data_dir, &event) {
        tracing::warn!(error = %error, "failed to persist resource pressure event");
    }
    let _ = update_system_proxy_owner_state(data_dir, |owner| {
        owner.pressure = observed;
        owner.last_action = Some("resource_pressure_changed".into());
    });
}

fn respond(stream: &mut TcpStream, snapshot: &RuntimeHealthSnapshot) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    let mut request = [0_u8; 512];
    let read = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..read]);
    if !request.starts_with("GET /health ") {
        stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }
    let body = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)
}

fn populate_event_metrics(event: &mut SystemProxyLifecycleEvent, snapshot: &RuntimeHealthSnapshot) {
    event.new_pid = Some(snapshot.pid);
    event.scheduler_heartbeat_age_ms = Some(snapshot.scheduler_heartbeat_age_ms);
    event.rss_bytes = Some(snapshot.rss_bytes);
    event.cpu_percent = Some(snapshot.cpu_percent);
    event.fd_count = Some(snapshot.fd_count);
    event.fd_limit = Some(snapshot.fd_limit);
    event.active_connections = Some(snapshot.active_connections);
    event.queue_depth = Some(snapshot.queue_depth);
    event.queue_capacity = Some(snapshot.queue_capacity);
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn pressure_override() -> Option<ResourcePressureLevel> {
    match std::env::var("BIFROST_RESOURCE_PRESSURE_OVERRIDE")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "normal" => Some(ResourcePressureLevel::Normal),
        "degraded" => Some(ResourcePressureLevel::Degraded),
        "critical" => Some(ResourcePressureLevel::Critical),
        _ => None,
    }
}

#[cfg(unix)]
fn current_fd_limit() -> u64 {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        limit.rlim_cur
    } else {
        0
    }
}

#[cfg(not(unix))]
fn current_fd_limit() -> u64 {
    0
}

#[cfg(target_os = "linux")]
fn current_fd_count() -> u64 {
    std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count() as u64)
        .unwrap_or(0)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn current_fd_count() -> u64 {
    std::fs::read_dir("/dev/fd")
        .map(|entries| entries.count() as u64)
        .unwrap_or(0)
}

#[cfg(not(unix))]
fn current_fd_count() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_lane_is_loopback_and_reports_scheduler_age() {
        let temp = tempfile::tempdir().unwrap();
        let (lane, reporter) = RuntimeHealthLane::start(temp.path().to_path_buf()).unwrap();
        reporter.heartbeat();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, lane.port())).unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("scheduler_heartbeat_age_ms"));
    }

    #[test]
    fn health_response_rejects_other_paths_and_event_captures_all_metrics() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream
                .write_all(b"GET /other HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        let (mut server, _) = listener.accept().unwrap();
        respond(&mut server, &RuntimeHealthSnapshot::default()).unwrap();
        drop(server);
        assert!(client.join().unwrap().starts_with("HTTP/1.1 404"));

        let temp = tempfile::tempdir().unwrap();
        let snapshot = RuntimeHealthSnapshot {
            pid: 7,
            scheduler_heartbeat_age_ms: 8,
            rss_bytes: 9,
            cpu_percent: 10.0,
            fd_count: 11,
            fd_limit: 12,
            active_connections: 13,
            connection_limit: 14,
            queue_depth: 15,
            queue_capacity: 16,
            pressure: ResourcePressureLevel::Critical,
        };
        record_pressure_transition(
            temp.path(),
            ResourcePressureLevel::Normal,
            ResourcePressureLevel::Critical,
            &snapshot,
        );
        let event = bifrost_core::read_recent_system_proxy_events(temp.path(), 1)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(event.new_pid, Some(7));
        assert_eq!(event.scheduler_heartbeat_age_ms, Some(8));
        assert_eq!(event.rss_bytes, Some(9));
        assert_eq!(event.cpu_percent, Some(10.0));
        assert_eq!(event.fd_count, Some(11));
        assert_eq!(event.fd_limit, Some(12));
        assert_eq!(event.active_connections, Some(13));
        assert_eq!(event.queue_depth, Some(15));
        assert_eq!(event.queue_capacity, Some(16));
        let owner = bifrost_core::read_system_proxy_owner_state(temp.path())
            .unwrap()
            .unwrap();
        assert_eq!(owner.pressure, ResourcePressureLevel::Critical);
    }

    #[test]
    fn pressure_override_accepts_documented_values() {
        let previous = std::env::var_os("BIFROST_RESOURCE_PRESSURE_OVERRIDE");
        for (value, expected) in [
            ("normal", Some(ResourcePressureLevel::Normal)),
            ("DEGRADED", Some(ResourcePressureLevel::Degraded)),
            ("critical", Some(ResourcePressureLevel::Critical)),
            ("unknown", None),
        ] {
            std::env::set_var("BIFROST_RESOURCE_PRESSURE_OVERRIDE", value);
            assert_eq!(pressure_override(), expected);
        }
        match previous {
            Some(value) => std::env::set_var("BIFROST_RESOURCE_PRESSURE_OVERRIDE", value),
            None => std::env::remove_var("BIFROST_RESOURCE_PRESSURE_OVERRIDE"),
        }
    }
}
