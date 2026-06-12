use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_ASR_HOST: &str = "127.0.0.1";
pub const DEFAULT_ASR_LANGUAGE: &str = "chinese";
pub const DEFAULT_ASR_MODEL: &str = "Qwen3-ASR-0.6B";
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    #[test]
    fn service_state_round_trip_read_write_and_clear() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();

        let state = AsrServiceState {
            host: "127.0.0.1".to_string(),
            port: 12345,
            model: DEFAULT_ASR_MODEL.to_string(),
            language: DEFAULT_ASR_LANGUAGE.to_string(),
            home: PathBuf::from("/tmp/asr_home"),
            pid: Some(42),
            managed_by: "cli".to_string(),
            owner_module: Some("bifrost_cli".to_string()),
            owner_id: Some("task-1".to_string()),
            started_at_ms: 1,
        };

        write_service_state(data_dir, &state).expect("write state");
        let path = service_state_path(data_dir);
        assert!(path.is_file());

        let loaded = read_service_state(data_dir).expect("state should be readable");
        assert_eq!(loaded, state);

        clear_service_state(data_dir).expect("clear state");
        assert!(!path.exists());
        assert!(read_service_state(data_dir).is_none());
    }

    #[test]
    fn lease_owner_module_prefers_owner_module_and_maps_webui() {
        fn make_state(managed_by: &str, owner_module: Option<&str>) -> AsrServiceState {
            AsrServiceState {
                host: "127.0.0.1".to_string(),
                port: 80,
                model: DEFAULT_ASR_MODEL.to_string(),
                language: DEFAULT_ASR_LANGUAGE.to_string(),
                home: PathBuf::from("/tmp/asr_home"),
                pid: None,
                managed_by: managed_by.to_string(),
                owner_module: owner_module.map(|s| s.to_string()),
                owner_id: None,
                started_at_ms: 0,
            }
        }

        let legacy = make_state("webui", None);
        assert_eq!(legacy.lease_owner_module(), "speech_workbench");

        let cli = make_state("cli", None);
        assert_eq!(cli.lease_owner_module(), "cli");

        let owned = make_state("webui", Some("override"));
        assert_eq!(owned.lease_owner_module(), "override");
    }

    #[test]
    fn path_helpers_use_expected_layout() {
        let home = Path::new("/home/user");
        let install = install_dir(home);
        assert!(install.ends_with(ASR_INSTALL_NAME));

        let model = model_dir(home, "Qwen3-ASR-0.6B");
        assert_eq!(model.parent().unwrap(), &install);

        let data_dir = Path::new("/data");
        let asr_dir = asr_data_dir(data_dir);
        assert_eq!(asr_dir, PathBuf::from("/data/asr"));
        assert_eq!(service_state_path(data_dir), asr_dir.join("service.json"));
        assert_eq!(text_output_dir(data_dir), asr_dir.join("data/text"));
    }

    #[test]
    fn fixed_asr_home_has_asr_suffix() {
        let home = fixed_asr_home();
        assert_eq!(home.file_name().unwrap(), "asr");
    }

    #[test]
    fn now_ms_returns_positive_timestamp() {
        let now = now_ms();
        assert!(now > 0);
    }

    #[test]
    fn probe_health_blocking_succeeds_against_local_http_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf);
                let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response);
            }
        });

        probe_health_blocking("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stop_pid_terminates_child_process() {
        use std::process::{Command, Stdio};

        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 60")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();

        stop_pid(pid).expect("stop child");

        let status = child.wait().expect("wait child");
        assert!(!status.success());
    }
}
