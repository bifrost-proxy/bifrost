use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::ImTaskRun;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_runs.json";
const MAX_RUNS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    runs: Vec<ImTaskRun>,
}

pub struct ImRunStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImRunStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            runs: Vec::new(),
        });
        Self {
            file_path,
            data: RwLock::new(data),
        }
    }

    pub fn list(&self) -> Vec<ImTaskRun> {
        self.data.read().runs.clone()
    }

    pub fn list_by_route(&self, route_id: &str) -> Vec<ImTaskRun> {
        self.data
            .read()
            .runs
            .iter()
            .filter(|r| r.route_id.as_deref() == Some(route_id))
            .cloned()
            .collect()
    }

    pub fn list_by_schedule(&self, schedule_id: &str) -> Vec<ImTaskRun> {
        self.data
            .read()
            .runs
            .iter()
            .filter(|r| r.schedule_id.as_deref() == Some(schedule_id))
            .cloned()
            .collect()
    }

    pub fn get(&self, run_id: &str) -> Option<ImTaskRun> {
        self.data
            .read()
            .runs
            .iter()
            .find(|r| r.run_id == run_id)
            .cloned()
    }

    pub fn add(&self, run: ImTaskRun) -> Result<()> {
        let mut data = self.data.write();
        data.runs.push(run);
        self.trim_locked(&mut data);
        self.save_locked(&data)
    }

    pub fn update(&self, run: ImTaskRun) -> Result<()> {
        let mut data = self.data.write();
        if let Some(existing) = data.runs.iter_mut().find(|r| r.run_id == run.run_id) {
            *existing = run;
            self.save_locked(&data)
        } else {
            Err(BifrostError::Config(format!(
                "run '{}' not found",
                run.run_id
            )))
        }
    }

    pub fn clear(&self) -> Result<()> {
        let mut data = self.data.write();
        data.runs.clear();
        self.save_locked(&data)
    }

    fn trim_locked(&self, data: &mut StoreData) {
        if data.runs.len() > MAX_RUNS {
            let drain_count = data.runs.len() - MAX_RUNS;
            data.runs.drain(..drain_count);
        }
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
            .map_err(|e| BifrostError::Config(format!("serialize run store: {e}")))?;
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
