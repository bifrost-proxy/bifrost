use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub port: u16,
    #[serde(default)]
    pub socks5_port: Option<u16>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub binary_path: Option<PathBuf>,
}

impl RuntimeInfo {
    pub fn admin_url(&self) -> String {
        let host = self.effective_host();
        format!("http://{}:{}/_bifrost/", host, self.port)
    }

    pub fn http_proxy_url(&self) -> String {
        let host = self.effective_host();
        format!("http://{}:{}", host, self.port)
    }

    pub fn socks5_proxy_url(&self) -> Option<String> {
        self.socks5_port.map(|p| {
            let host = self.effective_host();
            format!("socks5://{}:{}", host, p)
        })
    }

    pub fn effective_host(&self) -> &str {
        match self.host.as_deref() {
            Some("0.0.0.0") | Some("[::]") | Some("::") | None | Some("") => "127.0.0.1",
            Some(h) => h,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Stopped,
    Disconnected,
}

pub fn read_runtime(path: &Path) -> Option<RuntimeInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

#[cfg(unix)]
pub fn is_process_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn is_process_running(pid: u32) -> bool {
    use std::process::Command;
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH"])
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.contains(&pid.to_string())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_runtime_info() {
        let json = r#"{
            "pid": 12345,
            "port": 8800,
            "socks5_port": 1080,
            "host": "127.0.0.1",
            "started_at_ms": 1700000000000,
            "binary_path": "/usr/local/bin/bifrost"
        }"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.pid, 12345);
        assert_eq!(info.port, 8800);
        assert_eq!(info.socks5_port, Some(1080));
        assert_eq!(info.admin_url(), "http://127.0.0.1:8800/_bifrost/");
        assert_eq!(info.http_proxy_url(), "http://127.0.0.1:8800");
        assert_eq!(
            info.socks5_proxy_url(),
            Some("socks5://127.0.0.1:1080".to_string())
        );
    }

    #[test]
    fn test_parse_runtime_without_socks5() {
        let json = r#"{"pid": 100, "port": 9900}"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.socks5_port, None);
        assert_eq!(info.socks5_proxy_url(), None);
        assert_eq!(info.effective_host(), "127.0.0.1");
    }

    #[test]
    fn test_wildcard_host_normalizes() {
        let json = r#"{"pid": 1, "port": 8080, "host": "0.0.0.0"}"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.effective_host(), "127.0.0.1");
        assert_eq!(info.admin_url(), "http://127.0.0.1:8080/_bifrost/");
    }

    #[test]
    fn test_explicit_host_preserved() {
        let json = r#"{"pid": 1, "port": 8080, "host": "192.168.1.100"}"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.effective_host(), "192.168.1.100");
    }
}
