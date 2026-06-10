use super::config::{self, CustomMenuItem, MenuAction, TrayConfig};
use super::runtime::{RuntimeInfo, ServiceState};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MenuEntry {
    Item(MenuItemDef),
    Submenu(SubmenuDef),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SubmenuDef {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub children: Vec<MenuEntry>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MenuItemDef {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
    pub action: MenuItemAction,
}

#[derive(Debug, Clone)]
pub enum MenuItemAction {
    OpenUrl(String),
    CopyText(String),
    AdminApi {
        method: String,
        url: String,
    },
    StartService,
    StopService,
    RestartService,
    OpenDirectory(String),
    SelectRule {
        target: RuleTarget,
        all_targets: Vec<RuleTarget>,
    },
    QuitTray,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleTarget {
    Personal { name: String },
    Group { group_name: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayRule {
    pub target: RuleTarget,
    pub enabled: bool,
    pub sort_order: i32,
}

#[allow(clippy::too_many_arguments)]
pub fn build_menu(
    runtime: Option<&RuntimeInfo>,
    state: ServiceState,
    status_override: Option<&str>,
    service_action_busy: bool,
    custom_config: Option<&TrayConfig>,
    data_dir: &str,
    bin_available: bool,
    rules: &[TrayRule],
) -> Vec<MenuEntry> {
    let mut items = Vec::new();
    let is_running = state == ServiceState::Running;

    let status_label =
        status_override
            .map(str::to_string)
            .unwrap_or_else(|| match (state, runtime) {
                (ServiceState::Running, Some(rt)) => {
                    format!("Bifrost: Running on {}:{}", rt.effective_host(), rt.port)
                }
                (ServiceState::Stopped, _) => "Bifrost: Stopped".to_string(),
                (ServiceState::Disconnected, _) => "Bifrost: Disconnected".to_string(),
                _ => "Bifrost: Unknown".to_string(),
            });

    items.push(item(MenuItemDef {
        id: "_status".to_string(),
        label: status_label,
        enabled: false,
        checked: false,
        action: MenuItemAction::None,
    }));

    if let Some(rt) = runtime {
        let admin_url = rt.admin_url();

        items.push(item(MenuItemDef {
            id: "open_admin_ui".to_string(),
            label: "Open Admin UI".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::OpenUrl(admin_url.clone()),
        }));

        items.push(item(MenuItemDef {
            id: "open_traffic".to_string(),
            label: "Open Traffic".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::OpenUrl(format!("{}traffic", admin_url)),
        }));

        items.push(item(MenuItemDef {
            id: "open_rules".to_string(),
            label: "Open Rules".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::OpenUrl(format!("{}rules", admin_url)),
        }));

        items.push(item(MenuItemDef {
            id: "copy_admin_url".to_string(),
            label: "Copy Admin URL".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::CopyText(admin_url.clone()),
        }));

        items.push(item(MenuItemDef {
            id: "copy_http_proxy".to_string(),
            label: "Copy HTTP Proxy".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::CopyText(rt.http_proxy_url()),
        }));

        let has_socks5 = rt.socks5_port.is_some();
        items.push(item(MenuItemDef {
            id: "copy_socks5_proxy".to_string(),
            label: "Copy SOCKS5 Proxy".to_string(),
            enabled: is_running && has_socks5,
            checked: false,
            action: MenuItemAction::CopyText(rt.socks5_proxy_url().unwrap_or_default()),
        }));
    }

    if let Some(rules_menu) = build_rules_menu(rules, is_running) {
        items.push(MenuEntry::Submenu(rules_menu));
    }

    if let Some(cfg) = custom_config {
        if !cfg.items.is_empty() {
            items.push(separator("_sep_custom"));
            for item in &cfg.items {
                items.push(MenuEntry::Item(custom_item_to_def(
                    item, runtime, is_running,
                )));
            }
        }
    }

    items.push(separator("_sep_actions"));

    if is_running {
        items.push(item(MenuItemDef {
            id: "toggle_service".to_string(),
            label: "Stop Bifrost".to_string(),
            enabled: bin_available && !service_action_busy,
            checked: false,
            action: MenuItemAction::StopService,
        }));
        items.push(item(MenuItemDef {
            id: "restart_service".to_string(),
            label: "Restart Bifrost".to_string(),
            enabled: bin_available && !service_action_busy,
            checked: false,
            action: MenuItemAction::RestartService,
        }));
    } else {
        items.push(item(MenuItemDef {
            id: "toggle_service".to_string(),
            label: "Start Bifrost".to_string(),
            enabled: bin_available && !service_action_busy,
            checked: false,
            action: MenuItemAction::StartService,
        }));
    }

    items.push(item(MenuItemDef {
        id: "open_data_dir".to_string(),
        label: "Open Data Directory".to_string(),
        enabled: true,
        checked: false,
        action: MenuItemAction::OpenDirectory(data_dir.to_string()),
    }));

    items.push(item(MenuItemDef {
        id: "open_logs".to_string(),
        label: "Open Logs".to_string(),
        enabled: true,
        checked: false,
        action: MenuItemAction::OpenDirectory(format!("{}/logs", data_dir)),
    }));

    items.push(separator("_sep_quit"));

    items.push(item(MenuItemDef {
        id: "quit_tray".to_string(),
        label: "Quit Tray".to_string(),
        enabled: true,
        checked: false,
        action: MenuItemAction::QuitTray,
    }));

    items
}

fn item(def: MenuItemDef) -> MenuEntry {
    MenuEntry::Item(def)
}

fn separator(id: &str) -> MenuEntry {
    item(MenuItemDef {
        id: id.to_string(),
        label: "-".to_string(),
        enabled: false,
        checked: false,
        action: MenuItemAction::None,
    })
}

fn build_rules_menu(rules: &[TrayRule], is_running: bool) -> Option<SubmenuDef> {
    if rules.is_empty() {
        return None;
    }

    let all_targets = rules
        .iter()
        .map(|rule| rule.target.clone())
        .collect::<Vec<_>>();
    let active_label = active_rule_label(rules);
    let has_group_rules = rules
        .iter()
        .any(|rule| matches!(rule.target, RuleTarget::Group { .. }));
    let label = match active_label {
        Some(name) => format!("Rules: {name}"),
        None => "Rules: None".to_string(),
    };

    let children = if has_group_rules {
        build_grouped_rule_entries(rules, &all_targets, is_running)
    } else {
        rules
            .iter()
            .enumerate()
            .map(|(index, rule)| rule_entry(rule, index, &all_targets, is_running))
            .collect()
    };

    Some(SubmenuDef {
        id: "rules_switcher".to_string(),
        label,
        enabled: true,
        children,
    })
}

fn active_rule_label(rules: &[TrayRule]) -> Option<String> {
    let mut active = rules.iter().filter(|rule| rule.enabled);
    let first = active.next()?;
    if active.next().is_some() {
        Some("Multiple".to_string())
    } else {
        Some(rule_label(&first.target))
    }
}

fn build_grouped_rule_entries(
    rules: &[TrayRule],
    all_targets: &[RuleTarget],
    is_running: bool,
) -> Vec<MenuEntry> {
    let mut children = Vec::new();
    let personal_rules = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| matches!(rule.target, RuleTarget::Personal { .. }))
        .collect::<Vec<_>>();
    if !personal_rules.is_empty() {
        let personal_children = personal_rules
            .into_iter()
            .map(|(index, rule)| rule_entry(rule, index, all_targets, is_running))
            .collect();
        children.push(MenuEntry::Submenu(SubmenuDef {
            id: "rules_my_rules".to_string(),
            label: "My Rules".to_string(),
            enabled: true,
            children: personal_children,
        }));
    }

    let mut group_names = rules
        .iter()
        .filter_map(|rule| match &rule.target {
            RuleTarget::Group { group_name, .. } => Some(group_name.clone()),
            RuleTarget::Personal { .. } => None,
        })
        .collect::<Vec<_>>();
    group_names.sort();
    group_names.dedup();

    for group_name in group_names {
        let group_children = rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| match &rule.target {
                RuleTarget::Group {
                    group_name: name, ..
                } => name == &group_name,
                RuleTarget::Personal { .. } => false,
            })
            .map(|(index, rule)| rule_entry(rule, index, all_targets, is_running))
            .collect();
        children.push(MenuEntry::Submenu(SubmenuDef {
            id: format!("rules_group_{}", sanitize_menu_id(&group_name)),
            label: group_name,
            enabled: true,
            children: group_children,
        }));
    }

    children
}

fn rule_entry(
    rule: &TrayRule,
    index: usize,
    all_targets: &[RuleTarget],
    is_running: bool,
) -> MenuEntry {
    item(MenuItemDef {
        id: format!("select_rule_{index}"),
        label: short_rule_label(&rule.target),
        enabled: is_running,
        checked: rule.enabled,
        action: MenuItemAction::SelectRule {
            target: rule.target.clone(),
            all_targets: all_targets.to_vec(),
        },
    })
}

fn rule_label(target: &RuleTarget) -> String {
    match target {
        RuleTarget::Personal { name } => name.clone(),
        RuleTarget::Group { group_name, name } => format!("{group_name}/{name}"),
    }
}

fn short_rule_label(target: &RuleTarget) -> String {
    match target {
        RuleTarget::Personal { name } | RuleTarget::Group { name, .. } => name.clone(),
    }
}

fn sanitize_menu_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn sort_tray_rules(rules: &mut [TrayRule]) {
    rules.sort_by_key(|rule| rule_sort_key(&rule.target));
}

fn rule_sort_key(target: &RuleTarget) -> (u8, String, String) {
    match target {
        RuleTarget::Personal { name } => (0, name.to_ascii_lowercase(), String::new()),
        RuleTarget::Group { group_name, name } => (
            1,
            group_name.to_ascii_lowercase(),
            name.to_ascii_lowercase(),
        ),
    }
}

#[cfg(test)]
pub fn build_rules_menu_for_test(rules: &[TrayRule], is_running: bool) -> Option<SubmenuDef> {
    build_rules_menu(rules, is_running)
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
            let base = admin_url.trim_end_matches('/');
            let url = format!("{base}{path}");
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
        checked: false,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_item<'a>(entries: &'a [MenuEntry], id: &str) -> Option<&'a MenuItemDef> {
        for entry in entries {
            match entry {
                MenuEntry::Item(item) if item.id == id => return Some(item),
                MenuEntry::Submenu(submenu) => {
                    if let Some(item) = find_item(&submenu.children, id) {
                        return Some(item);
                    }
                }
                MenuEntry::Item(_) => {}
            }
        }
        None
    }

    fn find_submenu<'a>(entries: &'a [MenuEntry], id: &str) -> Option<&'a SubmenuDef> {
        for entry in entries {
            if let MenuEntry::Submenu(submenu) = entry {
                if submenu.id == id {
                    return Some(submenu);
                }
                if let Some(found) = find_submenu(&submenu.children, id) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn personal_rule(name: &str, enabled: bool) -> TrayRule {
        TrayRule {
            target: RuleTarget::Personal {
                name: name.to_string(),
            },
            enabled,
            sort_order: 0,
        }
    }

    fn group_rule(group_name: &str, name: &str, enabled: bool) -> TrayRule {
        TrayRule {
            target: RuleTarget::Group {
                group_name: group_name.to_string(),
                name: name.to_string(),
            },
            enabled,
            sort_order: 0,
        }
    }

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
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[],
        );
        let status = find_item(&menu, "_status").unwrap();
        assert!(status.label.contains("Running on 127.0.0.1:8800"));
        let open_admin = find_item(&menu, "open_admin_ui").unwrap();
        assert!(open_admin.enabled);
        // Restart is offered when running
        let restart = find_item(&menu, "restart_service").unwrap();
        assert!(restart.enabled);
    }

    #[test]
    fn test_menu_stopped_state() {
        let rt = sample_runtime();
        let menu = build_menu(
            Some(&rt),
            ServiceState::Stopped,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[],
        );
        let status = find_item(&menu, "_status").unwrap();
        assert!(status.label.contains("Stopped"));
        let open_admin = find_item(&menu, "open_admin_ui").unwrap();
        assert!(!open_admin.enabled);
        let quit = find_item(&menu, "quit_tray").unwrap();
        assert!(quit.enabled);
    }

    #[test]
    fn test_service_action_busy_overrides_status_and_disables_start() {
        let rt = sample_runtime();
        let menu = build_menu(
            Some(&rt),
            ServiceState::Stopped,
            Some("Bifrost: Starting..."),
            true,
            None,
            "/tmp/.bifrost",
            true,
            &[],
        );
        let status = find_item(&menu, "_status").unwrap();
        assert_eq!(status.label, "Bifrost: Starting...");
        let start = find_item(&menu, "toggle_service").unwrap();
        assert_eq!(start.label, "Start Bifrost");
        assert!(!start.enabled);
    }

    #[test]
    fn test_service_action_failure_status_allows_retry() {
        let rt = sample_runtime();
        let menu = build_menu(
            Some(&rt),
            ServiceState::Disconnected,
            Some("Bifrost: Start failed - open logs"),
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[],
        );
        let status = find_item(&menu, "_status").unwrap();
        assert_eq!(status.label, "Bifrost: Start failed - open logs");
        let start = find_item(&menu, "toggle_service").unwrap();
        assert!(start.enabled);
    }

    #[test]
    fn test_service_controls_disabled_without_bin() {
        let rt = sample_runtime();
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            false,
            &[],
        );
        let stop = find_item(&menu, "toggle_service").unwrap();
        assert!(!stop.enabled);
        let restart = find_item(&menu, "restart_service").unwrap();
        assert!(!restart.enabled);
    }

    #[test]
    fn test_menu_no_socks5_disabled() {
        let rt = RuntimeInfo {
            socks5_port: None,
            ..sample_runtime()
        };
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &[],
        );
        let socks5 = find_item(&menu, "copy_socks5_proxy").unwrap();
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
            None,
            false,
            Some(&config),
            "/tmp/.bifrost",
            true,
            &[],
        );
        let custom = find_item(&menu, "custom1").unwrap();
        assert_eq!(custom.label, "My Item");
        assert!(custom.enabled);
    }

    #[test]
    fn test_admin_api_custom_item_uses_admin_base_url() {
        let rt = sample_runtime();
        let config = TrayConfig {
            version: 1,
            items: vec![CustomMenuItem {
                id: "refresh".to_string(),
                label: "Refresh".to_string(),
                action: MenuAction::AdminApi {
                    method: "POST".to_string(),
                    path: "/api/proxy/system/refresh".to_string(),
                },
            }],
        };
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            Some(&config),
            "/tmp/.bifrost",
            true,
            &[],
        );
        let custom = find_item(&menu, "refresh").unwrap();
        match &custom.action {
            MenuItemAction::AdminApi { method, url } => {
                assert_eq!(method, "POST");
                assert_eq!(
                    url,
                    "http://127.0.0.1:8800/_bifrost/api/proxy/system/refresh"
                );
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn test_rules_menu_two_levels_without_groups() {
        let rules = vec![personal_rule("alpha", true), personal_rule("beta", false)];

        let menu = build_rules_menu_for_test(&rules, true).unwrap();
        assert_eq!(menu.label, "Rules: alpha");
        assert_eq!(menu.children.len(), 2);
        assert!(find_submenu(&menu.children, "rules_my_rules").is_none());
        let alpha = find_item(&menu.children, "select_rule_0").unwrap();
        let beta = find_item(&menu.children, "select_rule_1").unwrap();
        assert_eq!(alpha.label, "alpha");
        assert!(alpha.checked);
        assert_eq!(beta.label, "beta");
        assert!(!beta.checked);
    }

    #[test]
    fn test_rules_menu_three_levels_with_groups() {
        let rules = vec![
            personal_rule("personal", false),
            group_rule("Team A", "shared", true),
        ];

        let menu = build_rules_menu_for_test(&rules, true).unwrap();
        assert_eq!(menu.label, "Rules: Team A/shared");
        let personal = find_submenu(&menu.children, "rules_my_rules").unwrap();
        let group = find_submenu(&menu.children, "rules_group_Team_A").unwrap();
        assert_eq!(personal.label, "My Rules");
        assert_eq!(group.label, "Team A");
        assert!(find_item(&group.children, "select_rule_1").unwrap().checked);
    }
}
