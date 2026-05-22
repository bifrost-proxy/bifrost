use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_ASR_HOST: &str = "127.0.0.1";
pub const DEFAULT_ASR_LANGUAGE: &str = "chinese";
pub const DEFAULT_ASR_MODEL: &str = "Qwen3-ASR-1.7B";
pub const ASR_INSTALL_NAME: &str = "qwen3_asr_rs";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AsrServiceState {
    pub host: String,
    pub port: u16,
    pub model: String,
    pub language: String,
    pub home: PathBuf,
    pub pid: Option<u32>,
    pub managed_by: String,
    #[serde(default)]
    pub owner_module: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
    pub started_at_ms: u64,
}

impl AsrServiceState {
    pub fn lease_owner_module(&self) -> &str {
        self.owner_module
            .as_deref()
            .unwrap_or_else(|| legacy_owner_module(&self.managed_by))
    }
}

fn legacy_owner_module(managed_by: &str) -> &str {
    if managed_by == "webui" {
        "speech_workbench"
    } else {
        managed_by
    }
}

pub fn fixed_asr_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".bifrost/asr"))
        .unwrap_or_else(|| PathBuf::from(".bifrost/asr"))
}

pub fn install_dir(home: &Path) -> PathBuf {
    home.join(ASR_INSTALL_NAME)
}

pub fn model_dir(home: &Path, model: &str) -> PathBuf {
    install_dir(home).join(model)
}

pub fn asr_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("asr")
}

pub fn service_state_path(data_dir: &Path) -> PathBuf {
    asr_data_dir(data_dir).join("service.json")
}

pub fn text_output_dir(data_dir: &Path) -> PathBuf {
    asr_data_dir(data_dir).join("data/text")
}

pub fn read_service_state(data_dir: &Path) -> Option<AsrServiceState> {
    let path = service_state_path(data_dir);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_service_state(data_dir: &Path, state: &AsrServiceState) -> Result<(), String> {
    let path = service_state_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create ASR state dir {}: {error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(state)
        .map_err(|error| format!("serialize ASR service state: {error}"))?;
    std::fs::write(&path, content)
        .map_err(|error| format!("write ASR service state {}: {error}", path.display()))
}

pub fn clear_service_state(data_dir: &Path) -> Result<(), String> {
    let path = service_state_path(data_dir);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("remove ASR service state {}: {error}", path.display()))?;
    }
    Ok(())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn probe_health_blocking(host: &str, port: u16, timeout: Duration) -> Result<(), String> {
    let url = format!("http://{host}:{port}/health");
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    agent
        .get(&url)
        .call()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(unix)]
pub fn stop_pid(pid: u32) -> Result<(), String> {
    let status = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map_err(|error| format!("run kill: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill exited with {status}"))
    }
}

#[cfg(windows)]
pub fn stop_pid(pid: u32) -> Result<(), String> {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
        .map_err(|error| format!("run taskkill: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("taskkill exited with {status}"))
    }
}
