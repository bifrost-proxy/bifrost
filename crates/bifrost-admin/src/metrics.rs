use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sysinfo::{Disks, Pid, ProcessesToUpdate, System};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrafficType {
    Http,
    Https,
    Tunnel,
    Ws,
    Wss,
    H3,
    Socks5,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrafficTypeMetrics {
    pub requests: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub active_connections: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub timestamp: u64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub cpu_usage: f32,
    pub total_requests: u64,
    pub active_connections: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub bytes_sent_rate: f32,
    pub bytes_received_rate: f32,
    pub qps: f32,
    pub max_qps: f32,
    pub max_bytes_sent_rate: f32,
    pub max_bytes_received_rate: f32,
    pub client_process_resolution_failures: u64,
    pub client_process_policy_unknown_decisions: u64,
    pub http: TrafficTypeMetrics,
    pub https: TrafficTypeMetrics,
    pub tunnel: TrafficTypeMetrics,
    pub ws: TrafficTypeMetrics,
    pub wss: TrafficTypeMetrics,
    pub h3: TrafficTypeMetrics,
    pub socks5: TrafficTypeMetrics,
}

#[derive(Default)]
struct TrafficTypeCounters {
    requests: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    active_connections: AtomicU64,
}

impl TrafficTypeCounters {
    fn new() -> Self {
        Self::default()
    }

    fn to_metrics(&self) -> TrafficTypeMetrics {
        TrafficTypeMetrics {
            requests: self.requests.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
        }
    }
}

struct CachedCpuMetrics {
    memory_used: AtomicU64,
    memory_total: AtomicU64,
    smoothed_cpu_usage: RwLock<f32>,
    last_raw_cpu_usage: RwLock<f32>,
    last_refresh_time: AtomicU64,
}

impl Default for CachedCpuMetrics {
    fn default() -> Self {
        Self {
            memory_used: AtomicU64::new(0),
            memory_total: AtomicU64::new(0),
            smoothed_cpu_usage: RwLock::new(0.0),
            last_raw_cpu_usage: RwLock::new(0.0),
            last_refresh_time: AtomicU64::new(0),
        }
    }
}

struct CachedMetricsSnapshot {
    snapshot: RwLock<Option<MetricsSnapshot>>,
    last_refresh_time: AtomicU64,
}

impl Default for CachedMetricsSnapshot {
    fn default() -> Self {
        Self {
            snapshot: RwLock::new(None),
            last_refresh_time: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ByteEvent {
    timestamp: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

pub struct MetricsCollector {
    total_requests: AtomicU64,
    active_connections: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    history: RwLock<VecDeque<MetricsSnapshot>>,
    max_history: usize,
    last_request_count: AtomicU64,
    last_bytes_sent: AtomicU64,
    last_bytes_received: AtomicU64,
    last_snapshot_time: AtomicU64,
    system: RwLock<System>,
    pid: Pid,
    max_qps: RwLock<f32>,
    max_bytes_sent_rate: RwLock<f32>,
    max_bytes_received_rate: RwLock<f32>,
    client_process_resolution_failures: AtomicU64,
    client_process_policy_unknown_decisions: AtomicU64,
    cached_cpu: CachedCpuMetrics,
    cached_snapshot: CachedMetricsSnapshot,
    request_events: Mutex<VecDeque<u64>>,
    byte_events: Mutex<VecDeque<ByteEvent>>,
    http: TrafficTypeCounters,
    https: TrafficTypeCounters,
    tunnel: TrafficTypeCounters,
    ws: TrafficTypeCounters,
    wss: TrafficTypeCounters,
    h3: TrafficTypeCounters,
    socks5: TrafficTypeCounters,
}

impl MetricsCollector {
    pub fn new(max_history: usize) -> Self {
        let mut system = System::new_all();
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
        let memory_total = system.total_memory();
        Self {
            total_requests: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            history: RwLock::new(VecDeque::with_capacity(max_history)),
            max_history,
            last_request_count: AtomicU64::new(0),
            last_bytes_sent: AtomicU64::new(0),
            last_bytes_received: AtomicU64::new(0),
            last_snapshot_time: AtomicU64::new(0),
            system: RwLock::new(system),
            pid,
            max_qps: RwLock::new(0.0),
            max_bytes_sent_rate: RwLock::new(0.0),
            max_bytes_received_rate: RwLock::new(0.0),
            client_process_resolution_failures: AtomicU64::new(0),
            client_process_policy_unknown_decisions: AtomicU64::new(0),
            cached_cpu: CachedCpuMetrics {
                memory_total: AtomicU64::new(memory_total),
                ..Default::default()
            },
            cached_snapshot: CachedMetricsSnapshot::default(),
            request_events: Mutex::new(VecDeque::new()),
            byte_events: Mutex::new(VecDeque::new()),
            http: TrafficTypeCounters::new(),
            https: TrafficTypeCounters::new(),
            tunnel: TrafficTypeCounters::new(),
            ws: TrafficTypeCounters::new(),
            wss: TrafficTypeCounters::new(),
            h3: TrafficTypeCounters::new(),
            socks5: TrafficTypeCounters::new(),
        }
    }

    fn get_counters(&self, traffic_type: TrafficType) -> &TrafficTypeCounters {
        match traffic_type {
            TrafficType::Http => &self.http,
            TrafficType::Https => &self.https,
            TrafficType::Tunnel => &self.tunnel,
            TrafficType::Ws => &self.ws,
            TrafficType::Wss => &self.wss,
            TrafficType::H3 => &self.h3,
            TrafficType::Socks5 => &self.socks5,
        }
    }

    fn invalidate_cached_snapshot(&self) {
        self.cached_snapshot
            .last_refresh_time
            .store(0, Ordering::Relaxed);
        *self.cached_snapshot.snapshot.write() = None;
    }

    pub fn increment_requests(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.record_request_event();
        self.invalidate_cached_snapshot();
    }

    pub fn increment_requests_by_type(&self, traffic_type: TrafficType) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.get_counters(traffic_type)
            .requests
            .fetch_add(1, Ordering::Relaxed);
        self.record_request_event();
        self.invalidate_cached_snapshot();
    }

    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.invalidate_cached_snapshot();
    }

    pub fn increment_connections_by_type(&self, traffic_type: TrafficType) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.get_counters(traffic_type)
            .active_connections
            .fetch_add(1, Ordering::Relaxed);
        self.invalidate_cached_snapshot();
    }

    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
        self.invalidate_cached_snapshot();
    }

    pub fn decrement_connections_by_type(&self, traffic_type: TrafficType) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
        self.get_counters(traffic_type)
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
        self.invalidate_cached_snapshot();
    }

    pub fn add_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        self.record_byte_event(bytes, 0);
        self.invalidate_cached_snapshot();
    }

    pub fn add_bytes_sent_by_type(&self, traffic_type: TrafficType, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        self.get_counters(traffic_type)
            .bytes_sent
            .fetch_add(bytes, Ordering::Relaxed);
        self.record_byte_event(bytes, 0);
        self.invalidate_cached_snapshot();
    }

    pub fn add_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
        self.record_byte_event(0, bytes);
        self.invalidate_cached_snapshot();
    }

    pub fn add_bytes_received_by_type(&self, traffic_type: TrafficType, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
        self.get_counters(traffic_type)
            .bytes_received
            .fetch_add(bytes, Ordering::Relaxed);
        self.record_byte_event(0, bytes);
        self.invalidate_cached_snapshot();
    }

    fn now_ms() -> u64 {
        chrono::Utc::now().timestamp_millis() as u64
    }

    fn record_request_event(&self) {
        let now = Self::now_ms();
        let mut events = self.request_events.lock();
        events.push_back(now);
        prune_u64_events(&mut events, now, 10_000);
    }

    fn record_byte_event(&self, bytes_sent: u64, bytes_received: u64) {
        if bytes_sent == 0 && bytes_received == 0 {
            return;
        }
        let now = Self::now_ms();
        let mut events = self.byte_events.lock();
        events.push_back(ByteEvent {
            timestamp: now,
            bytes_sent,
            bytes_received,
        });
        prune_byte_events(&mut events, now, 10_000);
    }

    fn realtime_window_rates(&self, now: u64) -> (f32, f32, f32) {
        let window_ms = 1_000;
        let window_secs = window_ms as f32 / 1000.0;
        let cutoff = now.saturating_sub(window_ms);

        let request_count = {
            let mut events = self.request_events.lock();
            prune_u64_events(&mut events, now, 10_000);
            events.iter().filter(|&&ts| ts >= cutoff).count() as f32
        };

        let (bytes_sent, bytes_received) = {
            let mut events = self.byte_events.lock();
            prune_byte_events(&mut events, now, 10_000);
            events
                .iter()
                .filter(|event| event.timestamp >= cutoff)
                .fold((0u64, 0u64), |(sent, received), event| {
                    (
                        sent.saturating_add(event.bytes_sent),
                        received.saturating_add(event.bytes_received),
                    )
                })
        };

        (
            request_count / window_secs,
            bytes_sent as f32 / window_secs,
            bytes_received as f32 / window_secs,
        )
    }

    pub fn increment_client_process_resolution_failure(&self) {
        self.client_process_resolution_failures
            .fetch_add(1, Ordering::Relaxed);
        self.invalidate_cached_snapshot();
    }

    pub fn increment_client_process_policy_unknown_decision(&self) {
        self.client_process_policy_unknown_decisions
            .fetch_add(1, Ordering::Relaxed);
        self.invalidate_cached_snapshot();
    }

    pub fn refresh_cpu_metrics(&self) {
        let mut system = self.system.write();
        system.refresh_processes(ProcessesToUpdate::Some(&[self.pid]));

        let (memory_used, raw_cpu_usage) = if let Some(process) = system.process(self.pid) {
            (process.memory(), process.cpu_usage())
        } else {
            (0, 0.0)
        };

        self.cached_cpu
            .memory_used
            .store(memory_used, Ordering::Relaxed);

        let smoothing_alpha: f32 = 0.3;
        let mut smoothed = self.cached_cpu.smoothed_cpu_usage.write();
        let mut last_raw = self.cached_cpu.last_raw_cpu_usage.write();

        if *last_raw > 0.0 || raw_cpu_usage > 0.0 {
            *smoothed = smoothing_alpha * raw_cpu_usage + (1.0 - smoothing_alpha) * *smoothed;
        } else {
            *smoothed = raw_cpu_usage;
        }
        *last_raw = raw_cpu_usage;

        let now = chrono::Utc::now().timestamp_millis() as u64;
        self.cached_cpu
            .last_refresh_time
            .store(now, Ordering::Relaxed);

        tracing::trace!(
            raw_cpu = raw_cpu_usage,
            smoothed_cpu = *smoothed,
            memory_used = memory_used,
            "[METRICS] CPU metrics refreshed"
        );
    }

    pub fn get_current(&self) -> MetricsSnapshot {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let cached_at = self
            .cached_snapshot
            .last_refresh_time
            .load(Ordering::Relaxed);
        if now.saturating_sub(cached_at) <= 250 {
            if let Some(snapshot) = self.cached_snapshot.snapshot.read().clone() {
                return snapshot;
            }
        }

        let memory_used = self.cached_cpu.memory_used.load(Ordering::Relaxed);
        let memory_total = self.cached_cpu.memory_total.load(Ordering::Relaxed);
        let cpu_usage = *self.cached_cpu.smoothed_cpu_usage.read();

        let total_requests = self.total_requests.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let bytes_received = self.bytes_received.load(Ordering::Relaxed);

        let (qps, bytes_sent_rate, bytes_received_rate) = self.realtime_window_rates(now);

        let max_qps = *self.max_qps.read();
        let max_bytes_sent_rate = *self.max_bytes_sent_rate.read();
        let max_bytes_received_rate = *self.max_bytes_received_rate.read();

        let snapshot = MetricsSnapshot {
            timestamp: now,
            memory_used,
            memory_total,
            cpu_usage,
            total_requests,
            active_connections: self.active_connections.load(Ordering::Relaxed),
            bytes_sent,
            bytes_received,
            bytes_sent_rate,
            bytes_received_rate,
            qps,
            max_qps,
            max_bytes_sent_rate,
            max_bytes_received_rate,
            client_process_resolution_failures: self
                .client_process_resolution_failures
                .load(Ordering::Relaxed),
            client_process_policy_unknown_decisions: self
                .client_process_policy_unknown_decisions
                .load(Ordering::Relaxed),
            http: self.http.to_metrics(),
            https: self.https.to_metrics(),
            tunnel: self.tunnel.to_metrics(),
            ws: self.ws.to_metrics(),
            wss: self.wss.to_metrics(),
            h3: self.h3.to_metrics(),
            socks5: self.socks5.to_metrics(),
        };

        *self.cached_snapshot.snapshot.write() = Some(snapshot.clone());
        self.cached_snapshot
            .last_refresh_time
            .store(now, Ordering::Relaxed);

        snapshot
    }

    pub fn take_snapshot(&self) -> MetricsSnapshot {
        let mut snapshot = self.get_current();

        self.last_request_count
            .store(snapshot.total_requests, Ordering::Relaxed);
        self.last_bytes_sent
            .store(snapshot.bytes_sent, Ordering::Relaxed);
        self.last_bytes_received
            .store(snapshot.bytes_received, Ordering::Relaxed);
        self.last_snapshot_time
            .store(snapshot.timestamp, Ordering::Relaxed);

        {
            let mut max_qps = self.max_qps.write();
            if snapshot.qps > *max_qps {
                *max_qps = snapshot.qps;
            }
            snapshot.max_qps = *max_qps;
        }

        {
            let mut max_sent = self.max_bytes_sent_rate.write();
            if snapshot.bytes_sent_rate > *max_sent {
                *max_sent = snapshot.bytes_sent_rate;
            }
            snapshot.max_bytes_sent_rate = *max_sent;
        }

        {
            let mut max_recv = self.max_bytes_received_rate.write();
            if snapshot.bytes_received_rate > *max_recv {
                *max_recv = snapshot.bytes_received_rate;
            }
            snapshot.max_bytes_received_rate = *max_recv;
        }

        let mut history = self.history.write();
        if history.len() >= self.max_history {
            history.pop_front();
        }
        history.push_back(snapshot.clone());

        snapshot
    }

    pub fn get_history(&self, limit: Option<usize>) -> Vec<MetricsSnapshot> {
        let history = self.history.read();
        match limit {
            Some(n) => history.iter().rev().take(n).cloned().collect(),
            None => history.iter().cloned().collect(),
        }
    }

    pub fn get_total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    pub fn get_active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }
}

fn prune_u64_events(events: &mut VecDeque<u64>, now: u64, retention_ms: u64) {
    let cutoff = now.saturating_sub(retention_ms);
    while events.front().is_some_and(|timestamp| *timestamp < cutoff) {
        events.pop_front();
    }
}

fn prune_byte_events(events: &mut VecDeque<ByteEvent>, now: u64, retention_ms: u64) {
    let cutoff = now.saturating_sub(retention_ms);
    while events.front().is_some_and(|event| event.timestamp < cutoff) {
        events.pop_front();
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new(3600)
    }
}

pub type SharedMetricsCollector = Arc<MetricsCollector>;

pub fn start_metrics_collector_task(
    collector: SharedMetricsCollector,
    interval_secs: u64,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();

    let collector_snapshot = collector.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            collector_snapshot.take_snapshot();
        }
    }));

    let collector_cpu = collector.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            collector_cpu.refresh_cpu_metrics();
        }
    }));

    handles
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub device_name: String,
    pub os: String,
    pub arch: String,
    pub cpu_logical_cores: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_physical_cores: Option<usize>,
    pub memory_total_bytes: u64,
    pub memory_available_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_available_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_mount_point: Option<String>,
    pub uptime_secs: u64,
    pub pid: u32,
}

impl SystemInfo {
    pub fn new(start_time: u64) -> Self {
        let now = chrono::Utc::now().timestamp() as u64;
        let mut system = System::new_all();
        system.refresh_memory();
        let storage = current_storage_info(std::env::current_dir().ok().as_deref());
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            device_name: System::host_name().unwrap_or_else(|| "unknown".to_string()),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_logical_cores: system.cpus().len(),
            cpu_physical_cores: system.physical_core_count(),
            memory_total_bytes: system.total_memory(),
            memory_available_bytes: system.available_memory(),
            storage_total_bytes: storage.as_ref().map(|s| s.total_bytes),
            storage_available_bytes: storage.as_ref().map(|s| s.available_bytes),
            storage_mount_point: storage.map(|s| s.mount_point),
            uptime_secs: now.saturating_sub(start_time),
            pid: std::process::id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StorageInfo {
    total_bytes: u64,
    available_bytes: u64,
    mount_point: String,
}

fn current_storage_info(current_dir: Option<&Path>) -> Option<StorageInfo> {
    let disks = Disks::new_with_refreshed_list();
    select_storage_disk(disks.list(), current_dir).map(|disk| StorageInfo {
        total_bytes: disk.total_space(),
        available_bytes: disk.available_space(),
        mount_point: disk.mount_point().display().to_string(),
    })
}

fn select_storage_disk<'a>(
    disks: &'a [sysinfo::Disk],
    current_dir: Option<&Path>,
) -> Option<&'a sysinfo::Disk> {
    if let Some(path) = current_dir {
        if let Some(disk) = disks
            .iter()
            .filter(|disk| path.starts_with(disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().components().count())
        {
            return Some(disk);
        }
    }

    disks
        .iter()
        .find(|disk| disk.mount_point() == Path::new("/"))
        .or_else(|| disks.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collector() {
        let collector = MetricsCollector::new(100);

        assert_eq!(collector.get_total_requests(), 0);
        assert_eq!(collector.get_active_connections(), 0);

        collector.increment_requests();
        collector.increment_requests();
        assert_eq!(collector.get_total_requests(), 2);

        collector.increment_connections();
        assert_eq!(collector.get_active_connections(), 1);

        collector.decrement_connections();
        assert_eq!(collector.get_active_connections(), 0);
    }

    #[test]
    fn test_metrics_snapshot() {
        let collector = MetricsCollector::new(10);

        collector.increment_requests();
        collector.add_bytes_sent(100);
        collector.add_bytes_received(200);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.bytes_sent, 100);
        assert_eq!(snapshot.bytes_received, 200);
    }

    #[test]
    fn test_metrics_history() {
        let collector = MetricsCollector::new(3);

        for _ in 0..5 {
            collector.increment_requests();
            collector.take_snapshot();
        }

        let history = collector.get_history(None);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_system_info() {
        let start_time = chrono::Utc::now().timestamp() as u64 - 60;
        let info = SystemInfo::new(start_time);

        assert!(!info.version.is_empty());
        assert!(!info.device_name.is_empty());
        assert!(!info.os.is_empty());
        assert!(info.cpu_logical_cores > 0);
        assert!(info.memory_total_bytes > 0);
        assert!(info.memory_available_bytes <= info.memory_total_bytes);
        if let Some(total) = info.storage_total_bytes {
            assert!(total > 0);
            assert!(info.storage_available_bytes.unwrap_or_default() <= total);
        }
        assert!(info.uptime_secs >= 60);
    }

    #[test]
    fn test_traffic_type_metrics() {
        let collector = MetricsCollector::new(10);

        collector.increment_requests_by_type(TrafficType::Http);
        collector.increment_requests_by_type(TrafficType::Http);
        collector.increment_requests_by_type(TrafficType::Https);
        collector.add_bytes_sent_by_type(TrafficType::Http, 100);
        collector.add_bytes_received_by_type(TrafficType::Https, 200);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.total_requests, 3);
        assert_eq!(snapshot.http.requests, 2);
        assert_eq!(snapshot.https.requests, 1);
        assert_eq!(snapshot.http.bytes_sent, 100);
        assert_eq!(snapshot.https.bytes_received, 200);
    }

    #[test]
    fn test_realtime_metrics_use_recent_event_window() {
        let collector = MetricsCollector::new(10);

        collector.increment_requests_by_type(TrafficType::Http);
        collector.increment_requests_by_type(TrafficType::Http);
        collector.add_bytes_sent_by_type(TrafficType::Http, 512);
        collector.add_bytes_received_by_type(TrafficType::Http, 1024);

        let snapshot = collector.get_current();
        assert_eq!(snapshot.total_requests, 2);
        assert!(snapshot.qps >= 2.0, "qps was {}", snapshot.qps);
        assert!(
            snapshot.bytes_sent_rate >= 512.0,
            "upload rate was {}",
            snapshot.bytes_sent_rate
        );
        assert!(
            snapshot.bytes_received_rate >= 1024.0,
            "download rate was {}",
            snapshot.bytes_received_rate
        );

        std::thread::sleep(Duration::from_millis(1_300));

        let expired = collector.get_current();
        assert_eq!(expired.total_requests, 2);
        assert_eq!(expired.qps, 0.0);
        assert_eq!(expired.bytes_sent_rate, 0.0);
        assert_eq!(expired.bytes_received_rate, 0.0);
    }

    #[test]
    fn test_connection_tracking_by_type() {
        let collector = MetricsCollector::new(10);

        collector.increment_connections_by_type(TrafficType::Ws);
        collector.increment_connections_by_type(TrafficType::Wss);
        collector.increment_connections_by_type(TrafficType::Tunnel);

        let snapshot = collector.get_current();
        assert_eq!(snapshot.active_connections, 3);
        assert_eq!(snapshot.ws.active_connections, 1);
        assert_eq!(snapshot.wss.active_connections, 1);
        assert_eq!(snapshot.tunnel.active_connections, 1);

        collector.decrement_connections_by_type(TrafficType::Ws);
        let snapshot = collector.get_current();
        assert_eq!(snapshot.active_connections, 2);
        assert_eq!(snapshot.ws.active_connections, 0);
    }
}
