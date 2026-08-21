use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::oneshot;
use tracing::info;

use bifrost_core::Result;

use crate::im_gateway::feishu::{self, FeishuProvider};
use crate::im_gateway::provider::{EventSink, ImProvider};
use crate::im_gateway::types::{
    ConnectionHandle, ConnectionState, ConnectionStatus, ImProviderConfig, ImProviderType,
};
use crate::im_gateway::weixin::{WeixinConnectionStatusEvent, WeixinProvider};

// ---------------------------------------------------------------------------
// Managed Connection
// ---------------------------------------------------------------------------

struct ManagedConnection {
    #[allow(dead_code)]
    provider_id: String,
    handle: ConnectionHandle,
    status: ConnectionStatus,
    generation: u64,
    transport_fingerprint: Option<ProviderTransportFingerprint>,
}

/// Only fields consumed by the provider transport belong here. Display names,
/// owner/Agent settings and timestamps are deliberately excluded: changing
/// those values must not churn an otherwise healthy socket.
#[derive(Clone, PartialEq, Eq)]
struct ProviderTransportFingerprint {
    provider_type: ImProviderType,
    base_url: Option<String>,
    app_id: Option<String>,
    secret_ref: Option<String>,
    event_connection_enabled: bool,
    event_types: Vec<String>,
}

impl ProviderTransportFingerprint {
    fn from_config(config: &ImProviderConfig) -> Self {
        let mut event_types = config
            .event_types
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        event_types.sort();
        event_types.dedup();
        Self {
            provider_type: config.provider_type,
            base_url: normalized_optional_transport_value(config.base_url.as_deref(), true),
            app_id: normalized_optional_transport_value(config.app_id.as_deref(), false),
            secret_ref: normalized_optional_transport_value(config.secret_ref.as_deref(), false),
            event_connection_enabled: config.event_connection_enabled,
            event_types,
        }
    }
}

fn normalized_optional_transport_value(value: Option<&str>, trim_slash: bool) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if trim_slash {
                value.trim_end_matches('/').to_string()
            } else {
                value.to_string()
            }
        })
}

// ---------------------------------------------------------------------------
// Connection Manager
// ---------------------------------------------------------------------------

/// Manages the lifecycle of all IM provider long connections.
///
/// Responsibilities:
/// - Start/stop connections per provider
/// - Track connection status
/// - Provide status queries for WebUI and CLI
pub struct ImConnectionManager {
    connections: Arc<RwLock<HashMap<String, ManagedConnection>>>,
    feishu_provider: Arc<FeishuProvider>,
    weixin_provider: Arc<WeixinProvider>,
    next_generation: AtomicU64,
}

impl Default for ImConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ImConnectionManager {
    pub fn new() -> Self {
        Self::new_with_data_dir(&bifrost_storage::data_dir())
    }

    pub fn new_with_data_dir(data_dir: &Path) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            feishu_provider: Arc::new(FeishuProvider::new()),
            weixin_provider: Arc::new(WeixinProvider::new_with_data_dir(data_dir)),
            next_generation: AtomicU64::new(0),
        }
    }

    /// Get a reference to the Feishu provider for sending messages.
    pub fn feishu_provider(&self) -> &Arc<FeishuProvider> {
        &self.feishu_provider
    }

    /// Get a reference to the Weixin provider for sending messages and polling.
    pub fn weixin_provider(&self) -> &Arc<WeixinProvider> {
        &self.weixin_provider
    }

    /// Start a long connection for a provider.
    ///
    /// If a connection already exists for this provider, it will be stopped
    /// *only after* the new connection's prerequisites (e.g. tenant token
    /// fetch) succeed. This prevents a failed reconnect from leaving the
    /// provider with **no** working connection at all when the old one was
    /// perfectly healthy.
    pub async fn start_connection(
        &self,
        config: &ImProviderConfig,
        app_secret: &str,
        sink: EventSink,
    ) -> Result<()> {
        match config.provider_type {
            ImProviderType::Feishu => self.start_feishu_connection(config, app_secret, sink).await,
            ImProviderType::Weixin => self.start_weixin_connection(config, sink).await,
            _ => Err(bifrost_core::BifrostError::Config(format!(
                "long connection not supported for provider type {:?}",
                config.provider_type
            ))),
        }
    }

    async fn start_weixin_connection(
        &self,
        config: &ImProviderConfig,
        sink: EventSink,
    ) -> Result<()> {
        let provider_id = config.id.clone();
        self.weixin_provider.validate_config(config).await?;
        self.stop_connection_and_wait(&provider_id).await;
        let generation = self.reserve_generation();

        let status = ConnectionStatus {
            state: ConnectionState::Connecting,
            last_connected_at: None,
            last_event_at: None,
            reconnect_count: 0,
            last_error: None,
        };
        let (status_tx, mut status_rx) =
            tokio::sync::mpsc::unbounded_channel::<WeixinConnectionStatusEvent>();
        let handle = self
            .weixin_provider
            .connect_events_with_status(config, sink, Some(status_tx))
            .await?;
        {
            let mut conns = self.connections.write();
            conns.insert(
                provider_id.clone(),
                ManagedConnection {
                    provider_id: provider_id.clone(),
                    handle,
                    status,
                    generation,
                    transport_fingerprint: Some(ProviderTransportFingerprint::from_config(config)),
                },
            );
        }
        update_connection_state_if_generation(
            &self.connections,
            &provider_id,
            generation,
            ConnectionState::Connected,
            None,
        );
        let status_connections = self.connections_arc();
        let status_provider_id = provider_id.clone();
        tokio::spawn(async move {
            while let Some(event) = status_rx.recv().await {
                update_connection_state_if_generation(
                    &status_connections,
                    &status_provider_id,
                    generation,
                    event.state,
                    event.error,
                );
            }
        });
        info!(provider_id = %provider_id, "weixin poll connection started");
        Ok(())
    }

    /// Start a Feishu long connection.
    async fn start_feishu_connection(
        &self,
        config: &ImProviderConfig,
        app_secret: &str,
        sink: EventSink,
    ) -> Result<()> {
        let provider_id = config.id.clone();

        // Pre-fetch token to ensure credentials are valid BEFORE we tear
        // down any existing connection. If this fails we keep the old
        // connection running so the provider stays reachable.
        self.feishu_provider
            .get_tenant_token(config, app_secret)
            .await?;

        // Credentials verified — now it's safe to replace any prior
        // connection for this provider.
        self.stop_connection_and_wait(&provider_id).await;
        let generation = self.reserve_generation();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stopped_tx, stopped_rx) = oneshot::channel();

        // Update status to connecting
        let status = ConnectionStatus {
            state: ConnectionState::Connecting,
            last_connected_at: None,
            last_event_at: None,
            reconnect_count: 0,
            last_error: None,
        };

        let handle = ConnectionHandle {
            shutdown_tx,
            stopped_rx: Some(stopped_rx),
        };

        {
            let mut conns = self.connections.write();
            conns.insert(
                provider_id.clone(),
                ManagedConnection {
                    provider_id: provider_id.clone(),
                    handle,
                    status,
                    generation,
                    transport_fingerprint: Some(ProviderTransportFingerprint::from_config(config)),
                },
            );
        }

        // Spawn the long connection task
        let config_clone = config.clone();
        let secret_clone = app_secret.to_string();
        let http = feishu::build_feishu_http_client();
        let (status_tx, mut status_rx) =
            tokio::sync::mpsc::unbounded_channel::<feishu::FeishuConnectionStatusEvent>();

        let connections = self.connections_arc();
        let pid = provider_id.clone();
        let status_connections = Arc::clone(&connections);
        let status_pid = provider_id.clone();

        tokio::spawn(async move {
            while let Some(event) = status_rx.recv().await {
                update_connection_state_if_generation(
                    &status_connections,
                    &status_pid,
                    generation,
                    event.state,
                    event.error,
                );
            }
        });

        tokio::spawn(async move {
            feishu::start_long_connection(
                config_clone,
                secret_clone,
                sink,
                shutdown_rx,
                http,
                Some(status_tx),
            )
            .await;

            // Connection ended - update status
            update_connection_state_if_generation(
                &connections,
                &pid,
                generation,
                ConnectionState::Disconnected,
                Some("connection task ended".to_string()),
            );
            let _ = stopped_tx.send(());
        });

        info!(provider_id = %provider_id, "feishu long connection started");

        Ok(())
    }

    /// Stop a provider's long connection.
    pub fn stop_connection(&self, provider_id: &str) {
        let mut conns = self.connections.write();
        if let Some(conn) = conns.remove(provider_id) {
            // Sending on shutdown_tx will signal the connection task to stop
            let _ = conn.handle.shutdown_tx.send(());
            info!(provider_id = provider_id, "connection stop signal sent");
        }
    }

    /// Stop a provider connection and wait until its transport task has
    /// released every event-sink clone. Provider deletion uses this before
    /// tearing down the corresponding event pipeline so the same provider ID
    /// cannot be rebound while the old account can still publish events.
    pub async fn stop_connection_and_wait(&self, provider_id: &str) {
        let connection = self.connections.write().remove(provider_id);
        let Some(connection) = connection else {
            return;
        };
        let ConnectionHandle {
            shutdown_tx,
            stopped_rx,
        } = connection.handle;
        let _ = shutdown_tx.send(());
        if let Some(stopped_rx) = stopped_rx {
            let _ = stopped_rx.await;
        }
        info!(provider_id = provider_id, "connection stopped");
    }

    /// Get connection status for a specific provider.
    pub fn get_status(&self, provider_id: &str) -> Option<ConnectionStatus> {
        let conns = self.connections.read();
        conns.get(provider_id).map(|c| c.status.clone())
    }

    /// IDs with transport state, including failed/disconnected entries that
    /// should be reconsidered during a configuration hot reload.
    pub fn provider_ids(&self) -> Vec<String> {
        self.connections.read().keys().cloned().collect()
    }

    /// Returns true when the running transport was created from the same
    /// transport-relevant configuration. Non-transport edits intentionally
    /// return true so they can be picked up in-process without reconnecting.
    pub fn transport_matches(&self, config: &ImProviderConfig) -> bool {
        self.connections
            .read()
            .get(&config.id)
            .and_then(|connection| connection.transport_fingerprint.as_ref())
            .is_some_and(|fingerprint| {
                fingerprint == &ProviderTransportFingerprint::from_config(config)
            })
    }

    #[cfg(test)]
    pub(crate) fn set_status_for_test(&self, provider_id: &str, status: ConnectionStatus) {
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        self.connections.write().insert(
            provider_id.to_string(),
            ManagedConnection {
                provider_id: provider_id.to_string(),
                handle: ConnectionHandle {
                    shutdown_tx,
                    stopped_rx: None,
                },
                status,
                generation: self.reserve_generation(),
                transport_fingerprint: None,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn set_transport_config_for_test(&self, config: &ImProviderConfig) {
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        self.connections.write().insert(
            config.id.clone(),
            ManagedConnection {
                provider_id: config.id.clone(),
                handle: ConnectionHandle {
                    shutdown_tx,
                    stopped_rx: None,
                },
                status: ConnectionStatus::default(),
                generation: self.reserve_generation(),
                transport_fingerprint: Some(ProviderTransportFingerprint::from_config(config)),
            },
        );
    }

    /// Get all connection statuses.
    pub fn list_statuses(&self) -> Vec<(String, ConnectionStatus)> {
        let conns = self.connections.read();
        conns
            .iter()
            .map(|(id, c)| (id.clone(), c.status.clone()))
            .collect()
    }

    /// Stop all connections.
    pub fn stop_all(&self) {
        let mut conns = self.connections.write();
        let provider_ids: Vec<String> = conns.keys().cloned().collect();
        for id in &provider_ids {
            if let Some(conn) = conns.remove(id) {
                let _ = conn.handle.shutdown_tx.send(());
            }
        }
        info!(count = provider_ids.len(), "all im connections stopped");
    }

    /// Update connection status (used by connection tasks via shared state).
    pub fn update_status(&self, provider_id: &str, state: ConnectionState, error: Option<String>) {
        let mut conns = self.connections.write();
        if let Some(conn) = conns.get_mut(provider_id) {
            conn.status.state = state;
            if state == ConnectionState::Connected {
                conn.status.last_connected_at = Some(current_timestamp_ms());
                conn.status.last_error = None;
            } else if let Some(err) = error {
                conn.status.last_error = Some(err);
            }
            if state == ConnectionState::Reconnecting {
                conn.status.reconnect_count += 1;
            }
        }
    }

    pub fn mark_failed(&self, provider_id: &str, error: String) {
        let mut conns = self.connections.write();
        let status = ConnectionStatus {
            state: ConnectionState::Failed,
            last_connected_at: None,
            last_event_at: None,
            reconnect_count: 0,
            last_error: Some(error),
        };
        conns.insert(
            provider_id.to_string(),
            ManagedConnection {
                provider_id: provider_id.to_string(),
                handle: ConnectionHandle {
                    shutdown_tx: oneshot::channel().0,
                    stopped_rx: None,
                },
                status,
                generation: self.reserve_generation(),
                transport_fingerprint: None,
            },
        );
    }

    /// Get an Arc-like handle to connections for use in spawned tasks.
    ///
    /// This returns a raw pointer wrapper that allows status updates from async tasks.
    /// In practice, the ImConnectionManager should be wrapped in Arc at the service level.
    fn connections_arc(&self) -> Arc<RwLock<HashMap<String, ManagedConnection>>> {
        Arc::clone(&self.connections)
    }

    fn reserve_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed) + 1
    }
}

fn update_connection_state_if_generation(
    connections: &Arc<RwLock<HashMap<String, ManagedConnection>>>,
    provider_id: &str,
    generation: u64,
    state: ConnectionState,
    error: Option<String>,
) {
    let mut conns = connections.write();
    let Some(conn) = conns.get_mut(provider_id) else {
        return;
    };
    if conn.generation != generation {
        return;
    }
    conn.status.state = state;
    if state == ConnectionState::Connected {
        conn.status.last_connected_at = Some(current_timestamp_ms());
        conn.status.last_error = None;
    } else if let Some(err) = error {
        conn.status.last_error = Some(err);
    }
    if state == ConnectionState::Reconnecting {
        conn.status.reconnect_count += 1;
    }
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_manager_new() {
        let mgr = ImConnectionManager::new();
        assert!(mgr.list_statuses().is_empty());
    }

    #[test]
    fn transport_fingerprint_ignores_runtime_metadata_but_detects_socket_inputs() {
        let manager = ImConnectionManager::new();
        let config = ImProviderConfig {
            id: "feishu-hot-reload".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Original".to_string(),
            enabled: true,
            base_url: Some("https://open.feishu.cn/open-apis/".to_string()),
            app_id: Some("app-id".to_string()),
            secret_ref: Some("secret".to_string()),
            owner_open_id: Some("owner-a".to_string()),
            event_connection_enabled: true,
            event_types: vec!["event-b".to_string(), "event-a".to_string()],
            agent_config: None,
            created_at: 1,
            updated_at: 2,
        };
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        manager.connections.write().insert(
            config.id.clone(),
            ManagedConnection {
                provider_id: config.id.clone(),
                handle: ConnectionHandle {
                    shutdown_tx,
                    stopped_rx: None,
                },
                status: ConnectionStatus::default(),
                generation: 1,
                transport_fingerprint: Some(ProviderTransportFingerprint::from_config(&config)),
            },
        );

        let mut metadata_edit = config.clone();
        metadata_edit.display_name = "Renamed".to_string();
        metadata_edit.owner_open_id = Some("owner-b".to_string());
        metadata_edit.updated_at = 99;
        metadata_edit.event_types.reverse();
        assert!(manager.transport_matches(&metadata_edit));
        assert_eq!(manager.provider_ids(), vec![config.id.clone()]);

        let mut credential_edit = metadata_edit.clone();
        credential_edit.app_id = Some("replacement-app".to_string());
        assert!(!manager.transport_matches(&credential_edit));

        let mut subscription_edit = metadata_edit;
        subscription_edit.event_types.push("event-c".to_string());
        assert!(!manager.transport_matches(&subscription_edit));
    }

    #[test]
    fn test_get_status_nonexistent_returns_none() {
        let mgr = ImConnectionManager::new();
        assert!(mgr.get_status("nonexistent").is_none());
    }

    #[test]
    fn test_stop_connection_nonexistent_is_noop() {
        let mgr = ImConnectionManager::new();
        mgr.stop_connection("nonexistent"); // Should not panic
    }

    #[tokio::test]
    async fn stop_connection_and_wait_observes_transport_shutdown() {
        let manager = ImConnectionManager::new();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stopped_tx, stopped_rx) = oneshot::channel();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_stopped = Arc::clone(&stopped);
        tokio::spawn(async move {
            let _ = shutdown_rx.await;
            task_stopped.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = stopped_tx.send(());
        });
        manager.connections.write().insert(
            "provider-delete".to_string(),
            ManagedConnection {
                provider_id: "provider-delete".to_string(),
                handle: ConnectionHandle {
                    shutdown_tx,
                    stopped_rx: Some(stopped_rx),
                },
                status: ConnectionStatus::default(),
                generation: 1,
                transport_fingerprint: None,
            },
        );

        manager.stop_connection_and_wait("provider-delete").await;

        assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(manager.get_status("provider-delete").is_none());
    }

    #[test]
    fn test_stop_all_empty_is_noop() {
        let mgr = ImConnectionManager::new();
        mgr.stop_all(); // Should not panic
    }

    #[test]
    fn test_update_status_nonexistent_is_noop() {
        let mgr = ImConnectionManager::new();
        mgr.update_status("nonexistent", ConnectionState::Connected, None);
        // Should not panic, status remains None
        assert!(mgr.get_status("nonexistent").is_none());
    }

    #[test]
    fn test_connection_state_reconnect_error_clears_after_connected() {
        let mgr = ImConnectionManager::new();
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        mgr.connections.write().insert(
            "feishu-main".to_string(),
            ManagedConnection {
                provider_id: "feishu-main".to_string(),
                handle: ConnectionHandle {
                    shutdown_tx,
                    stopped_rx: None,
                },
                status: ConnectionStatus::default(),
                generation: 1,
                transport_fingerprint: None,
            },
        );

        mgr.update_status(
            "feishu-main",
            ConnectionState::Reconnecting,
            Some("ws endpoint fetch failed".to_string()),
        );
        let reconnecting = mgr.get_status("feishu-main").unwrap();
        assert_eq!(reconnecting.state, ConnectionState::Reconnecting);
        assert_eq!(reconnecting.reconnect_count, 1);
        assert_eq!(
            reconnecting.last_error.as_deref(),
            Some("ws endpoint fetch failed")
        );

        mgr.update_status("feishu-main", ConnectionState::Connected, None);
        let connected = mgr.get_status("feishu-main").unwrap();
        assert_eq!(connected.state, ConnectionState::Connected);
        assert!(connected.last_connected_at.is_some());
        assert!(connected.last_error.is_none());
    }

    #[test]
    fn generation_guard_ignores_missing_and_stale_connections_then_updates_current_one() {
        let connections = Arc::new(RwLock::new(HashMap::new()));
        update_connection_state_if_generation(
            &connections,
            "missing",
            1,
            ConnectionState::Reconnecting,
            Some("missing".to_string()),
        );

        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        connections.write().insert(
            "weixin-main".to_string(),
            ManagedConnection {
                provider_id: "weixin-main".to_string(),
                handle: ConnectionHandle {
                    shutdown_tx,
                    stopped_rx: None,
                },
                status: ConnectionStatus::default(),
                generation: 7,
                transport_fingerprint: None,
            },
        );
        update_connection_state_if_generation(
            &connections,
            "weixin-main",
            6,
            ConnectionState::Reconnecting,
            Some("stale".to_string()),
        );
        assert_eq!(
            connections.read()["weixin-main"].status.state,
            ConnectionState::Disconnected
        );

        update_connection_state_if_generation(
            &connections,
            "weixin-main",
            7,
            ConnectionState::Reconnecting,
            Some("retry".to_string()),
        );
        let status = connections.read()["weixin-main"].status.clone();
        assert_eq!(status.state, ConnectionState::Reconnecting);
        assert_eq!(status.reconnect_count, 1);
        assert_eq!(status.last_error.as_deref(), Some("retry"));
    }

    #[test]
    fn mark_failed_inserts_a_fresh_failed_generation() {
        let manager = ImConnectionManager::new();

        manager.mark_failed("weixin-main", "poll failed".to_string());

        let status = manager.get_status("weixin-main").unwrap();
        assert_eq!(status.state, ConnectionState::Failed);
        assert_eq!(status.last_error.as_deref(), Some("poll failed"));
        assert!(manager.connections.read()["weixin-main"].generation > 0);
    }

    #[tokio::test]
    async fn feishu_manager_registers_verified_connection_before_background_polling() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind feishu token mock");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut request = vec![0u8; 4096];
                let _ = stream.read(&mut request).await;
                let body = br#"{"code":0,"tenant_access_token":"token","expire":7200}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    std::str::from_utf8(body).unwrap()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let manager = ImConnectionManager::new();
        let config = ImProviderConfig {
            id: "feishu-status".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Status".to_string(),
            enabled: true,
            base_url: Some(format!("http://127.0.0.1:{port}/open-apis")),
            app_id: Some("app-id".to_string()),
            secret_ref: Some("app-secret".to_string()),
            owner_open_id: None,
            event_connection_enabled: true,
            event_types: vec!["im.message.receive_v1".to_string()],
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        };
        let (sink, _events) = tokio::sync::mpsc::unbounded_channel();

        manager
            .start_connection(&config, "app-secret", sink.into())
            .await
            .expect("start feishu connection after token validation");
        assert!(manager.get_status(&config.id).is_some());

        manager.stop_all();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn weixin_manager_starts_polling_and_propagates_auth_expiry_status() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind weixin poll mock");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut request = vec![0u8; 4096];
                let _ = stream.read(&mut request).await;
                let body = br#"{"ret":-14,"errmsg":"authorization expired"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    std::str::from_utf8(body).unwrap()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let manager = ImConnectionManager::new_with_data_dir(dir.path());
        let config = ImProviderConfig {
            id: "weixin-status".to_string(),
            provider_type: ImProviderType::Weixin,
            display_name: "Weixin Status".to_string(),
            enabled: true,
            base_url: Some(format!("http://127.0.0.1:{port}")),
            app_id: Some("bot@im.bot".to_string()),
            secret_ref: Some("token".to_string()),
            owner_open_id: Some("owner@im.wechat".to_string()),
            event_connection_enabled: true,
            event_types: vec!["message.receive".to_string()],
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        };
        let (sink, _events) = tokio::sync::mpsc::unbounded_channel();
        manager
            .start_connection(&config, "", sink.into())
            .await
            .expect("start weixin connection");

        let mut status = manager.get_status(&config.id).unwrap();
        for _ in 0..50 {
            if status.state == ConnectionState::AuthenticationRequired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            status = manager.get_status(&config.id).unwrap();
        }
        assert_eq!(status.state, ConnectionState::AuthenticationRequired);
        assert!(status
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("scan a new QR code")));
        manager.stop_all();
    }
}
