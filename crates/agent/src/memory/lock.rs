//! Cross-process exclusive lock for Phase 2 consolidation.

use crate::memory::utils::now_secs;
use fs2::FileExt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Cross-process exclusive lock for Phase 2 consolidation. Uses real `fs2`
/// advisory locking so that if a process dies abruptly the OS releases the
/// lock — no staleness heuristic required.
pub(crate) struct Phase2LockGuard {
    file: Option<fs::File>,
    path: PathBuf,
}

impl Drop for Phase2LockGuard {
    fn drop(&mut self) {
        if let Some(ref file) = self.file {
            let _ = FileExt::unlock(file);
        }
        // Keep the lock file on disk so that `ls -la` still shows it — the
        // advisory lock is what matters, not the file existence. Cleaning up
        // unconditionally can race with another process that is right now
        // trying to open+lock.
        let _ = &self.path;
    }
}

impl Phase2LockGuard {
    pub(crate) fn try_acquire(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(".phase2.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                // best-effort: record pid + timestamp for human debugging
                let note = format!("pid={} acquired_at={}\n", std::process::id(), now_secs());
                let _ = (&file).write_all(note.as_bytes());
                let _ = file.sync_data();
                Ok(Some(Self {
                    file: Some(file),
                    path,
                }))
            }
            Err(error) => {
                // contention: another process holds the lock
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                Err(format!("lock {}: {error}", path.display()))
            }
        }
    }
}
