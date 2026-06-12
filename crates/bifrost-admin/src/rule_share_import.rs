use bifrost_core::rule_share::{
    content_sha256, imported_rule_content_hash, imported_rule_description, imported_rule_name,
    imported_rule_source_name, RuleShareExclusiveScope, RuleShareMode, RuleSharePayload,
};
use bifrost_core::{
    normalize_rule_content, rule_reference_name, validate_rules, BifrostError, Result,
};
use bifrost_storage::RuleFile;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::handlers::rules::notify_rules_changed_pub;
use crate::notification_db::{count_unread, create_notification, CreateNotification};
use crate::push::{NotificationPushData, SETTINGS_SCOPE_NOTIFICATIONS};
use crate::state::SharedAdminState;
use crate::SharedPushManager;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareImportAction {
    Created,
    Reused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleShareImportOutcome {
    pub action: RuleShareImportAction,
    pub rule_name: String,
    pub requested_name: String,
    pub content_hash: String,
    pub disabled_rules: Vec<String>,
}

pub async fn import_rule_share_payload(
    state: SharedAdminState,
    push_manager: Option<&SharedPushManager>,
    payload: RuleSharePayload,
) -> Result<RuleShareImportOutcome> {
    validate_import_payload(&payload)?;
    let normalized_content = normalize_rule_content(&payload.content);
    let requested_name = payload.name.trim().to_string();
    let content_hash = content_sha256(&normalized_content);
    let rule_name_base = imported_rule_name(&requested_name);
    let description = imported_rule_description(&requested_name, &content_hash);
    let existing_rules = state.rules_storage.load_all()?;

    if let Some(existing) = find_reusable_rule(
        &requested_name,
        &rule_name_base,
        &content_hash,
        &existing_rules,
    ) {
        let mut rule = existing.clone();
        rule.content = normalized_content;
        rule.enabled = true;
        rule.description = Some(description);
        rule.touch_local_change();
        state.rules_storage.save(&rule)?;
        clear_deleted_rule_intent(&state, &rule.name).await;

        let disabled_rules = apply_exclusive_enable(&state, &existing.name).await?;
        notify_after_import(&state, push_manager, &existing.name, false).await;
        return Ok(RuleShareImportOutcome {
            action: RuleShareImportAction::Reused,
            rule_name: existing.name.clone(),
            requested_name,
            content_hash,
            disabled_rules,
        });
    }

    let rule_name = unique_rule_name(&rule_name_base, &existing_rules);
    let sort_order = existing_rules
        .iter()
        .map(|rule| rule.sort_order)
        .min()
        .map(|value| value - 1)
        .unwrap_or(0);
    let mut rule = RuleFile::new(&rule_name, &normalized_content)
        .with_enabled(true)
        .with_sort_order(sort_order)
        .with_description(Some(description));
    rule.touch_local_change();
    state.rules_storage.save(&rule)?;

    clear_deleted_rule_intent(&state, &rule_name).await;

    let disabled_rules = apply_exclusive_enable(&state, &rule_name).await?;
    notify_after_import(&state, push_manager, &rule_name, true).await;
    Ok(RuleShareImportOutcome {
        action: RuleShareImportAction::Created,
        rule_name,
        requested_name,
        content_hash,
        disabled_rules,
    })
}

fn validate_import_payload(payload: &RuleSharePayload) -> Result<()> {
    if payload.mode != RuleShareMode::EnableExclusive {
        return Err(BifrostError::Config(
            "unsupported rule share import mode".to_string(),
        ));
    }
    if payload.exclusive_scope != RuleShareExclusiveScope::MyRules {
        return Err(BifrostError::Config(
            "unsupported rule share exclusive scope".to_string(),
        ));
    }

    let validation_content = strip_rule_reference_lines_for_validation(&payload.content);
    let errors = validate_rules(&validation_content);
    if !errors.is_empty() {
        let message = errors
            .first()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "rule validation failed".to_string());
        return Err(BifrostError::Rule(format!(
            "invalid shared rule content: {message}"
        )));
    }
    Ok(())
}

fn strip_rule_reference_lines_for_validation(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if rule_reference_name(line).is_some() {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unique_rule_name(base_name: &str, rules: &[RuleFile]) -> String {
    if !rules.iter().any(|rule| rule.name == base_name) {
        return base_name.to_string();
    }

    for index in 2.. {
        let candidate = format!("{base_name} {index}");
        if !rules.iter().any(|rule| rule.name == candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded rule name search should always return");
}

fn find_reusable_rule<'a>(
    requested_name: &str,
    base_name: &str,
    content_hash: &str,
    rules: &'a [RuleFile],
) -> Option<&'a RuleFile> {
    rules.iter().find(|rule| {
        (rule.name == base_name || is_generated_suffix_name(base_name, &rule.name))
            && imported_rule_source_name(rule.description.as_deref()).as_deref()
                == Some(requested_name)
            && imported_rule_content_hash(rule.description.as_deref()).as_deref()
                == Some(content_hash)
    })
}

fn is_generated_suffix_name(base_name: &str, candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix(base_name) else {
        return false;
    };
    let Some(number) = suffix.strip_prefix(' ') else {
        return false;
    };
    number
        .parse::<usize>()
        .map(|value| value >= 2)
        .unwrap_or(false)
}

async fn clear_deleted_rule_intent(state: &SharedAdminState, rule_name: &str) {
    if let Some(sync_manager) = state.sync_manager.clone() {
        if let Err(error) = sync_manager.clear_deleted_rule(rule_name).await {
            warn!(
                target: "bifrost_admin::rule_share",
                rule_name = %rule_name,
                error = %error,
                "failed to clear deleted sync intent for imported rule"
            );
        }
    }
}

async fn apply_exclusive_enable(
    state: &SharedAdminState,
    active_rule_name: &str,
) -> Result<Vec<String>> {
    let mut disabled_rules = Vec::new();
    for mut rule in state.rules_storage.load_all()? {
        let should_enable = rule.name == active_rule_name;
        if rule.enabled == should_enable {
            continue;
        }
        rule.enabled = should_enable;
        rule.touch_local_change();
        state.rules_storage.save(&rule)?;
        if !should_enable {
            disabled_rules.push(rule.name);
        }
    }
    Ok(disabled_rules)
}

async fn notify_after_import(
    state: &SharedAdminState,
    push_manager: Option<&SharedPushManager>,
    rule_name: &str,
    created: bool,
) {
    notify_rules_changed_pub(state);
    let title = if created {
        "Rule imported"
    } else {
        "Shared rule reused"
    };
    let message = if created {
        format!("Imported and enabled rule '{rule_name}'. Other My Rules were disabled.")
    } else {
        format!("Enabled existing rule '{rule_name}'. Other My Rules were disabled.")
    };
    let metadata = json!({
        "rule_name": rule_name,
        "created": created,
    });

    if let Err(error) = create_notification(&CreateNotification {
        notification_type: "rule_share_imported".to_string(),
        title: title.to_string(),
        message: message.clone(),
        metadata: Some(metadata.to_string()),
    }) {
        warn!(
            target: "bifrost_admin::rule_share",
            rule_name = %rule_name,
            error = %error,
            "failed to record rule share notification"
        );
    }

    if let Some(push_manager) = push_manager {
        push_manager.invalidate_overview_cache();
        let unread_count = count_unread().unwrap_or(0);
        push_manager
            .broadcast_notification(NotificationPushData {
                notification_type: "rule_share_imported".to_string(),
                title: title.to_string(),
                message,
                metadata: Some(metadata),
                unread_count,
            })
            .await;
        push_manager
            .broadcast_settings_scope(SETTINGS_SCOPE_NOTIFICATIONS)
            .await;
    }

    info!(
        target: "bifrost_admin::rule_share",
        rule_name = %rule_name,
        created = created,
        "rule share import completed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_rules_state() -> SharedAdminState {
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap().keep();
        let storage = bifrost_storage::RulesStorage::with_dir(dir).unwrap();
        Arc::new(crate::state::AdminState::new_for_test(0, storage))
    }

    #[tokio::test]
    async fn import_reuses_same_name_same_content() {
        let state = temp_rules_state();
        let payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "example.com bp://127.0.0.1:3000",
        )
        .unwrap();
        let first = import_rule_share_payload(state.clone(), None, payload.clone())
            .await
            .unwrap();
        let second = import_rule_share_payload(state.clone(), None, payload)
            .await
            .unwrap();

        assert_eq!(first.action, RuleShareImportAction::Created);
        assert_eq!(second.action, RuleShareImportAction::Reused);
        assert_eq!(
            state.rules_storage.list().unwrap(),
            vec!["share/shared".to_string()]
        );
    }

    #[tokio::test]
    async fn import_suffixes_same_name_different_content_and_disables_others() {
        let state = temp_rules_state();
        let first_payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "example.com bp://127.0.0.1:3000",
        )
        .unwrap();
        let second_payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "foo.test host://127.0.0.1:3000",
        )
        .unwrap();
        import_rule_share_payload(state.clone(), None, first_payload)
            .await
            .unwrap();
        let second = import_rule_share_payload(state.clone(), None, second_payload)
            .await
            .unwrap();

        assert_eq!(second.rule_name, "share/shared 2");
        assert!(!state.rules_storage.load("share/shared").unwrap().enabled);
        assert!(state.rules_storage.load("share/shared 2").unwrap().enabled);

        let third_payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "foo.test host://127.0.0.1:3000",
        )
        .unwrap();
        let third = import_rule_share_payload(state.clone(), None, third_payload)
            .await
            .unwrap();

        assert_eq!(third.action, RuleShareImportAction::Reused);
        assert_eq!(third.rule_name, "share/shared 2");
        assert_eq!(state.rules_storage.list().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn import_does_not_overwrite_user_rule_with_original_name() {
        let state = temp_rules_state();
        let user_rule = RuleFile::new("shared", "user.test direct").with_enabled(true);
        state.rules_storage.save(&user_rule).unwrap();

        let payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "example.com bp://127.0.0.1:3000",
        )
        .unwrap();
        let outcome = import_rule_share_payload(state.clone(), None, payload)
            .await
            .unwrap();

        assert_eq!(outcome.rule_name, "share/shared");
        assert_eq!(
            state.rules_storage.load("shared").unwrap().content,
            "user.test direct"
        );
        assert!(!state.rules_storage.load("shared").unwrap().enabled);
        assert!(state.rules_storage.load("share/shared").unwrap().enabled);
    }

    #[tokio::test]
    async fn import_accepts_rule_reference_lines() {
        let state = temp_rules_state();
        state
            .rules_storage
            .save(&RuleFile::new("a", "a.com status://200"))
            .unwrap();

        let payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "@a\nexample.com bp://127.0.0.1:3000",
        )
        .unwrap();
        let outcome = import_rule_share_payload(state.clone(), None, payload)
            .await
            .unwrap();

        assert_eq!(outcome.rule_name, "share/shared");
        let imported = state.rules_storage.load("share/shared").unwrap();
        assert!(imported.content.contains("@a"));
        assert!(imported.enabled);
        assert!(!state.rules_storage.load("a").unwrap().enabled);
    }

    #[tokio::test]
    async fn import_reopens_same_link_over_existing_share_rule() {
        let state = temp_rules_state();
        let payload = bifrost_core::rule_share::new_rule_share_payload(
            "shared",
            "example.com bp://127.0.0.1:3000",
        )
        .unwrap();
        import_rule_share_payload(state.clone(), None, payload.clone())
            .await
            .unwrap();

        let mut edited = state.rules_storage.load("share/shared").unwrap();
        edited.content = "edited.test direct".to_string();
        edited.enabled = false;
        edited.touch_local_change();
        state.rules_storage.save(&edited).unwrap();

        let outcome = import_rule_share_payload(state.clone(), None, payload)
            .await
            .unwrap();

        assert_eq!(outcome.action, RuleShareImportAction::Reused);
        assert_eq!(outcome.rule_name, "share/shared");
        let imported = state.rules_storage.load("share/shared").unwrap();
        assert_eq!(imported.content, "example.com bp://127.0.0.1:3000");
        assert!(imported.enabled);
        assert_eq!(state.rules_storage.list().unwrap().len(), 1);
    }
}
