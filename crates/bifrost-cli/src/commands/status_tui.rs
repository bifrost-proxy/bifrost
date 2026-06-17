use std::io::stdout;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bifrost_admin::push::{
    PushMessage as AdminPushMessage, SETTINGS_SCOPE_CLI_PROXY, SETTINGS_SCOPE_PERFORMANCE_CONFIG,
    SETTINGS_SCOPE_PROXY_SETTINGS,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use futures::{SinkExt, StreamExt};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::CrosstermBackend,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, Tabs},
    Frame, Terminal,
};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use crate::process::{is_process_running, read_pid, read_runtime_port};

fn direct_agent() -> ureq::Agent {
    bifrost_core::direct_ureq_agent_builder()
        .timeout(HTTP_TIMEOUT)
        .build()
}

#[derive(Debug, Deserialize, Default, Clone)]
struct TrafficTypeMetrics {
    requests: u64,
    bytes_sent: u64,
    bytes_received: u64,
    active_connections: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct MetricsSnapshot {
    #[allow(dead_code)]
    timestamp: u64,
    memory_used: u64,
    memory_total: u64,
    cpu_usage: f32,
    total_requests: u64,
    active_connections: u64,
    bytes_sent: u64,
    bytes_received: u64,
    bytes_sent_rate: f32,
    bytes_received_rate: f32,
    qps: f32,
    max_qps: f32,
    max_bytes_sent_rate: f32,
    max_bytes_received_rate: f32,
    http: TrafficTypeMetrics,
    https: TrafficTypeMetrics,
    tunnel: TrafficTypeMetrics,
    ws: TrafficTypeMetrics,
    wss: TrafficTypeMetrics,
    socks5: TrafficTypeMetrics,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default, Clone)]
struct AppMetrics {
    app_name: String,
    requests: u64,
    active_connections: u64,
    bytes_sent: u64,
    bytes_received: u64,
    http_requests: u64,
    https_requests: u64,
    tunnel_requests: u64,
    ws_requests: u64,
    wss_requests: u64,
    h3_requests: u64,
    socks5_requests: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Default, Clone)]
struct HostMetrics {
    host: String,
    requests: u64,
    active_connections: u64,
    bytes_sent: u64,
    bytes_received: u64,
    http_requests: u64,
    https_requests: u64,
    tunnel_requests: u64,
    ws_requests: u64,
    wss_requests: u64,
    h3_requests: u64,
    socks5_requests: u64,
}

#[derive(Debug, Deserialize, Clone)]
struct RuleGroup {
    name: String,
    enabled: bool,
    rule_count: usize,
}

#[derive(Debug, Deserialize, Clone)]
struct Value {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ValuesResponse {
    values: Vec<Value>,
    #[allow(dead_code)]
    total: usize,
}

#[derive(Debug, Deserialize, Clone)]
struct Script {
    name: String,
    #[allow(dead_code)]
    script_type: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ScriptsResponse {
    request: Vec<Script>,
    response: Vec<Script>,
}

#[derive(Debug, Deserialize, Clone)]
struct ConfigResponse {
    tls: TlsConfig,
    port: u16,
    host: String,
}

#[derive(Debug, Deserialize, Clone)]
struct CliProxyStatus {
    enabled: bool,
    shell: String,
    config_files: Vec<String>,
    proxy_url: String,
}

#[derive(Debug, Deserialize, Clone)]
struct TlsConfig {
    enable_tls_interception: bool,
    intercept_include: Vec<String>,
    app_intercept_include: Vec<String>,
    unsafe_ssl: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
struct TrafficConfig {
    max_records: usize,
    max_db_size_bytes: u64,
    max_body_memory_size: usize,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    binary_traffic_performance_mode: bool,
    file_retention_days: u64,
    sse_stream_flush_bytes: usize,
    sse_stream_flush_interval_ms: u64,
    ws_payload_flush_bytes: usize,
    ws_payload_flush_interval_ms: u64,
    ws_payload_max_open_files: usize,
}

#[derive(Debug, Deserialize, Clone)]
struct BodyStoreStats {
    file_count: usize,
    total_size: u64,
}

#[derive(Debug, Deserialize, Clone)]
struct FrameStoreStats {
    connection_count: usize,
    total_size: u64,
}

#[derive(Debug, Deserialize, Clone)]
struct PerformanceConfigResponse {
    traffic: TrafficConfig,
    body_store_stats: Option<BodyStoreStats>,
    frame_store_stats: Option<FrameStoreStats>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct RemoteInvokeStatus {
    state: String,
    pending_pairings_count: usize,
    active_call_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct RemoteInvokeGrant {
    grant_id: String,
    caller_fingerprint: String,
    #[serde(default)]
    caller_display_name: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    grant_scope: Option<String>,
    status: String,
    #[serde(default)]
    first_connected_at: Option<u64>,
    #[serde(default)]
    created_at: Option<u64>,
    #[serde(default)]
    last_command_at: Option<u64>,
    #[serde(default)]
    last_used_at: Option<u64>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct RemoteInvokeCommandSummary {
    command_preview: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct RemoteInvokeCall {
    call_id: String,
    grant_id: String,
    caller_fingerprint: String,
    #[serde(default)]
    caller_display_name: Option<String>,
    #[serde(default)]
    auth_method: Option<String>,
    command_summary: RemoteInvokeCommandSummary,
    status: String,
    started_at: u64,
    #[serde(default)]
    ended_at: Option<u64>,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    bytes_out: Option<u64>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct RemoteInvokeGrantsResponse {
    grants: Vec<RemoteInvokeGrant>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct RemoteInvokeCallsResponse {
    calls: Vec<RemoteInvokeCall>,
}

#[derive(Debug, Clone, Default)]
struct RemoteInvokeSnapshot {
    status: Option<RemoteInvokeStatus>,
    grants: Vec<RemoteInvokeGrant>,
    calls: Vec<RemoteInvokeCall>,
}

const SLOW_REFRESH_INTERVAL: u64 = 5;
const PROCESS_CHECK_INTERVAL: Duration = Duration::from_secs(3);
const CPU_HISTORY_SIZE: usize = 3600;
const QPS_HISTORY_SIZE: usize = 60;
const PUSH_STALE_TIMEOUT: Duration = Duration::from_secs(4);
const PUSH_RECONNECT_DELAY: Duration = Duration::from_secs(2);
const PUSH_METRICS_INTERVAL_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy)]
struct FetchPlan {
    metrics: bool,
    rules: bool,
    values: bool,
    scripts: bool,
    config: bool,
    performance: bool,
    app_metrics: bool,
    host_metrics: bool,
    cli_proxy: bool,
    remote_invoke: bool,
}

#[derive(Debug)]
enum TuiPushEvent {
    Connected,
    Disconnected,
    Metrics(MetricsSnapshot),
    Values(Vec<Value>),
    Scripts(ScriptsResponse),
    Config(ConfigResponse),
    Performance(PerformanceConfigResponse),
    CliProxy(CliProxyStatus),
}

struct App {
    port: u16,
    push_port: Arc<AtomicU16>,
    push_rx: mpsc::Receiver<TuiPushEvent>,
    push_connected: bool,
    last_push_event: Option<Instant>,
    is_running: bool,
    pid: Option<u32>,
    metrics: MetricsSnapshot,
    qps_history: Vec<f64>,
    cpu_history: Vec<f32>,
    max_cpu: f32,
    memory_used_history: Vec<u64>,
    max_memory_used: u64,
    app_metrics: Vec<AppMetrics>,
    host_metrics: Vec<HostMetrics>,
    rules: Vec<RuleGroup>,
    values: Vec<Value>,
    scripts: ScriptsResponse,
    config: Option<ConfigResponse>,
    performance_config: Option<PerformanceConfigResponse>,
    cli_proxy: Option<CliProxyStatus>,
    remote_invoke: RemoteInvokeSnapshot,
    selected_tab: usize,
    last_process_check: Instant,
    last_update: Instant,
    last_slow_refresh: Instant,
    refresh_count: u64,
}

impl App {
    fn new() -> Self {
        let port = read_runtime_port().unwrap_or(9900);
        let push_port = Arc::new(AtomicU16::new(port));
        let push_rx = spawn_push_client(push_port.clone());
        Self {
            port,
            push_port,
            push_rx,
            push_connected: false,
            last_push_event: None,
            is_running: false,
            pid: None,
            metrics: MetricsSnapshot::default(),
            qps_history: vec![0.0; QPS_HISTORY_SIZE],
            cpu_history: vec![0.0; CPU_HISTORY_SIZE],
            max_cpu: 0.0,
            memory_used_history: vec![0; CPU_HISTORY_SIZE],
            max_memory_used: 0,
            app_metrics: Vec::new(),
            host_metrics: Vec::new(),
            rules: Vec::new(),
            values: Vec::new(),
            scripts: ScriptsResponse {
                request: Vec::new(),
                response: Vec::new(),
            },
            config: None,
            performance_config: None,
            cli_proxy: None,
            remote_invoke: RemoteInvokeSnapshot::default(),
            selected_tab: 0,
            last_process_check: Instant::now() - PROCESS_CHECK_INTERVAL,
            last_update: Instant::now(),
            last_slow_refresh: Instant::now() - Duration::from_secs(SLOW_REFRESH_INTERVAL),
            refresh_count: 0,
        }
    }

    fn refresh(&mut self) {
        self.refresh_with_options(false);
    }

    fn apply_metrics_snapshot(&mut self, metrics: MetricsSnapshot) {
        self.qps_history.remove(0);
        self.qps_history.push(metrics.qps as f64);

        self.cpu_history.remove(0);
        self.cpu_history.push(metrics.cpu_usage);
        self.max_cpu = self.max_cpu.max(metrics.cpu_usage);

        self.memory_used_history.remove(0);
        self.memory_used_history.push(metrics.memory_used);
        self.max_memory_used = self.max_memory_used.max(metrics.memory_used);

        self.metrics = metrics;
    }

    fn apply_push_event(&mut self, event: TuiPushEvent) {
        match event {
            TuiPushEvent::Connected => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
            }
            TuiPushEvent::Disconnected => {
                self.push_connected = false;
            }
            TuiPushEvent::Metrics(metrics) => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
                self.apply_metrics_snapshot(metrics);
            }
            TuiPushEvent::Values(values) => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
                self.values = values;
            }
            TuiPushEvent::Scripts(scripts) => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
                self.scripts = scripts;
            }
            TuiPushEvent::Config(config) => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
                self.config = Some(config);
            }
            TuiPushEvent::Performance(config) => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
                self.performance_config = Some(config);
            }
            TuiPushEvent::CliProxy(cli_proxy) => {
                self.push_connected = true;
                self.last_push_event = Some(Instant::now());
                self.cli_proxy = Some(cli_proxy);
            }
        }
    }

    fn drain_push_events(&mut self) {
        while let Ok(event) = self.push_rx.try_recv() {
            self.apply_push_event(event);
        }
    }

    fn push_is_healthy(&self) -> bool {
        self.push_connected
            && self
                .last_push_event
                .map(|instant| instant.elapsed() <= PUSH_STALE_TIMEOUT)
                .unwrap_or(false)
    }

    fn refresh_with_options(&mut self, force_all: bool) {
        if force_all
            || !self.is_running
            || self.last_process_check.elapsed() >= PROCESS_CHECK_INTERVAL
        {
            self.pid = read_pid();
            self.is_running = self.pid.map(is_process_running).unwrap_or(false);
            self.last_process_check = Instant::now();
        }

        if !self.is_running {
            self.port = read_runtime_port().unwrap_or(9900);
            self.push_port.store(self.port, Ordering::Relaxed);
            self.push_connected = false;
            return;
        }

        self.push_port.store(self.port, Ordering::Relaxed);
        self.drain_push_events();

        let need_slow_refresh =
            self.last_slow_refresh.elapsed() >= Duration::from_secs(SLOW_REFRESH_INTERVAL);
        let push_healthy = self.push_is_healthy();
        let force_full = self.refresh_count == 0 || force_all;
        let needs_rules_config = self.selected_tab == 1;

        let port = self.port;
        let fetch_agg_metrics = force_all && self.selected_tab == 2;
        let needs_remote_invoke = self.selected_tab == 3;
        let plan = FetchPlan {
            metrics: !push_healthy,
            rules: needs_rules_config && (need_slow_refresh || force_full),
            values: needs_rules_config && (need_slow_refresh || force_full) && !push_healthy,
            scripts: needs_rules_config && (need_slow_refresh || force_full) && !push_healthy,
            config: needs_rules_config && (need_slow_refresh || force_full) && !push_healthy,
            performance: needs_rules_config && (need_slow_refresh || force_full) && !push_healthy,
            app_metrics: fetch_agg_metrics,
            host_metrics: fetch_agg_metrics,
            cli_proxy: needs_rules_config && (need_slow_refresh || force_full) && !push_healthy,
            remote_invoke: needs_remote_invoke && (need_slow_refresh || force_full),
        };
        let (
            metrics,
            rules,
            values,
            scripts,
            config,
            performance_config,
            app_metrics,
            host_metrics,
            cli_proxy,
            remote_invoke,
        ) = fetch_all_data(port, plan);

        if let Some(m) = metrics {
            self.apply_metrics_snapshot(m);
        }

        if let Some(r) = rules {
            self.rules = r;
        }
        if let Some(v) = values {
            self.values = v;
        }
        if let Some(s) = scripts {
            self.scripts = s;
        }
        if let Some(c) = config {
            self.config = Some(c);
        }
        if let Some(p) = performance_config {
            self.performance_config = Some(p);
        }
        if let Some(a) = app_metrics {
            self.app_metrics = a;
        }
        if let Some(h) = host_metrics {
            self.host_metrics = h;
        }
        if let Some(s) = cli_proxy {
            self.cli_proxy = Some(s);
        }
        if let Some(remote) = remote_invoke {
            self.remote_invoke = remote;
        }

        if need_slow_refresh {
            self.last_slow_refresh = Instant::now();
        }
        self.last_update = Instant::now();
        self.refresh_count += 1;
    }

    fn next_tab(&mut self) {
        self.selected_tab = (self.selected_tab + 1) % 4;
        // 首次切换到 tab 时立即刷新一次，避免等待下一次 tick/slow refresh。
        // 注意：apps/hosts 属于 DB 聚合，仅在 Traffic Details(tab=2) 时触发。
        self.refresh_with_options(true);
    }

    fn prev_tab(&mut self) {
        self.selected_tab = if self.selected_tab == 0 {
            3
        } else {
            self.selected_tab - 1
        };
        // 首次切换到 tab 时立即刷新一次，避免等待下一次 tick/slow refresh。
        // 注意：apps/hosts 属于 DB 聚合，仅在 Traffic Details(tab=2) 时触发。
        self.refresh_with_options(true);
    }
}

const HTTP_TIMEOUT: Duration = Duration::from_millis(500);

type FetchAllDataResult = (
    Option<MetricsSnapshot>,
    Option<Vec<RuleGroup>>,
    Option<Vec<Value>>,
    Option<ScriptsResponse>,
    Option<ConfigResponse>,
    Option<PerformanceConfigResponse>,
    Option<Vec<AppMetrics>>,
    Option<Vec<HostMetrics>>,
    Option<CliProxyStatus>,
    Option<RemoteInvokeSnapshot>,
);

fn fetch_all_data(port: u16, plan: FetchPlan) -> FetchAllDataResult {
    let (tx, rx) = mpsc::channel();

    if plan.metrics {
        let tx_metrics = tx.clone();
        thread::spawn(move || {
            let _ = tx_metrics.send(("metrics", fetch_metrics(port)));
        });
    }

    if plan.rules {
        let tx_rules = tx.clone();
        thread::spawn(move || {
            let _ = tx_rules.send(("rules", fetch_rules(port)));
        });
    }

    if plan.values {
        let tx_values = tx.clone();
        thread::spawn(move || {
            let _ = tx_values.send(("values", fetch_values(port)));
        });
    }

    if plan.scripts {
        let tx_scripts = tx.clone();
        thread::spawn(move || {
            let _ = tx_scripts.send(("scripts", fetch_scripts(port)));
        });
    }

    if plan.config {
        let tx_config = tx.clone();
        thread::spawn(move || {
            let _ = tx_config.send(("config", fetch_config(port)));
        });
    }

    if plan.performance {
        let tx_performance = tx.clone();
        thread::spawn(move || {
            let _ = tx_performance.send(("performance", fetch_performance_config(port)));
        });
    }

    // apps/hosts 属于 DB 聚合计算：仅在用户主动触发时请求，避免后台定时拉取导致 CPU 开销过高。
    if plan.app_metrics {
        let tx_apps = tx.clone();
        thread::spawn(move || {
            let _ = tx_apps.send(("apps", fetch_app_metrics(port)));
        });
    }

    if plan.host_metrics {
        let tx_hosts = tx.clone();
        thread::spawn(move || {
            let _ = tx_hosts.send(("hosts", fetch_host_metrics(port)));
        });
    }

    if plan.cli_proxy {
        let tx_cli_proxy = tx.clone();
        thread::spawn(move || {
            let _ = tx_cli_proxy.send(("cli_proxy", fetch_cli_proxy(port)));
        });
    }

    if plan.remote_invoke {
        let tx_remote = tx.clone();
        thread::spawn(move || {
            let _ = tx_remote.send(("remote_invoke", fetch_remote_invoke_snapshot(port)));
        });
    }

    drop(tx);

    let mut metrics = None;
    let mut rules = None;
    let mut values = None;
    let mut scripts = None;
    let mut config = None;
    let mut performance = None;
    let mut app_metrics = None;
    let mut host_metrics = None;
    let mut cli_proxy = None;
    let mut remote_invoke = None;

    for (key, data) in rx {
        match key {
            "metrics" => metrics = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "rules" => rules = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "values" => values = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "scripts" => scripts = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "config" => config = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "performance" => performance = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "apps" => app_metrics = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "hosts" => host_metrics = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "cli_proxy" => cli_proxy = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            "remote_invoke" => remote_invoke = data.and_then(|d| d.downcast().ok()).map(|b| *b),
            _ => {}
        }
    }

    (
        metrics,
        rules,
        values,
        scripts,
        config,
        performance,
        app_metrics,
        host_metrics,
        cli_proxy,
        remote_invoke,
    )
}

fn fetch_metrics(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/metrics", port);
    let result: Option<MetricsSnapshot> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_rules(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/rules", port);
    let result: Option<Vec<RuleGroup>> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_values(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/values", port);
    let resp: Option<ValuesResponse> = direct_agent().get(&url).call().ok()?.into_json().ok();
    resp.map(|r| Box::new(r.values) as Box<dyn std::any::Any + Send>)
}

fn fetch_scripts(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/scripts", port);
    let result: Option<ScriptsResponse> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_config(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/config", port);
    let result: Option<ConfigResponse> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_performance_config(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/config/performance", port);
    let result: Option<PerformanceConfigResponse> =
        direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_app_metrics(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/metrics/apps", port);
    let result: Option<Vec<AppMetrics>> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_host_metrics(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/metrics/hosts", port);
    let result: Option<Vec<HostMetrics>> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_cli_proxy(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/proxy/cli", port);
    let result: Option<CliProxyStatus> = direct_agent().get(&url).call().ok()?.into_json().ok();
    result.map(|r| Box::new(r) as Box<dyn std::any::Any + Send>)
}

fn fetch_remote_invoke_snapshot(port: u16) -> Option<Box<dyn std::any::Any + Send>> {
    let base = format!("http://127.0.0.1:{}/_bifrost/api/remote-invoke", port);
    let status = direct_agent()
        .get(&format!("{}/status", base))
        .call()
        .ok()
        .and_then(|resp| resp.into_json::<RemoteInvokeStatus>().ok());
    let grants = direct_agent()
        .get(&format!("{}/grants", base))
        .call()
        .ok()
        .and_then(|resp| resp.into_json::<RemoteInvokeGrantsResponse>().ok())
        .map(|resp| resp.grants)
        .unwrap_or_default();
    let calls = direct_agent()
        .get(&format!("{}/calls?limit=20", base))
        .call()
        .ok()
        .and_then(|resp| resp.into_json::<RemoteInvokeCallsResponse>().ok())
        .map(|resp| resp.calls)
        .unwrap_or_default();

    Some(Box::new(RemoteInvokeSnapshot {
        status,
        grants,
        calls,
    }) as Box<dyn std::any::Any + Send>)
}

fn spawn_push_client(port: Arc<AtomicU16>) -> mpsc::Receiver<TuiPushEvent> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        runtime.block_on(async move {
            loop {
                let current_port = port.load(Ordering::Relaxed);
                let url = format!(
                    "ws://127.0.0.1:{}/_bifrost/api/push?need_metrics=true&need_values=true&need_scripts=true&settings_scopes={},{},{}&metrics_interval_ms={}",
                    current_port,
                    SETTINGS_SCOPE_PROXY_SETTINGS,
                    SETTINGS_SCOPE_PERFORMANCE_CONFIG,
                    SETTINGS_SCOPE_CLI_PROXY,
                    PUSH_METRICS_INTERVAL_MS
                );

                match connect_async(&url).await {
                    Ok((mut ws_stream, _)) => {
                        let _ = tx.send(TuiPushEvent::Connected);

                        while let Some(message) = ws_stream.next().await {
                            match message {
                                Ok(Message::Text(text)) => {
                                    if let Some(event) = parse_push_message(text.as_ref()) {
                                        let _ = tx.send(event);
                                    }
                                }
                                Ok(Message::Ping(payload)) => {
                                    let _ = ws_stream.send(Message::Pong(payload)).await;
                                }
                                Ok(Message::Close(_)) => {
                                    let _ = tx.send(TuiPushEvent::Disconnected);
                                    break;
                                }
                                Ok(_) => {}
                                Err(_) => {
                                    let _ = tx.send(TuiPushEvent::Disconnected);
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        let _ = tx.send(TuiPushEvent::Disconnected);
                    }
                }

                tokio::time::sleep(PUSH_RECONNECT_DELAY).await;
            }
        });
    });

    rx
}

fn parse_push_message(text: &str) -> Option<TuiPushEvent> {
    let message = serde_json::from_str::<AdminPushMessage>(text).ok()?;

    match message {
        AdminPushMessage::Connected(_) => Some(TuiPushEvent::Connected),
        AdminPushMessage::Disconnect(_) | AdminPushMessage::Error(_) => {
            Some(TuiPushEvent::Disconnected)
        }
        AdminPushMessage::MetricsUpdate(data) => {
            serde_json::from_value::<MetricsSnapshot>(data.metrics)
                .ok()
                .map(TuiPushEvent::Metrics)
        }
        AdminPushMessage::ValuesUpdate(data) => Some(TuiPushEvent::Values(
            data.values
                .into_iter()
                .map(|value| Value {
                    name: value.name,
                    value: value.value,
                })
                .collect(),
        )),
        AdminPushMessage::ScriptsUpdate(data) => Some(TuiPushEvent::Scripts(ScriptsResponse {
            request: data
                .request
                .into_iter()
                .map(|script| Script {
                    name: script.name,
                    script_type: script.script_type.to_string(),
                })
                .collect(),
            response: data
                .response
                .into_iter()
                .map(|script| Script {
                    name: script.name,
                    script_type: script.script_type.to_string(),
                })
                .collect(),
        })),
        AdminPushMessage::SettingsUpdate(data) => match data.scope.as_str() {
            SETTINGS_SCOPE_PROXY_SETTINGS => serde_json::from_value::<ConfigResponse>(data.data)
                .ok()
                .map(TuiPushEvent::Config),
            SETTINGS_SCOPE_PERFORMANCE_CONFIG => {
                serde_json::from_value::<PerformanceConfigResponse>(data.data)
                    .ok()
                    .map(TuiPushEvent::Performance)
            }
            SETTINGS_SCOPE_CLI_PROXY => serde_json::from_value::<CliProxyStatus>(data.data)
                .ok()
                .map(TuiPushEvent::CliProxy),
            _ => None,
        },
        _ => None,
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_rate(rate: f32) -> String {
    const KB: f32 = 1024.0;
    const MB: f32 = KB * 1024.0;

    if rate >= MB {
        format!("{:.2} MB/s", rate / MB)
    } else if rate >= KB {
        format!("{:.2} KB/s", rate / KB)
    } else {
        format!("{:.0} B/s", rate)
    }
}

fn format_time_span(seconds: usize) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        format!("{}h{}m", hours, mins)
    } else if seconds >= 60 {
        let mins = seconds / 60;
        let secs = seconds % 60;
        format!("{}m{}s", mins, secs)
    } else {
        format!("{}s", seconds)
    }
}

fn format_timestamp_millis(timestamp: Option<u64>) -> String {
    let Some(timestamp) = timestamp.filter(|value| *value > 0) else {
        return "-".to_string();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(timestamp);
    if now >= timestamp {
        format!(
            "{} ago",
            format_time_span(((now - timestamp) / 1000) as usize)
        )
    } else {
        "just now".to_string()
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut output = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push_str("...");
    output
}

/// 计算一张带边框、带表头的表格内部最多能展示多少条数据行。
///
/// `area_height` 为表格组件（含边框）的总高度；表格上下边框各占 1 行，
/// 表头本身占 `header_lines` 行（含 `bottom_margin`）。剩余高度即可用于数据行。
/// 返回值同时受 `max_rows` 上限约束，避免一次性渲染过多历史记录。
fn visible_table_rows(area_height: u16, header_lines: u16, max_rows: usize) -> usize {
    // 上下边框各 1 行。
    let border_rows = 2u16;
    let usable = area_height
        .saturating_sub(border_rows)
        .saturating_sub(header_lines);
    (usable as usize).min(max_rows)
}

#[derive(Clone, Copy, Debug)]
struct AdaptiveColumnSpec {
    min: u16,
    max: Option<u16>,
    weight: u16,
}

/// 根据表格实际可用宽度计算弹性列的真实宽度。
///
/// 与直接使用多个 `Constraint::Min` 不同，这里允许给低信息密度列设置上限，
/// 避免 `Client` 这类列在宽屏里持续吞掉空间，把剩余宽度优先让给命令/结果列。
fn adaptive_column_widths(
    area_width: u16,
    fixed_total: u16,
    total_columns: u16,
    specs: &[AdaptiveColumnSpec],
) -> Vec<u16> {
    if specs.is_empty() {
        return Vec::new();
    }

    let mut widths: Vec<u16> = specs.iter().map(|spec| spec.min).collect();
    let content_width = area_width.saturating_sub(2);
    let gaps = total_columns.saturating_sub(1);
    let min_total: u16 = widths.iter().copied().sum();
    let reserved = fixed_total.saturating_add(gaps);
    let available = content_width.saturating_sub(reserved);
    let mut extra = available.saturating_sub(min_total);
    if extra == 0 {
        return widths;
    }

    while extra > 0 {
        let expandable: Vec<usize> = specs
            .iter()
            .enumerate()
            .filter_map(|(idx, spec)| {
                if spec.max.is_some_and(|max| widths[idx] >= max) {
                    None
                } else {
                    Some(idx)
                }
            })
            .collect();
        if expandable.is_empty() {
            break;
        }

        let total_weight: u16 = expandable.iter().map(|idx| specs[*idx].weight.max(1)).sum();
        let before = extra;
        for idx in expandable {
            if extra == 0 {
                break;
            }
            let weight = specs[idx].weight.max(1);
            let weighted_share = ((before as u32 * weight as u32) / total_weight as u32)
                .max(1)
                .min(extra as u32) as u16;
            let room = specs[idx]
                .max
                .map(|max| max.saturating_sub(widths[idx]))
                .unwrap_or(u16::MAX);
            let delta = weighted_share.min(room).min(extra);
            if delta == 0 {
                continue;
            }
            widths[idx] = widths[idx].saturating_add(delta);
            extra -= delta;
        }

        if extra == before {
            break;
        }
    }

    widths
}

fn caller_label_with_budget(
    display_name: Option<&String>,
    label: Option<&String>,
    fingerprint: &str,
    max_chars: usize,
) -> String {
    display_name
        .filter(|value| !value.trim().is_empty())
        .or(label.filter(|value| !value.trim().is_empty()))
        .cloned()
        .map(|value| truncate_text(&value, max_chars))
        .unwrap_or_else(|| truncate_text(fingerprint, max_chars))
}

fn format_remote_auth(auth_method: Option<&String>) -> String {
    match auth_method.map(|value| value.as_str()) {
        Some("ssh_publickey") => "SSH key".to_string(),
        Some("pair_code") => "Pair code".to_string(),
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "-".to_string(),
    }
}

fn format_remote_result(call: &RemoteInvokeCall) -> String {
    let mut parts = Vec::new();
    if let Some(code) = call.exit_code {
        parts.push(format!("exit {}", code));
    }
    if let Some(duration_ms) = call.duration_ms {
        parts.push(format_time_span((duration_ms / 1000) as usize));
    }
    if let Some(bytes_out) = call.bytes_out {
        parts.push(format!("out {}", format_bytes(bytes_out)));
    }
    if parts.is_empty() {
        if call.ended_at.is_none() {
            "running".to_string()
        } else {
            "-".to_string()
        }
    } else {
        parts.join(" / ")
    }
}

fn latest_call_for_grant<'a>(
    grant: &RemoteInvokeGrant,
    calls: &'a [RemoteInvokeCall],
) -> Option<&'a RemoteInvokeCall> {
    calls
        .iter()
        .filter(|call| {
            call.grant_id == grant.grant_id || call.caller_fingerprint == grant.caller_fingerprint
        })
        .max_by_key(|call| call.started_at)
}

pub fn run_status_tui() -> bifrost_core::Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new();
    app.refresh();

    let tick_rate = Duration::from_millis(1000);
    let mut last_tick = Instant::now();

    loop {
        app.drain_push_events();
        terminal.draw(|frame| ui(frame, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Tab | KeyCode::Right => app.next_tab(),
                        KeyCode::BackTab | KeyCode::Left => app.prev_tab(),
                        KeyCode::Char('r') => app.refresh_with_options(true),
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.refresh();
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

fn ui(frame: &mut Frame, app: &App) {
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, main_layout[0], app);
    render_content(frame, main_layout[1], app);
    render_footer(frame, main_layout[2]);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let status = if app.is_running {
        Span::styled(" ● Running ", Style::default().fg(Color::Green).bold())
    } else {
        Span::styled(" ○ Stopped ", Style::default().fg(Color::Red).bold())
    };

    let pid_info = app.pid.map(|p| format!("PID: {}", p)).unwrap_or_default();

    let tabs = vec![
        "Overview",
        "Rules & Config",
        "Traffic Details",
        "Remote Invoke",
    ];
    let tabs_widget = Tabs::new(tabs)
        .block(Block::default().borders(Borders::ALL).title(vec![
            Span::raw(" Bifrost Status "),
            status,
            Span::styled(
                format!(" {} ", pid_info),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .select(app.selected_tab)
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(Color::Yellow).bold());

    frame.render_widget(tabs_widget, area);
}

fn render_content(frame: &mut Frame, area: Rect, app: &App) {
    if !app.is_running {
        let msg = Paragraph::new("Server is not running. Start with: bifrost start -d")
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(msg, area);
        return;
    }

    match app.selected_tab {
        0 => render_overview(frame, area, app),
        1 => render_rules_config(frame, area, app),
        2 => render_traffic_details(frame, area, app),
        3 => render_remote_invoke(frame, area, app),
        _ => {}
    }
}

fn render_overview(frame: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Min(0),
        ])
        .split(area);

    render_system_metrics(frame, layout[0], app);
    render_cpu_memory_sparklines(frame, layout[1], app);
    render_qps_sparkline(frame, layout[2], app);
    render_connection_stats(frame, layout[3], app);
}

fn render_system_metrics(frame: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    let cpu_gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" CPU "))
        .gauge_style(Style::default().fg(Color::Cyan))
        .percent(app.metrics.cpu_usage.min(100.0) as u16)
        .label(format!("{:.1}%", app.metrics.cpu_usage));
    frame.render_widget(cpu_gauge, layout[0]);

    let mem_percent =
        (app.metrics.memory_used as f64 / app.metrics.memory_total.max(1) as f64 * 100.0) as u16;
    let mem_gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Memory "))
        .gauge_style(Style::default().fg(Color::Magenta))
        .percent(mem_percent.min(100))
        .label(format!(
            "{} / {}",
            format_bytes(app.metrics.memory_used),
            format_bytes(app.metrics.memory_total)
        ));
    frame.render_widget(mem_gauge, layout[1]);

    let upload_block = Block::default().borders(Borders::ALL).title(" Upload ↑ ");
    let upload_text = vec![
        Line::from(format!(
            "Rate: {}",
            format_rate(app.metrics.bytes_sent_rate)
        )),
        Line::from(format!("Total: {}", format_bytes(app.metrics.bytes_sent))),
        Line::from(format!(
            "Max: {}",
            format_rate(app.metrics.max_bytes_sent_rate)
        )),
    ];
    let upload = Paragraph::new(upload_text)
        .block(upload_block)
        .style(Style::default().fg(Color::Green));
    frame.render_widget(upload, layout[2]);

    let download_block = Block::default().borders(Borders::ALL).title(" Download ↓ ");
    let download_text = vec![
        Line::from(format!(
            "Rate: {}",
            format_rate(app.metrics.bytes_received_rate)
        )),
        Line::from(format!(
            "Total: {}",
            format_bytes(app.metrics.bytes_received)
        )),
        Line::from(format!(
            "Max: {}",
            format_rate(app.metrics.max_bytes_received_rate)
        )),
    ];
    let download = Paragraph::new(download_text)
        .block(download_block)
        .style(Style::default().fg(Color::Blue));
    frame.render_widget(download, layout[3]);
}

fn render_cpu_sparkline(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(2) as usize;
    let data: Vec<u64> = if width > 0 && !app.cpu_history.is_empty() {
        let step = app.cpu_history.len() / width.max(1);
        let step = step.max(1);
        app.cpu_history
            .iter()
            .rev()
            .step_by(step)
            .take(width)
            .map(|&v| (v * 10.0) as u64)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        vec![0; width]
    };

    let total_samples = app.cpu_history.iter().filter(|&&v| v > 0.0).count();
    let time_span = format_time_span(total_samples);

    let sparkline = Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(format!(
            " CPU: {:.1}% (max: {:.1}%) | {} ",
            app.metrics.cpu_usage, app.max_cpu, time_span
        )))
        .data(&data)
        .max(1000)
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(sparkline, area);
}

fn render_memory_sparkline(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(2) as usize;
    let data: Vec<u64> = if width > 0 && !app.memory_used_history.is_empty() {
        let step = app.memory_used_history.len() / width.max(1);
        let step = step.max(1);
        app.memory_used_history
            .iter()
            .rev()
            .step_by(step)
            .take(width)
            .map(|&v| v / (1024 * 1024))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        vec![0; width]
    };

    let total_samples = app.memory_used_history.iter().filter(|&&v| v > 0).count();
    let time_span = format_time_span(total_samples);

    let max_mb = (app.max_memory_used / (1024 * 1024)).max(1);
    let sparkline = Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(format!(
            " Memory: {} / {} (max: {}) | {} ",
            format_bytes(app.metrics.memory_used),
            format_bytes(app.metrics.memory_total),
            format_bytes(app.max_memory_used),
            time_span
        )))
        .data(&data)
        .max(max_mb)
        .style(Style::default().fg(Color::Magenta));
    frame.render_widget(sparkline, area);
}

fn render_cpu_memory_sparklines(frame: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_cpu_sparkline(frame, layout[0], app);
    render_memory_sparkline(frame, layout[1], app);
}

fn render_qps_sparkline(frame: &mut Frame, area: Rect, app: &App) {
    let data: Vec<u64> = app.qps_history.iter().map(|&v| v as u64).collect();
    let max_qps = app.metrics.max_qps.max(1.0);

    let sparkline = Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(format!(
            " QPS: {:.1} (max: {:.1}) | last 60s ",
            app.metrics.qps, max_qps
        )))
        .data(&data)
        .max(max_qps as u64)
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(sparkline, area);
}

fn config_lines(app: &App) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    lines.push(Line::from("Proxy:"));
    if let Some(config) = &app.config {
        lines.push(Line::from(format!(
            "  Listen: {}:{}",
            config.host, config.port
        )));
        lines.push(Line::from(format!(
            "  TLS Interception: {}",
            if config.tls.enable_tls_interception {
                "Enabled"
            } else {
                "Disabled"
            }
        )));
        lines.push(Line::from(format!(
            "  Unsafe SSL: {}",
            config.tls.unsafe_ssl
        )));
        lines.push(Line::from("  Intercept Domains:"));
        lines.push(Line::from(format!(
            "    {}",
            if config.tls.intercept_include.is_empty() {
                "(none)".to_string()
            } else {
                config.tls.intercept_include.join(", ")
            }
        )));
        lines.push(Line::from("  Intercept Apps:"));
        lines.push(Line::from(format!(
            "    {}",
            if config.tls.app_intercept_include.is_empty() {
                "(none)".to_string()
            } else {
                config.tls.app_intercept_include.join(", ")
            }
        )));
    } else {
        lines.push(Line::from("  Loading..."));
    }

    lines.push(Line::from(""));
    lines.push(Line::from("CLI Proxy (ENV):"));
    if let Some(cli) = &app.cli_proxy {
        lines.push(Line::from(format!(
            "  Status: {}",
            if cli.enabled { "Enabled" } else { "Disabled" }
        )));
        lines.push(Line::from(format!("  Proxy URL: {}", cli.proxy_url)));
        lines.push(Line::from(format!("  Shell: {}", cli.shell)));
        lines.push(Line::from(format!(
            "  Config Files: {}",
            cli.config_files.len()
        )));
    } else {
        lines.push(Line::from("  Loading..."));
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Performance:"));
    if let Some(perf) = &app.performance_config {
        lines.push(Line::from(format!(
            "  Max Records: {}",
            perf.traffic.max_records
        )));
        lines.push(Line::from(format!(
            "  Max DB Size: {}",
            format_bytes(perf.traffic.max_db_size_bytes)
        )));
        lines.push(Line::from(format!(
            "  Max Body Inline (DB): {}",
            format_bytes(perf.traffic.max_body_memory_size as u64)
        )));
        lines.push(Line::from(format!(
            "  Max Body Buffer: {}",
            format_bytes(perf.traffic.max_body_buffer_size as u64)
        )));
        lines.push(Line::from(format!(
            "  Retention Days: {}",
            perf.traffic.file_retention_days
        )));
        lines.push(Line::from(format!(
            "  SSE Flush: {} / {}ms",
            format_bytes(perf.traffic.sse_stream_flush_bytes as u64),
            perf.traffic.sse_stream_flush_interval_ms
        )));
        lines.push(Line::from(format!(
            "  WS Flush: {} / {}ms",
            format_bytes(perf.traffic.ws_payload_flush_bytes as u64),
            perf.traffic.ws_payload_flush_interval_ms
        )));
        lines.push(Line::from(format!(
            "  WS Max Files: {}",
            perf.traffic.ws_payload_max_open_files
        )));
        if let Some(stats) = &perf.body_store_stats {
            lines.push(Line::from(format!(
                "  Body Store: {} files, {}",
                stats.file_count,
                format_bytes(stats.total_size)
            )));
        }
        if let Some(stats) = &perf.frame_store_stats {
            lines.push(Line::from(format!(
                "  Frame Store: {} conns, {}",
                stats.connection_count,
                format_bytes(stats.total_size)
            )));
        }
    } else {
        lines.push(Line::from("  Loading..."));
    }

    lines
}

fn render_connection_stats(frame: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    let stats_items = vec![
        ListItem::new(format!("Total Requests: {}", app.metrics.total_requests)),
        ListItem::new(format!(
            "Active Connections: {}",
            app.metrics.active_connections
        )),
        ListItem::new(""),
        ListItem::new(format!("HTTP:   {} reqs", app.metrics.http.requests)),
        ListItem::new(format!("HTTPS:  {} reqs", app.metrics.https.requests)),
        ListItem::new(format!("Tunnel: {} reqs", app.metrics.tunnel.requests)),
        ListItem::new(format!("WS:     {} reqs", app.metrics.ws.requests)),
        ListItem::new(format!("WSS:    {} reqs", app.metrics.wss.requests)),
        ListItem::new(format!("SOCKS5: {} reqs", app.metrics.socks5.requests)),
    ];

    let stats_list =
        List::new(stats_items).block(Block::default().borders(Borders::ALL).title(" Statistics "));
    frame.render_widget(stats_list, layout[0]);

    let enabled_rules: Vec<_> = app.rules.iter().filter(|r| r.enabled).collect();
    let total_rules: usize = enabled_rules.iter().map(|r| r.rule_count).sum();

    let summary_items = vec![
        ListItem::new(format!("Rule Groups: {}", app.rules.len())),
        ListItem::new(format!(
            "  Enabled: {} ({} rules)",
            enabled_rules.len(),
            total_rules
        )),
        ListItem::new(format!(
            "  Disabled: {}",
            app.rules.len() - enabled_rules.len()
        )),
        ListItem::new(""),
        ListItem::new(format!("Values: {}", app.values.len())),
        ListItem::new(format!(
            "Scripts: {} req / {} res",
            app.scripts.request.len(),
            app.scripts.response.len()
        )),
        ListItem::new(""),
        ListItem::new(format!(
            "TLS Interception: {}",
            app.config
                .as_ref()
                .map(|c| if c.tls.enable_tls_interception {
                    "Enabled"
                } else {
                    "Disabled"
                })
                .unwrap_or("N/A")
        )),
        ListItem::new(format!(
            "CLI Proxy: {}",
            app.cli_proxy
                .as_ref()
                .map(|s| if s.enabled { "Enabled" } else { "Disabled" })
                .unwrap_or("N/A")
        )),
    ];

    let summary_list =
        List::new(summary_items).block(Block::default().borders(Borders::ALL).title(" Summary "));
    frame.render_widget(summary_list, layout[1]);

    let config_para = Paragraph::new(config_lines(app)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Configuration "),
    );
    frame.render_widget(config_para, layout[2]);
}

fn render_rules_config(frame: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(layout[0]);

    let rules_rows: Vec<Row> = app
        .rules
        .iter()
        .map(|r| {
            let status = if r.enabled { "●" } else { "○" };
            let style = if r.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Row::new(vec![
                status.to_string(),
                r.name.clone(),
                r.rule_count.to_string(),
            ])
            .style(style)
        })
        .collect();

    let rules_table = Table::new(
        rules_rows,
        [
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(vec!["", "Name", "Rules"])
            .style(Style::default().fg(Color::Yellow).bold())
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Rule Groups "),
    );
    frame.render_widget(rules_table, left_layout[0]);

    let values_items: Vec<ListItem> = app
        .values
        .iter()
        .take(10)
        .map(|v| ListItem::new(format!("{}: {}", v.name, v.value)))
        .collect();

    let values_list = List::new(values_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Values ({}) ", app.values.len())),
    );
    frame.render_widget(values_list, left_layout[1]);

    let right_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(layout[1]);

    let mut script_items: Vec<ListItem> = Vec::new();
    if !app.scripts.request.is_empty() {
        script_items.push(ListItem::new("Request Scripts:").style(Style::default().bold()));
        for s in &app.scripts.request {
            script_items.push(ListItem::new(format!("  • {}", s.name)));
        }
    }
    if !app.scripts.response.is_empty() {
        script_items.push(ListItem::new("Response Scripts:").style(Style::default().bold()));
        for s in &app.scripts.response {
            script_items.push(ListItem::new(format!("  • {}", s.name)));
        }
    }
    if script_items.is_empty() {
        script_items.push(ListItem::new("No scripts configured"));
    }

    let scripts_list =
        List::new(script_items).block(Block::default().borders(Borders::ALL).title(" Scripts "));
    frame.render_widget(scripts_list, right_layout[0]);

    let config_para = Paragraph::new(config_lines(app)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Configuration "),
    );
    frame.render_widget(config_para, right_layout[1]);
}

fn render_traffic_details(frame: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let protocols = [
        ("HTTP", &app.metrics.http, Color::Blue),
        ("HTTPS", &app.metrics.https, Color::Green),
        ("Tunnel", &app.metrics.tunnel, Color::Yellow),
        ("WebSocket", &app.metrics.ws, Color::Magenta),
        ("WSS", &app.metrics.wss, Color::Cyan),
        ("SOCKS5", &app.metrics.socks5, Color::Red),
    ];

    let rows: Vec<Row> = protocols
        .iter()
        .map(|(name, m, color)| {
            Row::new(vec![
                name.to_string(),
                m.requests.to_string(),
                m.active_connections.to_string(),
                format_bytes(m.bytes_sent),
                format_bytes(m.bytes_received),
            ])
            .style(Style::default().fg(*color))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(15),
            Constraint::Length(15),
        ],
    )
    .header(
        Row::new(vec!["Protocol", "Requests", "Active", "Sent", "Received"])
            .style(Style::default().fg(Color::Yellow).bold())
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Traffic by Protocol "),
    );

    frame.render_widget(table, layout[0]);

    let detail_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(layout[1]);

    let app_rows: Vec<Row> = app
        .app_metrics
        .iter()
        .take(8)
        .map(|m| {
            Row::new(vec![
                m.app_name.clone(),
                m.requests.to_string(),
                m.active_connections.to_string(),
                m.socks5_requests.to_string(),
            ])
        })
        .collect();

    let apps_table = Table::new(
        app_rows,
        [
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Application", "Requests", "Active", "SOCKS5"])
            .style(Style::default().fg(Color::Yellow).bold())
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Top Applications "),
    );
    frame.render_widget(apps_table, detail_layout[0]);

    let host_rows: Vec<Row> = app
        .host_metrics
        .iter()
        .take(8)
        .map(|m| {
            Row::new(vec![
                m.host.clone(),
                m.requests.to_string(),
                m.active_connections.to_string(),
                m.socks5_requests.to_string(),
            ])
        })
        .collect();

    let hosts_table = Table::new(
        host_rows,
        [
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Host", "Requests", "Active", "SOCKS5"])
            .style(Style::default().fg(Color::Yellow).bold())
            .bottom_margin(1),
    )
    .block(Block::default().borders(Borders::ALL).title(" Top Hosts "));
    frame.render_widget(hosts_table, detail_layout[1]);
}

fn render_remote_invoke(frame: &mut Frame, area: Rect, app: &App) {
    // 单张表格最多展示的数据行数上限，避免历史记录过多时一次性渲染过载。
    const MAX_REMOTE_TABLE_ROWS: usize = 200;
    // 命令列在极窄窗口下的兜底最小可读宽度。
    const REMOTE_MIN_CMD_BUDGET: usize = 18;
    // Client 列在极窄窗口下保持与表格 Min 约束一致的兜底宽度。
    const REMOTE_MIN_CLIENT_BUDGET: usize = 12;

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Percentage(48),
            Constraint::Percentage(52),
        ])
        .split(area);

    let status = app.remote_invoke.status.as_ref();
    let state = status
        .map(|value| value.state.as_str())
        .unwrap_or("unavailable");
    let active_calls = status
        .map(|value| value.active_call_ids.len())
        .unwrap_or_default();
    let pending_pairings = status
        .map(|value| value.pending_pairings_count)
        .unwrap_or_default();
    let latest_call = app
        .remote_invoke
        .calls
        .iter()
        .max_by_key(|call| call.started_at);
    let latest_summary = latest_call
        .map(|call| {
            // 自适应：Latest 行占满状态卡片整行，命令预览按卡片宽度截断，
            // 预留 "Latest: " 标签、status、result 与分隔符的空间。
            let reserved = 8 // "Latest: "
                + call.status.chars().count()
                + format_remote_result(call).chars().count()
                + 6 // 两个 " | " 分隔符
                + 2; // 边框
            let cmd_budget = (layout[0].width as usize)
                .saturating_sub(reserved)
                .max(REMOTE_MIN_CMD_BUDGET);
            format!(
                "{} | {} | {}",
                truncate_text(&call.command_summary.command_preview, cmd_budget),
                call.status,
                format_remote_result(call)
            )
        })
        .unwrap_or_else(|| "No remote command history".to_string());

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("State: ", Style::default().fg(Color::DarkGray)),
            Span::styled(state.to_string(), Style::default().fg(Color::Cyan).bold()),
            Span::raw("  "),
            Span::styled("Clients: ", Style::default().fg(Color::DarkGray)),
            Span::raw(app.remote_invoke.grants.len().to_string()),
            Span::raw("  "),
            Span::styled("Active Calls: ", Style::default().fg(Color::DarkGray)),
            Span::raw(active_calls.to_string()),
            Span::raw("  "),
            Span::styled("Pending Pairings: ", Style::default().fg(Color::DarkGray)),
            Span::raw(pending_pairings.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Latest: ", Style::default().fg(Color::DarkGray)),
            Span::raw(latest_summary),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Remote Invoke Status "),
    );
    frame.render_widget(summary, layout[0]);

    // 自适应：根据 Connected Clients 表格的实际高度/宽度推导可见行数与列宽。
    // Client 列有信息密度上限，命令列不设上限并吸收剩余空间。
    const CLIENTS_FIXED_TOTAL: u16 = 10 + 18 + 10 + 11 + 11 + 12; // Auth+Scope+Grant+Connected+Last Cmd+Status
    const CLIENTS_FLEX_SPECS: [AdaptiveColumnSpec; 3] = [
        AdaptiveColumnSpec {
            min: 12,
            max: Some(36),
            weight: 1,
        },
        AdaptiveColumnSpec {
            min: 24,
            max: None,
            weight: 6,
        },
        AdaptiveColumnSpec {
            min: 18,
            max: Some(30),
            weight: 2,
        },
    ]; // Client, Latest Command, Result
    let client_capacity = visible_table_rows(layout[1].height, 2, MAX_REMOTE_TABLE_ROWS);
    let client_widths = adaptive_column_widths(
        layout[1].width,
        CLIENTS_FIXED_TOTAL,
        9, // 表格总列数
        &CLIENTS_FLEX_SPECS,
    );
    let client_label_width = client_widths
        .first()
        .copied()
        .unwrap_or(REMOTE_MIN_CLIENT_BUDGET as u16);
    let client_cmd_width = client_widths
        .get(1)
        .copied()
        .unwrap_or(REMOTE_MIN_CMD_BUDGET as u16);
    let client_result_width = client_widths.get(2).copied().unwrap_or(18);
    let client_label_budget = client_label_width as usize;
    let client_cmd_budget = client_cmd_width as usize;
    let client_result_budget = client_result_width as usize;

    let client_rows: Vec<Row> = app
        .remote_invoke
        .grants
        .iter()
        .take(client_capacity)
        .map(|grant| {
            let latest = latest_call_for_grant(grant, &app.remote_invoke.calls);
            let command = latest
                .map(|call| truncate_text(&call.command_summary.command_preview, client_cmd_budget))
                .unwrap_or_else(|| "-".to_string());
            let call_status = latest
                .map(|call| call.status.clone())
                .unwrap_or_else(|| "-".to_string());
            let result = latest
                .map(format_remote_result)
                .map(|value| truncate_text(&value, client_result_budget))
                .unwrap_or_else(|| "-".to_string());
            Row::new(vec![
                caller_label_with_budget(
                    grant.caller_display_name.as_ref(),
                    grant.label.as_ref(),
                    &grant.caller_fingerprint,
                    client_label_budget,
                ),
                format_remote_auth(grant.auth_method.as_ref()),
                grant.grant_scope.clone().unwrap_or_else(|| "-".to_string()),
                grant.status.clone(),
                format_timestamp_millis(grant.first_connected_at.or(grant.created_at)),
                format_timestamp_millis(grant.last_command_at.or(grant.last_used_at)),
                command,
                call_status,
                result,
            ])
        })
        .collect();

    let clients_table = Table::new(
        client_rows,
        [
            Constraint::Length(client_label_width),
            Constraint::Length(10),
            Constraint::Length(18),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(client_cmd_width),
            Constraint::Length(12),
            Constraint::Length(client_result_width),
        ],
    )
    .header(
        Row::new(vec![
            "Client",
            "Auth",
            "Scope",
            "Grant",
            "Connected",
            "Last Cmd",
            "Latest Command",
            "Status",
            "Result",
        ])
        .style(Style::default().fg(Color::Yellow).bold())
        .bottom_margin(1),
    )
    .block(Block::default().borders(Borders::ALL).title(format!(
        " Connected Clients ({}) ",
        app.remote_invoke.grants.len()
    )));
    frame.render_widget(clients_table, layout[1]);

    // 自适应：Recent Commands 表格同样限制 Client 列，把宽屏富余空间优先给 Command。
    const CALLS_FIXED_TOTAL: u16 = 10 + 12 + 11; // Auth+Status+Started
    const CALLS_FLEX_SPECS: [AdaptiveColumnSpec; 4] = [
        AdaptiveColumnSpec {
            min: 12,
            max: Some(36),
            weight: 1,
        },
        AdaptiveColumnSpec {
            min: 32,
            max: None,
            weight: 7,
        },
        AdaptiveColumnSpec {
            min: 20,
            max: Some(30),
            weight: 2,
        },
        AdaptiveColumnSpec {
            min: 12,
            max: Some(24),
            weight: 1,
        },
    ]; // Client, Command, Result, Call ID
    let call_capacity = visible_table_rows(layout[2].height, 2, MAX_REMOTE_TABLE_ROWS);
    let call_widths = adaptive_column_widths(
        layout[2].width,
        CALLS_FIXED_TOTAL,
        7, // 表格总列数
        &CALLS_FLEX_SPECS,
    );
    let call_client_width = call_widths
        .first()
        .copied()
        .unwrap_or(REMOTE_MIN_CLIENT_BUDGET as u16);
    let call_cmd_width = call_widths
        .get(1)
        .copied()
        .unwrap_or(REMOTE_MIN_CMD_BUDGET as u16);
    let call_result_width = call_widths.get(2).copied().unwrap_or(20);
    let call_id_width = call_widths.get(3).copied().unwrap_or(12);
    let call_client_budget = call_client_width as usize;
    let call_cmd_budget = call_cmd_width as usize;
    let call_result_budget = call_result_width as usize;
    let call_id_budget = call_id_width as usize;

    let call_rows: Vec<Row> = app
        .remote_invoke
        .calls
        .iter()
        .take(call_capacity)
        .map(|call| {
            Row::new(vec![
                caller_label_with_budget(
                    call.caller_display_name.as_ref(),
                    None,
                    &call.caller_fingerprint,
                    call_client_budget,
                ),
                format_remote_auth(call.auth_method.as_ref()),
                truncate_text(&call.command_summary.command_preview, call_cmd_budget),
                call.status.clone(),
                truncate_text(&format_remote_result(call), call_result_budget),
                format_timestamp_millis(Some(call.started_at)),
                truncate_text(&call.call_id, call_id_budget),
            ])
        })
        .collect();

    let calls_table = Table::new(
        call_rows,
        [
            Constraint::Length(call_client_width),
            Constraint::Length(10),
            Constraint::Length(call_cmd_width),
            Constraint::Length(12),
            Constraint::Length(call_result_width),
            Constraint::Length(11),
            Constraint::Length(call_id_width),
        ],
    )
    .header(
        Row::new(vec![
            "Client", "Auth", "Command", "Status", "Result", "Started", "Call ID",
        ])
        .style(Style::default().fg(Color::Yellow).bold())
        .bottom_margin(1),
    )
    .block(Block::default().borders(Borders::ALL).title(format!(
        " Recent Commands ({}) ",
        app.remote_invoke.calls.len()
    )));
    frame.render_widget(calls_table, layout[2]);
}

fn render_footer(frame: &mut Frame, area: Rect) {
    let help = Line::from(vec![
        Span::styled(" q ", Style::default().fg(Color::Yellow).bold()),
        Span::raw("Quit  "),
        Span::styled(" ←/→ ", Style::default().fg(Color::Yellow).bold()),
        Span::raw("Switch Tab  "),
        Span::styled(" r ", Style::default().fg(Color::Yellow).bold()),
        Span::raw("Refresh  "),
    ]);

    let footer = Paragraph::new(help).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_admin::push::{MetricsData, PushMessage, SettingsUpdateData};
    use ratatui::backend::TestBackend;
    use serde_json::json;

    #[test]
    fn parses_metrics_update_from_push_message() {
        let message = serde_json::to_string(&PushMessage::MetricsUpdate(MetricsData {
            metrics: json!({
                "timestamp": 1,
                "memory_used": 2,
                "memory_total": 3,
                "cpu_usage": 4.5,
                "total_requests": 6,
                "active_connections": 7,
                "bytes_sent": 8,
                "bytes_received": 9,
                "bytes_sent_rate": 10.0,
                "bytes_received_rate": 11.0,
                "qps": 12.0,
                "max_qps": 13.0,
                "max_bytes_sent_rate": 14.0,
                "max_bytes_received_rate": 15.0,
                "http": {"requests": 1, "bytes_sent": 2, "bytes_received": 3, "active_connections": 4},
                "https": {"requests": 0, "bytes_sent": 0, "bytes_received": 0, "active_connections": 0},
                "tunnel": {"requests": 0, "bytes_sent": 0, "bytes_received": 0, "active_connections": 0},
                "ws": {"requests": 0, "bytes_sent": 0, "bytes_received": 0, "active_connections": 0},
                "wss": {"requests": 0, "bytes_sent": 0, "bytes_received": 0, "active_connections": 0},
                "socks5": {"requests": 0, "bytes_sent": 0, "bytes_received": 0, "active_connections": 0}
            }),
        }))
        .expect("serialize push message");

        let event = parse_push_message(&message).expect("parse push message");
        match event {
            TuiPushEvent::Metrics(metrics) => {
                assert_eq!(metrics.memory_used, 2);
                assert_eq!(metrics.qps, 12.0);
            }
            _ => panic!("expected metrics event"),
        }
    }

    #[test]
    fn parses_performance_settings_update_from_push_message() {
        let message = serde_json::to_string(&PushMessage::SettingsUpdate(SettingsUpdateData {
            scope: SETTINGS_SCOPE_PERFORMANCE_CONFIG.to_string(),
            data: json!({
                "traffic": {
                    "max_records": 2048,
                    "max_db_size_bytes": 1048576,
                    "max_body_memory_size": 65536,
                    "max_body_buffer_size": 10485760,
                    "max_body_probe_size": 65536,
                    "binary_traffic_performance_mode": true,
                    "file_retention_days": 7,
                    "sse_stream_flush_bytes": 262144,
                    "sse_stream_flush_interval_ms": 250,
                    "ws_payload_flush_bytes": 262144,
                    "ws_payload_flush_interval_ms": 250,
                    "ws_payload_max_open_files": 64
                },
                "body_store_stats": {
                    "file_count": 1,
                    "total_size": 128
                },
                "frame_store_stats": {
                    "connection_count": 0,
                    "total_size": 0
                }
            }),
        }))
        .expect("serialize settings update");

        let event = parse_push_message(&message).expect("parse push message");
        match event {
            TuiPushEvent::Performance(config) => {
                assert_eq!(config.traffic.max_records, 2048);
                assert_eq!(
                    config
                        .body_store_stats
                        .expect("body stats should be present")
                        .file_count,
                    1
                );
            }
            _ => panic!("expected performance event"),
        }
    }

    #[test]
    fn format_bytes_formats_kb_mb_and_gb() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1_024), "1.00 KB");
        assert_eq!(format_bytes(1_048_576), "1.00 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.00 GB");
    }

    #[test]
    fn format_rate_formats_small_and_large_values() {
        assert_eq!(format_rate(128.0), "128 B/s");
        assert_eq!(format_rate(1_024.0), "1.00 KB/s");
        assert_eq!(format_rate(1_048_576.0), "1.00 MB/s");
    }

    #[test]
    fn format_time_span_formats_seconds_minutes_and_hours() {
        assert_eq!(format_time_span(30), "30s");
        assert_eq!(format_time_span(90), "1m30s");
        assert_eq!(format_time_span(3_600), "1h0m");
        assert_eq!(format_time_span(3_660), "1h1m");
    }

    #[test]
    fn visible_table_rows_scales_with_height() {
        // 高度 = 边框(2) + 表头(2) + 数据行。
        // 矮窗口：3 行总高，去掉边框与表头后没有可用数据行。
        assert_eq!(visible_table_rows(3, 2, 200), 0);
        // 边界：刚好放下表头，无数据行。
        assert_eq!(visible_table_rows(4, 2, 200), 0);
        // 普通窗口：高度 20 → 20-2-2=16 行可见。
        assert_eq!(visible_table_rows(20, 2, 200), 16);
        // 超大窗口：高度 1000 → 996 行但被 max_rows 上限钳制。
        assert_eq!(visible_table_rows(1000, 2, 200), 200);
        // 极端矮：高度 0 不应 panic。
        assert_eq!(visible_table_rows(0, 2, 200), 0);
    }

    #[test]
    fn adaptive_column_widths_caps_client_and_expands_command() {
        let fixed = 10 + 18 + 10 + 11 + 11 + 12;
        let specs = [
            AdaptiveColumnSpec {
                min: 12,
                max: Some(36),
                weight: 1,
            },
            AdaptiveColumnSpec {
                min: 24,
                max: None,
                weight: 6,
            },
            AdaptiveColumnSpec {
                min: 18,
                max: Some(30),
                weight: 2,
            },
        ];

        let widths = adaptive_column_widths(240, fixed, 9, &specs);

        assert!(
            widths[0] <= 36,
            "Client column should stay within its cap: {widths:?}"
        );
        assert!(
            widths[1] > widths[0],
            "Command column should absorb most wide-screen space: {widths:?}"
        );
        assert!(
            widths[2] > 18,
            "Result column should grow beyond the old fixed width: {widths:?}"
        );
    }

    #[test]
    fn adaptive_column_widths_keep_command_monotonic_after_client_cap() {
        let fixed = 10 + 12 + 11;
        let specs = [
            AdaptiveColumnSpec {
                min: 12,
                max: Some(36),
                weight: 1,
            },
            AdaptiveColumnSpec {
                min: 32,
                max: None,
                weight: 7,
            },
            AdaptiveColumnSpec {
                min: 20,
                max: Some(30),
                weight: 2,
            },
            AdaptiveColumnSpec {
                min: 12,
                max: Some(24),
                weight: 1,
            },
        ];

        let wide = adaptive_column_widths(220, fixed, 7, &specs);
        let wider = adaptive_column_widths(320, fixed, 7, &specs);

        assert!(
            wide[0] <= 36,
            "Client column should stay within its cap: {wide:?}"
        );
        assert!(
            wider[0] <= 36,
            "Client cap should hold as width grows: {wider:?}"
        );
        assert!(
            wider[1] > wide[1],
            "Command column should keep expanding after capped columns stop: {wide:?} -> {wider:?}"
        );
        assert!(wider[3] >= wide[3], "Call ID width should not shrink");
    }

    #[test]
    fn adaptive_column_widths_handles_narrow_and_degenerate_inputs() {
        let specs = [
            AdaptiveColumnSpec {
                min: 12,
                max: Some(36),
                weight: 1,
            },
            AdaptiveColumnSpec {
                min: 24,
                max: None,
                weight: 6,
            },
        ];

        assert_eq!(adaptive_column_widths(0, 90, 9, &specs), vec![12, 24]);
        assert_eq!(adaptive_column_widths(100, 90, 9, &specs), vec![12, 24]);
        assert!(adaptive_column_widths(200, 50, 9, &[]).is_empty());
    }

    #[test]
    fn remote_invoke_latest_call_matches_grant_and_prefers_newest() {
        let grant = RemoteInvokeGrant {
            grant_id: "grant-a".into(),
            caller_fingerprint: "caller-a".into(),
            status: "active".into(),
            ..Default::default()
        };
        let calls = vec![
            RemoteInvokeCall {
                call_id: "older".into(),
                grant_id: "grant-a".into(),
                caller_fingerprint: "caller-a".into(),
                command_summary: RemoteInvokeCommandSummary {
                    command_preview: "status".into(),
                },
                status: "completed".into(),
                started_at: 1_000,
                ..Default::default()
            },
            RemoteInvokeCall {
                call_id: "newer".into(),
                grant_id: "rotated-grant".into(),
                caller_fingerprint: "caller-a".into(),
                command_summary: RemoteInvokeCommandSummary {
                    command_preview: "shell.exec pwd".into(),
                },
                status: "streaming".into(),
                started_at: 2_000,
                ..Default::default()
            },
            RemoteInvokeCall {
                call_id: "other".into(),
                grant_id: "grant-b".into(),
                caller_fingerprint: "caller-b".into(),
                command_summary: RemoteInvokeCommandSummary {
                    command_preview: "traffic list".into(),
                },
                status: "completed".into(),
                started_at: 3_000,
                ..Default::default()
            },
        ];

        let latest = latest_call_for_grant(&grant, &calls).expect("latest call");

        assert_eq!(latest.call_id, "newer");
        assert_eq!(latest.command_summary.command_preview, "shell.exec pwd");
    }

    #[test]
    fn remote_invoke_result_formats_terminal_and_running_calls() {
        let completed = RemoteInvokeCall {
            status: "completed".into(),
            exit_code: Some(0),
            duration_ms: Some(1_500),
            bytes_out: Some(2_048),
            ..Default::default()
        };
        assert_eq!(
            format_remote_result(&completed),
            "exit 0 / 1s / out 2.00 KB"
        );

        let running = RemoteInvokeCall {
            status: "streaming".into(),
            ended_at: None,
            ..Default::default()
        };
        assert_eq!(format_remote_result(&running), "running");
    }

    #[test]
    fn remote_invoke_labels_prefer_human_names_and_normalize_auth() {
        assert_eq!(
            caller_label_with_budget(
                Some(&"Laptop".to_string()),
                Some(&"SSH Agent".to_string()),
                "abcdef0123456789",
                12,
            ),
            "Laptop"
        );
        assert_eq!(
            caller_label_with_budget(None, Some(&"SSH Agent".to_string()), "abcdef0123456789", 12),
            "SSH Agent"
        );
        assert_eq!(
            caller_label_with_budget(None, None, "caller-abcdef0123456789", 12),
            "caller-abcd..."
        );
        assert_eq!(
            caller_label_with_budget(None, None, "caller-abcdef0123456789", 24),
            "caller-abcdef0123456789"
        );
        assert_eq!(
            caller_label_with_budget(
                Some(&"Very Long Client Label".to_string()),
                None,
                "ignored",
                12
            ),
            "Very Long C..."
        );
        assert_eq!(
            format_remote_auth(Some(&"ssh_publickey".to_string())),
            "SSH key"
        );
        assert_eq!(
            format_remote_auth(Some(&"pair_code".to_string())),
            "Pair code"
        );
    }

    fn buffer_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect()
    }

    #[test]
    fn remote_invoke_render_caps_client_and_expands_trailing_columns() {
        let mut app = dummy_app();
        app.remote_invoke.status = Some(RemoteInvokeStatus {
            state: "connected".into(),
            ..Default::default()
        });
        app.remote_invoke.grants = vec![RemoteInvokeGrant {
            grant_id: "grant-wide".into(),
            caller_fingerprint: "caller-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            auth_method: Some("ssh_publickey".into()),
            grant_scope: Some("remote_shell_interactive".into()),
            status: "active".into(),
            first_connected_at: Some(1_700_000_000_000),
            last_command_at: Some(1_700_000_100_000),
            ..Default::default()
        }];
        app.remote_invoke.calls = vec![RemoteInvokeCall {
            call_id: "call-0123456789abcdef0123456789abcdef".into(),
            grant_id: "grant-wide".into(),
            caller_fingerprint: "caller-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            caller_display_name: None,
            auth_method: Some("ssh_publickey".into()),
            command_summary: RemoteInvokeCommandSummary {
                command_preview: "file.search json_response\\b /Users/eden/work/github/bifrost/crates/bifrost-cli/src/commands/status_tui.rs".into(),
            },
            status: "completed".into(),
            started_at: 1_700_000_100_000,
            ended_at: Some(1_700_000_112_000),
            exit_code: Some(0),
            duration_ms: Some(12_000),
            bytes_out: Some(65_536),
        }];

        let backend = TestBackend::new(220, 45);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_remote_invoke(frame, frame.area(), &app))
            .expect("draw remote invoke");

        let screen = buffer_lines(&terminal).join("\n");
        assert!(screen.contains("Latest Command"));
        assert!(screen.contains("file.search json_response"));
        assert!(screen.contains("64.00 KB"));
        assert!(screen.contains("call-0123456789abcdef"));

        let auth_columns = buffer_lines(&terminal)
            .into_iter()
            .filter(|line| line.contains("caller-") && line.contains("SSH key"))
            .map(|line| line.find("SSH key").expect("auth column"))
            .collect::<Vec<_>>();
        assert!(
            !auth_columns.is_empty(),
            "expected rendered caller rows in screen:\n{screen}"
        );
        let max_auth_column = auth_columns.into_iter().max().unwrap();
        assert!(
            max_auth_column <= 42,
            "Client column should be capped before auth column; auth starts at {max_auth_column}\n{screen}"
        );
    }

    fn dummy_app() -> App {
        let (_tx, rx) = mpsc::channel();
        App {
            port: 9900,
            push_port: Arc::new(AtomicU16::new(9900)),
            push_rx: rx,
            push_connected: false,
            last_push_event: None,
            is_running: true,
            pid: Some(1234),
            metrics: MetricsSnapshot::default(),
            qps_history: vec![0.0; QPS_HISTORY_SIZE],
            cpu_history: vec![0.0; CPU_HISTORY_SIZE],
            max_cpu: 0.0,
            memory_used_history: vec![0; CPU_HISTORY_SIZE],
            max_memory_used: 0,
            app_metrics: Vec::new(),
            host_metrics: Vec::new(),
            rules: Vec::new(),
            values: Vec::new(),
            scripts: ScriptsResponse {
                request: Vec::new(),
                response: Vec::new(),
            },
            config: None,
            performance_config: None,
            cli_proxy: None,
            remote_invoke: RemoteInvokeSnapshot::default(),
            selected_tab: 0,
            last_process_check: Instant::now(),
            last_update: Instant::now(),
            last_slow_refresh: Instant::now(),
            refresh_count: 0,
        }
    }

    #[test]
    fn apply_metrics_snapshot_updates_histories_and_maxima() {
        let mut app = dummy_app();
        let snapshot = MetricsSnapshot {
            qps: 42.0,
            cpu_usage: 77.5,
            memory_used: 1024 * 1024,
            memory_total: 2 * 1024 * 1024,
            ..Default::default()
        };

        app.apply_metrics_snapshot(snapshot);

        assert_eq!(app.metrics.qps, 42.0);
        assert_eq!(app.qps_history.last().copied(), Some(42.0));
        assert_eq!(app.cpu_history.last().copied(), Some(77.5));
        assert_eq!(app.memory_used_history.last().copied(), Some(1024 * 1024));
        assert_eq!(app.max_cpu, 77.5);
        assert_eq!(app.max_memory_used, 1024 * 1024);
    }

    #[test]
    fn push_is_healthy_checks_connection_and_staleness() {
        let mut app = dummy_app();
        assert!(!app.push_is_healthy());

        app.push_connected = true;
        app.last_push_event = Some(Instant::now());
        assert!(app.push_is_healthy());

        app.last_push_event = Some(Instant::now() - PUSH_STALE_TIMEOUT - Duration::from_secs(1));
        assert!(!app.push_is_healthy());
    }

    #[test]
    fn config_lines_include_proxy_cli_and_performance_sections() {
        let mut app = dummy_app();
        app.config = Some(ConfigResponse {
            tls: TlsConfig {
                enable_tls_interception: true,
                intercept_include: vec!["example.com".into()],
                app_intercept_include: vec![],
                unsafe_ssl: false,
            },
            port: 9900,
            host: "127.0.0.1".into(),
        });
        app.cli_proxy = Some(CliProxyStatus {
            enabled: true,
            shell: "bash".into(),
            config_files: vec!["/tmp/proxy.conf".into()],
            proxy_url: "http://127.0.0.1:9900".into(),
        });
        app.performance_config = Some(PerformanceConfigResponse {
            traffic: TrafficConfig {
                max_records: 1024,
                max_db_size_bytes: 1_048_576,
                max_body_memory_size: 65_536,
                max_body_buffer_size: 1_048_576,
                max_body_probe_size: 8_192,
                binary_traffic_performance_mode: true,
                file_retention_days: 7,
                sse_stream_flush_bytes: 4_096,
                sse_stream_flush_interval_ms: 250,
                ws_payload_flush_bytes: 4_096,
                ws_payload_flush_interval_ms: 250,
                ws_payload_max_open_files: 16,
            },
            body_store_stats: Some(BodyStoreStats {
                file_count: 1,
                total_size: 2048,
            }),
            frame_store_stats: None,
        });

        let lines = config_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(lines.iter().any(|l| l.contains("Listen: 127.0.0.1:9900")));
        assert!(lines
            .iter()
            .any(|l| l.contains("TLS Interception: Enabled")));
        assert!(lines.iter().any(|l| l.contains("CLI Proxy (ENV):")));
        assert!(lines.iter().any(|l| l.contains("Performance:")));
    }
}
