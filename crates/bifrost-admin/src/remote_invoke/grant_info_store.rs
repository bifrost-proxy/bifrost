use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::GrantInfo;

const GRANT_INFO_STORE_FILE: &str = "remote_invoke_grant_info.json";
const GRANT_INFO_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantInfoStoreFile {
    version: u32,
    entries: Vec<PersistedGrantInfoEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedGrantInfoEntry {
    grant_id: String,
    relay_url: String,
    info: GrantInfo,
    updated_at: u64,
}

pub struct GrantInfoStore {
    file_path: PathBuf,
    lock: Mutex<()>,
}

impl GrantInfoStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        Self {
            file_path: admin_dir.join(GRANT_INFO_STORE_FILE),
            lock: Mutex::new(()),
        }
    }

    pub fn load_for_relay(&self, relay_url: &str) -> Result<HashMap<String, GrantInfo>> {
        let _guard = self.lock.lock();
        let file = self.read_store_file()?;
        Ok(file
            .entries
            .into_iter()
            .filter(|entry| entry.relay_url == relay_url)
            .map(|entry| (entry.grant_id, entry.info))
            .collect())
    }

    pub fn upsert(&self, relay_url: &str, grant_id: &str, info: &GrantInfo) -> Result<()> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let updated_at = now_millis();
        if let Some(existing) = file
            .entries
            .iter_mut()
            .find(|entry| entry.grant_id == grant_id && entry.relay_url == relay_url)
        {
            existing.info = info.clone();
            existing.updated_at = updated_at;
        } else {
            file.entries.push(PersistedGrantInfoEntry {
                grant_id: grant_id.to_string(),
                relay_url: relay_url.to_string(),
                info: info.clone(),
                updated_at,
            });
        }
        self.write_store_file(&file)
    }

    pub fn remove(&self, grant_id: &str) -> Result<()> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let before = file.entries.len();
        file.entries.retain(|entry| entry.grant_id != grant_id);
        if before == file.entries.len() {
            return Ok(());
        }
        self.write_store_file(&file)
    }

    pub fn retain_only(&self, relay_url: &str, grant_ids: &HashSet<String>) -> Result<()> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let before = file.entries.len();
        file.entries
            .retain(|entry| entry.relay_url != relay_url || grant_ids.contains(&entry.grant_id));
        if before == file.entries.len() {
            return Ok(());
        }
        self.write_store_file(&file)
    }

    fn read_store_file(&self) -> Result<GrantInfoStoreFile> {
        if !self.file_path.exists() {
            return Ok(GrantInfoStoreFile {
                version: GRANT_INFO_STORE_VERSION,
                entries: Vec::new(),
            });
        }
        const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(&self.file_path) {
            if meta.len() > MAX_STORE_FILE_BYTES {
                return Err(BifrostError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("store file too large ({} bytes)", meta.len()),
                )));
            }
        }
        let content = std::fs::read_to_string(&self.file_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                self.file_path.display()
            )))
        })?;
        match serde_json::from_str::<GrantInfoStoreFile>(&content) {
            Ok(file) if file.version == GRANT_INFO_STORE_VERSION => Ok(file),
            Ok(file) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset incompatible grant info store version {}",
                    file.version
                )))
            }
            Err(e) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset unreadable grant info store {}: {e}",
                    self.file_path.display()
                )))
            }
        }
    }

    fn write_store_file(&self, file: &GrantInfoStoreFile) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_string_pretty(file)
            .map_err(|e| BifrostError::Config(format!("serialize grant info store: {e}")))?;
        std::fs::write(&self.file_path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.file_path.display()
            )))
        })?;
        Ok(())
    }

    fn reset_store_file(&self) -> Result<()> {
        if self.file_path.exists() {
            std::fs::remove_file(&self.file_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "remove {}: {e}",
                    self.file_path.display()
                )))
            })?;
        }
        Ok(())
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
