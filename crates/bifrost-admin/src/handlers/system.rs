use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use bifrost_core::upgrade_progress::{
    is_stale, read_progress, write_progress, UpgradePhase, UpgradeProgress, DEFAULT_STALE_SECS,
};
use bifrost_storage::data_dir;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tracing::warn;

use super::{error_response, json_response, json_response_with_status, method_not_allowed, BoxBody};
use crate::metrics::SystemInfo;
use crate::resource_alerts::build_resource_alerts;
use crate::state::SharedAdminState;

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
            Method::POST => start_upgrade(state).await,
            _ => method_not_allowed(),
        },
        "/api/system/upgrade/progress" => match method {
            Method::GET => get_upgrade_progress().await,
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

    let response = state.version_checker.check(force_refresh).await;
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
async fn start_upgrade(state: SharedAdminState) -> Response<BoxBody> {
    let dir = data_dir();

    // Refuse if an upgrade is already running and still alive.
    let current = read_progress(&dir);
    if current.is_active() && !is_stale(&current, DEFAULT_STALE_SECS) {
        return error_response(StatusCode::CONFLICT, "An upgrade is already in progress");
    }

    // Confirm there is actually a newer version to upgrade to.
    let version = state.version_checker.check(false).await;
    if !version.has_update {
        return error_response(StatusCode::CONFLICT, "No update available");
    }
    let target_version = version.latest_version.clone();

    // Seed the channel so the Web UI sees movement immediately, before the
    // subprocess has had a chance to start writing.
    let initial = UpgradeProgress::new(UpgradePhase::Checking, "Checking for updates…")
        .with_target(target_version.clone())
        .with_source(Some("admin".to_string()));
    write_progress(&dir, &initial);

    if let Err(error) = spawn_self_update(target_version.as_deref()) {
        warn!(error = %error, "[SYSTEM] failed to spawn self-update subprocess");
        let failed = UpgradeProgress::new(UpgradePhase::Failed, "Upgrade failed to start")
            .with_target(target_version)
            .with_source(Some("admin".to_string()))
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
fn spawn_self_update(target_version: Option<&str>) -> std::io::Result<()> {
    let program = std::env::current_exe().unwrap_or_else(|_| "bifrost".into());

    let mut command = Command::new(&program);
    command.arg("self-update");
    if let Some(target) = target_version {
        command.arg("--target").arg(target);
    }
    command
        .arg("--source")
        .arg("admin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

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

    let child = command.spawn()?;
    tracing::info!(
        target: "bifrost_admin::system",
        child_pid = child.id(),
        program = %program.display(),
        target_version = target_version.unwrap_or("latest"),
        "spawned background self-update subprocess"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_progress;
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
        assert_eq!(
            normalize_progress(completed).phase,
            UpgradePhase::Completed
        );

        let idle = UpgradeProgress::idle();
        assert_eq!(normalize_progress(idle).phase, UpgradePhase::Idle);
    }
}
