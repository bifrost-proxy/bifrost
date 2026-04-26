use std::path::PathBuf;
use std::sync::RwLock;

// Allow tests (and in-process reconfiguration) to swap the active data
// directory multiple times. Previously this was a `OnceLock`, which meant
// only the very first `set_data_dir` call in a given process was honored;
// subsequent calls were silently ignored. That was fine in production
// (the CLI sets the dir exactly once at startup), but it broke the E2E
// suite: multiple `remote_shell_exec_*` tests each try to point
// `data_dir` at their own temp directory, and when run in parallel only
// one of them would win the race. The rest would then read from the
// winner's (often already removed) directory and fail with opaque
// "policy not found" / IO errors — notably on Windows runners with
// `--jobs 8`.
static DATA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Set the data directory used by all storage helpers. Safe to call
/// multiple times; later calls override earlier ones. Intended for CLI
/// startup and in-process test setup.
pub fn set_data_dir(dir: PathBuf) {
    // `unwrap_or_else` keeps this infallible even if the lock were poisoned
    // by a previous panic — we just replace the inner state regardless.
    let mut guard = DATA_DIR.write().unwrap_or_else(|p| p.into_inner());
    *guard = Some(dir);
}

/// Return the active data directory. Resolution order:
/// 1. An explicit `set_data_dir` value, if any.
/// 2. The `BIFROST_DATA_DIR` environment variable.
/// 3. `$HOME/.bifrost`, falling back to `./.bifrost` if the home dir
///    cannot be resolved.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = DATA_DIR.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
        return dir.clone();
    }

    if let Ok(dir) = std::env::var("BIFROST_DATA_DIR") {
        return PathBuf::from(dir);
    }

    dirs::home_dir()
        .map(|h| h.join(".bifrost"))
        .unwrap_or_else(|| PathBuf::from(".bifrost"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_data_dir_is_reentrant() {
        let a = PathBuf::from("/tmp/bifrost-data-dir-test-a");
        let b = PathBuf::from("/tmp/bifrost-data-dir-test-b");
        set_data_dir(a.clone());
        assert_eq!(data_dir(), a);
        set_data_dir(b.clone());
        assert_eq!(data_dir(), b);
    }
}
