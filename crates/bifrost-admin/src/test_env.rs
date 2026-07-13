use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

static BIFROST_DATA_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) struct BifrostDataDirGuard {
    _guard: MutexGuard<'static, ()>,
    old_data_dir: Option<String>,
    old_static_dir: PathBuf,
}

/// Lock guarding the process-global bifrost data dir during tests.
///
/// `BifrostDataDirGuard` and the remote-invoke shell tests both mutate the same
/// process-global data dir via `bifrost_storage::set_data_dir`. They must share a
/// single mutex, otherwise a `BifrostDataDirGuard`-based test can clobber the
/// global between another test's save/load (observed as a flaky
/// `remove_grant_policy_*` failure under the parallel `--all-features` CI run).
/// No test acquires both guard families at once, so sharing the lock is
/// deadlock-free.
#[cfg(test)]
pub(crate) fn bifrost_data_dir_lock() -> MutexGuard<'static, ()> {
    BIFROST_DATA_DIR_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl BifrostDataDirGuard {
    pub(crate) fn set(data_dir: &Path) -> Self {
        let guard = BIFROST_DATA_DIR_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_data_dir = std::env::var("BIFROST_DATA_DIR").ok();
        // `bifrost_storage::data_dir()` consults the process-global static set
        // by `set_data_dir()` *before* the env var, so a guard that only set
        // the env var would be silently shadowed by any other test that called
        // `set_data_dir`. Drive both here (under one shared lock) so all
        // data-dir test guards are mutually exclusive and fully isolated.
        let old_static_dir = bifrost_storage::data_dir();
        std::env::set_var("BIFROST_DATA_DIR", data_dir);
        bifrost_storage::set_data_dir(data_dir.to_path_buf());
        Self {
            _guard: guard,
            old_data_dir,
            old_static_dir,
        }
    }
}

impl Drop for BifrostDataDirGuard {
    fn drop(&mut self) {
        match self.old_data_dir.take() {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
        bifrost_storage::set_data_dir(self.old_static_dir.clone());
    }
}

#[cfg(test)]
static VOICE_ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

/// Serializes voice tests that read or mutate process-global `BIFROST_VOICE_*`
/// env vars (e.g. `BIFROST_VOICE_ALLOW_STATEFUL_17B`,
/// `BIFROST_VOICE_ENABLE_FAKE_STATEFUL`). Without this, a test that only *reads*
/// a flag can observe a value another test transiently *set*, producing a flaky
/// failure under the parallel `--all-features` run. A `tokio::sync::Mutex` is
/// used so async tests can hold it across `.await` without `await_holding_lock`.
#[cfg(test)]
pub(crate) fn voice_env_lock() -> &'static tokio::sync::Mutex<()> {
    VOICE_ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
