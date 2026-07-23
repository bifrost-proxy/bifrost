use super::*;

pub(super) struct AbortTaskOnDrop(pub(super) tokio::task::AbortHandle);

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn external_cli_images_from_chat_images(
    images: Vec<bifrost_agent::ChatImageInput>,
) -> Vec<crate::im_gateway::external_cli::ExternalCliImageInput> {
    images
        .into_iter()
        .map(
            |image| crate::im_gateway::external_cli::ExternalCliImageInput {
                mime_type: image.mime_type,
                data: image.data,
                name: None,
            },
        )
        .collect()
}

pub(super) fn event_for_queue_item(
    base: &ImEvent,
    context: Option<&crate::im_gateway::queue_manager::QueueItemContext>,
) -> ImEvent {
    let mut event = base.clone();
    let Some(context) = context else {
        return event;
    };
    if !context.event_id.is_empty() {
        event.event_id = context.event_id.clone();
    }
    event.source.message_id = context.message_id.clone();
    event.source.user_id = context.user_id.clone();
    event.source.user_name = context.user_name.clone();
    event
}

pub(super) fn apply_session_bound_work_dir(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    session_work_dir: Option<&str>,
    fallback: Option<std::path::PathBuf>,
) {
    let runner_work_dir = request.work_dir.take();
    request.work_dir = session_work_dir
        .map(std::path::PathBuf::from)
        .or(runner_work_dir)
        .or(fallback);
}

pub(in crate::handlers::im_gateway) fn resolve_external_cli_delivery_mode(
    provider: &ImProviderConfig,
    settings: &crate::im_gateway::external_cli::ExternalCliAgentSettings,
    sources: &std::collections::BTreeMap<String, String>,
    input_override: Option<crate::im_gateway::external_cli::ExternalCliDeliveryMode>,
) -> crate::im_gateway::external_cli::ExternalCliDeliveryMode {
    if let Some(delivery_mode) = input_override {
        return delivery_mode;
    }
    if provider.provider_type == ImProviderType::Feishu
        && is_im_progress_card_external_adapter(&settings.adapter)
        && sources.get("deliveryMode").map(String::as_str) != Some("channel")
    {
        return crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard;
    }
    settings.delivery_mode
}

pub(super) fn is_im_progress_card_external_adapter(adapter: &str) -> bool {
    matches!(
        adapter,
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    )
}

pub(in crate::handlers::im_gateway) fn finalize_live_guide_group_turns(
    queue_manager: &SessionQueueManager,
    group_context_store: &ImGroupContextStore,
    session_key: &str,
    result: Result<(), &str>,
) {
    for turn_id in queue_manager.take_live_guide_turns(session_key) {
        let status_result = match result {
            Ok(()) => group_context_store.mark_turn_completed(&turn_id, now_ms()),
            Err(error) => group_context_store.mark_turn_failed(&turn_id, error, now_ms()),
        };
        if let Err(error) = status_result {
            warn!(turn_id = %turn_id, error = %error, "failed to finalize live guide group turn");
        }
    }
}

pub(super) async fn run_external_cli_agent_chat(
    ctx: ExternalCliChatContext<'_>,
    input: ExternalCliChatInput,
) {
    let mut current_group_turn_id = input.group_turn_id.clone();
    let config = ctx.external_cli_config_store.load();
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        Some(&ctx.provider.id),
        input.runner_id_override.as_deref(),
    );
    let mut settings = effective.settings;
    if let Some(adapter) = input
        .adapter_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        settings.adapter = adapter.to_string();
    }
    if let Some(instructions) = input
        .instructions_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        settings.instructions = match settings.instructions.take() {
            Some(existing) if !existing.trim().is_empty() => {
                Some(format!("{}\n\n{}", existing.trim(), instructions))
            }
            _ => Some(instructions.to_string()),
        };
    }

    // Intercept /clear and /reset after resolving the effective runner so the
    // reset only clears the current adapter/runner rather than every agent that
    // happens to share the same IM session key.
    let trimmed_msg = input.message_text.trim();
    if trimmed_msg == "/help" {
        let response = build_im_startup_help_for_runner(&ImHelpRunnerKind::External {
            adapter: settings.adapter.clone(),
        });
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            &response,
            ctx.message_log_store,
        )
        .await;
        return;
    }

    if trimmed_msg == "/clear" || trimmed_msg == "/reset" {
        let _ = request_agent_stop(ctx.agent_session_manager, &input.session_key).await;
        if let Some(mut session) = ctx
            .agent_session_manager
            .try_take_session(&input.session_key)
        {
            session.clear();
            ctx.agent_session_manager.return_session(session);
        } else {
            ctx.agent_session_manager.clear_session(&input.session_key);
        }
        ctx.queue_manager.clear_session(&input.session_key);
        if settings.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
            crate::im_gateway::chatgpt_web::clear_session_conversation(&input.session_key).await;
        }
        clear_persisted_agent_session_state(
            &input.session_key,
            Some(&settings.adapter),
            Some(&effective.runner_id),
        );
        if input.reset_group_context {
            if let Err(error) = ctx.group_context_store.advance_context_baseline(ctx.event) {
                warn!(
                    provider_id = %ctx.provider.id,
                    session_key = %input.session_key,
                    error = %error,
                    "session reset succeeded but group context baseline could not be advanced"
                );
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &format!("会话已重置，但群上下文基线更新失败：{error}"),
                    ctx.message_log_store,
                )
                .await;
                return;
            }
        }
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "会话已重置，下一条消息将开始新的对话。",
            ctx.message_log_store,
        )
        .await;
        return;
    }

    if !settings.enabled && !input.runner_selected {
        if let Some(turn_id) = current_group_turn_id.take() {
            if let Err(error) = ctx.group_context_store.release_turn(
                &turn_id,
                "Runner is not enabled for this IM channel",
                now_ms(),
            ) {
                warn!(turn_id = %turn_id, error = %error, "failed to release group turn for disabled runner");
            }
        }
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "Runner is not enabled for this IM channel.",
            ctx.message_log_store,
        )
        .await;
        return;
    }

    let persisted_state = crate::im_gateway::session_state::load_session_state(
        &input.session_key,
        &settings.adapter,
        Some(&effective.runner_id),
    );
    let delivery_mode = resolve_external_cli_delivery_mode(
        ctx.provider,
        &settings,
        &effective.sources,
        input.delivery_override,
    );
    let mut resolved_model_config =
        crate::im_gateway::external_cli::resolve_external_cli_model_config(
            &settings.adapter,
            &settings.adapter_config,
        );
    crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_model_config(
        &settings.adapter,
        persisted_state.as_ref(),
        &mut resolved_model_config,
    );
    let mut status_context =
        status_context_from_external_runner(&effective.runner_id, &settings.adapter);
    if let Some(model) = resolved_model_config.model.clone() {
        status_context.model = Some(model);
    }
    status_context.model_provider = resolved_model_config
        .model_provider
        .clone()
        .or_else(|| resolved_model_config.model_source.clone());
    status_context.model_reasoning_effort = resolved_model_config.reasoning_effort.clone();
    status_context.model_reasoning_summary = resolved_model_config.reasoning_summary.clone();

    let provider_agent_config =
        effective_agent_config_for_provider(&ctx.agent_config_store.load(), ctx.provider);
    let mut session_work_dir = ctx
        .group_context_store
        .work_dir_by_session(&input.session_key)
        .ok()
        .flatten()
        .map(|path| path.display().to_string())
        .or_else(|| provider_agent_config.work_dir.clone());
    let Some(mut session) = ctx
        .agent_session_manager
        .try_take_session_with_work_dir(&input.session_key, session_work_dir.clone())
    else {
        handle_busy_message(
            &input.message_text,
            &input.session_key,
            BusyMessageContext {
                queue_manager: ctx.queue_manager,
                client: ctx.client,
                provider: ctx.provider,
                event: ctx.event,
                message_log_store: ctx.message_log_store,
                agent_session_manager: ctx.agent_session_manager,
                progress_registry: ctx.progress_registry,
                external_cli_config_store: ctx.external_cli_config_store,
                agent_config: &provider_agent_config,
                group_context_store: ctx.group_context_store,
                group_turn_id: input.group_turn_id.as_deref(),
                default_mode: busy_default_mode_for_external_adapter(&settings.adapter),
                status_context,
                default_work_dir: session_work_dir.or_else(|| {
                    Some(
                        provider_agent_config
                            .resolve_work_dir()
                            .display()
                            .to_string(),
                    )
                }),
            },
        )
        .await;
        return;
    };

    let guide_channel = ctx
        .queue_manager
        .get_or_create_guide_channel(&input.session_key);
    restore_session_from_persisted_history(
        &mut session,
        &input.session_key,
        &settings.adapter,
        Some(&effective.runner_id),
        provider_agent_config
            .history
            .as_ref()
            .and_then(|h| h.max_bytes),
    );
    let mut current_message = input.message_text;
    let mut current_images = input.images;
    let mut current_event = ctx.event.clone();
    let mut recorder = session.recorder.take();
    let mut runner_metadata = persisted_state
        .as_ref()
        .map(crate::im_gateway::session_state::metadata_from_state)
        .unwrap_or_default();

    loop {
        if let Some(command) = parse_im_cwd_command(&current_message) {
            let reply = match command {
                Ok(path) => apply_im_cwd_switch_to_session(
                    ctx.provider_store,
                    ctx.group_context_store,
                    &ctx.provider.id,
                    &input.session_key,
                    &mut session,
                    &path,
                ),
                Err(reason) => format_im_cwd_error(&reason),
            };
            if session.work_dir.is_some() {
                session_work_dir = session.work_dir.clone();
            }
            send_agent_reply(
                ctx.client,
                ctx.provider,
                &current_event,
                &reply,
                ctx.message_log_store,
            )
            .await;
            let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
            if let Some(unconsumed) =
                bifrost_agent::session::combine_guide_messages(unconsumed_guides)
            {
                if !unconsumed.trim().is_empty() {
                    let _ = ctx.queue_manager.push_queue(&input.session_key, unconsumed);
                }
            }
            match ctx.queue_manager.pop_queue_item(&input.session_key) {
                Some(next_item) => {
                    current_group_turn_id = next_item
                        .context
                        .as_ref()
                        .and_then(|context| context.group_turn_id.clone());
                    current_event = event_for_queue_item(ctx.event, next_item.context.as_ref());
                    current_message = next_item.message;
                    current_images = external_cli_images_from_chat_images(next_item.images);
                    continue;
                }
                None => break,
            }
        }

        let mut request = crate::im_gateway::external_cli::run_request_from_settings(
            current_message.clone(),
            Some(ctx.provider.id.clone()),
            Some(input.session_key.clone()),
            &settings,
        );
        crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_run_request(
            &mut request,
            persisted_state.as_ref(),
        );
        request.images = std::mem::take(&mut current_images);
        apply_external_cli_resume_metadata(&mut request, &runner_metadata);
        apply_session_bound_work_dir(
            &mut request,
            session_work_dir.as_deref(),
            effective_agent_work_dir_for_provider(&ctx.agent_config_store.load(), ctx.provider),
        );
        // Graceful fallback: if the configured work_dir doesn't exist on disk,
        // clear it so the runner uses a default directory instead of failing.
        if let Some(ref work_dir) = request.work_dir {
            if !work_dir.exists() {
                tracing::warn!(
                    work_dir = %work_dir.display(),
                    provider_id = %ctx.provider.id,
                    "agent work_dir does not exist, falling back to default"
                );
                request.work_dir = None;
            }
        }
        if request.allow_work_dirs.is_empty() {
            if let Some(work_dir) = request.work_dir.as_ref() {
                request.allow_work_dirs = vec![work_dir.display().to_string()];
            }
        }
        session.remember_runner_model_config(
            resolved_model_config.model.clone(),
            resolved_model_config
                .model_provider
                .clone()
                .or_else(|| resolved_model_config.model_source.clone()),
            resolved_model_config.reasoning_effort.clone(),
            resolved_model_config.reasoning_summary.clone(),
        );
        ensure_external_cli_session_recorder(
            &mut session,
            &mut recorder,
            &input.session_key,
            ctx.provider,
            &effective.runner_id,
            &request,
        );
        apply_external_cli_session_attachment_base_dir(&mut request, recorder.as_ref());
        record_external_cli_input(
            &mut session,
            &mut recorder,
            &input.session_key,
            &effective.runner_id,
            &request,
        );
        emit_external_cli_timeline_changed(
            ctx.agent_session_manager,
            recorder.as_ref(),
            &input.session_key,
            "im_turn_started",
        );
        let mut progress_enabled = false;
        let mut progress_runner_metadata = std::collections::BTreeMap::new();
        let mut progress_tx_for_finish = None;
        let mut progress_task = None;
        if matches!(
            delivery_mode,
            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
        ) {
            if let (Some(progress_target), Some(feishu)) = (
                build_agent_reply_target(
                    ctx.provider,
                    &current_event,
                    "__agent_progress__",
                    "Agent Progress",
                    "interactive",
                ),
                ctx.client.feishu(),
            ) {
                let progress_result = if ctx
                    .progress_registry
                    .rollover_existing_replying_to(
                        &input.session_key,
                        &current_message,
                        current_event.source.message_id.as_deref(),
                    )
                    .await
                {
                    Ok(())
                } else {
                    ctx.progress_registry
                        .start_feishu_replying_to(
                            &input.session_key,
                            feishu,
                            ctx.provider.clone(),
                            progress_target,
                            &current_message,
                            current_event.source.message_id.as_deref(),
                        )
                        .await
                        .map(|_| ())
                };
                match progress_result {
                    Ok(_) => {
                        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<
                            bifrost_agent::AgentTurnProgressEvent,
                        >();
                        progress_tx_for_finish = Some(progress_tx.clone());
                        let progress_registry = Arc::clone(ctx.progress_registry);
                        let session_key_for_progress = input.session_key.clone();
                        progress_task = Some(tokio::spawn(async move {
                            super::super::agent_chat_progress::run_progress_event_coalescer(
                                progress_registry,
                                session_key_for_progress,
                                &mut progress_rx,
                            )
                            .await;
                        }));
                        progress_enabled = true;
                        let runner_summary = external_cli_progress_runner_summary(
                            &effective.runner_id,
                            &settings.adapter,
                            &request,
                            None,
                        );
                        let _ = ctx
                            .progress_registry
                            .update_runner_summary(&input.session_key, runner_summary)
                            .await;
                    }
                    Err(error) => {
                        warn!(
                            session_key = %input.session_key,
                            error = %error,
                            "failed to start external runner progress card; final reply will be sent when the run finishes"
                        );
                    }
                }
            }
        }
        let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
            crate::im_gateway::external_cli::default_runs_root(),
        );
        let (external_progress_tx, mut external_progress_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let request_for_progress = request.clone();
        // Keep the runner control loop independently polled while this task
        // handles a default Guide message (or a legacy inbound /g). Awaiting the
        // guide acknowledgement inline otherwise stalls `run_with_progress`,
        // which is also responsible for forwarding that guide to the worker.
        let request_for_run = request.clone();
        let mut run_task = tokio::spawn(async move {
            runtime
                .run_with_progress(request_for_run, Some(external_progress_tx))
                .await
        });
        let _run_task_guard = AbortTaskOnDrop(run_task.abort_handle());
        let result = loop {
            tokio::select! {
                result = &mut run_task => break result
                    .map_err(|error| format!("external runner task failed: {error}"))
                    .and_then(|result| result),
                Some(progress_event) = external_progress_rx.recv() => {
                    if let Some(recorder) = recorder.as_mut() {
                        if let Some(end_index) = super::super::chat_gateway::record_external_cli_progress_event_to_timeline(
                            recorder,
                            &input.session_key,
                            "im",
                            &effective.runner_id,
                            &settings.adapter,
                            &progress_event,
                        ) {
                            ctx.agent_session_manager.emit_timeline_changed(
                                &input.session_key,
                                &recorder.file_path().display().to_string(),
                                Some(end_index),
                                "im_progress",
                            );
                        }
                    }
                    if progress_enabled
                        && crate::im_gateway::external_cli::merge_external_cli_progress_metadata(
                            &settings.adapter,
                            &progress_event,
                            &mut progress_runner_metadata,
                        )
                    {
                        let runner_summary = external_cli_progress_runner_summary(
                            &effective.runner_id,
                            &settings.adapter,
                            &request_for_progress,
                            Some(&progress_runner_metadata),
                        );
                        let _ = ctx
                            .progress_registry
                            .update_runner_summary(&input.session_key, runner_summary)
                            .await;
                    }
                    if progress_enabled {
                        if let (Some(progress_tx), Some(agent_event)) = (
                            progress_tx_for_finish.as_ref(),
                            crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
                                &input.session_key,
                                &settings.adapter,
                                crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                                    Some(&effective.runner_id),
                                    resolved_model_config.model.as_deref(),
                                    resolved_model_config
                                        .model_provider
                                        .as_deref()
                                        .or(resolved_model_config.model_source.as_deref()),
                                    resolved_model_config.reasoning_effort.as_deref(),
                                    resolved_model_config.reasoning_summary.as_deref(),
                                    request_for_progress.work_dir.as_deref(),
                                ),
                                &progress_event,
                            ),
                        ) {
                            let _ = progress_tx.send(agent_event);
                        }
                    }
                }
                Some(next_event) = ctx.rx.recv() => {
                    maybe_stop_external_cli_for_event(&next_event, &input.session_key).await;
                    handle_concurrent_event_during_chat(
                        &next_event,
                        ctx.provider,
                        &input.session_key,
                        ctx.queue_manager,
                        ctx.client,
                        ctx.message_log_store,
                        ctx.agent_session_manager,
                        ctx.progress_registry,
                        ctx.agent_config_store,
                        ctx.provider_store,
                        ctx.event_store,
                        ctx.group_context_store,
                        ctx.external_cli_config_store,
                        busy_default_mode_for_external_adapter(&settings.adapter),
                    ).await;
                }
            }
        };
        while let Ok(progress_event) = external_progress_rx.try_recv() {
            if let Some(recorder) = recorder.as_mut() {
                if let Some(end_index) =
                    super::super::chat_gateway::record_external_cli_progress_event_to_timeline(
                        recorder,
                        &input.session_key,
                        "im",
                        &effective.runner_id,
                        &settings.adapter,
                        &progress_event,
                    )
                {
                    ctx.agent_session_manager.emit_timeline_changed(
                        &input.session_key,
                        &recorder.file_path().display().to_string(),
                        Some(end_index),
                        "im_progress",
                    );
                }
            }
            if progress_enabled {
                if let (Some(progress_tx), Some(agent_event)) = (
                    progress_tx_for_finish.as_ref(),
                    crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
                        &input.session_key,
                        &settings.adapter,
                        crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                            Some(&effective.runner_id),
                            resolved_model_config.model.as_deref(),
                            resolved_model_config
                                .model_provider
                                .as_deref()
                                .or(resolved_model_config.model_source.as_deref()),
                            resolved_model_config.reasoning_effort.as_deref(),
                            resolved_model_config.reasoning_summary.as_deref(),
                            request_for_progress.work_dir.as_deref(),
                        ),
                        &progress_event,
                    ),
                ) {
                    let _ = progress_tx.send(agent_event);
                }
            }
        }
        match result {
            Ok(mut result) => {
                let run_succeeded = matches!(
                    result.status,
                    crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
                );
                if !run_succeeded {
                    let failure_reply = external_cli_non_success_reply(&result);
                    result.response = failure_reply.clone();
                    result.responses = vec![failure_reply];
                }
                if let Some(turn_id) = current_group_turn_id.take() {
                    let status_result = if run_succeeded {
                        ctx.group_context_store
                            .mark_turn_completed(&turn_id, now_ms())
                    } else {
                        ctx.group_context_store.mark_turn_failed(
                            &turn_id,
                            &result.response,
                            now_ms(),
                        )
                    };
                    if let Err(error) = status_result {
                        warn!(turn_id = %turn_id, error = %error, "failed to finalize group turn");
                    }
                }
                finalize_live_guide_group_turns(
                    ctx.queue_manager,
                    ctx.group_context_store,
                    &input.session_key,
                    if run_succeeded {
                        Ok(())
                    } else {
                        Err(result.response.as_str())
                    },
                );
                remember_external_cli_result_metadata(&mut runner_metadata, &result.metadata);
                record_external_cli_result(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &result,
                );
                emit_external_cli_timeline_changed(
                    ctx.agent_session_manager,
                    recorder.as_ref(),
                    &input.session_key,
                    "im_turn_finished",
                );
                remember_session_state_values(
                    &input.session_key,
                    &settings.adapter,
                    Some(&effective.runner_id),
                    session.external_conversation_id.clone(),
                    session.external_thread_id.clone(),
                    recorder
                        .as_ref()
                        .map(|recorder| recorder.file_path().display().to_string()),
                    session.work_dir.clone(),
                );
                if progress_enabled {
                    let runner_summary = external_cli_progress_runner_summary(
                        &effective.runner_id,
                        &settings.adapter,
                        &request_for_progress,
                        Some(&result.metadata),
                    );
                    let _ = ctx
                        .progress_registry
                        .update_runner_summary(&input.session_key, runner_summary)
                        .await;
                    if let Some(progress_tx) = progress_tx_for_finish.take() {
                        let event = if run_succeeded {
                            bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                                content: result.response.clone(),
                            }
                        } else {
                            bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                                error: result.response.clone(),
                            }
                        };
                        let _ = progress_tx.send(event);
                        drop(progress_tx);
                    }
                    if let Some(task) = progress_task.take() {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
                    }
                    let _ = ctx
                        .progress_registry
                        .finish(
                            &input.session_key,
                            Some(result.response.clone()),
                            !run_succeeded,
                        )
                        .await;
                } else if !matches!(
                    delivery_mode,
                    crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm
                ) {
                    // Send individual response messages separately when there
                    // are multiple (e.g. ChatGPT thinking + answer).
                    let responses_to_send: Vec<String> = if result.responses.len() > 1 {
                        result
                            .responses
                            .iter()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    } else {
                        vec![result.response.trim().to_string()]
                    };
                    for (idx, reply_text) in responses_to_send.iter().enumerate() {
                        let reply = if matches!(
                            delivery_mode,
                            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
                        ) && idx == responses_to_send.len() - 1
                        {
                            // Only append run_id to the last message
                            format!("{}\n\n_run: `{}`_", reply_text, result.run_id)
                        } else {
                            reply_text.clone()
                        };
                        send_agent_reply_from_work_dir(
                            ctx.client,
                            ctx.provider,
                            &current_event,
                            &reply,
                            ctx.message_log_store,
                            request.work_dir.as_deref(),
                        )
                        .await;
                    }
                }
            }
            Err(error) => {
                if let Some(turn_id) = current_group_turn_id.take() {
                    if let Err(status_error) =
                        ctx.group_context_store
                            .mark_turn_failed(&turn_id, &error, now_ms())
                    {
                        warn!(turn_id = %turn_id, error = %status_error, "failed to mark group turn failed");
                    }
                }
                finalize_live_guide_group_turns(
                    ctx.queue_manager,
                    ctx.group_context_store,
                    &input.session_key,
                    Err(error.as_str()),
                );
                // Extract diagnostic screenshot path if present.
                let (clean_error, screenshot_path) = extract_diagnostic_screenshot_path(&error);
                let reply = format!("Runner failed: {}", truncate_str(&clean_error, 300));
                record_external_cli_failure(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &request,
                    &error,
                    &reply,
                );
                emit_external_cli_timeline_changed(
                    ctx.agent_session_manager,
                    recorder.as_ref(),
                    &input.session_key,
                    "im_turn_failed",
                );
                if progress_enabled {
                    if let Some(progress_tx) = progress_tx_for_finish.take() {
                        let _ =
                            progress_tx.send(bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                                error: reply.clone(),
                            });
                        drop(progress_tx);
                    }
                    if let Some(task) = progress_task.take() {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
                    }
                    let _ = ctx
                        .progress_registry
                        .finish(&input.session_key, Some(reply.clone()), true)
                        .await;
                } else {
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        &current_event,
                        &reply,
                        ctx.message_log_store,
                    )
                    .await;
                }
                // Send diagnostic screenshot via IM if available.
                if let Some(path) = screenshot_path {
                    if let Some(target) = build_agent_reply_target(
                        ctx.provider,
                        &current_event,
                        "__diag_screenshot__",
                        "Diagnostic Screenshot",
                        "image",
                    ) {
                        let images = vec![AgentReplyLocalImage {
                            alt: "diagnostic screenshot".to_string(),
                            path,
                        }];
                        send_agent_reply_images(
                            ctx.client,
                            ctx.provider,
                            &current_event,
                            &target,
                            &images,
                            ctx.message_log_store,
                        )
                        .await;
                    }
                }
            }
        };

        let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
        if let Some(unconsumed) = bifrost_agent::session::combine_guide_messages(unconsumed_guides)
        {
            if !unconsumed.trim().is_empty() {
                let _ = ctx.queue_manager.push_queue(&input.session_key, unconsumed);
            }
        }
        match ctx.queue_manager.pop_queue_item(&input.session_key) {
            Some(next_item) => {
                current_group_turn_id = next_item
                    .context
                    .as_ref()
                    .and_then(|context| context.group_turn_id.clone());
                current_event = event_for_queue_item(ctx.event, next_item.context.as_ref());
                if matches!(
                    delivery_mode,
                    crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
                ) {
                    let remaining = ctx.queue_manager.queue_status(&input.session_key).len();
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        &current_event,
                        &format!("开始处理排队消息，当前剩余 {remaining} 条。"),
                        ctx.message_log_store,
                    )
                    .await;
                }
                current_message = next_item.message;
                current_images = external_cli_images_from_chat_images(next_item.images);
            }
            None => break,
        };
    }
    if recorder.is_some() && !session.history_cleared {
        session.recorder = recorder;
    }
    remember_session_state_from_agent_session(
        &session,
        &settings.adapter,
        Some(&effective.runner_id),
    );
    ctx.queue_manager.clear_session(&input.session_key);
    ctx.agent_session_manager.return_session(session);
}

pub(super) fn external_cli_non_success_reply(
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) -> String {
    match result.status {
        crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut => format!(
            "Runner failed: external CLI timed out after {} seconds.\n\n_run: `{}`_",
            std::cmp::max(1, result.duration_ms / 1000),
            result.run_id
        ),
        crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped => format!(
            "Runner stopped before completion.\n\n_run: `{}`_",
            result.run_id
        ),
        crate::im_gateway::external_cli::ExternalCliRunStatus::Failed => {
            let detail = if result.response.trim().is_empty() {
                match result.exit_code {
                    Some(code) => format!("exit_code={code}"),
                    None => "external CLI exited unsuccessfully".to_string(),
                }
            } else {
                truncate_str(&result.response, 260)
            };
            format!("Runner failed: {detail}\n\n_run: `{}`_", result.run_id)
        }
        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded => result.response.clone(),
    }
}

pub(in crate::handlers::im_gateway) fn apply_external_cli_resume_metadata(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    metadata: &std::collections::BTreeMap<String, String>,
) {
    if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        if request
            .params
            .get("conversationId")
            .or_else(|| request.params.get("conversation_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            return;
        }
        let Some(conversation_id) = metadata
            .get("conversationId")
            .or_else(|| metadata.get("conversation_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        if !request.params.is_object() {
            request.params = serde_json::json!({});
        }
        if let Some(params) = request.params.as_object_mut() {
            params.insert(
                "conversationId".to_string(),
                serde_json::Value::String(conversation_id.to_string()),
            );
        }
        return;
    }
    if !matches!(
        request.adapter.as_str(),
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    ) {
        return;
    }
    if request
        .params
        .get("threadId")
        .or_else(|| request.params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return;
    }
    let Some(thread_id) = metadata
        .get("threadId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.insert(
            "threadId".to_string(),
            serde_json::Value::String(thread_id.to_string()),
        );
    }
}

pub(super) fn remember_external_cli_result_metadata(
    metadata: &mut std::collections::BTreeMap<String, String>,
    result_metadata: &std::collections::BTreeMap<String, String>,
) {
    if let Some(thread_id) = result_metadata
        .get("threadId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("threadId".to_string(), thread_id.to_string());
    }
    if let Some(conversation_id) = result_metadata
        .get("conversationId")
        .or_else(|| result_metadata.get("conversation_id"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("conversationId".to_string(), conversation_id.to_string());
    }
}

pub(super) fn ensure_external_cli_session_recorder(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    provider: &ImProviderConfig,
    runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    let adapter_label = external_cli_adapter_label(&request.adapter);
    if session.source == "unknown" {
        session.source = request.adapter.clone();
    }
    session.mark_external_runner_runtime(runner_id, &request.adapter);
    sync_external_cli_active_status(session);
    if session.title.is_none() {
        session.title = Some(format!(
            "{}: {}",
            adapter_label,
            truncate_str(request.message.trim(), 48)
        ));
    }
    if session.work_dir.is_none() {
        session.work_dir = request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string());
    }

    if recorder.is_none() {
        let data_dir = bifrost_agent::config::agent_home_dir();
        match ConversationRecorder::open_or_create(&data_dir, session_key, None) {
            Ok((mut rec, created)) => {
                if created {
                    if let Err(error) = rec.record_session_start(
                        session_key,
                        serde_json::json!({
                            "source": request.adapter,
                            "runtime": request.runtime,
                            "adapter": request.adapter,
                            "runner_id": runner_id,
                            "provider_id": provider.id,
                            "provider_type": format!("{:?}", provider.provider_type).to_lowercase(),
                            "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
                        }),
                    ) {
                        warn!(error = %error, "failed to record external cli session start");
                    }
                    if let Some(title) = session.title.as_deref() {
                        if let Err(error) = rec.record_title_updated(session_key, title) {
                            warn!(error = %error, "failed to record external cli session title");
                        }
                    }
                }
                *recorder = Some(rec);
            }
            Err(error) => {
                warn!(session_key = %session_key, error = %error, "failed to open the canonical external cli session history");
            }
        }
    }
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) =
            rec.record_run_state(session_key, "running", Some("im"), Some(runner_id))
        {
            warn!(error = %error, "failed to record external cli running state");
        }
    }
}

pub(super) fn apply_external_cli_session_attachment_base_dir(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    recorder: Option<&ConversationRecorder>,
) {
    let Some(recorder) = recorder else {
        return;
    };
    let Some(session_dir) = recorder.file_path().parent() else {
        return;
    };
    let Some(session_stem) = recorder.file_path().file_stem() else {
        return;
    };
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.remove("attachment_base_dir");
        params.insert(
            "attachmentBaseDir".to_string(),
            serde_json::Value::String(
                session_dir
                    .join("attachments")
                    .join(session_stem)
                    .display()
                    .to_string(),
            ),
        );
        params.insert(
            "historyPath".to_string(),
            serde_json::Value::String(recorder.file_path().display().to_string()),
        );
    }
}

pub(super) fn external_cli_request_chat_images(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) -> Vec<bifrost_agent::ChatImageInput> {
    request
        .images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .map(|image| bifrost_agent::ChatImageInput {
            mime_type: image.mime_type.clone(),
            data: image.data.clone(),
        })
        .collect()
}

pub(super) fn record_external_cli_input(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    _runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    let images = external_cli_request_chat_images(request);
    append_session_message(
        session,
        bifrost_agent::ChatMessage::user_with_images(&request.message, &images),
    );
    sync_external_cli_active_status(session);
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) =
            rec.record_user_message_with_images(session_key, &request.message, &images)
        {
            warn!(error = %error, "failed to record external cli user message");
        }
    }
}

pub(super) fn emit_external_cli_timeline_changed(
    agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    recorder: Option<&ConversationRecorder>,
    session_key: &str,
    reason: &str,
) {
    let Some(recorder) = recorder else {
        return;
    };
    agent_session_manager.emit_timeline_changed(
        session_key,
        &recorder.file_path().display().to_string(),
        recorder.event_count(),
        reason,
    );
}

pub(super) fn record_external_cli_result(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    session.remember_external_conversation_ref(
        result
            .metadata
            .get("conversationId")
            .or_else(|| result.metadata.get("conversation_id"))
            .cloned(),
        result
            .metadata
            .get("threadId")
            .or_else(|| result.metadata.get("thread_id"))
            .cloned(),
    );
    sync_external_cli_active_status(session);
    append_session_message(
        session,
        bifrost_agent::ChatMessage::assistant(&result.response),
    );
    sync_external_cli_active_status(session);
    if let Some(rec) = recorder.as_mut() {
        let run_state = if matches!(
            result.status,
            crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
        ) {
            "completed"
        } else {
            "failed"
        };
        if let Err(error) =
            rec.record_run_state(session_key, run_state, Some("im"), Some(&result.adapter))
        {
            warn!(error = %error, "failed to record external cli run state");
        }
        if let Err(error) = rec.record_assistant_message(session_key, &result.response) {
            warn!(error = %error, "failed to record external cli assistant message");
        }
    }
}

pub(super) fn record_external_cli_failure(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    _error: &str,
    reply: &str,
) {
    append_session_message(session, bifrost_agent::ChatMessage::assistant(reply));
    if let Some(rec) = recorder.as_mut() {
        if let Err(record_error) =
            rec.record_run_state(session_key, "failed", Some("im"), Some(&request.adapter))
        {
            warn!(error = %record_error, "failed to record external cli failure state");
        }
        if let Err(record_error) = rec.record_assistant_message(session_key, reply) {
            warn!(error = %record_error, "failed to record external cli failure message");
        }
    }
}

pub(super) fn append_session_message(
    session: &mut bifrost_agent::session::AgentSession,
    message: bifrost_agent::ChatMessage,
) {
    session.history.push(message);
    session.last_active_at = now_ms() / 1000;
    session.history_version = session.history_version.saturating_add(1);
}

pub(super) fn sync_external_cli_active_status(session: &bifrost_agent::session::AgentSession) {
    let Some(handle) = session.active_turn_status.as_ref() else {
        return;
    };
    let Ok(mut status) = handle.lock() else {
        return;
    };
    status.agent_type = session.agent_type.clone();
    status.runner_type = session.runner_type.clone();
    status.runner_id = session.runner_id.clone();
    status.model = session.model.clone();
    status.model_provider = session.model_provider.clone();
    status.model_reasoning_effort = session.model_reasoning_effort.clone();
    status.model_reasoning_summary = session.model_reasoning_summary.clone();
    status.external_conversation_id = session.external_conversation_id.clone();
    status.external_thread_id = session.external_thread_id.clone();
    status.user_turn_count = session.user_turn_count();
    status.message_count = session.history.len();
    status.work_dir = session.work_dir.clone();
    status.history_version = session.history_version;
    status.compaction_count = session.compaction_count;
    status.total_tokens_used = session.total_tokens_used;
    status.estimated_context_tokens = session.effective_token_count();
}

pub(super) fn external_cli_adapter_label(adapter: &str) -> &'static str {
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        "ChatGPT Web"
    } else {
        "Runner"
    }
}

pub(super) fn external_cli_progress_runner_summary(
    runner_id: &str,
    adapter: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    metadata: Option<&std::collections::BTreeMap<String, String>>,
) -> crate::im_gateway::progress_card::ProgressRunnerSummary {
    let resolved_model_config = crate::im_gateway::external_cli::resolve_external_cli_model_config(
        &request.adapter,
        &request.adapter_config,
    );
    let configured_model = resolved_model_config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let metadata_model = metadata
        .and_then(|metadata| metadata.get("model"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let model = configured_model.or(metadata_model).map(str::to_string);
    let model_source = if configured_model.is_some() {
        resolved_model_config
            .model_provider
            .clone()
            .or_else(|| resolved_model_config.model_source.clone())
            .map(|value| format_runner_model_source(&value))
    } else {
        metadata
            .and_then(|metadata| metadata.get("modelSource"))
            .map(String::as_str)
            .map(format_runner_model_source)
            .filter(|value| !value.trim().is_empty())
    };
    let external_thread_id = metadata.and_then(|metadata| {
        metadata
            .get("threadId")
            .or_else(|| metadata.get("thread_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let external_conversation_id = metadata.and_then(|metadata| {
        metadata
            .get("conversationId")
            .or_else(|| metadata.get("conversation_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    crate::im_gateway::progress_card::ProgressRunnerSummary {
        runner_id: runner_id.trim().to_string(),
        adapter: adapter.trim().to_string(),
        model,
        model_source,
        reasoning_effort: resolved_model_config.reasoning_effort.or_else(|| {
            metadata.and_then(|metadata| metadata.get("modelReasoningEffort").cloned())
        }),
        reasoning_summary: resolved_model_config.reasoning_summary.or_else(|| {
            metadata.and_then(|metadata| metadata.get("modelReasoningSummary").cloned())
        }),
        reasoning_source: resolved_model_config
            .reasoning_source
            .map(|value| format_runner_model_source(&value)),
        token_usage: metadata.and_then(external_cli_token_usage_from_metadata),
        weekly_usage: metadata.and_then(external_cli_weekly_usage_from_metadata),
        work_dir: request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        external_thread_id,
        external_conversation_id,
    }
}

pub(super) fn external_cli_weekly_usage_from_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
) -> Option<crate::im_gateway::progress_card::ProgressRunnerWeeklyUsage> {
    Some(
        crate::im_gateway::progress_card::ProgressRunnerWeeklyUsage {
            used_percent: metadata_u64(metadata, "codexWeeklyUsedPercent")?.min(100),
            window_minutes: metadata_u64(metadata, "codexWeeklyWindowMinutes")?,
            resets_at: metadata_u64(metadata, "codexWeeklyResetsAt"),
        },
    )
}

pub(super) fn external_cli_token_usage_from_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
) -> Option<crate::im_gateway::progress_card::ProgressRunnerTokenUsage> {
    let usage = crate::im_gateway::progress_card::ProgressRunnerTokenUsage {
        input_tokens: metadata_u64(metadata, "usageInputTokens"),
        cached_input_tokens: metadata_u64(metadata, "usageCachedInputTokens"),
        output_tokens: metadata_u64(metadata, "usageOutputTokens"),
        reasoning_output_tokens: metadata_u64(metadata, "usageReasoningOutputTokens"),
        total_tokens: metadata_u64(metadata, "usageTotalTokens"),
    };
    (usage.input_tokens.is_some()
        || usage.cached_input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.reasoning_output_tokens.is_some()
        || usage.total_tokens.is_some())
    .then_some(usage)
}

pub(super) fn metadata_u64(
    metadata: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Option<u64> {
    metadata.get(key)?.trim().parse().ok()
}

pub(super) fn format_runner_model_source(source: &str) -> String {
    match source.trim() {
        "runner config" => "runner 配置".to_string(),
        "codex default" => "Codex 默认".to_string(),
        "trae default" => "Trae 默认".to_string(),
        "codex config" => "Codex 配置".to_string(),
        "trae config" => "Trae 配置".to_string(),
        value => value.to_string(),
    }
}

pub(super) async fn maybe_stop_external_cli_for_event(event: &ImEvent, active_session_key: &str) {
    let Some(message) = event.message.as_ref() else {
        return;
    };
    let msg_text = agent_message_text(message);
    if msg_text.trim() != "/stop" {
        return;
    }
    let session_key = session_key_for_event(event);
    if session_key != active_session_key {
        return;
    }
    if let Err(error) = crate::im_gateway::external_cli::request_session_stop(
        crate::im_gateway::external_cli::default_runs_root(),
        active_session_key,
    )
    .await
    {
        debug!(session_key = %active_session_key, error = %error, "external cli session stop was not applied");
    }
}

pub(super) fn session_key_for_event(event: &ImEvent) -> String {
    if crate::im_gateway::group_context::is_feishu_group_event(event) {
        crate::im_gateway::group_context::build_group_session_key(
            &event.provider_id,
            event.source.chat_id.as_deref().unwrap_or_default(),
        )
    } else {
        build_session_key(&event.provider_id, event.source.user_id.as_deref())
    }
}
