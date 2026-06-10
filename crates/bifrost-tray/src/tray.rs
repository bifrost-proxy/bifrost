use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::TrayIconBuilder;

use crate::cli::TrayArgs;
use crate::config::{self, TrayConfig};
use crate::lock::TrayLock;
use crate::menu::{self, MenuItemAction, MenuItemDef};
use crate::runtime::{self, RuntimeInfo, ServiceState};

const STATE_RUNNING: u8 = 0;
const STATE_STOPPED: u8 = 1;
const STATE_DISCONNECTED: u8 = 2;

pub fn run(args: TrayArgs) -> Result<(), String> {
    init_logging(&args.data_dir);

    tracing::info!(
        data_dir = %args.data_dir.display(),
        parent_pid = args.parent_pid,
        "bifrost-tray starting"
    );

    let _lock = TrayLock::acquire(&args.data_dir)?;

    #[cfg(target_os = "macos")]
    let event_loop = {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        let mut event_loop = EventLoopBuilder::new().build();
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        event_loop
    };

    #[cfg(not(target_os = "macos"))]
    let event_loop = EventLoopBuilder::new().build();

    let icon_running = load_icon(false);
    let icon_stopped = load_icon(true);
    let runtime = runtime::read_runtime(&args.runtime_file);
    let state = determine_state(runtime.as_ref(), args.parent_pid);
    let custom_config = load_custom_config_safe(&args.data_dir);
    let data_dir_str = args.data_dir.to_string_lossy().to_string();
    let bin_available = args
        .bifrost_bin
        .as_ref()
        .map(|p| p.exists())
        .unwrap_or(false);

    let menu_items = menu::build_menu(
        runtime.as_ref(),
        state,
        custom_config.as_ref(),
        &data_dir_str,
        bin_available,
    );

    let (tray_menu, mut action_map) = build_native_menu(&menu_items);

    let initial_icon = match state {
        ServiceState::Running => &icon_running,
        _ => &icon_stopped,
    };

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Bifrost")
        .with_icon(initial_icon.clone());

    #[cfg(target_os = "macos")]
    {
        builder = builder.with_icon_as_template(true);
    }

    let tray_icon = builder
        .build()
        .map_err(|e| format!("failed to create tray icon: {e}"))?;

    let should_quit = Arc::new(AtomicBool::new(false));
    let should_reload = Arc::new(AtomicBool::new(false));
    let current_state = Arc::new(AtomicU8::new(match state {
        ServiceState::Running => STATE_RUNNING,
        ServiceState::Stopped => STATE_STOPPED,
        ServiceState::Disconnected => STATE_DISCONNECTED,
    }));

    let poll_quit = should_quit.clone();
    let poll_state = current_state.clone();
    let poll_args = args.clone();
    thread::spawn(move || {
        poll_service_state(&poll_quit, &poll_state, &poll_args);
    });

    let menu_receiver = MenuEvent::receiver().clone();
    let mut last_rendered_state = current_state.load(Ordering::Relaxed);

    event_loop.run(move |event, _, control_flow| {
        *control_flow =
            ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_millis(200));

        if should_quit.load(Ordering::Relaxed) {
            *control_flow = ControlFlow::Exit;
            return;
        }

        // Check state change: update icon + rebuild menu
        let new_state = current_state.load(Ordering::Relaxed);
        let state_changed = new_state != last_rendered_state;
        // ReloadMenu action sets this flag to request a menu rebuild from tray.json
        let reload_requested = should_reload.swap(false, Ordering::Relaxed);

        if state_changed || reload_requested {
            last_rendered_state = new_state;

            if state_changed {
                // Update icon with template flag preserved
                let new_icon = if new_state == STATE_RUNNING {
                    &icon_running
                } else {
                    &icon_stopped
                };
                #[cfg(target_os = "macos")]
                {
                    let _ = tray_icon.set_icon_with_as_template(Some(new_icon.clone()), true);
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = tray_icon.set_icon(Some(new_icon.clone()));
                }

                // Update tooltip
                let tooltip = match new_state {
                    STATE_RUNNING => "Bifrost - Running",
                    STATE_STOPPED => "Bifrost - Stopped",
                    _ => "Bifrost - Disconnected",
                };
                let _ = tray_icon.set_tooltip(Some(tooltip));
            }

            // Rebuild menu to reflect new state (enabled/disabled items + status text)
            let svc_state = match new_state {
                STATE_RUNNING => ServiceState::Running,
                STATE_STOPPED => ServiceState::Stopped,
                _ => ServiceState::Disconnected,
            };
            let rt = runtime::read_runtime(&args.runtime_file);
            let new_custom_config = load_custom_config_safe(&args.data_dir);
            let new_menu_items = menu::build_menu(
                rt.as_ref(),
                svc_state,
                new_custom_config.as_ref(),
                &data_dir_str,
                bin_available,
            );
            let (new_menu, new_action_map) = build_native_menu(&new_menu_items);
            tray_icon.set_menu(Some(Box::new(new_menu)));
            action_map = new_action_map;

            tracing::info!(
                state = new_state,
                reloaded = reload_requested,
                "tray icon and menu updated"
            );
        }

        if let Event::NewEvents(_) = event {
            while let Ok(event) = menu_receiver.try_recv() {
                if let Some(action) = action_map.get(&event.id) {
                    tracing::info!("menu action triggered");
                    execute_action(action, &args, &data_dir_str, &should_quit, &should_reload);
                }
            }
        }
    });
}

fn init_logging(data_dir: &Path) {
    let log_dir = data_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "tray.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Leak the guard so logging persists for the process lifetime
    Box::leak(Box::new(_guard));

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(false)
        .init();
}

fn load_icon(dimmed: bool) -> tray_icon::Icon {
    #[cfg(target_os = "macos")]
    let icon_bytes: &[u8] = include_bytes!("../../../assets/trayTemplate@2x.png");
    #[cfg(target_os = "windows")]
    let icon_bytes: &[u8] = include_bytes!("../../../assets/bifrost.ico");

    let img = image::load_from_memory(icon_bytes).expect("failed to load icon");
    let mut rgba = img.to_rgba8();

    // macOS template icons: pure black + alpha => system renders black/white per theme
    #[cfg(target_os = "macos")]
    for pixel in rgba.pixels_mut() {
        let alpha = pixel[3];
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
        pixel[3] = alpha;
    }

    if dimmed {
        for pixel in rgba.pixels_mut() {
            pixel[3] /= 3;
        }
    }

    let (width, height) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height).expect("failed to create icon")
}

fn determine_state(runtime: Option<&RuntimeInfo>, parent_pid: u32) -> ServiceState {
    match runtime {
        Some(rt) => {
            if runtime::is_process_running(rt.pid) || runtime::is_process_running(parent_pid) {
                ServiceState::Running
            } else {
                ServiceState::Stopped
            }
        }
        None => ServiceState::Disconnected,
    }
}

fn load_custom_config_safe(data_dir: &Path) -> Option<TrayConfig> {
    match config::load_custom_config(data_dir) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("custom menu config error: {e}");
            None
        }
    }
}

fn build_native_menu(items: &[MenuItemDef]) -> (Menu, HashMap<MenuId, MenuItemAction>) {
    let menu = Menu::new();
    let mut map = HashMap::new();

    for item in items {
        if item.label == "-" {
            let _ = menu.append(&PredefinedMenuItem::separator());
            continue;
        }

        let menu_item = MenuItem::new(&item.label, item.enabled, None);
        map.insert(menu_item.id().clone(), item.action.clone());
        let _ = menu.append(&menu_item);
    }

    (menu, map)
}

fn execute_action(
    action: &MenuItemAction,
    args: &TrayArgs,
    _data_dir_str: &str,
    quit_flag: &AtomicBool,
    reload_flag: &AtomicBool,
) {
    match action {
        MenuItemAction::OpenUrl(url) => {
            if let Err(e) = open::that(url) {
                tracing::error!(url = %url, error = %e, "failed to open URL");
            }
        }
        MenuItemAction::CopyText(text) => {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Err(e) = clipboard.set_text(text) {
                    tracing::error!(error = %e, "failed to copy to clipboard");
                }
            }
        }
        MenuItemAction::AdminApi { method, url } => {
            let method = method.clone();
            let url = url.clone();
            thread::spawn(move || {
                let result = match method.to_uppercase().as_str() {
                    "GET" => ureq::get(&url).call(),
                    _ => ureq::post(&url).call(),
                };
                match result {
                    Ok(resp) => {
                        tracing::info!(status = resp.status(), url = %url, "admin API called");
                    }
                    Err(e) => {
                        tracing::error!(url = %url, error = %e, "admin API call failed");
                    }
                }
            });
        }
        MenuItemAction::StartService => {
            let Some(bin) = resolve_bifrost_binary(args) else {
                tracing::error!("cannot find trusted bifrost binary to start service");
                return;
            };
            let data_dir = args.data_dir.to_string_lossy().to_string();
            let port = args.port;
            let extra_args = args.start_args.clone();
            thread::spawn(move || {
                spawn_start(&bin, &data_dir, port, &extra_args);
            });
        }
        MenuItemAction::StopService => {
            let Some(bin) = resolve_bifrost_binary(args) else {
                tracing::error!("cannot find trusted bifrost binary to stop service");
                return;
            };
            let data_dir = args.data_dir.to_string_lossy().to_string();
            thread::spawn(move || {
                spawn_stop(&bin, &data_dir);
            });
        }
        MenuItemAction::RestartService => {
            let Some(bin) = resolve_bifrost_binary(args) else {
                tracing::error!("cannot find trusted bifrost binary to restart service");
                return;
            };
            let data_dir = args.data_dir.to_string_lossy().to_string();
            let port = args.port;
            let extra_args = args.start_args.clone();
            thread::spawn(move || {
                spawn_stop(&bin, &data_dir);
                // Give the old process a moment to release the port/lock.
                thread::sleep(Duration::from_millis(1500));
                spawn_start(&bin, &data_dir, port, &extra_args);
            });
        }
        MenuItemAction::OpenDirectory(path) => {
            if let Err(e) = open::that(path) {
                tracing::error!(path = %path, error = %e, "failed to open directory");
            }
        }
        MenuItemAction::ReloadMenu => {
            tracing::info!("menu reload requested");
            reload_flag.store(true, Ordering::Relaxed);
        }
        MenuItemAction::QuitTray => {
            tracing::info!("quit tray requested");
            quit_flag.store(true, Ordering::Relaxed);
        }
        MenuItemAction::None => {}
    }
}

fn spawn_start(bin: &Path, data_dir: &str, port: Option<u16>, extra_args: &[String]) {
    let mut cmd = std::process::Command::new(bin);
    cmd.env("BIFROST_DATA_DIR", data_dir)
        .env("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT", "1")
        .arg("start")
        .arg("--no-tray")
        .arg("--no-system-proxy");
    if let Some(p) = port {
        cmd.arg("-p").arg(p.to_string());
    }
    for a in extra_args {
        cmd.arg(a);
    }
    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(pid = child.id(), "bifrost service started");
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to start bifrost service");
        }
    }
}

fn spawn_stop(bin: &Path, data_dir: &str) {
    match std::process::Command::new(bin)
        .env("BIFROST_DATA_DIR", data_dir)
        .arg("stop")
        .spawn()
    {
        Ok(child) => {
            tracing::info!(pid = child.id(), "bifrost stop invoked");
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to stop bifrost service");
        }
    }
}

/// Resolve the bifrost binary to use for service control.
///
/// Only trusted sources are honored: the explicit `--bifrost-bin` passed by the
/// launching CLI, falling back to a sibling binary next to this tray helper.
/// The `binary_path` recorded in runtime.json is intentionally NOT trusted, as
/// it is attacker-influenceable if the data dir is writable.
fn resolve_bifrost_binary(args: &TrayArgs) -> Option<std::path::PathBuf> {
    if let Some(bin) = &args.bifrost_bin {
        if bin.exists() {
            return Some(bin.clone());
        }
        tracing::warn!(path = %bin.display(), "--bifrost-bin set but file not found");
    }
    find_bifrost_binary()
}

fn find_bifrost_binary() -> Option<std::path::PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let dir = current_exe.parent()?;
    let sibling = dir.join("bifrost");
    if sibling.exists() {
        return Some(sibling);
    }
    None
}

fn poll_service_state(quit_flag: &AtomicBool, state: &AtomicU8, args: &TrayArgs) {
    loop {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_secs(3));

        let parent_alive = runtime::is_process_running(args.parent_pid);
        let runtime = runtime::read_runtime(&args.runtime_file);
        let service_alive = runtime
            .as_ref()
            .map(|rt| runtime::is_process_running(rt.pid))
            .unwrap_or(false);

        let new_state = if parent_alive || service_alive {
            STATE_RUNNING
        } else if runtime.is_some() {
            STATE_STOPPED
        } else {
            STATE_DISCONNECTED
        };

        let old = state.swap(new_state, Ordering::Relaxed);
        if old != new_state {
            tracing::info!(
                old_state = old,
                new_state = new_state,
                "service state transition detected"
            );
        }
    }
}
