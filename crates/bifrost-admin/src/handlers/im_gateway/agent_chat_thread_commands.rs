use super::*;

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

pub(super) struct ImCwdCommandContext<'a> {
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) provider_store: &'a Arc<ImProviderStore>,
    pub(super) group_context_store: &'a Arc<ImGroupContextStore>,
    pub(super) event: &'a ImEvent,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) session_manager: &'a Arc<ImAgentSessionManager>,
}

pub(super) struct ImRunnerCommandContext<'a> {
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) provider_store: &'a Arc<ImProviderStore>,
    pub(super) group_context_store: &'a Arc<ImGroupContextStore>,
    pub(super) external_cli_config_store:
        &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub(super) event: &'a ImEvent,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) session_manager: &'a Arc<ImAgentSessionManager>,
    pub(super) agent_config: &'a crate::im_gateway::agent::ImAgentConfig,
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

pub(super) fn format_effective_im_runner(
    group_context_store: &ImGroupContextStore,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    provider_id: &str,
) -> String {
    let configured =
        configured_runner_id_for_im_session(group_context_store, session_key, agent_config);
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        config,
        Some(provider_id),
        configured.as_deref(),
    );
    format!("当前 Runner：`{}`", effective.runner_id)
}

pub(super) fn format_effective_im_work_dir(
    group_context_store: &ImGroupContextStore,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
) -> String {
    let work_dir = group_context_store
        .work_dir_by_session(session_key)
        .ok()
        .flatten()
        .unwrap_or_else(|| agent_config.resolve_work_dir());
    format!("当前线程工作目录：\n`{}`", work_dir.display())
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
    group_context_store: &Arc<ImGroupContextStore>,
    provider_id: &str,
    session_key: &str,
    session: &mut bifrost_agent::AgentSession,
    selection: &ImRunnerSelection,
) -> String {
    match group_context_store.set_runner_id_by_session(session_key, &selection.runner_id) {
        Ok(true) => {}
        Ok(false) => {
            persist_provider_agent_runner(provider_store, provider_id, selection.runner.clone())
        }
        Err(error) => warn!(
            session_key = %session_key,
            error = %error,
            "failed to persist group runner binding"
        ),
    }
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
    group_context_store: &Arc<ImGroupContextStore>,
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
        group_context_store,
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

pub(super) async fn handle_im_runner_command(
    message: &str,
    session_key: &str,
    ctx: ImRunnerCommandContext<'_>,
) -> bool {
    let Some(command) = parse_im_runner_command(message) else {
        return false;
    };
    let config = ctx.external_cli_config_store.load();
    let reply = match command {
        ImRunnerCommand::List => {
            let status = format_effective_im_runner(
                ctx.group_context_store,
                session_key,
                ctx.agent_config,
                &config,
                &ctx.provider.id,
            );
            if ctx.provider.provider_type == ImProviderType::Feishu {
                let options = runner_choice_options(&config);
                if send_feishu_choice_card(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &format!("{status}\n\n请选择 Runner："),
                    options,
                    ctx.message_log_store,
                )
                .await
                {
                    return true;
                }
                format!(
                    "{status}\n\n支持的 Runner：\n{}",
                    format_im_runner_list(&config)
                )
            } else {
                status
            }
        }
        ImRunnerCommand::Switch(runner_id) => match apply_im_runner_switch(
            ctx.provider_store,
            ctx.group_context_store,
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

pub(super) fn configured_runner_id_for_im_session(
    group_context_store: &ImGroupContextStore,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
) -> Option<String> {
    group_context_store
        .runner_id_by_session(session_key)
        .ok()
        .flatten()
        .or_else(|| {
            agent_config
                .runner
                .as_ref()
                .and_then(|runner| runner.custom_runner_id())
                .map(ToString::to_string)
        })
}

pub(super) fn apply_im_cwd_switch_to_session(
    provider_store: &Arc<ImProviderStore>,
    group_context_store: &Arc<ImGroupContextStore>,
    provider_id: &str,
    session_key: &str,
    session: &mut bifrost_agent::AgentSession,
    work_dir: &Path,
) -> String {
    let canonical_work_dir =
        std::fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
    let work_dir = canonical_work_dir.display().to_string();
    match group_context_store.set_work_dir_by_session(session_key, &work_dir) {
        Ok(true) => {}
        Ok(false) => persist_provider_agent_work_dir(provider_store, provider_id, &work_dir),
        Err(error) => warn!(
            session_key = %session_key,
            error = %error,
            "failed to persist group work directory"
        ),
    }
    clear_persisted_agent_session_state(session_key, None, None);
    session.reinitialize_work_dir(work_dir.clone());
    format!("已切换工作目录到:\n`{work_dir}`\n\n下一条消息将使用新的工作目录。")
}

pub(super) fn apply_im_cwd_switch(
    provider_store: &Arc<ImProviderStore>,
    group_context_store: &Arc<ImGroupContextStore>,
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
        group_context_store,
        provider_id,
        session_key,
        &mut session,
        &canonical_work_dir,
    );
    session_manager.return_session(session);
    Ok(reply)
}

pub(super) async fn handle_im_cwd_command(
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
            ctx.group_context_store,
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
