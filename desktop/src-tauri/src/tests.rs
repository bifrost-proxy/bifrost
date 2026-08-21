use super::{
    append_desktop_bootstrap_log, begin_backend_recovery, clear_backend_unavailable_if_healthy,
    configure_backend_stop_command, configure_desktop_backend_environment,
    confirms_managed_runtime_unresponsive, deferred_desktop_install_version_error,
    desktop_backend_env, desktop_backend_start_args, desktop_pending_install_path,
    desktop_shutdown_backend_action, desktop_sidecar_rust_log, desktop_startup_deadline,
    desktop_startup_session_id, desktop_test_allows_multiple_instances,
    desktop_upgrade_relaunch_marker_path, desktop_upgrade_shutdown_requested,
    ensure_backend_running, ensure_backend_running_with_cli_wait,
    existing_backend_candidate_matches_runtime, external_cli_backend_matches_handoff,
    external_cli_handoff_wait, failed_cli_handoff_can_retry_immediately,
    host_window_close_behavior_for_platform, is_server_config_response,
    is_upgrade_relaunch_marker_active, main_interface_decorations_for_platform,
    mark_backend_unavailable_for_manual_start, parse_port_update_response,
    persist_desktop_upgrade_handoff_failure, poll_managed_backend_exit,
    probe_backend_health_with_timeout, publish_startup_ready, read_active_upgrade_relaunch_marker,
    read_pending_desktop_install, record_startup_deadline_error, relaunch_command_for_target,
    resolve_bifrost_binary_from_env, resolve_desktop_config_path, resolve_desktop_data_dir,
    resolve_external_cli_backend_handoff, runtime_marker_matches_active_backend,
    sanitize_desktop_upgrade_relaunch_command, save_desktop_config,
    should_allow_multiple_instances, should_handoff_to_main, should_retry_backend_candidate,
    startup_deadline_disposition, stop_backend_before_restart, terminate_managed_backend,
    upgrade_handoff_requires_backend_release, upgrade_relaunch_uses_external_cli_backend,
    uses_borderless_desktop_chrome_for_platform, wait_for_backend, wait_for_backend_stop_helper,
    wait_for_child_exit, wait_for_external_cli_backend, windows_desktop_upgrade_handoff_command,
    write_desktop_upgrade_terminal_progress, write_upgrade_relaunch_marker, BackendRecoveryBudget,
    BackendSignalSnapshot, BackendState, BackendSystemIdentity, BackendWaitFailureKind,
    BackendWatchdogHealth, DesktopConfig, DesktopInstallRollback, DesktopRuntimeMarker,
    DesktopShutdownBackendAction, DesktopUpgradeRelaunchMarker, ExternalCliBackendHandoff,
    HostWindowCloseBehavior, PendingDesktopInstall, StartupDeadlineDisposition,
    SustainedReadinessAction, WatchdogProbeDisposition, BACKEND_WATCHDOG_MAX_RECOVERIES,
    BACKEND_WATCHDOG_RECOVERY_WINDOW, DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV,
    DESKTOP_UPGRADE_SHUTDOWN_ARG, DETACHED_DAEMON_CHILD_ENV, EXTERNAL_CLI_WORKER_ENV,
    WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT,
};
#[cfg(target_os = "macos")]
use super::{macos_menu_action, MacosMenuAction, MACOS_APP_QUIT_MENU_ID};
use bifrost_storage::data_dir as shared_bifrost_data_dir;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static ENV_LOCK: Mutex<()> = Mutex::new(());

mod backend_wait;
mod cli_handoff_recovery;
mod recovery_ownership;
mod traffic_detail_window;
mod watchdog;

#[cfg(target_os = "macos")]
#[test]
fn macos_app_quit_menu_enters_desktop_shutdown_coordinator() {
    assert_eq!(
        macos_menu_action(MACOS_APP_QUIT_MENU_ID),
        MacosMenuAction::Quit
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_edit_and_unknown_menu_actions_keep_their_existing_routing() {
    assert_eq!(
        macos_menu_action("edit-undo"),
        MacosMenuAction::Edit("undo")
    );
    assert_eq!(
        macos_menu_action("edit-redo"),
        MacosMenuAction::Edit("redo")
    );
    assert_eq!(
        macos_menu_action("edit-select-all"),
        MacosMenuAction::Edit("editor.action.selectAll")
    );
    assert_eq!(macos_menu_action("unknown"), MacosMenuAction::Ignore);
}

fn assert_upgrade_relaunch_environment_removed(command: &Command) {
    for key in [
        super::DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV,
        super::DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV,
        super::DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV,
    ] {
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == OsStr::new(key) && value.is_none()),
            "relaunch command must remove {key} before it opens the new App"
        );
    }
}

#[test]
fn desktop_backend_stop_command_is_authorized_for_owned_runtime() {
    let data_dir = Path::new("/tmp/bifrost-desktop-owned");
    let mut command = Command::new("bifrost");
    configure_backend_stop_command(&mut command, data_dir);

    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![OsStr::new("stop")]
    );
    let env = command.get_envs().collect::<Vec<_>>();
    assert!(env.iter().any(|(name, value)| {
        *name == OsStr::new("BIFROST_DATA_DIR")
            && *value == Some(OsStr::new("/tmp/bifrost-desktop-owned"))
    }));
    assert!(env.iter().any(|(name, value)| {
        *name == OsStr::new("BIFROST_DESKTOP_AUTHORIZED_STOP_INTERNAL")
            && *value == Some(OsStr::new("1"))
    }));
    assert!(env.iter().any(|(name, value)| {
        *name == OsStr::new("BIFROST_TRAY_INVOKED_STOP") && value.is_none()
    }));
}

fn test_backend_state(
    data_dir: PathBuf,
    port: u16,
    startup_ready: bool,
    startup_error: Option<String>,
) -> BackendState {
    BackendState {
        binary_path: PathBuf::new(),
        data_dir: data_dir.clone(),
        config_path: data_dir.join("desktop-config.json"),
        startup_session_id: "test-session".to_string(),
        launcher_only: false,
        expected_port: Mutex::new(port),
        port: Mutex::new(port),
        child: Mutex::new(None),
        shutdown_started: AtomicBool::new(false),
        force_exit: AtomicBool::new(false),
        backend_recovery_in_progress: AtomicBool::new(false),
        startup_ready: AtomicBool::new(startup_ready),
        startup_error: Mutex::new(startup_error),
        main_webview_loaded: AtomicBool::new(false),
        main_window_ready: AtomicBool::new(false),
        handoff_started: AtomicBool::new(false),
        handoff_completed: AtomicBool::new(false),
        launcher_overlay: Mutex::new(None),
        pending_open_requests: Mutex::new(Vec::new()),
        upgrade_relaunch: Mutex::new(None),
    }
}

fn spawn_one_shot_health_server() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind health server");
    let port = listener.local_addr().expect("health server addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
        }
    });
    port
}

fn spawn_delayed_health_server(delay: Duration, status: u16) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind delayed health server");
    let port = listener
        .local_addr()
        .expect("delayed health server addr")
        .port();
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

fn spawn_one_shot_system_server(pid: u32, version: &str) -> u16 {
    spawn_system_server(pid, version, 1)
}

fn spawn_system_server(pid: u32, version: &str, request_count: usize) -> u16 {
    spawn_system_server_on("127.0.0.1", pid, version, request_count)
}

fn spawn_system_server_on(host: &str, pid: u32, version: &str, request_count: usize) -> u16 {
    let listener = TcpListener::bind((host, 0)).expect("bind system server");
    let port = listener.local_addr().expect("system server addr").port();
    let body = format!(r#"{{"version":"{version}","pid":{pid}}}"#);
    thread::spawn(move || {
        for _ in 0..request_count {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

#[cfg(unix)]
fn spawn_persistent_health_server(stop: Arc<AtomicBool>) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind health server");
    listener
        .set_nonblocking(true)
        .expect("set health server nonblocking");
    let port = listener.local_addr().expect("health server addr").port();
    let handle = thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 1024];
                    let _ = stream.read(&mut buffer);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("health server accept failed: {error}"),
            }
        }
    });
    (port, handle)
}

#[test]
fn desktop_config_uses_shared_data_dir() {
    let target = resolve_desktop_config_path(&PathBuf::from("/tmp/shared-bifrost"));
    assert_eq!(
        target,
        PathBuf::from("/tmp/shared-bifrost/desktop-config.json")
    );
}

#[test]
fn desktop_data_dir_matches_shared_cli_dir() {
    assert_eq!(
        resolve_desktop_data_dir().unwrap(),
        shared_bifrost_data_dir()
    );
}

#[test]
fn save_desktop_config_creates_parent_dir() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir
        .path()
        .join("missing")
        .join("nested")
        .join("desktop-config.json");

    save_desktop_config(&config_path, &DesktopConfig { proxy_port: 19945 }).unwrap();

    let content = fs::read_to_string(config_path).unwrap();
    assert!(content.contains("\"proxy_port\": 19945"));
}

#[test]
fn parses_snake_case_port_update_response() {
    let response =
        parse_port_update_response(r#"{"expected_port":9901,"actual_port":9901}"#).unwrap();
    assert_eq!(response.expected_port, 9901);
    assert_eq!(response.actual_port, 9901);
}

#[test]
fn parses_camel_case_port_update_response() {
    let response =
        parse_port_update_response(r#"{"expectedPort":9901,"actualPort":9902}"#).unwrap();
    assert_eq!(response.expected_port, 9901);
    assert_eq!(response.actual_port, 9902);
}

#[test]
fn detects_legacy_server_config_response() {
    assert!(is_server_config_response(
        r#"{"timeout_secs":30,"http1_max_header_size":65536,"http2_max_header_list_size":262144,"websocket_handshake_max_header_size":65536}"#
    ));
}

#[test]
fn macos_close_request_hides_window() {
    assert_eq!(
        host_window_close_behavior_for_platform(true),
        HostWindowCloseBehavior::HideWindow
    );
}

#[test]
fn non_macos_close_request_shuts_down_app() {
    assert_eq!(
        host_window_close_behavior_for_platform(false),
        HostWindowCloseBehavior::ShutdownApp
    );
}

#[test]
fn windows_desktop_chrome_is_borderless() {
    assert!(uses_borderless_desktop_chrome_for_platform(true));
    assert!(!uses_borderless_desktop_chrome_for_platform(false));
}

#[test]
fn windows_main_interface_handoff_keeps_native_decorations_disabled() {
    assert!(!main_interface_decorations_for_platform(true));
    assert!(main_interface_decorations_for_platform(false));
}

#[test]
fn desktop_binary_path_can_be_overridden_for_debug_verification() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("BIFROST_DESKTOP_BIN");
    std::env::set_var("BIFROST_DESKTOP_BIN", "/tmp/bifrost-debug");
    assert_eq!(
        resolve_bifrost_binary_from_env(),
        Some(PathBuf::from("/tmp/bifrost-debug"))
    );
    match previous {
        Some(value) => std::env::set_var("BIFROST_DESKTOP_BIN", value),
        None => std::env::remove_var("BIFROST_DESKTOP_BIN"),
    }
}

#[test]
fn desktop_startup_session_id_contains_timestamp_and_pid() {
    let session_id = desktop_startup_session_id();
    let (timestamp, pid) = session_id
        .rsplit_once('-')
        .expect("session id should contain a pid separator");

    assert!(timestamp.parse::<u128>().is_ok());
    assert_eq!(pid, std::process::id().to_string());
}

#[test]
fn multiple_instances_test_override_is_debug_only_and_explicit() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os(DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV);

    std::env::remove_var(DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV);
    assert!(!desktop_test_allows_multiple_instances());
    std::env::set_var(DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV, "1");
    assert_eq!(
        desktop_test_allows_multiple_instances(),
        cfg!(debug_assertions)
    );

    match previous {
        Some(value) => std::env::set_var(DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV, value),
        None => std::env::remove_var(DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV),
    }
}

#[test]
fn release_build_never_allows_multiple_instances() {
    assert!(!should_allow_multiple_instances(false, false));
    assert!(!should_allow_multiple_instances(false, true));
    assert!(!should_allow_multiple_instances(true, false));
    assert!(should_allow_multiple_instances(true, true));
}

#[test]
fn internal_upgrade_shutdown_argument_is_detected_without_consuming_other_open_requests() {
    assert!(desktop_upgrade_shutdown_requested([
        OsStr::new("--unrelated"),
        OsStr::new(DESKTOP_UPGRADE_SHUTDOWN_ARG),
    ]));
    assert!(!desktop_upgrade_shutdown_requested([
        OsStr::new("bifrost://rules"),
        OsStr::new("example.bifrost"),
    ]));
}

#[test]
fn desktop_bootstrap_log_concurrent_writes_remain_line_atomic() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let data_dir = Arc::new(temp_dir.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();

    for writer in 0..8 {
        let data_dir = data_dir.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            for line in 0..25 {
                append_desktop_bootstrap_log(
                    &data_dir,
                    format!("atomic_test writer={writer} line={line}"),
                );
            }
        }));
    }
    for handle in handles {
        handle.join().expect("log writer thread");
    }

    let content = fs::read_to_string(temp_dir.path().join("logs/desktop-bootstrap.log"))
        .expect("bootstrap log");
    let lines = content.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 200);
    assert!(lines.iter().all(|line| {
        line.starts_with("[SystemTime ")
            && line.matches("atomic_test writer=").count() == 1
            && line.matches(" line=").count() == 1
    }));
}

#[test]
fn desktop_sidecar_disables_launchd_cleanup_registration() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let env = desktop_backend_env(temp_dir.path(), "session-123");
    let expected_data_dir = temp_dir.path().to_string_lossy().into_owned();

    assert!(env.iter().any(|(key, value)| {
        *key == "BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL" && value == "1"
    }));
    assert!(env
        .iter()
        .any(|(key, value)| *key == "BIFROST_DESKTOP_CORE" && value == "1"));
    assert!(env.iter().any(|(key, value)| {
        *key == "BIFROST_DESKTOP_STARTUP_SESSION_ID" && value == "session-123"
    }));
    assert!(env
        .iter()
        .any(|(key, value)| { *key == "RUST_LOG" && value.contains("bifrost_cli::startup=info") }));
    assert!(env
        .iter()
        .any(|(key, value)| { *key == "BIFROST_DATA_DIR" && value == &expected_data_dir }));
}

#[test]
fn desktop_sidecar_clears_inherited_process_role_markers() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let previous_path = std::env::var_os("PATH");
    let minimal_path = std::env::join_paths([
        temp_dir.path().join("system-bin"),
        temp_dir.path().join("fallback-bin"),
    ])
    .expect("minimal path");
    std::env::set_var("PATH", minimal_path);
    let expected_path = bifrost_core::inherited_executable_path().expect("expected PATH");
    let mut command = Command::new("bifrost");
    command.env(EXTERNAL_CLI_WORKER_ENV, "leaked-worker-role");
    configure_desktop_backend_environment(&mut command, temp_dir.path(), "session-123");

    let env = command.get_envs().collect::<Vec<_>>();
    assert!(env
        .iter()
        .any(|(key, value)| { *key == OsStr::new(DETACHED_DAEMON_CHILD_ENV) && value.is_none() }));
    assert!(env
        .iter()
        .any(|(key, value)| { *key == OsStr::new(EXTERNAL_CLI_WORKER_ENV) && value.is_none() }));
    assert!(env.iter().any(|(key, value)| {
        *key == OsStr::new("BIFROST_DESKTOP_CORE")
            && value.is_some_and(|value| value == OsStr::new("1"))
    }));
    let path = env
        .iter()
        .find_map(|(key, value)| {
            (*key == OsStr::new("PATH"))
                .then(|| value.map(ToOwned::to_owned))
                .flatten()
        })
        .expect("desktop sidecar PATH");
    assert_eq!(path, expected_path);
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
}

#[test]
fn desktop_sidecar_rust_log_preserves_user_filter_and_startup_info() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("RUST_LOG");

    std::env::set_var("RUST_LOG", "warn,hyper=debug");
    assert_eq!(
        desktop_sidecar_rust_log(),
        "warn,hyper=debug,bifrost_cli::startup=info"
    );
    std::env::set_var("RUST_LOG", "debug,bifrost_cli::startup=trace");
    assert_eq!(
        desktop_sidecar_rust_log(),
        "debug,bifrost_cli::startup=trace"
    );

    match previous {
        Some(value) => std::env::set_var("RUST_LOG", value),
        None => std::env::remove_var("RUST_LOG"),
    }
}

#[test]
fn desktop_sidecar_start_args_keep_system_proxy_policy_separate_from_launchd_registration() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("BIFROST_DESKTOP_NO_SYSTEM_PROXY");
    std::env::remove_var("BIFROST_DESKTOP_NO_SYSTEM_PROXY");
    assert_eq!(
        desktop_backend_start_args(19990),
        vec![
            "start".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            "19990".to_string(),
            "--skip-cert-check".to_string(),
        ]
    );

    std::env::set_var("BIFROST_DESKTOP_NO_SYSTEM_PROXY", "1");
    assert!(desktop_backend_start_args(19990)
        .iter()
        .any(|arg| arg == "--no-system-proxy"));

    match previous {
        Some(value) => std::env::set_var("BIFROST_DESKTOP_NO_SYSTEM_PROXY", value),
        None => std::env::remove_var("BIFROST_DESKTOP_NO_SYSTEM_PROXY"),
    }
}

#[test]
fn upgrade_relaunch_marker_activity_requires_fresh_supported_marker() {
    let fresh = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: 10_000,
        old_app_pid: 42,
        old_core_pid: Some(43),
        observed_external_core_pid: None,
        proxy_port: 19900,
        app_target: "/Applications/Bifrost.app".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: None,
        rollback: None,
    };
    assert!(is_upgrade_relaunch_marker_active(&fresh, 10_001));
    assert!(
        is_upgrade_relaunch_marker_active(&fresh, 10_000 + 11 * 60 * 1000),
        "the marker must outlive the Windows helper's 11-minute timeout budget"
    );

    let stale = DesktopUpgradeRelaunchMarker {
        created_at_ms: 1,
        ..fresh.clone()
    };
    assert!(!is_upgrade_relaunch_marker_active(
        &stale,
        1 + super::DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS + 1
    ));

    let unsupported = DesktopUpgradeRelaunchMarker {
        schema_version: 99,
        ..fresh.clone()
    };
    assert!(!is_upgrade_relaunch_marker_active(&unsupported, 10_001));

    let missing_port = DesktopUpgradeRelaunchMarker {
        proxy_port: 0,
        ..fresh
    };
    assert!(!is_upgrade_relaunch_marker_active(&missing_port, 10_001));
}

#[test]
fn deferred_desktop_installer_marker_is_validated_before_handoff() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let package = temp_dir.path().join("Bifrost.msi");
    fs::write(&package, b"test installer").expect("write installer fixture");
    let now_ms = super::current_time_millis();
    let pending = PendingDesktopInstall {
        schema_version: 1,
        created_at_ms: now_ms.saturating_sub(11 * 60 * 1000),
        package_path: package.to_string_lossy().into_owned(),
        target_version: "0.0.156".to_string(),
        package_owned_by_updater: true,
    };
    fs::write(
        desktop_pending_install_path(temp_dir.path()),
        serde_json::to_string(&pending).expect("encode pending installer"),
    )
    .expect("write pending installer marker");

    assert_eq!(
        read_pending_desktop_install(temp_dir.path()).expect("read pending installer"),
        Some(pending.clone())
    );
    let marker_path = desktop_upgrade_relaunch_marker_path(temp_dir.path());
    let command = windows_desktop_upgrade_handoff_command(
        Path::new("C:\\Temp\\desktop-upgrade-handoff.ps1"),
        &marker_path,
        Path::new("C:\\Users\\test\\Bifrost\\bifrost-desktop.exe"),
    );
    assert_eq!(command.get_program(), "powershell.exe");
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.iter().any(|arg| arg == "-File"));
    assert!(args
        .iter()
        .any(|arg| arg == "C:\\Temp\\desktop-upgrade-handoff.ps1"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("Wait-ForProcessExit"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("msiexec.exe"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("$quotedPackagePath"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("WaitForExit(30000)"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("@(0, 1641, 3010)"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("Write-Progress \"failed\""));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("$progressPath.tmp.$PID.$([Guid]::NewGuid().ToString('N'))"));
    assert!(
        WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
            .matches("for ($attempt = 0; $attempt -lt 100; $attempt++)")
            .count()
            >= 2
    );
    assert!(
        WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
            .matches("$win32Code -notin @(5, 32, 33)")
            .count()
            >= 2
    );
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("Start-Sleep -Milliseconds (2 + ($attempt % 7))"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("function Read-JsonWithRetry([string]$Path)"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("$progress = Read-JsonWithRetry $progressPath"));
    assert!(
        WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("$marker = Read-JsonWithRetry $MarkerPath")
    );
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("$MarkerPath.tmp.$PID.$([Guid]::NewGuid().ToString('N'))"));
    assert!(!WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("$progressPath.tmp.$PID\""));
    let snapshot_index = WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .find("$rollback = New-InstallSnapshot $marker")
        .expect("deferred install snapshots the current App");
    let installer_index = WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .find("$installer = Start-Process")
        .expect("deferred installer launch");
    assert!(
        snapshot_index < installer_index,
        "the old App must be snapshotted before MSI/EXE execution"
    );
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("$terminal = Wait-ForDesktopVerification $startedApp"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains(".Bifrost.rollback-"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT.contains("Restore-InstallSnapshot $rollback"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("Desktop app verification failed; previous version restored"));
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("deferred desktop install transaction committed"));
    assert_eq!(
        WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
            .matches("Remove-Item -LiteralPath $pendingPath")
            .count(),
        3,
        "the deferred guard is released after commit, verified rollback, or setup failure"
    );
    assert!(WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
        .contains("if ([bool]$marker.pending_install.package_owned_by_updater)"));
    assert_eq!(
        WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT
            .matches("Remove-Item -LiteralPath $packagePath")
            .count(),
        3,
        "each terminal path cleans the package only inside the ownership guard"
    );

    let legacy_pending: PendingDesktopInstall = serde_json::from_str(
            r#"{"schema_version":1,"created_at_ms":123,"package_path":"Bifrost.msi","target_version":"0.0.156"}"#,
        )
        .expect("decode legacy pending installer");
    assert!(!legacy_pending.package_owned_by_updater);

    let stale = PendingDesktopInstall {
        created_at_ms: now_ms.saturating_sub(super::DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS + 1),
        ..pending
    };
    fs::write(
        desktop_pending_install_path(temp_dir.path()),
        serde_json::to_string(&stale).expect("encode stale installer"),
    )
    .expect("write stale installer marker");
    assert!(read_pending_desktop_install(temp_dir.path())
        .expect_err("stale installer must be rejected")
        .contains("stale"));
}

#[test]
fn deferred_desktop_install_completion_requires_target_version() {
    let pending = PendingDesktopInstall {
        schema_version: 1,
        created_at_ms: super::current_time_millis(),
        package_path: "C:\\Temp\\Bifrost.msi".to_string(),
        target_version: "0.0.156".to_string(),
        package_owned_by_updater: false,
    };
    let rollback = DesktopInstallRollback {
        install_dir: "C:\\Program Files\\Bifrost".to_string(),
        backup_dir: "C:\\Temp\\bifrost-rollback".to_string(),
        had_previous_install: true,
    };
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: Some(124),
        observed_external_core_pid: None,
        proxy_port: 19900,
        app_target: "C:\\Program Files\\Bifrost\\bifrost-desktop.exe".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: Some(pending),
        rollback: Some(rollback),
    };

    assert_eq!(
        deferred_desktop_install_version_error(&marker, "v0.0.156"),
        None
    );
    let mismatch = deferred_desktop_install_version_error(&marker, "0.0.155")
        .expect("wrong relaunched App version must fail the upgrade");
    assert!(mismatch.contains("expected v0.0.156"));
    assert!(mismatch.contains("relaunched v0.0.155"));

    let non_deferred = DesktopUpgradeRelaunchMarker {
        pending_install: None,
        ..marker
    };
    assert_eq!(
        deferred_desktop_install_version_error(&non_deferred, "0.0.155"),
        None
    );

    let legacy_marker: DesktopUpgradeRelaunchMarker = serde_json::from_str(
        r#"{"schema_version":1,"created_at_ms":1,"old_app_pid":2,"old_core_pid":3,"proxy_port":19900,"app_target":"Bifrost.exe","pending_install":null}"#,
    )
    .expect("legacy relaunch marker without rollback metadata remains readable");
    assert_eq!(legacy_marker.rollback, None);
}

#[test]
fn deferred_desktop_completion_preserves_transaction_artifacts_for_helper_commit() {
    use bifrost_core::upgrade_progress::{UpgradePhase, UpgradeProgress};

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let backup_dir = temp_dir.path().join(".Bifrost.rollback-test");
    let package = temp_dir.path().join("Bifrost.msi");
    fs::create_dir_all(&backup_dir).expect("rollback dir");
    fs::write(backup_dir.join("old.exe"), b"old").expect("old app snapshot");
    fs::write(&package, b"package").expect("owned package");
    fs::write(desktop_pending_install_path(temp_dir.path()), b"pending").expect("pending marker");
    bifrost_core::upgrade_progress::write_progress(
        temp_dir.path(),
        &UpgradeProgress::new(UpgradePhase::Restarting, "Waiting for desktop verification")
            .with_target(Some("0.0.156".to_string()))
            .with_source(Some("desktop".to_string())),
    );

    write_desktop_upgrade_terminal_progress(
        temp_dir.path(),
        UpgradePhase::Completed,
        "Desktop app and core update complete",
        None,
    );

    assert!(
        backup_dir.exists(),
        "only the waiting helper commits rollback cleanup"
    );
    assert!(
        desktop_pending_install_path(temp_dir.path()).exists(),
        "the cross-process guard survives until the helper observes Completed"
    );
    assert!(
        package.exists(),
        "the helper retains ownership of its package"
    );
}

#[test]
fn upgrade_handoff_terminal_progress_preserves_target_and_reports_failure() {
    use bifrost_core::upgrade_progress::{
        read_progress, write_progress, UpgradePhase, UpgradeProgress,
    };

    let temp_dir = tempfile::tempdir().expect("temp dir");
    write_progress(
        temp_dir.path(),
        &UpgradeProgress::new(UpgradePhase::Restarting, "Waiting for desktop shell")
            .with_target(Some("0.0.156".to_string()))
            .with_source(Some("desktop".to_string())),
    );
    write_desktop_upgrade_terminal_progress(
        temp_dir.path(),
        UpgradePhase::Failed,
        "New core failed",
        Some("port still occupied".to_string()),
    );

    let progress = read_progress(temp_dir.path());
    assert_eq!(progress.phase, UpgradePhase::Failed);
    assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));
    assert_eq!(progress.source.as_deref(), Some("desktop"));
    assert_eq!(progress.error.as_deref(), Some("port still occupied"));
}

#[test]
fn restart_handoff_setup_failure_overwrites_completed_progress() {
    use bifrost_core::upgrade_progress::{
        read_progress, write_progress, UpgradePhase, UpgradeProgress,
    };

    let temp_dir = tempfile::tempdir().expect("temp dir");
    write_progress(
        temp_dir.path(),
        &UpgradeProgress::new(UpgradePhase::Completed, "Desktop app update complete")
            .with_target(Some("0.0.156".to_string()))
            .with_source(Some("desktop".to_string())),
    );

    let returned = persist_desktop_upgrade_handoff_failure(
        temp_dir.path(),
        "failed to spawn desktop upgrade relaunch helper: denied".to_string(),
    );
    assert!(returned.contains("denied"));
    let progress = read_progress(temp_dir.path());
    assert_eq!(progress.phase, UpgradePhase::Failed);
    assert_eq!(progress.target_version.as_deref(), Some("0.0.156"));
    assert_eq!(progress.source.as_deref(), Some("desktop"));
    assert!(progress
        .error
        .as_deref()
        .is_some_and(|error| error.contains("denied")));
}

#[test]
fn upgrade_relaunch_marker_round_trips_and_stale_marker_is_removed() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(122),
        proxy_port: 19900,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: None,
        rollback: None,
    };

    let marker_path =
        write_upgrade_relaunch_marker(temp_dir.path(), &marker).expect("write marker");
    assert_eq!(
        marker_path,
        desktop_upgrade_relaunch_marker_path(temp_dir.path())
    );
    assert_eq!(
        read_active_upgrade_relaunch_marker(temp_dir.path()),
        Some(marker)
    );

    let stale_marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: 1,
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(122),
        proxy_port: 19900,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: None,
        rollback: None,
    };
    fs::write(
        desktop_upgrade_relaunch_marker_path(temp_dir.path()),
        serde_json::to_string(&stale_marker).expect("encode stale marker"),
    )
    .expect("write stale marker");

    assert_eq!(read_active_upgrade_relaunch_marker(temp_dir.path()), None);
    assert!(
        !desktop_upgrade_relaunch_marker_path(temp_dir.path()).exists(),
        "stale marker should be removed so normal startup is not blocked"
    );
}

#[test]
fn cli_owned_upgrade_relaunch_falls_back_to_managed_core_when_port_is_free() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
    let target_port = listener.local_addr().expect("reserved addr").port();
    drop(listener);
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(124),
        proxy_port: target_port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: None,
        rollback: None,
    };

    let started_at = Instant::now();
    let error = ensure_backend_running_with_cli_wait(
        Path::new("/missing/bundled/bifrost"),
        temp_dir.path(),
        "cli-owned-fallback-test",
        target_port,
        Some(&marker),
        Duration::ZERO,
    )
    .expect_err("missing bundled binary should fail after taking the fallback path");

    assert!(
        started_at.elapsed() < Duration::from_secs(2),
        "fallback should not wait for the normal upgrade handoff release once the port is free"
    );
    assert!(
        error.to_string().contains("failed to start backend"),
        "expected managed-core launch path, got: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains("CLI-owned backend did not restart"),
        "free port should not keep the App trapped in CLI-owned handoff refusal"
    );
    let bootstrap_log =
        fs::read_to_string(temp_dir.path().join("logs/desktop-bootstrap.log")).expect("log");
    assert!(bootstrap_log.contains("port is free, launching desktop-managed core"));
}

#[test]
fn cli_owned_upgrade_relaunch_keeps_refusing_when_port_is_still_occupied() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind((super::BACKEND_BIND_HOST, 0)).expect("occupy port");
    let target_port = listener.local_addr().expect("occupied addr").port();
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: None,
        observed_external_core_pid: Some(124),
        proxy_port: target_port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.156".to_string()),
        pending_install: None,
        rollback: None,
    };

    let error = ensure_backend_running_with_cli_wait(
        Path::new("/must-not-launch-a-second-core"),
        temp_dir.path(),
        "cli-owned-occupied-test",
        target_port,
        Some(&marker),
        Duration::ZERO,
    )
    .expect_err("occupied port should still block a replacement core");

    drop(listener);
    assert!(error
        .to_string()
        .contains("port is still occupied, refusing to launch a second desktop-managed core"));
}

#[cfg(target_os = "macos")]
#[test]
fn upgrade_relaunch_app_command_strips_helper_environment_before_open() {
    let target = PathBuf::from("/Applications/Bifrost.app");
    let mut command = relaunch_command_for_target(&target);
    sanitize_desktop_upgrade_relaunch_command(&mut command);

    assert_eq!(command.get_program(), OsStr::new("open"));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![OsStr::new("-n"), target.as_os_str()]
    );
    assert_upgrade_relaunch_environment_removed(&command);
}

#[test]
fn upgrade_relaunch_executable_command_strips_helper_environment() {
    let target = PathBuf::from("/tmp/bifrost-desktop");
    let mut command = relaunch_command_for_target(&target);
    sanitize_desktop_upgrade_relaunch_command(&mut command);

    assert_eq!(command.get_program(), target.as_os_str());
    assert_eq!(command.get_args().count(), 0);
    assert_upgrade_relaunch_environment_removed(&command);
}

#[test]
fn backend_recovery_guard_prevents_parallel_recovery() {
    let flag = AtomicBool::new(false);
    let state = BackendState {
        backend_recovery_in_progress: flag,
        ..test_backend_state(PathBuf::new(), 0, false, None)
    };

    let guard = begin_backend_recovery(&state).expect("first recovery guard");
    assert!(
        begin_backend_recovery(&state).is_none(),
        "second recovery must be rejected while the first one is active"
    );
    drop(guard);
    assert!(
        begin_backend_recovery(&state).is_some(),
        "recovery flag should be released after guard drop"
    );
}

#[test]
fn desktop_shutdown_stops_only_a_backend_owned_by_the_desktop() {
    assert_eq!(
        desktop_shutdown_backend_action(false, Some("daemon"), true),
        DesktopShutdownBackendAction::PreserveExternalRuntime
    );
    assert_eq!(
        desktop_shutdown_backend_action(false, None, false),
        DesktopShutdownBackendAction::PreserveExternalRuntime
    );
    assert_eq!(
        desktop_shutdown_backend_action(false, Some("desktop"), false),
        DesktopShutdownBackendAction::PreserveExternalRuntime,
        "a stale desktop runtime marker must not authorize stopping an unrelated core"
    );
    assert_eq!(
        desktop_shutdown_backend_action(false, Some("desktop"), true),
        DesktopShutdownBackendAction::StopOwnedRuntime
    );
    assert_eq!(
        desktop_shutdown_backend_action(true, Some("daemon"), false),
        DesktopShutdownBackendAction::StopOwnedRuntime,
        "the live managed child remains authoritative even before its runtime marker is visible"
    );

    let runtime = DesktopRuntimeMarker {
        pid: 456,
        port: 19900,
        health_port: None,
        start_mode: Some("desktop".to_string()),
    };
    let identity = BackendSystemIdentity {
        version: "0.0.163".to_string(),
        pid: 456,
    };
    assert!(runtime_marker_matches_active_backend(
        &runtime, 19900, &identity
    ));
    assert!(
        !runtime_marker_matches_active_backend(&runtime, 19901, &identity),
        "a stale desktop runtime on another port must not authorize a stop"
    );
}

#[test]
fn normal_startup_reuses_only_the_current_data_directory_runtime() {
    let runtime = DesktopRuntimeMarker {
        pid: 456,
        port: 19900,
        health_port: None,
        start_mode: Some("daemon".to_string()),
    };
    let matching_identity = BackendSystemIdentity {
        version: "0.0.165".to_string(),
        pid: 456,
    };
    let foreign_identity = BackendSystemIdentity {
        version: "0.0.165".to_string(),
        pid: 789,
    };

    assert!(existing_backend_candidate_matches_runtime(
        Some(&runtime),
        19900,
        Some(&matching_identity),
        true,
    ));
    assert!(!existing_backend_candidate_matches_runtime(
        None,
        19900,
        Some(&matching_identity),
        true,
    ));
    assert!(!existing_backend_candidate_matches_runtime(
        Some(&runtime),
        19901,
        Some(&matching_identity),
        true,
    ));
    assert!(!existing_backend_candidate_matches_runtime(
        Some(&runtime),
        19900,
        Some(&foreign_identity),
        true,
    ));
    assert!(!existing_backend_candidate_matches_runtime(
        Some(&runtime),
        19900,
        Some(&matching_identity),
        false,
    ));
}

#[test]
fn external_backend_health_failure_requires_manual_start() {
    let temp_dir =
        std::env::temp_dir().join(format!("bifrost-desktop-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("create temp dir");
    let state = test_backend_state(temp_dir.clone(), 19900, true, None);

    mark_backend_unavailable_for_manual_start(&state, "backend health probe failed on port 19900");

    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert!(state
        .startup_error
        .lock()
        .expect("startup error lock")
        .as_deref()
        .expect("startup error")
        .contains("Start the service from Bifrost Desktop"));
    assert!(state.child.lock().expect("child lock").is_none());
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn healthy_external_backend_clears_manual_start_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_one_shot_health_server();
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some(
            "Bifrost service is not running. Start the service from Bifrost Desktop to continue."
                .to_string(),
        ),
    );

    assert!(clear_backend_unavailable_if_healthy(
        &state,
        "test observed recovered backend",
    ));
    assert!(state.startup_ready.load(Ordering::SeqCst));
    assert!(state
        .startup_error
        .lock()
        .expect("startup error lock")
        .is_none());
}

#[test]
fn healthy_backend_still_clears_manual_start_gate_during_app_managed_upgrade() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let port = spawn_one_shot_health_server();
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: 1,
        created_at_ms: super::current_time_millis(),
        old_app_pid: 123,
        old_core_pid: Some(456),
        observed_external_core_pid: None,
        proxy_port: port,
        app_target: "/tmp/Bifrost.app".to_string(),
        target_version: Some("0.0.163".to_string()),
        pending_install: None,
        rollback: None,
    };
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some("previous app-managed handoff failed".to_string()),
    );
    *state.upgrade_relaunch.lock().expect("marker lock") = Some(marker);

    assert!(clear_backend_unavailable_if_healthy(
        &state,
        "test observed app-managed recovered backend",
    ));
    assert!(state.startup_ready.load(Ordering::SeqCst));
    assert!(state.startup_error.lock().expect("error lock").is_none());
}

#[test]
fn unhealthy_external_backend_keeps_manual_start_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
    let port = listener.local_addr().expect("reserved addr").port();
    drop(listener);
    let state = test_backend_state(
        temp_dir.path().to_path_buf(),
        port,
        false,
        Some("Bifrost service is not running.".to_string()),
    );

    assert!(!clear_backend_unavailable_if_healthy(
        &state,
        "test observed missing backend",
    ));
    assert!(!state.startup_ready.load(Ordering::SeqCst));
    assert_eq!(
        state
            .startup_error
            .lock()
            .expect("startup error lock")
            .as_deref(),
        Some("Bifrost service is not running.")
    );
}

#[test]
fn launcher_handoff_allows_ready_or_terminal_startup_state() {
    assert!(should_handoff_to_main(true, false, true));
    assert!(should_handoff_to_main(false, true, true));
    assert!(!should_handoff_to_main(false, false, true));
    assert!(!should_handoff_to_main(true, false, false));
    assert!(!should_handoff_to_main(false, true, false));
}

#[test]
fn desktop_startup_deadline_defaults_and_accepts_test_override() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os(super::DESKTOP_STARTUP_DEADLINE_MS_ENV);

    std::env::remove_var(super::DESKTOP_STARTUP_DEADLINE_MS_ENV);
    assert_eq!(
        desktop_startup_deadline(),
        super::DEFAULT_DESKTOP_STARTUP_DEADLINE
    );

    std::env::set_var(super::DESKTOP_STARTUP_DEADLINE_MS_ENV, "1250");
    assert_eq!(desktop_startup_deadline(), Duration::from_millis(1250));

    match previous {
        Some(value) => std::env::set_var(super::DESKTOP_STARTUP_DEADLINE_MS_ENV, value),
        None => std::env::remove_var(super::DESKTOP_STARTUP_DEADLINE_MS_ENV),
    }
}

#[test]
fn startup_deadline_uses_native_error_until_webview_is_loaded() {
    assert_eq!(
        startup_deadline_disposition(false),
        StartupDeadlineDisposition::ShowNativeError
    );
    assert_eq!(
        startup_deadline_disposition(true),
        StartupDeadlineDisposition::HandoffToWebview
    );
}

#[test]
fn startup_deadline_does_not_overwrite_a_ready_backend() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let ready_state = test_backend_state(temp_dir.path().to_path_buf(), 19900, true, None);
    assert!(!record_startup_deadline_error(
        &ready_state,
        Duration::from_secs(30)
    ));
    assert!(ready_state
        .startup_error
        .lock()
        .expect("startup error lock")
        .is_none());

    let pending_state = test_backend_state(temp_dir.path().to_path_buf(), 19900, false, None);
    assert!(record_startup_deadline_error(
        &pending_state,
        Duration::from_secs(30)
    ));
    assert!(pending_state
        .startup_error
        .lock()
        .expect("startup error lock")
        .as_deref()
        .is_some_and(|error| error.contains("did not finish starting")));

    publish_startup_ready(&pending_state);
    assert!(pending_state.startup_ready.load(Ordering::SeqCst));
    assert!(pending_state
        .startup_error
        .lock()
        .expect("startup error lock")
        .is_none());
    assert!(!record_startup_deadline_error(
        &pending_state,
        Duration::from_secs(30)
    ));
}

#[test]
fn port_retry_only_handles_confirmed_bind_races() {
    assert!(should_retry_backend_candidate(
        BackendWaitFailureKind::ChildExited,
        false,
        true
    ));
    assert!(!should_retry_backend_candidate(
        BackendWaitFailureKind::ChildExited,
        true,
        true
    ));
    assert!(!should_retry_backend_candidate(
        BackendWaitFailureKind::TimedOut,
        false,
        true
    ));
    assert!(!should_retry_backend_candidate(
        BackendWaitFailureKind::ChildInspection,
        false,
        true
    ));
    assert!(!should_retry_backend_candidate(
        BackendWaitFailureKind::ChildExited,
        false,
        false
    ));
}

#[cfg(unix)]
#[test]
fn stale_backend_stop_failure_blocks_a_second_start() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    fs::write(temp_dir.path().join("runtime.json"), "{}").expect("runtime marker");
    let stop_stub = temp_dir.path().join("failing-stop");
    fs::write(&stop_stub, "#!/bin/sh\nexit 17\n").expect("stop stub");
    let mut permissions = fs::metadata(&stop_stub)
        .expect("stub metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&stop_stub, permissions).expect("make stub executable");

    let error = super::cleanup_existing_backend(&stop_stub, temp_dir.path())
        .expect_err("failed stale stop must block startup");
    assert!(error
        .to_string()
        .contains("Refusing to start another service"));
    let log = fs::read_to_string(temp_dir.path().join("logs").join("desktop-bootstrap.log"))
        .expect("bootstrap log");
    assert!(log.contains("refusing to start a second backend"));
}

#[cfg(unix)]
#[test]
fn restart_stop_failure_blocks_a_replacement_backend() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let stop_stub = temp_dir.path().join("failing-restart-stop");
    fs::write(&stop_stub, "#!/bin/sh\nexit 23\n").expect("stop stub");
    let mut permissions = fs::metadata(&stop_stub)
        .expect("stub metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&stop_stub, permissions).expect("make stub executable");

    let error = stop_backend_before_restart(
        &stop_stub,
        temp_dir.path(),
        59998,
        Duration::from_millis(50),
    )
    .expect_err("failed restart stop must block replacement");
    assert!(error
        .to_string()
        .contains("Refusing to start a replacement"));
    let log = fs::read_to_string(temp_dir.path().join("logs").join("desktop-bootstrap.log"))
        .expect("bootstrap log");
    assert!(log.contains("backend stop failed before restart"));
    assert!(log.contains("refusing to start a replacement"));
}

#[cfg(unix)]
#[test]
fn restart_requires_the_old_backend_to_be_observed_down() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let stop_stub = temp_dir.path().join("successful-restart-stop");
    fs::write(&stop_stub, "#!/bin/sh\nexit 0\n").expect("stop stub");
    let mut permissions = fs::metadata(&stop_stub)
        .expect("stub metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&stop_stub, permissions).expect("make stub executable");

    let server_stop = Arc::new(AtomicBool::new(false));
    let (port, server) = spawn_persistent_health_server(Arc::clone(&server_stop));
    let error = stop_backend_before_restart(
        &stop_stub,
        temp_dir.path(),
        port,
        Duration::from_millis(100),
    )
    .expect_err("healthy old backend must block replacement");
    server_stop.store(true, Ordering::SeqCst);
    server.join().expect("health server join");

    assert!(error.to_string().contains("remained healthy"));
    assert!(error
        .to_string()
        .contains("Refusing to start a replacement"));
}

#[test]
fn poisoned_managed_child_state_blocks_a_replacement_backend() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = test_backend_state(temp_dir.path().to_path_buf(), 59997, false, None);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = state.child.lock().expect("child lock");
        panic!("poison child lock");
    }));

    let error = terminate_managed_backend(&state, "before test replacement")
        .expect_err("poisoned child state must block replacement");
    assert!(error
        .to_string()
        .contains("failed to access managed backend child"));
}

#[cfg(unix)]
#[test]
fn child_wait_timeout_terminates_stuck_process() {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "sleep 5"])
        .spawn()
        .expect("spawn stuck child");
    let started_at = Instant::now();
    let error = wait_for_child_exit(&mut child, Duration::from_millis(100))
        .expect_err("stuck child should time out");

    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(started_at.elapsed() < Duration::from_secs(2));
    assert!(child.try_wait().expect("poll killed child").is_some());
}

#[cfg(unix)]
#[test]
fn desktop_quit_waits_for_successful_backend_stop_helper() {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "sleep 0.05; exit 0"])
        .spawn()
        .expect("spawn successful stop helper");

    wait_for_backend_stop_helper(&mut child, Duration::from_secs(1))
        .expect("successful stop helper must allow Desktop exit");
}

#[cfg(unix)]
#[test]
fn desktop_quit_rejects_failed_backend_stop_helper() {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exit 23"])
        .spawn()
        .expect("spawn failed stop helper");

    let error = wait_for_backend_stop_helper(&mut child, Duration::from_secs(1))
        .expect_err("failed stop helper must keep Desktop alive");
    assert!(error.to_string().contains("status"));
}
