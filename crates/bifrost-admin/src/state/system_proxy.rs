use super::*;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER_ENV: &str =
    "BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER";
const SYSTEM_PROXY_LIFECYCLE_HELPER_PROGRAM_ENV: &str =
    "BIFROST_SYSTEM_PROXY_LIFECYCLE_HELPER_PROGRAM";

pub struct SystemProxyLifecycleHelperState {
    data_dir: PathBuf,
    parent_pid: u32,
    parent_started_at_ms: Option<u64>,
    child: parking_lot::Mutex<Option<Child>>,
}

#[derive(Clone, Copy)]
enum SystemProxyLifecycleHelperStartReason {
    Startup,
    AdminApiEnable,
}

impl SystemProxyLifecycleHelperState {
    pub fn new(data_dir: PathBuf, parent_pid: u32) -> Self {
        Self {
            data_dir,
            parent_pid,
            parent_started_at_ms: bifrost_core::current_process_start_time_ms(),
            child: parking_lot::Mutex::new(None),
        }
    }

    pub fn ensure_started_after_startup(&self) {
        self.ensure_started(SystemProxyLifecycleHelperStartReason::Startup);
    }

    pub fn ensure_started_after_admin_api_enable(&self) {
        self.ensure_started(SystemProxyLifecycleHelperStartReason::AdminApiEnable);
    }

    fn ensure_started(&self, reason: SystemProxyLifecycleHelperStartReason) {
        // The helper also owns standalone CLI proxy environment cleanup, so it must run on every
        // supported Bifrost platform even where there is no OS-level system proxy integration.
        if std::env::var(SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER_ENV)
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            tracing::info!(
                target: "bifrost_admin::proxy",
                env = SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER_ENV,
                "system proxy lifecycle helper disabled by environment"
            );
            return;
        }

        let mut child = self.child.lock();
        if let Some(existing) = child.as_mut() {
            match existing.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) => tracing::warn!(
                    target: "bifrost_admin::proxy",
                    status = %status,
                    reason = reason.as_str(),
                    "system proxy lifecycle helper exited before requested enable path"
                ),
                Err(error) => tracing::warn!(
                    target: "bifrost_admin::proxy",
                    error = %error,
                    reason = reason.as_str(),
                    "failed to inspect system proxy lifecycle helper before requested enable path"
                ),
            }
            *child = None;
        }

        match spawn_system_proxy_lifecycle_helper(
            &self.data_dir,
            self.parent_pid,
            self.parent_started_at_ms,
            reason,
        ) {
            Ok(helper) => *child = Some(helper),
            Err(error) => tracing::warn!(
                target: "bifrost_admin::proxy",
                error = %error,
                parent_pid = self.parent_pid,
                data_dir = %self.data_dir.display(),
                reason = reason.as_str(),
                "failed to start system proxy lifecycle helper"
            ),
        }
    }

    /// Detaches the helper child without killing it, leaving the watchdog process alive.
    /// Used during AdminState drop so that an abnormal Bifrost shutdown still benefits from
    /// the lifecycle helper observing the parent exit and cleaning up.
    pub fn detach(&self) {
        if let Some(child) = self.child.lock().take() {
            tracing::info!(
                target: "bifrost_admin::proxy",
                helper_pid = child.id(),
                "detaching system proxy lifecycle helper without kill"
            );
            std::mem::forget(child);
        }
    }
}

impl SystemProxyLifecycleHelperStartReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::AdminApiEnable => "admin_api_enable",
        }
    }

    fn started_message(self) -> &'static str {
        match self {
            // Keep the established log phrase for dashboards and diagnostics even though the
            // helper now also removes standalone CLI proxy environment blocks.
            Self::Startup => "system proxy lifecycle cleanup helper started",
            Self::AdminApiEnable => "system proxy lifecycle helper started after Admin API enable",
        }
    }
}

fn spawn_system_proxy_lifecycle_helper(
    data_dir: &Path,
    parent_pid: u32,
    parent_started_at_ms: Option<u64>,
    reason: SystemProxyLifecycleHelperStartReason,
) -> std::io::Result<Child> {
    let exe = resolve_system_proxy_lifecycle_helper_program()?;
    let mut command = Command::new(&exe);
    command
        .arg("system-proxy")
        .arg("lifecycle-helper")
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--parent-pid")
        .arg(parent_pid.to_string())
        .arg("--poll-secs")
        .arg("2")
        .stdin(Stdio::null());
    if let Some(started_at_ms) = parent_started_at_ms {
        command
            .arg("--parent-started-at-ms")
            .arg(started_at_ms.to_string());
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS so the helper survives parent exit
        // and is not attached to the parent console.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    let child = command.spawn()?;
    tracing::info!(
        target: "bifrost_admin::proxy",
        helper_pid = child.id(),
        parent_pid,
        parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
        data_dir = %data_dir.display(),
        helper_program = %exe.display(),
        reason = reason.as_str(),
        "{}",
        reason.started_message()
    );
    Ok(child)
}

fn resolve_system_proxy_lifecycle_helper_program() -> std::io::Result<PathBuf> {
    if let Ok(program) = std::env::var(SYSTEM_PROXY_LIFECYCLE_HELPER_PROGRAM_ENV) {
        let program = PathBuf::from(program);
        if program.exists() {
            return Ok(program);
        }
        tracing::warn!(
            target: "bifrost_admin::proxy",
            helper_program = %program.display(),
            env = SYSTEM_PROXY_LIFECYCLE_HELPER_PROGRAM_ENV,
            "configured system proxy lifecycle helper program does not exist; falling back"
        );
    }

    let current_exe = std::env::current_exe();
    let arg0 = std::env::args_os().next();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_system_proxy_lifecycle_helper_program_from_candidates(current_exe, arg0, &cwd)
}

pub(super) fn resolve_system_proxy_lifecycle_helper_program_from_candidates(
    current_exe: std::io::Result<PathBuf>,
    arg0: Option<OsString>,
    cwd: &Path,
) -> std::io::Result<PathBuf> {
    if let Ok(path) = current_exe.as_ref() {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    if let Some(arg0) = arg0 {
        let arg0 = PathBuf::from(arg0);
        let candidate = if arg0.is_absolute() {
            arg0
        } else {
            cwd.join(arg0)
        };
        if candidate.exists() {
            tracing::warn!(
                target: "bifrost_admin::proxy",
                helper_program = %candidate.display(),
                current_exe = current_exe
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|error| error.to_string()),
                "falling back to argv[0] for system proxy lifecycle helper program"
            );
            return Ok(candidate);
        }
    }

    current_exe
}

impl Drop for SystemProxyLifecycleHelperState {
    fn drop(&mut self) {
        // Detach (do not kill): if Bifrost is exiting abnormally, the helper must
        // outlive the parent so it can observe parent exit and clean up every managed proxy form.
        self.detach();
    }
}

pub type SharedSystemProxyLifecycleHelperState = Arc<SystemProxyLifecycleHelperState>;
