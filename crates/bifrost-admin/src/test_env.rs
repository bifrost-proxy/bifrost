use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

static BIFROST_DATA_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
#[cfg(test)]
static AGENT_WORKER_ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub(crate) struct BifrostDataDirGuard {
    _guard: MutexGuard<'static, ()>,
    old_data_dir: Option<String>,
    old_static_dir: PathBuf,
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
pub(crate) fn agent_worker_env_lock() -> &'static tokio::sync::Mutex<()> {
    AGENT_WORKER_ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
