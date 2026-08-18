use super::*;

pub(super) struct ImModelCommandContext<'a> {
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) external_cli_config_store:
        &'a Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub(super) group_context_store: &'a Arc<ImGroupContextStore>,
    pub(super) event: &'a ImEvent,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) active_session: bool,
}

pub(super) async fn handle_im_resume_command(
    message: &str,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    ctx: ImModelCommandContext<'_>,
) -> bool {
    let Some(command) =
        crate::im_gateway::external_cli::parse_external_cli_resume_slash_command(message)
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
    let Some(configured_runner_id) =
        configured_runner_id_for_im_session(ctx.group_context_store, session_key, agent_config)
    else {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "/resume 当前仅支持 Codex、Traex 或 Claude Code Runner。请先用 `/runner` 切换。",
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
    let adapter = effective.settings.adapter.clone();
    let should_persist = matches!(
        &command,
        crate::im_gateway::external_cli::ExternalCliResumeSlashCommand::Pick(_)
            | crate::im_gateway::external_cli::ExternalCliResumeSlashCommand::New
    );
    if matches!(
        &command,
        crate::im_gateway::external_cli::ExternalCliResumeSlashCommand::List
    ) && ctx.provider.provider_type == ImProviderType::Feishu
    {
        send_feishu_resume_choice(&adapter, &ctx).await;
        return true;
    }
    let selection = crate::im_gateway::external_cli::LocalSessionSelectionContext {
        session_key: session_key.to_string(),
        runner_id: effective.runner_id.clone(),
    };
    let reply = match crate::im_gateway::external_cli::execute_local_session_resume_command(
        adapter.clone(),
        command,
        Some(selection),
    )
    .await
    {
        Ok(reply) => reply,
        Err(error) => format!("❌ {error}"),
    };
    if should_persist && !reply.starts_with('❌') {
        persist_im_model_system_message(session_key, &adapter, &effective.runner_id, &reply);
    }
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

pub(super) async fn handle_im_model_command(
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
    let Some(configured_runner_id) =
        configured_runner_id_for_im_session(ctx.group_context_store, session_key, agent_config)
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
            let status = crate::im_gateway::external_cli::format_external_cli_model_status(
                &effective.settings.adapter,
                model.as_deref(),
                source.as_deref(),
                &effective.runner_id,
            );
            if ctx.provider.provider_type == ImProviderType::Feishu {
                match send_feishu_model_choice(&effective, &status, adapter_label, &ctx).await {
                    FeishuChoiceDelivery::Sent => return true,
                    FeishuChoiceDelivery::Fallback(reply) => reply,
                }
            } else {
                status
            }
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
            let mut reply = format!(
                "已清除 {adapter_label} Runner `{}` 的 session 模型 override。下一条消息将使用 Runner 配置或 {adapter_label} 默认模型。",
                effective.runner_id
            );
            if ctx.active_session {
                reply.push_str(&format_active_model_update(
                    session_key,
                    None,
                    crate::im_gateway::external_cli::request_managed_session_model_update(
                        session_key,
                        None,
                    )
                    .await,
                ));
            }
            reply
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
                            let mut reply = format!(
                                "已将 {adapter_label} Runner `{}` 的 session 模型设置为 `{}`。\n下一条消息会通过 `--model {}` 启动。",
                                effective.runner_id, model, model
                            );
                            persist_im_model_system_message(
                                session_key,
                                &effective.settings.adapter,
                                &effective.runner_id,
                                &format!("切换模型为 {model}"),
                            );
                            if ctx.active_session {
                                reply.push_str(
                                    &format_active_model_update(
                                        session_key,
                                        Some(model.clone()),
                                        crate::im_gateway::external_cli::request_managed_session_model_update(
                                            session_key,
                                            Some(model.clone()),
                                        )
                                        .await,
                                    ),
                                );
                            }
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

fn format_active_model_update(
    session_key: &str,
    model: Option<String>,
    result: Result<crate::im_gateway::external_cli::ExternalCliModelUpdateResult, String>,
) -> String {
    let target = model
        .as_deref()
        .map(|model| format!("`{model}`"))
        .unwrap_or_else(|| "Runner 默认模型".to_string());
    match result {
        Ok(result) if result.accepted => format!(
            "\n✅ 运行中的 session `{session_key}` 已确认切换到 {target}；该设置对后续响应/轮次生效，当前已经发出的生成不会重启。"
        ),
        Ok(result) => format!(
            "\n⚠️ session 配置已保存，但运行中的 Runner 未确认热切换（{}）。下一次启动仍会使用新配置。",
            result.reason.as_deref().unwrap_or("未返回原因")
        ),
        Err(error) => format!(
            "\n⚠️ session 配置已保存，但运行中的 Runner 热切换失败（{error}）。下一次启动仍会使用新配置。"
        ),
    }
}

pub(super) async fn handle_im_effort_command(
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
    let Some(configured_runner_id) =
        configured_runner_id_for_im_session(ctx.group_context_store, session_key, agent_config)
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
            let status = crate::im_gateway::external_cli::format_external_cli_effort_status(
                &effective.settings.adapter,
                effort.as_deref(),
                source.as_deref(),
                &effective.runner_id,
            );
            if ctx.provider.provider_type == ImProviderType::Feishu {
                match send_feishu_effort_choice(
                    &effective,
                    &resolved_model_config,
                    &model_catalog,
                    &status,
                    &ctx,
                )
                .await
                {
                    FeishuChoiceDelivery::Sent => return true,
                    FeishuChoiceDelivery::Fallback(reply) => reply,
                }
            } else {
                status
            }
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

pub(super) async fn handle_im_fast_command(
    message: &str,
    session_key: &str,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    ctx: ImModelCommandContext<'_>,
) -> bool {
    let Some(parsed_command) =
        crate::im_gateway::external_cli::parse_external_cli_fast_slash_command(message)
    else {
        return false;
    };
    let config = ctx.external_cli_config_store.load();
    let Some(configured_runner_id) =
        configured_runner_id_for_im_session(ctx.group_context_store, session_key, agent_config)
    else {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "当前 Runner 不支持 `/fast` 命令；该命令仅支持 Codex Runner。请先用 `/runner Codex` 切换。",
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
    if !crate::im_gateway::external_cli::supports_external_cli_fast_slash(
        &effective.settings.adapter,
    ) {
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "当前 Runner 不支持 `/fast` 命令；该命令仅支持 Codex Runner。",
            ctx.message_log_store,
        )
        .await;
        return true;
    }
    let command = match parsed_command {
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
    let state = crate::im_gateway::session_state::load_session_state(
        session_key,
        &effective.settings.adapter,
        Some(&effective.runner_id),
    );
    let (current_tier, current_source) = state
        .as_ref()
        .and_then(|state| {
            state.service_tier_override.as_ref().map(|tier| {
                (
                    Some(tier.clone()),
                    state
                        .service_tier_override_source
                        .clone()
                        .or_else(|| Some("session slash command".to_string())),
                )
            })
        })
        .unwrap_or_else(|| {
            crate::im_gateway::external_cli::resolve_external_cli_service_tier(
                &effective.settings.adapter,
                &effective.settings.adapter_config,
            )
        });
    let target_tier = match command {
        crate::im_gateway::external_cli::ExternalCliFastSlashCommand::Status => None,
        crate::im_gateway::external_cli::ExternalCliFastSlashCommand::On => {
            Some(crate::im_gateway::external_cli::CODEX_FAST_SERVICE_TIER)
        }
        crate::im_gateway::external_cli::ExternalCliFastSlashCommand::Off => {
            Some(crate::im_gateway::external_cli::CODEX_STANDARD_SERVICE_TIER)
        }
        crate::im_gateway::external_cli::ExternalCliFastSlashCommand::Toggle => {
            if current_tier.as_deref()
                == Some(crate::im_gateway::external_cli::CODEX_FAST_SERVICE_TIER)
            {
                Some(crate::im_gateway::external_cli::CODEX_STANDARD_SERVICE_TIER)
            } else {
                Some(crate::im_gateway::external_cli::CODEX_FAST_SERVICE_TIER)
            }
        }
    };
    let reply = if let Some(target_tier) = target_tier {
        if let Err(reason) = persist_im_service_tier_override(
            session_key,
            &effective.settings.adapter,
            &effective.runner_id,
            target_tier.to_string(),
        ) {
            send_agent_reply(
                ctx.client,
                ctx.provider,
                ctx.event,
                &format!("❌ 无法切换 Codex Fast 模式：{reason}"),
                ctx.message_log_store,
            )
            .await;
            return true;
        }
        let mode = if target_tier == crate::im_gateway::external_cli::CODEX_FAST_SERVICE_TIER {
            "快速模式"
        } else {
            "标准模式"
        };
        persist_im_model_system_message(
            session_key,
            &effective.settings.adapter,
            &effective.runner_id,
            &format!("切换 Codex 为{mode}"),
        );
        format!(
            "已将 Codex Runner `{}` 切换到{mode}（service tier: `{target_tier}`）。下一条消息生效。",
            effective.runner_id
        )
    } else {
        crate::im_gateway::external_cli::format_external_cli_fast_status(
            current_tier.as_deref(),
            current_source.as_deref(),
            &effective.runner_id,
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

fn persist_im_service_tier_override(
    session_key: &str,
    adapter: &str,
    runner_id: &str,
    service_tier: String,
) -> Result<(), String> {
    crate::im_gateway::session_state::upsert_session_state(
        session_key,
        adapter,
        Some(runner_id),
        |state| {
            state.service_tier_override = Some(service_tier);
            state.service_tier_override_source = Some("session slash command".to_string());
        },
    )
    .map(|_| ())
}

#[cfg(test)]
mod live_model_reply_tests {
    use super::*;

    fn result(
        accepted: bool,
        reason: Option<&str>,
    ) -> crate::im_gateway::external_cli::ExternalCliModelUpdateResult {
        crate::im_gateway::external_cli::ExternalCliModelUpdateResult {
            update_id: "update-test".to_string(),
            model: Some("gpt-test".to_string()),
            accepted,
            thread_id: accepted.then(|| "thread-test".to_string()),
            reason: reason.map(str::to_string),
        }
    }

    #[test]
    fn active_model_update_reply_covers_success_rejection_and_transport_error() {
        let accepted = format_active_model_update(
            "session-test",
            Some("gpt-test".to_string()),
            Ok(result(true, None)),
        );
        assert!(accepted.contains("`gpt-test`"));
        assert!(accepted.contains("后续响应/轮次生效"));

        let rejected = format_active_model_update(
            "session-test",
            None,
            Ok(result(false, Some("runner rejected"))),
        );
        assert!(rejected.contains("runner rejected"));

        let rejected_without_reason =
            format_active_model_update("session-test", None, Ok(result(false, None)));
        assert!(rejected_without_reason.contains("未返回原因"));

        let failed = format_active_model_update(
            "session-test",
            Some("gpt-test".to_string()),
            Err("broker unavailable".to_string()),
        );
        assert!(failed.contains("broker unavailable"));
    }
}
