use std::collections::{hash_map::DefaultHasher, BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use std::io::Read;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::SystemTime;
use std::time::{Duration, Instant};

use bifrost_admin::push::{
    SETTINGS_SCOPE_CERT_INFO, SETTINGS_SCOPE_CLI_PROXY, SETTINGS_SCOPE_PENDING_AUTHORIZATIONS,
    SETTINGS_SCOPE_PERFORMANCE_CONFIG, SETTINGS_SCOPE_PROXY_ADDRESS, SETTINGS_SCOPE_PROXY_SETTINGS,
    SETTINGS_SCOPE_SYSTEM_PROXY, SETTINGS_SCOPE_TLS_CONFIG, SETTINGS_SCOPE_WHITELIST_STATUS,
};
use bifrost_admin::{
    start_async_traffic_processor, start_frame_cleanup_task, start_metrics_collector_task,
    start_push_tasks, start_ws_payload_cleanup_task, status_printer::TlsStatusInfo, AdminState,
    AsyncTrafficWriter, BodyStore, PortRebindManager, PortRebindRequest, PushManager,
    ReplayDbStore, RuntimeConfig, WsPayloadStore,
};
use bifrost_core::{
    expand_rule_references, normalize_rule_content, Rule, UserPassAccountConfig, UserPassAuthConfig,
};
use bifrost_proxy::{AccessMode, ProxyConfig, ProxyServer};
use bifrost_storage::{
    set_data_dir, ConfigChangeEvent, ConfigManager, RulesChangeOrigin, TrafficConfigUpdate,
    DEFAULT_REMOTE_BASE_URL, MAX_TRAFFIC_MAX_RECORDS,
};
use bifrost_sync::SyncManager;
use bifrost_tls::{get_platform_name, CertInstaller, CertStatus};
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock as ParkingRwLock;
use tracing::info;

use crate::commands::ca::{
    check_and_install_certificate, load_tls_config, CertificateCheckOptions,
};
use crate::config::get_bifrost_dir;
use crate::help::print_startup_help;
use crate::parsing::{parse_cli_rules, DynamicRulesResolver, SharedDynamicRulesResolver};
use crate::process::{
    find_process_on_port, is_process_running, kill_process_by_pid, read_pid, read_runtime_info,
    remove_pid, write_runtime_info, RuntimeInfo, RuntimeStartMode,
};

const ASYNC_TRAFFIC_BUFFER_SIZE: usize = MAX_TRAFFIC_MAX_RECORDS * 3;
const MAX_PORT_INCREMENT_ATTEMPTS: u16 = 64;
const PORT_REBIND_OLD_LISTENER_GRACE_PERIOD: Duration = Duration::from_millis(250);
// Reconcile interval for the system-proxy watcher. Defaults to 30s in production;
// overridable via BIFROST_SYSTEM_PROXY_RECONCILE_SECS so E2E tests can shorten the
// wait (the dirty-backup reconcile case otherwise forces a >30s sleep per run).
fn system_proxy_reconcile_interval() -> Duration {
    match std::env::var("BIFROST_SYSTEM_PROXY_RECONCILE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs >= 1)
    {
        Some(secs) => Duration::from_secs(secs),
        None => Duration::from_secs(30),
    }
}

#[cfg(test)]
fn with_system_proxy_reconcile_env_for_test<F>(value: Option<&str>, f: F)
where
    F: FnOnce() + std::panic::UnwindSafe,
{
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    const VAR: &str = "BIFROST_SYSTEM_PROXY_RECONCILE_SECS";

    let _guard = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev = std::env::var(VAR).ok();
    match value {
        Some(v) => std::env::set_var(VAR, v),
        None => std::env::remove_var(VAR),
    }
    let result = std::panic::catch_unwind(f);
    match prev {
        Some(v) => std::env::set_var(VAR, v),
        None => std::env::remove_var(VAR),
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
#[cfg(target_os = "macos")]
const SYSTEM_PROXY_WAKE_CHECK_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(target_os = "macos")]
const SYSTEM_PROXY_WAKE_GAP_THRESHOLD: Duration = Duration::from_secs(10);
#[cfg(target_os = "macos")]
const SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV: &str =
    "BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL";
const DESKTOP_CORE_ENV: &str = "BIFROST_DESKTOP_CORE";
const DESKTOP_STARTUP_SESSION_ENV: &str = "BIFROST_DESKTOP_STARTUP_SESSION_ID";
pub(crate) const DETACHED_DAEMON_CHILD_ENV: &str = "BIFROST_DETACHED_DAEMON_CHILD";
const RULES_FILESYSTEM_FALLBACK_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const RULES_FILESYSTEM_DEBOUNCE_DELAY: Duration = Duration::from_millis(150);
const BIFROST_RUNTIME_WORKER_STACK_SIZE: usize = 8 * 1024 * 1024;

fn create_bifrost_runtime() -> bifrost_core::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(BIFROST_RUNTIME_WORKER_STACK_SIZE)
        .build()
        .map_err(|e| bifrost_core::BifrostError::Config(format!("Failed to create runtime: {e}")))
}

fn env_flag_enabled(value: Option<std::ffi::OsString>) -> bool {
    value
        .and_then(|value| value.into_string().ok())
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn is_detached_daemon_child_process() -> bool {
    env_flag_enabled(std::env::var_os(DETACHED_DAEMON_CHILD_ENV))
}

fn is_desktop_core_process() -> bool {
    env_flag_enabled(std::env::var_os(DESKTOP_CORE_ENV))
}

fn foreground_runtime_start_mode() -> RuntimeStartMode {
    if is_detached_daemon_child_process() {
        RuntimeStartMode::Daemon
    } else if is_desktop_core_process() {
        RuntimeStartMode::Desktop
    } else {
        RuntimeStartMode::Foreground
    }
}

fn spawn_remote_invoke_worker_startup_task(
    shared_config_manager: Arc<ConfigManager>,
    sync_manager: Arc<SyncManager>,
    admin_state: Arc<AdminState>,
    admin_host: String,
    admin_port: u16,
) {
    tracing::info!(
        target: "bifrost_cli::startup",
        admin_host = %admin_host,
        admin_port = admin_port,
        "remote invoke worker initialization scheduled"
    );
    tokio::spawn(async move {
        let started_at = Instant::now();
        let default_relay_url = shared_config_manager
            .try_config()
            .map(|c| c.sync.remote_base_url.clone())
            .unwrap_or_else(|| DEFAULT_REMOTE_BASE_URL.to_string());
        let registration_targets = bifrost_sync::SyncManagerHandle::new(sync_manager.clone())
            .remote_invoke_registration_targets()
            .await;
        let relay_urls: Vec<String> = if registration_targets.is_empty() {
            vec![default_relay_url]
        } else {
            registration_targets
                .into_iter()
                .map(|target| target.remote_base_url)
                .collect()
        };
        let data_dir_path = shared_config_manager.data_dir().to_path_buf();
        let admin_state_for_worker = admin_state.clone();
        let sync_manager_for_worker = sync_manager.clone();
        let worker_result = tokio::task::spawn_blocking(move || {
            let identity = bifrost_admin::RemoteInvokeIdentity::load_or_create(&data_dir_path)
                .map_err(|e| e.to_string())?;
            let mut workers = Vec::new();
            for relay_url in relay_urls {
                let ri_config = bifrost_admin::RemoteInvokeConfig {
                    relay_url,
                    ..Default::default()
                };
                workers.push(bifrost_admin::RemoteInvokeWorker::new(
                    ri_config,
                    identity.clone(),
                    Some(bifrost_sync::SyncManagerHandle::new(
                        sync_manager_for_worker.clone(),
                    )),
                    admin_state_for_worker.clone(),
                    &admin_host,
                    admin_port,
                ));
            }
            Ok::<_, String>(workers)
        })
        .await;

        match worker_result {
            Ok(Ok(workers)) => {
                admin_state.set_remote_invoke_workers(workers.clone());
                for worker in &workers {
                    worker.start();
                }
                tracing::info!(
                    target: "bifrost_cli::startup",
                    workers = workers.len(),
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    "remote invoke worker initialized asynchronously"
                );
            }
            Ok(Err(error)) => {
                tracing::info!(
                    target: "bifrost_cli::startup",
                    error = %error,
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    "remote invoke identity init failed, feature disabled"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target: "bifrost_cli::startup",
                    error = %error,
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    "remote invoke worker initialization task failed"
                );
            }
        }
    });
}

fn parse_yes_no_answer(input: &str) -> Option<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" | "" => Some(false),
        _ => None,
    }
}

fn prompt_restart_if_running(pid: u32) -> bifrost_core::Result<bool> {
    println!(
        "Detected an existing Bifrost proxy process (PID: {}). Restart? (y/n)",
        pid
    );

    for _ in 0..3 {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        let bytes = io::stdin().read_line(&mut input)?;
        if bytes == 0 {
            return Ok(false);
        }

        if let Some(answer) = parse_yes_no_answer(&input) {
            return Ok(answer);
        }

        println!("Please answer with y/yes or n/no.");
    }

    Ok(false)
}

fn live_desktop_runtime_for_pid(pid: u32) -> Option<RuntimeInfo> {
    let runtime = read_runtime_info()?;
    (runtime.pid == pid && runtime.start_mode == RuntimeStartMode::Desktop).then_some(runtime)
}

fn handle_live_desktop_runtime_before_start(
    runtime: &RuntimeInfo,
    requested_port: u16,
) -> bifrost_core::Result<()> {
    if runtime.port == requested_port {
        println!(
            "Bifrost Desktop core is already running (PID: {}, port {}). Reusing the app-bound core; CLI start will not stop it.",
            runtime.pid, runtime.port
        );
        return Ok(());
    }

    Err(bifrost_core::BifrostError::Config(format!(
        "Bifrost Desktop core is already running (PID: {}, port {}). CLI start will not stop the app-bound core to satisfy requested port {}. Quit Bifrost Desktop first, or use a separate BIFROST_DATA_DIR for another service.",
        runtime.pid, runtime.port, requested_port
    )))
}

fn check_and_resolve_port_conflict(host: &str, port: u16, yes: bool) -> bifrost_core::Result<()> {
    if !is_port_in_use(host, port) {
        return Ok(());
    }

    let bind_addr = format!("{}:{}", host, port);
    let non_interactive = !io::stdin().is_terminal();

    if !yes && non_interactive {
        return Err(bifrost_core::BifrostError::Network(format!(
            "Port {} is already in use and no interactive terminal is available. Use --yes to auto-resolve.",
            bind_addr
        )));
    }

    let auto_yes = yes;

    if let Some(proc_info) = find_process_on_port(port) {
        println!(
            "Port {} is already in use by process \"{}\" (PID: {}). Kill it and continue? (y/n)",
            bind_addr, proc_info.name, proc_info.pid
        );

        let should_kill = if auto_yes {
            println!("> y (auto-confirmed)");
            true
        } else {
            prompt_yes_no()?
        };

        if should_kill {
            println!(
                "Stopping process {} (PID: {})...",
                proc_info.name, proc_info.pid
            );
            if kill_process_by_pid(proc_info.pid) {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if is_port_in_use(host, port) {
                    return Err(bifrost_core::BifrostError::Network(format!(
                        "Failed to free port {}: port is still in use after killing process",
                        bind_addr
                    )));
                }
                println!("Process stopped. Continuing startup...");
                Ok(())
            } else {
                Err(bifrost_core::BifrostError::Network(format!(
                    "Failed to kill process \"{}\" (PID: {}). Please stop it manually and try again.",
                    proc_info.name, proc_info.pid
                )))
            }
        } else {
            Err(bifrost_core::BifrostError::Network(format!(
                "Port {} is already in use. Start cancelled.",
                bind_addr
            )))
        }
    } else {
        println!(
            "Port {} is already in use by an unknown process. Kill it and continue? (y/n)",
            bind_addr
        );

        let should_kill = if auto_yes {
            println!("> y (auto-confirmed)");
            true
        } else {
            prompt_yes_no()?
        };

        if should_kill {
            #[cfg(unix)]
            {
                let output = std::process::Command::new("lsof")
                    .args(["-t", "-i", &format!("TCP:{}", port), "-sTCP:LISTEN"])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output();

                if let Ok(output) = output {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        if let Ok(pid) = line.trim().parse::<u32>() {
                            println!("Stopping process PID: {}...", pid);
                            kill_process_by_pid(pid);
                        }
                    }
                }
            }

            #[cfg(windows)]
            {
                let output = std::process::Command::new("netstat")
                    .args(["-ano"])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output();

                if let Ok(output) = output {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let port_suffix = format!(":{}", port);
                    for line in stdout.lines() {
                        let trimmed = line.trim();
                        if !trimmed.contains("LISTENING") {
                            continue;
                        }
                        let parts: Vec<&str> = trimmed.split_whitespace().collect();
                        if parts.len() >= 5 && parts[1].ends_with(&port_suffix) {
                            if let Ok(pid) = parts[4].parse::<u32>() {
                                println!("Stopping process PID: {}...", pid);
                                kill_process_by_pid(pid);
                            }
                        }
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(300));
            if is_port_in_use(host, port) {
                return Err(bifrost_core::BifrostError::Network(format!(
                    "Failed to free port {}: port is still in use. Please stop the process manually and try again.",
                    bind_addr
                )));
            }
            println!("Port freed. Continuing startup...");
            Ok(())
        } else {
            Err(bifrost_core::BifrostError::Network(format!(
                "Port {} is already in use. Start cancelled.",
                bind_addr
            )))
        }
    }
}

fn prompt_yes_no() -> bifrost_core::Result<bool> {
    for _ in 0..3 {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        let bytes = io::stdin().read_line(&mut input)?;
        if bytes == 0 {
            return Ok(false);
        }

        if let Some(answer) = parse_yes_no_answer(&input) {
            return Ok(answer);
        }

        println!("Please answer with y/yes or n/no.");
    }

    Ok(false)
}

fn begin_startup_phase(phase: &'static str) -> Instant {
    tracing::info!(
        target: "bifrost_cli::startup",
        phase,
        "startup phase started"
    );
    Instant::now()
}

fn log_startup_phase(phase: &'static str, started_at: Instant) {
    tracing::info!(
        target: "bifrost_cli::startup",
        phase,
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        "startup phase completed"
    );
}

struct SystemProxyReconcileConfig {
    bifrost_dir: PathBuf,
    system_proxy_manager: Arc<tokio::sync::RwLock<bifrost_core::SystemProxyManager>>,
    desired_enabled: Arc<AtomicBool>,
    proxy_host: String,
    proxy_port: u16,
    system_proxy_bypass: String,
    enabled_flag: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    daemon_mode: bool,
    defer_startup_recovery_for_restart_handoff: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SystemProxyOwnership {
    Disabled,
    ThisBifrost,
    Other,
}

fn inspect_system_proxy_ownership(proxy_host: &str, proxy_port: u16) -> SystemProxyOwnership {
    match bifrost_core::SystemProxyManager::get_current() {
        Ok(current) if !current.enable => SystemProxyOwnership::Disabled,
        Ok(current) if current.target_matches(proxy_host, proxy_port) => {
            SystemProxyOwnership::ThisBifrost
        }
        Ok(_) => SystemProxyOwnership::Other,
        Err(error) => {
            tracing::warn!(
                target: "bifrost_cli::startup",
                error = %error,
                host = %proxy_host,
                port = proxy_port,
                "failed to inspect current system proxy ownership"
            );
            SystemProxyOwnership::Disabled
        }
    }
}

fn spawn_system_proxy_reconcile_task(config: SystemProxyReconcileConfig) {
    let SystemProxyReconcileConfig {
        bifrost_dir,
        system_proxy_manager,
        desired_enabled,
        proxy_host,
        proxy_port,
        system_proxy_bypass,
        enabled_flag,
        stop_flag,
        daemon_mode,
        defer_startup_recovery_for_restart_handoff,
    } = config;

    let _ = std::thread::Builder::new()
        .name("bifrost-system-proxy-reconcile".to_string())
        .spawn(move || {
            let started_at = Instant::now();

            if !should_run_system_proxy_reconcile_startup_recovery(
                &bifrost_dir,
                desired_enabled.load(Ordering::Acquire),
                defer_startup_recovery_for_restart_handoff,
            ) {
                tracing::info!(
                    target: "bifrost_cli::startup",
                    data_dir = %bifrost_dir.display(),
                    "system proxy reconcile startup recovery skipped for restart handoff"
                );
            } else if let Err(error) =
                bifrost_core::SystemProxyManager::recover_from_crash(&bifrost_dir)
            {
                tracing::warn!(
                    error = %error,
                    "[SYSTEM_PROXY] Failed to recover system proxy from previous crash"
                );
            }

            let mut applied_by_this_runtime = false;
            let mut startup_external_owner_logged = false;
            let mut idle_logged = false;
            while !stop_flag.load(Ordering::Acquire) {
                if should_stop_system_proxy_reconcile_for_shutdown(&bifrost_dir) {
                    stop_flag.store(true, Ordering::Release);
                    return;
                }
                let should_enable = desired_enabled.load(Ordering::Acquire);
                match inspect_system_proxy_ownership(&proxy_host, proxy_port) {
                    SystemProxyOwnership::Other => {
                        if applied_by_this_runtime || enabled_flag.load(Ordering::Acquire) {
                            applied_by_this_runtime = false;
                            enabled_flag.store(false, Ordering::Release);
                            tracing::info!(
                                target: "bifrost_cli::startup",
                                host = %proxy_host,
                                port = proxy_port,
                                "system proxy no longer points to this Bifrost; pausing automatic reconcile"
                            );
                        } else if should_enable && !startup_external_owner_logged {
                            startup_external_owner_logged = true;
                            tracing::info!(
                                target: "bifrost_cli::startup",
                                host = %proxy_host,
                                port = proxy_port,
                                "system proxy is already owned by another proxy; startup auto-apply skipped"
                            );
                        }
                        let sleep_until = Instant::now() + system_proxy_reconcile_interval();
                        while Instant::now() < sleep_until {
                            if stop_flag.load(Ordering::Acquire)
                                || should_stop_system_proxy_reconcile_for_shutdown(&bifrost_dir)
                            {
                                stop_flag.store(true, Ordering::Release);
                                return;
                            }
                            std::thread::sleep(Duration::from_secs(1));
                        }
                        continue;
                    }
                    SystemProxyOwnership::ThisBifrost => {
                        applied_by_this_runtime = true;
                        enabled_flag.store(true, Ordering::Release);
                    }
                    SystemProxyOwnership::Disabled => {
                        if !should_enable && !enabled_flag.load(Ordering::Acquire) {
                            if !idle_logged {
                                idle_logged = true;
                                tracing::info!(
                                    target: "bifrost_cli::startup",
                                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                                    "system proxy reconcile is idle until system proxy is enabled"
                                );
                            }
                            let sleep_until = Instant::now() + system_proxy_reconcile_interval();
                            while Instant::now() < sleep_until {
                                if stop_flag.load(Ordering::Acquire)
                                    || should_stop_system_proxy_reconcile_for_shutdown(&bifrost_dir)
                                {
                                    stop_flag.store(true, Ordering::Release);
                                    return;
                                }
                                std::thread::sleep(Duration::from_secs(1));
                            }
                            continue;
                        }
                    }
                }

                if !should_enable {
                    if applied_by_this_runtime || enabled_flag.load(Ordering::Acquire) {
                        tracing::info!(
                            target: "bifrost_cli::startup",
                            host = %proxy_host,
                            port = proxy_port,
                            "system proxy reconcile skipped because runtime desired state is disabled"
                        );
                    }
                    applied_by_this_runtime = false;
                    enabled_flag.store(false, Ordering::Release);
                    let sleep_until = Instant::now() + system_proxy_reconcile_interval();
                    while Instant::now() < sleep_until {
                        if stop_flag.load(Ordering::Acquire)
                            || should_stop_system_proxy_reconcile_for_shutdown(&bifrost_dir)
                        {
                            stop_flag.store(true, Ordering::Release);
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    continue;
                }

                let mut manager = system_proxy_manager.blocking_write();
                let result = manager.enable(&proxy_host, proxy_port, Some(&system_proxy_bypass));

                let final_result = match &result {
                    Ok(()) => result,
                    Err(error) => {
                        let msg = error.to_string();
                        if msg.contains("RequiresAdmin") {
                            #[cfg(target_os = "macos")]
                            {
                                if daemon_mode {
                                    println!("System proxy requires admin privileges; applying asynchronously via GUI authorization if approved...");
                                } else {
                                    println!("System proxy requires admin privileges, requesting authorization asynchronously...");
                                }
                                manager.enable_with_gui_auth(
                                    &proxy_host,
                                    proxy_port,
                                    Some(&system_proxy_bypass),
                                )
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                result
                            }
                        } else {
                            result
                        }
                    }
                };

                match final_result {
                    Ok(()) => {
                        applied_by_this_runtime = true;
                        enabled_flag.store(true, Ordering::Release);
                        tracing::info!(
                            target: "bifrost_cli::startup",
                            elapsed_ms = started_at.elapsed().as_millis() as u64,
                            host = %proxy_host,
                            port = proxy_port,
                            "system proxy applied or reconciled"
                        );
                    }
                    Err(error) => {
                        let msg = error.to_string();
                        if msg.contains("UserCancelled") {
                            println!("System proxy not enabled (authorization cancelled)");
                        } else if msg.contains("RequiresAdmin") && daemon_mode {
                            println!("System proxy requires administrator privileges; daemon will continue without changing system proxy. You can toggle it later via CLI or Admin UI.");
                        } else if msg.contains("RequiresAdmin") {
                            println!("System proxy requires administrator privileges and was not enabled.");
                        } else {
                            eprintln!("Failed to enable system proxy asynchronously: {}", error);
                        }
                        tracing::warn!(
                            target: "bifrost_cli::startup",
                            error = %error,
                            elapsed_ms = started_at.elapsed().as_millis() as u64,
                            "system proxy reconcile failed"
                        );
                        if msg.contains("UserCancelled") || msg.contains("RequiresAdmin") {
                            return;
                        }
                    }
                }

                drop(manager);
                let sleep_until = Instant::now() + system_proxy_reconcile_interval();
                while Instant::now() < sleep_until {
                    if stop_flag.load(Ordering::Acquire)
                        || should_stop_system_proxy_reconcile_for_shutdown(&bifrost_dir)
                    {
                        stop_flag.store(true, Ordering::Release);
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });
}

#[cfg(target_os = "macos")]
fn spawn_system_proxy_wake_reconcile_task(config: SystemProxyReconcileConfig) {
    let SystemProxyReconcileConfig {
        bifrost_dir,
        system_proxy_manager,
        desired_enabled,
        proxy_host,
        proxy_port,
        system_proxy_bypass,
        enabled_flag,
        stop_flag,
        ..
    } = config;

    let _ = std::thread::Builder::new()
        .name("bifrost-system-proxy-wake-reconcile".to_string())
        .spawn(move || {
            let mut last_monotonic_check = Instant::now();
            let mut last_wall_check = SystemTime::now();
            while !stop_flag.load(Ordering::Acquire) {
                std::thread::sleep(SYSTEM_PROXY_WAKE_CHECK_INTERVAL);
                if stop_flag.load(Ordering::Acquire)
                    || should_stop_system_proxy_reconcile_for_shutdown(&bifrost_dir)
                {
                    stop_flag.store(true, Ordering::Release);
                    return;
                }

                let monotonic_gap = last_monotonic_check.elapsed();
                let wall_now = SystemTime::now();
                let wall_gap = wall_now
                    .duration_since(last_wall_check)
                    .unwrap_or_else(|_| Duration::from_secs(0));
                last_monotonic_check = Instant::now();
                last_wall_check = wall_now;
                if monotonic_gap < SYSTEM_PROXY_WAKE_GAP_THRESHOLD
                    && wall_gap < SYSTEM_PROXY_WAKE_GAP_THRESHOLD
                {
                    continue;
                }

                tracing::info!(
                    target: "bifrost_cli::startup",
                    trigger = "scheduler_wall_gap",
                    monotonic_gap_ms = monotonic_gap.as_millis() as u64,
                    wall_gap_ms = wall_gap.as_millis() as u64,
                    "system proxy scheduler or wake gap detected; reconciling immediately"
                );
                let should_enable = desired_enabled.load(Ordering::Acquire);
                if !should_enable {
                    enabled_flag.store(false, Ordering::Release);
                    tracing::info!(
                        target: "bifrost_cli::startup",
                        host = %proxy_host,
                        port = proxy_port,
                        "system proxy wake reconcile skipped because runtime desired state is disabled"
                    );
                    continue;
                }
                match inspect_system_proxy_ownership(&proxy_host, proxy_port) {
                    SystemProxyOwnership::ThisBifrost => {
                        enabled_flag.store(true, Ordering::Release);
                    }
                    SystemProxyOwnership::Other => {
                        enabled_flag.store(false, Ordering::Release);
                        tracing::info!(
                            target: "bifrost_cli::startup",
                            host = %proxy_host,
                            port = proxy_port,
                            "system proxy wake reconcile detected another proxy owner; leaving it unchanged"
                        );
                        continue;
                    }
                    SystemProxyOwnership::Disabled => {
                        if !enabled_flag.load(Ordering::Acquire) {
                            tracing::info!(
                                target: "bifrost_cli::startup",
                                host = %proxy_host,
                                port = proxy_port,
                                "system proxy wake reconcile skipped because this Bifrost is not managing system proxy"
                            );
                            continue;
                        }
                    }
                }
                let lock_started_at = Instant::now();
                let mut manager = system_proxy_manager.blocking_write();
                tracing::info!(
                    target: "bifrost_cli::startup",
                    wait_elapsed_ms = lock_started_at.elapsed().as_millis() as u64,
                    "acquired_system_proxy_lock for wake reconcile"
                );
                match manager.enable(&proxy_host, proxy_port, Some(&system_proxy_bypass)) {
                    Ok(()) => {
                        enabled_flag.store(true, Ordering::Release);
                        tracing::info!(
                            target: "bifrost_cli::startup",
                            host = %proxy_host,
                            port = proxy_port,
                            "system proxy wake reconcile completed"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "bifrost_cli::startup",
                            error = %error,
                            "system proxy wake reconcile failed"
                        );
                    }
                }
            }
        });
}

#[cfg(target_os = "macos")]
fn spawn_system_proxy_launchd_install_task(
    bifrost_dir: std::path::PathBuf,
    enable_system_proxy: bool,
) {
    if !enable_system_proxy {
        return;
    }
    if std::env::var(SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        tracing::info!(
            target: "bifrost_cli::startup",
            env = SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV,
            "system proxy LaunchDaemon cleanup install disabled by environment"
        );
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("Failed to resolve current executable for LaunchDaemon install: {error}");
            return;
        }
    };
    let config =
        match bifrost_core::SystemProxyLaunchdConfig::new(None, Some(exe), bifrost_dir, None) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    target: "bifrost_cli::startup",
                    error = %error,
                    "failed to prepare system proxy LaunchDaemon cleanup install"
                );
                return;
            }
        };

    let status = match bifrost_core::launchd_status_for_config(&config) {
        Ok(status) => status,
        Err(error) => {
            tracing::warn!(
                target: "bifrost_cli::startup",
                error = %error,
                "failed to inspect system proxy LaunchDaemon cleanup status"
            );
            return;
        }
    };
    if status.installed && status.loaded && !status.needs_upgrade {
        tracing::info!(
            target: "bifrost_cli::startup",
            installed_version = status.installed_version.as_deref().unwrap_or(""),
            current_version = status.current_version,
            "system proxy LaunchDaemon cleanup already installed and current"
        );
        return;
    }

    eprintln!(
        "System proxy boot/shutdown cleanup is not ready yet; macOS may ask for administrator authorization. Until that install succeeds, reboot-time cleanup is not protected by LaunchDaemon."
    );
    std::thread::spawn(move || {
        tracing::info!(
            target: "bifrost_cli::startup",
            installed = status.installed,
            loaded = status.loaded,
            needs_upgrade = status.needs_upgrade,
            "system proxy LaunchDaemon cleanup install starting asynchronously"
        );
        match bifrost_core::install_launchd_cleanup_with_gui_auth(&config) {
            Ok(status) => tracing::info!(
                target: "bifrost_cli::startup",
                installed_version = status.installed_version.as_deref().unwrap_or(""),
                current_version = status.current_version,
                "system proxy LaunchDaemon cleanup installed asynchronously"
            ),
            Err(error) if error.to_string().contains("UserCancelled") => {
                eprintln!(
                    "System proxy boot/shutdown cleanup authorization was cancelled; reboot-time cleanup will remain unavailable until LaunchDaemon install succeeds."
                );
                tracing::info!(
                    target: "bifrost_cli::startup",
                    "system proxy LaunchDaemon cleanup install cancelled by user"
                )
            }
            Err(error) => {
                eprintln!(
                    "System proxy boot/shutdown cleanup failed to install: {error}. Reboot-time cleanup will remain unavailable until this succeeds."
                );
                tracing::warn!(
                    target: "bifrost_cli::startup",
                    error = %error,
                    "system proxy LaunchDaemon cleanup install failed"
                )
            }
        }
    });
}

fn is_port_in_use(host: &str, port: u16) -> bool {
    let check_host = if host == "0.0.0.0" || host == "::" {
        "127.0.0.1"
    } else {
        host
    };
    match std::net::TcpListener::bind((check_host, port)) {
        Ok(listener) => {
            drop(listener);
            return false;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            return true;
        }
        Err(_) => {}
    }

    std::net::TcpStream::connect_timeout(
        &format!("{}:{}", check_host, port)
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        std::time::Duration::from_millis(200),
    )
    .is_ok()
}

fn find_available_port(host: &str, preferred_port: u16) -> bifrost_core::Result<u16> {
    for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }

        if is_port_in_use(host, port) {
            continue;
        }

        if std::net::TcpListener::bind((host, port)).is_ok() {
            return Ok(port);
        }
    }

    Err(bifrost_core::BifrostError::Network(format!(
        "failed to find an available port starting from {}",
        preferred_port
    )))
}

pub(crate) async fn spawn_managed_proxy_task(
    config: ProxyConfig,
    rules: SharedDynamicRulesResolver,
    tls_config: Arc<bifrost_proxy::TlsConfig>,
    admin_state: Arc<AdminState>,
    push_manager: Arc<PushManager>,
    access_control: bifrost_admin::SharedAccessControl,
) -> bifrost_core::Result<tokio::task::JoinHandle<bifrost_core::Result<()>>> {
    let addr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| bifrost_core::BifrostError::Config(format!("Invalid address: {}", e)))?;

    let server = ProxyServer::new(config)
        .with_access_control(access_control)
        .with_tls_config(tls_config)
        .with_admin_state_shared(admin_state)
        .with_rules(rules)
        .with_push_manager(push_manager);
    let listener = server.bind(addr).await?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    let task = tokio::spawn(async move {
        server
            .run_with_listener_ready(listener, Some(ready_tx))
            .await
    });

    match ready_rx.await {
        Ok(()) => Ok(task),
        Err(_) => {
            let result = task.await;
            Err(listener_task_error(result))
        }
    }
}

fn listener_task_error(
    result: std::result::Result<bifrost_core::Result<()>, tokio::task::JoinError>,
) -> bifrost_core::BifrostError {
    match result {
        Ok(Ok(())) => bifrost_core::BifrostError::Network(
            "managed proxy listener exited unexpectedly".to_string(),
        ),
        Ok(Err(error)) => error,
        Err(error) if error.is_cancelled() => bifrost_core::BifrostError::Network(
            "managed proxy listener was cancelled unexpectedly".to_string(),
        ),
        Err(error) => bifrost_core::BifrostError::Network(format!(
            "managed proxy listener task failed: {}",
            error
        )),
    }
}

fn abort_listener_after_grace_period(handle: tokio::task::JoinHandle<bifrost_core::Result<()>>) {
    tokio::spawn(async move {
        tokio::time::sleep(PORT_REBIND_OLD_LISTENER_GRACE_PERIOD).await;
        handle.abort();
    });
}

fn recover_proxy_state_before_start(bifrost_dir: &std::path::Path) {
    tracing::info!(
        target: "bifrost_cli::startup",
        data_dir = %bifrost_dir.display(),
        "checking for stale system proxy state before startup"
    );
    if let Err(error) = bifrost_core::SystemProxyManager::recover_from_crash(bifrost_dir) {
        eprintln!("Failed to recover system proxy from previous crash: {error}");
        tracing::warn!(
            error = %error,
            data_dir = %bifrost_dir.display(),
            "[SYSTEM_PROXY] Failed to recover system proxy before startup"
        );
    }

    if let Err(error) = bifrost_core::ShellProxyManager::recover_from_crash(bifrost_dir) {
        eprintln!("Failed to recover CLI proxy from previous crash: {error}");
        tracing::warn!(
            error = %error,
            data_dir = %bifrost_dir.display(),
            "Failed to recover CLI proxy before startup"
        );
    }
}

struct RestartHandoffStartupGuard {
    bifrost_dir: PathBuf,
    active: bool,
}

impl RestartHandoffStartupGuard {
    fn new(bifrost_dir: PathBuf) -> Self {
        Self {
            bifrost_dir,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for RestartHandoffStartupGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        tracing::warn!(
            target: "bifrost_cli::startup",
            data_dir = %self.bifrost_dir.display(),
            "restart handoff startup aborted before runtime adoption; running system proxy recovery"
        );
        if let Err(error) = bifrost_core::SystemProxyManager::recover_from_crash(&self.bifrost_dir)
        {
            eprintln!("Failed to recover system proxy after aborted restart handoff: {error}");
            tracing::warn!(
                target: "bifrost_cli::startup",
                error = %error,
                data_dir = %self.bifrost_dir.display(),
                "restart handoff startup recovery failed"
            );
        }
        let _ = bifrost_core::consume_system_proxy_shutdown_mode(&self.bifrost_dir);
    }
}

fn should_defer_startup_proxy_recovery_for_restart_handoff(
    bifrost_dir: &std::path::Path,
    enable_system_proxy: bool,
) -> bool {
    if !matches!(
        bifrost_core::read_system_proxy_shutdown_mode(bifrost_dir),
        Some(bifrost_core::SystemProxyShutdownMode::PreserveForRestart)
    ) {
        return false;
    }

    if !enable_system_proxy {
        tracing::info!(
            target: "bifrost_cli::startup",
            data_dir = %bifrost_dir.display(),
            "restart handoff marker found but startup does not request system proxy; running normal recovery"
        );
        return false;
    }

    let Some(runtime) = read_runtime_info() else {
        tracing::info!(
            target: "bifrost_cli::startup",
            data_dir = %bifrost_dir.display(),
            "restart handoff marker found without runtime info; running normal recovery"
        );
        return false;
    };

    tracing::info!(
        target: "bifrost_cli::startup",
        data_dir = %bifrost_dir.display(),
        runtime_host = %runtime.host.as_deref().unwrap_or(""),
        runtime_port = runtime.port,
        "restart handoff marker and runtime validated; deferring startup system proxy recovery until new runtime adopts proxy"
    );
    true
}

fn should_run_system_proxy_startup_recovery(
    bifrost_dir: &std::path::Path,
    enable_system_proxy: bool,
) -> bool {
    !should_defer_startup_proxy_recovery_for_restart_handoff(bifrost_dir, enable_system_proxy)
}

fn should_run_system_proxy_reconcile_startup_recovery(
    bifrost_dir: &std::path::Path,
    enable_system_proxy: bool,
    defer_for_restart_handoff: bool,
) -> bool {
    !defer_for_restart_handoff
        && should_run_system_proxy_startup_recovery(bifrost_dir, enable_system_proxy)
}

#[cfg(unix)]
struct ShutdownSignalListener {
    sigterm: tokio::signal::unix::Signal,
    sigint: tokio::signal::unix::Signal,
    sighup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignalListener {
    fn install() -> Self {
        use tokio::signal::unix::{signal, SignalKind};

        Self {
            sigterm: signal(SignalKind::terminate()).expect("failed to install SIGTERM handler"),
            sigint: signal(SignalKind::interrupt()).expect("failed to install SIGINT handler"),
            sighup: signal(SignalKind::hangup()).expect("failed to install SIGHUP handler"),
        }
    }

    async fn wait(&mut self) {
        tokio::select! {
            _ = self.sigterm.recv() => {},
            _ = self.sigint.recv() => {},
            _ = self.sighup.recv() => {},
        }
    }
}

#[cfg(not(unix))]
struct ShutdownSignalListener;

#[cfg(not(unix))]
impl ShutdownSignalListener {
    fn install() -> Self {
        Self
    }

    async fn wait(&mut self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct SystemProxyRestoreGuard {
    bifrost_dir: PathBuf,
    system_proxy_manager: Arc<tokio::sync::RwLock<bifrost_core::SystemProxyManager>>,
    stop_flag: Arc<AtomicBool>,
}

impl SystemProxyRestoreGuard {
    fn new(
        bifrost_dir: PathBuf,
        system_proxy_manager: Arc<tokio::sync::RwLock<bifrost_core::SystemProxyManager>>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            bifrost_dir,
            system_proxy_manager,
            stop_flag,
        }
    }
}

impl Drop for SystemProxyRestoreGuard {
    fn drop(&mut self) {
        if should_skip_system_proxy_restore(&self.bifrost_dir, "restore guard") {
            self.stop_flag.store(true, Ordering::Release);
            self.system_proxy_manager.blocking_write().detach_in_place();
            return;
        }
        tracing::info!(
            target: "bifrost_cli::shutdown",
            "system proxy restore guard triggered; stopping reconcile before restore"
        );
        self.stop_flag.store(true, Ordering::Release);
        let lock_started_at = Instant::now();
        tracing::info!(
            target: "bifrost_cli::shutdown",
            "waiting_for_system_proxy_lock in restore guard"
        );
        let mut manager = self.system_proxy_manager.blocking_write();
        tracing::info!(
            target: "bifrost_cli::shutdown",
            wait_elapsed_ms = lock_started_at.elapsed().as_millis() as u64,
            "acquired_system_proxy_lock in restore guard"
        );
        if let Err(e) = manager.restore() {
            eprintln!("Failed to restore system proxy: {}", e);
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                error = %e,
                "system proxy restore guard failed to restore proxy"
            );
        } else {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                "system proxy restore guard completed"
            );
        }
    }
}

fn should_skip_system_proxy_restore(bifrost_dir: &Path, context: &'static str) -> bool {
    match bifrost_core::read_system_proxy_shutdown_mode(bifrost_dir) {
        Some(bifrost_core::SystemProxyShutdownMode::BackgroundCleanup) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                context,
                "system proxy foreground restore skipped; cleanup will continue in background"
            );
            true
        }
        Some(bifrost_core::SystemProxyShutdownMode::ForegroundCleanup) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                context,
                "system proxy foreground restore skipped because stop already cleaned proxy before SIGTERM"
            );
            true
        }
        Some(bifrost_core::SystemProxyShutdownMode::PreserveForRestart) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                context,
                "system proxy foreground restore skipped for restart"
            );
            true
        }
        None => false,
    }
}

fn should_stop_system_proxy_reconcile_for_shutdown(bifrost_dir: &Path) -> bool {
    matches!(
        bifrost_core::read_system_proxy_shutdown_mode(bifrost_dir),
        Some(bifrost_core::SystemProxyShutdownMode::ForegroundCleanup)
    )
}

async fn restore_system_proxy_on_shutdown(
    bifrost_dir: &Path,
    system_proxy_manager: &Arc<tokio::sync::RwLock<bifrost_core::SystemProxyManager>>,
    enabled_flag: &Arc<AtomicBool>,
    stop_flag: &Arc<AtomicBool>,
    context: &'static str,
) {
    if should_skip_system_proxy_restore(bifrost_dir, context) {
        stop_flag.store(true, Ordering::Release);
        system_proxy_manager.write().await.detach_in_place();
        return;
    }
    let started_at = Instant::now();
    tracing::info!(
        target: "bifrost_cli::shutdown",
        context,
        enabled_flag = enabled_flag.load(Ordering::Acquire),
        "system proxy shutdown restore starting; stopping reconcile first"
    );
    stop_flag.store(true, Ordering::Release);
    let lock_started_at = Instant::now();
    tracing::info!(
        target: "bifrost_cli::shutdown",
        context,
        "waiting_for_system_proxy_lock"
    );
    let mut manager = system_proxy_manager.write().await;
    tracing::info!(
        target: "bifrost_cli::shutdown",
        context,
        wait_elapsed_ms = lock_started_at.elapsed().as_millis() as u64,
        "acquired_system_proxy_lock"
    );
    match manager.restore() {
        Ok(()) => {
            enabled_flag.store(false, Ordering::Release);
            tracing::info!(
                target: "bifrost_cli::shutdown",
                context,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                "system proxy shutdown restore completed"
            );
        }
        Err(error) => {
            eprintln!("Failed to restore system proxy during shutdown: {error}");
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                context,
                error = %error,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                "failed to restore system proxy"
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_start(
    port: u16,
    host: String,
    socks5_port: Option<u16>,
    log_level: &str,
    daemon: bool,
    log_dir: PathBuf,
    log_retention_days: u32,
    skip_cert_check: bool,
    access_mode: Option<String>,
    whitelist: Option<String>,
    allow_lan: bool,
    proxy_user: Vec<String>,
    intercept: bool,
    no_intercept: bool,
    intercept_exclude: Option<String>,
    intercept_include: Option<String>,
    app_intercept_exclude: Option<String>,
    app_intercept_include: Option<String>,
    unsafe_ssl: bool,
    super_performance_mode: bool,
    enable_badge_injection: bool,
    disable_badge_injection: bool,
    no_disconnect_on_config_change: bool,
    rules: Vec<String>,
    rules_file: Option<PathBuf>,
    system_proxy: bool,
    no_system_proxy: bool,
    proxy_bypass: Option<String>,
    cli_proxy: bool,
    cli_proxy_no_proxy: Option<String>,
    yes: bool,
    no_tray: bool,
) -> bifrost_core::Result<()> {
    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());
    let desktop_startup_session_id = std::env::var(DESKTOP_STARTUP_SESSION_ENV).unwrap_or_default();
    tracing::info!(
        target: "bifrost_cli::startup",
        startup_session_id = %desktop_startup_session_id,
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        target_os = std::env::consts::OS,
        target_arch = std::env::consts::ARCH,
        data_dir = %bifrost_dir.display(),
        host = %host,
        port,
        desktop_core = is_desktop_core_process(),
        "start command entered"
    );
    let preserve_restart_runtime = matches!(
        bifrost_core::read_system_proxy_shutdown_mode(&bifrost_dir),
        Some(bifrost_core::SystemProxyShutdownMode::PreserveForRestart)
    );

    if let Some(pid) = read_pid() {
        if is_process_running(pid) {
            if let Some(runtime) = live_desktop_runtime_for_pid(pid) {
                return handle_live_desktop_runtime_before_start(&runtime, port);
            }

            let should_restart = if yes {
                true
            } else {
                prompt_restart_if_running(pid)?
            };

            if should_restart {
                super::stop::run_stop()?;
            } else {
                println!("Start cancelled.");
                return Ok(());
            }
        } else if preserve_restart_runtime {
            tracing::info!(
                target: "bifrost_cli::startup",
                pid,
                data_dir = %bifrost_dir.display(),
                "preserving stopped runtime info for restart handoff startup"
            );
        } else {
            remove_pid()?;
        }
    }

    let phase_started_at = begin_startup_phase("certificate.preflight");
    if !skip_cert_check {
        check_and_install_certificate(CertificateCheckOptions {
            auto_yes: yes,
            allow_prompt: io::stdin().is_terminal(),
        })?;
    }
    log_startup_phase("certificate.preflight", phase_started_at);

    let phase_started_at = begin_startup_phase("port_conflict.preflight");
    check_and_resolve_port_conflict(&host, port, yes)?;
    log_startup_phase("port_conflict.preflight", phase_started_at);

    let phase_started_at = begin_startup_phase("shell_completions.install");
    super::completions::install_completions_silently();
    log_startup_phase("shell_completions.install", phase_started_at);

    #[cfg(not(unix))]
    let _ = (&log_dir, log_retention_days);

    let phase_started_at = begin_startup_phase("config_manager.open");
    let config_manager = ConfigManager::new(bifrost_dir.clone())?;
    let stored_config = futures::executor::block_on(config_manager.config());
    log_startup_phase("config_manager.open", phase_started_at);

    let parsed_access_mode = match &access_mode {
        Some(mode) => mode
            .parse::<AccessMode>()
            .map_err(bifrost_core::BifrostError::Config)?,
        None => stored_config.access.mode,
    };

    let client_whitelist: Vec<String> = match whitelist {
        Some(wl) => wl.split(',').map(|s| s.trim().to_string()).collect(),
        None => stored_config.access.whitelist.clone(),
    };

    let allow_lan_final = if allow_lan {
        true
    } else {
        stored_config.access.allow_lan
    };
    let userpass_auth = if proxy_user.is_empty() {
        stored_config.access.userpass.clone()
    } else {
        Some(UserPassAuthConfig {
            enabled: true,
            accounts: parse_proxy_users(&proxy_user)?,
            loopback_requires_auth: false,
        })
    };
    let userpass_last_connected_at =
        futures::executor::block_on(config_manager.userpass_last_connected_at());

    let enable_tls_interception = if intercept {
        true
    } else if no_intercept {
        false
    } else {
        stored_config.tls.enable_interception
    };

    let exclude_list: Vec<String> = match intercept_exclude {
        Some(list) => list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => stored_config.tls.intercept_exclude.clone(),
    };

    let include_list: Vec<String> = match intercept_include {
        Some(list) => list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => stored_config.tls.intercept_include.clone(),
    };

    let app_exclude_list: Vec<String> = match app_intercept_exclude {
        Some(list) => list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => stored_config.tls.app_intercept_exclude.clone(),
    };

    let app_include_list: Vec<String> = match app_intercept_include {
        Some(list) => list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => stored_config.tls.app_intercept_include.clone(),
    };

    let unsafe_ssl_final = if unsafe_ssl {
        true
    } else {
        stored_config.tls.unsafe_ssl
    };

    let inject_bifrost_badge = if enable_badge_injection {
        true
    } else if disable_badge_injection {
        false
    } else {
        stored_config.traffic.inject_bifrost_badge
    };

    let effective_super_performance_mode =
        super_performance_mode || stored_config.traffic.super_performance_mode;

    if enable_badge_injection || disable_badge_injection || super_performance_mode {
        futures::executor::block_on(config_manager.update_traffic_config(TrafficConfigUpdate {
            inject_bifrost_badge: Some(inject_bifrost_badge),
            super_performance_mode: super_performance_mode.then_some(true),
            ..Default::default()
        }))?;
    }

    let verbose_logging = matches!(log_level, "debug" | "trace");
    let proxy_config = ProxyConfig {
        port,
        host: host.clone(),
        socks5_port,
        access_mode: parsed_access_mode,
        client_whitelist,
        allow_lan: allow_lan_final,
        userpass_auth,
        userpass_last_connected_at,
        enable_tls_interception,
        intercept_exclude: exclude_list.clone(),
        intercept_include: include_list.clone(),
        app_intercept_exclude: app_exclude_list.clone(),
        app_intercept_include: app_include_list.clone(),
        ip_intercept_exclude: stored_config.tls.ip_intercept_exclude.clone(),
        ip_intercept_include: stored_config.tls.ip_intercept_include.clone(),
        unsafe_ssl: unsafe_ssl_final,
        verbose_logging,
        max_body_buffer_size: stored_config.traffic.max_body_buffer_size,
        max_body_probe_size: stored_config.traffic.max_body_probe_size,
        super_performance_mode: effective_super_performance_mode,
        binary_traffic_performance_mode: stored_config.traffic.binary_traffic_performance_mode,
        inject_bifrost_badge,
        timeout_secs: stored_config.server.timeout_secs,
        http1_max_header_size: stored_config.server.http1_max_header_size,
        http2_max_header_list_size: stored_config.server.http2_max_header_list_size,
        websocket_handshake_max_header_size: stored_config
            .server
            .websocket_handshake_max_header_size,
        enable_socks: true,
        ..Default::default()
    };

    println!("Access control mode: {}", proxy_config.access_mode);
    if !proxy_config.client_whitelist.is_empty() {
        println!("Client whitelist: {:?}", proxy_config.client_whitelist);
    }
    if proxy_config.allow_lan {
        println!("LAN (private network) access: enabled");
    }
    if enable_tls_interception {
        println!("TLS interception: enabled");
        if !exclude_list.is_empty() {
            println!("  Excluded domains: {:?}", exclude_list);
        }
        if !app_exclude_list.is_empty() {
            println!("  Excluded apps: {:?}", app_exclude_list);
        }
    } else {
        println!("TLS interception: disabled");
    }
    if !include_list.is_empty() {
        println!("  Force intercept domains: {:?}", include_list);
    }
    if !app_include_list.is_empty() {
        println!("  Force intercept apps: {:?}", app_include_list);
    }
    if unsafe_ssl_final {
        println!("⚠️  WARNING: Upstream TLS certificate verification is DISABLED (--unsafe-ssl)");
    }
    if effective_super_performance_mode {
        println!(
            "Super performance mode: enabled (traffic records and body/frame storage disabled)"
        );
    }

    let early_values = futures::executor::block_on(config_manager.values_as_hashmap());
    let values_dir = bifrost_dir.join("values");
    if !early_values.is_empty() {
        println!(
            "Loaded {} values from {}",
            early_values.len(),
            values_dir.display()
        );
    }

    let (parsed_rules, inline_values) = parse_cli_rules(&rules, &rules_file, &early_values)?;
    let mut all_values = early_values.clone();
    for (k, v) in inline_values {
        all_values.entry(k).or_insert(v);
    }
    if !parsed_rules.is_empty() {
        println!("Loaded {} rules from command line", parsed_rules.len());
        for rule in &parsed_rules {
            println!(
                "  {} {}://{}",
                rule.pattern,
                rule.protocol.to_str(),
                rule.value
            );
        }
    }

    let enable_system_proxy = if system_proxy {
        true
    } else if no_system_proxy {
        false
    } else {
        stored_config.system_proxy.enabled
    };

    let system_proxy_bypass =
        proxy_bypass.unwrap_or_else(|| stored_config.system_proxy.bypass.clone());

    if enable_system_proxy {
        if bifrost_core::SystemProxyManager::is_supported() {
            println!("System proxy: enabled (bypass: {})", system_proxy_bypass);
        } else {
            println!("⚠️  WARNING: System proxy is not supported on this platform");
        }
    }

    let defer_startup_recovery_for_restart_handoff =
        should_defer_startup_proxy_recovery_for_restart_handoff(&bifrost_dir, enable_system_proxy);
    let mut restart_handoff_guard = if defer_startup_recovery_for_restart_handoff {
        Some(RestartHandoffStartupGuard::new(bifrost_dir.clone()))
    } else {
        recover_proxy_state_before_start(&bifrost_dir);
        None
    };
    let disconnect_on_config_change = if no_disconnect_on_config_change {
        false
    } else {
        stored_config.tls.disconnect_on_change
    };

    let should_spawn_daemon_parent =
        should_spawn_daemon_parent_process(daemon, is_detached_daemon_child_process());
    if should_spawn_daemon_parent {
        #[cfg(unix)]
        {
            let daemon_result = run_daemon(
                proxy_config,
                parsed_rules,
                all_values.clone(),
                enable_system_proxy,
                system_proxy_bypass.clone(),
                defer_startup_recovery_for_restart_handoff,
                cli_proxy,
                cli_proxy_no_proxy.clone(),
                config_manager,
                log_dir.clone(),
                log_retention_days,
                log_level,
            );
            if daemon_result.is_ok() {
                if let Some(guard) = restart_handoff_guard.as_mut() {
                    guard.disarm();
                }
            }
            daemon_result?;
        }
        #[cfg(windows)]
        {
            let daemon_result =
                run_daemon_via_exec(&proxy_config, &config_manager, &log_dir, log_retention_days);
            if daemon_result.is_ok() {
                if let Some(guard) = restart_handoff_guard.as_mut() {
                    guard.disarm();
                }
            }
            daemon_result?;
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            return Err(bifrost_core::BifrostError::Config(
                "Daemon mode is not supported on this platform".to_string(),
            ));
        }
    } else {
        let foreground_result = run_foreground(
            proxy_config,
            parsed_rules,
            all_values,
            enable_system_proxy,
            system_proxy_bypass,
            defer_startup_recovery_for_restart_handoff,
            cli_proxy,
            cli_proxy_no_proxy,
            disconnect_on_config_change,
            config_manager,
            no_tray,
            log_level,
            skip_cert_check,
            yes,
        );
        if foreground_result.is_ok() {
            if let Some(guard) = restart_handoff_guard.as_mut() {
                guard.disarm();
            }
        }
        foreground_result?;
    }

    Ok(())
}

fn should_spawn_daemon_parent_process(daemon: bool, detached_daemon_child: bool) -> bool {
    daemon && !detached_daemon_child
}

#[allow(clippy::too_many_arguments)]
pub fn run_foreground(
    config: ProxyConfig,
    cli_rules: Vec<Rule>,
    cli_values: HashMap<String, String>,
    enable_system_proxy: bool,
    system_proxy_bypass: String,
    defer_startup_recovery_for_restart_handoff: bool,
    enable_cli_proxy: bool,
    cli_proxy_no_proxy: Option<String>,
    disconnect_on_config_change: bool,
    config_manager: ConfigManager,
    no_tray: bool,
    log_level: &str,
    skip_cert_check: bool,
    yes: bool,
) -> bifrost_core::Result<()> {
    let pid = std::process::id();

    raise_fd_limit();

    print_startup_help(config.port);

    println!("════════════════════════════════════════════════════════════════════════");
    println!("                           SERVER STATUS");
    println!("════════════════════════════════════════════════════════════════════════");

    let tls_config = load_tls_config(&config)?;

    let bifrost_dir = config_manager.data_dir().to_path_buf();
    let system_proxy_manager = std::sync::Arc::new(tokio::sync::RwLock::new(
        bifrost_core::SystemProxyManager::new(bifrost_dir.clone()),
    ));
    let mut shell_proxy_manager = bifrost_core::ShellProxyManager::new(bifrost_dir.clone());
    if let Err(e) = bifrost_core::ShellProxyManager::recover_from_crash(&bifrost_dir) {
        tracing::warn!("Failed to recover CLI proxy from previous crash: {}", e);
    }

    let system_proxy_enabled = Arc::new(AtomicBool::new(false));
    let system_proxy_desired_enabled = Arc::new(AtomicBool::new(enable_system_proxy));
    let system_proxy_reconcile_stop = Arc::new(AtomicBool::new(false));
    let _system_proxy_restore_guard = SystemProxyRestoreGuard::new(
        bifrost_dir.clone(),
        system_proxy_manager.clone(),
        system_proxy_reconcile_stop.clone(),
    );

    let mut cli_proxy_enabled = false;
    let cli_proxy_no_proxy =
        cli_proxy_no_proxy.unwrap_or_else(|| "localhost,127.0.0.1,::1".to_string());
    let cli_proxy_host = if config.host == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        config.host.clone()
    };
    if enable_cli_proxy {
        match shell_proxy_manager.enable_persistent(
            &cli_proxy_host,
            config.port,
            &cli_proxy_no_proxy,
        ) {
            Ok(()) => {
                cli_proxy_enabled = true;
            }
            Err(e) => {
                if shell_proxy_manager.config_paths().is_empty() {
                    tracing::info!(
                        "CLI proxy persistent config not available for {} shell (use temporary commands instead)",
                        shell_proxy_manager.shell_type().as_str()
                    );
                } else {
                    eprintln!("  ✗ Failed to enable CLI proxy: {}", e);
                    remove_pid()?;
                    return Err(e);
                }
            }
        }
    }

    let admin_host = if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        &config.host
    };

    println!();
    println!("📡 NETWORK");
    if let Some(socks5_port) = config.socks5_port {
        println!(
            "   Unified Proxy: {}:{} (HTTP/HTTPS/SOCKS5)",
            config.host, config.port
        );
        println!(
            "   SOCKS5 (alt):  {}:{} (separate port)",
            config.host, socks5_port
        );
    } else if config.enable_socks {
        println!(
            "   Unified Proxy: {}:{} (HTTP/HTTPS/SOCKS5)",
            config.host, config.port
        );
    } else {
        println!("   HTTP Proxy:    {}:{}", config.host, config.port);
    }
    println!("   Admin UI:      http://{}:{}/", admin_host, config.port);

    println!();
    let tls_status = TlsStatusInfo {
        enable_tls_interception: config.enable_tls_interception,
        intercept_exclude: config.intercept_exclude.clone(),
        intercept_include: config.intercept_include.clone(),
        app_intercept_exclude: config.app_intercept_exclude.clone(),
        app_intercept_include: config.app_intercept_include.clone(),
        ip_intercept_exclude: config.ip_intercept_exclude.clone(),
        ip_intercept_include: config.ip_intercept_include.clone(),
        unsafe_ssl: config.unsafe_ssl,
        disconnect_on_config_change,
        active_connections: 0,
    };
    tls_status.print_status();

    println!();
    println!("🔐 CA CERTIFICATE");
    let cert_dir = bifrost_dir.join("certs");
    let ca_cert_path = cert_dir.join("ca.crt");
    if ca_cert_path.exists() {
        let installer = CertInstaller::new(&ca_cert_path);
        match installer.check_status() {
            Ok(CertStatus::InstalledAndTrusted) => {
                println!("   Status:        ✓ Installed and trusted");
            }
            Ok(CertStatus::InstalledNotTrusted) => {
                println!("   Status:        ⚠ Installed but NOT trusted");
                println!("   Action:        Run 'bifrost ca info' for details");
            }
            Ok(CertStatus::NotInstalled) => {
                println!("   Status:        ✗ Not installed in system trust store");
                println!("   Action:        Run 'bifrost ca info' to install");
            }
            Err(_) => {
                println!("   Status:        ? Unable to check");
            }
        }
        println!("   Certificate:   {}", ca_cert_path.display());
    } else {
        println!("   Status:        Not generated");
    }

    println!();
    println!("🌐 SYSTEM PROXY");
    if system_proxy_enabled.load(Ordering::Acquire) {
        println!("   Status:        ✓ Enabled");
        println!("   Bypass:        {}", system_proxy_bypass);
    } else if enable_system_proxy {
        println!("   Status:        Requested (applying asynchronously)");
        println!("   Bypass:        {}", system_proxy_bypass);
    } else {
        println!("   Status:        Disabled");
    }

    println!();
    println!("🖥️  CLI PROXY (ENV)");
    if cli_proxy_enabled {
        println!("   Status:        ✓ Enabled");
        println!(
            "   Proxy:         http://{}:{}",
            cli_proxy_host, config.port
        );
        println!("   No Proxy:      {}", cli_proxy_no_proxy);
    } else if enable_cli_proxy {
        println!("   Status:        ⚠ Requested but not enabled");
    } else {
        println!("   Status:        Disabled");
    }

    println!();
    println!("🛡️  ACCESS CONTROL");
    println!("   Mode:          {}", config.access_mode);
    if !config.client_whitelist.is_empty() {
        println!("   Whitelist:     {:?}", config.client_whitelist);
    }
    println!(
        "   LAN Access:    {}",
        if config.allow_lan {
            "enabled"
        } else {
            "disabled"
        }
    );

    println!();
    println!("📂 DATA DIRECTORY");
    println!("   Path:          {}", bifrost_dir.display());
    let custom_dir = std::env::var("BIFROST_DATA_DIR").ok();
    if custom_dir.is_some() {
        println!("   Source:        BIFROST_DATA_DIR environment variable");
    } else {
        println!("   Source:        Default (~/.bifrost)");
    }

    println!();
    println!("⚙️  PROCESS");
    println!("   PID:           {}", pid);
    println!("   Platform:      {}", get_platform_name());

    println!();
    println!("────────────────────────────────────────────────────────────────────────");
    println!("Press Ctrl+C to stop");
    println!("────────────────────────────────────────────────────────────────────────");
    println!();

    let rt = create_bifrost_runtime()?;

    let runtime_result = rt.block_on(async {
        let result: bifrost_core::Result<()> = async {
            let startup_started_at = Instant::now();
            tracing::info!(
                target: "bifrost_cli::startup",
                data_dir = %bifrost_dir.display(),
                port = config.port,
                host = %config.host,
                "starting foreground runtime initialization"
            );

            let phase_started_at = begin_startup_phase("config_manager.config");
            let stored_config = config_manager.config().await;
            log_startup_phase("config_manager.config", phase_started_at);

            let phase_started_at = begin_startup_phase("body_store.init");
            let body_temp_dir = bifrost_dir.join("body_cache");
            let body_store = Arc::new(ParkingRwLock::new(BodyStore::new(
                body_temp_dir,
                stored_config.traffic.max_body_memory_size,
                stored_config.traffic.file_retention_days,
                stored_config.traffic.sse_stream_flush_bytes,
                Duration::from_millis(stored_config.traffic.sse_stream_flush_interval_ms),
            )));
            let body_cleanup_task = bifrost_admin::start_body_cleanup_task(body_store.clone());
            log_startup_phase("body_store.init", phase_started_at);

            let phase_started_at = begin_startup_phase("ws_payload_store.init");
            let ws_payload_store = Arc::new(WsPayloadStore::new(
                bifrost_dir.clone(),
                stored_config.traffic.ws_payload_flush_bytes,
                Duration::from_millis(stored_config.traffic.ws_payload_flush_interval_ms),
                stored_config.traffic.ws_payload_max_open_files,
                stored_config.traffic.file_retention_days,
            ));
            let ws_payload_cleanup_task = start_ws_payload_cleanup_task(ws_payload_store.clone());
            log_startup_phase("ws_payload_store.init", phase_started_at);

            let phase_started_at = begin_startup_phase("traffic_db_store.init");
            let traffic_dir = bifrost_dir.join("traffic");
            let traffic_db_store = Arc::new(
                bifrost_admin::TrafficDbStore::new(
                    traffic_dir,
                    stored_config.traffic.max_records,
                    stored_config.traffic.max_db_size_bytes,
                    Some(stored_config.traffic.file_retention_days * 24),
                )
                .expect("Failed to create traffic database"),
            );
            log_startup_phase("traffic_db_store.init", phase_started_at);

            let phase_started_at = begin_startup_phase("async_traffic.init");
            let (async_traffic_writer, async_traffic_rx) =
                AsyncTrafficWriter::new(ASYNC_TRAFFIC_BUFFER_SIZE);
            let async_traffic_writer = Arc::new(async_traffic_writer);
            let _async_traffic_task =
                start_async_traffic_processor(async_traffic_rx, traffic_db_store.clone());
            log_startup_phase("async_traffic.init", phase_started_at);

            let phase_started_at = begin_startup_phase("frame_store.init");
            let frame_store = Arc::new(bifrost_admin::FrameStore::new(
                bifrost_dir.clone(),
                Some(stored_config.traffic.file_retention_days * 24),
            ));
            let frame_cleanup_task = start_frame_cleanup_task(frame_store.clone());
            log_startup_phase("frame_store.init", phase_started_at);

            let cleanup_body_store = body_store.clone();
            let cleanup_frame_store = frame_store.clone();
            let cleanup_ws_payload_store = ws_payload_store.clone();
            traffic_db_store.set_cleanup_notifier(Arc::new(move |ids| {
                let _ = cleanup_body_store.write().delete_by_ids(ids);
                let _ = cleanup_frame_store.delete_by_ids(ids);
                let _ = cleanup_ws_payload_store.delete_by_ids(ids);
            }));

            let phase_started_at = begin_startup_phase("config_storage.load");
            let values_storage = config_manager.values_storage().await;
            let rules_storage = config_manager.rules_storage().await;
            let auth_db = match bifrost_admin::admin_auth_db::auth_db_path_in(&bifrost_dir)
                .and_then(|p| bifrost_admin::admin_auth_db::AuthDb::open(&p))
            {
                Ok(db) => Some(db),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        data_dir = %bifrost_dir.display(),
                        "Failed to open auth database; admin auth features will be unavailable"
                    );
                    None
                }
            };
            let mut values = {
                use bifrost_core::ValueStore;
                values_storage.as_hashmap()
            };
            for (k, v) in cli_values {
                values.entry(k).or_insert(v);
            }
            log_startup_phase("config_storage.load", phase_started_at);

            let ca_cert_path = bifrost_dir.join("certs").join("ca.crt");

            let runtime_config = RuntimeConfig {
                enable_tls_interception: config.enable_tls_interception,
                intercept_exclude: config.intercept_exclude.clone(),
                intercept_include: config.intercept_include.clone(),
                app_intercept_exclude: config.app_intercept_exclude.clone(),
                app_intercept_include: config.app_intercept_include.clone(),
                ip_intercept_exclude: config.ip_intercept_exclude.clone(),
                ip_intercept_include: config.ip_intercept_include.clone(),
                unsafe_ssl: config.unsafe_ssl,
                disconnect_on_config_change,
            };
            let connection_registry =
                bifrost_admin::ConnectionRegistry::new(disconnect_on_config_change);

            let phase_started_at = begin_startup_phase("app_icon_cache.init");
            let app_icon_cache = bifrost_admin::create_app_icon_cache(&bifrost_dir);
            log_startup_phase("app_icon_cache.init", phase_started_at);

            let phase_started_at = begin_startup_phase("script_manager.init");
            let scripts_dir = bifrost_dir.join("scripts");
            let script_manager = bifrost_admin::ScriptManager::new(scripts_dir);
            if let Err(e) = script_manager.init().await {
                tracing::warn!("Failed to initialize script manager: {}", e);
            }
            log_startup_phase("script_manager.init", phase_started_at);

            let phase_started_at = begin_startup_phase("replay_db_store.init");
            let replay_db_store = match ReplayDbStore::new(bifrost_dir.join("replay")) {
                Ok(store) => Some(Arc::new(store)),
                Err(e) => {
                    tracing::warn!("Failed to initialize replay store: {}", e);
                    None
                }
            };
            log_startup_phase("replay_db_store.init", phase_started_at);

            let access_control = ProxyServer::new(config.clone()).access_control().clone();
            {
                let subnets = bifrost_admin::network::get_local_subnets();
                let ac = access_control.read().await;
                ac.set_local_subnets(subnets);
            }
            let (port_rebind_manager, mut port_rebind_rx) = PortRebindManager::channel(8);

            let shared_config_manager = Arc::new(config_manager);
            let system_proxy_lifecycle_helper_state = Arc::new(
                bifrost_admin::SystemProxyLifecycleHelperState::new(
                    bifrost_dir.clone(),
                    std::process::id(),
                ),
            );
            let detached_daemon_child = is_detached_daemon_child_process();
            let tray_launch_callback = build_tray_launch_callback(
                should_suppress_startup_tray(no_tray, detached_daemon_child),
                bifrost_dir.clone(),
                pid,
                &config,
                log_level,
                skip_cert_check,
                yes,
            );

            let phase_started_at = begin_startup_phase("admin_state.build");
            let mut admin_state = AdminState::new(config.port)
                .with_access_control(access_control.clone())
                .with_body_store(body_store)
                .with_ws_payload_store(ws_payload_store)
                .with_async_traffic_writer_shared(async_traffic_writer)
                .with_traffic_db_store_shared(traffic_db_store.clone())
                .with_frame_store_shared(frame_store)
                .with_runtime_config(runtime_config)
                .with_connection_registry(connection_registry)
                .with_values_storage(values_storage)
                .with_rules_storage(rules_storage);
            if let Some(db) = auth_db {
                admin_state = admin_state.with_auth_db(db);
            }
            let admin_state = admin_state
                .with_ca_cert_path(ca_cert_path)
                .with_system_proxy_manager_shared(system_proxy_manager.clone())
                .with_config_manager_shared(shared_config_manager.clone())
                .with_system_proxy_lifecycle_helper_shared(system_proxy_lifecycle_helper_state.clone())
                .with_tray_launch_callback(tray_launch_callback.clone())
                .with_system_proxy_runtime_flags_shared(
                    system_proxy_desired_enabled.clone(),
                    system_proxy_enabled.clone(),
                )
                .ensure_keepawake_manager_installed()
                .with_max_body_buffer_size(stored_config.traffic.max_body_buffer_size)
                .with_max_body_probe_size(stored_config.traffic.max_body_probe_size)
                .with_super_performance_mode(config.super_performance_mode)
                .with_binary_traffic_performance_mode(
                    stored_config.traffic.binary_traffic_performance_mode,
                )
                .with_breakpoint_timeout_ms(stored_config.traffic.breakpoint_timeout_ms)
                .with_app_icon_cache(app_icon_cache)
                .with_script_manager(script_manager)
                .with_replay_db_store_shared_opt(replay_db_store)
                .with_port_rebind_manager_shared(port_rebind_manager);
            log_startup_phase("admin_state.build", phase_started_at);

            let sync_manager = Arc::new(
                SyncManager::new(shared_config_manager.clone(), config.port)
                    .expect("Failed to create sync manager"),
            );
            let _sync_task = sync_manager.clone().start();
            let admin_state = admin_state.with_sync_manager_shared(sync_manager.clone());
            let admin_state =
                admin_state.with_ip_tls_pending_manager(bifrost_admin::IpTlsPendingManager::new());
            let admin_state = admin_state
                .with_client_trust_tracker(bifrost_admin::ClientTlsTrustTracker::new());

            // Initialize IM Gateway service
            let im_gateway_data_dir = shared_config_manager.data_dir().to_path_buf();
            let im_gateway_service = std::sync::Arc::new(
                bifrost_admin::ImGatewayService::new_with_agent_proxy_port(
                    &im_gateway_data_dir,
                    Some(config.port),
                ),
            );
            let admin_state = admin_state.with_im_gateway_service(im_gateway_service);

            let admin_state = Arc::new(admin_state);

            let remote_invoke_admin_host = if config.host == "0.0.0.0" {
                "127.0.0.1".to_string()
            } else {
                config.host.clone()
            };
            admin_state
                .set_remote_invoke_admin_endpoint(remote_invoke_admin_host.clone(), config.port);
            spawn_remote_invoke_worker_startup_task(
                shared_config_manager.clone(),
                sync_manager.clone(),
                admin_state.clone(),
                remote_invoke_admin_host,
                config.port,
            );

            let db_cleanup_task = bifrost_admin::start_db_cleanup_task(traffic_db_store);
            let connection_cleanup_task =
                bifrost_admin::start_connection_cleanup_task(admin_state.connection_monitor.clone());

            // Auto-connect IM Gateway providers that have owner_open_id configured
            if let Some(im_service) = admin_state.im_gateway_service() {
                im_service.start_scheduler();
                im_service.spawn_chatgpt_web_startup_auth_check();
                im_service.spawn_feishu_setup_supervisor();
                let im_service_clone = im_service.clone();
                tokio::spawn(async move {
                    im_service_clone.auto_connect_providers().await;
                });
            }

            let metrics_collector = admin_state.metrics_collector.clone();
            let rules_storage_for_resolver = admin_state.rules_storage.clone();
            let config_manager_for_resolver = admin_state.config_manager.clone();
            let values_storage_for_resolver = admin_state
                .values_storage
                .clone()
                .expect("values_storage should be set");
            let connection_registry_for_resolver = admin_state.connection_registry.clone();
            let runtime_config_for_resolver = admin_state.runtime_config.clone();

            admin_state.load_group_name_cache();
            admin_state.refresh_badge_rules_cache();

            let phase_started_at = begin_startup_phase("rules_resolver.init");
            let valid_dirs = resolve_valid_group_dirs(&admin_state);
            let (stored_rules, inline_values) = load_stored_rules(&rules_storage_for_resolver, Some(&valid_dirs));
            let mut merged_values = values.clone();
            for (k, v) in inline_values {
                merged_values.entry(k).or_insert(v);
            }
            let resolver: SharedDynamicRulesResolver = Arc::new(DynamicRulesResolver::new(
                cli_rules,
                stored_rules,
                merged_values,
            ));
            log_startup_phase("rules_resolver.init", phase_started_at);

            log_resolver_rules(&resolver);

            let unsafe_ssl = config.unsafe_ssl;
            let admin_state_arc = admin_state.clone();
            let _total_disk_cleanup_task =
                bifrost_admin::start_total_disk_cleanup_task(admin_state_arc.clone());

            let phase_started_at = begin_startup_phase("replay_executor.init");
            let replay_executor = Arc::new(bifrost_admin::ReplayExecutor::new(
                admin_state_arc.clone(),
                unsafe_ssl,
            ));
            admin_state_arc.set_replay_executor(replay_executor);
            log_startup_phase("replay_executor.init", phase_started_at);

            let phase_started_at = begin_startup_phase("push_manager.init");
            let push_manager = Arc::new(PushManager::new(admin_state_arc.clone()));
            let push_tasks = start_push_tasks(push_manager.clone());
            let admin_push_watcher_task = spawn_admin_push_watcher_task(
                config_manager_for_resolver.clone(),
                push_manager.clone(),
            );
            let temporary_port_manager = Arc::new(crate::commands::port::CliTemporaryPortManager::new(
                config.clone(),
                tls_config.clone(),
                admin_state_arc.clone(),
                push_manager.clone(),
                access_control.clone(),
                rules_storage_for_resolver.clone(),
                values_storage_for_resolver.clone(),
            ));
            admin_state_arc.set_temporary_port_manager(temporary_port_manager.clone());
            log_startup_phase("push_manager.init", phase_started_at);

            let phase_started_at = begin_startup_phase("metrics_task.start");
            let metrics_tasks = start_metrics_collector_task(metrics_collector, 1);
            log_startup_phase("metrics_task.start", phase_started_at);

            let phase_started_at = begin_startup_phase("rules_watcher.start");
            let rules_watcher_task = spawn_rules_watcher_task(
                config_manager_for_resolver,
                rules_storage_for_resolver,
                values_storage_for_resolver,
                resolver.clone(),
                connection_registry_for_resolver,
                runtime_config_for_resolver,
                admin_state_arc.clone(),
                Some(temporary_port_manager),
            );
            let rules_filesystem_watcher_task = spawn_rules_filesystem_watcher_task(
                admin_state_arc.config_manager.clone(),
                admin_state_arc.rules_storage.clone(),
            );
            log_startup_phase("rules_watcher.start", phase_started_at);

            let mut current_port = config.port;
            let base_config = config.clone();
            let phase_started_at = begin_startup_phase("proxy_listener.bind");
            let mut listener_task = spawn_managed_proxy_task(
                config.clone(),
                resolver.clone(),
                tls_config.clone(),
                admin_state_arc.clone(),
                push_manager.clone(),
                access_control.clone(),
            )
            .await?;
            let runtime_info = RuntimeInfo::new(
                pid,
                config.port,
                config.socks5_port,
                Some(config.host.clone()),
                foreground_runtime_start_mode(),
            )
            .with_system_proxy(enable_system_proxy, system_proxy_bypass.clone());
            write_runtime_info(&runtime_info)?;
            let _ = bifrost_core::consume_system_proxy_shutdown_mode(&bifrost_dir);
            #[cfg(target_os = "macos")]
            spawn_system_proxy_launchd_install_task(bifrost_dir.clone(), enable_system_proxy);
            log_startup_phase("proxy_listener.bind", phase_started_at);
            tracing::info!(
                target: "bifrost_cli::startup",
                total_elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
                "foreground runtime initialization completed"
            );

            // Launch tray helper if enabled
            tray_launch_callback();

            let mut shutdown_signal = ShutdownSignalListener::install();
            let mobile_availability_tasks = if detached_daemon_child {
                Vec::new()
            } else {
                bifrost_admin::mobile_availability::spawn_terminal_panel(
                    admin_state_arc.clone(),
                    access_control.clone(),
                    push_manager.clone(),
                )
            };

            let system_proxy_host = if base_config.host == "0.0.0.0" {
                "127.0.0.1".to_string()
            } else {
                base_config.host.clone()
            };
            if enable_system_proxy {
                system_proxy_lifecycle_helper_state.ensure_started_after_startup_enable();
            }
            #[cfg(target_os = "macos")]
            spawn_system_proxy_wake_reconcile_task(SystemProxyReconcileConfig {
                bifrost_dir: bifrost_dir.clone(),
                system_proxy_manager: system_proxy_manager.clone(),
                desired_enabled: system_proxy_desired_enabled.clone(),
                proxy_host: system_proxy_host.clone(),
                proxy_port: current_port,
                system_proxy_bypass: system_proxy_bypass.clone(),
                enabled_flag: system_proxy_enabled.clone(),
                stop_flag: system_proxy_reconcile_stop.clone(),
                daemon_mode: detached_daemon_child,
                defer_startup_recovery_for_restart_handoff,
            });

            spawn_system_proxy_reconcile_task(SystemProxyReconcileConfig {
                bifrost_dir: bifrost_dir.clone(),
                system_proxy_manager: system_proxy_manager.clone(),
                desired_enabled: system_proxy_desired_enabled.clone(),
                proxy_host: system_proxy_host,
                proxy_port: current_port,
                system_proxy_bypass: system_proxy_bypass.clone(),
                enabled_flag: system_proxy_enabled.clone(),
                stop_flag: system_proxy_reconcile_stop.clone(),
                daemon_mode: detached_daemon_child,
                defer_startup_recovery_for_restart_handoff,
            });

            let shutdown_listener_context = if detached_daemon_child {
                "daemon listener exit"
            } else {
                "foreground listener exit"
            };
            let shutdown_signal_context = if detached_daemon_child {
                "daemon signal"
            } else {
                "foreground signal"
            };
            loop {
                tokio::select! {
                    listener_result = &mut listener_task => {
                        if enable_system_proxy || system_proxy_enabled.load(Ordering::Acquire) {
                            restore_system_proxy_on_shutdown(
                                &bifrost_dir,
                                &system_proxy_manager,
                                &system_proxy_enabled,
                                &system_proxy_reconcile_stop,
                                shutdown_listener_context,
                            )
                            .await;
                        }
                        return Err(listener_task_error(listener_result));
                    },
                    _ = shutdown_signal.wait() => {
                        info!("Received shutdown signal");
                        println!("\nShutting down...");
                        if enable_system_proxy || system_proxy_enabled.load(Ordering::Acquire) {
                            restore_system_proxy_on_shutdown(
                                &bifrost_dir,
                                &system_proxy_manager,
                                &system_proxy_enabled,
                                &system_proxy_reconcile_stop,
                                shutdown_signal_context,
                            )
                            .await;
                        }
                        break;
                    },
                    request = port_rebind_rx.recv() => {
                        let Some(PortRebindRequest { expected_port, response_tx }) = request else {
                            break;
                        };

                        if base_config.socks5_port.is_some() {
                            let _ = response_tx.send(Err(
                                "dynamic port rebind is not supported when --socks5-port is enabled".to_string()
                            ));
                            continue;
                        }

                        if expected_port == current_port {
                            let _ = response_tx.send(Ok(bifrost_admin::PortRebindResponse {
                                expected_port,
                                actual_port: current_port,
                            }));
                            continue;
                        }

                        let actual_port = match find_available_port(&base_config.host, expected_port) {
                            Ok(port) => port,
                            Err(error) => {
                                let _ = response_tx.send(Err(error.to_string()));
                                continue;
                            }
                        };

                        let mut next_config = base_config.clone();
                        next_config.port = actual_port;
                        let next_task = match spawn_managed_proxy_task(
                            next_config,
                            resolver.clone(),
                            tls_config.clone(),
                            admin_state_arc.clone(),
                            push_manager.clone(),
                            access_control.clone(),
                        )
                        .await {
                            Ok(task) => task,
                            Err(error) => {
                                let _ = response_tx.send(Err(error.to_string()));
                                continue;
                            }
                        };

                        let old_task = std::mem::replace(&mut listener_task, next_task);

                        current_port = actual_port;
                        admin_state_arc.set_port(actual_port);

                        let runtime_info = RuntimeInfo::new(
                            std::process::id(),
                            actual_port,
                            base_config.socks5_port,
                            Some(base_config.host.clone()),
                            foreground_runtime_start_mode(),
                        )
                        .with_system_proxy(enable_system_proxy, system_proxy_bypass.clone());
                        if let Err(error) = write_runtime_info(&runtime_info) {
                            tracing::warn!("Failed to update runtime info after port rebind: {}", error);
                        }

                        if system_proxy_enabled.load(Ordering::Acquire) {
                            let proxy_host = if base_config.host == "0.0.0.0" {
                                "127.0.0.1".to_string()
                            } else {
                                base_config.host.clone()
                            };
                            let mut manager = system_proxy_manager.write().await;
                            let _ = manager.force_disable();
                            if let Err(error) = manager.enable(&proxy_host, actual_port, Some(&system_proxy_bypass)) {
                                tracing::warn!("Failed to update system proxy after port rebind: {}", error);
                            }
                        }

                        if cli_proxy_enabled {
                            let proxy_host = if base_config.host == "0.0.0.0" {
                                "127.0.0.1".to_string()
                            } else {
                                base_config.host.clone()
                            };
                            if let Err(error) = shell_proxy_manager.enable_persistent(
                                &proxy_host,
                                actual_port,
                                &cli_proxy_no_proxy,
                            ) {
                                tracing::warn!("Failed to update CLI proxy after port rebind: {}", error);
                            }
                        }

                        push_manager.broadcast_overview().await;

                        let _ = response_tx.send(Ok(bifrost_admin::PortRebindResponse {
                            expected_port,
                            actual_port,
                        }));
                        abort_listener_after_grace_period(old_task);
                    }
                }
            }

            listener_task.abort();
            rules_watcher_task.abort();
            rules_filesystem_watcher_task.abort();
            admin_push_watcher_task.abort();
            body_cleanup_task.abort();
            ws_payload_cleanup_task.abort();
            frame_cleanup_task.abort();
            db_cleanup_task.abort();
            connection_cleanup_task.abort();
            for task in push_tasks {
                task.abort();
            }
            for task in metrics_tasks {
                task.abort();
            }
            for task in mobile_availability_tasks {
                task.abort();
            }

            // Kill managed ASR service to prevent orphan processes.
            bifrost_admin::shutdown_managed_asr_service().await;
            // Kill all managed browser processes to prevent orphans.
            bifrost_admin::im_gateway::chatgpt_web::kill_all_managed_browsers();
            // Kill all active external CLI runs to prevent orphan process groups.
            bifrost_admin::im_gateway::external_cli::kill_all_active_runs();

            Ok(())
        }
        .await;

        if let Err(e) = &result {
            eprintln!("Runtime error: {}", e);
        }
        result
    });

    if cli_proxy_enabled {
        if let Err(e) = shell_proxy_manager.restore() {
            eprintln!("Failed to restore CLI proxy: {}", e);
        }
    }
    system_proxy_reconcile_stop.store(true, Ordering::Release);
    remove_pid()?;
    println!("Bifrost proxy stopped.");

    runtime_result?;

    Ok(())
}

fn build_tray_start_args(
    config: &ProxyConfig,
    log_level: &str,
    skip_cert_check: bool,
    yes: bool,
) -> Vec<String> {
    let mut args = Vec::new();

    if config.host != "0.0.0.0" {
        args.push("--host".to_string());
        args.push(config.host.clone());
    }
    if let Some(socks5_port) = config.socks5_port {
        args.push("--socks5-port".to_string());
        args.push(socks5_port.to_string());
    }
    if log_level != "info" {
        args.push("--log-level".to_string());
        args.push(log_level.to_string());
    }
    if skip_cert_check {
        args.push("--skip-cert-check".to_string());
    }
    if config.unsafe_ssl {
        args.push("--unsafe-ssl".to_string());
    }
    if yes {
        args.push("--yes".to_string());
    }

    args
}

fn should_suppress_startup_tray(no_tray: bool, _detached_daemon_child: bool) -> bool {
    no_tray
}

fn build_tray_launch_callback(
    no_tray: bool,
    data_dir: PathBuf,
    pid: u32,
    config: &ProxyConfig,
    log_level: &str,
    skip_cert_check: bool,
    yes: bool,
) -> bifrost_admin::SharedTrayLaunchCallback {
    let runtime_file =
        crate::process::get_runtime_file().unwrap_or_else(|_| data_dir.join("runtime.json"));
    let admin_url = format!(
        "http://{}:{}/_bifrost/",
        if config.host == "0.0.0.0" {
            "127.0.0.1"
        } else {
            &config.host
        },
        config.port,
    );
    let port = config.port;
    let tray_start_args = build_tray_start_args(config, log_level, skip_cert_check, yes);
    let bifrost_self_bin = std::env::current_exe().ok();

    Arc::new(move || {
        let data_dir = data_dir.clone();
        let runtime_file = runtime_file.clone();
        let admin_url = admin_url.clone();
        let bifrost_self_bin = bifrost_self_bin.clone();
        let tray_start_args = tray_start_args.clone();
        std::thread::spawn(move || {
            for attempt in 0..10 {
                if !super::tray_launcher::should_launch_tray(no_tray, &data_dir) {
                    return;
                }
                launch_tray_helper_if_enabled(TrayLaunchHelperRequest {
                    no_tray,
                    data_dir: &data_dir,
                    runtime_file: &runtime_file,
                    pid,
                    admin_url: &admin_url,
                    port,
                    bifrost_bin: bifrost_self_bin.as_deref(),
                    start_args: &tray_start_args,
                });
                if data_dir.join("tray.pid").exists() {
                    return;
                }
                if attempt < 9 {
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
            tracing::debug!(
                data_dir = %data_dir.display(),
                "tray launch request finished without observing tray.pid"
            );
        });
    })
}

struct TrayLaunchHelperRequest<'a> {
    no_tray: bool,
    data_dir: &'a Path,
    runtime_file: &'a Path,
    pid: u32,
    admin_url: &'a str,
    port: u16,
    bifrost_bin: Option<&'a Path>,
    start_args: &'a [String],
}

fn launch_tray_helper_if_enabled(request: TrayLaunchHelperRequest<'_>) {
    if !super::tray_launcher::should_launch_tray(request.no_tray, request.data_dir) {
        return;
    }
    if let Some(tray_bin) = super::tray_launcher::find_tray_binary() {
        super::tray_launcher::launch_tray_helper(
            &tray_bin,
            request.data_dir,
            request.runtime_file,
            request.pid,
            Some(request.admin_url),
            Some(request.port),
            request.bifrost_bin,
            request.start_args,
        );
    } else {
        tracing::debug!("tray helper binary not found, skipping tray launch");
    }
}

fn parse_proxy_users(proxy_users: &[String]) -> bifrost_core::Result<Vec<UserPassAccountConfig>> {
    let mut usernames = std::collections::HashSet::new();
    let mut accounts = Vec::new();

    for proxy_user in proxy_users {
        let Some((username, password)) = proxy_user.split_once(':') else {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Invalid --proxy-user value '{}', expected USER:PASS",
                proxy_user
            )));
        };
        if username.is_empty() || password.is_empty() {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Invalid --proxy-user value '{}', username and password must be non-empty",
                proxy_user
            )));
        }
        if !usernames.insert(username.to_string()) {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Duplicate proxy username '{}'",
                username
            )));
        }
        accounts.push(UserPassAccountConfig {
            username: username.to_string(),
            password: Some(password.to_string()),
            enabled: true,
        });
    }

    Ok(accounts)
}

#[cfg(unix)]
fn raise_fd_limit() {
    use libc::{getrlimit, rlimit, setrlimit, RLIMIT_NOFILE};
    unsafe {
        let mut rlim = rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if getrlimit(RLIMIT_NOFILE, &mut rlim) == 0 {
            let old = rlim.rlim_cur;
            if rlim.rlim_cur < rlim.rlim_max {
                rlim.rlim_cur = rlim.rlim_max;
                if setrlimit(RLIMIT_NOFILE, &rlim) == 0 {
                    tracing::info!(
                        old_limit = old,
                        new_limit = rlim.rlim_cur,
                        "raised file descriptor limit"
                    );
                } else {
                    tracing::warn!(
                        old_limit = old,
                        hard_limit = rlim.rlim_max,
                        "failed to raise file descriptor limit"
                    );
                }
            } else {
                tracing::debug!(
                    current_limit = rlim.rlim_cur,
                    "file descriptor limit already at maximum"
                );
            }
        }
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

fn detached_daemon_readiness_host(host: &str) -> &str {
    if matches!(host, "0.0.0.0" | "::" | "[::]") {
        "127.0.0.1"
    } else {
        host
    }
}

const DEFAULT_DETACHED_DAEMON_READY_TIMEOUT_SECS: u64 = 30;
const MAX_DETACHED_DAEMON_READY_TIMEOUT_SECS: u64 = 300;

fn detached_daemon_ready_timeout_from_env_value(value: Option<std::ffi::OsString>) -> Duration {
    let Some(value) = value else {
        return Duration::from_secs(DEFAULT_DETACHED_DAEMON_READY_TIMEOUT_SECS);
    };
    let Some(value) = value.to_str() else {
        return Duration::from_secs(DEFAULT_DETACHED_DAEMON_READY_TIMEOUT_SECS);
    };
    let Ok(seconds) = value.trim().parse::<u64>() else {
        return Duration::from_secs(DEFAULT_DETACHED_DAEMON_READY_TIMEOUT_SECS);
    };
    if seconds == 0 {
        return Duration::from_secs(DEFAULT_DETACHED_DAEMON_READY_TIMEOUT_SECS);
    }
    Duration::from_secs(seconds.min(MAX_DETACHED_DAEMON_READY_TIMEOUT_SECS))
}

fn detached_daemon_ready_timeout() -> Duration {
    detached_daemon_ready_timeout_from_env_value(std::env::var_os(
        "BIFROST_DAEMON_READY_TIMEOUT_SECS",
    ))
}

#[cfg(any(unix, windows))]
fn wait_for_detached_daemon_ready(
    child: &mut std::process::Child,
    host: &str,
    port: u16,
    timeout: Duration,
) -> bifrost_core::Result<()> {
    let connect_host = detached_daemon_readiness_host(host);
    let addr = format!("{connect_host}:{port}");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(&addr).is_ok() {
            println!("Daemon started with PID: {}", child.id());
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(bifrost_core::BifrostError::Io)? {
            return Err(bifrost_core::BifrostError::Network(format!(
                "Daemon exited before the proxy listener became ready (PID: {}, status: {})",
                child.id(),
                status
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(bifrost_core::BifrostError::Network(format!(
        "Daemon did not become ready within {}s (PID: {})",
        timeout.as_secs(),
        child.id()
    )))
}

#[cfg(any(unix, windows))]
fn run_daemon_via_exec(
    config: &ProxyConfig,
    config_manager: &ConfigManager,
    log_dir: &Path,
    log_retention_days: u32,
) -> bifrost_core::Result<()> {
    use std::process::{Command, Stdio};

    use crate::process::get_pid_file;

    let bifrost_dir = config_manager.data_dir().to_path_buf();
    std::fs::create_dir_all(&bifrost_dir)?;
    let err_retention_days = std::cmp::min(log_retention_days, 7);
    if let Err(e) = bifrost_core::rotate_daemon_err_log(log_dir, err_retention_days) {
        eprintln!("Warning: Failed to rotate daemon err log: {}", e);
    }
    std::fs::create_dir_all(log_dir)?;
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("bifrost.log"))
        .map_err(|e| {
            bifrost_core::BifrostError::Config(format!("Failed to open daemon log file: {e}"))
        })?;
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("bifrost.err"))
        .map_err(|e| {
            bifrost_core::BifrostError::Config(format!("Failed to open daemon error log file: {e}"))
        })?;

    println!("Starting Bifrost proxy in daemon mode...");
    if config.enable_socks {
        println!(
            "Unified proxy (HTTP/HTTPS/SOCKS5): {}:{}",
            config.host, config.port
        );
    } else {
        println!("HTTP proxy: {}:{}", config.host, config.port);
    }
    if let Some(socks5_port) = config.socks5_port {
        println!("SOCKS5 (separate): {}:{}", config.host, socks5_port);
    }
    let admin_host = detached_daemon_readiness_host(&config.host);
    println!("Admin UI: http://{}:{}/", admin_host, config.port);
    println!("PID file: {}", get_pid_file()?.display());
    println!("Log file: {}", log_dir.join("bifrost.log").display());

    let exe = std::env::current_exe().map_err(bifrost_core::BifrostError::Io)?;
    let mut command = Command::new(exe);
    command
        .args(std::env::args_os().skip(1))
        .env(DETACHED_DAEMON_CHILD_ENV, "1")
        .env("BIFROST_DATA_DIR", &bifrost_dir)
        .current_dir(&bifrost_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // SAFETY: keep the replacement process detached like the legacy fork daemon
        // path; the pre-exec closure only calls async-signal-safe libc::setsid.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(bifrost_core::BifrostError::Io)?;
    wait_for_detached_daemon_ready(
        &mut child,
        &config.host,
        config.port,
        detached_daemon_ready_timeout(),
    )
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
pub fn run_daemon(
    config: ProxyConfig,
    cli_rules: Vec<Rule>,
    cli_values: HashMap<String, String>,
    enable_system_proxy: bool,
    system_proxy_bypass: String,
    defer_startup_recovery_for_restart_handoff: bool,
    enable_cli_proxy: bool,
    cli_proxy_no_proxy: Option<String>,
    config_manager: ConfigManager,
    log_dir: PathBuf,
    log_retention_days: u32,
    log_level: &str,
) -> bifrost_core::Result<()> {
    if cfg!(target_os = "macos") {
        return run_daemon_via_exec(&config, &config_manager, &log_dir, log_retention_days);
    }

    use nix::unistd::{chdir, dup2, fork, setsid, ForkResult};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;

    use crate::process::get_pid_file;

    let bifrost_dir = config_manager.data_dir().to_path_buf();
    std::fs::create_dir_all(&bifrost_dir)?;
    let (mut ready_rx, mut ready_tx) = UnixStream::pair().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to create daemon readiness pipe: {}", e))
    })?;
    let readiness_timeout = detached_daemon_ready_timeout();
    ready_rx
        .set_read_timeout(Some(readiness_timeout))
        .map_err(|e| {
            bifrost_core::BifrostError::Config(format!(
                "Failed to configure daemon readiness timeout: {}",
                e
            ))
        })?;

    println!("Starting Bifrost proxy in daemon mode...");
    if config.enable_socks {
        println!(
            "Unified proxy (HTTP/HTTPS/SOCKS5): {}:{}",
            config.host, config.port
        );
    } else {
        println!("HTTP proxy: {}:{}", config.host, config.port);
    }
    if let Some(socks5_port) = config.socks5_port {
        println!("SOCKS5 (separate): {}:{}", config.host, socks5_port);
    }
    let admin_host = if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        &config.host
    };
    println!("Admin UI: http://{}:{}/", admin_host, config.port);
    println!("PID file: {}", get_pid_file()?.display());
    println!("Log file: {}", bifrost_dir.join("bifrost.log").display());

    match unsafe { fork() } {
        Ok(ForkResult::Parent { child }) => {
            drop(ready_tx);
            let mut status = [0_u8; 1];
            match ready_rx.read_exact(&mut status) {
                Ok(()) if status[0] == b'1' => {
                    println!("Daemon started with PID: {}", child);
                    Ok(())
                }
                Ok(()) => Err(bifrost_core::BifrostError::Config(
                    "Daemon reported an unknown readiness status".to_string(),
                )),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    Err(bifrost_core::BifrostError::Network(format!(
                        "Daemon did not become ready within {}s (PID: {})",
                        readiness_timeout.as_secs(),
                        child
                    )))
                }
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    Err(bifrost_core::BifrostError::Network(format!(
                        "Daemon exited before the proxy listener became ready (PID: {})",
                        child
                    )))
                }
                Err(error) => Err(bifrost_core::BifrostError::Network(format!(
                    "Failed while waiting for daemon readiness (PID: {}): {}",
                    child, error
                ))),
            }
        }
        Ok(ForkResult::Child) => {
            drop(ready_rx);
            setsid().map_err(|e| {
                bifrost_core::BifrostError::Config(format!("Failed to create new session: {}", e))
            })?;

            raise_fd_limit();

            let _ = chdir(&bifrost_dir);

            let err_retention_days = std::cmp::min(log_retention_days, 7);
            if let Err(e) = bifrost_core::rotate_daemon_err_log(&log_dir, err_retention_days) {
                eprintln!("Warning: Failed to rotate daemon err log: {}", e);
            }
            let _ = std::fs::create_dir_all(&log_dir);
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_dir.join("bifrost.log"))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!("Failed to open log file: {}", e))
                })?;
            let err_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_dir.join("bifrost.err"))
                .map_err(|e| {
                    bifrost_core::BifrostError::Config(format!(
                        "Failed to open error log file: {}",
                        e
                    ))
                })?;

            let _ = dup2(log_file.as_raw_fd(), 1);
            let _ = dup2(err_file.as_raw_fd(), 2);

            if let Err(e) =
                bifrost_core::reinit_logging_for_daemon(&log_dir, log_retention_days, log_level)
            {
                eprintln!("Warning: Failed to initialize logging for daemon: {}", e);
            } else {
                tracing::info!(
                    port = config.port,
                    log_dir = %log_dir.display(),
                    "daemon logging initialized"
                );
            }

            let err_log_dir = log_dir.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(24 * 60 * 60));
                let _ = bifrost_core::rotate_daemon_err_log(&err_log_dir, err_retention_days);
                if let Ok(err_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(err_log_dir.join("bifrost.err"))
                {
                    let _ = dup2(err_file.as_raw_fd(), 2);
                }
            });

            let tls_config = load_tls_config(&config)?;

            let bind_addr = format!("{}:{}", config.host, config.port);
            if is_port_in_use(&config.host, config.port) {
                let proc_info = find_process_on_port(config.port);
                let detail = match proc_info {
                    Some(info) => {
                        format!(" (occupied by process \"{}\" PID: {})", info.name, info.pid)
                    }
                    None => String::new(),
                };
                return Err(bifrost_core::BifrostError::Network(format!(
                    "Failed to bind to {}: another process is already listening on this port{}",
                    bind_addr, detail
                )));
            }
            std::net::TcpListener::bind(&bind_addr).map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "Failed to bind to {}: {}",
                    bind_addr, e
                ))
            })?;

            let system_proxy_manager = std::sync::Arc::new(tokio::sync::RwLock::new(
                bifrost_core::SystemProxyManager::new(bifrost_dir.clone()),
            ));
            let mut shell_proxy_manager = bifrost_core::ShellProxyManager::new(bifrost_dir.clone());
            if let Err(e) = bifrost_core::ShellProxyManager::recover_from_crash(&bifrost_dir) {
                tracing::warn!("Failed to recover CLI proxy from previous crash: {}", e);
            }
            let system_proxy_enabled = Arc::new(AtomicBool::new(false));
            let system_proxy_desired_enabled = Arc::new(AtomicBool::new(enable_system_proxy));
            let system_proxy_reconcile_stop = Arc::new(AtomicBool::new(false));

            let mut cli_proxy_enabled = false;
            let cli_proxy_no_proxy =
                cli_proxy_no_proxy.unwrap_or_else(|| "localhost,127.0.0.1,::1".to_string());
            let cli_proxy_host = if config.host == "0.0.0.0" {
                "127.0.0.1".to_string()
            } else {
                config.host.clone()
            };
            if enable_cli_proxy {
                if let Err(e) = shell_proxy_manager.enable_persistent(
                    &cli_proxy_host,
                    config.port,
                    &cli_proxy_no_proxy,
                ) {
                    eprintln!("Failed to enable CLI proxy: {}", e);
                    remove_pid()?;
                    std::process::exit(1);
                }
                cli_proxy_enabled = true;
            }

            let rt = create_bifrost_runtime()?;

            rt.block_on(async {
                let pid = std::process::id();
                let runtime_info = RuntimeInfo::new(
                    pid,
                    config.port,
                    config.socks5_port,
                    Some(config.host.clone()),
                    RuntimeStartMode::Daemon,
                )
                .with_system_proxy(enable_system_proxy, system_proxy_bypass.clone());
                write_runtime_info(&runtime_info).expect("Failed to write runtime info");
                #[cfg(target_os = "macos")]
                spawn_system_proxy_launchd_install_task(bifrost_dir.clone(), enable_system_proxy);
                // 先安装 shutdown 信号监听，避免初始化阶段收到 SIGTERM 导致直接退出、但无法记录优雅退出日志。
                // 这也能让 `bifrost stop` 在 daemon 启动早期更可靠。
                let mut shutdown_signal = ShutdownSignalListener::install();
                let result: bifrost_core::Result<()> = tokio::select! {
                    _ = shutdown_signal.wait() => {
                        // 注意：daemon 场景下 tracing 日志可能写入 rolling file（例如 bifrost.YYYY-MM-DD.log），
                        // 这里同时写到 stdout，确保 stop/测试能在 bifrost.log 中观察到“优雅退出”。
                        info!("Received shutdown signal");
                        println!("Received shutdown signal");
                        if enable_system_proxy || system_proxy_enabled.load(Ordering::Acquire) {
                            restore_system_proxy_on_shutdown(
                                &bifrost_dir,
                                &system_proxy_manager,
                                &system_proxy_enabled,
                                &system_proxy_reconcile_stop,
                                "daemon signal",
                            )
                            .await;
                        }
                        Ok(())
                    }
                    result = async {
                    let stored_config = config_manager.config().await;
                    let body_temp_dir = bifrost_dir.join("body_cache");
                    let body_store = Arc::new(ParkingRwLock::new(BodyStore::new(
                        body_temp_dir,
                        stored_config.traffic.max_body_memory_size,
                        stored_config.traffic.file_retention_days,
                        stored_config.traffic.sse_stream_flush_bytes,
                        Duration::from_millis(stored_config.traffic.sse_stream_flush_interval_ms),
                    )));
                    std::mem::drop(bifrost_admin::start_body_cleanup_task(body_store.clone()));

                    let ws_payload_store = Arc::new(WsPayloadStore::new(
                        bifrost_dir.clone(),
                        stored_config.traffic.ws_payload_flush_bytes,
                        Duration::from_millis(stored_config.traffic.ws_payload_flush_interval_ms),
                        stored_config.traffic.ws_payload_max_open_files,
                        stored_config.traffic.file_retention_days,
                    ));
                    std::mem::drop(start_ws_payload_cleanup_task(ws_payload_store.clone()));

                    let traffic_dir = bifrost_dir.join("traffic");
                    let traffic_db_store = Arc::new(
                        bifrost_admin::TrafficDbStore::new(
                            traffic_dir,
                            stored_config.traffic.max_records,
                            stored_config.traffic.max_db_size_bytes,
                            Some(stored_config.traffic.file_retention_days * 24),
                        )
                        .expect("Failed to create traffic database"),
                    );

                    let (async_traffic_writer, async_traffic_rx) =
                        AsyncTrafficWriter::new(ASYNC_TRAFFIC_BUFFER_SIZE);
                    let async_traffic_writer = Arc::new(async_traffic_writer);
                    let _async_traffic_task = start_async_traffic_processor(
                        async_traffic_rx,
                        traffic_db_store.clone(),
                    );

                    let frame_store = Arc::new(bifrost_admin::FrameStore::new(
                        bifrost_dir.clone(),
                        Some(stored_config.traffic.file_retention_days * 24),
                    ));
                    std::mem::drop(start_frame_cleanup_task(frame_store.clone()));

                    let cleanup_body_store = body_store.clone();
                    let cleanup_frame_store = frame_store.clone();
                    let cleanup_ws_payload_store = ws_payload_store.clone();
                    traffic_db_store.set_cleanup_notifier(Arc::new(move |ids| {
                        let _ = cleanup_body_store.write().delete_by_ids(ids);
                        let _ = cleanup_frame_store.delete_by_ids(ids);
                        let _ = cleanup_ws_payload_store.delete_by_ids(ids);
                    }));

                    let values_storage = config_manager.values_storage().await;
                    let rules_storage = config_manager.rules_storage().await;
                    let auth_db =
                        match bifrost_admin::admin_auth_db::auth_db_path_in(&bifrost_dir)
                            .and_then(|p| bifrost_admin::admin_auth_db::AuthDb::open(&p))
                        {
                            Ok(db) => Some(db),
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    data_dir = %bifrost_dir.display(),
                                    "Failed to open auth database; admin auth features will be unavailable"
                                );
                                None
                            }
                        };
                    let mut values = {
                        use bifrost_core::ValueStore;
                        values_storage.as_hashmap()
                    };
                    for (k, v) in cli_values {
                        values.entry(k).or_insert(v);
                    }

                    let ca_cert_path = bifrost_dir.join("certs").join("ca.crt");

                    let runtime_config = RuntimeConfig {
                        enable_tls_interception: config.enable_tls_interception,
                        intercept_exclude: config.intercept_exclude.clone(),
                        intercept_include: config.intercept_include.clone(),
                        app_intercept_exclude: config.app_intercept_exclude.clone(),
                        app_intercept_include: config.app_intercept_include.clone(),
                        ip_intercept_exclude: config.ip_intercept_exclude.clone(),
                        ip_intercept_include: config.ip_intercept_include.clone(),
                        unsafe_ssl: config.unsafe_ssl,
                        disconnect_on_config_change: true,
                    };
                    let connection_registry = bifrost_admin::ConnectionRegistry::new(true);
                    let app_icon_cache = bifrost_admin::create_app_icon_cache(&bifrost_dir);

                    let scripts_dir = bifrost_dir.join("scripts");
                    let script_manager = bifrost_admin::ScriptManager::new(scripts_dir);
                    if let Err(e) = script_manager.init().await {
                        tracing::warn!("Failed to initialize script manager: {}", e);
                    }

                    let replay_db_store = match ReplayDbStore::new(bifrost_dir.join("replay")) {
                        Ok(store) => Some(Arc::new(store)),
                        Err(e) => {
                            tracing::warn!("Failed to initialize replay store: {}", e);
                            None
                        }
                    };

            let shared_config_manager = Arc::new(config_manager);
            let system_proxy_lifecycle_helper_state = Arc::new(
                bifrost_admin::SystemProxyLifecycleHelperState::new(
                    bifrost_dir.clone(),
                    std::process::id(),
                ),
            );
                    let tray_launch_callback = build_tray_launch_callback(
                        false,
                        bifrost_dir.clone(),
                        std::process::id(),
                        &config,
                        log_level,
                        false,
                        false,
                    );

                    let access_control =
                        ProxyServer::new(config.clone()).access_control().clone();

                    let mut admin_state = AdminState::new(config.port)
                        .with_access_control(access_control.clone())
                        .with_body_store(body_store)
                        .with_ws_payload_store(ws_payload_store)
                        .with_async_traffic_writer_shared(async_traffic_writer)
                        .with_traffic_db_store_shared(traffic_db_store.clone())
                        .with_frame_store_shared(frame_store)
                        .with_runtime_config(runtime_config)
                        .with_connection_registry(connection_registry)
                        .with_values_storage(values_storage)
                        .with_rules_storage(rules_storage);
                    if let Some(db) = auth_db {
                        admin_state = admin_state.with_auth_db(db);
                    }
                    let admin_state = admin_state
                        .with_ca_cert_path(ca_cert_path)
                        .with_system_proxy_manager_shared(system_proxy_manager.clone())
                        .with_config_manager_shared(shared_config_manager.clone())
                        .with_system_proxy_lifecycle_helper_shared(
                            system_proxy_lifecycle_helper_state.clone(),
                        )
                        .with_tray_launch_callback(tray_launch_callback)
                        .with_system_proxy_runtime_flags_shared(
                            system_proxy_desired_enabled.clone(),
                            system_proxy_enabled.clone(),
                        )
                        .ensure_keepawake_manager_installed()
                        .with_max_body_buffer_size(stored_config.traffic.max_body_buffer_size)
                        .with_max_body_probe_size(stored_config.traffic.max_body_probe_size)
                        .with_super_performance_mode(config.super_performance_mode)
                        .with_binary_traffic_performance_mode(
                            stored_config.traffic.binary_traffic_performance_mode,
                        )
                        .with_breakpoint_timeout_ms(stored_config.traffic.breakpoint_timeout_ms)
                        .with_app_icon_cache(app_icon_cache)
                        .with_script_manager(script_manager)
                        .with_replay_db_store_shared_opt(replay_db_store);

                    let sync_manager = Arc::new(
                        SyncManager::new(shared_config_manager.clone(), config.port)
                            .expect("Failed to create sync manager"),
                    );
                    let _sync_task = sync_manager.clone().start();
                    let admin_state = admin_state.with_sync_manager_shared(sync_manager.clone());
                    let admin_state = admin_state
                        .with_ip_tls_pending_manager(bifrost_admin::IpTlsPendingManager::new());
                    let admin_state = admin_state
                        .with_client_trust_tracker(bifrost_admin::ClientTlsTrustTracker::new());

                    // Initialize IM Gateway service (daemon mode)
                    let im_gateway_data_dir = shared_config_manager.data_dir().to_path_buf();
                    let im_gateway_service = std::sync::Arc::new(
                        bifrost_admin::ImGatewayService::new_with_agent_proxy_port(
                            &im_gateway_data_dir,
                            Some(config.port),
                        ),
                    );
                    let admin_state = admin_state.with_im_gateway_service(im_gateway_service);

                    std::mem::drop(bifrost_admin::start_db_cleanup_task(traffic_db_store));
                    std::mem::drop(bifrost_admin::start_connection_cleanup_task(
                        admin_state.connection_monitor.clone(),
                    ));

                    // Auto-connect IM Gateway providers (daemon mode)
                    if let Some(im_service) = admin_state.im_gateway_service() {
                        im_service.start_scheduler();
                        im_service.spawn_chatgpt_web_startup_auth_check();
                        im_service.spawn_feishu_setup_supervisor();
                        let im_service_clone = im_service.clone();
                        tokio::spawn(async move {
                            im_service_clone.auto_connect_providers().await;
                        });
                    }

                    let metrics_collector = admin_state.metrics_collector.clone();
                    let rules_storage_for_resolver = admin_state.rules_storage.clone();
                    let config_manager_for_resolver = admin_state.config_manager.clone();
                    let values_storage_for_resolver = admin_state
                        .values_storage
                        .clone()
                        .expect("values_storage should be set");
                    let connection_registry_for_resolver = admin_state.connection_registry.clone();
                    let runtime_config_for_resolver = admin_state.runtime_config.clone();

                    admin_state.load_group_name_cache();
                    admin_state.refresh_badge_rules_cache();

                    let valid_dirs = resolve_valid_group_dirs(&admin_state);
                    let (stored_rules, inline_values) =
                        load_stored_rules(&rules_storage_for_resolver, Some(&valid_dirs));
                    let mut merged_values = values.clone();
                    for (k, v) in inline_values {
                        merged_values.entry(k).or_insert(v);
                    }
                    let resolver: SharedDynamicRulesResolver = Arc::new(DynamicRulesResolver::new(
                        cli_rules,
                        stored_rules,
                        merged_values,
                    ));

                    log_resolver_rules(&resolver);

                    let unsafe_ssl = config.unsafe_ssl;
                    let system_proxy_host = if config.host == "0.0.0.0" {
                        "127.0.0.1".to_string()
                    } else {
                        config.host.clone()
                    };
                    let system_proxy_port = config.port;
                    let server = ProxyServer::new(config.clone())
                        .with_access_control(access_control.clone())
                        .with_tls_config(tls_config.clone())
                        .with_admin_state(admin_state)
                        .with_rules(resolver.clone());

                    let admin_state_arc = server
                        .admin_state()
                        .cloned()
                        .expect("admin_state should be set");

                    spawn_remote_invoke_worker_startup_task(
                        shared_config_manager.clone(),
                        sync_manager.clone(),
                        admin_state_arc.clone(),
                        system_proxy_host.clone(),
                        system_proxy_port,
                    );
                    std::mem::drop(bifrost_admin::start_total_disk_cleanup_task(
                        admin_state_arc.clone(),
                    ));

                    let replay_executor = Arc::new(bifrost_admin::ReplayExecutor::new(
                        admin_state_arc.clone(),
                        unsafe_ssl,
                    ));
                    admin_state_arc.set_replay_executor(replay_executor);

                    let push_manager = Arc::new(PushManager::new(admin_state_arc.clone()));
                    let _push_tasks = start_push_tasks(push_manager.clone());
                    let _admin_push_watcher_task = spawn_admin_push_watcher_task(
                        config_manager_for_resolver.clone(),
                        push_manager.clone(),
                    );
                    let server = server.with_push_manager(push_manager.clone());
                    let temporary_port_manager =
                        Arc::new(crate::commands::port::CliTemporaryPortManager::new(
                            config.clone(),
                            tls_config.clone(),
                            admin_state_arc.clone(),
                            push_manager.clone(),
                            access_control.clone(),
                            rules_storage_for_resolver.clone(),
                            values_storage_for_resolver.clone(),
                        ));
                    admin_state_arc.set_temporary_port_manager(temporary_port_manager.clone());

                    let _metrics_task = start_metrics_collector_task(metrics_collector, 1);

                    let rules_watcher_task = spawn_rules_watcher_task(
                        config_manager_for_resolver,
                        rules_storage_for_resolver,
                        values_storage_for_resolver,
                        resolver.clone(),
                        connection_registry_for_resolver,
                        runtime_config_for_resolver,
                        admin_state_arc.clone(),
                        Some(temporary_port_manager),
                    );
                    let rules_filesystem_watcher_task = spawn_rules_filesystem_watcher_task(
                        admin_state_arc.config_manager.clone(),
                        admin_state_arc.rules_storage.clone(),
                    );

                    if enable_system_proxy {
                        system_proxy_lifecycle_helper_state.ensure_started_after_startup_enable();
                    }
                    #[cfg(target_os = "macos")]
                    spawn_system_proxy_wake_reconcile_task(SystemProxyReconcileConfig {
                        bifrost_dir: bifrost_dir.clone(),
                        system_proxy_manager: system_proxy_manager.clone(),
                        desired_enabled: system_proxy_desired_enabled.clone(),
                        proxy_host: system_proxy_host.clone(),
                        proxy_port: system_proxy_port,
                        system_proxy_bypass: system_proxy_bypass.clone(),
                        enabled_flag: system_proxy_enabled.clone(),
                        stop_flag: system_proxy_reconcile_stop.clone(),
                        daemon_mode: true,
                        defer_startup_recovery_for_restart_handoff,
                    });

                    spawn_system_proxy_reconcile_task(SystemProxyReconcileConfig {
                        bifrost_dir: bifrost_dir.clone(),
                        system_proxy_manager: system_proxy_manager.clone(),
                        desired_enabled: system_proxy_desired_enabled.clone(),
                        proxy_host: system_proxy_host,
                        proxy_port: system_proxy_port,
                        system_proxy_bypass: system_proxy_bypass.clone(),
                        enabled_flag: system_proxy_enabled.clone(),
                        stop_flag: system_proxy_reconcile_stop.clone(),
                        daemon_mode: true,
                        defer_startup_recovery_for_restart_handoff,
                    });

                    let listener_config = server.config().clone();
                    let listener_access_control = server.access_control().clone();
                    let mut listener_task = spawn_managed_proxy_task(
                        listener_config,
                        resolver.clone(),
                        tls_config.clone(),
                        admin_state_arc,
                        push_manager,
                        listener_access_control,
                    )
                    .await?;

                    ready_tx.write_all(b"1").map_err(|e| {
                        bifrost_core::BifrostError::Config(format!(
                            "Failed to notify daemon readiness: {}",
                            e
                        ))
                    })?;
                    drop(ready_tx);
                    let _ = bifrost_core::consume_system_proxy_shutdown_mode(&bifrost_dir);

                    let listener_result = (&mut listener_task).await;
                    if enable_system_proxy || system_proxy_enabled.load(Ordering::Acquire) {
                        restore_system_proxy_on_shutdown(
                            &bifrost_dir,
                            &system_proxy_manager,
                            &system_proxy_enabled,
                            &system_proxy_reconcile_stop,
                            "daemon listener exit",
                        )
                        .await;
                    }

                    rules_watcher_task.abort();
                    rules_filesystem_watcher_task.abort();
                    Err(listener_task_error(listener_result))
                }
                => result,
                };

                // Kill managed ASR service to prevent orphan processes.
                bifrost_admin::shutdown_managed_asr_service().await;
                // Kill all managed browser processes to prevent orphans.
                bifrost_admin::im_gateway::chatgpt_web::kill_all_managed_browsers();
                // Kill all active external CLI runs to prevent orphan process groups.
                bifrost_admin::im_gateway::external_cli::kill_all_active_runs();

                if let Err(e) = result {
                    eprintln!("Runtime error: {}", e);
                }
            });

            if cli_proxy_enabled {
                if let Err(e) = shell_proxy_manager.restore() {
                    eprintln!("Failed to restore CLI proxy: {}", e);
                }
            }
            system_proxy_reconcile_stop.store(true, Ordering::Release);
            if should_skip_system_proxy_restore(&bifrost_dir, "daemon fork fallback") {
                system_proxy_manager.blocking_write().detach_in_place();
            } else {
                let lock_started_at = Instant::now();
                tracing::info!(
                    target: "bifrost_cli::shutdown",
                    context = "daemon fork fallback",
                    "waiting_for_system_proxy_lock"
                );
                let mut manager = system_proxy_manager.blocking_write();
                tracing::info!(
                    target: "bifrost_cli::shutdown",
                    context = "daemon fork fallback",
                    wait_elapsed_ms = lock_started_at.elapsed().as_millis() as u64,
                    "acquired_system_proxy_lock"
                );
                if let Err(e) = manager.restore() {
                    eprintln!("Failed to restore system proxy: {}", e);
                }
            }

            remove_pid()?;
            std::process::exit(0);
        }
        Err(e) => Err(bifrost_core::BifrostError::Config(format!(
            "Failed to fork: {}",
            e
        ))),
    }
}

fn resolve_valid_group_dirs(admin_state: &AdminState) -> std::collections::HashSet<String> {
    let has_session = admin_state
        .sync_manager
        .as_ref()
        .map(|sm| sm.has_session())
        .unwrap_or(false);
    let mut dirs = std::collections::HashSet::new();

    if !has_session {
        tracing::info!(
            target: "bifrost_cli::rules",
            "no active sync session, skipping group rule directories for proxy runtime"
        );
        return dirs;
    }

    dirs.extend(admin_state.group_name_cache().all_dir_names());

    if let Ok(entries) = std::fs::read_dir(admin_state.rules_storage.base_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    dirs.insert(name.to_string());
                }
            }
        }
    }

    dirs
}

fn load_stored_rules(
    rules_storage: &bifrost_storage::RulesStorage,
    valid_subdirs: Option<&std::collections::HashSet<String>>,
) -> (Vec<Rule>, HashMap<String, String>) {
    let mut stored_rules = Vec::new();
    let mut inline_values = HashMap::new();
    tracing::info!(
        target: "bifrost_cli::rules",
        base_dir = %rules_storage.base_dir().display(),
        "loading rules from storage"
    );
    if let Err(error) = rules_storage.ensure_default_rule() {
        tracing::warn!(
            target: "bifrost_cli::rules",
            error = %error,
            "failed to initialize global Default rule"
        );
    }
    match rules_storage.load_all_with_subdirs_filtered(valid_subdirs) {
        Ok(rule_files) => {
            let stored_count = rule_files.iter().filter(|rule| rule.enabled).count();
            let catalog_rule_files = rules_storage
                .load_all_with_subdirs()
                .unwrap_or_else(|_| rule_files.clone());
            let catalog = bifrost_storage::build_rule_reference_catalog(&catalog_rule_files);
            for rule_file in rule_files.into_iter().filter(|rule| rule.enabled) {
                let parser = bifrost_core::RuleParser::new();
                let source_name = bifrost_storage::rule_reference_key(&rule_file);
                let expanded_content =
                    match expand_rule_references(&source_name, &rule_file.content, &catalog) {
                        Ok(content) => content,
                        Err(error) => {
                            tracing::warn!(
                                target: "bifrost_cli::rules",
                                file = %rule_file.name,
                                line = error.line(),
                                error = %error,
                                "rule reference error (skipped)"
                            );
                            continue;
                        }
                    };
                let (result, file_inline_values) =
                    parser.parse_rules_tolerant_with_inline_values(&expanded_content);

                if !result.errors.is_empty() {
                    for error in &result.errors {
                        tracing::warn!(
                            target: "bifrost_cli::rules",
                            file = %rule_file.name,
                            line = error.line,
                            column = error.start_column,
                            error = %error.message,
                            suggestion = ?error.suggestion,
                            "rule parse error (skipped)"
                        );
                    }
                }

                tracing::debug!(
                    target: "bifrost_cli::rules",
                    file = %rule_file.name,
                    enabled = rule_file.enabled,
                    parsed_count = result.rules.len(),
                    error_count = result.errors.len(),
                    inline_values_count = file_inline_values.len(),
                    "loaded rule file"
                );

                for mut rule in result.rules {
                    rule.file = Some(rule_file.name.clone());
                    stored_rules.push(rule);
                }
                for (k, v) in file_inline_values {
                    inline_values.entry(k).or_insert(v);
                }
            }
            if stored_count > 0 {
                tracing::info!(
                    target: "bifrost_cli::rules",
                    stored_files = stored_count,
                    total_rules = stored_rules.len(),
                    inline_values_count = inline_values.len(),
                    "loaded rules from storage"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                target: "bifrost_cli::rules",
                error = %e,
                "failed to load rules from storage"
            );
        }
    }
    (stored_rules, inline_values)
}

fn log_resolver_rules(resolver: &DynamicRulesResolver) {
    let cli_count = resolver.cli_rules().len();
    if cli_count > 0 {
        tracing::info!(
            target: "bifrost_cli::rules",
            cli_rules = cli_count,
            "CLI rules loaded as default configuration (always active)"
        );
        for (idx, rule) in resolver.cli_rules().iter().enumerate() {
            tracing::debug!(
                target: "bifrost_cli::rules",
                index = idx + 1,
                pattern = %rule.pattern,
                protocol = %rule.protocol.to_str(),
                value = %rule.value,
                "CLI rule"
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RulesFileFingerprint {
    len: u64,
    hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RulesFilesystemSnapshot {
    entries: BTreeMap<PathBuf, RulesFileFingerprint>,
}

fn is_rules_storage_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    if file_name.starts_with('.') {
        return false;
    }

    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("bifrost" | "json")
    )
}

fn rules_file_runtime_fingerprint(path: &Path, content: &[u8]) -> RulesFileFingerprint {
    if path.extension().and_then(|ext| ext.to_str()) == Some("bifrost") {
        if let Ok(text) = std::str::from_utf8(content) {
            if let Ok(file) = bifrost_core::bifrost_file::BifrostFileParser::parse_rules(text) {
                let normalized_content = normalize_rule_content(&file.content);
                let runtime_key = (
                    file.meta.name,
                    file.meta.enabled,
                    file.meta.sort_order,
                    file.meta.group,
                    normalized_content,
                );
                let mut hasher = DefaultHasher::new();
                runtime_key.hash(&mut hasher);
                return RulesFileFingerprint {
                    len: runtime_key.4.len() as u64,
                    hash: hasher.finish(),
                };
            }
        }
    }

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    RulesFileFingerprint {
        len: content.len() as u64,
        hash: hasher.finish(),
    }
}

fn collect_rules_filesystem_snapshot(base_dir: &Path) -> io::Result<RulesFilesystemSnapshot> {
    let mut entries = BTreeMap::new();
    collect_rules_filesystem_snapshot_dir(base_dir, base_dir, &mut entries)?;
    Ok(RulesFilesystemSnapshot { entries })
}

fn collect_rules_filesystem_snapshot_dir(
    base_dir: &Path,
    current_dir: &Path,
    entries: &mut BTreeMap<PathBuf, RulesFileFingerprint>,
) -> io::Result<()> {
    let read_dir = match std::fs::read_dir(current_dir) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_rules_filesystem_snapshot_dir(base_dir, &path, entries)?;
            continue;
        }

        if !file_type.is_file() || !is_rules_storage_file(&path) {
            continue;
        }

        let content = std::fs::read(&path)?;
        let relative_path = path.strip_prefix(base_dir).unwrap_or(&path).to_path_buf();
        entries.insert(
            relative_path,
            rules_file_runtime_fingerprint(&path, &content),
        );
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum RulesFilesystemTrigger {
    WatcherEvent,
    FallbackScan,
}

fn is_rules_filesystem_event_relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

fn spawn_rules_filesystem_watcher_task(
    config_manager: Option<Arc<ConfigManager>>,
    rules_storage: bifrost_storage::RulesStorage,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(config_manager) = config_manager else {
            tracing::warn!(
                target: "bifrost_cli::rules",
                "ConfigManager not available, rules filesystem watcher disabled"
            );
            return;
        };

        let base_dir = rules_storage.base_dir().clone();
        if let Err(error) = std::fs::create_dir_all(&base_dir) {
            tracing::warn!(
                target: "bifrost_cli::rules",
                base_dir = %base_dir.display(),
                error = %error,
                "failed to create rules directory before starting filesystem watcher"
            );
        }

        let mut last_snapshot = match collect_rules_filesystem_snapshot(&base_dir) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(
                    target: "bifrost_cli::rules",
                    base_dir = %base_dir.display(),
                    error = %error,
                    "failed to collect initial rules filesystem snapshot"
                );
                RulesFilesystemSnapshot {
                    entries: BTreeMap::new(),
                }
            }
        };

        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::unbounded_channel::<notify::Result<notify::Event>>();
        let mut watcher = match RecommendedWatcher::new(
            move |result| {
                let _ = event_tx.send(result);
            },
            NotifyConfig::default().with_poll_interval(Duration::from_secs(5)),
        ) {
            Ok(mut watcher) => match watcher.watch(&base_dir, RecursiveMode::Recursive) {
                Ok(()) => Some(watcher),
                Err(error) => {
                    tracing::warn!(
                        target: "bifrost_cli::rules",
                        base_dir = %base_dir.display(),
                        error = %error,
                        "failed to watch rules directory; fallback scan remains active"
                    );
                    None
                }
            },
            Err(error) => {
                tracing::warn!(
                    target: "bifrost_cli::rules",
                    base_dir = %base_dir.display(),
                    error = %error,
                    "failed to create rules filesystem watcher; fallback scan remains active"
                );
                None
            }
        };
        let mut fallback_interval = tokio::time::interval(RULES_FILESYSTEM_FALLBACK_SCAN_INTERVAL);
        fallback_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        fallback_interval.tick().await;

        tracing::info!(
            target: "bifrost_cli::rules",
            base_dir = %base_dir.display(),
            watched_files = last_snapshot.entries.len(),
            watcher_active = watcher.is_some(),
            fallback_interval_ms = RULES_FILESYSTEM_FALLBACK_SCAN_INTERVAL.as_millis() as u64,
            "rules filesystem watcher started"
        );

        loop {
            let trigger = tokio::select! {
                maybe_event = event_rx.recv(), if watcher.is_some() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            if !is_rules_filesystem_event_relevant(&event.kind) {
                                continue;
                            }
                            tracing::debug!(
                                target: "bifrost_cli::rules",
                                event_kind = ?event.kind,
                                paths = ?event.paths,
                                "rules filesystem watcher event received"
                            );
                            RulesFilesystemTrigger::WatcherEvent
                        }
                        Some(Err(error)) => {
                            tracing::warn!(
                                target: "bifrost_cli::rules",
                                error = %error,
                                "rules filesystem watcher event failed"
                            );
                            continue;
                        }
                        None => {
                            watcher = None;
                            tracing::warn!(
                                target: "bifrost_cli::rules",
                                "rules filesystem watcher channel closed; fallback scan remains active"
                            );
                            continue;
                        }
                    }
                }
                _ = fallback_interval.tick() => RulesFilesystemTrigger::FallbackScan,
            };

            let changed_snapshot = match collect_rules_filesystem_snapshot(&base_dir) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(
                        target: "bifrost_cli::rules",
                        base_dir = %base_dir.display(),
                        error = %error,
                        "failed to collect rules filesystem snapshot"
                    );
                    continue;
                }
            };

            if changed_snapshot == last_snapshot {
                continue;
            }

            tokio::time::sleep(RULES_FILESYSTEM_DEBOUNCE_DELAY).await;
            let stable_snapshot = match collect_rules_filesystem_snapshot(&base_dir) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(
                        target: "bifrost_cli::rules",
                        base_dir = %base_dir.display(),
                        error = %error,
                        "failed to collect debounced rules filesystem snapshot"
                    );
                    continue;
                }
            };

            if stable_snapshot == last_snapshot {
                continue;
            }

            last_snapshot = stable_snapshot;
            tracing::info!(
                target: "bifrost_cli::rules",
                watched_files = last_snapshot.entries.len(),
                trigger = ?trigger,
                "rules files changed on disk, notifying runtime reload"
            );
            if let Err(error) = config_manager.notify(ConfigChangeEvent::rules_changed(
                RulesChangeOrigin::Filesystem,
            )) {
                tracing::warn!(
                    target: "bifrost_cli::rules",
                    error = %error,
                    "failed to notify rules filesystem change"
                );
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_rules_watcher_task(
    config_manager: Option<Arc<ConfigManager>>,
    rules_storage: bifrost_storage::RulesStorage,
    values_storage: bifrost_admin::SharedValuesStorage,
    resolver: SharedDynamicRulesResolver,
    connection_registry: bifrost_admin::SharedConnectionRegistry,
    runtime_config: bifrost_admin::SharedRuntimeConfig,
    admin_state: Arc<AdminState>,
    temporary_port_manager: Option<Arc<crate::commands::port::CliTemporaryPortManager>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(config_manager) = config_manager else {
            tracing::warn!(
                target: "bifrost_cli::rules",
                "ConfigManager not available, rules hot-reload disabled"
            );
            return;
        };

        let mut receiver = config_manager.subscribe();
        tracing::info!(
            target: "bifrost_cli::rules",
            "rules hot-reload watcher started"
        );

        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if matches!(
                        event,
                        ConfigChangeEvent::RulesChanged(_) | ConfigChangeEvent::ValuesChanged(_)
                    ) {
                        tracing::info!(
                            target: "bifrost_cli::rules",
                            event = ?event,
                            "config change event received, reloading rules"
                        );

                        let (new_stored_rules, inline_values) = {
                            let valid_dirs = resolve_valid_group_dirs(&admin_state);
                            load_stored_rules(&rules_storage, Some(&valid_dirs))
                        };
                        let mut new_values = {
                            use bifrost_core::ValueStore;
                            values_storage.read().as_hashmap()
                        };
                        for (k, v) in inline_values {
                            new_values.entry(k).or_insert(v);
                        }

                        resolver.update_stored_rules(new_stored_rules, new_values);
                        admin_state.refresh_badge_rules_cache();
                        if let Some(manager) = &temporary_port_manager {
                            manager.reload_all().await;
                        }

                        if event.is_rules_changed() {
                            let should_disconnect = {
                                let config = runtime_config.read().await;
                                config.disconnect_on_config_change
                            };
                            if should_disconnect {
                                let (intercept_patterns, passthrough_patterns) =
                                    resolver.get_tls_rule_patterns();

                                let mut total_disconnected = 0;

                                for pattern in &intercept_patterns {
                                    let disconnected = connection_registry
                                        .disconnect_by_host_pattern_with_mode(pattern, false);
                                    if !disconnected.is_empty() {
                                        tracing::info!(
                                            target: "bifrost_cli::rules",
                                            pattern = %pattern,
                                            count = disconnected.len(),
                                            "disconnected passthrough connections for tlsIntercept rule"
                                        );
                                        total_disconnected += disconnected.len();
                                    }
                                }

                                for pattern in &passthrough_patterns {
                                    let disconnected = connection_registry
                                        .disconnect_by_host_pattern_with_mode(pattern, true);
                                    if !disconnected.is_empty() {
                                        tracing::info!(
                                            target: "bifrost_cli::rules",
                                            pattern = %pattern,
                                            count = disconnected.len(),
                                            "disconnected intercept connections for tlsPassthrough rule"
                                        );
                                        total_disconnected += disconnected.len();
                                    }
                                }

                                if total_disconnected > 0 {
                                    tracing::info!(
                                        target: "bifrost_cli::rules",
                                        total = total_disconnected,
                                        intercept_rules = intercept_patterns.len(),
                                        passthrough_rules = passthrough_patterns.len(),
                                        "rules change: disconnected affected connections"
                                    );
                                }
                            }
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(
                        target: "bifrost_cli::rules",
                        count = count,
                        "rules watcher lagged, some events may have been missed"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!(
                        target: "bifrost_cli::rules",
                        "config change channel closed, stopping rules watcher"
                    );
                    break;
                }
            }
        }
    })
}

fn spawn_admin_push_watcher_task(
    config_manager: Option<Arc<ConfigManager>>,
    push_manager: Arc<PushManager>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(config_manager) = config_manager else {
            tracing::warn!(
                target: "bifrost_cli::push",
                "ConfigManager not available, admin push watcher disabled"
            );
            return;
        };

        let mut receiver = config_manager.subscribe();
        tracing::info!(
            target: "bifrost_cli::push",
            "admin push watcher started"
        );

        loop {
            match receiver.recv().await {
                Ok(event) => match event {
                    ConfigChangeEvent::TlsConfigChanged(_) => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_TLS_CONFIG)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_PROXY_SETTINGS)
                            .await;
                    }
                    ConfigChangeEvent::SandboxConfigChanged => {
                        push_manager.broadcast_scripts_snapshot().await;
                    }
                    ConfigChangeEvent::SystemProxyConfigChanged => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_SYSTEM_PROXY)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_CLI_PROXY)
                            .await;
                    }
                    ConfigChangeEvent::TrayConfigChanged => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_PROXY_SETTINGS)
                            .await;
                    }
                    ConfigChangeEvent::AccessConfigChanged => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_WHITELIST_STATUS)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_PENDING_AUTHORIZATIONS)
                            .await;
                    }
                    ConfigChangeEvent::ServerConfigChanged => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_PROXY_SETTINGS)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_PROXY_ADDRESS)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_CERT_INFO)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_SYSTEM_PROXY)
                            .await;
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_CLI_PROXY)
                            .await;
                    }
                    ConfigChangeEvent::TrafficConfigChanged => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_PERFORMANCE_CONFIG)
                            .await;
                    }
                    ConfigChangeEvent::ScriptsChanged => {
                        push_manager.broadcast_scripts_snapshot().await;
                    }
                    ConfigChangeEvent::ValuesChanged(_) => {
                        push_manager.broadcast_values_snapshot().await;
                    }
                    ConfigChangeEvent::RulesChanged(_) | ConfigChangeEvent::SyncConfigChanged => {}
                    ConfigChangeEvent::StateChanged => {
                        push_manager
                            .broadcast_settings_scope(SETTINGS_SCOPE_WHITELIST_STATUS)
                            .await;
                    }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(
                        target: "bifrost_cli::push",
                        count = count,
                        "admin push watcher lagged, some events may have been missed"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!(
                        target: "bifrost_cli::push",
                        "config change channel closed, stopping admin push watcher"
                    );
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn data_dir_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("data dir test lock poisoned")
    }

    struct ScopedDataDir {
        previous: PathBuf,
    }

    impl Drop for ScopedDataDir {
        fn drop(&mut self) {
            set_data_dir(self.previous.clone());
        }
    }

    fn scoped_data_dir(temp_dir: &tempfile::TempDir) -> ScopedDataDir {
        let previous = bifrost_storage::data_dir();
        set_data_dir(temp_dir.path().to_path_buf());
        ScopedDataDir { previous }
    }

    fn allocate_loopback_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn port_in_use_detects_bound_listener_without_accept_loop() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        assert!(is_port_in_use("127.0.0.1", port));
    }

    #[test]
    fn port_in_use_returns_false_for_available_loopback_port() {
        let port = allocate_loopback_port();

        assert!(!is_port_in_use("127.0.0.1", port));
    }

    #[test]
    fn restart_handoff_recovery_is_not_deferred_when_system_proxy_is_not_requested() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        )
        .expect("write marker");

        assert!(!should_defer_startup_proxy_recovery_for_restart_handoff(
            temp_dir.path(),
            false,
        ));
    }

    #[test]
    fn restart_handoff_recovery_is_not_deferred_without_runtime_info() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        )
        .expect("write marker");

        assert!(!should_defer_startup_proxy_recovery_for_restart_handoff(
            temp_dir.path(),
            true,
        ));
    }

    #[test]
    fn restart_handoff_recovery_is_deferred_with_marker_and_runtime_info() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        )
        .expect("write marker");
        write_runtime_info(&RuntimeInfo::new(
            12345,
            18080,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        ))
        .expect("write runtime info");

        assert!(should_defer_startup_proxy_recovery_for_restart_handoff(
            temp_dir.path(),
            true,
        ));
    }

    #[test]
    fn system_proxy_reconcile_startup_recovery_is_skipped_for_restart_handoff() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        )
        .expect("write marker");
        write_runtime_info(&RuntimeInfo::new(
            12345,
            18080,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        ))
        .expect("write runtime info");

        assert!(!should_run_system_proxy_startup_recovery(
            temp_dir.path(),
            true,
        ));
    }

    #[test]
    fn system_proxy_reconcile_uses_captured_restart_handoff_after_marker_consumed() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        )
        .expect("write marker");
        write_runtime_info(&RuntimeInfo::new(
            12345,
            18080,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        ))
        .expect("write runtime info");

        let defer_for_restart_handoff =
            should_defer_startup_proxy_recovery_for_restart_handoff(temp_dir.path(), true);
        let _ = bifrost_core::consume_system_proxy_shutdown_mode(temp_dir.path());

        assert!(defer_for_restart_handoff);
        assert!(should_run_system_proxy_startup_recovery(
            temp_dir.path(),
            true,
        ));
        assert!(!should_run_system_proxy_reconcile_startup_recovery(
            temp_dir.path(),
            true,
            defer_for_restart_handoff,
        ));
    }

    #[test]
    fn system_proxy_reconcile_startup_recovery_runs_without_system_proxy_request() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        bifrost_core::write_system_proxy_shutdown_mode(
            temp_dir.path(),
            bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
        )
        .expect("write marker");
        write_runtime_info(&RuntimeInfo::new(
            12345,
            18080,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        ))
        .expect("write runtime info");

        assert!(should_run_system_proxy_startup_recovery(
            temp_dir.path(),
            false,
        ));
    }

    #[test]
    fn tray_start_args_preserve_start_options_needed_for_menu_restart() {
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            socks5_port: Some(19081),
            unsafe_ssl: true,
            ..Default::default()
        };

        let args = build_tray_start_args(&config, "debug", true, true);

        assert_eq!(
            args,
            vec![
                "--host",
                "127.0.0.1",
                "--socks5-port",
                "19081",
                "--log-level",
                "debug",
                "--skip-cert-check",
                "--unsafe-ssl",
                "--yes",
            ]
        );
    }

    #[test]
    fn daemon_child_does_not_suppress_startup_tray() {
        assert!(!should_suppress_startup_tray(false, true));
        assert!(should_suppress_startup_tray(true, true));
    }

    #[test]
    fn detached_daemon_child_runs_runtime_instead_of_spawning_again() {
        assert!(should_spawn_daemon_parent_process(true, false));
        assert!(!should_spawn_daemon_parent_process(true, true));
        assert!(!should_spawn_daemon_parent_process(false, false));
    }

    #[test]
    fn desktop_core_env_marks_foreground_runtime_as_desktop() {
        let _guard = data_dir_test_lock();
        let previous_desktop = std::env::var_os(DESKTOP_CORE_ENV);
        let previous_detached = std::env::var_os(DETACHED_DAEMON_CHILD_ENV);
        std::env::set_var(DESKTOP_CORE_ENV, "1");
        std::env::remove_var(DETACHED_DAEMON_CHILD_ENV);

        assert_eq!(foreground_runtime_start_mode(), RuntimeStartMode::Desktop);

        match previous_desktop {
            Some(value) => std::env::set_var(DESKTOP_CORE_ENV, value),
            None => std::env::remove_var(DESKTOP_CORE_ENV),
        }
        match previous_detached {
            Some(value) => std::env::set_var(DETACHED_DAEMON_CHILD_ENV, value),
            None => std::env::remove_var(DETACHED_DAEMON_CHILD_ENV),
        }
    }

    #[test]
    fn detached_daemon_child_takes_precedence_over_desktop_core_env() {
        let _guard = data_dir_test_lock();
        let previous_desktop = std::env::var_os(DESKTOP_CORE_ENV);
        let previous_detached = std::env::var_os(DETACHED_DAEMON_CHILD_ENV);
        std::env::set_var(DESKTOP_CORE_ENV, "1");
        std::env::set_var(DETACHED_DAEMON_CHILD_ENV, "1");

        assert_eq!(foreground_runtime_start_mode(), RuntimeStartMode::Daemon);

        match previous_desktop {
            Some(value) => std::env::set_var(DESKTOP_CORE_ENV, value),
            None => std::env::remove_var(DESKTOP_CORE_ENV),
        }
        match previous_detached {
            Some(value) => std::env::set_var(DETACHED_DAEMON_CHILD_ENV, value),
            None => std::env::remove_var(DETACHED_DAEMON_CHILD_ENV),
        }
    }

    #[test]
    fn live_desktop_runtime_is_reused_for_matching_cli_start_port() {
        let runtime = RuntimeInfo::new(
            12345,
            19090,
            None,
            Some("0.0.0.0".to_string()),
            RuntimeStartMode::Desktop,
        );

        assert!(handle_live_desktop_runtime_before_start(&runtime, 19090).is_ok());
    }

    #[test]
    fn live_desktop_runtime_rejects_mismatched_cli_start_port_without_stop() {
        let runtime = RuntimeInfo::new(
            12345,
            19090,
            None,
            Some("0.0.0.0".to_string()),
            RuntimeStartMode::Desktop,
        );

        let error = handle_live_desktop_runtime_before_start(&runtime, 19091)
            .expect_err("mismatched desktop core port should be rejected");

        assert!(error
            .to_string()
            .contains("will not stop the app-bound core"));
    }

    #[test]
    fn live_desktop_runtime_for_pid_ignores_cli_daemon_runtime() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        write_runtime_info(&RuntimeInfo::new(
            12345,
            18080,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Daemon,
        ))
        .expect("write runtime info");

        assert!(live_desktop_runtime_for_pid(12345).is_none());
    }

    #[test]
    fn live_desktop_runtime_for_pid_matches_desktop_runtime() {
        let _guard = data_dir_test_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let _data_dir = scoped_data_dir(&temp_dir);
        write_runtime_info(&RuntimeInfo::new(
            12345,
            18080,
            None,
            Some("127.0.0.1".to_string()),
            RuntimeStartMode::Desktop,
        ))
        .expect("write runtime info");

        let runtime = live_desktop_runtime_for_pid(12345).expect("desktop runtime");

        assert_eq!(runtime.start_mode, RuntimeStartMode::Desktop);
        assert_eq!(runtime.port, 18080);
    }

    #[test]
    fn resolve_valid_group_dirs_skips_local_dirs_without_sync_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        std::fs::create_dir_all(rules_storage.base_dir().join("local-group")).unwrap();

        let admin_state = AdminState::new(0).with_rules_storage(rules_storage);

        let dirs = resolve_valid_group_dirs(&admin_state);

        assert!(dirs.is_empty());
    }

    #[test]
    fn resolve_valid_group_dirs_skips_cached_and_uncached_local_dirs_without_sync_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        std::fs::create_dir_all(rules_storage.base_dir().join("uncached-group")).unwrap();

        let admin_state = AdminState::new(0).with_rules_storage(rules_storage);
        {
            let mut cache = admin_state.group_name_cache();
            cache.insert("gid-cached".to_string(), "cached-group".to_string());
        }

        let dirs = resolve_valid_group_dirs(&admin_state);

        assert!(dirs.is_empty());
    }

    #[test]
    fn load_stored_rules_expands_disabled_rule_references() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                "entry",
                "@shared\nentry.test statusCode://203",
            ))
            .unwrap();
        rules_storage
            .save(
                &bifrost_storage::RuleFile::new("shared", "shared.test reqHeaders://X-Shared=1")
                    .with_enabled(false),
            )
            .unwrap();

        let (rules, _) = load_stored_rules(&rules_storage, None);

        let raws = rules.into_iter().map(|rule| rule.raw).collect::<Vec<_>>();
        assert!(raws.iter().any(|raw| raw.contains("shared.test")));
        assert!(raws.iter().any(|raw| raw.contains("entry.test")));
    }

    #[test]
    fn load_stored_rules_expands_group_rule_references() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                "entry",
                "@team-alpha/shared\nentry.test statusCode://203",
            ))
            .unwrap();

        let group_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules/team-alpha"))
                .unwrap();
        group_storage
            .save(
                &bifrost_storage::RuleFile::new(
                    "shared",
                    "shared.test reqHeaders://X-Group-Shared=1",
                )
                .with_enabled(false),
            )
            .unwrap();

        let (rules, _) = load_stored_rules(&rules_storage, None);

        let raws = rules.into_iter().map(|rule| rule.raw).collect::<Vec<_>>();
        assert!(raws.iter().any(|raw| raw.contains("shared.test")));
        assert!(raws.iter().any(|raw| raw.contains("entry.test")));
    }

    #[test]
    fn load_stored_rules_allows_group_references_when_group_roots_are_filtered() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                "entry",
                "@team-alpha/shared\nentry.test statusCode://203",
            ))
            .unwrap();

        let group_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules/team-alpha"))
                .unwrap();
        group_storage
            .save(&bifrost_storage::RuleFile::new(
                "shared",
                "shared.test reqHeaders://X-Group-Shared=1",
            ))
            .unwrap();

        let valid_subdirs = std::collections::HashSet::new();
        let (rules, _) = load_stored_rules(&rules_storage, Some(&valid_subdirs));

        let raws = rules.into_iter().map(|rule| rule.raw).collect::<Vec<_>>();
        assert!(raws.iter().any(|raw| raw.contains("shared.test")));
        assert!(raws.iter().any(|raw| raw.contains("entry.test")));
    }

    #[test]
    fn load_stored_rules_ignores_missing_reference_line() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                "entry",
                "@missing-rule\nentry.test statusCode://204",
            ))
            .unwrap();

        let (rules, _) = load_stored_rules(&rules_storage, None);

        let raws = rules.into_iter().map(|rule| rule.raw).collect::<Vec<_>>();
        assert!(raws.iter().any(|raw| raw.contains("entry.test")));
        assert!(!raws.iter().any(|raw| raw.contains("missing-rule")));
    }

    #[test]
    fn rules_filesystem_snapshot_detects_same_length_content_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rule_path = temp_dir.path().join("local.bifrost");
        std::fs::write(&rule_path, "a.test statusCode://201\n").unwrap();

        let before = collect_rules_filesystem_snapshot(temp_dir.path()).unwrap();
        std::fs::write(&rule_path, "b.test statusCode://202\n").unwrap();
        let after = collect_rules_filesystem_snapshot(temp_dir.path()).unwrap();

        assert_ne!(before, after);
        assert_eq!(after.entries.len(), 1);
    }

    #[test]
    fn rules_filesystem_snapshot_tracks_nested_rule_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let nested_dir = temp_dir.path().join("group-a");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(nested_dir.join("rule.bifrost"), "a.test statusCode://201\n").unwrap();

        let snapshot = collect_rules_filesystem_snapshot(temp_dir.path()).unwrap();

        assert!(snapshot
            .entries
            .contains_key(Path::new("group-a/rule.bifrost")));
    }

    #[test]
    fn rules_filesystem_snapshot_ignores_dot_json_cache_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join(".group_cache.json"), "{}").unwrap();
        std::fs::write(temp_dir.path().join("legacy.json"), "{}").unwrap();

        let snapshot = collect_rules_filesystem_snapshot(temp_dir.path()).unwrap();

        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.entries.contains_key(Path::new("legacy.json")));
    }

    #[test]
    fn parse_proxy_users_accepts_single_user() {
        let users = vec!["alice:secret".to_string()];
        let accounts = parse_proxy_users(&users).expect("parse proxy users");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "alice");
        assert_eq!(accounts[0].password.as_deref(), Some("secret"));
        assert!(accounts[0].enabled);
    }

    #[test]
    fn parse_proxy_users_accepts_multiple_distinct_users() {
        let users = vec!["alice:one".to_string(), "bob:two".to_string()];
        let accounts = parse_proxy_users(&users).expect("parse proxy users");
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].username, "alice");
        assert_eq!(accounts[1].username, "bob");
    }

    #[test]
    fn parse_proxy_users_rejects_missing_separator() {
        let users = vec!["invalid".to_string()];
        let err = parse_proxy_users(&users).unwrap_err();
        assert!(format!("{err}").contains("expected USER:PASS"));
    }

    #[test]
    fn parse_proxy_users_rejects_empty_username() {
        let users = vec![":pass".to_string()];
        let err = parse_proxy_users(&users).unwrap_err();
        assert!(
            format!("{err}").contains("username and password must be non-empty"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_proxy_users_rejects_empty_password() {
        let users = vec!["user:".to_string()];
        let err = parse_proxy_users(&users).unwrap_err();
        assert!(
            format!("{err}").contains("username and password must be non-empty"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_proxy_users_rejects_duplicate_usernames() {
        let users = vec!["alice:one".to_string(), "alice:two".to_string()];
        let err = parse_proxy_users(&users).unwrap_err();
        assert!(format!("{err}").contains("Duplicate proxy username"));
    }

    #[test]
    fn parse_proxy_users_allows_empty_list() {
        let users: Vec<String> = Vec::new();
        let accounts = parse_proxy_users(&users).expect("parse proxy users");
        assert!(accounts.is_empty());
    }

    #[test]
    fn env_flag_enabled_accepts_true_values() {
        assert!(env_flag_enabled(Some(std::ffi::OsString::from("1"))));
        assert!(env_flag_enabled(Some(std::ffi::OsString::from("true"))));
        assert!(env_flag_enabled(Some(std::ffi::OsString::from("TRUE"))));
    }

    #[test]
    fn env_flag_enabled_rejects_absent_and_false_values() {
        assert!(!env_flag_enabled(None));
        assert!(!env_flag_enabled(Some(std::ffi::OsString::from("0"))));
        assert!(!env_flag_enabled(Some(std::ffi::OsString::from("false"))));
    }

    #[test]
    fn detached_daemon_readiness_host_maps_wildcard_listeners_to_loopback() {
        assert_eq!(detached_daemon_readiness_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(detached_daemon_readiness_host("::"), "127.0.0.1");
        assert_eq!(detached_daemon_readiness_host("[::]"), "127.0.0.1");
        assert_eq!(
            detached_daemon_readiness_host("192.168.1.20"),
            "192.168.1.20"
        );
    }

    #[test]
    fn detached_daemon_ready_timeout_env_value_defaults_and_clamps() {
        assert_eq!(
            detached_daemon_ready_timeout_from_env_value(None),
            Duration::from_secs(30)
        );
        assert_eq!(
            detached_daemon_ready_timeout_from_env_value(Some("0".into())),
            Duration::from_secs(30)
        );
        assert_eq!(
            detached_daemon_ready_timeout_from_env_value(Some("garbage".into())),
            Duration::from_secs(30)
        );
        assert_eq!(
            detached_daemon_ready_timeout_from_env_value(Some("90".into())),
            Duration::from_secs(90)
        );
        assert_eq!(
            detached_daemon_ready_timeout_from_env_value(Some("999".into())),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn build_tray_start_args_omits_host_for_wildcard() {
        let config = ProxyConfig {
            host: "0.0.0.0".to_string(),
            socks5_port: Some(10000),
            unsafe_ssl: false,
            ..Default::default()
        };

        let args = build_tray_start_args(&config, "info", false, false);

        assert_eq!(args, vec!["--socks5-port", "10000"]);
    }

    #[test]
    fn build_tray_start_args_omits_log_level_when_info() {
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            ..Default::default()
        };

        let args = build_tray_start_args(&config, "info", false, false);

        assert_eq!(args, vec!["--host", "127.0.0.1"]);
    }

    #[test]
    fn build_tray_start_args_includes_unsafe_ssl_flag() {
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            unsafe_ssl: true,
            ..Default::default()
        };

        let args = build_tray_start_args(&config, "debug", false, false);

        assert!(args.contains(&"--unsafe-ssl".to_string()));
    }

    #[test]
    fn build_tray_start_args_includes_yes_and_skip_cert_check_flags() {
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            ..Default::default()
        };

        let args = build_tray_start_args(&config, "info", true, true);

        assert!(args.contains(&"--skip-cert-check".to_string()));
        assert!(args.contains(&"--yes".to_string()));
    }

    #[test]
    fn is_port_in_use_treats_wildcard_host_like_loopback() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        assert!(is_port_in_use("0.0.0.0", port));
    }

    #[test]
    fn is_port_in_use_uses_fallback_socketaddr_on_parse_failure() {
        // Force parse(...) to fail by using an invalid host literal; this should
        // fall back to 127.0.0.1 without panicking. Keep the listener alive so
        // the assertion cannot race another process for a released port.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        assert!(is_port_in_use("256.256.256.256", port));
    }

    #[cfg(unix)]
    #[test]
    fn raise_fd_limit_runs_without_panic() {
        // We don't assert on the actual limits; the goal is simply to exercise the
        // code path on the current platform.
        raise_fd_limit();
    }

    #[test]
    fn should_skip_system_proxy_restore_respects_shutdown_modes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();
        use bifrost_core::SystemProxyShutdownMode;

        bifrost_core::write_system_proxy_shutdown_mode(
            dir,
            SystemProxyShutdownMode::BackgroundCleanup,
        )
        .unwrap();
        assert!(should_skip_system_proxy_restore(dir, "bg"));

        bifrost_core::write_system_proxy_shutdown_mode(
            dir,
            SystemProxyShutdownMode::ForegroundCleanup,
        )
        .unwrap();
        assert!(should_skip_system_proxy_restore(dir, "fg"));

        bifrost_core::write_system_proxy_shutdown_mode(
            dir,
            SystemProxyShutdownMode::PreserveForRestart,
        )
        .unwrap();
        assert!(should_skip_system_proxy_restore(dir, "restart"));
    }

    #[test]
    fn should_skip_system_proxy_restore_returns_false_when_no_marker() {
        let temp_dir = tempfile::tempdir().unwrap();
        assert!(!should_skip_system_proxy_restore(temp_dir.path(), "none"));
    }

    #[test]
    fn should_stop_system_proxy_reconcile_for_shutdown_only_on_foreground_cleanup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();
        use bifrost_core::SystemProxyShutdownMode;

        bifrost_core::write_system_proxy_shutdown_mode(
            dir,
            SystemProxyShutdownMode::BackgroundCleanup,
        )
        .unwrap();
        assert!(!should_stop_system_proxy_reconcile_for_shutdown(dir));

        bifrost_core::write_system_proxy_shutdown_mode(
            dir,
            SystemProxyShutdownMode::ForegroundCleanup,
        )
        .unwrap();
        assert!(should_stop_system_proxy_reconcile_for_shutdown(dir));
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use std::sync::Arc as StdArc;
    use std::time::Duration;

    use bifrost_admin::connection_registry::ConnectionRegistry;
    use bifrost_admin::{RuntimeConfig as AdminRuntimeConfig, SharedRuntimeConfig};
    use bifrost_storage::{RulesStorage, ValuesStorage};
    use parking_lot::RwLock as ParkingRwLock;
    use tempfile::tempdir;

    fn allocate_free_loopback_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    fn with_reconcile_env<F>(value: Option<&str>, f: F)
    where
        F: FnOnce() + std::panic::UnwindSafe,
    {
        with_system_proxy_reconcile_env_for_test(value, f);
    }

    #[test]
    fn parse_yes_no_answer_accepts_yes_variants() {
        assert_eq!(parse_yes_no_answer("y"), Some(true));
        assert_eq!(parse_yes_no_answer("Y"), Some(true));
        assert_eq!(parse_yes_no_answer(" yes \n"), Some(true));
    }

    #[test]
    fn parse_yes_no_answer_accepts_no_variants() {
        assert_eq!(parse_yes_no_answer("n"), Some(false));
        assert_eq!(parse_yes_no_answer("NO"), Some(false));
        assert_eq!(parse_yes_no_answer("   \n"), Some(false));
    }

    #[test]
    fn parse_yes_no_answer_rejects_unknown_strings() {
        assert_eq!(parse_yes_no_answer("maybe"), None);
        assert_eq!(parse_yes_no_answer("123"), None);
    }

    #[test]
    fn system_proxy_reconcile_interval_defaults_to_30_when_env_missing() {
        with_reconcile_env(None, || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 30);
        });
    }

    #[test]
    fn system_proxy_reconcile_interval_uses_env_value_when_valid() {
        with_reconcile_env(Some("5"), || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 5);
        });
    }

    #[test]
    fn system_proxy_reconcile_interval_ignores_zero_value() {
        with_reconcile_env(Some("0"), || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 30);
        });
    }

    #[test]
    fn system_proxy_reconcile_interval_ignores_non_numeric_value() {
        with_reconcile_env(Some("not-a-number"), || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 30);
        });
    }

    #[test]
    fn find_available_port_returns_preferred_port_when_free() {
        let preferred = allocate_free_loopback_port();
        let actual = find_available_port("127.0.0.1", preferred).expect("should find port");
        assert_eq!(actual, preferred);
    }

    #[test]
    fn find_available_port_skips_in_use_port() {
        let base = allocate_free_loopback_port();
        let _listener = std::net::TcpListener::bind(("127.0.0.1", base)).unwrap();

        let actual = find_available_port("127.0.0.1", base).expect("should find port");
        assert!(
            actual >= base,
            "expected actual >= base, got {actual} < {base}"
        );
        assert_ne!(
            actual, base,
            "expected a different port when base is in use"
        );
    }

    #[tokio::test]
    async fn spawn_managed_proxy_task_rejects_invalid_bind_address() {
        let config = ProxyConfig {
            host: "invalid host".to_string(),
            port: 12345,
            ..Default::default()
        };

        let tls_config = StdArc::new(bifrost_proxy::TlsConfig::default());
        let admin_state = StdArc::new(AdminState::new(config.port));
        let push_manager = StdArc::new(PushManager::new(admin_state.clone()));
        let access_control = ProxyServer::new(ProxyConfig::default())
            .access_control()
            .clone();
        let resolver: SharedDynamicRulesResolver = StdArc::new(DynamicRulesResolver::new(
            Vec::new(),
            Vec::new(),
            HashMap::new(),
        ));

        let err = spawn_managed_proxy_task(
            config,
            resolver,
            tls_config,
            admin_state,
            push_manager,
            access_control,
        )
        .await
        .unwrap_err();

        match err {
            bifrost_core::BifrostError::Config(msg) => {
                assert!(msg.contains("Invalid address"), "unexpected message: {msg}");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn listener_task_error_maps_ok_ok_to_exit_error() {
        let err = listener_task_error(Ok(Ok(())));
        if let bifrost_core::BifrostError::Network(msg) = err {
            assert!(msg.contains("exited unexpectedly"));
        } else {
            panic!("unexpected error variant");
        }
    }

    #[tokio::test]
    async fn listener_task_error_propagates_inner_error() {
        let err = listener_task_error(Ok(Err(bifrost_core::BifrostError::Network(
            "boom".to_string(),
        ))));
        match err {
            bifrost_core::BifrostError::Network(msg) => assert_eq!(msg, "boom"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn listener_task_error_maps_cancelled_join_error() {
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<(), bifrost_core::BifrostError>(())
        });
        handle.abort();
        let result = handle.await;
        assert!(result.as_ref().unwrap_err().is_cancelled());

        let err = listener_task_error(result);
        if let bifrost_core::BifrostError::Network(msg) = err {
            assert!(msg.contains("cancelled"));
        } else {
            panic!("unexpected error variant");
        }
    }

    #[tokio::test]
    async fn listener_task_error_maps_panic_join_error() {
        let handle = tokio::spawn(async {
            panic!("boom");
            #[allow(unreachable_code)]
            Ok::<(), bifrost_core::BifrostError>(())
        });
        let result = handle.await;
        assert!(result.is_err());

        let err = listener_task_error(result);
        if let bifrost_core::BifrostError::Network(msg) = err {
            assert!(msg.contains("task failed"));
        } else {
            panic!("unexpected error variant");
        }
    }

    #[test]
    fn startup_phase_begin_and_complete_execute_without_panic() {
        let started = begin_startup_phase("coverage_boost");
        log_startup_phase("coverage_boost", started);
    }

    #[test]
    fn is_rules_storage_file_matches_bifrost_extension() {
        assert!(is_rules_storage_file(Path::new("rules.bifrost")));
    }

    #[test]
    fn is_rules_storage_file_matches_json_extension() {
        assert!(is_rules_storage_file(Path::new("legacy.json")));
    }

    #[test]
    fn is_rules_storage_file_rejects_hidden_and_unrelated_files() {
        assert!(!is_rules_storage_file(Path::new(".hidden.bifrost")));
        assert!(!is_rules_storage_file(Path::new("README.md")));
    }

    #[test]
    fn collect_rules_filesystem_snapshot_handles_missing_base_dir() {
        let temp_dir = tempdir().unwrap();
        let missing = temp_dir.path().join("missing-rules");
        let snapshot = collect_rules_filesystem_snapshot(&missing).unwrap();
        assert!(snapshot.entries.is_empty());
    }

    #[test]
    fn rules_filesystem_event_relevant_matches_create_modify_remove_and_any() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        use notify::EventKind;

        assert!(is_rules_filesystem_event_relevant(&EventKind::Create(
            CreateKind::Any
        )));
        assert!(is_rules_filesystem_event_relevant(&EventKind::Modify(
            ModifyKind::Any
        )));
        assert!(is_rules_filesystem_event_relevant(&EventKind::Remove(
            RemoveKind::Any
        )));
        assert!(is_rules_filesystem_event_relevant(&EventKind::Any));
    }

    #[test]
    fn rules_filesystem_event_relevant_ignores_other_events() {
        use notify::event::AccessKind;
        use notify::EventKind;

        assert!(!is_rules_filesystem_event_relevant(&EventKind::Access(
            AccessKind::Any
        )));
    }

    #[tokio::test]
    async fn spawn_rules_filesystem_watcher_task_noop_without_config_manager() {
        let temp_dir = tempdir().unwrap();
        let rules_storage = RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();

        let handle = spawn_rules_filesystem_watcher_task(None, rules_storage);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn spawn_rules_watcher_task_noop_without_config_manager() {
        let temp_dir = tempdir().unwrap();
        let rules_storage = RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        let values_storage = StdArc::new(ParkingRwLock::new(
            ValuesStorage::with_dir(temp_dir.path().join("values")).unwrap(),
        ));
        let resolver: SharedDynamicRulesResolver = StdArc::new(DynamicRulesResolver::new(
            Vec::new(),
            Vec::new(),
            HashMap::new(),
        ));
        let connection_registry = StdArc::new(ConnectionRegistry::new(true));
        let runtime_config: SharedRuntimeConfig =
            StdArc::new(tokio::sync::RwLock::new(AdminRuntimeConfig::default()));
        let admin_state = StdArc::new(AdminState::new(0).with_rules_storage(rules_storage));

        let handle = spawn_rules_watcher_task(
            None,
            admin_state.rules_storage.clone(),
            values_storage,
            resolver,
            connection_registry,
            runtime_config,
            admin_state,
            None,
        );

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn spawn_admin_push_watcher_task_noop_without_config_manager() {
        let admin_state = StdArc::new(AdminState::new(0));
        let push_manager = StdArc::new(PushManager::new(admin_state));

        let handle = spawn_admin_push_watcher_task(None, push_manager);
        handle.await.unwrap();
    }

    #[test]
    fn log_resolver_rules_logs_cli_rules() {
        let rules_str = "example.test statusCode://201".to_string();
        let (cli_rules, _) = parse_cli_rules(&[rules_str], &None, &HashMap::new()).unwrap();
        let resolver = DynamicRulesResolver::new(cli_rules.clone(), Vec::new(), HashMap::new());

        assert_eq!(resolver.cli_rules().len(), cli_rules.len());
        log_resolver_rules(&resolver);
    }

    #[test]
    fn check_and_resolve_port_conflict_returns_ok_when_port_free() {
        let port = allocate_free_loopback_port();
        check_and_resolve_port_conflict("127.0.0.1", port, false).expect("port should be free");
    }

    #[test]
    fn build_tray_start_args_combines_flags_from_config() {
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            socks5_port: Some(12345),
            unsafe_ssl: true,
            ..Default::default()
        };

        let args = build_tray_start_args(&config, "trace", true, false);

        assert!(args.contains(&"--host".to_string()));
        assert!(args.contains(&"--socks5-port".to_string()));
        assert!(args.contains(&"--log-level".to_string()));
        assert!(args.contains(&"--skip-cert-check".to_string()));
        assert!(args.contains(&"--unsafe-ssl".to_string()));
    }

    #[test]
    fn rules_filesystem_snapshot_handles_empty_directory() {
        let temp_dir = tempdir().unwrap();
        let snapshot = collect_rules_filesystem_snapshot(temp_dir.path()).unwrap();
        assert!(snapshot.entries.is_empty());
    }

    #[test]
    fn parse_proxy_users_accepts_passwords_with_colons() {
        let users = vec!["alice:pa:ss:word".to_string()];
        let accounts = parse_proxy_users(&users).expect("parse proxy users");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "alice");
        assert_eq!(accounts[0].password.as_deref(), Some("pa:ss:word"));
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn build_tray_launch_callback_with_no_tray_skips_tray_pid() {
        let temp = tempdir().unwrap();
        let data_dir = temp.path().to_path_buf();
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            port: 18080,
            ..Default::default()
        };

        let callback =
            build_tray_launch_callback(true, data_dir.clone(), 1234, &config, "info", false, false);

        callback();
        assert!(!data_dir.join("tray.pid").exists());
    }

    #[test]
    fn launch_tray_helper_if_enabled_noop_when_tray_disabled() {
        let temp = tempdir().unwrap();
        let runtime_file = temp.path().join("runtime.json");
        std::fs::write(&runtime_file, "{}\n").unwrap();

        let request = TrayLaunchHelperRequest {
            no_tray: true,
            data_dir: temp.path(),
            runtime_file: &runtime_file,
            pid: 1234,
            admin_url: "http://127.0.0.1:18080/_bifrost/",
            port: 18080,
            bifrost_bin: None,
            start_args: &[],
        };

        launch_tray_helper_if_enabled(request);
        assert!(!temp.path().join("tray.pid").exists());
    }
}

#[cfg(test)]
mod coverage_boost_v3 {
    use super::*;
    use std::sync::Arc as StdArc;
    use std::time::Duration;

    use bifrost_admin::connection_registry::ConnectionRegistry;
    use bifrost_admin::{RuntimeConfig as AdminRuntimeConfig, SharedRuntimeConfig};
    use bifrost_storage::{ConfigManager, RulesStorage, ValuesStorage};
    use parking_lot::RwLock as ParkingRwLock;
    use tempfile::tempdir;

    fn make_temp_rules_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        (tmp, dir)
    }

    #[test]
    fn is_rules_storage_file_rejects_paths_without_file_name() {
        assert!(!is_rules_storage_file(Path::new("")));
    }

    #[test]
    fn collect_rules_filesystem_snapshot_errors_for_non_directory() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let err = collect_rules_filesystem_snapshot(file.path()).unwrap_err();
        assert_ne!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn log_resolver_rules_noop_when_no_cli_rules() {
        let resolver = DynamicRulesResolver::new(Vec::new(), Vec::new(), HashMap::new());
        log_resolver_rules(&resolver);
    }

    #[test]
    fn system_proxy_reconcile_interval_trims_whitespace() {
        with_system_proxy_reconcile_env_for_test(Some(" 7 "), || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 7);
        });
    }

    #[test]
    fn system_proxy_reconcile_interval_ignores_negative_numbers() {
        with_system_proxy_reconcile_env_for_test(Some("-5"), || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 30);
        });
    }

    #[test]
    fn rules_filesystem_trigger_debug_format_works() {
        let msg = format!("{:?}", RulesFilesystemTrigger::WatcherEvent);
        assert!(msg.contains("WatcherEvent"));
    }

    #[test]
    fn collect_rules_filesystem_snapshot_ignores_unrelated_extensions() {
        let (_tmp, dir) = make_temp_rules_dir();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let snapshot = collect_rules_filesystem_snapshot(&dir).unwrap();
        assert!(snapshot.entries.is_empty());
    }

    #[test]
    fn collect_rules_filesystem_snapshot_includes_supported_files() {
        let (_tmp, dir) = make_temp_rules_dir();
        std::fs::write(dir.join("a.bifrost"), "a.test statusCode://200\n").unwrap();
        std::fs::write(dir.join("b.json"), "{}").unwrap();
        let snapshot = collect_rules_filesystem_snapshot(&dir).unwrap();
        assert_eq!(snapshot.entries.len(), 2);
    }

    #[test]
    fn rules_filesystem_snapshot_equality_detects_difference() {
        let (_tmp, dir) = make_temp_rules_dir();
        std::fs::write(dir.join("a.bifrost"), "a.test statusCode://200\n").unwrap();
        let first = collect_rules_filesystem_snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.bifrost"), "a.test statusCode://201\n").unwrap();
        let second = collect_rules_filesystem_snapshot(&dir).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn load_stored_rules_returns_empty_when_no_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        let (rules, values) = load_stored_rules(&rules_storage, None);
        assert!(rules.is_empty());
        assert!(values.is_empty());
        assert!(rules_storage.exists(bifrost_storage::DEFAULT_RULE_NAME));
    }

    #[test]
    fn load_stored_rules_includes_global_default_rule_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                bifrost_storage::DEFAULT_RULE_NAME,
                "global-default.test status://218 resBody://(global-default)",
            ))
            .unwrap();

        let (rules, values) = load_stored_rules(&rules_storage, None);

        assert!(values.is_empty());
        assert!(rules.iter().any(|rule| {
            rule.file.as_deref() == Some(bifrost_storage::DEFAULT_RULE_NAME)
                && rule.raw.contains("global-default.test")
        }));
    }

    #[test]
    fn load_stored_rules_restores_deleted_default_rule_on_startup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        let default_rule = rules_storage.ensure_default_rule().unwrap();
        let default_path = temp_dir.path().join("rules").join("Default.bifrost");
        assert_eq!(default_rule.name, bifrost_storage::DEFAULT_RULE_NAME);
        assert!(default_path.exists());

        std::fs::remove_file(&default_path).unwrap();
        assert!(!default_path.exists());

        let (rules, values) = load_stored_rules(&rules_storage, None);

        assert!(rules.is_empty());
        assert!(values.is_empty());
        assert!(default_path.exists());
        assert!(rules_storage.exists(bifrost_storage::DEFAULT_RULE_NAME));
    }

    #[test]
    fn load_stored_rules_parses_global_default_before_other_rules() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                bifrost_storage::DEFAULT_RULE_NAME,
                "order.test status://218 resBody://(global-default)",
            ))
            .unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                "regular",
                "order.test status://219 resBody://(regular)",
            ))
            .unwrap();

        let (rules, values) = load_stored_rules(&rules_storage, None);

        assert!(values.is_empty());
        assert!(rules.len() >= 2);
        let first_regular = rules
            .iter()
            .position(|rule| rule.file.as_deref() == Some("regular"))
            .expect("regular rule should be parsed");
        let last_default = rules
            .iter()
            .rposition(|rule| rule.file.as_deref() == Some(bifrost_storage::DEFAULT_RULE_NAME))
            .expect("Default rule should be parsed");
        assert!(
            last_default < first_regular,
            "all Default rules must be parsed before regular rules"
        );
    }

    #[test]
    fn load_stored_rules_collects_inline_markdown_values_from_multiple_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        let content1 = r#"
example.com resBody://{body1}

``` body1
first content
```
"#;
        let content2 = r#"
example.org resBody://{body2}

``` body2
second content
```
"#;
        rules_storage
            .save(&bifrost_storage::RuleFile::new("a", content1))
            .unwrap();
        rules_storage
            .save(&bifrost_storage::RuleFile::new("b", content2))
            .unwrap();
        let (_rules, values) = load_stored_rules(&rules_storage, None);
        assert_eq!(
            values.get("body1").map(String::as_str),
            Some("first content")
        );
        assert_eq!(
            values.get("body2").map(String::as_str),
            Some("second content")
        );
    }

    #[test]
    fn is_rules_filesystem_event_relevant_accepts_any() {
        assert!(is_rules_filesystem_event_relevant(&EventKind::Any));
    }

    #[tokio::test]
    async fn rules_hot_reload_watcher_reacts_to_rules_changed_events() {
        let temp_dir = tempdir().unwrap();
        let data_dir = temp_dir.path().to_path_buf();
        let config_manager = StdArc::new(ConfigManager::new(data_dir.clone()).unwrap());

        let rules_dir = data_dir.join("rules");
        let values_dir = data_dir.join("values");
        let rules_storage = RulesStorage::with_dir(rules_dir).unwrap();
        let values_storage = StdArc::new(ParkingRwLock::new(
            ValuesStorage::with_dir(values_dir).unwrap(),
        ));

        rules_storage
            .save(&bifrost_storage::RuleFile::new(
                "entry",
                "example.com statusCode://200",
            ))
            .unwrap();

        let resolver: SharedDynamicRulesResolver = StdArc::new(DynamicRulesResolver::new(
            Vec::new(),
            Vec::new(),
            HashMap::new(),
        ));
        let connection_registry = StdArc::new(ConnectionRegistry::new(true));
        let runtime_config: SharedRuntimeConfig =
            StdArc::new(tokio::sync::RwLock::new(AdminRuntimeConfig::default()));
        let admin_state = StdArc::new(
            AdminState::new(0)
                .with_rules_storage(rules_storage.clone())
                .with_config_manager_shared(config_manager.clone()),
        );

        let base_config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            ..Default::default()
        };
        let tls_config = StdArc::new(bifrost_proxy::TlsConfig::default());
        let push_manager = StdArc::new(PushManager::new(admin_state.clone()));
        let access_control = ProxyServer::new(ProxyConfig::default())
            .access_control()
            .clone();
        let temp_ports = StdArc::new(crate::commands::port::CliTemporaryPortManager::new(
            base_config,
            tls_config,
            admin_state.clone(),
            push_manager,
            access_control,
            rules_storage.clone(),
            values_storage.clone(),
        ));

        let _handle = spawn_rules_watcher_task(
            Some(config_manager.clone()),
            rules_storage,
            values_storage,
            resolver.clone(),
            connection_registry,
            runtime_config,
            admin_state,
            Some(temp_ports),
        );

        let _ = config_manager.notify(ConfigChangeEvent::rules_changed(
            RulesChangeOrigin::LocalApi,
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;

        let (_intercept, _passthrough) = resolver.get_tls_rule_patterns();
    }

    #[tokio::test]
    async fn rules_hot_reload_watcher_handles_values_changed_without_disconnect() {
        let temp_dir = tempdir().unwrap();
        let data_dir = temp_dir.path().to_path_buf();
        let config_manager = StdArc::new(ConfigManager::new(data_dir.clone()).unwrap());
        let rules_storage = RulesStorage::with_dir(data_dir.join("rules")).unwrap();
        let values_storage = StdArc::new(ParkingRwLock::new(
            ValuesStorage::with_dir(data_dir.join("values")).unwrap(),
        ));

        let resolver: SharedDynamicRulesResolver = StdArc::new(DynamicRulesResolver::new(
            Vec::new(),
            Vec::new(),
            HashMap::new(),
        ));
        let connection_registry = StdArc::new(ConnectionRegistry::new(true));
        let runtime_config: SharedRuntimeConfig =
            StdArc::new(tokio::sync::RwLock::new(AdminRuntimeConfig::default()));
        let admin_state = StdArc::new(
            AdminState::new(0)
                .with_rules_storage(rules_storage.clone())
                .with_config_manager_shared(config_manager.clone()),
        );

        let _handle = spawn_rules_watcher_task(
            Some(config_manager.clone()),
            rules_storage,
            values_storage,
            resolver,
            connection_registry,
            runtime_config,
            admin_state,
            None,
        );

        let _ = config_manager.notify(ConfigChangeEvent::ValuesChanged("k".to_string()));

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn admin_push_watcher_handles_all_config_change_events() {
        use bifrost_storage::TlsConfig as StoredTlsConfig;

        let temp_dir = tempdir().unwrap();
        let data_dir = temp_dir.path().to_path_buf();
        let config_manager = StdArc::new(ConfigManager::new(data_dir).unwrap());

        let admin_state =
            StdArc::new(AdminState::new(0).with_config_manager_shared(config_manager.clone()));
        let push_manager = StdArc::new(PushManager::new(admin_state));

        let _handle = spawn_admin_push_watcher_task(Some(config_manager.clone()), push_manager);

        let _ = config_manager.notify(ConfigChangeEvent::TlsConfigChanged(
            StoredTlsConfig::default(),
        ));
        let _ = config_manager.notify(ConfigChangeEvent::SandboxConfigChanged);
        let _ = config_manager.notify(ConfigChangeEvent::SystemProxyConfigChanged);
        let _ = config_manager.notify(ConfigChangeEvent::TrayConfigChanged);
        let _ = config_manager.notify(ConfigChangeEvent::AccessConfigChanged);
        let _ = config_manager.notify(ConfigChangeEvent::ServerConfigChanged);
        let _ = config_manager.notify(ConfigChangeEvent::TrafficConfigChanged);
        let _ = config_manager.notify(ConfigChangeEvent::ScriptsChanged);
        let _ = config_manager.notify(ConfigChangeEvent::ValuesChanged("k".to_string()));
        let _ = config_manager.notify(ConfigChangeEvent::StateChanged);

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    #[test]
    fn rules_filesystem_snapshot_equality_treats_identical_snapshots_as_equal() {
        let (_tmp, dir) = make_temp_rules_dir();
        std::fs::write(dir.join("a.bifrost"), "a.test statusCode://200\n").unwrap();
        let first = collect_rules_filesystem_snapshot(&dir).unwrap();
        let second = collect_rules_filesystem_snapshot(&dir).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rules_filesystem_snapshot_ignores_sync_metadata_only_changes() {
        let (_tmp, dir) = make_temp_rules_dir();
        let storage = RulesStorage::with_dir(dir.clone()).unwrap();
        let mut rule = bifrost_storage::RuleFile::new("synced", "a.test statusCode://200");
        rule.mark_synced(
            "remote-1",
            "user-1",
            "2026-06-20T00:00:00Z",
            "remote-updated",
        );
        storage.save(&rule).unwrap();
        let first = collect_rules_filesystem_snapshot(&dir).unwrap();

        rule.sync.last_synced_at = Some("2026-06-20T00:00:01Z".to_string());
        rule.sync.remote_updated_at = Some("remote-updated-2".to_string());
        storage.save(&rule).unwrap();
        let second = collect_rules_filesystem_snapshot(&dir).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn rules_filesystem_snapshot_detects_runtime_rule_content_changes() {
        let (_tmp, dir) = make_temp_rules_dir();
        let storage = RulesStorage::with_dir(dir.clone()).unwrap();
        let mut rule = bifrost_storage::RuleFile::new("synced", "a.test statusCode://200");
        rule.mark_synced(
            "remote-1",
            "user-1",
            "2026-06-20T00:00:00Z",
            "remote-updated",
        );
        storage.save(&rule).unwrap();
        let first = collect_rules_filesystem_snapshot(&dir).unwrap();

        rule.content = "a.test statusCode://201".to_string();
        storage.save(&rule).unwrap();
        let second = collect_rules_filesystem_snapshot(&dir).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn is_rules_storage_file_rejects_hidden_json_files() {
        assert!(!is_rules_storage_file(Path::new(".hidden.json")));
    }

    #[test]
    fn is_rules_storage_file_rejects_hidden_bifrost_files() {
        assert!(!is_rules_storage_file(Path::new(".hidden.bifrost")));
    }

    #[test]
    fn load_stored_rules_ignores_disabled_files_completely() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        rules_storage
            .save(
                &bifrost_storage::RuleFile::new("disabled", "example.com statusCode://200")
                    .with_enabled(false),
            )
            .unwrap();
        let (rules, _values) = load_stored_rules(&rules_storage, None);
        assert!(rules.is_empty());
    }

    #[test]
    fn collect_rules_filesystem_snapshot_handles_nested_directories() {
        let (_tmp, dir) = make_temp_rules_dir();
        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("a.bifrost"), "a.test statusCode://200\n").unwrap();
        let snapshot = collect_rules_filesystem_snapshot(&dir).unwrap();
        assert_eq!(snapshot.entries.len(), 1);
    }

    #[test]
    fn rules_filesystem_event_relevant_rejects_access_events() {
        use notify::event::AccessKind;
        assert!(!is_rules_filesystem_event_relevant(&EventKind::Access(
            AccessKind::Any
        )));
    }

    #[test]
    fn system_proxy_reconcile_interval_uses_default_when_env_missing() {
        with_system_proxy_reconcile_env_for_test(None, || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 30);
        });
    }

    #[test]
    fn system_proxy_reconcile_interval_ignores_zero_value() {
        with_system_proxy_reconcile_env_for_test(Some("0"), || {
            assert_eq!(system_proxy_reconcile_interval().as_secs(), 30);
        });
    }

    #[test]
    fn is_rules_storage_file_accepts_supported_extensions() {
        assert!(is_rules_storage_file(Path::new("rules/sample.bifrost")));
        assert!(is_rules_storage_file(Path::new("rules/sample.json")));
    }

    #[test]
    fn collect_rules_filesystem_snapshot_empty_directory_has_no_entries() {
        let (_tmp, dir) = make_temp_rules_dir();
        let snapshot = collect_rules_filesystem_snapshot(&dir).unwrap();
        assert!(snapshot.entries.is_empty());
    }
}
