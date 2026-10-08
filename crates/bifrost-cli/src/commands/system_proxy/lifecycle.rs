use super::*;

pub(super) fn cleanup_after_parent_exit(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
    trigger: LifecycleRecoveryTrigger,
) -> bifrost_core::Result<()> {
    if read_runtime_info_from(data_dir).is_some_and(|runtime| {
        parent_pid.is_some_and(|pid| pid != runtime.pid)
            || parent_started_at_ms
                .zip(runtime.started_at_ms)
                .is_some_and(|(old, new)| old != new)
    }) {
        tracing::info!("stale lifecycle helper left replacement runtime unchanged");
        return Ok(());
    }
    let runtime = read_runtime_info_from_checked(data_dir)?;
    let manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
    let fence = RuntimeCleanupFence {
        runtime,
        parent_pid,
        parent_started_at_ms,
        generation: manager
            .read_managed_ownership()?
            .map(|ownership| ownership.generation),
    };
    let recovery_started_at = std::time::Instant::now();
    let helper_pid = std::process::id();
    let _ = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.helper_pid = Some(helper_pid);
        state.helper_last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
        state.phase = Some("parent_exit_recovery".into());
        state.last_action = Some(trigger.as_str().into());
    });
    tracing::info!(
        target: "bifrost_cli::shutdown",
        helper_pid,
        parent_pid = parent_pid.unwrap_or_default(),
        parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
        detection_method = trigger.as_str(),
        data_dir = %data_dir.display(),
        "system proxy lifecycle recovery started"
    );
    let (action, result) = match bifrost_core::read_system_proxy_shutdown_mode(data_dir) {
        Some(bifrost_core::SystemProxyShutdownMode::BackgroundCleanup) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                detection_method = trigger.as_str(),
                "system proxy lifecycle helper running stop-requested background cleanup"
            );
            (
                "background_cleanup",
                cleanup_owned_proxy_after_exit(data_dir, &fence),
            )
        }
        Some(bifrost_core::SystemProxyShutdownMode::ForegroundCleanup) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                detection_method = trigger.as_str(),
                "system proxy lifecycle helper exiting because stop cleaned proxy before parent exit"
            );
            ("already_cleaned", Ok(()))
        }
        Some(bifrost_core::SystemProxyShutdownMode::PreserveForRestart) => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                data_dir = %data_dir.display(),
                detection_method = trigger.as_str(),
                "system proxy lifecycle helper skipping cleanup for restart"
            );
            ("preserve_for_restart", Ok(()))
        }
        None => (
            "restart_or_restore",
            cleanup_or_restart_managed_runtime(data_dir, &fence),
        ),
    };

    match &result {
        Ok(()) => tracing::info!(
            target: "bifrost_cli::shutdown",
            helper_pid,
            parent_pid = parent_pid.unwrap_or_default(),
            parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
            detection_method = trigger.as_str(),
            recovery_action = action,
            elapsed_ms = recovery_started_at.elapsed().as_millis() as u64,
            "system proxy lifecycle recovery completed"
        ),
        Err(error) => tracing::warn!(
            target: "bifrost_cli::shutdown",
            helper_pid,
            parent_pid = parent_pid.unwrap_or_default(),
            parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
            detection_method = trigger.as_str(),
            recovery_action = action,
            elapsed_ms = recovery_started_at.elapsed().as_millis() as u64,
            error = %error,
            "system proxy lifecycle recovery failed; managed proxy state may require manual repair"
        ),
    }
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
        "lifecycle_helper_recovery_completed",
        "system_proxy_helper",
    );
    event.old_pid = parent_pid;
    event.new_pid = read_runtime_info_from(data_dir).map(|runtime| runtime.pid);
    event.trigger = Some(trigger.as_str().into());
    event.decision = Some(action.into());
    event.error = result.as_ref().err().map(ToString::to_string);
    event.recovery_elapsed_ms = Some(recovery_started_at.elapsed().as_millis() as u64);
    let _ = bifrost_core::append_system_proxy_event(data_dir, &event);
    result
}

#[cfg(unix)]
pub(super) fn reconcile_system_proxy_after_power_wake(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
) -> bifrost_core::Result<()> {
    reconcile_system_proxy_after_power_wake_with(
        data_dir,
        parent_pid,
        parent_started_at_ms,
        |data_dir| {
            let mut manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
            let outcome = restart_managed_runtime_before_cleanup_with_timeout_and_proxy(
                data_dir,
                std::time::Duration::from_secs(5),
                &mut manager,
            );
            manager.detach();
            outcome
        },
    )
}

#[cfg(unix)]
pub(super) fn reconcile_system_proxy_after_power_wake_with(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
    recover: impl FnOnce(&std::path::Path) -> ManagedRuntimeRestartOutcome,
) -> bifrost_core::Result<()> {
    if explicit_stop_requested(data_dir) {
        return Ok(());
    }
    let Some(runtime) = read_runtime_info_from(data_dir) else {
        // A missing marker during startup is inconclusive; do not restore a
        // replacement's OS proxy based on an old helper's wake notification.
        return Ok(());
    };
    if parent_pid.is_some_and(|pid| pid != runtime.pid)
        || parent_started_at_ms
            .zip(runtime.started_at_ms)
            .is_some_and(|(old, new)| old != new)
    {
        return Ok(());
    }
    if matches!(
        parent_identity_status(parent_pid, parent_started_at_ms),
        ProcessIdentityStatus::Unknown
    ) {
        return Ok(());
    }
    let configured = read_system_proxy_config(data_dir)?;
    if !crate::process::resolve_runtime_system_proxy_intent(Some(&runtime), &configured).0 {
        return Ok(());
    }
    // Recovery uses HTTP canaries with a stability window, current intent,
    // and the persisted generation. It never calls blanket `enable` on wake.
    let outcome = recover(data_dir);
    match outcome {
        ManagedRuntimeRestartOutcome::Ready
        | ManagedRuntimeRestartOutcome::Cancelled
        | ManagedRuntimeRestartOutcome::OwnershipChanged
        | ManagedRuntimeRestartOutcome::NotAttempted => Ok(()),
        _ => Err(bifrost_core::BifrostError::Config(
            "Wake recovery is not ready; retrying without taking another proxy owner's settings"
                .into(),
        )),
    }
}

#[cfg(unix)]
pub(super) fn process_power_notifications(
    events: &std::sync::mpsc::Receiver<PowerEvent>,
    recovery_pending: &mut bool,
    reconcile: impl FnOnce() -> bifrost_core::Result<()>,
) {
    while let Ok(event) = events.try_recv() {
        tracing::info!(target: "bifrost_cli::shutdown", ?event,
            "system proxy lifecycle helper received power notification");
        if event == PowerEvent::SystemHasPoweredOn {
            *recovery_pending = true;
        }
    }
    if *recovery_pending {
        match reconcile() {
            Ok(()) => *recovery_pending = false,
            Err(error) => tracing::warn!(%error, "system proxy wake recovery deferred for retry"),
        }
    }
}

pub(super) fn parent_identity_status(
    parent_pid: Option<u32>,
    recorded_started_at_ms: Option<u64>,
) -> ProcessIdentityStatus {
    parent_pid.map_or(ProcessIdentityStatus::Unknown, |pid| {
        inspect_process_identity(pid, recorded_started_at_ms)
    })
}

pub(super) fn immediate_parent_exit_trigger(
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
) -> Option<LifecycleRecoveryTrigger> {
    match parent_identity_status(parent_pid, parent_started_at_ms) {
        ProcessIdentityStatus::Exited => Some(LifecycleRecoveryTrigger::PidMissing),
        ProcessIdentityStatus::Reused => Some(LifecycleRecoveryTrigger::PidReused),
        ProcessIdentityStatus::Alive | ProcessIdentityStatus::Unknown => None,
    }
}

pub(super) fn record_lifecycle_helper_heartbeat(
    data_dir: &std::path::Path,
    parent_pid: Option<u32>,
    phase: &str,
) {
    if let Err(error) = bifrost_core::update_system_proxy_owner_state(data_dir, |state| {
        state.helper_pid = Some(std::process::id());
        state.helper_started_at_ms = state
            .helper_started_at_ms
            .or_else(bifrost_core::current_process_start_time_ms);
        state.helper_last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
        state.pid = parent_pid.or(state.pid);
        state.phase = Some(phase.into());
    }) {
        tracing::warn!(error = %error, "failed to persist lifecycle helper heartbeat");
    }
}

pub(super) fn run_system_proxy_lifecycle_helper(
    data_dir: std::path::PathBuf,
    parent_pid: Option<u32>,
    parent_started_at_ms: Option<u64>,
    poll_secs: u64,
) -> bifrost_core::Result<()> {
    set_data_dir(data_dir.clone());
    let poll_interval = std::time::Duration::from_secs(poll_secs.max(1));
    let required_parent_misses = 3_u32;
    record_lifecycle_helper_heartbeat(&data_dir, parent_pid, "monitoring_parent");
    let mut event = bifrost_core::SystemProxyLifecycleEvent::new(
        "lifecycle_helper_started",
        "system_proxy_helper",
    );
    event.old_pid = parent_pid;
    event.new_pid = Some(std::process::id());
    let _ = bifrost_core::append_system_proxy_event(&data_dir, &event);
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        parent_pid = parent_pid.unwrap_or_default(),
        parent_started_at_ms = parent_started_at_ms.unwrap_or_default(),
        poll_secs = poll_interval.as_secs(),
        fast_identity_poll_ms = 250_u64,
        required_parent_misses,
        "system proxy lifecycle helper started; fast process-identity checks do not use listener or HTTP readiness"
    );

    match parent_identity_status(parent_pid, parent_started_at_ms) {
        ProcessIdentityStatus::Reused => {
            tracing::warn!(
                target: "bifrost_cli::shutdown",
                parent_pid = parent_pid.unwrap_or_default(),
                detection_method = LifecycleRecoveryTrigger::PidReused.as_str(),
                "system proxy lifecycle helper detected parent PID reuse at startup; running guarded recovery"
            );
            return cleanup_after_parent_exit(
                &data_dir,
                parent_pid,
                parent_started_at_ms,
                LifecycleRecoveryTrigger::PidReused,
            );
        }
        ProcessIdentityStatus::Exited => {
            tracing::info!(
                target: "bifrost_cli::shutdown",
                parent_pid = parent_pid.unwrap_or_default(),
                detection_method = LifecycleRecoveryTrigger::PidMissing.as_str(),
                "system proxy lifecycle helper observed parent PID missing at startup; running immediate guarded recovery"
            );
            return cleanup_after_parent_exit(
                &data_dir,
                parent_pid,
                parent_started_at_ms,
                LifecycleRecoveryTrigger::PidMissing,
            );
        }
        ProcessIdentityStatus::Alive | ProcessIdentityStatus::Unknown => {}
    }

    #[cfg(unix)]
    {
        let (power_tx, power_rx) = std::sync::mpsc::channel::<PowerEvent>();
        #[cfg(target_os = "macos")]
        let _power_watcher = {
            match PowerNotificationWatcher::start(power_tx) {
                Ok(watcher) => {
                    tracing::info!(
                        target: "bifrost_cli::shutdown",
                        "system proxy lifecycle helper power watcher started"
                    );
                    Some(watcher)
                }
                Err(error) => {
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        error = %error,
                        "system proxy lifecycle helper power watcher failed to start"
                    );
                    None
                }
            }
        };
        #[cfg(not(target_os = "macos"))]
        let _power_watcher = {
            drop(power_tx);
            None::<()>
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to start system proxy lifecycle helper runtime: {error}"
                ))
            })?;
        runtime.block_on(async move {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm = signal(SignalKind::terminate()).map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to install SIGTERM handler for system proxy lifecycle helper: {error}"
                ))
            })?;
            let mut sigint = signal(SignalKind::interrupt()).map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to install SIGINT handler for system proxy lifecycle helper: {error}"
                ))
            })?;
            let mut sighup = signal(SignalKind::hangup()).map_err(|error| {
                bifrost_core::BifrostError::Config(format!(
                    "Failed to install SIGHUP handler for system proxy lifecycle helper: {error}"
                ))
            })?;

            let mut consecutive_parent_misses = 0_u32;
            let mut wake_recovery_pending = false;
            let mut parent_poll = tokio::time::interval(poll_interval);
            parent_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // A direct process-instance disappearance is a strong liveness
            // signal. Check it much more frequently than the conservative
            // zombie/legacy fallback, without involving the proxy port or
            // Admin readiness endpoint.
            let mut parent_identity_poll =
                tokio::time::interval(std::time::Duration::from_millis(250));
            parent_identity_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut power_poll = tokio::time::interval(std::time::Duration::from_millis(250));
            power_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut helper_heartbeat_poll =
                tokio::time::interval(std::time::Duration::from_secs(5));
            helper_heartbeat_poll
                .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = helper_heartbeat_poll.tick() => {
                        record_lifecycle_helper_heartbeat(&data_dir, parent_pid, "monitoring_parent");
                    },
                    _ = sigterm.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGTERM");
                        return cleanup_after_parent_exit(&data_dir, parent_pid, parent_started_at_ms, LifecycleRecoveryTrigger::Signal("sigterm"));
                    },
                    _ = sigint.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGINT");
                        return cleanup_after_parent_exit(&data_dir, parent_pid, parent_started_at_ms, LifecycleRecoveryTrigger::Signal("sigint"));
                    },
                    _ = sighup.recv() => {
                        tracing::info!(target: "bifrost_cli::shutdown", "system proxy lifecycle helper received SIGHUP");
                        return cleanup_after_parent_exit(&data_dir, parent_pid, parent_started_at_ms, LifecycleRecoveryTrigger::Signal("sighup"));
                    },
                    _ = parent_identity_poll.tick() => {
                        if let Some(trigger) = immediate_parent_exit_trigger(
                            parent_pid,
                            parent_started_at_ms,
                        ) {
                            tracing::info!(
                                target: "bifrost_cli::shutdown",
                                parent_pid = parent_pid.unwrap_or_default(),
                                detection_method = trigger.as_str(),
                                "system proxy lifecycle helper observed confirmed parent-instance exit during fast identity check"
                            );
                            return cleanup_after_parent_exit(
                                &data_dir,
                                parent_pid,
                                parent_started_at_ms,
                                trigger,
                            );
                        }
                    },
                    _ = power_poll.tick() => {
                        process_power_notifications(&power_rx, &mut wake_recovery_pending, || {
                            reconcile_system_proxy_after_power_wake(&data_dir, parent_pid, parent_started_at_ms)
                        });
                    },
                    _ = parent_poll.tick() => {
                        if let Some(pid) = parent_pid {
                            if !is_process_running(pid) {
                                consecutive_parent_misses += 1;
                                tracing::warn!(
                                    target: "bifrost_cli::shutdown",
                                    parent_pid = pid,
                                    consecutive_parent_misses,
                                    required_parent_misses,
                                    "system proxy lifecycle helper parent process not visible"
                                );
                                if consecutive_parent_misses >= required_parent_misses {
                                    tracing::info!(
                                        target: "bifrost_cli::shutdown",
                                        parent_pid = pid,
                                        "system proxy lifecycle helper confirmed parent exit"
                                    );
                                    return cleanup_after_parent_exit(
                                        &data_dir,
                                        parent_pid,
                                        parent_started_at_ms,
                                        LifecycleRecoveryTrigger::PollConfirmedExit,
                                    );
                                }
                            } else {
                                consecutive_parent_misses = 0;
                            }
                        }
                    },
                }
            }
        })
    }

    #[cfg(not(unix))]
    {
        let mut consecutive_parent_misses = 0_u32;
        let fast_identity_interval = std::time::Duration::from_millis(250);
        let mut next_parent_poll = std::time::Instant::now() + poll_interval;
        let mut next_helper_heartbeat =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            std::thread::sleep(fast_identity_interval);
            if std::time::Instant::now() >= next_helper_heartbeat {
                record_lifecycle_helper_heartbeat(&data_dir, parent_pid, "monitoring_parent");
                next_helper_heartbeat =
                    std::time::Instant::now() + std::time::Duration::from_secs(5);
            }
            match parent_identity_status(parent_pid, parent_started_at_ms) {
                ProcessIdentityStatus::Reused => {
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        parent_pid = parent_pid.unwrap_or_default(),
                        detection_method = LifecycleRecoveryTrigger::PidReused.as_str(),
                        "system proxy lifecycle helper detected parent PID reuse; running guarded recovery"
                    );
                    return cleanup_after_parent_exit(
                        &data_dir,
                        parent_pid,
                        parent_started_at_ms,
                        LifecycleRecoveryTrigger::PidReused,
                    );
                }
                ProcessIdentityStatus::Exited => {
                    tracing::info!(
                        target: "bifrost_cli::shutdown",
                        parent_pid = parent_pid.unwrap_or_default(),
                        detection_method = LifecycleRecoveryTrigger::PidMissing.as_str(),
                        "system proxy lifecycle helper observed parent PID missing; running immediate guarded recovery"
                    );
                    return cleanup_after_parent_exit(
                        &data_dir,
                        parent_pid,
                        parent_started_at_ms,
                        LifecycleRecoveryTrigger::PidMissing,
                    );
                }
                ProcessIdentityStatus::Alive | ProcessIdentityStatus::Unknown => {}
            }

            // Keep the historical boolean probe at its conservative cadence.
            // It still handles zombie and platform-specific fallback cases, but
            // it must not delay an explicit PID-instance disappearance.
            if std::time::Instant::now() < next_parent_poll {
                continue;
            }
            next_parent_poll = std::time::Instant::now() + poll_interval;
            if let Some(pid) = parent_pid {
                if !is_process_running(pid) {
                    consecutive_parent_misses += 1;
                    tracing::warn!(
                        target: "bifrost_cli::shutdown",
                        parent_pid = pid,
                        consecutive_parent_misses,
                        required_parent_misses,
                        "system proxy lifecycle helper parent process not visible"
                    );
                    if consecutive_parent_misses >= required_parent_misses {
                        tracing::info!(
                            target: "bifrost_cli::shutdown",
                            parent_pid = pid,
                            "system proxy lifecycle helper confirmed parent exit"
                        );
                        return cleanup_after_parent_exit(
                            &data_dir,
                            parent_pid,
                            parent_started_at_ms,
                            LifecycleRecoveryTrigger::PollConfirmedExit,
                        );
                    }
                } else {
                    consecutive_parent_misses = 0;
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn run_system_proxy_cleanup_daemon(
    data_dir: std::path::PathBuf,
    installed_version: Option<String>,
) -> bifrost_core::Result<()> {
    set_data_dir(data_dir.clone());
    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        installed_version = installed_version.as_deref().unwrap_or(""),
        current_version = bifrost_core::system_proxy_launchd::CURRENT_VERSION,
        "system proxy launchd cleanup daemon started"
    );

    let startup_started_at = std::time::Instant::now();
    match bifrost_core::system_proxy_launchd::recover_if_no_live_runtime_with_startup_retry(
        &data_dir,
    ) {
        Ok(bifrost_core::SystemProxyLaunchdRecoveryOutcome::Recovered) => tracing::info!(
            target: "bifrost_cli::shutdown",
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery completed"
        ),
        Ok(bifrost_core::SystemProxyLaunchdRecoveryOutcome::Skipped) => tracing::info!(
            target: "bifrost_cli::shutdown",
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery skipped"
        ),
        Err(error) => tracing::warn!(
            target: "bifrost_cli::shutdown",
            error = %error,
            elapsed_ms = startup_started_at.elapsed().as_millis() as u64,
            "system proxy launchd cleanup daemon startup recovery failed"
        ),
    }

    tracing::info!(
        target: "bifrost_cli::shutdown",
        data_dir = %data_dir.display(),
        installed_version = installed_version.as_deref().unwrap_or(""),
        current_version = bifrost_core::system_proxy_launchd::CURRENT_VERSION,
        "system proxy launchd cleanup daemon exiting after one-shot recovery check"
    );
    Ok(())
}
