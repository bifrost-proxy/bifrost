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
    OpenAppRoute {
        route: String,
        fallback_url: String,
    },
    CopyText(String),
    AdminApi {
        method: String,
        url: String,
    },
    SetSystemProxy {
        url: String,
        enabled: bool,
    },
    SetTlsInterception {
        url: String,
        enabled: bool,
    },
    StartService,
    StopService,
    QuitDesktop {
        service_pid: u32,
        service_started_at_ms: Option<u64>,
    },
    StartUpgrade {
        target_version: String,
    },
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
    pub known: bool,
    pub supported: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemStatsMenuLines {
    pub system: String,
    pub network: String,
    pub menu_bar: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingMenuAction {
    SystemProxy { enabled: bool },
    TlsInterception { enabled: bool },
    Rule { target: RuleTarget, enabled: bool },
}

#[allow(clippy::too_many_arguments, dead_code)]
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
    update_available: Option<&str>,
    upgrade_in_progress: bool,
    system_stats: Option<&SystemStatsMenuLines>,
) -> Vec<MenuEntry> {
    build_menu_with_pending_and_tls(
        runtime,
        state,
        status_override,
        service_action_busy,
        custom_config,
        data_dir,
        bin_available,
        rules,
        recent_rule_targets,
        system_proxy,
        false,
        false,
        update_available,
        upgrade_in_progress,
        system_stats,
        None,
    )
}

#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_menu_with_pending(
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
    update_available: Option<&str>,
    upgrade_in_progress: bool,
    system_stats: Option<&SystemStatsMenuLines>,
    pending_action: Option<&PendingMenuAction>,
) -> Vec<MenuEntry> {
    build_menu_with_pending_and_tls(
        runtime,
        state,
        status_override,
        service_action_busy,
        custom_config,
        data_dir,
        bin_available,
        rules,
        recent_rule_targets,
        system_proxy,
        false,
        false,
        update_available,
        upgrade_in_progress,
        system_stats,
        pending_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_menu_with_pending_and_tls(
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
    tls_interception_known: bool,
    tls_interception_enabled: bool,
    update_available: Option<&str>,
    upgrade_in_progress: bool,
    system_stats: Option<&SystemStatsMenuLines>,
    pending_action: Option<&PendingMenuAction>,
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

    if let Some(stats) = system_stats {
        items.push(item(MenuItemDef {
            id: "_system_stats".to_string(),
            label: stats.system.clone(),
            enabled: false,
            checked: false,
            action: MenuItemAction::None,
        }));
        items.push(item(MenuItemDef {
            id: "_network_stats".to_string(),
            label: stats.network.clone(),
            enabled: false,
            checked: false,
            action: MenuItemAction::None,
        }));
    }

    if let Some(rt) = runtime {
        let admin_url = rt.admin_url();

        items.push(item(MenuItemDef {
            id: "open_traffic".to_string(),
            label: "Open Traffic".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::OpenAppRoute {
                route: "/traffic".to_string(),
                fallback_url: format!("{}traffic", admin_url),
            },
        }));

        items.push(item(MenuItemDef {
            id: "open_rules".to_string(),
            label: "Open Rules".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::OpenAppRoute {
                route: "/rules".to_string(),
                fallback_url: format!("{}rules", admin_url),
            },
        }));

        items.push(item(MenuItemDef {
            id: "open_settings".to_string(),
            label: "Open Settings".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::OpenAppRoute {
                route: "/settings".to_string(),
                fallback_url: format!("{}settings", admin_url),
            },
        }));

        items.push(item(MenuItemDef {
            id: "copy_http_proxy".to_string(),
            label: "Copy HTTP Proxy".to_string(),
            enabled: is_running,
            checked: false,
            action: MenuItemAction::CopyText(rt.http_proxy_url()),
        }));
    }

    let admin_rules_url = runtime.map(|rt| format!("{}rules", rt.admin_url()));
    if let Some(rules_menu) = build_rules_menu(
        rules,
        recent_rule_targets,
        is_running,
        admin_rules_url.as_deref(),
        pending_action,
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

    if let Some(latest) = update_available {
        let label = if upgrade_in_progress {
            "Updating…".to_string()
        } else {
            format!("Update to v{latest}")
        };
        items.push(item(MenuItemDef {
            id: "update_now".to_string(),
            label,
            enabled: bin_available && !upgrade_in_progress,
            checked: false,
            action: MenuItemAction::StartUpgrade {
                target_version: latest.to_string(),
            },
        }));
    }

    let desktop_runtime = runtime.filter(|runtime| is_running && runtime.is_desktop_owned());
    if let Some(desktop_runtime) = desktop_runtime {
        let label = if service_action_busy && status_label == "Bifrost: Quitting..." {
            "Quitting Bifrost..."
        } else {
            "Quit Bifrost"
        };
        items.push(item(MenuItemDef {
            id: "toggle_service".to_string(),
            label: label.to_string(),
            enabled: !service_action_busy,
            checked: false,
            action: MenuItemAction::QuitDesktop {
                service_pid: desktop_runtime.pid,
                service_started_at_ms: desktop_runtime.started_at_ms,
            },
        }));
    } else if is_running {
        let label = if service_action_busy && status_label == "Bifrost: Stopping..." {
            "Stopping Bifrost..."
        } else {
            "Stop Bifrost"
        };
        items.push(item(MenuItemDef {
            id: "toggle_service".to_string(),
            label: label.to_string(),
            enabled: bin_available && !service_action_busy,
            checked: false,
            action: MenuItemAction::StopService,
        }));
    } else {
        let label = if service_action_busy && status_label == "Bifrost: Starting..." {
            "Starting Bifrost..."
        } else {
            "Start Bifrost"
        };
        items.push(item(MenuItemDef {
            id: "toggle_service".to_string(),
            label: label.to_string(),
            enabled: bin_available && !service_action_busy,
            checked: false,
            action: MenuItemAction::StartService,
        }));
    }

    if let Some(rt) = runtime {
        let admin_url = rt.admin_url();
        let fallback_system_proxy;
        let system_proxy = if is_running {
            if let Some(system_proxy) = system_proxy {
                Some(system_proxy)
            } else {
                fallback_system_proxy = SystemProxyMenuState {
                    known: false,
                    supported: false,
                    enabled: false,
                };
                Some(&fallback_system_proxy)
            }
        } else {
            None
        };
        if let Some(system_proxy) = system_proxy {
            let pending_system_proxy = pending_action.and_then(|pending| match pending {
                PendingMenuAction::SystemProxy { enabled } => Some(*enabled),
                PendingMenuAction::Rule { .. } | PendingMenuAction::TlsInterception { .. } => None,
            });
            let label = match pending_system_proxy {
                Some(true) => "Enabling System Proxy...",
                Some(false) => "Disabling System Proxy...",
                None => "System Proxy",
            };
            items.push(item(MenuItemDef {
                id: "toggle_system_proxy".to_string(),
                label: label.to_string(),
                enabled: is_running
                    && system_proxy.known
                    && system_proxy.supported
                    && !service_action_busy
                    && pending_action.is_none(),
                checked: system_proxy.enabled,
                action: MenuItemAction::SetSystemProxy {
                    url: format!("{}api/proxy/system", admin_url),
                    enabled: !system_proxy.enabled,
                },
            }));
        }

        let has_pending_tls = matches!(
            pending_action,
            Some(PendingMenuAction::TlsInterception { .. })
        );
        if is_running && (tls_interception_known || has_pending_tls) {
            let pending_tls = pending_action.and_then(|pending| match pending {
                PendingMenuAction::TlsInterception { enabled } => Some(*enabled),
                PendingMenuAction::Rule { .. } | PendingMenuAction::SystemProxy { .. } => None,
            });
            let label = match pending_tls {
                Some(true) => "Enabling TLS Interception...",
                Some(false) => "Disabling TLS Interception...",
                None if !tls_interception_known => "TLS Interception: Checking...",
                None if tls_interception_enabled => "TLS Interception: On",
                None => "TLS Interception: Off",
            };
            items.push(item(MenuItemDef {
                id: "toggle_tls_interception".to_string(),
                label: label.to_string(),
                enabled: tls_interception_known && !service_action_busy && pending_action.is_none(),
                checked: tls_interception_enabled,
                action: MenuItemAction::SetTlsInterception {
                    url: format!("{}api/config/tls", admin_url),
                    enabled: !tls_interception_enabled,
                },
            }));
        }
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

    items.push(separator("_sep_info"));

    items.push(item(MenuItemDef {
        id: "_status".to_string(),
        label: status_label,
        enabled: false,
        checked: false,
        action: MenuItemAction::None,
    }));

    items.push(item(MenuItemDef {
        id: "_version".to_string(),
        label: format!("Version v{}", env!("CARGO_PKG_VERSION")),
        enabled: false,
        checked: false,
        action: MenuItemAction::None,
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
    pending_action: Option<&PendingMenuAction>,
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
    let pending_rule = pending_action.and_then(|pending| match pending {
        PendingMenuAction::Rule { target, enabled } => Some((target, *enabled)),
        PendingMenuAction::SystemProxy { .. } | PendingMenuAction::TlsInterception { .. } => None,
    });
    let rule_items_enabled = is_running && pending_action.is_none();
    let label = if let Some((target, enabled)) = pending_rule {
        if enabled {
            format!("Rules: Applying {}", rule_label(target))
        } else {
            format!("Rules: Disabling {}", rule_label(target))
        }
    } else {
        match active_label {
            Some(name) => format!("Rules: {name}"),
            None => "Rules: None".to_string(),
        }
    };

    let mut children = build_recent_rule_entries(
        rules,
        recent_rule_targets,
        &enabled_targets,
        rule_items_enabled,
        pending_rule,
    );
    let grouped_children = if has_group_rules {
        build_grouped_rule_entries(
            rules,
            &enabled_targets,
            rule_items_enabled,
            admin_rules_url,
            pending_rule,
        )
    } else {
        rules
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                rule_entry(
                    rule,
                    index,
                    &enabled_targets,
                    rule_items_enabled,
                    pending_rule,
                )
            })
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
    pending_rule: Option<(&RuleTarget, bool)>,
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
        let (label, enabled) = rule_action_label_and_enabled(rule, is_running, pending_rule, true);
        children.push(item(MenuItemDef {
            id: format!("recent_rule_{}", children.len()),
            label,
            enabled,
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
    pending_rule: Option<(&RuleTarget, bool)>,
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
            .map(|(index, rule)| rule_entry(rule, index, enabled_targets, is_running, pending_rule))
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
            .map(|(index, rule)| rule_entry(rule, index, enabled_targets, is_running, pending_rule))
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
    pending_rule: Option<(&RuleTarget, bool)>,
) -> MenuEntry {
    let (label, enabled) = rule_action_label_and_enabled(rule, is_running, pending_rule, false);
    item(MenuItemDef {
        id: format!("select_rule_{index}"),
        label,
        enabled,
        checked: rule.enabled,
        action: MenuItemAction::SelectRule {
            target: rule.target.clone(),
            enabled_targets: other_enabled_targets(enabled_targets, &rule.target),
            currently_enabled: rule.enabled,
        },
    })
}

fn rule_action_label_and_enabled(
    rule: &TrayRule,
    is_running: bool,
    pending_rule: Option<(&RuleTarget, bool)>,
    full_label: bool,
) -> (String, bool) {
    let base_label = if full_label {
        rule_label(&rule.target)
    } else {
        short_rule_label(&rule.target)
    };
    let Some((target, enabled)) = pending_rule else {
        return (base_label, is_running);
    };
    if target == &rule.target {
        let prefix = if enabled { "Applying" } else { "Disabling" };
        (format!("{prefix} {base_label}..."), false)
    } else {
        (base_label, false)
    }
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
        None,
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
        None,
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
            start_mode: super::super::runtime::RuntimeStartMode::Unknown,
            binary_path: None,
        }
    }

    fn sample_desktop_runtime() -> RuntimeInfo {
        RuntimeInfo {
            start_mode: super::super::runtime::RuntimeStartMode::Desktop,
            ..sample_runtime()
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
            None,
            false,
            None,
        );
        let status = find_item(&menu, "_status").unwrap();
        assert!(status.label.contains("Running on 127.0.0.1:8800"));
        let version = find_item(&menu, "_version").unwrap();
        assert_eq!(
            version.label,
            format!("Version v{}", env!("CARGO_PKG_VERSION"))
        );
        assert!(!version.enabled);
        assert!(find_item(&menu, "open_admin_ui").is_none());
        let open_traffic = find_item(&menu, "open_traffic").unwrap();
        assert!(open_traffic.enabled);
        let open_settings = find_item(&menu, "open_settings").unwrap();
        assert_eq!(open_settings.label, "Open Settings");
        assert!(open_settings.enabled);
        match &open_settings.action {
            MenuItemAction::OpenAppRoute {
                route,
                fallback_url,
            } => {
                assert_eq!(route, "/settings");
                assert_eq!(fallback_url, "http://127.0.0.1:8800/_bifrost/settings");
            }
            other => panic!("unexpected action: {other:?}"),
        }
        assert!(find_item(&menu, "restart_service").is_none());
        assert!(find_item(&menu, "open_data_dir").is_none());
        let stop = find_item(&menu, "toggle_service").unwrap();
        assert_eq!(stop.label, "Stop Bifrost");
        assert!(matches!(stop.action, MenuItemAction::StopService));
    }

    #[test]
    fn test_desktop_owned_runtime_shows_quit_instead_of_stop() {
        let rt = sample_desktop_runtime();
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
            None,
            false,
            None,
        );

        let quit = find_item(&menu, "toggle_service").unwrap();
        assert_eq!(quit.label, "Quit Bifrost");
        assert!(quit.enabled);
        assert!(matches!(
            quit.action,
            MenuItemAction::QuitDesktop {
                service_pid: 1234,
                service_started_at_ms: None
            }
        ));
    }

    #[test]
    fn test_non_desktop_runtime_modes_keep_stop_action() {
        for start_mode in [
            super::super::runtime::RuntimeStartMode::Foreground,
            super::super::runtime::RuntimeStartMode::Daemon,
            super::super::runtime::RuntimeStartMode::Unknown,
        ] {
            let rt = RuntimeInfo {
                start_mode,
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
                None,
                false,
                None,
            );

            let stop = find_item(&menu, "toggle_service").unwrap();
            assert_eq!(stop.label, "Stop Bifrost");
            assert!(matches!(stop.action, MenuItemAction::StopService));
        }
    }

    #[test]
    fn test_desktop_quit_in_progress_is_visible_and_disabled() {
        let rt = sample_desktop_runtime();
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            Some("Bifrost: Quitting..."),
            true,
            None,
            "/tmp/.bifrost",
            true,
            &[],
            &[],
            None,
            None,
            false,
            None,
        );

        let status = find_item(&menu, "_status").unwrap();
        assert_eq!(status.label, "Bifrost: Quitting...");
        let quit = find_item(&menu, "toggle_service").unwrap();
        assert_eq!(quit.label, "Quitting Bifrost...");
        assert!(!quit.enabled);
    }

    #[test]
    fn test_menu_system_stats_uses_two_disabled_rows_when_enabled() {
        let rt = sample_runtime();
        let stats = SystemStatsMenuLines {
            system: "System: CPU 23% | Memory 18.0 GB / 32.0 GB | Disk 59%".to_string(),
            network: "Network: Up 1.5 MB/s | Down 512 KB/s".to_string(),
            menu_bar: "C23% | M56% | D59% | ↑1.5 M/s ↓512 K/s".to_string(),
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
            None,
            false,
            Some(&stats),
        );

        let system = find_item(&menu, "_system_stats").unwrap();
        assert_eq!(system.label, stats.system);
        assert!(!system.enabled);
        let network = find_item(&menu, "_network_stats").unwrap();
        assert_eq!(network.label, stats.network);
        assert!(!network.enabled);
    }

    #[test]
    fn test_menu_system_stats_hidden_when_disabled() {
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
            None,
            false,
            None,
        );

        assert!(find_item(&menu, "_system_stats").is_none());
        assert!(find_item(&menu, "_network_stats").is_none());
    }

    #[test]
    fn test_system_proxy_toggle_is_below_stop_without_restart_or_data_dir() {
        let rt = sample_runtime();
        let system_proxy = SystemProxyMenuState {
            known: true,
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
            None,
            false,
            None,
        );

        let labels = menu
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(item) => item.label.as_str(),
                MenuEntry::Submenu(submenu) => submenu.label.as_str(),
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
        assert_eq!(toggle.label, "System Proxy");
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
    fn test_system_proxy_toggle_is_disabled_while_running_without_cached_state() {
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
            None,
            false,
            None,
        );

        let toggle = find_item(&menu, "toggle_system_proxy").unwrap();
        assert_eq!(toggle.label, "System Proxy");
        assert!(!toggle.enabled);
        assert!(!toggle.checked);
        match &toggle.action {
            MenuItemAction::SetSystemProxy { url, enabled } => {
                assert_eq!(url, "http://127.0.0.1:8800/_bifrost/api/proxy/system");
                assert!(*enabled);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn test_system_proxy_pending_shows_busy_label_and_disables_async_toggles() {
        let rt = sample_runtime();
        let system_proxy = SystemProxyMenuState {
            known: true,
            supported: true,
            enabled: false,
        };
        let rules = vec![personal_rule("alpha", false)];
        let menu = build_menu_with_pending(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &rules,
            &[],
            Some(&system_proxy),
            None,
            false,
            None,
            Some(&PendingMenuAction::SystemProxy { enabled: true }),
        );

        let toggle = find_item(&menu, "toggle_system_proxy").unwrap();
        assert_eq!(toggle.label, "Enabling System Proxy...");
        assert!(!toggle.enabled);
        assert!(!toggle.checked);

        let rules_menu = find_submenu(&menu, "rules_switcher").unwrap();
        assert_eq!(rules_menu.label, "Rules: None");
        let alpha = find_item(&rules_menu.children, "select_rule_0").unwrap();
        assert_eq!(alpha.label, "alpha");
        assert!(!alpha.enabled);
    }

    #[test]
    fn test_menu_update_available_shows_update_item() {
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
            Some("0.0.104"),
            false,
            None,
        );
        let update = find_item(&menu, "update_now").expect("update_now present");
        assert_eq!(update.label, "Update to v0.0.104");
        assert!(update.enabled);
        match &update.action {
            MenuItemAction::StartUpgrade { target_version } => {
                assert_eq!(target_version, "0.0.104");
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn test_menu_no_update_hides_update_item() {
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
            None,
            false,
            None,
        );
        assert!(find_item(&menu, "update_now").is_none());
    }

    #[test]
    fn test_menu_update_in_progress_disables_item() {
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
            Some("0.0.104"),
            true,
            None,
        );
        let update = find_item(&menu, "update_now").expect("update_now present");
        assert_eq!(update.label, "Updating…");
        assert!(!update.enabled);
    }

    #[test]
    fn test_menu_stopped_state() {
        let rt = sample_runtime();
        let system_proxy = SystemProxyMenuState {
            known: true,
            supported: true,
            enabled: true,
        };
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
            Some(&system_proxy),
            None,
            false,
            None,
        );
        let status = find_item(&menu, "_status").unwrap();
        assert!(status.label.contains("Stopped"));
        assert!(find_item(&menu, "open_admin_ui").is_none());
        let open_traffic = find_item(&menu, "open_traffic").unwrap();
        assert!(!open_traffic.enabled);
        assert!(find_item(&menu, "toggle_system_proxy").is_none());
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
            None,
            false,
            None,
        );
        let status = find_item(&menu, "_status").unwrap();
        assert_eq!(status.label, "Bifrost: Starting...");
        let start = find_item(&menu, "toggle_service").unwrap();
        assert_eq!(start.label, "Starting Bifrost...");
        assert!(!start.enabled);
    }

    #[test]
    fn test_service_action_busy_shows_stopping_label() {
        let rt = sample_runtime();
        let menu = build_menu(
            Some(&rt),
            ServiceState::Running,
            Some("Bifrost: Stopping..."),
            true,
            None,
            "/tmp/.bifrost",
            true,
            &[],
            &[],
            None,
            None,
            false,
            None,
        );
        let status = find_item(&menu, "_status").unwrap();
        assert_eq!(status.label, "Bifrost: Stopping...");
        let stop = find_item(&menu, "toggle_service").unwrap();
        assert_eq!(stop.label, "Stopping Bifrost...");
        assert!(!stop.enabled);
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
            None,
            false,
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
            None,
            false,
            None,
        );
        let stop = find_item(&menu, "toggle_service").unwrap();
        assert!(!stop.enabled);
        assert!(find_item(&menu, "restart_service").is_none());
    }

    #[test]
    fn test_menu_omits_low_frequency_copy_socks5_proxy_item() {
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
            None,
            false,
            None,
        );
        assert!(find_item(&menu, "copy_socks5_proxy").is_none());
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
            None,
            false,
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
            None,
            false,
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
    fn test_rules_pending_shows_busy_label_and_disables_async_toggles() {
        let rt = sample_runtime();
        let system_proxy = SystemProxyMenuState {
            known: true,
            supported: true,
            enabled: true,
        };
        let target = RuleTarget::Personal {
            name: "beta".to_string(),
        };
        let rules = vec![personal_rule("alpha", true), personal_rule("beta", false)];
        let menu = build_menu_with_pending(
            Some(&rt),
            ServiceState::Running,
            None,
            false,
            None,
            "/tmp/.bifrost",
            true,
            &rules,
            &[],
            Some(&system_proxy),
            None,
            false,
            None,
            Some(&PendingMenuAction::Rule {
                target: target.clone(),
                enabled: true,
            }),
        );

        let rules_menu = find_submenu(&menu, "rules_switcher").unwrap();
        assert_eq!(rules_menu.label, "Rules: Applying beta");
        let alpha = find_item(&rules_menu.children, "select_rule_0").unwrap();
        let beta = find_item(&rules_menu.children, "select_rule_1").unwrap();
        assert_eq!(alpha.label, "alpha");
        assert!(!alpha.enabled);
        assert_eq!(beta.label, "Applying beta...");
        assert!(!beta.enabled);

        let toggle = find_item(&menu, "toggle_system_proxy").unwrap();
        assert_eq!(toggle.label, "System Proxy");
        assert!(!toggle.enabled);
        assert!(toggle.checked);
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
