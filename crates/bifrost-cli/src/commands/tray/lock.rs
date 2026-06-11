use std::fs::{self, File};
use std::path::{Path, PathBuf};

use fs2::FileExt;

pub struct TrayLock {
    _file: File,
    pid_path: PathBuf,
}

impl TrayLock {
    pub fn acquire(data_dir: &Path) -> Result<Self, String> {
        let lock_path = data_dir.join("tray.lock");
        let pid_path = data_dir.join("tray.pid");

        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create data dir: {e}"))?;
        }

        let file = File::create(&lock_path).map_err(|e| format!("cannot create tray.lock: {e}"))?;

        file.try_lock_exclusive()
            .map_err(|_| "another tray helper is already running".to_string())?;

        let pid = std::process::id();
        let _ = fs::write(&pid_path, pid.to_string());

        Ok(Self {
            _file: file,
            pid_path,
        })
    }
}

impl Drop for TrayLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.pid_path);
    }
}
