use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::{ImMessageLog, ImMessageReference, MessageDirection, MessageStatus};

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_message_logs.json";
const MAX_MESSAGES: usize = 5000;
const MAX_CONTENT_CHARS: usize = 32_000;
const MAX_FULL_CONTENT_MESSAGES: usize = 256;
const REFERENCE_TIME_WINDOW_MS: u64 = 60_000;
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
    pub fn add(&self, mut log: ImMessageLog) -> Result<()> {
        if let Some(content) = log.content.as_deref() {
            log.content = Some(bifrost_core::text::truncate_chars(
                content,
                MAX_CONTENT_CHARS,
            ));
        }
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        data.messages.push(log);
        self.trim_locked(&mut data);
        self.save_locked(&data)
    }

    pub fn resolve_reference_text(
        &self,
        provider_id: &str,
        peer_id: Option<&str>,
        current_message_id: Option<&str>,
        reference: &ImMessageReference,
    ) -> Option<String> {
        if let Some(text) = reference
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return Some(text.to_string());
        }

        self.refresh_from_disk();
        let data = self.data.read();
        let matches_scope = |message: &ImMessageLog| {
            message.provider_id == provider_id
                && message.status == MessageStatus::Success
                && Self::stored_text(message).is_some()
                && current_message_id.is_none_or(|current_message_id| {
                    message.message_id.as_deref() != Some(current_message_id)
                })
                && Self::matches_peer(message, peer_id)
        };

        if let Some(message_id) = reference
            .message_id
            .as_deref()
            .map(str::trim)
            .filter(|message_id| !message_id.is_empty())
        {
            if let Some(message) = data
                .messages
                .iter()
                .rev()
                .filter(|message| matches_scope(message))
                .find(|message| message.message_id.as_deref() == Some(message_id))
            {
                return Self::stored_text(message).map(str::to_string);
            }
        }

        let created_at_ms = reference.created_at_ms?;
        data.messages
            .iter()
            .rev()
            .filter(|message| matches_scope(message))
            .filter_map(|message| {
                let delta = message.timestamp.abs_diff(created_at_ms);
                (delta <= REFERENCE_TIME_WINDOW_MS).then_some((delta, message))
            })
            .min_by_key(|(delta, _)| *delta)
            .and_then(|(_, message)| Self::stored_text(message))
            .map(str::to_string)
    }

    fn matches_peer(message: &ImMessageLog, peer_id: Option<&str>) -> bool {
        let Some(peer_id) = peer_id.map(str::trim).filter(|peer_id| !peer_id.is_empty()) else {
            return true;
        };
        match message.direction {
            MessageDirection::Inbound => message.sender_open_id.as_deref() == Some(peer_id),
            MessageDirection::Outbound => message.target_id.as_deref() == Some(peer_id),
        }
    }

    fn stored_text(message: &ImMessageLog) -> Option<&str> {
        message
            .content
            .as_deref()
            .or(message.content_preview.as_deref())
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }

    /// List all message logs, newest first.
    pub fn list(&self) -> Vec<ImMessageLog> {
        self.refresh_from_disk();
        let data = self.data.read();
        let mut msgs = data.messages.clone();
        msgs.reverse();
        msgs
    }

    /// List message logs for a specific provider, newest first.
    pub fn list_by_provider(&self, provider_id: &str) -> Vec<ImMessageLog> {
        self.refresh_from_disk();
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
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        data.messages.retain(|m| m.provider_id != provider_id);
        self.save_locked(&data)
    }

    /// Clear all message logs.
    pub fn clear_all(&self) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
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

        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
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
        let preview_only_count = data
            .messages
            .len()
            .saturating_sub(MAX_FULL_CONTENT_MESSAGES);
        for message in &mut data.messages[..preview_only_count] {
            message.content = None;
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
        let content = serde_json::to_vec_pretty(data)
            .map_err(|e| BifrostError::Config(format!("serialize message log store: {e}")))?;
        let parent = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "create temporary message log store for {}: {e}",
                self.file_path.display()
            )))
        })?;
        temporary.write_all(&content)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.file_path).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "atomically replace {}: {}",
                self.file_path.display(),
                error.error
            )))
        })?;
        Ok(())
    }

    fn acquire_write_lock(&self) -> Result<File> {
        let lock_path = self.file_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn refresh_from_disk(&self) {
        if let Some(data) = Self::load_from_disk(&self.file_path) {
            *self.data.write() = data;
        }
    }

    fn refresh_locked(&self, data: &mut StoreData) {
        if let Some(latest) = Self::load_from_disk(&self.file_path) {
            *data = latest;
        }
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
                tracing::warn!(
                    path = %file_path.display(),
                    "preserving unreadable IM Gateway message log store"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(
        id: &str,
        provider_id: &str,
        direction: MessageDirection,
        peer_id: &str,
        message_id: Option<&str>,
        timestamp: u64,
        content: &str,
    ) -> ImMessageLog {
        ImMessageLog {
            id: id.to_string(),
            provider_id: provider_id.to_string(),
            direction,
            status: MessageStatus::Success,
            timestamp,
            target_id: (direction == MessageDirection::Outbound).then(|| peer_id.to_string()),
            target_name: None,
            message_id: message_id.map(str::to_string),
            msg_type: Some("text".to_string()),
            content_preview: Some(content.chars().take(16).collect()),
            content: Some(content.to_string()),
            trigger: Some("test".to_string()),
            error: None,
            sender_open_id: (direction == MessageDirection::Inbound).then(|| peer_id.to_string()),
            event_id: None,
            reaction_added: None,
        }
    }

    #[test]
    fn resolves_reference_by_message_id_without_crossing_scope() {
        let temp = tempfile::tempdir().expect("temp message store");
        let store = ImMessageLogStore::new(temp.path());
        store
            .add(message(
                "other-provider",
                "provider-b",
                MessageDirection::Outbound,
                "peer-a",
                Some("server-msg-1"),
                1_000,
                "wrong provider",
            ))
            .expect("add other provider message");
        store
            .add(message(
                "other-peer",
                "provider-a",
                MessageDirection::Outbound,
                "peer-b",
                Some("server-msg-1"),
                1_050,
                "wrong peer",
            ))
            .expect("add other peer message");
        store
            .add(message(
                "expected",
                "provider-a",
                MessageDirection::Outbound,
                "peer-a",
                Some("server-msg-1"),
                1_100,
                "quoted text https://example.com/article",
            ))
            .expect("add expected message");

        let resolved = store.resolve_reference_text(
            "provider-a",
            Some("peer-a"),
            None,
            &ImMessageReference {
                message_id: Some("server-msg-1".to_string()),
                created_at_ms: None,
                text: None,
            },
        );

        assert_eq!(
            resolved.as_deref(),
            Some("quoted text https://example.com/article")
        );
    }

    #[test]
    fn resolves_weixin_reference_by_nearest_timestamp_within_same_peer() {
        let temp = tempfile::tempdir().expect("temp message store");
        let store = ImMessageLogStore::new(temp.path());
        store
            .add(message(
                "wrong-peer",
                "weixin-main",
                MessageDirection::Outbound,
                "peer-b",
                None,
                10_001,
                "wrong peer",
            ))
            .expect("add wrong peer message");
        store
            .add(message(
                "expected",
                "weixin-main",
                MessageDirection::Outbound,
                "peer-a",
                None,
                10_250,
                "expected quoted reply",
            ))
            .expect("add expected message");

        let resolved = store.resolve_reference_text(
            "weixin-main",
            Some("peer-a"),
            None,
            &ImMessageReference {
                message_id: Some("unmapped-weixin-server-id".to_string()),
                created_at_ms: Some(10_000),
                text: None,
            },
        );

        assert_eq!(resolved.as_deref(), Some("expected quoted reply"));
    }

    #[test]
    fn blank_peer_filter_matches_any_peer() {
        let value = message(
            "inbound",
            "provider-a",
            MessageDirection::Inbound,
            "peer-a",
            None,
            1,
            "hello",
        );
        assert!(ImMessageLogStore::matches_peer(&value, Some("   ")));
    }

    #[test]
    fn rejects_timestamp_reference_outside_window() {
        let temp = tempfile::tempdir().expect("temp message store");
        let store = ImMessageLogStore::new(temp.path());
        store
            .add(message(
                "old",
                "weixin-main",
                MessageDirection::Inbound,
                "peer-a",
                None,
                1_000,
                "too old",
            ))
            .expect("add old message");

        let resolved = store.resolve_reference_text(
            "weixin-main",
            Some("peer-a"),
            None,
            &ImMessageReference {
                message_id: None,
                created_at_ms: Some(1_000 + REFERENCE_TIME_WINDOW_MS + 1),
                text: None,
            },
        );

        assert!(resolved.is_none());
    }

    #[test]
    fn excludes_current_message_from_timestamp_fallback() {
        let temp = tempfile::tempdir().expect("temp message store");
        let store = ImMessageLogStore::new(temp.path());
        store
            .add(message(
                "quoted",
                "weixin-main",
                MessageDirection::Outbound,
                "peer-a",
                None,
                10_250,
                "quoted bot response",
            ))
            .expect("add quoted message");
        store
            .add(message(
                "current",
                "weixin-main",
                MessageDirection::Inbound,
                "peer-a",
                Some("current-message-id"),
                10_001,
                "current user question",
            ))
            .expect("add current message");

        let resolved = store.resolve_reference_text(
            "weixin-main",
            Some("peer-a"),
            Some("current-message-id"),
            &ImMessageReference {
                message_id: Some("unmapped-weixin-server-id".to_string()),
                created_at_ms: Some(10_000),
                text: None,
            },
        );

        assert_eq!(resolved.as_deref(), Some("quoted bot response"));
    }

    #[test]
    fn retains_full_content_only_for_recent_messages() {
        let temp = tempfile::tempdir().expect("temp message store");
        let store = ImMessageLogStore::new(temp.path());
        let mut data = StoreData {
            version: STORE_VERSION,
            messages: (0..MAX_FULL_CONTENT_MESSAGES + 2)
                .map(|index| {
                    message(
                        &format!("message-{index}"),
                        "weixin-main",
                        MessageDirection::Outbound,
                        "peer-a",
                        None,
                        index as u64,
                        "full content",
                    )
                })
                .collect(),
        };

        store.trim_locked(&mut data);

        assert!(data.messages[0].content.is_none());
        assert!(data.messages[1].content.is_none());
        assert_eq!(data.messages[2].content.as_deref(), Some("full content"));
        assert_eq!(
            data.messages.last().unwrap().content.as_deref(),
            Some("full content")
        );
    }

    #[test]
    fn concurrent_store_instances_preserve_both_updates_and_invalid_evidence() {
        let temp = tempfile::tempdir().expect("temp message store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let mut threads = Vec::new();
        for index in 0..2 {
            let root = temp.path().to_path_buf();
            let barrier = barrier.clone();
            let timestamp = timestamp + index;
            threads.push(std::thread::spawn(move || {
                let store = ImMessageLogStore::new(&root);
                barrier.wait();
                store
                    .add(message(
                        &format!("concurrent-{index}"),
                        "provider",
                        MessageDirection::Inbound,
                        "peer",
                        Some(&format!("message-{index}")),
                        timestamp,
                        "content",
                    ))
                    .unwrap();
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        let ids = ImMessageLogStore::new(temp.path())
            .list()
            .into_iter()
            .map(|message| message.id)
            .collect::<std::collections::HashSet<_>>();
        assert!(ids.contains("concurrent-0"));
        assert!(ids.contains("concurrent-1"));

        let first = ImMessageLogStore::new(temp.path());
        let second = ImMessageLogStore::new(temp.path());
        assert_eq!(first.list_by_provider("provider").len(), 2);
        second.clear_by_provider("provider").unwrap();
        assert!(first.list().is_empty());
        second
            .add(message(
                "remaining",
                "other-provider",
                MessageDirection::Outbound,
                "peer",
                None,
                timestamp,
                "content",
            ))
            .unwrap();
        second.clear_all().unwrap();
        assert!(first.list().is_empty());
        second.purge_expired().unwrap();

        let store_path = temp.path().join("admin").join(STORE_FILENAME);
        std::fs::write(&store_path, b"not-json").unwrap();
        let _ = ImMessageLogStore::new(temp.path());
        assert_eq!(std::fs::read(&store_path).unwrap(), b"not-json");
    }

    #[test]
    fn atomic_persist_and_oversized_load_fail_closed() {
        let temp = tempfile::tempdir().expect("temp message store");
        let store = ImMessageLogStore::new(temp.path());
        std::fs::create_dir(&store.file_path).unwrap();
        let error = store
            .save_locked(&StoreData {
                version: STORE_VERSION,
                messages: Vec::new(),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("atomically replace"), "{error}");

        std::fs::remove_dir(&store.file_path).unwrap();
        let oversized = std::fs::File::create(&store.file_path).unwrap();
        oversized.set_len(256 * 1024 * 1024 + 1).unwrap();
        assert!(ImMessageLogStore::load_from_disk(&store.file_path).is_none());
        drop(oversized);
        std::fs::write(&store.file_path, b"{invalid-json").unwrap();
        assert!(ImMessageLogStore::load_from_disk(&store.file_path).is_none());
    }
}
