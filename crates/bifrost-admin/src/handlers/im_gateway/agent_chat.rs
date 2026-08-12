use super::*;

// ---------------------------------------------------------------------------

pub(super) struct IdleImCommandContext<'a> {
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) provider_store: &'a Arc<ImProviderStore>,
    pub(super) group_context_store: &'a Arc<ImGroupContextStore>,
    pub(super) external_cli_config_store:
        &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub(super) event: &'a ImEvent,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) agent_session_manager: &'a Arc<ImAgentSessionManager>,
    pub(super) queue_manager: &'a Arc<SessionQueueManager>,
}

pub(super) fn parse_im_new_group_command(message: &str) -> Option<Result<String, String>> {
    let trimmed = message.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    if parts.next()? != "/new" {
        return None;
    }
    let name = parts.next().unwrap_or_default().trim();
    if name.is_empty() {
        return Some(Err("用法: /new <群名>".to_string()));
    }
    if name.chars().count() > 60 {
        return Some(Err("群名不能超过 60 个字符。".to_string()));
    }
    Some(Ok(name.to_string()))
}

pub(super) async fn handle_im_new_group_command(
    message: &str,
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    group_context_store: &ImGroupContextStore,
    message_log_store: &Arc<ImMessageLogStore>,
) -> bool {
    if provider.provider_type != ImProviderType::Feishu {
        return false;
    }
    let Some(parsed) = parse_im_new_group_command(message) else {
        return false;
    };
    let group_name = match parsed {
        Ok(name) => name,
        Err(error) => {
            send_agent_reply(client, provider, event, &error, message_log_store).await;
            return true;
        }
    };
    let Some(configured_owner) = provider
        .owner_open_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        send_agent_reply(
            client,
            provider,
            event,
            "当前飞书 Provider 未配置 owner，已拒绝创建群。",
            message_log_store,
        )
        .await;
        return true;
    };
    let Some(sender_open_id) = event
        .source
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        send_agent_reply(
            client,
            provider,
            event,
            "无法识别命令发送者，已拒绝创建群。",
            message_log_store,
        )
        .await;
        return true;
    };
    if sender_open_id != configured_owner {
        send_agent_reply(
            client,
            provider,
            event,
            "只有当前飞书 Provider 的 owner 可以使用 /new 创建群。",
            message_log_store,
        )
        .await;
        return true;
    }
    let source_message_id = event
        .source
        .message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(event.event_id.as_str());
    match group_context_store.created_feishu_group(&provider.id, source_message_id) {
        Ok(Some(record)) => {
            send_agent_reply(
                client,
                provider,
                event,
                &format!(
                    "群已创建：{}\n群 ID：{}\n（本次消息已处理，未重复创建）",
                    record.group_name, record.chat_id
                ),
                message_log_store,
            )
            .await;
            return true;
        }
        Ok(None) => {}
        Err(error) => {
            send_agent_reply(
                client,
                provider,
                event,
                &format!("读取建群幂等记录失败：{error}"),
                message_log_store,
            )
            .await;
            return true;
        }
    }
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(format!("{}:{source_message_id}", provider.id));
    let uuid = format!("bifrost-{}", &format!("{digest:x}")[..32]);
    let created = match client
        .create_feishu_group_chat(provider, &group_name, sender_open_id, &uuid)
        .await
    {
        Ok(created) => created,
        Err(error) => {
            send_agent_reply(
                client,
                provider,
                event,
                &format!("创建飞书群失败：{error}"),
                message_log_store,
            )
            .await;
            return true;
        }
    };
    let record = crate::im_gateway::group_context::CreatedFeishuGroupRecord {
        provider_id: provider.id.clone(),
        source_message_id: source_message_id.to_string(),
        group_name: created.name.clone(),
        chat_id: created.chat_id.clone(),
        owner_open_id: sender_open_id.to_string(),
        created_at: now_ms(),
    };
    if let Err(error) = group_context_store.save_created_feishu_group(&record) {
        send_agent_reply(
            client,
            provider,
            event,
            &format!(
                "群已创建：{}（{}），但保存幂等记录失败：{}。请勿重复发送原命令。",
                created.name, created.chat_id, error
            ),
            message_log_store,
        )
        .await;
        return true;
    }
    let now = now_ms();
    let welcome_target = ImTarget {
        id: format!("new-group:{}", created.chat_id),
        provider_id: provider.id.clone(),
        display_name: created.name.clone(),
        receive_id_type: "chat_id".to_string(),
        receive_id: created.chat_id.clone(),
        default_msg_type: "interactive".to_string(),
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    let welcome_error = client
        .send_text(
            provider,
            &welcome_target,
            &format!("群「{}」已创建，Bifrost 机器人已加入。", created.name),
        )
        .await
        .err();
    let reply = match welcome_error {
        None => format!("群已创建：{}\n群 ID：{}", created.name, created.chat_id),
        Some(error) => format!(
            "群已创建：{}\n群 ID：{}\n但欢迎消息发送失败：{}",
            created.name, created.chat_id, error
        ),
    };
    send_agent_reply(client, provider, event, &reply, message_log_store).await;
    true
}

#[rustfmt::skip] pub(super) async fn handle_idle_im_command(
    msg_text: &str,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    ctx: IdleImCommandContext<'_>,
) -> bool {
    let trimmed = msg_text.trim();
    if trimmed == "/help" {
        let config = ctx.external_cli_config_store.load();
        let response = build_im_help_text_for_agent_config(
            agent_config,
            &config,
            Some(ctx.provider.id.as_str()),
            ctx.provider.provider_type,
        );
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

    if trimmed == "/status" {
        let detail = ctx.agent_session_manager.get_session_detail(session_key);
        let runner_id_override = ctx.group_context_store.runner_id_by_session(session_key).ok().flatten();
        let status_context = resolve_im_status_runtime_context(agent_config, &ctx.external_cli_config_store.load(), &ctx.provider.id, session_key, runner_id_override.as_deref());
        let default_work_dir = ctx
            .group_context_store
            .work_dir_by_session(session_key)
            .ok()
            .flatten()
            .unwrap_or_else(|| agent_config.resolve_work_dir())
            .display()
            .to_string();
        let device_name = current_device_name();
        let reply = build_im_status_text(
            detail.as_ref(),
            &status_context,
            Some(default_work_dir.as_str()),
            &ImStatusChannelContext { provider: ctx.provider, device_name: &device_name, session_key, queue_info: "无排队消息", status: "Ready" },
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

    if trimmed == "/q" {
        let reply = format_queue_status(
            "📋 当前线程排队消息",
            &ctx.queue_manager.queue_status(session_key),
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

    if trimmed == "/pwd" {
        let reply = format_effective_im_work_dir(
            ctx.group_context_store,
            session_key,
            agent_config,
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
            group_context_store: ctx.group_context_store,
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
            group_context_store: ctx.group_context_store,
            external_cli_config_store: ctx.external_cli_config_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
            session_manager: ctx.agent_session_manager,
            agent_config,
        },
    )
    .await
    {
        return true;
    }

    if handle_im_resume_command(
        trimmed,
        session_key,
        agent_config,
        ImModelCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            external_cli_config_store: ctx.external_cli_config_store,
            group_context_store: ctx.group_context_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
        },
    )
    .await
    {
        return true;
    }

    if handle_im_model_command(
        trimmed,
        session_key,
        agent_config,
        ImModelCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            external_cli_config_store: ctx.external_cli_config_store,
            group_context_store: ctx.group_context_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
        },
    )
    .await
    {
        return true;
    }

    if handle_im_effort_command(
        trimmed,
        session_key,
        agent_config,
        ImModelCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            external_cli_config_store: ctx.external_cli_config_store,
            group_context_store: ctx.group_context_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
        },
    )
    .await
    {
        return true;
    }

    if handle_im_fast_command(
        trimmed,
        session_key,
        agent_config,
        ImModelCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            external_cli_config_store: ctx.external_cli_config_store,
            group_context_store: ctx.group_context_store,
            event: ctx.event,
            message_log_store: ctx.message_log_store,
        },
    )
    .await
    {
        return true;
    }

    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImHelpRunnerKind {
    External { adapter: String },
}

impl ImHelpRunnerKind {
    pub(super) fn from_runner_type(runner_type: &str) -> Self {
        let runner_type = runner_type.trim();
        Self::External {
            adapter: runner_type.to_string(),
        }
    }
}

pub(super) fn im_help_runner_kind_for_agent_config(
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    external_cli_config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    provider_id: Option<&str>,
) -> ImHelpRunnerKind {
    let runner_id = agent_config
        .runner
        .as_ref()
        .and_then(|runner| runner.custom_runner_id());
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        external_cli_config,
        provider_id,
        runner_id,
    );
    ImHelpRunnerKind::External {
        adapter: effective.settings.adapter,
    }
}

pub(super) fn build_im_help_text_for_agent_config(
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    external_cli_config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    provider_id: Option<&str>,
    provider_type: ImProviderType,
) -> String {
    let runner_kind =
        im_help_runner_kind_for_agent_config(agent_config, external_cli_config, provider_id);
    format!(
        "可用命令:\n\n{}",
        build_im_channel_help_sections(&runner_kind, provider_type)
    )
}

pub(super) fn build_im_startup_help_for_runner(
    runner_kind: &ImHelpRunnerKind,
    provider_type: ImProviderType,
) -> String {
    format!(
        "可用命令:\n\n{}",
        build_im_channel_help_sections(runner_kind, provider_type)
    )
}

pub(super) fn build_im_channel_help_sections(
    runner_kind: &ImHelpRunnerKind,
    provider_type: ImProviderType,
) -> String {
    let mut channel_commands = "IM 通道命令（所有 Runner）:\n\
         /help           显示此帮助信息\n\
         /status         查看当前 IM 会话状态、Runner、模型和排队情况\n\
         /pwd            查看当前线程的工作目录\n\
         /cwd <绝对路径>  切换当前 IM 通道绑定的工作目录；路径必须存在且是目录，运行中会排队到当前任务结束后执行\n\
         /runner [Runner]  不带参数查看当前 Runner；带参数切换当前 IM 通道绑定的 Runner"
        .to_string();
    if provider_type == ImProviderType::Feishu {
        channel_commands.push_str(
            "\n/new <群名>      创建同名飞书私有群，将命令发送者设为群主，并自动加入当前机器人（仅 Provider owner）",
        );
    }
    channel_commands.push_str(
        "\n\
         /clear          重置当前 IM 会话上下文\n\
         /reset          重置当前 IM 会话上下文\n\
         /q [消息]       不带参数查看当前线程排队；带消息则在当前任务结束后继续处理\n\
         /rq <序号>      取消一条排队消息\n\
         /stop           停止当前正在执行的任务",
    );
    let mut sections = vec![channel_commands];

    match runner_kind {
        ImHelpRunnerKind::External { adapter } => {
            let mut runner_lines = Vec::new();
            if adapter != crate::im_gateway::chatgpt_web::ADAPTER_ID {
                runner_lines.push("普通后续消息默认按引导处理，使用 /q 才排队");
            }
            if crate::im_gateway::external_cli::supports_external_cli_model_slash(adapter) {
                runner_lines
                    .push("/models        查看当前 Codex/Traex/Claude Code Runner 可选模型");
                runner_lines.push(
                    "/model [模型]   查看或切换当前 Codex/Traex/Claude Code Runner 的 session 模型；/model clear 清除",
                );
            }
            if crate::im_gateway::external_cli::supports_external_cli_resume_slash(adapter) {
                runner_lines.push(
                    "/resume [session-id]  查看最近 20 个本地会话，或选择一个会话在下一条消息恢复",
                );
            }
            if !crate::im_gateway::external_cli::external_cli_effort_options(adapter).is_empty() {
                runner_lines.push(
                    "/efforts       查看当前 Codex/Traex/Claude Code Runner 可选 Reasoning Effort",
                );
                runner_lines.push(
                    "/effort [级别]  查看或切换当前 Codex/Traex/Claude Code Runner 的 Reasoning Effort；/effort clear 清除",
                );
            }
            if crate::im_gateway::external_cli::supports_external_cli_fast_slash(adapter) {
                runner_lines.push(
                    "/fast [on|off|status]  切换或查看当前 Codex session 的快速模式；直接发送 /fast 可切换",
                );
            }
            if !runner_lines.is_empty() {
                let label =
                    crate::im_gateway::external_cli::external_cli_model_adapter_label(adapter);
                let mut section = format!("{label} Runner 命令:\n");
                section.push_str(&runner_lines.join("\n"));
                sections.push(section);
            }
        }
    }

    sections.join("\n\n")
}

pub(super) async fn resolve_event_images(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    images: &[ImImageAttachment],
) -> Vec<bifrost_agent::ChatImageInput> {
    let mut resolved = Vec::new();
    if images.len() > MAX_AGENT_ATTACHMENTS_PER_MESSAGE {
        warn!(
            provider_id = %provider.id,
            event_id = %event.event_id,
            image_count = images.len(),
            max_images = MAX_AGENT_ATTACHMENTS_PER_MESSAGE,
            "too many IM images in one message; truncating images for agent multimodal input"
        );
    }
    for image in images.iter().take(MAX_AGENT_ATTACHMENTS_PER_MESSAGE) {
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

pub(super) async fn resolve_event_files(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    files: &[ImFileAttachment],
) -> Vec<crate::im_gateway::external_cli::ExternalCliFileInput> {
    let mut resolved = Vec::new();
    let mut total_file_bytes = 0u64;
    if files.len() > MAX_AGENT_ATTACHMENTS_PER_MESSAGE {
        warn!(
            provider_id = %provider.id,
            event_id = %event.event_id,
            file_count = files.len(),
            max_files = MAX_AGENT_ATTACHMENTS_PER_MESSAGE,
            "too many IM file attachments in one message; truncating files for agent input"
        );
    }
    for file in files.iter().take(MAX_AGENT_ATTACHMENTS_PER_MESSAGE) {
        let file_label = file.name.as_deref().unwrap_or(&file.file_key);
        if let Some(data) = &file.data_base64 {
            let decoded_size = match preloaded_payload_size(
                Some(data),
                "文件",
                file_label,
                MAX_FEISHU_REFERENCED_FILE_BYTES,
            ) {
                Ok(size) => size.unwrap_or_default(),
                Err(problem) => {
                    warn!(
                        provider_id = %provider.id,
                        event_id = %event.event_id,
                        file = %file_label,
                        problem,
                        "skipping oversized inline IM file attachment"
                    );
                    continue;
                }
            };
            if referenced_file_budget_exceeded_with_limit(
                total_file_bytes,
                decoded_size,
                MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES,
            ) {
                warn!(
                    provider_id = %provider.id,
                    event_id = %event.event_id,
                    file = %file_label,
                    "skipping IM file attachment because the message total exceeds 250 MiB"
                );
                continue;
            }
            total_file_bytes = total_file_bytes.saturating_add(decoded_size);
            resolved.push(crate::im_gateway::external_cli::ExternalCliFileInput {
                mime_type: file
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                data: data.clone(),
                name: file.name.clone(),
            });
            continue;
        }

        if file
            .size_bytes
            .is_some_and(|size| size > MAX_FEISHU_REFERENCED_FILE_BYTES)
        {
            warn!(
                provider_id = %provider.id,
                event_id = %event.event_id,
                file = %file_label,
                "skipping oversized IM file attachment before download"
            );
            continue;
        }
        if file.size_bytes.is_some_and(|size| {
            referenced_file_budget_exceeded_with_limit(
                total_file_bytes,
                size,
                MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES,
            )
        }) {
            warn!(
                provider_id = %provider.id,
                event_id = %event.event_id,
                file = %file_label,
                "skipping IM file attachment because the declared message total exceeds 250 MiB"
            );
            continue;
        }

        let Some(message_id) = event.source.message_id.as_deref() else {
            warn!(
                provider_id = %provider.id,
                file_key = %file.file_key,
                "cannot download IM file because message_id is missing"
            );
            continue;
        };
        match client
            .download_message_file_resource(provider, message_id, file)
            .await
        {
            Ok((mime_type, bytes)) => {
                if bytes.len() as u64 > MAX_FEISHU_REFERENCED_FILE_BYTES {
                    warn!(
                        provider_id = %provider.id,
                        message_id = %message_id,
                        file = %file_label,
                        "skipping oversized downloaded IM file attachment"
                    );
                    continue;
                }
                if referenced_file_budget_exceeded_with_limit(
                    total_file_bytes,
                    bytes.len() as u64,
                    MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES,
                ) {
                    warn!(
                        provider_id = %provider.id,
                        message_id = %message_id,
                        file = %file_label,
                        "skipping downloaded IM file attachment because the message total exceeds 250 MiB"
                    );
                    continue;
                }
                total_file_bytes = total_file_bytes.saturating_add(bytes.len() as u64);
                info!(
                    provider_id = %provider.id,
                    message_id = %message_id,
                    file_key = %file.file_key,
                    mime_type = %mime_type,
                    byte_len = bytes.len(),
                    "downloaded IM file resource for agent attachment input"
                );
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                resolved.push(crate::im_gateway::external_cli::ExternalCliFileInput {
                    mime_type,
                    data,
                    name: file.name.clone(),
                });
            }
            Err(error) => {
                warn!(
                    provider_id = %provider.id,
                    message_id = %message_id,
                    file_key = %file.file_key,
                    error = %error,
                    "failed to download IM file resource"
                );
            }
        }
    }
    resolved
}

pub(super) async fn hydrate_referenced_group_attachments(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    referenced: crate::im_gateway::group_context::ReferencedGroupAttachments,
) -> (Vec<ImImageAttachment>, Vec<ImFileAttachment>, Vec<String>) {
    hydrate_referenced_group_attachments_with_limits(
        client,
        provider,
        referenced,
        default_referenced_attachment_limits(),
    )
    .await
}

pub(super) type ReferencedAttachmentLimits = [u64; 4];

fn default_referenced_attachment_limits() -> ReferencedAttachmentLimits {
    [
        MAX_AGENT_ATTACHMENTS_PER_MESSAGE as u64,
        MAX_AGENT_REPLY_IMAGE_BYTES,
        MAX_FEISHU_REFERENCED_FILE_BYTES,
        MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES,
    ]
}

pub(super) async fn hydrate_referenced_group_attachments_with_limits(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    referenced: crate::im_gateway::group_context::ReferencedGroupAttachments,
    limits: ReferencedAttachmentLimits,
) -> (Vec<ImImageAttachment>, Vec<ImFileAttachment>, Vec<String>) {
    let [max_per_kind, max_image_bytes, max_file_bytes, max_total_file_bytes] = limits;
    let max_per_kind = max_per_kind as usize;
    let mut notices = Vec::new();
    let mut images = Vec::new();
    if referenced.images.len() > max_per_kind {
        notices.push(count_notice("图片", referenced.images.len(), max_per_kind));
    }
    for mut image in referenced.images.into_iter().take(max_per_kind) {
        if let Err(notice) = preloaded_payload_size(
            image.data_base64.as_deref(),
            "图片",
            &image.file_key,
            max_image_bytes,
        ) {
            notices.push(notice);
            continue;
        }
        if image.data_base64.is_none() {
            let (mime_type, bytes) = match client
                .download_message_image_resource(provider, &referenced.message_id, &image)
                .await
            {
                Ok(downloaded) => downloaded,
                Err(error) => {
                    let problem = format!("下载失败：{error}");
                    notices.push(problem_notice("图片", &image.file_key, &problem));
                    continue;
                }
            };
            if bytes.len() as u64 > max_image_bytes {
                notices.push(size_notice("图片", &image.file_key, max_image_bytes));
                continue;
            }
            image.mime_type = Some(mime_type);
            image.data_base64 = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
        }
        images.push(image);
    }

    let mut files = Vec::new();
    if referenced.files.len() > max_per_kind {
        notices.push(count_notice("文件", referenced.files.len(), max_per_kind));
    }
    let mut total_file_bytes = 0u64;
    for mut file in referenced.files.into_iter().take(max_per_kind) {
        let file_label = file.name.as_deref().unwrap_or(&file.file_key).to_string();
        let preloaded_size = match preloaded_payload_size(
            file.data_base64.as_deref(),
            "文件",
            &file_label,
            max_file_bytes,
        ) {
            Ok(size) => size,
            Err(notice) => {
                notices.push(notice);
                continue;
            }
        };
        let expected_size = preloaded_size.or(file.size_bytes);
        if expected_size.is_some_and(|size| size > max_file_bytes) {
            notices.push(size_notice("文件", &file_label, max_file_bytes));
            continue;
        }
        if expected_size.is_some_and(|size| {
            referenced_file_budget_exceeded_with_limit(total_file_bytes, size, max_total_file_bytes)
        }) {
            notices.push(total_notice(&file_label, max_total_file_bytes));
            continue;
        }
        if preloaded_size.is_none() {
            let (mime_type, bytes) = match client
                .download_message_file_resource(provider, &referenced.message_id, &file)
                .await
            {
                Ok(downloaded) => downloaded,
                Err(error) => {
                    let problem = format!("下载失败：{error}");
                    notices.push(problem_notice("文件", &file_label, &problem));
                    continue;
                }
            };
            if bytes.len() as u64 > max_file_bytes {
                notices.push(size_notice("文件", &file_label, max_file_bytes));
                continue;
            }
            file.mime_type = Some(mime_type);
            file.size_bytes = Some(bytes.len() as u64);
            file.data_base64 = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
        } else {
            // Metadata attached to restored events is not authoritative. Use
            // the decoded payload size so forged/missing size_bytes cannot
            // bypass the per-file or per-turn budgets.
            file.size_bytes = preloaded_size;
        }
        let file_bytes = file.size_bytes.unwrap_or_default();
        if referenced_file_budget_exceeded_with_limit(
            total_file_bytes,
            file_bytes,
            max_total_file_bytes,
        ) {
            notices.push(total_notice(&file_label, max_total_file_bytes));
            continue;
        }
        total_file_bytes = total_file_bytes.saturating_add(file_bytes);
        files.push(file);
    }
    (images, files, notices)
}

fn count_notice(kind: &str, count: usize, max: usize) -> String {
    let unit = if kind == "图片" { "张" } else { "个" };
    format!("引用消息包含 {count} {unit}{kind}，最多处理 {max} {unit}；其余已跳过，任务继续执行。")
}

fn size_notice(kind: &str, label: &str, max_bytes: u64) -> String {
    let max_mib = max_bytes / 1024 / 1024;
    format!("引用{kind}「{label}」超过 {max_mib} MiB 上限；已跳过，任务继续执行。")
}

fn problem_notice(kind: &str, label: &str, problem: &str) -> String {
    format!("引用{kind}「{label}」{problem}；已跳过，任务继续执行。")
}

fn total_notice(label: &str, max_bytes: u64) -> String {
    let max_mib = max_bytes / 1024 / 1024;
    format!("引用文件「{label}」会使附件总量超过 {max_mib} MiB；已跳过，任务继续执行。")
}

#[cfg(test)]
pub(super) fn referenced_file_budget_exceeded(current_bytes: u64, next_bytes: u64) -> bool {
    referenced_file_budget_exceeded_with_limit(
        current_bytes,
        next_bytes,
        MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES,
    )
}

pub(super) fn referenced_file_budget_exceeded_with_limit(
    current_bytes: u64,
    next_bytes: u64,
    max_bytes: u64,
) -> bool {
    current_bytes.saturating_add(next_bytes) > max_bytes
}

pub(super) fn preloaded_payload_size(
    data: Option<&str>,
    kind: &str,
    label: &str,
    max_bytes: u64,
) -> Result<Option<u64>, String> {
    let Some(data) = data else { return Ok(None) };
    let decoded_upper_bound = data
        .len()
        .checked_add(3)
        .and_then(|length| length.checked_div(4))
        .and_then(|groups| groups.checked_mul(3))
        .ok_or_else(|| size_notice(kind, label, max_bytes))?;
    if decoded_upper_bound as u64 > max_bytes.saturating_add(2) {
        return Err(size_notice(kind, label, max_bytes));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| problem_notice(kind, label, "内容不是有效 Base64"))?;
    if decoded.len() as u64 > max_bytes {
        return Err(size_notice(kind, label, max_bytes));
    }
    Ok(Some(decoded.len() as u64))
}

pub(super) fn agent_message_text(message: &crate::im_gateway::types::ImEventMessage) -> String {
    let text = message.text.trim();
    if !text.is_empty() {
        text.to_string()
    } else if !message.images.is_empty() {
        IMAGE_ONLY_AGENT_PROMPT.to_string()
    } else if !message.files.is_empty() {
        format!("[附件消息: {} 个]", message.files.len())
    } else {
        String::new()
    }
}

pub(super) const MAX_QUOTED_AGENT_CONTEXT_CHARS: usize = 8_000;

pub(super) fn agent_message_text_with_reference(
    message: &crate::im_gateway::types::ImEventMessage,
    provider_id: &str,
    peer_id: Option<&str>,
    current_message_id: Option<&str>,
    message_log_store: &ImMessageLogStore,
) -> String {
    let current = agent_message_text(message);
    if current.trim_start().starts_with('/') {
        return current;
    }
    let Some(reference) = message.reply_to.as_ref() else {
        return current;
    };
    let Some(quoted) = message_log_store.resolve_reference_text(
        provider_id,
        peer_id,
        current_message_id,
        reference,
    ) else {
        debug!(
            provider_id,
            peer_id,
            reference_message_id = ?reference.message_id,
            reference_created_at_ms = ?reference.created_at_ms,
            "quoted IM message could not be resolved; continuing with current message"
        );
        return current;
    };
    let quoted = bifrost_core::text::truncate_chars_with_ellipsis(
        quoted.trim(),
        MAX_QUOTED_AGENT_CONTEXT_CHARS,
    );
    format!(
        "【引用消息（仅作为上下文）】\n{quoted}\n\n【当前消息】\n{}",
        current.trim()
    )
}

pub(super) fn inbound_message_preview(
    message: &crate::im_gateway::types::ImEventMessage,
) -> String {
    if message.text.trim().is_empty() && !message.images.is_empty() {
        format!("[图片消息: {} 张]", message.images.len())
    } else if message.text.trim().is_empty() && !message.files.is_empty() {
        format!("[附件消息: {} 个]", message.files.len())
    } else {
        truncate_str(&message.text, 200)
    }
}

#[rustfmt::skip] fn fill_missing_status_context(target: &mut bifrost_agent::StatusRuntimeContext, fallback: &bifrost_agent::StatusRuntimeContext) { target.model = target.model.take().or_else(|| fallback.model.clone()); target.model_provider = target.model_provider.take().or_else(|| fallback.model_provider.clone()); target.external_thread_id = target.external_thread_id.take().or_else(|| fallback.external_thread_id.clone()); target.external_conversation_id = target.external_conversation_id.take().or_else(|| fallback.external_conversation_id.clone()); }
#[rustfmt::skip] fn format_status_queue_info(len: usize) -> String { if len == 0 { "无排队消息".to_string() } else { format!("{len} 条排队消息") } }
#[rustfmt::skip] pub(super) async fn handle_busy_message(
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
        let runner_id_override = ctx.group_context_store.runner_id_by_session(session_key).ok().flatten();
        let mut status_context = resolve_im_status_runtime_context(ctx.agent_config, &ctx.external_cli_config_store.load(), &ctx.provider.id, session_key, runner_id_override.as_deref());
        fill_missing_status_context(&mut status_context, &ctx.status_context);
        let queue_items = queue_manager.queue_status(session_key);
        let queue_info = format_status_queue_info(queue_items.len());
        let device_name = current_device_name();
        // Try to get session detail from idle sessions
        if let Some(detail) = agent_session_manager.get_session_detail(session_key) {
            let reply = build_im_status_text(
                Some(&detail),
                &status_context,
                ctx.default_work_dir.as_deref(),
                &ImStatusChannelContext { provider, device_name: &device_name, session_key, queue_info: &queue_info, status: "Ready" },
            );
            send_agent_reply(client, provider, event, &reply, message_log_store).await;
        } else if let Some(mut status) = agent_session_manager.get_active_turn_status(session_key) {
            status.pending_guide_messages = merge_pending_guide_messages(
                &status.pending_guide_messages,
                queue_manager.guide_status(session_key),
            );
            let reply = build_active_im_status_text(&status, &status_context, ctx.default_work_dir.as_deref(), &ImStatusChannelContext { provider, device_name: &device_name, session_key, queue_info: &queue_info, status: "Running" });
            send_agent_reply(client, provider, event, &reply, message_log_store).await;
        } else {
            // Session is currently being processed (taken out of the pool)
            let guide_info = format_pending_guide_status(&queue_manager.guide_status(session_key));
            let mut reply = build_im_status_text(
                None,
                &status_context,
                ctx.default_work_dir.as_deref(),
                &ImStatusChannelContext {
                    provider,
                    device_name: &device_name,
                    session_key,
                    queue_info: &queue_info,
                    status: "Running",
                },
            );
            reply.push_str(&format!("\n{guide_info}"));
            send_agent_reply(client, provider, event, &reply, message_log_store).await;
        }
        return;
    }

    if trimmed == "/q" {
        let reply = format_queue_status(
            "📋 当前线程排队消息",
            &queue_manager.queue_status(session_key),
        );
        send_agent_reply(client, provider, event, &reply, message_log_store).await;
        return;
    }

    if trimmed == "/pwd" {
        let reply = format_effective_im_work_dir(
            ctx.group_context_store,
            session_key,
            ctx.agent_config,
        );
        send_agent_reply(client, provider, event, &reply, message_log_store).await;
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
            ImRunnerCommand::List => format_effective_im_runner(
                ctx.group_context_store,
                session_key,
                ctx.agent_config,
                &config,
                &ctx.provider.id,
            ),
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

    if crate::im_gateway::external_cli::parse_external_cli_model_slash_command(trimmed).is_some() {
        send_agent_reply(
            client,
            provider,
            event,
            "当前任务正在处理中，请等待任务结束后再切换 Runner 模型。",
            message_log_store,
        )
        .await;
        return;
    }

    if crate::im_gateway::external_cli::parse_external_cli_resume_slash_command(trimmed).is_some() {
        send_agent_reply(
            client,
            provider,
            event,
            "当前任务正在处理中，请等待任务结束后再切换本地 session。",
            message_log_store,
        )
        .await;
        return;
    }

    if handle_im_effort_command(
        trimmed,
        session_key,
        ctx.agent_config,
        ImModelCommandContext {
            client,
            provider,
            external_cli_config_store: ctx.external_cli_config_store,
            group_context_store: ctx.group_context_store,
            event,
            message_log_store,
        },
    )
    .await
    {
        return;
    }

    if handle_im_fast_command(
        trimmed,
        session_key,
        ctx.agent_config,
        ImModelCommandContext {
            client,
            provider,
            external_cli_config_store: ctx.external_cli_config_store,
            group_context_store: ctx.group_context_store,
            event,
            message_log_store,
        },
    )
    .await
    {
        return;
    }

    if let Some(command) = parse_im_cwd_command(trimmed) {
        match command {
            Ok(path) => {
                let queued_command = format!("/cwd {}", path.display());
                match queue_manager.push_queue_with_images_and_context(
                    session_key,
                    queued_command,
                    Vec::new(),
                    Some(queue_item_context(&ctx)),
                ) {
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
                        release_busy_group_turn(&ctx, err);
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
            release_busy_group_turn(&ctx, "queued message is empty");
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
        let images = match event.message.as_ref() {
            Some(event_message) if !event_message.images.is_empty() => {
                resolve_event_images(client, provider, event, &event_message.images).await
            }
            _ => Vec::new(),
        };
        let files = match event.message.as_ref() {
            Some(event_message) if !event_message.files.is_empty() => {
                resolve_event_files(client, provider, event, &event_message.files).await
            }
            _ => Vec::new(),
        };
        match queue_manager.push_queue_with_attachments_and_context(
            session_key,
            queue_text.to_string(),
            images,
            files,
            Some(queue_item_context(&ctx)),
        ) {
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
                release_busy_group_turn(&ctx, err);
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
                if let Some(removed) = queue_manager.remove_queue_item(session_key, seq) {
                    if let Some(turn_id) = removed
                        .context
                        .as_ref()
                        .and_then(|context| context.group_turn_id.as_deref())
                    {
                        if let Err(error) = ctx.group_context_store.release_turn(
                            turn_id,
                            "queued message was removed",
                            now_ms(),
                        ) {
                            warn!(turn_id = %turn_id, error = %error, "failed to release removed queued group turn");
                        }
                    }
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

    // Session reset commands need the active external run to release its state.
    if matches!(trimmed, "/clear" | "/reset") {
        let reply = format!(
            "⏳ Agent 正在处理中，{} 命令需要等待当前任务完成后执行。\n\n\
             可用操作:\n\
             - /q <消息> — 排队消息\n\
             - /rq <序号> — 取消排队\n\
             - /status — 查看状态\n\
             - /stop — 立即停止当前 Runner\n\
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
            release_busy_group_turn(&ctx, "guide message is empty");
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
        if ctx.default_mode != BusyMessageDefaultMode::Guide
            && ctx
                .event
                .message
                .as_ref()
                .is_some_and(|message| !message.images.is_empty() || !message.files.is_empty())
        {
            handle_busy_default_message(guide_text, session_key, &ctx).await;
            return;
        }
        handle_busy_guide_command(guide_text, session_key, &ctx).await;
        return;
    }

    // External runners try live guide except ChatGPT Web, which keeps queue semantics.
    handle_busy_default_message(trimmed, session_key, &ctx).await;
}

#[cfg(test)]
mod local_resume_tests {
    use super::*;

    #[test]
    fn status_context_fallback_fills_only_missing_external_metadata() {
        assert_eq!(format_status_queue_info(0), "无排队消息");
        assert_eq!(format_status_queue_info(2), "2 条排队消息");
        let fallback = bifrost_agent::StatusRuntimeContext {
            model: Some("fallback-model".into()),
            model_provider: Some("fallback-provider".into()),
            external_thread_id: Some("fallback-thread".into()),
            external_conversation_id: Some("fallback-conversation".into()),
            ..Default::default()
        };
        let mut missing = bifrost_agent::StatusRuntimeContext::default();
        fill_missing_status_context(&mut missing, &fallback);
        assert_eq!(missing.model.as_deref(), Some("fallback-model"));
        assert_eq!(missing.model_provider.as_deref(), Some("fallback-provider"));
        assert_eq!(
            missing.external_thread_id.as_deref(),
            Some("fallback-thread")
        );
        assert_eq!(
            missing.external_conversation_id.as_deref(),
            Some("fallback-conversation")
        );

        let mut existing = bifrost_agent::StatusRuntimeContext {
            model: Some("existing-model".into()),
            model_provider: Some("existing-provider".into()),
            external_thread_id: Some("existing-thread".into()),
            external_conversation_id: Some("existing-conversation".into()),
            ..Default::default()
        };
        fill_missing_status_context(&mut existing, &fallback);
        assert_eq!(existing.model.as_deref(), Some("existing-model"));
        assert_eq!(
            existing.model_provider.as_deref(),
            Some("existing-provider")
        );
        assert_eq!(
            existing.external_thread_id.as_deref(),
            Some("existing-thread")
        );
        assert_eq!(
            existing.external_conversation_id.as_deref(),
            Some("existing-conversation")
        );
    }

    struct CodexHomeGuard(Option<std::ffi::OsString>);

    impl Drop for CodexHomeGuard {
        fn drop(&mut self) {
            if let Some(value) = self.0.take() {
                std::env::set_var("CODEX_HOME", value);
            } else {
                std::env::remove_var("CODEX_HOME");
            }
        }
    }

    fn resume_event(provider: &ImProviderConfig, command: &str) -> ImEvent {
        ImEvent {
            event_id: format!("evt-resume-{}", uuid_short()),
            provider_id: provider.id.clone(),
            provider_type: provider.provider_type,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("resume-chat".to_string()),
                user_id: Some("resume-user".to_string()),
                message_id: Some(format!("om-resume-{}", uuid_short())),
                ..Default::default()
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: command.to_string(),
                ..Default::default()
            }),
            received_at: now_ms(),
            raw_digest: None,
        }
    }

    #[tokio::test]
    async fn local_resume_commands_cover_idle_selection_errors_and_busy_rejection() {
        let _local_session_lock = crate::im_gateway::external_cli::local_session_test_env_lock()
            .lock()
            .await;
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let codex_home = tempfile::tempdir().expect("codex home");
        let _data_guard =
            crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let _home_guard = CodexHomeGuard(std::env::var_os("CODEX_HOME"));
        std::env::set_var("CODEX_HOME", codex_home.path());
        let id = "eeeeeeee-0000-0000-0000-000000000006";
        let session_path = codex_home.path().join("sessions/resume.jsonl");
        std::fs::create_dir_all(session_path.parent().expect("parent")).expect("create sessions");
        std::fs::write(
            &session_path,
            format!(
                "{{\"timestamp\":\"2026-08-07T05:06:07Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\"}}}}\n"
            ),
        )
        .expect("write session");

        let service = ImGatewayService::new(temp_dir.path());
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        provider.id = "weixin-resume".to_string();
        provider.provider_type = ImProviderType::Weixin;
        provider.secret_ref = None;
        let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
        let agent_config = service.agent_config_store.load();
        let session_key = build_session_key(&provider.id, Some("resume-user"));

        for command in ["/resume", "/resume eeeeeeee", "/resume too many"] {
            let event = resume_event(&provider, command);
            assert!(
                handle_idle_im_command(
                    command,
                    &session_key,
                    &agent_config,
                    IdleImCommandContext {
                        client: &client,
                        provider: &provider,
                        provider_store: &service.provider_store,
                        group_context_store: &service.group_context_store,
                        external_cli_config_store: &service.external_cli_config_store,
                        event: &event,
                        message_log_store: &service.message_log_store,
                        agent_session_manager: &service.agent_session_manager,
                        queue_manager: &service.queue_manager,
                    },
                )
                .await
            );
        }
        let state = crate::im_gateway::session_state::load_session_state(
            &session_key,
            "codex",
            Some(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID),
        )
        .expect("selected state");
        assert_eq!(state.external_thread_id.as_deref(), Some(id));

        let event = resume_event(&provider, "/resume");
        let no_runner = crate::im_gateway::agent::ImAgentConfig {
            runner: None,
            ..agent_config.clone()
        };
        assert!(
            handle_im_resume_command(
                "/resume",
                "resume-no-runner",
                &no_runner,
                ImModelCommandContext {
                    client: &client,
                    provider: &provider,
                    external_cli_config_store: &service.external_cli_config_store,
                    group_context_store: &service.group_context_store,
                    event: &event,
                    message_log_store: &service.message_log_store,
                },
            )
            .await
        );

        let busy_event = resume_event(&provider, "/resume eeeeeeee");
        handle_busy_message(
            "/resume eeeeeeee",
            &session_key,
            BusyMessageContext {
                queue_manager: &service.queue_manager,
                client: &client,
                provider: &provider,
                event: &busy_event,
                message_log_store: &service.message_log_store,
                agent_session_manager: &service.agent_session_manager,
                progress_registry: &service.progress_registry,
                external_cli_config_store: &service.external_cli_config_store,
                agent_config: &agent_config,
                group_context_store: &service.group_context_store,
                group_turn_id: None,
                default_mode: BusyMessageDefaultMode::Queue,
                status_context: Default::default(),
                default_work_dir: None,
            },
        )
        .await;

        let replies = service.message_log_store.list();
        for expected in [
            id,
            "已选择本地 session",
            "用法: /resume",
            "请先用 `/runner`",
            "任务正在处理中",
        ] {
            assert!(
                replies.iter().any(|log| {
                    log.content
                        .as_deref()
                        .is_some_and(|content| content.contains(expected))
                }),
                "missing reply containing {expected}"
            );
        }

        let help = build_im_channel_help_sections(
            &ImHelpRunnerKind::External {
                adapter: "codex".to_string(),
            },
            ImProviderType::Feishu,
        );
        assert!(help.contains("/resume"));
        let unsupported = build_im_channel_help_sections(
            &ImHelpRunnerKind::External {
                adapter: "mock".to_string(),
            },
            ImProviderType::Weixin,
        );
        assert!(!unsupported.contains("/resume"));
        assert!(!unsupported.contains("/new <群名>"));
    }
}
