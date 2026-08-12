use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::warn;

use bifrost_core::{BifrostError, Result};

use super::types::ImEvent;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_events.json";
const MAX_EVENTS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    events: Vec<ImEvent>,
}

pub struct ImEventStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImEventStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let mut data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            events: Vec::new(),
        });
        let mut removed_legacy_credentials = false;
        for event in &mut data.events {
            removed_legacy_credentials |= redact_event_for_history_in_place(event);
        }
        let store = Self {
            file_path,
            data: RwLock::new(data),
        };
        if removed_legacy_credentials {
            let data = store.data.read();
            if let Err(error) = store.save_locked(&data) {
                warn!(
                    path = %store.file_path.display(),
                    error = %error,
                    "failed to rewrite legacy IM history after removing Weixin media credentials"
                );
            }
        }
        store
    }

    pub fn list(&self) -> Vec<ImEvent> {
        self.data.read().events.clone()
    }

    pub fn list_by_provider(&self, provider_id: &str) -> Vec<ImEvent> {
        self.data
            .read()
            .events
            .iter()
            .filter(|e| e.provider_id == provider_id)
            .cloned()
            .collect()
    }

    pub fn add(&self, event: ImEvent) -> Result<()> {
        let mut data = self.data.write();
        data.events.push(redact_event_for_history(event));
        self.trim_locked(&mut data);
        self.save_locked(&data)
    }

    pub fn clear(&self) -> Result<()> {
        let mut data = self.data.write();
        data.events.clear();
        self.save_locked(&data)
    }

    fn trim_locked(&self, data: &mut StoreData) {
        if data.events.len() > MAX_EVENTS {
            let drain_count = data.events.len() - MAX_EVENTS;
            data.events.drain(..drain_count);
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
            .map_err(|e| BifrostError::Config(format!("serialize event store: {e}")))?;
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
        const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
        if std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0) > MAX_STORE_FILE_BYTES {
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

fn redact_event_for_history(mut event: ImEvent) -> ImEvent {
    redact_event_for_history_in_place(&mut event);
    event
}

fn redact_event_for_history_in_place(event: &mut ImEvent) -> bool {
    if event.provider_type != super::types::ImProviderType::Weixin {
        return false;
    }
    let mut changed = event.raw_digest.take().is_some();
    if let Some(message) = event.message.as_mut() {
        changed |= message.raw_content.take().is_some();
        for image in &mut message.images {
            changed |= image.data_base64.take().is_some();
            changed |= image.download_url.take().is_some();
            changed |= image.encrypted_query_param.take().is_some();
            changed |= image.aes_key.take().is_some();
        }
        for file in &mut message.files {
            changed |= file.data_base64.take().is_some();
            changed |= file.download_url.take().is_some();
            changed |= file.encrypted_query_param.take().is_some();
            changed |= file.aes_key.take().is_some();
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_legacy_string_mentions_without_deleting_history() {
        let temp = tempfile::tempdir().unwrap();
        let admin_dir = temp.path().join("admin");
        std::fs::create_dir_all(&admin_dir).unwrap();
        let file_path = admin_dir.join(STORE_FILENAME);
        std::fs::write(
            &file_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": STORE_VERSION,
                "events": [{
                    "event_id": "legacy-event",
                    "provider_id": "feishu-main",
                    "provider_type": "feishu",
                    "event_type": "message.receive",
                    "source": {
                        "chat_id": "oc_group",
                        "chat_type": "group",
                        "message_id": "om_legacy"
                    },
                    "message": {
                        "text": "@_user_1 hello",
                        "mentions": ["@_user_1"]
                    },
                    "received_at": 1
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let store = ImEventStore::new(temp.path());
        let events = store.list();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message.as_ref().unwrap().mentions.len(), 1);
        assert_eq!(
            events[0].message.as_ref().unwrap().mentions[0],
            crate::im_gateway::types::ImMention {
                key: "@_user_1".to_string(),
                ..Default::default()
            }
        );
        assert!(file_path.exists(), "legacy history must not be deleted");
    }
}
