use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use bifrost_core::upgrade_progress::{
    consume_desktop_upgrade_origin_token, is_stale, read_progress, write_progress, UpgradePhase,
    UpgradeProgress, DEFAULT_STALE_SECS,
};
use bifrost_storage::data_dir;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tracing::warn;

use super::{
    error_response, json_response, json_response_with_status, method_not_allowed, BoxBody,
};
use crate::metrics::SystemInfo;
use crate::resource_alerts::build_resource_alerts;
use crate::state::SharedAdminState;

mod version_companion;
use version_companion::{
    desktop_app_version_for_version_check, standalone_cli_version_for_version_check,
};

const DESKTOP_INSTALL_SKILL_TIMEOUT: Duration = Duration::from_secs(20);
const DESKTOP_CORE_ENV: &str = "BIFROST_DESKTOP_CORE";
const DESKTOP_UPGRADE_HANDOFF_ENV: &str = "BIFROST_DESKTOP_UPGRADE_HANDOFF";
const WEBVIEW_UPGRADE_ORIGIN_ENV: &str = "BIFROST_WEBVIEW_UPGRADE_ORIGIN_INTERNAL";
const DESKTOP_UPGRADE_ORIGIN_HEADER: &str = "x-bifrost-desktop-upgrade-origin";

fn upgrade_start_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub async fn handle_system(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();
    let query = req.uri().query().map(str::to_owned);
    let desktop_origin_token = req
        .headers()
        .get(DESKTOP_UPGRADE_ORIGIN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    match path {
        "/api/system" | "/api/system/" => match method {
            Method::GET => get_system_info(state).await,
            _ => method_not_allowed(),
        },
        "/api/system/overview" => match method {
            Method::GET => get_overview(state).await,
            _ => method_not_allowed(),
        },
        "/api/system/memory" | "/api/system/memory/" => match method {
            Method::GET => get_memory_diagnostics(state).await,
            _ => method_not_allowed(),
        },
        "/api/system/version-check" => match method {
            Method::GET => check_version(state, query.as_deref()).await,
            _ => method_not_allowed(),
        },
        "/api/system/upgrade" => match method {
            Method::POST => {
                start_upgrade(state, query.as_deref(), desktop_origin_token.as_deref()).await
            }
            _ => method_not_allowed(),
        },
        "/api/system/upgrade/progress" => match method {
            Method::GET => get_upgrade_progress().await,
            _ => method_not_allowed(),
        },
        "/api/system/cli-install" => match method {
            Method::GET => get_cli_install_status().await,
            Method::POST => install_cli_from_desktop(req).await,
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn get_system_info(state: SharedAdminState) -> Response<BoxBody> {
    let info = SystemInfo::new(state.start_time);
    json_response(&info)
}

#[derive(Debug, serde::Serialize)]
struct ProcessMemoryInfo {
    pid: u32,
    /// 进程 RSS（KiB），来自 sysinfo
    rss_kib: u64,
    /// 进程虚拟内存（KiB），来自 sysinfo
    vms_kib: u64,
    /// 进程 CPU 使用率（%），来自 sysinfo
    cpu_usage_percent: f32,
    /// 系统总内存（KiB），来自 sysinfo
    system_total_kib: u64,
}

#[derive(Debug, serde::Serialize)]
struct AdminMemoryDiagnostics {
    system: SystemInfo,
    process: ProcessMemoryInfo,
    traffic_db: Option<serde_json::Value>,
    connections: serde_json::Value,
    stores: serde_json::Value,
}

async fn get_memory_diagnostics(state: SharedAdminState) -> Response<BoxBody> {
    // 进程级信息：这里做一次“即时刷新”，避免仅依赖 metrics 缓存。
    let pid_u32 = std::process::id();
    let pid = Pid::from_u32(pid_u32);
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]));

    let (rss_kib, vms_kib, cpu_usage_percent) = if let Some(p) = system.process(pid) {
        (p.memory(), p.virtual_memory(), p.cpu_usage())
    } else {
        warn!(pid = pid_u32, "[SYSTEM] sysinfo missing process info");
        (0, 0, 0.0)
    };

    let process = ProcessMemoryInfo {
        pid: pid_u32,
        rss_kib,
        vms_kib,
        cpu_usage_percent,
        system_total_kib: system.total_memory(),
    };

    let traffic_db = state.traffic_db_store.as_ref().map(|db| {
        let stats = db.stats();
        let cache = db.recent_cache_stats();
        serde_json::json!({
            "db": stats,
            "recent_cache": cache,
        })
    });

    // 连接/活跃对象：这些通常是“管理端页面关闭后依然可能占用内存”的主要线索。
    let ws_monitor_stats = state.connection_monitor.memory_stats();
    let tunnel_registry_active = state.connection_registry.active_count();
    let sse_total = state.sse_hub.connection_count();
    let sse_open = state.sse_hub.open_connection_count();

    let connections = serde_json::json!({
        "tunnel_registry_active": tunnel_registry_active,
        "ws_monitor": ws_monitor_stats,
        "sse": {
            "connections": sse_total,
            "open": sse_open,
        }
    });

    // 存储/缓存（主要是“内存侧的 pending/buffer”与“磁盘侧 size”同时给出，便于区分 RSS vs 磁盘占用）。
    let body_store = state.body_store.as_ref().map(|s| s.read().stats());
    let frame_store_stats = state.frame_store.as_ref().map(|s| {
        serde_json::json!({
            "disk": s.stats(),
            "memory": s.memory_stats(),
        })
    });
    let ws_payload_store = state.ws_payload_store.as_ref().map(|s| s.stats());
    let ws_payload_store_stats = state.ws_payload_store.as_ref().map(|s| {
        serde_json::json!({
            "disk": s.stats(),
            "memory": s.memory_stats(),
        })
    });
    let resource_alerts = build_resource_alerts(body_store.as_ref(), ws_payload_store.as_ref());

    let stores = serde_json::json!({
        "body_store": body_store,
        "frame_store": frame_store_stats,
        "ws_payload_store": ws_payload_store_stats,
        "resource_alerts": resource_alerts,
        "max_body_buffer_size": state.get_max_body_buffer_size(),
        "max_body_probe_size": state.get_max_body_probe_size(),
    });

    let out = AdminMemoryDiagnostics {
        system: SystemInfo::new(state.start_time),
        process,
        traffic_db,
        connections,
        stores,
    };

    json_response(&out)
}

async fn get_overview(state: SharedAdminState) -> Response<BoxBody> {
    let system_info = SystemInfo::new(state.start_time);
    let metrics = state.metrics_collector.get_current();
    let traffic_count = if let Some(ref db_store) = state.traffic_db_store {
        db_store.count()
    } else {
        0
    };

    let (rules_total, rules_enabled) = match state.rules_storage.load_all() {
        Ok(rules) => {
            let enabled = rules.iter().filter(|r| r.enabled).count();
            (rules.len(), enabled)
        }
        Err(_) => (0, 0),
    };

    let pending_count = if let Some(ref access_control) = state.access_control {
        let ac = access_control.read().await;
        ac.pending_authorization_count()
    } else {
        0
    };

    let overview = serde_json::json!({
        "system": system_info,
        "metrics": metrics,
        "rules": {
            "total": rules_total,
            "enabled": rules_enabled
        },
        "traffic": {
            "recorded": traffic_count
        },
        "server": {
            "port": state.port(),
            "admin_url": format!("http://127.0.0.1:{}/_bifrost/", state.port())
        },
        "pending_authorizations": pending_count
    });

    json_response(&overview)
}

async fn check_version(state: SharedAdminState, query: Option<&str>) -> Response<BoxBody> {
    let force_refresh = query
        .map(|q| q.contains("refresh=true") || q.contains("refresh=1"))
        .unwrap_or(false);
    let channel = parse_upgrade_channel(query);

    let response = check_unified_version_for_channel(state, force_refresh, channel).await;
    json_response(&response)
}

/// `POST /api/system/upgrade` — trigger an unattended background upgrade.
///
/// Returns `409 Conflict` when there is no newer version available or an
/// upgrade is already in flight (non-stale active progress). On success it
/// writes an initial `Checking` progress record, spawns a detached
/// `bifrost self-update --source admin` subprocess and returns `202 Accepted`
/// with the current progress snapshot. The server process owns channel
/// selection: the runtime owner selects both the component version gate and the
/// orchestrator, so a stale/conflicting UI query cannot start the wrong flow.
///
/// The upgrade is never executed inside the admin process: a CLI-owned core is
/// stopped and restarted by the detached subprocess, while a desktop-owned core
/// remains alive until the App handoff replaces it. The progress file survives
/// either handoff so the Web UI can read the terminal state after reconnecting.
async fn start_upgrade(
    state: SharedAdminState,
    query: Option<&str>,
    desktop_origin_token: Option<&str>,
) -> Response<BoxBody> {
    // Serialize check -> claim -> spawn. Without this process-local critical
    // section, two simultaneous Web UI POSTs can both observe an idle progress
    // file and launch competing installers/restarts.
    let _start_guard = upgrade_start_lock().lock().await;
    let dir = data_dir();
    let (requested_channel, channel, running_proxy) = upgrade_request_plan(
        query,
        desktop_core_env_enabled(std::env::var_os(DESKTOP_CORE_ENV)),
        std::process::id(),
        state.port(),
    );
    let webview_origin =
        validated_webview_upgrade_origin(&dir, requested_channel, desktop_origin_token);
    if let Err(message) =
        validate_upgrade_request_channel(requested_channel, channel, webview_origin)
    {
        return error_response(StatusCode::CONFLICT, message);
    }

    // Refuse if an upgrade is already running and still alive.
    let current = read_progress(&dir);
    if current.is_active() && !is_stale(&current, DEFAULT_STALE_SECS) {
        return error_response(StatusCode::CONFLICT, "An upgrade is already in progress");
    }

    // Confirm there is actually a newer version to upgrade to.
    let version = check_unified_version_for_channel(state, false, channel).await;
    if !version.has_update {
        return error_response(StatusCode::CONFLICT, "No update available");
    }
    let target_version = match required_upgrade_target(&version) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, message),
    };

    // Seed the channel so the Web UI sees movement immediately, before the
    // subprocess has had a chance to start writing.
    let initial = UpgradeProgress::new(UpgradePhase::Checking, "Checking for updates…")
        .with_target(Some(target_version.clone()))
        .with_source(Some(channel.progress_source().to_string()));
    write_progress(&dir, &initial);

    if let Err(error) = spawn_upgrade_process(
        channel,
        Some(target_version.as_str()),
        running_proxy,
        None,
        webview_origin,
    ) {
        warn!(error = %error, "[SYSTEM] failed to spawn self-update subprocess");
        let failed = UpgradeProgress::new(UpgradePhase::Failed, "Upgrade failed to start")
            .with_target(Some(target_version))
            .with_source(Some(channel.progress_source().to_string()))
            .with_error(Some(error.to_string()));
        write_progress(&dir, &failed);
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to start upgrade process",
        );
    }

    json_response_with_status(StatusCode::ACCEPTED, &initial)
}

/// `GET /api/system/upgrade/progress` — return the current upgrade progress.
///
/// A stale active record (no update within [`DEFAULT_STALE_SECS`]) is
/// normalized to `Failed` so the UI never hangs on a crashed/abandoned upgrade.
async fn get_upgrade_progress() -> Response<BoxBody> {
    let dir = data_dir();
    let progress = normalize_progress(read_progress(&dir));
    json_response(&progress)
}

#[derive(Debug, Deserialize, Default)]
struct CliInstallRequest {
    install_dir: Option<PathBuf>,
    install_skills: Option<bool>,
    dry_run: Option<bool>,
}

#[derive(Debug, Serialize)]
struct CliInstallResponse {
    installed: bool,
    install_path: String,
    install_dir: String,
    current_exe: String,
    in_path: bool,
    path_hint: Option<String>,
    skills_installed: Option<bool>,
    skills_message: Option<String>,
    dry_run: bool,
}

async fn get_cli_install_status() -> Response<BoxBody> {
    match build_cli_install_status(None, None, false) {
        Ok(response) => json_response(&response),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn install_cli_from_desktop(req: Request<Incoming>) -> Response<BoxBody> {
    let request = match read_optional_cli_install_request(req).await {
        Ok(request) => request,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    match install_cli_from_current_exe(request) {
        Ok(response) => json_response(&response),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn read_optional_cli_install_request(
    req: Request<Incoming>,
) -> Result<CliInstallRequest, String> {
    let bytes = req
        .into_body()
        .collect()
        .await
        .map_err(|error| format!("read request body: {error}"))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(CliInstallRequest::default());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid request body: {error}"))
}

fn install_cli_from_current_exe(
    request: CliInstallRequest,
) -> Result<CliInstallResponse, std::io::Error> {
    let install_dir = resolve_cli_install_dir(request.install_dir);
    let install_path = install_dir.join(cli_binary_name());
    let current_exe = std::env::current_exe()?;
    let dry_run = request.dry_run.unwrap_or(false);

    if dry_run {
        return build_cli_install_status(Some(install_dir), Some(current_exe), true);
    }

    fs::create_dir_all(&install_dir)?;
    install_binary_atomically(&current_exe, &install_path)?;
    #[cfg(target_os = "macos")]
    clear_macos_xattrs(&install_path);

    let mut response = build_cli_install_status(Some(install_dir), Some(current_exe), false)?;
    response.installed = install_path.exists();

    if request.install_skills.unwrap_or(true) {
        match run_desktop_install_skill(&install_path, DESKTOP_INSTALL_SKILL_TIMEOUT) {
            Ok(DesktopInstallSkillStatus::Success) => {
                response.skills_installed = Some(true);
                response.skills_message =
                    Some("Bifrost AI skills installed from embedded desktop bundle".to_string());
            }
            Ok(DesktopInstallSkillStatus::Failed(message)) => {
                response.skills_installed = Some(false);
                response.skills_message = Some(format!(
                    "{message}; retry with `bifrost install-skill --tool all -y`"
                ));
            }
            Ok(DesktopInstallSkillStatus::TimedOut) => {
                response.skills_installed = Some(false);
                response.skills_message = Some(format!(
                    "install-skill timed out after {}s; retry with `bifrost install-skill --tool all -y`",
                    DESKTOP_INSTALL_SKILL_TIMEOUT.as_secs()
                ));
            }
            Err(error) => {
                response.skills_installed = Some(false);
                response.skills_message = Some(format!(
                    "install-skill failed: {error}; retry with `bifrost install-skill --tool all -y`"
                ));
            }
        }
    } else {
        response.skills_installed = None;
        response.skills_message = Some("Bifrost AI skill installation skipped".to_string());
    }

    Ok(response)
}

#[derive(Debug, PartialEq, Eq)]
enum DesktopInstallSkillStatus {
    Success,
    Failed(String),
    TimedOut,
}

fn run_desktop_install_skill(
    install_path: &Path,
    timeout: Duration,
) -> std::io::Result<DesktopInstallSkillStatus> {
    let mut child = Command::new(install_path)
        .args(["install-skill", "--tool", "all", "-y"])
        .env("BIFROST_INSTALL_SKILL_SOURCE", "embedded")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(if status.success() {
                DesktopInstallSkillStatus::Success
            } else {
                DesktopInstallSkillStatus::Failed(format!(
                    "install-skill exited with status {status}"
                ))
            });
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(DesktopInstallSkillStatus::TimedOut);
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn build_cli_install_status(
    install_dir: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    dry_run: bool,
) -> Result<CliInstallResponse, std::io::Error> {
    let install_dir = resolve_cli_install_dir(install_dir);
    let install_path = install_dir.join(cli_binary_name());
    let current_exe = match current_exe {
        Some(path) => path,
        None => std::env::current_exe()?,
    };
    let in_path = path_contains_dir(&install_dir);
    Ok(CliInstallResponse {
        installed: install_path.exists(),
        install_path: install_path.display().to_string(),
        install_dir: install_dir.display().to_string(),
        current_exe: current_exe.display().to_string(),
        in_path,
        path_hint: if in_path {
            None
        } else {
            Some(cli_path_hint(&install_dir))
        },
        skills_installed: None,
        skills_message: None,
        dry_run,
    })
}

fn resolve_cli_install_dir(override_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir;
    }
    if let Some(dir) = std::env::var_os("BIFROST_INSTALL_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("bifrost").join("bin");
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local").join("bin");
        }
    }
    std::env::temp_dir().join("bifrost-bin")
}

fn cli_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "bifrost.exe"
    }
    #[cfg(not(windows))]
    {
        "bifrost"
    }
}

fn install_binary_atomically(source: &Path, target: &Path) -> Result<(), std::io::Error> {
    if same_file_path(source, target)? {
        return Ok(());
    }

    let tmp = target.with_extension(format!(
        "{}.tmp.{}",
        target
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin"),
        std::process::id()
    ));
    let _ = fs::remove_file(&tmp);
    fs::copy(source, &tmp)?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&tmp)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&tmp, permissions)?;
    }
    fs::rename(&tmp, target).or_else(|error| {
        let _ = fs::remove_file(target);
        fs::rename(&tmp, target).map_err(|_| error)
    })
}

fn same_file_path(source: &Path, target: &Path) -> Result<bool, std::io::Error> {
    let source = fs::canonicalize(source)?;
    let target = match fs::canonicalize(target) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };

    #[cfg(windows)]
    {
        Ok(source
            .to_string_lossy()
            .eq_ignore_ascii_case(&target.to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        Ok(source == target)
    }
}

fn path_contains_dir(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|entry| {
                let entry = fs::canonicalize(&entry).unwrap_or(entry);
                let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
                entry == dir
            })
        })
        .unwrap_or(false)
}

fn cli_path_hint(dir: &Path) -> String {
    #[cfg(windows)]
    {
        format!(
            "Add {} to your Windows User PATH, then restart PowerShell/CMD.",
            dir.display()
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "Add `export PATH=\"{}:$PATH\"` to your shell profile.",
            dir.display()
        )
    }
}

#[cfg(target_os = "macos")]
fn clear_macos_xattrs(path: &Path) {
    let _ = Command::new("xattr")
        .args(["-cr"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("xattr")
        .args(["-d", "com.apple.provenance"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Normalize a progress snapshot for readers: a stale active record (no update
/// within [`DEFAULT_STALE_SECS`]) is mapped to `Failed` so the UI never hangs on
/// a crashed/abandoned upgrade process. Non-stale and terminal records pass
/// through unchanged.
fn normalize_progress(progress: UpgradeProgress) -> UpgradeProgress {
    if progress.is_active() && is_stale(&progress, DEFAULT_STALE_SECS) {
        return UpgradeProgress::new(UpgradePhase::Failed, "Upgrade process is not responding")
            .with_target(progress.target_version.clone())
            .with_source(progress.source.clone())
            .with_error(Some("Upgrade process stopped responding".to_string()));
    }
    progress
}

/// Spawn the detached upgrade orchestrator selected for the current runtime.
///
/// The binary is resolved via `current_exe()` (the admin server runs inside the
/// bifrost process, so `current_exe` is bifrost itself), falling back to a bare
/// `bifrost` looked up on `PATH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpgradeChannel {
    Cli,
    Desktop,
}

impl UpgradeChannel {
    fn progress_source(self) -> &'static str {
        match self {
            Self::Cli => "admin",
            Self::Desktop => "desktop",
        }
    }
}

async fn check_version_for_channel(
    state: SharedAdminState,
    force_refresh: bool,
    channel: UpgradeChannel,
) -> crate::VersionCheckResponse {
    if channel == UpgradeChannel::Desktop {
        if let Some(app_version) = desktop_app_version_for_version_check() {
            return state
                .version_checker
                .check_with_current_version(force_refresh, app_version)
                .await;
        }
    }
    state.version_checker.check(force_refresh).await
}

async fn check_unified_version_for_channel(
    state: SharedAdminState,
    force_refresh: bool,
    primary_channel: UpgradeChannel,
) -> crate::VersionCheckResponse {
    let primary = check_version_for_channel(state.clone(), force_refresh, primary_channel).await;
    let companion = match primary_channel {
        UpgradeChannel::Cli => {
            check_version_for_channel(state, false, UpgradeChannel::Desktop).await
        }
        UpgradeChannel::Desktop => {
            if desktop_version_check_uses_standalone_cli(desktop_core_env_enabled(
                std::env::var_os(DESKTOP_CORE_ENV),
            )) {
                let standalone_cli_version =
                    tokio::task::spawn_blocking(standalone_cli_version_for_version_check)
                        .await
                        .ok()
                        .flatten();
                if let Some(cli_version) = standalone_cli_version {
                    state
                        .version_checker
                        .check_with_current_version(false, cli_version)
                        .await
                } else {
                    check_version_for_channel(state, false, UpgradeChannel::Cli).await
                }
            } else {
                // A desktop WebView can reuse a CLI-owned core. In that mode
                // the serving executable is the effective CLI companion; do
                // not skip it and accidentally select an inactive old copy
                // elsewhere on PATH (for example ~/.cargo/bin/bifrost).
                check_version_for_channel(state, false, UpgradeChannel::Cli).await
            }
        }
    };
    merge_companion_update(primary, companion)
}

fn merge_companion_update(
    mut primary: crate::VersionCheckResponse,
    companion: crate::VersionCheckResponse,
) -> crate::VersionCheckResponse {
    if !primary.has_update && companion.has_update {
        primary.has_update = true;
        primary.current_version = companion.current_version;
        primary.latest_version = companion.latest_version;
        primary.release_highlights = companion.release_highlights;
        primary.release_url = companion.release_url;
        primary.checked_at = companion.checked_at;
    }
    primary
}

fn desktop_version_check_uses_standalone_cli(desktop_core: bool) -> bool {
    desktop_core
}

fn required_upgrade_target(version: &crate::VersionCheckResponse) -> Result<String, &'static str> {
    version
        .latest_version
        .clone()
        .ok_or("Update metadata did not include a target version")
}

fn parse_upgrade_channel(query: Option<&str>) -> UpgradeChannel {
    if query.unwrap_or_default().split('&').any(|part| {
        matches!(
            part,
            "channel=desktop" | "target=desktop" | "source=desktop"
        )
    }) {
        UpgradeChannel::Desktop
    } else {
        UpgradeChannel::Cli
    }
}

fn desktop_core_env_enabled(value: Option<std::ffi::OsString>) -> bool {
    value
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn effective_upgrade_channel(_requested: UpgradeChannel, desktop_core: bool) -> UpgradeChannel {
    if desktop_core {
        UpgradeChannel::Desktop
    } else {
        UpgradeChannel::Cli
    }
}

fn validate_upgrade_request_channel(
    requested: UpgradeChannel,
    orchestrator: UpgradeChannel,
    webview_origin: bool,
) -> Result<(), &'static str> {
    if orchestrator == UpgradeChannel::Desktop
        && (requested != UpgradeChannel::Desktop || !webview_origin)
    {
        return Err("A desktop-owned core must be upgraded from the Bifrost desktop app");
    }
    Ok(())
}

fn validated_webview_upgrade_origin(
    data_dir: &Path,
    requested: UpgradeChannel,
    token: Option<&str>,
) -> bool {
    requested == UpgradeChannel::Desktop
        && token.is_some_and(|token| consume_desktop_upgrade_origin_token(data_dir, token))
}

fn upgrade_request_plan(
    query: Option<&str>,
    desktop_core: bool,
    pid: u32,
    port: u16,
) -> (UpgradeChannel, UpgradeChannel, Option<(u32, u16)>) {
    let requested = parse_upgrade_channel(query);
    let orchestrator = effective_upgrade_channel(requested, desktop_core);
    let running_proxy = (orchestrator == UpgradeChannel::Cli).then_some((pid, port));
    (requested, orchestrator, running_proxy)
}

fn spawn_upgrade_process(
    channel: UpgradeChannel,
    target_version: Option<&str>,
    running_proxy: Option<(u32, u16)>,
    app_dir: Option<&Path>,
    webview_origin: bool,
) -> std::io::Result<()> {
    let program = std::env::current_exe().unwrap_or_else(|_| "bifrost".into());

    let mut command = Command::new(&program);
    command.args(upgrade_process_args(
        channel,
        target_version,
        running_proxy,
        app_dir,
    ));
    for (key, value) in upgrade_process_environment(channel, webview_origin) {
        command.env(key, value);
    }
    command
        .stdin(Stdio::null())
        .stdout(upgrade_log_stdio())
        .stderr(upgrade_log_stdio());

    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    let mut child = command.spawn()?;
    let child_pid = child.id();
    tracing::info!(
        target: "bifrost_admin::system",
        child_pid,
        program = %program.display(),
        channel = ?channel,
        target_version = target_version.unwrap_or("latest"),
        "spawned background self-update subprocess"
    );
    thread::spawn(move || match child.wait() {
        Ok(status) => tracing::info!(
            target: "bifrost_admin::system",
            child_pid,
            status = %status,
            "background self-update subprocess exited"
        ),
        Err(error) => tracing::warn!(
            target: "bifrost_admin::system",
            child_pid,
            error = %error,
            "failed to reap background self-update subprocess"
        ),
    });
    Ok(())
}

fn upgrade_process_environment(
    channel: UpgradeChannel,
    webview_origin: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut environment = Vec::new();
    if webview_origin {
        // Preserve the initiating surface independently from runtime owner.
        // A CLI-owned core can still be controlled by the desktop WebView.
        environment.push((WEBVIEW_UPGRADE_ORIGIN_ENV, "1"));
    }
    if channel == UpgradeChannel::Desktop {
        // Only the App-owned orchestrator itself leaves installation/restart
        // work for the Tauri shell. CLI-owned orchestration decides this later
        // after it has also checked whether the installed shell is running.
        environment.push((DESKTOP_UPGRADE_HANDOFF_ENV, "1"));
    }
    environment
}

fn upgrade_process_args(
    channel: UpgradeChannel,
    target_version: Option<&str>,
    running_proxy: Option<(u32, u16)>,
    app_dir: Option<&Path>,
) -> Vec<String> {
    let mut args = Vec::new();
    match channel {
        UpgradeChannel::Cli => {
            args.push("self-update".to_string());
            if let Some(target) = target_version {
                args.push("--target".to_string());
                args.push(target.to_string());
            }
            args.push("--source".to_string());
            args.push("admin".to_string());
            if let Some((pid, port)) = running_proxy {
                args.push("--running-proxy-pid".to_string());
                args.push(pid.to_string());
                args.push("--running-proxy-port".to_string());
                args.push(port.to_string());
            }
        }
        UpgradeChannel::Desktop => {
            args.push("app".to_string());
            args.push("upgrade".to_string());
            if let Some(target) = target_version {
                args.push("--version".to_string());
                args.push(target.to_string());
            }
            args.push("--source".to_string());
            args.push("desktop".to_string());
            if let Some(app_dir) = app_dir {
                args.push("--app-dir".to_string());
                args.push(app_dir.to_string_lossy().into_owned());
            }
            args.push("-y".to_string());
        }
    }
    args
}

fn upgrade_log_stdio() -> Stdio {
    let log_dir = data_dir().join("logs");
    if std::fs::create_dir_all(&log_dir).is_ok() {
        let log_path = log_dir.join("upgrade-background.log");
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(log_path) {
            return Stdio::from(file);
        }
    }
    Stdio::null()
}

#[cfg(test)]
mod tests {
    use super::{
        build_cli_install_status, check_unified_version_for_channel, check_version,
        desktop_core_env_enabled, desktop_version_check_uses_standalone_cli,
        effective_upgrade_channel, install_binary_atomically, install_cli_from_current_exe,
        merge_companion_update, normalize_progress, parse_upgrade_channel, required_upgrade_target,
        spawn_upgrade_process, start_upgrade, upgrade_process_args, upgrade_process_environment,
        upgrade_request_plan, upgrade_start_lock, validate_upgrade_request_channel,
        validated_webview_upgrade_origin, CliInstallRequest, StatusCode, UpgradeChannel,
    };
    use bifrost_core::upgrade_progress::{UpgradePhase, UpgradeProgress, DEFAULT_STALE_SECS};
    use chrono::Utc;
    use std::path::Path;
    use std::sync::Arc;

    fn version_response(
        current: &str,
        latest: &str,
        has_update: bool,
    ) -> crate::VersionCheckResponse {
        crate::VersionCheckResponse {
            has_update,
            current_version: current.to_string(),
            latest_version: Some(latest.to_string()),
            release_highlights: vec![format!("release {latest}")],
            release_url: Some(format!("https://example.test/v{latest}")),
            checked_at: Some("2026-07-18T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn companion_version_drift_keeps_the_unified_update_available() {
        let primary = version_response("0.0.156", "0.0.156", false);
        let companion = version_response("0.0.155", "0.0.156", true);
        let merged = merge_companion_update(primary, companion);
        assert!(merged.has_update);
        assert_eq!(merged.current_version, "0.0.155");
        assert_eq!(merged.latest_version.as_deref(), Some("0.0.156"));

        let primary_update = version_response("0.0.154", "0.0.156", true);
        let companion_update = version_response("0.0.155", "0.0.157", true);
        assert_eq!(
            merge_companion_update(primary_update, companion_update)
                .latest_version
                .as_deref(),
            Some("0.0.156"),
            "the primary target remains pinned when both components need an update"
        );
    }

    #[test]
    fn desktop_version_check_only_probes_standalone_cli_for_desktop_owned_core() {
        assert!(desktop_version_check_uses_standalone_cli(true));
        assert!(!desktop_version_check_uses_standalone_cli(false));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn desktop_unified_version_check_executes_both_runtime_owner_paths() {
        const CHILD_ENV: &str = "BIFROST_TEST_DESKTOP_VERSION_OWNER_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = std::process::Command::new(
                std::env::current_exe().expect("current test executable"),
            )
            .args([
                "--exact",
                "handlers::system::tests::desktop_unified_version_check_executes_both_runtime_owner_paths",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated desktop version owner test");
            assert!(
                status.success(),
                "isolated desktop version owner test failed"
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let cli_dir = dir.path().join("bin");
        std::fs::create_dir_all(&cli_dir).expect("create CLI dir");
        let cli_path = cli_dir.join("bifrost");
        std::fs::write(&cli_path, "#!/bin/sh\nprintf 'bifrost 0.0.1\\n'\n")
            .expect("write stale CLI fixture");
        let mut permissions = std::fs::metadata(&cli_path)
            .expect("stale CLI metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&cli_path, permissions).expect("make stale CLI executable");
        std::env::set_var("BIFROST_DATA_DIR", dir.path());
        std::env::set_var("BIFROST_APP_INSTALL_DIR", dir.path().join("missing-app"));
        std::env::set_var("PATH", &cli_dir);
        std::fs::write(
            dir.path().join("version_cache.json"),
            serde_json::json!({
                "latest_version": env!("CARGO_PKG_VERSION"),
                "release_highlights": ["runtime owner test"],
                "checked_at": Utc::now(),
            })
            .to_string(),
        )
        .expect("write fresh version cache");
        let state = Arc::new(crate::state::AdminState::new(0));
        std::env::remove_var(super::DESKTOP_CORE_ENV);
        let cli_owned =
            check_unified_version_for_channel(state.clone(), false, UpgradeChannel::Desktop).await;
        assert!(!cli_owned.has_update);
        assert_eq!(cli_owned.current_version, env!("CARGO_PKG_VERSION"));
        std::env::set_var(super::DESKTOP_CORE_ENV, "1");
        let app_owned =
            check_unified_version_for_channel(state.clone(), false, UpgradeChannel::Desktop).await;
        assert!(app_owned.has_update);
        assert_eq!(app_owned.current_version, "0.0.1");
        assert_eq!(
            app_owned.latest_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        std::fs::write(&cli_path, "#!/bin/sh\nexit 1\n").expect("break CLI version fixture");
        let app_owned_without_cli_version =
            check_unified_version_for_channel(state, false, UpgradeChannel::Desktop).await;
        assert!(!app_owned_without_cli_version.has_update);
        assert_eq!(
            app_owned_without_cli_version.current_version,
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn upgrade_requires_a_pinned_target_in_release_metadata() {
        let valid = version_response("0.0.155", "0.0.156", true);
        assert_eq!(required_upgrade_target(&valid).as_deref(), Ok("0.0.156"));

        let mut missing = valid;
        missing.latest_version = None;
        assert_eq!(
            required_upgrade_target(&missing),
            Err("Update metadata did not include a target version")
        );
    }

    #[test]
    fn stale_active_progress_is_normalized_to_failed() {
        let mut progress = UpgradeProgress::new(UpgradePhase::Downloading, "Downloading…")
            .with_target(Some("0.0.104".to_string()))
            .with_source(Some("admin".to_string()));
        progress.updated_at =
            (Utc::now() - chrono::Duration::seconds(DEFAULT_STALE_SECS + 30)).to_rfc3339();

        let normalized = normalize_progress(progress);
        assert_eq!(normalized.phase, UpgradePhase::Failed);
        assert_eq!(normalized.target_version, Some("0.0.104".to_string()));
        assert_eq!(normalized.source, Some("admin".to_string()));
        assert!(normalized.error.is_some());
    }

    #[test]
    fn fresh_active_progress_passes_through() {
        let progress = UpgradeProgress::new(UpgradePhase::Downloading, "Downloading…")
            .with_percent(Some(42.0));
        let normalized = normalize_progress(progress.clone());
        assert_eq!(normalized.phase, UpgradePhase::Downloading);
        assert_eq!(normalized.percent, Some(42.0));
    }

    #[test]
    fn terminal_progress_passes_through_even_when_old() {
        let mut completed = UpgradeProgress::new(UpgradePhase::Completed, "Done");
        completed.updated_at =
            (Utc::now() - chrono::Duration::seconds(DEFAULT_STALE_SECS + 300)).to_rfc3339();
        assert_eq!(normalize_progress(completed).phase, UpgradePhase::Completed);

        let idle = UpgradeProgress::idle();
        assert_eq!(normalize_progress(idle).phase, UpgradePhase::Idle);
    }

    #[test]
    fn parse_upgrade_channel_defaults_to_cli_and_accepts_desktop_aliases() {
        assert_eq!(parse_upgrade_channel(None), UpgradeChannel::Cli);
        assert_eq!(
            parse_upgrade_channel(Some("refresh=true")),
            UpgradeChannel::Cli
        );
        assert_eq!(
            parse_upgrade_channel(Some("channel=desktop")),
            UpgradeChannel::Desktop
        );
        assert_eq!(
            parse_upgrade_channel(Some("refresh=true&target=desktop")),
            UpgradeChannel::Desktop
        );
        assert_eq!(
            parse_upgrade_channel(Some("source=desktop")),
            UpgradeChannel::Desktop
        );
    }

    #[test]
    fn runtime_owner_overrides_the_request_channel() {
        assert_eq!(
            effective_upgrade_channel(UpgradeChannel::Desktop, false),
            UpgradeChannel::Cli
        );
        assert_eq!(
            effective_upgrade_channel(UpgradeChannel::Cli, true),
            UpgradeChannel::Desktop
        );
        assert!(desktop_core_env_enabled(Some("true".into())));
        assert!(desktop_core_env_enabled(Some("1".into())));
        assert!(!desktop_core_env_enabled(Some("0".into())));
        assert!(!desktop_core_env_enabled(None));

        assert_eq!(
            upgrade_request_plan(Some("channel=desktop"), false, 12345, 19900),
            (
                UpgradeChannel::Desktop,
                UpgradeChannel::Cli,
                Some((12345, 19900))
            )
        );
        assert_eq!(
            upgrade_request_plan(Some("channel=cli"), true, 12345, 19900),
            (UpgradeChannel::Cli, UpgradeChannel::Desktop, None)
        );
        assert_eq!(
            validate_upgrade_request_channel(UpgradeChannel::Cli, UpgradeChannel::Desktop, false,),
            Err("A desktop-owned core must be upgraded from the Bifrost desktop app")
        );
        assert_eq!(
            validate_upgrade_request_channel(
                UpgradeChannel::Desktop,
                UpgradeChannel::Desktop,
                false,
            ),
            Err("A desktop-owned core must be upgraded from the Bifrost desktop app")
        );
        assert!(validate_upgrade_request_channel(
            UpgradeChannel::Desktop,
            UpgradeChannel::Desktop,
            true,
        )
        .is_ok());
        assert!(
            validate_upgrade_request_channel(UpgradeChannel::Cli, UpgradeChannel::Cli, false,)
                .is_ok()
        );
        assert!(validate_upgrade_request_channel(
            UpgradeChannel::Desktop,
            UpgradeChannel::Cli,
            false,
        )
        .is_ok());

        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!validated_webview_upgrade_origin(
            dir.path(),
            UpgradeChannel::Desktop,
            None,
        ));
        let token = bifrost_core::upgrade_progress::issue_desktop_upgrade_origin_token(dir.path())
            .expect("issue desktop origin");
        assert!(!validated_webview_upgrade_origin(
            dir.path(),
            UpgradeChannel::Cli,
            Some(&token),
        ));
        assert!(validated_webview_upgrade_origin(
            dir.path(),
            UpgradeChannel::Desktop,
            Some(&token),
        ));
        assert!(!validated_webview_upgrade_origin(
            dir.path(),
            UpgradeChannel::Desktop,
            Some(&token),
        ));
    }

    #[tokio::test]
    async fn upgrade_start_lock_serializes_claims() {
        let first = upgrade_start_lock().lock().await;
        assert!(upgrade_start_lock().try_lock().is_err());
        drop(first);
        assert!(upgrade_start_lock().try_lock().is_ok());
    }

    #[tokio::test]
    async fn admin_upgrade_claims_once_and_pins_the_cached_target() {
        const CHILD_ENV: &str = "BIFROST_TEST_ADMIN_UPGRADE_HANDLER_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = std::process::Command::new(
                std::env::current_exe().expect("current test executable"),
            )
            .args([
                "--exact",
                "handlers::system::tests::admin_upgrade_claims_once_and_pins_the_cached_target",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated Admin handler test");
            assert!(status.success(), "isolated Admin handler test failed");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("BIFROST_DATA_DIR", dir.path());
        std::fs::write(
            dir.path().join("version_cache.json"),
            serde_json::json!({
                "latest_version": "999.0.0",
                "release_highlights": ["test release"],
                "checked_at": Utc::now(),
            })
            .to_string(),
        )
        .expect("write fresh version cache");
        let state = Arc::new(crate::state::AdminState::new(19990));

        let version = check_version(state.clone(), Some("channel=desktop")).await;
        assert_eq!(version.status(), StatusCode::OK);

        let token = bifrost_core::upgrade_progress::issue_desktop_upgrade_origin_token(dir.path())
            .expect("issue desktop origin");
        let accepted =
            start_upgrade(state.clone(), Some("channel=desktop"), Some(token.as_str())).await;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        let progress = bifrost_core::upgrade_progress::read_progress(dir.path());
        assert_eq!(progress.target_version.as_deref(), Some("999.0.0"));
        assert_eq!(progress.source.as_deref(), Some("admin"));

        let conflict = start_upgrade(state, Some("channel=cli"), None).await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn upgrade_process_args_separate_cli_and_desktop_channels() {
        assert_eq!(
            upgrade_process_args(
                UpgradeChannel::Cli,
                Some("0.0.139"),
                Some((12345, 19900)),
                None,
            ),
            vec![
                "self-update",
                "--target",
                "0.0.139",
                "--source",
                "admin",
                "--running-proxy-pid",
                "12345",
                "--running-proxy-port",
                "19900",
            ]
        );
        assert_eq!(
            upgrade_process_args(
                UpgradeChannel::Desktop,
                Some("0.0.139"),
                Some((12345, 19900)),
                Some(Path::new("/Users/test/Applications")),
            ),
            vec![
                "app",
                "upgrade",
                "--version",
                "0.0.139",
                "--source",
                "desktop",
                "--app-dir",
                "/Users/test/Applications",
                "-y"
            ]
        );
        assert_eq!(
            upgrade_process_args(UpgradeChannel::Desktop, Some("0.0.139"), None, None),
            vec![
                "app",
                "upgrade",
                "--version",
                "0.0.139",
                "--source",
                "desktop",
                "-y"
            ],
            "desktop-owned Admin dispatch must let the bundled core resolve its active App path"
        );
        assert_eq!(
            upgrade_process_environment(UpgradeChannel::Cli, true),
            vec![(super::WEBVIEW_UPGRADE_ORIGIN_ENV, "1")],
            "a desktop WebView can initiate the CLI-owned orchestrator"
        );
        assert!(upgrade_process_environment(UpgradeChannel::Cli, false).is_empty());
        assert_eq!(
            upgrade_process_environment(UpgradeChannel::Desktop, true),
            vec![
                (super::WEBVIEW_UPGRADE_ORIGIN_ENV, "1"),
                (super::DESKTOP_UPGRADE_HANDOFF_ENV, "1")
            ]
        );
    }

    #[test]
    fn upgrade_process_spawn_runs_in_an_isolated_data_dir() {
        const CHILD_ENV: &str = "BIFROST_TEST_ADMIN_UPGRADE_SPAWN_CHILD";
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = std::process::Command::new(
                std::env::current_exe().expect("current test executable"),
            )
            .args([
                "--exact",
                "handlers::system::tests::upgrade_process_spawn_runs_in_an_isolated_data_dir",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("spawn isolated Admin upgrade test");
            assert!(status.success(), "isolated Admin upgrade test failed");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("BIFROST_DATA_DIR", dir.path());
        spawn_upgrade_process(UpgradeChannel::Desktop, None, None, None, true)
            .expect("spawn detached desktop upgrade command");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    #[test]
    fn cli_install_status_uses_override_dir_and_path_hint() {
        let dir =
            std::env::temp_dir().join(format!("bifrost-cli-install-status-{}", std::process::id()));
        let current_exe = std::env::current_exe().expect("current exe");
        let status =
            build_cli_install_status(Some(dir.clone()), Some(current_exe), true).expect("status");

        assert_eq!(status.install_dir, dir.display().to_string());
        assert_eq!(
            status.install_path,
            dir.join(super::cli_binary_name()).display().to_string()
        );
        assert!(status.path_hint.is_some());
        assert!(status.dry_run);
    }

    #[test]
    fn cli_install_copies_current_exe_to_override_dir_without_skills() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-cli-install-copy-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let response = install_cli_from_current_exe(CliInstallRequest {
            install_dir: Some(dir.clone()),
            install_skills: Some(false),
            dry_run: Some(false),
        })
        .expect("install cli");

        let install_path = dir.join(super::cli_binary_name());
        assert!(install_path.exists());
        assert!(response.installed);
        assert_eq!(response.install_path, install_path.display().to_string());
        assert_eq!(response.skills_installed, None);
        assert_eq!(
            response.skills_message,
            Some("Bifrost AI skill installation skipped".to_string())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_install_with_skill_failure_still_installs_cli_binary() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-cli-install-with-skills-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let response = install_cli_from_current_exe(CliInstallRequest {
            install_dir: Some(dir.clone()),
            install_skills: Some(true),
            dry_run: Some(false),
        })
        .expect("install cli");

        let install_path = dir.join(super::cli_binary_name());
        assert!(install_path.exists());
        assert!(response.installed);
        assert_eq!(response.install_path, install_path.display().to_string());
        assert_eq!(response.skills_installed, Some(false));
        assert!(response
            .skills_message
            .as_deref()
            .unwrap_or_default()
            .contains("install-skill"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_binary_atomically_skips_when_source_is_target() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-cli-install-same-file-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(super::cli_binary_name());
        std::fs::write(&path, b"same-file").expect("write source");

        install_binary_atomically(&path, &path).expect("same file install");

        assert_eq!(
            std::fs::read(&path).expect("read source"),
            b"same-file".to_vec()
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
