use super::*;

pub(super) fn cleanup_system_proxy_state(data_dir: &std::path::Path) -> bifrost_core::Result<()> {
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        "system proxy cleanup helper restore starting"
    );
    let started_at = std::time::Instant::now();
    let system_proxy_result = bifrost_core::retry_with_policy(
        bifrost_core::RECOVERY_RETRY_WINDOW,
        bifrost_core::RECOVERY_RETRY_INTERVAL,
        |attempt| {
            tracing::debug!(
                target: "bifrost_cli::shutdown",
                attempt,
                "system proxy cleanup helper invoking recover_from_crash"
            );
            bifrost_core::SystemProxyManager::recover_from_crash(data_dir)
        },
    );
    // CLI profile removal is independent from OS proxy recovery. Always attempt both so a
    // temporary networksetup/WinINET failure cannot leave shell proxy variables behind.
    let cli_proxy_result = bifrost_core::CliProxyEnvironmentManager::disable_all_managed();
    let cli_proxy_profile_count =
        combine_proxy_cleanup_results(system_proxy_result, cli_proxy_result)?;
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        cli_proxy_profiles = cli_proxy_profile_count,
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        "proxy cleanup helper restore completed"
    );
    Ok(())
}

pub(super) fn combine_proxy_cleanup_results(
    system_proxy_result: bifrost_core::Result<()>,
    cli_proxy_result: bifrost_core::Result<Vec<std::path::PathBuf>>,
) -> bifrost_core::Result<usize> {
    match (system_proxy_result, cli_proxy_result) {
        (Ok(()), Ok(paths)) => Ok(paths.len()),
        (Err(system_error), Ok(_)) => Err(system_error),
        (Ok(()), Err(cli_error)) => Err(cli_error),
        (Err(system_error), Err(cli_error)) => Err(bifrost_core::BifrostError::Config(format!(
            "System proxy cleanup failed: {system_error}; CLI proxy environment cleanup failed: {cli_error}"
        ))),
    }
}

pub(super) fn should_try_managed_runtime_restart(
    runtime: &RuntimeInfo,
    configured: &bifrost_storage::NewSystemProxyConfig,
) -> bool {
    runtime.restartable_daemon()
        && runtime.binary_path.is_some()
        && crate::process::resolve_runtime_system_proxy_intent(Some(runtime), configured).0
}

pub(super) fn build_managed_runtime_restart_args(
    runtime: &RuntimeInfo,
    desired: &(bool, String),
) -> Vec<String> {
    let mut args = vec![
        "start".to_string(),
        "--daemon".to_string(),
        "--port".to_string(),
        runtime.port.to_string(),
    ];
    if let Some(host) = runtime.host.as_deref().filter(|host| !host.is_empty()) {
        args.extend(["--host".into(), host.into()]);
    }
    if let Some(port) = runtime.socks5_port {
        args.extend(["--socks5-port".into(), port.to_string()]);
    }
    args.push(
        if desired.0 {
            "--system-proxy"
        } else {
            "--no-system-proxy"
        }
        .into(),
    );
    args.extend(["--proxy-bypass".into(), desired.1.clone()]);
    args
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedRuntimeRestartOutcome {
    NotAttempted,
    Ready,
    FailOpenSuspended,
    FailClosedPreserved,
    OwnershipChanged,
    Cancelled,
    RecoveryFailed,
}

pub(super) trait ManagedSystemProxyRecovery {
    fn ensure_managed_ownership(
        &mut self,
    ) -> bifrost_core::Result<Option<bifrost_core::ManagedSystemProxyOwnership>>;

    fn suspend_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition>;

    fn resume_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition>;

    fn suspend_managed_if_generation_guarded(
        &mut self,
        expected_generation: &str,
        should_suspend: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition>;
}

impl ManagedSystemProxyRecovery for bifrost_core::SystemProxyManager {
    fn suspend_managed_if_generation_guarded(
        &mut self,
        generation: &str,
        should_suspend: impl FnOnce() -> bifrost_core::Result<bool>,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
        bifrost_core::SystemProxyManager::suspend_managed_if_generation_guarded(
            self,
            generation,
            should_suspend,
        )
    }

    fn ensure_managed_ownership(
        &mut self,
    ) -> bifrost_core::Result<Option<bifrost_core::ManagedSystemProxyOwnership>> {
        bifrost_core::SystemProxyManager::ensure_managed_ownership(self)
    }

    fn suspend_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
        bifrost_core::SystemProxyManager::suspend_managed_if_generation(self, expected_generation)
    }

    fn resume_managed_if_generation(
        &mut self,
        expected_generation: &str,
    ) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
        bifrost_core::SystemProxyManager::resume_managed_if_generation(self, expected_generation)
    }
}

pub(super) fn reconcile_proxy_after_runtime_ready(
    manager: &mut impl ManagedSystemProxyRecovery,
    generation: &str,
) -> bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition> {
    // Even an already-active target must be verified under the core ownership
    // lock. A ready listener alone does not establish that we still own the OS.
    manager.resume_managed_if_generation(generation)
}

pub(super) fn read_system_proxy_config(
    data_dir: &std::path::Path,
) -> bifrost_core::Result<bifrost_storage::NewSystemProxyConfig> {
    bifrost_storage::read_persisted_system_proxy_config(data_dir)
}

pub(super) fn explicit_stop_requested(data_dir: &std::path::Path) -> bool {
    matches!(
        bifrost_core::read_system_proxy_shutdown_mode(data_dir),
        Some(
            bifrost_core::SystemProxyShutdownMode::BackgroundCleanup
                | bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
        )
    )
}

pub(super) fn transition_succeeded(
    result: &bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition>,
) -> bool {
    matches!(
        result,
        Ok(bifrost_core::GuardedSystemProxyTransition::Applied
            | bifrost_core::GuardedSystemProxyTransition::AlreadyInState)
    )
}

pub(super) fn transition_lost_ownership(
    result: &bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition>,
) -> bool {
    matches!(
        result,
        Ok(bifrost_core::GuardedSystemProxyTransition::OwnershipChanged
            | bifrost_core::GuardedSystemProxyTransition::NotManaged)
    )
}

fn record_recovery_transition(
    data_dir: &std::path::Path,
    generation: &str,
    event_name: &str,
    phase: &str,
    transition: &bifrost_core::Result<bifrost_core::GuardedSystemProxyTransition>,
    started_at: std::time::Instant,
) {
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(event_name, "system_proxy_helper");
    event.ownership_generation = Some(generation.into());
    event.system_proxy_action = Some(format!("{transition:?}").to_ascii_lowercase());
    event.error = transition.as_ref().err().map(ToString::to_string);
    event.recovery_elapsed_ms = Some(started_at.elapsed().as_millis() as u64);
    let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.ownership_generation = Some(generation.into());
        state.helper_pid = Some(std::process::id());
        state.helper_last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
        state.phase = Some(phase.into());
        state.last_action = Some(event_name.into());
        state.last_error = event.error.clone();
    });
}

pub(super) fn system_proxy_recovery_policy(
    data_dir: &std::path::Path,
) -> (SystemProxyRecoveryMode, std::time::Duration) {
    read_system_proxy_config(data_dir)
        .ok()
        .map(|config| {
            (
                config.recovery_mode,
                std::time::Duration::from_secs(config.recovery_grace_secs.clamp(
                    MIN_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
                    MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS,
                )),
            )
        })
        .unwrap_or((
            SystemProxyRecoveryMode::FailOpen,
            std::time::Duration::from_secs(MAX_SYSTEM_PROXY_RECOVERY_GRACE_SECS),
        ))
}

pub(super) fn recovery_mode_name(mode: SystemProxyRecoveryMode) -> &'static str {
    match mode {
        SystemProxyRecoveryMode::FailOpen => "fail_open",
        SystemProxyRecoveryMode::FailClosed => "fail_closed",
    }
}

pub(super) fn read_runtime_info_from(data_dir: &std::path::Path) -> Option<RuntimeInfo> {
    read_runtime_info_from_checked(data_dir).ok().flatten()
}

pub(super) fn read_runtime_info_from_checked(
    data_dir: &std::path::Path,
) -> bifrost_core::Result<Option<RuntimeInfo>> {
    let content = match std::fs::read_to_string(data_dir.join("runtime.json")) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&content).map(Some).map_err(|error| {
        bifrost_core::BifrostError::Config(format!("Cannot verify runtime marker: {error}"))
    })
}

#[cfg(test)]
pub(super) fn restart_managed_runtime_before_cleanup(
    data_dir: &std::path::Path,
) -> ManagedRuntimeRestartOutcome {
    restart_managed_runtime_before_cleanup_with_timeout(
        data_dir,
        std::time::Duration::from_secs(30),
    )
}

#[cfg(test)]
pub(super) fn restart_managed_runtime_before_cleanup_with_timeout(
    data_dir: &std::path::Path,
    ready_timeout: std::time::Duration,
) -> ManagedRuntimeRestartOutcome {
    let mut manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
    let outcome = restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
        data_dir,
        ready_timeout,
        &mut manager,
    );
    manager.detach();
    outcome
}

#[cfg(any(unix, test))]
pub(super) fn restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
    data_dir: &std::path::Path,
    ready_timeout: std::time::Duration,
    manager: &mut impl ManagedSystemProxyRecovery,
) -> ManagedRuntimeRestartOutcome {
    restart_managed_runtime_with_generation(data_dir, ready_timeout, manager, None)
}

pub(super) fn restart_managed_runtime_with_generation(
    data_dir: &std::path::Path,
    ready_timeout: std::time::Duration,
    manager: &mut impl ManagedSystemProxyRecovery,
    expected_generation: Option<&str>,
) -> ManagedRuntimeRestartOutcome {
    let Some(runtime) = read_runtime_info_from(data_dir) else {
        return ManagedRuntimeRestartOutcome::NotAttempted;
    };
    if explicit_stop_requested(data_dir) {
        return ManagedRuntimeRestartOutcome::Cancelled;
    }
    let configured = match read_system_proxy_config(data_dir) {
        Ok(configured) => configured,
        Err(error) => {
            tracing::warn!(%error, "cannot read latest proxy intent; recovery deferred");
            return ManagedRuntimeRestartOutcome::RecoveryFailed;
        }
    };
    let identity = inspect_process_identity(runtime.pid, runtime.started_at_ms);
    if identity == ProcessIdentityStatus::Unknown {
        return ManagedRuntimeRestartOutcome::RecoveryFailed;
    }
    let restarting = matches!(
        identity,
        ProcessIdentityStatus::Exited | ProcessIdentityStatus::Reused
    );
    let desktop_exit =
        restarting && runtime.start_mode == crate::process::RuntimeStartMode::Desktop;
    if !crate::process::resolve_runtime_system_proxy_intent(Some(&runtime), &configured).0
        || (restarting
            && !desktop_exit
            && !should_try_managed_runtime_restart(&runtime, &configured))
    {
        return ManagedRuntimeRestartOutcome::NotAttempted;
    }
    if restarting
        && !desktop_exit
        && !runtime
            .binary_path
            .as_ref()
            .is_some_and(|path| path.exists())
    {
        let error = format!(
            "runtime binary path does not exist: {}",
            runtime
                .binary_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        );
        let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
            state.phase = Some("restart_preflight_failed".into());
            state.last_action = Some("validate_runtime_binary".into());
            state.last_error = Some(error);
        });
        return ManagedRuntimeRestartOutcome::NotAttempted;
    }
    let ownership = match manager.ensure_managed_ownership() {
        Ok(Some(ownership))
            if ownership.target.target_matches(
                runtime_system_proxy_host(runtime.host.as_deref()),
                runtime.port,
            ) =>
        {
            ownership
        }
        Ok(_) => return ManagedRuntimeRestartOutcome::NotAttempted,
        Err(error) => {
            tracing::warn!(%error, "runtime restart could not load proxy ownership");
            return ManagedRuntimeRestartOutcome::NotAttempted;
        }
    };
    if expected_generation.is_some_and(|expected| ownership.generation != expected) {
        return ManagedRuntimeRestartOutcome::OwnershipChanged;
    }
    let generation = ownership.generation.clone();
    let (recovery_mode, grace) = system_proxy_recovery_policy(data_dir);
    let started_at = std::time::Instant::now();
    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.recovery_mode = Some(recovery_mode_name(recovery_mode).into());
        state.recovery_grace_secs = Some(grace.as_secs());
    });
    if desktop_exit {
        return suspend_desktop_exit_for_replacement(
            data_dir,
            manager,
            &runtime,
            &generation,
            recovery_mode,
            ready_timeout,
            started_at,
        );
    }
    // Partial durable suspension phases are retryable, not proof of success.
    let mut suspended = false;
    let mut last_error = None;
    // A slow or hung live runtime is not evidence of process death. In
    // particular wake must never use `start --yes` to kill a live replacement.
    if restarting {
        if explicit_stop_requested(data_dir)
            || !read_runtime_info_from(data_dir)
                .as_ref()
                .is_some_and(|current| same_runtime_identity(current, &runtime))
        {
            return ManagedRuntimeRestartOutcome::Cancelled;
        }
        let (desired, intent_revision) = match read_system_proxy_config(data_dir) {
            Ok(config) => (
                crate::process::resolve_runtime_system_proxy_intent(Some(&runtime), &config),
                config.intent_revision,
            ),
            Err(_) => return ManagedRuntimeRestartOutcome::RecoveryFailed,
        };
        if !desired.0 {
            return ManagedRuntimeRestartOutcome::NotAttempted;
        }
        match bifrost_core::try_write_system_proxy_restart_handoff(data_dir) {
            Ok(true) => {}
            Ok(false) => return ManagedRuntimeRestartOutcome::Cancelled,
            Err(error) => {
                tracing::warn!(%error, "runtime restart failed to persist handoff marker");
                return ManagedRuntimeRestartOutcome::NotAttempted;
            }
        }
        if explicit_stop_requested(data_dir)
            || !read_runtime_info_from(data_dir)
                .as_ref()
                .is_some_and(|current| same_runtime_identity(current, &runtime))
        {
            return ManagedRuntimeRestartOutcome::Cancelled;
        }
        // Never pass --yes: if another launcher wins after this check, normal
        // start must refuse to kill its live runtime rather than replace it.
        let args = build_managed_runtime_restart_args(&runtime, &desired);
        let mut command = std::process::Command::new(
            runtime
                .binary_path
                .as_ref()
                .expect("restart preflight checked binary"),
        );
        command
            .args(args)
            .env("BIFROST_DATA_DIR", data_dir)
            .env(
                "BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL",
                &generation,
            )
            .env(
                "BIFROST_SYSTEM_PROXY_INTENT_REVISION_INTERNAL",
                intent_revision.to_string(),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0200 | 0x0000_0008);
        }
        match command.spawn() {
            Ok(mut launcher) => {
                std::thread::spawn(move || {
                    let _ = launcher.wait();
                });
            }
            Err(error) => {
                consume_restart_handoff(data_dir);
                return apply_recovery_policy_after_failed_restart(
                    data_dir,
                    manager,
                    &generation,
                    recovery_mode,
                    started_at,
                    suspended,
                    Some(error.to_string()),
                );
            }
        }
        record_recovery_transition(
            data_dir,
            &generation,
            "helper_runtime_restart_started",
            "restarting_daemon",
            &Ok(bifrost_core::GuardedSystemProxyTransition::AlreadyInState),
            started_at,
        );
    }

    let mut readiness = StableReadiness::default();
    let mut closed_reported = false;
    // Stop and latest intent must also be checked after the readiness budget
    // expires. Slow metadata I/O can exhaust it before the first probe.
    loop {
        if explicit_stop_requested(data_dir) {
            return ManagedRuntimeRestartOutcome::Cancelled;
        }
        let current = read_runtime_info_from(data_dir);
        let desired = match read_system_proxy_config(data_dir) {
            Ok(config) => crate::process::resolve_runtime_system_proxy_intent(
                current.as_ref().or(Some(&runtime)),
                &config,
            ),
            Err(error) => {
                if started_at.elapsed() >= ready_timeout {
                    return ManagedRuntimeRestartOutcome::RecoveryFailed;
                }
                last_error = Some(error.to_string());
                readiness.reset();
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            }
        };
        if !desired.0 {
            let transition = manager.suspend_managed_if_generation(&generation);
            record_recovery_transition(
                data_dir,
                &generation,
                "helper_recovery_cancelled",
                "desired_disabled",
                &transition,
                started_at,
            );
            return if transition_succeeded(&transition) {
                ManagedRuntimeRestartOutcome::Cancelled
            } else if transition_lost_ownership(&transition) {
                ManagedRuntimeRestartOutcome::OwnershipChanged
            } else {
                ManagedRuntimeRestartOutcome::RecoveryFailed
            };
        }
        if started_at.elapsed() >= ready_timeout {
            break;
        }
        let current = current.filter(|current| same_runtime_target(current, &runtime));
        let ready = current.as_ref().is_some_and(|current| {
            runtime_identity_is_current(current) && runtime_data_plane_is_ready(current)
        });
        if readiness.observe(current.as_ref(), ready, started_at.elapsed()) {
            // Re-read intent after the canary and immediately before the
            // generation-guarded transition. A disabled target is never resumed.
            let latest = read_system_proxy_config(data_dir).ok().map(|config| {
                crate::process::resolve_runtime_system_proxy_intent(current.as_ref(), &config).0
            });
            if latest != Some(true) || explicit_stop_requested(data_dir) {
                readiness.reset();
                continue;
            }
            if !read_runtime_info_from(data_dir)
                .as_ref()
                .zip(current.as_ref())
                .is_some_and(|(latest, sampled)| same_runtime_identity(latest, sampled))
            {
                readiness.reset();
                continue;
            }
            let transition = reconcile_proxy_after_runtime_ready(manager, &generation);
            if transition_succeeded(&transition) {
                suspended = false;
                let latest_runtime = read_runtime_info_from(data_dir);
                let still_desired = read_system_proxy_config(data_dir).ok().map(|config| {
                    crate::process::resolve_runtime_system_proxy_intent(
                        latest_runtime.as_ref(),
                        &config,
                    )
                    .0
                });
                if still_desired.is_none() {
                    return ManagedRuntimeRestartOutcome::RecoveryFailed;
                }
                if still_desired == Some(false) {
                    let compensated = manager.suspend_managed_if_generation(&generation);
                    record_recovery_transition(
                        data_dir,
                        &generation,
                        "helper_resume_intent_changed",
                        "desired_disabled",
                        &compensated,
                        started_at,
                    );
                    return if transition_succeeded(&compensated) {
                        ManagedRuntimeRestartOutcome::Cancelled
                    } else if transition_lost_ownership(&compensated) {
                        ManagedRuntimeRestartOutcome::OwnershipChanged
                    } else {
                        ManagedRuntimeRestartOutcome::RecoveryFailed
                    };
                }
                if explicit_stop_requested(data_dir) {
                    return ManagedRuntimeRestartOutcome::Cancelled;
                }
                if !latest_runtime
                    .as_ref()
                    .zip(current.as_ref())
                    .is_some_and(|(latest, sampled)| same_runtime_identity(latest, sampled))
                {
                    readiness.reset();
                    continue;
                }
                record_recovery_transition(
                    data_dir,
                    &generation,
                    "helper_runtime_restart_ready",
                    "running",
                    &transition,
                    started_at,
                );
                consume_restart_handoff(data_dir);
                return ManagedRuntimeRestartOutcome::Ready;
            }
            record_recovery_transition(
                data_dir,
                &generation,
                "helper_proxy_resume_failed",
                "recovering_resume_failed",
                &transition,
                started_at,
            );
            if transition_lost_ownership(&transition) {
                return ManagedRuntimeRestartOutcome::OwnershipChanged;
            }
            last_error = transition.err().map(|error| error.to_string());
            suspended = false;
            readiness.reset();
        }
        if started_at.elapsed() >= grace {
            match recovery_mode {
                SystemProxyRecoveryMode::FailOpen if !suspended => {
                    let transition = manager.suspend_managed_if_generation(&generation);
                    suspended = transition_succeeded(&transition);
                    record_recovery_transition(
                        data_dir,
                        &generation,
                        if suspended {
                            "helper_fail_open_applied"
                        } else {
                            "helper_fail_open_failed"
                        },
                        if suspended {
                            "recovering_fail_open"
                        } else {
                            "recovering_suspend_failed"
                        },
                        &transition,
                        started_at,
                    );
                    if transition_lost_ownership(&transition) {
                        return ManagedRuntimeRestartOutcome::OwnershipChanged;
                    }
                    last_error = transition.err().map(|error| error.to_string());
                }
                SystemProxyRecoveryMode::FailClosed if !closed_reported => {
                    closed_reported = true;
                    record_recovery_transition(
                        data_dir,
                        &generation,
                        "helper_fail_closed_preserved",
                        "recovering_fail_closed",
                        &Ok(bifrost_core::GuardedSystemProxyTransition::AlreadyInState),
                        started_at,
                    );
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    // Conditional consumption cannot delete a newer explicit stop request.
    consume_restart_handoff(data_dir);
    apply_recovery_policy_after_failed_restart(
        data_dir,
        manager,
        &generation,
        recovery_mode,
        started_at,
        suspended,
        last_error,
    )
}

pub(super) fn apply_recovery_policy_after_failed_restart(
    data_dir: &std::path::Path,
    manager: &mut impl ManagedSystemProxyRecovery,
    generation: &str,
    recovery_mode: SystemProxyRecoveryMode,
    recovery_started_at: std::time::Instant,
    _action_already_applied: bool,
    error: Option<String>,
) -> ManagedRuntimeRestartOutcome {
    if explicit_stop_requested(data_dir) {
        return ManagedRuntimeRestartOutcome::Cancelled;
    }
    // Revalidate the generation even if a previous suspension succeeded: another
    // owner may have changed the settings while we waited for the runtime.
    let action = if recovery_mode == SystemProxyRecoveryMode::FailOpen {
        manager.suspend_managed_if_generation(generation)
    } else {
        Ok(bifrost_core::GuardedSystemProxyTransition::AlreadyInState)
    };
    let outcome = if transition_lost_ownership(&action) {
        ManagedRuntimeRestartOutcome::OwnershipChanged
    } else if !transition_succeeded(&action) {
        ManagedRuntimeRestartOutcome::RecoveryFailed
    } else if recovery_mode == SystemProxyRecoveryMode::FailOpen {
        ManagedRuntimeRestartOutcome::FailOpenSuspended
    } else {
        ManagedRuntimeRestartOutcome::FailClosedPreserved
    };
    let phase = match outcome {
        ManagedRuntimeRestartOutcome::FailOpenSuspended => "recovering_fail_open",
        ManagedRuntimeRestartOutcome::FailClosedPreserved => "recovering_fail_closed",
        ManagedRuntimeRestartOutcome::OwnershipChanged => "ownership_changed",
        _ => "recovery_failed",
    };
    record_recovery_transition(
        data_dir,
        generation,
        "helper_runtime_restart_not_ready",
        phase,
        &action,
        recovery_started_at,
    );
    if let Some(error) = error {
        let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
            state.last_error = Some(error)
        });
    }
    outcome
}

pub(super) fn cleanup_or_restart_managed_runtime(
    data_dir: &std::path::Path,
    fence: &RuntimeCleanupFence,
) -> bifrost_core::Result<()> {
    let Some(generation) = fence.generation.as_deref() else {
        return cleanup_owned_proxy_after_exit(data_dir, fence);
    };
    let mut manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
    let outcome = restart_managed_runtime_with_generation(
        data_dir,
        std::time::Duration::from_secs(30),
        &mut manager,
        Some(generation),
    );
    manager.detach();
    match outcome {
        ManagedRuntimeRestartOutcome::NotAttempted => {
            cleanup_owned_proxy_after_exit(data_dir, fence)
        }
        ManagedRuntimeRestartOutcome::RecoveryFailed => {
            let fail_closed = bifrost_core::read_system_proxy_owner_state(data_dir)
                .ok()
                .flatten()
                .and_then(|state| state.recovery_mode)
                .as_deref()
                == Some("fail_closed");
            let desktop_exit = fence.runtime.as_ref().is_some_and(|runtime| {
                runtime.start_mode == crate::process::RuntimeStartMode::Desktop
            });
            if !fail_closed && !desktop_exit {
                // Missing intent never authorizes acquisition, but an old,
                // confirmed-dead generation may still be safely restored.
                cleanup_owned_proxy_after_exit(data_dir, fence)?;
            }
            Err(bifrost_core::BifrostError::Config(
                "Managed proxy recovery is incomplete; see the recorded transition error".into(),
            ))
        }
        ManagedRuntimeRestartOutcome::Ready
        | ManagedRuntimeRestartOutcome::FailOpenSuspended
        | ManagedRuntimeRestartOutcome::FailClosedPreserved
        | ManagedRuntimeRestartOutcome::OwnershipChanged
        | ManagedRuntimeRestartOutcome::Cancelled => Ok(()),
    }
}

fn consume_restart_handoff(data_dir: &std::path::Path) {
    bifrost_core::consume_system_proxy_shutdown_mode_if(
        data_dir,
        bifrost_core::SystemProxyShutdownMode::PreserveForRestart,
    );
}

fn suspend_desktop_exit_for_replacement(
    data_dir: &std::path::Path,
    manager: &mut impl ManagedSystemProxyRecovery,
    runtime: &RuntimeInfo,
    generation: &str,
    recovery_mode: SystemProxyRecoveryMode,
    timeout: std::time::Duration,
    started_at: std::time::Instant,
) -> ManagedRuntimeRestartOutcome {
    if recovery_mode == SystemProxyRecoveryMode::FailClosed {
        return ManagedRuntimeRestartOutcome::FailClosedPreserved;
    }
    let fence = RuntimeCleanupFence {
        runtime: Some(runtime.clone()),
        parent_pid: Some(runtime.pid),
        parent_started_at_ms: runtime.started_at_ms,
        generation: Some(generation.into()),
    };
    loop {
        if explicit_stop_requested(data_dir) {
            return ManagedRuntimeRestartOutcome::Cancelled;
        }
        let transition = manager
            .suspend_managed_if_generation_guarded(generation, || fence.permits_cleanup(data_dir));
        let succeeded = transition_succeeded(&transition);
        record_recovery_transition(
            data_dir,
            generation,
            if succeeded {
                "helper_desktop_exit_suspended"
            } else {
                "helper_desktop_exit_suspend_failed"
            },
            if succeeded {
                "recovering_desktop_fail_open"
            } else {
                "recovering_suspend_failed"
            },
            &transition,
            started_at,
        );
        if succeeded {
            return ManagedRuntimeRestartOutcome::FailOpenSuspended;
        }
        if transition_lost_ownership(&transition) {
            return ManagedRuntimeRestartOutcome::OwnershipChanged;
        }
        if started_at.elapsed() >= timeout {
            return ManagedRuntimeRestartOutcome::RecoveryFailed;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
