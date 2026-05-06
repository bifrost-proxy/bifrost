use std::fs;
use std::path::{Path, PathBuf};

use bifrost_core::{BifrostError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const REMOTE_SHELL_FILENAME: &str = "remote_shell.json";
const REMOTE_SHELL_STORE_SCHEMA_VERSION: u64 = 1;

fn default_schema_version() -> u64 {
    REMOTE_SHELL_STORE_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RemoteShellPolicy {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub profile_id: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RemoteShellProfile {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteShellSet {
    #[serde(default = "default_schema_version")]
    pub schema_version: u64,
    pub version: u64,
    pub policies: Vec<RemoteShellPolicy>,
    pub profiles: Vec<RemoteShellProfile>,
}

impl Default for RemoteShellSet {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            version: 0,
            policies: Vec::new(),
            profiles: Vec::new(),
        }
    }
}

impl RemoteShellSet {
    pub fn find_policy(&self, policy_id: &str) -> Option<&RemoteShellPolicy> {
        self.policies.iter().find(|policy| policy.id == policy_id)
    }

    pub fn find_profile(&self, profile_id: &str) -> Option<&RemoteShellProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id == profile_id)
    }

    pub fn current_version(&self) -> u64 {
        self.version
    }
}

#[derive(Debug, Clone)]
pub struct RemoteShellStore {
    file_path: PathBuf,
}

impl RemoteShellStore {
    pub fn new() -> Result<Self> {
        Self::with_file(crate::data_dir().join(REMOTE_SHELL_FILENAME))
    }

    pub fn with_file(file_path: PathBuf) -> Result<Self> {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { file_path })
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn load(&self) -> Result<RemoteShellSet> {
        if !self.file_path.exists() {
            return Ok(RemoteShellSet::default());
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

        let content = fs::read_to_string(&self.file_path)?;
        if content.trim().is_empty() {
            return Ok(RemoteShellSet::default());
        }

        let set: RemoteShellSet = serde_json::from_str(&content).map_err(|error| {
            BifrostError::Parse(format!(
                "Failed to parse remote shell store '{}': {}",
                self.file_path.display(),
                error
            ))
        })?;
        Ok(set)
    }

    pub fn save(&self, set: &RemoteShellSet) -> Result<()> {
        let content = serde_json::to_string_pretty(set).map_err(|error| {
            BifrostError::Config(format!("Failed to serialize remote shell store: {}", error))
        })?;

        let temp_path = self.file_path.with_extension("json.tmp");
        fs::write(&temp_path, content)?;
        if let Err(error) = fs::rename(&temp_path, &self.file_path) {
            // Some test environments on macOS have surfaced EINVAL for rename()
            // despite both paths being valid. Fall back to a direct write so the
            // shell policy store remains usable instead of failing the entire flow.
            fs::copy(&temp_path, &self.file_path).map_err(|copy_error| {
                BifrostError::Io(std::io::Error::new(
                    copy_error.kind(),
                    format!(
                        "rename remote shell store failed: {}; fallback copy also failed: {}",
                        error, copy_error
                    ),
                ))
            })?;
            let _ = fs::remove_file(&temp_path);
        }
        Ok(())
    }

    pub fn find_policy(&self, policy_id: &str) -> Result<Option<RemoteShellPolicy>> {
        Ok(self.load()?.find_policy(policy_id).cloned())
    }

    pub fn find_profile(&self, profile_id: &str) -> Result<Option<RemoteShellProfile>> {
        Ok(self.load()?.find_profile(profile_id).cloned())
    }

    pub fn current_version(&self) -> Result<u64> {
        Ok(self.load()?.current_version())
    }
}

impl Default for RemoteShellStore {
    fn default() -> Self {
        Self::new().expect("Failed to create default RemoteShellStore")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_store() -> (TempDir, RemoteShellStore) {
        let temp_dir = TempDir::new().unwrap();
        let store =
            RemoteShellStore::with_file(temp_dir.path().join(REMOTE_SHELL_FILENAME)).unwrap();
        (temp_dir, store)
    }

    fn sample_set() -> RemoteShellSet {
        RemoteShellSet {
            schema_version: REMOTE_SHELL_STORE_SCHEMA_VERSION,
            version: 42,
            policies: vec![RemoteShellPolicy {
                id: "deploy-api".to_string(),
                name: "Deploy API".to_string(),
                description: Some("Allow safe deploy flow".to_string()),
                enabled: true,
                profile_id: Some("locked-down".to_string()),
                metadata: serde_json::json!({
                    "exec_mode": "argv_exec",
                    "risk_level": "medium"
                }),
            }],
            profiles: vec![RemoteShellProfile {
                id: "locked-down".to_string(),
                name: "Locked Down".to_string(),
                description: Some("No network and narrow fs".to_string()),
                enabled: true,
                metadata: serde_json::json!({
                    "network_scope": {"mode": "off"},
                    "filesystem_scope": {"allow": ["/srv/api"]}
                }),
            }],
        }
    }

    #[test]
    fn test_load_missing_file_returns_default_set() {
        let (_temp_dir, store) = setup_store();

        let set = store.load().unwrap();

        assert_eq!(set.schema_version, REMOTE_SHELL_STORE_SCHEMA_VERSION);
        assert_eq!(set.current_version(), 0);
        assert!(set.policies.is_empty());
        assert!(set.profiles.is_empty());
    }

    #[test]
    fn test_save_and_load_round_trip() {
        let (_temp_dir, store) = setup_store();
        let set = sample_set();

        store.save(&set).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded, set);
    }

    #[test]
    fn test_find_policy_returns_match() {
        let (_temp_dir, store) = setup_store();
        store.save(&sample_set()).unwrap();

        let policy = store.find_policy("deploy-api").unwrap().unwrap();

        assert_eq!(policy.name, "Deploy API");
        assert_eq!(policy.profile_id.as_deref(), Some("locked-down"));
    }

    #[test]
    fn test_find_profile_returns_match() {
        let (_temp_dir, store) = setup_store();
        store.save(&sample_set()).unwrap();

        let profile = store.find_profile("locked-down").unwrap().unwrap();

        assert_eq!(profile.name, "Locked Down");
        assert!(profile.enabled);
    }

    #[test]
    fn test_current_version_reads_saved_policy_set_version() {
        let (_temp_dir, store) = setup_store();
        store.save(&sample_set()).unwrap();

        let version = store.current_version().unwrap();

        assert_eq!(version, 42);
    }

    #[test]
    fn test_load_rejects_invalid_json() {
        let (temp_dir, store) = setup_store();
        let file_path = temp_dir.path().join(REMOTE_SHELL_FILENAME);
        fs::write(&file_path, "{not valid json").unwrap();

        let error = store.load().unwrap_err();

        match error {
            BifrostError::Parse(message) => {
                assert!(message.contains("Failed to parse remote shell store"));
            }
            other => panic!("unexpected error: {}", other),
        }
    }

    #[test]
    fn test_load_empty_file_returns_default_set() {
        let (temp_dir, store) = setup_store();
        let file_path = temp_dir.path().join(REMOTE_SHELL_FILENAME);
        fs::write(&file_path, "   \n").unwrap();

        let set = store.load().unwrap();

        assert_eq!(set, RemoteShellSet::default());
    }
}
