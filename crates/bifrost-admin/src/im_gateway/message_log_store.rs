use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::ImMessageLog;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_message_logs.json";
const MAX_MESSAGES: usize = 5000;
/// 90 days in milliseconds
const TTL_MS: u64 = 90 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    messages: Vec<ImMessageLog>,
}

pub struct ImMessageLogStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImMessageLogStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            messages: Vec::new(),
        });
        let store = Self {
            file_path,
            data: RwLock::new(data),
        };
        // Purge expired entries on startup
        let _ = store.purge_expired();
        store
    }

    /// Add a message log entry.
    pub fn add(&self, log: ImMessageLog) -> Result<()> {
        let mut data = self.data.write();
        data.messages.push(log);
        self.trim_locked(&mut data);
        self.save_locked(&data)
    }

    /// List all message logs, newest first.
    pub fn list(&self) -> Vec<ImMessageLog> {
        let data = self.data.read();
        let mut msgs = data.messages.clone();
        msgs.reverse();
        msgs
    }

    /// List message logs for a specific provider, newest first.
    pub fn list_by_provider(&self, provider_id: &str) -> Vec<ImMessageLog> {
        let data = self.data.read();
        let mut msgs: Vec<_> = data
            .messages
            .iter()
            .filter(|m| m.provider_id == provider_id)
            .cloned()
            .collect();
        msgs.reverse();
        msgs
    }

    /// Clear all message logs for a specific provider.
    pub fn clear_by_provider(&self, provider_id: &str) -> Result<()> {
        let mut data = self.data.write();
        data.messages.retain(|m| m.provider_id != provider_id);
        self.save_locked(&data)
    }

    /// Clear all message logs.
    pub fn clear_all(&self) -> Result<()> {
        let mut data = self.data.write();
        data.messages.clear();
        self.save_locked(&data)
    }

    /// Remove messages older than 90 days.
    pub fn purge_expired(&self) -> Result<()> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let cutoff = now_ms.saturating_sub(TTL_MS);

        let mut data = self.data.write();
        let before = data.messages.len();
        data.messages.retain(|m| m.timestamp >= cutoff);
        if data.messages.len() != before {
            self.save_locked(&data)?;
        }
        Ok(())
    }

    fn trim_locked(&self, data: &mut StoreData) {
        if data.messages.len() > MAX_MESSAGES {
            let drain_count = data.messages.len() - MAX_MESSAGES;
            data.messages.drain(..drain_count);
        }
    }

    fn save_locked(&self, data: &StoreData) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| BifrostError::Config(format!("serialize message log store: {e}")))?;
        std::fs::write(&self.file_path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.file_path.display()
            )))
        })?;
        Ok(())
    }

    fn load_from_disk(file_path: &Path) -> Option<StoreData> {
        if !file_path.exists() {
            return None;
        }
        let content = std::fs::read_to_string(file_path).ok()?;
        match serde_json::from_str::<StoreData>(&content) {
            Ok(data) if data.version == STORE_VERSION => Some(data),
            _ => {
                let _ = std::fs::remove_file(file_path);
                None
            }
        }
    }
}
