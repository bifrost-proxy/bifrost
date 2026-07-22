use serde::Serialize;

pub const FEISHU_BOT_MENU_EVENT_TYPE: &str = "application.bot.menu_v6";
pub const MAX_FEISHU_BOT_MENU_EVENT_KEY_CHARS: usize = 30;
const MAX_TOP_LEVEL_MENU_ITEMS: usize = 3;
const MAX_CHILD_MENU_ITEMS: usize = 5;
const MAX_TOTAL_MENU_ITEMS: usize = 10;
const FIXED_COMMON_CHILD_ITEMS: usize = 2;
const MAX_DYNAMIC_CHILD_ITEMS: usize =
    MAX_TOTAL_MENU_ITEMS - MAX_TOP_LEVEL_MENU_ITEMS - FIXED_COMMON_CHILD_ITEMS;
const MAX_RUNNER_CHILD_ITEMS_WHEN_MODELS_EXIST: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeishuBotMenuItem {
    pub menu_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_menu_id: Option<String>,
    pub sort: usize,
    pub default_name: String,
    pub menu_content_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeishuBotAbilityUpdate {
    pub enable: bool,
    pub bot_menu_enable: bool,
    pub bot_menus: Vec<FeishuBotMenuItem>,
    pub bot_menu_display_strategy: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateFeishuApplicationAbilityRequest {
    pub bot: FeishuBotAbilityUpdate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeishuBotMenuModelOption {
    pub slug: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeishuBotMenuCommand {
    Status,
    Help,
    ListRunners,
    SwitchRunner(String),
    ListModels,
    SwitchModel(String),
}

impl FeishuBotMenuCommand {
    pub fn slash_command(&self) -> String {
        match self {
            Self::Status => "/status".to_string(),
            Self::Help => "/help".to_string(),
            Self::ListRunners => "/runner".to_string(),
            Self::SwitchRunner(runner_id) => format!("/runner {runner_id}"),
            Self::ListModels => "/models".to_string(),
            Self::SwitchModel(model) => format!("/model {model}"),
        }
    }
}

pub fn parse_feishu_bot_menu_event_key(value: &str) -> Option<FeishuBotMenuCommand> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_FEISHU_BOT_MENU_EVENT_KEY_CHARS {
        return None;
    }
    match value {
        "bf_status" => return Some(FeishuBotMenuCommand::Status),
        "bf_help" => return Some(FeishuBotMenuCommand::Help),
        "bf_runner" => return Some(FeishuBotMenuCommand::ListRunners),
        "bf_models" => return Some(FeishuBotMenuCommand::ListModels),
        _ => {}
    }

    let (prefix, argument) = value.split_once(':')?;
    if !valid_dynamic_argument(argument) {
        return None;
    }
    match prefix {
        "bf_runner" => Some(FeishuBotMenuCommand::SwitchRunner(argument.to_string())),
        "bf_model" => Some(FeishuBotMenuCommand::SwitchModel(argument.to_string())),
        _ => None,
    }
}

pub fn build_feishu_bot_menu(
    runner_ids: impl IntoIterator<Item = String>,
    models: impl IntoIterator<Item = FeishuBotMenuModelOption>,
) -> UpdateFeishuApplicationAbilityRequest {
    let mut runner_children = runner_ids
        .into_iter()
        .filter_map(|runner_id| {
            let event_key = format!("bf_runner:{runner_id}");
            valid_menu_event_key(&event_key).then(|| (menu_label(&runner_id), event_key))
        })
        .take(MAX_CHILD_MENU_ITEMS)
        .collect::<Vec<_>>();
    let mut model_children = models
        .into_iter()
        .filter_map(|model| {
            let event_key = format!("bf_model:{}", model.slug);
            valid_menu_event_key(&event_key).then(|| {
                let label = model
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(&model.slug);
                (menu_label(label), event_key)
            })
        })
        .take(MAX_CHILD_MENU_ITEMS)
        .collect::<Vec<_>>();
    if !runner_children.is_empty() && !model_children.is_empty() {
        runner_children.truncate(MAX_RUNNER_CHILD_ITEMS_WHEN_MODELS_EXIST);
    }
    model_children.truncate(MAX_DYNAMIC_CHILD_ITEMS.saturating_sub(runner_children.len()));

    let mut menu_items = vec![parent("bf_common", 1, "常用")];
    menu_items.push(event_leaf("bf_status", "bf_common", 1, "状态", "bf_status"));
    menu_items.push(event_leaf("bf_help", "bf_common", 2, "帮助", "bf_help"));

    if runner_children.is_empty() {
        menu_items.push(top_level_event("bf_agents", 2, "切 Agent", "bf_runner"));
    } else {
        menu_items.push(parent("bf_agents", 2, "切 Agent"));
        menu_items.extend(runner_children.into_iter().enumerate().map(
            |(index, (name, event_key))| {
                event_leaf(
                    &format!("bf_agent_{}", index + 1),
                    "bf_agents",
                    index + 1,
                    name,
                    event_key,
                )
            },
        ));
    }

    if model_children.is_empty() {
        menu_items.push(top_level_event("bf_models", 3, "切换模型", "bf_models"));
    } else {
        menu_items.push(parent("bf_models", 3, "切换模型"));
        menu_items.extend(model_children.into_iter().enumerate().map(
            |(index, (name, event_key))| {
                event_leaf(
                    &format!("bf_model_{}", index + 1),
                    "bf_models",
                    index + 1,
                    name,
                    event_key,
                )
            },
        ));
    }

    debug_assert_eq!(
        menu_items
            .iter()
            .filter(|item| item.parent_menu_id.is_none())
            .count(),
        MAX_TOP_LEVEL_MENU_ITEMS
    );
    debug_assert!(menu_items.len() <= MAX_TOTAL_MENU_ITEMS);
    UpdateFeishuApplicationAbilityRequest {
        bot: FeishuBotAbilityUpdate {
            enable: true,
            bot_menu_enable: true,
            bot_menus: menu_items,
            bot_menu_display_strategy: 1,
        },
    }
}

fn parent(menu_id: &str, sort: usize, name: impl Into<String>) -> FeishuBotMenuItem {
    FeishuBotMenuItem {
        menu_id: menu_id.to_string(),
        parent_menu_id: None,
        sort,
        default_name: name.into(),
        menu_content_type: 3,
        event_key: None,
    }
}

fn event_leaf(
    menu_id: &str,
    parent_menu_id: &str,
    sort: usize,
    name: impl Into<String>,
    event_key: impl Into<String>,
) -> FeishuBotMenuItem {
    FeishuBotMenuItem {
        menu_id: menu_id.to_string(),
        parent_menu_id: Some(parent_menu_id.to_string()),
        sort,
        default_name: name.into(),
        menu_content_type: 2,
        event_key: Some(event_key.into()),
    }
}

fn top_level_event(
    menu_id: &str,
    sort: usize,
    name: impl Into<String>,
    event_key: impl Into<String>,
) -> FeishuBotMenuItem {
    let mut item = event_leaf(menu_id, "", sort, name, event_key);
    item.parent_menu_id = None;
    item
}

fn valid_menu_event_key(value: &str) -> bool {
    value.chars().count() <= MAX_FEISHU_BOT_MENU_EVENT_KEY_CHARS
        && value
            .split_once(':')
            .is_some_and(|(_, argument)| valid_dynamic_argument(argument))
}

fn valid_dynamic_argument(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn menu_label(value: &str) -> String {
    const MAX_LABEL_CHARS: usize = 12;
    value.trim().chars().take(MAX_LABEL_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_keys_map_only_allowlisted_menu_actions_to_slash_commands() {
        assert_eq!(
            parse_feishu_bot_menu_event_key("bf_status").map(|command| command.slash_command()),
            Some("/status".to_string())
        );
        assert_eq!(
            parse_feishu_bot_menu_event_key("bf_runner:Codex")
                .map(|command| command.slash_command()),
            Some("/runner Codex".to_string())
        );
        assert_eq!(
            parse_feishu_bot_menu_event_key("bf_runner").map(|command| command.slash_command()),
            Some("/runner".to_string())
        );
        assert_eq!(
            parse_feishu_bot_menu_event_key("bf_models").map(|command| command.slash_command()),
            Some("/models".to_string())
        );
        assert_eq!(
            parse_feishu_bot_menu_event_key("bf_model:gpt-5.3-codex")
                .map(|command| command.slash_command()),
            Some("/model gpt-5.3-codex".to_string())
        );
        assert!(parse_feishu_bot_menu_event_key("/status").is_none());
        assert!(parse_feishu_bot_menu_event_key("bf_runner:bad runner").is_none());
        assert!(parse_feishu_bot_menu_event_key("bf_model:").is_none());
        assert!(parse_feishu_bot_menu_event_key("bf_unknown:value").is_none());
        assert!(parse_feishu_bot_menu_event_key(&format!("bf_model:{}", "x".repeat(30))).is_none());
    }

    #[test]
    fn menu_payload_has_three_roots_and_caps_dynamic_children() {
        let payload = build_feishu_bot_menu(
            (1..=8).map(|index| format!("Runner-{index}")),
            (1..=8).map(|index| FeishuBotMenuModelOption {
                slug: format!("model-{index}"),
                display_name: None,
            }),
        );
        let value = serde_json::to_value(payload).unwrap();

        let items = value["bot"]["bot_menus"].as_array().unwrap();
        assert_eq!(items.len(), 10);
        assert_eq!(
            items
                .iter()
                .filter(|item| item.get("parent_menu_id").is_none())
                .count(),
            3
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| item["parent_menu_id"] == "bf_agents")
                .count(),
            3
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| item["parent_menu_id"] == "bf_models")
                .count(),
            2
        );
        assert_eq!(items[1]["event_key"], "bf_status");
        assert_eq!(items[4]["event_key"], "bf_runner:Runner-1");
        assert_eq!(value["bot"]["bot_menu_display_strategy"], 1);
    }

    #[test]
    fn empty_dynamic_sections_fall_back_to_list_commands() {
        let payload = build_feishu_bot_menu(Vec::new(), Vec::new());
        let value = serde_json::to_value(payload).unwrap();
        let items = value["bot"]["bot_menus"].as_array().unwrap();

        assert_eq!(items[3]["event_key"], "bf_runner");
        assert_eq!(items[3]["menu_content_type"], 2);
        assert_eq!(items[4]["event_key"], "bf_models");
    }
}
