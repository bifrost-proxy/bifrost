use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use bifrost_core::upgrade_progress::{
    is_stale, read_progress, write_progress, UpgradePhase, UpgradeProgress, DEFAULT_STALE_SECS,
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

const DESKTOP_INSTALL_SKILL_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn handle_system(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();
    let query = req.uri().query();

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
            Method::GET => check_version(state, query).await,
            _ => method_not_allowed(),
        },
        "/api/system/upgrade" => match method {
            Method::POST => start_upgrade(state, query).await,
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
        db_store.stats().record_count
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

    let response = check_version_for_channel(state, force_refresh, channel).await;
    json_response(&response)
}

/// `POST /api/system/upgrade` — trigger an unattended background upgrade.
///
/// Returns `409 Conflict` when there is no newer version available or an
/// upgrade is already in flight (non-stale active progress). On success it
/// writes an initial `Checking` progress record, spawns a detached
/// `bifrost self-update --source admin` subprocess and returns `202 Accepted`
/// with the current progress snapshot.
///
/// The upgrade is never executed inside the admin process: the subprocess stops
/// the old proxy, swaps the binary and restarts, which would otherwise kill the
/// admin server mid-request. The progress file survives the restart so the
/// Web UI can read the terminal state after reconnecting.
async fn start_upgrade(state: SharedAdminState, query: Option<&str>) -> Response<BoxBody> {
    let dir = data_dir();
    let channel = parse_upgrade_channel(query);

    // Refuse if an upgrade is already running and still alive.
    let current = read_progress(&dir);
    if current.is_active() && !is_stale(&current, DEFAULT_STALE_SECS) {
        return error_response(StatusCode::CONFLICT, "An upgrade is already in progress");
    }

    // Confirm there is actually a newer version to upgrade to.
    let version = check_version_for_channel(state, false, channel).await;
    if !version.has_update {
        return error_response(StatusCode::CONFLICT, "No update available");
    }
    let target_version = version.latest_version.clone();

    // Seed the channel so the Web UI sees movement immediately, before the
    // subprocess has had a chance to start writing.
    let initial = UpgradeProgress::new(UpgradePhase::Checking, "Checking for updates…")
        .with_target(target_version.clone())
        .with_source(Some(channel.progress_source().to_string()));
    write_progress(&dir, &initial);

    if let Err(error) = spawn_upgrade_process(channel, target_version.as_deref()) {
        warn!(error = %error, "[SYSTEM] failed to spawn self-update subprocess");
        let failed = UpgradeProgress::new(UpgradePhase::Failed, "Upgrade failed to start")
            .with_target(target_version)
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

/// Spawn a detached `bifrost self-update` subprocess.
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

fn desktop_app_version_for_version_check() -> Option<String> {
    desktop_app_install_candidates()
        .into_iter()
        .find_map(|path| installed_desktop_app_version(&path))
}

fn desktop_app_install_candidates() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("BIFROST_APP_INSTALL_DIR") {
        return vec![resolve_desktop_app_path(&PathBuf::from(dir))];
    }

    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/Applications/Bifrost.app"));
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join("Applications").join("Bifrost.app"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Bifrost")
                    .join("bifrost-desktop.exe"),
            );
        }
    }
    candidates
}

fn resolve_desktop_app_path(app_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        app_dir.join("Bifrost.app")
    }
    #[cfg(target_os = "windows")]
    {
        app_dir.join("bifrost-desktop.exe")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        app_dir.join("Bifrost")
    }
}

fn installed_desktop_app_version(install_path: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let plist_path = install_path.join("Contents").join("Info.plist");
        let plist = plist::Value::from_file(plist_path).ok()?;
        let dict = plist.as_dictionary()?;
        ["CFBundleShortVersionString", "CFBundleVersion"]
            .into_iter()
            .find_map(|key| dict.get(key).and_then(|value| value.as_string()))
            .map(str::to_string)
    }
    #[cfg(target_os = "windows")]
    {
        if !install_path.is_file() {
            return None;
        }
        let script = r#"
param([string]$Path)
$info = (Get-Item -LiteralPath $Path).VersionInfo
if ($info.ProductVersion) { $info.ProductVersion } elseif ($info.FileVersion) { $info.FileVersion }
"#;
        let powershell = if Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            "powershell.exe"
        } else {
            "pwsh"
        };
        let output = Command::new(powershell)
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script)
            .arg(install_path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!version.is_empty()).then_some(version)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = install_path;
        None
    }
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

fn spawn_upgrade_process(
    channel: UpgradeChannel,
    target_version: Option<&str>,
) -> std::io::Result<()> {
    let program = std::env::current_exe().unwrap_or_else(|_| "bifrost".into());

    let mut command = Command::new(&program);
    command.args(upgrade_process_args(channel, target_version));
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

fn upgrade_process_args(channel: UpgradeChannel, target_version: Option<&str>) -> Vec<String> {
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
        build_cli_install_status, install_binary_atomically, install_cli_from_current_exe,
        normalize_progress, parse_upgrade_channel, upgrade_process_args, CliInstallRequest,
        UpgradeChannel,
    };
    use bifrost_core::upgrade_progress::{UpgradePhase, UpgradeProgress, DEFAULT_STALE_SECS};
    use chrono::Utc;

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
    fn upgrade_process_args_separate_cli_and_desktop_channels() {
        assert_eq!(
            upgrade_process_args(UpgradeChannel::Cli, Some("0.0.139")),
            vec!["self-update", "--target", "0.0.139", "--source", "admin"]
        );
        assert_eq!(
            upgrade_process_args(UpgradeChannel::Desktop, Some("0.0.139")),
            vec![
                "app",
                "upgrade",
                "--version",
                "0.0.139",
                "--source",
                "desktop",
                "-y"
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn desktop_version_check_reads_installed_app_version_from_override_dir() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("tempdir");
        let app = temp.path().join("Bifrost.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(&contents).expect("create Contents");
        std::fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key><string>0.0.144</string>
  <key>CFBundleVersion</key><string>144</string>
</dict>
</plist>
"#,
        )
        .expect("write plist");

        let previous = std::env::var_os("BIFROST_APP_INSTALL_DIR");
        std::env::set_var("BIFROST_APP_INSTALL_DIR", temp.path());
        let version = super::desktop_app_version_for_version_check();
        match previous {
            Some(value) => std::env::set_var("BIFROST_APP_INSTALL_DIR", value),
            None => std::env::remove_var("BIFROST_APP_INSTALL_DIR"),
        }

        assert_eq!(version.as_deref(), Some("0.0.144"));
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
