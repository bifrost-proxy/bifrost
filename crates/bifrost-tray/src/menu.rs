use crate::config::{self, CustomMenuItem, MenuAction, TrayConfig};
use crate::runtime::{RuntimeInfo, ServiceState};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MenuItemDef {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub action: MenuItemAction,
}

#[derive(Debug, Clone)]
pub enum MenuItemAction {
    OpenUrl(String),
    CopyText(String),
    AdminApi { method: String, url: String },
    StartService,
    StopService,
    OpenDirectory(String),
    ReloadMenu,
    QuitTray,
    None,
}

pub fn build_menu(
    runtime: Option<&RuntimeInfo>,
    state: ServiceState,
    custom_config: Option<&TrayConfig>,
    data_dir: &str,
) -> Vec<MenuItemDef> {
    let mut items = Vec::new();
    let is_running = state == ServiceState::Running;

    let status_label = match (state, runtime) {
        (ServiceState::Running, Some(rt)) => {
            format!("Bifrost: Running on {}:{}", rt.effective_host(), rt.port)
        }
        (ServiceState::Stopped, _) => "Bifrost: Stopped".to_string(),
        (ServiceState::Disconnected, _) => "Bifrost: Disconnected".to_string(),
        _ => "Bifrost: Unknown".to_string(),
    };

    items.push(MenuItemDef {
        id: "_status".to_string(),
        label: status_label,
        enabled: false,
        action: MenuItemAction::None,
    });

    if let Some(rt) = runtime {
        let admin_url = rt.admin_url();

        items.push(MenuItemDef {
            id: "open_admin_ui".to_string(),
            label: "Open Admin UI".to_string(),
            enabled: is_running,
            action: MenuItemAction::OpenUrl(admin_url.clone()),
        });

        items.push(MenuItemDef {
            id: "open_traffic".to_string(),
            label: "Open Traffic".to_string(),
            enabled: is_running,
            action: MenuItemAction::OpenUrl(format!("{}traffic", admin_url)),
        });

        items.push(MenuItemDef {
            id: "open_rules".to_string(),
            label: "Open Rules".to_string(),
            enabled: is_running,
            action: MenuItemAction::OpenUrl(format!("{}rules", admin_url)),
        });

        items.push(MenuItemDef {
            id: "copy_admin_url".to_string(),
            label: "Copy Admin URL".to_string(),
            enabled: is_running,
            action: MenuItemAction::CopyText(admin_url.clone()),
        });

        items.push(MenuItemDef {
            id: "copy_http_proxy".to_string(),
            label: "Copy HTTP Proxy".to_string(),
            enabled: is_running,
            action: MenuItemAction::CopyText(rt.http_proxy_url()),
        });

        let has_socks5 = rt.socks5_port.is_some();
        items.push(MenuItemDef {
            id: "copy_socks5_proxy".to_string(),
            label: "Copy SOCKS5 Proxy".to_string(),
            enabled: is_running && has_socks5,
            action: MenuItemAction::CopyText(rt.socks5_proxy_url().unwrap_or_default()),
        });
    }

    if let Some(cfg) = custom_config {
        if !cfg.items.is_empty() {
            items.push(MenuItemDef {
                id: "_sep_custom".to_string(),
                label: "-".to_string(),
                enabled: false,
                action: MenuItemAction::None,
            });
            for item in &cfg.items {
                items.push(custom_item_to_def(item, runtime, is_running));
            }
        }
    }

    items.push(MenuItemDef {
        id: "_sep_actions".to_string(),
        label: "-".to_string(),
        enabled: false,
        action: MenuItemAction::None,
    });

    if is_running {
        items.push(MenuItemDef {
            id: "toggle_service".to_string(),
            label: "Stop Bifrost".to_string(),
            enabled: true,
            action: MenuItemAction::StopService,
        });
    } else {
        items.push(MenuItemDef {
            id: "toggle_service".to_string(),
            label: "Start Bifrost".to_string(),
            enabled: true,
            action: MenuItemAction::StartService,
        });
    }

    items.push(MenuItemDef {
        id: "open_data_dir".to_string(),
        label: "Open Data Directory".to_string(),
        enabled: true,
        action: MenuItemAction::OpenDirectory(data_dir.to_string()),
    });

    items.push(MenuItemDef {
        id: "open_logs".to_string(),
        label: "Open Logs".to_string(),
        enabled: true,
        action: MenuItemAction::OpenDirectory(format!("{}/logs", data_dir)),
    });

    items.push(MenuItemDef {
        id: "reload_menu".to_string(),
        label: "Reload Tray Menu".to_string(),
        enabled: true,
        action: MenuItemAction::ReloadMenu,
    });

    items.push(MenuItemDef {
        id: "_sep_quit".to_string(),
        label: "-".to_string(),
        enabled: false,
        action: MenuItemAction::None,
    });

    items.push(MenuItemDef {
        id: "quit_tray".to_string(),
        label: "Quit Tray".to_string(),
        enabled: true,
        action: MenuItemAction::QuitTray,
    });

    items
}

fn custom_item_to_def(
    item: &CustomMenuItem,
    runtime: Option<&RuntimeInfo>,
    is_running: bool,
) -> MenuItemDef {
    let admin_url = runtime.map(|rt| rt.admin_url()).unwrap_or_default();
    let http_proxy = runtime.map(|rt| rt.http_proxy_url()).unwrap_or_default();
    let socks5_proxy = runtime
        .and_then(|rt| rt.socks5_proxy_url())
        .unwrap_or_default();

    let action = match &item.action {
        MenuAction::OpenAdminRoute { route } => {
            let base = admin_url.trim_end_matches('/');
            MenuItemAction::OpenUrl(format!("{}{}", base, route))
        }
        MenuAction::OpenUrl { url } => MenuItemAction::OpenUrl(url.clone()),
        MenuAction::CopyText { text } => {
            let expanded = config::expand_template(text, &admin_url, &http_proxy, &socks5_proxy);
            MenuItemAction::CopyText(expanded)
        }
        MenuAction::AdminApi { method, path } => {
            let url = format!(
                "http://{}:{}{}",
                runtime.map(|r| r.effective_host()).unwrap_or("127.0.0.1"),
                runtime.map(|r| r.port).unwrap_or(9900),
                path
            );
            MenuItemAction::AdminApi {
                method: method.clone(),
                url,
            }
        }
    };

    MenuItemDef {
        id: item.id.clone(),
        label: item.label.clone(),
        enabled: is_running,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_runtime() -> RuntimeInfo {
        RuntimeInfo {
            pid: 1234,
            port: 8800,
            socks5_port: Some(1080),
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            binary_path: None,
        }
    }

    #[test]
    fn test_menu_running_state() {
        let rt = sample_runtime();
        let menu = build_menu(Some(&rt), ServiceState::Running, None, "/tmp/.bifrost");
        assert!(menu[0].label.contains("Running on 127.0.0.1:8800"));
        let open_admin = menu.iter().find(|m| m.id == "open_admin_ui").unwrap();
        assert!(open_admin.enabled);
    }

    #[test]
    fn test_menu_stopped_state() {
        let rt = sample_runtime();
        let menu = build_menu(Some(&rt), ServiceState::Stopped, None, "/tmp/.bifrost");
        assert!(menu[0].label.contains("Stopped"));
        let open_admin = menu.iter().find(|m| m.id == "open_admin_ui").unwrap();
        assert!(!open_admin.enabled);
        let quit = menu.iter().find(|m| m.id == "quit_tray").unwrap();
        assert!(quit.enabled);
    }

    #[test]
    fn test_menu_no_socks5_disabled() {
        let rt = RuntimeInfo {
            socks5_port: None,
            ..sample_runtime()
        };
        let menu = build_menu(Some(&rt), ServiceState::Running, None, "/tmp/.bifrost");
        let socks5 = menu.iter().find(|m| m.id == "copy_socks5_proxy").unwrap();
        assert!(!socks5.enabled);
    }

    #[test]
    fn test_custom_items_included() {
        let rt = sample_runtime();
        let config = TrayConfig {
            version: 1,
            items: vec![CustomMenuItem {
                id: "custom1".to_string(),
                label: "My Item".to_string(),
                action: MenuAction::OpenUrl {
                    url: "https://example.com".to_string(),
                },
            }],
        };
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            Some(&config),
            "/tmp/.bifrost",
        );
        let custom = menu.iter().find(|m| m.id == "custom1").unwrap();
        assert_eq!(custom.label, "My Item");
        assert!(custom.enabled);
    }
}
