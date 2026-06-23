use std::collections::HashMap;
use std::fs;
#[cfg(target_os = "macos")]
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
#[cfg(target_os = "macos")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Deserialize;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{
    CheckMenuItem, ContextMenu, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem,
    Submenu,
};
use tray_icon::{TrayIconBuilder, TrayIconEvent};

use super::cli::TrayArgs;
use super::config::{self, TrayConfig};
#[cfg(target_os = "macos")]
use super::dashboard::{
    self, DashboardTheme, TrayDashboardBitmap, TrayDashboardHistory, TrayDashboardSnapshot,
    DASHBOARD_HEIGHT, DASHBOARD_WIDTH,
};
use super::lock::TrayLock;
#[cfg(target_os = "macos")]
use super::menu::SystemStatsMenuLines;
use super::menu::{self, MenuEntry, MenuItemAction, MenuItemDef, RuleTarget, SubmenuDef};
use super::runtime::{self, RuntimeInfo, ServiceState};
#[cfg(target_os = "macos")]
use super::system_stats::{self, SystemStatsSampler};

#[cfg(target_os = "macos")]
use image::ImageEncoder;
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyObject, ProtocolObject};
#[cfg(target_os = "macos")]
use objc2::{
    define_class, msg_send, sel, AnyThread, ClassType, DeclaredClass, MainThreadMarker,
    MainThreadOnly,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSAccessibility, NSAppearance, NSBitmapFormat, NSBitmapImageRep, NSCellImagePosition,
    NSDeviceRGBColorSpace, NSImage, NSImageScaling, NSImageView, NSMenu, NSMenuDelegate,
    NSMenuItem, NSStatusBar, NSStatusItem, NSView,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSData, NSObject, NSObjectProtocol, NSSize, NSString};

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct TraySystemStatsConfig {
    enabled: bool,
    items: bifrost_storage::TraySystemStatsItems,
}

#[cfg(target_os = "macos")]
impl TraySystemStatsConfig {
    fn visible(&self) -> bool {
        self.enabled && self.items.any_enabled()
    }
}

const STATE_RUNNING: u8 = 0;
const STATE_STOPPED: u8 = 1;
const STATE_DISCONNECTED: u8 = 2;
const OP_IDLE: u8 = 0;
const OP_STARTING: u8 = 1;
const OP_STOPPING: u8 = 2;
const OP_UPGRADING: u8 = 3;
const OP_START_FAILED: u8 = 4;
const OP_STOP_FAILED: u8 = 5;
const OP_UPGRADE_FAILED: u8 = 6;
/// Sentinel meaning "no download percent available" for the upgrade percent atomic.
const UPGRADE_PERCENT_NONE: u8 = u8::MAX;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MENU_DATA_POLL_INTERVAL: Duration = Duration::from_secs(3);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(3);
const START_READY_TIMEOUT: Duration = Duration::from_secs(15);
const LOG_RETENTION_DAYS: u32 = bifrost_core::DEFAULT_LOG_RETENTION_DAYS;
const MENU_REBUILD_SUPPRESSION_AFTER_CLICK: Duration = Duration::from_secs(3);
#[cfg(target_os = "macos")]
const MENU_STRUCTURAL_STATUS_UPDATE_SUPPRESSION: Duration = Duration::from_secs(2);
const REMOTE_GROUP_FAILURE_BACKOFF: Duration = Duration::from_secs(60);
const SERVICE_IDLE_EXIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const RECENT_RULES_FILE: &str = "tray_recent_rules.json";
const VERSION_CACHE_FILE: &str = "version_cache.json";
const RECENT_RULE_LIMIT: usize = 5;
const TRAY_THREAD_STACK_SIZE: usize = 512 * 1024;
const TRAY_LOG_BUFFERED_LINES_LIMIT: usize = 1024;
const TRAY_UPDATE_CHECK_INITIAL_DELAY: Duration = Duration::from_secs(30);
const TRAY_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const TRAY_UPDATE_CACHE_MAX_AGE_SECS: i64 = 6 * 60 * 60;
const EVENT_LOOP_BACKGROUND_POLL_INTERVAL: Duration = Duration::from_secs(1);
const SYSTEM_STATS_POLL_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(target_os = "macos")]
const SYSTEM_STATS_CONFIG_FALLBACK_RELOAD_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(target_os = "macos")]
const NATIVE_STATS_VIEW_ENV: &str = "BIFROST_TRAY_NATIVE_STATS_VIEW";
#[cfg(target_os = "macos")]
const TRAY_DASHBOARD_ENV: &str = "BIFROST_TRAY_DASHBOARD";

#[derive(Debug, Clone, PartialEq)]
struct MenuDataSnapshot {
    runtime: Option<RuntimeInfo>,
    custom_config: Option<TrayConfig>,
    rules: Vec<menu::TrayRule>,
    recent_rule_targets: Vec<RuleTarget>,
    system_proxy: Option<menu::SystemProxyMenuState>,
    bin_available: bool,
    update_available: Option<String>,
    #[cfg(target_os = "macos")]
    system_stats: Option<SystemStatsMenuLines>,
    #[cfg(target_os = "macos")]
    dashboard: Option<TrayDashboardSnapshot>,
}

#[cfg(target_os = "macos")]
enum DashboardSnapshotUpdate {
    Preserve,
    Set(Option<Box<TrayDashboardSnapshot>>),
}

pub fn run(args: TrayArgs) -> Result<(), String> {
    init_logging(&args.data_dir);

    tracing::info!(
        data_dir = %args.data_dir.display(),
        parent_pid = args.parent_pid,
        "bifrost-tray starting"
    );

    let _lock = TrayLock::acquire(&args.data_dir)?;
    if !crate::commands::tray_launcher::tray_enabled_by_config(&args.data_dir) {
        tracing::info!("tray disabled by config; exiting helper before creating icon");
        remove_own_tray_pid(&args.data_dir);
        return Ok(());
    }

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
    let runtime = runtime_for_menu(&args);
    let state = determine_state(runtime.as_ref(), args.parent_pid);
    let data_dir_str = args.data_dir.to_string_lossy().to_string();

    let initial_menu_data = load_menu_data_snapshot(&args, state, false, false);
    #[cfg(target_os = "macos")]
    let initial_menu_bar_title = menu_bar_stats_title(&initial_menu_data, state);
    let menu_items =
        build_menu_from_snapshot(&initial_menu_data, state, None, false, &data_dir_str, false);

    #[cfg(target_os = "macos")]
    let (native_action_sender, native_action_receiver) = std::sync::mpsc::channel::<MenuId>();
    let mut native_menu = NativeMenuState::new(
        &menu_items,
        #[cfg(target_os = "macos")]
        initial_menu_data.dashboard.as_ref(),
        #[cfg(target_os = "macos")]
        Some(native_action_sender.clone()),
    );
    let mut action_map = native_menu.action_map.clone();

    let initial_icon = match state {
        ServiceState::Running => &icon_running,
        _ => &icon_stopped,
    };

    #[cfg(target_os = "macos")]
    let native_stats_view_enabled = native_stats_view_enabled();
    #[cfg(target_os = "macos")]
    let initial_icon_for_builder = initial_menu_bar_title
        .as_deref()
        .and_then(menu_bar_stats_icon)
        .unwrap_or_else(|| initial_icon.clone());
    #[cfg(not(target_os = "macos"))]
    let initial_icon_for_builder = initial_icon.clone();

    let should_quit = Arc::new(AtomicBool::new(false));
    let should_reload = Arc::new(AtomicBool::new(false));
    let current_operation = Arc::new(AtomicU8::new(OP_IDLE));
    let upgrade_percent = Arc::new(AtomicU8::new(UPGRADE_PERCENT_NONE));
    let current_state = Arc::new(AtomicU8::new(match state {
        ServiceState::Running => STATE_RUNNING,
        ServiceState::Stopped => STATE_STOPPED,
        ServiceState::Disconnected => STATE_DISCONNECTED,
    }));
    let menu_data = Arc::new(Mutex::new(initial_menu_data));
    let menu_data_generation = Arc::new(AtomicU64::new(0));
    #[cfg(target_os = "macos")]
    let system_stats_generation = Arc::new(AtomicU64::new(0));
    #[cfg(target_os = "macos")]
    let native_menu_open_state = Arc::new(NativeMenuOpenState::default());
    let system_proxy_refresh_in_flight = Arc::new(AtomicBool::new(false));

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(native_menu.menu.clone()))
        .with_tooltip(state_tooltip(state))
        .with_icon(initial_icon_for_builder);

    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);

    #[cfg(target_os = "macos")]
    let (tray_icon, mut native_stats_item) = if native_stats_view_enabled {
        match NativeStatsStatusItem::new(
            initial_menu_bar_title.as_deref(),
            &native_menu.menu,
            state_tooltip(state),
            Arc::new(SystemProxyMenuLifecycle {
                args: args.clone(),
                state: current_state.clone(),
                menu_data: menu_data.clone(),
                generation: menu_data_generation.clone(),
                refresh_in_flight: system_proxy_refresh_in_flight.clone(),
                menu_open_state: native_menu_open_state.clone(),
            }),
        ) {
            Some(item) => {
                tracing::info!("native macOS tray stats view enabled as primary status item");
                (None, Some(item))
            }
            None => {
                tracing::warn!(
                    "native macOS tray stats view unavailable; falling back to icon bitmap"
                );
                let tray_icon = builder
                    .build()
                    .map_err(|e| format!("failed to create tray icon: {e}"))?;
                set_menu_bar_stats_indicator(
                    &tray_icon,
                    initial_menu_bar_title.as_deref(),
                    initial_icon,
                );
                (Some(tray_icon), None)
            }
        }
    } else {
        let tray_icon = builder
            .build()
            .map_err(|e| format!("failed to create tray icon: {e}"))?;
        (Some(tray_icon), None)
    };
    #[cfg(not(target_os = "macos"))]
    let tray_icon = Some(
        builder
            .build()
            .map_err(|e| format!("failed to create tray icon: {e}"))?,
    );

    let poll_quit = should_quit.clone();
    let poll_state = current_state.clone();
    let poll_operation = current_operation.clone();
    let poll_upgrade_percent = upgrade_percent.clone();
    let poll_reload = should_reload.clone();
    let poll_data_snapshot = menu_data.clone();
    let poll_data_generation = menu_data_generation.clone();
    let poll_args = args.clone();
    spawn_tray_thread("bifrost-tray-state-poll", move || {
        poll_service_state(
            &poll_quit,
            &poll_state,
            &poll_operation,
            &poll_upgrade_percent,
            &poll_reload,
            &poll_data_snapshot,
            &poll_data_generation,
            &poll_args,
        );
    })
    .map_err(|error| format!("failed to spawn tray state poll thread: {error}"))?;

    let data_quit = should_quit.clone();
    let data_state = current_state.clone();
    let data_args = args.clone();
    let data_snapshot = menu_data.clone();
    let data_generation = menu_data_generation.clone();
    spawn_tray_thread("bifrost-tray-menu-poll", move || {
        poll_menu_data(
            &data_quit,
            &data_state,
            &data_args,
            &data_snapshot,
            &data_generation,
        );
    })
    .map_err(|error| format!("failed to spawn tray menu poll thread: {error}"))?;

    let update_quit = should_quit.clone();
    let update_state = current_state.clone();
    let update_args = args.clone();
    let update_snapshot = menu_data.clone();
    let update_generation = menu_data_generation.clone();
    spawn_tray_thread("bifrost-tray-update-check", move || {
        poll_update_check(
            &update_quit,
            &update_state,
            &update_args,
            &update_snapshot,
            &update_generation,
        );
    })
    .map_err(|error| format!("failed to spawn tray update check thread: {error}"))?;

    #[cfg(target_os = "macos")]
    {
        let stats_quit = should_quit.clone();
        let stats_args = args.clone();
        let stats_snapshot = menu_data.clone();
        let stats_generation = system_stats_generation.clone();
        let stats_menu_open_state = native_menu_open_state.clone();
        spawn_tray_thread("bifrost-tray-system-stats", move || {
            poll_system_stats(
                &stats_quit,
                &stats_args,
                &stats_snapshot,
                &stats_generation,
                &stats_menu_open_state,
            );
        })
        .map_err(|error| format!("failed to spawn tray system stats thread: {error}"))?;
    }

    let menu_receiver = MenuEvent::receiver().clone();
    let tray_receiver = TrayIconEvent::receiver().clone();
    let mut last_rendered_state = current_state.load(Ordering::Relaxed);
    let mut last_rendered_operation = current_operation.load(Ordering::Relaxed);
    let mut last_rendered_data_generation = menu_data_generation.load(Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    let mut last_rendered_menu_bar_title = initial_menu_bar_title;
    #[cfg(target_os = "macos")]
    let mut last_rendered_system_stats_generation = system_stats_generation.load(Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    let mut last_native_menu_lifecycle_generation =
        native_menu_open_state.generation.load(Ordering::Relaxed);
    let mut last_tray_interaction_at: Option<Instant> = None;
    let system_proxy_refresh_state = current_state.clone();
    let system_proxy_refresh_data = menu_data.clone();
    let system_proxy_refresh_generation = menu_data_generation.clone();
    let system_proxy_refresh_args = args.clone();
    let system_proxy_refresh_in_flight_for_event = system_proxy_refresh_in_flight.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow =
            ControlFlow::WaitUntil(std::time::Instant::now() + EVENT_LOOP_BACKGROUND_POLL_INTERVAL);

        if should_quit.load(Ordering::Relaxed) {
            *control_flow = ControlFlow::Exit;
            return;
        }

        let mut action_triggered = false;
        if let Event::NewEvents(_) = event {
            while let Ok(event) = tray_receiver.try_recv() {
                if tray_event_may_open_menu(&event) {
                    last_tray_interaction_at = Some(Instant::now());
                    request_system_proxy_menu_refresh(
                        &system_proxy_refresh_args,
                        &system_proxy_refresh_state,
                        &system_proxy_refresh_data,
                        &system_proxy_refresh_generation,
                        &system_proxy_refresh_in_flight_for_event,
                    );
                }
            }

            while let Ok(event) = menu_receiver.try_recv() {
                if let Some(action) = action_map.get(&event.id) {
                    tracing::info!("menu action triggered");
                    execute_action(
                        action,
                        &args,
                        &should_quit,
                        &should_reload,
                        &current_operation,
                        &menu_data,
                        &menu_data_generation,
                        &system_proxy_refresh_in_flight,
                    );
                    action_triggered = true;
                }
            }

            #[cfg(target_os = "macos")]
            while let Ok(id) = native_action_receiver.try_recv() {
                if let Some(action) = action_map.get(&id) {
                    tracing::info!("native menu action triggered");
                    execute_action(
                        action,
                        &args,
                        &should_quit,
                        &should_reload,
                        &current_operation,
                        &menu_data,
                        &menu_data_generation,
                        &system_proxy_refresh_in_flight,
                    );
                    action_triggered = true;
                }
            }
        }

        // Check state change: update icon + rebuild menu
        let new_state = current_state.load(Ordering::Relaxed);
        clear_completed_operation(&current_operation, new_state);
        let new_operation = current_operation.load(Ordering::Relaxed);
        let state_changed = new_state != last_rendered_state;
        let operation_changed = new_operation != last_rendered_operation;
        let reload_requested = should_reload.swap(false, Ordering::Relaxed);
        let data_generation = menu_data_generation.load(Ordering::Relaxed);
        let data_changed = data_generation != last_rendered_data_generation;
        #[cfg(target_os = "macos")]
        let system_stats_data_generation = system_stats_generation.load(Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        let system_stats_changed =
            system_stats_data_generation != last_rendered_system_stats_generation;
        #[cfg(target_os = "macos")]
        let mut native_menu_lifecycle_changed = false;
        #[cfg(target_os = "macos")]
        {
            let lifecycle_generation = native_menu_open_state.generation.load(Ordering::Relaxed);
            if lifecycle_generation != last_native_menu_lifecycle_generation {
                last_native_menu_lifecycle_generation = lifecycle_generation;
                native_menu_lifecycle_changed = true;
                last_tray_interaction_at = Some(Instant::now());
            }
        }
        #[cfg(target_os = "macos")]
        let menu_currently_open = native_menu_open_state.open.load(Ordering::Relaxed);
        #[cfg(not(target_os = "macos"))]
        let menu_currently_open = false;
        let menu_recently_interacted = last_tray_interaction_at
            .is_some_and(|instant| instant.elapsed() < MENU_REBUILD_SUPPRESSION_AFTER_CLICK)
            || menu_currently_open;
        let svc_state = match new_state {
            STATE_RUNNING => ServiceState::Running,
            STATE_STOPPED => ServiceState::Stopped,
            _ => ServiceState::Disconnected,
        };

        if state_changed {
            last_rendered_state = new_state;

            // Do not replace the native menu from background polling. Replacing
            // the menu object closes the currently open system menu on
            // macOS/Windows, which makes the tray feel impossible to open while
            // data is refreshing. State polling only updates non-menu
            // affordances; explicit reloads/actions rebuild the menu below.
            let new_icon = if new_state == STATE_RUNNING {
                &icon_running
            } else {
                &icon_stopped
            };
            #[cfg(target_os = "macos")]
            {
                if let Some(native_stats_item) = native_stats_item.as_mut() {
                    native_stats_item.set_tooltip(state_code_tooltip(new_state));
                } else if let Some(tray_icon) = tray_icon.as_ref() {
                    let _ = tray_icon.set_icon_with_as_template(Some(new_icon.clone()), true);
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                if let Some(tray_icon) = tray_icon.as_ref() {
                    let _ = tray_icon.set_icon(Some(new_icon.clone()));
                }
            }

            if let Some(tray_icon) = tray_icon.as_ref() {
                let _ = tray_icon.set_tooltip(Some(state_code_tooltip(new_state)));
            }
            tracing::info!(state = new_state, "tray icon state updated");
        }

        let should_refresh_menu = should_refresh_native_menu(
            state_changed,
            operation_changed,
            reload_requested,
            action_triggered,
            data_changed,
        );

        #[cfg(target_os = "macos")]
        if state_changed || data_changed || system_stats_changed || native_menu_lifecycle_changed {
            let snapshot = clone_menu_data_snapshot(&menu_data);
            let new_title = menu_bar_stats_title(&snapshot, svc_state);
            if new_title != last_rendered_menu_bar_title {
                if let Some(native_stats_item) = native_stats_item.as_mut() {
                    let allow_structural_update = !menu_currently_open
                        && !last_tray_interaction_at.is_some_and(|instant| {
                            instant.elapsed() < MENU_STRUCTURAL_STATUS_UPDATE_SUPPRESSION
                        });
                    if native_stats_item.set_title(new_title.as_deref(), allow_structural_update) {
                        last_rendered_menu_bar_title = new_title;
                    }
                } else {
                    if let Some(tray_icon) = tray_icon.as_ref() {
                        set_menu_bar_stats_indicator(
                            tray_icon,
                            new_title.as_deref(),
                            match svc_state {
                                ServiceState::Running => &icon_running,
                                _ => &icon_stopped,
                            },
                        );
                    }
                    last_rendered_menu_bar_title = new_title;
                }
                tracing::debug!("tray menu bar title updated");
            }
            last_rendered_system_stats_generation = system_stats_data_generation;
            #[cfg(target_os = "macos")]
            if should_refresh_dashboard(
                menu_currently_open,
                system_stats_changed,
                native_menu_lifecycle_changed,
                native_menu.dashboard_header.is_some(),
            ) {
                native_menu.refresh_dashboard(snapshot.dashboard.as_ref());
            }
        }

        if should_refresh_menu {
            if menu_currently_open {
                tracing::debug!(
                    state = new_state,
                    operation = new_operation,
                    data_changed = data_changed,
                    "tray menu data changed while native menu is open; delaying refresh"
                );
            } else {
                let snapshot = clone_menu_data_snapshot(&menu_data);
                let upgrade_status =
                    upgrade_status_label(new_operation, upgrade_percent.load(Ordering::Relaxed));
                let status_label = upgrade_status
                    .as_deref()
                    .or_else(|| operation_status_label(new_operation));
                let new_menu_items = build_menu_from_snapshot(
                    &snapshot,
                    svc_state,
                    status_label,
                    operation_busy(new_operation),
                    &data_dir_str,
                    new_operation == OP_UPGRADING,
                );

                if native_menu.refresh_in_place(&new_menu_items) {
                    #[cfg(target_os = "macos")]
                    native_menu.refresh_dashboard(snapshot.dashboard.as_ref());
                    last_rendered_state = new_state;
                    last_rendered_operation = new_operation;
                    last_rendered_data_generation = data_generation;
                    action_map = native_menu.action_map.clone();
                    tracing::info!(
                        state = new_state,
                        operation = new_operation,
                        data_changed = data_changed,
                        reloaded = reload_requested,
                        "tray menu refreshed in place"
                    );
                } else if should_replace_native_menu(
                    state_changed,
                    operation_changed,
                    reload_requested,
                    action_triggered,
                    menu_recently_interacted,
                ) {
                    last_rendered_state = new_state;
                    last_rendered_operation = new_operation;
                    last_rendered_data_generation = data_generation;
                    native_menu = NativeMenuState::new(
                        &new_menu_items,
                        #[cfg(target_os = "macos")]
                        snapshot.dashboard.as_ref(),
                        #[cfg(target_os = "macos")]
                        Some(native_action_sender.clone()),
                    );
                    if let Some(tray_icon) = tray_icon.as_ref() {
                        tray_icon.set_menu(Some(Box::new(native_menu.menu.clone())));
                    }
                    #[cfg(target_os = "macos")]
                    if let Some(native_stats_item) = native_stats_item.as_mut() {
                        native_stats_item.set_menu(&native_menu.menu);
                    }
                    action_map = native_menu.action_map.clone();

                    tracing::info!(
                        state = new_state,
                        operation = new_operation,
                        data_changed = data_changed,
                        reloaded = reload_requested,
                        "tray menu rebuilt"
                    );
                } else {
                    tracing::debug!(
                        state = new_state,
                        operation = new_operation,
                        data_changed = data_changed,
                        "tray menu structure changed while recently interacted; delaying rebuild"
                    );
                }
            }
        }
    });
}

fn init_logging(data_dir: &Path) {
    let log_dir = data_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let _ = bifrost_core::cleanup_bifrost_log_dir(&log_dir, LOG_RETENTION_DAYS);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "tray.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(TRAY_LOG_BUFFERED_LINES_LIMIT)
        .finish(file_appender);

    // Leak the guard so logging persists for the process lifetime
    Box::leak(Box::new(_guard));

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(false)
        .init();
}

#[cfg(target_os = "macos")]
fn menu_bar_stats_title(snapshot: &MenuDataSnapshot, state: ServiceState) -> Option<String> {
    if state != ServiceState::Running {
        return None;
    }
    snapshot
        .system_stats
        .as_ref()
        .map(|stats| stats.menu_bar.clone())
}

#[cfg(target_os = "macos")]
fn set_menu_bar_stats_indicator(
    tray_icon: &tray_icon::TrayIcon,
    title: Option<&str>,
    fallback_icon: &tray_icon::Icon,
) {
    // Render a single template image so the Bifrost icon, separators, and
    // fixed-width metric columns stay aligned as one status item.
    tray_icon.set_title(Some(""));
    let icon = title
        .and_then(menu_bar_stats_icon)
        .unwrap_or_else(|| fallback_icon.clone());
    let _ = tray_icon.set_icon_with_as_template(Some(icon), true);
}

#[cfg(target_os = "macos")]
fn native_stats_view_enabled() -> bool {
    native_stats_view_enabled_from_env(std::env::var(NATIVE_STATS_VIEW_ENV).ok().as_deref())
}

#[cfg(target_os = "macos")]
fn native_stats_view_enabled_from_env(value: Option<&str>) -> bool {
    !matches!(
        value,
        Some("0" | "false" | "FALSE" | "no" | "NO" | "off" | "OFF")
    )
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct NativeMenuOpenState {
    open: AtomicBool,
    generation: AtomicU64,
}

#[cfg(target_os = "macos")]
impl NativeMenuOpenState {
    fn mark_open(&self) {
        self.open.store(true, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn mark_closed(&self) {
        self.open.store(false, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(target_os = "macos")]
trait NativeStatsMenuLifecycle: Send + Sync {
    fn menu_will_open(&self);
    fn menu_did_close(&self);
}

#[cfg(target_os = "macos")]
struct SystemProxyMenuLifecycle {
    args: TrayArgs,
    state: Arc<AtomicU8>,
    menu_data: Arc<Mutex<MenuDataSnapshot>>,
    generation: Arc<AtomicU64>,
    refresh_in_flight: Arc<AtomicBool>,
    menu_open_state: Arc<NativeMenuOpenState>,
}

#[cfg(target_os = "macos")]
impl NativeStatsMenuLifecycle for SystemProxyMenuLifecycle {
    fn menu_will_open(&self) {
        self.menu_open_state.mark_open();
        request_system_proxy_menu_refresh(
            &self.args,
            &self.state,
            &self.menu_data,
            &self.generation,
            &self.refresh_in_flight,
        );
    }

    fn menu_did_close(&self) {
        self.menu_open_state.mark_closed();
    }
}

#[cfg(target_os = "macos")]
struct NativeStatsMenuDelegateIvars {
    lifecycle: Arc<dyn NativeStatsMenuLifecycle>,
}

#[cfg(target_os = "macos")]
struct NativeMenuActionTargetIvars {
    id: MenuId,
    sender: std::rc::Rc<std::sync::mpsc::Sender<MenuId>>,
}

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BifrostNativeStatsMenuDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = NativeStatsMenuDelegateIvars]
    struct NativeStatsMenuDelegate;

    unsafe impl NSObjectProtocol for NativeStatsMenuDelegate {}

    unsafe impl NSMenuDelegate for NativeStatsMenuDelegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().lifecycle.menu_will_open();
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().lifecycle.menu_did_close();
        }
    }
);

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BifrostNativeMenuActionTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = NativeMenuActionTargetIvars]
    struct NativeMenuActionTarget;

    unsafe impl NSObjectProtocol for NativeMenuActionTarget {}

    impl NativeMenuActionTarget {
        #[unsafe(method(fireBifrostNativeMenuAction:))]
        fn fire_bifrost_native_menu_action(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().sender.send(self.ivars().id.clone());
        }
    }
);

#[cfg(target_os = "macos")]
impl NativeStatsMenuDelegate {
    fn new(lifecycle: Arc<dyn NativeStatsMenuLifecycle>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NativeStatsMenuDelegateIvars { lifecycle });
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(target_os = "macos")]
impl NativeMenuActionTarget {
    fn new(
        id: MenuId,
        sender: std::rc::Rc<std::sync::mpsc::Sender<MenuId>>,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NativeMenuActionTargetIvars { id, sender });
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(target_os = "macos")]
/// Native AppKit status item. This object owns AppKit handles and must be
/// created, used, and dropped on the macOS main thread.
struct NativeStatsStatusItem {
    item: Retained<NSStatusItem>,
    menu_delegate: Retained<NativeStatsMenuDelegate>,
    rendered_image: Option<NativeStatsImage>,
    rendered_title: Option<String>,
    render_scratch: Option<MenuBarStatsBitmap>,
    mtm: MainThreadMarker,
    _main_thread_only: PhantomData<std::rc::Rc<()>>,
}

#[cfg(target_os = "macos")]
struct NativeStatsImage {
    image: Retained<NSImage>,
    image_rep: Option<Retained<NSBitmapImageRep>>,
    width: u32,
    height: u32,
}

#[cfg(target_os = "macos")]
impl NativeStatsStatusItem {
    fn new(
        title: Option<&str>,
        menu: &Menu,
        tooltip: &str,
        menu_lifecycle: Arc<dyn NativeStatsMenuLifecycle>,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(1.0);
        let menu_delegate = NativeStatsMenuDelegate::new(menu_lifecycle, mtm);
        let mut native = Self {
            item,
            menu_delegate,
            rendered_image: None,
            rendered_title: None,
            render_scratch: None,
            mtm,
            _main_thread_only: PhantomData,
        };
        native.set_menu(menu);
        native.set_tooltip(tooltip);
        native.set_title(title, true);
        Some(native)
    }

    fn set_menu(&mut self, menu: &Menu) {
        unsafe {
            let ns_menu = menu.ns_menu().cast::<NSMenu>().as_ref();
            if let Some(ns_menu) = ns_menu {
                let delegate = ProtocolObject::from_ref(&*self.menu_delegate);
                ns_menu.setDelegate(Some(delegate));
            }
            self.item.setMenu(ns_menu);
        }
    }

    fn set_tooltip(&mut self, tooltip: &str) {
        if let Some(button) = self.item.button(self.mtm) {
            let tooltip = NSString::from_str(tooltip);
            button.setToolTip(Some(&tooltip));
        }
    }

    fn set_title(&mut self, title: Option<&str>, allow_structural_update: bool) -> bool {
        if self.rendered_title.as_deref() == title {
            return true;
        }
        let Some(bitmap) =
            render_native_menu_bar_status_bitmap_reusing(title, self.render_scratch.take())
        else {
            return false;
        };
        let Some(button) = self.item.button(self.mtm) else {
            self.render_scratch = Some(bitmap);
            return false;
        };
        let structural_update_needed = !matches!(
            self.rendered_image.as_ref(),
            Some(rendered)
                if rendered.width == bitmap.width
                    && rendered.height == bitmap.height
                    && rendered.image_rep.is_some()
        );
        if structural_update_needed && !allow_structural_update {
            self.render_scratch = Some(bitmap);
            return false;
        }

        let (image, image_changed) = match self.rendered_image.as_mut() {
            Some(rendered)
                if rendered.width == bitmap.width
                    && rendered.height == bitmap.height
                    && rendered.image_rep.is_some() =>
            {
                if copy_menu_bar_stats_bitmap_to_image_rep(
                    &bitmap,
                    rendered.image_rep.as_deref().expect("checked image rep"),
                ) {
                    (rendered.image.clone(), false)
                } else {
                    self.rendered_image = None;
                    let Some(rendered) = native_stats_image_from_bitmap(&bitmap)
                        .or_else(|| native_stats_image_from_png(&bitmap))
                    else {
                        return false;
                    };
                    let image = rendered.image.clone();
                    self.rendered_image = Some(rendered);
                    (image, true)
                }
            }
            _ => {
                let Some(rendered) = native_stats_image_from_bitmap(&bitmap)
                    .or_else(|| native_stats_image_from_png(&bitmap))
                else {
                    return false;
                };
                let image = rendered.image.clone();
                self.rendered_image = Some(rendered);
                (image, true)
            }
        };
        let width_points = f64::from(bitmap.width) / 2.0;
        let height_points = f64::from(bitmap.height) / 2.0;
        if image_changed {
            image.setSize(NSSize::new(width_points, height_points));
            image.setTemplate(true);
            self.item.setLength(width_points);
            button.setImage(Some(&image));
            button.setImagePosition(NSCellImagePosition::ImageOnly);
        } else {
            unsafe {
                let _: () = msg_send![&button, setNeedsDisplay: true];
            }
        }
        let label = NSString::from_str(&native_stats_accessibility_label(title));
        button.setAccessibilityLabel(Some(&label));
        self.rendered_title = title.map(ToString::to_string);
        self.render_scratch = Some(bitmap);
        true
    }
}

#[cfg(target_os = "macos")]
impl Drop for NativeStatsStatusItem {
    fn drop(&mut self) {
        if MainThreadMarker::new().is_none() {
            tracing::warn!(
                "native macOS stats status item dropped off main thread; leaving AppKit cleanup to process exit"
            );
            return;
        }
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct MenuBarStatsBitmap {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

#[cfg(target_os = "macos")]
const MENU_BAR_STATS_SEPARATOR_GAP: u32 = 6;
#[cfg(target_os = "macos")]
const MENU_BAR_STATS_SEPARATOR_WIDTH: u32 = 2;

#[cfg(target_os = "macos")]
fn menu_bar_stats_icon(title: &str) -> Option<tray_icon::Icon> {
    let bitmap = render_menu_bar_stats_bitmap(title)?;
    tray_icon::Icon::from_rgba(bitmap.rgba, bitmap.width, bitmap.height).ok()
}

#[cfg(target_os = "macos")]
fn render_menu_bar_stats_bitmap(title: &str) -> Option<MenuBarStatsBitmap> {
    const PADDING_X: u32 = 2;
    const ICON_SIZE: u32 = 32;
    const ICON_GAP: u32 = 5;
    const HEIGHT: u32 = 36;
    const VALUE_BASELINE: i32 = 18;
    const LABEL_BASELINE: i32 = 32;
    const VALUE_FONT_PX: f32 = 18.0;
    const LABEL_FONT_PX: f32 = 9.5;
    const SINGLE_ROW_BASELINE: i32 = 31;
    const SINGLE_ROW_FONT_PX: f32 = 28.0;
    let rows = menu_bar_stats_rows(title);
    if rows.values.is_empty() && rows.labels.is_empty() {
        return None;
    }

    let font = menu_bar_stats_font()?;
    let single_row = rows.labels.is_empty();
    let columns = if single_row {
        menu_bar_stats_columns(font, &rows, SINGLE_ROW_FONT_PX, LABEL_FONT_PX)
    } else {
        menu_bar_stats_columns(font, &rows, VALUE_FONT_PX, LABEL_FONT_PX)
    };
    let text_width = columns
        .last()
        .map(|column| column.x + column.width)
        .unwrap_or(0);
    let text_x = PADDING_X + ICON_SIZE + ICON_GAP;
    let width = (text_x + text_width + PADDING_X).clamp(96, 1400);
    let mut rgba = vec![0_u8; (width * HEIGHT * 4) as usize];
    draw_menu_bar_bifrost_icon(
        &mut rgba,
        width,
        PADDING_X,
        (HEIGHT - ICON_SIZE) / 2,
        ICON_SIZE,
    );
    draw_menu_bar_stats_separators(
        &mut rgba,
        width,
        text_x,
        if single_row { 3 } else { 4 },
        33,
        rows.values.len().max(rows.labels.len()),
        &columns,
    );
    draw_menu_bar_stats_row(
        &mut rgba,
        width,
        text_x,
        if single_row {
            SINGLE_ROW_BASELINE
        } else {
            VALUE_BASELINE
        },
        true,
        &rows.values,
        &columns,
        font,
    );
    if !single_row {
        draw_menu_bar_stats_row(
            &mut rgba,
            width,
            text_x,
            LABEL_BASELINE,
            false,
            &rows.labels,
            &columns,
            font,
        );
    }

    Some(MenuBarStatsBitmap {
        rgba,
        width,
        height: HEIGHT,
    })
}

#[cfg(all(target_os = "macos", test))]
fn render_native_menu_bar_stats_bitmap(title: &str) -> Option<MenuBarStatsBitmap> {
    render_native_menu_bar_status_bitmap_reusing(Some(title), None)
}

#[cfg(target_os = "macos")]
fn render_native_menu_bar_status_bitmap_reusing(
    title: Option<&str>,
    reusable: Option<MenuBarStatsBitmap>,
) -> Option<MenuBarStatsBitmap> {
    const PADDING_X: u32 = 2;
    const ICON_SIZE: u32 = 36;
    const ICON_GAP: u32 = 5;
    const HEIGHT: u32 = 48;
    const VALUE_BASELINE: i32 = 21;
    const LABEL_BASELINE: i32 = 44;
    const VALUE_FONT_PX: f32 = 24.0;
    const LABEL_FONT_PX: f32 = 14.0;

    let rows = title
        .map(native_menu_bar_stats_rows)
        .unwrap_or_else(|| MenuBarStatsRows {
            values: Vec::new(),
            labels: Vec::new(),
        });
    let font = menu_bar_stats_font();
    let columns = font
        .map(|font| menu_bar_stats_columns(font, &rows, VALUE_FONT_PX, LABEL_FONT_PX))
        .unwrap_or_default();
    let text_width = if rows.values.is_empty() && rows.labels.is_empty() {
        0
    } else {
        columns
            .last()
            .map(|column| column.x + column.width)
            .unwrap_or(0)
    };
    let text_x = PADDING_X + ICON_SIZE + ICON_GAP;
    let width = (text_x + text_width + PADDING_X).clamp(24, 1400);
    let mut rgba = reusable
        .filter(|bitmap| bitmap.width == width && bitmap.height == HEIGHT)
        .map(|mut bitmap| {
            bitmap.rgba.fill(0);
            bitmap.rgba
        })
        .unwrap_or_else(|| vec![0_u8; (width * HEIGHT * 4) as usize]);
    draw_menu_bar_bifrost_icon(
        &mut rgba,
        width,
        PADDING_X,
        (HEIGHT - ICON_SIZE) / 2,
        ICON_SIZE,
    );
    if let Some(font) = font {
        draw_menu_bar_stats_separators(
            &mut rgba,
            width,
            text_x,
            4,
            44,
            rows.values.len().max(rows.labels.len()),
            &columns,
        );
        draw_menu_bar_stats_row(
            &mut rgba,
            width,
            text_x,
            VALUE_BASELINE,
            true,
            &rows.values,
            &columns,
            font,
        );
        draw_menu_bar_stats_row(
            &mut rgba,
            width,
            text_x,
            LABEL_BASELINE,
            false,
            &rows.labels,
            &columns,
            font,
        );
    }

    Some(MenuBarStatsBitmap {
        rgba,
        width,
        height: HEIGHT,
    })
}

#[cfg(target_os = "macos")]
fn native_stats_accessibility_label(title: Option<&str>) -> String {
    title
        .filter(|title| !title.trim().is_empty())
        .map(|title| format!("Bifrost: {}", title.replace('\n', " ")))
        .unwrap_or_else(|| "Bifrost".to_string())
}

#[cfg(target_os = "macos")]
fn encode_menu_bar_stats_png(bitmap: &MenuBarStatsBitmap) -> Option<Vec<u8>> {
    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png);
    encoder
        .write_image(
            &bitmap.rgba,
            bitmap.width,
            bitmap.height,
            image::ColorType::Rgba8.into(),
        )
        .ok()?;
    Some(png)
}

#[cfg(target_os = "macos")]
fn native_stats_image_from_bitmap(bitmap: &MenuBarStatsBitmap) -> Option<NativeStatsImage> {
    let width = bitmap.width as isize;
    let height = bitmap.height as isize;
    let bytes_per_row = (bitmap.width * 4) as isize;
    let bits_per_pixel = 32;
    let bitmap_format =
        NSBitmapFormat::AlphaNonpremultiplied | NSBitmapFormat::ThirtyTwoBitBigEndian;
    let image_rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width,
            height,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            bitmap_format,
            bytes_per_row,
            bits_per_pixel,
        )?
    };
    if !copy_menu_bar_stats_bitmap_to_image_rep(bitmap, &image_rep) {
        return None;
    }
    let image = NSImage::initWithSize(
        NSImage::alloc(),
        NSSize::new(
            f64::from(bitmap.width) / 2.0,
            f64::from(bitmap.height) / 2.0,
        ),
    );
    image.addRepresentation(&image_rep);
    Some(NativeStatsImage {
        image,
        image_rep: Some(image_rep),
        width: bitmap.width,
        height: bitmap.height,
    })
}

#[cfg(target_os = "macos")]
fn native_dashboard_image_from_bitmap(bitmap: &TrayDashboardBitmap) -> Option<NativeStatsImage> {
    let width = bitmap.width as isize;
    let height = bitmap.height as isize;
    let bytes_per_row = (bitmap.width * 4) as isize;
    let bits_per_pixel = 32;
    let bitmap_format =
        NSBitmapFormat::AlphaNonpremultiplied | NSBitmapFormat::ThirtyTwoBitBigEndian;
    let image_rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width,
            height,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            bitmap_format,
            bytes_per_row,
            bits_per_pixel,
        )?
    };
    if !copy_dashboard_bitmap_to_image_rep(bitmap, &image_rep) {
        return None;
    }
    let image = NSImage::initWithSize(
        NSImage::alloc(),
        NSSize::new(
            f64::from(bitmap.width) / 2.0,
            f64::from(bitmap.height) / 2.0,
        ),
    );
    image.addRepresentation(&image_rep);
    Some(NativeStatsImage {
        image,
        image_rep: Some(image_rep),
        width: bitmap.width,
        height: bitmap.height,
    })
}

#[cfg(target_os = "macos")]
fn native_stats_image_from_png(bitmap: &MenuBarStatsBitmap) -> Option<NativeStatsImage> {
    let png = encode_menu_bar_stats_png(bitmap)?;
    let data = NSData::from_vec(png);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    Some(NativeStatsImage {
        image,
        image_rep: None,
        width: bitmap.width,
        height: bitmap.height,
    })
}

#[cfg(target_os = "macos")]
fn copy_dashboard_bitmap_to_image_rep(
    bitmap: &TrayDashboardBitmap,
    image_rep: &NSBitmapImageRep,
) -> bool {
    let bitmap_data = image_rep.bitmapData();
    if bitmap_data.is_null() {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bitmap.rgba.as_ptr(), bitmap_data, bitmap.rgba.len());
    }
    true
}

#[cfg(target_os = "macos")]
fn copy_menu_bar_stats_bitmap_to_image_rep(
    bitmap: &MenuBarStatsBitmap,
    image_rep: &NSBitmapImageRep,
) -> bool {
    let bitmap_data = image_rep.bitmapData();
    if bitmap_data.is_null() {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bitmap.rgba.as_ptr(), bitmap_data, bitmap.rgba.len());
    }
    true
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MenuBarStatsRows {
    values: Vec<String>,
    labels: Vec<String>,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq)]
struct MenuBarStatsColumn {
    x: u32,
    width: u32,
    value_font_px: f32,
    label_font_px: f32,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuBarNetworkDirection {
    Up,
    Down,
}

#[cfg(target_os = "macos")]
fn menu_bar_stats_rows(title: &str) -> MenuBarStatsRows {
    let mut lines = title.lines();
    let values = split_menu_bar_stats_row(lines.next().unwrap_or_default());
    let labels = split_menu_bar_stats_row(lines.next().unwrap_or_default());
    MenuBarStatsRows { values, labels }
}

#[cfg(target_os = "macos")]
fn native_menu_bar_stats_rows(title: &str) -> MenuBarStatsRows {
    let rows = menu_bar_stats_rows(title);
    if !rows.labels.is_empty() {
        return rows;
    }

    let mut values = Vec::with_capacity(rows.values.len());
    let mut labels = Vec::with_capacity(rows.values.len());
    for part in rows.values {
        if let Some(rest) = part.strip_prefix('C') {
            values.push(rest.to_string());
            labels.push("CPU".to_string());
        } else if let Some(rest) = part.strip_prefix('M') {
            values.push(rest.to_string());
            labels.push("MEM".to_string());
        } else if let Some(rest) = part.strip_prefix('D') {
            values.push(rest.to_string());
            labels.push("SSD".to_string());
        } else if part.starts_with('↑') || part.starts_with('↓') {
            let (up, down) = split_native_network_part(&part);
            values.push(up.unwrap_or_default());
            labels.push(down.unwrap_or_default());
        } else {
            values.push(part);
            labels.push(String::new());
        }
    }
    MenuBarStatsRows { values, labels }
}

#[cfg(target_os = "macos")]
fn split_native_network_part(part: &str) -> (Option<String>, Option<String>) {
    let trimmed = part.trim();
    let down_idx = trimmed.find('↓');
    let up_idx = trimmed.find('↑');
    match (up_idx, down_idx) {
        (Some(up), Some(down)) if up < down => {
            let up_text = trimmed[up..down].trim().to_string();
            let down_text = trimmed[down..].trim().to_string();
            (Some(up_text), Some(down_text))
        }
        (Some(up), _) => (Some(trimmed[up..].trim().to_string()), None),
        (_, Some(down)) => (None, Some(trimmed[down..].trim().to_string())),
        _ => (None, None),
    }
}

#[cfg(target_os = "macos")]
fn split_menu_bar_stats_row(row: &str) -> Vec<String> {
    row.split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(target_os = "macos")]
fn menu_bar_stats_columns(
    font: &fontdue::Font,
    rows: &MenuBarStatsRows,
    value_font_px: f32,
    label_font_px: f32,
) -> Vec<MenuBarStatsColumn> {
    let count = rows.values.len().max(rows.labels.len());
    let mut columns = Vec::with_capacity(count);
    let mut x = 0_u32;
    for idx in 0..count {
        let value = rows.values.get(idx);
        let label = rows.labels.get(idx);
        let value_font = value_font_px;
        let label_font = if is_menu_bar_network_column(value, label) {
            value_font_px
        } else {
            label_font_px
        };
        let value_width = rows
            .values
            .get(idx)
            .map(|text| measure_menu_bar_stats_part_width(font, text, value_font))
            .unwrap_or(0);
        let label_width = rows
            .labels
            .get(idx)
            .map(|text| measure_menu_bar_stats_part_width(font, text, label_font))
            .unwrap_or(0);
        let min_width = menu_bar_stats_min_column_width(font, value, label, value_font, label_font);
        let width = value_width.max(label_width).max(min_width);
        columns.push(MenuBarStatsColumn {
            x,
            width,
            value_font_px: value_font,
            label_font_px: label_font,
        });
        x = x
            .saturating_add(width)
            .saturating_add(MENU_BAR_STATS_SEPARATOR_GAP * 2 + MENU_BAR_STATS_SEPARATOR_WIDTH);
    }
    columns
}

#[cfg(target_os = "macos")]
fn is_menu_bar_network_column(value: Option<&String>, label: Option<&String>) -> bool {
    value.is_some_and(|text| text.starts_with('↑') || text.starts_with('↓'))
        || label.is_some_and(|text| text.starts_with('↑') || text.starts_with('↓'))
}

#[cfg(target_os = "macos")]
fn menu_bar_stats_min_column_width(
    font: &fontdue::Font,
    value: Option<&String>,
    label: Option<&String>,
    value_font_px: f32,
    label_font_px: f32,
) -> u32 {
    if value
        .map(|text| text.starts_with('↑') && text.contains('↓'))
        .unwrap_or(false)
    {
        return measure_text_width(font, "↑999.9 M/s ↓999.9 M/s", value_font_px);
    }
    if is_menu_bar_network_column(value, label)
        || value
            .map(|text| text.starts_with('↑') || text.starts_with('↓'))
            .unwrap_or(false)
        || label
            .map(|text| text.starts_with('↑') || text.starts_with('↓'))
            .unwrap_or(false)
    {
        return menu_bar_network_stable_width(font, value_font_px);
    }
    if value
        .map(|text| text == "--%" || text.ends_with('%'))
        .unwrap_or(false)
    {
        return measure_text_width(font, "100%", value_font_px).max(
            label
                .map(|text| measure_text_width(font, text, label_font_px))
                .unwrap_or(0),
        );
    }
    0
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_stats_separators(
    rgba: &mut [u8],
    canvas_width: u32,
    text_x: u32,
    top_y: u32,
    bottom_y: u32,
    column_count: usize,
    columns: &[MenuBarStatsColumn],
) {
    for idx in 0..column_count.saturating_sub(1) {
        let Some(column) = columns.get(idx) else {
            break;
        };
        let x = text_x + column.x + column.width + MENU_BAR_STATS_SEPARATOR_GAP;
        draw_menu_bar_stats_separator(rgba, canvas_width, x, top_y, bottom_y);
    }
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_stats_separator(
    rgba: &mut [u8],
    canvas_width: u32,
    x: u32,
    top_y: u32,
    bottom_y: u32,
) {
    for y in top_y..=bottom_y {
        for dx in 0..MENU_BAR_STATS_SEPARATOR_WIDTH {
            let px = x + dx;
            if px >= canvas_width {
                continue;
            }
            let idx = ((y * canvas_width + px) * 4) as usize;
            if idx + 3 < rgba.len() {
                rgba[idx] = 0;
                rgba[idx + 1] = 0;
                rgba[idx + 2] = 0;
                rgba[idx + 3] = rgba[idx + 3].max(210);
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_menu_bar_stats_row(
    rgba: &mut [u8],
    canvas_width: u32,
    text_x: u32,
    baseline_y: i32,
    is_value_row: bool,
    parts: &[String],
    columns: &[MenuBarStatsColumn],
    font: &fontdue::Font,
) {
    for (idx, part) in parts.iter().enumerate() {
        let Some(column) = columns.get(idx) else {
            break;
        };
        let font_px = if is_value_row {
            column.value_font_px
        } else {
            column.label_font_px
        };
        if let Some(direction) = menu_bar_network_direction(part) {
            draw_menu_bar_network_stats_part(
                rgba,
                canvas_width,
                text_x,
                baseline_y,
                part,
                column,
                direction,
                font_px,
                font,
            );
            continue;
        }
        let part_width = measure_menu_bar_stats_part_width(font, part, font_px);
        let centered_x = column.x + column.width.saturating_sub(part_width) / 2;
        draw_font_text(
            rgba,
            canvas_width,
            (text_x + centered_x) as f32,
            baseline_y,
            font_px,
            part,
            font,
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_menu_bar_network_stats_part(
    rgba: &mut [u8],
    canvas_width: u32,
    text_x: u32,
    baseline_y: i32,
    part: &str,
    column: &MenuBarStatsColumn,
    direction: MenuBarNetworkDirection,
    font_px: f32,
    font: &fontdue::Font,
) {
    let stable_width = menu_bar_network_stable_width(font, font_px);
    let start_x = text_x + column.x + column.width.saturating_sub(stable_width) / 2;
    let arrow_x = start_x as i32;
    let arrow_y = baseline_y - MENU_BAR_NETWORK_ARROW_HEIGHT as i32 - 4;
    let text = menu_bar_network_text_without_arrow(part);
    let text_width = measure_text_width(font, text, font_px);
    let text_x = start_x + stable_width.saturating_sub(text_width);
    draw_menu_bar_network_arrow(rgba, canvas_width, arrow_x, arrow_y, direction);
    draw_font_text(
        rgba,
        canvas_width,
        text_x as f32,
        baseline_y,
        font_px,
        text,
        font,
    );
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_bifrost_icon(rgba: &mut [u8], canvas_width: u32, x: u32, y: u32, icon_size: u32) {
    let Some(icon) = menu_bar_bifrost_icon_alpha(icon_size) else {
        return;
    };
    for row in 0..icon.height {
        for col in 0..icon.width {
            let alpha = icon.alpha[(row * icon.width + col) as usize];
            if alpha == 0 {
                continue;
            }
            let idx = (((y + row) * canvas_width + x + col) * 4) as usize;
            if idx + 3 < rgba.len() {
                rgba[idx] = 0;
                rgba[idx + 1] = 0;
                rgba[idx + 2] = 0;
                rgba[idx + 3] = alpha;
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct MenuBarIconAlpha {
    alpha: Vec<u8>,
    width: u32,
    height: u32,
}

#[cfg(target_os = "macos")]
fn menu_bar_bifrost_icon_alpha(icon_size: u32) -> Option<Arc<MenuBarIconAlpha>> {
    static ICONS: OnceLock<Mutex<HashMap<u32, Option<Arc<MenuBarIconAlpha>>>>> = OnceLock::new();
    let icons = ICONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut icons = icons.lock().ok()?;
    if let Some(icon) = icons.get(&icon_size) {
        return icon.clone();
    }
    let icon = (|| {
        let img =
            image::load_from_memory(include_bytes!("../../../../../assets/trayTemplate@2x.png"))
                .ok()?
                .to_rgba8();
        let resized = image::imageops::resize(
            &img,
            icon_size,
            icon_size,
            image::imageops::FilterType::Lanczos3,
        );
        let alpha = resized.pixels().map(|pixel| pixel[3]).collect();
        Some(Arc::new(MenuBarIconAlpha {
            alpha,
            width: icon_size,
            height: icon_size,
        }))
    })();
    icons.insert(icon_size, icon.clone());
    icon
}

#[cfg(target_os = "macos")]
pub(super) fn menu_bar_stats_font() -> Option<&'static fontdue::Font> {
    static FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/SFNSMono.ttf",
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/SFCompact.ttf",
            "/System/Library/Fonts/HelveticaNeue.ttc",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/System/Library/Fonts/Supplemental/DIN Alternate Bold.ttf",
        ]
        .iter()
        .find_map(|path| {
            let bytes = fs::read(path).ok()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
        })
    })
    .as_ref()
}

#[cfg(target_os = "macos")]
fn measure_text_width(font: &fontdue::Font, text: &str, font_px: f32) -> u32 {
    text.chars()
        .map(|ch| font.metrics(ch, font_px).advance_width)
        .sum::<f32>()
        .ceil() as u32
}

#[cfg(target_os = "macos")]
const MENU_BAR_NETWORK_ARROW_WIDTH: u32 = 9;
#[cfg(target_os = "macos")]
const MENU_BAR_NETWORK_ARROW_HEIGHT: u32 = 13;
#[cfg(target_os = "macos")]
const MENU_BAR_NETWORK_ARROW_GAP: u32 = 3;

#[cfg(target_os = "macos")]
fn menu_bar_network_direction(text: &str) -> Option<MenuBarNetworkDirection> {
    let trimmed = text.trim_start();
    let first = trimmed.chars().next()?;
    let rest = &trimmed[first.len_utf8()..];
    match first {
        '↑' if !rest.contains('↓') => Some(MenuBarNetworkDirection::Up),
        '↓' if !rest.contains('↑') => Some(MenuBarNetworkDirection::Down),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn menu_bar_network_text_without_arrow(text: &str) -> &str {
    let trimmed = text.trim_start();
    if menu_bar_network_direction(trimmed).is_some() {
        let first = trimmed.chars().next().expect("checked direction");
        trimmed[first.len_utf8()..].trim_start()
    } else {
        text
    }
}

#[cfg(target_os = "macos")]
fn measure_menu_bar_stats_part_width(font: &fontdue::Font, text: &str, font_px: f32) -> u32 {
    if menu_bar_network_direction(text).is_some() {
        MENU_BAR_NETWORK_ARROW_WIDTH
            + MENU_BAR_NETWORK_ARROW_GAP
            + measure_text_width(font, menu_bar_network_text_without_arrow(text), font_px)
    } else {
        measure_text_width(font, text, font_px)
    }
}

#[cfg(target_os = "macos")]
fn menu_bar_network_stable_width(font: &fontdue::Font, font_px: f32) -> u32 {
    MENU_BAR_NETWORK_ARROW_WIDTH
        + MENU_BAR_NETWORK_ARROW_GAP
        + measure_text_width(font, "999.9 M/s", font_px)
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_network_arrow(
    rgba: &mut [u8],
    canvas_width: u32,
    x: i32,
    y: i32,
    direction: MenuBarNetworkDirection,
) {
    match direction {
        MenuBarNetworkDirection::Up => {
            draw_menu_bar_arrow_triangle(rgba, canvas_width, x + 4, y, -1);
            draw_menu_bar_arrow_stem(
                rgba,
                canvas_width,
                x + 3,
                y + 5,
                MENU_BAR_NETWORK_ARROW_HEIGHT as i32 - 5,
            );
        }
        MenuBarNetworkDirection::Down => {
            draw_menu_bar_arrow_stem(
                rgba,
                canvas_width,
                x + 3,
                y,
                MENU_BAR_NETWORK_ARROW_HEIGHT as i32 - 5,
            );
            draw_menu_bar_arrow_triangle(
                rgba,
                canvas_width,
                x + 4,
                y + MENU_BAR_NETWORK_ARROW_HEIGHT as i32 - 1,
                1,
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_arrow_stem(rgba: &mut [u8], canvas_width: u32, x: i32, y: i32, height: i32) {
    for dy in 0..height {
        for dx in 0..3 {
            draw_menu_bar_stats_pixel(rgba, canvas_width, x + dx, y + dy, 245);
        }
    }
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_arrow_triangle(
    rgba: &mut [u8],
    canvas_width: u32,
    center_x: i32,
    tip_y: i32,
    direction: i32,
) {
    for row in 0..5 {
        let half_width = row;
        let y = tip_y - direction * row;
        for dx in -half_width..=half_width {
            draw_menu_bar_stats_pixel(rgba, canvas_width, center_x + dx, y, 245);
        }
    }
}

#[cfg(target_os = "macos")]
fn draw_menu_bar_stats_pixel(rgba: &mut [u8], canvas_width: u32, x: i32, y: i32, alpha: u8) {
    if x < 0 || y < 0 || x as u32 >= canvas_width {
        return;
    }
    let idx = ((y as u32 * canvas_width + x as u32) * 4) as usize;
    if idx + 3 < rgba.len() {
        rgba[idx] = 0;
        rgba[idx + 1] = 0;
        rgba[idx + 2] = 0;
        rgba[idx + 3] = rgba[idx + 3].max(alpha);
    }
}

#[cfg(target_os = "macos")]
fn draw_font_text(
    rgba: &mut [u8],
    canvas_width: u32,
    x: f32,
    baseline_y: i32,
    font_px: f32,
    text: &str,
    font: &fontdue::Font,
) {
    let mut cursor_x = x;
    for ch in text.chars() {
        let glyph = cached_menu_bar_glyph(font, ch, font_px);
        let metrics = &glyph.metrics;
        let glyph_x = cursor_x.round() as i32 + metrics.xmin;
        let glyph_y = baseline_y - metrics.ymin - metrics.height as i32;
        draw_rasterized_glyph(
            rgba,
            canvas_width,
            glyph_x,
            glyph_y,
            metrics.width,
            &glyph.bitmap,
        );
        draw_rasterized_glyph(
            rgba,
            canvas_width,
            glyph_x + 1,
            glyph_y,
            metrics.width,
            &glyph.bitmap,
        );
        cursor_x += metrics.advance_width;
    }
}

#[cfg(target_os = "macos")]
pub(super) struct MenuBarGlyph {
    pub(super) metrics: fontdue::Metrics,
    pub(super) bitmap: Vec<u8>,
}

#[cfg(target_os = "macos")]
type MenuBarGlyphCache = Mutex<HashMap<(char, u32), Arc<MenuBarGlyph>>>;

#[cfg(target_os = "macos")]
pub(super) fn cached_menu_bar_glyph(
    font: &fontdue::Font,
    ch: char,
    font_px: f32,
) -> Arc<MenuBarGlyph> {
    static GLYPHS: OnceLock<MenuBarGlyphCache> = OnceLock::new();
    let key = (ch, font_px.to_bits());
    let cache = GLYPHS.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(mut glyphs) = cache.lock() {
        if let Some(glyph) = glyphs.get(&key) {
            return glyph.clone();
        }
        let (metrics, bitmap) = font.rasterize(ch, font_px);
        let glyph = Arc::new(MenuBarGlyph { metrics, bitmap });
        glyphs.insert(key, glyph.clone());
        glyph
    } else {
        let (metrics, bitmap) = font.rasterize(ch, font_px);
        Arc::new(MenuBarGlyph { metrics, bitmap })
    }
}

#[cfg(target_os = "macos")]
fn draw_rasterized_glyph(
    rgba: &mut [u8],
    canvas_width: u32,
    x: i32,
    y: i32,
    glyph_width: usize,
    bitmap: &[u8],
) {
    for (idx, alpha) in bitmap.iter().enumerate() {
        if *alpha == 0 {
            continue;
        }
        let px = x + (idx % glyph_width) as i32;
        let py = y + (idx / glyph_width) as i32;
        if px < 0 || py < 0 || px as u32 >= canvas_width {
            continue;
        }
        let out_idx = ((py as u32 * canvas_width + px as u32) * 4) as usize;
        if out_idx + 3 < rgba.len() {
            rgba[out_idx] = 0;
            rgba[out_idx + 1] = 0;
            rgba[out_idx + 2] = 0;
            rgba[out_idx + 3] = rgba[out_idx + 3].max(*alpha);
        }
    }
}

fn state_tooltip(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "Bifrost - Running",
        ServiceState::Stopped => "Bifrost - Stopped",
        ServiceState::Disconnected => "Bifrost - Disconnected",
    }
}

fn state_code_tooltip(state: u8) -> &'static str {
    match state {
        STATE_RUNNING => "Bifrost - Running",
        STATE_STOPPED => "Bifrost - Stopped",
        _ => "Bifrost - Disconnected",
    }
}

fn operation_status_label(operation: u8) -> Option<&'static str> {
    match operation {
        OP_STARTING => Some("Bifrost: Starting..."),
        OP_STOPPING => Some("Bifrost: Stopping..."),
        OP_UPGRADING => Some("Bifrost: Updating…"),
        OP_START_FAILED => Some("Bifrost: Start failed - open logs"),
        OP_STOP_FAILED => Some("Bifrost: Stop failed - open logs"),
        OP_UPGRADE_FAILED => Some("Bifrost: Update failed - open logs"),
        _ => None,
    }
}

fn operation_busy(operation: u8) -> bool {
    matches!(operation, OP_STARTING | OP_STOPPING | OP_UPGRADING)
}

/// Build a dynamic "Updating… NN%" label while a download is in progress.
/// Returns `None` when the operation is not an active upgrade or no download
/// percent is available (the caller then falls back to the static label).
fn upgrade_status_label(operation: u8, percent: u8) -> Option<String> {
    if operation != OP_UPGRADING {
        return None;
    }
    if percent == UPGRADE_PERCENT_NONE {
        return None;
    }
    Some(format!("Bifrost: Updating… {percent}%"))
}

fn clear_completed_operation(operation: &AtomicU8, state: u8) {
    let current = operation.load(Ordering::Relaxed);
    let completed = (matches!(current, OP_STARTING | OP_START_FAILED) && state == STATE_RUNNING)
        || (matches!(current, OP_STOPPING | OP_STOP_FAILED)
            && matches!(state, STATE_STOPPED | STATE_DISCONNECTED));
    if completed {
        operation.store(OP_IDLE, Ordering::Relaxed);
    }
}

fn should_refresh_native_menu(
    state_changed: bool,
    operation_changed: bool,
    reload_requested: bool,
    action_triggered: bool,
    data_changed: bool,
) -> bool {
    state_changed || operation_changed || reload_requested || action_triggered || data_changed
}

fn should_replace_native_menu(
    state_changed: bool,
    operation_changed: bool,
    reload_requested: bool,
    action_triggered: bool,
    menu_recently_interacted: bool,
) -> bool {
    (state_changed || operation_changed || reload_requested || action_triggered)
        && !menu_recently_interacted
}

#[cfg(target_os = "macos")]
fn should_refresh_dashboard(
    menu_currently_open: bool,
    system_stats_changed: bool,
    native_menu_lifecycle_changed: bool,
    dashboard_installed: bool,
) -> bool {
    (!dashboard_installed && system_stats_changed)
        || (menu_currently_open && (system_stats_changed || native_menu_lifecycle_changed))
}

fn tray_event_may_open_menu(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }
    )
}

fn load_icon(dimmed: bool) -> tray_icon::Icon {
    #[cfg(target_os = "macos")]
    let icon_bytes: &[u8] = include_bytes!("../../../../../assets/trayTemplate@2x.png");
    #[cfg(target_os = "windows")]
    let icon_bytes: &[u8] = include_bytes!("../../../../../assets/bifrost.ico");

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

fn runtime_for_menu(args: &TrayArgs) -> Option<RuntimeInfo> {
    runtime::read_runtime(&args.runtime_file).or_else(|| {
        if runtime::is_process_running(args.parent_pid) {
            fallback_runtime_from_args(args)
        } else {
            None
        }
    })
}

fn fallback_runtime_from_args(args: &TrayArgs) -> Option<RuntimeInfo> {
    let port = args.port.or_else(|| {
        args.admin_url
            .as_deref()
            .and_then(parse_port_from_admin_url)
    })?;
    Some(RuntimeInfo {
        pid: args.parent_pid,
        port,
        socks5_port: None,
        host: args
            .admin_url
            .as_deref()
            .and_then(parse_host_from_admin_url),
        started_at_ms: None,
        binary_path: args.bifrost_bin.clone(),
    })
}

fn parse_host_from_admin_url(admin_url: &str) -> Option<String> {
    parse_authority_from_admin_url(admin_url).and_then(|authority| {
        if authority.starts_with('[') {
            let end = authority.find(']')?;
            Some(authority[..=end].to_string())
        } else {
            authority
                .split(':')
                .next()
                .filter(|host| !host.is_empty())
                .map(str::to_string)
        }
    })
}

fn parse_port_from_admin_url(admin_url: &str) -> Option<u16> {
    let authority = parse_authority_from_admin_url(admin_url)?;
    let port = if authority.starts_with('[') {
        let end = authority.find(']')?;
        authority.get(end + 2..)?
    } else {
        authority.rsplit_once(':')?.1
    };
    port.parse::<u16>().ok()
}

fn parse_authority_from_admin_url(admin_url: &str) -> Option<&str> {
    let rest = admin_url
        .strip_prefix("http://")
        .or_else(|| admin_url.strip_prefix("https://"))?;
    rest.split('/')
        .next()
        .filter(|authority| !authority.is_empty())
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

#[cfg(target_os = "macos")]
fn load_tray_system_stats_config(data_dir: &Path) -> TraySystemStatsConfig {
    match try_load_tray_system_stats_config(data_dir) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                data_dir = %data_dir.display(),
                error = %error,
                "failed to load tray system stats config; keeping system stats enabled"
            );
            default_tray_system_stats_config()
        }
    }
}

#[cfg(target_os = "macos")]
fn try_load_tray_system_stats_config(data_dir: &Path) -> Result<TraySystemStatsConfig, String> {
    let path = data_dir.join("config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_tray_system_stats_config());
        }
        Err(error) => {
            return Err(format!("read {}: {error}", path.display()));
        }
    };

    match toml::from_str::<bifrost_storage::UnifiedConfig>(&content) {
        Ok(config) => Ok(TraySystemStatsConfig {
            enabled: config.tray.show_system_stats,
            items: config.tray.system_stats_items,
        }),
        Err(error) => Err(format!("parse {}: {error}", path.display())),
    }
}

#[cfg(target_os = "macos")]
fn default_tray_system_stats_config() -> TraySystemStatsConfig {
    TraySystemStatsConfig {
        enabled: true,
        items: bifrost_storage::TraySystemStatsItems::default(),
    }
}

#[cfg(target_os = "macos")]
struct TraySystemStatsConfigWatcher {
    data_dir: PathBuf,
    config_path: PathBuf,
    config: TraySystemStatsConfig,
    last_reload_at: Instant,
    event_rx: Option<mpsc::Receiver<notify::Result<notify::Event>>>,
    _watcher: Option<notify::RecommendedWatcher>,
}

#[cfg(target_os = "macos")]
impl TraySystemStatsConfigWatcher {
    fn new(data_dir: &Path, now: Instant) -> Self {
        let config = load_tray_system_stats_config(data_dir);
        let config_path = data_dir.join("config.toml");
        let (event_rx, watcher) = create_tray_system_stats_config_watcher(data_dir);
        Self {
            data_dir: data_dir.to_path_buf(),
            config_path,
            config,
            last_reload_at: now,
            event_rx,
            _watcher: watcher,
        }
    }

    fn current(&mut self, now: Instant) -> TraySystemStatsConfig {
        let mut should_reload = false;
        let mut disconnected = false;

        if let Some(rx) = &self.event_rx {
            loop {
                match rx.try_recv() {
                    Ok(Ok(event)) => {
                        if tray_system_stats_config_event_is_relevant(&event, &self.config_path) {
                            should_reload = true;
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::debug!(
                            error = %error,
                            "tray system stats config watcher event failed"
                        );
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if disconnected {
            self.event_rx = None;
            self._watcher = None;
            tracing::debug!("tray system stats config watcher disconnected; using periodic reload");
        }

        if now.saturating_duration_since(self.last_reload_at)
            >= SYSTEM_STATS_CONFIG_FALLBACK_RELOAD_INTERVAL
        {
            should_reload = true;
        }

        if should_reload {
            self.last_reload_at = now;
            match try_load_tray_system_stats_config(&self.data_dir) {
                Ok(config) => {
                    self.config = config;
                }
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "failed to refresh tray system stats config; keeping previous config"
                    );
                }
            }
        }

        self.config.clone()
    }
}

#[cfg(target_os = "macos")]
fn create_tray_system_stats_config_watcher(
    data_dir: &Path,
) -> (
    Option<mpsc::Receiver<notify::Result<notify::Event>>>,
    Option<notify::RecommendedWatcher>,
) {
    let (tx, rx) = mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            tracing::debug!(
                data_dir = %data_dir.display(),
                error = %error,
                "failed to create tray system stats config watcher; using periodic reload"
            );
            return (None, None);
        }
    };

    if let Err(error) = watcher.watch(data_dir, RecursiveMode::NonRecursive) {
        tracing::debug!(
            data_dir = %data_dir.display(),
            error = %error,
            "failed to watch tray system stats config directory; using periodic reload"
        );
        return (None, None);
    }

    (Some(rx), Some(watcher))
}

#[cfg(target_os = "macos")]
fn tray_system_stats_config_event_is_relevant(event: &notify::Event, config_path: &Path) -> bool {
    if !matches!(
        event.kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return false;
    }

    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            path == config_path || path.file_name().is_some_and(|name| name == "config.toml")
        })
}

fn load_recent_rule_targets(data_dir: &Path) -> Vec<RuleTarget> {
    let path = data_dir.join(RECENT_RULES_FILE);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to read recent tray rules"
            );
            return Vec::new();
        }
    };
    match serde_json::from_str::<Vec<RuleTarget>>(&content) {
        Ok(mut targets) => {
            targets.dedup();
            targets.truncate(RECENT_RULE_LIMIT);
            targets
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to parse recent tray rules"
            );
            Vec::new()
        }
    }
}

fn save_recent_rule_targets(data_dir: &Path, targets: &[RuleTarget]) {
    let path = data_dir.join(RECENT_RULES_FILE);
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            tracing::warn!(
                path = %parent.display(),
                error = %error,
                "failed to create data dir before saving recent tray rules"
            );
            return;
        }
    }
    match serde_json::to_string_pretty(targets) {
        Ok(content) => {
            if let Err(error) = fs::write(&path, content) {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "failed to save recent tray rules"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "failed to encode recent tray rules"
            );
        }
    }
}

fn record_recent_rule_target(data_dir: &Path, target: &RuleTarget) {
    let mut targets = load_recent_rule_targets(data_dir);
    targets.retain(|candidate| candidate != target);
    targets.insert(0, target.clone());
    targets.truncate(RECENT_RULE_LIMIT);
    save_recent_rule_targets(data_dir, &targets);
}

fn load_menu_data_snapshot(
    args: &TrayArgs,
    state: ServiceState,
    include_remote: bool,
    include_system_proxy: bool,
) -> MenuDataSnapshot {
    let runtime = runtime_for_menu(args);
    let custom_config = load_custom_config_safe(&args.data_dir);
    let bin_available = trusted_bifrost_binary_available(args);
    let recent_rule_targets = load_recent_rule_targets(&args.data_dir);
    let (rules, system_proxy) = if include_remote {
        (
            load_rules_for_menu(runtime.as_ref(), state),
            if include_system_proxy {
                load_system_proxy_for_menu(runtime.as_ref(), state)
            } else {
                None
            },
        )
    } else {
        (Vec::new(), None)
    };

    MenuDataSnapshot {
        runtime,
        custom_config,
        rules,
        recent_rule_targets,
        system_proxy,
        bin_available,
        update_available: detect_update_available(&args.data_dir),
        #[cfg(target_os = "macos")]
        system_stats: {
            let stats_config = load_tray_system_stats_config(&args.data_dir);
            stats_config
                .visible()
                .then(|| SystemStatsMenuLines::collecting(&stats_config.items))
        },
        #[cfg(target_os = "macos")]
        dashboard: None,
    }
}

/// Read the version cache written by the admin `VersionChecker` and return the
/// latest version string when it is newer than the running tray binary.
fn detect_update_available(data_dir: &Path) -> Option<String> {
    let cache = read_version_cache(data_dir)?;
    let current = env!("CARGO_PKG_VERSION");
    if bifrost_core::version_check::is_newer_version(current, &cache.latest_version) {
        Some(cache.latest_version)
    } else {
        None
    }
}

fn version_cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(VERSION_CACHE_FILE)
}

fn read_version_cache(data_dir: &Path) -> Option<bifrost_core::version_check::VersionCache> {
    let content = fs::read_to_string(version_cache_path(data_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_version_cache(data_dir: &Path, cache: &bifrost_core::version_check::VersionCache) -> bool {
    let path = version_cache_path(data_dir);
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            tracing::warn!(
                path = %parent.display(),
                error = %error,
                "failed to create data dir before writing version cache"
            );
            return false;
        }
    }
    match serde_json::to_string_pretty(cache) {
        Ok(content) => {
            let tmp = path.with_extension("json.tmp");
            match fs::write(&tmp, content).and_then(|_| fs::rename(&tmp, &path)) {
                Ok(()) => true,
                Err(error) => {
                    let _ = fs::remove_file(&tmp);
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "failed to write version cache"
                    );
                    false
                }
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "failed to encode version cache");
            false
        }
    }
}

fn version_cache_is_fresh(cache: &bifrost_core::version_check::VersionCache) -> bool {
    chrono::Utc::now().signed_duration_since(cache.checked_at)
        < chrono::Duration::seconds(TRAY_UPDATE_CACHE_MAX_AGE_SECS)
}

fn should_fetch_update_cache(data_dir: &Path) -> bool {
    read_version_cache(data_dir)
        .as_ref()
        .is_none_or(|cache| !version_cache_is_fresh(cache))
}

fn refresh_update_cache_from_github(data_dir: &Path) -> bool {
    if !should_fetch_update_cache(data_dir) {
        tracing::info!("tray update check skipped; cached version is still fresh");
        return false;
    }

    match bifrost_core::version_check::fetch_latest_release_sync() {
        Ok((latest, highlights)) => {
            let cache = bifrost_core::version_check::VersionCache {
                latest_version: latest,
                release_highlights: highlights,
                checked_at: chrono::Utc::now(),
            };
            let changed = read_version_cache(data_dir)
                .as_ref()
                .is_none_or(|current| current.latest_version != cache.latest_version);
            if write_version_cache(data_dir, &cache) {
                tracing::info!(
                    latest_version = %cache.latest_version,
                    has_update = bifrost_core::version_check::is_newer_version(
                        env!("CARGO_PKG_VERSION"),
                        &cache.latest_version
                    ),
                    "tray background update check completed"
                );
                return changed;
            }
        }
        Err(error) => {
            tracing::debug!(
                error = %error,
                "tray background update check failed; keeping existing cache"
            );
        }
    }

    false
}

fn load_system_proxy_for_menu(
    runtime: Option<&RuntimeInfo>,
    state: ServiceState,
) -> Option<menu::SystemProxyMenuState> {
    if state != ServiceState::Running {
        return None;
    }
    runtime.and_then(|rt| load_system_proxy_state(&rt.admin_url()))
}

fn clone_menu_data_snapshot(menu_data: &Arc<Mutex<MenuDataSnapshot>>) -> MenuDataSnapshot {
    match menu_data.lock() {
        Ok(snapshot) => snapshot.clone(),
        Err(poisoned) => {
            tracing::warn!("tray menu data snapshot lock was poisoned; using last snapshot");
            poisoned.into_inner().clone()
        }
    }
}

fn build_menu_from_snapshot(
    snapshot: &MenuDataSnapshot,
    state: ServiceState,
    status_override: Option<&str>,
    service_action_busy: bool,
    data_dir: &str,
    upgrade_in_progress: bool,
) -> Vec<MenuEntry> {
    let menu_system_stats = None;

    menu::build_menu(
        snapshot.runtime.as_ref(),
        state,
        status_override,
        service_action_busy,
        snapshot.custom_config.as_ref(),
        data_dir,
        snapshot.bin_available,
        &snapshot.rules,
        &snapshot.recent_rule_targets,
        snapshot.system_proxy.as_ref(),
        snapshot.update_available.as_deref(),
        upgrade_in_progress,
        menu_system_stats,
    )
}

#[derive(Debug, Deserialize)]
struct RuleReferenceCandidate {
    rule_name: String,
    group_name: Option<String>,
    group_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedGroupForTray {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ActiveSummaryResponse {
    rules: Vec<ActiveRuleItem>,
}

#[derive(Debug, Deserialize)]
struct ActiveRuleItem {
    name: String,
    group_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SystemProxyStatusResponse {
    supported: bool,
    enabled: bool,
    managed_by_bifrost: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GroupRulesResponseForTray {
    group_name: String,
    rules: Vec<GroupRuleInfoForTray>,
}

#[derive(Debug, Deserialize)]
struct GroupRuleInfoForTray {
    name: String,
    enabled: bool,
    sort_order: i32,
}

fn load_rules_for_menu(runtime: Option<&RuntimeInfo>, state: ServiceState) -> Vec<menu::TrayRule> {
    if state != ServiceState::Running {
        return Vec::new();
    }
    let Some(runtime) = runtime else {
        return Vec::new();
    };
    load_rules_from_admin(&runtime.admin_url())
}

fn load_rules_from_admin(admin_url: &str) -> Vec<menu::TrayRule> {
    let base = admin_url.trim_end_matches('/');
    let candidates_url = format!("{base}/api/rules/reference-candidates");
    let groups_url = format!("{base}/api/group");
    let active_url = format!("{base}/api/rules/active-summary");
    let agent = http_agent();

    let candidates = match agent.get(&candidates_url).call() {
        Ok(resp) => match resp.into_json::<Vec<RuleReferenceCandidate>>() {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::warn!(error = %error, "failed to decode rule candidates for tray menu");
                return Vec::new();
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "failed to load rule candidates for tray menu");
            return Vec::new();
        }
    };

    let managed_groups = load_managed_groups_from_admin(&agent, &groups_url);

    let active = match agent.get(&active_url).call() {
        Ok(resp) => match resp.into_json::<ActiveSummaryResponse>() {
            Ok(active) => active.rules,
            Err(error) => {
                tracing::warn!(error = %error, "failed to decode active rules for tray menu");
                Vec::new()
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "failed to load active rules for tray menu");
            Vec::new()
        }
    };

    let active_targets = active
        .into_iter()
        .map(|rule| match rule.group_name {
            Some(group_name) => menu::RuleTarget::Group {
                group_name,
                name: rule.name,
            },
            None => menu::RuleTarget::Personal { name: rule.name },
        })
        .collect::<Vec<_>>();

    let mut rules = candidates
        .iter()
        .filter_map(|candidate| {
            if candidate.group_name.is_some() {
                return None;
            }
            let target = menu::RuleTarget::Personal {
                name: candidate.rule_name.clone(),
            };
            let enabled = active_targets.contains(&target);
            Some(menu::TrayRule {
                target,
                enabled,
                sort_order: 0,
                managed_group: false,
            })
        })
        .collect::<Vec<_>>();

    match managed_groups {
        Some(managed_groups) => {
            rules.extend(
                candidates
                    .into_iter()
                    .filter(|candidate| hidden_group_candidate(candidate, &managed_groups))
                    .map(|candidate| {
                        let target = menu::RuleTarget::Group {
                            group_name: candidate.group_name.unwrap_or_default(),
                            name: candidate.rule_name,
                        };
                        let enabled = active_targets.contains(&target);
                        menu::TrayRule {
                            target,
                            enabled,
                            sort_order: 0,
                            managed_group: false,
                        }
                    }),
            );
            rules.extend(load_group_rules_from_managed_groups(
                &agent,
                base,
                &managed_groups,
            ));
        }
        None => {
            tracing::warn!(
                "failed to load remote group permissions for tray menu; group rules hidden"
            );
        }
    }

    menu::sort_tray_rules(&mut rules);
    rules
}

fn hidden_group_candidate(
    candidate: &RuleReferenceCandidate,
    managed_groups: &[ManagedGroupForTray],
) -> bool {
    let Some(group_name) = candidate.group_name.as_deref() else {
        return false;
    };
    !managed_groups.iter().any(|group| {
        candidate
            .group_id
            .as_deref()
            .is_some_and(|id| id == group.id)
            || group.name == group_name
    })
}

fn load_group_rules_from_managed_groups(
    agent: &ureq::Agent,
    base: &str,
    groups: &[ManagedGroupForTray],
) -> Vec<menu::TrayRule> {
    let mut rules = Vec::new();
    for group in groups {
        let url = format!("{base}/api/group-rules/{}", urlencoding::encode(&group.id));
        match agent.get(&url).call() {
            Ok(resp) => match resp.into_json::<GroupRulesResponseForTray>() {
                Ok(group_rules) => {
                    let group_name = if group_rules.group_name.is_empty() {
                        group.name.clone()
                    } else {
                        group_rules.group_name
                    };
                    rules.extend(group_rules.rules.into_iter().map(|rule| menu::TrayRule {
                        target: menu::RuleTarget::Group {
                            group_name: group_name.clone(),
                            name: rule.name,
                        },
                        enabled: rule.enabled,
                        sort_order: rule.sort_order,
                        managed_group: true,
                    }));
                }
                Err(error) => {
                    tracing::warn!(
                        group_id = %group.id,
                        group_name = %group.name,
                        error = %error,
                        "failed to decode group rules for tray menu"
                    );
                }
            },
            Err(error) => {
                tracing::warn!(
                    group_id = %group.id,
                    group_name = %group.name,
                    error = %error,
                    "failed to load group rules for tray menu"
                );
            }
        }
    }
    rules
}

fn load_system_proxy_state(admin_url: &str) -> Option<menu::SystemProxyMenuState> {
    let base = admin_url.trim_end_matches('/');
    let url = format!("{base}/api/proxy/system");
    let agent = http_agent();
    match agent.get(&url).call() {
        Ok(resp) => match resp.into_json::<SystemProxyStatusResponse>() {
            Ok(status) => Some(menu::SystemProxyMenuState {
                supported: status.supported,
                enabled: status.enabled && status.managed_by_bifrost.unwrap_or(true),
            }),
            Err(error) => {
                tracing::warn!(error = %error, "failed to decode system proxy status for tray menu");
                None
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "failed to load system proxy status for tray menu");
            None
        }
    }
}

fn load_managed_groups_from_admin(
    agent: &ureq::Agent,
    groups_url: &str,
) -> Option<Vec<ManagedGroupForTray>> {
    if remote_group_failure_backoff_active() {
        tracing::debug!("skipping tray remote group refresh during failure backoff");
        return Some(Vec::new());
    }

    let mut managed_groups = Vec::new();

    let value = match agent.get(groups_url).call() {
        Ok(resp) => match resp.into_json::<serde_json::Value>() {
            Ok(value) => value,
            Err(error) => {
                if record_remote_group_failure() {
                    tracing::warn!(error = %error, "failed to decode group list for tray menu");
                } else {
                    tracing::debug!(error = %error, "failed to decode group list for tray menu");
                }
                return Some(Vec::new());
            }
        },
        Err(error) => {
            if record_remote_group_failure() {
                tracing::warn!(error = %error, "failed to load group list for tray menu");
            } else {
                tracing::debug!(error = %error, "failed to load group list for tray menu");
            }
            return Some(Vec::new());
        }
    };

    let groups = value
        .pointer("/data/list")
        .or_else(|| value.pointer("/data"))
        .and_then(|data| data.as_array());

    if let Some(groups) = groups {
        for group in groups {
            let is_managed = group
                .pointer("/level")
                .and_then(|v| v.as_i64())
                .is_some_and(|level| level >= 1);
            if !is_managed {
                continue;
            }
            let Some(id) = group
                .pointer("/id")
                .or_else(|| group.pointer("/group_id"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let Some(name) = group
                .pointer("/name")
                .or_else(|| group.pointer("/group_name"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            managed_groups.push(ManagedGroupForTray {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
    }

    clear_remote_group_failure();
    Some(managed_groups)
}

#[derive(Debug, Default)]
struct RemoteGroupFailureState {
    failed_at: Option<Instant>,
    warned: bool,
}

fn remote_group_failure_backoff() -> &'static Mutex<RemoteGroupFailureState> {
    static BACKOFF: OnceLock<Mutex<RemoteGroupFailureState>> = OnceLock::new();
    BACKOFF.get_or_init(|| Mutex::new(RemoteGroupFailureState::default()))
}

fn remote_group_failure_backoff_active() -> bool {
    match remote_group_failure_backoff().lock() {
        Ok(guard) => guard
            .failed_at
            .is_some_and(|failed_at| failed_at.elapsed() < REMOTE_GROUP_FAILURE_BACKOFF),
        Err(poisoned) => poisoned
            .into_inner()
            .failed_at
            .is_some_and(|failed_at| failed_at.elapsed() < REMOTE_GROUP_FAILURE_BACKOFF),
    }
}

fn record_remote_group_failure() -> bool {
    match remote_group_failure_backoff().lock() {
        Ok(mut guard) => {
            let should_warn = !guard.warned;
            guard.failed_at = Some(Instant::now());
            guard.warned = true;
            should_warn
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            let should_warn = !guard.warned;
            guard.failed_at = Some(Instant::now());
            guard.warned = true;
            should_warn
        }
    }
}

fn clear_remote_group_failure() {
    match remote_group_failure_backoff().lock() {
        Ok(mut guard) => *guard = RemoteGroupFailureState::default(),
        Err(poisoned) => *poisoned.into_inner() = RemoteGroupFailureState::default(),
    }
}

fn http_agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            bifrost_core::direct_ureq_agent_builder()
                .timeout_connect(HTTP_CONNECT_TIMEOUT)
                .timeout_read(HTTP_READ_TIMEOUT)
                .build()
        })
        .clone()
}

fn spawn_tray_thread<F>(name: &'static str, task: F) -> std::io::Result<thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_string())
        .stack_size(TRAY_THREAD_STACK_SIZE)
        .spawn(task)
}

fn spawn_tray_task<F>(name: &'static str, task: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = spawn_tray_thread(name, task) {
        tracing::error!(
            thread = name,
            error = %error,
            "failed to spawn tray task"
        );
    }
}

struct NativeMenuState {
    menu: Menu,
    action_map: HashMap<MenuId, MenuItemAction>,
    handles: Vec<NativeMenuHandle>,
    shape: Vec<NativeMenuShape>,
    #[cfg(target_os = "macos")]
    dashboard_header: Option<NativeDashboardHeader>,
    #[cfg(target_os = "macos")]
    action_sender: Option<std::rc::Rc<std::sync::mpsc::Sender<MenuId>>>,
    #[cfg(target_os = "macos")]
    action_targets: Vec<Retained<NativeMenuActionTarget>>,
}

impl NativeMenuState {
    fn new(
        items: &[MenuEntry],
        #[cfg(target_os = "macos")] dashboard: Option<&TrayDashboardSnapshot>,
        #[cfg(target_os = "macos")] action_sender: Option<std::sync::mpsc::Sender<MenuId>>,
    ) -> Self {
        let menu = Menu::new();
        let mut action_map = HashMap::new();
        let mut handles = Vec::new();
        let mut shape = Vec::new();

        for entry in items {
            append_menu_entry(&menu, entry, &mut action_map, &mut handles, &mut shape);
        }

        #[cfg(target_os = "macos")]
        let action_sender = action_sender.map(std::rc::Rc::new);

        let mut state = Self {
            menu,
            action_map,
            handles,
            shape,
            #[cfg(target_os = "macos")]
            dashboard_header: None,
            #[cfg(target_os = "macos")]
            action_sender,
            #[cfg(target_os = "macos")]
            action_targets: Vec::new(),
        };
        #[cfg(target_os = "macos")]
        {
            state.install_action_targets(items);
            state.dashboard_header = install_dashboard_header(&state.menu, dashboard);
        }
        state
    }

    fn refresh_in_place(&mut self, items: &[MenuEntry]) -> bool {
        let updates = menu_updates(items);
        let next_shape = menu_shape_from_updates(&updates);
        if next_shape != self.shape || updates.len() != self.handles.len() {
            return false;
        }

        let mut next_action_map = HashMap::new();
        for (handle, update) in self.handles.iter().zip(updates) {
            handle.apply(&update);
            if let Some(action) = update.action {
                if let Some(id) = handle.menu_id() {
                    next_action_map.insert(id, action);
                }
            }
        }
        self.action_map = next_action_map;
        #[cfg(target_os = "macos")]
        self.install_action_targets(items);
        true
    }

    #[cfg(target_os = "macos")]
    fn refresh_dashboard(&mut self, dashboard: Option<&TrayDashboardSnapshot>) {
        let theme = current_dashboard_theme();
        match (&mut self.dashboard_header, dashboard) {
            (Some(header), Some(snapshot)) => {
                header.set_snapshot(snapshot, theme);
            }
            (None, Some(snapshot)) => {
                self.dashboard_header = install_dashboard_header(&self.menu, Some(snapshot));
            }
            (Some(_), None) => {}
            (None, None) => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn install_action_targets(&mut self, items: &[MenuEntry]) {
        self.action_targets.clear();
        let Some(sender) = &self.action_sender else {
            return;
        };
        self.action_targets = install_native_menu_action_targets(
            &self.menu,
            items,
            sender,
            usize::from(self.dashboard_header.is_some()),
        );
    }
}

#[cfg(target_os = "macos")]
struct NativeDashboardHeader {
    item: Retained<NSMenuItem>,
    image_view: Retained<NSImageView>,
    rendered_image: Option<NativeStatsImage>,
}

#[cfg(target_os = "macos")]
impl NativeDashboardHeader {
    fn new(
        snapshot: &TrayDashboardSnapshot,
        theme: DashboardTheme,
        mtm: MainThreadMarker,
    ) -> Option<Self> {
        let bitmap = dashboard::render_dashboard_with_theme(snapshot, theme)?;
        let rendered = native_dashboard_image_from_bitmap(&bitmap)?;
        let view_size = NSSize::new(
            f64::from(bitmap.width) / 2.0,
            f64::from(bitmap.height) / 2.0,
        );
        rendered.image.setSize(view_size);
        rendered.image.setTemplate(false);
        let image_view = NSImageView::imageViewWithImage(&rendered.image, mtm);
        image_view.setEditable(false);
        image_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        let image_view_ref: &NSImageView = &image_view;
        let image_view_view: &NSView = image_view_ref.as_super().as_super();
        image_view_view.setFrameSize(view_size);
        let title = NSString::from_str("");
        let key = NSString::from_str("");
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &title,
                None,
                &key,
            )
        };
        item.setView(Some(image_view_view));
        item.setEnabled(false);
        Some(Self {
            item,
            image_view,
            rendered_image: Some(rendered),
        })
    }

    fn set_snapshot(&mut self, snapshot: &TrayDashboardSnapshot, theme: DashboardTheme) -> bool {
        let Some(bitmap) = dashboard::render_dashboard_with_theme(snapshot, theme) else {
            return false;
        };
        if let Some(rendered) = self.rendered_image.as_mut() {
            let reusable = rendered.width == bitmap.width && rendered.height == bitmap.height;
            if reusable {
                if let Some(image_rep) = rendered.image_rep.as_deref() {
                    if copy_dashboard_bitmap_to_image_rep(&bitmap, image_rep) {
                        unsafe {
                            let _: () = msg_send![&self.image_view, setNeedsDisplay: true];
                        }
                        return true;
                    }
                }
            }
        }

        let Some(rendered) = native_dashboard_image_from_bitmap(&bitmap) else {
            return false;
        };
        let view_size = NSSize::new(
            f64::from(bitmap.width) / 2.0,
            f64::from(bitmap.height) / 2.0,
        );
        rendered.image.setSize(view_size);
        rendered.image.setTemplate(false);
        self.image_view.setImage(Some(&rendered.image));
        let image_view_ref: &NSImageView = &self.image_view;
        let image_view_view: &NSView = image_view_ref.as_super().as_super();
        image_view_view.setFrameSize(view_size);
        self.rendered_image = Some(rendered);
        true
    }
}

#[cfg(target_os = "macos")]
fn install_dashboard_header(
    menu: &Menu,
    snapshot: Option<&TrayDashboardSnapshot>,
) -> Option<NativeDashboardHeader> {
    if !tray_dashboard_enabled() {
        tracing::debug!("tray dashboard header disabled by environment");
        return None;
    }
    unsafe {
        let ns_menu = menu.ns_menu().cast::<NSMenu>().as_ref()?;
        let mtm = MainThreadMarker::from(ns_menu);
        let snapshot = snapshot?;
        let theme = current_dashboard_theme();
        let Some(header) = NativeDashboardHeader::new(snapshot, theme, mtm) else {
            tracing::warn!("failed to render native tray dashboard header");
            return None;
        };
        ns_menu.insertItem_atIndex(&header.item, 0);
        tracing::info!(
            items = ns_menu.numberOfItems(),
            width = header
                .rendered_image
                .as_ref()
                .map(|image| image.width / 2)
                .unwrap_or(DASHBOARD_WIDTH / 2),
            height = DASHBOARD_HEIGHT / 2,
            "native tray dashboard header installed"
        );
        Some(header)
    }
}

#[cfg(target_os = "macos")]
fn tray_dashboard_enabled() -> bool {
    dashboard::dashboard_enabled_from_env(std::env::var(TRAY_DASHBOARD_ENV).ok().as_deref())
}

#[cfg(target_os = "macos")]
fn current_dashboard_theme() -> DashboardTheme {
    let name = NSAppearance::currentDrawingAppearance().name().to_string();
    if name.to_ascii_lowercase().contains("dark") {
        DashboardTheme::Dark
    } else {
        DashboardTheme::Light
    }
}

#[cfg(target_os = "macos")]
fn install_native_menu_action_targets(
    menu: &Menu,
    items: &[MenuEntry],
    sender: &std::rc::Rc<std::sync::mpsc::Sender<MenuId>>,
    index_offset: usize,
) -> Vec<Retained<NativeMenuActionTarget>> {
    let mut targets = Vec::new();
    unsafe {
        let Some(ns_menu) = menu.ns_menu().cast::<NSMenu>().as_ref() else {
            return targets;
        };
        install_native_menu_action_targets_for_ns_menu(
            ns_menu,
            items,
            sender,
            index_offset,
            &mut targets,
        );
    }
    targets
}

#[cfg(target_os = "macos")]
fn install_native_menu_action_targets_for_ns_menu(
    ns_menu: &NSMenu,
    items: &[MenuEntry],
    sender: &std::rc::Rc<std::sync::mpsc::Sender<MenuId>>,
    index_offset: usize,
    targets: &mut Vec<Retained<NativeMenuActionTarget>>,
) {
    let mtm = MainThreadMarker::from(ns_menu);
    for (index, entry) in items.iter().enumerate() {
        let native_index = index.saturating_add(index_offset);
        let Some(ns_item) = ns_menu.itemAtIndex(native_index as objc2_foundation::NSInteger) else {
            break;
        };
        match entry {
            MenuEntry::Item(item) => {
                if !matches!(item.action, MenuItemAction::None) {
                    let target =
                        NativeMenuActionTarget::new(MenuId::new(&item.id), sender.clone(), mtm);
                    unsafe {
                        ns_item.setTarget(Some(&target));
                        ns_item.setAction(Some(sel!(fireBifrostNativeMenuAction:)));
                    }
                    targets.push(target);
                }
            }
            MenuEntry::Submenu(submenu) => {
                if let Some(child_menu) = ns_item.submenu() {
                    install_native_menu_action_targets_for_ns_menu(
                        &child_menu,
                        &submenu.children,
                        sender,
                        0,
                        targets,
                    );
                }
            }
        }
    }
}

fn append_menu_entry(
    menu: &dyn MenuAppend,
    entry: &MenuEntry,
    map: &mut HashMap<MenuId, MenuItemAction>,
    handles: &mut Vec<NativeMenuHandle>,
    shape: &mut Vec<NativeMenuShape>,
) {
    match entry {
        MenuEntry::Item(item) => append_menu_item(menu, item, map, handles, shape),
        MenuEntry::Submenu(submenu) => append_submenu(menu, submenu, map, handles, shape),
    }
}

trait MenuAppend {
    fn append_item(&self, item: &dyn IsMenuItem);
}

impl MenuAppend for Menu {
    fn append_item(&self, item: &dyn IsMenuItem) {
        let _ = self.append(item);
    }
}

impl MenuAppend for Submenu {
    fn append_item(&self, item: &dyn IsMenuItem) {
        let _ = self.append(item);
    }
}

fn append_menu_item(
    menu: &dyn MenuAppend,
    item: &MenuItemDef,
    map: &mut HashMap<MenuId, MenuItemAction>,
    handles: &mut Vec<NativeMenuHandle>,
    shape: &mut Vec<NativeMenuShape>,
) {
    if item.label == "-" {
        menu.append_item(&PredefinedMenuItem::separator());
        handles.push(NativeMenuHandle::Separator);
        shape.push(menu_item_shape(item));
        return;
    }

    if item.checked
        || matches!(
            item.action,
            MenuItemAction::SelectRule { .. } | MenuItemAction::SetSystemProxy { .. }
        )
    {
        let menu_item = CheckMenuItem::with_id(
            MenuId::new(&item.id),
            &item.label,
            item.enabled,
            item.checked,
            None,
        );
        map.insert(menu_item.id().clone(), item.action.clone());
        menu.append_item(&menu_item);
        handles.push(NativeMenuHandle::Check(menu_item));
    } else {
        let menu_item = MenuItem::with_id(MenuId::new(&item.id), &item.label, item.enabled, None);
        map.insert(menu_item.id().clone(), item.action.clone());
        menu.append_item(&menu_item);
        handles.push(NativeMenuHandle::Item(menu_item));
    }
    shape.push(menu_item_shape(item));
}

fn append_submenu(
    menu: &dyn MenuAppend,
    submenu: &SubmenuDef,
    map: &mut HashMap<MenuId, MenuItemAction>,
    handles: &mut Vec<NativeMenuHandle>,
    shape: &mut Vec<NativeMenuShape>,
) {
    let native = Submenu::new(&submenu.label, submenu.enabled);
    handles.push(NativeMenuHandle::Submenu(native.clone()));
    shape.push(NativeMenuShape {
        id: submenu.id.clone(),
        kind: NativeMenuShapeKind::Submenu,
    });
    for child in &submenu.children {
        append_menu_entry(&native, child, map, handles, shape);
    }
    menu.append_item(&native);
}

#[derive(Debug, Clone)]
struct NativeMenuUpdate {
    shape: NativeMenuShape,
    label: String,
    enabled: bool,
    checked: bool,
    action: Option<MenuItemAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeMenuShape {
    id: String,
    kind: NativeMenuShapeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeMenuShapeKind {
    Item,
    Check,
    Submenu,
    Separator,
}

enum NativeMenuHandle {
    Item(MenuItem),
    Check(CheckMenuItem),
    Submenu(Submenu),
    Separator,
}

impl NativeMenuHandle {
    fn apply(&self, update: &NativeMenuUpdate) {
        match self {
            Self::Item(item) => {
                item.set_text(&update.label);
                item.set_enabled(update.enabled);
            }
            Self::Check(item) => {
                item.set_text(&update.label);
                item.set_enabled(update.enabled);
                item.set_checked(update.checked);
            }
            Self::Submenu(submenu) => {
                submenu.set_text(&update.label);
                submenu.set_enabled(update.enabled);
            }
            Self::Separator => {}
        }
    }

    fn menu_id(&self) -> Option<MenuId> {
        match self {
            Self::Item(item) => Some(item.id().clone()),
            Self::Check(item) => Some(item.id().clone()),
            Self::Submenu(_) | Self::Separator => None,
        }
    }
}

fn menu_updates(items: &[MenuEntry]) -> Vec<NativeMenuUpdate> {
    let mut updates = Vec::new();
    for entry in items {
        collect_menu_update(entry, &mut updates);
    }
    updates
}

#[cfg(test)]
fn menu_shape(items: &[MenuEntry]) -> Vec<NativeMenuShape> {
    menu_shape_from_updates(&menu_updates(items))
}

fn menu_shape_from_updates(updates: &[NativeMenuUpdate]) -> Vec<NativeMenuShape> {
    updates
        .iter()
        .map(|update| update.shape.clone())
        .collect::<Vec<_>>()
}

fn collect_menu_update(entry: &MenuEntry, updates: &mut Vec<NativeMenuUpdate>) {
    match entry {
        MenuEntry::Item(item) => {
            let is_separator = item.label == "-";
            updates.push(NativeMenuUpdate {
                shape: menu_item_shape(item),
                label: item.label.clone(),
                enabled: item.enabled,
                checked: item.checked,
                action: (!is_separator).then(|| item.action.clone()),
            });
        }
        MenuEntry::Submenu(submenu) => {
            updates.push(NativeMenuUpdate {
                shape: NativeMenuShape {
                    id: submenu.id.clone(),
                    kind: NativeMenuShapeKind::Submenu,
                },
                label: submenu.label.clone(),
                enabled: submenu.enabled,
                checked: false,
                action: None,
            });
            for child in &submenu.children {
                collect_menu_update(child, updates);
            }
        }
    }
}

fn menu_item_shape(item: &MenuItemDef) -> NativeMenuShape {
    let kind = if item.label == "-" {
        NativeMenuShapeKind::Separator
    } else if item.checked
        || matches!(
            item.action,
            MenuItemAction::SelectRule { .. } | MenuItemAction::SetSystemProxy { .. }
        )
    {
        NativeMenuShapeKind::Check
    } else {
        NativeMenuShapeKind::Item
    };
    NativeMenuShape {
        id: item.id.clone(),
        kind,
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_action(
    action: &MenuItemAction,
    args: &TrayArgs,
    quit_flag: &AtomicBool,
    reload_flag: &Arc<AtomicBool>,
    operation: &Arc<AtomicU8>,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    menu_data_generation: &Arc<AtomicU64>,
    system_proxy_refresh_in_flight: &Arc<AtomicBool>,
) {
    match action {
        MenuItemAction::OpenUrl(url) => {
            if let Err(e) = open_tray_target(url) {
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
            spawn_tray_task("bifrost-tray-admin-api", move || {
                let agent = http_agent();
                let result = match method.to_uppercase().as_str() {
                    "GET" => agent.get(&url).call(),
                    "POST" => agent.post(&url).call(),
                    unsupported => {
                        tracing::error!(method = unsupported, url = %url, "unsupported admin API method");
                        return;
                    }
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
        MenuItemAction::SetSystemProxy { url, enabled } => {
            let url = url.clone();
            let enabled = *enabled;
            let args = args.clone();
            let reload_flag = reload_flag.clone();
            let menu_data = menu_data.clone();
            let menu_data_generation = menu_data_generation.clone();
            let system_proxy_refresh_in_flight = system_proxy_refresh_in_flight.clone();
            spawn_tray_task("bifrost-tray-system-proxy", move || {
                system_proxy_refresh_in_flight.store(true, Ordering::Relaxed);
                let agent = http_agent();
                let body = format!(r#"{{"enabled":{enabled}}}"#);
                match agent
                    .put(&url)
                    .set("Content-Type", "application/json")
                    .send_string(&body)
                {
                    Ok(resp) => {
                        tracing::info!(
                            status = resp.status(),
                            enabled = enabled,
                            url = %url,
                            "system proxy toggle called"
                        );
                    }
                    Err(error) => {
                        tracing::error!(enabled = enabled, url = %url, error = %error, "system proxy toggle failed");
                    }
                }
                refresh_menu_data_snapshot(
                    &args,
                    ServiceState::Running,
                    &menu_data,
                    &menu_data_generation,
                    true,
                );
                system_proxy_refresh_in_flight.store(false, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
            });
        }
        MenuItemAction::StartService => {
            if operation_busy(operation.load(Ordering::Relaxed)) {
                tracing::warn!("service action ignored while another action is running");
                return;
            }
            let Some(bin) = resolve_bifrost_binary(args) else {
                tracing::error!("cannot find trusted bifrost binary to start service");
                operation.store(OP_START_FAILED, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
                return;
            };
            let data_dir = args.data_dir.to_string_lossy().to_string();
            let runtime_file = args.runtime_file.clone();
            let port = args.port;
            let extra_args = args.start_args.clone();
            let operation = operation.clone();
            let reload_flag = reload_flag.clone();
            operation.store(OP_STARTING, Ordering::Relaxed);
            reload_flag.store(true, Ordering::Relaxed);
            spawn_tray_task("bifrost-tray-start-service", move || {
                match spawn_start(&bin, &data_dir, port, &extra_args) {
                    Some(child) => monitor_start_child(
                        child,
                        runtime_file,
                        operation,
                        reload_flag,
                        OP_START_FAILED,
                    ),
                    None => {
                        operation.store(OP_START_FAILED, Ordering::Relaxed);
                        reload_flag.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
        MenuItemAction::StopService => {
            if operation_busy(operation.load(Ordering::Relaxed)) {
                tracing::warn!("service action ignored while another action is running");
                return;
            }
            let Some(bin) = resolve_bifrost_binary(args) else {
                tracing::error!("cannot find trusted bifrost binary to stop service");
                operation.store(OP_STOP_FAILED, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
                return;
            };
            let data_dir = args.data_dir.to_string_lossy().to_string();
            let operation = operation.clone();
            let reload_flag = reload_flag.clone();
            operation.store(OP_STOPPING, Ordering::Relaxed);
            reload_flag.store(true, Ordering::Relaxed);
            spawn_tray_task("bifrost-tray-stop-service", move || {
                if spawn_stop(&bin, &data_dir) {
                    operation.store(OP_IDLE, Ordering::Relaxed);
                } else {
                    operation.store(OP_STOP_FAILED, Ordering::Relaxed);
                }
                reload_flag.store(true, Ordering::Relaxed);
            });
        }
        MenuItemAction::StartUpgrade { target_version } => {
            if operation_busy(operation.load(Ordering::Relaxed)) {
                tracing::warn!("upgrade ignored while another action is running");
                return;
            }
            let Some(bin) = resolve_bifrost_binary(args) else {
                tracing::error!("cannot find trusted bifrost binary to start upgrade");
                operation.store(OP_UPGRADE_FAILED, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
                return;
            };
            let data_dir = args.data_dir.to_string_lossy().to_string();
            let target_version = target_version.clone();
            let operation = operation.clone();
            let reload_flag = reload_flag.clone();
            operation.store(OP_UPGRADING, Ordering::Relaxed);
            reload_flag.store(true, Ordering::Relaxed);
            spawn_tray_task("bifrost-tray-self-update", move || {
                if !spawn_self_update(&bin, &data_dir, &target_version) {
                    operation.store(OP_UPGRADE_FAILED, Ordering::Relaxed);
                    reload_flag.store(true, Ordering::Relaxed);
                }
                // On success the upgrade subprocess writes the progress file; the
                // state-poll thread reconciles it to OP_IDLE / OP_UPGRADE_FAILED.
            });
        }
        MenuItemAction::OpenDirectory(path) => {
            if let Err(e) = open_tray_target(path) {
                tracing::error!(path = %path, error = %e, "failed to open directory");
            }
        }
        MenuItemAction::SelectRule {
            target,
            enabled_targets,
            currently_enabled,
        } => {
            let rt = runtime_for_menu(args);
            let Some(rt) = rt else {
                tracing::error!("cannot select rule without runtime");
                return;
            };
            let admin_url = rt.admin_url();
            let target = target.clone();
            let enabled_targets = enabled_targets.clone();
            let currently_enabled = *currently_enabled;
            let data_dir = args.data_dir.clone();
            let args = args.clone();
            let reload_flag = reload_flag.clone();
            let menu_data = menu_data.clone();
            let menu_data_generation = menu_data_generation.clone();
            spawn_tray_task("bifrost-tray-select-rule", move || {
                if toggle_single_rule(&admin_url, &target, &enabled_targets, currently_enabled) {
                    if !currently_enabled {
                        record_recent_rule_target(&data_dir, &target);
                    }
                    refresh_menu_data_snapshot(
                        &args,
                        ServiceState::Running,
                        &menu_data,
                        &menu_data_generation,
                        false,
                    );
                    reload_flag.store(true, Ordering::Relaxed);
                }
            });
        }
        MenuItemAction::QuitTray => {
            tracing::info!("quit tray requested");
            remove_own_tray_pid(&args.data_dir);
            quit_flag.store(true, Ordering::Relaxed);
        }
        MenuItemAction::None => {}
    }
}

fn toggle_single_rule(
    admin_url: &str,
    target: &RuleTarget,
    enabled_targets: &[RuleTarget],
    currently_enabled: bool,
) -> bool {
    let agent = http_agent();
    let mut success = true;
    if currently_enabled {
        return call_rule_toggle(&agent, admin_url, target, false);
    }

    for candidate in enabled_targets {
        if candidate == target {
            continue;
        }
        if !call_rule_toggle(&agent, admin_url, candidate, false) {
            success = false;
        }
    }
    if !call_rule_toggle(&agent, admin_url, target, true) {
        success = false;
    }
    success
}

fn call_rule_toggle(
    agent: &ureq::Agent,
    admin_url: &str,
    target: &RuleTarget,
    enabled: bool,
) -> bool {
    let url = rule_toggle_url(admin_url, target, enabled);
    match agent.put(&url).call() {
        Ok(resp) => {
            let status = resp.status();
            if (200..300).contains(&status) {
                tracing::info!(url = %url, status = status, "rule toggle API called");
                true
            } else {
                tracing::error!(url = %url, status = status, "rule toggle API returned error");
                false
            }
        }
        Err(error) => {
            tracing::error!(url = %url, error = %error, "rule toggle API failed");
            false
        }
    }
}

fn rule_toggle_url(admin_url: &str, target: &RuleTarget, enabled: bool) -> String {
    let base = admin_url.trim_end_matches('/');
    let action = if enabled { "enable" } else { "disable" };
    match target {
        RuleTarget::Personal { name } => {
            format!("{base}/api/rules/{}/{action}", urlencoding::encode(name))
        }
        RuleTarget::Group { group_name, name } => format!(
            "{base}/api/group-rules/{}/{}/{}",
            urlencoding::encode(group_name),
            urlencoding::encode(name),
            action
        ),
    }
}

fn spawn_start(
    bin: &Path,
    data_dir: &str,
    port: Option<u16>,
    extra_args: &[String],
) -> Option<Child> {
    let mut cmd = Command::new(bin);
    cmd.env("BIFROST_DATA_DIR", data_dir)
        .env("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT", "1")
        .args(build_service_start_args(port, extra_args));
    configure_service_command(&mut cmd);
    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(pid = child.id(), "bifrost service started");
            Some(child)
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to start bifrost service");
            None
        }
    }
}

fn build_service_start_args(port: Option<u16>, extra_args: &[String]) -> Vec<String> {
    let mut args = vec![
        "start".to_string(),
        "--daemon".to_string(),
        "--no-tray".to_string(),
        "--no-system-proxy".to_string(),
    ];
    if let Some(p) = port {
        args.push("-p".to_string());
        args.push(p.to_string());
    }
    args.extend(extra_args.iter().cloned());
    args
}

#[cfg(target_os = "windows")]
fn open_tray_target(target: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let operation = wide("open");
    let target = wide(target);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        return Err(format!("ShellExecuteW failed with code {result}"));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn open_tray_target(target: &str) -> Result<(), String> {
    open::that(target).map_err(|error| error.to_string())
}

fn monitor_start_child(
    mut child: Child,
    runtime_file: PathBuf,
    operation: Arc<AtomicU8>,
    reload_flag: Arc<AtomicBool>,
    failure_operation: u8,
) {
    let start = std::time::Instant::now();
    let mut ready = false;
    let mut child_exited = false;
    while start.elapsed() < START_READY_TIMEOUT {
        if runtime_file_points_to_running_service(&runtime_file) {
            ready = true;
            break;
        }

        if !child_exited {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    child_exited = true;
                    tracing::info!(
                        status = %status,
                        "bifrost start command exited successfully; waiting for service readiness"
                    );
                }
                Ok(Some(status)) => {
                    tracing::error!(status = %status, "bifrost start exited before service became ready");
                    operation.store(failure_operation, Ordering::Relaxed);
                    reload_flag.store(true, Ordering::Relaxed);
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::error!(error = %error, "failed to poll bifrost start child");
                    operation.store(failure_operation, Ordering::Relaxed);
                    reload_flag.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    if ready {
        operation.store(OP_IDLE, Ordering::Relaxed);
        reload_flag.store(true, Ordering::Relaxed);
    } else {
        tracing::error!("timed out waiting for bifrost service to become ready");
        operation.store(failure_operation, Ordering::Relaxed);
        reload_flag.store(true, Ordering::Relaxed);
    }

    if !child_exited {
        if let Err(error) = child.wait() {
            tracing::warn!(error = %error, "failed to reap bifrost start child");
        }
    }
    reload_flag.store(true, Ordering::Relaxed);
}

fn runtime_file_points_to_running_service(runtime_file: &Path) -> bool {
    runtime::read_runtime(runtime_file)
        .map(|rt| runtime::is_process_running(rt.pid))
        .unwrap_or(false)
}

fn spawn_stop(bin: &Path, data_dir: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.env("BIFROST_DATA_DIR", data_dir)
        .env("BIFROST_TRAY_INVOKED_STOP", "1")
        .arg("stop");
    configure_service_command(&mut cmd);

    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(pid = child.id(), "bifrost stop invoked");
            wait_for_child(child, "bifrost stop")
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to stop bifrost service");
            false
        }
    }
}

/// Spawn a detached `bifrost self-update` subprocess that performs the upgrade
/// and writes progress to `<data_dir>/upgrade-progress.json`. The tray observes
/// that file rather than waiting on this child (the upgrade restarts the proxy
/// but never the tray process itself).
fn spawn_self_update(bin: &Path, data_dir: &str, target_version: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.env("BIFROST_DATA_DIR", data_dir)
        .env("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT", "1")
        .args([
            "self-update",
            "--target",
            target_version,
            "--source",
            "tray",
        ]);
    configure_service_command(&mut cmd);
    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(pid = child.id(), target = %target_version, "bifrost self-update spawned");
            true
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to spawn bifrost self-update");
            false
        }
    }
}

fn wait_for_child(mut child: Child, label: &str) -> bool {
    match child.wait() {
        Ok(status) => {
            if status.success() {
                tracing::info!(%label, "child process exited successfully");
                true
            } else {
                tracing::error!(%label, status = %status, "child process exited with failure");
                false
            }
        }
        Err(error) => {
            tracing::error!(%label, error = %error, "failed to wait for child process");
            false
        }
    }
}

fn configure_service_command(cmd: &mut Command) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
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

fn trusted_bifrost_binary_available(args: &TrayArgs) -> bool {
    args.bifrost_bin.as_ref().is_some_and(|bin| bin.exists()) || find_bifrost_binary().is_some()
}

fn find_bifrost_binary() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let dir = current_exe.parent()?;
    let sibling = dir.join(bifrost_binary_name());
    if sibling.exists() {
        return Some(sibling);
    }
    None
}

fn bifrost_binary_name() -> &'static str {
    if cfg!(windows) {
        "bifrost.exe"
    } else {
        "bifrost"
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_service_state(
    quit_flag: &AtomicBool,
    state: &AtomicU8,
    operation: &AtomicU8,
    upgrade_percent: &AtomicU8,
    reload_flag: &AtomicBool,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    menu_data_generation: &AtomicU64,
    args: &TrayArgs,
) {
    let mut service_idle_since = match state.load(Ordering::Relaxed) {
        STATE_RUNNING => None,
        _ => Some(Instant::now()),
    };

    loop {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(POLL_INTERVAL);

        let new_state = compute_service_state(args);
        let old = state.swap(new_state, Ordering::Relaxed);
        if old != new_state {
            tracing::info!(
                old_state = old,
                new_state = new_state,
                "service state transition detected"
            );
        }

        reconcile_upgrade_progress(
            args,
            operation,
            upgrade_percent,
            reload_flag,
            menu_data,
            menu_data_generation,
        );

        let current_operation = operation.load(Ordering::Relaxed);
        if should_auto_exit_for_service_idle(
            &mut service_idle_since,
            new_state,
            current_operation,
            Instant::now(),
        ) {
            tracing::info!(
                timeout_secs = SERVICE_IDLE_EXIT_TIMEOUT.as_secs(),
                "service has been stopped without restart; exiting tray helper"
            );
            remove_own_tray_pid(&args.data_dir);
            quit_flag.store(true, Ordering::Relaxed);
            break;
        }
    }
}

/// Reflect the cross-process upgrade-progress file into the tray operation state.
///
/// While an upgrade subprocess runs it writes `<data_dir>/upgrade-progress.json`.
/// We map its phases onto the tray operation indicator and clear the file on a
/// terminal state so the tray returns to its normal display.
fn reconcile_upgrade_progress(
    args: &TrayArgs,
    operation: &AtomicU8,
    upgrade_percent: &AtomicU8,
    reload_flag: &AtomicBool,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    menu_data_generation: &AtomicU64,
) {
    use bifrost_core::upgrade_progress::{
        clear_progress, is_stale, read_progress, UpgradePhase, DEFAULT_STALE_SECS,
    };

    let current_op = operation.load(Ordering::Relaxed);
    // Only observe progress while an upgrade is active or its terminal state is
    // still pending acknowledgement. This avoids reacting to a stale file from a
    // prior run when the tray is otherwise idle.
    let progress = read_progress(&args.data_dir);

    match progress.phase {
        UpgradePhase::Idle => {
            // Nothing to do; if we were upgrading the terminal handler cleared it.
        }
        UpgradePhase::Checking
        | UpgradePhase::Downloading
        | UpgradePhase::Installing
        | UpgradePhase::Restarting => {
            if is_stale(&progress, DEFAULT_STALE_SECS) {
                tracing::warn!("upgrade progress is stale; marking update as failed");
                clear_progress(&args.data_dir);
                operation.store(OP_UPGRADE_FAILED, Ordering::Relaxed);
                upgrade_percent.store(UPGRADE_PERCENT_NONE, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
                return;
            }
            if current_op != OP_UPGRADING {
                operation.store(OP_UPGRADING, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
            }
            let pct = match (progress.phase, progress.percent) {
                (UpgradePhase::Downloading, Some(p)) => p.clamp(0.0, 100.0) as u8,
                _ => UPGRADE_PERCENT_NONE,
            };
            if upgrade_percent.swap(pct, Ordering::Relaxed) != pct {
                reload_flag.store(true, Ordering::Relaxed);
            }
        }
        UpgradePhase::Completed => {
            if current_op == OP_UPGRADING {
                clear_progress(&args.data_dir);
                operation.store(OP_IDLE, Ordering::Relaxed);
                upgrade_percent.store(UPGRADE_PERCENT_NONE, Ordering::Relaxed);
                // Refresh so the "Update to vX" entry disappears once the new
                // version_cache catches up.
                refresh_menu_data_snapshot(
                    args,
                    ServiceState::Running,
                    menu_data,
                    menu_data_generation,
                    false,
                );
                reload_flag.store(true, Ordering::Relaxed);
            }
        }
        UpgradePhase::Failed => {
            if current_op == OP_UPGRADING {
                clear_progress(&args.data_dir);
                operation.store(OP_UPGRADE_FAILED, Ordering::Relaxed);
                upgrade_percent.store(UPGRADE_PERCENT_NONE, Ordering::Relaxed);
                reload_flag.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn should_auto_exit_for_service_idle(
    service_idle_since: &mut Option<Instant>,
    service_state: u8,
    operation: u8,
    now: Instant,
) -> bool {
    if service_state == STATE_RUNNING {
        *service_idle_since = None;
        return false;
    }

    if operation == OP_STARTING {
        *service_idle_since = None;
        return false;
    }

    let since = service_idle_since.get_or_insert(now);
    now.duration_since(*since) >= SERVICE_IDLE_EXIT_TIMEOUT
}

fn poll_menu_data(
    quit_flag: &AtomicBool,
    state: &AtomicU8,
    args: &TrayArgs,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &AtomicU64,
) {
    loop {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }
        if !crate::commands::tray_launcher::tray_enabled_by_config(&args.data_dir) {
            tracing::info!("tray disabled by config; exiting helper");
            remove_own_tray_pid(&args.data_dir);
            quit_flag.store(true, Ordering::Relaxed);
            break;
        }

        let svc_state = match state.load(Ordering::Relaxed) {
            STATE_RUNNING => ServiceState::Running,
            STATE_STOPPED => ServiceState::Stopped,
            _ => ServiceState::Disconnected,
        };
        refresh_menu_data_snapshot(args, svc_state, menu_data, generation, false);

        sleep_until_next_menu_data_poll(quit_flag);
    }
}

#[cfg(target_os = "macos")]
fn poll_system_stats(
    quit_flag: &AtomicBool,
    args: &TrayArgs,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &AtomicU64,
    menu_open_state: &Arc<NativeMenuOpenState>,
) {
    let mut sampler = SystemStatsSampler::new(&args.data_dir);
    let mut dashboard_history = TrayDashboardHistory;
    let mut stats_config_watcher =
        TraySystemStatsConfigWatcher::new(&args.data_dir, Instant::now());
    loop {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }

        let stats_config = stats_config_watcher.current(Instant::now());
        let menu_is_open = menu_open_state.open.load(Ordering::Relaxed);
        if stats_config.visible() {
            let snapshot =
                sampler.sample_for_menu_state(Instant::now(), &stats_config.items, menu_is_open);
            let lines = system_stats::menu_lines_for_menu_state(
                &snapshot,
                &stats_config.items,
                menu_is_open,
            );
            let menu_snapshot = clone_menu_data_snapshot(menu_data);
            let dashboard_update = if menu_is_open || menu_snapshot.dashboard.is_none() {
                let service_state =
                    determine_state(menu_snapshot.runtime.as_ref(), args.parent_pid);
                DashboardSnapshotUpdate::Set(Some(Box::new(dashboard_history.snapshot(
                    service_state,
                    dashboard_runtime_label(menu_snapshot.runtime.as_ref(), service_state),
                    dashboard_system_proxy_label(menu_snapshot.system_proxy.as_ref()),
                    &snapshot,
                ))))
            } else {
                DashboardSnapshotUpdate::Preserve
            };
            update_system_stats_snapshot(Some(lines), dashboard_update, menu_data, generation);
        } else {
            sampler.reset_network_baseline();
            update_system_stats_snapshot(
                None,
                DashboardSnapshotUpdate::Set(None),
                menu_data,
                generation,
            );
        }

        sleep_interruptibly(quit_flag, SYSTEM_STATS_POLL_INTERVAL);
    }
}

#[cfg(target_os = "macos")]
fn update_system_stats_snapshot(
    lines: Option<SystemStatsMenuLines>,
    dashboard: DashboardSnapshotUpdate,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &AtomicU64,
) -> bool {
    let changed = match menu_data.lock() {
        Ok(mut current) => apply_system_stats_snapshot_update(&mut current, lines, dashboard),
        Err(poisoned) => {
            tracing::warn!("tray menu data snapshot lock was poisoned; updating system stats");
            let mut current = poisoned.into_inner();
            apply_system_stats_snapshot_update(&mut current, lines, dashboard)
        }
    };
    if changed {
        generation.fetch_add(1, Ordering::Relaxed);
    }
    changed
}

#[cfg(target_os = "macos")]
fn apply_system_stats_snapshot_update(
    current: &mut MenuDataSnapshot,
    lines: Option<SystemStatsMenuLines>,
    dashboard: DashboardSnapshotUpdate,
) -> bool {
    let preserving_dashboard = matches!(dashboard, DashboardSnapshotUpdate::Preserve);
    let lines_changed = if preserving_dashboard
        && current
            .system_stats
            .as_ref()
            .map(|lines| lines.menu_bar.as_str())
            == lines.as_ref().map(|lines| lines.menu_bar.as_str())
    {
        false
    } else {
        current.system_stats != lines
    };
    let dashboard_changed = match &dashboard {
        DashboardSnapshotUpdate::Preserve => false,
        DashboardSnapshotUpdate::Set(next) => current.dashboard.as_ref() != next.as_deref(),
    };
    if !lines_changed && !dashboard_changed {
        return false;
    }

    current.system_stats = lines;
    if let DashboardSnapshotUpdate::Set(next) = dashboard {
        current.dashboard = next.map(|snapshot| *snapshot);
    }
    true
}

#[cfg(target_os = "macos")]
fn dashboard_runtime_label(runtime: Option<&RuntimeInfo>, state: ServiceState) -> String {
    match (state, runtime) {
        (ServiceState::Running, Some(runtime)) => {
            format!("{}:{}", runtime.effective_host(), runtime.port)
        }
        (ServiceState::Stopped, _) => "Service stopped".to_string(),
        (ServiceState::Disconnected, _) => "Service disconnected".to_string(),
        _ => "Service unknown".to_string(),
    }
}

#[cfg(target_os = "macos")]
fn dashboard_system_proxy_label(system_proxy: Option<&menu::SystemProxyMenuState>) -> String {
    match system_proxy {
        Some(state) if !state.supported => "System proxy unsupported".to_string(),
        Some(state) if state.enabled => "System proxy On".to_string(),
        Some(_) => "System proxy Off".to_string(),
        None => "System proxy --".to_string(),
    }
}

fn poll_update_check(
    quit_flag: &AtomicBool,
    state: &AtomicU8,
    args: &TrayArgs,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &AtomicU64,
) {
    sleep_interruptibly(quit_flag, tray_update_check_initial_delay());

    loop {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }

        let cache_changed = refresh_update_cache_from_github(&args.data_dir);
        if cache_changed {
            let svc_state = match state.load(Ordering::Relaxed) {
                STATE_RUNNING => ServiceState::Running,
                STATE_STOPPED => ServiceState::Stopped,
                _ => ServiceState::Disconnected,
            };
            refresh_menu_data_snapshot(args, svc_state, menu_data, generation, false);
        }

        sleep_interruptibly(quit_flag, TRAY_UPDATE_CHECK_INTERVAL);
    }
}

fn tray_update_check_initial_delay() -> Duration {
    std::env::var("BIFROST_TRAY_UPDATE_CHECK_INITIAL_DELAY_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(TRAY_UPDATE_CHECK_INITIAL_DELAY)
}

fn request_system_proxy_menu_refresh(
    args: &TrayArgs,
    state: &AtomicU8,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &Arc<AtomicU64>,
    refresh_in_flight: &Arc<AtomicBool>,
) {
    if state.load(Ordering::Relaxed) != STATE_RUNNING {
        return;
    }
    if refresh_in_flight.swap(true, Ordering::Relaxed) {
        return;
    }

    let args = args.clone();
    let menu_data = menu_data.clone();
    let refresh_in_flight = refresh_in_flight.clone();
    let generation = generation.clone();
    spawn_tray_task("bifrost-tray-system-proxy-refresh", move || {
        refresh_menu_data_snapshot(&args, ServiceState::Running, &menu_data, &generation, true);
        refresh_in_flight.store(false, Ordering::Relaxed);
    });
}

fn refresh_menu_data_snapshot(
    args: &TrayArgs,
    state: ServiceState,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &AtomicU64,
    include_system_proxy: bool,
) -> bool {
    let should_load_system_proxy = include_system_proxy
        || match menu_data.lock() {
            Ok(current) => state == ServiceState::Running && current.system_proxy.is_none(),
            Err(poisoned) => {
                let current = poisoned.into_inner();
                state == ServiceState::Running && current.system_proxy.is_none()
            }
        };
    let mut next = load_menu_data_snapshot(args, state, true, should_load_system_proxy);
    let changed = match menu_data.lock() {
        Ok(mut current) => {
            if !include_system_proxy && current.system_proxy.is_some() {
                next.system_proxy = current.system_proxy.clone();
            }
            #[cfg(target_os = "macos")]
            if next.system_stats.is_some() {
                next.system_stats = current.system_stats.clone().or(next.system_stats);
            }
            #[cfg(target_os = "macos")]
            {
                next.dashboard = current.dashboard.clone();
            }
            if *current != next {
                *current = next;
                true
            } else {
                false
            }
        }
        Err(poisoned) => {
            tracing::warn!("tray menu data snapshot lock was poisoned; replacing snapshot");
            *poisoned.into_inner() = next;
            true
        }
    };
    if changed {
        generation.fetch_add(1, Ordering::Relaxed);
    }
    changed
}

fn remove_own_tray_pid(data_dir: &Path) {
    let pid_path = data_dir.join("tray.pid");
    let current_pid = std::process::id().to_string();
    let should_remove = match std::fs::read_to_string(&pid_path) {
        Ok(pid) => pid.trim() == current_pid,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    };
    if should_remove {
        let _ = std::fs::remove_file(pid_path);
    }
}

fn sleep_until_next_menu_data_poll(quit_flag: &AtomicBool) {
    sleep_interruptibly(quit_flag, MENU_DATA_POLL_INTERVAL);
}

fn sleep_interruptibly(quit_flag: &AtomicBool, duration: Duration) {
    let mut slept = Duration::ZERO;
    while slept < duration {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }
        let remaining = duration.saturating_sub(slept);
        let chunk = remaining.min(Duration::from_millis(100));
        thread::sleep(chunk);
        slept += chunk;
    }
}

/// Compute the current service state code (running/stopped/disconnected) by
/// re-reading runtime.json and probing the relevant processes.
fn compute_service_state(args: &TrayArgs) -> u8 {
    let parent_alive = runtime::is_process_running(args.parent_pid);
    let runtime = runtime::read_runtime(&args.runtime_file);
    let service_alive = runtime
        .as_ref()
        .map(|rt| runtime::is_process_running(rt.pid))
        .unwrap_or(false);

    if parent_alive || service_alive {
        STATE_RUNNING
    } else if runtime.is_some() {
        STATE_STOPPED
    } else {
        STATE_DISCONNECTED
    }
}

#[cfg(test)]
mod tests {
    include!("tray_tests.rs");
}
