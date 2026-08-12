use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use bifrost_core::{BifrostError, Result};
use bifrost_storage::LocalSecretKey;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_weixin_sync_cursors.json";
const MAX_CURSOR_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    entries: BTreeMap<String, String>,
}

pub(super) struct WeixinSyncCursorStore {
    path: PathBuf,
    key: LocalSecretKey,
    data: RwLock<StoreData>,
}

impl WeixinSyncCursorStore {
    pub(super) fn new(data_dir: &Path) -> Result<Self> {
        let key = LocalSecretKey::for_data_dir(data_dir)?;
        let path = data_dir.join("admin").join(STORE_FILENAME);
        let data = Self::load(&path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            entries: BTreeMap::new(),
        });
        Ok(Self {
            path,
            key,
            data: RwLock::new(data),
        })
    }

    pub(super) fn get(&self, provider_id: &str, account_id: &str) -> Option<String> {
        let encoded = self
            .data
            .read()
            .entries
            .get(&entry_key(provider_id, account_id))?
            .clone();
        let cursor = self.key.decrypt_string(&encoded).ok()?;
        (cursor != encoded && cursor.len() <= MAX_CURSOR_BYTES).then_some(cursor)
    }

    pub(super) fn put(&self, provider_id: &str, account_id: &str, cursor: &str) -> Result<()> {
        if cursor.len() > MAX_CURSOR_BYTES {
            return Err(BifrostError::Config(format!(
                "weixin sync cursor exceeds {MAX_CURSOR_BYTES} byte limit"
            )));
        }
        let mut data = self.data.write();
        let mut next = data.clone();
        next.version = STORE_VERSION;
        next.entries.insert(
            entry_key(provider_id, account_id),
            self.key.encrypt_string(cursor)?,
        );
        self.save(&next)?;
        *data = next;
        Ok(())
    }

    fn save(&self, data: &StoreData) -> Result<()> {
        let parent = self.path.parent().ok_or_else(|| {
            BifrostError::Config("weixin sync cursor store path has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create weixin sync cursor directory {}: {error}",
                parent.display()
            )))
        })?;
        let bytes = serde_json::to_vec_pretty(data).map_err(|error| {
            BifrostError::Config(format!("serialize weixin sync cursor store: {error}"))
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "open temporary weixin sync cursor store in {}: {error}",
                parent.display()
            )))
        })?;
        temporary.write_all(&bytes).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "write weixin sync cursor store {}: {error}",
                temporary.path().display()
            )))
        })?;
        temporary.as_file().sync_all().map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "sync weixin sync cursor store {}: {error}",
                temporary.path().display()
            )))
        })?;
        harden_private_file(temporary.path())?;
        temporary.persist(&self.path).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "replace weixin sync cursor store {}: {error}",
                self.path.display(),
                error.error
            )))
        })?;
        harden_private_file(&self.path)?;
        sync_directory(parent)
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

fn entry_key(provider_id: &str, account_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider_id.trim().as_bytes());
    hasher.update([0]);
    hasher.update(account_id.trim().as_bytes());
    format!("{:x}", hasher.finalize())
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
                "sync weixin cursor directory {}: {error}",
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
    fn cursor_is_encrypted_isolated_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = WeixinSyncCursorStore::new(dir.path()).unwrap();
        store.put("provider-a", "account", "cursor-a").unwrap();
        store.put("provider-b", "account", "cursor-b").unwrap();
        let disk = std::fs::read_to_string(&store.path).unwrap();
        assert!(!disk.contains("cursor-a"));
        assert!(!disk.contains("provider-a"));

        let restarted = WeixinSyncCursorStore::new(dir.path()).unwrap();
        assert_eq!(
            restarted.get("provider-a", "account").as_deref(),
            Some("cursor-a")
        );
        assert_eq!(
            restarted.get("provider-b", "account").as_deref(),
            Some("cursor-b")
        );
    }

    #[test]
    fn oversized_cursor_is_rejected_without_publishing() {
        let dir = tempfile::tempdir().unwrap();
        let store = WeixinSyncCursorStore::new(dir.path()).unwrap();
        let oversized = "x".repeat(MAX_CURSOR_BYTES + 1);
        assert!(store.put("provider", "account", &oversized).is_err());
        assert!(store.get("provider", "account").is_none());
    }

    #[test]
    fn corrupt_oversized_and_wrong_version_stores_start_empty() {
        for bytes in [
            b"not json".to_vec(),
            serde_json::to_vec(&StoreData {
                version: STORE_VERSION + 1,
                entries: BTreeMap::new(),
            })
            .unwrap(),
            vec![b'x'; 16 * 1024 * 1024 + 1],
        ] {
            let dir = tempfile::tempdir().unwrap();
            let admin = dir.path().join("admin");
            std::fs::create_dir_all(&admin).unwrap();
            std::fs::write(admin.join(STORE_FILENAME), bytes).unwrap();
            let store = WeixinSyncCursorStore::new(dir.path()).unwrap();
            assert!(store.get("provider", "account").is_none());
        }
    }

    #[test]
    fn save_reports_parent_replace_and_permission_failures() {
        let no_parent_dir = tempfile::tempdir().unwrap();
        let mut no_parent = WeixinSyncCursorStore::new(no_parent_dir.path()).unwrap();
        no_parent.path = PathBuf::from("/");
        assert!(no_parent.save(&StoreData::default()).is_err());

        let blocked_parent_dir = tempfile::tempdir().unwrap();
        let blocked_parent = WeixinSyncCursorStore::new(blocked_parent_dir.path()).unwrap();
        std::fs::write(blocked_parent_dir.path().join("admin"), b"file").unwrap();
        assert!(blocked_parent
            .put("provider", "account", "cursor")
            .unwrap_err()
            .to_string()
            .contains("create weixin sync cursor directory"));

        let blocked_rename_dir = tempfile::tempdir().unwrap();
        let blocked_rename = WeixinSyncCursorStore::new(blocked_rename_dir.path()).unwrap();
        std::fs::create_dir_all(blocked_rename.path.parent().unwrap()).unwrap();
        std::fs::create_dir(&blocked_rename.path).unwrap();
        assert!(blocked_rename
            .put("provider", "account", "cursor")
            .unwrap_err()
            .to_string()
            .contains("replace weixin sync cursor store"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_hardening_and_directory_sync_report_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(harden_private_file(&missing).is_err());
        assert!(sync_directory(&missing).is_err());
    }

    #[cfg(not(unix))]
    #[test]
    fn non_unix_file_hardening_and_directory_sync_are_noops() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(harden_private_file(&missing).is_ok());
        assert!(sync_directory(&missing).is_ok());
    }
}
