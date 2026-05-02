use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::ImProviderConfig;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_providers.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    providers: Vec<ImProviderConfig>,
}

pub struct ImProviderStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImProviderStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            providers: Vec::new(),
        });
        Self {
            file_path,
            data: RwLock::new(data),
        }
    }

    pub fn list(&self) -> Vec<ImProviderConfig> {
        self.data.read().providers.clone()
    }

    pub fn get(&self, id: &str) -> Option<ImProviderConfig> {
        self.data
            .read()
            .providers
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    pub fn add(&self, provider: ImProviderConfig) -> Result<()> {
        let mut data = self.data.write();
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
        let mut data = self.data.write();
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
        let mut data = self.data.write();
        let before = data.providers.len();
        data.providers.retain(|p| p.id != id);
        if data.providers.len() == before {
            return Err(BifrostError::Config(format!("provider '{id}' not found")));
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
            .map_err(|e| BifrostError::Config(format!("serialize provider store: {e}")))?;
        std::fs::write(&self.file_path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.file_path.display()
            )))
        })?;
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
                // Incompatible version or parse error: reset
                let _ = std::fs::remove_file(file_path);
                None
            }
        }
    }
}
