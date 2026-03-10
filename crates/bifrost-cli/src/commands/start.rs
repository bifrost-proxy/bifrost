use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bifrost_admin::{
    start_async_traffic_processor, start_frame_cleanup_task, start_metrics_collector_task,
    start_push_tasks, start_ws_payload_cleanup_task, status_printer::TlsStatusInfo, AdminState,
    AsyncTrafficWriter, BodyStore, PushManager, ReplayDbStore, RuntimeConfig, WsPayloadStore,
};
use bifrost_core::Rule;
use bifrost_proxy::{AccessMode, ProxyConfig, ProxyServer};
use bifrost_storage::{set_data_dir, ConfigChangeEvent, ConfigManager};
use bifrost_tls::{get_platform_name, CertInstaller, CertStatus};
use parking_lot::RwLock as ParkingRwLock;
use tracing::info;

use crate::commands::ca::{check_and_install_certificate, load_tls_config};
use crate::config::get_bifrost_dir;
use crate::help::print_startup_help;
use crate::parsing::{parse_cli_rules, DynamicRulesResolver, SharedDynamicRulesResolver};
use crate::process::{is_process_running, read_pid, remove_pid, write_runtime_info, RuntimeInfo};

const ASYNC_TRAFFIC_BUFFER_SIZE: usize = 10000;

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
    disconnect_on_config_change: bool,
    config_manager: ConfigManager,
) -> bifrost_core::Result<()> {
    let pid = std::process::id();
    let runtime_info = RuntimeInfo {
        pid,
        port: config.port,
        socks5_port: config.socks5_port,
        host: Some(config.host.clone()),
    };
    write_runtime_info(&runtime_info)?;

    print_startup_help(config.port);

    println!("════════════════════════════════════════════════════════════════════════");
    println!("                           SERVER STATUS");
    println!("════════════════════════════════════════════════════════════════════════");

    let tls_config = load_tls_config(&config)?;

    let bind_addr = format!("{}:{}", config.host, config.port);
    std::net::TcpListener::bind(&bind_addr).map_err(|e| {
        bifrost_core::BifrostError::Network(format!("Failed to bind to {}: {}", bind_addr, e))
    })?;

    let bifrost_dir = config_manager.data_dir().to_path_buf();
    let system_proxy_manager = std::sync::Arc::new(tokio::sync::RwLock::new(
        bifrost_core::SystemProxyManager::new(bifrost_dir.clone()),
    ));
    let _system_proxy_restore_guard = SystemProxyRestoreGuard::new(system_proxy_manager.clone());

    if let Err(e) = bifrost_core::SystemProxyManager::recover_from_crash(&bifrost_dir) {
        tracing::warn!("Failed to recover system proxy from previous crash: {}", e);
    }

    let mut system_proxy_enabled = false;
    if enable_system_proxy {
        let proxy_host = if config.host == "0.0.0.0" {
            "127.0.0.1".to_string()
        } else {
            config.host.clone()
        };
        let mut manager = system_proxy_manager.blocking_write();
        let result = manager.enable(&proxy_host, config.port, Some(&system_proxy_bypass));

        let final_result = match &result {
            Ok(()) => result,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("RequiresAdmin") {
                    println!(
                        "  ⚠ System proxy requires admin privileges, requesting authorization..."
                    );
                    #[cfg(target_os = "macos")]
                    {
                        manager.enable_with_gui_auth(
                            &proxy_host,
                            config.port,
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
                system_proxy_enabled = true;
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("UserCancelled") {
                    println!("  ⚠ System proxy not enabled (authorization cancelled)");
                } else if msg.contains("RequiresAdmin") {
                    println!("  ⚠ System proxy requires admin privileges (not enabled)");
                } else {
                    eprintln!("  ✗ Failed to enable system proxy: {}", e);
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
    if system_proxy_enabled {
        println!("   Status:        ✓ Enabled");
        println!("   Bypass:        {}", system_proxy_bypass);
    } else if enable_system_proxy {
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

    rt.block_on(async {
        let stored_config = config_manager.config().await;
        let body_temp_dir = bifrost_dir.join("body_cache");
        let body_store = Arc::new(ParkingRwLock::new(BodyStore::new(
            body_temp_dir,
            stored_config.traffic.max_body_memory_size,
            stored_config.traffic.file_retention_days,
            stored_config.traffic.sse_stream_flush_bytes,
            Duration::from_millis(stored_config.traffic.sse_stream_flush_interval_ms),
        )));
        bifrost_admin::start_body_cleanup_task(body_store.clone());

        let ws_payload_store = Arc::new(WsPayloadStore::new(
            bifrost_dir.clone(),
            stored_config.traffic.ws_payload_flush_bytes,
            Duration::from_millis(stored_config.traffic.ws_payload_flush_interval_ms),
            stored_config.traffic.ws_payload_max_open_files,
            stored_config.traffic.file_retention_days,
        ));
        start_ws_payload_cleanup_task(ws_payload_store.clone());

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
        traffic_db_store.set_body_index_enabled(stored_config.traffic.enable_body_index);

        let (async_traffic_writer, async_traffic_rx) =
            AsyncTrafficWriter::new(ASYNC_TRAFFIC_BUFFER_SIZE);
        let async_traffic_writer = Arc::new(async_traffic_writer);
        let _async_traffic_task =
            start_async_traffic_processor(async_traffic_rx, Some(traffic_db_store.clone()), None);

        let frame_store = Arc::new(bifrost_admin::FrameStore::new(
            bifrost_dir.clone(),
            Some(stored_config.traffic.file_retention_days * 24),
        ));
        start_frame_cleanup_task(frame_store.clone());

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
            disconnect_on_config_change,
        };
        let connection_registry =
            bifrost_admin::ConnectionRegistry::new(disconnect_on_config_change);

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

        let admin_state = AdminState::new(config.port)
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
            .with_config_manager(config_manager)
            .with_max_body_buffer_size(stored_config.traffic.max_body_buffer_size)
            .with_max_body_probe_size(stored_config.traffic.max_body_probe_size)
            .with_app_icon_cache(app_icon_cache)
            .with_script_manager(script_manager)
            .with_replay_db_store_shared_opt(replay_db_store);

        bifrost_admin::start_db_cleanup_task(traffic_db_store);
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

        log_resolver_rules(&resolver);

        let unsafe_ssl = config.unsafe_ssl;
        let server = ProxyServer::new(config)
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

        tokio::select! {
            result = server.run() => {
                if let Err(e) = result {
                    eprintln!("Server error: {}", e);
                }
            }
            _ = wait_for_shutdown_signal() => {
                info!("Received shutdown signal");
                println!("\nShutting down...");
            }
        }

        rules_watcher_task.abort();
    });

    remove_pid()?;
    println!("Bifrost proxy stopped.");
    Ok(())
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
pub fn run_daemon(
    config: ProxyConfig,
    cli_rules: Vec<Rule>,
    cli_values: HashMap<String, String>,
    enable_system_proxy: bool,
    system_proxy_bypass: String,
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
            std::net::TcpListener::bind(&bind_addr).map_err(|e| {
                bifrost_core::BifrostError::Network(format!(
                    "Failed to bind to {}: {}",
                    bind_addr, e
                ))
            })?;

            let system_proxy_manager = std::sync::Arc::new(tokio::sync::RwLock::new(
                bifrost_core::SystemProxyManager::new(bifrost_dir.clone()),
            ));
            let _system_proxy_restore_guard =
                SystemProxyRestoreGuard::new(system_proxy_manager.clone());

            if let Err(e) = bifrost_core::SystemProxyManager::recover_from_crash(&bifrost_dir) {
                tracing::warn!("Failed to recover system proxy from previous crash: {}", e);
            }

            if enable_system_proxy {
                let proxy_host = if config.host == "0.0.0.0" {
                    "127.0.0.1".to_string()
                } else {
                    config.host.clone()
                };
                let mut manager = system_proxy_manager.blocking_write();
                let result = manager.enable(&proxy_host, config.port, Some(&system_proxy_bypass));

                let final_result = match &result {
                    Ok(()) => result,
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("RequiresAdmin") {
                            println!("System proxy requires admin privileges, requesting authorization...");
                            #[cfg(target_os = "macos")]
                            {
                                manager.enable_with_gui_auth(
                                    &proxy_host,
                                    config.port,
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

                if let Err(e) = final_result {
                    let msg = e.to_string();
                    if msg.contains("UserCancelled") {
                        println!("System proxy not enabled (authorization cancelled)");
                    } else if msg.contains("RequiresAdmin") {
                        println!("System proxy requires administrator privileges; daemon will continue without changing system proxy. You can toggle it later via CLI or Admin UI.");
                    } else {
                        eprintln!("Failed to enable system proxy: {}", e);
                    }
                }
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
                let shutdown = tokio::spawn(wait_for_shutdown_signal());
                tokio::task::yield_now().await;

                let stored_config = config_manager.config().await;
                let body_temp_dir = bifrost_dir.join("body_cache");
                let body_store = Arc::new(ParkingRwLock::new(BodyStore::new(
                    body_temp_dir,
                    stored_config.traffic.max_body_memory_size,
                    stored_config.traffic.file_retention_days,
                    stored_config.traffic.sse_stream_flush_bytes,
                    Duration::from_millis(stored_config.traffic.sse_stream_flush_interval_ms),
                )));
                bifrost_admin::start_body_cleanup_task(body_store.clone());

                let ws_payload_store = Arc::new(WsPayloadStore::new(
                    bifrost_dir.clone(),
                    stored_config.traffic.ws_payload_flush_bytes,
                    Duration::from_millis(stored_config.traffic.ws_payload_flush_interval_ms),
                    stored_config.traffic.ws_payload_max_open_files,
                    stored_config.traffic.file_retention_days,
                ));
                start_ws_payload_cleanup_task(ws_payload_store.clone());

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
                traffic_db_store.set_body_index_enabled(stored_config.traffic.enable_body_index);

                let (async_traffic_writer, async_traffic_rx) =
                    AsyncTrafficWriter::new(ASYNC_TRAFFIC_BUFFER_SIZE);
                let async_traffic_writer = Arc::new(async_traffic_writer);
                let _async_traffic_task = start_async_traffic_processor(
                    async_traffic_rx,
                    Some(traffic_db_store.clone()),
                    None,
                );

                let frame_store = Arc::new(bifrost_admin::FrameStore::new(
                    bifrost_dir.clone(),
                    Some(stored_config.traffic.file_retention_days * 24),
                ));
                start_frame_cleanup_task(frame_store.clone());

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

                let admin_state = AdminState::new(config.port)
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
                    .with_config_manager(config_manager)
                    .with_max_body_buffer_size(stored_config.traffic.max_body_buffer_size)
                    .with_app_icon_cache(app_icon_cache)
                    .with_script_manager(script_manager)
                    .with_replay_db_store_shared_opt(replay_db_store);

                bifrost_admin::start_db_cleanup_task(traffic_db_store);
                bifrost_admin::start_connection_cleanup_task(
                    admin_state.connection_monitor.clone(),
                );

                let metrics_collector = admin_state.metrics_collector.clone();
                let rules_storage_for_resolver = admin_state.rules_storage.clone();
                let config_manager_for_resolver = admin_state.config_manager.clone();
                let values_storage_for_resolver = admin_state
                    .values_storage
                    .clone()
                    .expect("values_storage should be set");
                let connection_registry_for_resolver = admin_state.connection_registry.clone();
                let runtime_config_for_resolver = admin_state.runtime_config.clone();

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

                log_resolver_rules(&resolver);

                let unsafe_ssl = config.unsafe_ssl;
                let server = ProxyServer::new(config)
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

                tokio::select! {
                    result = server.run() => {
                        if let Err(e) = result {
                            eprintln!("Server error: {}", e);
                        }
                    }
                    _ = shutdown => {
                        tracing::info!("Received shutdown signal");
                        eprintln!("Received shutdown signal");
                    }
                }

                rules_watcher_task.abort();
            });

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
