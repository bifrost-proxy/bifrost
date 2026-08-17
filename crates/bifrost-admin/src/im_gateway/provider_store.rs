use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::{normalize_provider_base_url, ImProviderConfig};

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_providers.json";
const MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    providers: Vec<ImProviderConfig>,
}

pub struct ImProviderStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
    write_blocked: bool,
}

impl ImProviderStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let loaded = Self::load_with_backup(&file_path);
        let write_blocked =
            loaded.is_none() && (file_path.exists() || backup_path(&file_path).exists());
        let data = loaded.unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            providers: Vec::new(),
        });
        Self {
            file_path,
            data: RwLock::new(data),
            write_blocked,
        }
    }

    pub fn list(&self) -> Vec<ImProviderConfig> {
        self.refresh_from_disk();
        self.data
            .read()
            .providers
            .iter()
            .cloned()
            .map(normalized_provider)
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ImProviderConfig> {
        self.refresh_from_disk();
        self.data
            .read()
            .providers
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .map(normalized_provider)
    }

    pub fn add(&self, provider: ImProviderConfig) -> Result<()> {
        self.ensure_writable()?;
        let _file_lock = self.acquire_write_lock()?;
        let provider = normalized_provider(provider);
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if data.providers.iter().any(|p| p.id == provider.id) {
            return Err(BifrostError::Config(format!(
                "provider with id '{}' already exists",
                provider.id
            )));
        }
        data.providers.push(provider);
        self.save_locked(&data)
    }

    pub fn update(&self, provider: ImProviderConfig) -> Result<()> {
        self.ensure_writable()?;
        let _file_lock = self.acquire_write_lock()?;
        let provider = normalized_provider(provider);
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if let Some(existing) = data.providers.iter_mut().find(|p| p.id == provider.id) {
            *existing = provider;
            self.save_locked(&data)
        } else {
            Err(BifrostError::Config(format!(
                "provider '{}' not found",
                provider.id
            )))
        }
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.ensure_writable()?;
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        let before = data.providers.len();
        data.providers.retain(|p| p.id != id);
        if data.providers.len() == before {
            return Err(BifrostError::Config(format!("provider '{id}' not found")));
        }
        self.save_locked(&data)
    }

    fn save_locked(&self, data: &StoreData) -> Result<()> {
        self.ensure_writable()?;
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| BifrostError::Config(format!("serialize provider store: {e}")))?;
        let backup_path = backup_path(&self.file_path);

        // Keep a last-known-good snapshot before replacing the primary file. A
        // test/dev process can be interrupted at any point; neither that crash
        // nor a concurrent reader should ever observe a truncated JSON file.
        match Self::load_file(&self.file_path) {
            Ok(Some(previous)) => {
                let previous_content = serde_json::to_string_pretty(&previous).map_err(|e| {
                    BifrostError::Config(format!("serialize provider store backup: {e}"))
                })?;
                atomic_write(&backup_path, previous_content.as_bytes())?;
            }
            Ok(None) => {
                // First save: seed the recovery file with the same valid data.
                atomic_write(&backup_path, content.as_bytes())?;
            }
            Err(error) => {
                // Never replace a valid backup with bytes from an invalid
                // primary. The primary write below is still atomic and repairs
                // the active file while preserving the recovery point.
                tracing::warn!(
                    path = %self.file_path.display(),
                    error = %error,
                    "provider store primary is invalid; preserving existing backup"
                );
            }
        }

        atomic_write(&self.file_path, content.as_bytes())?;
        crate::worker_runtime::im_gateway::notify_runtime_config_changed();
        Ok(())
    }

    fn refresh_from_disk(&self) {
        if let Some(data) = Self::load_with_backup(&self.file_path) {
            *self.data.write() = data;
        }
    }

    fn refresh_locked(&self, data: &mut StoreData) {
        if let Some(latest) = Self::load_with_backup(&self.file_path) {
            *data = latest;
        }
    }

    fn acquire_write_lock(&self) -> Result<File> {
        let parent = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let lock_path = parent.join(format!("{STORE_FILENAME}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                BifrostError::Io(std::io::Error::other(format!(
                    "open provider store lock {}: {error}",
                    lock_path.display()
                )))
            })?;
        file.lock_exclusive().map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "lock provider store {}: {error}",
                lock_path.display()
            )))
        })?;
        Ok(file)
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.write_blocked {
            return Err(BifrostError::Config(format!(
                "IM provider store '{}' and its backup could not be loaded; refusing to overwrite recovery evidence",
                self.file_path.display()
            )));
        }
        Ok(())
    }

    fn load_with_backup(file_path: &Path) -> Option<StoreData> {
        match Self::load_file(file_path) {
            Ok(Some(data)) => Some(data),
            Ok(None) => Self::recover_from_backup(file_path, "primary store is missing"),
            Err(error) => {
                tracing::warn!(
                    path = %file_path.display(),
                    error = %error,
                    "failed to load IM provider store; preserving the primary file"
                );
                Self::recover_from_backup(file_path, &error)
            }
        }
    }

    fn recover_from_backup(file_path: &Path, reason: &str) -> Option<StoreData> {
        let backup_path = backup_path(file_path);
        match Self::load_file(&backup_path) {
            Ok(Some(data)) => {
                match serde_json::to_vec_pretty(&data)
                    .map_err(|e| BifrostError::Config(format!("serialize recovered store: {e}")))
                    .and_then(|content| atomic_write(file_path, &content))
                {
                    Ok(()) => tracing::warn!(
                        path = %file_path.display(),
                        backup_path = %backup_path.display(),
                        reason = %reason,
                        "recovered IM provider store from backup"
                    ),
                    Err(error) => tracing::warn!(
                        path = %file_path.display(),
                        backup_path = %backup_path.display(),
                        reason = %reason,
                        error = %error,
                        "loaded IM provider backup but failed to restore primary"
                    ),
                }
                Some(data)
            }
            Ok(None) => None,
            Err(backup_error) => {
                tracing::warn!(
                    path = %file_path.display(),
                    backup_path = %backup_path.display(),
                    reason = %reason,
                    error = %backup_error,
                    "IM provider store and backup are both unreadable; files were preserved"
                );
                None
            }
        }
    }

    fn load_file(file_path: &Path) -> std::result::Result<Option<StoreData>, String> {
        if !file_path.exists() {
            return Ok(None);
        }
        let size = std::fs::metadata(file_path)
            .map_err(|error| format!("read metadata failed: {error}"))?
            .len();
        if size > MAX_STORE_FILE_BYTES {
            return Err(format!(
                "provider store exceeds {MAX_STORE_FILE_BYTES} bytes"
            ));
        }
        let content = std::fs::read_to_string(file_path)
            .map_err(|error| format!("read provider store failed: {error}"))?;
        match serde_json::from_str::<StoreData>(&content) {
            Ok(data) if data.version == STORE_VERSION => Ok(Some(data)),
            Ok(data) => Err(format!(
                "unsupported provider store version {}",
                data.version
            )),
            Err(error) => Err(format!("parse provider store failed: {error}")),
        }
    }
}

fn backup_path(file_path: &Path) -> PathBuf {
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(STORE_FILENAME);
    file_path.with_file_name(format!("{file_name}.bak"))
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!(
            "atomically replace {}: {}",
            path.display(),
            error.error
        )))
    })?;
    Ok(())
}

fn normalized_provider(mut provider: ImProviderConfig) -> ImProviderConfig {
    normalize_provider_base_url(&mut provider);
    provider
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::types::ImProviderType;

    fn provider(id: &str) -> ImProviderConfig {
        ImProviderConfig {
            id: id.to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: id.to_string(),
            enabled: true,
            base_url: None,
            app_id: Some("cli_test".to_string()),
            secret_ref: Some("env:TEST_SECRET".to_string()),
            owner_open_id: None,
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn atomic_save_seeds_valid_recovery_backup() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = ImProviderStore::new(temp.path());
        store.add(provider("feishu-main")).expect("add provider");

        let primary = temp.path().join("admin").join(STORE_FILENAME);
        let backup = backup_path(&primary);
        assert!(ImProviderStore::load_file(&primary).unwrap().is_some());
        assert!(ImProviderStore::load_file(&backup).unwrap().is_some());
        assert!(temp
            .path()
            .join("admin")
            .read_dir()
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn truncated_primary_recovers_provider_from_backup() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = ImProviderStore::new(temp.path());
        store.add(provider("feishu-main")).expect("add provider");
        let primary = temp.path().join("admin").join(STORE_FILENAME);
        std::fs::write(&primary, b"{\"version\":1,\"providers\":[").expect("truncate primary");

        let recovered = ImProviderStore::new(temp.path());

        assert!(recovered.get("feishu-main").is_some());
        assert!(ImProviderStore::load_file(&primary).unwrap().is_some());
    }

    #[test]
    fn invalid_primary_without_backup_is_preserved() {
        let temp = tempfile::tempdir().expect("temp dir");
        let primary = temp.path().join("admin").join(STORE_FILENAME);
        std::fs::create_dir_all(primary.parent().unwrap()).expect("admin dir");
        let invalid = b"not valid provider json";
        std::fs::write(&primary, invalid).expect("invalid primary");

        let store = ImProviderStore::new(temp.path());

        assert!(store.list().is_empty());
        assert!(store.add(provider("replacement")).is_err());
        assert!(store.list().is_empty());
        assert_eq!(std::fs::read(&primary).unwrap(), invalid);
        assert!(!backup_path(&primary).exists());
    }

    #[test]
    fn invalid_backup_does_not_delete_invalid_primary() {
        let temp = tempfile::tempdir().expect("temp dir");
        let primary = temp.path().join("admin").join(STORE_FILENAME);
        std::fs::create_dir_all(primary.parent().unwrap()).expect("admin dir");
        std::fs::write(&primary, b"broken primary").expect("primary");
        std::fs::write(backup_path(&primary), b"broken backup").expect("backup");

        let store = ImProviderStore::new(temp.path());

        assert!(store.list().is_empty());
        assert!(store.add(provider("replacement")).is_err());
        assert!(store.list().is_empty());
        assert_eq!(std::fs::read(&primary).unwrap(), b"broken primary");
        assert_eq!(
            std::fs::read(backup_path(&primary)).unwrap(),
            b"broken backup"
        );
    }

    #[test]
    fn unsupported_version_without_backup_is_preserved() {
        let temp = tempfile::tempdir().expect("temp dir");
        let primary = temp.path().join("admin").join(STORE_FILENAME);
        std::fs::create_dir_all(primary.parent().unwrap()).expect("admin dir");
        let unsupported = br#"{"version":999,"providers":[]}"#;
        std::fs::write(&primary, unsupported).expect("unsupported primary");

        let store = ImProviderStore::new(temp.path());

        assert!(store.list().is_empty());
        assert!(store.add(provider("replacement")).is_err());
        assert!(store.list().is_empty());
        assert_eq!(std::fs::read(&primary).unwrap(), unsupported);
        assert!(!backup_path(&primary).exists());
    }

    #[test]
    fn coverage_gap_provider_store_reports_lock_open_and_atomic_write_replace_failures() {
        let temp = tempfile::tempdir().expect("temp dir");
        let admin_dir = temp.path().join("admin");
        std::fs::create_dir_all(&admin_dir).expect("admin dir");

        let store = ImProviderStore::new(temp.path());
        let lock_path = admin_dir.join(format!("{STORE_FILENAME}.lock"));
        std::fs::create_dir(&lock_path).expect("blocking lock directory");
        let lock_error = store
            .add(provider("blocked-lock"))
            .expect_err("a directory cannot be opened as the provider lock file");
        assert!(lock_error.to_string().contains("open provider store lock"));

        let destination = admin_dir.join("atomic-destination");
        std::fs::create_dir(&destination).expect("blocking destination directory");
        let write_error = atomic_write(&destination, b"provider data")
            .expect_err("atomic replacement must reject a directory destination");
        assert!(write_error.to_string().contains("atomically replace"));
    }
}
