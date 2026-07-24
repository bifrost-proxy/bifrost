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
    pub(super) external_cli_config_store:
        &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub(super) agent_config: &'a crate::im_gateway::agent::ImAgentConfig,
    pub(super) group_context_store: &'a Arc<ImGroupContextStore>,
    pub(super) group_turn_id: Option<&'a str>,
    pub(super) default_mode: BusyMessageDefaultMode,
    pub(super) status_context: bifrost_agent::StatusRuntimeContext,
    pub(super) default_work_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BusyMessageDefaultMode {
    Guide,
    ExternalGuide,
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
    external_cli_config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    provider_id: Option<&str>,
) -> BusyMessageDefaultMode {
    let runner_id = agent_config
        .runner
        .as_ref()
        .and_then(|runner| runner.custom_runner_id());
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        external_cli_config,
        provider_id,
        runner_id,
    );
    busy_default_mode_for_external_adapter(&effective.settings.adapter)
}

pub(super) fn busy_default_mode_for_external_adapter(adapter: &str) -> BusyMessageDefaultMode {
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        BusyMessageDefaultMode::Queue
    } else {
        BusyMessageDefaultMode::ExternalGuide
    }
}

#[cfg(test)]
pub(super) fn apply_busy_message_default(
    queue_manager: &SessionQueueManager,
    session_key: &str,
    message: &str,
    mode: BusyMessageDefaultMode,
) -> Result<BusyMessageDefaultResult, &'static str> {
    apply_busy_message_default_with_context(queue_manager, session_key, message, mode, None)
}

fn apply_busy_message_default_with_context(
    queue_manager: &SessionQueueManager,
    session_key: &str,
    message: &str,
    mode: BusyMessageDefaultMode,
    context: Option<crate::im_gateway::queue_manager::QueueItemContext>,
) -> Result<BusyMessageDefaultResult, &'static str> {
    let message = message.trim();
    if message.is_empty() {
        return Err("消息内容不能为空");
    }

    match mode {
        // A persisted group turn cannot be considered complete merely because
        // its guide entered this process's in-memory channel. Keep its reply
        // context and turn ID on the deferred queue so the eventual run owns
        // the terminal status transition.
        BusyMessageDefaultMode::Guide
            if context
                .as_ref()
                .and_then(|context| context.group_turn_id.as_deref())
                .is_some() =>
        {
            queue_manager
                .push_queue_with_images_and_context(
                    session_key,
                    message.to_string(),
                    Vec::new(),
                    context,
                )
                .map(|items| BusyMessageDefaultResult::Queue { items })
        }
        BusyMessageDefaultMode::Guide => {
            let pending_count = queue_manager.inject_guide(session_key, message.to_string());
            Ok(BusyMessageDefaultResult::Guide { pending_count })
        }
        BusyMessageDefaultMode::ExternalGuide | BusyMessageDefaultMode::Queue => queue_manager
            .push_queue_with_images_and_context(
                session_key,
                message.to_string(),
                Vec::new(),
                context,
            )
            .map(|items| BusyMessageDefaultResult::Queue { items }),
    }
}

pub(super) fn queue_item_context(
    ctx: &BusyMessageContext<'_>,
) -> crate::im_gateway::queue_manager::QueueItemContext {
    crate::im_gateway::queue_manager::QueueItemContext {
        event_id: ctx.event.event_id.clone(),
        message_id: ctx.event.source.message_id.clone(),
        user_id: ctx.event.source.user_id.clone(),
        user_name: ctx.event.source.user_name.clone(),
        group_turn_id: ctx.group_turn_id.map(ToString::to_string),
    }
}

pub(super) fn release_busy_group_turn(ctx: &BusyMessageContext<'_>, reason: &str) {
    let Some(turn_id) = ctx.group_turn_id else {
        return;
    };
    if let Err(error) = ctx
        .group_context_store
        .release_turn(turn_id, reason, now_ms())
    {
        warn!(turn_id = %turn_id, error = %error, "failed to release rejected busy group turn");
    }
}

pub(super) async fn handle_busy_guide_command(
    guide_text: &str,
    session_key: &str,
    ctx: &BusyMessageContext<'_>,
) {
    if ctx.default_mode == BusyMessageDefaultMode::ExternalGuide {
        let guide_id = format!("guide-{}", uuid::Uuid::new_v4());
        match crate::im_gateway::external_cli::request_worker_session_guide(
            session_key,
            guide_id,
            guide_text.to_string(),
        )
        .await
        {
            Ok(result) if result.accepted => {
                if let Some(turn_id) = ctx.group_turn_id {
                    ctx.queue_manager
                        .track_live_guide_turn(session_key, turn_id.to_string());
                }
                info!(
                    session_key = %session_key,
                    thread_id = ?result.thread_id,
                    turn_id = ?result.turn_id,
                    guide_msg_len = guide_text.len(),
                    "guide message delivered to active external runner session"
                );
                let reply = "🔀 已发送到当前 Runner session，将按 Runner 的实时引导语义生效";
                let updated = ctx
                    .progress_registry
                    .update_queue_state(
                        session_key,
                        ctx.queue_manager.queue_status(session_key),
                        false,
                        Some(format!("已收到引导：{}", truncate_str(guide_text, 48))),
                    )
                    .await;
                if !updated {
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        ctx.event,
                        reply,
                        ctx.message_log_store,
                    )
                    .await;
                }
                return;
            }
            Ok(result) => {
                warn!(
                    session_key = %session_key,
                    reason = ?result.reason,
                    "active external runner rejected guide; falling back to queue"
                );
            }
            Err(error) => {
                warn!(
                    session_key = %session_key,
                    error = %error,
                    "active external runner guide request failed; falling back to queue"
                );
            }
        }
    }
    match apply_busy_message_default_with_context(
        ctx.queue_manager,
        session_key,
        guide_text,
        ctx.default_mode,
        Some(queue_item_context(ctx)),
    ) {
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
            release_busy_group_turn(ctx, err);
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
    let message = message.trim();
    if message.is_empty() {
        release_busy_group_turn(ctx, "queued message is empty");
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "排队失败: 消息内容不能为空",
            ctx.message_log_store,
        )
        .await;
        return;
    }
    if ctx.default_mode == BusyMessageDefaultMode::ExternalGuide
        && ctx.event.message.as_ref().is_none_or(|event_message| {
            event_message.images.is_empty() && event_message.files.is_empty()
        })
    {
        handle_busy_guide_command(message, session_key, ctx).await;
        return;
    }
    if matches!(
        ctx.default_mode,
        BusyMessageDefaultMode::ExternalGuide | BusyMessageDefaultMode::Queue
    ) {
        let images = match ctx.event.message.as_ref() {
            Some(event_message) if !event_message.images.is_empty() => {
                resolve_event_images(ctx.client, ctx.provider, ctx.event, &event_message.images)
                    .await
            }
            _ => Vec::new(),
        };
        let files = match ctx.event.message.as_ref() {
            Some(event_message) if !event_message.files.is_empty() => {
                resolve_event_files(ctx.client, ctx.provider, ctx.event, &event_message.files).await
            }
            _ => Vec::new(),
        };
        match ctx.queue_manager.push_queue_with_attachments_and_context(
            session_key,
            message.to_string(),
            images,
            files,
            Some(queue_item_context(ctx)),
        ) {
            Ok(items) => {
                let guide_pending = !ctx.queue_manager.guide_status(session_key).is_empty();
                let status_message = if ctx.default_mode == BusyMessageDefaultMode::ExternalGuide {
                    format!(
                        "运行中引导暂不支持图片，已保留附件并排队：{}",
                        truncate_str(message, 48)
                    )
                } else {
                    format!("消息已排队：{}", truncate_str(message, 48))
                };
                let updated = ctx
                    .progress_registry
                    .update_queue_state(
                        session_key,
                        items.clone(),
                        guide_pending,
                        Some(status_message),
                    )
                    .await;
                if !updated {
                    let reply = if ctx.default_mode == BusyMessageDefaultMode::ExternalGuide {
                        format!(
                            "⚠️ 运行中引导暂不支持图片，已保留附件并排队（排队 {} 条）",
                            items.len()
                        )
                    } else {
                        format!(
                            "✅ 消息已收到，将在当前任务完成后处理（排队 {} 条）",
                            items.len()
                        )
                    };
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
                release_busy_group_turn(ctx, err);
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
        return;
    }

    match apply_busy_message_default_with_context(
        ctx.queue_manager,
        session_key,
        message,
        ctx.default_mode,
        Some(queue_item_context(ctx)),
    ) {
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
            release_busy_group_turn(ctx, err);
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
