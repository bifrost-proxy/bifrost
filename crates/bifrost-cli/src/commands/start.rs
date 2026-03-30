use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
use bifrost_core::Rule;
use bifrost_proxy::{AccessMode, ProxyConfig, ProxyServer};
use bifrost_storage::{set_data_dir, ConfigChangeEvent, ConfigManager};
use bifrost_sync::SyncManager;
use bifrost_tls::{get_platform_name, CertInstaller, CertStatus};
use parking_lot::RwLock as ParkingRwLock;
use tracing::info;

use crate::commands::ca::{check_and_install_certificate, load_tls_config};
use crate::config::get_bifrost_dir;
use crate::help::print_startup_help;
use crate::parsing::{parse_cli_rules, DynamicRulesResolver, SharedDynamicRulesResolver};
use crate::process::{is_process_running, read_pid, remove_pid, write_runtime_info, RuntimeInfo};

const ASYNC_TRAFFIC_BUFFER_SIZE: usize = 10000;
const MAX_PORT_INCREMENT_ATTEMPTS: u16 = 64;
const PORT_REBIND_OLD_LISTENER_GRACE_PERIOD: Duration = Duration::from_millis(250);

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
    should_enable: bool,
    proxy_host: String,
    proxy_port: u16,
    system_proxy_bypass: String,
    enabled_flag: Arc<AtomicBool>,
    daemon_mode: bool,
}

fn spawn_system_proxy_reconcile_task(config: SystemProxyReconcileConfig) {
    let SystemProxyReconcileConfig {
        bifrost_dir,
        system_proxy_manager,
        should_enable,
        proxy_host,
        proxy_port,
        system_proxy_bypass,
        enabled_flag,
        daemon_mode,
    } = config;

    let _ = std::thread::Builder::new()
        .name("bifrost-system-proxy-reconcile".to_string())
        .spawn(move || {
            let started_at = Instant::now();

            if let Err(error) = bifrost_core::SystemProxyManager::recover_from_crash(&bifrost_dir) {
                tracing::warn!(
                    error = %error,
                    "[SYSTEM_PROXY] Failed to recover system proxy from previous crash"
                );
            }

            if !should_enable {
                tracing::info!(
                    target: "bifrost_cli::startup",
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    "system proxy reconcile completed without apply"
                );
                return;
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
                    enabled_flag.store(true, Ordering::Release);
                    tracing::info!(
                        target: "bifrost_cli::startup",
                        elapsed_ms = started_at.elapsed().as_millis() as u64,
                        host = %proxy_host,
                        port = proxy_port,
                        "system proxy applied asynchronously"
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

async fn spawn_managed_proxy_task(
    config: ProxyConfig,
    rules: SharedDynamicRulesResolver,
    tls_config: Arc<bifrost_proxy::TlsConfig>,
    admin_state: Arc<AdminState>,
    push_manager: Arc<PushManager>,
    access_control: bifrost_admin::SharedAccessControl,
) -> bifrost_core::Result<tokio::task::JoinHandle<()>> {
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

    Ok(tokio::spawn(async move {
        if let Err(error) = server.run_with_listener(listener).await {
            tracing::error!("Managed proxy listener exited with error: {}", error);
        }
    }))
}

fn abort_listener_after_grace_period(handle: tokio::task::JoinHandle<()>) {
    tokio::spawn(async move {
        tokio::time::sleep(PORT_REBIND_OLD_LISTENER_GRACE_PERIOD).await;
        handle.abort();
    });
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sighup = signal(SignalKind::hangup()).expect("failed to install SIGHUP handler");

        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
            _ = sighup.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct SystemProxyRestoreGuard {
    system_proxy_manager: Arc<tokio::sync::RwLock<bifrost_core::SystemProxyManager>>,
}

impl SystemProxyRestoreGuard {
    fn new(
        system_proxy_manager: Arc<tokio::sync::RwLock<bifrost_core::SystemProxyManager>>,
    ) -> Self {
        Self {
            system_proxy_manager,
        }
    }
}

impl Drop for SystemProxyRestoreGuard {
    fn drop(&mut self) {
        if let Err(e) = self.system_proxy_manager.blocking_write().restore() {
            eprintln!("Failed to restore system proxy: {}", e);
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
    intercept: bool,
    no_intercept: bool,
    intercept_exclude: Option<String>,
    intercept_include: Option<String>,
    app_intercept_exclude: Option<String>,
    app_intercept_include: Option<String>,
    unsafe_ssl: bool,
    no_disconnect_on_config_change: bool,
    rules: Vec<String>,
    rules_file: Option<PathBuf>,
    system_proxy: bool,
    proxy_bypass: Option<String>,
    cli_proxy: bool,
    cli_proxy_no_proxy: Option<String>,
) -> bifrost_core::Result<()> {
    if let Some(pid) = read_pid() {
        if is_process_running(pid) {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Bifrost proxy is already running (PID: {})",
                pid
            )));
        }
        remove_pid()?;
    }

    if !daemon && !skip_cert_check {
        check_and_install_certificate()?;
    }

    #[cfg(not(unix))]
    let _ = (&log_dir, log_retention_days);

    let bifrost_dir = get_bifrost_dir()?;
    set_data_dir(bifrost_dir.clone());

    let config_manager = ConfigManager::new(bifrost_dir.clone())?;
    let stored_config = futures::executor::block_on(config_manager.config());

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

    let verbose_logging = matches!(log_level, "debug" | "trace");
    let proxy_config = ProxyConfig {
        port,
        host: host.clone(),
        socks5_port,
        access_mode: parsed_access_mode,
        client_whitelist,
        allow_lan: allow_lan_final,
        enable_tls_interception,
        intercept_exclude: exclude_list.clone(),
        intercept_include: include_list.clone(),
        app_intercept_exclude: app_exclude_list.clone(),
        app_intercept_include: app_include_list.clone(),
        unsafe_ssl: unsafe_ssl_final,
        verbose_logging,
        max_body_buffer_size: stored_config.traffic.max_body_buffer_size,
        max_body_probe_size: stored_config.traffic.max_body_probe_size,
        binary_traffic_performance_mode: stored_config.traffic.binary_traffic_performance_mode,
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

    let disconnect_on_config_change = if no_disconnect_on_config_change {
        false
    } else {
        stored_config.tls.disconnect_on_change
    };

    if daemon {
        #[cfg(unix)]
        {
            run_daemon(
                proxy_config,
                parsed_rules,
                all_values.clone(),
                enable_system_proxy,
                system_proxy_bypass.clone(),
                cli_proxy,
                cli_proxy_no_proxy.clone(),
                config_manager,
                log_dir.clone(),
                log_retention_days,
            )?;
        }
        #[cfg(not(unix))]
        {
            return Err(bifrost_core::BifrostError::Config(
                "Daemon mode is not supported on this platform".to_string(),
            ));
        }
    } else {
        run_foreground(
            proxy_config,
            parsed_rules,
            all_values,
            enable_system_proxy,
            system_proxy_bypass,
            cli_proxy,
            cli_proxy_no_proxy,
            disconnect_on_config_change,
            config_manager,
        )?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_foreground(
    config: ProxyConfig,
    cli_rules: Vec<Rule>,
    cli_values: HashMap<String, String>,
    enable_system_proxy: bool,
    system_proxy_bypass: String,
    enable_cli_proxy: bool,
    cli_proxy_no_proxy: Option<String>,
    disconnect_on_config_change: bool,
    config_manager: ConfigManager,
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
    let _system_proxy_restore_guard = SystemProxyRestoreGuard::new(system_proxy_manager.clone());
    let mut shell_proxy_manager = bifrost_core::ShellProxyManager::new(bifrost_dir.clone());
    if let Err(e) = bifrost_core::ShellProxyManager::recover_from_crash(&bifrost_dir) {
        tracing::warn!("Failed to recover CLI proxy from previous crash: {}", e);
    }

    let system_proxy_enabled = Arc::new(AtomicBool::new(false));

    let mut cli_proxy_enabled = false;
    let cli_proxy_no_proxy =
        cli_proxy_no_proxy.unwrap_or_else(|| "localhost,127.0.0.1,::1,*.local".to_string());
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

    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to create runtime: {}", e))
    })?;

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

            let phase_started_at = Instant::now();
            let stored_config = config_manager.config().await;
            log_startup_phase("config_manager.config", phase_started_at);

            let phase_started_at = Instant::now();
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

            let phase_started_at = Instant::now();
            let ws_payload_store = Arc::new(WsPayloadStore::new(
                bifrost_dir.clone(),
                stored_config.traffic.ws_payload_flush_bytes,
                Duration::from_millis(stored_config.traffic.ws_payload_flush_interval_ms),
                stored_config.traffic.ws_payload_max_open_files,
                stored_config.traffic.file_retention_days,
            ));
            let ws_payload_cleanup_task = start_ws_payload_cleanup_task(ws_payload_store.clone());
            log_startup_phase("ws_payload_store.init", phase_started_at);

            let phase_started_at = Instant::now();
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

            let phase_started_at = Instant::now();
            let (async_traffic_writer, async_traffic_rx) =
                AsyncTrafficWriter::new(ASYNC_TRAFFIC_BUFFER_SIZE);
            let async_traffic_writer = Arc::new(async_traffic_writer);
            let _async_traffic_task =
                start_async_traffic_processor(async_traffic_rx, traffic_db_store.clone());
            log_startup_phase("async_traffic.init", phase_started_at);

            let phase_started_at = Instant::now();
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

            let phase_started_at = Instant::now();
            let values_storage = config_manager.values_storage().await;
            let rules_storage = config_manager.rules_storage().await;
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
                unsafe_ssl: config.unsafe_ssl,
                disconnect_on_config_change,
            };
            let connection_registry =
                bifrost_admin::ConnectionRegistry::new(disconnect_on_config_change);

            let phase_started_at = Instant::now();
            let app_icon_cache = bifrost_admin::create_app_icon_cache(&bifrost_dir);
            log_startup_phase("app_icon_cache.init", phase_started_at);

            let phase_started_at = Instant::now();
            let scripts_dir = bifrost_dir.join("scripts");
            let script_manager = bifrost_admin::ScriptManager::new(scripts_dir);
            if let Err(e) = script_manager.init().await {
                tracing::warn!("Failed to initialize script manager: {}", e);
            }
            log_startup_phase("script_manager.init", phase_started_at);

            let phase_started_at = Instant::now();
            let replay_db_store = match ReplayDbStore::new(bifrost_dir.join("replay")) {
                Ok(store) => Some(Arc::new(store)),
                Err(e) => {
                    tracing::warn!("Failed to initialize replay store: {}", e);
                    None
                }
            };
            log_startup_phase("replay_db_store.init", phase_started_at);

            let access_control = ProxyServer::new(config.clone()).access_control().clone();
            let (port_rebind_manager, mut port_rebind_rx) = PortRebindManager::channel(8);

            let shared_config_manager = Arc::new(config_manager);

            let phase_started_at = Instant::now();
            let admin_state = AdminState::new(config.port)
                .with_access_control(access_control.clone())
                .with_body_store(body_store)
                .with_ws_payload_store(ws_payload_store)
                .with_async_traffic_writer_shared(async_traffic_writer)
                .with_traffic_db_store_shared(traffic_db_store.clone())
                .with_frame_store_shared(frame_store)
                .with_runtime_config(runtime_config)
                .with_connection_registry(connection_registry)
                .with_values_storage(values_storage)
                .with_rules_storage(rules_storage)
                .with_ca_cert_path(ca_cert_path)
                .with_system_proxy_manager_shared(system_proxy_manager.clone())
                .with_config_manager_shared(shared_config_manager.clone())
                .with_max_body_buffer_size(stored_config.traffic.max_body_buffer_size)
                .with_max_body_probe_size(stored_config.traffic.max_body_probe_size)
                .with_binary_traffic_performance_mode(
                    stored_config.traffic.binary_traffic_performance_mode,
                )
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
            let admin_state = admin_state.with_sync_manager_shared(sync_manager);

            let db_cleanup_task = bifrost_admin::start_db_cleanup_task(traffic_db_store);
            let connection_cleanup_task =
                bifrost_admin::start_connection_cleanup_task(admin_state.connection_monitor.clone());

            let metrics_collector = admin_state.metrics_collector.clone();
            let rules_storage_for_resolver = admin_state.rules_storage.clone();
            let config_manager_for_resolver = admin_state.config_manager.clone();
            let values_storage_for_resolver = admin_state
                .values_storage
                .clone()
                .expect("values_storage should be set");
            let connection_registry_for_resolver = admin_state.connection_registry.clone();
            let runtime_config_for_resolver = admin_state.runtime_config.clone();

            let phase_started_at = Instant::now();
            let (stored_rules, inline_values) = load_stored_rules(&rules_storage_for_resolver);
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
            let admin_state_arc = Arc::new(admin_state);

            let phase_started_at = Instant::now();
            let replay_executor = Arc::new(bifrost_admin::ReplayExecutor::new(
                admin_state_arc.clone(),
                unsafe_ssl,
            ));
            admin_state_arc.set_replay_executor(replay_executor);
            log_startup_phase("replay_executor.init", phase_started_at);

            let phase_started_at = Instant::now();
            let push_manager = Arc::new(PushManager::new(admin_state_arc.clone()));
            let push_tasks = start_push_tasks(push_manager.clone());
            let admin_push_watcher_task = spawn_admin_push_watcher_task(
                config_manager_for_resolver.clone(),
                push_manager.clone(),
            );
            log_startup_phase("push_manager.init", phase_started_at);

            let phase_started_at = Instant::now();
            let metrics_tasks = start_metrics_collector_task(metrics_collector, 1);
            log_startup_phase("metrics_task.start", phase_started_at);

            let phase_started_at = Instant::now();
            let rules_watcher_task = spawn_rules_watcher_task(
                config_manager_for_resolver,
                rules_storage_for_resolver,
                values_storage_for_resolver,
                resolver.clone(),
                connection_registry_for_resolver,
                runtime_config_for_resolver,
            );
            log_startup_phase("rules_watcher.start", phase_started_at);

            let mut current_port = config.port;
            let base_config = config.clone();
            let phase_started_at = Instant::now();
            let mut listener_task = spawn_managed_proxy_task(
                config.clone(),
                resolver.clone(),
                tls_config.clone(),
                admin_state_arc.clone(),
                push_manager.clone(),
                access_control.clone(),
            )
            .await?;
            let runtime_info = RuntimeInfo {
                pid,
                port: config.port,
                socks5_port: config.socks5_port,
                host: Some(config.host.clone()),
            };
            write_runtime_info(&runtime_info)?;
            log_startup_phase("proxy_listener.bind", phase_started_at);
            tracing::info!(
                target: "bifrost_cli::startup",
                total_elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
                "foreground runtime initialization completed"
            );

            let system_proxy_host = if base_config.host == "0.0.0.0" {
                "127.0.0.1".to_string()
            } else {
                base_config.host.clone()
            };
            spawn_system_proxy_reconcile_task(SystemProxyReconcileConfig {
                bifrost_dir: bifrost_dir.clone(),
                system_proxy_manager: system_proxy_manager.clone(),
                should_enable: enable_system_proxy,
                proxy_host: system_proxy_host,
                proxy_port: current_port,
                system_proxy_bypass: system_proxy_bypass.clone(),
                enabled_flag: system_proxy_enabled.clone(),
                daemon_mode: false,
            });

            tokio::select! {
                _ = wait_for_shutdown_signal() => {
                    info!("Received shutdown signal");
                    println!("\nShutting down...");
                },
                _ = async {
                    #[cfg(unix)]
                    {
                        let Ok(mut sigterm) = tokio::signal::unix::signal(
                            tokio::signal::unix::SignalKind::terminate(),
                        ) else {
                            std::future::pending::<()>().await;
                            return;
                        };
                        sigterm.recv().await;
                    }
                    #[cfg(not(unix))]
                    {
                        std::future::pending::<()>().await;
                    }
                } => {
                    info!("Received SIGTERM");
                    println!("\nShutting down...");
                },
                _ = async {
                    while let Some(PortRebindRequest { expected_port, response_tx }) = port_rebind_rx.recv().await {
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

                        let runtime_info = RuntimeInfo {
                            pid: std::process::id(),
                            port: actual_port,
                            socks5_port: base_config.socks5_port,
                            host: Some(base_config.host.clone()),
                        };
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
                } => {}
            }

            listener_task.abort();
            rules_watcher_task.abort();
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
    remove_pid()?;
    println!("Bifrost proxy stopped.");

    runtime_result?;

    Ok(())
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

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
pub fn run_daemon(
    config: ProxyConfig,
    cli_rules: Vec<Rule>,
    cli_values: HashMap<String, String>,
    enable_system_proxy: bool,
    system_proxy_bypass: String,
    enable_cli_proxy: bool,
    cli_proxy_no_proxy: Option<String>,
    config_manager: ConfigManager,
    log_dir: PathBuf,
    log_retention_days: u32,
) -> bifrost_core::Result<()> {
    use nix::unistd::{chdir, dup2, fork, setsid, ForkResult};
    use std::os::unix::io::AsRawFd;

    use crate::process::get_pid_file;

    let bifrost_dir = config_manager.data_dir().to_path_buf();
    std::fs::create_dir_all(&bifrost_dir)?;

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
            println!("Daemon started with PID: {}", child);
            Ok(())
        }
        Ok(ForkResult::Child) => {
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

            if let Err(e) = bifrost_core::reinit_logging_for_daemon(&log_dir, log_retention_days) {
                eprintln!("Warning: Failed to initialize logging for daemon: {}", e);
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
                return Err(bifrost_core::BifrostError::Network(format!(
                    "Failed to bind to {}: another process is already listening on this port",
                    bind_addr
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

            let mut cli_proxy_enabled = false;
            let cli_proxy_no_proxy =
                cli_proxy_no_proxy.unwrap_or_else(|| "localhost,127.0.0.1,::1,*.local".to_string());
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

            let rt = tokio::runtime::Runtime::new().map_err(|e| {
                bifrost_core::BifrostError::Config(format!("Failed to create runtime: {}", e))
            })?;

            rt.block_on(async {
                let pid = std::process::id();
                let runtime_info = RuntimeInfo {
                    pid,
                    port: config.port,
                    socks5_port: config.socks5_port,
                    host: Some(config.host.clone()),
                };
                write_runtime_info(&runtime_info).expect("Failed to write runtime info");
                // 先安装 shutdown 信号监听，避免初始化阶段收到 SIGTERM 导致直接退出、但无法记录优雅退出日志。
                // 这也能让 `bifrost stop` 在 daemon 启动早期更可靠。
                let result: bifrost_core::Result<()> = tokio::select! {
                    _ = wait_for_shutdown_signal() => {
                        // 注意：daemon 场景下 tracing 日志可能写入 rolling file（例如 bifrost.YYYY-MM-DD.log），
                        // 这里同时写到 stdout，确保 stop/测试能在 bifrost.log 中观察到“优雅退出”。
                        info!("Received shutdown signal");
                        println!("Received shutdown signal");
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

                    let access_control =
                        ProxyServer::new(config.clone()).access_control().clone();

                    let admin_state = AdminState::new(config.port)
                        .with_access_control(access_control.clone())
                        .with_body_store(body_store)
                        .with_ws_payload_store(ws_payload_store)
                        .with_async_traffic_writer_shared(async_traffic_writer)
                        .with_traffic_db_store_shared(traffic_db_store.clone())
                        .with_frame_store_shared(frame_store)
                        .with_runtime_config(runtime_config)
                        .with_connection_registry(connection_registry)
                        .with_values_storage(values_storage)
                        .with_rules_storage(rules_storage)
                        .with_ca_cert_path(ca_cert_path)
                        .with_system_proxy_manager_shared(system_proxy_manager.clone())
                        .with_config_manager_shared(shared_config_manager.clone())
                        .with_max_body_buffer_size(stored_config.traffic.max_body_buffer_size)
                        .with_app_icon_cache(app_icon_cache)
                        .with_script_manager(script_manager)
                        .with_replay_db_store_shared_opt(replay_db_store);

                    let sync_manager = Arc::new(
                        SyncManager::new(shared_config_manager.clone(), config.port)
                            .expect("Failed to create sync manager"),
                    );
                    let _sync_task = sync_manager.clone().start();
                    let admin_state = admin_state.with_sync_manager_shared(sync_manager);

                    std::mem::drop(bifrost_admin::start_db_cleanup_task(traffic_db_store));
                    std::mem::drop(bifrost_admin::start_connection_cleanup_task(
                        admin_state.connection_monitor.clone(),
                    ));

                    let metrics_collector = admin_state.metrics_collector.clone();
                    let rules_storage_for_resolver = admin_state.rules_storage.clone();
                    let config_manager_for_resolver = admin_state.config_manager.clone();
                    let values_storage_for_resolver = admin_state
                        .values_storage
                        .clone()
                        .expect("values_storage should be set");
                    let connection_registry_for_resolver = admin_state.connection_registry.clone();
                    let runtime_config_for_resolver = admin_state.runtime_config.clone();

                    let (stored_rules, inline_values) =
                        load_stored_rules(&rules_storage_for_resolver);
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
                    let server = ProxyServer::new(config)
                        .with_access_control(access_control)
                        .with_tls_config(tls_config)
                        .with_admin_state(admin_state)
                        .with_rules(resolver.clone());

                    let admin_state_arc = server
                        .admin_state()
                        .cloned()
                        .expect("admin_state should be set");

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
                    let server = server.with_push_manager(push_manager);

                    let _metrics_task = start_metrics_collector_task(metrics_collector, 1);

                    let rules_watcher_task = spawn_rules_watcher_task(
                        config_manager_for_resolver,
                        rules_storage_for_resolver,
                        values_storage_for_resolver,
                        resolver.clone(),
                        connection_registry_for_resolver,
                        runtime_config_for_resolver,
                    );

                    spawn_system_proxy_reconcile_task(SystemProxyReconcileConfig {
                        bifrost_dir: bifrost_dir.clone(),
                        system_proxy_manager: system_proxy_manager.clone(),
                        should_enable: enable_system_proxy,
                        proxy_host: system_proxy_host,
                        proxy_port: system_proxy_port,
                        system_proxy_bypass: system_proxy_bypass.clone(),
                        enabled_flag: system_proxy_enabled.clone(),
                        daemon_mode: true,
                    });

                    if let Err(e) = server.run().await {
                        eprintln!("Server error: {}", e);
                    }

                    rules_watcher_task.abort();
                    Ok(())
                }
                => result,
                };

                if let Err(e) = result {
                    eprintln!("Runtime error: {}", e);
                }
            });

            if cli_proxy_enabled {
                if let Err(e) = shell_proxy_manager.restore() {
                    eprintln!("Failed to restore CLI proxy: {}", e);
                }
            }
            if let Err(e) = system_proxy_manager.blocking_write().restore() {
                eprintln!("Failed to restore system proxy: {}", e);
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

fn load_stored_rules(
    rules_storage: &bifrost_storage::RulesStorage,
) -> (Vec<Rule>, HashMap<String, String>) {
    let mut stored_rules = Vec::new();
    let mut inline_values = HashMap::new();
    tracing::info!(
        target: "bifrost_cli::rules",
        base_dir = %rules_storage.base_dir().display(),
        "loading rules from storage"
    );
    match rules_storage.load_enabled() {
        Ok(rule_files) => {
            let stored_count = rule_files.len();
            for rule_file in rule_files {
                let parser = bifrost_core::RuleParser::new();
                let (result, file_inline_values) =
                    parser.parse_rules_tolerant_with_inline_values(&rule_file.content);

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

                tracing::info!(
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

fn spawn_rules_watcher_task(
    config_manager: Option<Arc<ConfigManager>>,
    rules_storage: bifrost_storage::RulesStorage,
    values_storage: bifrost_admin::SharedValuesStorage,
    resolver: SharedDynamicRulesResolver,
    connection_registry: bifrost_admin::SharedConnectionRegistry,
    runtime_config: bifrost_admin::SharedRuntimeConfig,
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
                        ConfigChangeEvent::RulesChanged | ConfigChangeEvent::ValuesChanged(_)
                    ) {
                        tracing::info!(
                            target: "bifrost_cli::rules",
                            event = ?event,
                            "config change event received, reloading rules"
                        );

                        let (new_stored_rules, inline_values) = load_stored_rules(&rules_storage);
                        let mut new_values = {
                            use bifrost_core::ValueStore;
                            values_storage.read().as_hashmap()
                        };
                        for (k, v) in inline_values {
                            new_values.entry(k).or_insert(v);
                        }

                        resolver.update_stored_rules(new_stored_rules, new_values);

                        if matches!(event, ConfigChangeEvent::RulesChanged) {
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
                    ConfigChangeEvent::RulesChanged
                    | ConfigChangeEvent::StateChanged
                    | ConfigChangeEvent::SyncConfigChanged => {}
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
