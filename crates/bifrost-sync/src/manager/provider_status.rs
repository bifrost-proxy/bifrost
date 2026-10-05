use std::collections::HashMap;

use bifrost_storage::{SyncConfig, DEFAULT_REMOTE_BASE_URL};

use super::{
    is_github_gist_auth_error, is_github_gist_error, is_managed_remote, normalize_remote_base_url,
    provider_meta_for, session_for_provider, ProviderSyncMeta, RemoteUser,
    SyncProviderCapabilities, SyncProviderSession, SyncProviderStatus, SyncReason,
    SyncRuntimeState, GITHUB_GIST_AUTO_SYNC_MIN_INTERVAL_SECS, GITHUB_GIST_REMOTE_BASE_URL,
    SERVER_AUTO_SYNC_MIN_INTERVAL_SECS,
};

pub(super) fn build_provider_statuses(
    sync_config: &SyncConfig,
    runtime: &SyncRuntimeState,
    user: &Option<RemoteUser>,
    has_session: bool,
    provider_sessions: &HashMap<String, SyncProviderSession>,
    provider_sync: &HashMap<String, ProviderSyncMeta>,
) -> Vec<SyncProviderStatus> {
    let current_url = normalize_remote_base_url(&sync_config.remote_base_url);
    let legacy_current_connected =
        provider_sessions.is_empty() && has_session && runtime.authorized;
    let bytedance_session = session_for_provider(provider_sessions, "bytedance_internal");
    let cloud_session = session_for_provider(provider_sessions, "bifrost_cloud");
    let github_gist_session = session_for_provider(provider_sessions, "github_gist");
    // Runtime health belongs to the selected remote URL, not every saved session of its type.
    let bytedance_selected = is_managed_remote(&current_url)
        && bytedance_session.is_none_or(|session| {
            normalize_remote_base_url(&session.remote_base_url) == current_url
        });
    let cloud_selected = !is_managed_remote(&current_url)
        && !current_url.is_empty()
        && cloud_session.is_none_or(|session| {
            normalize_remote_base_url(&session.remote_base_url) == current_url
        });
    let bytedance_uses_runtime =
        bytedance_selected && (bytedance_session.is_some() || provider_sessions.is_empty());
    let cloud_uses_runtime =
        cloud_selected && (cloud_session.is_some() || provider_sessions.is_empty());
    let bytedance_meta = provider_meta_for(provider_sync, "bytedance_internal");
    let cloud_meta = provider_meta_for(provider_sync, "bifrost_cloud");
    let github_gist_meta = provider_meta_for(provider_sync, "github_gist");
    let github_gist_last_error = runtime
        .last_error
        .as_ref()
        .filter(|error| is_github_gist_error(error))
        .cloned()
        .or_else(|| github_gist_meta.and_then(|meta| meta.last_error.clone()));
    let github_gist_auth_error = github_gist_last_error
        .as_deref()
        .is_some_and(is_github_gist_auth_error);
    let bytedance_last_error = (bytedance_selected
        && !runtime
            .last_error
            .as_deref()
            .is_some_and(is_github_gist_error))
    .then(|| runtime.last_error.clone())
    .flatten()
    .or_else(|| bytedance_meta.and_then(|meta| meta.last_error.clone()));
    let cloud_last_error = (cloud_selected
        && !runtime
            .last_error
            .as_deref()
            .is_some_and(is_github_gist_error))
    .then(|| runtime.last_error.clone())
    .flatten()
    .or_else(|| cloud_meta.and_then(|meta| meta.last_error.clone()));
    let bytedance_connected =
        bytedance_session.is_some() || (bytedance_selected && legacy_current_connected);
    let cloud_connected = cloud_session.is_some() || (cloud_selected && legacy_current_connected);
    let github_gist_connected = github_gist_session.is_some();
    let github_gist_authorized = github_gist_connected && !github_gist_auth_error;

    vec![
        SyncProviderStatus {
            id: "bytedance_internal".to_string(),
            name: "ByteDance Internal".to_string(),
            description: "Internal trusted sync and Remote Invoke provider.".to_string(),
            remote_base_url: Some(DEFAULT_REMOTE_BASE_URL.to_string()),
            connected: bytedance_connected,
            enabled: sync_config.enabled && (bytedance_selected || bytedance_session.is_some()),
            reachable: if bytedance_uses_runtime {
                runtime.reachable
            } else {
                bytedance_session.is_some()
            },
            authorized: if bytedance_uses_runtime {
                runtime.authorized
            } else {
                bytedance_session.is_some()
            },
            reason: if bytedance_selected
                && runtime.reason == SyncReason::Ready
                && runtime.last_error.is_none()
                && bytedance_meta
                    .and_then(|meta| meta.last_error.as_ref())
                    .is_some()
            {
                SyncReason::Error
            } else if bytedance_selected {
                runtime.reason.clone()
            } else if bytedance_session.is_some() && bytedance_last_error.is_some() {
                SyncReason::Error
            } else if bytedance_session.is_some() {
                SyncReason::Ready
            } else {
                SyncReason::Unauthorized
            },
            last_error: bytedance_last_error,
            last_sync_at: bytedance_meta.and_then(|meta| meta.last_sync_at.clone()),
            last_sync_action: bytedance_meta.and_then(|meta| meta.last_sync_action),
            last_changed_sync_at: bytedance_meta.and_then(|meta| meta.last_changed_sync_at.clone()),
            last_changed_sync_action: bytedance_meta.and_then(|meta| meta.last_changed_sync_action),
            check_interval_secs: Some(SERVER_AUTO_SYNC_MIN_INTERVAL_SECS),
            user: bytedance_session
                .and_then(|session| session.user.clone())
                .or_else(|| {
                    (bytedance_selected && provider_sessions.is_empty() && has_session)
                        .then(|| user.clone())
                        .flatten()
                }),
            capabilities: SyncProviderCapabilities {
                rules_sync: true,
                config_sync: true,
                remote_invoke: true,
            },
            remote_invoke_registered: bytedance_connected,
        },
        SyncProviderStatus {
            id: "bifrost_cloud".to_string(),
            name: "Bifrost Cloud".to_string(),
            description: "Custom Bifrost sync service for teams and self-hosting.".to_string(),
            remote_base_url: cloud_session
                .map(|session| session.remote_base_url.clone())
                .or_else(|| cloud_selected.then(|| current_url.clone())),
            connected: cloud_connected,
            enabled: sync_config.enabled && (cloud_selected || cloud_session.is_some()),
            reachable: if cloud_uses_runtime {
                runtime.reachable
            } else {
                cloud_session.is_some()
            },
            authorized: if cloud_uses_runtime {
                runtime.authorized
            } else {
                cloud_session.is_some()
            },
            reason: if cloud_selected
                && runtime.reason == SyncReason::Ready
                && runtime.last_error.is_none()
                && cloud_meta
                    .and_then(|meta| meta.last_error.as_ref())
                    .is_some()
            {
                SyncReason::Error
            } else if cloud_selected {
                runtime.reason.clone()
            } else if cloud_session.is_some() && cloud_last_error.is_some() {
                SyncReason::Error
            } else if cloud_session.is_some() {
                SyncReason::Ready
            } else {
                SyncReason::Unauthorized
            },
            last_error: cloud_last_error,
            last_sync_at: cloud_meta.and_then(|meta| meta.last_sync_at.clone()),
            last_sync_action: cloud_meta.and_then(|meta| meta.last_sync_action),
            last_changed_sync_at: cloud_meta.and_then(|meta| meta.last_changed_sync_at.clone()),
            last_changed_sync_action: cloud_meta.and_then(|meta| meta.last_changed_sync_action),
            check_interval_secs: Some(SERVER_AUTO_SYNC_MIN_INTERVAL_SECS),
            user: cloud_session
                .and_then(|session| session.user.clone())
                .or_else(|| {
                    (cloud_selected && provider_sessions.is_empty() && has_session)
                        .then(|| user.clone())
                        .flatten()
                }),
            capabilities: SyncProviderCapabilities {
                rules_sync: true,
                config_sync: true,
                remote_invoke: true,
            },
            remote_invoke_registered: cloud_connected,
        },
        SyncProviderStatus {
            id: "github_gist".to_string(),
            name: "GitHub Gist".to_string(),
            description: "Public GitHub Gist-backed portable sync provider.".to_string(),
            remote_base_url: Some(GITHUB_GIST_REMOTE_BASE_URL.to_string()),
            connected: github_gist_connected,
            enabled: sync_config.enabled && github_gist_connected,
            reachable: github_gist_connected,
            authorized: github_gist_authorized,
            reason: if github_gist_last_error.is_some() {
                SyncReason::Error
            } else if github_gist_connected {
                SyncReason::Ready
            } else {
                SyncReason::Unauthorized
            },
            last_error: github_gist_last_error,
            last_sync_at: github_gist_meta.and_then(|meta| meta.last_sync_at.clone()),
            last_sync_action: github_gist_meta.and_then(|meta| meta.last_sync_action),
            last_changed_sync_at: github_gist_meta
                .and_then(|meta| meta.last_changed_sync_at.clone()),
            last_changed_sync_action: github_gist_meta
                .and_then(|meta| meta.last_changed_sync_action),
            check_interval_secs: Some(GITHUB_GIST_AUTO_SYNC_MIN_INTERVAL_SECS),
            user: github_gist_session.and_then(|session| session.user.clone()),
            capabilities: SyncProviderCapabilities {
                rules_sync: true,
                config_sync: true,
                remote_invoke: false,
            },
            remote_invoke_registered: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{SyncManager, SyncStatus};
    use bifrost_storage::{ConfigManager, SyncConfigUpdate};
    use std::sync::Arc;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn manager_for_remote(remote: &str) -> (TempDir, SyncManager) {
        let temp = TempDir::new().unwrap();
        let config = Arc::new(ConfigManager::new(temp.path().to_path_buf()).unwrap());
        config
            .update_sync_config(SyncConfigUpdate {
                enabled: Some(true),
                auto_sync: Some(false),
                remote_base_url: Some(remote.to_string()),
                connect_timeout_ms: Some(500),
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = SyncManager::new(config, 18080).unwrap();
        let provider_id = super::super::provider_id_for_remote(remote).unwrap();
        {
            let mut state = manager.state.lock();
            state.token = Some("test-token".to_string());
            state
                .provider_sessions
                .insert(provider_id.to_string(), session(remote));
        }
        (temp, manager)
    }

    fn session(remote: &str) -> SyncProviderSession {
        SyncProviderSession {
            token: "test-token".to_string(),
            remote_base_url: remote.to_string(),
            user: None,
        }
    }

    fn provider<'a>(status: &'a SyncStatus, id: &str) -> &'a SyncProviderStatus {
        status
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .unwrap()
    }

    async fn mock_health(server: &MockServer, probe_status: u16, user_status: u16) {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/v4/sso/check"))
            .respond_with(ResponseTemplate::new(probe_status))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v4/sso/info"))
            .respond_with(
                ResponseTemplate::new(user_status).set_body_json(serde_json::json!({
                    "code": 0, "message": "ok",
                    "data": {"user_id": "test-user", "nickname": "Test", "avatar": "", "email": ""}
                })),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn saved_current_server_session_follows_runtime_health() {
        for remote in [
            DEFAULT_REMOTE_BASE_URL.as_str(),
            "https://sync.example.test",
        ] {
            let (_temp, manager) = manager_for_remote(remote).await;
            let id = super::super::provider_id_for_remote(remote).unwrap();
            // Trailing slashes must not prevent applying health to the saved session.
            manager
                .state
                .lock()
                .provider_sessions
                .get_mut(id)
                .unwrap()
                .remote_base_url = format!("{remote}/");
            for (reachable, authorized, reason) in [
                (true, true, SyncReason::Ready),
                (false, false, SyncReason::Unreachable),
                (true, false, SyncReason::Unauthorized),
                (true, true, SyncReason::Ready),
            ] {
                *manager.runtime.write().await = SyncRuntimeState {
                    reachable,
                    authorized,
                    reason: reason.clone(),
                    ..Default::default()
                };
                let status = manager.status().await;
                let current = provider(&status, id);
                assert!(current.connected);
                assert_eq!(current.reachable, reachable, "provider {id}: {reason:?}");
                assert_eq!(current.authorized, authorized, "provider {id}: {reason:?}");
                assert_eq!(current.reason, reason);
                assert_eq!(status.reachable, reachable);
                assert_eq!(status.authorized, authorized);
                assert_eq!(status.reason, reason);
                assert!(status.has_session);
                assert!(!status.first_run_prompt_required);
                assert_eq!(manager.session_token().as_deref(), Some("test-token"));
            }
        }
    }

    #[tokio::test]
    async fn current_provider_outage_and_recovery_preserve_saved_session() {
        // A dedicated server really shuts down when dropped instead of returning to the pool.
        let server = MockServer::builder().start().await;
        let (_temp, manager) = manager_for_remote(&server.uri()).await;
        for probe_status in [200, 503, 200] {
            mock_health(&server, probe_status, 200).await;
            manager.tick().await.unwrap();
            let status = manager.status().await;
            let current = provider(&status, "bifrost_cloud");
            let healthy = probe_status == 200;
            assert_eq!(current.reachable, healthy);
            assert_eq!(current.authorized, healthy);
            assert_eq!(status.reachable, healthy);
            assert_eq!(status.authorized, healthy);
            assert_eq!(
                status.reason,
                if healthy {
                    SyncReason::Ready
                } else {
                    SyncReason::Unreachable
                }
            );
            assert!(current.connected);
            assert!(status.has_session);
            assert_eq!(manager.session_token().as_deref(), Some("test-token"));
        }
        let address = *server.address();
        drop(server);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Ok(stream) = tokio::net::TcpStream::connect(address).await {
                drop(stream);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("dedicated mock listener should close after shutdown");
        manager.tick().await.unwrap();
        let status = manager.status().await;
        assert_eq!(status.reason, SyncReason::Unreachable);
        assert!(!status.reachable);
        assert!(!status.authorized);
        assert!(provider(&status, "bifrost_cloud").connected);
        assert_eq!(manager.session_token().as_deref(), Some("test-token"));
    }

    #[tokio::test]
    async fn rejected_current_token_does_not_report_saved_session_as_authorized() {
        let server = MockServer::start().await;
        let (_temp, manager) = manager_for_remote(&server.uri()).await;
        mock_health(&server, 200, 401).await;
        manager.tick().await.unwrap();
        let status = manager.status().await;
        let current = provider(&status, "bifrost_cloud");
        assert!(
            current.connected,
            "saved session keeps reconnect actions visible"
        );
        assert!(current.reachable);
        assert!(!current.authorized);
        assert_eq!(current.reason, SyncReason::Unauthorized);
        assert_eq!(status.reason, SyncReason::Unauthorized);
        assert!(!status.authorized);
        // Preserve the existing token-expiry behavior; status must not restore authorization.
        assert!(manager.session_token().is_none());
        assert!(manager
            .state
            .lock()
            .provider_sessions
            .contains_key("bifrost_cloud"));
    }

    #[tokio::test]
    async fn another_healthy_provider_keeps_aggregate_ready_during_current_outage() {
        for (other_id, other_url) in [
            ("bytedance_internal", DEFAULT_REMOTE_BASE_URL.as_str()),
            ("github_gist", GITHUB_GIST_REMOTE_BASE_URL),
        ] {
            let (_temp, manager) = manager_for_remote("https://sync.example.test").await;
            manager
                .state
                .lock()
                .provider_sessions
                .insert(other_id.to_string(), session(other_url));
            *manager.runtime.write().await = SyncRuntimeState {
                reason: SyncReason::Unreachable,
                ..Default::default()
            };
            let status = manager.status().await;
            let current = provider(&status, "bifrost_cloud");
            assert!(!current.reachable);
            assert!(!current.authorized);
            assert_eq!(current.reason, SyncReason::Unreachable);
            let other = provider(&status, other_id);
            assert!(other.connected && other.reachable && other.authorized);
            assert_eq!(other.reason, SyncReason::Ready);
            assert!(status.reachable && status.authorized);
            assert_eq!(status.reason, SyncReason::Ready);
        }
    }

    #[tokio::test]
    async fn changed_current_url_does_not_apply_health_to_another_saved_url() {
        let saved_server = MockServer::start().await;
        let (_temp, manager) = manager_for_remote(&saved_server.uri()).await;
        mock_health(&saved_server, 200, 200).await;
        manager.tick().await.unwrap();
        let other_server = MockServer::start().await;
        mock_health(&other_server, 503, 200).await;
        manager
            .config_manager
            .update_sync_config(SyncConfigUpdate {
                remote_base_url: Some(other_server.uri()),
                ..Default::default()
            })
            .await
            .unwrap();
        manager.tick().await.unwrap();
        manager.runtime.write().await.last_error = Some("new remote unavailable".to_string());
        let status = manager.status().await;
        let saved = provider(&status, "bifrost_cloud");
        assert_eq!(
            saved.remote_base_url.as_deref(),
            Some(saved_server.uri().as_str())
        );
        assert!(saved.connected && saved.reachable && saved.authorized);
        assert_eq!(saved.reason, SyncReason::Ready);
        assert!(saved.last_error.is_none());
        assert_eq!(status.reason, SyncReason::Ready);
        assert!(status.reachable && status.authorized);
    }
}
