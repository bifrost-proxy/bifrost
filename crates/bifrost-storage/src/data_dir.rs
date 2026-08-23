use std::path::PathBuf;
use std::sync::RwLock;

use sha2::{Digest, Sha256};

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

/// Return a stable, non-reversible identity for a Bifrost data directory.
///
/// Lifecycle clients use this value to distinguish two Bifrost instances that
/// happen to listen on the same port at different times. The canonical path is
/// hashed instead of returned over the Admin API so local directory names are
/// not exposed to callers.
pub fn data_dir_fingerprint_for(path: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|current| current.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    });
    let mut hasher = Sha256::new();
    hasher.update(b"bifrost-data-dir-v1\0");
    hasher.update(resolved.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Return the identity of the active Bifrost data directory.
pub fn data_dir_fingerprint() -> String {
    data_dir_fingerprint_for(&data_dir())
}

#[cfg(test)]
mod tests {
    use super::data_dir_fingerprint_for;

    #[test]
    fn fingerprint_is_stable_for_equivalent_paths_and_distinct_for_other_directories() {
        let root = tempfile::tempdir().expect("temp root");
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir_all(&first).expect("create first");
        std::fs::create_dir_all(&second).expect("create second");

        assert_eq!(
            data_dir_fingerprint_for(&first),
            data_dir_fingerprint_for(&first.join("..").join("first"))
        );
        assert_ne!(
            data_dir_fingerprint_for(&first),
            data_dir_fingerprint_for(&second)
        );
    }

    #[test]
    fn fingerprint_is_stable_before_a_directory_exists() {
        let root = tempfile::tempdir().expect("temp root");
        let missing_absolute = root.path().join("not-created");
        assert_eq!(
            data_dir_fingerprint_for(&missing_absolute),
            data_dir_fingerprint_for(&missing_absolute)
        );

        let relative = std::path::Path::new("relative-data-dir-not-created");
        assert_eq!(
            data_dir_fingerprint_for(relative),
            data_dir_fingerprint_for(relative)
        );
    }
}
