use super::RuntimeInfo;
use bifrost_storage::NewSystemProxyConfig;

/// Runtime overrides apply only until the next explicit persisted user toggle.
/// Legacy metadata has no trustworthy freshness marker, so use saved intent.
/// OS state is deliberately excluded: temporary suspension is not a disable.
pub fn resolve_runtime_system_proxy_intent(
    runtime: Option<&RuntimeInfo>,
    configured: &NewSystemProxyConfig,
) -> (bool, String) {
    if let Some(runtime) = runtime
        .filter(|runtime| runtime.system_proxy_config_revision == Some(configured.intent_revision))
    {
        if let Some(enabled) = runtime.system_proxy_enabled {
            return (
                enabled,
                runtime
                    .system_proxy_bypass
                    .clone()
                    .unwrap_or_else(|| configured.bypass.clone()),
            );
        }
    }
    (configured.enabled, configured.bypass.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RuntimeStartMode;

    fn runtime(enabled: bool, revision: u64) -> RuntimeInfo {
        RuntimeInfo::new(123, 18891, None, None, RuntimeStartMode::Daemon)
            .with_system_proxy(enabled, "session.example")
            .with_system_proxy_config_revision(revision)
    }

    #[test]
    fn unchanged_config_preserves_explicit_session_enable_and_disable() {
        for enabled in [false, true] {
            let config = NewSystemProxyConfig {
                enabled: !enabled,
                intent_revision: 7,
                ..Default::default()
            };
            assert_eq!(
                resolve_runtime_system_proxy_intent(Some(&runtime(enabled, 7)), &config),
                (enabled, "session.example".to_string())
            );
        }
    }

    #[test]
    fn newer_user_intent_wins_over_stale_runtime_in_both_directions() {
        for enabled in [false, true] {
            let config = NewSystemProxyConfig {
                enabled,
                intent_revision: 8,
                bypass: "latest.example".to_string(),
                ..Default::default()
            };
            assert_eq!(
                resolve_runtime_system_proxy_intent(Some(&runtime(!enabled, 7)), &config),
                (enabled, "latest.example".to_string())
            );
        }
    }

    #[test]
    fn legacy_or_missing_runtime_uses_saved_intent() {
        for enabled in [false, true] {
            let config = NewSystemProxyConfig {
                enabled,
                ..Default::default()
            };
            let mut legacy = runtime(!enabled, 0);
            legacy.system_proxy_config_revision = None;
            for runtime in [None, Some(&legacy)] {
                assert_eq!(
                    resolve_runtime_system_proxy_intent(runtime, &config),
                    (enabled, config.bypass.clone())
                );
            }
        }
    }

    #[test]
    fn matching_revision_without_runtime_desire_falls_back_to_config() {
        let config = NewSystemProxyConfig::default();
        let mut metadata = runtime(false, 0);
        metadata.system_proxy_enabled = None;
        assert_eq!(
            resolve_runtime_system_proxy_intent(Some(&metadata), &config),
            (true, config.bypass.clone())
        );
        metadata.system_proxy_enabled = Some(false);
        metadata.system_proxy_bypass = None;
        assert_eq!(
            resolve_runtime_system_proxy_intent(Some(&metadata), &config),
            (false, config.bypass.clone())
        );
    }
}
