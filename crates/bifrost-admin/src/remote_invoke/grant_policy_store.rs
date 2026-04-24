use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use bifrost_core::{BifrostError, Result};

use super::types::GrantScope;

const GRANT_POLICY_STORE_FILE: &str = "remote_invoke_grant_policy.json";
const GRANT_POLICY_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredGrantPolicy {
    pub grant_scope: GrantScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_binding: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_policy_set_version_snapshot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactive_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin_allowed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantPolicyStoreFile {
    version: u32,
    entries: Vec<PersistedGrantPolicyEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedGrantPolicyEntry {
    grant_id: String,
    relay_url: String,
    policy: StoredGrantPolicy,
    updated_at: u64,
}

pub struct GrantPolicyStore {
    file_path: PathBuf,
    lock: Mutex<()>,
}

impl GrantPolicyStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        Self {
            file_path: admin_dir.join(GRANT_POLICY_STORE_FILE),
            lock: Mutex::new(()),
        }
    }

    pub fn load_for_relay(&self, relay_url: &str) -> Result<HashMap<String, StoredGrantPolicy>> {
        let _guard = self.lock.lock();
        let file = self.read_store_file()?;
        Ok(file
            .entries
            .into_iter()
            .filter(|entry| entry.relay_url == relay_url)
            .map(|entry| (entry.grant_id, entry.policy))
            .collect())
    }

    pub fn upsert(
        &self,
        relay_url: &str,
        grant_id: &str,
        policy: &StoredGrantPolicy,
    ) -> Result<()> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let updated_at = now_millis();
        if let Some(existing) = file
            .entries
            .iter_mut()
            .find(|entry| entry.grant_id == grant_id && entry.relay_url == relay_url)
        {
            existing.policy = policy.clone();
            existing.updated_at = updated_at;
        } else {
            file.entries.push(PersistedGrantPolicyEntry {
                grant_id: grant_id.to_string(),
                relay_url: relay_url.to_string(),
                policy: policy.clone(),
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

    fn read_store_file(&self) -> Result<GrantPolicyStoreFile> {
        if !self.file_path.exists() {
            return Ok(GrantPolicyStoreFile {
                version: GRANT_POLICY_STORE_VERSION,
                entries: Vec::new(),
            });
        }
        let content = std::fs::read_to_string(&self.file_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                self.file_path.display()
            )))
        })?;
        match serde_json::from_str::<GrantPolicyStoreFile>(&content) {
            Ok(file) if file.version == GRANT_POLICY_STORE_VERSION => Ok(file),
            Ok(file) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset incompatible grant policy store version {}",
                    file.version
                )))
            }
            Err(e) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset unreadable grant policy store {}: {e}",
                    self.file_path.display()
                )))
            }
        }
    }

    fn write_store_file(&self, file: &GrantPolicyStoreFile) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_string_pretty(file)
            .map_err(|e| BifrostError::Config(format!("serialize grant policy store: {e}")))?;
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
