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
                if let Err(e) = crate::im_gateway::schedule_tools::normalize_schedule(&mut schedule)
                {
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
            if let Err(e) = crate::im_gateway::schedule_tools::normalize_schedule(&mut existing) {
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

    json_response(&serde_json::json!({
        "success": true,
        "run_id": run_id,
        "schedule_id": schedule.id,
        "status": format!("{:?}", task_run.status),
        "duration_ms": task_run.duration_ms,
        "exit_code": task_run.exit_code,
        "stdout_preview": task_run.stdout_preview,
        "stderr_preview": task_run.stderr_preview,
        "error": task_run.error,
    }))
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
    let Some(target) = service.target_store.get(&schedule.target_id) else {
        return failed_schedule_run(
            run_id,
            trigger_source,
            &schedule.id,
            None,
            Some(schedule.target_id.clone()),
            format!("Target '{}' not found", schedule.target_id),
        );
    };
    let Some(provider) = service.provider_store.get(&target.provider_id) else {
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

pub(super) async fn execute_agent_schedule_once(
    service: &ImGatewayService,
    schedule: &ImSchedule,
    run_id: String,
    trigger_source: crate::im_gateway::types::TriggerSource,
) -> crate::im_gateway::types::ImTaskRun {
    let now = now_ms();
    let (target, provider) = if schedule.target_id.trim().is_empty() {
        (None, None)
    } else {
        match service.target_store.get(&schedule.target_id) {
            Some(target) => {
                let provider = service.provider_store.get(&target.provider_id);
                (Some(target), provider)
            }
            None => (None, None),
        }
    };
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
        stdout_preview: None,
        stderr_preview: None,
        stdout_digest: None,
        stderr_digest: None,
        error: None,
        task_type: Some(crate::im_gateway::types::ScheduleTaskType::Agent),
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

    let mut config = provider
        .as_ref()
        .map(|provider| {
            effective_agent_config_for_provider(&service.agent_config_store.load(), provider)
        })
        .unwrap_or_else(|| service.agent_config_store.load());
    if let Some(work_dir) = agent_task
        .work_dir
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        config.work_dir = Some(work_dir.clone());
    }
    let session_key = agent_task
        .session_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("schedule:{}", schedule.id));
    let mut session = service
        .agent_session_manager
        .take_session_with_work_dir(&session_key, agent_task.work_dir.clone());
    session.source = "schedule".to_string();

    let turn = tokio::time::timeout(
        std::time::Duration::from_millis(schedule.timeout_ms),
        bifrost_agent::session::run_turn(
            &service.agent_client,
            &config,
            &mut session,
            &service.agent_tools,
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
        stdout_preview: None,
        stderr_preview: None,
        stdout_digest: None,
        stderr_digest: None,
        error: Some(error),
        task_type: None,
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
    let provider = task_run
        .provider_id
        .as_deref()
        .and_then(|provider_id| service.provider_store.get(provider_id));
    let Some(provider) = provider else {
        return;
    };
    let Some(ref owner_id) = provider.owner_open_id else {
        return;
    };

    let stdout = task_run.stdout_preview.as_deref().unwrap_or("(no output)");
    let status_icon = if task_run.status == crate::im_gateway::types::TaskRunStatus::Success {
        "✅"
    } else {
        "❌"
    };
    let msg = format!(
        "{} Schedule '{}' executed\nStatus: {:?}\nDuration: {}ms\nOutput:\n{}",
        status_icon,
        schedule.id,
        task_run.status,
        task_run.duration_ms.unwrap_or(0),
        stdout
    );
    let owner_target = crate::im_gateway::types::ImTarget {
        id: "__owner__".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Owner".to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id: owner_id.clone(),
        default_msg_type: "text".to_string(),
        created_at: 0,
        updated_at: 0,
    };
    let client = service.provider_client(&provider);
    let content = serde_json::json!({"text": msg});
    let content_str = serde_json::to_string(&content).unwrap_or_default();
    let send_result = client
        .send_text(&provider, &owner_target, &content_str)
        .await;

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
        target_id: Some("__owner__".to_string()),
        target_name: Some("Owner".to_string()),
        message_id,
        msg_type: Some("text".to_string()),
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
