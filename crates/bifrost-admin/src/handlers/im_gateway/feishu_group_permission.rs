use std::sync::Arc;

use tracing::{error, info, warn};

use super::*;

pub(super) async fn handle_feishu_group_permission_check(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    store: &Arc<crate::im_gateway::FeishuGroupPermissionStore>,
    message_log_store: &Arc<ImMessageLogStore>,
    legacy_group: bool,
) -> bool {
    if provider.provider_type != ImProviderType::Feishu {
        return true;
    }
    let Some(chat_id) = event
        .source
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        warn!(
            provider_id = %provider.id,
            event_id = %event.event_id,
            "Feishu group permission check skipped because chat_id is missing"
        );
        return true;
    };
    let trigger_id = if legacy_group {
        "first-visible-message"
    } else {
        event.event_id.as_str()
    };
    let check_key = if legacy_group {
        store.legacy_check_key(&provider.id, chat_id)
    } else {
        store.join_check_key(&provider.id, chat_id, &event.event_id)
    };
    match store.is_complete(&check_key) {
        Ok(true) => return true,
        Ok(false) => {}
        Err(error) => {
            error!(
                provider_id = %provider.id,
                chat_id,
                event_id = %event.event_id,
                error = %error,
                "failed to read Feishu group permission check state"
            );
            return legacy_group;
        }
    }

    let Some(feishu) = client.feishu() else {
        return true;
    };
    let scope_status = match feishu
        .scope_grant_status(
            provider,
            crate::im_gateway::feishu_group_permission::REQUIRED_GROUP_MESSAGE_SCOPE,
        )
        .await
    {
        Ok(status) => status,
        Err(error) => {
            warn!(
                provider_id = %provider.id,
                chat_id,
                event_id = %event.event_id,
                error = %error,
                "unable to determine Feishu group-message permission; leaving check pending"
            );
            return legacy_group;
        }
    };

    if scope_status == crate::im_gateway::feishu::FeishuScopeGrantStatus::Granted {
        if let Err(error) = store.mark_granted(&check_key, &provider.id, chat_id, trigger_id) {
            error!(
                provider_id = %provider.id,
                chat_id,
                event_id = %event.event_id,
                error = %error,
                "failed to persist granted Feishu group-message permission"
            );
            return legacy_group;
        }
        if !legacy_group {
            let legacy_key = store.legacy_check_key(&provider.id, chat_id);
            if let Err(error) =
                store.mark_granted(&legacy_key, &provider.id, chat_id, "first-visible-message")
            {
                error!(
                    provider_id = %provider.id,
                    chat_id,
                    event_id = %event.event_id,
                    error = %error,
                    "failed to persist legacy-group granted permission state"
                );
            }
        }
        info!(
            provider_id = %provider.id,
            chat_id,
            event_id = %event.event_id,
            "Feishu group-message permission is granted"
        );
        return true;
    }

    let app_id = provider.app_id.as_deref().unwrap_or_default().trim();
    if app_id.is_empty() {
        error!(
            provider_id = %provider.id,
            chat_id,
            event_id = %event.event_id,
            "cannot build Feishu permission application link because app_id is empty"
        );
        return legacy_group;
    }
    let stable_uuid = match store.mark_sending_notice(&check_key, &provider.id, chat_id, trigger_id)
    {
        Ok(uuid) => uuid,
        Err(error) => {
            error!(
                provider_id = %provider.id,
                chat_id,
                event_id = %event.event_id,
                error = %error,
                "failed to persist pending Feishu permission notice"
            );
            return legacy_group;
        }
    };
    let target = ImTarget {
        id: format!("__permission_notice__:{chat_id}"),
        provider_id: provider.id.clone(),
        display_name: "Feishu group".to_string(),
        enabled: true,
        receive_id_type: "chat_id".to_string(),
        receive_id: chat_id.to_string(),
        default_msg_type: "text".to_string(),
        created_at: 0,
        updated_at: 0,
    };
    let notice = crate::im_gateway::feishu_group_permission::missing_permission_notice(app_id);
    let send_result = client
        .send_text_with_uuid(provider, &target, &notice, Some(&stable_uuid))
        .await;
    let (status, message_id, send_error) = match &send_result {
        Ok(result) => (MessageStatus::Success, result.message_id.clone(), None),
        Err(error) => (MessageStatus::Failed, None, Some(error.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(chat_id.to_string()),
        target_name: Some("Feishu group".to_string()),
        message_id: message_id.clone(),
        msg_type: Some(outbound_log_msg_type(provider, "text")),
        content_preview: Some(notice.clone()),
        content: Some(notice),
        trigger: Some("feishu_group_permission".to_string()),
        error: send_error.clone(),
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    let _ = message_log_store.add(log);

    match send_result {
        Ok(result) => {
            if let Err(error) = store.mark_notice_sent(&check_key, result.message_id.as_deref()) {
                error!(
                    provider_id = %provider.id,
                    chat_id,
                    event_id = %event.event_id,
                    error = %error,
                    "Feishu permission notice was sent but completion state was not persisted"
                );
                return legacy_group;
            }
            if !legacy_group {
                let legacy_key = store.legacy_check_key(&provider.id, chat_id);
                if store
                    .mark_sending_notice(
                        &legacy_key,
                        &provider.id,
                        chat_id,
                        "first-visible-message",
                    )
                    .and_then(|_| store.mark_notice_sent(&legacy_key, result.message_id.as_deref()))
                    .is_err()
                {
                    warn!(
                        provider_id = %provider.id,
                        chat_id,
                        event_id = %event.event_id,
                        "permission notice sent but legacy-group suppression state was not persisted"
                    );
                }
            }
            info!(
                provider_id = %provider.id,
                chat_id,
                event_id = %event.event_id,
                "sent Feishu group-message permission application notice"
            );
            true
        }
        Err(error) => {
            if let Err(store_error) = store.mark_notice_pending(&check_key, &error.to_string()) {
                error!(
                    provider_id = %provider.id,
                    chat_id,
                    event_id = %event.event_id,
                    error = %store_error,
                    "failed to persist Feishu permission notice retry state"
                );
            }
            warn!(
                provider_id = %provider.id,
                chat_id,
                event_id = %event.event_id,
                error = %error,
                "failed to send Feishu permission notice; leaving check pending"
            );
            legacy_group
        }
    }
}
