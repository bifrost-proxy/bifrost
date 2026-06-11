use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayConfig {
    pub version: u32,
    #[serde(default)]
    pub items: Vec<CustomMenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomMenuItem {
    pub id: String,
    pub label: String,
    pub action: MenuAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MenuAction {
    OpenAdminRoute { route: String },
    OpenUrl { url: String },
    CopyText { text: String },
    AdminApi { method: String, path: String },
}

const ADMIN_API_PATH_ALLOWLIST: &[&str] = &[
    "/api/proxy/system/refresh",
    "/api/proxy/system/enable",
    "/api/proxy/system/disable",
    "/api/proxy/restart",
    "/api/proxy/stop",
];
const MAX_TRAY_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum ConfigError {
    Json(String),
    TooLarge(u64),
    Version(u32),
    Action { id: String, reason: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid tray.json: {e}"),
            Self::TooLarge(size) => write!(
                f,
                "tray.json is too large: {size} bytes, max {MAX_TRAY_CONFIG_BYTES} bytes"
            ),
            Self::Version(v) => write!(f, "unsupported tray.json version: {v}"),
            Self::Action { id, reason } => {
                write!(f, "invalid action for item '{id}': {reason}")
            }
        }
    }
}

pub fn load_custom_config(data_dir: &Path) -> Result<Option<TrayConfig>, ConfigError> {
    let config_path = data_dir.join("tray.json");
    if !config_path.exists() {
        return Ok(None);
    }

    let mut file =
        std::fs::File::open(&config_path).map_err(|e| ConfigError::Json(e.to_string()))?;
    let mut content = String::new();
    file.by_ref()
        .take(MAX_TRAY_CONFIG_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|e| ConfigError::Json(e.to_string()))?;
    if content.len() as u64 > MAX_TRAY_CONFIG_BYTES {
        return Err(ConfigError::TooLarge(content.len() as u64));
    }

    let config: TrayConfig =
        serde_json::from_str(&content).map_err(|e| ConfigError::Json(e.to_string()))?;

    if config.version != 1 {
        return Err(ConfigError::Version(config.version));
    }

    for item in &config.items {
        validate_action(&item.id, &item.action)?;
    }

    Ok(Some(config))
}

fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

fn validate_action(id: &str, action: &MenuAction) -> Result<(), ConfigError> {
    match action {
        MenuAction::OpenAdminRoute { route } => {
            if has_control_chars(route) {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "route must not contain control characters".to_string(),
                });
            }
            if route.contains("://") {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "open_admin_route must be a relative path, not an absolute URL"
                        .to_string(),
                });
            }
            if !route.starts_with('/') {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "route must start with '/'".to_string(),
                });
            }
        }
        MenuAction::OpenUrl { url } => {
            if has_control_chars(url) {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "url must not contain control characters".to_string(),
                });
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "only http:// and https:// URLs are allowed".to_string(),
                });
            }
        }
        MenuAction::CopyText { .. } => {}
        MenuAction::AdminApi { method, path } => {
            if has_control_chars(path) || has_control_chars(method) {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "admin_api method/path must not contain control characters".to_string(),
                });
            }
            let method_upper = method.to_ascii_uppercase();
            if method_upper != "GET" && method_upper != "POST" {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: "admin_api method must be GET or POST".to_string(),
                });
            }
            if !ADMIN_API_PATH_ALLOWLIST.contains(&path.as_str()) {
                return Err(ConfigError::Action {
                    id: id.to_string(),
                    reason: format!("admin_api path '{path}' is not in the allowlist"),
                });
            }
        }
    }
    Ok(())
}

pub fn expand_template(
    text: &str,
    admin_url: &str,
    http_proxy: &str,
    socks5_proxy: &str,
) -> String {
    text.replace("{admin_url}", admin_url)
        .replace("{http_proxy}", http_proxy)
        .replace("{socks5_proxy}", socks5_proxy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_config() {
        let json = r#"{
            "version": 1,
            "items": [
                {"id": "settings", "label": "Settings", "action": {"type": "open_admin_route", "route": "/settings"}},
                {"id": "docs", "label": "Docs", "action": {"type": "open_url", "url": "https://example.com"}},
                {"id": "copy", "label": "Copy", "action": {"type": "copy_text", "text": "{admin_url}"}},
                {"id": "refresh", "label": "Refresh", "action": {"type": "admin_api", "method": "POST", "path": "/api/proxy/system/refresh"}}
            ]
        }"#;
        let config: TrayConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.items.len(), 4);
        for item in &config.items {
            assert!(validate_action(&item.id, &item.action).is_ok());
        }
    }

    #[test]
    fn test_invalid_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tray.json"),
            r#"{"version": 99, "items": []}"#,
        )
        .unwrap();
        let result = load_custom_config(dir.path());
        assert!(matches!(result, Err(ConfigError::Version(99))));
    }

    #[test]
    fn test_open_admin_route_rejects_absolute_url() {
        let action = MenuAction::OpenAdminRoute {
            route: "http://evil.com/hack".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_admin_route_rejects_no_leading_slash() {
        let action = MenuAction::OpenAdminRoute {
            route: "settings".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_url_rejects_non_http() {
        let action = MenuAction::OpenUrl {
            url: "file:///etc/passwd".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_admin_route_rejects_control_chars() {
        let action = MenuAction::OpenAdminRoute {
            route: "/settings\r\nHost: evil".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_url_rejects_control_chars() {
        let action = MenuAction::OpenUrl {
            url: "https://example.com/\nX".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(result.is_err());
    }

    #[test]
    fn test_admin_api_rejects_unknown_path() {
        let action = MenuAction::AdminApi {
            method: "POST".to_string(),
            path: "/api/secret/admin".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(result.is_err());
    }

    #[test]
    fn test_admin_api_rejects_unknown_method() {
        let action = MenuAction::AdminApi {
            method: "DELETE".to_string(),
            path: "/api/proxy/system/refresh".to_string(),
        };
        let result = validate_action("test", &action);
        assert!(matches!(result, Err(ConfigError::Action { .. })));
    }

    #[test]
    fn test_template_expansion() {
        let result = expand_template(
            "Proxy: {http_proxy}, Admin: {admin_url}, SOCKS: {socks5_proxy}",
            "http://127.0.0.1:8800/_bifrost/",
            "http://127.0.0.1:8800",
            "socks5://127.0.0.1:1080",
        );
        assert_eq!(
            result,
            "Proxy: http://127.0.0.1:8800, Admin: http://127.0.0.1:8800/_bifrost/, SOCKS: socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn test_missing_config_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_custom_config(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tray.json"), "not json").unwrap();
        let result = load_custom_config(dir.path());
        assert!(matches!(result, Err(ConfigError::Json(_))));
    }

    #[test]
    fn test_oversized_config_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let content = format!(
            r#"{{"version":1,"items":[],"padding":"{}"}}"#,
            "x".repeat(MAX_TRAY_CONFIG_BYTES as usize)
        );
        std::fs::write(dir.path().join("tray.json"), content).unwrap();
        let result = load_custom_config(dir.path());
        assert!(matches!(result, Err(ConfigError::TooLarge(_))));
    }
}
