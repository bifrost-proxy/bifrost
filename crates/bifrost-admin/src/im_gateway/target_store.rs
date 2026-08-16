use std::path::{Path, PathBuf};

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
        self.data.read().targets.clone()
    }

    pub fn list_by_provider(&self, provider_id: &str) -> Vec<ImTarget> {
        self.data
            .read()
            .targets
            .iter()
            .filter(|t| t.provider_id == provider_id)
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ImTarget> {
        self.data
            .read()
            .targets
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    pub fn add(&self, target: ImTarget) -> Result<()> {
        let mut data = self.data.write();
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
        let mut data = self.data.write();
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
        let mut data = self.data.write();
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
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| BifrostError::Config(format!("serialize target store: {e}")))?;
        std::fs::write(&self.file_path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.file_path.display()
            )))
        })?;
        crate::worker_runtime::im_gateway::notify_config_changed();
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
