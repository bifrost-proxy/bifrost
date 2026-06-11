use super::config::{self, CustomMenuItem, MenuAction, TrayConfig};
use super::runtime::{RuntimeInfo, ServiceState};
use serde::{Deserialize, Serialize};

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
    SetSystemProxy {
        url: String,
        enabled: bool,
    },
    StartService,
    StopService,
    OpenDirectory(String),
    SelectRule {
        target: RuleTarget,
        enabled_targets: Vec<RuleTarget>,
        currently_enabled: bool,
    },
    QuitTray,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleTarget {
    Personal { name: String },
    Group { group_name: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayRule {
    pub target: RuleTarget,
    pub enabled: bool,
    pub sort_order: i32,
    pub managed_group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxyMenuState {
    pub supported: bool,
    pub enabled: bool,
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
    recent_rule_targets: &[RuleTarget],
    system_proxy: Option<&SystemProxyMenuState>,
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

        items.push(item(MenuItemDef {
            id: "copy_socks5_proxy".to_string(),
            label: "Copy SOCKS5 Proxy".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::CopyText(rt.socks5_proxy_url().unwrap_or_default()),
        }));
    }

    let admin_rules_url = runtime.map(|rt| format!("{}rules", rt.admin_url()));
    if let Some(rules_menu) = build_rules_menu(
        rules,
        recent_rule_targets,
        is_running,
        admin_rules_url.as_deref(),
    ) {
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
    } else {
        items.push(item(MenuItemDef {
            id: "toggle_service".to_string(),
            label: "Start Bifrost".to_string(),
            enabled: bin_available && !service_action_busy,
            checked: false,
            action: MenuItemAction::StartService,
        }));
    }

    if let (Some(rt), Some(system_proxy)) = (runtime, system_proxy) {
        let admin_url = rt.admin_url();
        items.push(item(MenuItemDef {
            id: "toggle_system_proxy".to_string(),
            label: "System Proxy".to_string(),
            enabled: is_running && system_proxy.supported && !service_action_busy,
            checked: system_proxy.enabled,
            action: MenuItemAction::SetSystemProxy {
                url: format!("{}api/proxy/system", admin_url),
                enabled: !system_proxy.enabled,
            },
        }));
    }

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

fn build_rules_menu(
    rules: &[TrayRule],
    recent_rule_targets: &[RuleTarget],
    is_running: bool,
    admin_rules_url: Option<&str>,
) -> Option<SubmenuDef> {
    if rules.is_empty() {
        if !is_running {
            return None;
        }
        return Some(SubmenuDef {
            id: "rules_switcher".to_string(),
            label: "Rules: None".to_string(),
            enabled: true,
            children: vec![item(MenuItemDef {
                id: "rules_empty".to_string(),
                label: "No rules available".to_string(),
                enabled: false,
                checked: false,
                action: MenuItemAction::None,
            })],
        });
    }

    let enabled_targets = rules
        .iter()
        .filter(|rule| rule.enabled)
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

    let mut children =
        build_recent_rule_entries(rules, recent_rule_targets, &enabled_targets, is_running);
    let grouped_children = if has_group_rules {
        build_grouped_rule_entries(rules, &enabled_targets, is_running, admin_rules_url)
    } else {
        rules
            .iter()
            .enumerate()
            .map(|(index, rule)| rule_entry(rule, index, &enabled_targets, is_running))
            .collect()
    };
    if !children.is_empty() && !grouped_children.is_empty() {
        children.push(separator("rules_recent_separator"));
    }
    children.extend(grouped_children);

    Some(SubmenuDef {
        id: "rules_switcher".to_string(),
        label,
        enabled: true,
        children,
    })
}

fn build_recent_rule_entries(
    rules: &[TrayRule],
    recent_rule_targets: &[RuleTarget],
    enabled_targets: &[RuleTarget],
    is_running: bool,
) -> Vec<MenuEntry> {
    let mut children = Vec::new();
    let mut seen = Vec::<RuleTarget>::new();
    for target in recent_rule_targets {
        if seen.contains(target) {
            continue;
        }
        let Some(rule) = rules.iter().find(|rule| &rule.target == target) else {
            continue;
        };
        seen.push(target.clone());
        children.push(item(MenuItemDef {
            id: format!("recent_rule_{}", children.len()),
            label: rule_label(&rule.target),
            enabled: is_running,
            checked: rule.enabled,
            action: MenuItemAction::SelectRule {
                target: rule.target.clone(),
                enabled_targets: other_enabled_targets(enabled_targets, &rule.target),
                currently_enabled: rule.enabled,
            },
        }));
        if children.len() >= 5 {
            break;
        }
    }
    children
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
    enabled_targets: &[RuleTarget],
    is_running: bool,
    admin_rules_url: Option<&str>,
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
            .map(|(index, rule)| rule_entry(rule, index, enabled_targets, is_running))
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
            RuleTarget::Group { group_name, .. } if rule.managed_group => Some(group_name.clone()),
            RuleTarget::Personal { .. } => None,
            RuleTarget::Group { .. } => None,
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
                } => name == &group_name && rule.managed_group,
                RuleTarget::Personal { .. } => false,
            })
            .map(|(index, rule)| rule_entry(rule, index, enabled_targets, is_running))
            .collect();
        children.push(MenuEntry::Submenu(SubmenuDef {
            id: format!("rules_group_{}", sanitize_menu_id(&group_name)),
            label: group_name,
            enabled: true,
            children: group_children,
        }));
    }

    if rules
        .iter()
        .any(|rule| !rule.managed_group && matches!(rule.target, RuleTarget::Group { .. }))
    {
        if !children.is_empty() {
            children.push(separator("rules_more_separator"));
        }
        children.push(item(MenuItemDef {
            id: "rules_more".to_string(),
            label: "More...".to_string(),
            enabled: is_running && admin_rules_url.is_some(),
            checked: false,
            action: admin_rules_url
                .map(|url| MenuItemAction::OpenUrl(url.to_string()))
                .unwrap_or(MenuItemAction::None),
        }));
    }

    children
}

fn rule_entry(
    rule: &TrayRule,
    index: usize,
    enabled_targets: &[RuleTarget],
    is_running: bool,
) -> MenuEntry {
    item(MenuItemDef {
        id: format!("select_rule_{index}"),
        label: short_rule_label(&rule.target),
        enabled: is_running,
        checked: rule.enabled,
        action: MenuItemAction::SelectRule {
            target: rule.target.clone(),
            enabled_targets: other_enabled_targets(enabled_targets, &rule.target),
            currently_enabled: rule.enabled,
        },
    })
}

fn other_enabled_targets(enabled_targets: &[RuleTarget], target: &RuleTarget) -> Vec<RuleTarget> {
    enabled_targets
        .iter()
        .filter(|enabled_target| *enabled_target != target)
        .cloned()
        .collect()
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
    build_rules_menu(
        rules,
        &[],
        is_running,
        Some("http://127.0.0.1:8800/_bifrost/rules"),
    )
}

#[cfg(test)]
pub fn build_rules_menu_for_test_with_recent(
    rules: &[TrayRule],
    recent_rule_targets: &[RuleTarget],
    is_running: bool,
) -> Option<SubmenuDef> {
    build_rules_menu(
        rules,
        recent_rule_targets,
        is_running,
        Some("http://127.0.0.1:8800/_bifrost/rules"),
    )
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
            managed_group: false,
        }
    }

    fn group_rule(group_name: &str, name: &str, enabled: bool) -> TrayRule {
        group_rule_with_managed(group_name, name, enabled, true)
    }

    fn group_rule_with_managed(
        group_name: &str,
        name: &str,
        enabled: bool,
        managed_group: bool,
    ) -> TrayRule {
        TrayRule {
            target: RuleTarget::Group {
                group_name: group_name.to_string(),
                name: name.to_string(),
            },
            enabled,
            sort_order: 0,
            managed_group,
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
            &[],
            None,
        );
        let status = find_item(&menu, "_status").unwrap();
        assert!(status.label.contains("Running on 127.0.0.1:8800"));
        let open_admin = find_item(&menu, "open_admin_ui").unwrap();
        assert!(open_admin.enabled);
        assert!(find_item(&menu, "restart_service").is_none());
        assert!(find_item(&menu, "open_data_dir").is_none());
    }

    #[test]
    fn test_system_proxy_toggle_is_below_stop_without_restart_or_data_dir() {
        let rt = sample_runtime();
        let system_proxy = SystemProxyMenuState {
            supported: true,
            enabled: true,
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
            &[],
            Some(&system_proxy),
        );

        let labels = menu
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.label.as_str()),
                MenuEntry::Submenu(_) => None,
            })
            .collect::<Vec<_>>();
        let stop_index = labels
            .iter()
            .position(|label| *label == "Stop Bifrost")
            .unwrap();
        assert_eq!(labels.get(stop_index + 1), Some(&"System Proxy"));
        assert!(!labels.contains(&"Restart Bifrost"));
        assert!(!labels.contains(&"Open Data Directory"));

        let toggle = find_item(&menu, "toggle_system_proxy").unwrap();
        assert!(toggle.enabled);
        assert!(toggle.checked);
        match &toggle.action {
            MenuItemAction::SetSystemProxy { url, enabled } => {
                assert_eq!(url, "http://127.0.0.1:8800/_bifrost/api/proxy/system");
                assert!(!enabled);
            }
            other => panic!("unexpected action: {other:?}"),
        }
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
        );
        let stop = find_item(&menu, "toggle_service").unwrap();
        assert!(!stop.enabled);
        assert!(find_item(&menu, "restart_service").is_none());
    }

    #[test]
    fn test_menu_unified_proxy_socks5_uses_main_port() {
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
            &[],
            None,
        );
        let socks5 = find_item(&menu, "copy_socks5_proxy").unwrap();
        assert!(socks5.enabled);
        match &socks5.action {
            MenuItemAction::CopyText(text) => {
                assert_eq!(text, "socks5://127.0.0.1:8800");
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn test_rules_menu_empty_running_state_keeps_visible_placeholder() {
        let menu = build_rules_menu_for_test(&[], true).unwrap();
        assert_eq!(menu.label, "Rules: None");
        assert!(menu.enabled);
        let empty = find_item(&menu.children, "rules_empty").unwrap();
        assert_eq!(empty.label, "No rules available");
        assert!(!empty.enabled);
    }

    #[test]
    fn test_rules_menu_empty_stopped_state_hidden() {
        assert!(build_rules_menu_for_test(&[], false).is_none());
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
            &[],
            None,
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
            &[],
            None,
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
        match &alpha.action {
            MenuItemAction::SelectRule {
                currently_enabled,
                enabled_targets,
                ..
            } => {
                assert!(*currently_enabled);
                assert!(enabled_targets.is_empty());
            }
            other => panic!("unexpected action: {other:?}"),
        }
        assert_eq!(beta.label, "beta");
        assert!(!beta.checked);
        match &beta.action {
            MenuItemAction::SelectRule {
                currently_enabled,
                enabled_targets,
                ..
            } => {
                assert!(!*currently_enabled);
                assert_eq!(
                    enabled_targets,
                    &vec![RuleTarget::Personal {
                        name: "alpha".to_string()
                    }]
                );
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn test_rules_menu_recent_rules_are_shown_first_with_group_labels() {
        let rules = vec![
            personal_rule("alpha", false),
            personal_rule("beta", true),
            group_rule("Team A", "shared", false),
            group_rule("Team B", "deploy", false),
            group_rule("Team C", "debug", false),
            group_rule("Team D", "ops", false),
            group_rule("Team E", "fallback", false),
            group_rule("Team F", "overflow", false),
        ];
        let recent = vec![
            RuleTarget::Group {
                group_name: "Team A".to_string(),
                name: "shared".to_string(),
            },
            RuleTarget::Personal {
                name: "beta".to_string(),
            },
            RuleTarget::Group {
                group_name: "Missing".to_string(),
                name: "deleted".to_string(),
            },
            RuleTarget::Group {
                group_name: "Team B".to_string(),
                name: "deploy".to_string(),
            },
            RuleTarget::Group {
                group_name: "Team C".to_string(),
                name: "debug".to_string(),
            },
            RuleTarget::Group {
                group_name: "Team D".to_string(),
                name: "ops".to_string(),
            },
            RuleTarget::Group {
                group_name: "Team E".to_string(),
                name: "fallback".to_string(),
            },
            RuleTarget::Group {
                group_name: "Team F".to_string(),
                name: "overflow".to_string(),
            },
        ];

        let menu = build_rules_menu_for_test_with_recent(&rules, &recent, true).unwrap();
        let labels = menu
            .children
            .iter()
            .take(6)
            .map(|entry| match entry {
                MenuEntry::Item(item) => item.label.as_str(),
                MenuEntry::Submenu(submenu) => submenu.label.as_str(),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Team A/shared",
                "beta",
                "Team B/deploy",
                "Team C/debug",
                "Team D/ops",
                "-"
            ]
        );
        let beta = find_item(&menu.children, "recent_rule_1").unwrap();
        assert!(beta.checked);
        match &beta.action {
            MenuItemAction::SelectRule {
                currently_enabled,
                enabled_targets,
                ..
            } => {
                assert!(*currently_enabled);
                assert!(enabled_targets.is_empty());
            }
            other => panic!("unexpected action: {other:?}"),
        }
        assert!(find_item(&menu.children, "recent_rule_5").is_none());
        assert!(find_submenu(&menu.children, "rules_my_rules").is_some());
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

    #[test]
    fn test_rules_menu_hides_unmanaged_groups_behind_more() {
        let rules = vec![
            personal_rule("personal", false),
            group_rule("Private Team", "private-shared", false),
            group_rule_with_managed("Discover Team", "discover-shared", true, false),
        ];

        let menu = build_rules_menu_for_test(&rules, true).unwrap();
        assert_eq!(menu.label, "Rules: Discover Team/discover-shared");
        assert!(find_submenu(&menu.children, "rules_my_rules").is_some());
        assert!(find_submenu(&menu.children, "rules_group_Private_Team").is_some());
        assert!(find_submenu(&menu.children, "rules_group_Discover_Team").is_none());

        let more = find_item(&menu.children, "rules_more").unwrap();
        assert_eq!(more.label, "More...");
        assert!(more.enabled);
        match &more.action {
            MenuItemAction::OpenUrl(url) => {
                assert_eq!(url, "http://127.0.0.1:8800/_bifrost/rules");
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}
