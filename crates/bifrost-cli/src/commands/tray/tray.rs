use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{TrayIconBuilder, TrayIconEvent};

use super::cli::TrayArgs;
use super::config::{self, TrayConfig};
use super::lock::TrayLock;
#[cfg(target_os = "macos")]
use super::menu::SystemStatsMenuLines;
use super::menu::{self, MenuEntry, MenuItemAction, MenuItemDef, RuleTarget, SubmenuDef};
use super::runtime::{self, RuntimeInfo, ServiceState};
#[cfg(target_os = "macos")]
use super::system_stats::{self, SystemStatsSampler};

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
const LOG_RETENTION_DAYS: u64 = 30;
const MENU_REBUILD_SUPPRESSION_AFTER_CLICK: Duration = Duration::from_secs(3);
const REMOTE_GROUP_FAILURE_BACKOFF: Duration = Duration::from_secs(5);
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

    let mut native_menu = NativeMenuState::new(&menu_items);
    let mut action_map = native_menu.action_map.clone();

    let initial_icon = match state {
        ServiceState::Running => &icon_running,
        _ => &icon_stopped,
    };

    #[cfg(target_os = "macos")]
    let initial_icon_for_builder = initial_menu_bar_title
        .as_deref()
        .and_then(menu_bar_stats_icon)
        .unwrap_or_else(|| initial_icon.clone());
    #[cfg(not(target_os = "macos"))]
    let initial_icon_for_builder = initial_icon.clone();

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(native_menu.menu.clone()))
        .with_tooltip(state_tooltip(state))
        .with_icon(initial_icon_for_builder);

    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);

    let tray_icon = builder
        .build()
        .map_err(|e| format!("failed to create tray icon: {e}"))?;

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
    let system_proxy_refresh_in_flight = Arc::new(AtomicBool::new(false));

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
        spawn_tray_thread("bifrost-tray-system-stats", move || {
            poll_system_stats(&stats_quit, &stats_args, &stats_snapshot, &stats_generation);
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
        let menu_recently_interacted = last_tray_interaction_at
            .is_some_and(|instant| instant.elapsed() < MENU_REBUILD_SUPPRESSION_AFTER_CLICK);
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
                let _ = tray_icon.set_icon_with_as_template(Some(new_icon.clone()), true);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = tray_icon.set_icon(Some(new_icon.clone()));
            }

            let _ = tray_icon.set_tooltip(Some(state_code_tooltip(new_state)));
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
        if state_changed || data_changed || system_stats_changed {
            let snapshot = clone_menu_data_snapshot(&menu_data);
            let new_title = menu_bar_stats_title(&snapshot, svc_state);
            if new_title != last_rendered_menu_bar_title {
                set_menu_bar_stats_indicator(
                    &tray_icon,
                    new_title.as_deref(),
                    match svc_state {
                        ServiceState::Running => &icon_running,
                        _ => &icon_stopped,
                    },
                );
                last_rendered_menu_bar_title = new_title;
                tracing::debug!("tray menu bar title updated");
            }
            last_rendered_system_stats_generation = system_stats_data_generation;
        }

        if should_refresh_menu {
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
                reload_requested,
                action_triggered,
                menu_recently_interacted,
            ) {
                last_rendered_state = new_state;
                last_rendered_operation = new_operation;
                last_rendered_data_generation = data_generation;
                native_menu = NativeMenuState::new(&new_menu_items);
                tray_icon.set_menu(Some(Box::new(native_menu.menu.clone())));
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
    });
}

fn init_logging(data_dir: &Path) {
    let log_dir = data_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    cleanup_old_logs(&log_dir);

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

fn cleanup_old_logs(log_dir: &Path) {
    let cutoff = Duration::from_secs(LOG_RETENTION_DAYS * 24 * 60 * 60);
    let now = SystemTime::now();
    let Ok(entries) = fs::read_dir(log_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("tray.log") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now.duration_since(modified).is_ok_and(|age| age > cutoff) {
            if let Err(error) = fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), error = %error, "failed to remove old tray log");
            }
        }
    }
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
    draw_menu_bar_bifrost_icon(&mut rgba, width, PADDING_X, (HEIGHT - ICON_SIZE) / 2);
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
fn menu_bar_stats_rows(title: &str) -> MenuBarStatsRows {
    let mut lines = title.lines();
    let values = split_menu_bar_stats_row(lines.next().unwrap_or_default());
    let labels = split_menu_bar_stats_row(lines.next().unwrap_or_default());
    MenuBarStatsRows { values, labels }
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
            .map(|text| measure_text_width(font, text, value_font))
            .unwrap_or(0);
        let label_width = rows
            .labels
            .get(idx)
            .map(|text| measure_text_width(font, text, label_font))
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
    value.is_some_and(|text| text.starts_with('↑'))
        && label.is_some_and(|text| text.starts_with('↓'))
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
    {
        return measure_text_width(font, "↓999.9 M/s", value_font_px);
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
        let part_width = measure_text_width(font, part, font_px);
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
fn draw_menu_bar_bifrost_icon(rgba: &mut [u8], canvas_width: u32, x: u32, y: u32) {
    let Some(icon) = menu_bar_bifrost_icon_alpha() else {
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
fn menu_bar_bifrost_icon_alpha() -> Option<&'static MenuBarIconAlpha> {
    static ICON: OnceLock<Option<MenuBarIconAlpha>> = OnceLock::new();
    ICON.get_or_init(|| {
        const ICON_SIZE: u32 = 32;
        let img =
            image::load_from_memory(include_bytes!("../../../../../assets/trayTemplate@2x.png"))
                .ok()?
                .to_rgba8();
        let resized = image::imageops::resize(
            &img,
            ICON_SIZE,
            ICON_SIZE,
            image::imageops::FilterType::Lanczos3,
        );
        let alpha = resized.pixels().map(|pixel| pixel[3]).collect();
        Some(MenuBarIconAlpha {
            alpha,
            width: ICON_SIZE,
            height: ICON_SIZE,
        })
    })
    .as_ref()
}

#[cfg(target_os = "macos")]
fn menu_bar_stats_font() -> Option<&'static fontdue::Font> {
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
struct MenuBarGlyph {
    metrics: fontdue::Metrics,
    bitmap: Vec<u8>,
}

#[cfg(target_os = "macos")]
type MenuBarGlyphCache = Mutex<HashMap<(char, u32), Arc<MenuBarGlyph>>>;

#[cfg(target_os = "macos")]
fn cached_menu_bar_glyph(font: &fontdue::Font, ch: char, font_px: f32) -> Arc<MenuBarGlyph> {
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
    reload_requested: bool,
    action_triggered: bool,
    menu_recently_interacted: bool,
) -> bool {
    reload_requested || action_triggered || !menu_recently_interacted
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
    let path = data_dir.join("config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return TraySystemStatsConfig {
                enabled: true,
                items: bifrost_storage::TraySystemStatsItems::default(),
            };
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to read tray system stats config; keeping system stats enabled"
            );
            return TraySystemStatsConfig {
                enabled: true,
                items: bifrost_storage::TraySystemStatsItems::default(),
            };
        }
    };

    match toml::from_str::<bifrost_storage::UnifiedConfig>(&content) {
        Ok(config) => TraySystemStatsConfig {
            enabled: config.tray.show_system_stats,
            items: config.tray.system_stats_items,
        },
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to parse tray system stats config; keeping system stats enabled"
            );
            TraySystemStatsConfig {
                enabled: true,
                items: bifrost_storage::TraySystemStatsItems::default(),
            }
        }
    }
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
                record_remote_group_failure();
                tracing::warn!(error = %error, "failed to decode group list for tray menu");
                return None;
            }
        },
        Err(error) => {
            record_remote_group_failure();
            tracing::warn!(error = %error, "failed to load group list for tray menu");
            return None;
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

fn remote_group_failure_backoff() -> &'static Mutex<Option<Instant>> {
    static BACKOFF: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    BACKOFF.get_or_init(|| Mutex::new(None))
}

fn remote_group_failure_backoff_active() -> bool {
    match remote_group_failure_backoff().lock() {
        Ok(guard) => {
            guard.is_some_and(|failed_at| failed_at.elapsed() < REMOTE_GROUP_FAILURE_BACKOFF)
        }
        Err(poisoned) => poisoned
            .into_inner()
            .is_some_and(|failed_at| failed_at.elapsed() < REMOTE_GROUP_FAILURE_BACKOFF),
    }
}

fn record_remote_group_failure() {
    match remote_group_failure_backoff().lock() {
        Ok(mut guard) => *guard = Some(Instant::now()),
        Err(poisoned) => *poisoned.into_inner() = Some(Instant::now()),
    }
}

fn clear_remote_group_failure() {
    match remote_group_failure_backoff().lock() {
        Ok(mut guard) => *guard = None,
        Err(poisoned) => *poisoned.into_inner() = None,
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
}

impl NativeMenuState {
    fn new(items: &[MenuEntry]) -> Self {
        let menu = Menu::new();
        let mut action_map = HashMap::new();
        let mut handles = Vec::new();
        let mut shape = Vec::new();

        for entry in items {
            append_menu_entry(&menu, entry, &mut action_map, &mut handles, &mut shape);
        }

        Self {
            menu,
            action_map,
            handles,
            shape,
        }
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
        true
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
        let menu_item = CheckMenuItem::new(&item.label, item.enabled, item.checked, None);
        map.insert(menu_item.id().clone(), item.action.clone());
        menu.append_item(&menu_item);
        handles.push(NativeMenuHandle::Check(menu_item));
    } else {
        let menu_item = MenuItem::new(&item.label, item.enabled, None);
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
) {
    let mut sampler = SystemStatsSampler::new(&args.data_dir);
    loop {
        if quit_flag.load(Ordering::Relaxed) {
            break;
        }

        let stats_config = load_tray_system_stats_config(&args.data_dir);
        if stats_config.visible() {
            let snapshot = sampler.sample(Instant::now(), &stats_config.items);
            let lines = system_stats::menu_lines(&snapshot, &stats_config.items);
            update_system_stats_snapshot(Some(lines), menu_data, generation);
        } else {
            sampler.reset_network_baseline();
            update_system_stats_snapshot(None, menu_data, generation);
        }

        sleep_interruptibly(quit_flag, SYSTEM_STATS_POLL_INTERVAL);
    }
}

#[cfg(target_os = "macos")]
fn update_system_stats_snapshot(
    lines: Option<SystemStatsMenuLines>,
    menu_data: &Arc<Mutex<MenuDataSnapshot>>,
    generation: &AtomicU64,
) -> bool {
    let changed = match menu_data.lock() {
        Ok(mut current) => {
            if current.system_stats == lines {
                false
            } else {
                current.system_stats = lines;
                true
            }
        }
        Err(poisoned) => {
            tracing::warn!("tray menu data snapshot lock was poisoned; updating system stats");
            let mut current = poisoned.into_inner();
            if current.system_stats == lines {
                false
            } else {
                current.system_stats = lines;
                true
            }
        }
    };
    if changed {
        generation.fetch_add(1, Ordering::Relaxed);
    }
    changed
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
    let mut next = load_menu_data_snapshot(args, state, true, include_system_proxy);
    let changed = match menu_data.lock() {
        Ok(mut current) => {
            if !include_system_proxy {
                next.system_proxy = current.system_proxy.clone();
            }
            #[cfg(target_os = "macos")]
            if next.system_stats.is_some() {
                next.system_stats = current.system_stats.clone().or(next.system_stats);
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
