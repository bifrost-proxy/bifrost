use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

use super::types::ImSchedule;

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "im_gateway_schedules.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    version: u32,
    schedules: Vec<ImSchedule>,
}

pub struct ImScheduleStore {
    file_path: PathBuf,
    data: RwLock<StoreData>,
}

impl ImScheduleStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(STORE_FILENAME);
        let data = Self::load_from_disk(&file_path).unwrap_or_else(|| StoreData {
            version: STORE_VERSION,
            schedules: Vec::new(),
        });
        Self {
            file_path,
            data: RwLock::new(data),
        }
    }

    pub fn list(&self) -> Vec<ImSchedule> {
        self.refresh_from_disk();
        self.data.read().schedules.clone()
    }

    pub fn get(&self, id: &str) -> Option<ImSchedule> {
        self.refresh_from_disk();
        self.data
            .read()
            .schedules
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    pub fn add(&self, schedule: ImSchedule) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if data.schedules.iter().any(|s| s.id == schedule.id) {
            return Err(BifrostError::Config(format!(
                "schedule with id '{}' already exists",
                schedule.id
            )));
        }
        data.schedules.push(schedule);
        self.save_locked(&data)
    }

    pub fn update(&self, schedule: ImSchedule) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        if let Some(existing) = data.schedules.iter_mut().find(|s| s.id == schedule.id) {
            *existing = schedule;
            self.save_locked(&data)
        } else {
            Err(BifrostError::Config(format!(
                "schedule '{}' not found",
                schedule.id
            )))
        }
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        let before = data.schedules.len();
        data.schedules.retain(|s| s.id != id);
        if data.schedules.len() == before {
            return Err(BifrostError::Config(format!("schedule '{id}' not found")));
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
            .map_err(|e| BifrostError::Config(format!("serialize schedule store: {e}")))?;
        atomic_write(&self.file_path, content.as_bytes())?;
        crate::worker_runtime::im_gateway::notify_runtime_config_changed();
        Ok(())
    }

    fn acquire_write_lock(&self) -> Result<File> {
        let lock_path = self.file_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
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
