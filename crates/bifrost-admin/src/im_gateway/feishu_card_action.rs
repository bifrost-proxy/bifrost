use std::time::Duration;

use ring::digest::{digest, SHA256};
use serde::Deserialize;
use tracing::{debug, error, warn};

use crate::im_gateway::external_cli::{
    parse_external_cli_effort_slash_command, parse_external_cli_model_slash_command,
    parse_external_cli_resume_slash_command, ExternalCliEffortSlashCommand,
    ExternalCliModelSlashCommand, ExternalCliResumeSlashCommand,
};
use crate::im_gateway::provider::EventSink;
use crate::im_gateway::types::{ImEvent, ImEventMessage, ImEventSource, ImProviderType};

const SLASH_CHOICE_ACTION: &str = "slash_choice";
const MAX_BUTTON_LABEL_CHARS: usize = 100;
pub(crate) const FEISHU_CHOICE_CARD_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeishuChoiceCardBinding {
    pub provider_id: String,
    pub chat_id: String,
    pub chat_type: String,
    pub user_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeishuChoiceCardOption {
    pub label: String,
    pub command: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlashChoiceValue {
    bifrost_action: String,
    provider_id: String,
    chat_id: String,
    chat_type: String,
    user_id: String,
    command: String,
    expires_at_ms: u64,
}

pub(crate) fn build_feishu_choice_card(
    markdown: &str,
    binding: &FeishuChoiceCardBinding,
    options: &[FeishuChoiceCardOption],
    now_ms: u64,
) -> serde_json::Value {
    let expires_at_ms = now_ms.saturating_add(FEISHU_CHOICE_CARD_TTL.as_millis() as u64);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": crate::im_gateway::markdown_converter::convert_to_feishu_markdown(markdown),
        "element_id": "choice_summary"
    })];

    // Each option carries the full slash-choice binding serialized into its
    // string `value`. Feishu `select_static` option values must be unique
    // strings, and the distinct per-option `command` guarantees uniqueness.
    let select_options = options
        .iter()
        .map(|option| {
            let label = normalize_button_label(&option.label);
            let value = serde_json::json!({
                "bifrostAction": SLASH_CHOICE_ACTION,
                "providerId": binding.provider_id,
                "chatId": binding.chat_id,
                "chatType": binding.chat_type,
                "userId": binding.user_id,
                "command": option.command,
                "expiresAtMs": expires_at_ms
            })
            .to_string();
            serde_json::json!({
                "text": {
                    "tag": "plain_text",
                    "content": label
                },
                "value": value
            })
        })
        .collect::<Vec<_>>();

    elements.push(serde_json::json!({
        "tag": "select_static",
        "element_id": "choice_select",
        "width": "fill",
        "placeholder": {
            "tag": "plain_text",
            "content": "请选择…"
        },
        "behaviors": [{
            "type": "callback",
            "value": {
                "bifrostAction": SLASH_CHOICE_ACTION
            }
        }],
        "options": select_options
    }));

    serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "body": {
            "elements": elements
        }
    })
}

pub(super) fn handle_ws_message(text: &str, provider_id: &str, sink: &EventSink) {
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            warn!(provider_id, error = %error, "failed to parse feishu ws message");
            return;
        }
    };

    if parsed
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|message_type| matches!(message_type, "pong" | "heartbeat"))
    {
        debug!(provider_id, "received feishu protocol heartbeat");
        return;
    }

    let normalized =
        match normalize_feishu_card_action(&parsed, provider_id, super::current_timestamp_ms()) {
            Ok(Some(event)) => Some(event),
            Ok(None) => super::normalize_feishu_event(&parsed, provider_id),
            Err(reason) => {
                warn!(provider_id, reason, "rejecting invalid feishu card action");
                None
            }
        };
    if let Some(event) = normalized {
        let lifecycle =
            event.event_type == crate::im_gateway::feishu_group_permission::BOT_JOINED_EVENT_TYPE;
        let result = if lifecycle {
            sink.persist_and_send(event)
                .map_err(|error| error.to_string())
        } else {
            sink.send(event).map_err(|error| error.to_string())
        };
        if let Err(error) = result {
            error!(
                provider_id,
                error = %error,
                "failed to send event to sink, receiver may be dropped"
            );
        }
    }
}

pub(crate) fn normalize_feishu_card_action(
    raw: &serde_json::Value,
    provider_id: &str,
    now_ms: u64,
) -> Result<Option<ImEvent>, String> {
    let Some(event_type) = raw
        .get("header")
        .and_then(|header| header.get("event_type"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    if event_type != "card.action.trigger" {
        return Ok(None);
    }

    let header = raw
        .get("header")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "card action is missing header".to_string())?;
    let event_id = non_empty_string(header.get("event_id"))
        .ok_or_else(|| "card action is missing header.event_id".to_string())?;
    let event = raw
        .get("event")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "card action is missing event".to_string())?;
    let operator_id = event
        .get("operator")
        .and_then(|operator| operator.get("open_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "card action is missing event.operator.open_id".to_string())?;
    let action = event
        .get("action")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "card action is missing event.action".to_string())?;
    // Choice cards render a single `select_static` dropdown; each option's
    // string `value` carries the full serialized binding. Legacy `button`
    // cards (already delivered before this change) stay clickable by reading
    // the binding object from `action.value`.
    let (value, raw_payload): (SlashChoiceValue, serde_json::Value) = match action
        .get("tag")
        .and_then(serde_json::Value::as_str)
    {
        Some("select_static") => {
            let option = action
                .get("option")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "card action is missing event.action.option".to_string())?;
            let parsed: SlashChoiceValue = serde_json::from_str(option)
                .map_err(|error| format!("invalid card action option: {error}"))?;
            (parsed, serde_json::Value::String(option.to_string()))
        }
        Some("button") => {
            let raw_value = action
                .get("value")
                .cloned()
                .ok_or_else(|| "card action is missing event.action.value".to_string())?;
            let parsed: SlashChoiceValue = serde_json::from_value(raw_value.clone())
                .map_err(|error| format!("invalid card action value: {error}"))?;
            (parsed, raw_value)
        }
        _ => {
            return Err("card action does not originate from a select_static or button".to_string())
        }
    };
    let context = event
        .get("context")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "card action is missing event.context".to_string())?;
    let open_chat_id = non_empty_string(context.get("open_chat_id"))
        .ok_or_else(|| "card action is missing event.context.open_chat_id".to_string())?;
    non_empty_string(context.get("open_message_id"))
        .ok_or_else(|| "card action is missing event.context.open_message_id".to_string())?;

    validate_slash_choice_value(&value, provider_id, operator_id, &open_chat_id, now_ms)?;
    let command = value.command.trim().to_string();
    let raw_digest = raw_digest(raw);

    Ok(Some(ImEvent {
        event_id,
        provider_id: provider_id.to_string(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: ImEventSource {
            chat_id: Some(open_chat_id),
            chat_type: Some(value.chat_type),
            user_id: Some(operator_id.to_string()),
            user_name: None,
            sender_type: Some("user".to_string()),
            message_id: None,
        },
        message: Some(ImEventMessage {
            text: command,
            raw_type: Some("interactive_callback".to_string()),
            raw_content: Some(raw_payload),
            ..ImEventMessage::default()
        }),
        received_at: now_ms,
        raw_digest: Some(raw_digest),
    }))
}

fn validate_slash_choice_value(
    value: &SlashChoiceValue,
    provider_id: &str,
    operator_id: &str,
    open_chat_id: &str,
    now_ms: u64,
) -> Result<(), String> {
    if value.bifrost_action != SLASH_CHOICE_ACTION {
        return Err("unsupported card action".to_string());
    }
    if value.provider_id != provider_id {
        return Err("card action provider binding mismatch".to_string());
    }
    if value.user_id != operator_id {
        return Err("card action user binding mismatch".to_string());
    }
    if value.chat_id != open_chat_id {
        return Err("card action chat binding mismatch".to_string());
    }
    if !matches!(value.chat_type.as_str(), "p2p" | "group") {
        return Err("card action has unsupported chat type".to_string());
    }
    if value.expires_at_ms <= now_ms {
        return Err("card action has expired".to_string());
    }
    if !is_allowed_choice_command(&value.command) {
        return Err("card action command is not allowed".to_string());
    }
    Ok(())
}

pub(crate) fn is_allowed_choice_command(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    let Some(name) = parts.next() else {
        return false;
    };
    let Some(argument) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }

    if name.eq_ignore_ascii_case("/resume") {
        return matches!(
            parse_external_cli_resume_slash_command(command),
            Some(Ok(
                ExternalCliResumeSlashCommand::Pick(_) | ExternalCliResumeSlashCommand::New
            ))
        );
    }
    if name.eq_ignore_ascii_case("/model") {
        return matches!(
            parse_external_cli_model_slash_command(command),
            Some(Ok(ExternalCliModelSlashCommand::Set(_)))
        ) || (argument.eq_ignore_ascii_case("clear")
            && matches!(
                parse_external_cli_model_slash_command(command),
                Some(Ok(ExternalCliModelSlashCommand::Clear))
            ));
    }
    if name.eq_ignore_ascii_case("/effort") {
        return matches!(
            parse_external_cli_effort_slash_command(command),
            Some(Ok(ExternalCliEffortSlashCommand::Set(_)))
        ) || (argument.eq_ignore_ascii_case("clear")
            && matches!(
                parse_external_cli_effort_slash_command(command),
                Some(Ok(ExternalCliEffortSlashCommand::Clear))
            ));
    }
    false
}

fn normalize_button_label(label: &str) -> String {
    let normalized = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_BUTTON_LABEL_CHARS {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(MAX_BUTTON_LABEL_CHARS.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn non_empty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn raw_digest(raw: &serde_json::Value) -> String {
    let digest = digest(&SHA256, raw.to_string().as_bytes());
    let mut output = String::with_capacity(digest.as_ref().len() * 2 + 7);
    output.push_str("sha256:");
    for byte in digest.as_ref() {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn binding(chat_type: &str) -> FeishuChoiceCardBinding {
        FeishuChoiceCardBinding {
            provider_id: "feishu-main".to_string(),
            chat_id: "oc_chat".to_string(),
            chat_type: chat_type.to_string(),
            user_id: "ou_owner".to_string(),
        }
    }

    fn card_action(
        action: serde_json::Value,
        operator_id: &str,
        chat_id: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_id": "evt_click_1",
                "event_type": "card.action.trigger",
                "create_time": "1710000000000"
            },
            "event": {
                "operator": {
                    "open_id": operator_id
                },
                "action": action,
                "context": {
                    "open_message_id": "om_card",
                    "open_chat_id": chat_id
                }
            }
        })
    }

    fn binding_value_string(command: &str, chat_type: &str, expires_at_ms: u64) -> String {
        serde_json::json!({
            "bifrostAction": "slash_choice",
            "providerId": "feishu-main",
            "chatId": "oc_chat",
            "chatType": chat_type,
            "userId": "ou_owner",
            "command": command,
            "expiresAtMs": expires_at_ms
        })
        .to_string()
    }

    /// Builds a `select_static` callback (the current card shape): the selected
    /// binding travels as a JSON string inside `event.action.option`.
    fn callback(command: &str, chat_type: &str, expires_at_ms: u64) -> serde_json::Value {
        select_callback(
            &binding_value_string(command, chat_type, expires_at_ms),
            "ou_owner",
            "oc_chat",
        )
    }

    fn select_callback(option: &str, operator_id: &str, chat_id: &str) -> serde_json::Value {
        card_action(
            serde_json::json!({
                "tag": "select_static",
                "option": option,
                "value": { "bifrostAction": "slash_choice" }
            }),
            operator_id,
            chat_id,
        )
    }

    /// Builds a legacy `button` callback, where the binding is an object in
    /// `event.action.value`. Kept clickable for cards delivered before the
    /// dropdown migration.
    fn button_callback(
        command: &str,
        chat_type: &str,
        expires_at_ms: u64,
        operator_id: &str,
        chat_id: &str,
    ) -> serde_json::Value {
        card_action(
            serde_json::json!({
                "tag": "button",
                "value": {
                    "bifrostAction": "slash_choice",
                    "providerId": "feishu-main",
                    "chatId": "oc_chat",
                    "chatType": chat_type,
                    "userId": "ou_owner",
                    "command": command,
                    "expiresAtMs": expires_at_ms
                }
            }),
            operator_id,
            chat_id,
        )
    }

    #[test]
    fn feishu_choice_card_builds_select_static_dropdown() {
        let mut options = (0..5)
            .map(|index| FeishuChoiceCardOption {
                label: format!("Option {index}"),
                command: format!("/model model-{index}"),
            })
            .collect::<Vec<_>>();
        options[4].label = format!("  {}\n{}", "模型".repeat(60), "尾部");
        let card = build_feishu_choice_card("请选择模型", &binding("p2p"), &options, 1_000);

        assert_eq!(card["schema"], "2.0");
        assert!(card.get("header").is_none());
        let elements = card["body"]["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0]["tag"], "markdown");
        let select = &elements[1];
        assert_eq!(select["tag"], "select_static");
        assert_eq!(select["element_id"], "choice_select");
        assert!(select["placeholder"]["content"].as_str().is_some());
        assert_eq!(select["behaviors"][0]["type"], "callback");

        let select_options = select["options"].as_array().unwrap();
        assert_eq!(select_options.len(), options.len());
        let mut seen_values = HashSet::new();
        for (index, option) in options.iter().enumerate() {
            let entry = &select_options[index];
            let label = entry["text"]["content"].as_str().unwrap();
            assert!(label.chars().count() <= MAX_BUTTON_LABEL_CHARS);
            assert!(!label.contains('\n'));
            if index < 4 {
                assert_eq!(label, option.label);
            } else {
                assert!(label.ends_with("..."));
            }
            let value = entry["value"].as_str().expect("option value is a string");
            assert!(
                seen_values.insert(value.to_string()),
                "option values unique"
            );
            let parsed: serde_json::Value = serde_json::from_str(value).unwrap();
            assert_eq!(parsed["command"], option.command);
            assert_eq!(parsed["bifrostAction"], "slash_choice");
            assert_eq!(
                parsed["expiresAtMs"],
                1_000 + FEISHU_CHOICE_CARD_TTL.as_millis() as u64
            );
        }
    }

    #[test]
    fn feishu_card_action_normalizes_authorized_group_and_p2p_clicks() {
        for (chat_type, command) in [
            ("p2p", "/resume 01234567-89ab"),
            ("p2p", "/resume new"),
            ("group", "/model gpt-5.4"),
            ("p2p", "/model clear"),
            ("group", "/effort xhigh"),
            ("p2p", "/effort clear"),
        ] {
            let raw = callback(command, chat_type, 20_000);
            let event = normalize_feishu_card_action(&raw, "feishu-main", 10_000)
                .unwrap()
                .unwrap();
            assert_eq!(event.event_id, "evt_click_1");
            assert_eq!(event.event_type, "message.receive");
            assert_eq!(event.source.chat_id.as_deref(), Some("oc_chat"));
            assert_eq!(event.source.chat_type.as_deref(), Some(chat_type));
            assert_eq!(event.source.user_id.as_deref(), Some("ou_owner"));
            assert_eq!(event.source.message_id, None);
            assert_eq!(event.message.unwrap().text, command);
        }
    }

    #[test]
    fn feishu_card_action_normalizes_legacy_button_clicks() {
        let raw = button_callback("/effort high", "group", 20_000, "ou_owner", "oc_chat");
        let event = normalize_feishu_card_action(&raw, "feishu-main", 10_000)
            .unwrap()
            .unwrap();
        assert_eq!(event.message.unwrap().text, "/effort high");

        let expired = button_callback("/effort high", "group", 5_000, "ou_owner", "oc_chat");
        assert!(normalize_feishu_card_action(&expired, "feishu-main", 10_000).is_err());
    }

    #[test]
    fn feishu_card_action_rejects_unauthorized_expired_or_arbitrary_commands() {
        let valid_option = binding_value_string("/model gpt-5.4", "p2p", 20_000);
        let cases = [
            // wrong operator / wrong chat
            select_callback(&valid_option, "ou_intruder", "oc_chat"),
            select_callback(&valid_option, "ou_owner", "oc_other"),
            // expired
            select_callback(
                &binding_value_string("/model gpt-5.4", "p2p", 10_000),
                "ou_owner",
                "oc_chat",
            ),
            // forbidden / malformed commands
            select_callback(
                &binding_value_string("/stop now", "p2p", 20_000),
                "ou_owner",
                "oc_chat",
            ),
            select_callback(
                &binding_value_string("/model", "p2p", 20_000),
                "ou_owner",
                "oc_chat",
            ),
            select_callback(
                &binding_value_string("/model reset", "p2p", 20_000),
                "ou_owner",
                "oc_chat",
            ),
            select_callback(
                &binding_value_string("/effort auto", "p2p", 20_000),
                "ou_owner",
                "oc_chat",
            ),
            select_callback(
                &binding_value_string("/resume abc def", "p2p", 20_000),
                "ou_owner",
                "oc_chat",
            ),
            // option payload is not valid JSON
            select_callback("not-json", "ou_owner", "oc_chat"),
            // missing option field entirely
            card_action(
                serde_json::json!({ "tag": "select_static" }),
                "ou_owner",
                "oc_chat",
            ),
            // unsupported action tag
            card_action(
                serde_json::json!({ "tag": "date_picker", "value": {} }),
                "ou_owner",
                "oc_chat",
            ),
        ];
        for raw in cases {
            assert!(
                normalize_feishu_card_action(&raw, "feishu-main", 10_000).is_err(),
                "unexpected accepted action: {raw}"
            );
        }

        assert!(normalize_feishu_card_action(
            &select_callback(&valid_option, "ou_owner", "oc_chat"),
            "feishu-other",
            10_000
        )
        .is_err());
        assert!(normalize_feishu_card_action(
            &serde_json::json!({
                "header": {"event_type": "im.message.receive_v1"}
            }),
            "feishu-main",
            10_000
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn ws_message_dispatches_card_actions_and_rejects_invalid_clicks() {
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let sink = EventSink::from(sender);
        let raw = callback("/effort high", "group", u64::MAX);
        handle_ws_message(&raw.to_string(), "feishu-main", &sink);
        let event = events.try_recv().expect("card action event");
        assert_eq!(event.event_id, "evt_click_1");
        assert_eq!(event.source.message_id, None);
        assert_eq!(event.message.unwrap().text, "/effort high");

        let invalid = select_callback(
            &binding_value_string("/effort high", "group", u64::MAX),
            "ou_other",
            "oc_chat",
        );
        handle_ws_message(&invalid.to_string(), "feishu-main", &sink);
        assert!(events.try_recv().is_err());

        handle_ws_message("not-json", "feishu-main", &sink);
        handle_ws_message(r#"{"type":"pong"}"#, "feishu-main", &sink);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn permission_ws_message_persists_bot_added_lifecycle_events() {
        let temp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::im_gateway::ImEventStore::new(temp.path()));
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let sink =
            EventSink::with_durable_store(sender, std::sync::Arc::clone(&store), "feishu-main");
        let raw = serde_json::json!({
            "header": {
                "event_id": "evt-bot-added-ws",
                "event_type": "im.chat.member.bot.added_v1"
            },
            "event": {"chat_id": "oc_group"}
        });

        handle_ws_message(&raw.to_string(), "feishu-main", &sink);
        assert_eq!(
            events.try_recv().unwrap().event_type,
            crate::im_gateway::feishu_group_permission::BOT_JOINED_EVENT_TYPE
        );
        assert_eq!(store.pending_by_provider("feishu-main").len(), 1);
    }
}
