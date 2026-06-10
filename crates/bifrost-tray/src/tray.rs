use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::TrayIconBuilder;

use crate::cli::TrayArgs;
use crate::config::{self, TrayConfig};
use crate::lock::TrayLock;
use crate::menu::{self, MenuEntry, MenuItemAction, MenuItemDef, RuleTarget, SubmenuDef};
use crate::runtime::{self, RuntimeInfo, ServiceState};

const STATE_RUNNING: u8 = 0;
const STATE_STOPPED: u8 = 1;
const STATE_DISCONNECTED: u8 = 2;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(3);
const RESTART_STOP_TIMEOUT: Duration = Duration::from_secs(8);
const LOG_RETENTION_DAYS: u64 = 30;

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
    let bin_available = trusted_bifrost_binary_available(&args);

    let rules = load_rules_for_menu(runtime.as_ref(), state);
    let menu_items = menu::build_menu(
        runtime.as_ref(),
        state,
        custom_config.as_ref(),
        &data_dir_str,
        bin_available,
        &rules,
    );

    let (tray_menu, mut action_map) = build_native_menu(&menu_items);

    let initial_icon = match state {
        ServiceState::Running => &icon_running,
        _ => &icon_stopped,
    };

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip(state_tooltip(state))
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
                let _ = tray_icon.set_tooltip(Some(state_code_tooltip(new_state)));
            }

            // Rebuild menu to reflect new state (enabled/disabled items + status text)
            let svc_state = match new_state {
                STATE_RUNNING => ServiceState::Running,
                STATE_STOPPED => ServiceState::Stopped,
                _ => ServiceState::Disconnected,
            };
            let rt = runtime::read_runtime(&args.runtime_file);
            let new_custom_config = load_custom_config_safe(&args.data_dir);
            let rules = load_rules_for_menu(rt.as_ref(), svc_state);
            let bin_available = trusted_bifrost_binary_available(&args);
            let new_menu_items = menu::build_menu(
                rt.as_ref(),
                svc_state,
                new_custom_config.as_ref(),
                &data_dir_str,
                bin_available,
                &rules,
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
    cleanup_old_logs(&log_dir);

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

#[derive(Debug, Deserialize)]
struct RuleReferenceCandidate {
    rule_name: String,
    group_name: Option<String>,
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
        .into_iter()
        .map(|candidate| {
            let target = match candidate.group_name {
                Some(group_name) => menu::RuleTarget::Group {
                    group_name,
                    name: candidate.rule_name,
                },
                None => menu::RuleTarget::Personal {
                    name: candidate.rule_name,
                },
            };
            let enabled = active_targets.contains(&target);
            menu::TrayRule {
                target,
                enabled,
                sort_order: 0,
            }
        })
        .collect::<Vec<_>>();

    menu::sort_tray_rules(&mut rules);
    rules
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(HTTP_CONNECT_TIMEOUT)
        .timeout_read(HTTP_READ_TIMEOUT)
        .build()
}

fn build_native_menu(items: &[MenuEntry]) -> (Menu, HashMap<MenuId, MenuItemAction>) {
    let menu = Menu::new();
    let mut map = HashMap::new();

    for entry in items {
        append_menu_entry(&menu, entry, &mut map);
    }

    (menu, map)
}

fn append_menu_entry(
    menu: &dyn MenuAppend,
    entry: &MenuEntry,
    map: &mut HashMap<MenuId, MenuItemAction>,
) {
    match entry {
        MenuEntry::Item(item) => append_menu_item(menu, item, map),
        MenuEntry::Submenu(submenu) => append_submenu(menu, submenu, map),
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
) {
    if item.label == "-" {
        menu.append_item(&PredefinedMenuItem::separator());
        return;
    }

    if item.checked || matches!(item.action, MenuItemAction::SelectRule { .. }) {
        let menu_item = CheckMenuItem::new(&item.label, item.enabled, item.checked, None);
        map.insert(menu_item.id().clone(), item.action.clone());
        menu.append_item(&menu_item);
    } else {
        let menu_item = MenuItem::new(&item.label, item.enabled, None);
        map.insert(menu_item.id().clone(), item.action.clone());
        menu.append_item(&menu_item);
    }
}

fn append_submenu(
    menu: &dyn MenuAppend,
    submenu: &SubmenuDef,
    map: &mut HashMap<MenuId, MenuItemAction>,
) {
    let native = Submenu::new(&submenu.label, submenu.enabled);
    for child in &submenu.children {
        append_menu_entry(&native, child, map);
    }
    menu.append_item(&native);
}

fn execute_action(
    action: &MenuItemAction,
    args: &TrayArgs,
    _data_dir_str: &str,
    quit_flag: &AtomicBool,
    reload_flag: &Arc<AtomicBool>,
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
            let runtime_file = args.runtime_file.clone();
            let port = args.port;
            let extra_args = args.start_args.clone();
            thread::spawn(move || {
                let old_pid = runtime::read_runtime(&runtime_file).map(|rt| rt.pid);
                if !spawn_stop(&bin, &data_dir) {
                    return;
                }
                if !wait_for_runtime_pid_exit(old_pid, RESTART_STOP_TIMEOUT) {
                    tracing::error!("timed out waiting for bifrost service to stop before restart");
                    return;
                }
                spawn_start(&bin, &data_dir, port, &extra_args);
            });
        }
        MenuItemAction::OpenDirectory(path) => {
            if let Err(e) = open::that(path) {
                tracing::error!(path = %path, error = %e, "failed to open directory");
            }
        }
        MenuItemAction::SelectRule {
            target,
            all_targets,
        } => {
            let rt = runtime::read_runtime(&args.runtime_file);
            let Some(rt) = rt else {
                tracing::error!("cannot select rule without runtime");
                return;
            };
            let admin_url = rt.admin_url();
            let target = target.clone();
            let all_targets = all_targets.clone();
            let reload_flag = reload_flag.clone();
            thread::spawn(move || {
                if select_single_rule(&admin_url, &target, &all_targets) {
                    reload_flag.store(true, Ordering::Relaxed);
                }
            });
        }
        MenuItemAction::QuitTray => {
            tracing::info!("quit tray requested");
            quit_flag.store(true, Ordering::Relaxed);
        }
        MenuItemAction::None => {}
    }
}

fn select_single_rule(admin_url: &str, target: &RuleTarget, all_targets: &[RuleTarget]) -> bool {
    let agent = http_agent();
    let mut success = true;
    for candidate in all_targets {
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

fn spawn_start(bin: &Path, data_dir: &str, port: Option<u16>, extra_args: &[String]) {
    let mut cmd = Command::new(bin);
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
    configure_service_command(&mut cmd);
    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(pid = child.id(), "bifrost service started");
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to start bifrost service");
        }
    }
}

fn spawn_stop(bin: &Path, data_dir: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.env("BIFROST_DATA_DIR", data_dir).arg("stop");
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

fn wait_for_runtime_pid_exit(pid: Option<u32>, timeout: Duration) -> bool {
    let Some(pid) = pid else {
        return true;
    };
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !runtime::is_process_running(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    !runtime::is_process_running(pid)
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

fn poll_service_state(quit_flag: &AtomicBool, state: &AtomicU8, args: &TrayArgs) {
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
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn spawn_test_http_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_thread = Arc::clone(&seen);
        let handle = thread::spawn(move || {
            for (status_line, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 2048];
                let n = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                if let Some(first_line) = request.lines().next() {
                    seen_for_thread.lock().unwrap().push(first_line.to_string());
                }
                let response = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{addr}/_bifrost/"), seen, handle)
    }

    #[test]
    fn test_rule_toggle_url_for_personal_rule() {
        let target = RuleTarget::Personal {
            name: "qa rule".to_string(),
        };
        let url = rule_toggle_url("http://127.0.0.1:8800/_bifrost/", &target, true);
        assert_eq!(
            url,
            "http://127.0.0.1:8800/_bifrost/api/rules/qa%20rule/enable"
        );
    }

    #[test]
    fn test_rule_toggle_url_for_group_rule() {
        let target = RuleTarget::Group {
            group_name: "Team A".to_string(),
            name: "shared/rule".to_string(),
        };
        let url = rule_toggle_url("http://127.0.0.1:8800/_bifrost/", &target, false);
        assert_eq!(
            url,
            "http://127.0.0.1:8800/_bifrost/api/group-rules/Team%20A/shared%2Frule/disable"
        );
    }

    #[test]
    fn test_load_rules_from_admin_marks_active_personal_and_group_rules() {
        let (admin_url, seen, handle) = spawn_test_http_server(vec![
            (
                "HTTP/1.1 200 OK",
                r#"[
                    {"name":"alpha","rule_name":"alpha","group_name":null,"group_id":null},
                    {"name":"Team A/shared","rule_name":"shared","group_name":"Team A","group_id":"grp-a"}
                ]"#,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"total":1,"rules":[{"name":"shared","rule_count":1,"group_id":"grp-a","group_name":"Team A"}],"variable_conflicts":[],"merged_content":""}"#,
            ),
        ]);

        let rules = load_rules_from_admin(&admin_url);
        handle.join().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                "GET /_bifrost/api/rules/reference-candidates HTTP/1.1",
                "GET /_bifrost/api/rules/active-summary HTTP/1.1",
            ]
        );
        assert_eq!(rules.len(), 2);
        let personal = rules
            .iter()
            .find(|rule| {
                rule.target
                    == RuleTarget::Personal {
                        name: "alpha".to_string(),
                    }
            })
            .unwrap();
        assert!(!personal.enabled);
        let group = rules
            .iter()
            .find(|rule| {
                rule.target
                    == RuleTarget::Group {
                        group_name: "Team A".to_string(),
                        name: "shared".to_string(),
                    }
            })
            .unwrap();
        assert!(group.enabled);
    }

    #[test]
    fn test_select_single_rule_calls_admin_api_for_disable_then_enable() {
        let (admin_url, seen, handle) = spawn_test_http_server(vec![
            ("HTTP/1.1 200 OK", r#"{"success":true}"#),
            ("HTTP/1.1 200 OK", r#"{"success":true}"#),
        ]);
        let target = RuleTarget::Personal {
            name: "beta".to_string(),
        };
        let all_targets = vec![
            RuleTarget::Personal {
                name: "alpha".to_string(),
            },
            target.clone(),
        ];

        assert!(select_single_rule(&admin_url, &target, &all_targets));
        handle.join().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                "PUT /_bifrost/api/rules/alpha/disable HTTP/1.1",
                "PUT /_bifrost/api/rules/beta/enable HTTP/1.1",
            ]
        );
    }
}
