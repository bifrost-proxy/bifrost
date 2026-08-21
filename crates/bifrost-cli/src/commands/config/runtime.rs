use crate::commands::config::client::ConfigApiClient;
use crate::process::{
    discover_bifrost_runtime, is_process_running, read_runtime_info, RuntimeInfo,
};
use bifrost_core::{BifrostError, Result, StartTimeMatch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeApiState {
    Offline,
    Live(u16),
}

fn classify_runtime_api_state(
    recorded: Option<&RuntimeInfo>,
    process_running: bool,
    start_time_match: StartTimeMatch,
    discovered: Option<&RuntimeInfo>,
) -> Result<RuntimeApiState> {
    let Some(recorded) = recorded else {
        return Ok(RuntimeApiState::Offline);
    };

    if !process_running || matches!(start_time_match, StartTimeMatch::Mismatch { .. }) {
        return Ok(RuntimeApiState::Offline);
    }

    let Some(discovered) = discovered else {
        return Err(BifrostError::Config(format!(
            "Bifrost runtime PID {} is active, but its Admin API on port {} could not be verified; refusing direct file writes",
            recorded.pid, recorded.port
        )));
    };

    if discovered.pid != recorded.pid || discovered.port != recorded.port {
        return Err(BifrostError::Config(format!(
            "Bifrost runtime identity mismatch (recorded PID {} on port {}, discovered PID {} on port {}); refusing direct file writes",
            recorded.pid, recorded.port, discovered.pid, discovered.port
        )));
    }

    Ok(RuntimeApiState::Live(recorded.port))
}

/// Resolve the Admin API belonging to the current `BIFROST_DATA_DIR`.
///
/// A missing or stale runtime record is treated as offline. Once the recorded
/// process identity is confirmed alive, however, API verification becomes
/// mandatory so callers never silently create daemon/file split-brain state.
pub(crate) fn live_config_api_client() -> Result<Option<ConfigApiClient>> {
    live_config_api_client_with(
        read_runtime_info(),
        is_process_running,
        bifrost_core::get_process_start_time_ms,
        discover_bifrost_runtime,
    )
}

fn live_config_api_client_with(
    recorded: Option<RuntimeInfo>,
    process_is_running: impl FnOnce(u32) -> bool,
    process_start_time_ms: impl FnOnce(u32) -> Option<u64>,
    discover_runtime: impl FnOnce(u16) -> Option<RuntimeInfo>,
) -> Result<Option<ConfigApiClient>> {
    let Some(runtime) = recorded.as_ref() else {
        return Ok(None);
    };

    let process_running = process_is_running(runtime.pid);
    let observed_started_at_ms = if process_running {
        process_start_time_ms(runtime.pid)
    } else {
        None
    };
    let start_time_match =
        bifrost_core::start_times_match(runtime.started_at_ms, observed_started_at_ms);
    let discovered =
        if process_running && !matches!(start_time_match, StartTimeMatch::Mismatch { .. }) {
            discover_runtime(runtime.port)
        } else {
            None
        };

    match classify_runtime_api_state(
        recorded.as_ref(),
        process_running,
        start_time_match,
        discovered.as_ref(),
    )? {
        RuntimeApiState::Offline => Ok(None),
        RuntimeApiState::Live(port) => Ok(Some(ConfigApiClient::new("127.0.0.1", port))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RuntimeStartMode;

    fn runtime(pid: u32, port: u16) -> RuntimeInfo {
        RuntimeInfo {
            pid,
            port,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: Some(1_000),
            start_mode: RuntimeStartMode::Daemon,
            restartable_runtime: true,
            binary_path: None,
            system_proxy_enabled: None,
            system_proxy_bypass: None,
            health_port: None,
        }
    }

    #[test]
    fn no_runtime_record_is_offline() {
        assert_eq!(
            classify_runtime_api_state(None, false, StartTimeMatch::Unknown, None).unwrap(),
            RuntimeApiState::Offline
        );
    }

    #[test]
    fn stale_or_reused_runtime_is_offline() {
        let recorded = runtime(10, 9900);
        assert_eq!(
            classify_runtime_api_state(Some(&recorded), false, StartTimeMatch::Unknown, None)
                .unwrap(),
            RuntimeApiState::Offline
        );
        assert_eq!(
            classify_runtime_api_state(
                Some(&recorded),
                true,
                StartTimeMatch::Mismatch {
                    recorded: 1_000,
                    observed: 9_000,
                },
                None,
            )
            .unwrap(),
            RuntimeApiState::Offline
        );
    }

    #[test]
    fn matching_runtime_uses_api() {
        let recorded = runtime(10, 18883);
        let discovered = runtime(10, 18883);
        assert_eq!(
            classify_runtime_api_state(
                Some(&recorded),
                true,
                StartTimeMatch::Match,
                Some(&discovered),
            )
            .unwrap(),
            RuntimeApiState::Live(18883)
        );
    }

    #[test]
    fn active_runtime_without_verified_api_fails_closed() {
        let recorded = runtime(10, 9900);
        let error =
            classify_runtime_api_state(Some(&recorded), true, StartTimeMatch::Unknown, None)
                .unwrap_err();
        assert!(error.to_string().contains("refusing direct file writes"));
    }

    #[test]
    fn mismatched_admin_runtime_fails_closed() {
        let recorded = runtime(10, 9900);
        let discovered = runtime(11, 9900);
        let error = classify_runtime_api_state(
            Some(&recorded),
            true,
            StartTimeMatch::Match,
            Some(&discovered),
        )
        .unwrap_err();
        assert!(error.to_string().contains("identity mismatch"));
    }

    #[test]
    fn live_client_probe_handles_missing_stale_and_matching_runtime() {
        assert!(
            live_config_api_client_with(None, |_| true, |_| Some(1_000), |_| None)
                .unwrap()
                .is_none()
        );

        let stale = runtime(10, 9900);
        assert!(
            live_config_api_client_with(Some(stale), |_| false, |_| Some(1_000), |_| None)
                .unwrap()
                .is_none()
        );

        let matching = runtime(10, 18883);
        assert!(live_config_api_client_with(
            Some(matching.clone()),
            |pid| pid == 10,
            |_| Some(1_000),
            |_| Some(matching),
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn live_client_probe_fails_closed_when_active_runtime_cannot_be_verified() {
        let recorded = runtime(10, 9900);
        let error = match live_config_api_client_with(
            Some(recorded),
            |_| true,
            |_| Some(1_000),
            |_| None,
        ) {
            Ok(_) => panic!("active but unverifiable runtime must fail closed"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("refusing direct file writes"));
    }
}
