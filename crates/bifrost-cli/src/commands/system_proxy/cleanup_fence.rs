use super::*;

pub(super) struct RuntimeCleanupFence {
    pub(super) runtime: Option<RuntimeInfo>,
    pub(super) parent_pid: Option<u32>,
    pub(super) parent_started_at_ms: Option<u64>,
    pub(super) generation: Option<String>,
}

impl RuntimeCleanupFence {
    // Called by core while holding its cross-process OS proxy lock, including
    // on every retry. A generation can intentionally survive a runtime restart.
    pub(super) fn permits_cleanup(&self, data_dir: &std::path::Path) -> bifrost_core::Result<bool> {
        let marker = bifrost_core::read_system_proxy_shutdown_mode_checked(data_dir)?;
        if matches!(
            marker,
            Some(
                bifrost_core::SystemProxyShutdownMode::ForegroundCleanup
                    | bifrost_core::SystemProxyShutdownMode::PreserveForRestart
            )
        ) {
            return Ok(false);
        }
        if let Some(current) = read_runtime_info_from_checked(data_dir)? {
            if !matches!(
                inspect_process_identity(current.pid, current.started_at_ms),
                ProcessIdentityStatus::Exited | ProcessIdentityStatus::Reused
            ) {
                return Ok(false);
            }
            if !self
                .runtime
                .as_ref()
                .is_some_and(|expected| same_runtime_identity(&current, expected))
            {
                return Ok(false);
            }
        }
        let pid = self
            .parent_pid
            .or_else(|| self.runtime.as_ref().map(|runtime| runtime.pid));
        let start = self.parent_started_at_ms.or_else(|| {
            self.runtime
                .as_ref()
                .and_then(|runtime| runtime.started_at_ms)
        });
        Ok(matches!(
            parent_identity_status(pid, start),
            ProcessIdentityStatus::Exited | ProcessIdentityStatus::Reused
        ))
    }
}

pub(super) fn cleanup_owned_proxy_after_exit(
    data_dir: &std::path::Path,
    fence: &RuntimeCleanupFence,
) -> bifrost_core::Result<()> {
    let system_proxy_result = if let Some(generation) = fence.generation.as_deref() {
        let mut manager = bifrost_core::SystemProxyManager::new(data_dir.to_path_buf());
        let result = bifrost_core::retry_with_policy(
            bifrost_core::RECOVERY_RETRY_WINDOW,
            bifrost_core::RECOVERY_RETRY_INTERVAL,
            |_| {
                manager.restore_managed_if_generation_guarded(generation, || {
                    fence.permits_cleanup(data_dir)
                })
            },
        );
        manager.detach();
        result.map(|transition| {
            tracing::info!(
                ?transition,
                generation,
                "generation-fenced parent-exit proxy cleanup finished"
            );
        })
    } else {
        Ok(())
    };
    // An OS error must not leave a separately-owned dead CLI endpoint behind.
    let cli_proxy_result = if let Some(runtime) = fence.runtime.as_ref() {
        bifrost_core::CliProxyEnvironmentManager::disable_for_runtime_guarded(
            data_dir,
            runtime_system_proxy_host(runtime.host.as_deref()),
            runtime.port,
            || fence.permits_cleanup(data_dir),
        )
    } else {
        Ok(Vec::new())
    };
    combine_proxy_cleanup_results(system_proxy_result, cli_proxy_result)?;
    if fence.permits_cleanup(data_dir)? {
        bifrost_core::consume_system_proxy_shutdown_mode_if(
            data_dir,
            bifrost_core::SystemProxyShutdownMode::BackgroundCleanup,
        );
    }
    Ok(())
}
