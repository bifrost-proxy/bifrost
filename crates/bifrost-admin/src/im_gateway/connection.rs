use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::oneshot;
use tracing::info;

use bifrost_core::Result;

use crate::im_gateway::feishu::{self, FeishuProvider};
use crate::im_gateway::provider::EventSink;
use crate::im_gateway::types::{
    ConnectionHandle, ConnectionState, ConnectionStatus, ImProviderConfig, ImProviderType,
};

// ---------------------------------------------------------------------------
// Managed Connection
// ---------------------------------------------------------------------------

struct ManagedConnection {
    #[allow(dead_code)]
    provider_id: String,
    handle: ConnectionHandle,
    status: ConnectionStatus,
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
}

impl Default for ImConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ImConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            feishu_provider: Arc::new(FeishuProvider::new()),
        }
    }

    /// Get a reference to the Feishu provider for sending messages.
    pub fn feishu_provider(&self) -> &Arc<FeishuProvider> {
        &self.feishu_provider
    }

    /// Start a long connection for a provider.
    ///
    /// If a connection already exists for this provider, it will be stopped first.
    pub async fn start_connection(
        &self,
        config: &ImProviderConfig,
        app_secret: &str,
        sink: EventSink,
    ) -> Result<()> {
        let provider_id = config.id.clone();

        // Stop existing connection if any
        self.stop_connection(&provider_id);

        match config.provider_type {
            ImProviderType::Feishu => self.start_feishu_connection(config, app_secret, sink).await,
            _ => Err(bifrost_core::BifrostError::Config(format!(
                "long connection not supported for provider type {:?}",
                config.provider_type
            ))),
        }
    }

    /// Start a Feishu long connection.
    async fn start_feishu_connection(
        &self,
        config: &ImProviderConfig,
        app_secret: &str,
        sink: EventSink,
    ) -> Result<()> {
        let provider_id = config.id.clone();

        // Pre-fetch token to ensure credentials are valid before spawning
        self.feishu_provider
            .get_tenant_token(config, app_secret)
            .await?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Update status to connecting
        let status = ConnectionStatus {
            state: ConnectionState::Connecting,
            last_connected_at: None,
            last_event_at: None,
            reconnect_count: 0,
            last_error: None,
        };

        let handle = ConnectionHandle { shutdown_tx };

        {
            let mut conns = self.connections.write();
            conns.insert(
                provider_id.clone(),
                ManagedConnection {
                    provider_id: provider_id.clone(),
                    handle,
                    status,
                },
            );
        }

        // Spawn the long connection task
        let config_clone = config.clone();
        let secret_clone = app_secret.to_string();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let connections = self.connections_arc();
        let pid = provider_id.clone();

        tokio::spawn(async move {
            // Update to connected once we enter the loop
            update_connection_state(&connections, &pid, ConnectionState::Connected, None);

            feishu::start_long_connection(config_clone, secret_clone, sink, shutdown_rx, http)
                .await;

            // Connection ended - update status
            update_connection_state(
                &connections,
                &pid,
                ConnectionState::Disconnected,
                Some("connection task ended".to_string()),
            );
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

    /// Get connection status for a specific provider.
    pub fn get_status(&self, provider_id: &str) -> Option<ConnectionStatus> {
        let conns = self.connections.read();
        conns.get(provider_id).map(|c| c.status.clone())
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
            if let Some(err) = error {
                conn.status.last_error = Some(err);
            }
            if state == ConnectionState::Connected {
                conn.status.last_connected_at = Some(current_timestamp_ms());
            }
            if state == ConnectionState::Reconnecting {
                conn.status.reconnect_count += 1;
            }
        }
    }

    /// Get an Arc-like handle to connections for use in spawned tasks.
    ///
    /// This returns a raw pointer wrapper that allows status updates from async tasks.
    /// In practice, the ImConnectionManager should be wrapped in Arc at the service level.
    fn connections_arc(&self) -> Arc<RwLock<HashMap<String, ManagedConnection>>> {
        Arc::clone(&self.connections)
    }
}

// ---------------------------------------------------------------------------
// Helper for background task status updates
// ---------------------------------------------------------------------------

fn update_connection_state(
    connections: &Arc<RwLock<HashMap<String, ManagedConnection>>>,
    provider_id: &str,
    state: ConnectionState,
    error: Option<String>,
) {
    let mut conns = connections.write();
    if let Some(conn) = conns.get_mut(provider_id) {
        conn.status.state = state;
        if let Some(err) = error {
            conn.status.last_error = Some(err);
        }
        if state == ConnectionState::Connected {
            conn.status.last_connected_at = Some(current_timestamp_ms());
        }
        if state == ConnectionState::Reconnecting {
            conn.status.reconnect_count += 1;
        }
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
    fn test_get_status_nonexistent_returns_none() {
        let mgr = ImConnectionManager::new();
        assert!(mgr.get_status("nonexistent").is_none());
    }

    #[test]
    fn test_stop_connection_nonexistent_is_noop() {
        let mgr = ImConnectionManager::new();
        mgr.stop_connection("nonexistent"); // Should not panic
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
}
