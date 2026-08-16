use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};
use bifrost_storage::LocalSecretKey;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_weixin_contexts.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    entries: BTreeMap<String, String>,
}

pub(super) struct WeixinContextStore {
    path: PathBuf,
    key: LocalSecretKey,
    data: RwLock<StoreData>,
}

impl WeixinContextStore {
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

    pub(super) fn get(&self, account_id: &str, user_id: &str) -> Option<String> {
        // In worker mode inbound events persist context tokens from the IM
        // runtime process while Admin sends may still use the main-process
        // provider instance. Refresh the tiny encrypted store before lookup so
        // the control plane observes worker-owned context updates.
        if self.path.exists() {
            let latest = Self::load(&self.path)?;
            *self.data.write() = latest;
        }
        let key = entry_key(account_id, user_id);
        let encoded = self.data.read().entries.get(&key)?.clone();
        let decrypted = self.key.decrypt_string(&encoded).ok()?;
        // This store has never supported plaintext values. LocalSecretKey keeps
        // malformed/legacy-looking envelopes unchanged for config compatibility,
        // so reject that fallback here instead of treating corruption as a token.
        (decrypted != encoded && !decrypted.is_empty()).then_some(decrypted)
    }

    pub(super) fn put(&self, account_id: &str, user_id: &str, token: &str) -> Result<()> {
        let mut data = self.data.write();
        let mut next = data.clone();
        next.version = STORE_VERSION;
        next.entries.insert(
            entry_key(account_id, user_id),
            self.key.encrypt_string(token)?,
        );
        self.save(&next)?;
        *data = next;
        Ok(())
    }

    fn save(&self, data: &StoreData) -> Result<()> {
        let parent = self.path.parent().ok_or_else(|| {
            BifrostError::Config("weixin context store path has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create weixin context directory {}: {error}",
                parent.display()
            )))
        })?;
        let bytes = serde_json::to_vec_pretty(data).map_err(|error| {
            BifrostError::Config(format!("serialize weixin context store: {error}"))
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
                "open weixin context store {}: {error}",
                temporary.display()
            )))
        })?;
        file.write_all(&bytes).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "write weixin context store {}: {error}",
                temporary.display()
            )))
        })?;
        file.sync_all().map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "sync weixin context store {}: {error}",
                temporary.display()
            )))
        })?;
        harden_private_file(&temporary)?;
        std::fs::rename(&temporary, &self.path).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "replace weixin context store {}: {error}",
                self.path.display()
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

fn entry_key(account_id: &str, user_id: &str) -> String {
    format!("{}\u{0}{}", account_id.trim(), user_id.trim())
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
                "sync weixin context directory {}: {error}",
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
    fn encrypted_context_survives_restart_without_plaintext_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = WeixinContextStore::new(dir.path()).unwrap();
        store.put("account", "owner", "sensitive-context").unwrap();

        let bytes = std::fs::read(&store.path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("sensitive-context"));

        let restarted = WeixinContextStore::new(dir.path()).unwrap();
        assert_eq!(
            restarted.get("account", "owner").as_deref(),
            Some("sensitive-context")
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
    fn existing_instance_observes_context_written_by_worker_instance() {
        let dir = tempfile::tempdir().unwrap();
        let main = WeixinContextStore::new(dir.path()).unwrap();
        let worker = WeixinContextStore::new(dir.path()).unwrap();

        worker.put("account", "owner", "worker-context").unwrap();

        assert_eq!(
            main.get("account", "owner").as_deref(),
            Some("worker-context")
        );
    }

    #[test]
    fn corrupt_entry_is_not_send_ready() {
        let dir = tempfile::tempdir().unwrap();
        let store = WeixinContextStore::new(dir.path()).unwrap();
        store.data.write().entries.insert(
            entry_key("account", "owner"),
            "bifrost-local-secret:{}".into(),
        );
        assert!(store.get("account", "owner").is_none());
    }

    #[test]
    fn failed_persistence_does_not_publish_context_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WeixinContextStore::new(dir.path()).unwrap();
        store.path = dir.path().join("blocked.json");
        std::fs::create_dir(&store.path).unwrap();

        assert!(store.put("account", "owner", "context").is_err());
        assert!(store.get("account", "owner").is_none());
    }

    #[test]
    fn store_filesystem_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let data = StoreData {
            version: STORE_VERSION,
            entries: BTreeMap::new(),
        };

        let mut no_parent = WeixinContextStore::new(dir.path()).unwrap();
        no_parent.path = PathBuf::new();
        assert!(no_parent.save(&data).is_err());

        let blocked_parent = dir.path().join("blocked-parent");
        std::fs::write(&blocked_parent, b"file").unwrap();
        let mut blocked = WeixinContextStore::new(dir.path()).unwrap();
        blocked.path = blocked_parent.join(STORE_FILENAME);
        assert!(blocked.save(&data).is_err());

        let open_dir = tempfile::tempdir().unwrap();
        let open_blocked = WeixinContextStore::new(open_dir.path()).unwrap();
        let temporary = open_blocked.path.with_extension("json.tmp");
        std::fs::create_dir_all(&temporary).unwrap();
        assert!(open_blocked.save(&data).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let write_dir = tempfile::tempdir().unwrap();
            let write_blocked = WeixinContextStore::new(write_dir.path()).unwrap();
            std::fs::create_dir_all(write_blocked.path.parent().unwrap()).unwrap();
            symlink("/dev/full", write_blocked.path.with_extension("json.tmp")).unwrap();
            assert!(write_blocked.save(&data).is_err());

            let sync_dir = tempfile::tempdir().unwrap();
            let sync_blocked = WeixinContextStore::new(sync_dir.path()).unwrap();
            std::fs::create_dir_all(sync_blocked.path.parent().unwrap()).unwrap();
            symlink("/dev/null", sync_blocked.path.with_extension("json.tmp")).unwrap();
            assert!(sync_blocked.save(&data).is_err());

            assert!(harden_private_file(&dir.path().join("missing-file")).is_err());
            assert!(sync_directory(&dir.path().join("missing-directory")).is_err());
        }
    }

    #[test]
    fn oversized_context_store_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        assert!(WeixinContextStore::load(&path).is_none());
    }
}
