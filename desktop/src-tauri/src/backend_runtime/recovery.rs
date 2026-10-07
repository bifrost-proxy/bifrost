use super::*;

pub(crate) const SYSTEM_PROXY_RECOVERY_GENERATION_ENV: &str =
    "BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL";

pub(crate) fn configure_backend_recovery_generation(
    command: &mut Command,
    generation: Option<&str>,
) {
    // Recovery leases are scoped to this launch, never inherited by a later
    // manual start or an unrelated child command.
    command.env_remove(SYSTEM_PROXY_RECOVERY_GENERATION_ENV);
    if let Some(generation) = generation {
        command.env(SYSTEM_PROXY_RECOVERY_GENERATION_ENV, generation);
    }
}

pub(crate) struct BackendRecoveryGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for BackendRecoveryGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn begin_backend_recovery(state: &BackendState) -> Option<BackendRecoveryGuard<'_>> {
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

pub(crate) fn poll_managed_backend_exit(
    state: &BackendState,
) -> Result<Option<ManagedBackendExit>, String> {
    let mut child_guard = state
        .child
        .lock()
        .map_err(|_| "failed to access managed backend child".to_string())?;
    let Some(child) = child_guard.as_mut() else {
        return Ok(None);
    };

    match child.try_wait() {
        Ok(Some(status)) => {
            let pid = child.id();
            let _ = child_guard.take();
            #[cfg(unix)]
            let exit_signal = {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            };
            #[cfg(not(unix))]
            let exit_signal = None;
            Ok(Some(ManagedBackendExit {
                pid,
                exit_code: status.code(),
                exit_signal,
                detail: format!("managed backend child pid={pid} exited with status {status}"),
            }))
        }
        Ok(None) => Ok(None),
        Err(error) => {
            let pid = child.id();
            Err(format!(
                "failed to poll managed backend child pid={pid}: {error}"
            ))
        }
    }
}

/// A probe is evidence only for this exact lifecycle generation. The child handle
/// prevents accidental adoption of an external runtime; the epoch also fences ABA
/// transitions where a manual restart happens to reuse the same port or PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendRecoverySnapshot {
    pub(crate) epoch: u64,
    pub(crate) port: u16,
    pub(crate) expected_port: u16,
    pub(crate) child_pid: Option<u32>,
    pub(crate) child_start_time_ms: Option<u64>,
    runtime_marker: Option<Vec<u8>>,
    pid_marker: Option<Vec<u8>>,
    pub(crate) proxy_generation: Option<String>,
}

pub(crate) fn backend_shutdown_requested(state: &BackendState) -> bool {
    state.shutdown_started.load(Ordering::SeqCst) || state.force_exit.load(Ordering::SeqCst)
}

fn read_marker(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

pub(crate) fn recovery_generation_is_compatible(
    expected: Option<&str>,
    observed: Option<&str>,
) -> bool {
    // Cleanup may release the old journal. Absence is no takeover proof: the
    // replacement receives an empty lease and cannot freshly acquire OS proxy.
    observed.is_none() || observed == expected
}

impl BackendRecoverySnapshot {
    pub(crate) fn capture(state: &BackendState) -> Result<Self, String> {
        let child_pid = state
            .child
            .lock()
            .map_err(|_| "failed to inspect managed backend child")?
            .as_ref()
            .map(Child::id);
        let ownership = bifrost_core::SystemProxyManager::new(state.data_dir.clone())
            .read_managed_ownership()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            epoch: state.backend_lifecycle_epoch.load(Ordering::SeqCst),
            port: *state
                .port
                .lock()
                .map_err(|_| "failed to read backend port")?,
            expected_port: *state
                .expected_port
                .lock()
                .map_err(|_| "failed to read expected port")?,
            child_pid,
            child_start_time_ms: child_pid.and_then(bifrost_core::get_process_start_time_ms),
            runtime_marker: read_marker(&state.data_dir.join("runtime.json"))?,
            pid_marker: read_marker(&state.data_dir.join("bifrost.pid"))?,
            proxy_generation: ownership
                .map(|owner| owner.generation)
                .filter(|value| !value.is_empty()),
        })
    }

    pub(crate) fn is_current(&self, state: &BackendState) -> bool {
        !backend_shutdown_requested(state)
            && Self::capture(state).is_ok_and(|current| current == *self)
    }

    pub(crate) fn after_owned_exit(&self, state: &BackendState, exited_pid: u32) -> Option<Self> {
        let next = Self::capture(state).ok()?;
        (self.child_pid == Some(exited_pid)
            && next.child_pid.is_none()
            && next.epoch == self.epoch
            && next.port == self.port
            && next.expected_port == self.expected_port
            && recovery_generation_is_compatible(
                self.proxy_generation.as_deref(),
                next.proxy_generation.as_deref(),
            )
            && (next.runtime_marker.is_none() || next.runtime_marker == self.runtime_marker)
            && (next.pid_marker.is_none() || next.pid_marker == self.pid_marker)
            && next.markers_belong_to_owned_pid(exited_pid, &[]))
        .then_some(next)
    }

    pub(crate) fn refresh_retry_snapshot(&self, state: &BackendState) -> Option<Self> {
        if self.child_pid.is_some() || backend_shutdown_requested(state) {
            return None;
        }
        let next = Self::capture(state).ok()?;
        if !recovery_generation_is_compatible(
            self.proxy_generation.as_deref(),
            next.proxy_generation.as_deref(),
        ) {
            return None;
        }
        let mut previous = self.clone();
        previous.proxy_generation = next.proxy_generation.clone();
        // Exited-runtime cleanup may remove its own markers while we are waiting.
        // Replacing them with another marker is never an ownership handoff.
        if next.runtime_marker.is_none() {
            previous.runtime_marker = None;
        }
        if next.pid_marker.is_none() {
            previous.pid_marker = None;
        }
        (previous == next).then_some(next)
    }

    fn runtime_pid(&self) -> Option<u32> {
        self.runtime_marker.as_ref().and_then(|bytes| {
            serde_json::from_slice::<DesktopRuntimeMarker>(bytes)
                .ok()
                .map(|marker| marker.pid)
        })
    }

    fn markers_belong_to_owned_pid(&self, exited_pid: u32, launched_pids: &[u32]) -> bool {
        let owned = |pid| pid == exited_pid || launched_pids.contains(&pid);
        let runtime_matches = self.runtime_marker.as_ref().is_none_or(|bytes| {
            serde_json::from_slice::<DesktopRuntimeMarker>(bytes)
                .is_ok_and(|marker| owned(marker.pid))
        });
        let pid_matches = self.pid_marker.as_ref().is_none_or(|bytes| {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .is_some_and(owned)
        });
        runtime_matches && pid_matches
    }

    pub(crate) fn owned_runtime_matches(&self) -> bool {
        self.child_pid.is_some_and(|pid| {
            self.runtime_marker.as_ref().is_some_and(|bytes| {
                serde_json::from_slice::<DesktopRuntimeMarker>(bytes)
                    .is_ok_and(|marker| marker.pid == pid && marker.port == self.port)
            })
        })
    }
}

/// Acquire before revalidation and keep the guard until every destructive action
/// and replacement publication has finished. Never acquire it only after killing.
pub(crate) fn begin_observed_backend_recovery<'a>(
    state: &'a BackendState,
    expected: &BackendRecoverySnapshot,
) -> Option<BackendRecoveryGuard<'a>> {
    let guard = begin_backend_recovery(state)?;
    expected.is_current(state).then_some(guard)
}

pub(crate) fn terminate_observed_backend(
    state: &BackendState,
    guard: &BackendRecoveryGuard<'_>,
    expected: &BackendRecoverySnapshot,
) -> tauri::Result<bool> {
    terminate_observed_backend_with(state, guard, expected, |child| {
        kill_child_and_wait(child, BACKEND_KILL_WAIT_TIMEOUT)
            .map(|_| ())
            .map_err(|error| {
                anyhow(format!(
                    "failed to terminate owned unresponsive backend: {error}"
                ))
            })
    })
}

pub(crate) fn terminate_observed_backend_with(
    state: &BackendState,
    _guard: &BackendRecoveryGuard<'_>,
    expected: &BackendRecoverySnapshot,
    terminate: impl FnOnce(&mut Child) -> tauri::Result<()>,
) -> tauri::Result<bool> {
    if !expected.is_current(state) || !expected.owned_runtime_matches() {
        return Ok(false);
    }
    let mut child_guard = state
        .child
        .lock()
        .map_err(|_| anyhow("failed to lock owned backend".into()))?;
    let Some(child) = child_guard.as_mut() else {
        return Ok(false);
    };
    if backend_shutdown_requested(state) || expected.child_pid != Some(child.id()) {
        return Ok(false);
    }
    // Retain the handle on failure; a failed kill must not silently turn an
    // owned, possibly still-running child into an unowned external runtime.
    terminate(child)?;
    child_guard.take();
    Ok(true)
}

#[derive(Debug, Clone)]
pub(crate) struct PendingBackendRecovery {
    pub(crate) expected: BackendRecoverySnapshot,
    pub(crate) exited: ManagedBackendExit,
    pub(crate) retry_at: Instant,
}

impl PendingBackendRecovery {
    pub(crate) fn is_due(&self, now: Instant) -> bool {
        now >= self.retry_at
    }
}

#[derive(Debug)]
pub(crate) enum BackendRecoveryResult {
    Recovered,
    Retry(PendingBackendRecovery),
    Cancelled,
}

/// Fail open only for the generation observed before this recovery. Core performs
/// the final ownership/OS-state comparison under its system-proxy file lock.
/// Never call the unguarded disable/restore path from the Desktop watchdog.
pub(crate) fn suspend_failed_backend_proxy(
    state: &BackendState,
    _guard: &BackendRecoveryGuard<'_>,
    expected: &BackendRecoverySnapshot,
) {
    if !expected.is_current(state) {
        return;
    }
    let Some(generation) = expected.proxy_generation.as_deref() else {
        return;
    };
    let mut manager = bifrost_core::SystemProxyManager::new(state.data_dir.clone());
    let result = manager.suspend_managed_if_generation(generation);
    manager.detach_in_place();
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!("desktop backend fail-open handoff; generation={generation} outcome={result:?}"),
    );
}

pub(crate) fn attempt_backend_recovery(
    state: &BackendState,
    _guard: &BackendRecoveryGuard<'_>,
    expected: &BackendRecoverySnapshot,
    exited: &ManagedBackendExit,
) -> BackendRecoveryResult {
    attempt_backend_recovery_with_launcher(state, _guard, expected, exited, |launched_pids| {
        port_retry::launch_backend_on_available_port_observed(
            &state.binary_path,
            &state.data_dir,
            &state.startup_session_id,
            expected.port,
            false,
            Some(expected.proxy_generation.as_deref().unwrap_or("")),
            &mut |pid| launched_pids.push(pid),
        )
    })
}

pub(crate) fn attempt_backend_recovery_with_launcher(
    state: &BackendState,
    guard: &BackendRecoveryGuard<'_>,
    expected: &BackendRecoverySnapshot,
    exited: &ManagedBackendExit,
    launch: impl FnOnce(&mut Vec<u32>) -> tauri::Result<(Option<Child>, u16)>,
) -> BackendRecoveryResult {
    if !expected.is_current(state) || expected.child_pid.is_some() {
        return BackendRecoveryResult::Cancelled;
    }
    // A new CLI-owned runtime must never be stopped or adopted for automatic
    // replacement. Launch without generic directory-wide cleanup or --yes;
    // stdin is null, so a racing external runtime cannot trigger a restart prompt.
    if !expected.markers_belong_to_owned_pid(exited.pid, &[]) {
        return BackendRecoveryResult::Cancelled;
    }
    let started = Instant::now();
    state.startup_ready.store(false, Ordering::SeqCst);
    append_desktop_bootstrap_log(
        &state.data_dir,
        format!(
            "desktop backend starting guarded automatic replacement; {}",
            exited.detail
        ),
    );
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
        "managed_child_exit_confirmed",
        "desktop_watchdog",
    );
    event.old_pid = Some(exited.pid);
    event.exit_code = exited.exit_code;
    event.exit_signal = exited.exit_signal;
    event.trigger = Some(exited.detail.clone());
    let _ = bifrost_core::append_system_proxy_event(&state.data_dir, &event);

    let mut launched_pids = Vec::new();
    let launched = launch(&mut launched_pids);
    // Shutdown invalidates the epoch without waiting for slow startup probes.
    // Keep the returned child in state so the serialized shutdown coordinator can
    // stop/reap it, but never publish it ready or schedule a replacement.
    match launched {
        Ok((child, port)) => {
            let new_pid = child.as_ref().map(Child::id);
            if let Ok(mut current) = state.child.lock() {
                *current = child;
            } else {
                if let Some(child) = child {
                    let _ = terminate_child(child);
                }
                record_startup_error(state, "failed to retain replacement child".into());
                return BackendRecoveryResult::Cancelled;
            }
            if backend_shutdown_requested(state)
                || state.backend_lifecycle_epoch.load(Ordering::SeqCst) != expected.epoch
            {
                return BackendRecoveryResult::Cancelled;
            }
            if let Ok(mut current) = state.port.lock() {
                *current = port;
            }
            state.backend_lifecycle_epoch.fetch_add(1, Ordering::SeqCst);
            publish_startup_ready(state);
            if new_pid.is_none() {
                append_desktop_bootstrap_log(&state.data_dir,
                    "automatic replacement observed an external runtime; preserving it without claiming ownership or scheduling retries");
                return BackendRecoveryResult::Cancelled;
            }
            let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
                "managed_child_recovery_succeeded",
                "desktop_watchdog",
            );
            event.old_pid = Some(exited.pid);
            event.new_pid = new_pid;
            event.recovery_elapsed_ms = Some(started.elapsed().as_millis() as u64);
            let _ = bifrost_core::append_system_proxy_event(&state.data_dir, &event);
            BackendRecoveryResult::Recovered
        }
        Err(error) => {
            if backend_shutdown_requested(state)
                || state.backend_lifecycle_epoch.load(Ordering::SeqCst) != expected.epoch
            {
                return BackendRecoveryResult::Cancelled;
            }
            record_startup_error(state, format!("desktop watchdog recovery failed: {error}"));
            let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
                "managed_child_recovery_failed",
                "desktop_watchdog",
            );
            event.old_pid = Some(exited.pid);
            event.error = Some(error.to_string());
            event.recovery_elapsed_ms = Some(started.elapsed().as_millis() as u64);
            let _ = bifrost_core::append_system_proxy_event(&state.data_dir, &event);
            // Do not bless a newer marker or generation just because launch failed.
            // A marker removed by stale-PID cleanup is safe; another PID is not.
            let Ok(next) = BackendRecoverySnapshot::capture(state) else {
                return BackendRecoveryResult::Cancelled;
            };
            if next.epoch != expected.epoch
                || next.child_pid.is_some()
                || !recovery_generation_is_compatible(
                    expected.proxy_generation.as_deref(),
                    next.proxy_generation.as_deref(),
                )
                || !next.markers_belong_to_owned_pid(exited.pid, &launched_pids)
            {
                return BackendRecoveryResult::Cancelled;
            }
            suspend_failed_backend_proxy(state, guard, &next);
            let mut retry_exit = exited.clone();
            if let Some(pid) = next.runtime_pid().filter(|pid| launched_pids.contains(pid)) {
                retry_exit.pid = pid;
                retry_exit.detail =
                    format!("owned replacement pid={pid} failed readiness and was terminated");
            }
            BackendRecoveryResult::Retry(PendingBackendRecovery {
                expected: next,
                exited: retry_exit,
                retry_at: Instant::now() + BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY,
            })
        }
    }
}
