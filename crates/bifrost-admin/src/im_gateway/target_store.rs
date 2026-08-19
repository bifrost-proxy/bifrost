use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::ImTarget;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_targets.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    targets: Vec<ImTarget>,
}

pub struct ImTargetStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImTargetStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            targets: Vec::new(),
        });
        Self {
            file_path,
            data: RwLock::new(data),
        }
    }

    pub fn list(&self) -> Vec<ImTarget> {
        self.refresh_from_disk();
        self.data.read().targets.clone()
    }

    pub fn list_by_provider(&self, provider_id: &str) -> Vec<ImTarget> {
        self.refresh_from_disk();
        self.data
            .read()
            .targets
            .iter()
            .filter(|t| t.provider_id == provider_id)
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ImTarget> {
        self.refresh_from_disk();
        self.data
            .read()
            .targets
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    pub fn add(&self, target: ImTarget) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if data.targets.iter().any(|t| t.id == target.id) {
            return Err(BifrostError::Config(format!(
                "target with id '{}' already exists",
                target.id
            )));
        }
        data.targets.push(target);
        self.save_locked(&data)
    }

    pub fn update(&self, target: ImTarget) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if let Some(existing) = data.targets.iter_mut().find(|t| t.id == target.id) {
            *existing = target;
            self.save_locked(&data)
        } else {
            Err(BifrostError::Config(format!(
                "target '{}' not found",
                target.id
            )))
        }
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        let before = data.targets.len();
        data.targets.retain(|t| t.id != id);
        if data.targets.len() == before {
            return Err(BifrostError::Config(format!("target '{id}' not found")));
        }
        self.save_locked(&data)
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
            .map_err(|e| BifrostError::Config(format!("serialize target store: {e}")))?;
        let parent = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "create temporary target store for {}: {e}",
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
        crate::worker_runtime::im_gateway::notify_config_changed();
        Ok(())
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
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str) -> ImTarget {
        ImTarget {
            id: id.to_string(),
            provider_id: "provider".to_string(),
            display_name: id.to_string(),
            receive_id_type: "chat_id".to_string(),
            receive_id: format!("chat-{id}"),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn independent_store_instances_preserve_updates_and_refresh_crud_reads() {
        let temp = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for index in 0..2 {
            let root = temp.path().to_path_buf();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let store = ImTargetStore::new(&root);
                barrier.wait();
                store.add(target(&format!("target-{index}"))).unwrap();
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }

        let first = ImTargetStore::new(temp.path());
        let second = ImTargetStore::new(temp.path());
        assert_eq!(first.list().len(), 2);
        assert_eq!(first.list_by_provider("provider").len(), 2);
        let mut updated = second.get("target-0").unwrap();
        updated.display_name = "updated".to_string();
        second.update(updated).unwrap();
        assert_eq!(first.get("target-0").unwrap().display_name, "updated");
        second.delete("target-1").unwrap();
        assert!(first.get("target-1").is_none());
        assert!(second.update(target("missing")).is_err());
        assert!(second.delete("missing").is_err());
        second.add(target("duplicate")).unwrap();
        assert!(second.add(target("duplicate")).is_err());
    }

    #[test]
    fn atomic_persist_and_load_fail_closed_for_invalid_store_files() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImTargetStore::new(temp.path());
        std::fs::create_dir_all(store.file_path.parent().unwrap()).unwrap();
        std::fs::create_dir(&store.file_path).unwrap();
        let error = store
            .save_locked(&StoreData {
                version: STORE_VERSION,
                targets: vec![target("persist-failure")],
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("atomically replace"), "{error}");

        std::fs::remove_dir(&store.file_path).unwrap();
        let oversized = std::fs::File::create(&store.file_path).unwrap();
        oversized.set_len(256 * 1024 * 1024 + 1).unwrap();
        assert!(ImTargetStore::load_from_disk(&store.file_path).is_none());
        drop(oversized);
        std::fs::write(&store.file_path, b"{invalid-json").unwrap();
        assert!(ImTargetStore::load_from_disk(&store.file_path).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let parent = store.file_path.parent().unwrap();
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();
            let error = store
                .save_locked(&StoreData {
                    version: STORE_VERSION,
                    targets: vec![target("temporary-create-failure")],
                })
                .unwrap_err()
                .to_string();
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(error.contains("create temporary target store"), "{error}");
        }
    }
}
