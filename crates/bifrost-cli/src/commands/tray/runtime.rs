use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStartMode {
    Foreground,
    Daemon,
    Desktop,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub port: u16,
    #[serde(default)]
    pub socks5_port: Option<u16>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(rename = "runtime_start_mode", alias = "start_mode", default)]
    pub start_mode: RuntimeStartMode,
    #[serde(default)]
    pub binary_path: Option<PathBuf>,
}

impl RuntimeInfo {
    pub fn is_desktop_owned(&self) -> bool {
        self.start_mode == RuntimeStartMode::Desktop
    }

    pub fn admin_url(&self) -> String {
        let host = self.url_host();
        format!("http://{}:{}/_bifrost/", host, self.port)
    }

    pub fn http_proxy_url(&self) -> String {
        let host = self.url_host();
        format!("http://{}:{}", host, self.port)
    }

    pub fn socks5_proxy_url(&self) -> Option<String> {
        let host = self.url_host();
        Some(format!(
            "socks5://{}:{}",
            host,
            self.socks5_port.unwrap_or(self.port)
        ))
    }

    pub fn effective_host(&self) -> &str {
        match self.host.as_deref() {
            Some("0.0.0.0") | Some("[::]") | Some("::") | None | Some("") => "127.0.0.1",
            Some(h) => h,
        }
    }

    pub fn url_host(&self) -> String {
        let host = self.effective_host();
        if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
            format!("[{host}]")
        } else {
            host.to_string()
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
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }

    let mut exit_code = 0_u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    unsafe {
        CloseHandle(handle);
    }

    ok != 0 && exit_code == STILL_ACTIVE as u32
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
            "runtime_start_mode": "desktop",
            "binary_path": "/usr/local/bin/bifrost",
            "system_proxy_enabled": true,
            "system_proxy_bypass": "localhost,127.0.0.1"
        }"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.pid, 12345);
        assert_eq!(info.port, 8800);
        assert_eq!(info.socks5_port, Some(1080));
        assert_eq!(info.start_mode, RuntimeStartMode::Desktop);
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
        assert_eq!(info.start_mode, RuntimeStartMode::Unknown);
        assert_eq!(
            info.socks5_proxy_url(),
            Some("socks5://127.0.0.1:9900".to_string())
        );
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

    #[test]
    fn test_ipv6_host_is_bracketed_for_urls() {
        let json = r#"{"pid": 1, "port": 8080, "socks5_port": 1080, "host": "::1"}"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.effective_host(), "::1");
        assert_eq!(info.url_host(), "[::1]");
        assert_eq!(info.admin_url(), "http://[::1]:8080/_bifrost/");
        assert_eq!(info.http_proxy_url(), "http://[::1]:8080");
        assert_eq!(
            info.socks5_proxy_url(),
            Some("socks5://[::1]:1080".to_string())
        );
    }

    #[test]
    fn test_bracketed_ipv6_host_is_not_double_bracketed() {
        let json = r#"{"pid": 1, "port": 8080, "host": "[::1]"}"#;
        let info: RuntimeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.admin_url(), "http://[::1]:8080/_bifrost/");
    }
}
