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

struct ImModelCommandContext<'a> {
    client: &'a ImProviderClient,
    provider: &'a ImProviderConfig,
    external_cli_config_store: &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    event: &'a ImEvent,
    message_log_store: &'a Arc<ImMessageLogStore>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImRunnerCommand {
    List,
    Switch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImRunnerSelection {
    pub(super) runner_id: String,
    pub(super) runner: bifrost_agent::AgentRunnerMode,
    pub(super) adapter: Option<String>,
}

pub(super) async fn handle_idle_im_command(
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

    if handle_im_model_command(
        trimmed,
        session_key,
        agent_config,
        ImModelCommandContext {
            client: ctx.client,
            provider: ctx.provider,
            external_cli_config_store: ctx.external_cli_config_store,
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
) -> String {
    let runner_kind =
        im_help_runner_kind_for_agent_config(agent_config, external_cli_config, provider_id);
    format!(
        "可用命令:\n\n{}",
        build_im_channel_help_sections(&runner_kind)
    )
}

pub(super) fn build_im_startup_help_for_runner(runner_kind: &ImHelpRunnerKind) -> String {
    format!(
        "可用命令:\n\n{}",
        build_im_channel_help_sections(runner_kind)
    )
}

pub(super) fn build_im_channel_help_sections(runner_kind: &ImHelpRunnerKind) -> String {
    let mut sections = vec![
        "IM 通道命令（所有 Runner）:\n\
         /help           显示此帮助信息\n\
         /status         查看当前 IM 会话状态、Runner、模型和排队情况\n\
         /cwd <绝对路径>  切换当前 IM 通道绑定的工作目录；路径必须存在且是目录，运行中会排队到当前任务结束后执行\n\
         /runner [Runner]  查看或切换当前 IM 通道绑定的 Runner\n\
         /clear          重置当前 IM 会话上下文\n\
         /reset          重置当前 IM 会话上下文\n\
         /q <消息>       将消息加入队列，当前任务结束后自动继续处理\n\
         /rq <序号>      取消一条排队消息\n\
         /stop           停止当前正在执行的任务"
            .to_string(),
    ];

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
            if !crate::im_gateway::external_cli::external_cli_effort_options(adapter).is_empty() {
                runner_lines.push(
                    "/efforts       查看当前 Codex/Traex/Claude Code Runner 可选 Reasoning Effort",
                );
                runner_lines.push(
                    "/effort [级别]  查看或切换当前 Codex/Traex/Claude Code Runner 的 Reasoning Effort；/effort clear 清除",
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
    let canonical_runner_id =
        crate::im_gateway::external_cli::canonical_external_cli_runner_id(config, runner_id);
    let Some(settings) = config.runners.get(&canonical_runner_id) else {
        return Err(format!(
            "找不到 Runner: `{}`\n\n支持的 Runner:\n{}",
            runner_id,
            format_im_runner_list(config)
        ));
    };
    Ok(ImRunnerSelection {
        runner_id: canonical_runner_id.clone(),
        runner: bifrost_agent::AgentRunnerMode::Custom(canonical_runner_id),
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
        bifrost_agent::AgentRunnerMode::Custom(_) => session.mark_external_runner_runtime(
            &selection.runner_id,
            selection.adapter.as_deref().unwrap_or(&selection.runner_id),
        ),
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

async fn handle_im_model_command(
    message: &str,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    ctx: ImModelCommandContext<'_>,
) -> bool {
    let Some(command) =
        crate::im_gateway::external_cli::parse_external_cli_model_slash_command(message)
    else {
        return false;
    };
    let command = match command {
        Ok(command) => command,
        Err(reason) => {
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                &format!("❌ {reason}"),
                ctx.message_log_store,
            )
            .await;
            return true;
        }
    };
    let config = ctx.external_cli_config_store.load();
    let Some(configured_runner_id) = agent_config
        .runner
        .as_ref()
        .and_then(|runner| runner.custom_runner_id())
        .map(ToString::to_string)
    else {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
        "/model 和 /models 当前仅支持 Codex、Traex 或 Claude Code Runner。请先用 `/runner Codex`、`/runner Traex` 或 `/runner Claude Code` 切换。",
            ctx.message_log_store,
        )
        .await;
        return true;
    };
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        Some(ctx.provider.id.as_str()),
        Some(configured_runner_id.as_str()),
    );
    if !crate::im_gateway::external_cli::supports_external_cli_model_slash(
        &effective.settings.adapter,
    ) {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "/model 和 /models 当前仅支持 Codex、Traex 或 Claude Code Runner。",
            ctx.message_log_store,
        )
        .await;
        return true;
    }
    let adapter_label = crate::im_gateway::external_cli::external_cli_model_adapter_label(
        &effective.settings.adapter,
    );
    let reply = match command {
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::List => {
            match crate::im_gateway::external_cli::load_external_cli_model_catalog(
                &effective.settings.adapter,
                &effective.settings.adapter_config,
                None,
            )
            .await
            {
                Ok(models) => crate::im_gateway::external_cli::format_external_cli_model_catalog(
                    &effective.settings.adapter,
                    &models,
                ),
                Err(error) => format!("无法获取 {adapter_label} 模型列表：{error}"),
            }
        }
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::Show => {
            let state = crate::im_gateway::session_state::load_session_state(
                session_key,
                &effective.settings.adapter,
                Some(&effective.runner_id),
            );
            let (model, source) = state
                .and_then(|state| {
                    if state.model_override.is_none() && state.model_override_source.is_none() {
                        None
                    } else {
                        Some((state.model_override, state.model_override_source))
                    }
                })
                .unwrap_or_else(|| {
                    let resolved =
                        crate::im_gateway::external_cli::resolve_external_cli_model_config(
                            &effective.settings.adapter,
                            &effective.settings.adapter_config,
                        );
                    (resolved.model, resolved.model_source)
                });
            crate::im_gateway::external_cli::format_external_cli_model_status(
                &effective.settings.adapter,
                model.as_deref(),
                source.as_deref(),
                &effective.runner_id,
            )
        }
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::Clear => {
            persist_im_model_override(
                session_key,
                &effective.settings.adapter,
                &effective.runner_id,
                None,
            );
            persist_im_model_system_message(
                session_key,
                &effective.settings.adapter,
                &effective.runner_id,
                "清除模型切换",
            );
            format!(
                "已清除 {adapter_label} Runner `{}` 的 session 模型 override。下一条消息将使用 Runner 配置或 {adapter_label} 默认模型。",
                effective.runner_id
            )
        }
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::Set(model) => {
            match crate::im_gateway::external_cli::load_external_cli_model_catalog(
                &effective.settings.adapter,
                &effective.settings.adapter_config,
                None,
            )
            .await
            {
                Ok(models) => {
                    match crate::im_gateway::external_cli::validate_external_cli_model_selection(
                        &effective.settings.adapter,
                        &model,
                        &models,
                    ) {
                        Ok(model) => {
                            persist_im_model_override(
                                session_key,
                                &effective.settings.adapter,
                                &effective.runner_id,
                                Some(model.clone()),
                            );
                            let reply = format!(
                                "已将 {adapter_label} Runner `{}` 的 session 模型设置为 `{}`。\n下一条消息会通过 `--model {}` 启动。",
                                effective.runner_id, model, model
                            );
                            persist_im_model_system_message(
                                session_key,
                                &effective.settings.adapter,
                                &effective.runner_id,
                                &format!("切换模型为 {model}"),
                            );
                            reply
                        }
                        Err(response) => response,
                    }
                }
                Err(error) => {
                    format!("未切换模型：无法验证 {adapter_label} 模型 `{model}`：{error}")
                }
            }
        }
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

async fn handle_im_effort_command(
    message: &str,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    ctx: ImModelCommandContext<'_>,
) -> bool {
    let Some(command) =
        crate::im_gateway::external_cli::parse_external_cli_effort_slash_command(message)
    else {
        return false;
    };
    let command = match command {
        Ok(command) => command,
        Err(reason) => {
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                &format!("❌ {reason}"),
                ctx.message_log_store,
            )
            .await;
            return true;
        }
    };
    let config = ctx.external_cli_config_store.load();
    let Some(configured_runner_id) = agent_config
        .runner
        .as_ref()
        .and_then(|runner| runner.custom_runner_id())
        .map(ToString::to_string)
    else {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
        "/effort 当前仅支持 Codex、Traex 或 Claude Code Runner。请先用 `/runner Codex`、`/runner Traex` 或 `/runner Claude Code` 切换。",
            ctx.message_log_store,
        )
        .await;
        return true;
    };
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        Some(ctx.provider.id.as_str()),
        Some(configured_runner_id.as_str()),
    );
    if crate::im_gateway::external_cli::external_cli_effort_options(&effective.settings.adapter)
        .is_empty()
    {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "/effort 当前仅支持 Codex、Traex 或 Claude Code Runner。",
            ctx.message_log_store,
        )
        .await;
        return true;
    }
    let adapter_label = crate::im_gateway::external_cli::external_cli_model_adapter_label(
        &effective.settings.adapter,
    );
    let mut resolved_model_config =
        crate::im_gateway::external_cli::resolve_external_cli_model_config(
            &effective.settings.adapter,
            &effective.settings.adapter_config,
        );
    if let Some(state) = crate::im_gateway::session_state::load_session_state(
        session_key,
        &effective.settings.adapter,
        Some(&effective.runner_id),
    ) {
        if let Some(model) = state
            .model_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            resolved_model_config.model = Some(model.to_string());
            resolved_model_config.model_source = state.model_override_source;
        }
    }
    let model_catalog = crate::im_gateway::external_cli::load_external_cli_model_catalog(
        &effective.settings.adapter,
        &effective.settings.adapter_config,
        None,
    )
    .await
    .unwrap_or_default();
    let reply = match command {
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::List => {
            crate::im_gateway::external_cli::format_external_cli_effort_catalog_for_model(
                &effective.settings.adapter,
                resolved_model_config.model.as_deref(),
                &model_catalog,
            )
        }
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::Show => {
            let state = crate::im_gateway::session_state::load_session_state(
                session_key,
                &effective.settings.adapter,
                Some(&effective.runner_id),
            );
            let (effort, source) = state
                .and_then(|state| {
                    if state.reasoning_effort_override.is_none()
                        && state.reasoning_effort_override_source.is_none()
                    {
                        None
                    } else {
                        Some((
                            state.reasoning_effort_override,
                            state.reasoning_effort_override_source,
                        ))
                    }
                })
                .unwrap_or_else(|| {
                    (
                        resolved_model_config.reasoning_effort.clone(),
                        resolved_model_config.reasoning_source.clone(),
                    )
                });
            crate::im_gateway::external_cli::format_external_cli_effort_status(
                &effective.settings.adapter,
                effort.as_deref(),
                source.as_deref(),
                &effective.runner_id,
            )
        }
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::Clear => {
            persist_im_reasoning_effort_override(
                session_key,
                &effective.settings.adapter,
                &effective.runner_id,
                None,
            );
            persist_im_model_system_message(
                session_key,
                &effective.settings.adapter,
                &effective.runner_id,
                "清除 Reasoning Effort 切换",
            );
            format!(
                "已清除 {adapter_label} Runner `{}` 的 session Reasoning Effort override。下一条消息将使用 Runner 配置或 {adapter_label} 默认值。",
                effective.runner_id
            )
        }
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::Set(effort) => {
            match crate::im_gateway::external_cli::validate_external_cli_effort_selection_for_model(
                &effective.settings.adapter,
                &effort,
                resolved_model_config.model.as_deref(),
                &model_catalog,
            ) {
                Ok(effort) => {
                    persist_im_reasoning_effort_override(
                        session_key,
                        &effective.settings.adapter,
                        &effective.runner_id,
                        Some(effort.clone()),
                    );
                    persist_im_model_system_message(
                        session_key,
                        &effective.settings.adapter,
                        &effective.runner_id,
                        &format!("切换 Reasoning Effort 为 {effort}"),
                    );
                    format!(
                        "已将 {adapter_label} Runner `{}` 的 session Reasoning Effort 设置为 `{}`。下一条消息会使用该推理强度启动。",
                        effective.runner_id, effort
                    )
                }
                Err(response) => response,
            }
        }
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

fn persist_im_model_system_message(
    session_key: &str,
    adapter: &str,
    runner_id: &str,
    message: &str,
) {
    let message = message.trim();
    if message.is_empty() {
        return;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    if let Err(error) =
        crate::im_gateway::session_state::upsert_session_state(
            session_key,
            adapter,
            Some(runner_id),
            |state| {
                if state.messages.last().is_some_and(|existing| {
                    existing.role == "system" && existing.content == message
                }) {
                    return;
                }
                state
                    .messages
                    .push(crate::im_gateway::session_state::ImAgentSessionMessage {
                        role: "system".to_string(),
                        content: message.to_string(),
                        timestamp: Some(timestamp),
                        content_parts: None,
                    });
            },
        )
    {
        warn!(
            session_key = %session_key,
            adapter = %adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist IM model system message"
        );
    }
}

fn persist_im_model_override(
    session_key: &str,
    adapter: &str,
    runner_id: &str,
    model: Option<String>,
) {
    let source = model.as_ref().map(|_| "session slash command".to_string());
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        adapter,
        Some(runner_id),
        |state| {
            state.model_override = model;
            state.model_override_source = source;
        },
    ) {
        warn!(
            session_key = %session_key,
            adapter = %adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist IM external CLI model override"
        );
    }
}

fn persist_im_reasoning_effort_override(
    session_key: &str,
    adapter: &str,
    runner_id: &str,
    effort: Option<String>,
) {
    let source = effort.as_ref().map(|_| "session slash command".to_string());
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        adapter,
        Some(runner_id),
        |state| {
            state.reasoning_effort_override = effort;
            state.reasoning_effort_override_source = source;
        },
    ) {
        warn!(
            session_key = %session_key,
            adapter = %adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist IM external CLI reasoning effort override"
        );
    }
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

    if handle_im_effort_command(
        trimmed,
        session_key,
        ctx.agent_config,
        ImModelCommandContext {
            client,
            provider,
            external_cli_config_store: ctx.external_cli_config_store,
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
        let images = match event.message.as_ref() {
            Some(event_message) if !event_message.images.is_empty() => {
                resolve_event_images(client, provider, event, &event_message.images).await
            }
            _ => Vec::new(),
        };
        match queue_manager.push_queue_with_images(session_key, queue_text.to_string(), images) {
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
                .is_some_and(|message| !message.images.is_empty())
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

/// Handle an inbound event while an external runner is active.
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
        return;
    }
    if provider.provider_type == ImProviderType::Feishu {
        if let Some(ref owner_id) = provider.owner_open_id {
            if event.source.user_id.as_deref().unwrap_or("") != owner_id {
                return;
            }
        }
    }
    if let Err(error) = event_store.add(event.clone()) {
        error!(error = %error, "failed to store concurrent event");
    }
    let message = match event.message.as_ref() {
        Some(message) if !message.text.trim().is_empty() || !message.images.is_empty() => message,
        _ => return,
    };
    let message_text = agent_message_text(message);
    let session_key = build_session_key(&event.provider_id, event.source.user_id.as_deref());
    let agent_config = effective_agent_config_for_provider(&agent_config_store.load(), &provider);
    if session_key == active_session_key {
        if message_text.trim() == "/help" {
            let config = external_cli_config_store.load();
            let response = build_im_help_text_for_agent_config(
                &agent_config,
                &config,
                Some(provider.id.as_str()),
            );
            send_agent_reply(client, &provider, event, &response, message_log_store).await;
            return;
        }
        handle_busy_message(
            &message_text,
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
    } else if agent_session_manager.is_session_active(&session_key) {
        let busy_default_mode = busy_default_mode_for_agent_config(
            &agent_config,
            &external_cli_config_store.load(),
            Some(provider.id.as_str()),
        );
        handle_busy_message(
            &message_text,
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
                default_mode: busy_default_mode,
                status_context: status_context_from_agent_config(&agent_config),
                default_work_dir: Some(agent_config.resolve_work_dir().display().to_string()),
            },
        )
        .await;
    } else {
        let _ = queue_manager.push_queue(&session_key, message_text);
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
