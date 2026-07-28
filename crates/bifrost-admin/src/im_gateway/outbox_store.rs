use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_outbox.json";
const MAX_RECORDS: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImOutboxRecord {
    idempotency_key: String,
    provider_id: String,
    target_id: String,
    msg_type: String,
    payload_sha256: String,
    status: String,
    attempt_count: u32,
    created_at_ms: u64,
    updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    records: BTreeMap<String, ImOutboxRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImOutboxBegin {
    Send { stable_client_id: String },
    Replay { message_id: Option<String> },
}

pub struct ImOutboxStore {
    path: PathBuf,
    data: RwLock<StoreData>,
    load_error: Option<String>,
}

impl ImOutboxStore {
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("admin").join(STORE_FILENAME);
        let loaded = Self::load(&path);
        let load_error = (path.exists() && loaded.is_none()).then(|| {
            format!(
                "IM outbox {} is corrupt or has an unsupported version; refusing replay-unsafe sends",
                path.display()
            )
        });
        let data = loaded.unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            records: BTreeMap::new(),
        });
        Self {
            path,
            data: RwLock::new(data),
            load_error,
        }
    }

    pub fn begin(
        &self,
        idempotency_key: &str,
        provider_id: &str,
        target_id: &str,
        msg_type: &str,
        payload_sha256: &str,
    ) -> Result<ImOutboxBegin> {
        if let Some(error) = self.load_error.as_deref() {
            return Err(BifrostError::Config(error.to_string()));
        }
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if let Some(record) = data.records.get(idempotency_key) {
            if record.provider_id != provider_id
                || record.target_id != target_id
                || record.msg_type != msg_type
                || record.payload_sha256 != payload_sha256
            {
                return Err(BifrostError::Config(
                    "idempotency key was already used for a different IM payload".to_string(),
                ));
            }
            if record.status == "sent" {
                return Ok(ImOutboxBegin::Replay {
                    message_id: record.message_id.clone(),
                });
            }
        }

        let mut next = data.clone();
        let now = now_ms();
        let record = next
            .records
            .entry(idempotency_key.to_string())
            .or_insert_with(|| ImOutboxRecord {
                idempotency_key: idempotency_key.to_string(),
                provider_id: provider_id.to_string(),
                target_id: target_id.to_string(),
                msg_type: msg_type.to_string(),
                payload_sha256: payload_sha256.to_string(),
                status: "pending".to_string(),
                attempt_count: 0,
                created_at_ms: now,
                updated_at_ms: now,
                message_id: None,
                last_error: None,
            });
        record.status = "sending".to_string();
        record.attempt_count = record.attempt_count.saturating_add(1);
        record.updated_at_ms = now;
        record.last_error = None;
        let stable_client_id = stable_client_id(idempotency_key);
        trim_records(&mut next.records);
        self.save_locked(&next)?;
        *data = next;
        Ok(ImOutboxBegin::Send { stable_client_id })
    }

    pub fn mark_sent(&self, idempotency_key: &str, message_id: Option<&str>) -> Result<()> {
        self.update(idempotency_key, |record| {
            record.status = "sent".to_string();
            record.message_id = message_id.map(str::to_string);
            record.last_error = None;
        })
    }

    pub fn mark_pending(&self, idempotency_key: &str, error: &str) -> Result<()> {
        self.update(idempotency_key, |record| {
            record.status = "pending".to_string();
            record.last_error = Some(error.to_string());
        })
    }

    fn update(&self, idempotency_key: &str, apply: impl FnOnce(&mut ImOutboxRecord)) -> Result<()> {
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        let mut next = data.clone();
        let record = next.records.get_mut(idempotency_key).ok_or_else(|| {
            BifrostError::Config(format!("IM outbox record disappeared: {idempotency_key}"))
        })?;
        apply(record);
        record.updated_at_ms = now_ms();
        self.save_locked(&next)?;
        *data = next;
        Ok(())
    }

    fn refresh_locked(&self, data: &mut StoreData) {
        if let Some(latest) = Self::load(&self.path) {
            *data = latest;
        }
    }

    fn save_locked(&self, data: &StoreData) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| BifrostError::Config("IM outbox path has no parent".to_string()))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create IM outbox directory {}: {error}",
                parent.display()
            )))
        })?;
        let bytes = serde_json::to_vec_pretty(data)
            .map_err(|error| BifrostError::Config(format!("serialize IM outbox: {error}")))?;
        let temporary = self.path.with_extension("json.tmp");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "open IM outbox {}: {error}",
                temporary.display()
            )))
        })?;
        file.write_all(&bytes).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "write IM outbox {}: {error}",
                temporary.display()
            )))
        })?;
        file.sync_all().map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "sync IM outbox {}: {error}",
                temporary.display()
            )))
        })?;
        harden_private_file(&temporary)?;
        std::fs::rename(&temporary, &self.path).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "replace IM outbox {}: {error}",
                self.path.display()
            )))
        })?;
        harden_private_file(&self.path)?;
        sync_directory(parent)
    }

    fn load(path: &Path) -> Option<StoreData> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() > 128 * 1024 * 1024 {
            return None;
        }
        let data: StoreData = serde_json::from_slice(&bytes).ok()?;
        (data.version == STORE_VERSION).then_some(data)
    }
}

fn stable_client_id(idempotency_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(idempotency_key.as_bytes());
    let mut encoded = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    format!("bifrost-idem-{encoded}")
}

fn trim_records(records: &mut BTreeMap<String, ImOutboxRecord>) {
    if records.len() <= MAX_RECORDS {
        return;
    }
    let mut keys = records
        .iter()
        .filter(|(_, record)| record.status == "sent")
        .map(|(key, record)| (key.clone(), record.updated_at_ms))
        .collect::<Vec<_>>();
    keys.sort_by_key(|(_, updated_at)| *updated_at);
    let remove_count = records.len() - MAX_RECORDS;
    for (key, _) in keys.into_iter().take(remove_count) {
        records.remove(&key);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
                "sync IM outbox directory {}: {error}",
                path.display()
            )))
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sent_record_replays_and_payload_mismatch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImOutboxStore::new(dir.path());
        let first = store.begin("daily-1", "weixin", "owner", "text", "hash");
        assert!(matches!(first.unwrap(), ImOutboxBegin::Send { .. }));
        store.mark_sent("daily-1", Some("message-1")).unwrap();

        assert_eq!(
            store
                .begin("daily-1", "weixin", "owner", "text", "hash")
                .unwrap(),
            ImOutboxBegin::Replay {
                message_id: Some("message-1".to_string())
            }
        );
        assert!(store
            .begin("daily-1", "weixin", "owner", "text", "other")
            .is_err());
    }

    #[test]
    fn failed_record_retries_with_same_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImOutboxStore::new(dir.path());
        let first = store
            .begin("daily-2", "weixin", "owner", "text", "hash")
            .unwrap();
        store.mark_pending("daily-2", "network").unwrap();
        let second = store
            .begin("daily-2", "weixin", "owner", "text", "hash")
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn outbox_is_private_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImOutboxStore::new(dir.path());
        store
            .begin("daily-3", "weixin", "owner", "text", "hash")
            .unwrap();
        store.mark_sent("daily-3", Some("message-3")).unwrap();

        let restarted = ImOutboxStore::new(dir.path());
        assert_eq!(
            restarted
                .begin("daily-3", "weixin", "owner", "text", "hash")
                .unwrap(),
            ImOutboxBegin::Replay {
                message_id: Some("message-3".to_string())
            }
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failed_persistence_does_not_publish_in_memory_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ImOutboxStore::new(dir.path());
        store.path = dir.path().join("blocked.json");
        std::fs::create_dir(&store.path).unwrap();

        assert!(store
            .begin("daily-4", "weixin", "owner", "text", "hash")
            .is_err());
        assert!(!store.data.read().records.contains_key("daily-4"));
    }

    #[test]
    fn corrupt_outbox_fails_closed_instead_of_forgetting_replays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("admin").join(STORE_FILENAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not-json").unwrap();

        let store = ImOutboxStore::new(dir.path());
        let error = store
            .begin("daily-5", "weixin", "owner", "text", "hash")
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing replay-unsafe sends"));
    }
}
