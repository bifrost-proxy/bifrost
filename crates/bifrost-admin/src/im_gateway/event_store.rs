use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{error, warn};

use bifrost_core::{BifrostError, Result};
use bifrost_storage::LocalSecretKey;

use super::types::ImEvent;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_events.json";
const PENDING_STORE_FILENAME: &str = "im_gateway_pending_events.json";
const MAX_EVENTS: usize = 1000;
const MAX_PENDING_EVENTS: usize = 1000;
const MAX_PENDING_STORE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    events: Vec<ImEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingStoreData {
    version: u32,
    entries: BTreeMap<String, String>,
}

pub struct ImEventStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
    pending_path: PathBuf,
    pending_key: Option<LocalSecretKey>,
    pending: RwLock<PendingStoreData>,
}

impl ImEventStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let pending_path = admin_dir.join(PENDING_STORE_FILENAME);
        let mut data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            events: Vec::new(),
        });
        let pending_key = LocalSecretKey::for_data_dir(data_dir)
            .map_err(|error| {
                warn!(error = %error, "durable IM pending-event encryption is unavailable");
                error
            })
            .ok();
        let pending =
            Self::load_pending_from_disk(&pending_path).unwrap_or_else(|| PendingStoreData {
                version: STORE_VERSION,
                entries: BTreeMap::new(),
            });
        let mut removed_legacy_credentials = false;
        for event in &mut data.events {
            removed_legacy_credentials |= redact_event_for_history_in_place(event);
        }
        let store = Self {
            file_path,
            data: RwLock::new(data),
            pending_path,
            pending_key,
            pending: RwLock::new(pending),
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

    /// Persist a full-fidelity inbound event before the provider commits its
    /// remote cursor. Entries are encrypted because Weixin media events can
    /// contain short-lived signed download credentials that history redacts.
    pub fn add_pending(&self, event: &ImEvent) -> Result<()> {
        let key = self.pending_key.as_ref().ok_or_else(|| {
            BifrostError::Config("durable IM pending-event encryption is unavailable".to_string())
        })?;
        let entry_key = pending_event_key(event)?;
        let mut pending = self.pending.write();
        if pending.entries.contains_key(&entry_key) {
            return Ok(());
        }
        if pending.entries.len() >= MAX_PENDING_EVENTS {
            return Err(BifrostError::Config(format!(
                "durable IM pending-event queue reached its {MAX_PENDING_EVENTS} event limit"
            )));
        }
        let plaintext = serde_json::to_string(event).map_err(|error| {
            BifrostError::Config(format!("serialize pending IM event: {error}"))
        })?;
        let encrypted = key.encrypt_string(&plaintext)?;
        let mut next = pending.clone();
        next.version = STORE_VERSION;
        next.entries.insert(entry_key, encrypted);
        self.save_pending_locked(&next)?;
        *pending = next;
        Ok(())
    }

    /// Return pending events for one provider in their original arrival order.
    /// Invalid or undecryptable entries stay on disk and fail closed rather
    /// than being silently acknowledged.
    pub fn pending_by_provider(&self, provider_id: &str) -> Vec<ImEvent> {
        let Some(key) = self.pending_key.as_ref() else {
            return Vec::new();
        };
        let mut events = self
            .pending
            .read()
            .entries
            .values()
            .filter_map(|encoded| {
                let plaintext = key.decrypt_string(encoded).ok()?;
                if plaintext == *encoded {
                    return None;
                }
                serde_json::from_str::<ImEvent>(&plaintext).ok()
            })
            .filter(|event| event.provider_id == provider_id)
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.received_at);
        events
    }

    pub fn complete_pending(&self, event: &ImEvent) -> Result<()> {
        let entry_key = pending_event_key(event)?;
        let mut pending = self.pending.write();
        if !pending.entries.contains_key(&entry_key) {
            return Ok(());
        }
        let mut next = pending.clone();
        next.entries.remove(&entry_key);
        self.save_pending_locked(&next)?;
        *pending = next;
        Ok(())
    }

    pub fn pending_completion(self: &Arc<Self>, event: &ImEvent) -> PendingEventCompletion {
        PendingEventCompletion {
            store: Arc::clone(self),
            event: event.clone(),
            armed: true,
        }
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

    fn save_pending_locked(&self, data: &PendingStoreData) -> Result<()> {
        let parent = self.pending_path.parent().ok_or_else(|| {
            BifrostError::Config("pending IM event store path has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create pending IM event directory {}: {error}",
                parent.display()
            )))
        })?;
        let bytes = serde_json::to_vec_pretty(data).map_err(|error| {
            BifrostError::Config(format!("serialize pending IM event store: {error}"))
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        harden_private_file(temporary.path())?;
        temporary.persist(&self.pending_path).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "replace pending IM event store {}: {}",
                self.pending_path.display(),
                error.error
            )))
        })?;
        harden_private_file(&self.pending_path)?;
        sync_directory(parent)
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

    fn load_pending_from_disk(file_path: &Path) -> Option<PendingStoreData> {
        if !file_path.exists() {
            return None;
        }
        if std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0) > MAX_PENDING_STORE_BYTES {
            return None;
        }
        let bytes = std::fs::read(file_path).ok()?;
        let data = serde_json::from_slice::<PendingStoreData>(&bytes).ok()?;
        (data.version == STORE_VERSION).then_some(data)
    }
}

pub struct PendingEventCompletion {
    store: Arc<ImEventStore>,
    event: ImEvent,
    armed: bool,
}

impl PendingEventCompletion {
    pub fn complete(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        if let Err(error) = self.store.complete_pending(&self.event) {
            error!(
                provider_id = %self.event.provider_id,
                event_id = %self.event.event_id,
                error = %error,
                "failed to acknowledge durable pending IM event"
            );
        }
    }

    /// Transfer completion responsibility to another processing context.
    pub fn defer(&mut self) {
        self.armed = false;
    }
}

fn pending_event_key(event: &ImEvent) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(event.provider_id.trim().as_bytes());
    hasher.update([0]);
    if event.event_id.trim().is_empty() {
        hasher.update(serde_json::to_vec(event).map_err(|error| {
            BifrostError::Config(format!("serialize pending IM event identity: {error}"))
        })?);
    } else {
        hasher.update(event.event_id.trim().as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn harden_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!(
            "chmod 0600 {}: {error}",
            path.display()
        )))
    })
}

#[cfg(not(unix))]
fn harden_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "sync pending IM event directory {}: {error}",
                path.display()
            )))
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
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
    fn pending_events_are_encrypted_replayed_and_completed() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ImEventStore::new(temp.path()));
        let mut pending_event = legacy_credential_event("pending-1");
        pending_event.provider_id = "weixin-pending".to_string();

        store.add_pending(&pending_event).unwrap();
        let disk = std::fs::read_to_string(&store.pending_path).unwrap();
        assert!(!disk.contains("image-query-secret"));
        assert!(!disk.contains("pending-1"));

        let restarted = Arc::new(ImEventStore::new(temp.path()));
        let replayed = restarted.pending_by_provider("weixin-pending");
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].event_id, "pending-1");
        assert_eq!(
            replayed[0].message.as_ref().unwrap().images[0]
                .encrypted_query_param
                .as_deref(),
            Some("image-query-secret")
        );

        let mut completion = restarted.pending_completion(&replayed[0]);
        completion.complete();
        assert!(restarted.pending_by_provider("weixin-pending").is_empty());
        restarted.complete_pending(&replayed[0]).unwrap();

        restarted.add_pending(&pending_event).unwrap();
        let mut deferred = restarted.pending_completion(&pending_event);
        deferred.defer();
        drop(deferred);
        assert_eq!(restarted.pending_by_provider("weixin-pending").len(), 1);
        restarted.complete_pending(&pending_event).unwrap();
    }

    #[test]
    fn pending_event_store_fails_closed_for_missing_key_and_capacity() {
        let missing_key_dir = tempfile::tempdir().unwrap();
        let mut missing_key = ImEventStore::new(missing_key_dir.path());
        missing_key.pending_key = None;
        assert!(missing_key.add_pending(&event("missing-key")).is_err());
        assert!(missing_key.pending_by_provider("weixin-main").is_empty());

        let full_dir = tempfile::tempdir().unwrap();
        let full = ImEventStore::new(full_dir.path());
        full.pending.write().entries.extend(
            (0..MAX_PENDING_EVENTS).map(|index| (format!("entry-{index}"), "invalid".to_string())),
        );
        assert!(full.add_pending(&event("overflow")).is_err());
    }

    #[test]
    fn pending_event_store_reports_path_failures_without_publishing_memory() {
        let temp = tempfile::tempdir().unwrap();
        let blocked_parent = temp.path().join("blocked-parent");
        std::fs::write(&blocked_parent, b"file").unwrap();
        let mut store = ImEventStore::new(temp.path());
        store.pending_path = blocked_parent.join(PENDING_STORE_FILENAME);

        assert!(store.add_pending(&event("blocked-parent")).is_err());
        assert!(store.pending.read().entries.is_empty());

        let blocked_target = temp.path().join("blocked-target");
        std::fs::create_dir_all(&blocked_target).unwrap();
        store.pending_path = blocked_target;
        assert!(store.add_pending(&event("blocked-target")).is_err());
        assert!(store.pending.read().entries.is_empty());
    }

    #[test]
    fn pending_event_loader_rejects_missing_oversized_invalid_and_wrong_version_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(PENDING_STORE_FILENAME);
        assert!(ImEventStore::load_pending_from_disk(&path).is_none());

        let oversized = std::fs::File::create(&path).unwrap();
        oversized.set_len(MAX_PENDING_STORE_BYTES + 1).unwrap();
        assert!(ImEventStore::load_pending_from_disk(&path).is_none());

        std::fs::write(&path, b"not-json").unwrap();
        assert!(ImEventStore::load_pending_from_disk(&path).is_none());

        std::fs::write(
            &path,
            serde_json::to_vec(&PendingStoreData {
                version: STORE_VERSION + 1,
                entries: BTreeMap::new(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(ImEventStore::load_pending_from_disk(&path).is_none());
    }

    #[test]
    fn pending_event_identity_supports_missing_protocol_ids_and_skips_plaintext_entries() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImEventStore::new(temp.path());
        let mut without_id = event("");
        without_id.message = Some(super::super::types::ImEventMessage {
            text: "stable identity".to_string(),
            ..Default::default()
        });
        store.add_pending(&without_id).unwrap();
        assert_eq!(store.pending_by_provider("weixin-main").len(), 1);

        store
            .pending
            .write()
            .entries
            .insert("plaintext".to_string(), "not-encrypted".to_string());
        assert_eq!(store.pending_by_provider("weixin-main").len(), 1);
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
