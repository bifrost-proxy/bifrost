use std::future::Future;

use super::*;

#[derive(Debug)]
pub(super) struct SystemProxyVerificationError {
    pub(super) message: String,
    pub(super) status: Option<SystemProxyStatus>,
}

pub(super) async fn wait_for_system_proxy_status(
    expected_enabled: bool,
    expected_host: &str,
    expected_port: u16,
) -> Result<SystemProxyStatus, SystemProxyVerificationError> {
    wait_for_system_proxy_status_with(
        expected_enabled,
        expected_host,
        expected_port,
        &SYSTEM_PROXY_VERIFY_DELAYS_MS,
        || read_system_proxy_status(expected_host, expected_port),
    )
    .await
}

async fn wait_for_system_proxy_status_with<F, Fut>(
    expected_enabled: bool,
    expected_host: &str,
    expected_port: u16,
    delays_ms: &[u64],
    mut read: F,
) -> Result<SystemProxyStatus, SystemProxyVerificationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<SystemProxyStatus, String>>,
{
    let mut latest = None;
    let mut last_error = None;
    for attempt in 0..=delays_ms.len() {
        if attempt > 0 {
            sleep(Duration::from_millis(delays_ms[attempt - 1])).await;
        }
        match read().await {
            Ok(status) => {
                if matches_expected_system_proxy(
                    &status,
                    expected_enabled,
                    expected_host,
                    expected_port,
                ) {
                    return Ok(status);
                }
                latest = Some(status);
                last_error = None;
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(SystemProxyVerificationError {
        message: last_error.unwrap_or_else(|| format!(
            "System proxy did not converge to requested enabled={expected_enabled} at {expected_host}:{expected_port}"
        )),
        status: latest,
    })
}

fn persisted_system_proxy_update(request: &SetSystemProxyRequest) -> SystemProxyConfigUpdate {
    SystemProxyConfigUpdate {
        enabled: Some(request.enabled),
        // Store the user's request, never an unrelated or suspended OS proxy.
        bypass: request.bypass.clone(),
        auto_enable: None,
        recovery_mode: request.recovery_mode,
        recovery_grace_secs: request.recovery_grace_secs,
    }
}

pub(super) async fn apply_system_proxy_intent_with<A, AF, V, VF, C>(
    state: SharedAdminState,
    request: SetSystemProxyRequest,
    apply: A,
    verify: V,
    complete: C,
) -> Response<BoxBody>
where
    A: FnOnce(SystemProxyOperation) -> AF,
    AF: Future<Output = Result<(), String>>,
    V: FnOnce(bool, u16) -> VF,
    VF: Future<Output = Result<SystemProxyStatus, SystemProxyVerificationError>>,
    C: FnOnce(&SharedAdminState, &SystemProxyStatus),
{
    // Persistence, desired publication, OS mutation and readback are one ordered
    // request. Older completions cannot overwrite a newer accepted user toggle.
    let _intent_guard = state.system_proxy_intent_lock.lock().await;
    if state.system_proxy_manager.is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "System proxy manager not initialized",
        );
    }
    let Some(config_manager) = &state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };
    let before = config_manager.config().await.system_proxy;
    let config = match config_manager
        .update_system_proxy_config_with_snapshot(persisted_system_proxy_update(&request))
        .await
    {
        Ok(config) => config,
        Err(error) => {
            return system_proxy_operation_error_response(
                &format!("Failed to persist system proxy intent: {error}"),
                Some(&before),
                None,
            );
        }
    };
    let result = apply(SystemProxyOperation {
        state: state.clone(),
        enabled: request.enabled,
        bypass: config.bypass.clone(),
        intent_revision: config.intent_revision,
    })
    .await;
    let verification_port = state.port();
    let verification = verify(request.enabled, verification_port).await;
    let config = current_system_proxy_config(&state).await;
    let (mut status, verification_error) = match verification {
        Ok(status) => (Some(status), None),
        Err(error) => (error.status, Some(error.message)),
    };
    if let Some(status) = status.as_mut() {
        status.apply_config(&config);
        state.store_system_proxy_runtime_managed(status.enabled && status.managed_by_bifrost);
    }
    // Accepted intent survives transient OS errors and verification mismatch.
    if let Err(error) = result {
        return system_proxy_operation_error_response(&error, Some(&config), status.as_ref());
    }
    if let Some(error) = verification_error {
        return system_proxy_operation_error_response(
            &format!("Failed to verify system proxy: {error}"),
            Some(&config),
            status.as_ref(),
        );
    }
    if state.port() != verification_port {
        return system_proxy_operation_error_response(
            "System proxy target changed during verification; refresh status",
            Some(&config),
            status.as_ref(),
        );
    }
    let status = status.expect("successful verification always returns status");
    complete(&state, &status);
    json_response(&status)
}

/// The desired flag and OS mutation share the reconciler's manager lock. Always
/// re-read disk after taking it: a request may have waited behind a newer intent.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn apply_system_proxy_operation_locked(
    operation: SystemProxyOperation,
    mutate: impl FnOnce(&mut SystemProxyManager, u16) -> Result<(), String>,
) -> Result<(), String> {
    let state = operation.state.clone();
    let revision = operation.intent_revision;
    apply_system_proxy_operation_locked_with(operation, mutate, |manager, target_port| {
        // Both ownership and latest-off intent must still match inside the OS
        // writer lock. A newer enable can arrive while compensation is queued.
        let ownership = manager
            .read_managed_ownership()
            .map_err(|error| error.to_string())?;
        if let Some(ownership) = ownership.filter(|ownership| {
            !ownership.generation.is_empty()
                && ownership.target.target_matches("127.0.0.1", target_port)
        }) {
            manager
                .restore_managed_if_generation_guarded(&ownership.generation, || {
                    newer_system_proxy_disable_is_current(&state, revision, target_port)
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn apply_system_proxy_operation_locked_with(
    operation: SystemProxyOperation,
    mutate: impl FnOnce(&mut SystemProxyManager, u16) -> Result<(), String>,
    compensate_disable: impl FnOnce(&mut SystemProxyManager, u16) -> Result<(), String>,
) -> Result<(), String> {
    let manager = operation
        .state
        .system_proxy_manager
        .as_ref()
        .ok_or("System proxy manager not initialized")?;
    let config_manager = operation
        .state
        .config_manager
        .as_ref()
        .ok_or("Config manager not available")?;
    let mut manager = manager.blocking_write();
    let latest = bifrost_storage::read_persisted_system_proxy_config(config_manager.data_dir())
        .map_err(|error| format!("Failed to read system proxy intent: {error}"))?;
    operation
        .state
        .store_system_proxy_runtime_desired_enabled(latest.enabled);
    if latest.intent_revision != operation.intent_revision {
        return Err("System proxy request was superseded by newer persisted intent".to_string());
    }
    let target_port = operation.state.port();
    let result = mutate(&mut manager, target_port);
    let after = bifrost_storage::read_persisted_system_proxy_config(config_manager.data_dir())
        .map_err(|error| format!("Failed to recheck system proxy intent: {error}"))?;
    if after.intent_revision != operation.intent_revision {
        operation
            .state
            .store_system_proxy_runtime_desired_enabled(after.enabled);
        if operation.enabled && !after.enabled {
            compensate_disable(&mut manager, target_port)?;
        }
        return Err("System proxy request was superseded while applying OS settings".to_string());
    }
    result
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn accepted_system_proxy_intent_is_current(
    state: &SharedAdminState,
    revision: u64,
    enabled: bool,
    target_port: u16,
) -> bifrost_core::Result<bool> {
    let config = persisted_system_proxy_intent(state)?;
    Ok(config.intent_revision == revision
        && config.enabled == enabled
        && state.port() == target_port)
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn newer_system_proxy_disable_is_current(
    state: &SharedAdminState,
    previous_revision: u64,
    target_port: u16,
) -> bifrost_core::Result<bool> {
    let config = persisted_system_proxy_intent(state)?;
    Ok(
        config.intent_revision > previous_revision
            && !config.enabled
            && state.port() == target_port,
    )
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn persisted_system_proxy_intent(
    state: &SharedAdminState,
) -> bifrost_core::Result<SystemProxyConfig> {
    let config_manager = state.config_manager.as_ref().ok_or_else(|| {
        bifrost_core::BifrostError::Config("Config manager not available".to_string())
    })?;
    bifrost_storage::read_persisted_system_proxy_config(config_manager.data_dir())
}

pub(super) fn system_proxy_operation_error_response(
    message: &str,
    config: Option<&SystemProxyConfig>,
    status: Option<&SystemProxyStatus>,
) -> Response<BoxBody> {
    let (code, error, message) = if message.contains("UserCancelled") {
        (
            StatusCode::FORBIDDEN,
            "user_cancelled".to_string(),
            "Authorization was cancelled by user.".to_string(),
        )
    } else if message.contains("RequiresAdmin") {
        (StatusCode::FORBIDDEN, "requires_admin".to_string(), "System proxy requires administrator privileges. Please run the CLI with sudo or grant permission.".to_string())
    } else {
        let message = format!("Failed to set system proxy: {message}");
        (StatusCode::INTERNAL_SERVER_ERROR, message.clone(), message)
    };
    json_response_with_status(
        code,
        &serde_json::json!({
            "error": error,
            "message": message,
            "configured_enabled": config.map(|config| config.enabled),
            "effective_status": status,
        }),
    )
}

#[cfg(test)]
mod tests;
