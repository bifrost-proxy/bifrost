mod native_launcher;

use bifrost_core::direct_blocking_reqwest_client_builder;
use bifrost_storage::data_dir as shared_bifrost_data_dir;
use bifrost_tls::{ensure_valid_ca, generate_root_ca, save_root_ca, CertInstaller, CertStatus};
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, Instant, SystemTime};

#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;
#[cfg(target_os = "macos")]
use tauri::window::EffectState;
use tauri::window::{Window, WindowBuilder};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    webview::{Color, WebviewBuilder},
    window::{Effect, EffectsBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Size, State, WebviewUrl,
};

const BACKEND_BIND_HOST: &str = "0.0.0.0";
const BACKEND_ADMIN_HOST: &str = "127.0.0.1";
const DEFAULT_BACKEND_PORT: u16 = 9900;
const MAX_PORT_INCREMENT_ATTEMPTS: u16 = 64;
const HOST_WINDOW_LABEL: &str = "host";
const MAIN_WINDOW_LABEL: &str = "main";
const INITIAL_WINDOW_WIDTH: f64 = 360.0;
const INITIAL_WINDOW_HEIGHT: f64 = 260.0;
const TARGET_WINDOW_WIDTH: f64 = 1440.0;
const TARGET_WINDOW_HEIGHT: f64 = 920.0;
const TARGET_WINDOW_MIN_WIDTH: f64 = 1180.0;
const TARGET_WINDOW_MIN_HEIGHT: f64 = 760.0;
const WINDOW_EXPAND_STEPS: u16 = 10;
const WINDOW_EXPAND_STEP_DELAY: Duration = Duration::from_millis(16);
const OVERLAY_FADE_STEPS: u16 = 8;
const OVERLAY_FADE_STEP_DELAY: Duration = Duration::from_millis(14);
const BACKEND_WATCHDOG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(3);
const WEBVIEW_PARK_OFFSET: f64 = 2000.0;
const WEBVIEW_REVEAL_SETTLE_DELAY: Duration = Duration::from_millis(90);
const HANDOFF_COMPLETE_EVENT: &str = "desktop://handoff-complete";

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

enum BackendPortTransition {
    Rebound(DesktopPortUpdateResponse),
    RestartRequired,
}

struct BackendState {
    binary_path: PathBuf,
    data_dir: PathBuf,
    config_path: PathBuf,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostWindowCloseBehavior {
    HideWindow,
    ShutdownApp,
}

fn main() {
    tauri::Builder::default()
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
        })
        .invoke_handler(tauri::generate_handler![
            get_desktop_runtime,
            update_desktop_proxy_port,
            notify_main_window_ready,
            set_document_edited,
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
                    "desktop setup started; binary_path={} data_dir={} config_dir={}",
                    binary_path.display(),
                    app_data_dir.display(),
                    app_config_dir.display()
                ),
            );
            let config = load_desktop_config(&config_path)?;
            let launcher_only = is_launcher_only_mode();

            app.manage(BackendState {
                binary_path,
                data_dir: app_data_dir,
                config_path,
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
            });

            if supports_native_launcher() {
                if let Some(state) = app.try_state::<BackendState>() {
                    if let Some(overlay_ptr) = native_launcher::install(&host_window)? {
                        native_launcher::start_animation(&host_window, overlay_ptr)?;
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
                    has_visible_windows,
                    ..
                } => {
                    if !has_visible_windows {
                        restore_host_window(app_handle);
                    }
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
            .inner_size(INITIAL_WINDOW_WIDTH, INITIAL_WINDOW_HEIGHT)
            .min_inner_size(INITIAL_WINDOW_WIDTH, INITIAL_WINDOW_HEIGHT)
            .resizable(true)
            .maximizable(true)
            .decorations(false)
            .visible(true)
            .transparent(true)
            .shadow(false)
            .background_color(Color(0, 0, 0, 0));
    } else {
        builder = builder
            .inner_size(TARGET_WINDOW_WIDTH, TARGET_WINDOW_HEIGHT)
            .min_inner_size(TARGET_WINDOW_MIN_WIDTH, TARGET_WINDOW_MIN_HEIGHT)
            .resizable(true)
            .maximizable(true)
            .decorations(true)
            .visible(true)
            .transparent(false)
            .shadow(true)
            .background_color(Color(8, 17, 23, 255));
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
            Size::Logical(LogicalSize::new(
                if supports_native_launcher() {
                    INITIAL_WINDOW_WIDTH
                } else {
                    TARGET_WINDOW_WIDTH
                },
                if supports_native_launcher() {
                    INITIAL_WINDOW_HEIGHT
                } else {
                    TARGET_WINDOW_HEIGHT
                },
            )),
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

fn resolve_desktop_data_dir() -> tauri::Result<PathBuf> {
    Ok(shared_bifrost_data_dir())
}

fn resolve_desktop_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("desktop-config.json")
}

fn ensure_desktop_cert_ready(data_dir: &Path) {
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

fn start_backend(binary_path: &Path, data_dir: &Path, port: u16) -> tauri::Result<Child> {
    let port = port.to_string();
    let stdout_log = open_sidecar_log_file(data_dir, "desktop-sidecar.out.log")?;
    let stderr_log = open_sidecar_log_file(data_dir, "desktop-sidecar.err.log")?;

    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "starting sidecar; binary_path={} data_dir={} port={} stdout_log={} stderr_log={}",
            binary_path.display(),
            data_dir.display(),
            port,
            log_dir(data_dir).join("desktop-sidecar.out.log").display(),
            log_dir(data_dir).join("desktop-sidecar.err.log").display()
        ),
    );

    Command::new(binary_path)
        .args([
            "start",
            "--host",
            BACKEND_BIND_HOST,
            "--port",
            &port,
            "--skip-cert-check",
        ])
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .map_err(|error| anyhow(format!("failed to start backend: {error}")))
}

fn ensure_backend_running(
    binary_path: &Path,
    data_dir: &Path,
    preferred_port: u16,
) -> tauri::Result<(Option<Child>, u16)> {
    append_desktop_bootstrap_log(
        data_dir,
        format!(
            "ensuring backend is running; preferred_port={} data_dir={}",
            preferred_port,
            data_dir.display()
        ),
    );

    if let Some(port) = find_existing_backend_port(data_dir, preferred_port) {
        append_desktop_bootstrap_log(
            data_dir,
            format!("reusing existing backend instance already serving on port {port}"),
        );
        return Ok((None, port));
    }

    cleanup_existing_backend(binary_path, data_dir);

    let (child, port) = launch_backend_on_available_port(binary_path, data_dir, preferred_port)?;
    Ok((Some(child), port))
}

fn launch_backend_on_available_port(
    binary_path: &Path,
    data_dir: &Path,
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

        let child = start_backend(binary_path, data_dir, port)?;
        match wait_for_backend(port, Duration::from_secs(20)) {
            Ok(()) => {
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend became ready at http://{BACKEND_ADMIN_HOST}:{port}"),
                );
                return Ok((child, port));
            }
            Err(error) => {
                append_desktop_bootstrap_log(
                    data_dir,
                    format!("backend failed to become ready on port {port}: {error}"),
                );
                let _ = stop_backend_with_binary(binary_path, data_dir);
                let _ = terminate_child(child);
                if offset == MAX_PORT_INCREMENT_ATTEMPTS {
                    return Err(error);
                }
            }
        }
    }

    Err(anyhow(format!(
        "failed to find an available backend port starting from {preferred_port}"
    )))
}

fn bootstrap_desktop_backend(app: &AppHandle) {
    let Some(state) = app.try_state::<BackendState>() else {
        return;
    };

    append_desktop_bootstrap_log(
        &state.data_dir,
        "desktop backend bootstrap started asynchronously",
    );

    let preferred_port = match state.expected_port.lock() {
        Ok(port) => *port,
        Err(_) => {
            record_startup_error(
                &state,
                "failed to read desktop expected proxy port during startup".to_string(),
            );
            return;
        }
    };

    match ensure_backend_running(&state.binary_path, &state.data_dir, preferred_port) {
        Ok((child, port)) => {
            if let Ok(mut child_guard) = state.child.lock() {
                *child_guard = child;
            }

            if let Ok(mut current_port) = state.port.lock() {
                *current_port = port;
            }

            if let Ok(mut startup_error) = state.startup_error.lock() {
                *startup_error = None;
            }

            state.startup_ready.store(true, Ordering::SeqCst);
            append_desktop_bootstrap_log(
                &state.data_dir,
                format!("desktop backend bootstrap finished; active_port={port}"),
            );
            try_start_native_handoff(app, "backend ready");
            schedule_desktop_cert_ready(&state.data_dir);
        }
        Err(error) => {
            record_startup_error(&state, error.to_string());
            request_desktop_shutdown(app);
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

        if let Some(reason) = poll_managed_backend_exit(&state) {
            attempt_backend_recovery(app, &reason);
            continue;
        }

        let current_port = match state.port.lock() {
            Ok(port) => *port,
            Err(_) => continue,
        };

        if current_port == 0 || probe_backend_health(current_port) {
            continue;
        }

        attempt_backend_recovery(
            app,
            &format!("backend health probe failed on port {current_port}"),
        );
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

    if let Ok(mut child_guard) = state.child.lock() {
        if let Some(child) = child_guard.take() {
            if let Err(error) = terminate_child(child) {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("failed to terminate managed backend child during recovery: {error}"),
                );
            }
        }
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

    match ensure_backend_running(&state.binary_path, &state.data_dir, preferred_port) {
        Ok((child, port)) => {
            if let Ok(mut child_guard) = state.child.lock() {
                *child_guard = child;
            }

            if let Ok(mut current_port) = state.port.lock() {
                *current_port = port;
            }

            if let Ok(mut startup_error) = state.startup_error.lock() {
                *startup_error = None;
            }

            state.startup_ready.store(true, Ordering::SeqCst);
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

fn wait_for_backend(port: u16, timeout: Duration) -> tauri::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_backend_ready(port) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    Err(anyhow(format!(
        "backend did not become ready at http://{BACKEND_ADMIN_HOST}:{port}"
    )))
}

fn is_backend_ready(port: u16) -> bool {
    probe_backend_health(port)
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

fn wait_for_backend_shutdown(port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !probe_backend_health(port) {
            return;
        }

        std::thread::sleep(Duration::from_millis(150));
    }
}

fn is_port_available(port: u16) -> bool {
    TcpListener::bind((BACKEND_BIND_HOST, port)).is_ok()
}

fn has_runtime_marker(data_dir: &Path) -> bool {
    data_dir.join("bifrost.pid").exists() || data_dir.join("runtime.json").exists()
}

fn cleanup_existing_backend(binary_path: &Path, data_dir: &Path) {
    if has_runtime_marker(data_dir) {
        append_desktop_bootstrap_log(
            data_dir,
            format!(
                "found existing backend runtime markers under {}; stopping stale backend",
                data_dir.display()
            ),
        );
        let _ = stop_backend_with_binary(binary_path, data_dir);
    }
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
    let status = Command::new(binary_path)
        .arg("stop")
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| anyhow(format!("failed to stop backend: {error}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow(format!(
            "backend stop command exited with status {status}"
        )))
    }
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

    Command::new(binary_path)
        .arg("stop")
        .env("BIFROST_DATA_DIR", data_dir)
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .map_err(|error| anyhow(format!("failed to spawn backend stop: {error}")))
}

fn terminate_child(mut child: Child) -> tauri::Result<()> {
    let _ = child.kill();
    child
        .wait()
        .map_err(|error| anyhow(format!("failed to wait for backend child: {error}")))?;
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

    if state.handoff_started.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("starting embedded webview handoff; reason={reason}"),
    );

    let host_window = app
        .get_window(HOST_WINDOW_LABEL)
        .ok_or_else(|| anyhow("missing host window during embedded handoff".to_string()))?;
    let overlay_ptr = state
        .launcher_overlay
        .lock()
        .ok()
        .and_then(|mut overlay| overlay.take());
    animate_host_window_to_main_size(&host_window, overlay_ptr)?;
    let _ = host_window.set_background_color(Some(Color(8, 17, 23, 255)));
    let _ = host_window.set_decorations(true);
    #[cfg(target_os = "macos")]
    let _ = host_window.set_shadow(true);
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

    if !state.startup_ready.load(Ordering::SeqCst) {
        return;
    }

    if !state.main_webview_loaded.load(Ordering::SeqCst) {
        return;
    }

    let _ = start_main_window_handoff(app, reason);
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

fn reveal_host_window(window: &Window) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

fn animate_host_window_to_main_size(
    window: &Window,
    overlay_ptr: Option<usize>,
) -> tauri::Result<()> {
    let scale_factor = window.scale_factor()?;
    let start_size = window.outer_size()?.to_logical::<f64>(scale_factor);
    let start_position = window.outer_position()?.to_logical::<f64>(scale_factor);
    let center_x = start_position.x + start_size.width * 0.5;
    let center_y = start_position.y + start_size.height * 0.5;

    for step in 1..=WINDOW_EXPAND_STEPS {
        let progress = f64::from(step) / f64::from(WINDOW_EXPAND_STEPS);
        let eased = 1.0 - (1.0 - progress) * (1.0 - progress);
        let width = lerp(start_size.width, TARGET_WINDOW_WIDTH, eased);
        let height = lerp(start_size.height, TARGET_WINDOW_HEIGHT, eased);
        let x = center_x - width * 0.5;
        let y = center_y - height * 0.5;

        let _ = window.set_size(LogicalSize::new(width, height));
        let _ = window.set_position(LogicalPosition::new(x, y));
        if let Some(overlay_ptr) = overlay_ptr {
            let _ = native_launcher::set_overlay_progress(window, overlay_ptr, eased);
        }
        std::thread::sleep(WINDOW_EXPAND_STEP_DELAY);
    }

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

fn lerp(start: f64, end: f64, progress: f64) -> f64 {
    start + (end - start) * progress
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
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| anyhow(format!("failed to encode desktop config: {error}")))?;
    fs::write(config_path, format!("{content}\n"))
        .map_err(|error| anyhow(format!("failed to write desktop config: {error}")))
}

#[tauri::command]
fn get_desktop_runtime(state: State<'_, BackendState>) -> Result<DesktopRuntimeInfo, String> {
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
    })
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
        return run_result.map_err(|error| error.to_string());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        let _ = edited;
        Ok(())
    }
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
        return Ok(());
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

    if let Ok(mut child_guard) = state.child.lock() {
        if let Some(child) = child_guard.take() {
            if let Err(error) = terminate_child(child) {
                append_desktop_bootstrap_log(
                    &state.data_dir,
                    format!("failed to terminate managed backend child before restart: {error}"),
                );
            }
        }
    }

    if let Err(error) = stop_backend_with_binary(&state.binary_path, &state.data_dir) {
        append_desktop_bootstrap_log(
            &state.data_dir,
            format!("backend stop helper returned before restart: {error}"),
        );
    }

    wait_for_backend_shutdown(current_port, Duration::from_secs(3));

    let (child, actual_port) =
        launch_backend_on_available_port(&state.binary_path, &state.data_dir, expected_port)?;

    if let Ok(mut child_guard) = state.child.lock() {
        *child_guard = Some(child);
    }

    state.startup_ready.store(true, Ordering::SeqCst);

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

fn append_desktop_bootstrap_log(data_dir: &Path, message: impl AsRef<str>) {
    let log_dir = log_dir(data_dir);
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }

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
        begin_backend_recovery, host_window_close_behavior_for_platform, is_server_config_response,
        parse_port_update_response, poll_managed_backend_exit, resolve_desktop_config_path,
        resolve_desktop_data_dir, BackendState, HostWindowCloseBehavior,
    };
    use bifrost_storage::data_dir as shared_bifrost_data_dir;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

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
    fn backend_recovery_guard_prevents_parallel_recovery() {
        let flag = AtomicBool::new(false);
        let state = BackendState {
            binary_path: PathBuf::new(),
            data_dir: PathBuf::new(),
            config_path: PathBuf::new(),
            launcher_only: false,
            expected_port: Mutex::new(0),
            port: Mutex::new(0),
            child: Mutex::new(None),
            shutdown_started: AtomicBool::new(false),
            force_exit: AtomicBool::new(false),
            backend_recovery_in_progress: flag,
            startup_ready: AtomicBool::new(false),
            startup_error: Mutex::new(None),
            main_webview_loaded: AtomicBool::new(false),
            main_window_ready: AtomicBool::new(false),
            handoff_started: AtomicBool::new(false),
            handoff_completed: AtomicBool::new(false),
            launcher_overlay: Mutex::new(None),
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
