use super::*;

// ---------------------------------------------------------------------------

pub(super) async fn handle_schedules(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /schedules  |  POST /schedules
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let schedules = service.schedule_store.list();
                json_response(&schedules)
            }
            Method::POST => {
                let mut schedule: ImSchedule = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if schedule.created_at == 0 {
                    schedule.created_at = now;
                }
                schedule.updated_at = now;
                if let Err(e) = crate::im_gateway::schedule_tools::normalize_schedule_with_context(
                    &mut schedule,
                    Some(&service.target_store),
                    None,
                ) {
                    return error_response(StatusCode::BAD_REQUEST, &e);
                }
                match service.schedule_store.add(schedule.clone()) {
                    Ok(()) => {
                        service.scheduler.notify_reschedule();
                        json_response(&schedule)
                    }
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/pause, /:id/resume, /:id/run, /:id/runs
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // /:id/pause
        if let Some(id) = extract_segment_before(id_and_rest, "/pause") {
            return handle_schedule_pause(req, service, id).await;
        }
        // /:id/resume
        if let Some(id) = extract_segment_before(id_and_rest, "/resume") {
            return handle_schedule_resume(req, service, id).await;
        }
        // /:id/run
        if let Some(id) = extract_segment_before(id_and_rest, "/run") {
            return handle_schedule_run(req, service, id).await;
        }
        // /:id/runs
        if let Some(id) = extract_segment_before(id_and_rest, "/runs") {
            return handle_schedule_runs(&req, service, id);
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_schedule_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Schedule endpoint not found")
}

pub(super) async fn handle_schedule_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.schedule_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Schedule not found");
            };
            apply_schedule_patch(&mut existing, &patch);
            if let Err(e) = crate::im_gateway::schedule_tools::normalize_schedule_with_context(
                &mut existing,
                Some(&service.target_store),
                None,
            ) {
                return error_response(StatusCode::BAD_REQUEST, &e);
            }
            match service.schedule_store.update(existing.clone()) {
                Ok(()) => {
                    service.scheduler.notify_reschedule();
                    json_response(&existing)
                }
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.schedule_store.delete(id) {
            Ok(()) => {
                service.scheduler.notify_reschedule();
                json_response(&serde_json::json!({"success": true}))
            }
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

pub(super) async fn handle_schedule_pause(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut schedule) = service.schedule_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Schedule not found");
    };
    schedule.enabled = false;
    schedule.updated_at = now_ms();
    match service.schedule_store.update(schedule.clone()) {
        Ok(()) => {
            service.scheduler.notify_reschedule();
            json_response(&schedule)
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub(super) async fn handle_schedule_resume(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut schedule) = service.schedule_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Schedule not found");
    };
    schedule.enabled = true;
    schedule.updated_at = now_ms();
    schedule.next_run_at = crate::im_gateway::scheduler::ImScheduler::compute_next_run_for_schedule(
        &schedule,
        schedule.updated_at,
    );
    match service.schedule_store.update(schedule.clone()) {
        Ok(()) => {
            service.scheduler.notify_reschedule();
            json_response(&schedule)
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub(super) async fn handle_schedule_run(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(schedule) = service.schedule_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Schedule not found");
    };

    let run_id = uuid_short();
    let task_run = execute_schedule_once(
        service,
        &schedule,
        run_id.clone(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    // Persist the run record
    let _ = service.run_store.add(task_run.clone());

    send_schedule_run_notification(service, &schedule, &task_run).await;

    json_response(&task_run)
}

pub(super) async fn execute_schedule_once(
    service: &ImGatewayService,
    schedule: &ImSchedule,
    run_id: String,
    trigger_source: crate::im_gateway::types::TriggerSource,
) -> crate::im_gateway::types::ImTaskRun {
    match schedule.task_type {
        crate::im_gateway::types::ScheduleTaskType::Script => {
            execute_script_schedule_once(service, schedule, run_id, trigger_source).await
        }
        crate::im_gateway::types::ScheduleTaskType::Agent => {
            execute_agent_schedule_once(service, schedule, run_id, trigger_source).await
        }
    }
}

pub(super) async fn execute_script_schedule_once(
    service: &ImGatewayService,
    schedule: &ImSchedule,
    run_id: String,
    trigger_source: crate::im_gateway::types::TriggerSource,
) -> crate::im_gateway::types::ImTaskRun {
    let (target, provider) = resolve_schedule_message_target(service, schedule);
    let Some(target) = target else {
        return failed_schedule_run(
            run_id,
            trigger_source,
            &schedule.id,
            None,
            schedule
                .message_channel
                .as_ref()
                .map(|channel| channel.target_id.clone()),
            "schedule target not found".to_string(),
        );
    };
    let Some(provider) = provider else {
        return failed_schedule_run(
            run_id,
            trigger_source,
            &schedule.id,
            Some(target.provider_id.clone()),
            Some(target.id.clone()),
            format!("Provider '{}' not found", target.provider_id),
        );
    };

    let request = crate::im_gateway::types::ImTaskExecutionRequest {
        provider_id: provider.id.clone(),
        trigger_source,
        policy_id: None,
        script_policy_binding: None,
        script: schedule.script.clone(),
        timeout_ms: schedule.timeout_ms,
        max_output_bytes: schedule.max_output_bytes,
    };

    let match_ctx = crate::im_gateway::task_executor::MatchContext::default();
    let mut task_run = crate::im_gateway::task_executor::ImTaskExecutor::execute(
        &request,
        run_id,
        None,
        Some(schedule.id.clone()),
        &match_ctx,
    )
    .await;
    task_run.target_id = Some(target.id);
    task_run
}

fn resolve_schedule_message_target(
    service: &ImGatewayService,
    schedule: &ImSchedule,
) -> (Option<ImTarget>, Option<ImProviderConfig>) {
    if let Some(channel) = schedule.message_channel.as_ref() {
        let provider = service.provider_store.get(&channel.provider_id);
        let Some(provider) = provider else {
            return (None, None);
        };
        let target = match channel.target_mode {
            crate::im_gateway::types::MessageTargetMode::ConfiguredTarget => {
                if channel.target_id == "__owner__" || channel.target_id == "owner" {
                    owner_schedule_target(&provider)
                } else {
                    service.target_store.get(&channel.target_id)
                }
            }
            crate::im_gateway::types::MessageTargetMode::Owner => owner_schedule_target(&provider),
            crate::im_gateway::types::MessageTargetMode::SourceThread => {
                Some(transient_schedule_target(
                    &provider,
                    "__source_thread__",
                    "Source Thread",
                    if provider.provider_type == ImProviderType::Feishu {
                        "chat_id"
                    } else {
                        "open_id"
                    },
                    &channel.target_id,
                ))
            }
            crate::im_gateway::types::MessageTargetMode::SourceUser => {
                Some(transient_schedule_target(
                    &provider,
                    "__source_user__",
                    "Source User",
                    "open_id",
                    &channel.target_id,
                ))
            }
        };
        return (target, Some(provider));
    }

    (None, None)
}

fn owner_schedule_target(provider: &ImProviderConfig) -> Option<ImTarget> {
    let owner_id = provider
        .owner_open_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(transient_schedule_target(
        provider,
        "__owner__",
        "Owner",
        "open_id",
        owner_id,
    ))
}

fn transient_schedule_target(
    provider: &ImProviderConfig,
    id: &str,
    display_name: &str,
    receive_id_type: &str,
    receive_id: &str,
) -> ImTarget {
    ImTarget {
        id: id.to_string(),
        provider_id: provider.id.clone(),
        display_name: display_name.to_string(),
        enabled: true,
        receive_id_type: receive_id_type.to_string(),
        receive_id: receive_id.to_string(),
        default_msg_type: "text".to_string(),
        created_at: 0,
        updated_at: 0,
    }
}

pub(super) async fn execute_agent_schedule_once(
    service: &ImGatewayService,
    schedule: &ImSchedule,
    run_id: String,
    trigger_source: crate::im_gateway::types::TriggerSource,
) -> crate::im_gateway::types::ImTaskRun {
    let now = now_ms();
    let (target, provider) = resolve_schedule_message_target(service, schedule);
    let mut run = crate::im_gateway::types::ImTaskRun {
        run_id,
        trigger_source,
        route_id: None,
        schedule_id: Some(schedule.id.clone()),
        provider_id: provider.as_ref().map(|provider| provider.id.clone()),
        target_id: target.as_ref().map(|target| target.id.clone()),
        status: crate::im_gateway::types::TaskRunStatus::Running,
        started_at: now,
        ended_at: None,
        duration_ms: None,
        exit_code: None,
        input_preview: None,
        stdout_preview: None,
        stderr_preview: None,
        stdout_digest: None,
        stderr_digest: None,
        error: None,
        task_type: Some(crate::im_gateway::types::ScheduleTaskType::Agent),
        runner_id: schedule
            .agent
            .as_ref()
            .and_then(|agent| normalize_schedule_runner_id(agent.runner_id.as_deref())),
        agent_final_response: None,
        agent_tool_calls: Vec::new(),
        agent_plan_steps: None,
    };

    let Some(agent_task) = schedule.agent.as_ref() else {
        finish_failed_run(&mut run, "agent schedule missing agent config".to_string());
        return run;
    };
    if agent_task.prompt.trim().is_empty() {
        finish_failed_run(
            &mut run,
            "agent schedule prompt cannot be empty".to_string(),
        );
        return run;
    }
    run.input_preview = Some(agent_schedule_input_preview(
        agent_task.initial_prompt.as_deref(),
        &agent_task.prompt,
    ));

    let Some(target) = target else {
        finish_failed_run(&mut run, "agent schedule target not found".to_string());
        return run;
    };
    let Some(provider) = provider else {
        finish_failed_run(&mut run, "agent schedule provider not found".to_string());
        return run;
    };
    run.provider_id = Some(provider.id.clone());
    run.target_id = Some(target.id.clone());

    let runner_id = normalize_schedule_runner_id(agent_task.runner_id.as_deref());
    run.runner_id = runner_id.clone();
    if let Some(runner_id) = runner_id.as_deref() {
        return execute_external_runner_schedule_once(
            service, schedule, agent_task, run, runner_id,
        )
        .await;
    }

    let mut config =
        effective_agent_config_for_provider(&service.agent_config_store.load(), &provider);
    let schedule_work_dir =
        schedule_agent_effective_work_dir(agent_task, config.work_dir.as_deref());
    config.work_dir = schedule_work_dir.clone();
    let session_key = agent_task
        .session_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("schedule:{}", schedule.id));
    let mut session = service
        .agent_session_manager
        .take_session_with_work_dir(&session_key, schedule_work_dir);
    session.source = "schedule".to_string();

    let turn = tokio::time::timeout(
        std::time::Duration::from_millis(schedule.timeout_ms),
        bifrost_agent::session::run_turn(
            &service.agent_client,
            &config,
            &mut session,
            &service.build_agent_tool_registry(schedule.message_channel.clone()),
            &agent_task.prompt,
            agent_task.system_prompt.as_deref(),
        ),
    )
    .await;
    service.agent_session_manager.return_session(session);

    match turn {
        Ok(Ok(result)) => {
            let ended = now_ms();
            run.status = crate::im_gateway::types::TaskRunStatus::Success;
            run.stdout_preview = Some(truncate_str(
                &result.response,
                schedule.max_output_bytes.min(4096) as usize,
            ));
            run.agent_final_response = Some(result.response.clone());
            run.agent_tool_calls = result.tool_calls_log;
            run.agent_plan_steps = result.plan_steps;
            run.ended_at = Some(ended);
            run.duration_ms = Some(ended.saturating_sub(run.started_at));
        }
        Ok(Err(error)) => finish_failed_run(&mut run, error),
        Err(_) => {
            let ended = now_ms();
            run.status = crate::im_gateway::types::TaskRunStatus::Timeout;
            run.error = Some(format!("timeout after {}ms", schedule.timeout_ms));
            run.ended_at = Some(ended);
            run.duration_ms = Some(ended.saturating_sub(run.started_at));
        }
    }

    run
}

fn normalize_schedule_runner_id(runner_id: Option<&str>) -> Option<String> {
    runner_id
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "bifrost_agent")
        .map(str::to_string)
}

async fn execute_external_runner_schedule_once(
    service: &ImGatewayService,
    schedule: &ImSchedule,
    agent_task: &crate::im_gateway::types::ScheduleAgentTask,
    mut run: crate::im_gateway::types::ImTaskRun,
    runner_id: &str,
) -> crate::im_gateway::types::ImTaskRun {
    let config = service.external_cli_config_store.load();
    if !config.runners.contains_key(runner_id) {
        finish_failed_run(&mut run, format!("runner '{runner_id}' not found"));
        return run;
    }
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        run.provider_id.as_deref(),
        Some(runner_id),
    );
    let mut settings = effective.settings;
    if let Some(adapter_config) = agent_task.adapter_config.as_ref() {
        merge_schedule_adapter_config(&mut settings.adapter_config, adapter_config);
    }
    let schedule_work_dir = external_schedule_effective_work_dir(service, agent_task, &run);
    let initial_prompt = agent_task
        .initial_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let is_chatgpt_web = settings.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID;
    if !is_chatgpt_web {
        if let Some(initial_prompt) = initial_prompt {
            settings.instructions = match settings.instructions.take() {
                Some(existing) if !existing.trim().is_empty() => {
                    Some(format!("{}\n\n{}", existing.trim(), initial_prompt))
                }
                _ => Some(initial_prompt.to_string()),
            };
        }
    }
    let session_key = agent_task
        .session_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("schedule:{}", schedule.id));
    let schedule_conversation_ref = schedule_conversation_ref_for_adapter(
        &settings.adapter,
        agent_task.conversation_ref.as_ref(),
    );
    let initialized = if schedule_conversation_ref.is_some() {
        true
    } else if is_chatgpt_web {
        crate::im_gateway::chatgpt_web::session_conversation_exists(&session_key).await
    } else {
        false
    };
    let messages = schedule_external_runner_messages(
        &settings.adapter,
        initial_prompt,
        initialized,
        &agent_task.prompt,
    );
    run.input_preview = Some(external_schedule_input_preview(
        settings.instructions.as_deref(),
        &messages,
    ));
    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
        crate::im_gateway::external_cli::default_runs_root(),
    );
    let mut final_result = None;
    for (idx, message) in messages.iter().enumerate() {
        let request = external_schedule_run_request(ExternalScheduleRunRequestParams {
            message: message.clone(),
            provider_id: run.provider_id.clone(),
            session_key: Some(session_key.clone()),
            runner_id: &effective.runner_id,
            settings: &settings,
            work_dir: schedule_work_dir.as_deref(),
            schedule_id: &schedule.id,
            conversation_ref: schedule_conversation_ref,
        });
        let run_future = Box::pin(runtime.run(request));
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(schedule.timeout_ms),
            run_future,
        )
        .await;
        match result {
            Ok(Ok(result)) if idx + 1 < messages.len() => {
                if result.status != crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
                {
                    finish_failed_run(
                        &mut run,
                        format!(
                            "runner initial prompt finished with status {:?}",
                            result.status
                        ),
                    );
                    return run;
                }
                if let Some(conversation_ref) =
                    schedule_conversation_ref_from_external_result(&settings.adapter, &result)
                {
                    persist_schedule_conversation_ref(service, &schedule.id, conversation_ref);
                }
            }
            other => {
                final_result = Some(other);
                break;
            }
        }
    }

    match final_result {
        None => finish_failed_run(&mut run, "runner did not produce a result".to_string()),
        Some(Ok(Ok(result))) => {
            if let Some(conversation_ref) =
                schedule_conversation_ref_from_external_result(&settings.adapter, &result)
            {
                persist_schedule_conversation_ref(service, &schedule.id, conversation_ref);
            }
            let ended = now_ms();
            run.status = match result.status {
                crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded => {
                    crate::im_gateway::types::TaskRunStatus::Success
                }
                crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut => {
                    crate::im_gateway::types::TaskRunStatus::Timeout
                }
                crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped => {
                    crate::im_gateway::types::TaskRunStatus::Cancelled
                }
                crate::im_gateway::external_cli::ExternalCliRunStatus::Failed => {
                    crate::im_gateway::types::TaskRunStatus::Failed
                }
            };
            run.exit_code = result.exit_code;
            run.stdout_preview = Some(truncate_str(
                &result.response,
                schedule.max_output_bytes.min(4096) as usize,
            ));
            run.agent_final_response = Some(result.response);
            run.ended_at = Some(ended);
            run.duration_ms = Some(ended.saturating_sub(run.started_at));
            if run.status != crate::im_gateway::types::TaskRunStatus::Success {
                run.error = Some(format!("runner finished with status {:?}", run.status));
            }
        }
        Some(Ok(Err(error))) => finish_failed_run(&mut run, error),
        Some(Err(_)) => {
            let ended = now_ms();
            run.status = crate::im_gateway::types::TaskRunStatus::Timeout;
            run.error = Some(format!("timeout after {}ms", schedule.timeout_ms));
            run.ended_at = Some(ended);
            run.duration_ms = Some(ended.saturating_sub(run.started_at));
        }
    }

    run
}

pub(super) fn schedule_agent_effective_work_dir(
    agent_task: &crate::im_gateway::types::ScheduleAgentTask,
    inherited_work_dir: Option<&str>,
) -> Option<String> {
    agent_task
        .work_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            inherited_work_dir
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn external_schedule_effective_work_dir(
    service: &ImGatewayService,
    agent_task: &crate::im_gateway::types::ScheduleAgentTask,
    run: &crate::im_gateway::types::ImTaskRun,
) -> Option<String> {
    if let Some(work_dir) = agent_task
        .work_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(work_dir.to_string());
    }
    let provider_id = run.provider_id.as_deref()?;
    let provider = service.provider_store.get(provider_id)?;
    effective_agent_config_for_provider(&service.agent_config_store.load(), &provider)
        .work_dir
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn merge_schedule_adapter_config(
    base: &mut crate::im_gateway::external_cli::ExternalCliAdapterConfig,
    override_config: &crate::im_gateway::external_cli::ExternalCliAdapterConfig,
) {
    if override_config.executable.is_some() {
        base.executable.clone_from(&override_config.executable);
    }
    if !override_config.args.is_empty() {
        base.args.clone_from(&override_config.args);
    }
    if !override_config.env.is_empty() {
        base.env.extend(override_config.env.clone());
    }
    if override_config.profile.is_some() {
        base.profile.clone_from(&override_config.profile);
    }
    if override_config.profile_v2.is_some() {
        base.profile_v2.clone_from(&override_config.profile_v2);
    }
    if override_config.model.is_some() {
        base.model.clone_from(&override_config.model);
    }
    if override_config.sandbox.is_some() {
        base.sandbox.clone_from(&override_config.sandbox);
    }
    if override_config.approval_policy.is_some() {
        base.approval_policy
            .clone_from(&override_config.approval_policy);
    }
    if override_config.reasoning_effort.is_some() {
        base.reasoning_effort
            .clone_from(&override_config.reasoning_effort);
    }
    if override_config.reasoning_summary.is_some() {
        base.reasoning_summary
            .clone_from(&override_config.reasoning_summary);
    }
    if override_config.danger_full_access.is_some() {
        base.danger_full_access = override_config.danger_full_access;
    }
    if override_config.dangerously_bypass_hook_trust.is_some() {
        base.dangerously_bypass_hook_trust = override_config.dangerously_bypass_hook_trust;
    }
    if override_config.strict_config.is_some() {
        base.strict_config = override_config.strict_config;
    }
    if override_config.skip_git_repo_check.is_some() {
        base.skip_git_repo_check = override_config.skip_git_repo_check;
    }
    if override_config.ignore_user_config.is_some() {
        base.ignore_user_config = override_config.ignore_user_config;
    }
    if override_config.ignore_rules.is_some() {
        base.ignore_rules = override_config.ignore_rules;
    }
    if override_config.oss.is_some() {
        base.oss = override_config.oss;
    }
    if override_config.local_provider.is_some() {
        base.local_provider
            .clone_from(&override_config.local_provider);
    }
    if override_config.output_schema.is_some() {
        base.output_schema
            .clone_from(&override_config.output_schema);
    }
    if override_config.color.is_some() {
        base.color.clone_from(&override_config.color);
    }
    if !override_config.add_dirs.is_empty() {
        base.add_dirs.clone_from(&override_config.add_dirs);
    }
    if !override_config.config_overrides.is_empty() {
        base.config_overrides
            .clone_from(&override_config.config_overrides);
    }
    if !override_config.enable_features.is_empty() {
        base.enable_features
            .clone_from(&override_config.enable_features);
    }
    if !override_config.disable_features.is_empty() {
        base.disable_features
            .clone_from(&override_config.disable_features);
    }
    if override_config.search.is_some() {
        base.search = override_config.search;
    }
    if override_config.ephemeral.is_some() {
        base.ephemeral = override_config.ephemeral;
    }
    if override_config.timeout_secs.is_some() {
        base.timeout_secs = override_config.timeout_secs;
    }
    if !override_config.extra.is_empty() {
        base.extra.extend(override_config.extra.clone());
    }
}

pub(super) fn schedule_external_runner_messages(
    adapter: &str,
    initial_prompt: Option<&str>,
    initialized: bool,
    prompt: &str,
) -> Vec<String> {
    let prompt = prompt.trim().to_string();
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID && !initialized {
        if let Some(initial_prompt) = initial_prompt
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return vec![initial_prompt.to_string(), prompt];
        }
    }
    vec![prompt]
}

fn agent_schedule_input_preview(initial_prompt: Option<&str>, prompt: &str) -> String {
    let mut parts = Vec::new();
    if let Some(initial_prompt) = initial_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("First-round Prompt:\n{initial_prompt}"));
    }
    parts.push(format!("Preset Prompt:\n{}", prompt.trim()));
    parts.join("\n\n")
}

fn external_schedule_input_preview(instructions: Option<&str>, messages: &[String]) -> String {
    let mut parts = Vec::new();
    if let Some(instructions) = instructions
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("Instructions:\n{instructions}"));
    }
    parts.extend(
        messages
            .iter()
            .enumerate()
            .map(|(idx, message)| format!("Message {}:\n{}", idx + 1, message.trim())),
    );
    parts.join("\n\n")
}

struct ExternalScheduleRunRequestParams<'a> {
    message: String,
    provider_id: Option<String>,
    session_key: Option<String>,
    runner_id: &'a str,
    settings: &'a crate::im_gateway::external_cli::ExternalCliAgentSettings,
    work_dir: Option<&'a str>,
    schedule_id: &'a str,
    conversation_ref: Option<&'a crate::im_gateway::types::ScheduleConversationRef>,
}

fn external_schedule_run_request(
    params: ExternalScheduleRunRequestParams<'_>,
) -> crate::im_gateway::external_cli::ExternalCliRunRequest {
    let mut request = crate::im_gateway::external_cli::run_request_from_settings(
        params.message,
        params.provider_id,
        params.session_key,
        params.settings,
    );
    request.runner_id = Some(params.runner_id.to_string());
    apply_schedule_conversation_ref_to_request(
        &mut request,
        &params.settings.adapter,
        params.conversation_ref,
    );
    if let Some(work_dir) = params
        .work_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request.work_dir = Some(PathBuf::from(work_dir));
    }
    if request.allow_work_dirs.is_empty() {
        if let Some(work_dir) = request.work_dir.as_ref() {
            request.allow_work_dirs = vec![work_dir.display().to_string()];
        }
    }
    if let Some(work_dir) = request.work_dir.as_ref() {
        tracing::debug!(
            work_dir = %work_dir.display(),
            schedule_id = %params.schedule_id,
            runner_id = %params.runner_id,
            "schedule external runner will execute from configured work_dir"
        );
    }
    request
}

fn schedule_conversation_ref_for_adapter<'a>(
    adapter: &str,
    conversation_ref: Option<&'a crate::im_gateway::types::ScheduleConversationRef>,
) -> Option<&'a crate::im_gateway::types::ScheduleConversationRef> {
    let conversation_ref = conversation_ref?;
    if conversation_ref.adapter != adapter {
        return None;
    }
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        return conversation_ref
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|_| conversation_ref);
    }
    if matches!(
        adapter,
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    ) {
        return conversation_ref
            .thread_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|_| conversation_ref);
    }
    None
}

fn apply_schedule_conversation_ref_to_request(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    adapter: &str,
    conversation_ref: Option<&crate::im_gateway::types::ScheduleConversationRef>,
) {
    let Some(conversation_ref) = schedule_conversation_ref_for_adapter(adapter, conversation_ref)
    else {
        return;
    };
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        if let Some(conversation_id) = conversation_ref
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request.params = serde_json::json!({ "conversationId": conversation_id });
        }
    } else if matches!(
        adapter,
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    ) {
        if let Some(thread_id) = conversation_ref
            .thread_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request.params = serde_json::json!({ "threadId": thread_id });
        }
    }
}

pub(super) fn schedule_conversation_ref_from_external_result(
    adapter: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) -> Option<crate::im_gateway::types::ScheduleConversationRef> {
    let now = now_ms();
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        let conversation_id = result_string_field(
            result,
            &["conversationId", "conversation_id", "conversationID"],
        )?;
        return Some(crate::im_gateway::types::ScheduleConversationRef {
            adapter: adapter.to_string(),
            conversation_id: Some(conversation_id),
            thread_id: None,
            updated_at: now,
        });
    }
    if matches!(
        adapter,
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    ) {
        let thread_id = result_string_field(result, &["threadId", "thread_id"])?;
        return Some(crate::im_gateway::types::ScheduleConversationRef {
            adapter: adapter.to_string(),
            conversation_id: None,
            thread_id: Some(thread_id),
            updated_at: now,
        });
    }
    None
}

fn result_string_field(
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
    keys: &[&str],
) -> Option<String> {
    for key in keys {
        if let Some(value) = result
            .metadata
            .get(*key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }
    result.events.iter().find_map(|event| {
        keys.iter().find_map(|key| {
            event
                .raw
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    })
}

fn persist_schedule_conversation_ref(
    service: &ImGatewayService,
    schedule_id: &str,
    conversation_ref: crate::im_gateway::types::ScheduleConversationRef,
) {
    let Some(mut latest) = service.schedule_store.get(schedule_id) else {
        return;
    };
    let Some(agent) = latest.agent.as_mut() else {
        return;
    };
    if agent.conversation_ref.as_ref() == Some(&conversation_ref) {
        return;
    }
    agent.conversation_ref = Some(conversation_ref);
    latest.updated_at = now_ms();
    if let Err(error) = service.schedule_store.update(latest) {
        tracing::warn!(
            schedule_id = %schedule_id,
            error = %error,
            "failed to persist schedule conversation reference"
        );
    }
}

pub(super) fn failed_schedule_run(
    run_id: String,
    trigger_source: crate::im_gateway::types::TriggerSource,
    schedule_id: &str,
    provider_id: Option<String>,
    target_id: Option<String>,
    error: String,
) -> crate::im_gateway::types::ImTaskRun {
    let now = now_ms();
    crate::im_gateway::types::ImTaskRun {
        run_id,
        trigger_source,
        route_id: None,
        schedule_id: Some(schedule_id.to_string()),
        provider_id,
        target_id,
        status: crate::im_gateway::types::TaskRunStatus::Failed,
        started_at: now,
        ended_at: Some(now),
        duration_ms: Some(0),
        exit_code: None,
        input_preview: None,
        stdout_preview: None,
        stderr_preview: None,
        stdout_digest: None,
        stderr_digest: None,
        error: Some(error),
        task_type: None,
        runner_id: None,
        agent_final_response: None,
        agent_tool_calls: Vec::new(),
        agent_plan_steps: None,
    }
}

pub(super) fn finish_failed_run(run: &mut crate::im_gateway::types::ImTaskRun, error: String) {
    let ended = now_ms();
    run.status = crate::im_gateway::types::TaskRunStatus::Failed;
    run.error = Some(error);
    run.ended_at = Some(ended);
    run.duration_ms = Some(ended.saturating_sub(run.started_at));
}

pub(super) async fn send_schedule_run_notification(
    service: &ImGatewayService,
    schedule: &ImSchedule,
    task_run: &crate::im_gateway::types::ImTaskRun,
) {
    let (target, provider) = resolve_schedule_message_target(service, schedule);
    let Some(provider) = provider else {
        return;
    };
    let Some(target) = target else {
        return;
    };

    let output = task_run
        .agent_final_response
        .as_deref()
        .or(task_run.stdout_preview.as_deref())
        .unwrap_or("(no output)");
    let output = truncate_str(output, schedule.max_output_bytes.min(4096) as usize);
    let status_label = if task_run.status == crate::im_gateway::types::TaskRunStatus::Success {
        "SUCCESS"
    } else {
        "FAILED"
    };
    let msg = format!(
        "**Schedule `{}` executed**\n\n- **Status**: {} ({:?})\n- **Duration**: {} ms\n\n**Output**\n```text\n{}\n```",
        schedule.id,
        status_label,
        task_run.status,
        task_run.duration_ms.unwrap_or(0),
        output
    );
    let client = service.provider_client(&provider);
    let send_result = client.send_text(&provider, &target, &msg).await;

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
        target_id: Some(target.id),
        target_name: Some(target.display_name),
        message_id,
        msg_type: Some(outbound_log_msg_type(&provider, "text")),
        content_preview: Some(truncate_str(&msg, 200)),
        trigger: Some(format!("schedule:{}", schedule.id)),
        error: error_msg,
        sender_open_id: None,
        event_id: None,
        reaction_added: None,
    };
    if let Err(e) = service.message_log_store.add(log) {
        error!(error = %e, "failed to store schedule outbound message log");
    }
}

pub(super) fn handle_schedule_runs(
    req: &Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let runs = service.run_store.list_by_schedule(id);
    json_response(&runs)
}

// ---------------------------------------------------------------------------
