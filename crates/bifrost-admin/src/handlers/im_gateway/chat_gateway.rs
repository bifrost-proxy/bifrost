use super::*;
use std::path::PathBuf;

fn message_image_content_parts(
    message: &str,
    images: &[crate::im_gateway::external_cli::ExternalCliImageInput],
) -> Option<serde_json::Value> {
    let normalized: Vec<&crate::im_gateway::external_cli::ExternalCliImageInput> = images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .take(MAX_AGENT_IMAGES_PER_MESSAGE)
        .collect();
    if normalized.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !message.trim().is_empty() {
        parts.push(serde_json::json!({"type": "text", "text": message}));
    }
    for image in normalized {
        let data = if image.data.starts_with("data:") {
            image.data.clone()
        } else {
            format!("data:{};base64,{}", image.mime_type, image.data)
        };
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": data, "detail": "auto"}
        }));
    }
    Some(serde_json::Value::Array(parts))
}

fn image_message_preview(
    message: &str,
    images: &[crate::im_gateway::external_cli::ExternalCliImageInput],
) -> String {
    let trimmed = message.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let count = images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .take(MAX_AGENT_IMAGES_PER_MESSAGE)
        .count();
    if count == 1 {
        "Attached 1 image".to_string()
    } else {
        format!("Attached {count} images")
    }
}

pub(super) async fn handle_chat_gateway(
    req: Request<Incoming>,
    _service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    if rest == "/config" {
        return match *req.method() {
            Method::GET => {
                let config = _service.external_cli_config_store.load();
                json_response(&config)
            }
            Method::PATCH => {
                let config: crate::im_gateway::external_cli::ExternalCliGatewayConfig =
                    match read_body_json(req).await {
                        Ok(value) => value,
                        Err(response) => return response,
                    };
                match _service.external_cli_config_store.save(config) {
                    Ok(()) => json_response(&_service.external_cli_config_store.load()),
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest == "/adapters/chatgpt-web/auth/status" {
        return match *req.method() {
            Method::GET => {
                let runner_id = query_param(req.uri().query(), "runnerId");
                match chatgpt_web_settings(_service, runner_id.as_deref()) {
                    Ok(settings) => {
                        match crate::im_gateway::chatgpt_web::auth_status(&settings).await {
                            Ok(status) => json_response(&status),
                            Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                        }
                    }
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest == "/adapters/chatgpt-web/auth/open" {
        return match *req.method() {
            Method::POST => {
                let payload: serde_json::Value = match read_body_json(req).await {
                    Ok(value) => value,
                    Err(response) => return response,
                };
                let runner_id = payload
                    .get("runnerId")
                    .and_then(|value| value.as_str())
                    .map(ToString::to_string);
                match chatgpt_web_settings(_service, runner_id.as_deref()) {
                    Ok(settings) => {
                        match crate::im_gateway::chatgpt_web::open_login(&settings).await {
                            Ok(status) => json_response(&status),
                            Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                        }
                    }
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest == "/adapters/chatgpt-web/auth/stop" {
        return match *req.method() {
            Method::POST => {
                let payload: serde_json::Value = match read_body_json(req).await {
                    Ok(value) => value,
                    Err(response) => return response,
                };
                let runner_id = payload
                    .get("runnerId")
                    .and_then(|value| value.as_str())
                    .map(ToString::to_string);
                match chatgpt_web_settings(_service, runner_id.as_deref()) {
                    Ok(settings) => {
                        match crate::im_gateway::chatgpt_web::stop_login(&settings).await {
                            Ok(status) => json_response(&status),
                            Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                        }
                    }
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest == "/runner-calls/stream" {
        return match *req.method() {
            Method::POST => {
                let body: RunnerCallStreamRequest = match read_body_json(req).await {
                    Ok(value) => value,
                    Err(response) => return response,
                };
                match runner_call_stream_response(_service, body).await {
                    Ok(response) => response,
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest == "/stream" {
        return match *req.method() {
            Method::POST => {
                let mut request: crate::im_gateway::external_cli::ExternalCliRunRequest =
                    match read_body_json(req).await {
                        Ok(value) => value,
                        Err(response) => return response,
                    };
                let config = _service.external_cli_config_store.load();
                let effective =
                    crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
                        &config,
                        request.provider_id.as_deref(),
                        request.runner_id.as_deref(),
                    );
                request = crate::im_gateway::external_cli::merge_run_request_with_settings(
                    request,
                    &effective.settings,
                );
                if is_clear_session_command(&request.message) {
                    return clear_chat_gateway_session_response(
                        &request,
                        &effective.runner_id,
                        true,
                    )
                    .await;
                }
                if !effective.settings.enabled {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("runner '{}' is not enabled", effective.runner_id),
                    );
                }
                apply_provider_work_dir_to_external_cli_request(_service, &mut request);
                apply_persisted_external_cli_state(&mut request, &effective.runner_id);
                if request.message.trim() == "/stop" {
                    return stop_external_cli_stream_response(
                        request.session_key.as_deref().unwrap_or_default(),
                    )
                    .await;
                }
                if let Some(session_key) = request.session_key.as_deref() {
                    if !_service
                        .agent_session_manager
                        .try_start_external_session_preview(
                            session_key,
                            first_message_title_preview(&image_message_preview(
                                &request.message,
                                &request.images,
                            )),
                            request
                                .work_dir
                                .as_ref()
                                .map(|path| path.display().to_string()),
                            Some("admin-api".to_string()),
                            Some(request.adapter.clone()),
                            Some(effective.runner_id.clone()),
                        )
                    {
                        return queue_external_cli_stream_response(
                            _service,
                            session_key,
                            &request.message,
                        );
                    }
                }
                let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
                    crate::im_gateway::external_cli::default_runs_root(),
                );
                let (tx, rx) = tokio::sync::mpsc::channel::<
                    Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>,
                >(16);
                let runner_id_for_state = effective.runner_id.clone();
                let request_for_state = request.clone();
                let agent_session_manager = _service.agent_session_manager.clone();
                let queue_manager = _service.queue_manager.clone();
                let session_key_for_preview = request.session_key.clone();
                tokio::spawn(async move {
                    let mut current_request = request;
                    loop {
                        remember_external_cli_started_state(&current_request, &runner_id_for_state);
                        let started =
                            serde_json::json!({"eventType":"run_started","content":"started"});
                        let _ = tx
                            .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(format!(
                                "{}\n",
                                started
                            )))))
                            .await;
                        let request_snapshot = current_request.clone();
                        let run_result = runtime.run(current_request).await;
                        match run_result {
                            Ok(result) => {
                                remember_external_cli_result_state(
                                    &request_snapshot,
                                    &runner_id_for_state,
                                    &result,
                                );
                                for event in &result.events {
                                    if matches!(
                                        event.event_type,
                                        crate::im_gateway::external_cli::ExternalCliProgressEventType::RunStarted
                                            | crate::im_gateway::external_cli::ExternalCliProgressEventType::RunFinished
                                    ) {
                                        continue;
                                    }
                                    let line = serde_json::to_string(event)
                                        .unwrap_or_else(|_| "{}".to_string());
                                    let _ = tx
                                        .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                            format!("{line}\n"),
                                        ))))
                                        .await;
                                }
                                let finished = serde_json::json!({"eventType":"run_finished","runId":result.run_id,"status":result.status,"response":result.response});
                                let _ = tx
                                    .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                        format!("{}\n", finished),
                                    ))))
                                    .await;
                            }
                            Err(error) => {
                                let failed =
                                    serde_json::json!({"eventType":"run_failed","error":error});
                                let _ = tx
                                    .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                        format!("{}\n", failed),
                                    ))))
                                    .await;
                                break;
                            }
                        }
                        let Some(session_key) = session_key_for_preview.as_deref() else {
                            break;
                        };
                        let Some(next_message) = queue_manager.pop_queue(session_key) else {
                            break;
                        };
                        current_request = request_for_state.clone();
                        current_request.message = next_message;
                        apply_persisted_external_cli_state(
                            &mut current_request,
                            &runner_id_for_state,
                        );
                    }
                    if let Some(session_key) = session_key_for_preview.as_deref() {
                        agent_session_manager.clear_active_session_preview(session_key);
                    }
                });
                let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                let body = http_body_util::StreamBody::new(stream);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/x-ndjson")
                    .body(http_body_util::BodyExt::boxed(body))
                    .unwrap()
            }
            _ => method_not_allowed(),
        };
    }

    if let Some(provider_id) = rest.strip_prefix("/config/channels/") {
        let provider_id = provider_id.split('/').next().unwrap_or(provider_id);
        return match *req.method() {
            Method::GET => {
                let config = _service.external_cli_config_store.load();
                let effective = crate::im_gateway::external_cli::effective_config_for_provider(
                    &config,
                    Some(provider_id),
                );
                json_response(&serde_json::json!({
                    "providerId": provider_id,
                    "override": config.channels.get(provider_id),
                    "effective": effective,
                }))
            }
            Method::PATCH => {
                let settings: crate::im_gateway::external_cli::ExternalCliChannelSettings =
                    match read_body_json(req).await {
                        Ok(value) => value,
                        Err(response) => return response,
                    };
                match _service
                    .external_cli_config_store
                    .update_channel(provider_id, settings)
                {
                    Ok(effective) => {
                        let config = _service.external_cli_config_store.load();
                        json_response(&serde_json::json!({
                            "providerId": provider_id,
                            "override": config.channels.get(provider_id),
                            "effective": effective,
                        }))
                    }
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest.is_empty() {
        return match *req.method() {
            Method::POST => {
                let mut request: crate::im_gateway::external_cli::ExternalCliRunRequest =
                    match read_body_json(req).await {
                        Ok(value) => value,
                        Err(response) => return response,
                    };
                let config = _service.external_cli_config_store.load();
                let effective =
                    crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
                        &config,
                        request.provider_id.as_deref(),
                        request.runner_id.as_deref(),
                    );
                request = crate::im_gateway::external_cli::merge_run_request_with_settings(
                    request,
                    &effective.settings,
                );
                if is_clear_session_command(&request.message) {
                    return clear_chat_gateway_session_response(
                        &request,
                        &effective.runner_id,
                        false,
                    )
                    .await;
                }
                if !effective.settings.enabled {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("runner '{}' is not enabled", effective.runner_id),
                    );
                }
                apply_provider_work_dir_to_external_cli_request(_service, &mut request);
                apply_persisted_external_cli_state(&mut request, &effective.runner_id);
                let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
                    crate::im_gateway::external_cli::default_runs_root(),
                );
                let request_for_state = request.clone();
                remember_external_cli_started_state(&request_for_state, &effective.runner_id);
                match runtime.run(request).await {
                    Ok(result) => {
                        remember_external_cli_result_state(
                            &request_for_state,
                            &effective.runner_id,
                            &result,
                        );
                        json_response(&result)
                    }
                    Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if let Some(run_rest) = rest.strip_prefix("/runs/") {
        let mut parts = run_rest.split('/');
        let run_id = parts.next().unwrap_or(run_rest);
        let run_action = parts.next();
        if run_action == Some("stop") {
            return match *req.method() {
                Method::POST => match crate::im_gateway::external_cli::request_run_stop(
                    crate::im_gateway::external_cli::default_runs_root(),
                    run_id,
                )
                .await
                {
                    Ok(()) => json_response(&serde_json::json!({"success": true, "runId": run_id})),
                    Err(error) => error_response(StatusCode::NOT_FOUND, &error),
                },
                _ => method_not_allowed(),
            };
        }
        return match *req.method() {
            Method::GET => match crate::im_gateway::external_cli::read_run_detail(
                crate::im_gateway::external_cli::default_runs_root(),
                run_id,
            )
            .await
            {
                Ok(detail) => json_response(&detail),
                Err(error) => {
                    if error.contains("invalid run_id") {
                        error_response(StatusCode::BAD_REQUEST, &error)
                    } else {
                        error_response(StatusCode::NOT_FOUND, &error)
                    }
                }
            },
            _ => method_not_allowed(),
        };
    }

    error_response(StatusCode::NOT_FOUND, "Chat Gateway endpoint not found")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerCallStreamRequest {
    caller_session_key: String,
    #[serde(default)]
    caller_runner_id: Option<String>,
    #[serde(default)]
    caller_runner_adapter: Option<String>,
    target_runner_id: String,
    message: String,
    #[serde(default)]
    work_dir: Option<PathBuf>,
    #[serde(default)]
    history_path: Option<String>,
    #[serde(default)]
    caller_messages: Vec<RunnerCallMessage>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerCallMessage {
    role: String,
    content: String,
}

#[derive(Clone, Debug)]
enum RunnerCallTarget {
    BuiltinAgent,
    External(Box<crate::im_gateway::external_cli::ExternalCliEffectiveConfig>),
}

impl RunnerCallTarget {
    fn runner_id(&self) -> &str {
        match self {
            Self::BuiltinAgent => crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            Self::External(effective) => &effective.runner_id,
        }
    }

    fn adapter(&self) -> &str {
        match self {
            Self::BuiltinAgent => crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            Self::External(effective) => &effective.settings.adapter,
        }
    }
}

fn resolve_runner_call_target(
    config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    target_runner_id: &str,
) -> Result<RunnerCallTarget, String> {
    if target_runner_id == crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER {
        return Ok(RunnerCallTarget::BuiltinAgent);
    }
    if !config.runners.contains_key(target_runner_id) {
        return Err(format!("runner '{target_runner_id}' not found"));
    }
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        config,
        None,
        Some(target_runner_id),
    );
    if !effective.settings.enabled {
        return Err(format!("runner '{}' is not enabled", effective.runner_id));
    }
    Ok(RunnerCallTarget::External(Box::new(effective)))
}

async fn runner_call_stream_response(
    service: &ImGatewayService,
    body: RunnerCallStreamRequest,
) -> Result<Response<BoxBody>, String> {
    let caller_session_key = body.caller_session_key.trim().to_string();
    let target_runner_id = body.target_runner_id.trim().to_string();
    let user_message = body.message.trim().to_string();
    if caller_session_key.is_empty() {
        return Err("callerSessionKey is required".to_string());
    }
    if target_runner_id.is_empty() {
        return Err("targetRunnerId is required".to_string());
    }
    if user_message.is_empty() {
        return Err("message is required".to_string());
    }

    let config = service.external_cli_config_store.load();
    let target = resolve_runner_call_target(&config, &target_runner_id)?;

    let call_id = format!("runner-call-{}", uuid::Uuid::new_v4());
    let child_session_key = format!("runner-call:{}:{}", caller_session_key, target_runner_id);
    let prompt = build_runner_call_prompt(
        &caller_session_key,
        body.caller_runner_id.as_deref(),
        target.runner_id(),
        target.adapter(),
        body.history_path.as_deref(),
        &body.caller_messages,
        &user_message,
    );
    if matches!(target, RunnerCallTarget::BuiltinAgent) {
        return Ok(builtin_runner_call_stream_response(
            service,
            BuiltinRunnerCallStreamInput {
                call_id,
                caller_session_key,
                caller_scope: caller_runner_scope(
                    body.caller_runner_id.as_deref(),
                    body.caller_runner_adapter.as_deref(),
                ),
                child_session_key,
                prompt,
                user_message,
                work_dir: body.work_dir,
            },
        ));
    }
    let RunnerCallTarget::External(effective) = target else {
        unreachable!("builtin target already returned")
    };
    let mut request = crate::im_gateway::external_cli::run_request_from_settings(
        prompt,
        None,
        Some(child_session_key.clone()),
        &effective.settings,
    );
    request.runner_id = Some(target_runner_id.clone());
    request.work_dir = body.work_dir.clone();
    apply_provider_work_dir_to_external_cli_request(service, &mut request);
    apply_persisted_external_cli_state(&mut request, &effective.runner_id);

    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
        crate::im_gateway::external_cli::default_runs_root(),
    );
    let (tx, rx) =
        tokio::sync::mpsc::channel::<Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>>(16);
    let service_agent_sessions = service.agent_session_manager.clone();
    let caller_scope = caller_runner_scope(
        body.caller_runner_id.as_deref(),
        body.caller_runner_adapter.as_deref(),
    );
    tokio::spawn(async move {
        let started_at = now_ms();
        let started = serde_json::json!({
            "eventType": "runner_call_started",
            "callId": call_id.clone(),
            "callerSessionKey": caller_session_key.clone(),
            "childSessionKey": child_session_key.clone(),
            "targetRunnerId": target_runner_id.clone(),
            "targetAdapter": effective.settings.adapter.clone(),
        });
        let _ = send_ndjson_event(&tx, &started).await;
        let request_snapshot = request.clone();
        let raw_user_message = user_message.clone();
        remember_runner_call_started_for_caller(
            &service_agent_sessions,
            &caller_scope,
            &caller_session_key,
            &call_id,
            &target_runner_id,
            &effective.settings.adapter,
            &raw_user_message,
            started_at,
        );
        remember_external_cli_started_state(&request_snapshot, &effective.runner_id);
        match runtime.run(request).await {
            Ok(result) => {
                remember_external_cli_result_state(
                    &request_snapshot,
                    &effective.runner_id,
                    &result,
                );
                for event in &result.events {
                    let line =
                        serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
                    let _ = send_ndjson_event(&tx, &line).await;
                }
                remember_runner_call_for_caller(
                    &service_agent_sessions,
                    &caller_scope,
                    &call_id,
                    &raw_user_message,
                    &request_snapshot,
                    &result,
                );
                let finished = serde_json::json!({
                    "eventType": "runner_call_finished",
                    "callId": call_id.clone(),
                    "runId": result.run_id,
                    "status": result.status,
                    "response": result.response,
                    "childSessionKey": request_snapshot.session_key,
                    "targetRunnerId": effective.runner_id,
                    "targetAdapter": result.adapter,
                });
                let _ = send_ndjson_event(&tx, &finished).await;
            }
            Err(error) => {
                let failed = serde_json::json!({
                    "eventType": "runner_call_failed",
                    "callId": call_id.clone(),
                    "error": error,
                });
                let _ = send_ndjson_event(&tx, &failed).await;
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = http_body_util::StreamBody::new(stream);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .body(http_body_util::BodyExt::boxed(body))
        .unwrap())
}

struct BuiltinRunnerCallStreamInput {
    call_id: String,
    caller_session_key: String,
    caller_scope: (String, Option<String>, String),
    child_session_key: String,
    prompt: String,
    user_message: String,
    work_dir: Option<PathBuf>,
}

fn builtin_runner_call_stream_response(
    service: &ImGatewayService,
    input: BuiltinRunnerCallStreamInput,
) -> Response<BoxBody> {
    let (tx, rx) =
        tokio::sync::mpsc::channel::<Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>>(16);
    let agent_session_manager = service.agent_session_manager.clone();
    let config = service.agent_config_store.load();
    tokio::spawn(async move {
        let run_started_at = now_ms();
        let started = serde_json::json!({
            "eventType": "runner_call_started",
            "callId": input.call_id.clone(),
            "callerSessionKey": input.caller_session_key.clone(),
            "childSessionKey": input.child_session_key.clone(),
            "targetRunnerId": crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            "targetAdapter": crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
        });
        let _ = send_ndjson_event(&tx, &started).await;
        remember_runner_call_started_for_caller(
            &agent_session_manager,
            &input.caller_scope,
            &input.caller_session_key,
            &input.call_id,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            &input.user_message,
            run_started_at,
        );

        let Some(mut session) = agent_session_manager.try_take_session_with_work_dir(
            &input.child_session_key,
            input
                .work_dir
                .as_ref()
                .map(|path| path.display().to_string()),
        ) else {
            let failed = serde_json::json!({
                "eventType": "runner_call_failed",
                "callId": input.call_id,
                "error": "target Bifrost Agent runner is already busy",
            });
            let _ = send_ndjson_event(&tx, &failed).await;
            return;
        };
        session.source = "runner_call".to_string();
        session.mark_bifrost_agent_runtime();
        if session.title.is_none() {
            session.title = Some("Runner Call".to_string());
        }
        let history_path = session
            .recorder
            .as_ref()
            .map(|recorder| recorder.file_path().display().to_string());
        remember_session_turn_started(
            &input.child_session_key,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            None,
            &input.user_message,
            history_path.clone(),
            session.work_dir.clone(),
        );
        let mut worker_request = crate::im_gateway::agent_worker::build_run_request(
            input.child_session_key.clone(),
            input.prompt.clone(),
            Vec::new(),
            &config,
            session.work_dir.clone(),
            history_path,
            Some("runner_call".to_string()),
        );
        worker_request.default_message_channel = config.default_message_channel.clone();
        let mut worker =
            crate::im_gateway::agent_worker::AgentWorkerClient::spawn_or_fallback(worker_request)
                .await;
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel::<
            crate::im_gateway::agent_worker::AgentWorkerStopRequest,
        >();
        let worker_pid = worker.child_id().unwrap_or(0);
        crate::im_gateway::agent_worker::register_active_worker(
            &input.child_session_key,
            worker_pid,
            stop_tx,
        );

        let run_call_id = input.call_id.clone();
        let run_caller_session_key = input.caller_session_key.clone();
        let run_caller_scope = input.caller_scope.clone();
        let run_child_session_key = input.child_session_key.clone();
        let run_user_message = input.user_message.clone();
        loop {
            tokio::select! {
                maybe_stop = stop_rx.recv() => {
                    let _ = worker.terminate().await;
                    crate::im_gateway::agent_worker::clear_active_worker(&run_child_session_key);
                    agent_session_manager.return_session(session);
                    if let Some(stop_request) = maybe_stop {
                        stop_request.ack();
                    }
                    let failed = serde_json::json!({
                        "eventType": "runner_call_failed",
                        "callId": run_call_id,
                        "error": "target Bifrost Agent runner was stopped",
                    });
                    let _ = send_ndjson_event(&tx, &failed).await;
                    return;
                }
                event = worker.next_event() => {
                    let finished_at = now_ms();
                    match event {
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Started { .. })) => {}
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Progress { event })) => {
                            let payload = builtin_runner_call_progress_event_payload(&run_child_session_key, event);
                            let _ = send_ndjson_event(&tx, &payload).await;
                        }
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Finished { result: turn_result })) => {
                            if let Some(history_path) = turn_result.history_path.as_deref() {
                                let _ = restore_session_from_history_path(
                                    &mut session,
                                    std::path::Path::new(history_path),
                                    &run_child_session_key,
                                    config.history.as_ref().and_then(|h| h.max_bytes),
                                );
                            }
                            remember_runner_call_result_for_caller(
                                &agent_session_manager,
                                &run_caller_scope,
                                &run_caller_session_key,
                                &run_call_id,
                                crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                                crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                                &run_user_message,
                                &turn_result.response,
                                run_started_at,
                                finished_at,
                            );
                            let finished = serde_json::json!({
                                "eventType": "runner_call_finished",
                                "callId": run_call_id,
                                "runId": run_child_session_key,
                                "status": "success",
                                "response": turn_result.response,
                                "childSessionKey": run_child_session_key,
                                "targetRunnerId": crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                                "targetAdapter": crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                                "planSteps": turn_result.plan_steps,
                                "toolCalls": turn_result.tool_calls_log,
                            });
                            let _ = send_ndjson_event(&tx, &finished).await;
                            crate::im_gateway::agent_worker::clear_active_worker(&run_child_session_key);
                            remember_session_state_from_agent_session(
                                &session,
                                crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                                None,
                            );
                            agent_session_manager.return_session(session);
                            return;
                        }
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Failed { error })) => {
                            let failed = serde_json::json!({
                                "eventType": "runner_call_failed",
                                "callId": run_call_id,
                                "error": error,
                            });
                            let _ = send_ndjson_event(&tx, &failed).await;
                            crate::im_gateway::agent_worker::clear_active_worker(&run_child_session_key);
                            agent_session_manager.return_session(session);
                            return;
                        }
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Stopped)) => {
                            let failed = serde_json::json!({
                                "eventType": "runner_call_failed",
                                "callId": run_call_id,
                                "error": "target Bifrost Agent runner was stopped",
                            });
                            let _ = send_ndjson_event(&tx, &failed).await;
                            crate::im_gateway::agent_worker::clear_active_worker(&run_child_session_key);
                            agent_session_manager.return_session(session);
                            return;
                        }
                        Ok(None) => {
                            crate::im_gateway::agent_worker::clear_active_worker(&run_child_session_key);
                            agent_session_manager.return_session(session);
                            return;
                        }
                        Err(error) => {
                            let failed = serde_json::json!({
                                "eventType": "runner_call_failed",
                                "callId": run_call_id,
                                "error": format!("target Bifrost Agent worker failed: {error}"),
                            });
                            let _ = send_ndjson_event(&tx, &failed).await;
                            crate::im_gateway::agent_worker::clear_active_worker(&run_child_session_key);
                            agent_session_manager.return_session(session);
                            return;
                        }
                    }
                }
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = http_body_util::StreamBody::new(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .body(http_body_util::BodyExt::boxed(body))
        .unwrap()
}

fn builtin_runner_call_progress_event_payload(
    session_key: &str,
    event: bifrost_agent::AgentTurnProgressEvent,
) -> serde_json::Value {
    match event {
        bifrost_agent::AgentTurnProgressEvent::Status(status) => serde_json::json!({
            "eventType": "status",
            "sessionKey": session_key,
            "status": status,
        }),
        bifrost_agent::AgentTurnProgressEvent::ContextUpdated { context } => serde_json::json!({
            "eventType": "context_updated",
            "sessionKey": session_key,
            "context": context,
        }),
        bifrost_agent::AgentTurnProgressEvent::ToolStarted {
            tool_name,
            arguments,
        } => serde_json::json!({
            "eventType": "tool_started",
            "sessionKey": session_key,
            "toolName": tool_name,
            "arguments": arguments,
        }),
        bifrost_agent::AgentTurnProgressEvent::ToolFinished { log, duration_ms } => {
            serde_json::json!({
                "eventType": "tool_finished",
                "sessionKey": session_key,
                "log": log,
                "durationMs": duration_ms,
            })
        }
        bifrost_agent::AgentTurnProgressEvent::PlanUpdated { steps, title } => serde_json::json!({
            "eventType": "plan_updated",
            "sessionKey": session_key,
            "steps": steps,
            "title": title,
        }),
        bifrost_agent::AgentTurnProgressEvent::ProposedPlan { content } => serde_json::json!({
            "eventType": "proposed_plan",
            "sessionKey": session_key,
            "content": content,
        }),
        bifrost_agent::AgentTurnProgressEvent::TitleUpdated { title } => serde_json::json!({
            "eventType": "title_updated",
            "sessionKey": session_key,
            "title": title,
        }),
        bifrost_agent::AgentTurnProgressEvent::AssistantDelta { content } => serde_json::json!({
            "eventType": "assistant_delta",
            "sessionKey": session_key,
            "content": content,
        }),
        bifrost_agent::AgentTurnProgressEvent::AssistantFinal { content } => serde_json::json!({
            "eventType": "assistant_final",
            "sessionKey": session_key,
            "content": content,
        }),
        bifrost_agent::AgentTurnProgressEvent::TurnFinished { content } => serde_json::json!({
            "eventType": "turn_finished",
            "sessionKey": session_key,
            "content": content,
        }),
        bifrost_agent::AgentTurnProgressEvent::TurnFailed { error } => serde_json::json!({
            "eventType": "turn_failed",
            "sessionKey": session_key,
            "error": error,
        }),
        bifrost_agent::AgentTurnProgressEvent::CompactionStarted { progress } => {
            serde_json::json!({
                "eventType": "compaction_started",
                "sessionKey": session_key,
                "compaction": progress,
            })
        }
        bifrost_agent::AgentTurnProgressEvent::CompactionFinished { progress } => {
            serde_json::json!({
                "eventType": "compaction_finished",
                "sessionKey": session_key,
                "compaction": progress,
            })
        }
        bifrost_agent::AgentTurnProgressEvent::CompactionFailed { progress, error } => {
            serde_json::json!({
                "eventType": "compaction_failed",
                "sessionKey": session_key,
                "compaction": progress,
                "error": error,
            })
        }
        bifrost_agent::AgentTurnProgressEvent::LongTaskStatus {
            session_key: event_session_key,
            session_id,
            profile,
            state,
            elapsed_ms,
            last_output_preview,
            next_check_at_ms,
            unchanged_heartbeats,
        } => serde_json::json!({
            "eventType": "long_task_status",
            "sessionKey": event_session_key,
            "sessionId": session_id,
            "profile": profile,
            "state": state,
            "elapsedMs": elapsed_ms,
            "lastOutputPreview": last_output_preview,
            "nextCheckAtMs": next_check_at_ms,
            "unchangedHeartbeats": unchanged_heartbeats,
        }),
    }
}

async fn send_ndjson_event(
    tx: &tokio::sync::mpsc::Sender<Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>>,
    event: &serde_json::Value,
) -> Result<
    (),
    tokio::sync::mpsc::error::SendError<Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>>,
> {
    tx.send(Ok(hyper::body::Frame::data(bytes::Bytes::from(format!(
        "{event}\n"
    )))))
    .await
}

fn build_runner_call_prompt(
    caller_session_key: &str,
    caller_runner_id: Option<&str>,
    target_runner_id: &str,
    target_adapter: &str,
    history_path: Option<&str>,
    caller_messages: &[RunnerCallMessage],
    user_message: &str,
) -> String {
    let mut prompt = String::from("# Runner Call Context\n\n");
    prompt.push_str("Source session: ");
    prompt.push_str(caller_session_key);
    prompt.push('\n');
    if let Some(caller_runner_id) = caller_runner_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        prompt.push_str("Current runner: ");
        prompt.push_str(caller_runner_id);
        prompt.push('\n');
    }
    prompt.push_str("Target runner: ");
    prompt.push_str(target_runner_id);
    prompt.push_str(" (");
    prompt.push_str(target_adapter);
    prompt.push_str(")\n");
    if let Some(history_path) = history_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        prompt.push_str("History path: ");
        prompt.push_str(history_path);
        prompt.push('\n');
    }
    prompt.push_str("\n## Source Conversation Transcript\n\n");
    let mut included = 0usize;
    for message in caller_messages {
        let role = normalize_transcript_role(&message.role);
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        included += 1;
        prompt.push_str(role);
        prompt.push_str(":\n");
        prompt.push_str(content);
        prompt.push_str("\n\n");
    }
    if included == 0 {
        prompt.push_str("(No prior visible messages were provided.)\n\n");
    }
    prompt.push_str("## User Request For Target Runner\n\n");
    prompt.push_str(user_message.trim());
    prompt.push('\n');
    prompt
}

fn normalize_transcript_role(role: &str) -> &'static str {
    match role.trim().to_lowercase().as_str() {
        "assistant" => "Assistant",
        "system" => "System",
        "developer" => "Developer",
        _ => "User",
    }
}

fn caller_runner_scope(
    caller_runner_id: Option<&str>,
    caller_runner_adapter: Option<&str>,
) -> (String, Option<String>, String) {
    let runner_id = caller_runner_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER);
    if runner_id == crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER {
        return (
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER.to_string(),
            None,
            runner_id.to_string(),
        );
    }
    let adapter = caller_runner_adapter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(runner_id);
    (
        adapter.to_string(),
        Some(runner_id.to_string()),
        runner_id.to_string(),
    )
}

fn runner_call_visible_user(target_runner_id: &str, raw_user_message: &str) -> String {
    format!("Run with {}: {}", target_runner_id, raw_user_message.trim())
}

fn runner_call_running_message(target_runner_id: &str) -> String {
    format!("Runner `{target_runner_id}` is running...")
}

fn runner_call_completed_message(target_runner_id: &str, response: &str) -> String {
    format!(
        "Runner `{}` completed this call.\n\n{}",
        target_runner_id,
        response.trim()
    )
}

fn update_agent_runner_call_messages(
    messages: &mut Vec<bifrost_agent::ChatMessage>,
    visible_user: &str,
    running_message: &str,
    assistant_message: &str,
) {
    let user_index = messages
        .iter()
        .rposition(|message| {
            message.role == "user" && message.content.as_deref() == Some(visible_user)
        })
        .unwrap_or_else(|| {
            messages.push(bifrost_agent::ChatMessage::user(visible_user));
            messages.len() - 1
        });
    if let Some(message) = messages.iter_mut().skip(user_index + 1).find(|message| {
        message.role == "assistant"
            && matches!(
                message.content.as_deref(),
                Some(content) if content == running_message || content.starts_with("Runner `")
            )
    }) {
        message.content = Some(assistant_message.to_string());
        return;
    }
    messages.push(bifrost_agent::ChatMessage::assistant(assistant_message));
}

fn update_session_runner_call_messages(
    messages: &mut Vec<crate::im_gateway::session_state::ImAgentSessionMessage>,
    visible_user: &str,
    running_message: &str,
    assistant_message: &str,
    user_timestamp: u64,
    assistant_timestamp: u64,
) {
    let user_index = messages
        .iter()
        .rposition(|message| message.role == "user" && message.content == visible_user)
        .unwrap_or_else(|| {
            messages.push(crate::im_gateway::session_state::ImAgentSessionMessage {
                role: "user".to_string(),
                content: visible_user.to_string(),
                timestamp: Some(user_timestamp),
                content_parts: None,
            });
            messages.len() - 1
        });
    if let Some(message) = messages.iter_mut().skip(user_index + 1).find(|message| {
        message.role == "assistant"
            && (message.content == running_message || message.content.starts_with("Runner `"))
    }) {
        message.content = assistant_message.to_string();
        message.timestamp = Some(assistant_timestamp);
        return;
    }
    messages.push(crate::im_gateway::session_state::ImAgentSessionMessage {
        role: "assistant".to_string(),
        content: assistant_message.to_string(),
        timestamp: Some(assistant_timestamp),
        content_parts: None,
    });
}

#[allow(clippy::too_many_arguments)]
fn remember_runner_call_started_for_caller(
    agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    caller_scope: &(String, Option<String>, String),
    source_session_key: &str,
    call_id: &str,
    target_runner_id: &str,
    target_adapter: &str,
    raw_user_message: &str,
    started_at: u64,
) {
    let visible_user = runner_call_visible_user(target_runner_id, raw_user_message);
    let running_message = runner_call_running_message(target_runner_id);
    let (caller_adapter, caller_runner_id, _) = caller_scope;
    if caller_adapter == crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER {
        if let Some(mut session) =
            agent_session_manager.try_take_session_with_work_dir(source_session_key, None)
        {
            update_agent_runner_call_messages(
                &mut session.history,
                &visible_user,
                &running_message,
                &running_message,
            );
            session.last_active_at = started_at / 1000;
            agent_session_manager.return_session(session);
        } else if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
            source_session_key,
            caller_adapter,
            None,
            |state| {
                state.title.get_or_insert_with(|| visible_user.clone());
                state.last_user_message = Some(visible_user.clone());
                state.last_response = Some(running_message.clone());
                state.status = Some("running".to_string());
                update_session_runner_call_messages(
                    &mut state.messages,
                    &visible_user,
                    &running_message,
                    &running_message,
                    started_at / 1000,
                    started_at / 1000,
                );
            },
        ) {
            tracing::warn!(
                session_key = %source_session_key,
                call_id = %call_id,
                target_runner_id = %target_runner_id,
                error = %error,
                "failed to persist started runner call for built-in caller"
            );
        }
        return;
    }
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        source_session_key,
        caller_adapter,
        caller_runner_id.as_deref(),
        |state| {
            state.title.get_or_insert_with(|| visible_user.clone());
            state.last_user_message = Some(visible_user.clone());
            state.last_response = Some(running_message.clone());
            state.status = Some("running".to_string());
            update_session_runner_call_messages(
                &mut state.messages,
                &visible_user,
                &running_message,
                &running_message,
                started_at / 1000,
                started_at / 1000,
            );
        },
    ) {
        tracing::warn!(
            session_key = %source_session_key,
            adapter = %caller_adapter,
            runner_id = ?caller_runner_id,
            call_id = %call_id,
            target_runner_id = %target_runner_id,
            target_adapter = %target_adapter,
            error = %error,
            "failed to persist started runner call for external caller"
        );
    }
}

fn remember_runner_call_for_caller(
    agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    caller_scope: &(String, Option<String>, String),
    call_id: &str,
    raw_user_message: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    let Some(source_session_key) = request
        .session_key
        .as_deref()
        .and_then(|child_key| child_key.strip_prefix("runner-call:"))
        .and_then(|rest| rest.rsplit_once(':').map(|(source, _)| source.to_string()))
    else {
        return;
    };
    let response = result.response.trim();
    if response.is_empty() {
        return;
    }
    let target_runner_id = result
        .session_key
        .as_deref()
        .and_then(|key| key.rsplit_once(':').map(|(_, runner)| runner.to_string()))
        .unwrap_or_else(|| result.adapter.clone());
    remember_runner_call_result_for_caller(
        agent_session_manager,
        caller_scope,
        &source_session_key,
        call_id,
        &target_runner_id,
        &result.adapter,
        raw_user_message,
        response,
        result.started_at,
        result.finished_at,
    );
}

#[allow(clippy::too_many_arguments)]
fn remember_runner_call_result_for_caller(
    agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    caller_scope: &(String, Option<String>, String),
    source_session_key: &str,
    call_id: &str,
    target_runner_id: &str,
    target_adapter: &str,
    raw_user_message: &str,
    response: &str,
    started_at: u64,
    finished_at: u64,
) {
    let response = response.trim();
    if response.is_empty() {
        return;
    }
    let visible = runner_call_completed_message(target_runner_id, response);
    let visible_user = runner_call_visible_user(target_runner_id, raw_user_message);
    let running_message = runner_call_running_message(target_runner_id);
    let imported = crate::im_gateway::session_state::ImImportedRunnerContext {
        call_id: call_id.to_string(),
        source_session_key: source_session_key.to_string(),
        target_runner_id: target_runner_id.to_string(),
        target_adapter: target_adapter.to_string(),
        user_message: raw_user_message.to_string(),
        response: response.to_string(),
        created_at: finished_at,
    };
    let (caller_adapter, caller_runner_id, _) = caller_scope;
    if caller_adapter == crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER {
        if let Some(mut session) =
            agent_session_manager.try_take_session_with_work_dir(source_session_key, None)
        {
            update_agent_runner_call_messages(
                &mut session.history,
                &visible_user,
                &running_message,
                &visible,
            );
            session.last_active_at = finished_at / 1000;
            agent_session_manager.return_session(session);
        } else if let Err(error) = crate::im_gateway::session_state::push_imported_context(
            source_session_key,
            caller_adapter,
            None,
            imported,
        ) {
            tracing::warn!(
                session_key = %source_session_key,
                error = %error,
                "failed to persist imported runner context for busy built-in caller"
            );
        }
        return;
    }
    if let Err(error) = crate::im_gateway::session_state::push_imported_context(
        source_session_key,
        caller_adapter,
        caller_runner_id.as_deref(),
        imported,
    ) {
        tracing::warn!(
            session_key = %source_session_key,
            adapter = %caller_adapter,
            runner_id = ?caller_runner_id,
            error = %error,
            "failed to persist imported runner context for external caller"
        );
    }
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        source_session_key,
        caller_adapter,
        caller_runner_id.as_deref(),
        |state| {
            state.last_response = Some(visible.clone());
            state.status = Some("succeeded".to_string());
            update_session_runner_call_messages(
                &mut state.messages,
                &visible_user,
                &running_message,
                &visible,
                started_at / 1000,
                finished_at / 1000,
            );
        },
    ) {
        tracing::warn!(
            session_key = %source_session_key,
            adapter = %caller_adapter,
            error = %error,
            "failed to append visible runner call message"
        );
    }
}

fn queue_external_cli_stream_response(
    service: &ImGatewayService,
    session_key: &str,
    message: &str,
) -> Response<BoxBody> {
    let trimmed = message.trim();
    let payload = if let Some(rest) = trimmed.strip_prefix("/rq ") {
        match rest.trim().parse::<u64>() {
            Ok(seq) if service.queue_manager.remove_queue(session_key, seq) => {
                let items = service.queue_manager.queue_status(session_key);
                serde_json::json!({
                    "eventType": "run_finished",
                    "sessionKey": session_key,
                    "response": format!("🗑️ 已删除排队消息 #{seq}"),
                    "queued": true,
                    "queueLength": items.len(),
                    "queueItems": items,
                })
            }
            Ok(seq) => serde_json::json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": format!("❌ 未找到排队消息 #{seq}"),
            }),
            Err(_) => serde_json::json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": "用法: /rq <序号>（如 /rq 1）",
            }),
        }
    } else {
        let queue_message = trimmed
            .strip_prefix("/q ")
            .map(str::trim)
            .unwrap_or(trimmed);
        match service
            .queue_manager
            .push_queue(session_key, queue_message.to_string())
        {
            Ok(items) => serde_json::json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": format!("✅ 消息已收到，将在当前任务完成后处理（排队 {} 条）", items.len()),
                "queued": true,
                "queueLength": items.len(),
                "queueItems": items,
            }),
            Err(error) => serde_json::json!({
                "eventType": "run_failed",
                "sessionKey": session_key,
                "error": format!("排队失败: {error}"),
            }),
        }
    };
    let stream = tokio_stream::once(Ok::<_, hyper::Error>(hyper::body::Frame::data(
        bytes::Bytes::from(format!("{payload}\n")),
    )));
    let body = http_body_util::StreamBody::new(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .body(http_body_util::BodyExt::boxed(body))
        .unwrap()
}

async fn stop_external_cli_stream_response(session_key: &str) -> Response<BoxBody> {
    let worker_stopped =
        crate::im_gateway::external_cli::request_worker_session_stop(session_key).await;
    let stopped = crate::im_gateway::external_cli::request_session_stop(
        crate::im_gateway::external_cli::default_runs_root(),
        session_key,
    )
    .await
    .is_ok()
        || worker_stopped;
    let payload = serde_json::json!({
        "eventType": "run_finished",
        "sessionKey": session_key,
        "response": if stopped {
            "已请求停止当前 Runner。"
        } else {
            "当前没有正在执行的 Runner。"
        },
        "stopped": stopped,
    });
    let stream = tokio_stream::once(Ok::<_, hyper::Error>(hyper::body::Frame::data(
        bytes::Bytes::from(format!("{payload}\n")),
    )));
    let body = http_body_util::StreamBody::new(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .body(http_body_util::BodyExt::boxed(body))
        .unwrap()
}

fn is_clear_session_command(message: &str) -> bool {
    matches!(message.trim(), "/clear" | "/reset")
}

fn first_message_title_preview(message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 80;
    let preview: String = message.chars().take(MAX_CHARS).collect();
    if message.chars().count() > MAX_CHARS {
        Some(format!("{preview}…"))
    } else {
        Some(preview)
    }
}

async fn clear_chat_gateway_session_response(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    stream: bool,
) -> Response<BoxBody> {
    let Some(session_key) = request
        .session_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "sessionKey is required to reset Chat Gateway session",
        );
    };
    if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        crate::im_gateway::chatgpt_web::clear_session_conversation(session_key).await;
    }
    clear_persisted_agent_session_state(session_key, Some(&request.adapter), Some(runner_id));
    let payload = serde_json::json!({
        "success": true,
        "cleared": true,
        "sessionKey": session_key,
        "response": "Session reset.",
    });
    if !stream {
        return json_response(&payload);
    }
    let finished = serde_json::json!({
        "eventType": "run_finished",
        "status": "cleared",
        "response": "Session reset.",
        "cleared": true,
        "sessionKey": session_key,
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .body(crate::handlers::full_body(format!("{finished}\n")))
        .unwrap()
}

fn apply_persisted_external_cli_state(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let Some(state) = crate::im_gateway::session_state::load_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
    ) else {
        consume_imported_contexts_for_external_runner(request, runner_id);
        return;
    };
    if request_history_path(request).is_none() {
        if let Some(history_path) = state.history_path.as_deref() {
            set_request_history_path(request, history_path);
        }
    }
    let metadata = crate::im_gateway::session_state::metadata_from_state(&state);
    apply_external_cli_resume_metadata(request, &metadata);
    consume_imported_contexts_for_external_runner(request, runner_id);
}

fn consume_imported_contexts_for_external_runner(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let contexts = match crate::im_gateway::session_state::take_imported_contexts(
        session_key,
        &request.adapter,
        Some(runner_id),
    ) {
        Ok(contexts) => contexts,
        Err(error) => {
            tracing::warn!(
                session_key = %session_key,
                adapter = %request.adapter,
                runner_id = %runner_id,
                error = %error,
                "failed to consume imported runner contexts"
            );
            Vec::new()
        }
    };
    let Some(rendered) = crate::im_gateway::session_state::render_imported_contexts(&contexts)
    else {
        return;
    };
    request.instructions = Some(match request.instructions.take() {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{}\n\n{}", existing.trim(), rendered.trim())
        }
        _ => rendered,
    });
}

fn remember_external_cli_started_state(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    remember_session_state_values(
        session_key,
        &request.adapter,
        Some(runner_id),
        None,
        None,
        request_history_path(request),
        request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string()),
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
        |state| {
            state.last_user_message = first_message_title_preview(&image_message_preview(
                &request.message,
                &request.images,
            ));
            state.title = state
                .title
                .clone()
                .or_else(|| state.last_user_message.clone());
            state.status = Some("running".to_string());
            state.work_dir = request
                .work_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| state.work_dir.clone());
            append_external_runner_user_message_once(state, request, now);
        },
    ) {
        tracing::warn!(
            session_key = %session_key,
            adapter = %request.adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist external runner started state"
        );
    }
    record_external_cli_web_turn_started(request, runner_id);
}

fn remember_external_cli_result_state(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    remember_session_state_values(
        session_key,
        &result.adapter,
        Some(runner_id),
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
        request_history_path(request)
            .or_else(|| persisted_history_path_for_request(request, runner_id)),
        request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string()),
    );
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        &result.adapter,
        Some(runner_id),
        |state| {
            state.latest_run_id = Some(result.run_id.clone());
            state.last_user_message = first_message_title_preview(&image_message_preview(
                &request.message,
                &request.images,
            ));
            state.title = state
                .title
                .clone()
                .or_else(|| state.last_user_message.clone());
            state.last_response = if result.response.trim().is_empty() {
                None
            } else {
                Some(result.response.clone())
            };
            append_external_runner_turn_messages(state, request, result);
            state.status = Some(
                match result.status {
                    crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded => "succeeded",
                    crate::im_gateway::external_cli::ExternalCliRunStatus::Failed => "failed",
                    crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped => "stopped",
                    crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut => "timed_out",
                }
                .to_string(),
            );
            state.work_dir = request
                .work_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| state.work_dir.clone());
        },
    ) {
        tracing::warn!(
            session_key = %session_key,
            adapter = %result.adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist external runner result state"
        );
    }
    record_external_cli_web_turn_result(request, runner_id, result);
}

fn request_history_path(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) -> Option<String> {
    request
        .params
        .get("historyPath")
        .or_else(|| request.params.get("history_path"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn set_request_history_path(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    history_path: &str,
) {
    let Some(params) = request.params.as_object_mut() else {
        request.params = serde_json::json!({ "historyPath": history_path });
        return;
    };
    params
        .entry("historyPath".to_string())
        .or_insert_with(|| serde_json::Value::String(history_path.to_string()));
}

fn persisted_history_path_for_request(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) -> Option<String> {
    let session_key = request.session_key.as_deref()?;
    crate::im_gateway::session_state::load_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
    )
    .and_then(|state| state.history_path)
}

fn external_cli_timeline_recorder(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) -> Option<bifrost_agent::persistence::ConversationRecorder> {
    let session_key = request.session_key.as_deref()?;
    let data_dir = bifrost_agent::config::agent_home_dir();
    let history_paths = [
        request_history_path(request),
        persisted_history_path_for_request(request, runner_id),
    ];
    for history_path in history_paths.into_iter().flatten() {
        match bifrost_agent::persistence::validate_conversation_path(
            &data_dir,
            std::path::Path::new(&history_path),
        ) {
            Ok(path) => {
                return Some(
                    bifrost_agent::persistence::ConversationRecorder::from_existing_file(
                        path, None,
                    ),
                );
            }
            Err(error) => {
                tracing::warn!(
                    session_key = %session_key,
                    adapter = %request.adapter,
                    runner_id = %runner_id,
                    history_path = %history_path,
                    error = %error,
                    "external runner history path is invalid; creating a new timeline"
                );
            }
        }
    }

    let mut recorder =
        bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
    let path = recorder.file_path().display().to_string();
    remember_session_state_values(
        session_key,
        &request.adapter,
        Some(runner_id),
        None,
        None,
        Some(path),
        request
            .work_dir
            .as_ref()
            .map(|work_dir| work_dir.display().to_string()),
    );
    if let Err(error) = recorder.record_session_start(
        session_key,
        serde_json::json!({
            "source": "admin-api",
            "runtime": request.runtime,
            "adapter": request.adapter,
            "runner_id": runner_id,
            "provider_id": request.provider_id,
            "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
        }),
    ) {
        tracing::warn!(
            session_key = %session_key,
            adapter = %request.adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to record external runner session start"
        );
    }
    Some(recorder)
}

fn record_external_cli_web_turn_started(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let Some(mut recorder) = external_cli_timeline_recorder(request, runner_id) else {
        return;
    };
    if let Err(error) =
        recorder.record_run_state(session_key, "running", Some("web"), Some(runner_id))
    {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner running state");
    }
    if let Err(error) = recorder.record_user_message(session_key, &request.message) {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner user message");
    }
    let arguments = serde_json::json!({
        "runtime": request.runtime,
        "adapter": request.adapter,
        "runner_id": runner_id,
        "operation": request.operation,
        "provider_id": request.provider_id,
        "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
        "message_length": request.message.chars().count(),
    })
    .to_string();
    if let Err(error) =
        recorder.record_tool_call_with_id(session_key, &request.adapter, &arguments, None)
    {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner tool call");
    }
}

fn record_external_cli_web_turn_result(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let Some(mut recorder) = external_cli_timeline_recorder(request, runner_id) else {
        return;
    };
    let run_state = if matches!(
        result.status,
        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
    ) {
        "completed"
    } else {
        "failed"
    };
    if let Err(error) =
        recorder.record_run_state(session_key, run_state, Some("web"), Some(runner_id))
    {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner final state");
    }
    let tool_result = serde_json::json!({
        "run_id": result.run_id,
        "runtime": result.runtime,
        "adapter": result.adapter,
        "status": result.status,
        "exit_code": result.exit_code,
        "duration_ms": result.duration_ms,
        "artifacts": result.artifacts,
        "event_types": result.events.iter().map(|event| format!("{:?}", event.event_type)).collect::<Vec<_>>(),
        "response_preview": truncate_str(&result.response, 500),
    })
    .to_string();
    let success = matches!(
        result.status,
        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
    );
    if let Err(error) = recorder.record_tool_result_with_call_id(
        session_key,
        &result.adapter,
        &tool_result,
        success,
        Some(&result.run_id),
    ) {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner tool result");
    }
    if !result.response.trim().is_empty() {
        if let Err(error) = recorder.record_assistant_message(session_key, &result.response) {
            tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner assistant message");
        }
    }
}

fn append_external_runner_turn_messages(
    state: &mut crate::im_gateway::session_state::ImAgentSessionState,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    append_external_runner_user_message_once(state, request, result.started_at / 1000);
    let assistant_message = result.response.trim();
    if !assistant_message.is_empty() {
        if state.messages.last().is_some_and(|message| {
            message.role == "assistant" && message.content == assistant_message
        }) {
            return;
        }
        state
            .messages
            .push(crate::im_gateway::session_state::ImAgentSessionMessage {
                role: "assistant".to_string(),
                content: assistant_message.to_string(),
                timestamp: Some(result.finished_at / 1000),
                content_parts: None,
            });
    }
}

fn append_external_runner_user_message_once(
    state: &mut crate::im_gateway::session_state::ImAgentSessionState,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    timestamp: u64,
) {
    let user_message = image_message_preview(&request.message, &request.images);
    if user_message.is_empty() || user_message == "Attached 0 images" {
        return;
    }
    let content_parts = message_image_content_parts(&request.message, &request.images);
    if let Some(existing) = state
        .messages
        .last_mut()
        .filter(|message| message.role == "user" && message.content == user_message)
    {
        if existing.timestamp.is_none() {
            existing.timestamp = Some(timestamp);
        }
        if existing.content_parts.is_none() {
            existing.content_parts = content_parts;
        }
        return;
    }
    state
        .messages
        .push(crate::im_gateway::session_state::ImAgentSessionMessage {
            role: "user".to_string(),
            content: user_message.to_string(),
            timestamp: Some(timestamp),
            content_parts,
        });
}

fn apply_provider_work_dir_to_external_cli_request(
    service: &ImGatewayService,
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    if request.work_dir.is_some() {
        return;
    }
    let agent_config = service.agent_config_store.load();
    let work_dir = if let Some(provider_id) = request.provider_id.as_deref() {
        service
            .provider_store
            .get(provider_id)
            .and_then(|provider| effective_agent_work_dir_for_provider(&agent_config, &provider))
    } else {
        agent_config.work_dir.as_ref().map(PathBuf::from)
    };
    if let Some(work_dir) = work_dir {
        request.work_dir = Some(work_dir.clone());
        if request.allow_work_dirs.is_empty() {
            request.allow_work_dirs = vec![work_dir.display().to_string()];
        }
    }
}

fn chatgpt_web_settings(
    service: &ImGatewayService,
    runner_id: Option<&str>,
) -> Result<crate::im_gateway::external_cli::ExternalCliAgentSettings, String> {
    let config = service.external_cli_config_store.load();
    let runner_id = runner_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(config.default_runner_id.as_str());
    let settings = config
        .runners
        .get(runner_id)
        .cloned()
        .ok_or_else(|| format!("runner '{runner_id}' not found"))?;
    if settings.adapter != crate::im_gateway::chatgpt_web::ADAPTER_ID {
        return Err(format!(
            "runner '{}' uses adapter '{}', not chatgpt_web",
            runner_id, settings.adapter
        ));
    }
    Ok(settings)
}

fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    query.and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn response_json(response: Response<BoxBody>) -> serde_json::Value {
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect response body")
            .to_bytes();
        serde_json::from_slice(&body).expect("response should be json")
    }

    #[tokio::test]
    async fn queue_stream_remove_deletes_item_before_drain() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let service = ImGatewayService::new(temp_dir.path());

        let queued = response_json(queue_external_cli_stream_response(
            &service,
            "web-queue-delete",
            "/q queued follow up",
        ))
        .await;
        assert_eq!(queued["queued"], true);
        assert_eq!(queued["queueLength"], 1);
        assert_eq!(
            service.queue_manager.queue_status("web-queue-delete")[0].message,
            "queued follow up"
        );

        let removed = response_json(queue_external_cli_stream_response(
            &service,
            "web-queue-delete",
            "/rq 1",
        ))
        .await;
        assert_eq!(removed["queued"], true);
        assert_eq!(removed["queueLength"], 0);
        assert_eq!(
            removed["queueItems"].as_array().expect("queue items").len(),
            0
        );
        assert!(service
            .queue_manager
            .queue_status("web-queue-delete")
            .is_empty());
        assert!(service
            .queue_manager
            .pop_queue("web-queue-delete")
            .is_none());
    }

    #[test]
    fn external_runner_persists_user_message_before_result_and_dedupes_finish() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            message: "今天的AI领域相关的新闻。".to_string(),
            operation: "chat".to_string(),
            params: serde_json::Value::Null,
            provider_id: None,
            runner_id: Some("web".to_string()),
            session_key: Some("web-refresh-running-session".to_string()),
            runtime: "local".to_string(),
            adapter: "chatgpt_web".to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/web-workspace")),
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };

        remember_external_cli_started_state(&request, "web");
        let running_state = crate::im_gateway::session_state::load_session_state(
            "web-refresh-running-session",
            "chatgpt_web",
            Some("web"),
        )
        .expect("running state should be persisted immediately");
        assert_eq!(running_state.status.as_deref(), Some("running"));
        assert_eq!(running_state.messages.len(), 1);
        assert_eq!(running_state.messages[0].role, "user");
        assert_eq!(running_state.messages[0].content, request.message);

        let result = crate::im_gateway::external_cli::ExternalCliRunResult {
            run_id: "run-web-refresh".to_string(),
            session_key: request.session_key.clone(),
            runtime: request.runtime.clone(),
            adapter: request.adapter.clone(),
            status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
            exit_code: Some(0),
            response: "这是今天的 AI 新闻摘要。".to_string(),
            responses: Vec::new(),
            started_at: 1_779_700_000_000,
            finished_at: 1_779_700_002_000,
            duration_ms: 2_000,
            artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
                run_dir: String::new(),
                prompt: String::new(),
                command_snapshot: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                normalized_events: String::new(),
                last_message: String::new(),
            },
            events: Vec::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        remember_external_cli_result_state(&request, "web", &result);

        let finished_state = crate::im_gateway::session_state::load_session_state(
            "web-refresh-running-session",
            "chatgpt_web",
            Some("web"),
        )
        .expect("finished state");
        assert_eq!(finished_state.status.as_deref(), Some("succeeded"));
        assert_eq!(finished_state.messages.len(), 2);
        assert_eq!(finished_state.messages[0].role, "user");
        assert_eq!(finished_state.messages[0].content, request.message);
        assert_eq!(finished_state.messages[1].role, "assistant");
        assert_eq!(finished_state.messages[1].content, result.response);
    }

    #[test]
    fn external_runner_web_turn_appends_to_existing_history_timeline() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "shared-codex-history";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(session_key, serde_json::json!({"source": "im"}))
            .expect("record start");
        recorder
            .record_user_message(session_key, "old IM message")
            .expect("record old user");
        recorder
            .record_assistant_message(session_key, "old IM response")
            .expect("record old assistant");
        let history_path = recorder.file_path().display().to_string();

        let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            message: "new Web message".to_string(),
            operation: "chat".to_string(),
            params: serde_json::json!({ "historyPath": history_path }),
            provider_id: Some("feishu-main".to_string()),
            runner_id: Some("codex".to_string()),
            session_key: Some(session_key.to_string()),
            runtime: "external_cli".to_string(),
            adapter: "codex".to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/web-workspace")),
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };

        remember_external_cli_started_state(&request, "codex");
        let result = crate::im_gateway::external_cli::ExternalCliRunResult {
            run_id: "run-codex-web-history".to_string(),
            session_key: request.session_key.clone(),
            runtime: request.runtime.clone(),
            adapter: request.adapter.clone(),
            status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
            exit_code: Some(0),
            response: "new Codex response".to_string(),
            responses: Vec::new(),
            started_at: 1_779_700_000_000,
            finished_at: 1_779_700_002_000,
            duration_ms: 2_000,
            artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
                run_dir: String::new(),
                prompt: String::new(),
                command_snapshot: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                normalized_events: String::new(),
                last_message: String::new(),
            },
            events: Vec::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        remember_external_cli_result_state(&request, "codex", &result);

        let events = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load history events");
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::USER_MESSAGE
                && event.content.get("message").and_then(|v| v.as_str()) == Some("new Web message")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("new Codex response")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::RUN_STATE_CHANGED
                && event.content.get("source_channel").and_then(|v| v.as_str()) == Some("web")
                && event.content.get("agent_kind").and_then(|v| v.as_str()) == Some("codex")
        }));
        let state = crate::im_gateway::session_state::load_session_state(
            session_key,
            "codex",
            Some("codex"),
        )
        .expect("state");
        assert_eq!(state.history_path.as_deref(), Some(history_path.as_str()));

        let gpt_request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            message: "new GPT Web message".to_string(),
            operation: "chat".to_string(),
            params: serde_json::json!({ "historyPath": history_path }),
            provider_id: None,
            runner_id: Some("web".to_string()),
            session_key: Some(session_key.to_string()),
            runtime: "external_cli".to_string(),
            adapter: "chatgpt_web".to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/web-workspace")),
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };
        remember_external_cli_started_state(&gpt_request, "web");
        let gpt_result = crate::im_gateway::external_cli::ExternalCliRunResult {
            run_id: "run-gpt-web-history".to_string(),
            session_key: gpt_request.session_key.clone(),
            runtime: gpt_request.runtime.clone(),
            adapter: gpt_request.adapter.clone(),
            status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
            exit_code: Some(0),
            response: "new GPT Web response".to_string(),
            responses: Vec::new(),
            started_at: 1_779_700_003_000,
            finished_at: 1_779_700_004_000,
            duration_ms: 1_000,
            artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
                run_dir: String::new(),
                prompt: String::new(),
                command_snapshot: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                normalized_events: String::new(),
                last_message: String::new(),
            },
            events: Vec::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        remember_external_cli_result_state(&gpt_request, "web", &gpt_result);
        let events = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("reload history events");
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::USER_MESSAGE
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("new GPT Web message")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("new GPT Web response")
        }));
    }

    #[test]
    fn external_runner_web_turn_without_history_creates_canonical_timeline() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "active-gpt-web-history";
        let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            message: "active GPT Web message".to_string(),
            operation: "chat".to_string(),
            params: serde_json::json!({}),
            provider_id: None,
            runner_id: Some("gpt".to_string()),
            session_key: Some(session_key.to_string()),
            runtime: "external_cli".to_string(),
            adapter: "chatgpt_web".to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/web-workspace")),
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };

        remember_external_cli_started_state(&request, "gpt");
        let started_state = crate::im_gateway::session_state::load_session_state(
            session_key,
            "chatgpt_web",
            Some("gpt"),
        )
        .expect("started state");
        let history_path = started_state
            .history_path
            .as_deref()
            .expect("created history path")
            .to_string();

        let result = crate::im_gateway::external_cli::ExternalCliRunResult {
            run_id: "run-gpt-web-active".to_string(),
            session_key: request.session_key.clone(),
            runtime: request.runtime.clone(),
            adapter: request.adapter.clone(),
            status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
            exit_code: Some(0),
            response: "active GPT Web response".to_string(),
            responses: Vec::new(),
            started_at: 1_779_700_005_000,
            finished_at: 1_779_700_006_000,
            duration_ms: 1_000,
            artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
                run_dir: String::new(),
                prompt: String::new(),
                command_snapshot: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                normalized_events: String::new(),
                last_message: String::new(),
            },
            events: Vec::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        remember_external_cli_result_state(&request, "gpt", &result);

        let events = bifrost_agent::persistence::load_conversation_events(std::path::Path::new(
            &history_path,
        ))
        .expect("load created history events");
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::USER_MESSAGE
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("active GPT Web message")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("active GPT Web response")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::RUN_STATE_CHANGED
                && event.content.get("source_channel").and_then(|v| v.as_str()) == Some("web")
                && event.content.get("agent_kind").and_then(|v| v.as_str()) == Some("gpt")
        }));
        let finished_state = crate::im_gateway::session_state::load_session_state(
            session_key,
            "chatgpt_web",
            Some("gpt"),
        )
        .expect("finished state");
        assert_eq!(
            finished_state.history_path.as_deref(),
            Some(history_path.as_str())
        );
    }

    #[test]
    fn runner_call_prompt_includes_source_transcript_and_user_request() {
        let prompt = build_runner_call_prompt(
            "admin-chat-1",
            Some("bifrost_agent"),
            "codex",
            "codex",
            Some("/tmp/session.jsonl"),
            &[
                RunnerCallMessage {
                    role: "user".to_string(),
                    content: "Original question".to_string(),
                },
                RunnerCallMessage {
                    role: "assistant".to_string(),
                    content: "Original answer".to_string(),
                },
            ],
            "Review this from another runner",
        );

        assert!(prompt.contains("Source session: admin-chat-1"));
        assert!(prompt.contains("Current runner: bifrost_agent"));
        assert!(prompt.contains("Target runner: codex (codex)"));
        assert!(prompt.contains("History path: /tmp/session.jsonl"));
        assert!(prompt.contains("User:\nOriginal question"));
        assert!(prompt.contains("Assistant:\nOriginal answer"));
        assert!(prompt.contains("Review this from another runner"));
    }

    #[test]
    fn runner_call_target_accepts_builtin_agent() {
        let config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
        let target = resolve_runner_call_target(
            &config,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
        )
        .expect("builtin target should be accepted");
        assert!(matches!(target, RunnerCallTarget::BuiltinAgent));
        assert_eq!(
            target.adapter(),
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER
        );
    }

    #[test]
    fn runner_call_visible_messages_stay_on_source_thread() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let manager = std::sync::Arc::new(bifrost_agent::AgentSessionManager::new(60));
        let caller_scope = (
            "codex".to_string(),
            Some("codex".to_string()),
            "codex".to_string(),
        );

        remember_runner_call_started_for_caller(
            &manager,
            &caller_scope,
            "source-session",
            "call-visible",
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            "summarize this context",
            1_000,
        );
        let running = crate::im_gateway::session_state::load_session_state(
            "source-session",
            "codex",
            Some("codex"),
        )
        .expect("running source state");
        assert_eq!(running.status.as_deref(), Some("running"));
        assert_eq!(running.messages.len(), 2);
        assert_eq!(running.messages[0].role, "user");
        assert_eq!(
            running.messages[0].content,
            "Run with bifrost_agent: summarize this context"
        );
        assert_eq!(running.messages[1].role, "assistant");
        assert_eq!(
            running.messages[1].content,
            "Runner `bifrost_agent` is running..."
        );

        remember_runner_call_result_for_caller(
            &manager,
            &caller_scope,
            "source-session",
            "call-visible",
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            "summarize this context",
            "done",
            1_000,
            2_000,
        );
        let finished = crate::im_gateway::session_state::load_session_state(
            "source-session",
            "codex",
            Some("codex"),
        )
        .expect("finished source state");
        assert_eq!(finished.status.as_deref(), Some("succeeded"));
        assert_eq!(finished.messages.len(), 2);
        assert_eq!(
            finished.messages[1].content,
            "Runner `bifrost_agent` completed this call.\n\ndone"
        );
    }

    #[test]
    fn external_runner_consumes_imported_context_into_instructions_once() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        crate::im_gateway::session_state::push_imported_context(
            "external-import-session",
            "codex",
            Some("codex"),
            crate::im_gateway::session_state::ImImportedRunnerContext {
                call_id: "call-import".to_string(),
                source_session_key: "external-import-session".to_string(),
                target_runner_id: "web".to_string(),
                target_adapter: "chatgpt_web".to_string(),
                user_message: "ask web".to_string(),
                response: "web answer".to_string(),
                created_at: 1,
            },
        )
        .expect("push imported context");
        let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            message: "continue".to_string(),
            operation: "chat".to_string(),
            params: serde_json::Value::Null,
            provider_id: None,
            runner_id: Some("codex".to_string()),
            session_key: Some("external-import-session".to_string()),
            runtime: "local".to_string(),
            adapter: "codex".to_string(),
            work_dir: None,
            instructions: Some("base instructions".to_string()),
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };

        consume_imported_contexts_for_external_runner(&mut request, "codex");
        let instructions = request.instructions.expect("instructions");
        assert!(instructions.contains("base instructions"));
        assert!(instructions.contains("Imported Runner Results"));
        assert!(instructions.contains("web answer"));
        assert!(crate::im_gateway::session_state::take_imported_contexts(
            "external-import-session",
            "codex",
            Some("codex"),
        )
        .expect("take after consume")
        .is_empty());
    }
}
