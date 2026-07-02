pub mod call_history_store;
pub mod config;
pub mod executor;
pub mod file_access_roots;
pub mod file_ops;
pub mod file_policy_store;
pub mod file_transfer;
pub mod grant_crypto_store;
pub mod grant_info_store;
pub mod grant_policy_store;
pub mod identity;
pub mod relay_client;
pub mod session_ring;
pub mod ssh_keys;
#[allow(dead_code)]
pub mod stream_emit;
pub mod types;
pub mod worker;

pub use config::RemoteInvokeConfig;
pub use executor::RemoteInvokeExecutor;
pub use grant_info_store::GrantInfoStore;
pub use identity::Identity;
pub use relay_client::RelayClient;
pub use types::{RemoteInvokeRequest, RemoteInvokeResponse};
pub use worker::RemoteInvokeWorker;

#[cfg(test)]
pub(crate) fn remote_shell_test_guard() -> std::sync::MutexGuard<'static, ()> {
    // Share the single process-global data-dir lock with `BifrostDataDirGuard`.
    // These tests mutate `bifrost_storage::set_data_dir` directly, so they must
    // be mutually exclusive with every other data-dir test guard; otherwise a
    // parallel guard can clobber the global between this test's save/load.
    crate::test_env::bifrost_data_dir_lock()
}
