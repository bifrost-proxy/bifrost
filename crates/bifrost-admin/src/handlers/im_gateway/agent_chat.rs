use super::*;

// ---------------------------------------------------------------------------

pub(super) struct IdleImCommandContext<'a> {
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) provider_store: &'a Arc<ImProviderStore>,
    pub(super) external_cli_config_store:
        &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub(super) event: &'a ImEvent,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) agent_session_manager: &'a Arc<ImAgentSessionManager>,
}

struct ImCwdCommandContext<'a> {
    client: &'a ImProviderClient,
    provider: &'a ImProviderConfig,
    provider_store: &'a Arc<ImProviderStore>,
    event: &'a ImEvent,
    message_log_store: &'a Arc<ImMessageLogStore>,
    session_manager: &'a Arc<ImAgentSessionManager>,
}

struct ImRunnerCommandContext<'a> {
    client: &'a ImProviderClient,
    provider: &'a ImProviderConfig,
    provider_store: &'a Arc<ImProviderStore>,
    external_cli_config_store: &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    event: &'a ImEvent,
    message_log_store: &'a Arc<ImMessageLogStore>,
    session_manager: &'a Arc<ImAgentSessionManager>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImRunnerCommand {
    List,
    Switch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImRunnerSelection {
    runner_id: String,
    runner: bifrost_agent::AgentRunnerMode,
    adapter: Option<String>,
}

const BUILTIN_IM_RUNNER_ID: &str = "bifrost_agent";

pub(super) async fn handle_idle_im_command(
    msg_text: &str,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    ctx: IdleImCommandContext<'_>,
) -> bool {
    let trimmed = msg_text.trim();
    if trimmed == "/status" {
        let detail = ctx.agent_session_manager.get_session_detail(session_key);
        let status_context = status_context_from_agent_config(agent_config);
        let default_work_dir = agent_config.resolve_work_dir().display().to_string();
        let reply = build_im_status_text(
            detail.as_ref(),
            &status_context,
            Some(default_work_dir.as_str()),
        );
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            &reply,
            ctx.message_log_store,
        )
        .await;
        return true;
    }

    if trimmed == "/stop" {
        let stopped = request_agent_stop(ctx.agent_session_manager, session_key).await;
        let reply = if stopped {
            "已请求停止当前 Agent loop。"
        } else {
            "当前没有正在执行的 Agent loop。"
        };
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            reply,
            ctx.message_log_store,
        )
        .await;
        return true;
    }

    if handle_im_cwd_command(
        trimmed,
        session_key,
        ImCwdCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            provider_store: ctx.provider_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
            session_manager: ctx.agent_session_manager,
        },
    )
    .await
    {
        return true;
    }

    if handle_im_runner_command(
        trimmed,
        session_key,
        ImRunnerCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            provider_store: ctx.provider_store,
            external_cli_config_store: ctx.external_cli_config_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
            session_manager: ctx.agent_session_manager,
        },
    )
    .await
    {
        return true;
    }

    if let Some(response) =
        bifrost_agent::handle_session_free_command(session_key, msg_text, agent_config)
    {
        let response = if trimmed == "/help" {
            append_im_channel_help(response)
        } else {
            response
        };
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            &response,
            ctx.message_log_store,
        )
        .await;
        return true;
    }

    false
}

pub(super) fn append_im_channel_help(mut help_text: String) -> String {
    help_text.push_str(
        "\n\nIM 通道命令:\n\
         /cwd <绝对路径>  切换当前 IM 通道绑定的工作目录；路径必须存在且是目录，运行中会排队到当前任务结束后执行\n\
         /runner [Runner]  查看或切换当前 IM 通道绑定的 Runner\n\
         /q <消息>       将消息加入队列，当前任务结束后自动继续处理\n\
         /rq <序号>      取消一条排队消息\n\
         /g <引导内容>   给正在运行的内置 Agent 注入引导；外部 Runner 会按队列处理",
    );
    help_text
}

pub(super) fn parse_im_cwd_command(message: &str) -> Option<Result<PathBuf, String>> {
    let trimmed = message.trim();
    if trimmed == "/cwd" {
        return Some(Err("用法: /cwd <绝对路径>".to_string()));
    }
    let rest = trimmed.strip_prefix("/cwd ")?;
    let mut path_text = rest.trim();
    if path_text.is_empty() {
        return Some(Err("用法: /cwd <绝对路径>".to_string()));
    }
    if path_text.len() >= 2 {
        let bytes = path_text.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            path_text = &path_text[1..path_text.len() - 1];
        }
    }
    let path = PathBuf::from(path_text);
    if !path.is_absolute() {
        return Some(Err(
            "请使用绝对路径，例如 /cwd /Users/eden/work/github/bifrost".to_string(),
        ));
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return Some(Err(format!("路径不存在: {}", path.display()))),
    };
    if !metadata.is_dir() {
        return Some(Err(format!("路径存在但不是目录: {}", path.display())));
    }
    Some(Ok(std::fs::canonicalize(&path).unwrap_or(path)))
}

pub(super) fn parse_im_runner_command(message: &str) -> Option<ImRunnerCommand> {
    let trimmed = message.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let command = parts.next()?;
    if !command.eq_ignore_ascii_case("/runner") {
        return None;
    }
    let runner_id = parts.next().unwrap_or("").trim();
    if runner_id.is_empty() {
        Some(ImRunnerCommand::List)
    } else {
        Some(ImRunnerCommand::Switch(runner_id.to_string()))
    }
}

pub(super) fn format_im_runner_list(
    config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
) -> String {
    let mut names = std::collections::BTreeSet::new();
    names.insert(BUILTIN_IM_RUNNER_ID.to_string());
    if !config.default_runner_id.trim().is_empty() {
        names.insert(config.default_runner_id.trim().to_string());
    }
    names.extend(
        config
            .runners
            .keys()
            .map(|runner_id| runner_id.trim())
            .filter(|runner_id| !runner_id.is_empty())
            .map(ToString::to_string),
    );
    names.into_iter().collect::<Vec<_>>().join("\n")
}

pub(super) fn resolve_im_runner_selection(
    config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    runner_id: &str,
) -> Result<ImRunnerSelection, String> {
    let runner_id = runner_id.trim();
    if runner_id.is_empty() {
        return Err("用法: /runner <Runner>".to_string());
    }
    if runner_id == BUILTIN_IM_RUNNER_ID {
        return Ok(ImRunnerSelection {
            runner_id: BUILTIN_IM_RUNNER_ID.to_string(),
            runner: bifrost_agent::AgentRunnerMode::BifrostAgent,
            adapter: None,
        });
    }
    let Some(settings) = config.runners.get(runner_id) else {
        return Err(format!(
            "找不到 Runner: `{}`\n\n支持的 Runner:\n{}",
            runner_id,
            format_im_runner_list(config)
        ));
    };
    Ok(ImRunnerSelection {
        runner_id: runner_id.to_string(),
        runner: bifrost_agent::AgentRunnerMode::Custom(runner_id.to_string()),
        adapter: Some(settings.adapter.clone()),
    })
}

pub(super) fn apply_im_runner_switch_to_session(
    provider_store: &Arc<ImProviderStore>,
    provider_id: &str,
    session_key: &str,
    session: &mut bifrost_agent::AgentSession,
    selection: &ImRunnerSelection,
) -> String {
    persist_provider_agent_runner(provider_store, provider_id, selection.runner.clone());
    clear_persisted_agent_session_state(session_key, None, None);
    session.clear();
    match &selection.runner {
        bifrost_agent::AgentRunnerMode::BifrostAgent => session.mark_bifrost_agent_runtime(),
        bifrost_agent::AgentRunnerMode::Custom(_) => {
            session.mark_external_runner_runtime(
                &selection.runner_id,
                selection.adapter.as_deref().unwrap_or(&selection.runner_id),
            );
        }
    }
    format!(
        "已切换 Runner 到:\n`{}`\n\n下一条消息将使用新的 Runner。",
        selection.runner_id
    )
}

pub(super) fn apply_im_runner_switch(
    provider_store: &Arc<ImProviderStore>,
    session_manager: &Arc<ImAgentSessionManager>,
    provider_id: &str,
    session_key: &str,
    config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    runner_id: &str,
) -> Result<String, String> {
    let selection = resolve_im_runner_selection(config, runner_id)?;
    let Some(mut session) = session_manager.try_take_session_with_work_dir(session_key, None)
    else {
        return Err("当前 Agent 正在处理中，Runner 切换需要等待当前任务完成后执行。".to_string());
    };
    let reply = apply_im_runner_switch_to_session(
        provider_store,
        provider_id,
        session_key,
        &mut session,
        &selection,
    );
    session_manager.return_session(session);
    Ok(reply)
}

pub(super) fn format_im_runner_error(reason: &str) -> String {
    format!("无法切换 Runner：{reason}")
}

pub(super) fn format_im_cwd_error(reason: &str) -> String {
    format!("❌ 无法切换工作目录：{reason}\n\n用法: /cwd <绝对路径>")
}

async fn handle_im_runner_command(
    message: &str,
    session_key: &str,
    ctx: ImRunnerCommandContext<'_>,
) -> bool {
    let Some(command) = parse_im_runner_command(message) else {
        return false;
    };
    let config = ctx.external_cli_config_store.load();
    let reply = match command {
        ImRunnerCommand::List => format_im_runner_list(&config),
        ImRunnerCommand::Switch(runner_id) => match apply_im_runner_switch(
            ctx.provider_store,
            ctx.session_manager,
            &ctx.provider.id,
            session_key,
            &config,
            &runner_id,
        ) {
            Ok(reply) => reply,
            Err(reason) => format_im_runner_error(&reason),
        },
    };
    send_agent_reply(
        ctx.client,
        ctx.provider,
        ctx.event,
        &reply,
        ctx.message_log_store,
    )
    .await;
    true
}

pub(super) fn apply_im_cwd_switch_to_session(
    provider_store: &Arc<ImProviderStore>,
    provider_id: &str,
    session_key: &str,
    session: &mut bifrost_agent::AgentSession,
    work_dir: &Path,
) -> String {
    let canonical_work_dir =
        std::fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
    let work_dir = canonical_work_dir.display().to_string();
    persist_provider_agent_work_dir(provider_store, provider_id, &work_dir);
    clear_persisted_agent_session_state(session_key, None, None);
    session.reinitialize_work_dir(work_dir.clone());
    format!("已切换工作目录到:\n`{work_dir}`\n\n下一条消息将使用新的工作目录。")
}

pub(super) fn apply_im_cwd_switch(
    provider_store: &Arc<ImProviderStore>,
    session_manager: &Arc<ImAgentSessionManager>,
    provider_id: &str,
    session_key: &str,
    work_dir: &Path,
) -> Result<String, String> {
    let canonical_work_dir =
        std::fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
    let Some(mut session) = session_manager.try_take_session_with_work_dir(
        session_key,
        Some(canonical_work_dir.display().to_string()),
    ) else {
        return Err("当前 Agent 正在处理中，/cwd 需要等待当前任务完成后执行。".to_string());
    };
    let reply = apply_im_cwd_switch_to_session(
        provider_store,
        provider_id,
        session_key,
        &mut session,
        &canonical_work_dir,
    );
    session_manager.return_session(session);
    Ok(reply)
}

async fn handle_im_cwd_command(
    message: &str,
    session_key: &str,
    ctx: ImCwdCommandContext<'_>,
) -> bool {
    let Some(command) = parse_im_cwd_command(message) else {
        return false;
    };
    let reply = match command {
        Ok(path) => match apply_im_cwd_switch(
            ctx.provider_store,
            ctx.session_manager,
            &ctx.provider.id,
            session_key,
            &path,
        ) {
            Ok(reply) => reply,
            Err(reason) => format_im_cwd_error(&reason),
        },
        Err(reason) => format_im_cwd_error(&reason),
    };
    send_agent_reply(
        ctx.client,
        ctx.provider,
        ctx.event,
        &reply,
        ctx.message_log_store,
    )
    .await;
    true
}

pub(super) async fn resolve_event_images(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    images: &[ImImageAttachment],
) -> Vec<bifrost_agent::ChatImageInput> {
    let mut resolved = Vec::new();
    if images.len() > MAX_AGENT_IMAGES_PER_MESSAGE {
        warn!(
            provider_id = %provider.id,
            event_id = %event.event_id,
            image_count = images.len(),
            max_images = MAX_AGENT_IMAGES_PER_MESSAGE,
            "too many IM images in one message; truncating images for agent multimodal input"
        );
    }
    for image in images.iter().take(MAX_AGENT_IMAGES_PER_MESSAGE) {
        if let (Some(mime_type), Some(data)) = (&image.mime_type, &image.data_base64) {
            resolved.push(bifrost_agent::ChatImageInput {
                mime_type: mime_type.clone(),
                data: data.clone(),
            });
            continue;
        }

        let Some(message_id) = event.source.message_id.as_deref() else {
            warn!(
                provider_id = %provider.id,
                file_key = %image.file_key,
                "cannot download IM image because message_id is missing"
            );
            continue;
        };
        match client
            .download_message_image_resource(provider, message_id, image)
            .await
        {
            Ok((mime_type, bytes)) => {
                info!(
                    provider_id = %provider.id,
                    message_id = %message_id,
                    file_key = %image.file_key,
                    mime_type = %mime_type,
                    byte_len = bytes.len(),
                    "downloaded IM image resource for agent multimodal input"
                );
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                resolved.push(bifrost_agent::ChatImageInput { mime_type, data });
            }
            Err(error) => {
                warn!(
                    provider_id = %provider.id,
                    message_id = %message_id,
                    file_key = %image.file_key,
                    error = %error,
                    "failed to download IM image resource"
                );
            }
        }
    }
    resolved
}

pub(super) fn agent_message_text(message: &crate::im_gateway::types::ImEventMessage) -> String {
    let text = message.text.trim();
    if !text.is_empty() {
        text.to_string()
    } else if !message.images.is_empty() {
        IMAGE_ONLY_AGENT_PROMPT.to_string()
    } else {
        String::new()
    }
}

pub(super) fn inbound_message_preview(
    message: &crate::im_gateway::types::ImEventMessage,
) -> String {
    if message.text.trim().is_empty() && !message.images.is_empty() {
        format!("[图片消息: {} 张]", message.images.len())
    } else {
        truncate_str(&message.text, 200)
    }
}

pub(super) async fn handle_busy_message(
    msg_text: &str,
    session_key: &str,
    ctx: BusyMessageContext<'_>,
) {
    let queue_manager = ctx.queue_manager;
    let client = ctx.client;
    let provider = ctx.provider;
    let event = ctx.event;
    let message_log_store = ctx.message_log_store;
    let agent_session_manager = ctx.agent_session_manager;
    let progress_registry = ctx.progress_registry;
    let trimmed = msg_text.trim();

    // /status — show session status or busy indicator
    if trimmed == "/status" {
        // Try to get session detail from idle sessions
        if let Some(detail) = agent_session_manager.get_session_detail(session_key) {
            let reply = build_im_status_text(
                Some(&detail),
                &ctx.status_context,
                ctx.default_work_dir.as_deref(),
            );
            send_agent_reply(client, provider, event, &reply, message_log_store).await;
        } else if let Some(mut status) = agent_session_manager.get_active_turn_status(session_key) {
            status.pending_guide_messages = merge_pending_guide_messages(
                &status.pending_guide_messages,
                queue_manager.guide_status(session_key),
            );
            let queue_items = queue_manager.queue_status(session_key);
            let queue_info = if queue_items.is_empty() {
                "无排队消息".to_string()
            } else {
                format!("{} 条排队消息", queue_items.len())
            };
            let reply = format!(
                "{}\n- 排队: {}",
                bifrost_agent::format_active_turn_status_text_with_context(
                    &status,
                    &ctx.status_context
                ),
                queue_info
            );
            send_agent_reply(client, provider, event, &reply, message_log_store).await;
        } else {
            // Session is currently being processed (taken out of the pool)
            let queue_items = queue_manager.queue_status(session_key);
            let queue_info = if queue_items.is_empty() {
                "无排队消息".to_string()
            } else {
                format!("{} 条排队消息", queue_items.len())
            };
            let guide_info = format_pending_guide_status(&queue_manager.guide_status(session_key));
            let reply = format!(
                "会话状态:\n- 状态: 🔵 正在处理中\n- 排队: {}\n{}\n\n请等待当前任务完成后再查询详细状态。",
                queue_info, guide_info
            );
            send_agent_reply(client, provider, event, &reply, message_log_store).await;
        }
        return;
    }

    // /stop — cooperative cancellation of the active turn loop
    if trimmed == "/stop" {
        let stopped = request_agent_stop(agent_session_manager, session_key).await;
        let reply = if stopped {
            "🛑 已请求停止当前 Agent loop。"
        } else {
            "当前没有正在执行的 Agent loop。"
        };
        send_agent_reply(client, provider, event, reply, message_log_store).await;
        return;
    }

    if let Some(command) = parse_im_runner_command(trimmed) {
        let config = ctx.external_cli_config_store.load();
        let reply = match command {
            ImRunnerCommand::List => format_im_runner_list(&config),
            ImRunnerCommand::Switch(runner_id) => {
                match resolve_im_runner_selection(&config, &runner_id) {
                    Ok(_) => "当前任务正在处理中，请等待任务结束后再切换 Runner。".to_string(),
                    Err(reason) => format_im_runner_error(&reason),
                }
            }
        };
        send_agent_reply(client, provider, event, &reply, message_log_store).await;
        return;
    }

    if let Some(command) = parse_im_cwd_command(trimmed) {
        match command {
            Ok(path) => {
                let queued_command = format!("/cwd {}", path.display());
                match queue_manager.push_queue(session_key, queued_command) {
                    Ok(items) => {
                        let guide_pending = !queue_manager.guide_status(session_key).is_empty();
                        let updated = progress_registry
                            .update_queue_state(
                                session_key,
                                items.clone(),
                                guide_pending,
                                Some(format!("已将工作目录切换排队：{}", path.display())),
                            )
                            .await;
                        if !updated {
                            let reply = format!(
                                "⏳ 当前任务仍在处理中，已将工作目录切换排队。\n\n当前任务结束后将切换到:\n`{}`",
                                path.display()
                            );
                            send_agent_reply(client, provider, event, &reply, message_log_store)
                                .await;
                        }
                    }
                    Err(err) => {
                        send_agent_reply(
                            client,
                            provider,
                            event,
                            &format!("❌ {err}"),
                            message_log_store,
                        )
                        .await;
                    }
                }
            }
            Err(reason) => {
                send_agent_reply(
                    client,
                    provider,
                    event,
                    &format_im_cwd_error(&reason),
                    message_log_store,
                )
                .await;
            }
        }
        return;
    }

    // /q <text> — queue mode
    if let Some(rest) = trimmed.strip_prefix("/q ") {
        let queue_text = rest.trim();
        if queue_text.is_empty() {
            send_agent_reply(
                client,
                provider,
                event,
                "用法: /q <消息内容>",
                message_log_store,
            )
            .await;
            return;
        }
        match queue_manager.push_queue(session_key, queue_text.to_string()) {
            Ok(items) => {
                let guide_pending = !queue_manager.guide_status(session_key).is_empty();
                let updated = progress_registry
                    .update_queue_state(
                        session_key,
                        items.clone(),
                        guide_pending,
                        Some(format!("已加入排队：{} 条", items.len())),
                    )
                    .await;
                if !updated {
                    let reply = format_queue_status("✅ 已加入排队", &items);
                    send_agent_reply(client, provider, event, &reply, message_log_store).await;
                }
            }
            Err(err) => {
                send_agent_reply(
                    client,
                    provider,
                    event,
                    &format!("❌ {err}"),
                    message_log_store,
                )
                .await;
            }
        }
        return;
    }

    // /rq <N> — remove queued message
    if let Some(rest) = trimmed.strip_prefix("/rq ") {
        let rest = rest.trim();
        match rest.parse::<u64>() {
            Ok(seq) => {
                if queue_manager.remove_queue(session_key, seq) {
                    let items = queue_manager.queue_status(session_key);
                    let guide_pending = !queue_manager.guide_status(session_key).is_empty();
                    let updated = progress_registry
                        .update_queue_state(
                            session_key,
                            items.clone(),
                            guide_pending,
                            Some(format!("已删除排队消息 #{seq}")),
                        )
                        .await;
                    if !updated {
                        let reply = format_queue_status(&format!("🗑️ 已删除 #{seq}"), &items);
                        send_agent_reply(client, provider, event, &reply, message_log_store).await;
                    }
                } else {
                    send_agent_reply(
                        client,
                        provider,
                        event,
                        &format!("❌ 未找到排队消息 #{seq}"),
                        message_log_store,
                    )
                    .await;
                }
            }
            Err(_) => {
                send_agent_reply(
                    client,
                    provider,
                    event,
                    "用法: /rq <序号>（如 /rq 1）",
                    message_log_store,
                )
                .await;
            }
        }
        return;
    }

    // Other builtin commands that need session state — defer until session is free
    if matches!(
        trimmed,
        "/clear" | "/reset" | "/undo" | "/compact" | "/resume" | "/goal" | "/skill"
    ) || trimmed.starts_with("/undo ")
        || trimmed.starts_with("/goal ")
        || trimmed.starts_with("/skill ")
    {
        let reply = format!(
            "⏳ Agent 正在处理中，{} 命令需要等待当前任务完成后执行。\n\n\
             可用操作:\n\
             - /q <消息> — 排队消息\n\
             - /rq <序号> — 取消排队\n\
             - /status — 查看状态\n\
             - /stop — 立即停止当前 loop\n\
             - /help — 查看帮助",
            trimmed.split_whitespace().next().unwrap_or(trimmed)
        );
        send_agent_reply(client, provider, event, &reply, message_log_store).await;
        return;
    }

    // /g <text> — guide injection when the active runtime supports it; otherwise queue.
    if let Some(rest) = trimmed.strip_prefix("/g ") {
        let guide_text = rest.trim();
        if guide_text.is_empty() {
            send_agent_reply(
                client,
                provider,
                event,
                "用法: /g <引导内容>",
                message_log_store,
            )
            .await;
            return;
        }
        handle_busy_guide_command(guide_text, session_key, &ctx).await;
        return;
    }

    // Default behavior depends on the active runtime:
    // - built-in Bifrost Agent supports mid-turn guide injection
    // - external runners (ChatGPT Web / CLI runners) must queue until the run finishes
    handle_busy_default_message(trimmed, session_key, &ctx).await;
}

pub(super) async fn run_progress_event_coalescer(
    progress_registry: Arc<ImAgentProgressRegistry>,
    session_key: String,
    rx: &mut mpsc::UnboundedReceiver<bifrost_agent::AgentTurnProgressEvent>,
) {
    const STATUS_COALESCE_MS: u64 = 300;
    while let Some(first) = rx.recv().await {
        let mut immediate = progress_event_needs_immediate_flush(&first);
        let mut events = vec![first];
        while let Ok(event) = rx.try_recv() {
            immediate |= progress_event_needs_immediate_flush(&event);
            events.push(event);
        }
        if !immediate {
            let deadline = tokio::time::sleep(std::time::Duration::from_millis(STATUS_COALESCE_MS));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    maybe_event = rx.recv() => {
                        let Some(event) = maybe_event else {
                            break;
                        };
                        let mut batch_is_immediate = progress_event_needs_immediate_flush(&event);
                        events.push(event);
                        while let Ok(event) = rx.try_recv() {
                            let drained_is_immediate = progress_event_needs_immediate_flush(&event);
                            events.push(event);
                            if drained_is_immediate {
                                batch_is_immediate = true;
                                break;
                            }
                        }
                        if batch_is_immediate {
                            break;
                        }
                    }
                }
            }
        }
        progress_registry.apply_events(&session_key, events).await;
    }
}

pub(super) fn progress_event_needs_immediate_flush(
    event: &bifrost_agent::AgentTurnProgressEvent,
) -> bool {
    matches!(
        event,
        bifrost_agent::AgentTurnProgressEvent::ToolStarted { .. }
            | bifrost_agent::AgentTurnProgressEvent::ToolFinished { .. }
            | bifrost_agent::AgentTurnProgressEvent::LongTaskStatus { .. }
            | bifrost_agent::AgentTurnProgressEvent::PlanUpdated { .. }
            | bifrost_agent::AgentTurnProgressEvent::ProposedPlan { .. }
            | bifrost_agent::AgentTurnProgressEvent::TitleUpdated { .. }
            | bifrost_agent::AgentTurnProgressEvent::AssistantDelta { .. }
            | bifrost_agent::AgentTurnProgressEvent::AssistantFinal { .. }
            | bifrost_agent::AgentTurnProgressEvent::TurnFinished { .. }
            | bifrost_agent::AgentTurnProgressEvent::TurnFailed { .. }
    )
}

/// Run agent chat with `tokio::select!` interleaving.
///
/// While the agent turn is executing, this function continues to receive events
/// from the channel and routes them through `handle_busy_message` (guide/queue).
/// After the turn completes, it drains the queue by processing queued messages
/// one by one.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_chat_with_interleave(
    rx: &mut mpsc::UnboundedReceiver<ImEvent>,
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    provider_store: &Arc<ImProviderStore>,
    initial_event: &ImEvent,
    agent_client: &Arc<ImAgentClient>,
    agent_config_store: &Arc<ImAgentConfigStore>,
    agent_tools: &Arc<ImAgentToolRegistry>,
    schedule_store: &Arc<ImScheduleStore>,
    scheduler: &Arc<ImScheduler>,
    target_store: &Arc<ImTargetStore>,
    connection_manager: &Arc<ImConnectionManager>,
    agent_session_manager: &Arc<ImAgentSessionManager>,
    queue_manager: &Arc<SessionQueueManager>,
    progress_registry: &Arc<ImAgentProgressRegistry>,
    session_key: &str,
    initial_message: &str,
    initial_images: Vec<bifrost_agent::ChatImageInput>,
    system_prompt_override: Option<&str>,
    mcp_manager: &mut ImMcpManager,
    message_log_store: &Arc<ImMessageLogStore>,
    event_store: &Arc<ImEventStore>,
    external_cli_config_store: &Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
) {
    // Set up the guide channel before starting the turn
    let guide_channel = queue_manager.get_or_create_guide_channel(session_key);

    let mut current_msg = initial_message.to_string();
    let mut current_images = initial_images;

    // Queue drain loop: process initial message, then drain queued messages
    loop {
        let current_provider = provider_store
            .get(&provider.id)
            .unwrap_or_else(|| provider.clone());
        let agent_config =
            effective_agent_config_for_provider(&agent_config_store.load(), &current_provider);
        // Clone into a local so the future borrows the local, not `current_msg`.
        let msg_for_turn = current_msg.clone();
        let images_for_turn = current_images.clone();
        current_images.clear();

        if handle_idle_im_command(
            &msg_for_turn,
            session_key,
            &agent_config,
            IdleImCommandContext {
                client,
                provider: &current_provider,
                provider_store,
                external_cli_config_store,
                event: initial_event,
                message_log_store,
                agent_session_manager,
            },
        )
        .await
        {
            match queue_manager.pop_queue(session_key) {
                Some(next_msg) => {
                    current_msg = next_msg;
                    continue;
                }
                None => {
                    queue_manager.clear_session(session_key);
                    break;
                }
            }
        }

        // Run agent chat with interleaved event processing
        let chat_future = AssertUnwindSafe(process_agent_chat(
            client,
            &current_provider,
            provider_store,
            initial_event,
            agent_client,
            &agent_config,
            agent_tools,
            schedule_store,
            scheduler,
            target_store,
            connection_manager,
            agent_session_manager,
            progress_registry,
            queue_manager,
            session_key,
            &msg_for_turn,
            &images_for_turn,
            system_prompt_override,
            Some(mcp_manager),
            message_log_store,
            Some(guide_channel.clone()),
        ))
        .catch_unwind();

        // Use select! to interleave event processing with agent chat
        tokio::pin!(chat_future);
        loop {
            tokio::select! {
                result = &mut chat_future => {
                    // Chat completed (or panicked)
                    if let Err(panic_err) = result {
                        let panic_msg = panic_err
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| panic_err.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic");
                        error!(
                            session_key = %session_key,
                            panic = %panic_msg,
                            "process_agent_chat panicked, event loop continues"
                        );
                        agent_session_manager.release_active(session_key);
                        send_agent_reply(
                            client,
                            provider,
                            initial_event,
                            &format!("Agent 内部错误 (panic): {}", truncate_str(panic_msg, 200)),
                            message_log_store,
                        )
                        .await;
                    }
                    break;
                }
                Some(event) = rx.recv() => {
                    // Handle concurrent event while chat is running
                    handle_concurrent_event_during_chat(
                        &event,
                        &current_provider,
                        session_key,
                        queue_manager,
                        client,
                        message_log_store,
                        agent_session_manager,
                        progress_registry,
                        agent_config_store,
                        provider_store,
                        event_store,
                        external_cli_config_store,
                        BusyMessageDefaultMode::Guide,
                    )
                    .await;
                }
            }
        }

        drain_ready_events_after_turn(
            rx,
            &current_provider,
            session_key,
            ReadyEventDrainContext {
                queue_manager,
                client,
                message_log_store,
                agent_session_manager,
                progress_registry,
                agent_config_store,
                provider_store,
                event_store,
                external_cli_config_store,
            },
        )
        .await;

        // After turn completes, drain any unconsumed guide message into the queue
        // so it's not lost when clear_session removes the guide slot.
        // This handles the race where a guide message arrives after the session's
        // turn-end checkpoint but before the select! loop breaks.
        let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
        if let Some(unconsumed) = bifrost_agent::session::combine_guide_messages(unconsumed_guides)
        {
            if !unconsumed.trim().is_empty() {
                info!(
                    session_key = %session_key,
                    guide_msg_len = unconsumed.len(),
                    "draining unconsumed guide message into queue after turn"
                );
                let _ = queue_manager.push_queue(session_key, unconsumed);
            }
        }

        // After turn completes, check for queued messages.
        // Pop the next message from queue and process it as a new turn.
        // The session layer also supports `pending_messages` for inline queue
        // drain (used by `/agent/chat` API for testing), but in the IM event loop
        // we process one message per outer iteration to maintain interleave support.
        match queue_manager.pop_queue(session_key) {
            Some(next_msg) => {
                let remaining = queue_manager.queue_status(session_key).len();
                info!(
                    session_key = %session_key,
                    queued_msg_len = next_msg.len(),
                    remaining_queue = remaining,
                    "processing next queued message"
                );
                current_msg = next_msg;
            }
            None => {
                queue_manager.clear_session(session_key);
                break;
            }
        }
    }
}

struct ReadyEventDrainContext<'a> {
    queue_manager: &'a Arc<SessionQueueManager>,
    client: &'a ImProviderClient,
    message_log_store: &'a Arc<ImMessageLogStore>,
    agent_session_manager: &'a Arc<ImAgentSessionManager>,
    progress_registry: &'a Arc<ImAgentProgressRegistry>,
    agent_config_store: &'a Arc<ImAgentConfigStore>,
    provider_store: &'a Arc<ImProviderStore>,
    event_store: &'a Arc<ImEventStore>,
    external_cli_config_store: &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
}

async fn drain_ready_events_after_turn(
    rx: &mut mpsc::UnboundedReceiver<ImEvent>,
    provider: &ImProviderConfig,
    active_session_key: &str,
    ctx: ReadyEventDrainContext<'_>,
) {
    let mut drained_count = 0usize;
    while drained_count < 64 {
        let Ok(event) = rx.try_recv() else {
            break;
        };
        drained_count += 1;
        handle_concurrent_event_during_chat(
            &event,
            provider,
            active_session_key,
            ctx.queue_manager,
            ctx.client,
            ctx.message_log_store,
            ctx.agent_session_manager,
            ctx.progress_registry,
            ctx.agent_config_store,
            ctx.provider_store,
            ctx.event_store,
            ctx.external_cli_config_store,
            BusyMessageDefaultMode::Guide,
        )
        .await;
    }
    if drained_count > 0 {
        info!(
            session_key = %active_session_key,
            drained_count,
            "drained ready IM events after agent turn completion"
        );
    }
}

/// Handle an event that arrives during an active agent chat.
///
/// Performs the same security/logging/routing as the main loop, but for events
/// that come in while a chat is being processed. Messages for the active session
/// are routed through guide/queue mode.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_concurrent_event_during_chat(
    event: &ImEvent,
    provider: &ImProviderConfig,
    active_session_key: &str,
    queue_manager: &Arc<SessionQueueManager>,
    client: &ImProviderClient,
    message_log_store: &Arc<ImMessageLogStore>,
    agent_session_manager: &Arc<ImAgentSessionManager>,
    progress_registry: &Arc<ImAgentProgressRegistry>,
    agent_config_store: &Arc<ImAgentConfigStore>,
    provider_store: &Arc<ImProviderStore>,
    event_store: &Arc<ImEventStore>,
    external_cli_config_store: &Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    active_session_default_mode: BusyMessageDefaultMode,
) {
    let provider = provider_store
        .get(&event.provider_id)
        .unwrap_or_else(|| provider.clone());

    if !provider.enabled {
        debug!(
            event_id = %event.event_id,
            provider_id = %event.provider_id,
            "dropping concurrent event because provider is disabled"
        );
        return;
    }

    // Feishu preserves the owner boundary; Weixin replies to the sender/chat.
    if provider.provider_type == ImProviderType::Feishu {
        if let Some(ref owner_id) = provider.owner_open_id {
            let sender_id = event.source.user_id.as_deref().unwrap_or("");
            if sender_id != owner_id {
                debug!(
                    event_id = %event.event_id,
                    "rejecting concurrent event from non-owner"
                );
                return;
            }
        }
    }

    info!(
        provider_id = %event.provider_id,
        event_id = %event.event_id,
        event_type = %event.event_type,
        message_type = ?event.message.as_ref().and_then(|m| m.raw_type.as_deref()),
        message_text_len = event.message.as_ref().map(|m| m.text.len()).unwrap_or(0),
        image_count = event.message.as_ref().map(|m| m.images.len()).unwrap_or(0),
        has_text = event.message.as_ref().is_some_and(|m| !m.text.trim().is_empty()),
        "received concurrent inbound event during active agent chat"
    );
    if let Err(e) = event_store.add(event.clone()) {
        error!(error = %e, "failed to store concurrent event");
    }

    let message = match event.message.as_ref() {
        Some(m) if !m.text.trim().is_empty() || !m.images.is_empty() => m,
        _ => return,
    };
    let msg_text = agent_message_text(message);

    let session_key = build_session_key(&event.provider_id, event.source.user_id.as_deref());

    // Check if this event is for the currently active session
    if session_key == active_session_key {
        // Session-free commands are still instant
        let agent_config =
            effective_agent_config_for_provider(&agent_config_store.load(), &provider);
        if let Some(response) =
            bifrost_agent::handle_session_free_command(&session_key, &msg_text, &agent_config)
        {
            let response = if msg_text.trim() == "/help" {
                append_im_channel_help(response)
            } else {
                response
            };
            send_agent_reply(client, &provider, event, &response, message_log_store).await;
            return;
        }
        // Route through guide/queue mode
        handle_busy_message(
            &msg_text,
            &session_key,
            BusyMessageContext {
                queue_manager,
                client,
                provider: &provider,
                event,
                message_log_store,
                agent_session_manager,
                progress_registry,
                external_cli_config_store,
                default_mode: active_session_default_mode,
                status_context: status_context_from_agent_config(&agent_config),
                default_work_dir: Some(agent_config.resolve_work_dir().display().to_string()),
            },
        )
        .await;
    } else {
        // Different session — check if it's also busy
        if agent_session_manager.is_session_active(&session_key) {
            let agent_config =
                effective_agent_config_for_provider(&agent_config_store.load(), &provider);
            handle_busy_message(
                &msg_text,
                &session_key,
                BusyMessageContext {
                    queue_manager,
                    client,
                    provider: &provider,
                    event,
                    message_log_store,
                    agent_session_manager,
                    progress_registry,
                    external_cli_config_store,
                    default_mode: busy_default_mode_for_agent_config(&agent_config),
                    status_context: status_context_from_agent_config(&agent_config),
                    default_work_dir: Some(agent_config.resolve_work_dir().display().to_string()),
                },
            )
            .await;
        } else {
            // Session is free but we can't process it now (MCP is in use).
            // Queue it for later processing.
            let _ = queue_manager.push_queue(&session_key, msg_text);
            send_agent_reply(
                client,
                &provider,
                event,
                "⏳ 消息已排队，将在当前任务完成后处理。",
                message_log_store,
            )
            .await;
        }
    }
}

async fn wait_for_guide_notification(guide_channel: Option<&bifrost_agent::session::GuideChannel>) {
    if let Some(channel) = guide_channel {
        channel.notified().await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn forward_pending_guides_to_worker(
    guide_channel: Option<&bifrost_agent::session::GuideChannel>,
    queue_manager: &SessionQueueManager,
    session_key: &str,
    worker: &mut crate::im_gateway::agent_worker::AgentWorkerRun,
) {
    let Some(channel) = guide_channel else {
        return;
    };
    let guides = channel.drain();
    let Some(guide) = bifrost_agent::session::combine_guide_messages(guides) else {
        return;
    };
    if guide.trim().is_empty() {
        return;
    }

    queue_manager.mark_guides_handed_to_worker(session_key, std::slice::from_ref(&guide));
    if let Err(error) = worker.send_guide(guide).await {
        warn!(
            session_key = %session_key,
            error = %error,
            "failed to forward guide message to agent worker"
        );
    }
}

/// Process an agent chat: run the full turn loop (with tool calls), send reply via Feishu, log the outbound message.
#[allow(clippy::too_many_arguments)]
pub(super) async fn process_agent_chat(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    provider_store: &Arc<ImProviderStore>,
    event: &ImEvent,
    _agent_client: &Arc<ImAgentClient>,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    _agent_tools: &Arc<ImAgentToolRegistry>,
    _schedule_store: &Arc<ImScheduleStore>,
    _scheduler: &Arc<ImScheduler>,
    _target_store: &Arc<ImTargetStore>,
    _connection_manager: &Arc<ImConnectionManager>,
    session_manager: &Arc<ImAgentSessionManager>,
    progress_registry: &Arc<ImAgentProgressRegistry>,
    queue_manager: &Arc<SessionQueueManager>,
    session_key: &str,
    user_message: &str,
    images: &[bifrost_agent::ChatImageInput],
    system_prompt_override: Option<&str>,
    _mcp: Option<&mut ImMcpManager>,
    message_log_store: &Arc<ImMessageLogStore>,
    guide_channel: Option<bifrost_agent::session::GuideChannel>,
) {
    info!(
        session_key = %session_key,
        user_message_len = user_message.len(),
        "invoking agent chat (turn loop)"
    );
    if handle_im_cwd_command(
        user_message,
        session_key,
        ImCwdCommandContext {
            client,
            provider,
            provider_store,
            event,
            message_log_store,
            session_manager,
        },
    )
    .await
    {
        return;
    }

    let slash_mode = crate::im_gateway::agent_slash::parse_agent_slash_mode(user_message);
    let collaboration_mode = slash_mode.collaboration_mode;
    let user_message = slash_mode.message;
    if user_message.trim().is_empty() && images.is_empty() {
        send_agent_reply(
            client,
            provider,
            event,
            "请在 /plan 后输入需要规划的内容。",
            message_log_store,
        )
        .await;
        return;
    }

    // ── Session-free command fast path ────────────────────────────────────
    // Commands like /help, /remember, /memories, /forget don't need session
    // state and can respond immediately even while a turn loop is running.
    if let Some(response) =
        bifrost_agent::handle_session_free_command(session_key, &user_message, agent_config)
    {
        let response = if user_message.trim() == "/help" {
            append_im_channel_help(response)
        } else {
            response
        };
        debug!(
            session_key = %session_key,
            "handled session-free command without taking session"
        );
        send_agent_reply(client, provider, event, &response, message_log_store).await;
        return;
    }

    // ── Busy check ───────────────────────────────────────────────────────
    // If another turn loop is already processing this session, reject early
    // instead of creating a duplicate empty session.
    let resolved_work_dir = agent_config.resolve_work_dir().display().to_string();
    let mut session = match session_manager
        .try_take_session_with_work_dir(session_key, agent_config.work_dir.clone())
    {
        Some(s) => s,
        None => {
            info!(
                session_key = %session_key,
                "session is busy, rejecting concurrent request"
            );
            let busy_msg =
                "⏳ Agent 正在处理中，请稍后再试。\n\n提示: /stop 可立即停止当前 loop；/help、/remember、/memories、/forget 等命令即使在处理中也可立即响应。";
            send_agent_reply(client, provider, event, busy_msg, message_log_store).await;
            return;
        }
    };
    restore_session_from_persisted_history(
        &mut session,
        session_key,
        crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
        None,
        agent_config.history.as_ref().and_then(|h| h.max_bytes),
    );
    if session.history.is_empty() {
        if let Some(work_dir) = agent_config
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if session.work_dir.as_deref() != Some(work_dir) {
                session.reinitialize_work_dir(work_dir.to_string());
            }
        } else if session.work_dir.is_none() {
            session.reinitialize_work_dir(resolved_work_dir.clone());
        }
    } else if session.work_dir.is_none() {
        session.work_dir = Some(resolved_work_dir.clone());
    }
    session.source = format!("{:?}", provider.provider_type).to_lowercase();
    session.mark_bifrost_agent_runtime();
    session.guide_channel = guide_channel.clone();

    let target_open_id = agent_reply_target_id(provider, event).unwrap_or_default();
    let mut progress_enabled = false;
    let mut progress_tx_for_finish = None;
    let mut progress_task = None;
    if !target_open_id.is_empty() {
        let progress_target = crate::im_gateway::types::ImTarget {
            id: "__agent_progress__".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Agent Progress".to_string(),
            enabled: true,
            receive_id_type: "open_id".to_string(),
            receive_id: target_open_id.to_string(),
            default_msg_type: "interactive".to_string(),
            created_at: 0,
            updated_at: 0,
        };
        if let Some(feishu) = client.feishu() {
            let progress_result = if progress_registry
                .rollover_existing(session_key, &user_message)
                .await
            {
                Ok(())
            } else {
                progress_registry
                    .start_feishu(
                        session_key,
                        feishu,
                        provider.clone(),
                        progress_target,
                        &user_message,
                    )
                    .await
                    .map(|_| ())
            };
            match progress_result {
                Ok(_) => {
                    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<
                        bifrost_agent::AgentTurnProgressEvent,
                    >();
                    session.progress_sender = Some(progress_tx.clone());
                    progress_tx_for_finish = Some(progress_tx);
                    let progress_registry = Arc::clone(progress_registry);
                    let session_key_for_progress = session_key.to_string();
                    progress_task = Some(tokio::spawn(async move {
                        run_progress_event_coalescer(
                            progress_registry,
                            session_key_for_progress,
                            &mut progress_rx,
                        )
                        .await;
                    }));
                    progress_enabled = true;
                }
                Err(error) => {
                    warn!(
                        session_key = %session_key,
                        error = %error,
                        "failed to start IM streaming progress card; falling back to final reply card"
                    );
                }
            }
        }
    }

    if should_send_plain_im_task_start_notice(provider, progress_enabled) {
        send_agent_reply(client, provider, event, "已开始处理。", message_log_store).await;
    }

    // Set up plan update channel for real-time plan card rendering.
    // The turn loop pushes plan steps through this channel; a background task
    // sends (first time) or patches (subsequent) a single Feishu card.
    if !progress_enabled && client.feishu().is_some() {
        let (plan_tx, mut plan_rx) =
            tokio::sync::mpsc::unbounded_channel::<(Vec<bifrost_agent::PlanStep>, Option<String>)>(
            );
        session.plan_sender = Some(plan_tx);
        let feishu = client.feishu().expect("checked above");
        let provider = provider.clone();
        let target_open_id = provider
            .owner_open_id
            .as_deref()
            .or(event.source.user_id.as_deref())
            .unwrap_or("")
            .to_string();
        tokio::spawn(async move {
            let mut plan_card_msg_id: Option<String> = None;
            while let Some((steps, title)) = plan_rx.recv().await {
                let card = build_plan_card(&steps, title.as_deref());
                if let Some(ref msg_id) = plan_card_msg_id {
                    // Patch existing card
                    if let Err(e) = feishu.patch_card(&provider, msg_id, card).await {
                        tracing::warn!(error = %e, "failed to patch plan card");
                    }
                } else if !target_open_id.is_empty() {
                    // Send new card
                    let target = crate::im_gateway::types::ImTarget {
                        id: "__plan_card__".to_string(),
                        provider_id: provider.id.clone(),
                        display_name: "Plan Card".to_string(),
                        enabled: true,
                        receive_id_type: "open_id".to_string(),
                        receive_id: target_open_id.clone(),
                        default_msg_type: "interactive".to_string(),
                        created_at: 0,
                        updated_at: 0,
                    };
                    match feishu
                        .send_card(
                            &provider,
                            &target,
                            card,
                            crate::im_gateway::types::SendOptions::default(),
                        )
                        .await
                    {
                        Ok(r) => {
                            plan_card_msg_id = r.message_id;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to send plan card");
                        }
                    }
                }
            }
        });
    }

    let message_channel =
        crate::im_gateway::send_msg_tool::message_channel_from_event(provider, event)
            .or_else(|| agent_config.default_message_channel.clone());
    let history_path = session
        .recorder
        .as_ref()
        .map(|recorder| recorder.file_path().display().to_string());
    remember_session_turn_started(
        session_key,
        crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
        None,
        &user_message,
        history_path.clone(),
        session.work_dir.clone(),
    );

    let mut worker_request = crate::im_gateway::agent_worker::build_run_request(
        session_key.to_string(),
        user_message.clone(),
        images.to_vec(),
        agent_config,
        session.work_dir.clone(),
        history_path,
        Some(format!("{:?}", provider.provider_type).to_lowercase()),
    );
    worker_request.system_prompt = system_prompt_override.map(ToString::to_string);
    worker_request.collaboration_mode = collaboration_mode;
    worker_request.default_message_channel = message_channel;
    let mut result: Result<bifrost_agent::TurnResult, String> =
        Err("agent worker exited without result".to_string());
    let mut worker_history_path: Option<String> = None;
    let mut stop_ack = None;

    match crate::im_gateway::agent_worker::AgentWorkerClient::spawn_or_fallback(worker_request)
        .await
    {
        Ok(mut worker) => {
            let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel::<
                crate::im_gateway::agent_worker::AgentWorkerStopRequest,
            >();
            // pid=0 signals in-process worker (test-only fallback; no external process to kill).
            let worker_pid = worker.child_id().unwrap_or(0);
            crate::im_gateway::agent_worker::register_active_worker(
                session_key,
                worker_pid,
                stop_tx,
            );

            loop {
                forward_pending_guides_to_worker(
                    guide_channel.as_ref(),
                    queue_manager,
                    session_key,
                    &mut worker,
                )
                .await;
                tokio::select! {
                    _ = wait_for_guide_notification(guide_channel.as_ref()) => {
                        continue;
                    }
                    maybe_stop = stop_rx.recv() => {
                        let _ = worker.terminate().await;
                        stop_ack = maybe_stop;
                        result = Err("已收到 /stop，Agent worker 子进程已停止。".to_string());
                        break;
                    }
                    event = worker.next_event() => {
                        match event {
                            Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Started { .. })) => {}
                            Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Progress { event })) => {
                                if let bifrost_agent::AgentTurnProgressEvent::TitleUpdated { title } = &event {
                                    session.title = Some(title.clone());
                                    session_manager.update_active_session_preview(
                                        session_key,
                                        Some(title.clone()),
                                        None,
                                        None,
                                        None,
                                        None,
                                    );
                                }
                                if let bifrost_agent::AgentTurnProgressEvent::Status(status) = &event {
                                    session_manager.update_active_turn_status_from_worker((**status).clone());
                                }
                                if progress_enabled {
                                    if let Some(tx) = progress_tx_for_finish.as_ref() {
                                        let _ = tx.send(event);
                                    }
                                } else if let Some(tx) = session.plan_sender.as_ref() {
                                    if let bifrost_agent::AgentTurnProgressEvent::PlanUpdated { steps, title } = event {
                                        let _ = tx.send((steps, title));
                                    }
                                }
                            }
                            Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Finished { result: turn_result })) => {
                                worker_history_path = turn_result.history_path.clone();
                                result = Ok(bifrost_agent::TurnResult::from(turn_result));
                                break;
                            }
                            Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Failed { error })) => {
                                result = Err(error);
                                break;
                            }
                            Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Stopped)) => {
                                result = Err("已收到 /stop，Agent worker 子进程已停止。".to_string());
                                break;
                            }
                            Ok(None) => break,
                            Err(error) => {
                                result = Err(format!("agent worker failed: {error}"));
                                break;
                            }
                        }
                    }
                }
            }
            crate::im_gateway::agent_worker::clear_active_worker(session_key);
        }
        Err(error) => {
            result = Err(format!("Agent worker 启动失败: {error}"));
        }
    }

    if let Some(history_path) = worker_history_path.as_deref() {
        let _ = restore_session_from_history_path(
            &mut session,
            Path::new(history_path),
            session_key,
            agent_config.history.as_ref().and_then(|h| h.max_bytes),
        );
    }
    let session_title = session.title.clone();
    let reply_image_base_dir = session
        .work_dir
        .as_deref()
        .or(agent_config.work_dir.as_deref())
        .map(PathBuf::from);
    session.progress_sender = None;
    session.plan_sender = None;
    remember_session_state_from_agent_session(
        &session,
        crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
        None,
    );
    session_manager.return_session(session);
    session_manager.cleanup_expired();
    if let Some(stop_request) = stop_ack {
        stop_request.ack();
    }

    // Separate main response and tool calls for card rendering
    let mut progress_failed = false;
    let (main_response, tool_calls_panel, plan_steps) = match result {
        Ok(turn_result) => {
            let mut response = turn_result.response;
            // Log work_dir switch if it happened
            if let Some(ref new_dir) = turn_result.work_dir_switched {
                info!(
                    session_key = %session_key,
                    new_work_dir = %new_dir,
                    "session work directory switched via agent tool"
                );
                persist_provider_agent_work_dir(provider_store, &provider.id, new_dir);
                response.push_str(&format!("\n\n当前工作路径: `{new_dir}`"));
            }
            // Build tool calls info for collapsible panel
            let panel = if !turn_result.tool_calls_log.is_empty() {
                let mut tool_md = String::new();
                for log in &turn_result.tool_calls_log {
                    let icon = if log.success { "✅" } else { "❌" };
                    tool_md.push_str(&format!("{} `{}`\n", icon, log.tool_name));
                    let result_preview = truncate_bytes_with_suffix(&log.result, 500, "...");
                    tool_md.push_str(&format!("```\n{}\n```\n", result_preview));
                }
                Some((turn_result.tool_calls_log.len(), tool_md))
            } else {
                None
            };
            let plan = turn_result.plan_steps;
            (response, panel, plan)
        }
        Err(e) => {
            progress_failed = true;
            error!(
                session_key = %session_key,
                error = %e,
                "agent chat failed after retry"
            );
            (
                format!(
                    "⚠️ **Agent 执行失败**\n\n**错误原因**: {}\n\n请稍后重试，或发送 `/clear` 重置会话。",
                    truncate_str(&e, 300)
                ),
                None,
                None,
            )
        }
    };

    let (main_response_for_card, reply_images, reply_attachments) =
        prepare_agent_reply_text_and_images_with_downloads(
            &main_response,
            reply_image_base_dir.as_deref(),
        )
        .await;
    let rendered_main_response = if let Some(feishu) = client.feishu() {
        render_agent_markdown_for_feishu(
            &feishu,
            provider,
            &main_response_for_card,
            reply_image_base_dir.as_deref(),
        )
        .await
    } else {
        main_response_for_card.clone()
    };

    if progress_enabled {
        if let Some(tx) = progress_tx_for_finish.take() {
            let event = if progress_failed {
                bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                    error: rendered_main_response.clone(),
                }
            } else {
                bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                    content: rendered_main_response.clone(),
                }
            };
            let _ = tx.send(event);
            drop(tx);
        }
        if let Some(task) = progress_task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
        let progress_message_info = progress_registry
            .finish(
                session_key,
                Some(rendered_main_response.clone()),
                progress_failed,
            )
            .await;

        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status: MessageStatus::Success,
            timestamp: now_ms(),
            target_id: Some(
                progress_message_info
                    .as_ref()
                    .map(|info| format!("__agent_progress__:{}", info.card_id))
                    .unwrap_or_else(|| "__agent_progress__".to_string()),
            ),
            target_name: Some("Agent Progress".to_string()),
            message_id: progress_message_info.and_then(|info| info.message_id),
            msg_type: Some("interactive".to_string()),
            content_preview: Some(truncate_str(&main_response_for_card, 200)),
            trigger: Some("agent_streaming".to_string()),
            error: None,
            sender_open_id: None,
            event_id: Some(event.event_id.clone()),
            reaction_added: None,
        };
        if let Err(e) = message_log_store.add(log) {
            error!(error = %e, "failed to store agent streaming outbound message log");
        }
        send_agent_reply_images_for_event(
            client,
            provider,
            event,
            &reply_images,
            &reply_attachments,
            message_log_store,
        )
        .await;
        return;
    }

    let Some(reply_target) = build_agent_reply_target(
        provider,
        event,
        "__agent_reply__",
        "Agent Reply",
        "interactive",
    ) else {
        error!("no target open_id to send agent reply");
        return;
    };

    // Build Feishu Card JSON 2.0: main response visible, tool calls in collapsible panel
    let main_response =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&rendered_main_response);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": main_response,
        "element_id": "agent_reply"
    })];
    // Plan progress panel (between response and tool calls)
    if let Some(ref steps) = plan_steps {
        let completed = steps
            .iter()
            .filter(|s| matches!(s.status, bifrost_agent::PlanStepStatus::Completed))
            .count();
        let total = steps.len();
        let mut plan_md = String::new();
        for s in steps {
            plan_md.push_str(&format!("{} {}\n", s.status.emoji(), s.step));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": true,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("📋 任务计划（{}/{}）", completed, total)
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": plan_md
            }]
        }));
    }
    if let Some((count, ref tool_md)) = tool_calls_panel {
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": false,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("🔧 工具调用记录（{}次）", count)
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": tool_md
            }]
        }));
    }
    let rich_card_title = session_title.as_deref().unwrap_or("Bifrost AI");
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": rich_card_title
            }
        },
        "body": {
            "elements": elements
        }
    });

    let send_result = client
        .send_card(
            provider,
            &reply_target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

    // Record outbound message log
    let (status, message_id, error_msg) = match &send_result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(reply_target.receive_id.clone()),
        target_name: Some(reply_target.display_name.clone()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(&main_response_for_card, 200)),
        trigger: Some("agent".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => info!(session_key = %session_key, "agent reply sent successfully"),
        Err(e) => error!(session_key = %session_key, error = %e, "failed to send agent reply"),
    }

    send_agent_reply_assets(
        client,
        provider,
        event,
        &reply_target,
        &reply_images,
        &reply_attachments,
        message_log_store,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ProgressCardMockCounters {
        recall: Arc<AtomicUsize>,
        card_update: Arc<AtomicUsize>,
        settings_update: Arc<AtomicUsize>,
    }

    async fn spawn_progress_card_mock_server() -> (String, ProgressCardMockCounters) {
        let recall_counter = Arc::new(AtomicUsize::new(0));
        let card_update_counter = Arc::new(AtomicUsize::new(0));
        let settings_update_counter = Arc::new(AtomicUsize::new(0));
        let card_counter = Arc::new(AtomicUsize::new(0));
        let message_counter = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock feishu server");
        let port = listener.local_addr().expect("mock local addr").port();
        let recall_counter_for_server = Arc::clone(&recall_counter);
        let card_update_counter_for_server = Arc::clone(&card_update_counter);
        let settings_update_counter_for_server = Arc::clone(&settings_update_counter);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let recall_counter = Arc::clone(&recall_counter_for_server);
                let card_update_counter = Arc::clone(&card_update_counter_for_server);
                let settings_update_counter = Arc::clone(&settings_update_counter_for_server);
                let card_counter = Arc::clone(&card_counter);
                let message_counter = Arc::clone(&message_counter);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let recall_counter = Arc::clone(&recall_counter);
                        let card_update_counter = Arc::clone(&card_update_counter);
                        let settings_update_counter = Arc::clone(&settings_update_counter);
                        let card_counter = Arc::clone(&card_counter);
                        let message_counter = Arc::clone(&message_counter);
                        async move {
                            let method = req.method().clone();
                            let path = req.uri().path().to_string();
                            let _body = req
                                .into_body()
                                .collect()
                                .await
                                .expect("collect request body")
                                .to_bytes();
                            if method == Method::POST
                                && path == "/open-apis/auth/v3/tenant_access_token/internal"
                            {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(
                                            br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            if method == Method::POST && path == "/open-apis/cardkit/v1/cards" {
                                let idx = card_counter.fetch_add(1, Ordering::SeqCst) + 1;
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from(format!(
                                            r#"{{"code":0,"data":{{"card_id":"card_{idx}"}}}}"#
                                        ))))
                                        .unwrap(),
                                );
                            }
                            if method == Method::PUT
                                && path.starts_with("/open-apis/cardkit/v1/cards/")
                                && !path.contains("/elements/")
                            {
                                card_update_counter.fetch_add(1, Ordering::SeqCst);
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                        .unwrap(),
                                );
                            }
                            if method == Method::PATCH
                                && path.starts_with("/open-apis/cardkit/v1/cards/")
                                && path.ends_with("/settings")
                            {
                                settings_update_counter.fetch_add(1, Ordering::SeqCst);
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                        .unwrap(),
                                );
                            }
                            if method == Method::POST && path == "/open-apis/im/v1/messages" {
                                let idx = message_counter.fetch_add(1, Ordering::SeqCst) + 1;
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from(format!(
                                            r#"{{"code":0,"data":{{"message_id":"om_{idx}"}}}}"#
                                        ))))
                                        .unwrap(),
                                );
                            }
                            if method == Method::DELETE
                                && path.starts_with("/open-apis/im/v1/messages/")
                            {
                                recall_counter.fetch_add(1, Ordering::SeqCst);
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                        .unwrap(),
                                );
                            }
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Full::new(Bytes::from_static(b"{}")))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });
        (
            format!("http://127.0.0.1:{port}/open-apis"),
            ProgressCardMockCounters {
                recall: recall_counter,
                card_update: card_update_counter,
                settings_update: settings_update_counter,
            },
        )
    }

    fn mock_feishu_provider(base_url: String) -> ImProviderConfig {
        ImProviderConfig {
            id: "feishu-main".to_string(),
            provider_type: crate::im_gateway::types::ImProviderType::Feishu,
            display_name: "Feishu Main".to_string(),
            enabled: true,
            base_url: Some(base_url),
            app_id: Some("cli_xxx".to_string()),
            secret_ref: Some("secret".to_string()),
            owner_open_id: Some("ou_owner".to_string()),
            event_connection_enabled: true,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn mock_progress_target() -> ImTarget {
        ImTarget {
            id: "progress".to_string(),
            provider_id: "feishu-main".to_string(),
            display_name: "Progress".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou_owner".to_string(),
            default_msg_type: "interactive".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn mock_owner_text_event(event_id: &str, text: &str) -> ImEvent {
        ImEvent {
            event_id: event_id.to_string(),
            provider_id: "feishu-main".to_string(),
            provider_type: crate::im_gateway::types::ImProviderType::Feishu,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: None,
                user_id: Some("ou_owner".to_string()),
                message_id: Some(format!("om_{event_id}")),
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: text.to_string(),
                mentions: Vec::new(),
                images: Vec::new(),
                raw_type: Some("text".to_string()),
            }),
            received_at: now_ms(),
            raw_digest: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drain_ready_events_after_turn_preserves_late_message_as_guide() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let (base_url, counters) = spawn_progress_card_mock_server().await;
        let feishu = Arc::new(crate::im_gateway::feishu::FeishuProvider::new());
        let provider = mock_feishu_provider(base_url);
        let client = ImProviderClient::Feishu(Arc::clone(&feishu));
        let queue_manager = Arc::new(SessionQueueManager::new());
        let progress_registry = Arc::new(ImAgentProgressRegistry::new());
        progress_registry
            .start_feishu(
                "feishu-main:ou_owner",
                Arc::clone(&feishu),
                provider.clone(),
                mock_progress_target(),
                "active turn",
            )
            .await
            .expect("start progress card");

        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(mock_owner_text_event(
            "late_1",
            "late message at final boundary",
        ))
        .expect("send late event");

        let provider_store = Arc::new(ImProviderStore::new(temp_dir.path()));
        let event_store = Arc::new(ImEventStore::new(temp_dir.path()));
        let message_log_store = Arc::new(ImMessageLogStore::new(temp_dir.path()));
        let agent_session_manager = Arc::new(ImAgentSessionManager::new(3600));
        let agent_config_store = Arc::new(ImAgentConfigStore::new(&temp_dir.path().join("agent")));
        let external_cli_config_store =
            Arc::new(crate::im_gateway::external_cli::ExternalCliConfigStore::new(temp_dir.path()));

        drain_ready_events_after_turn(
            &mut rx,
            &provider,
            "feishu-main:ou_owner",
            ReadyEventDrainContext {
                queue_manager: &queue_manager,
                client: &client,
                message_log_store: &message_log_store,
                agent_session_manager: &agent_session_manager,
                progress_registry: &progress_registry,
                agent_config_store: &agent_config_store,
                provider_store: &provider_store,
                event_store: &event_store,
                external_cli_config_store: &external_cli_config_store,
            },
        )
        .await;

        let guides = queue_manager.guide_status("feishu-main:ou_owner");
        assert_eq!(guides, vec!["late message at final boundary".to_string()]);
        let drained: Vec<String> = queue_manager
            .get_or_create_guide_channel("feishu-main:ou_owner")
            .lock()
            .unwrap()
            .drain(..)
            .collect();
        let combined =
            bifrost_agent::session::combine_guide_messages(drained).expect("combined guide");
        queue_manager
            .push_queue("feishu-main:ou_owner", combined)
            .expect("push combined guide into queue");
        assert_eq!(
            queue_manager.pop_queue("feishu-main:ou_owner").as_deref(),
            Some("late message at final boundary")
        );
        assert_eq!(counters.card_update.load(Ordering::SeqCst), 1);
        assert_eq!(counters.settings_update.load(Ordering::SeqCst), 1);
        assert_eq!(counters.recall.load(Ordering::SeqCst), 0);
    }
}
