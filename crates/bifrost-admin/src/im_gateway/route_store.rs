use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::{ImRoute, ImRouteAction};

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_routes.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    routes: Vec<ImRoute>,
}

pub struct ImRouteStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImRouteStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            routes: Vec::new(),
        });
        Self {
            file_path,
            data: RwLock::new(data),
        }
    }

    pub fn list(&self) -> Vec<ImRoute> {
        self.data.read().routes.clone()
    }

    pub fn list_by_provider(&self, provider_id: &str) -> Vec<ImRoute> {
        self.data
            .read()
            .routes
            .iter()
            .filter(|r| r.provider_id == provider_id)
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ImRoute> {
        self.data.read().routes.iter().find(|r| r.id == id).cloned()
    }

    pub fn add(&self, route: ImRoute) -> Result<()> {
        self.validate_route(&route)?;
        let mut data = self.data.write();
        if data.routes.iter().any(|r| r.id == route.id) {
            return Err(BifrostError::Config(format!(
                "route with id '{}' already exists",
                route.id
            )));
        }
        data.routes.push(route);
        self.save_locked(&data)
    }

    pub fn update(&self, route: ImRoute) -> Result<()> {
        self.validate_route(&route)?;
        let mut data = self.data.write();
        if let Some(existing) = data.routes.iter_mut().find(|r| r.id == route.id) {
            *existing = route;
            self.save_locked(&data)
        } else {
            Err(BifrostError::Config(format!(
                "route '{}' not found",
                route.id
            )))
        }
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let mut data = self.data.write();
        let before = data.routes.len();
        data.routes.retain(|r| r.id != id);
        if data.routes.len() == before {
            return Err(BifrostError::Config(format!("route '{id}' not found")));
        }
        self.save_locked(&data)
    }

    /// Validate that a route has at least one matcher condition and uses an allowed action type.
    fn validate_route(&self, route: &ImRoute) -> Result<()> {
        // Must have at least one matcher condition
        let matcher = &route.matcher;
        if matcher.chat_ids.is_empty()
            && matcher.user_ids.is_empty()
            && matcher.keyword.is_none()
            && matcher.regex.is_none()
        {
            return Err(BifrostError::Config(
                "route matcher must have at least one condition (chat_ids, user_ids, keyword, or regex)".to_string(),
            ));
        }

        // V1: RunScriptAndReply and AgentChat are allowed
        match &route.action {
            ImRouteAction::RunScriptAndReply { .. } | ImRouteAction::AgentChat { .. } => {}
        }

        Ok(())
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
            .map_err(|e| BifrostError::Config(format!("serialize route store: {e}")))?;
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
                let _ = std::fs::remove_file(file_path);
                None
            }
        }
    }
}
