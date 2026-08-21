#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod backend_runtime;
#[cfg(target_os = "macos")]
mod macos_menu;
mod native_launcher;
mod open_requests;
mod runtime_ownership;
mod traffic_detail_window;
mod upgrade_handoff;

use backend_runtime::*;
#[cfg(target_os = "macos")]
use macos_menu::*;
use runtime_ownership::*;
use traffic_detail_window::*;
use upgrade_handoff::*;

use bifrost_core::upgrade_progress::{
    read_progress, write_progress, UpgradePhase, UpgradeProgress,
};
use bifrost_core::{
    cleanup_bifrost_log_dir, direct_blocking_reqwest_client_builder, inherited_executable_path,
    EXTERNAL_CLI_WORKER_ENV,
};
use bifrost_storage::data_dir as shared_bifrost_data_dir;
use bifrost_tls::{ensure_valid_ca, generate_root_ca, save_root_ca, CertInstaller, CertStatus};
use open_requests::{parse_open_url, DesktopOpenRequest, OpenRequestParseError};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;
#[cfg(target_os = "macos")]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
#[cfg(target_os = "macos")]
use tauri::window::EffectState;
use tauri::window::{Window, WindowBuilder};
#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;
use tauri::{
    image::Image,
    webview::{Color, WebviewBuilder},
    window::{Effect, EffectsBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Size, State, WebviewUrl,
};
use tauri_plugin_deep_link::DeepLinkExt;

const BACKEND_BIND_HOST: &str = "0.0.0.0";
const BACKEND_ADMIN_HOST: &str = "127.0.0.1";
const DEFAULT_BACKEND_PORT: u16 = 9900;
const MAX_PORT_INCREMENT_ATTEMPTS: u16 = 64;
const HOST_WINDOW_LABEL: &str = "host";
const MAIN_WINDOW_LABEL: &str = "main";
const DESKTOP_LOG_RETENTION_DAYS: u32 = bifrost_core::DEFAULT_LOG_RETENTION_DAYS;
const TARGET_WINDOW_WIDTH: f64 = 1440.0;
const TARGET_WINDOW_HEIGHT: f64 = 920.0;
const TARGET_WINDOW_MIN_WIDTH: f64 = 1180.0;
const TARGET_WINDOW_MIN_HEIGHT: f64 = 760.0;
const OVERLAY_FADE_STEPS: u16 = 8;
const OVERLAY_FADE_STEP_DELAY: Duration = Duration::from_millis(14);
const BACKEND_WATCHDOG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(3);
const BACKEND_HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(450);
const BACKEND_HEALTH_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(3);
const BACKEND_WATCHDOG_MIN_FAILURES: u32 = 4;
const BACKEND_WATCHDOG_UNHEALTHY_GRACE: Duration = Duration::from_secs(15);
const BACKEND_WATCHDOG_MAX_RECOVERIES: usize = 3;
const BACKEND_WATCHDOG_RECOVERY_WINDOW: Duration = Duration::from_secs(5 * 60);
const BACKEND_STOP_TIMEOUT: Duration = Duration::from_secs(35);
const DESKTOP_QUIT_STOP_TIMEOUT: Duration = Duration::from_secs(35);
const BACKEND_KILL_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_DESKTOP_STARTUP_DEADLINE: Duration = Duration::from_secs(30);
const WEBVIEW_PARK_OFFSET: f64 = 2000.0;
const WEBVIEW_REVEAL_SETTLE_DELAY: Duration = Duration::from_millis(90);
const HANDOFF_COMPLETE_EVENT: &str = "desktop://handoff-complete";
const OPEN_REQUEST_EVENT: &str = "desktop://open-request";
const DESKTOP_CORE_ENV: &str = "BIFROST_DESKTOP_CORE";
const DESKTOP_RESTART_STOP_ENV: &str = "BIFROST_DESKTOP_RESTART_STOP_INTERNAL";
const DETACHED_DAEMON_CHILD_ENV: &str = "BIFROST_DETACHED_DAEMON_CHILD";
const DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV: &str = "BIFROST_DESKTOP_UPGRADE_RELAUNCH_HELPER";
const DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV: &str = "BIFROST_DESKTOP_UPGRADE_RELAUNCH_MARKER";
const DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV: &str = "BIFROST_DESKTOP_UPGRADE_RELAUNCH_TARGET";
const DESKTOP_UPGRADE_RELAUNCH_MARKER_FILE: &str = "desktop-upgrade-relaunch.json";
const DESKTOP_PENDING_INSTALL_FILE: &str = "desktop-upgrade-pending-install.json";
const DESKTOP_UPGRADE_RELAUNCH_SCHEMA_VERSION: u8 = 1;
const DESKTOP_PENDING_INSTALL_SCHEMA_VERSION: u8 = 1;
// Must exceed the Windows helper's 30s App wait + 30s core wait + 10m
// installer timeout so a valid handoff cannot expire before relaunch.
const DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS: u64 = 15 * 60 * 1000;
const DESKTOP_UPGRADE_RELAUNCH_PROCESS_WAIT: Duration = Duration::from_secs(30);
const DESKTOP_UPGRADE_RELAUNCH_PORT_WAIT: Duration = Duration::from_secs(30);
const SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV: &str =
    "BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL";
const DESKTOP_STARTUP_DEADLINE_MS_ENV: &str = "BIFROST_DESKTOP_STARTUP_DEADLINE_MS";
const DESKTOP_STARTUP_SESSION_ENV: &str = "BIFROST_DESKTOP_STARTUP_SESSION_ID";
const DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV: &str =
    "BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES";
const DESKTOP_UPGRADE_SHUTDOWN_ARG: &str = "--bifrost-upgrade-shutdown";
#[cfg(target_os = "macos")]
const MACOS_APP_QUIT_MENU_ID: &str = "app-quit";
static DESKTOP_BOOTSTRAP_LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopConfig {
    proxy_port: u16,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            proxy_port: DEFAULT_BACKEND_PORT,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopRuntimeInfo {
    expected_proxy_port: u16,
    proxy_port: u16,
    platform: &'static str,
    startup_ready: bool,
    startup_error: Option<String>,
    handoff_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopPortUpdateResponse {
    #[serde(alias = "expectedPort")]
    expected_port: u16,
    #[serde(alias = "actualPort")]
    actual_port: u16,
}

#[derive(Debug, Deserialize)]
struct DesktopServerConfigResponse {
    timeout_secs: u64,
    http1_max_header_size: usize,
    http2_max_header_list_size: usize,
    websocket_handshake_max_header_size: usize,
}

#[derive(Debug, Deserialize)]
struct DesktopRuntimeMarker {
    pid: u32,
    port: u16,
    #[serde(default)]
    health_port: Option<u16>,
    #[serde(default, rename = "runtime_start_mode", alias = "start_mode")]
    start_mode: Option<String>,
}

enum BackendPortTransition {
    Rebound(DesktopPortUpdateResponse),
    RestartRequired,
}

struct BackendState {
    binary_path: PathBuf,
    data_dir: PathBuf,
    config_path: PathBuf,
    startup_session_id: String,
    launcher_only: bool,
    expected_port: Mutex<u16>,
    port: Mutex<u16>,
    child: Mutex<Option<Child>>,
    shutdown_started: AtomicBool,
    force_exit: AtomicBool,
    backend_recovery_in_progress: AtomicBool,
    startup_ready: AtomicBool,
    startup_error: Mutex<Option<String>>,
    main_webview_loaded: AtomicBool,
    main_window_ready: AtomicBool,
    handoff_started: AtomicBool,
    handoff_completed: AtomicBool,
    launcher_overlay: Mutex<Option<usize>>,
    pending_open_requests: Mutex<Vec<DesktopOpenRequest>>,
    upgrade_relaunch: Mutex<Option<DesktopUpgradeRelaunchMarker>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostWindowCloseBehavior {
    HideWindow,
    ShutdownApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendWaitFailureKind {
    ChildExited,
    ChildInspection,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupDeadlineDisposition {
    HandoffToWebview,
    ShowNativeError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopShutdownBackendAction {
    StopOwnedRuntime,
    PreserveExternalRuntime,
}

#[derive(Debug)]
struct BackendWaitFailure {
    kind: BackendWaitFailureKind,
    message: String,
}

impl std::fmt::Display for BackendWaitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackendWaitFailure {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingDesktopInstall {
    schema_version: u8,
    created_at_ms: u64,
    package_path: String,
    target_version: String,
    #[serde(default)]
    package_owned_by_updater: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DesktopInstallRollback {
    install_dir: String,
    backup_dir: String,
    had_previous_install: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DesktopUpgradeRelaunchMarker {
    schema_version: u8,
    created_at_ms: u64,
    old_app_pid: u32,
    old_core_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_external_core_pid: Option<u32>,
    proxy_port: u16,
    app_target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_install: Option<PendingDesktopInstall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback: Option<DesktopInstallRollback>,
}

fn main() {
    if run_desktop_upgrade_relaunch_helper_from_env() {
        return;
    }

    let builder = tauri::Builder::default();

    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(|app| {
            // App menu (macOS displays first submenu as app name)
            // A predefined Quit item calls AppKit terminate: directly and can
            // bypass Tauri's ExitRequested callback. Keep Quit custom so both
            // the menu click and Cmd+Q enter the owned lifecycle coordinator.
            let quit = MenuItem::with_id(
                app,
                MACOS_APP_QUIT_MENU_ID,
                "Quit Bifrost",
                true,
                Some("CmdOrCtrl+Q"),
            )?;
            let app_menu = Submenu::with_items(
                app,
                "Bifrost",
                true,
                &[
                    &PredefinedMenuItem::about(app, None, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::show_all(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;
            let file_menu = Submenu::with_items(
                app,
                "File",
                true,
                &[&PredefinedMenuItem::close_window(app, None)?],
            )?;
            // Edit menu: Undo/Redo/SelectAll use custom MenuItem so on_menu_event
            // can forward them to the WebView (PredefinedMenuItem bypasses JS).
            // Cut/Copy/Paste keep PredefinedMenuItem — they work natively via
            // WKWebView clipboard events that Monaco already handles.
            let undo = MenuItem::with_id(app, "edit-undo", "Undo", true, Some("CmdOrCtrl+Z"))?;
            let redo = MenuItem::with_id(
                app,
                "edit-redo",
                "Redo",
                true,
                Some("CmdOrCtrl+Shift+Z"),
            )?;
            let select_all = MenuItem::with_id(
                app,
                "edit-select-all",
                "Select All",
                true,
                Some("CmdOrCtrl+A"),
            )?;
            let edit_menu = Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &undo,
                    &redo,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &select_all,
                ],
            )?;
            let view_menu = Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?;
            let window_menu = Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::close_window(app, None)?,
                ],
            )?;
            Menu::with_items(
                app,
                &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
            )
        })
        .on_menu_event(|app, event| {
            let action = macos_menu_action(event.id().as_ref());
            if action == MacosMenuAction::Quit {
                request_desktop_shutdown(app);
                return;
            }

            // Forward custom Edit menu actions directly to the WebView via eval().
            // We bypass the Tauri event system because emit_to target routing
            // may not match JS-side listen() calls. Instead we dispatch a DOM
            // CustomEvent that the JS layer picks up reliably.
            if let MacosMenuAction::Edit(action) = action {
                if let Some(webview) = app.get_webview(MAIN_WINDOW_LABEL) {
                    let js = format!(
                        r#"window.dispatchEvent(new CustomEvent("bifrost-edit-command",{{detail:"{action}"}}))"#
                    );
                    let _ = webview.eval(&js);
                }
            }
        });

    let builder = builder.plugin(tauri_plugin_deep_link::init());
    let builder = if desktop_test_allows_multiple_instances() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            if desktop_upgrade_shutdown_requested(args.iter().skip(1)) {
                request_desktop_shutdown(app);
            } else {
                handle_cli_file_open_arguments(
                    app,
                    args.into_iter().skip(1),
                    Some(PathBuf::from(cwd)),
                );
            }
        }))
    };

    builder
        .invoke_handler(tauri::generate_handler![
            get_desktop_runtime,
            start_desktop_core,
            update_desktop_proxy_port,
            issue_desktop_upgrade_origin_token,
            restart_desktop_after_update,
            notify_main_window_ready,
            get_pending_desktop_open_requests,
            set_document_edited,
            open_traffic_detail_window,
            close_traffic_detail_window,
            open_external_url,
            write_clipboard
        ])
        .setup(|app| {
            let upgrade_shutdown_requested =
                desktop_upgrade_shutdown_requested(std::env::args_os().skip(1));
            let host_window = create_host_window(app.handle())?;
            host_window.set_icon(load_app_icon()?)?;
            if !supports_native_launcher() {
                apply_window_effects(&host_window)?;
            }

            let binary_path = resolve_bifrost_binary(app.handle())?;
            let app_data_dir = resolve_desktop_data_dir()?;
            let config_path = resolve_desktop_config_path(&app_data_dir);
            let startup_session_id = desktop_startup_session_id();
            let app_config_dir = config_path
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| anyhow("missing desktop config dir".to_string()))?;

            fs::create_dir_all(&app_config_dir)
                .map_err(|error| anyhow(format!("failed to create config dir: {error}")))?;
            fs::create_dir_all(&app_data_dir)
                .map_err(|error| anyhow(format!("failed to create data dir: {error}")))?;
            append_desktop_bootstrap_log(
                &app_data_dir,
                format!(
                    "desktop startup session started; session_id={} desktop_pid={} app_version={} target_os={} target_arch={}",
                    startup_session_id,
                    std::process::id(),
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                ),
            );
            append_desktop_bootstrap_log(
                &app_data_dir,
                format!(
                    "desktop setup started; session_id={} binary_path={} data_dir={} config_dir={}",
                    startup_session_id,
                    binary_path.display(),
                    app_data_dir.display(),
                    app_config_dir.display()
                ),
            );
            if desktop_test_allows_multiple_instances() {
                append_desktop_bootstrap_log(
                    &app_data_dir,
                    "debug E2E mode enabled; single-instance plugin intentionally disabled",
                );
            }
            let config = load_desktop_config(&config_path)?;
            let launcher_only = is_launcher_only_mode();
            let upgrade_relaunch = read_active_upgrade_relaunch_marker(&app_data_dir);
            if let Some(marker) = upgrade_relaunch.as_ref() {
                append_desktop_bootstrap_log(
                    &app_data_dir,
                    format!(
                        "desktop upgrade relaunch marker accepted; old_app_pid={} old_core_pid={:?} proxy_port={} target={}",
                        marker.old_app_pid,
                        marker.old_core_pid,
                        marker.proxy_port,
                        marker.app_target
                    ),
                );
            }

            app.manage(BackendState {
                binary_path,
                data_dir: app_data_dir,
                config_path,
                startup_session_id,
                launcher_only,
                expected_port: Mutex::new(config.proxy_port),
                port: Mutex::new(config.proxy_port),
                child: Mutex::new(None),
                shutdown_started: AtomicBool::new(false),
                force_exit: AtomicBool::new(false),
                backend_recovery_in_progress: AtomicBool::new(false),
                startup_ready: AtomicBool::new(false),
                startup_error: Mutex::new(None),
                main_webview_loaded: AtomicBool::new(false),
                main_window_ready: AtomicBool::new(false),
                handoff_started: AtomicBool::new(false),
                handoff_completed: AtomicBool::new(false),
                launcher_overlay: Mutex::new(None),
                pending_open_requests: Mutex::new(Vec::new()),
                upgrade_relaunch: Mutex::new(upgrade_relaunch),
            });

            if upgrade_shutdown_requested {
                request_desktop_shutdown(app.handle());
                return Ok(());
            }

            install_open_request_handlers(app.handle());

            if supports_native_launcher() {
                if let Some(state) = app.try_state::<BackendState>() {
                    if let Some(overlay_ptr) = native_launcher::install(&host_window)? {
                        native_launcher::start_animation(&host_window, overlay_ptr)?;
                        reveal_host_window(&host_window);
                        if let Ok(mut overlay_guard) = state.launcher_overlay.lock() {
                            *overlay_guard = Some(overlay_ptr);
                        }
                    }
                }
            } else if let Some(state) = app.try_state::<BackendState>() {
                state.handoff_started.store(true, Ordering::SeqCst);
                state.handoff_completed.store(true, Ordering::SeqCst);
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "native launcher unsupported on this platform; entering webview directly",
                );
            }

            if launcher_only {
                if let Some(state) = app.try_state::<BackendState>() {
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        "launcher-only mode enabled; skipping embedded webview and backend bootstrap",
                    );
                }
            } else {
                create_main_webview(&host_window)?;

                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    bootstrap_desktop_backend(&app_handle);
                });

                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    monitor_desktop_backend(&app_handle);
                });

                schedule_desktop_startup_deadline(app.handle());
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != HOST_WINDOW_LABEL {
                return;
            }

            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                handle_host_window_close_request(window);
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build desktop app")
        .run(|app_handle, event| {
            match event {
                tauri::RunEvent::ExitRequested { api, .. } => {
                    if should_intercept_exit(app_handle) {
                        api.prevent_exit();
                        request_desktop_shutdown(app_handle);
                    }
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen {
                    has_visible_windows: false,
                    ..
                } => {
                    restore_host_window(app_handle);
                }
                _ => {}
            }
        });
}

fn should_intercept_exit(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<BackendState>() else {
        return false;
    };

    !state.force_exit.load(Ordering::SeqCst)
}

fn handle_host_window_close_request(window: &Window) {
    match host_window_close_behavior() {
        HostWindowCloseBehavior::HideWindow => {
            if let Some(state) = window.app_handle().try_state::<BackendState>() {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "host window close requested on macOS; hiding window and keeping app alive",
                );
            }
            let _ = window.hide();
        }
        HostWindowCloseBehavior::ShutdownApp => request_desktop_shutdown(window.app_handle()),
    }
}

fn host_window_close_behavior() -> HostWindowCloseBehavior {
    host_window_close_behavior_for_platform(cfg!(target_os = "macos"))
}

fn host_window_close_behavior_for_platform(is_macos: bool) -> HostWindowCloseBehavior {
    if is_macos {
        HostWindowCloseBehavior::HideWindow
    } else {
        HostWindowCloseBehavior::ShutdownApp
    }
}

fn is_launcher_only_mode() -> bool {
    matches!(
        std::env::var("BIFROST_DESKTOP_LAUNCHER_ONLY"),
        Ok(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
    )
}

fn supports_native_launcher() -> bool {
    cfg!(target_os = "macos")
}

fn uses_borderless_desktop_chrome() -> bool {
    uses_borderless_desktop_chrome_for_platform(cfg!(target_os = "windows"))
}

fn uses_borderless_desktop_chrome_for_platform(is_windows: bool) -> bool {
    is_windows
}

fn main_interface_decorations_for_platform(is_windows: bool) -> bool {
    !uses_borderless_desktop_chrome_for_platform(is_windows)
}

fn main_interface_decorations() -> bool {
    main_interface_decorations_for_platform(cfg!(target_os = "windows"))
}

fn load_app_icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!("../../../assets/bifrost.png"))
}

fn apply_window_effects(window: &Window) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    window.set_effects(
        EffectsBuilder::new()
            .effects([Effect::UnderWindowBackground, Effect::Sidebar])
            .state(EffectState::Active)
            .radius(18.0)
            .build(),
    )?;

    #[cfg(target_os = "windows")]
    window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build())?;

    Ok(())
}

fn create_host_window(app: &AppHandle) -> tauri::Result<Window> {
    if let Some(window) = app.get_window(HOST_WINDOW_LABEL) {
        return Ok(window);
    }

    let mut builder = WindowBuilder::new(app, HOST_WINDOW_LABEL)
        .title("Bifrost")
        .center();

    if supports_native_launcher() {
        builder = builder
            .inner_size(TARGET_WINDOW_WIDTH, TARGET_WINDOW_HEIGHT)
            .min_inner_size(TARGET_WINDOW_MIN_WIDTH, TARGET_WINDOW_MIN_HEIGHT)
            .resizable(true)
            .maximizable(true)
            .visible(true)
            .transparent(true)
            .background_color(Color(0, 0, 0, 0));
        #[cfg(target_os = "macos")]
        {
            builder = builder.decorations(true).shadow(true);
        }
        #[cfg(not(target_os = "macos"))]
        {
            builder = builder.decorations(false).shadow(false);
        }
    } else {
        builder = builder
            .inner_size(TARGET_WINDOW_WIDTH, TARGET_WINDOW_HEIGHT)
            .min_inner_size(TARGET_WINDOW_MIN_WIDTH, TARGET_WINDOW_MIN_HEIGHT)
            .resizable(true)
            .maximizable(true)
            .decorations(!uses_borderless_desktop_chrome())
            .visible(true)
            .transparent(false)
            .shadow(true)
            .background_color(Color(8, 17, 23, 255));
    }

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .hidden_title(true)
            .title_bar_style(TitleBarStyle::Overlay);
    }

    builder
        .build()
        .map_err(|error| anyhow(format!("failed to create host window: {error}")))
}

fn create_main_webview(window: &Window) -> tauri::Result<()> {
    if window.app_handle().get_webview(MAIN_WINDOW_LABEL).is_some() {
        return Ok(());
    }

    let webview = WebviewBuilder::new(MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
        .background_color(Color(8, 17, 23, 255))
        .auto_resize()
        .disable_drag_drop_handler()
        .on_page_load(|webview, payload| {
            if let Some(state) = webview.try_state::<BackendState>() {
                if payload.event() == tauri::webview::PageLoadEvent::Finished {
                    state.main_webview_loaded.store(true, Ordering::SeqCst);
                }
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!(
                        "embedded webview page load event {:?} on {}",
                        payload.event(),
                        payload.url()
                    ),
                );
            }

            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                try_start_native_handoff(webview.app_handle(), "webview finished loading");
            }
        });

    let webview = window
        .add_child(
            webview,
            Position::Logical(LogicalPosition::new(
                if supports_native_launcher() {
                    WEBVIEW_PARK_OFFSET
                } else {
                    0.0
                },
                0.0,
            )),
            Size::Logical(LogicalSize::new(TARGET_WINDOW_WIDTH, TARGET_WINDOW_HEIGHT)),
        )
        .map_err(|error| anyhow(format!("failed to create embedded webview: {error}")))?;
    let _ = webview.set_background_color(Some(Color(8, 17, 23, 255)));

    Ok(())
}

fn resolve_bifrost_binary(app: &AppHandle) -> tauri::Result<PathBuf> {
    let binary_name = if cfg!(target_os = "windows") {
        "bifrost.exe"
    } else {
        "bifrost"
    };

    if let Some(path) = resolve_bifrost_binary_from_env() {
        return Ok(path);
    }

    if cfg!(debug_assertions) {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        return Ok(manifest_dir
            .join("..")
            .join("..")
            .join("target")
            .join("debug")
            .join(binary_name));
    }

    let resource_dir = app.path().resource_dir()?;
    let bundled_path = resource_dir.join("resources").join("bin").join(binary_name);
    if bundled_path.exists() {
        return Ok(bundled_path);
    }

    Ok(resource_dir.join("bin").join(binary_name))
}

fn resolve_bifrost_binary_from_env() -> Option<PathBuf> {
    std::env::var_os("BIFROST_DESKTOP_BIN").and_then(|value| {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            None
        } else {
            Some(path)
        }
    })
}

fn resolve_desktop_data_dir() -> tauri::Result<PathBuf> {
    Ok(shared_bifrost_data_dir())
}

fn resolve_desktop_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("desktop-config.json")
}

fn ensure_desktop_cert_ready(data_dir: &Path) {
    if desktop_skip_cert_preflight_requested() {
        append_desktop_bootstrap_log(
            data_dir,
            "desktop certificate preflight skipped by BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT",
        );
        return;
    }

    match prepare_desktop_certificates(data_dir) {
        Ok(CertStatus::InstalledAndTrusted) => append_desktop_bootstrap_log(
            data_dir,
            "desktop certificate preflight complete; CA already installed and trusted",
        ),
        Ok(CertStatus::InstalledNotTrusted) => append_desktop_bootstrap_log(
            data_dir,
            "desktop certificate preflight complete; CA trust was repaired",
        ),
        Ok(CertStatus::NotInstalled) => append_desktop_bootstrap_log(
            data_dir,
            "desktop certificate preflight complete; CA was installed and trusted",
        ),
        Err(error) => {
            let message = error.to_string();
            if message.contains("UserCancelled") {
                append_desktop_bootstrap_log(
                    data_dir,
                    "desktop certificate preflight cancelled by user; continuing startup without trusted CA",
                );
            } else {
                append_desktop_bootstrap_log(
                    data_dir,
                    format!(
                        "desktop certificate preflight failed; continuing startup without trusted CA: {error}"
                    ),
                );
            }
        }
    }
}

fn prepare_desktop_certificates(data_dir: &Path) -> Result<CertStatus, String> {
    let cert_dir = data_dir.join("certs");
    let ca_cert_path = cert_dir.join("ca.crt");
    let ca_key_path = cert_dir.join("ca.key");

    fs::create_dir_all(&cert_dir).map_err(|error| format!("failed to create cert dir: {error}"))?;

    let ca_valid = ensure_valid_ca(&ca_cert_path, &ca_key_path)
        .map_err(|error| format!("failed to validate CA certificate: {error}"))?;
    if !ca_valid {
        let ca = generate_root_ca().map_err(|error| format!("failed to generate CA: {error}"))?;
        save_root_ca(&ca_cert_path, &ca_key_path, &ca)
            .map_err(|error| format!("failed to save CA files: {error}"))?;
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "generated desktop CA certificate at {}",
                ca_cert_path.display()
            ),
        );
    }

    let installer = CertInstaller::new(&ca_cert_path);
    let status = installer
        .check_status()
        .map_err(|error| format!("failed to check CA trust status: {error}"))?;

    if status == CertStatus::InstalledAndTrusted {
        return Ok(status);
    }

    append_desktop_bootstrap_log(
        data_dir,
        format!("desktop CA status is {status}; attempting GUI install/trust"),
    );
    installer
        .install_and_trust_gui()
        .map_err(|error| format!("failed to install/trust desktop CA via GUI flow: {error}"))?;

    installer
        .check_status()
        .map_err(|error| format!("failed to re-check CA trust status: {error}"))
}

fn start_backend(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    port: u16,
) -> tauri::Result<Child> {
    let stdout_log = open_sidecar_log_file(data_dir, "desktop-sidecar.out.log")?;
    let stderr_log = open_sidecar_log_file(data_dir, "desktop-sidecar.err.log")?;

    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "starting sidecar; session_id={} binary_path={} data_dir={} port={} stdout_log={} stderr_log={}",
            startup_session_id,
            binary_path.display(),
            data_dir.display(),
            port,
            log_dir(data_dir).join("desktop-sidecar.out.log").display(),
            log_dir(data_dir).join("desktop-sidecar.err.log").display()
        ),
    );

    let mut command = Command::new(binary_path);
    command
        .args(desktop_backend_start_args(port))
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));
    configure_desktop_backend_environment(&mut command, data_dir, startup_session_id);
    hide_windows_child_console(&mut command);
    let child = command
        .spawn()
        .map_err(|error| anyhow(format!("failed to start backend: {error}")))?;
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "sidecar spawned; session_id={} pid={} port={}",
            startup_session_id,
            child.id(),
            port
        ),
    );
    Ok(child)
}

fn desktop_backend_start_args(port: u16) -> Vec<String> {
    let mut args = vec![
        "start".to_string(),
        "--host".to_string(),
        BACKEND_BIND_HOST.to_string(),
        "--port".to_string(),
        port.to_string(),
        "--skip-cert-check".to_string(),
    ];
    if desktop_no_system_proxy_requested() {
        args.push("--no-system-proxy".to_string());
    }
    args
}

fn configure_desktop_backend_environment(
    command: &mut Command,
    data_dir: &Path,
    startup_session_id: &str,
) {
    command
        .env_remove(DETACHED_DAEMON_CHILD_ENV)
        .env_remove(EXTERNAL_CLI_WORKER_ENV);
    if let Some(path) = inherited_executable_path() {
        command.env("PATH", path);
    }
    command.envs(desktop_backend_env(data_dir, startup_session_id));
}

fn desktop_startup_session_id() -> String {
    let started_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{started_at_ms}-{}", std::process::id())
}

fn desktop_backend_env(data_dir: &Path, startup_session_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("BIFROST_DATA_DIR", data_dir.to_string_lossy().into_owned()),
        (DESKTOP_CORE_ENV, "1".to_string()),
        (DESKTOP_STARTUP_SESSION_ENV, startup_session_id.to_string()),
        ("RUST_LOG", desktop_sidecar_rust_log()),
        (SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV, "1".to_string()),
    ]
}

fn desktop_sidecar_rust_log() -> String {
    let inherited = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "info".to_string());
    if inherited
        .split(',')
        .any(|directive| directive.trim().starts_with("bifrost_cli::startup="))
    {
        inherited
    } else {
        format!("{inherited},bifrost_cli::startup=info")
    }
}

fn desktop_test_allows_multiple_instances() -> bool {
    should_allow_multiple_instances(
        cfg!(debug_assertions),
        env_flag_enabled(DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV),
    )
}

fn should_allow_multiple_instances(debug_build: bool, requested: bool) -> bool {
    debug_build && requested
}

fn desktop_no_system_proxy_requested() -> bool {
    env_flag_enabled("BIFROST_DESKTOP_NO_SYSTEM_PROXY")
}

fn desktop_skip_cert_preflight_requested() -> bool {
    env_flag_enabled("BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT")
}

fn env_flag_enabled(key: &str) -> bool {
    std::env::var(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

fn desktop_upgrade_shutdown_requested<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(DESKTOP_UPGRADE_SHUTDOWN_ARG))
}

fn request_desktop_shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        app.exit(0);
        return;
    };

    if state.shutdown_started.swap(true, Ordering::SeqCst) {
        return;
    }

    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop shutdown requested; hiding window and waiting for owned backend and tray to stop",
    );
    if let Some(window) = app.get_window(HOST_WINDOW_LABEL) {
        let _ = window.hide();
    }

    let app_handle = app.clone();
    if state.launcher_only {
        state.force_exit.store(true, Ordering::SeqCst);
        app.exit(0);
    } else {
        std::thread::spawn(move || {
            complete_desktop_shutdown(&app_handle);
        });
    }
}

fn complete_desktop_shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        app.exit(0);
        return;
    };

    match desktop_shutdown_backend_action_for_state(&state) {
        DesktopShutdownBackendAction::StopOwnedRuntime => {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop shutdown owns the active backend; requesting backend stop",
            );
            let stop_result = match spawn_backend_stop(&state.binary_path, &state.data_dir) {
                Ok(mut child) => {
                    let helper_pid = child.id();
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!(
                            "spawned backend stop helper pid={helper_pid}; waiting for owned backend and tray shutdown"
                        ),
                    );
                    wait_for_backend_stop_helper(&mut child, DESKTOP_QUIT_STOP_TIMEOUT).map_err(
                        |error| {
                            format!(
                                "backend stop helper pid={helper_pid} did not complete successfully: {error}"
                            )
                        },
                    )
                }
                Err(error) => Err(format!("failed to spawn backend stop helper: {error}")),
            };
            if let Err(error) = stop_result {
                cancel_desktop_shutdown(app, &state, &error);
                return;
            }
            append_desktop_bootstrap_log(
                &state.data_dir,
                "backend stop helper completed successfully; owned backend and tray are stopped",
            );
        }
        DesktopShutdownBackendAction::PreserveExternalRuntime => {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop shutdown is preserving the external CLI-owned backend",
            );
        }
    }

    if let Ok(mut child_guard) = state.child.lock() {
        if let Some(mut child) = child_guard.take() {
            let child_pid = child.id();
            match wait_for_child_exit(&mut child, BACKEND_KILL_WAIT_TIMEOUT) {
                Ok(status) => append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("reaped stopped backend child pid={child_pid}; status={status}"),
                ),
                Err(error) => append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("failed to reap stopped backend child pid={child_pid}: {error}"),
                ),
            }
        }
    } else {
        append_desktop_bootstrap_log(
            &state.data_dir,
            "failed to lock managed backend child after successful stop; continuing final Desktop exit",
        );
    }

    state.force_exit.store(true, Ordering::SeqCst);
    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop lifecycle group shutdown complete; requesting final app exit",
    );
    app.exit(0);
}

fn cancel_desktop_shutdown(app: &AppHandle, state: &BackendState, error: &str) {
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop shutdown cancelled because owned backend/tray stop failed; keeping Desktop alive: {error}"
        ),
    );
    state.shutdown_started.store(false, Ordering::SeqCst);
    if let Some(window) = app.get_window(HOST_WINDOW_LABEL) {
        reveal_host_window(&window);
    }
}

fn start_main_window_handoff(app: &AppHandle, reason: &str) -> tauri::Result<()> {
    let Some(state) = app.try_state::<BackendState>() else {
        return Ok(());
    };

    if state.handoff_completed.load(Ordering::SeqCst) {
        return Ok(());
    }

    let host_window = app
        .get_window(HOST_WINDOW_LABEL)
        .ok_or_else(|| anyhow("missing host window during embedded handoff".to_string()))?;
    if state.handoff_started.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("starting embedded webview handoff; reason={reason}"),
    );

    let overlay_ptr = state
        .launcher_overlay
        .lock()
        .ok()
        .and_then(|mut overlay| overlay.take());
    if let Err(error) = animate_host_window_to_main_size(&host_window, overlay_ptr) {
        state.handoff_started.store(false, Ordering::SeqCst);
        if let Ok(mut overlay) = state.launcher_overlay.lock() {
            if overlay.is_none() {
                *overlay = overlay_ptr;
            }
        }
        return Err(error);
    }
    let _ = host_window.set_background_color(Some(Color(8, 17, 23, 255)));
    let _ = host_window.set_decorations(main_interface_decorations());
    #[cfg(target_os = "macos")]
    {
        let _ = host_window.set_title_bar_style(TitleBarStyle::Overlay);
        let _ = host_window.set_shadow(true);
    }
    let _ = apply_window_effects(&host_window);
    reveal_host_window(&host_window);
    let _ = host_window.set_resizable(true);
    let _ = host_window.set_maximizable(true);
    let _ = host_window.set_min_size(Some(LogicalSize::new(
        TARGET_WINDOW_MIN_WIDTH,
        TARGET_WINDOW_MIN_HEIGHT,
    )));
    prepare_main_webview(app, &host_window);
    let app_handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(WEBVIEW_REVEAL_SETTLE_DELAY);
        reveal_main_webview(&app_handle, &host_window);

        if let Some(overlay_ptr) = overlay_ptr {
            fade_out_launcher_overlay(&app_handle, overlay_ptr);
        }

        if let Some(state) = app_handle.try_state::<BackendState>() {
            state.handoff_completed.store(true, Ordering::SeqCst);
            let _ = app_handle.emit_to(MAIN_WINDOW_LABEL, HANDOFF_COMPLETE_EVENT, ());
            append_desktop_bootstrap_log(
                &state.data_dir,
                "embedded webview handoff completed; native launcher overlay removed",
            );
        }
    });

    Ok(())
}

fn try_start_native_handoff(app: &AppHandle, reason: &str) {
    if !supports_native_launcher() {
        return;
    }

    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    let startup_error_present = state
        .startup_error
        .lock()
        .map(|error| error.is_some())
        .unwrap_or(true);
    if !should_handoff_to_main(
        state.startup_ready.load(Ordering::SeqCst),
        startup_error_present,
        state.main_webview_loaded.load(Ordering::SeqCst),
    ) {
        return;
    }

    let _ = start_main_window_handoff(app, reason);
}

fn should_handoff_to_main(
    startup_ready: bool,
    startup_error_present: bool,
    main_webview_loaded: bool,
) -> bool {
    main_webview_loaded && (startup_ready || startup_error_present)
}

fn restore_host_window(app: &AppHandle) {
    let Some(window) = app.get_window(HOST_WINDOW_LABEL) else {
        return;
    };

    if let Some(state) = app.try_state::<BackendState>() {
        append_desktop_bootstrap_log(
            &state.data_dir,
            "desktop reopen requested on macOS; restoring host window",
        );
    }

    reveal_host_window(&window);
}

fn install_open_request_handlers(app: &AppHandle) {
    register_desktop_deep_links(app);

    let app_handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        handle_open_urls(&app_handle, event.urls());
    });

    match app.deep_link().get_current() {
        Ok(Some(urls)) => handle_open_urls(app, urls),
        Ok(None) => {}
        Err(error) => {
            if let Some(state) = app.try_state::<BackendState>() {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("failed to read current desktop deep link: {error}"),
                );
            }
        }
    }

    handle_initial_cli_open_arguments(app);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn register_desktop_deep_links(app: &AppHandle) {
    if let Err(error) = app.deep_link().register_all() {
        if let Some(state) = app.try_state::<BackendState>() {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("failed to register desktop deep links: {error}"),
            );
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn register_desktop_deep_links(_app: &AppHandle) {}

fn handle_initial_cli_open_arguments(app: &AppHandle) {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    handle_cli_file_open_arguments(app, args, None);
}

fn handle_cli_file_open_arguments<I>(app: &AppHandle, args: I, cwd: Option<PathBuf>)
where
    I: IntoIterator<Item = String>,
{
    let urls = args
        .into_iter()
        .filter_map(|arg| cli_arg_to_bifrost_file_url(&arg, cwd.as_deref()))
        .collect::<Vec<_>>();

    if !urls.is_empty() {
        handle_open_urls(app, urls);
    }
}

fn cli_arg_to_bifrost_file_url(arg: &str, cwd: Option<&Path>) -> Option<tauri::Url> {
    let path = PathBuf::from(arg);
    let is_bifrost_file = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bifrost"));
    if !is_bifrost_file {
        return None;
    }

    let path = if path.is_absolute() {
        path
    } else if let Some(cwd) = cwd {
        cwd.join(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    };

    tauri::Url::from_file_path(path).ok()
}

fn handle_open_urls(app: &AppHandle, urls: Vec<tauri::Url>) {
    for url in urls {
        match parse_open_url(&url) {
            Ok(Some(request)) => dispatch_open_request(app, request),
            Ok(None) => {}
            Err(error) => log_open_request_error(app, &url, error),
        }
    }
}

fn dispatch_open_request(app: &AppHandle, request: DesktopOpenRequest) {
    restore_host_window(app);

    if let Some(state) = app.try_state::<BackendState>() {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("desktop open request received: {request:?}"),
        );

        if let Ok(mut pending) = state.pending_open_requests.lock() {
            pending.push(request.clone());
        }
    }

    if app.get_webview(MAIN_WINDOW_LABEL).is_some() {
        let _ = app.emit_to(MAIN_WINDOW_LABEL, OPEN_REQUEST_EVENT, &request);
    }
}

fn log_open_request_error(app: &AppHandle, url: &tauri::Url, error: OpenRequestParseError) {
    if let Some(state) = app.try_state::<BackendState>() {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("ignored desktop open URL {url}: {error}"),
        );
    }
}

fn reveal_host_window(window: &Window) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

fn animate_host_window_to_main_size(
    window: &Window,
    overlay_ptr: Option<usize>,
) -> tauri::Result<()> {
    let _ = window.set_size(LogicalSize::new(TARGET_WINDOW_WIDTH, TARGET_WINDOW_HEIGHT));
    if let Some(overlay_ptr) = overlay_ptr {
        let _ = native_launcher::set_overlay_progress(window, overlay_ptr, 1.0);
    }
    Ok(())
}

fn prepare_main_webview(app: &AppHandle, host_window: &Window) {
    let Some(webview) = app.get_webview(MAIN_WINDOW_LABEL) else {
        return;
    };

    if let Ok(inner_size) = host_window.inner_size() {
        let _ = webview.set_size(inner_size);
    }
}

fn reveal_main_webview(app: &AppHandle, host_window: &Window) {
    let Some(webview) = app.get_webview(MAIN_WINDOW_LABEL) else {
        return;
    };

    if let Ok(inner_size) = host_window.inner_size() {
        let _ = webview.set_size(inner_size);
    }

    let _ = webview.set_position(LogicalPosition::new(0.0, 0.0));
}

fn fade_out_launcher_overlay(app: &AppHandle, overlay_ptr: usize) {
    let Some(window) = app.get_window(HOST_WINDOW_LABEL) else {
        return;
    };

    for step in (0..OVERLAY_FADE_STEPS).rev() {
        let alpha = f64::from(step) / f64::from(OVERLAY_FADE_STEPS);
        let _ = window.run_on_main_thread({
            let window = window.clone();
            move || {
                let _ = native_launcher::set_overlay_alpha(&window, overlay_ptr, alpha);
            }
        });
        std::thread::sleep(OVERLAY_FADE_STEP_DELAY);
    }

    let _ = window.run_on_main_thread({
        let window = window.clone();
        move || {
            let _ = native_launcher::remove_overlay(&window, overlay_ptr);
        }
    });
}

fn load_desktop_config(config_path: &Path) -> tauri::Result<DesktopConfig> {
    if !config_path.exists() {
        let config = DesktopConfig::default();
        save_desktop_config(config_path, &config)?;
        return Ok(config);
    }

    let content = fs::read_to_string(config_path)
        .map_err(|error| anyhow(format!("failed to read desktop config: {error}")))?;
    serde_json::from_str(&content)
        .map_err(|error| anyhow(format!("failed to parse desktop config: {error}")))
}

fn save_desktop_config(config_path: &Path, config: &DesktopConfig) -> tauri::Result<()> {
    if let Some(config_dir) = config_path.parent() {
        fs::create_dir_all(config_dir)
            .map_err(|error| anyhow(format!("failed to create desktop config dir: {error}")))?;
    }

    let content = serde_json::to_string_pretty(config)
        .map_err(|error| anyhow(format!("failed to encode desktop config: {error}")))?;
    fs::write(config_path, format!("{content}\n"))
        .map_err(|error| anyhow(format!("failed to write desktop config: {error}")))
}

#[cfg(test)]
mod tests;
