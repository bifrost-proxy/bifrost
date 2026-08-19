use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::GrantInfo;

const GRANT_INFO_STORE_FILE: &str = "remote_invoke_grant_info.json";
const GRANT_INFO_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantInfoStoreFile {
    version: u32,
    entries: Vec<PersistedGrantInfoEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedGrantInfoEntry {
    grant_id: String,
    relay_url: String,
    info: GrantInfo,
    updated_at: u64,
}

pub struct GrantInfoStore {
    file_path: PathBuf,
    lock_path: PathBuf,
    lock: Mutex<()>,
}

impl GrantInfoStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        Self {
            file_path: admin_dir.join(GRANT_INFO_STORE_FILE),
            lock_path: admin_dir.join(format!("{GRANT_INFO_STORE_FILE}.lock")),
            lock: Mutex::new(()),
        }
    }

    pub fn load_for_relay(&self, relay_url: &str) -> Result<HashMap<String, GrantInfo>> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let file = self.read_store_file()?;
        Ok(file
            .entries
            .into_iter()
            .filter(|entry| entry.relay_url == relay_url)
            .map(|entry| (entry.grant_id, entry.info))
            .collect())
    }

    pub fn upsert(&self, relay_url: &str, grant_id: &str, info: &GrantInfo) -> Result<()> {
        self.update_for_relay(relay_url, |grants| {
            grants.insert(grant_id.to_string(), info.clone());
            Ok(())
        })
    }

    pub fn remove(&self, grant_id: &str) -> Result<()> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut file = self.read_store_file()?;
        let before = file.entries.len();
        file.entries.retain(|entry| entry.grant_id != grant_id);
        if before == file.entries.len() {
            return Ok(());
        }
        self.write_store_file(&file)
    }

    pub fn retain_only(&self, relay_url: &str, grant_ids: &HashSet<String>) -> Result<()> {
        self.update_for_relay(relay_url, |grants| {
            grants.retain(|grant_id, _| grant_ids.contains(grant_id));
            Ok(())
        })
    }

    /// Run one relay-scoped read/validate/update operation while holding the
    /// same cross-process lock used by grant creation, revocation and cleanup.
    /// This prevents authorization from racing a revoke or consuming a
    /// single-use grant more than once.
    pub fn update_for_relay<T, F>(&self, relay_url: &str, update: F) -> Result<T>
    where
        F: FnOnce(&mut HashMap<String, GrantInfo>) -> Result<T>,
    {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut file = self.read_store_file()?;
        let mut grants = file
            .entries
            .iter()
            .filter(|entry| entry.relay_url == relay_url)
            .map(|entry| (entry.grant_id.clone(), entry.info.clone()))
            .collect::<HashMap<_, _>>();
        let result = update(&mut grants)?;
        file.entries.retain(|entry| entry.relay_url != relay_url);
        let updated_at = now_millis();
        file.entries.extend(
            grants
                .into_iter()
                .map(|(grant_id, info)| PersistedGrantInfoEntry {
                    grant_id,
                    relay_url: relay_url.to_string(),
                    info,
                    updated_at,
                }),
        );
        self.write_store_file(&file)?;
        Ok(result)
    }

    fn acquire_file_lock(&self) -> Result<File> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn read_store_file(&self) -> Result<GrantInfoStoreFile> {
        if !self.file_path.exists() {
            return Ok(GrantInfoStoreFile {
                version: GRANT_INFO_STORE_VERSION,
                entries: Vec::new(),
            });
        }
        const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(&self.file_path) {
            if meta.len() > MAX_STORE_FILE_BYTES {
                return Err(BifrostError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("store file too large ({} bytes)", meta.len()),
                )));
            }
        }
        let content = std::fs::read_to_string(&self.file_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                self.file_path.display()
            )))
        })?;
        match serde_json::from_str::<GrantInfoStoreFile>(&content) {
            Ok(file) if file.version == GRANT_INFO_STORE_VERSION => Ok(file),
            Ok(file) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset incompatible grant info store version {}",
                    file.version
                )))
            }
            Err(e) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset unreadable grant info store {}: {e}",
                    self.file_path.display()
                )))
            }
        }
    }

    fn write_store_file(&self, file: &GrantInfoStoreFile) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_vec_pretty(file)
            .map_err(|e| BifrostError::Config(format!("serialize grant info store: {e}")))?;
        let parent = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "create temporary grant info store for {}: {e}",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_invoke::types::{
        AuthMethod, FileAccessScope, GrantMode, GrantScope, GrantStatus,
    };

    fn grant(id: &str) -> GrantInfo {
        GrantInfo {
            grant_id: id.to_string(),
            client_instance_id: "client".to_string(),
            caller_fingerprint: "caller".to_string(),
            caller_display_name: None,
            label: None,
            grant_mode: GrantMode::Permanent,
            grant_scope: GrantScope::RemoteQuery,
            file_access: FileAccessScope::None,
            auth_method: AuthMethod::PairCode,
            status: GrantStatus::Active,
            first_authorized_at: 1,
            last_command_at: None,
            expires_at: None,
            last_used_at: None,
            max_calls: None,
            remaining_calls: None,
            use_count: 0,
            ssh_key_id: None,
            ssh_key_fingerprint: None,
            caller_ephemeral_pub: None,
            client_ephemeral_pub: None,
            policy_binding: None,
            shell_policy_set_version_snapshot: None,
            interactive_allowed: None,
            stdin_allowed: None,
            os_version: None,
            arch: None,
        }
    }

    #[test]
    fn independent_store_instances_do_not_lose_concurrent_grant_updates() {
        let temp = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for index in 0..2 {
            let root = temp.path().to_path_buf();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let id = format!("grant-{index}");
                barrier.wait();
                GrantInfoStore::new(&root)
                    .upsert("https://relay.example", &id, &grant(&id))
                    .unwrap();
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        let grants = GrantInfoStore::new(temp.path())
            .load_for_relay("https://relay.example")
            .unwrap();
        assert!(grants.contains_key("grant-0"));
        assert!(grants.contains_key("grant-1"));
    }

    #[test]
    fn relay_transaction_rolls_back_when_authorization_fails() {
        let temp = tempfile::tempdir().unwrap();
        let store = GrantInfoStore::new(temp.path());
        store
            .upsert("https://relay.example", "grant", &grant("grant"))
            .unwrap();
        let result: Result<()> = store.update_for_relay("https://relay.example", |grants| {
            grants.get_mut("grant").unwrap().use_count = 99;
            Err(BifrostError::Config("reject".to_string()))
        });
        assert!(result.is_err());
        assert_eq!(
            store.load_for_relay("https://relay.example").unwrap()["grant"].use_count,
            0
        );
    }

    #[test]
    fn atomic_persist_and_oversized_load_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let store = GrantInfoStore::new(temp.path());
        std::fs::create_dir_all(store.file_path.parent().unwrap()).unwrap();
        std::fs::create_dir(&store.file_path).unwrap();
        let error = store
            .write_store_file(&GrantInfoStoreFile {
                version: GRANT_INFO_STORE_VERSION,
                entries: Vec::new(),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("atomically replace"), "{error}");

        std::fs::remove_dir(&store.file_path).unwrap();
        let oversized = std::fs::File::create(&store.file_path).unwrap();
        oversized.set_len(256 * 1024 * 1024 + 1).unwrap();
        assert!(store
            .read_store_file()
            .unwrap_err()
            .to_string()
            .contains("too large"));
        drop(oversized);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let parent = store.file_path.parent().unwrap();
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();
            let error = store
                .write_store_file(&GrantInfoStoreFile {
                    version: GRANT_INFO_STORE_VERSION,
                    entries: Vec::new(),
                })
                .unwrap_err()
                .to_string();
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(
                error.contains("create temporary grant info store"),
                "{error}"
            );
        }
    }
}
