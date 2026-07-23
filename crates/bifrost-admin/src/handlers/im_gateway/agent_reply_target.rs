use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentReplyTargetRef {
    pub(super) receive_id_type: String,
    pub(super) receive_id: String,
}

pub(super) fn agent_reply_target_ref(
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> Option<AgentReplyTargetRef> {
    match provider.provider_type {
        crate::im_gateway::types::ImProviderType::Weixin
        | crate::im_gateway::types::ImProviderType::WeChat
        | crate::im_gateway::types::ImProviderType::Webhook => first_non_empty([
            event.source.chat_id.as_deref(),
            event.source.user_id.as_deref(),
            provider.owner_open_id.as_deref(),
        ])
        .map(|receive_id| AgentReplyTargetRef {
            receive_id_type: "open_id".to_string(),
            receive_id,
        }),
        crate::im_gateway::types::ImProviderType::Feishu => {
            if let Some(chat_id) = first_non_empty([event.source.chat_id.as_deref()]) {
                return Some(AgentReplyTargetRef {
                    receive_id_type: "chat_id".to_string(),
                    receive_id: chat_id,
                });
            }
            first_non_empty([
                event.source.user_id.as_deref(),
                provider.owner_open_id.as_deref(),
            ])
            .map(|receive_id| AgentReplyTargetRef {
                receive_id_type: "open_id".to_string(),
                receive_id,
            })
        }
    }
}

fn first_non_empty<const N: usize>(values: [Option<&str>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn build_agent_reply_target(
    provider: &ImProviderConfig,
    event: &ImEvent,
    id: &str,
    display_name: &str,
    default_msg_type: &str,
) -> Option<crate::im_gateway::types::ImTarget> {
    let target_ref = agent_reply_target_ref(provider, event)?;
    Some(crate::im_gateway::types::ImTarget {
        id: id.to_string(),
        provider_id: provider.id.clone(),
        display_name: display_name.to_string(),
        enabled: true,
        receive_id_type: target_ref.receive_id_type,
        receive_id: target_ref.receive_id,
        default_msg_type: default_msg_type.to_string(),
        created_at: 0,
        updated_at: 0,
    })
}
