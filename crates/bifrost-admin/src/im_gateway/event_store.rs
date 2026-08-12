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
        if !event.event_id.is_empty()
            && data.events.iter().any(|stored| {
                stored.provider_id == event.provider_id && stored.event_id == event.event_id
            })
        {
            return Ok(());
        }
        let previous = data.clone();
        data.events.push(redact_event_for_history(event));
        self.trim_locked(&mut data);
        if let Err(error) = self.save_locked(&data) {
            *data = previous;
            return Err(error);
        }
        Ok(())
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

    fn event(event_id: &str) -> ImEvent {
        ImEvent {
            event_id: event_id.to_string(),
            provider_id: "weixin-main".to_string(),
            provider_type: super::super::types::ImProviderType::Weixin,
            event_type: "message.receive".to_string(),
            source: super::super::types::ImEventSource::default(),
            message: None,
            received_at: 1,
            raw_digest: None,
        }
    }

    fn legacy_credential_event(event_id: &str) -> ImEvent {
        let mut event = event(event_id);
        event.raw_digest = Some("raw-secret".to_string());
        event.message = Some(super::super::types::ImEventMessage {
            raw_content: Some(serde_json::json!({"credential": "raw-content-secret"})),
            images: vec![super::super::types::ImImageAttachment {
                file_key: "image-key".to_string(),
                data_base64: Some("image-base64-secret".to_string()),
                download_url: Some("https://example.invalid/image-secret".to_string()),
                encrypted_query_param: Some("image-query-secret".to_string()),
                aes_key: Some("image-aes-secret".to_string()),
                ..Default::default()
            }],
            files: vec![super::super::types::ImFileAttachment {
                file_key: "file-key".to_string(),
                data_base64: Some("file-base64-secret".to_string()),
                download_url: Some("https://example.invalid/file-secret".to_string()),
                encrypted_query_param: Some("file-query-secret".to_string()),
                aes_key: Some("file-aes-secret".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        });
        event
    }

    fn write_store_data(temp: &tempfile::TempDir, events: Vec<ImEvent>) -> PathBuf {
        let admin_dir = temp.path().join("admin");
        std::fs::create_dir_all(&admin_dir).unwrap();
        let file_path = admin_dir.join(STORE_FILENAME);
        std::fs::write(
            &file_path,
            serde_json::to_vec_pretty(&StoreData {
                version: STORE_VERSION,
                events,
            })
            .unwrap(),
        )
        .unwrap();
        file_path
    }

    #[test]
    fn add_is_idempotent_for_the_same_provider_event() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImEventStore::new(temp.path());

        store.add(event("event-1")).unwrap();
        store.add(event("event-1")).unwrap();

        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn failed_add_rolls_back_memory_before_retry() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImEventStore::new(temp.path());
        let blocked_path = temp.path().join("admin").join(STORE_FILENAME);
        std::fs::create_dir_all(&blocked_path).unwrap();

        assert!(store.add(event("event-retry")).is_err());
        assert!(store.list().is_empty());

        std::fs::remove_dir(&blocked_path).unwrap();
        store.add(event("event-retry")).unwrap();
        assert_eq!(store.list().len(), 1);
    }

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

    #[test]
    fn loading_legacy_weixin_history_redacts_and_rewrites_media_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = write_store_data(&temp, vec![legacy_credential_event("legacy-secret")]);

        let store = ImEventStore::new(temp.path());
        let events = store.list();
        let event = &events[0];
        let message = event.message.as_ref().unwrap();
        assert!(event.raw_digest.is_none());
        assert!(message.raw_content.is_none());
        assert!(message.images[0].data_base64.is_none());
        assert!(message.images[0].download_url.is_none());
        assert!(message.images[0].encrypted_query_param.is_none());
        assert!(message.images[0].aes_key.is_none());
        assert!(message.files[0].data_base64.is_none());
        assert!(message.files[0].download_url.is_none());
        assert!(message.files[0].encrypted_query_param.is_none());
        assert!(message.files[0].aes_key.is_none());

        let persisted = std::fs::read_to_string(file_path).unwrap();
        for credential in [
            "raw-secret",
            "raw-content-secret",
            "image-base64-secret",
            "image-query-secret",
            "image-aes-secret",
            "file-base64-secret",
            "file-query-secret",
            "file-aes-secret",
        ] {
            assert!(!persisted.contains(credential));
        }
    }

    #[cfg(unix)]
    #[test]
    fn loading_legacy_weixin_history_keeps_redacted_memory_when_rewrite_fails() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let file_path = write_store_data(&temp, vec![legacy_credential_event("legacy-readonly")]);
        let original_mode = std::fs::metadata(&file_path).unwrap().permissions().mode();
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o400)).unwrap();

        let store = ImEventStore::new(temp.path());
        assert!(store.list()[0].raw_digest.is_none());
        assert!(
            std::fs::read_to_string(&file_path)
                .unwrap()
                .contains("raw-secret"),
            "the read-only legacy file should remain unchanged"
        );

        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(original_mode))
            .unwrap();
    }
}
