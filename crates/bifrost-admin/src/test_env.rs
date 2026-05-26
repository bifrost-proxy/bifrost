use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

static BIFROST_DATA_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) struct BifrostDataDirGuard {
    _guard: MutexGuard<'static, ()>,
    old_data_dir: Option<String>,
}

impl BifrostDataDirGuard {
    pub(crate) fn set(data_dir: &Path) -> Self {
        let guard = BIFROST_DATA_DIR_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_data_dir = std::env::var("BIFROST_DATA_DIR").ok();
        std::env::set_var("BIFROST_DATA_DIR", data_dir);
        Self {
            _guard: guard,
            old_data_dir,
        }
    }
}

impl Drop for BifrostDataDirGuard {
    fn drop(&mut self) {
        match self.old_data_dir.take() {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
    }
}
