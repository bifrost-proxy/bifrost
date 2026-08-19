use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
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
        self.refresh_from_disk();
        self.data.read().runs.clone()
    }

    pub fn list_by_route(&self, route_id: &str) -> Vec<ImTaskRun> {
        self.refresh_from_disk();
        self.data
            .read()
            .runs
            .iter()
            .filter(|r| r.route_id.as_deref() == Some(route_id))
            .cloned()
            .collect()
    }

    pub fn list_by_schedule(&self, schedule_id: &str) -> Vec<ImTaskRun> {
        self.refresh_from_disk();
        self.data
            .read()
            .runs
            .iter()
            .filter(|r| r.schedule_id.as_deref() == Some(schedule_id))
            .cloned()
            .collect()
    }

    pub fn get(&self, run_id: &str) -> Option<ImTaskRun> {
        self.refresh_from_disk();
        self.data
            .read()
            .runs
            .iter()
            .find(|r| r.run_id == run_id)
            .cloned()
    }

    pub fn add(&self, run: ImTaskRun) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
        data.runs.push(run);
        self.trim_locked(&mut data);
        self.save_locked(&data)
    }

    pub fn update(&self, run: ImTaskRun) -> Result<()> {
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
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
        let _file_lock = self.acquire_write_lock()?;
        let mut data = self.data.write();
        self.refresh_locked(&mut data);
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
        let content = serde_json::to_vec_pretty(data)
            .map_err(|e| BifrostError::Config(format!("serialize run store: {e}")))?;
        let parent = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "create temporary run store for {}: {e}",
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
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::types::{TaskRunStatus, TriggerSource};

    fn run(id: &str) -> ImTaskRun {
        ImTaskRun {
            run_id: id.to_string(),
            trigger_source: TriggerSource::ManualRun,
            route_id: None,
            schedule_id: Some("schedule".to_string()),
            provider_id: None,
            target_id: None,
            status: TaskRunStatus::Success,
            started_at: 1,
            ended_at: Some(2),
            duration_ms: Some(1),
            exit_code: Some(0),
            input_preview: None,
            stdout_preview: None,
            stderr_preview: None,
            stdout_digest: None,
            stderr_digest: None,
            error: None,
            task_type: None,
            runner_id: None,
            agent_final_response: None,
            agent_tool_calls: Vec::new(),
            agent_plan_steps: None,
        }
    }

    #[test]
    fn independent_store_instances_preserve_concurrent_runs_and_refresh_reads() {
        let temp = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for index in 0..2 {
            let root = temp.path().to_path_buf();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let store = ImRunStore::new(&root);
                barrier.wait();
                store.add(run(&format!("run-{index}"))).unwrap();
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        let ids = ImRunStore::new(temp.path())
            .list_by_schedule("schedule")
            .into_iter()
            .map(|run| run.run_id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("run-0"));
        assert!(ids.contains("run-1"));

        let first = ImRunStore::new(temp.path());
        let second = ImRunStore::new(temp.path());
        assert_eq!(first.list().len(), 2);
        assert_eq!(first.get("run-0").unwrap().status, TaskRunStatus::Success);
        let mut updated = second.get("run-0").unwrap();
        updated.status = TaskRunStatus::Failed;
        updated.route_id = Some("route".to_string());
        second.update(updated).unwrap();
        assert_eq!(first.get("run-0").unwrap().status, TaskRunStatus::Failed);
        assert_eq!(first.list_by_route("route").len(), 1);
        assert!(second.update(run("missing")).is_err());
        second.clear().unwrap();
        assert!(first.list().is_empty());
    }

    #[test]
    fn atomic_persist_and_load_fail_closed_for_invalid_store_files() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImRunStore::new(temp.path());
        std::fs::create_dir_all(store.file_path.parent().unwrap()).unwrap();
        std::fs::create_dir(&store.file_path).unwrap();
        let error = store
            .save_locked(&StoreData {
                version: STORE_VERSION,
                runs: vec![run("persist-failure")],
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("atomically replace"), "{error}");

        std::fs::remove_dir(&store.file_path).unwrap();
        let oversized = std::fs::File::create(&store.file_path).unwrap();
        oversized.set_len(256 * 1024 * 1024 + 1).unwrap();
        assert!(ImRunStore::load_from_disk(&store.file_path).is_none());
        drop(oversized);
        std::fs::write(&store.file_path, b"{invalid-json").unwrap();
        assert!(ImRunStore::load_from_disk(&store.file_path).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let parent = store.file_path.parent().unwrap();
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();
            let error = store
                .save_locked(&StoreData {
                    version: STORE_VERSION,
                    runs: vec![run("temporary-create-failure")],
                })
                .unwrap_err()
                .to_string();
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(error.contains("create temporary run store"), "{error}");
        }
    }
}
