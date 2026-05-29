use super::*;

/// Handle an incoming message when the target session is already busy.
pub(super) struct BusyMessageContext<'a> {
    pub(super) queue_manager: &'a Arc<SessionQueueManager>,
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) event: &'a ImEvent,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) agent_session_manager: &'a Arc<ImAgentSessionManager>,
    pub(super) progress_registry: &'a Arc<ImAgentProgressRegistry>,
    pub(super) default_mode: BusyMessageDefaultMode,
    pub(super) status_context: bifrost_agent::StatusRuntimeContext,
    pub(super) default_work_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BusyMessageDefaultMode {
    Guide,
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BusyMessageDefaultResult {
    Guide {
        pending_count: usize,
    },
    Queue {
        items: Vec<crate::im_gateway::queue_manager::QueueItem>,
    },
}

pub(super) fn busy_default_mode_for_agent_config(
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
) -> BusyMessageDefaultMode {
    if agent_config
        .runner
        .as_ref()
        .and_then(|runner| runner.custom_runner_id())
        .is_some()
    {
        BusyMessageDefaultMode::Queue
    } else {
        BusyMessageDefaultMode::Guide
    }
}

pub(super) fn apply_busy_message_default(
    queue_manager: &SessionQueueManager,
    session_key: &str,
    message: &str,
    mode: BusyMessageDefaultMode,
) -> Result<BusyMessageDefaultResult, &'static str> {
    let message = message.trim();
    if message.is_empty() {
        return Err("消息内容不能为空");
    }

    match mode {
        BusyMessageDefaultMode::Guide => {
            let pending_count = queue_manager.inject_guide(session_key, message.to_string());
            Ok(BusyMessageDefaultResult::Guide { pending_count })
        }
        BusyMessageDefaultMode::Queue => queue_manager
            .push_queue(session_key, message.to_string())
            .map(|items| BusyMessageDefaultResult::Queue { items }),
    }
}

pub(super) async fn handle_busy_guide_command(
    guide_text: &str,
    session_key: &str,
    ctx: &BusyMessageContext<'_>,
) {
    match apply_busy_message_default(ctx.queue_manager, session_key, guide_text, ctx.default_mode) {
        Ok(BusyMessageDefaultResult::Guide {
            pending_count: pending_guide_count,
        }) => {
            let reply = if pending_guide_count > 1 {
                format!(
                    "🔀 已追加引导消息（当前 {} 条尚未进入 loop，将合并后生效）",
                    pending_guide_count
                )
            } else {
                "🔀 已注入引导消息，将在当前工具调用完成后生效".to_string()
            };
            info!(
                session_key = %session_key,
                guide_msg_len = guide_text.len(),
                pending_guide_count = pending_guide_count,
                "guide message injected via IM /g command"
            );
            let updated = ctx
                .progress_registry
                .update_queue_state(
                    session_key,
                    ctx.queue_manager.queue_status(session_key),
                    true,
                    Some(format!("已收到引导：{}", truncate_str(guide_text, 48))),
                )
                .await;
            if !updated {
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &reply,
                    ctx.message_log_store,
                )
                .await;
            }
        }
        Ok(BusyMessageDefaultResult::Queue { items }) => {
            info!(
                session_key = %session_key,
                guide_msg_len = guide_text.len(),
                queue_len = items.len(),
                "guide command queued because active runner does not support guide injection"
            );
            let guide_pending = !ctx.queue_manager.guide_status(session_key).is_empty();
            let updated = ctx
                .progress_registry
                .update_queue_state(
                    session_key,
                    items.clone(),
                    guide_pending,
                    Some(format!(
                        "Runner 不支持运行中引导，已排队：{}",
                        truncate_str(guide_text, 48)
                    )),
                )
                .await;
            if !updated {
                let reply = format!(
                    "⚠️ 当前 Runner 不支持运行中引导，已作为排队消息处理（排队 {} 条）",
                    items.len()
                );
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &reply,
                    ctx.message_log_store,
                )
                .await;
            }
        }
        Err(err) => {
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                &format!("❌ {err}"),
                ctx.message_log_store,
            )
            .await;
        }
    }
}

pub(super) async fn handle_busy_default_message(
    message: &str,
    session_key: &str,
    ctx: &BusyMessageContext<'_>,
) {
    match apply_busy_message_default(ctx.queue_manager, session_key, message, ctx.default_mode) {
        Ok(BusyMessageDefaultResult::Guide { pending_count }) => {
            let reply = if pending_count > 1 {
                format!(
                    "🔀 已追加引导消息（当前 {} 条尚未进入 loop，将合并后生效）",
                    pending_count
                )
            } else {
                "🔀 已注入引导消息，将在当前工具调用完成后生效".to_string()
            };
            let updated = ctx
                .progress_registry
                .update_queue_state(
                    session_key,
                    ctx.queue_manager.queue_status(session_key),
                    true,
                    Some(format!("已收到引导：{}", truncate_str(message, 48))),
                )
                .await;
            if !updated {
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &reply,
                    ctx.message_log_store,
                )
                .await;
            }
        }
        Ok(BusyMessageDefaultResult::Queue { items }) => {
            let guide_pending = !ctx.queue_manager.guide_status(session_key).is_empty();
            let updated = ctx
                .progress_registry
                .update_queue_state(
                    session_key,
                    items.clone(),
                    guide_pending,
                    Some(format!("消息已排队：{}", truncate_str(message, 48))),
                )
                .await;
            if !updated {
                let reply = format!(
                    "✅ 消息已收到，将在当前任务完成后处理（排队 {} 条）",
                    items.len()
                );
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &reply,
                    ctx.message_log_store,
                )
                .await;
            }
        }
        Err(err) => {
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                &format!("排队失败: {err}"),
                ctx.message_log_store,
            )
            .await;
        }
    }
}

/// Format queue status as a user-friendly string.
pub(super) fn format_queue_status(
    header: &str,
    items: &[crate::im_gateway::queue_manager::QueueItem],
) -> String {
    let mut text = header.to_string();
    if items.is_empty() {
        text.push_str("\n\n📋 排队已清空");
    } else {
        text.push_str(&format!("\n\n📋 当前排队（{}条）：", items.len()));
        for item in items {
            let preview = truncate_str(&item.message, 60);
            text.push_str(&format!(
                "\n{}. [#{}] {}",
                items.iter().position(|i| i.seq == item.seq).unwrap_or(0) + 1,
                item.seq,
                preview
            ));
        }
    }
    text
}

pub(super) fn format_pending_guide_status(guides: &[String]) -> String {
    if guides.is_empty() {
        return "- 引导消息: 无".to_string();
    }

    let mut text = format!("- 引导消息: {} 条尚未进入 loop", guides.len());
    for (idx, guide) in guides.iter().enumerate() {
        text.push_str(&format!("\n  {}. {}", idx + 1, truncate_str(guide, 80)));
    }
    text
}

pub(super) fn merge_pending_guide_messages(
    active_guides: &[String],
    queue_guides: Vec<String>,
) -> Vec<String> {
    let mut merged = active_guides.to_vec();
    for guide in queue_guides {
        if !merged.iter().any(|existing| existing == &guide) {
            merged.push(guide);
        }
    }
    merged
}
