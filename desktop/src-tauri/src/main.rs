#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod native_launcher;
mod open_requests;

use bifrost_core::upgrade_progress::{
    read_progress, write_progress, UpgradePhase, UpgradeProgress,
};
use bifrost_core::{cleanup_bifrost_log_dir, direct_blocking_reqwest_client_builder};
use bifrost_storage::data_dir as shared_bifrost_data_dir;
use bifrost_tls::{ensure_valid_ca, generate_root_ca, save_root_ca, CertInstaller, CertStatus};
use open_requests::{parse_open_url, DesktopOpenRequest, OpenRequestParseError};
use serde::{Deserialize, Serialize};
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
const BACKEND_WATCHDOG_FAILURE_THRESHOLD: u8 = 2;
const BACKEND_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const BACKEND_KILL_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_DESKTOP_STARTUP_DEADLINE: Duration = Duration::from_secs(30);
const WEBVIEW_PARK_OFFSET: f64 = 2000.0;
const WEBVIEW_REVEAL_SETTLE_DELAY: Duration = Duration::from_millis(90);
const HANDOFF_COMPLETE_EVENT: &str = "desktop://handoff-complete";
const OPEN_REQUEST_EVENT: &str = "desktop://open-request";
const DESKTOP_CORE_ENV: &str = "BIFROST_DESKTOP_CORE";
const DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV: &str = "BIFROST_DESKTOP_UPGRADE_RELAUNCH_HELPER";
const DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV: &str = "BIFROST_DESKTOP_UPGRADE_RELAUNCH_MARKER";
const DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV: &str = "BIFROST_DESKTOP_UPGRADE_RELAUNCH_TARGET";
const DESKTOP_UPGRADE_RELAUNCH_MARKER_FILE: &str = "desktop-upgrade-relaunch.json";
const DESKTOP_PENDING_INSTALL_FILE: &str = "desktop-upgrade-pending-install.json";
const DESKTOP_UPGRADE_RELAUNCH_SCHEMA_VERSION: u8 = 1;
const DESKTOP_PENDING_INSTALL_SCHEMA_VERSION: u8 = 1;
const DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS: u64 = 10 * 60 * 1000;
const DESKTOP_UPGRADE_RELAUNCH_PROCESS_WAIT: Duration = Duration::from_secs(30);
const DESKTOP_UPGRADE_RELAUNCH_PORT_WAIT: Duration = Duration::from_secs(30);
const SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV: &str =
    "BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL";
const DESKTOP_STARTUP_DEADLINE_MS_ENV: &str = "BIFROST_DESKTOP_STARTUP_DEADLINE_MS";
const DESKTOP_STARTUP_SESSION_ENV: &str = "BIFROST_DESKTOP_STARTUP_SESSION_ID";
const DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV: &str =
    "BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES";
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DesktopUpgradeRelaunchMarker {
    schema_version: u8,
    created_at_ms: u64,
    old_app_pid: u32,
    old_core_pid: Option<u32>,
    proxy_port: u16,
    app_target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_install: Option<PendingDesktopInstall>,
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
                    &PredefinedMenuItem::quit(app, None)?,
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
            // Forward custom Edit menu actions directly to the WebView via eval().
            // We bypass the Tauri event system because emit_to target routing
            // may not match JS-side listen() calls. Instead we dispatch a DOM
            // CustomEvent that the JS layer picks up reliably.
            let action = match event.id().as_ref() {
                "edit-undo" => Some("undo"),
                "edit-redo" => Some("redo"),
                "edit-select-all" => Some("editor.action.selectAll"),
                _ => None,
            };
            if let Some(action) = action {
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
            handle_cli_file_open_arguments(app, args.into_iter().skip(1), Some(PathBuf::from(cwd)));
        }))
    };

    builder
        .invoke_handler(tauri::generate_handler![
            get_desktop_runtime,
            start_desktop_core,
            update_desktop_proxy_port,
            restart_desktop_after_update,
            notify_main_window_ready,
            get_pending_desktop_open_requests,
            set_document_edited,
            open_external_url,
            write_clipboard
        ])
        .setup(|app| {
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
        .envs(desktop_backend_env(data_dir, startup_session_id))
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));
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

fn desktop_upgrade_relaunch_marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DESKTOP_UPGRADE_RELAUNCH_MARKER_FILE)
}

fn desktop_pending_install_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DESKTOP_PENDING_INSTALL_FILE)
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn read_pending_desktop_install(data_dir: &Path) -> Result<Option<PendingDesktopInstall>, String> {
    let marker_path = desktop_pending_install_path(data_dir);
    let content = match fs::read_to_string(&marker_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read deferred desktop installer {}: {error}",
                marker_path.display()
            ))
        }
    };
    let pending: PendingDesktopInstall = serde_json::from_str(&content)
        .map_err(|error| format!("failed to parse deferred desktop installer: {error}"))?;
    if pending.schema_version != DESKTOP_PENDING_INSTALL_SCHEMA_VERSION {
        return Err(format!(
            "unsupported deferred desktop installer schema {}",
            pending.schema_version
        ));
    }
    let fresh = current_time_millis()
        .checked_sub(pending.created_at_ms)
        .map(|age_ms| age_ms <= DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS)
        .unwrap_or(true);
    if !fresh {
        return Err("deferred desktop installer marker is stale".to_string());
    }
    let package = Path::new(&pending.package_path);
    if !package.is_file() {
        return Err(format!(
            "deferred desktop installer is missing: {}",
            package.display()
        ));
    }
    let extension = package
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("msi") && !extension.eq_ignore_ascii_case("exe") {
        return Err(format!(
            "unsupported deferred desktop installer type: {}",
            package.display()
        ));
    }
    if pending.target_version.trim().is_empty() {
        return Err("deferred desktop installer target version is empty".to_string());
    }
    Ok(Some(pending))
}

fn is_upgrade_relaunch_marker_active(marker: &DesktopUpgradeRelaunchMarker, now_ms: u64) -> bool {
    if marker.schema_version != DESKTOP_UPGRADE_RELAUNCH_SCHEMA_VERSION || marker.proxy_port == 0 {
        return false;
    }

    now_ms
        .checked_sub(marker.created_at_ms)
        .map(|age_ms| age_ms <= DESKTOP_UPGRADE_RELAUNCH_STALE_AFTER_MS)
        .unwrap_or(true)
}

fn write_upgrade_relaunch_marker(
    data_dir: &Path,
    marker: &DesktopUpgradeRelaunchMarker,
) -> tauri::Result<PathBuf> {
    fs::create_dir_all(data_dir)
        .map_err(|error| anyhow(format!("failed to create desktop data dir: {error}")))?;
    let marker_path = desktop_upgrade_relaunch_marker_path(data_dir);
    let content = serde_json::to_string_pretty(marker)
        .map_err(|error| anyhow(format!("failed to encode upgrade relaunch marker: {error}")))?;
    fs::write(&marker_path, format!("{content}\n"))
        .map_err(|error| anyhow(format!("failed to write upgrade relaunch marker: {error}")))?;
    Ok(marker_path)
}

fn read_active_upgrade_relaunch_marker(data_dir: &Path) -> Option<DesktopUpgradeRelaunchMarker> {
    let marker_path = desktop_upgrade_relaunch_marker_path(data_dir);
    let content = match fs::read_to_string(&marker_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            append_desktop_bootstrap_log(
                data_dir,
                format!("failed to read desktop upgrade relaunch marker: {error}"),
            );
            return None;
        }
    };

    let marker = match serde_json::from_str::<DesktopUpgradeRelaunchMarker>(&content) {
        Ok(marker) => marker,
        Err(error) => {
            append_desktop_bootstrap_log(
                data_dir,
                format!("discarding invalid desktop upgrade relaunch marker: {error}"),
            );
            let _ = fs::remove_file(&marker_path);
            return None;
        }
    };

    if is_upgrade_relaunch_marker_active(&marker, current_time_millis()) {
        Some(marker)
    } else {
        append_desktop_bootstrap_log(data_dir, "discarding stale desktop upgrade relaunch marker");
        let _ = fs::remove_file(&marker_path);
        None
    }
}

fn clear_upgrade_relaunch_marker(data_dir: &Path) {
    let marker_path = desktop_upgrade_relaunch_marker_path(data_dir);
    if let Err(error) = fs::remove_file(&marker_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            append_desktop_bootstrap_log(
                data_dir,
                format!("failed to clear desktop upgrade relaunch marker: {error}"),
            );
        }
    }
}

fn write_desktop_upgrade_terminal_progress(
    data_dir: &Path,
    phase: UpgradePhase,
    message: &str,
    error: Option<String>,
) {
    let previous = read_progress(data_dir);
    let progress = UpgradeProgress::new(phase, message)
        .with_target(previous.target_version)
        .with_source(Some(
            previous.source.unwrap_or_else(|| "desktop".to_string()),
        ))
        .with_error(error);
    write_progress(data_dir, &progress);
}

fn persist_desktop_upgrade_handoff_failure(data_dir: &Path, message: String) -> String {
    append_desktop_bootstrap_log(
        data_dir,
        format!("desktop upgrade restart handoff failed: {message}"),
    );
    write_desktop_upgrade_terminal_progress(
        data_dir,
        UpgradePhase::Failed,
        "Desktop restart handoff failed",
        Some(message.clone()),
    );
    message
}

fn may_reuse_existing_backend(upgrade_relaunch: Option<&DesktopUpgradeRelaunchMarker>) -> bool {
    upgrade_relaunch.is_none()
}

fn wait_for_upgrade_handoff_release(data_dir: &Path, marker: &DesktopUpgradeRelaunchMarker) {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "waiting for desktop upgrade handoff release; old_app_pid={} old_core_pid={:?} proxy_port={}",
            marker.old_app_pid, marker.old_core_pid, marker.proxy_port
        ),
    );
    let app_exited =
        wait_for_process_exit(marker.old_app_pid, DESKTOP_UPGRADE_RELAUNCH_PROCESS_WAIT);
    if let Some(old_core_pid) = marker.old_core_pid {
        let core_exited =
            wait_for_process_exit(old_core_pid, DESKTOP_UPGRADE_RELAUNCH_PROCESS_WAIT);
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "desktop upgrade process wait complete; app_exited={} core_exited={}",
                app_exited, core_exited
            ),
        );
    } else {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "desktop upgrade process wait complete; app_exited={} core_exited=unknown",
                app_exited
            ),
        );
    }
    let port_released =
        wait_for_backend_shutdown(marker.proxy_port, DESKTOP_UPGRADE_RELAUNCH_PORT_WAIT);
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "desktop upgrade port wait complete; proxy_port={} released={}",
            marker.proxy_port, port_released
        ),
    );
}

fn run_desktop_upgrade_relaunch_helper_from_env() -> bool {
    if !env_flag_enabled(DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV) {
        return false;
    }

    let marker_path = match std::env::var_os(DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV) {
        Some(path) => PathBuf::from(path),
        None => return true,
    };
    let target = match std::env::var_os(DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV) {
        Some(target) => PathBuf::from(target),
        None => return true,
    };
    let data_dir = marker_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let marker = match fs::read_to_string(&marker_path)
        .ok()
        .and_then(|content| serde_json::from_str::<DesktopUpgradeRelaunchMarker>(&content).ok())
    {
        Some(marker) => marker,
        None => return true,
    };

    append_desktop_bootstrap_log(
        &data_dir,
        format!(
            "desktop upgrade relaunch helper started; old_app_pid={} old_core_pid={:?} proxy_port={} target={}",
            marker.old_app_pid,
            marker.old_core_pid,
            marker.proxy_port,
            target.display()
        ),
    );
    wait_for_upgrade_handoff_release(&data_dir, &marker);

    let mut command = relaunch_command_for_target(&target);
    sanitize_desktop_upgrade_relaunch_command(&mut command);
    hide_windows_child_console(&mut command);
    match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => append_desktop_bootstrap_log(
            &data_dir,
            format!(
                "desktop upgrade relaunch helper opened target; pid={}",
                child.id()
            ),
        ),
        Err(error) => {
            let message = format!("desktop upgrade relaunch helper failed to open target: {error}");
            append_desktop_bootstrap_log(&data_dir, &message);
            write_desktop_upgrade_terminal_progress(
                &data_dir,
                UpgradePhase::Failed,
                "Desktop app restart failed",
                Some(message),
            );
        }
    }

    true
}

fn relaunch_command_for_target(target: &Path) -> Command {
    #[cfg(target_os = "macos")]
    {
        if target.extension().and_then(|extension| extension.to_str()) == Some("app") {
            let mut command = Command::new("open");
            command.arg("-n").arg(target);
            return command;
        }
    }

    Command::new(target)
}

fn sanitize_desktop_upgrade_relaunch_command(command: &mut Command) {
    command
        .env_remove(DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV)
        .env_remove(DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV)
        .env_remove(DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV);
}

fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    if pid == 0 {
        return true;
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_is_running(pid) {
            return true;
        }

        std::thread::sleep(Duration::from_millis(150));
    }

    !process_is_running(pid)
}

fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .stdin(Stdio::null())
            .output()
            .map(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }

    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn ensure_backend_running(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    preferred_port: u16,
    upgrade_relaunch: Option<&DesktopUpgradeRelaunchMarker>,
) -> tauri::Result<(Option<Child>, u16)> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "ensuring backend is running; preferred_port={} data_dir={}",
            preferred_port,
            data_dir.display()
        ),
    );

    if may_reuse_existing_backend(upgrade_relaunch) {
        if let Some(port) = find_existing_backend_port(data_dir, preferred_port) {
            append_desktop_bootstrap_log(
                data_dir,
                format!("reusing existing backend instance already serving on port {port}"),
            );
            return Ok((None, port));
        }
    } else if let Some(marker) = upgrade_relaunch {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "desktop upgrade handoff is active; skipping existing backend reuse on port {}",
                marker.proxy_port
            ),
        );
        wait_for_upgrade_handoff_release(data_dir, marker);
    }

    cleanup_existing_backend(binary_path, data_dir)?;

    let (child, port) = launch_backend_on_available_port(
        binary_path,
        data_dir,
        startup_session_id,
        preferred_port,
    )?;
    Ok((Some(child), port))
}

fn launch_backend_on_available_port(
    binary_path: &Path,
    data_dir: &Path,
    startup_session_id: &str,
    preferred_port: u16,
) -> tauri::Result<(Child, u16)> {
    for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }
        if !is_port_available(port) {
            continue;
        }

        let mut child = start_backend(binary_path, data_dir, startup_session_id, port)?;
        match wait_for_backend(&mut child, data_dir, port, Duration::from_secs(20)) {
            Ok(()) => {
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend became ready at http://{BACKEND_ADMIN_HOST}:{port}"),
                );
                return Ok((child, port));
            }
            Err(error) => {
                let should_retry_port = should_retry_backend_candidate(
                    error.kind,
                    is_port_available(port),
                    offset < MAX_PORT_INCREMENT_ATTEMPTS,
                );
                let error_message = format!(
                    "{error}; inspect {}",
                    log_dir(data_dir).join("desktop-sidecar.err.log").display()
                );
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend failed to become ready on port {port}: {error_message}"),
                );
                if let Err(stop_error) = stop_backend_with_binary(binary_path, data_dir) {
                    append_desktop_bootstrap_log(
                        data_dir,
                        format!("backend stop after failed start returned an error: {stop_error}"),
                    );
                }
                let _ = terminate_child(child);
                if should_retry_port {
                    append_desktop_bootstrap_log(
                        data_dir,
                        format!(
                            "backend child exited while port {port} became unavailable; retrying the next candidate port"
                        ),
                    );
                    continue;
                }
                return Err(anyhow(error_message));
            }
        }
    }

    Err(anyhow(format!(
        "failed to find an available backend port starting from {preferred_port}"
    )))
}

fn should_retry_backend_candidate(
    failure_kind: BackendWaitFailureKind,
    port_is_available_after_exit: bool,
    has_more_candidates: bool,
) -> bool {
    failure_kind == BackendWaitFailureKind::ChildExited
        && !port_is_available_after_exit
        && has_more_candidates
}

fn bootstrap_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop backend bootstrap started asynchronously",
    );

    let _ = start_desktop_backend_now(app, "startup");
}

fn desktop_startup_deadline() -> Duration {
    std::env::var(DESKTOP_STARTUP_DEADLINE_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_DESKTOP_STARTUP_DEADLINE)
}

fn startup_deadline_disposition(main_webview_loaded: bool) -> StartupDeadlineDisposition {
    if main_webview_loaded {
        StartupDeadlineDisposition::HandoffToWebview
    } else {
        StartupDeadlineDisposition::ShowNativeError
    }
}

fn publish_startup_ready(state: &BackendState) {
    if let Ok(mut startup_error) = state.startup_error.lock() {
        state.startup_ready.store(true, Ordering::SeqCst);
        *startup_error = None;
        return;
    }

    state.startup_ready.store(true, Ordering::SeqCst);
}

fn record_startup_deadline_error(state: &BackendState, deadline: Duration) -> bool {
    if state.startup_ready.load(Ordering::SeqCst) {
        return false;
    }
    let Ok(mut startup_error) = state.startup_error.lock() else {
        return false;
    };
    if state.startup_ready.load(Ordering::SeqCst) || startup_error.is_some() {
        return false;
    }
    *startup_error = Some(format!(
        "Bifrost core did not finish starting within {} seconds. Check {} and retry.",
        deadline.as_secs_f32(),
        log_dir(&state.data_dir)
            .join("desktop-bootstrap.log")
            .display()
    ));
    true
}

fn schedule_desktop_startup_deadline(app: &AppHandle) {
    if !supports_native_launcher() {
        return;
    }

    let app = app.clone();
    let deadline = desktop_startup_deadline();
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        let Some(state) = app.try_state::<BackendState>() else {
            return;
        };
        if state.handoff_started.load(Ordering::SeqCst)
            || state.handoff_completed.load(Ordering::SeqCst)
        {
            return;
        }

        let startup_ready = state.startup_ready.load(Ordering::SeqCst);
        let webview_loaded = state.main_webview_loaded.load(Ordering::SeqCst);
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!(
                "desktop startup deadline exceeded after {}ms; startup_ready={startup_ready} main_webview_loaded={webview_loaded}",
                deadline.as_millis()
            ),
        );

        if record_startup_deadline_error(&state, deadline) {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop startup deadline recorded a recoverable startup error",
            );
        }

        match startup_deadline_disposition(state.main_webview_loaded.load(Ordering::SeqCst)) {
            StartupDeadlineDisposition::ShowNativeError => {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "desktop startup deadline retained native launcher as an error surface because the embedded webview is not loaded",
                );
                show_native_launcher_startup_error(&app);
            }
            StartupDeadlineDisposition::HandoffToWebview => {
                if let Err(error) = start_main_window_handoff(&app, "desktop startup deadline") {
                    append_desktop_bootstrap_log(
                        &state.data_dir,
                        format!("desktop startup deadline handoff failed: {error}"),
                    );
                }
            }
        }
    });
}

fn show_native_launcher_startup_error(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };
    let Some(host_window) = app.get_window(HOST_WINDOW_LABEL) else {
        append_desktop_bootstrap_log(
            &state.data_dir,
            "failed to show native launcher startup error: missing host window",
        );
        return;
    };
    let overlay_ptr = state
        .launcher_overlay
        .lock()
        .ok()
        .and_then(|overlay| *overlay);
    if let Some(overlay_ptr) = overlay_ptr {
        if let Err(error) = native_launcher::set_overlay_error(&host_window, overlay_ptr) {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("failed to show native launcher startup error: {error}"),
            );
        }
    }
}

struct BackendRecoveryGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for BackendRecoveryGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

fn begin_backend_recovery(state: &BackendState) -> Option<BackendRecoveryGuard<'_>> {
    if state
        .backend_recovery_in_progress
        .swap(true, Ordering::SeqCst)
    {
        return None;
    }

    Some(BackendRecoveryGuard {
        flag: &state.backend_recovery_in_progress,
    })
}

fn monitor_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(&state.data_dir, "desktop backend watchdog started");

    let mut consecutive_health_failures = 0u8;
    loop {
        std::thread::sleep(BACKEND_WATCHDOG_POLL_INTERVAL);

        let Some(state) = app.try_state::<BackendState>() else {
            return;
        };

        if state.shutdown_started.load(Ordering::SeqCst) || state.force_exit.load(Ordering::SeqCst)
        {
            append_desktop_bootstrap_log(
                &state.data_dir,
                "desktop backend watchdog stopped because desktop shutdown is in progress",
            );
            return;
        }

        if state.backend_recovery_in_progress.load(Ordering::SeqCst) {
            consecutive_health_failures = 0;
            continue;
        }

        if let Some(reason) = poll_managed_backend_exit(&state) {
            consecutive_health_failures = 0;
            attempt_backend_recovery(app, &reason);
            continue;
        }

        let current_port = match state.port.lock() {
            Ok(port) => *port,
            Err(_) => continue,
        };

        if current_port == 0 {
            consecutive_health_failures = 0;
            continue;
        }

        if probe_backend_health(current_port) {
            clear_backend_unavailable_after_healthy_probe(
                &state,
                current_port,
                "desktop backend watchdog observed healthy backend",
            );
            consecutive_health_failures = 0;
            continue;
        }

        consecutive_health_failures = consecutive_health_failures.saturating_add(1);
        if consecutive_health_failures < BACKEND_WATCHDOG_FAILURE_THRESHOLD {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!(
                    "desktop backend health probe failed on port {current_port}; waiting for confirmation ({consecutive_health_failures}/{BACKEND_WATCHDOG_FAILURE_THRESHOLD})"
                ),
            );
            continue;
        }
        consecutive_health_failures = 0;
        let managed_backend = state
            .child
            .lock()
            .map(|child| child.is_some())
            .unwrap_or(false);
        let reason = format!("backend health probe failed on port {current_port}");
        if managed_backend {
            attempt_backend_recovery(app, &reason);
        } else {
            mark_backend_unavailable_for_manual_start(&state, &reason);
        }
    }
}

fn poll_managed_backend_exit(state: &BackendState) -> Option<String> {
    let mut child_guard = state.child.lock().ok()?;
    let child = child_guard.as_mut()?;

    match child.try_wait() {
        Ok(Some(status)) => {
            let pid = child.id();
            let _ = child_guard.take();
            Some(format!(
                "managed backend child pid={pid} exited with status {status}"
            ))
        }
        Ok(None) => None,
        Err(error) => {
            let pid = child.id();
            let _ = child_guard.take();
            Some(format!(
                "failed to poll managed backend child pid={pid}: {error}"
            ))
        }
    }
}

fn attempt_backend_recovery(app: &AppHandle, reason: &str) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    if state.shutdown_started.load(Ordering::SeqCst) || state.force_exit.load(Ordering::SeqCst) {
        return;
    }

    let Some(_recovery_guard) = begin_backend_recovery(&state) else {
        return;
    };

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend watchdog triggering recovery; reason={reason}"),
    );
    state.startup_ready.store(false, Ordering::SeqCst);

    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = None;
    }

    if let Err(error) = terminate_managed_backend(&state, "during watchdog recovery") {
        let message = format!(
            "failed to terminate managed backend child during recovery; refusing to start a replacement: {error}"
        );
        record_startup_error(&state, message);
        try_start_native_handoff(app, "backend recovery failed");
        return;
    }

    let preferred_port = match state.expected_port.lock() {
        Ok(port) => *port,
        Err(_) => {
            record_startup_error(
                &state,
                "failed to read desktop expected proxy port during watchdog recovery".to_string(),
            );
            return;
        }
    };

    match ensure_backend_running(
        &state.binary_path,
        &state.data_dir,
        &state.startup_session_id,
        preferred_port,
        None,
    ) {
        Ok((child, port)) => {
            if let Ok(mut child_guard) = state.child.lock() {
                *child_guard = child;
            }

            if let Ok(mut current_port) = state.port.lock() {
                *current_port = port;
            }

            publish_startup_ready(&state);
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("desktop backend watchdog recovery succeeded; active_port={port}"),
            );
            try_start_native_handoff(app, "backend watchdog recovery");
        }
        Err(error) => {
            record_startup_error(&state, format!("desktop watchdog recovery failed: {error}"));
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!(
                    "desktop backend watchdog recovery failed; will retry after {:?}",
                    BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY
                ),
            );
            std::thread::sleep(BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY);
        }
    }
}

fn schedule_desktop_cert_ready(data_dir: &Path) {
    let data_dir = data_dir.to_path_buf();
    std::thread::spawn(move || {
        // Wait briefly so the window and embedded core can settle before any
        // macOS trust prompt interrupts the startup flow.
        std::thread::sleep(Duration::from_secs(2));
        append_desktop_bootstrap_log(
            &data_dir,
            "starting deferred desktop certificate preflight after startup",
        );
        ensure_desktop_cert_ready(&data_dir);
    });
}

fn record_startup_error(state: &BackendState, error: String) {
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend bootstrap failed: {error}"),
    );

    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = Some(error);
    }
}

fn mark_backend_unavailable_for_manual_start(state: &BackendState, reason: &str) {
    let was_ready = state.startup_ready.swap(false, Ordering::SeqCst);
    let mut should_log = was_ready;
    if let Ok(mut startup_error) = state.startup_error.lock() {
        if startup_error.is_none() {
            should_log = true;
        }
        *startup_error = Some(
            "Bifrost service is not running. Start the service from Bifrost Desktop to continue."
                .to_string(),
        );
    }

    if should_log {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("desktop backend requires manual start; reason={reason}"),
        );
    }
}

fn clear_backend_unavailable_if_healthy(state: &BackendState, reason: &str) -> bool {
    let Ok(current_port) = state.port.lock().map(|port| *port) else {
        return false;
    };

    if current_port == 0 || !probe_backend_health(current_port) {
        return false;
    }

    clear_backend_unavailable_after_healthy_probe(state, current_port, reason)
}

fn clear_backend_unavailable_after_healthy_probe(
    state: &BackendState,
    current_port: u16,
    reason: &str,
) -> bool {
    let was_ready = state.startup_ready.swap(true, Ordering::SeqCst);
    let mut should_log = !was_ready;
    if let Ok(mut startup_error) = state.startup_error.lock() {
        if startup_error.is_some() {
            should_log = true;
        }
        *startup_error = None;
    }

    if should_log {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("desktop backend recovered from manual-start state; reason={reason}; active_port={current_port}"),
        );
    }

    true
}

fn desktop_runtime_snapshot(state: &BackendState) -> Result<DesktopRuntimeInfo, String> {
    if !state.startup_ready.load(Ordering::SeqCst)
        || state
            .startup_error
            .lock()
            .map_err(|_| "failed to read desktop startup error".to_string())?
            .is_some()
    {
        clear_backend_unavailable_if_healthy(
            state,
            "desktop runtime snapshot observed healthy backend",
        );
    }

    let expected_port = *state
        .expected_port
        .lock()
        .map_err(|_| "failed to read desktop expected proxy port".to_string())?;
    let port = *state
        .port
        .lock()
        .map_err(|_| "failed to read desktop proxy port".to_string())?;
    let startup_error = state
        .startup_error
        .lock()
        .map_err(|_| "failed to read desktop startup error".to_string())?
        .clone();

    Ok(DesktopRuntimeInfo {
        expected_proxy_port: expected_port,
        proxy_port: port,
        platform: std::env::consts::OS,
        startup_ready: state.startup_ready.load(Ordering::SeqCst),
        startup_error,
        handoff_completed: state.handoff_completed.load(Ordering::SeqCst),
    })
}

fn start_desktop_backend_now(app: &AppHandle, reason: &str) -> Result<DesktopRuntimeInfo, String> {
    let Some(state) = app.try_state::<BackendState>() else {
        return Err("desktop backend state is not available".to_string());
    };

    let _recovery_guard = begin_backend_recovery(&state)
        .ok_or_else(|| "desktop backend start is already in progress".to_string())?;

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend start requested; reason={reason}"),
    );

    state.startup_ready.store(false, Ordering::SeqCst);
    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = None;
    }

    if let Err(error) = terminate_managed_backend(&state, "before manual start") {
        let message = format!(
            "failed to terminate managed backend before manual start; refusing to start a replacement: {error}"
        );
        record_startup_error(&state, message.clone());
        try_start_native_handoff(app, "backend manual start failed");
        return Err(message);
    }

    let preferred_port = *state
        .expected_port
        .lock()
        .map_err(|_| "failed to read desktop expected proxy port".to_string())?;

    let upgrade_relaunch = state
        .upgrade_relaunch
        .lock()
        .ok()
        .and_then(|guard| guard.clone());

    match ensure_backend_running(
        &state.binary_path,
        &state.data_dir,
        &state.startup_session_id,
        preferred_port,
        upgrade_relaunch.as_ref(),
    ) {
        Ok((child, port)) => {
            if let Ok(mut child_guard) = state.child.lock() {
                *child_guard = child;
            }
            if let Ok(mut current_port) = state.port.lock() {
                *current_port = port;
            }
            publish_startup_ready(&state);
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("desktop backend start succeeded; active_port={port} reason={reason}"),
            );
            if upgrade_relaunch.is_some() {
                write_desktop_upgrade_terminal_progress(
                    &state.data_dir,
                    UpgradePhase::Completed,
                    "Desktop app and core update complete",
                    None,
                );
                clear_upgrade_relaunch_marker(&state.data_dir);
                if let Ok(mut marker_guard) = state.upgrade_relaunch.lock() {
                    *marker_guard = None;
                }
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    "desktop upgrade relaunch marker cleared after managed backend start",
                );
            }
            try_start_native_handoff(app, "backend ready");
            schedule_desktop_cert_ready(&state.data_dir);
            desktop_runtime_snapshot(&state)
        }
        Err(error) => {
            let message = error.to_string();
            if upgrade_relaunch.is_some() {
                write_desktop_upgrade_terminal_progress(
                    &state.data_dir,
                    UpgradePhase::Failed,
                    "Desktop app updated but the new core failed to start",
                    Some(message.clone()),
                );
            }
            record_startup_error(&state, message.clone());
            try_start_native_handoff(app, "backend startup failed");
            Err(message)
        }
    }
}

fn wait_for_backend(
    child: &mut Child,
    data_dir: &Path,
    port: u16,
    timeout: Duration,
) -> Result<(), BackendWaitFailure> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let pid = child.id();
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(BackendWaitFailure {
                    kind: BackendWaitFailureKind::ChildExited,
                    message: format!(
                        "backend process pid={pid} exited before becoming ready at http://{BACKEND_ADMIN_HOST}:{port}; status={status}"
                    ),
                });
            }
            Ok(None) => {}
            Err(error) => {
                return Err(BackendWaitFailure {
                    kind: BackendWaitFailureKind::ChildInspection,
                    message: format!(
                        "failed to inspect backend process pid={pid} while waiting for http://{BACKEND_ADMIN_HOST}:{port}: {error}"
                    ),
                });
            }
        }

        if is_backend_ready(port) && runtime_marker_matches_child(data_dir, pid, port) {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(250));
    }

    Err(BackendWaitFailure {
        kind: BackendWaitFailureKind::TimedOut,
        message: format!("backend did not become ready at http://{BACKEND_ADMIN_HOST}:{port}"),
    })
}

fn is_backend_ready(port: u16) -> bool {
    probe_backend_health(port)
}

fn runtime_marker_matches_child(data_dir: &Path, child_pid: u32, port: u16) -> bool {
    let runtime_file = data_dir.join("runtime.json");
    let Ok(content) = fs::read_to_string(runtime_file) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<DesktopRuntimeMarker>(&content) else {
        return false;
    };

    marker.pid == child_pid && marker.port == port
}

fn find_existing_backend_port(data_dir: &Path, preferred_port: u16) -> Option<u16> {
    for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        if port == 0 {
            continue;
        }

        if probe_backend_health(port) {
            append_desktop_bootstrap_log(
                data_dir,
                format!("detected healthy backend candidate on port {port} before spawning"),
            );
            return Some(port);
        }
    }

    None
}

fn probe_backend_health(port: u16) -> bool {
    let Ok(client) = direct_blocking_reqwest_client_builder()
        .timeout(Duration::from_millis(450))
        .build()
    else {
        return false;
    };

    let url = format!("http://{BACKEND_ADMIN_HOST}:{port}/_bifrost/api/proxy/system/support");
    let Ok(response) = client.get(url).send() else {
        return false;
    };

    response.status().is_success()
}

fn wait_for_backend_shutdown(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !probe_backend_health(port) {
            return true;
        }

        std::thread::sleep(Duration::from_millis(150));
    }

    !probe_backend_health(port)
}

fn is_port_available(port: u16) -> bool {
    TcpListener::bind((BACKEND_BIND_HOST, port)).is_ok()
}

fn has_runtime_marker(data_dir: &Path) -> bool {
    data_dir.join("bifrost.pid").exists() || data_dir.join("runtime.json").exists()
}

fn cleanup_existing_backend(binary_path: &Path, data_dir: &Path) -> tauri::Result<()> {
    if has_runtime_marker(data_dir) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "found existing backend runtime markers under {}; stopping stale backend",
                data_dir.display()
            ),
        );
        if let Err(error) = stop_backend_with_binary(binary_path, data_dir) {
            append_desktop_bootstrap_log(
                data_dir,
                format!(
                    "stale backend stop failed; refusing to start a second backend for the same data directory: {error}"
                ),
            );
            return Err(anyhow(format!(
                "failed to stop the stale Bifrost service safely: {error}. Refusing to start another service with the same data directory"
            )));
        }
    }
    Ok(())
}

fn terminate_managed_backend(state: &BackendState, context: &str) -> tauri::Result<()> {
    let child = state
        .child
        .lock()
        .map_err(|_| anyhow(format!("failed to access managed backend child {context}")))?
        .take();
    if let Some(child) = child {
        terminate_child(child).map_err(|error| {
            anyhow(format!(
                "managed backend termination failed {context}: {error}"
            ))
        })?;
    }
    Ok(())
}

fn stop_backend_before_restart(
    binary_path: &Path,
    data_dir: &Path,
    current_port: u16,
    shutdown_timeout: Duration,
) -> tauri::Result<()> {
    if let Err(error) = stop_backend_with_binary(binary_path, data_dir) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "backend stop failed before restart; refusing to start a replacement for the same data directory: {error}"
            ),
        );
        return Err(anyhow(format!(
            "failed to stop the existing Bifrost service safely before restart: {error}. Refusing to start a replacement with the same data directory"
        )));
    }

    if !wait_for_backend_shutdown(current_port, shutdown_timeout) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "backend remained healthy on port {current_port} after stop; refusing to start a replacement for the same data directory"
            ),
        );
        return Err(anyhow(format!(
            "the existing Bifrost service remained healthy on port {current_port} after stop. Refusing to start a replacement with the same data directory"
        )));
    }

    Ok(())
}

fn stop_backend_with_binary(binary_path: &Path, data_dir: &Path) -> tauri::Result<()> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "running synchronous backend stop; binary_path={} data_dir={}",
            binary_path.display(),
            data_dir.display()
        ),
    );
    let mut command = Command::new(binary_path);
    command
        .arg("stop")
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_windows_child_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| anyhow(format!("failed to stop backend: {error}")))?;
    let status = wait_for_child_exit(&mut child, BACKEND_STOP_TIMEOUT).map_err(|error| {
        anyhow(format!(
            "backend stop command did not complete within {}ms: {error}",
            BACKEND_STOP_TIMEOUT.as_millis()
        ))
    })?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow(format!(
            "backend stop command exited with status {status}"
        )))
    }
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    match kill_child_and_wait(child, BACKEND_KILL_WAIT_TIMEOUT) {
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("process pid={} timed out and was killed", child.id()),
        )),
        Err(error) => Err(error),
    }
}

fn kill_child_and_wait(child: &mut Child, timeout: Duration) -> std::io::Result<ExitStatus> {
    child.kill().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("process pid={} could not be killed: {error}", child.id()),
        )
    })?;

    let kill_deadline = Instant::now() + timeout;
    while Instant::now() < kill_deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "process pid={} did not exit within {}ms after kill",
            child.id(),
            timeout.as_millis()
        ),
    ))
}

fn spawn_backend_stop(binary_path: &Path, data_dir: &Path) -> tauri::Result<Child> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "spawning asynchronous backend stop; binary_path={} data_dir={}",
            binary_path.display(),
            data_dir.display()
        ),
    );
    let stdout_log = open_sidecar_log_file(data_dir, "desktop-sidecar.out.log")?;
    let stderr_log = open_sidecar_log_file(data_dir, "desktop-sidecar.err.log")?;

    let mut command = Command::new(binary_path);
    command
        .arg("stop")
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));
    hide_windows_child_console(&mut command);
    command
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn backend stop: {error}")))
}

fn hide_windows_child_console(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = command;
    }
}

fn terminate_child(mut child: Child) -> tauri::Result<()> {
    kill_child_and_wait(&mut child, BACKEND_KILL_WAIT_TIMEOUT).map_err(|error| {
        anyhow(format!(
            "failed to terminate backend child within {}ms: {error}",
            BACKEND_KILL_WAIT_TIMEOUT.as_millis()
        ))
    })?;
    Ok(())
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
        "desktop shutdown requested; hiding window and stopping backend asynchronously",
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

    match spawn_backend_stop(&state.binary_path, &state.data_dir) {
        Ok(child) => {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("spawned backend stop helper pid={}", child.id()),
            );
        }
        Err(error) => {
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("failed to spawn backend stop helper: {error}"),
            );
        }
    }

    let Ok(mut child_guard) = state.child.lock() else {
        state.force_exit.store(true, Ordering::SeqCst);
        app.exit(0);
        return;
    };

    if let Some(child) = child_guard.take() {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!(
                "detached backend child pid={} so desktop UI can exit immediately",
                child.id()
            ),
        );
    }

    state.force_exit.store(true, Ordering::SeqCst);
    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop shutdown handoff complete; requesting final app exit",
    );
    app.exit(0);
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

#[tauri::command]
fn restart_desktop_after_update(app: AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<BackendState>()
        .ok_or_else(|| "desktop backend state is not available".to_string())?;
    let exe = std::env::current_exe().map_err(|error| {
        persist_desktop_upgrade_handoff_failure(
            &state.data_dir,
            format!("failed to resolve current desktop executable: {error}"),
        )
    })?;

    let old_core_pid = state
        .child
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|child| child.id()));
    let proxy_port = state
        .port
        .lock()
        .map(|guard| *guard)
        .unwrap_or(DEFAULT_BACKEND_PORT);
    let pending_install = read_pending_desktop_install(&state.data_dir).map_err(|error| {
        persist_desktop_upgrade_handoff_failure(
            &state.data_dir,
            format!("failed to prepare deferred desktop install: {error}"),
        )
    })?;
    let app_target = desktop_relaunch_target(&exe);
    let marker = DesktopUpgradeRelaunchMarker {
        schema_version: DESKTOP_UPGRADE_RELAUNCH_SCHEMA_VERSION,
        created_at_ms: current_time_millis(),
        old_app_pid: std::process::id(),
        old_core_pid,
        proxy_port,
        app_target: app_target.to_string_lossy().into_owned(),
        pending_install,
    };
    let marker_path = write_upgrade_relaunch_marker(&state.data_dir, &marker).map_err(|error| {
        persist_desktop_upgrade_handoff_failure(
            &state.data_dir,
            format!("failed to prepare desktop upgrade relaunch: {error}"),
        )
    })?;
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop upgrade relaunch marker written; old_app_pid={} old_core_pid={:?} proxy_port={} target={} deferred_install={}",
            marker.old_app_pid,
            marker.old_core_pid,
            marker.proxy_port,
            marker.app_target,
            marker.pending_install.is_some()
        ),
    );
    let helper = spawn_desktop_upgrade_relaunch_helper(&exe, &marker_path, &app_target, &marker)
        .map_err(|error| {
            clear_upgrade_relaunch_marker(&state.data_dir);
            persist_desktop_upgrade_handoff_failure(
                &state.data_dir,
                format!("failed to spawn desktop upgrade relaunch helper: {error}"),
            )
        })?;
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop upgrade relaunch helper spawned; pid={} target={}",
            helper.id(),
            marker.app_target
        ),
    );
    app.exit(0);
    Ok(())
}

fn desktop_relaunch_target(exe: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(app_bundle) = macos_app_bundle_from_exe_path(exe) {
            return app_bundle;
        }
    }

    exe.to_path_buf()
}

fn spawn_desktop_upgrade_relaunch_helper(
    exe: &Path,
    marker_path: &Path,
    target: &Path,
    marker: &DesktopUpgradeRelaunchMarker,
) -> tauri::Result<Child> {
    #[cfg(target_os = "windows")]
    if marker.pending_install.is_some() {
        return spawn_windows_desktop_upgrade_handoff(marker_path, target);
    }

    #[cfg(not(target_os = "windows"))]
    let _ = marker;
    let mut command = Command::new(exe);
    command
        .env(DESKTOP_UPGRADE_RELAUNCH_HELPER_ENV, "1")
        .env(DESKTOP_UPGRADE_RELAUNCH_MARKER_ENV, marker_path)
        .env(DESKTOP_UPGRADE_RELAUNCH_TARGET_ENV, target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_windows_child_console(&mut command);
    command
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn relaunch helper: {error}")))
}

#[cfg(target_os = "windows")]
fn spawn_windows_desktop_upgrade_handoff(
    marker_path: &Path,
    target: &Path,
) -> tauri::Result<Child> {
    let data_dir = marker_path
        .parent()
        .ok_or_else(|| anyhow("desktop upgrade marker has no parent directory".to_string()))?;
    let script_path = data_dir.join(format!(
        ".desktop-upgrade-handoff-{}.ps1",
        std::process::id()
    ));
    fs::write(&script_path, WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT)
        .map_err(|error| anyhow(format!("failed to write Windows upgrade handoff: {error}")))?;
    let mut command = windows_desktop_upgrade_handoff_command(&script_path, marker_path, target);
    hide_windows_child_console(&mut command);
    let result = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn Windows upgrade handoff: {error}")));
    if result.is_err() {
        let _ = fs::remove_file(script_path);
    }
    result
}

#[cfg(any(target_os = "windows", test))]
const WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT: &str = r#"
param([string]$MarkerPath, [string]$TargetPath)
$ErrorActionPreference = "Stop"
$dataDir = Split-Path -Parent $MarkerPath
$progressPath = Join-Path $dataDir "upgrade-progress.json"
$pendingPath = Join-Path $dataDir "desktop-upgrade-pending-install.json"
$bootstrapLog = Join-Path $dataDir "logs\desktop-bootstrap.log"

function Write-BootstrapLog([string]$Message) {
  $logDir = Split-Path -Parent $bootstrapLog
  New-Item -ItemType Directory -Path $logDir -Force | Out-Null
  Add-Content -LiteralPath $bootstrapLog -Value "$(Get-Date -Format o) $Message" -Encoding UTF8
}

function Write-Progress([string]$Phase, [string]$Message, [string]$TargetVersion, [string]$ErrorMessage) {
  $payload = [ordered]@{
    phase = $Phase
    percent = $null
    message = $Message
    target_version = $TargetVersion
    source = "desktop"
    error = if ($ErrorMessage) { $ErrorMessage } else { $null }
    updated_at = (Get-Date).ToUniversalTime().ToString("o")
  }
  $tmpPath = "$progressPath.tmp.$PID"
  $json = $payload | ConvertTo-Json -Compress
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($tmpPath, $json, $utf8NoBom)
  Move-Item -LiteralPath $tmpPath -Destination $progressPath -Force
}

function Wait-ForProcessExit([uint32]$ProcessId, [string]$Label) {
  if ($ProcessId -eq 0) { return }
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  while ([DateTime]::UtcNow -lt $deadline) {
    if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) { return }
    Start-Sleep -Milliseconds 200
  }
  throw "$Label process $ProcessId did not exit within 30 seconds"
}

$targetVersion = $null
try {
  $marker = Get-Content -LiteralPath $MarkerPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($marker.pending_install) { $targetVersion = [string]$marker.pending_install.target_version }
  Wait-ForProcessExit ([uint32]$marker.old_app_pid) "desktop app"
  if ($null -ne $marker.old_core_pid) {
    Wait-ForProcessExit ([uint32]$marker.old_core_pid) "desktop core"
  }

  if ($marker.pending_install) {
    $packagePath = [string]$marker.pending_install.package_path
    if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
      throw "deferred desktop installer is missing: $packagePath"
    }
    Write-Progress "installing" "Installing desktop app after shutdown..." $targetVersion $null
    Write-BootstrapLog "starting deferred desktop installer; target_version=$targetVersion package=$packagePath"
    $extension = [System.IO.Path]::GetExtension($packagePath).ToLowerInvariant()
    if ($extension -eq ".msi") {
      $installerLog = Join-Path $dataDir "logs\desktop-upgrade-installer.log"
      $quotedPackagePath = '"' + $packagePath + '"'
      $quotedInstallerLog = '"' + $installerLog + '"'
      $installerArgs = @("/i", $quotedPackagePath, "/qn", "/norestart", "ALLUSERS=2", "MSIINSTALLPERUSER=1", "/l*v", $quotedInstallerLog)
      $installer = Start-Process -FilePath "msiexec.exe" -ArgumentList $installerArgs -PassThru
    } elseif ($extension -eq ".exe") {
      $installer = Start-Process -FilePath $packagePath -ArgumentList @("/S") -PassThru
    } else {
      throw "unsupported deferred desktop installer type: $packagePath"
    }

    $deadline = [DateTime]::UtcNow.AddMinutes(10)
    while (-not $installer.WaitForExit(30000)) {
      if ([DateTime]::UtcNow -ge $deadline) {
        try { $installer.Kill() } catch {}
        throw "desktop installer timed out after 600 seconds"
      }
      Write-Progress "installing" "Installing desktop app after shutdown..." $targetVersion $null
    }
    if ($installer.ExitCode -notin @(0, 1641, 3010)) {
      throw "desktop installer exited with code $($installer.ExitCode)"
    }
    Remove-Item -LiteralPath $pendingPath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $packagePath -Force -ErrorAction SilentlyContinue
    Write-BootstrapLog "deferred desktop installer completed; target_version=$targetVersion"
  }

  Start-Process -FilePath $TargetPath | Out-Null
  Write-BootstrapLog "desktop upgrade handoff opened target: $TargetPath"
  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
} catch {
  $message = "desktop upgrade handoff failed: $($_.Exception.Message)"
  Remove-Item -LiteralPath $MarkerPath -Force -ErrorAction SilentlyContinue
  Write-Progress "failed" "Desktop app install or restart failed" $targetVersion $message
  Write-BootstrapLog $message
  try { Start-Process -FilePath $TargetPath | Out-Null } catch {}
  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
  exit 1
}
"#;

#[cfg(any(target_os = "windows", test))]
fn windows_desktop_upgrade_handoff_command(
    script_path: &Path,
    marker_path: &Path,
    target: &Path,
) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(script_path)
        .args(["-MarkerPath"])
        .arg(marker_path)
        .arg("-TargetPath")
        .arg(target);
    command
}

#[cfg(target_os = "macos")]
fn macos_app_bundle_from_exe_path(exe_path: &Path) -> Option<PathBuf> {
    exe_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("Bifrost.app"))
        .map(Path::to_path_buf)
}

#[tauri::command]
fn get_desktop_runtime(state: State<'_, BackendState>) -> Result<DesktopRuntimeInfo, String> {
    desktop_runtime_snapshot(&state)
}

#[tauri::command]
fn start_desktop_core(app: AppHandle) -> Result<DesktopRuntimeInfo, String> {
    start_desktop_backend_now(&app, "frontend request")
}

#[tauri::command]
fn update_desktop_proxy_port(
    state: State<'_, BackendState>,
    port: u16,
) -> Result<DesktopRuntimeInfo, String> {
    if port == 0 {
        return Err("proxy port must be greater than 0".to_string());
    }

    {
        let current_expected_port = state
            .expected_port
            .lock()
            .map_err(|_| "failed to access current desktop expected port".to_string())?;
        if *current_expected_port == port {
            let current_port = *state
                .port
                .lock()
                .map_err(|_| "failed to access current desktop port".to_string())?;
            return Ok(DesktopRuntimeInfo {
                expected_proxy_port: port,
                proxy_port: current_port,
                platform: std::env::consts::OS,
                startup_ready: state.startup_ready.load(Ordering::SeqCst),
                startup_error: state
                    .startup_error
                    .lock()
                    .map_err(|_| "failed to read desktop startup error".to_string())?
                    .clone(),
                handoff_completed: state.handoff_completed.load(Ordering::SeqCst),
            });
        }
    }

    let current_port = *state
        .port
        .lock()
        .map_err(|_| "failed to access current desktop port".to_string())?;
    let updated_runtime = match request_backend_port_transition(current_port, port)
        .map_err(|error| error.to_string())?
    {
        BackendPortTransition::Rebound(runtime) => runtime,
        BackendPortTransition::RestartRequired => {
            restart_backend_on_port(&state, current_port, port)
                .map_err(|error| error.to_string())?
        }
    };
    save_desktop_config(&state.config_path, &DesktopConfig { proxy_port: port })
        .map_err(|error| error.to_string())?;

    {
        let mut expected_port = state
            .expected_port
            .lock()
            .map_err(|_| "failed to update desktop expected proxy port".to_string())?;
        *expected_port = port;
    }
    {
        let mut current_port = state
            .port
            .lock()
            .map_err(|_| "failed to update desktop proxy port".to_string())?;
        *current_port = updated_runtime.actual_port;
    }

    Ok(DesktopRuntimeInfo {
        expected_proxy_port: port,
        proxy_port: updated_runtime.actual_port,
        platform: std::env::consts::OS,
        startup_ready: state.startup_ready.load(Ordering::SeqCst),
        startup_error: state
            .startup_error
            .lock()
            .map_err(|_| "failed to read desktop startup error".to_string())?
            .clone(),
        handoff_completed: state.handoff_completed.load(Ordering::SeqCst),
    })
}

#[tauri::command]
fn notify_main_window_ready(app: AppHandle) -> Result<(), String> {
    if !supports_native_launcher() {
        return Ok(());
    }

    let Some(state) = app.try_state::<BackendState>() else {
        return Ok(());
    };

    state.main_window_ready.store(true, Ordering::SeqCst);
    append_desktop_bootstrap_log(
        &state.data_dir,
        "received embedded webview ready handshake from frontend shell",
    );

    start_main_window_handoff(&app, "frontend ready handshake").map_err(|error| error.to_string())
}

#[tauri::command]
fn get_pending_desktop_open_requests(
    state: State<'_, BackendState>,
) -> Result<Vec<DesktopOpenRequest>, String> {
    let mut pending = state
        .pending_open_requests
        .lock()
        .map_err(|_| "failed to read pending desktop open requests".to_string())?;
    Ok(pending.drain(..).collect())
}

#[tauri::command]
fn set_document_edited(app: AppHandle, edited: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let window = app
            .get_window(HOST_WINDOW_LABEL)
            .ok_or_else(|| "host window not found".to_string())?;
        let window_for_main_thread = window.clone();
        let run_result = window.run_on_main_thread(move || unsafe {
            let ns_window: &NSWindow = &*window_for_main_thread
                .ns_window()
                .expect("failed to get ns_window for host window")
                .cast();
            ns_window.setDocumentEdited(edited);
        });
        run_result.map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        let _ = edited;
        Ok(())
    }
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|error| format!("invalid URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" | "mailto" | "bifrost" | "macappstore" => {}
        scheme => return Err(format!("unsupported URL scheme: {scheme}")),
    }

    open::that(parsed.as_str()).map_err(|error| format!("failed to open URL: {error}"))
}

#[tauri::command]
fn write_clipboard(text: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::NSString;

        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns_string = NSString::from_str(&text);
        let ok = unsafe { pb.setString_forType(&ns_string, objc2_app_kit::NSPasteboardTypeString) };
        if !ok {
            return Err("NSPasteboard setString failed".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::io::Write as _;
        #[cfg(target_os = "windows")]
        let mut child = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn clip: {e}"))?;
        #[cfg(target_os = "linux")]
        let mut child = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn xclip: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("failed to write to clipboard process: {e}"))?;
        }
        child
            .wait()
            .map_err(|e| format!("clipboard process failed: {e}"))?;
        Ok(())
    }
}

fn restart_backend_on_port(
    state: &BackendState,
    current_port: u16,
    expected_port: u16,
) -> tauri::Result<DesktopPortUpdateResponse> {
    let _recovery_guard = begin_backend_recovery(state)
        .ok_or_else(|| anyhow("desktop backend recovery is already in progress".to_string()))?;

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "backend did not confirm dynamic port rebind; restarting embedded core on preferred port {expected_port}"
        ),
    );

    state.startup_ready.store(false, Ordering::SeqCst);

    if let Ok(mut startup_error) = state.startup_error.lock() {
        *startup_error = None;
    }

    if let Err(error) = terminate_managed_backend(state, "before port-change restart") {
        let message = format!(
            "failed to terminate managed backend child before restart; refusing to start a replacement: {error}"
        );
        record_startup_error(state, message.clone());
        return Err(anyhow(message));
    }

    if let Err(error) = stop_backend_before_restart(
        &state.binary_path,
        &state.data_dir,
        current_port,
        Duration::from_secs(3),
    ) {
        let message = error.to_string();
        record_startup_error(state, message.clone());
        return Err(anyhow(message));
    }

    let (child, actual_port) = launch_backend_on_available_port(
        &state.binary_path,
        &state.data_dir,
        &state.startup_session_id,
        expected_port,
    )?;

    if let Ok(mut child_guard) = state.child.lock() {
        *child_guard = Some(child);
    }

    publish_startup_ready(state);

    Ok(DesktopPortUpdateResponse {
        expected_port,
        actual_port,
    })
}

fn anyhow(message: String) -> tauri::Error {
    let error: Box<dyn std::error::Error> = Box::new(std::io::Error::other(message));
    tauri::Error::Setup(error.into())
}

fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

fn cleanup_desktop_logs_once(data_dir: &Path) {
    static CLEANED_DIRS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    let log_dir = log_dir(data_dir);
    let should_cleanup = CLEANED_DIRS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut dirs| {
            if dirs.iter().any(|dir| dir == &log_dir) {
                false
            } else {
                dirs.push(log_dir.clone());
                true
            }
        })
        .unwrap_or(true);
    if should_cleanup {
        let _ = cleanup_bifrost_log_dir(&log_dir, DESKTOP_LOG_RETENTION_DAYS);
    }
}

fn append_desktop_bootstrap_log(data_dir: &Path, message: impl AsRef<str>) {
    let log_dir = log_dir(data_dir);
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    cleanup_desktop_logs_once(data_dir);

    let _write_guard = DESKTOP_BOOTSTRAP_LOG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let log_path = log_dir.join("desktop-bootstrap.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };

    let _ = writeln!(file, "[{:?}] {}", SystemTime::now(), message.as_ref());
}

fn open_sidecar_log_file(data_dir: &Path, file_name: &str) -> tauri::Result<fs::File> {
    let log_dir = log_dir(data_dir);
    fs::create_dir_all(&log_dir)
        .map_err(|error| anyhow(format!("failed to create log dir: {error}")))?;
    cleanup_desktop_logs_once(data_dir);

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join(file_name))
        .map_err(|error| anyhow(format!("failed to open {file_name}: {error}")))
}

fn request_backend_port_transition(
    current_port: u16,
    expected_port: u16,
) -> tauri::Result<BackendPortTransition> {
    let client = direct_blocking_reqwest_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| anyhow(format!("failed to build backend rebind client: {error}")))?;
    let url = format!("http://{BACKEND_ADMIN_HOST}:{current_port}/_bifrost/api/config/server");
    let response = client
        .put(url)
        .json(&serde_json::json!({ "port": expected_port }))
        .send()
        .map_err(|error| anyhow(format!("failed to call backend port rebind API: {error}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(anyhow(format!(
            "backend port rebind API failed with status {}: {}",
            status, body
        )));
    }

    let response_body = response.text().map_err(|error| {
        anyhow(format!(
            "failed to read backend port rebind response: {error}"
        ))
    })?;

    if let Some(runtime) = parse_port_update_response(&response_body) {
        return Ok(BackendPortTransition::Rebound(runtime));
    }

    if is_server_config_response(&response_body) {
        return Ok(BackendPortTransition::RestartRequired);
    }

    let actual_port = wait_for_rebound_backend_port(expected_port, Duration::from_secs(2))
        .map_err(|probe_error| {
            anyhow(format!(
                "failed to decode backend port rebind response; fallback probe failed: {probe_error}; body={response_body}"
            ))
        })?;

    Ok(BackendPortTransition::Rebound(DesktopPortUpdateResponse {
        expected_port,
        actual_port,
    }))
}

fn wait_for_rebound_backend_port(expected_port: u16, timeout: Duration) -> tauri::Result<u16> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for offset in 0..=MAX_PORT_INCREMENT_ATTEMPTS {
            let port = expected_port.saturating_add(offset);
            if port == 0 {
                continue;
            }

            if probe_backend_health(port) {
                return Ok(port);
            }
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    Err(anyhow(format!(
        "backend did not become healthy on any port starting from {expected_port}"
    )))
}

fn parse_port_update_response(response_body: &str) -> Option<DesktopPortUpdateResponse> {
    serde_json::from_str::<DesktopPortUpdateResponse>(response_body).ok()
}

fn is_server_config_response(response_body: &str) -> bool {
    serde_json::from_str::<DesktopServerConfigResponse>(response_body)
        .map(|response| {
            response.timeout_secs > 0
                && response.http1_max_header_size > 0
                && response.http2_max_header_list_size > 0
                && response.websocket_handshake_max_header_size > 0
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        append_desktop_bootstrap_log, begin_backend_recovery, clear_backend_unavailable_if_healthy,
        desktop_backend_env, desktop_backend_start_args, desktop_pending_install_path,
        desktop_sidecar_rust_log, desktop_startup_deadline, desktop_startup_session_id,
        desktop_test_allows_multiple_instances, desktop_upgrade_relaunch_marker_path,
        host_window_close_behavior_for_platform, is_server_config_response,
        is_upgrade_relaunch_marker_active, main_interface_decorations_for_platform,
        mark_backend_unavailable_for_manual_start, may_reuse_existing_backend,
        parse_port_update_response, persist_desktop_upgrade_handoff_failure,
        poll_managed_backend_exit, publish_startup_ready, read_active_upgrade_relaunch_marker,
        read_pending_desktop_install, record_startup_deadline_error, relaunch_command_for_target,
        resolve_bifrost_binary_from_env, resolve_desktop_config_path, resolve_desktop_data_dir,
        sanitize_desktop_upgrade_relaunch_command, save_desktop_config,
        should_allow_multiple_instances, should_handoff_to_main, should_retry_backend_candidate,
        startup_deadline_disposition, stop_backend_before_restart, terminate_managed_backend,
        uses_borderless_desktop_chrome_for_platform, wait_for_backend, wait_for_child_exit,
        windows_desktop_upgrade_handoff_command, write_desktop_upgrade_terminal_progress,
        write_upgrade_relaunch_marker, BackendState, BackendWaitFailureKind, DesktopConfig,
        DesktopUpgradeRelaunchMarker, HostWindowCloseBehavior, PendingDesktopInstall,
        StartupDeadlineDisposition, DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES_ENV,
        WINDOWS_DESKTOP_UPGRADE_HANDOFF_SCRIPT,
    };
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
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                );
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
        assert!(env.iter().any(|(key, value)| {
            *key == "RUST_LOG" && value.contains("bifrost_cli::startup=info")
        }));
        assert!(env
            .iter()
            .any(|(key, value)| { *key == "BIFROST_DATA_DIR" && value == &expected_data_dir }));
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
            proxy_port: 9900,
            app_target: "/Applications/Bifrost.app".to_string(),
            pending_install: None,
        };
        assert!(is_upgrade_relaunch_marker_active(&fresh, 10_001));

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
        let pending = PendingDesktopInstall {
            schema_version: 1,
            created_at_ms: super::current_time_millis(),
            package_path: package.to_string_lossy().into_owned(),
            target_version: "0.0.156".to_string(),
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

        let stale = PendingDesktopInstall {
            created_at_ms: 1,
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
            proxy_port: 9900,
            app_target: "/tmp/Bifrost.app".to_string(),
            pending_install: None,
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
            proxy_port: 9900,
            app_target: "/tmp/Bifrost.app".to_string(),
            pending_install: None,
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
    fn active_upgrade_relaunch_marker_disables_existing_backend_reuse() {
        let marker = DesktopUpgradeRelaunchMarker {
            schema_version: 1,
            created_at_ms: super::current_time_millis(),
            old_app_pid: 123,
            old_core_pid: Some(124),
            proxy_port: 9900,
            app_target: "/tmp/Bifrost.app".to_string(),
            pending_install: None,
        };

        assert!(may_reuse_existing_backend(None));
        assert!(!may_reuse_existing_backend(Some(&marker)));
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
    fn external_backend_health_failure_requires_manual_start() {
        let temp_dir =
            std::env::temp_dir().join(format!("bifrost-desktop-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        let state = test_backend_state(temp_dir.clone(), 19900, true, None);

        mark_backend_unavailable_for_manual_start(
            &state,
            "backend health probe failed on port 19900",
        );

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

    #[test]
    fn wait_for_backend_reports_child_exit_without_waiting_for_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
        let port = listener.local_addr().expect("reserved addr").port();
        drop(listener);

        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--list", "--format", "terse"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn short-lived child");
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let started_at = Instant::now();
        let error = wait_for_backend(&mut child, temp_dir.path(), port, Duration::from_secs(5))
            .expect_err("short-lived child should fail readiness wait");

        assert!(
            started_at.elapsed() < Duration::from_secs(2),
            "child exit should short-circuit the readiness timeout"
        );
        assert_eq!(error.kind, BackendWaitFailureKind::ChildExited);
        assert!(error.to_string().contains("exited before becoming ready"));
    }

    #[cfg(unix)]
    #[test]
    fn wait_for_backend_ignores_health_from_unrelated_process() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let stop = Arc::new(AtomicBool::new(false));
        let (port, health_server) = spawn_persistent_health_server(stop.clone());
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 0.2")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn short-lived child");

        let error = wait_for_backend(&mut child, temp_dir.path(), port, Duration::from_secs(3))
            .expect_err("external health server must not satisfy managed child readiness");

        stop.store(true, Ordering::SeqCst);
        health_server.join().expect("health server thread");
        assert_eq!(error.kind, BackendWaitFailureKind::ChildExited);
    }

    #[cfg(unix)]
    #[test]
    fn wait_for_backend_accepts_health_from_matching_runtime_child() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let stop = Arc::new(AtomicBool::new(false));
        let (port, health_server) = spawn_persistent_health_server(stop.clone());
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 3")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn long-lived child");
        fs::write(
            temp_dir.path().join("runtime.json"),
            format!(r#"{{"pid":{},"port":{}}}"#, child.id(), port),
        )
        .expect("write runtime marker");

        wait_for_backend(&mut child, temp_dir.path(), port, Duration::from_secs(3))
            .expect("matching runtime marker should satisfy readiness");

        let _ = child.kill();
        let _ = child.wait();
        stop.store(true, Ordering::SeqCst);
        health_server.join().expect("health server thread");
    }

    #[test]
    fn poll_managed_backend_exit_reports_exited_child() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("spawn test child");
        let _ = child.wait_with_output();

        let state = BackendState {
            binary_path: PathBuf::new(),
            data_dir: PathBuf::new(),
            config_path: PathBuf::new(),
            startup_session_id: "test-session".to_string(),
            launcher_only: false,
            expected_port: Mutex::new(0),
            port: Mutex::new(0),
            child: Mutex::new(Some(
                Command::new("sh")
                    .arg("-c")
                    .arg("exit 0")
                    .spawn()
                    .expect("spawn managed child"),
            )),
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
            upgrade_relaunch: Mutex::new(None),
        };

        {
            let mut child_guard = state.child.lock().expect("child lock");
            let child = child_guard.as_mut().expect("child");
            let _ = child.wait();
        }

        let reason = poll_managed_backend_exit(&state).expect("exited child reason");
        assert!(reason.contains("exited with status"));
        assert!(state.child.lock().expect("child lock").is_none());
        assert!(!state.backend_recovery_in_progress.load(Ordering::SeqCst));
    }
}
