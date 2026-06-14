//! Test harness utilities for `bifrost-admin`.
//!
//! This module is only compiled for tests and when the `test-support` feature
//! is enabled. It provides a small, tempdir-backed `AdminState` wiring that
//! never touches the real user data directory and performs no external
//! network I/O.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use bifrost_storage::{ConfigManager, RemoteShellStore, RulesStorage, ValuesStorage};
use parking_lot::RwLock as ParkingRwLock;

use crate::test_env::BifrostDataDirGuard;
use crate::{
    AdminState, BodyStore, FrameStore, ImGatewayService, PushManager, ReplayDbStore,
    ReplayExecutor, SharedBodyStore, SharedFrameStore, SharedReplayDbStore, SharedReplayExecutor,
    SharedTrafficDbStore, SharedValuesStorage, SharedWsPayloadStore, TrafficDbStore,
    WsPayloadStore,
};

/// Fully wired admin test harness backed by a temporary data directory.
///
/// The harness owns the tempdir and an `AdminState` configured with:
///
/// * tempdir-backed `RulesStorage` and `ValuesStorage`.
/// * a SQLite `TrafficDbStore` under the tempdir.
/// * `BodyStore`, `FrameStore`, and `WsPayloadStore` using subdirectories
///   of the tempdir.
/// * a `ConfigManager` whose data_dir is the tempdir.
/// * a `ReplayDbStore` + `ReplayExecutor` pair wired into the state.
///
/// No external network is touched while constructing the harness.
pub struct TestAdminState {
    temp_dir: TempDir,
    // Keeps `BIFROST_DATA_DIR` pointed at the harness temp dir for the
    // lifetime of the harness so any `bifrost_storage::data_dir()` calls
    // remain sandboxed.
    _data_dir_guard: BifrostDataDirGuard,
    admin_state: Arc<AdminState>,
    pub config_manager: Arc<ConfigManager>,
    pub values_storage: SharedValuesStorage,
    pub traffic_db: SharedTrafficDbStore,
    pub body_store: SharedBodyStore,
    pub frame_store: SharedFrameStore,
    pub ws_payload_store: SharedWsPayloadStore,
    pub replay_db_store: SharedReplayDbStore,
    pub replay_executor: SharedReplayExecutor,
}

/// Builder for [`TestAdminState`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TestAdminStateBuilder {
    port: u16,
}

impl TestAdminState {
    /// Start building a new tempdir-backed `AdminState` harness.
    pub fn builder() -> TestAdminStateBuilder {
        TestAdminStateBuilder::default()
    }

    /// Shared handle to the underlying `AdminState`.
    pub fn state(&self) -> Arc<AdminState> {
        self.admin_state.clone()
    }

    /// Root data directory used by this harness.
    pub fn data_dir(&self) -> &std::path::Path {
        self.temp_dir.path()
    }

    /// Convenience helper for tests that want a tempdir-scoped `RemoteShellStore`.
    pub fn remote_shell_store(&self) -> RemoteShellStore {
        RemoteShellStore::with_file(self.data_dir().join("remote_shell.json"))
            .expect("create RemoteShellStore for test harness")
    }

    /// Convenience helper for tests that need a push manager bound to this state.
    pub fn push_manager(&self) -> crate::SharedPushManager {
        Arc::new(PushManager::new(self.admin_state.clone()))
    }

    /// Convenience helper for tests that need an `ImGatewayService` bound to
    /// the same data directory. This does not perform any external network I/O
    /// by itself; callers are in control of which methods they exercise.
    pub fn im_gateway_service(&self) -> crate::SharedImGatewayService {
        Arc::new(ImGatewayService::new(self.data_dir()))
    }
}

impl TestAdminStateBuilder {
    /// Override the admin API port used for the constructed `AdminState`.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Build a new [`TestAdminState`] harness.
    pub fn build(self) -> TestAdminState {
        // Isolated temp data directory.
        let temp_dir = TempDir::new().expect("create bifrost-admin test data dir");
        let data_dir: PathBuf = temp_dir.path().to_path_buf();

        // Ensure all `bifrost_storage::data_dir()` callers see the same
        // sandbox directory for the lifetime of this harness.
        let data_dir_guard = BifrostDataDirGuard::set(&data_dir);

        let rules_storage =
            RulesStorage::with_dir(data_dir.join("rules")).expect("create test RulesStorage");
        let values_storage_raw =
            ValuesStorage::with_dir(data_dir.join("values")).expect("create test ValuesStorage");

        let config_manager =
            Arc::new(ConfigManager::new(data_dir.clone()).expect("create test ConfigManager"));

        // Traffic DB: small but non-trivial limits suitable for unit tests.
        let traffic_db =
            TrafficDbStore::new(data_dir.join("traffic"), 1024, 16 * 1024 * 1024, Some(24))
                .expect("create test TrafficDbStore");
        let traffic_db_shared: SharedTrafficDbStore = Arc::new(traffic_db);

        // Body / frame / WS payload stores all scoped under the temp data dir.
        let body_store_inner = BodyStore::new(
            data_dir.join("body_cache"),
            8 * 1024 * 1024,
            7,
            64 * 1024,
            Duration::from_millis(200),
        );
        let body_store: SharedBodyStore = Arc::new(ParkingRwLock::new(body_store_inner));

        let frame_store_inner = FrameStore::new(data_dir.clone(), Some(24));
        let frame_store: SharedFrameStore = Arc::new(frame_store_inner);

        let ws_payload_store_inner = WsPayloadStore::new(
            data_dir.clone(),
            64 * 1024,
            Duration::from_millis(200),
            32,
            7,
        );
        let ws_payload_store: SharedWsPayloadStore = Arc::new(ws_payload_store_inner);

        let replay_db_store =
            ReplayDbStore::new(data_dir.join("replay")).expect("create test ReplayDbStore");
        let replay_db_store: SharedReplayDbStore = Arc::new(replay_db_store);

        // Admin state wired with the in-process stores.
        let admin_state = AdminState::new(self.port)
            .with_rules_storage(rules_storage)
            .with_values_storage(values_storage_raw)
            .with_traffic_db_store_shared(traffic_db_shared.clone())
            .with_body_store(body_store.clone())
            .with_frame_store_shared(frame_store.clone())
            .with_ws_payload_store(ws_payload_store.clone())
            .with_config_manager_shared(config_manager.clone())
            .with_replay_db_store_shared(replay_db_store.clone());

        let admin_state = Arc::new(admin_state);

        // Replay executor uses the shared AdminState but performs no network
        // I/O until its `execute` method is called.
        let replay_executor: SharedReplayExecutor =
            Arc::new(ReplayExecutor::new(admin_state.clone(), false));
        admin_state.set_replay_executor(replay_executor.clone());

        let values_storage = admin_state
            .values_storage
            .as_ref()
            .cloned()
            .expect("values_storage must be configured for TestAdminState");

        TestAdminState {
            temp_dir,
            _data_dir_guard: data_dir_guard,
            admin_state,
            config_manager,
            values_storage,
            traffic_db: traffic_db_shared,
            body_store,
            frame_store,
            ws_payload_store,
            replay_db_store,
            replay_executor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_creates_isolated_admin_state() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();

        // Ensure the harness wires basic components.
        assert!(state.values_storage.is_some());
        assert!(state.traffic_db_store.is_some());
        assert!(state.body_store.is_some());
        assert!(state.frame_store.is_some());
        assert!(state.ws_payload_store.is_some());

        // Ensure the data dir points inside the tempdir.
        let data_dir = harness.data_dir().to_path_buf();
        assert!(data_dir.exists());
    }
}
