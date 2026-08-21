use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use bifrost_core::{BifrostError, Result};

pub const BOT_JOINED_EVENT_TYPE: &str = "conversation.bot_joined";
pub const REQUIRED_GROUP_MESSAGE_SCOPE: &str = "im:message.group_msg";
pub const NOTICE_VERSION: u32 = 1;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_feishu_group_permissions.json";
const LEGACY_TRIGGER: &str = "first-visible-message";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Granted,
    SendingNotice,
    NoticePending,
    NoticeSent,
}

impl CheckStatus {
    fn is_complete(self) -> bool {
        matches!(self, Self::Granted | Self::NoticeSent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckRecord {
    provider_id: String,
    chat_id: String,
    trigger_id: String,
    required_scope: String,
    notice_version: u32,
    status: CheckStatus,
    updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

impl CheckRecord {
    fn new(provider_id: &str, chat_id: &str, trigger_id: &str, status: CheckStatus) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            chat_id: chat_id.to_string(),
            trigger_id: trigger_id.to_string(),
            required_scope: REQUIRED_GROUP_MESSAGE_SCOPE.to_string(),
            notice_version: NOTICE_VERSION,
            status,
            updated_at_ms: now_ms(),
            message_id: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    records: BTreeMap<String, CheckRecord>,
}

pub struct FeishuGroupPermissionStore {
    path: PathBuf,
    data: RwLock<StoreData>,
    load_error: Option<String>,
}

impl FeishuGroupPermissionStore {
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("admin").join(STORE_FILENAME);
        let loaded = Self::load(&path);
        let load_error = (path.exists() && loaded.is_none()).then(|| {
            format!(
                "Feishu group permission store {} is corrupt or has an unsupported version",
                path.display()
            )
        });
        Self {
            path,
            data: RwLock::new(loaded.unwrap_or_else(|| StoreData {
                version: STORE_VERSION,
                records: BTreeMap::new(),
            })),
            load_error,
        }
    }

    pub fn join_check_key(&self, provider_id: &str, chat_id: &str, event_id: &str) -> String {
        check_key(provider_id, chat_id, event_id)
    }

    pub fn legacy_check_key(&self, provider_id: &str, chat_id: &str) -> String {
        check_key(provider_id, chat_id, LEGACY_TRIGGER)
    }

    pub fn is_complete(&self, key: &str) -> Result<bool> {
        self.ensure_readable()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        Ok(data
            .records
            .get(key)
            .is_some_and(|record| record.status.is_complete()))
    }

    pub fn mark_granted(
        &self,
        key: &str,
        provider_id: &str,
        chat_id: &str,
        trigger_id: &str,
    ) -> Result<()> {
        self.upsert(
            key,
            CheckRecord::new(provider_id, chat_id, trigger_id, CheckStatus::Granted),
        )
    }

    pub fn mark_sending_notice(
        &self,
        key: &str,
        provider_id: &str,
        chat_id: &str,
        trigger_id: &str,
    ) -> Result<String> {
        self.upsert(
            key,
            CheckRecord::new(provider_id, chat_id, trigger_id, CheckStatus::SendingNotice),
        )?;
        Ok(stable_notice_uuid(key))
    }

    pub fn mark_notice_sent(&self, key: &str, message_id: Option<&str>) -> Result<()> {
        self.update(key, |record| {
            record.status = CheckStatus::NoticeSent;
            record.message_id = message_id.map(str::to_string);
            record.last_error = None;
        })
    }

    pub fn mark_notice_pending(&self, key: &str, error: &str) -> Result<()> {
        self.update(key, |record| {
            record.status = CheckStatus::NoticePending;
            record.last_error = Some(error.to_string());
        })
    }

    fn upsert(&self, key: &str, record: CheckRecord) -> Result<()> {
        self.ensure_readable()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        let mut next = data.clone();
        next.records.insert(key.to_string(), record);
        self.save_locked(&next)?;
        *data = next;
        Ok(())
    }

    fn update(&self, key: &str, apply: impl FnOnce(&mut CheckRecord)) -> Result<()> {
        self.ensure_readable()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        let mut next = data.clone();
        let record = next.records.get_mut(key).ok_or_else(|| {
            BifrostError::Config(format!(
                "Feishu group permission check record disappeared: {key}"
            ))
        })?;
        apply(record);
        record.updated_at_ms = now_ms();
        self.save_locked(&next)?;
        *data = next;
        Ok(())
    }

    fn ensure_readable(&self) -> Result<()> {
        match self.load_error.as_deref() {
            Some(error) => Err(BifrostError::Config(error.to_string())),
            None => Ok(()),
        }
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
            .ok_or_else(|| BifrostError::Config("permission store path has no parent".into()))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create Feishu permission store directory {}: {error}",
                parent.display()
            )))
        })?;
        let bytes = serde_json::to_vec_pretty(data).map_err(|error| {
            BifrostError::Config(format!("serialize Feishu permission store: {error}"))
        })?;
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
                "open Feishu permission store {}: {error}",
                temporary.display()
            )))
        })?;
        file.write_all(&bytes).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "write Feishu permission store {}: {error}",
                temporary.display()
            )))
        })?;
        file.sync_all().map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "sync Feishu permission store {}: {error}",
                temporary.display()
            )))
        })?;
        harden_private_file(&temporary)?;
        std::fs::rename(&temporary, &self.path).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "replace Feishu permission store {}: {error}",
                self.path.display()
            )))
        })?;
        harden_private_file(&self.path)
    }

    fn load(path: &Path) -> Option<StoreData> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() > 16 * 1024 * 1024 {
            return None;
        }
        let data: StoreData = serde_json::from_slice(&bytes).ok()?;
        (data.version == STORE_VERSION).then_some(data)
    }
}

pub fn authorization_url(app_id: &str) -> String {
    format!(
        "https://open.larkoffice.com/app/{}/auth",
        urlencoding::encode(app_id.trim())
    )
}

pub fn missing_permission_notice(app_id: &str) -> String {
    let url = authorization_url(app_id);
    format!(
        "⚠️ **群消息权限尚未开通**\n\n当前机器人无法读取群内全部消息。若要使用无需 @ 的群聊上下文等完整功能，请由应用管理员申请权限，并由企业管理员审批。\n\n[前往飞书开放平台申请权限]({url})\n\n需要申请的权限：`{REQUIRED_GROUP_MESSAGE_SCOPE}`"
    )
}

fn check_key(provider_id: &str, chat_id: &str, trigger_id: &str) -> String {
    format!(
        "{}:{}:{}:{}:v{}",
        provider_id.trim(),
        chat_id.trim(),
        trigger_id.trim(),
        REQUIRED_GROUP_MESSAGE_SCOPE,
        NOTICE_VERSION
    )
}

fn stable_notice_uuid(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut encoded = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    format!("bifrost-perm-{encoded}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_notice_contains_direct_app_link_and_scope() {
        let notice = missing_permission_notice("cli_test");
        assert!(notice.contains("https://open.larkoffice.com/app/cli_test/auth"));
        assert!(notice.contains(REQUIRED_GROUP_MESSAGE_SCOPE));
        assert!(notice.contains("应用管理员"));
        assert!(notice.contains("企业管理员"));
    }

    #[test]
    fn store_persists_completion_and_allows_rejoin_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeishuGroupPermissionStore::new(dir.path());
        let first = store.join_check_key("provider", "chat", "event-1");
        let second = store.join_check_key("provider", "chat", "event-2");
        assert!(!store.is_complete(&first).unwrap());

        store
            .mark_granted(&first, "provider", "chat", "event-1")
            .unwrap();
        assert!(store.is_complete(&first).unwrap());
        assert!(!store.is_complete(&second).unwrap());

        let restarted = FeishuGroupPermissionStore::new(dir.path());
        assert!(restarted.is_complete(&first).unwrap());
        assert!(!restarted.is_complete(&second).unwrap());
    }

    #[test]
    fn notice_retry_uses_stable_uuid_until_sent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeishuGroupPermissionStore::new(dir.path());
        let key = store.legacy_check_key("provider", "chat");
        let first = store
            .mark_sending_notice(&key, "provider", "chat", LEGACY_TRIGGER)
            .unwrap();
        store.mark_notice_pending(&key, "temporary").unwrap();
        let retry = store
            .mark_sending_notice(&key, "provider", "chat", LEGACY_TRIGGER)
            .unwrap();
        assert_eq!(first, retry);
        assert!(!store.is_complete(&key).unwrap());

        store.mark_notice_sent(&key, Some("om_1")).unwrap();
        assert!(store.is_complete(&key).unwrap());
    }

    #[test]
    fn corrupt_store_and_missing_records_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let admin = dir.path().join("admin");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join(STORE_FILENAME), b"not-json").unwrap();

        let corrupt = FeishuGroupPermissionStore::new(dir.path());
        let error = corrupt.is_complete("missing").unwrap_err().to_string();
        assert!(error.contains("corrupt or has an unsupported version"));

        let clean_dir = tempfile::tempdir().unwrap();
        let clean = FeishuGroupPermissionStore::new(clean_dir.path());
        assert!(clean.mark_notice_sent("missing", None).is_err());
    }

    #[test]
    fn store_reports_parent_creation_failure_and_rejects_oversized_state() {
        let blocked_dir = tempfile::tempdir().unwrap();
        std::fs::write(blocked_dir.path().join("admin"), b"not a directory").unwrap();
        let blocked = FeishuGroupPermissionStore::new(blocked_dir.path());
        let key = blocked.join_check_key("provider", "chat", "event");
        assert!(blocked
            .mark_granted(&key, "provider", "chat", "event")
            .unwrap_err()
            .to_string()
            .contains("create Feishu permission store directory"));

        let oversized_dir = tempfile::tempdir().unwrap();
        let admin = oversized_dir.path().join("admin");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join(STORE_FILENAME), vec![b' '; 16 * 1024 * 1024 + 1]).unwrap();
        assert!(FeishuGroupPermissionStore::new(oversized_dir.path())
            .is_complete("missing")
            .is_err());
    }

    #[test]
    fn store_reports_temporary_file_open_failure() {
        let dir = tempfile::tempdir().unwrap();
        let admin = dir.path().join("admin");
        std::fs::create_dir_all(admin.join("im_gateway_feishu_group_permissions.json.tmp"))
            .unwrap();
        let store = FeishuGroupPermissionStore::new(dir.path());
        let key = store.join_check_key("provider", "chat", "event");

        assert!(store
            .mark_granted(&key, "provider", "chat", "event")
            .unwrap_err()
            .to_string()
            .contains("open Feishu permission store"));
    }
}
