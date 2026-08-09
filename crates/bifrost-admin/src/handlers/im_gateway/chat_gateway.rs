use super::*;
use crate::handlers::json_response_with_status;
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct BoundWebImProgressTarget {
    provider: ImProviderConfig,
    target: ImTarget,
}

#[derive(Clone)]
struct WebImProgressContext {
    binding: BoundWebImProgressTarget,
    registry: Arc<ImAgentProgressRegistry>,
    feishu: Arc<crate::im_gateway::feishu::FeishuProvider>,
}

#[derive(Clone)]
struct WebImProgressForwarder {
    tx: tokio::sync::mpsc::UnboundedSender<bifrost_agent::AgentTurnProgressEvent>,
    session_key: String,
    runner_id: String,
    adapter: String,
    model: Option<String>,
    model_provider: Option<String>,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    work_dir: Option<PathBuf>,
}

impl WebImProgressForwarder {
    fn forward(&self, event: &crate::im_gateway::external_cli::ExternalCliProgressEvent) {
        let Some(event) = crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
            &self.session_key,
            &self.adapter,
            crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                Some(&self.runner_id),
                self.model.as_deref(),
                self.model_provider.as_deref(),
                self.reasoning_effort.as_deref(),
                self.reasoning_summary.as_deref(),
                self.work_dir.as_deref(),
            ),
            event,
        ) else {
            return;
        };
        let _ = self.tx.send(event);
    }
}

struct WebImProgressTurn {
    registry: Arc<ImAgentProgressRegistry>,
    session_key: String,
    forwarder: WebImProgressForwarder,
    coalescer: tokio::task::JoinHandle<()>,
    runner_id: String,
    request: crate::im_gateway::external_cli::ExternalCliRunRequest,
}

impl WebImProgressTurn {
    async fn finish(
        self,
        output: String,
        failed: bool,
        metadata: Option<&std::collections::BTreeMap<String, String>>,
    ) {
        let runner_summary = external_cli_progress_runner_summary(
            &self.runner_id,
            &self.request.adapter,
            &self.request,
            metadata,
        );
        let _ = self
            .registry
            .update_runner_summary(&self.session_key, runner_summary)
            .await;
        let terminal_event = if failed {
            bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                error: output.clone(),
            }
        } else {
            bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                content: output.clone(),
            }
        };
        let _ = self.forwarder.tx.send(terminal_event);
        drop(self.forwarder);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.coalescer).await;
        let _ = self
            .registry
            .finish(&self.session_key, Some(output), failed)
            .await;
    }
}

fn bound_web_im_progress_target(
    service: &ImGatewayService,
    session_key: Option<&str>,
) -> Option<BoundWebImProgressTarget> {
    let session_key = session_key
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if let Some(binding) = service
        .group_context_store
        .binding_by_session(session_key)
        .ok()
        .flatten()
    {
        let provider = service.provider_store.get(&binding.provider_id)?;
        if !provider.enabled || provider.provider_type != ImProviderType::Feishu {
            return None;
        }
        return Some(BoundWebImProgressTarget {
            target: ImTarget {
                id: "__web_agent_progress__".to_string(),
                provider_id: provider.id.clone(),
                display_name: binding
                    .chat_name
                    .unwrap_or_else(|| "Bound group".to_string()),
                receive_id_type: "chat_id".to_string(),
                receive_id: binding.chat_id,
                default_msg_type: "interactive".to_string(),
                enabled: true,
                created_at: 0,
                updated_at: 0,
            },
            provider,
        });
    }

    service
        .provider_store
        .list()
        .into_iter()
        .find_map(|provider| {
            if !provider.enabled || provider.provider_type != ImProviderType::Feishu {
                return None;
            }
            let owner_open_id = provider
                .owner_open_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            if build_session_key(&provider.id, Some(owner_open_id)) != session_key {
                return None;
            }
            Some(BoundWebImProgressTarget {
                target: ImTarget {
                    id: "__web_agent_progress__".to_string(),
                    provider_id: provider.id.clone(),
                    display_name: "Bound private chat".to_string(),
                    receive_id_type: "open_id".to_string(),
                    receive_id: owner_open_id.to_string(),
                    default_msg_type: "interactive".to_string(),
                    enabled: true,
                    created_at: 0,
                    updated_at: 0,
                },
                provider,
            })
        })
}

async fn start_bound_web_im_progress(
    context: Option<&WebImProgressContext>,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) -> Option<WebImProgressTurn> {
    let context = context?;
    let session_key = request.session_key.as_deref()?;
    let initial_message = image_message_preview(&request.message, &request.images, &request.files);
    let started = if context
        .registry
        .rollover_existing(session_key, &initial_message)
        .await
    {
        Ok(())
    } else {
        context
            .registry
            .start_feishu(
                session_key,
                Arc::clone(&context.feishu),
                context.binding.provider.clone(),
                context.binding.target.clone(),
                &initial_message,
            )
            .await
            .map(|_| ())
    };
    if let Err(error) = started {
        warn!(
            session_key,
            error = %error,
            "failed to mirror Web Agent turn to bound IM progress card"
        );
        return None;
    }

    let runner_summary =
        external_cli_progress_runner_summary(runner_id, &request.adapter, request, None);
    let _ = context
        .registry
        .update_runner_summary(session_key, runner_summary)
        .await;
    let resolved_model_config = crate::im_gateway::external_cli::resolve_external_cli_model_config(
        &request.adapter,
        &request.adapter_config,
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registry = Arc::clone(&context.registry);
    let progress_session_key = session_key.to_string();
    let coalescer = tokio::spawn(async move {
        super::agent_chat_progress::run_progress_event_coalescer(
            registry,
            progress_session_key,
            &mut rx,
        )
        .await;
    });
    Some(WebImProgressTurn {
        registry: Arc::clone(&context.registry),
        session_key: session_key.to_string(),
        forwarder: WebImProgressForwarder {
            tx,
            session_key: session_key.to_string(),
            runner_id: runner_id.to_string(),
            adapter: request.adapter.clone(),
            model: resolved_model_config.model,
            model_provider: resolved_model_config
                .model_provider
                .or(resolved_model_config.model_source),
            reasoning_effort: resolved_model_config.reasoning_effort,
            reasoning_summary: resolved_model_config.reasoning_summary,
            work_dir: request.work_dir.clone(),
        },
        coalescer,
        runner_id: runner_id.to_string(),
        request: request.clone(),
    })
}

fn message_image_content_parts(
    message: &str,
    images: &[crate::im_gateway::external_cli::ExternalCliImageInput],
) -> Option<serde_json::Value> {
    let normalized: Vec<&crate::im_gateway::external_cli::ExternalCliImageInput> = images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .take(MAX_AGENT_ATTACHMENTS_PER_MESSAGE)
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
    files: &[crate::im_gateway::external_cli::ExternalCliFileInput],
) -> String {
    let trimmed = message.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let count = images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .take(MAX_AGENT_ATTACHMENTS_PER_MESSAGE)
        .count();
    if count > 0 {
        if count == 1 {
            return "Attached 1 image".to_string();
        }
        return format!("Attached {count} images");
    }
    let file_count = files
        .iter()
        .filter(|file| !file.data.trim().is_empty())
        .take(MAX_AGENT_ATTACHMENTS_PER_MESSAGE)
        .count();
    if file_count == 1 {
        "Attached 1 file".to_string()
    } else if file_count > 1 {
        format!("Attached {file_count} files")
    } else {
        String::new()
    }
}

fn external_cli_request_chat_images(
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

fn set_request_string_param(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    key: &str,
    value: String,
) {
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.insert(key.to_string(), serde_json::Value::String(value));
    }
}

fn session_attachment_base_dir_from_history_path(history_path: &str) -> Option<String> {
    let path = Path::new(history_path);
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy();
    Some(
        parent
            .join("attachments")
            .join(stem.as_ref())
            .display()
            .to_string(),
    )
}

fn prepare_external_cli_session_attachment_params(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    if let Some(params) = request.params.as_object_mut() {
        params.remove("attachmentBaseDir");
        params.remove("attachment_base_dir");
    }
    let history_path = persisted_history_path_for_request(request, runner_id).or_else(|| {
        external_cli_timeline_recorder(request, runner_id)
            .map(|recorder| recorder.file_path().display().to_string())
    });
    let Some(history_path) = history_path else {
        return;
    };
    if let Some(attachment_base_dir) = session_attachment_base_dir_from_history_path(&history_path)
    {
        set_request_string_param(request, "historyPath", history_path);
        set_request_string_param(request, "attachmentBaseDir", attachment_base_dir);
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

    if let Some(encoded_session_key) = rest
        .strip_prefix("/sessions/")
        .and_then(|value| value.strip_suffix("/guide"))
    {
        return match *req.method() {
            Method::POST => {
                let session_key = match urlencoding::decode(encoded_session_key) {
                    Ok(value) if !value.trim().is_empty() => value.into_owned(),
                    _ => {
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            "session key is missing or invalid",
                        )
                    }
                };
                let payload: serde_json::Value = match read_body_json(req).await {
                    Ok(value) => value,
                    Err(response) => return response,
                };
                let message = payload
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if message.is_empty() {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "guide message cannot be empty",
                    );
                }
                let guide_id = payload
                    .get("guideId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("guide-{}", uuid::Uuid::new_v4()));
                guide_external_cli_session(_service, &session_key, guide_id, message.to_string())
                    .await
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
                let bound_im_target =
                    bound_web_im_progress_target(_service, request.session_key.as_deref());
                if let Some(binding) = bound_im_target.as_ref() {
                    request.provider_id = Some(binding.provider.id.clone());
                }
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
                apply_agent_instructions_to_external_cli_request(
                    _service,
                    &mut request,
                    &effective.runner_id,
                );
                if let Some(response) =
                    maybe_external_cli_slash_response(&request, &effective, true).await
                {
                    return response;
                }
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
                                &request.files,
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
                        if request.adapter != crate::im_gateway::chatgpt_web::ADAPTER_ID {
                            if let Some(guide_message) = request
                                .message
                                .trim()
                                .strip_prefix("/g ")
                                .map(str::trim)
                                .filter(|message| !message.is_empty())
                            {
                                return guide_external_cli_stream_response(
                                    _service,
                                    session_key,
                                    guide_message,
                                )
                                .await;
                            }
                        }
                        return queue_external_cli_stream_response(
                            _service,
                            session_key,
                            &request.message,
                        )
                        .await;
                    }
                }
                prepare_external_cli_session_attachment_params(&mut request, &effective.runner_id);
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
                let web_im_progress_context = bound_im_target
                    .filter(|_| {
                        effective.settings.delivery_mode
                            != crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm
                    })
                    .map(|binding| WebImProgressContext {
                        binding,
                        registry: Arc::clone(&_service.progress_registry),
                        feishu: Arc::clone(_service.connection_manager.feishu_provider()),
                    });
                tokio::spawn(async move {
                    let mut current_request = request;
                    loop {
                        let web_im_progress = start_bound_web_im_progress(
                            web_im_progress_context.as_ref(),
                            &current_request,
                            &runner_id_for_state,
                        )
                        .await;
                        remember_external_cli_started_state(&current_request, &runner_id_for_state);
                        emit_external_cli_timeline_changed_from_request(
                            &agent_session_manager,
                            &current_request,
                            &runner_id_for_state,
                            "web_turn_started",
                        );
                        let started =
                            serde_json::json!({"eventType":"run_started","content":"started"});
                        let _ = tx
                            .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(format!(
                                "{}\n",
                                started
                            )))))
                            .await;
                        let request_snapshot = current_request.clone();
                        let streams_progress =
                            current_request.adapter != crate::im_gateway::chatgpt_web::ADAPTER_ID;
                        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<
                            crate::im_gateway::external_cli::ExternalCliProgressEvent,
                        >();
                        let http_progress_tx = tx.clone();
                        let progress_request = request_snapshot.clone();
                        let progress_runner_id = runner_id_for_state.clone();
                        let progress_agent_session_manager = agent_session_manager.clone();
                        let web_im_progress_forwarder = web_im_progress
                            .as_ref()
                            .map(|progress| progress.forwarder.clone());
                        let progress_task = tokio::spawn(async move {
                            let mut recorder = external_cli_timeline_recorder(
                                &progress_request,
                                &progress_runner_id,
                            );
                            while let Some(event) = progress_rx.recv().await {
                                if let Some(progress) = web_im_progress_forwarder.as_ref() {
                                    progress.forward(&event);
                                }
                                if let Some(end_index) = record_external_cli_web_progress_event(
                                    recorder.as_mut(),
                                    &progress_request,
                                    &progress_runner_id,
                                    &event,
                                ) {
                                    if let (Some(session_key), Some(recorder)) =
                                        (progress_request.session_key.as_deref(), recorder.as_ref())
                                    {
                                        progress_agent_session_manager.emit_timeline_changed(
                                            session_key,
                                            &recorder.file_path().display().to_string(),
                                            Some(end_index),
                                            "web_progress",
                                        );
                                    }
                                }
                                if matches!(
                                    event.event_type,
                                    crate::im_gateway::external_cli::ExternalCliProgressEventType::RunStarted
                                        | crate::im_gateway::external_cli::ExternalCliProgressEventType::RunFinished
                                ) {
                                    continue;
                                }
                                let line = serde_json::to_string(
                                    &external_cli_progress_event_payload(&event),
                                )
                                .unwrap_or_else(|_| "{}".to_string());
                                let _ = http_progress_tx
                                    .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                        format!("{line}\n"),
                                    ))))
                                    .await;
                            }
                        });
                        let run_result = runtime
                            .run_with_progress(current_request, Some(progress_tx))
                            .await;
                        let _ = progress_task.await;
                        match run_result {
                            Ok(result) => {
                                remember_external_cli_result_state(
                                    &request_snapshot,
                                    &runner_id_for_state,
                                    &result,
                                );
                                emit_external_cli_timeline_changed_from_request(
                                    &agent_session_manager,
                                    &request_snapshot,
                                    &runner_id_for_state,
                                    "web_turn_finished",
                                );
                                if !streams_progress {
                                    for event in &result.events {
                                        if let Some(progress) = web_im_progress.as_ref() {
                                            progress.forwarder.forward(event);
                                        }
                                        if matches!(
                                            event.event_type,
                                            crate::im_gateway::external_cli::ExternalCliProgressEventType::RunStarted
                                                | crate::im_gateway::external_cli::ExternalCliProgressEventType::RunFinished
                                        ) {
                                            continue;
                                        }
                                        let line = serde_json::to_string(
                                            &external_cli_progress_event_payload(event),
                                        )
                                        .unwrap_or_else(|_| "{}".to_string());
                                        let _ = tx
                                            .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                                format!("{line}\n"),
                                            ))))
                                            .await;
                                    }
                                }
                                let finished = serde_json::json!({"eventType":"run_finished","runId":result.run_id,"status":result.status,"response":result.response});
                                let _ = tx
                                    .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                        format!("{}\n", finished),
                                    ))))
                                    .await;
                                if let Some(progress) = web_im_progress {
                                    let failed = !matches!(
                                        result.status,
                                        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
                                    );
                                    progress
                                        .finish(
                                            result.response.clone(),
                                            failed,
                                            Some(&result.metadata),
                                        )
                                        .await;
                                }
                            }
                            Err(error) => {
                                let failed =
                                    serde_json::json!({"eventType":"run_failed","error":error});
                                let _ = tx
                                    .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                        format!("{}\n", failed),
                                    ))))
                                    .await;
                                if let Some(progress) = web_im_progress {
                                    progress.finish(error, true, None).await;
                                }
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
                        current_request.images.clear();
                        current_request.files.clear();
                        apply_persisted_external_cli_state(
                            &mut current_request,
                            &runner_id_for_state,
                        );
                        prepare_external_cli_session_attachment_params(
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
                apply_agent_instructions_to_external_cli_request(
                    _service,
                    &mut request,
                    &effective.runner_id,
                );
                if let Some(response) =
                    maybe_external_cli_slash_response(&request, &effective, false).await
                {
                    return response;
                }
                prepare_external_cli_session_attachment_params(&mut request, &effective.runner_id);
                let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
                    crate::im_gateway::external_cli::default_runs_root(),
                );
                let request_for_state = request.clone();
                remember_external_cli_started_state(&request_for_state, &effective.runner_id);
                emit_external_cli_timeline_changed_from_request(
                    &_service.agent_session_manager,
                    &request_for_state,
                    &effective.runner_id,
                    "web_turn_started",
                );
                match runtime.run(request).await {
                    Ok(result) => {
                        remember_external_cli_result_state(
                            &request_for_state,
                            &effective.runner_id,
                            &result,
                        );
                        emit_external_cli_timeline_changed_from_request(
                            &_service.agent_session_manager,
                            &request_for_state,
                            &effective.runner_id,
                            "web_turn_finished",
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

async fn update_web_guide_progress(
    service: &ImGatewayService,
    session_key: &str,
    message: &str,
    queue_items: Vec<crate::im_gateway::queue_manager::QueueItem>,
    queued: bool,
) -> bool {
    let notice = if queued {
        format!(
            "WebUI 引导无法实时注入，已排队：{}",
            truncate_str(message, 48)
        )
    } else {
        format!("已从 WebUI 收到引导：{}", truncate_str(message, 48))
    };
    service
        .progress_registry
        .update_queue_state(session_key, queue_items, false, Some(notice))
        .await
}

async fn accepted_web_guide_payload(
    service: &ImGatewayService,
    session_key: &str,
    message: &str,
    result: &crate::im_gateway::external_cli::ExternalCliGuideResult,
    stream_response: bool,
) -> serde_json::Value {
    update_web_guide_progress(
        service,
        session_key,
        message,
        service.queue_manager.queue_status(session_key),
        false,
    )
    .await;
    if stream_response {
        serde_json::json!({
            "eventType": "run_finished",
            "sessionKey": session_key,
            "response": "🔀 已发送到当前 Runner session，将按 Runner 的实时引导语义生效",
            "guide": true,
            "guideId": result.guide_id,
            "delivery": "steered",
            "threadId": result.thread_id,
            "turnId": result.turn_id,
        })
    } else {
        serde_json::json!({
            "guideId": result.guide_id,
            "sessionKey": session_key,
            "delivery": "steered",
            "threadId": result.thread_id,
            "turnId": result.turn_id,
        })
    }
}

async fn guide_external_cli_session(
    service: &ImGatewayService,
    session_key: &str,
    guide_id: String,
    message: String,
) -> Response<BoxBody> {
    match crate::im_gateway::external_cli::request_worker_session_guide(
        session_key,
        guide_id.clone(),
        message.clone(),
    )
    .await
    {
        Ok(result) if result.accepted => json_response(
            &accepted_web_guide_payload(service, session_key, &message, &result, false).await,
        ),
        outcome => {
            let reason = match outcome {
                Ok(result) => result
                    .reason
                    .unwrap_or_else(|| "active turn rejected guide input".to_string()),
                Err(error) => error,
            };
            if !service.agent_session_manager.is_session_active(session_key) {
                return json_response_with_status(
                    StatusCode::CONFLICT,
                    &serde_json::json!({
                        "guideId": guide_id,
                        "sessionKey": session_key,
                        "delivery": "rejected",
                        "reason": reason,
                    }),
                );
            }
            match service
                .queue_manager
                .push_queue(session_key, message.to_string())
            {
                Ok(items) => {
                    update_web_guide_progress(service, session_key, &message, items.clone(), true)
                        .await;
                    json_response(&serde_json::json!({
                        "guideId": guide_id,
                        "sessionKey": session_key,
                        "delivery": "queued",
                        "queueLength": items.len(),
                        "reason": reason,
                    }))
                }
                Err(error) => error_response(
                    StatusCode::CONFLICT,
                    &format!("guide was not accepted and queue fallback failed: {error}"),
                ),
            }
        }
    }
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
    images: Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
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
    External(Box<crate::im_gateway::external_cli::ExternalCliEffectiveConfig>),
}

impl RunnerCallTarget {
    fn runner_id(&self) -> &str {
        match self {
            Self::External(effective) => &effective.runner_id,
        }
    }

    fn adapter(&self) -> &str {
        match self {
            Self::External(effective) => &effective.settings.adapter,
        }
    }
}

fn resolve_runner_call_target(
    config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    target_runner_id: &str,
) -> Result<RunnerCallTarget, String> {
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
    let has_images = body
        .images
        .iter()
        .any(|image| !image.data.trim().is_empty());
    if user_message.is_empty() && !has_images {
        return Err("message or images are required".to_string());
    }
    let visible_user_message = image_message_preview(&user_message, &body.images, &[]);

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
        &visible_user_message,
    );
    let RunnerCallTarget::External(effective) = target;
    let mut request = crate::im_gateway::external_cli::run_request_from_settings(
        prompt,
        None,
        Some(child_session_key.clone()),
        &effective.settings,
    );
    request.images = body.images;
    request.runner_id = Some(target_runner_id.clone());
    request.work_dir = body.work_dir.clone();
    apply_provider_work_dir_to_external_cli_request(service, &mut request);
    apply_persisted_external_cli_state(&mut request, &effective.runner_id);
    apply_agent_instructions_to_external_cli_request(service, &mut request, &effective.runner_id);
    prepare_external_cli_session_attachment_params(&mut request, &effective.runner_id);

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
        let raw_user_message = visible_user_message.clone();
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
        emit_external_cli_timeline_changed_from_request(
            &service_agent_sessions,
            &request_snapshot,
            &effective.runner_id,
            "web_runner_call_started",
        );
        let streams_progress = request.adapter != crate::im_gateway::chatgpt_web::ADAPTER_ID;
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<
            crate::im_gateway::external_cli::ExternalCliProgressEvent,
        >();
        let call_progress_tx = tx.clone();
        let progress_request = request_snapshot.clone();
        let progress_runner_id = effective.runner_id.clone();
        let progress_agent_session_manager = service_agent_sessions.clone();
        let progress_task = tokio::spawn(async move {
            let mut recorder =
                external_cli_timeline_recorder(&progress_request, &progress_runner_id);
            while let Some(event) = progress_rx.recv().await {
                if let Some(end_index) = record_external_cli_web_progress_event(
                    recorder.as_mut(),
                    &progress_request,
                    &progress_runner_id,
                    &event,
                ) {
                    if let (Some(session_key), Some(recorder)) =
                        (progress_request.session_key.as_deref(), recorder.as_ref())
                    {
                        progress_agent_session_manager.emit_timeline_changed(
                            session_key,
                            &recorder.file_path().display().to_string(),
                            Some(end_index),
                            "web_runner_call_progress",
                        );
                    }
                }
                let line = external_cli_progress_event_payload(&event);
                let _ = send_ndjson_event(&call_progress_tx, &line).await;
            }
        });
        match runtime.run_with_progress(request, Some(progress_tx)).await {
            Ok(result) => {
                let _ = progress_task.await;
                remember_external_cli_result_state(
                    &request_snapshot,
                    &effective.runner_id,
                    &result,
                );
                emit_external_cli_timeline_changed_from_request(
                    &service_agent_sessions,
                    &request_snapshot,
                    &effective.runner_id,
                    "web_runner_call_finished",
                );
                if !streams_progress {
                    for event in &result.events {
                        let line = external_cli_progress_event_payload(event);
                        let _ = send_ndjson_event(&tx, &line).await;
                    }
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
                let _ = progress_task.await;
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

fn external_cli_progress_event_payload(
    event: &crate::im_gateway::external_cli::ExternalCliProgressEvent,
) -> serde_json::Value {
    let mut payload = serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
    if event.event_type
        == crate::im_gateway::external_cli::ExternalCliProgressEventType::PlanUpdated
    {
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "steps".to_string(),
                serde_json::to_value(
                    crate::im_gateway::external_cli::external_progress_plan_steps(event),
                )
                .unwrap_or_else(|_| serde_json::json!([])),
            );
        }
    } else if event.event_type
        == crate::im_gateway::external_cli::ExternalCliProgressEventType::SubAgentUpdated
    {
        if let (Some(object), Some(subagent)) = (payload.as_object_mut(), event.raw.get("subagent"))
        {
            object.insert("subagent".to_string(), subagent.clone());
        }
    }
    payload
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
        .unwrap_or("external");
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

fn append_runner_call_started_to_parent_history(
    source_session_key: &str,
    caller_adapter: &str,
    caller_runner_id: Option<&str>,
    visible_user: &str,
    running_message: &str,
    target_runner_id: &str,
) {
    let _ = (caller_adapter, caller_runner_id);
    let data_dir = bifrost_agent::config::agent_home_dir();
    let mut recorder = match bifrost_agent::persistence::ConversationRecorder::open_or_create(
        &data_dir,
        source_session_key,
        None,
    ) {
        Ok((recorder, false)) => recorder,
        Ok((_recorder, true)) => return,
        Err(error) => {
            tracing::warn!(
                session_key = %source_session_key,
                error = %error,
                "runner call parent canonical history is unavailable"
            );
            return;
        }
    };
    if let Err(error) = recorder.record_run_state(
        source_session_key,
        "running",
        Some("web"),
        Some(target_runner_id),
    ) {
        tracing::warn!(session_key = %source_session_key, error = %error, "failed to record runner call started state in parent history");
    }
    if let Err(error) = recorder.record_user_message(source_session_key, visible_user) {
        tracing::warn!(session_key = %source_session_key, error = %error, "failed to record runner call user message in parent history");
    }
    if let Err(error) = recorder.record_assistant_message(source_session_key, running_message) {
        tracing::warn!(session_key = %source_session_key, error = %error, "failed to record runner call running message in parent history");
    }
}

fn append_runner_call_result_to_parent_history(
    source_session_key: &str,
    caller_adapter: &str,
    caller_runner_id: Option<&str>,
    visible: &str,
    target_runner_id: &str,
) {
    let _ = (caller_adapter, caller_runner_id);
    let data_dir = bifrost_agent::config::agent_home_dir();
    let mut recorder = match bifrost_agent::persistence::ConversationRecorder::open_or_create(
        &data_dir,
        source_session_key,
        None,
    ) {
        Ok((recorder, false)) => recorder,
        Ok((_recorder, true)) => return,
        Err(error) => {
            tracing::warn!(
                session_key = %source_session_key,
                error = %error,
                "runner call parent canonical history is unavailable"
            );
            return;
        }
    };
    if let Err(error) = recorder.record_run_state(
        source_session_key,
        "completed",
        Some("web"),
        Some(target_runner_id),
    ) {
        tracing::warn!(session_key = %source_session_key, error = %error, "failed to record runner call completed state in parent history");
    }
    if let Err(error) = recorder.record_assistant_message(source_session_key, visible) {
        tracing::warn!(session_key = %source_session_key, error = %error, "failed to record runner call result in parent history");
    }
}

#[allow(clippy::too_many_arguments)]
fn remember_runner_call_started_for_caller(
    _agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
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
    append_runner_call_started_to_parent_history(
        source_session_key,
        caller_adapter,
        caller_runner_id.as_deref(),
        &visible_user,
        &running_message,
        target_runner_id,
    );
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
        Some(&result.run_id),
        raw_user_message,
        response,
        result.started_at,
        result.finished_at,
    );
}

#[allow(clippy::too_many_arguments)]
fn remember_runner_call_result_for_caller(
    _agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    caller_scope: &(String, Option<String>, String),
    source_session_key: &str,
    call_id: &str,
    target_runner_id: &str,
    target_adapter: &str,
    latest_run_id: Option<&str>,
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
    append_runner_call_result_to_parent_history(
        source_session_key,
        caller_adapter,
        caller_runner_id.as_deref(),
        &visible,
        target_runner_id,
    );
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
            if let Some(run_id) = latest_run_id {
                state.latest_run_id = Some(run_id.to_string());
            }
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

async fn queue_external_cli_stream_response(
    service: &ImGatewayService,
    session_key: &str,
    message: &str,
) -> Response<BoxBody> {
    let trimmed = message.trim();
    let payload = if let Some(rest) = trimmed.strip_prefix("/rq ") {
        match rest.trim().parse::<u64>() {
            Ok(seq) if service.queue_manager.remove_queue(session_key, seq) => {
                let items = service.queue_manager.queue_status(session_key);
                let _ = service
                    .progress_registry
                    .update_queue_state(
                        session_key,
                        items.clone(),
                        !service.queue_manager.guide_status(session_key).is_empty(),
                        Some(format!("WebUI 已删除排队消息 #{seq}")),
                    )
                    .await;
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
            Ok(items) => {
                let _ = service
                    .progress_registry
                    .update_queue_state(
                        session_key,
                        items.clone(),
                        !service.queue_manager.guide_status(session_key).is_empty(),
                        Some(format!(
                            "已从 WebUI 收到排队消息：{}",
                            truncate_str(queue_message, 48)
                        )),
                    )
                    .await;
                serde_json::json!({
                    "eventType": "run_finished",
                    "sessionKey": session_key,
                    "response": format!("✅ 消息已收到，将在当前任务完成后处理（排队 {} 条）", items.len()),
                    "queued": true,
                    "queueLength": items.len(),
                    "queueItems": items,
                })
            }
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

async fn guide_external_cli_stream_response(
    service: &ImGatewayService,
    session_key: &str,
    message: &str,
) -> Response<BoxBody> {
    let guide_id = format!("guide-{}", uuid::Uuid::new_v4());
    let outcome = crate::im_gateway::external_cli::request_worker_session_guide(
        session_key,
        guide_id.clone(),
        message.to_string(),
    )
    .await;
    let payload = match outcome {
        Ok(result) if result.accepted => {
            accepted_web_guide_payload(service, session_key, message, &result, true).await
        }
        outcome => {
            let reason = match outcome {
                Ok(result) => result
                    .reason
                    .unwrap_or_else(|| "active turn rejected guide input".to_string()),
                Err(error) => error,
            };
            match service
                .queue_manager
                .push_queue(session_key, message.to_string())
            {
                Ok(items) => {
                    update_web_guide_progress(service, session_key, message, items.clone(), true)
                        .await;
                    serde_json::json!({
                        "eventType": "run_finished",
                        "sessionKey": session_key,
                        "response": format!(
                            "⚠️ 当前 Runner 无法实时接收引导，已作为排队消息处理（排队 {} 条）",
                            items.len()
                        ),
                        "guide": true,
                        "queued": true,
                        "guideId": guide_id,
                        "delivery": "queued",
                        "queueLength": items.len(),
                        "queueItems": items,
                        "reason": reason,
                    })
                }
                Err(error) => serde_json::json!({
                    "eventType": "run_failed",
                    "sessionKey": session_key,
                    "guideId": guide_id,
                    "delivery": "rejected",
                    "error": format!("guide was not accepted and queue fallback failed: {error}"),
                    "reason": reason,
                }),
            }
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

async fn maybe_external_cli_model_slash_response(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
    stream: bool,
) -> Option<Response<BoxBody>> {
    let command =
        crate::im_gateway::external_cli::parse_external_cli_model_slash_command(&request.message)?;
    let command = match command {
        Ok(command) => command,
        Err(error) => return Some(model_slash_error_response(&error, stream)),
    };
    if !crate::im_gateway::external_cli::supports_external_cli_model_slash(&request.adapter) {
        return Some(model_slash_error_response(
            "/model 和 /models 当前仅支持 Codex、Traex 或 Claude Code Runner。",
            stream,
        ));
    }
    let adapter_label =
        crate::im_gateway::external_cli::external_cli_model_adapter_label(&request.adapter);
    let mut display_message: Option<String> = None;
    let response = match command {
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::List => {
            match crate::im_gateway::external_cli::load_external_cli_model_catalog(
                &request.adapter,
                &request.adapter_config,
                request.work_dir.as_deref(),
            )
            .await
            {
                Ok(models) => crate::im_gateway::external_cli::format_external_cli_model_catalog(
                    &request.adapter,
                    &models,
                ),
                Err(error) => format!("无法获取 {adapter_label} 模型列表：{error}"),
            }
        }
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::Show => {
            let (model, source) = current_session_model_override(request, &effective.runner_id)
                .unwrap_or_else(|| {
                    let resolved =
                        crate::im_gateway::external_cli::resolve_external_cli_model_config(
                            &request.adapter,
                            &request.adapter_config,
                        );
                    (resolved.model, resolved.model_source)
                });
            crate::im_gateway::external_cli::format_external_cli_model_status(
                &request.adapter,
                model.as_deref(),
                source.as_deref(),
                &effective.runner_id,
            )
        }
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::Clear => {
            persist_session_model_override(request, &effective.runner_id, None);
            display_message = Some("清除模型切换".to_string());
            format!(
                "已清除 {adapter_label} Runner `{}` 的 session 模型 override。下一条消息将使用 Runner 配置或 {adapter_label} 默认模型。",
                effective.runner_id,
            )
        }
        crate::im_gateway::external_cli::ExternalCliModelSlashCommand::Set(model) => {
            let models = match crate::im_gateway::external_cli::load_external_cli_model_catalog(
                &request.adapter,
                &request.adapter_config,
                request.work_dir.as_deref(),
            )
            .await
            {
                Ok(models) => models,
                Err(error) => {
                    let response =
                        format!("未切换模型：无法验证 {adapter_label} 模型 `{model}`：{error}");
                    remember_model_slash_result_state(request, &effective.runner_id, &response);
                    return Some(model_slash_success_response(&response, stream));
                }
            };
            let model = match crate::im_gateway::external_cli::validate_external_cli_model_selection(
                &request.adapter,
                &model,
                &models,
            ) {
                Ok(model) => model,
                Err(response) => {
                    remember_model_slash_result_state(request, &effective.runner_id, &response);
                    return Some(model_slash_success_response(&response, stream));
                }
            };
            persist_session_model_override(request, &effective.runner_id, Some(model.clone()));
            display_message = Some(format!("切换模型为 {model}"));
            format!(
                "已将 {adapter_label} Runner `{}` 的 session 模型设置为 `{}`。\n下一条消息会通过 `--model {}` 启动。",
                effective.runner_id, model, model,
            )
        }
    };
    remember_model_slash_result_state(
        request,
        &effective.runner_id,
        display_message.as_deref().unwrap_or(&response),
    );
    Some(model_slash_success_response(&response, stream))
}

async fn maybe_external_cli_slash_response(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
    stream: bool,
) -> Option<Response<BoxBody>> {
    if let Some(response) =
        maybe_external_cli_resume_slash_response(request, effective, stream).await
    {
        return Some(response);
    }
    if let Some(response) =
        maybe_external_cli_model_slash_response(request, effective, stream).await
    {
        return Some(response);
    }
    maybe_external_cli_effort_slash_response(request, effective, stream).await
}

async fn maybe_external_cli_resume_slash_response(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
    stream: bool,
) -> Option<Response<BoxBody>> {
    let command =
        crate::im_gateway::external_cli::parse_external_cli_resume_slash_command(&request.message)?;
    let command = match command {
        Ok(command) => command,
        Err(error) => return Some(model_slash_error_response(&error, stream)),
    };
    let selection = request.session_key.as_ref().map(|session_key| {
        crate::im_gateway::external_cli::LocalSessionSelectionContext {
            session_key: session_key.clone(),
            runner_id: effective.runner_id.clone(),
        }
    });
    let response = match crate::im_gateway::external_cli::execute_local_session_resume_command(
        request.adapter.clone(),
        command,
        selection,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => return Some(model_slash_error_response(&error, stream)),
    };
    remember_model_slash_result_state(request, &effective.runner_id, &response);
    Some(model_slash_success_response(&response, stream))
}

async fn maybe_external_cli_effort_slash_response(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
    stream: bool,
) -> Option<Response<BoxBody>> {
    let command =
        crate::im_gateway::external_cli::parse_external_cli_effort_slash_command(&request.message)?;
    let command = match command {
        Ok(command) => command,
        Err(error) => return Some(model_slash_error_response(&error, stream)),
    };
    if crate::im_gateway::external_cli::external_cli_effort_options(&request.adapter).is_empty() {
        return Some(model_slash_error_response(
            "/effort 当前仅支持 Codex、Traex 或 Claude Code Runner。",
            stream,
        ));
    }
    let adapter_label =
        crate::im_gateway::external_cli::external_cli_model_adapter_label(&request.adapter);
    let resolved_model_config = crate::im_gateway::external_cli::resolve_external_cli_model_config(
        &request.adapter,
        &request.adapter_config,
    );
    let model_catalog = crate::im_gateway::external_cli::load_external_cli_model_catalog(
        &request.adapter,
        &request.adapter_config,
        request.work_dir.as_deref(),
    )
    .await
    .unwrap_or_default();
    let mut display_message: Option<String> = None;
    let response = match command {
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::List => {
            crate::im_gateway::external_cli::format_external_cli_effort_catalog_for_model(
                &request.adapter,
                resolved_model_config.model.as_deref(),
                &model_catalog,
            )
        }
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::Show => {
            let (effort, source) =
                current_session_reasoning_effort_override(request, &effective.runner_id)
                    .unwrap_or_else(|| {
                        (
                            resolved_model_config.reasoning_effort.clone(),
                            resolved_model_config.reasoning_source.clone(),
                        )
                    });
            crate::im_gateway::external_cli::format_external_cli_effort_status(
                &request.adapter,
                effort.as_deref(),
                source.as_deref(),
                &effective.runner_id,
            )
        }
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::Clear => {
            persist_session_reasoning_effort_override(request, &effective.runner_id, None);
            display_message = Some("清除 Reasoning Effort 切换".to_string());
            format!(
                "已清除 {adapter_label} Runner `{}` 的 session Reasoning Effort override。下一条消息将使用 Runner 配置或 {adapter_label} 默认值。",
                effective.runner_id,
            )
        }
        crate::im_gateway::external_cli::ExternalCliEffortSlashCommand::Set(effort) => {
            let effort =
                match crate::im_gateway::external_cli::validate_external_cli_effort_selection_for_model(
                    &request.adapter,
                    &effort,
                    resolved_model_config.model.as_deref(),
                    &model_catalog,
                ) {
                    Ok(effort) => effort,
                    Err(response) => {
                        remember_model_slash_result_state(request, &effective.runner_id, &response);
                        return Some(model_slash_success_response(&response, stream));
                    }
                };
            persist_session_reasoning_effort_override(
                request,
                &effective.runner_id,
                Some(effort.clone()),
            );
            display_message = Some(format!("切换 Reasoning Effort 为 {effort}"));
            format!(
                "已将 {adapter_label} Runner `{}` 的 session Reasoning Effort 设置为 `{}`。下一条消息会使用该推理强度启动。",
                effective.runner_id, effort,
            )
        }
    };
    remember_model_slash_result_state(
        request,
        &effective.runner_id,
        display_message.as_deref().unwrap_or(&response),
    );
    Some(model_slash_success_response(&response, stream))
}

fn model_slash_success_response(response: &str, stream: bool) -> Response<BoxBody> {
    if !stream {
        return json_response(&serde_json::json!({
            "status": "succeeded",
            "response": response,
        }));
    }
    let assistant = serde_json::json!({
        "eventType": "assistant_final",
        "content": response,
    });
    let finished = serde_json::json!({
        "eventType": "run_finished",
        "status": "succeeded",
        "response": response,
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .body(crate::handlers::full_body(format!(
            "{assistant}\n{finished}\n"
        )))
        .unwrap()
}

fn model_slash_error_response(error: &str, stream: bool) -> Response<BoxBody> {
    if !stream {
        return error_response(StatusCode::BAD_REQUEST, error);
    }
    let failed = serde_json::json!({
        "eventType": "run_failed",
        "error": error,
    });
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header("Content-Type", "application/x-ndjson")
        .body(crate::handlers::full_body(format!("{failed}\n")))
        .unwrap()
}

fn current_session_model_override(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    let session_key = request.session_key.as_deref()?;
    let state = crate::im_gateway::session_state::load_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
    )?;
    if state.model_override.is_none() && state.model_override_source.is_none() {
        return None;
    }
    Some((state.model_override, state.model_override_source))
}

fn current_session_reasoning_effort_override(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    let session_key = request.session_key.as_deref()?;
    let state = crate::im_gateway::session_state::load_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
    )?;
    if state.reasoning_effort_override.is_none() && state.reasoning_effort_override_source.is_none()
    {
        return None;
    }
    Some((
        state.reasoning_effort_override,
        state.reasoning_effort_override_source,
    ))
}

fn persist_session_model_override(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    model: Option<String>,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let source = model.as_ref().map(|_| "session slash command".to_string());
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
        |state| {
            state.model_override = model;
            state.model_override_source = source;
        },
    ) {
        warn!(
            session_key = %session_key,
            adapter = %request.adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist external CLI model override"
        );
    }
}

fn persist_session_reasoning_effort_override(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    effort: Option<String>,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let source = effort.as_ref().map(|_| "session slash command".to_string());
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
        |state| {
            state.reasoning_effort_override = effort;
            state.reasoning_effort_override_source = source;
        },
    ) {
        warn!(
            session_key = %session_key,
            adapter = %request.adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist external CLI reasoning effort override"
        );
    }
}

fn remember_model_slash_result_state(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    response: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let trimmed_message = request.message.trim();
    let title = if trimmed_message.eq_ignore_ascii_case("/models") {
        format!(
            "{} models",
            crate::im_gateway::external_cli::external_cli_model_adapter_label(&request.adapter)
        )
    } else {
        trimmed_message.to_string()
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        &request.adapter,
        Some(runner_id),
        |state| {
            state.title.get_or_insert(title);
            if state.last_user_message.is_none() {
                state.last_user_message = Some(trimmed_message.to_string());
            }
            state.status = Some("ended".to_string());
            append_display_message_once(state, "system", response, timestamp);
        },
    ) {
        warn!(
            session_key = %session_key,
            adapter = %request.adapter,
            runner_id = %runner_id,
            error = %error,
            "failed to persist external CLI model slash response"
        );
    }
}

fn append_display_message_once(
    state: &mut crate::im_gateway::session_state::ImAgentSessionState,
    role: &str,
    content: &str,
    timestamp: u64,
) {
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    if state
        .messages
        .last()
        .is_some_and(|message| message.role == role && message.content == content)
    {
        return;
    }
    state
        .messages
        .push(crate::im_gateway::session_state::ImAgentSessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Some(timestamp),
            content_parts: None,
        });
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
    crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_run_request(
        request,
        Some(&state),
    );
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
                &request.files,
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
                &request.files,
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
    let (mut recorder, created) =
        match bifrost_agent::persistence::ConversationRecorder::open_or_create(
            &data_dir,
            session_key,
            None,
        ) {
            Ok(opened) => opened,
            Err(error) => {
                tracing::warn!(
                    session_key = %session_key,
                    adapter = %request.adapter,
                    runner_id = %runner_id,
                    error = %error,
                    "failed to open canonical external runner timeline"
                );
                return None;
            }
        };
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
    if created {
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
    }
    Some(recorder)
}

fn emit_external_cli_timeline_changed_from_request(
    agent_session_manager: &bifrost_agent::AgentSessionManager,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    reason: &str,
) {
    let Some(session_key) = request.session_key.as_deref() else {
        return;
    };
    let Some(history_path) = request_history_path(request)
        .or_else(|| persisted_history_path_for_request(request, runner_id))
    else {
        return;
    };
    agent_session_manager.emit_timeline_changed(session_key, &history_path, None, reason);
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
    let images = external_cli_request_chat_images(request);
    if let Err(error) =
        recorder.record_user_message_with_images(session_key, &request.message, &images)
    {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner user message");
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
    if !result.response.trim().is_empty() {
        if let Err(error) = recorder.record_assistant_message(session_key, &result.response) {
            tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner assistant message");
        }
    }
    if let Err(error) =
        recorder.record_run_state(session_key, run_state, Some("web"), Some(runner_id))
    {
        tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner final state");
    }
}

fn record_external_cli_web_progress_event(
    recorder: Option<&mut bifrost_agent::persistence::ConversationRecorder>,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
    event: &crate::im_gateway::external_cli::ExternalCliProgressEvent,
) -> Option<usize> {
    let session_key = request.session_key.as_deref()?;
    let recorder = recorder?;
    record_external_cli_progress_event_to_timeline(
        recorder,
        session_key,
        "web",
        runner_id,
        &request.adapter,
        event,
    )
}

pub(super) fn record_external_cli_progress_event_to_timeline(
    recorder: &mut bifrost_agent::persistence::ConversationRecorder,
    session_key: &str,
    source_channel: &str,
    runner_id: &str,
    adapter: &str,
    event: &crate::im_gateway::external_cli::ExternalCliProgressEvent,
) -> Option<usize> {
    use crate::im_gateway::external_cli::ExternalCliProgressEventType as EventType;

    let mut changed = false;
    match event.event_type {
        EventType::RunStarted => {
            if let Err(error) = recorder.record_run_state(
                session_key,
                "running",
                Some(source_channel),
                Some(runner_id),
            ) {
                tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner progress state");
            } else {
                changed = true;
            }
        }
        EventType::Status => {
            let content = event.content.trim();
            if !content.is_empty()
                && content != "turn started"
                && content != "turn completed"
                && !crate::im_gateway::progress_card::is_token_usage_machine_status(content)
            {
                let message = if let Some(title) = event
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    format!("{title}: {content}")
                } else {
                    content.to_string()
                };
                if let Err(error) = recorder.record_assistant_delta(session_key, &message) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner status delta");
                } else {
                    changed = true;
                }
            }
        }
        EventType::AssistantDelta => {
            if !event.content.trim().is_empty() {
                if let Err(error) = recorder.record_assistant_delta(session_key, &event.content) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner assistant delta");
                } else {
                    changed = true;
                }
            }
        }
        EventType::PlanUpdated => {
            let steps = crate::im_gateway::external_cli::external_progress_plan_steps(event);
            if !steps.is_empty() {
                if let Err(error) =
                    recorder.record_plan_updated(session_key, &steps, event.title.as_deref())
                {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner plan update");
                } else {
                    changed = true;
                }
            }
        }
        EventType::SubAgentUpdated => {
            if let Some(progress) =
                crate::im_gateway::external_cli::external_progress_subagent(event)
            {
                if let Err(error) = recorder.record_subagent_updated(session_key, &progress) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner subagent update");
                } else {
                    changed = true;
                }
            }
        }
        EventType::ToolStarted => {
            let call_id = external_progress_call_id(adapter, event);
            let tool_name = external_progress_tool_name(event, adapter);
            let arguments = external_progress_tool_arguments(event);
            if !timeline_has_tool_call(recorder, session_key, &call_id) {
                if let Err(error) = recorder.record_tool_call_with_id(
                    session_key,
                    &tool_name,
                    &arguments,
                    Some(&call_id),
                ) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner tool started");
                } else {
                    changed = true;
                }
            }
        }
        EventType::ToolFinished => {
            let call_id = external_progress_call_id(adapter, event);
            let tool_name = external_progress_tool_name(event, adapter);
            let arguments = external_progress_tool_arguments(event);
            if !timeline_has_tool_call(recorder, session_key, &call_id) {
                if let Err(error) = recorder.record_tool_call_with_id(
                    session_key,
                    &tool_name,
                    &arguments,
                    Some(&call_id),
                ) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner completed tool call");
                } else {
                    changed = true;
                }
            }
            let success = event
                .raw
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let result = if event.content.trim().is_empty() {
                event.raw.to_string()
            } else {
                event.content.clone()
            };
            if let Err(error) = recorder.record_tool_result_with_call_id(
                session_key,
                &tool_name,
                &result,
                success,
                Some(&call_id),
            ) {
                tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner tool finished");
            } else {
                changed = true;
            }
        }
        EventType::RunFinished => {
            // stdout progress can contain `turn.completed` before the runner has flushed the
            // final response into `last_message.md` / result state. Keep the canonical session
            // running until `record_external_cli_web_turn_result` writes the final assistant
            // message and terminal run_state together.
        }
        EventType::RunFailed => {
            if let Err(error) = recorder.record_run_state(
                session_key,
                "failed",
                Some(source_channel),
                Some(runner_id),
            ) {
                tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner failed progress state");
            } else {
                changed = true;
            }
            if !event.content.trim().is_empty() {
                let message = format!("Runner failed: {}", event.content.trim());
                if let Err(error) = recorder.record_assistant_delta(session_key, &message) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner failure delta");
                } else {
                    changed = true;
                }
            }
        }
        EventType::AssistantFinal => {
            if !event.content.trim().is_empty() {
                if let Err(error) = recorder.record_assistant_delta(session_key, &event.content) {
                    tracing::warn!(session_key = %session_key, error = %error, "failed to record external runner assistant content");
                } else {
                    changed = true;
                }
            }
        }
    }
    changed.then(|| recorder.event_count()).flatten()
}

fn timeline_has_tool_call(
    recorder: &bifrost_agent::persistence::ConversationRecorder,
    session_key: &str,
    call_id: &str,
) -> bool {
    bifrost_agent::persistence::load_conversation_events(recorder.file_path())
        .map(|events| {
            events.iter().any(|event| {
                event.session_key == session_key
                    && event.event_type == bifrost_agent::persistence::event_types::TOOL_CALL
                    && event
                        .content
                        .get("call_id")
                        .and_then(|value| value.as_str())
                        == Some(call_id)
            })
        })
        .unwrap_or(false)
}

fn external_progress_call_id(
    adapter: &str,
    event: &crate::im_gateway::external_cli::ExternalCliProgressEvent,
) -> String {
    event
        .raw
        .get("call_id")
        .or_else(|| event.raw.get("id"))
        .or_else(|| event.raw.get("item").and_then(|item| item.get("id")))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "{}-{}-{}",
                adapter,
                external_progress_tool_name(event, "runner"),
                stable_json_hash(&event.raw)
            )
        })
}

fn external_progress_tool_name(
    event: &crate::im_gateway::external_cli::ExternalCliProgressEvent,
    default: &str,
) -> String {
    event
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            event
                .raw
                .get("tool_name")
                .or_else(|| event.raw.get("name"))
                .or_else(|| event.raw.get("item").and_then(|item| item.get("name")))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(default)
        .to_string()
}

fn external_progress_tool_arguments(
    event: &crate::im_gateway::external_cli::ExternalCliProgressEvent,
) -> String {
    event
        .raw
        .get("arguments")
        .or_else(|| event.raw.get("args"))
        .or_else(|| event.raw.get("item").and_then(|item| item.get("arguments")))
        .or_else(|| event.raw.get("item").and_then(|item| item.get("args")))
        .map(serde_json::Value::to_string)
        .unwrap_or_else(|| {
            if event.content.trim().is_empty() {
                "{}".to_string()
            } else {
                serde_json::json!({ "content": event.content }).to_string()
            }
        })
}

fn stable_json_hash(value: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    hasher.finish()
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
    let user_message = image_message_preview(&request.message, &request.images, &request.files);
    if user_message.is_empty() {
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

fn apply_agent_instructions_to_external_cli_request(
    service: &ImGatewayService,
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    runner_id: &str,
) {
    let base_agent_config = service.agent_config_store.load();
    let agent_config = request
        .provider_id
        .as_deref()
        .and_then(|provider_id| service.provider_store.get(provider_id))
        .map(|provider| effective_agent_config_for_provider(&base_agent_config, &provider))
        .unwrap_or(base_agent_config);
    let include_base_instructions = request
        .session_key
        .as_deref()
        .and_then(|session_key| {
            crate::im_gateway::session_state::load_session_state(
                session_key,
                &request.adapter,
                Some(runner_id),
            )
        })
        .is_none_or(|state| {
            state.last_user_message.is_none()
                && !state.messages.iter().any(|message| message.role == "user")
        });
    request.instructions =
        crate::im_gateway::external_cli::compose_external_cli_message_instructions(
            include_base_instructions,
            agent_config.base_instructions.as_deref(),
            agent_config.developer_instructions.as_deref(),
            agent_config.user_instructions.as_deref(),
            request.instructions.as_deref(),
        );
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

    async fn spawn_chat_gateway_test_server(
        service: crate::SharedImGatewayService,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind chat gateway test server");
        let address = listener.local_addr().expect("chat gateway address");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    let handler = service_fn(move |request: Request<Incoming>| {
                        let service = Arc::clone(&service);
                        async move {
                            Ok::<_, hyper::Error>(
                                handle_chat_gateway(request, &service, "/stream").await,
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), handler)
                        .await;
                });
            }
        });
        (format!("http://{address}"), handle)
    }

    fn configure_mock_web_runner(
        service: &ImGatewayService,
        executable: &str,
        args: Vec<String>,
        delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode,
    ) {
        let mut config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
        let runner = config
            .runners
            .get_mut(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID)
            .expect("default runner");
        runner.enabled = true;
        runner.adapter = "mock".to_string();
        runner.inject_bifrost_tools = false;
        runner.delivery_mode = delivery_mode;
        runner.adapter_config = crate::im_gateway::external_cli::ExternalCliAdapterConfig {
            executable: Some(executable.to_string()),
            args,
            timeout_secs: Some(10),
            ..Default::default()
        };
        service
            .external_cli_config_store
            .save(config)
            .expect("save mock runner");
    }

    fn bound_progress_provider(id: &str, owner_open_id: Option<&str>) -> ImProviderConfig {
        ImProviderConfig {
            id: id.to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: id.to_string(),
            enabled: true,
            base_url: None,
            app_id: None,
            secret_ref: None,
            owner_open_id: owner_open_id.map(str::to_string),
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn bound_group_event(provider_id: &str, chat_id: &str) -> ImEvent {
        ImEvent {
            event_id: "event-bound-group".to_string(),
            provider_id: provider_id.to_string(),
            provider_type: ImProviderType::Feishu,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some(chat_id.to_string()),
                chat_type: Some("group".to_string()),
                user_id: Some("ou_member".to_string()),
                user_name: Some("Member".to_string()),
                sender_type: Some("user".to_string()),
                message_id: Some("om-bound-group".to_string()),
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: "bind group".to_string(),
                raw_type: Some("text".to_string()),
                create_time: Some(1),
                ..Default::default()
            }),
            received_at: 1,
            raw_digest: None,
        }
    }

    #[test]
    fn web_im_progress_target_resolves_bound_private_chat_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let service = ImGatewayService::new(temp_dir.path());
        let provider = bound_progress_provider("feishu-private", Some("ou_owner"));
        service.provider_store.add(provider).unwrap();

        let resolved =
            bound_web_im_progress_target(&service, Some("feishu-private:ou_owner")).unwrap();
        assert_eq!(resolved.provider.id, "feishu-private");
        assert_eq!(resolved.target.receive_id_type, "open_id");
        assert_eq!(resolved.target.receive_id, "ou_owner");
        assert!(bound_web_im_progress_target(&service, Some("admin-chat-1")).is_none());
        assert!(
            bound_web_im_progress_target(&service, Some("feishu-private:ou_someone_else"))
                .is_none()
        );
    }

    #[test]
    fn web_im_progress_target_uses_persisted_group_binding() {
        let temp_dir = tempfile::tempdir().unwrap();
        let service = ImGatewayService::new(temp_dir.path());
        let provider = bound_progress_provider("feishu-group", Some("ou_owner"));
        service.provider_store.add(provider).unwrap();
        let event = bound_group_event("feishu-group", "oc_team");
        service
            .group_context_store
            .record_event(&event, "test")
            .unwrap();
        service
            .group_context_store
            .set_chat_name("feishu-group", "oc_team", "Team Chat", 2)
            .unwrap();

        let resolved = bound_web_im_progress_target(
            &service,
            Some(&crate::im_gateway::group_context::build_group_session_key(
                "feishu-group",
                "oc_team",
            )),
        )
        .unwrap();
        assert_eq!(resolved.provider.id, "feishu-group");
        assert_eq!(resolved.target.receive_id_type, "chat_id");
        assert_eq!(resolved.target.receive_id, "oc_team");
        assert_eq!(resolved.target.display_name, "Team Chat");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bound_web_turn_sends_direct_progress_card_and_final_output() {
        let server =
            crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
        let registry = Arc::new(ImAgentProgressRegistry::new());
        let provider =
            crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
        let target = crate::im_gateway::progress_card::tests::mock_progress_target();
        let context = WebImProgressContext {
            binding: BoundWebImProgressTarget { provider, target },
            registry,
            feishu: Arc::new(crate::im_gateway::feishu::FeishuProvider::new()),
        };
        let mut request = timeline_test_request("feishu-main:ou_owner");
        request.message = "message from WebUI".to_string();
        let progress = start_bound_web_im_progress(Some(&context), &request, "codex")
            .await
            .expect("bound progress");
        progress
            .forwarder
            .forward(&crate::im_gateway::external_cli::ExternalCliProgressEvent {
                event_type:
                    crate::im_gateway::external_cli::ExternalCliProgressEventType::AssistantDelta,
                content: "working from shared session".to_string(),
                title: None,
                raw: serde_json::Value::Null,
            });
        progress
            .finish("final from WebUI".to_string(), false, None)
            .await;

        assert_eq!(
            server
                .message_paths
                .lock()
                .expect("message paths")
                .as_slice(),
            ["/open-apis/im/v1/messages"]
        );
        let payloads = server.message_payloads.lock().expect("message payloads");
        assert_eq!(payloads[0]["receive_id"], "ou_owner");
        drop(payloads);
        assert!(server
            .card_update_payloads
            .lock()
            .expect("card update payloads")
            .iter()
            .any(|payload| payload.contains("final from WebUI")));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn bound_web_stream_handler_mirrors_existing_bound_progress_card() {
        let server =
            crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
        let harness = crate::test_support::TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let mut provider = bound_progress_provider("feishu-main", Some("ou_owner"));
        provider.owner_open_id = Some("ou_owner".to_string());
        service.provider_store.add(provider).expect("add provider");
        assert!(
            bound_web_im_progress_target(&service, Some("feishu-main:ou_owner")).is_some(),
            "canonical private session must resolve before HTTP dispatch"
        );
        configure_mock_web_runner(
            &service,
            "sh",
            vec![
                "-c".to_string(),
                concat!(
                    "cat >/dev/null; ",
                    "printf '%s\\n' ",
                    "'{\"type\":\"assistant_delta\",\"delta\":\"working from handler\"}' ",
                    "'{\"type\":\"assistant_final\",\"content\":\"final from handler\"}'"
                )
                .to_string(),
            ],
            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard,
        );
        service
            .progress_registry
            .start_feishu(
                "feishu-main:ou_owner",
                Arc::clone(service.connection_manager.feishu_provider()),
                crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url),
                crate::im_gateway::progress_card::tests::mock_progress_target(),
                "seed bound card",
            )
            .await
            .expect("seed bound progress card");
        let (base_url, gateway) = spawn_chat_gateway_test_server(Arc::clone(&service)).await;

        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build local test client")
            .post(base_url)
            .json(&serde_json::json!({
                "message": "message from WebUI handler",
                "runnerId": crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID,
                "providerId": "untrusted-provider",
                "sessionKey": "feishu-main:ou_owner",
                "runtime": "external_cli"
            }))
            .send()
            .await
            .expect("send Web stream request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.expect("Web stream body");
        assert!(body.contains("working from handler"), "{body}");
        assert!(body.contains("final from handler"), "{body}");
        assert!(server
            .card_update_payloads
            .lock()
            .expect("card updates")
            .iter()
            .any(|payload| payload.contains("final from handler")));
        gateway.abort();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn bound_web_stream_handler_finishes_existing_im_card_on_runner_failure() {
        let server =
            crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
        let harness = crate::test_support::TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let mut provider = bound_progress_provider("feishu-main", Some("ou_owner"));
        provider.owner_open_id = Some("ou_owner".to_string());
        service.provider_store.add(provider).expect("add provider");
        assert!(
            bound_web_im_progress_target(&service, Some("feishu-main:ou_owner")).is_some(),
            "canonical private session must resolve before HTTP dispatch"
        );
        configure_mock_web_runner(
            &service,
            "/definitely/missing/bifrost-cross-channel-runner",
            Vec::new(),
            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard,
        );
        service
            .progress_registry
            .start_feishu(
                "feishu-main:ou_owner",
                Arc::clone(service.connection_manager.feishu_provider()),
                crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url),
                crate::im_gateway::progress_card::tests::mock_progress_target(),
                "seed failing bound card",
            )
            .await
            .expect("seed bound progress card");
        let (base_url, gateway) = spawn_chat_gateway_test_server(Arc::clone(&service)).await;

        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build local test client")
            .post(base_url)
            .json(&serde_json::json!({
                "message": "fail this bound WebUI turn",
                "sessionKey": "feishu-main:ou_owner",
                "runtime": "external_cli"
            }))
            .send()
            .await
            .expect("send failing Web stream request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.expect("failing Web stream body");
        assert!(body.contains("run_failed"), "{body}");
        assert!(server
            .card_update_payloads
            .lock()
            .expect("card updates")
            .iter()
            .any(|payload| payload.contains("Agent 执行失败")));
        gateway.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn web_guide_queue_fallback_updates_bound_im_progress_card() {
        let server =
            crate::im_gateway::progress_card::tests::spawn_mock_feishu_progress_server().await;
        let temp_dir = tempfile::tempdir().unwrap();
        let service = ImGatewayService::new(temp_dir.path());
        let provider =
            crate::im_gateway::progress_card::tests::mock_feishu_provider(&server.base_url);
        let target = crate::im_gateway::progress_card::tests::mock_progress_target();
        let context = WebImProgressContext {
            binding: BoundWebImProgressTarget { provider, target },
            registry: Arc::clone(&service.progress_registry),
            feishu: Arc::clone(service.connection_manager.feishu_provider()),
        };
        let mut request = timeline_test_request("feishu-main:ou_owner");
        request.message = "message from WebUI".to_string();
        let progress = start_bound_web_im_progress(Some(&context), &request, "codex")
            .await
            .expect("bound progress");
        assert!(service
            .agent_session_manager
            .try_start_external_session_preview(
                "feishu-main:ou_owner",
                Some("active".to_string()),
                None,
                Some("admin-api".to_string()),
                Some("codex".to_string()),
                Some("codex".to_string()),
            ));

        let payload = response_json(
            guide_external_cli_session(
                &service,
                "feishu-main:ou_owner",
                "guide-web".to_string(),
                "focus from WebUI".to_string(),
            )
            .await,
        )
        .await;
        assert_eq!(payload["delivery"], "queued");
        assert_eq!(payload["queueLength"], 1);
        let card_payloads = server
            .card_create_payloads
            .lock()
            .expect("card create payloads")
            .iter()
            .cloned()
            .chain(
                server
                    .card_update_payloads
                    .lock()
                    .expect("card update payloads")
                    .iter()
                    .cloned(),
            )
            .collect::<Vec<_>>();
        assert!(card_payloads.iter().any(|payload| {
            payload.contains("WebUI 引导无法实时注入") && payload.contains("focus from WebUI")
        }));
        let accepted_result = crate::im_gateway::external_cli::ExternalCliGuideResult {
            guide_id: "guide-accepted".to_string(),
            accepted: true,
            thread_id: Some("thread-accepted".to_string()),
            turn_id: Some("turn-accepted".to_string()),
            reason: None,
        };
        let api_payload = accepted_web_guide_payload(
            &service,
            "feishu-main:ou_owner",
            "live focus from WebUI",
            &accepted_result,
            false,
        )
        .await;
        assert_eq!(api_payload["delivery"], "steered");
        assert!(api_payload.get("eventType").is_none());
        let stream_payload = accepted_web_guide_payload(
            &service,
            "feishu-main:ou_owner",
            "second live focus from WebUI",
            &accepted_result,
            true,
        )
        .await;
        assert_eq!(stream_payload["eventType"], "run_finished");
        assert_eq!(stream_payload["guideId"], "guide-accepted");

        service
            .agent_session_manager
            .clear_active_session_preview("feishu-main:ou_owner");
        progress
            .finish("final after queued guide".to_string(), false, None)
            .await;
    }

    fn timeline_test_request(
        session_key: &str,
    ) -> crate::im_gateway::external_cli::ExternalCliRunRequest {
        crate::im_gateway::external_cli::ExternalCliRunRequest {
            message: "timeline".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            operation: "chat".to_string(),
            params: serde_json::Value::Null,
            provider_id: None,
            runner_id: Some("codex".to_string()),
            session_key: Some(session_key.to_string()),
            runtime: "external_cli".to_string(),
            adapter: "codex".to_string(),
            work_dir: None,
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        }
    }

    #[test]
    fn chat_gateway_agent_instructions_include_base_only_on_first_message() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let service = ImGatewayService::new(temp_dir.path());
        let mut agent_config = service.agent_config_store.load();
        agent_config.base_instructions = Some("base".to_string());
        agent_config.developer_instructions = Some("developer".to_string());
        agent_config.user_instructions = Some("user".to_string());
        service
            .agent_config_store
            .save(&agent_config)
            .expect("save agent config");

        let mut first = timeline_test_request("web-instruction-lifecycle");
        first.instructions = Some("runner".to_string());
        apply_agent_instructions_to_external_cli_request(&service, &mut first, "codex");
        assert_eq!(
            first.instructions.as_deref(),
            Some("base\n\ndeveloper\n\nuser\n\nrunner")
        );
        remember_external_cli_started_state(&first, "codex");

        let mut second = timeline_test_request("web-instruction-lifecycle");
        second.message = "second".to_string();
        second.instructions = Some("runner".to_string());
        apply_agent_instructions_to_external_cli_request(&service, &mut second, "codex");
        assert_eq!(
            second.instructions.as_deref(),
            Some("developer\n\nuser\n\nrunner")
        );
    }

    #[test]
    fn external_timeline_recorder_rejects_an_unremovable_canonical_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "web-recorder-invalid-path";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let canonical =
            bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key);
        std::fs::create_dir_all(canonical).unwrap();

        assert!(
            external_cli_timeline_recorder(&timeline_test_request(session_key), "codex").is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_timeline_recorder_handles_session_start_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "web-recorder-readonly";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let parent =
            bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key)
                .parent()
                .unwrap()
                .to_path_buf();
        std::fs::create_dir_all(&parent).unwrap();
        let original_mode = std::fs::metadata(&parent).unwrap().permissions().mode();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();

        let recorder = external_cli_timeline_recorder(&timeline_test_request(session_key), "codex")
            .expect("recorder allocation is lazy");

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(original_mode)).unwrap();
        assert!(!recorder.file_path().exists());
    }

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

        let queued = response_json(
            queue_external_cli_stream_response(&service, "web-queue-delete", "/q queued follow up")
                .await,
        )
        .await;
        assert_eq!(queued["queued"], true);
        assert_eq!(queued["queueLength"], 1);
        assert_eq!(
            service.queue_manager.queue_status("web-queue-delete")[0].message,
            "queued follow up"
        );

        let removed = response_json(
            queue_external_cli_stream_response(&service, "web-queue-delete", "/rq 1").await,
        )
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
            files: Vec::new(),
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
            files: Vec::new(),
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
            files: Vec::new(),
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
    fn external_runner_progress_events_are_recorded_as_visible_timeline_steps() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "external-progress-history";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(
                session_key,
                serde_json::json!({"source": "admin-api", "adapter": "traex"}),
            )
            .expect("record start");

        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &crate::im_gateway::external_cli::ExternalCliProgressEvent {
                event_type: crate::im_gateway::external_cli::ExternalCliProgressEventType::Status,
                content: "model rerouted".to_string(),
                title: Some("status".to_string()),
                raw: serde_json::json!({"type":"item.completed"}),
            },
        );
        for (title, content) in [
            ("token_usage", "token usage updated"),
            ("rate_limits", "usage updated"),
        ] {
            record_external_cli_progress_event_to_timeline(
                &mut recorder,
                session_key,
                "web",
                "traex",
                "traex",
                &crate::im_gateway::external_cli::ExternalCliProgressEvent {
                    event_type:
                        crate::im_gateway::external_cli::ExternalCliProgressEventType::Status,
                    content: content.to_string(),
                    title: Some(title.to_string()),
                    raw: serde_json::json!({"type": title}),
                },
            );
        }
        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &crate::im_gateway::external_cli::ExternalCliProgressEvent {
                event_type:
                    crate::im_gateway::external_cli::ExternalCliProgressEventType::AssistantFinal,
                content: "I will inspect the diff first.".to_string(),
                title: Some("agent_message".to_string()),
                raw: serde_json::json!({
                    "type": "item.completed",
                    "item": {"type": "agent_message"}
                }),
            },
        );
        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &crate::im_gateway::external_cli::ExternalCliProgressEvent {
                event_type:
                    crate::im_gateway::external_cli::ExternalCliProgressEventType::ToolFinished,
                content: "pwd ok".to_string(),
                title: Some("exec_command".to_string()),
                raw: serde_json::json!({
                    "type": "tool_finished",
                    "id": "tool-1",
                    "arguments": {"cmd": "pwd"},
                    "success": true
                }),
            },
        );

        let events = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load progress events");
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_DELTA
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("status: model rerouted")
        }));
        assert!(!events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_DELTA
                && event
                    .content
                    .get("message")
                    .and_then(|value| value.as_str())
                    .is_some_and(|message| {
                        message.contains("token_usage") || message.contains("rate_limits")
                    })
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_DELTA
                && event.content.get("message").and_then(|v| v.as_str())
                    == Some("I will inspect the diff first.")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::TOOL_CALL
                && event.content.get("tool_name").and_then(|v| v.as_str()) == Some("exec_command")
                && event.content.get("call_id").and_then(|v| v.as_str()) == Some("tool-1")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::TOOL_RESULT
                && event.content.get("result").and_then(|v| v.as_str()) == Some("pwd ok")
                && event.content.get("success").and_then(|v| v.as_bool()) == Some(true)
        }));
    }

    #[test]
    fn external_runner_progress_run_finished_does_not_complete_before_final_response() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "external-progress-finished-before-final";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(
                session_key,
                serde_json::json!({"source": "admin-api", "adapter": "traex"}),
            )
            .expect("record start");

        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &crate::im_gateway::external_cli::ExternalCliProgressEvent {
                event_type:
                    crate::im_gateway::external_cli::ExternalCliProgressEventType::RunStarted,
                content: "turn started".to_string(),
                title: Some("Codex turn".to_string()),
                raw: serde_json::json!({"type":"turn.started"}),
            },
        );
        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &crate::im_gateway::external_cli::ExternalCliProgressEvent {
                event_type:
                    crate::im_gateway::external_cli::ExternalCliProgressEventType::RunFinished,
                content: "turn completed".to_string(),
                title: Some("Codex turn".to_string()),
                raw: serde_json::json!({"type":"turn.completed"}),
            },
        );

        let summary = bifrost_agent::persistence::scan_session_summary(recorder.file_path());
        assert_eq!(summary.run_state.as_deref(), Some("running"));
        let events = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load progress events");
        assert!(!events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::RUN_STATE_CHANGED
                && event.content.get("state").and_then(|value| value.as_str()) == Some("completed")
        }));
    }

    #[test]
    fn external_runner_final_result_records_message_before_completed_state() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            files: Vec::new(),
            message: "explain the image".to_string(),
            operation: "chat".to_string(),
            params: serde_json::Value::Null,
            provider_id: None,
            runner_id: Some("traex".to_string()),
            session_key: Some("traex-final-order-session".to_string()),
            runtime: "local".to_string(),
            adapter: "traex".to_string(),
            work_dir: Some(std::path::PathBuf::from("/tmp/traex-workspace")),
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        };
        remember_external_cli_started_state(&request, "traex");

        let result = crate::im_gateway::external_cli::ExternalCliRunResult {
            run_id: "run-traex-final-order".to_string(),
            session_key: request.session_key.clone(),
            runtime: request.runtime.clone(),
            adapter: request.adapter.clone(),
            status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
            exit_code: Some(0),
            response: "final Traex answer".to_string(),
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
        remember_external_cli_result_state(&request, "traex", &result);

        let state = crate::im_gateway::session_state::load_session_state(
            "traex-final-order-session",
            "traex",
            Some("traex"),
        )
        .expect("finished state");
        let history_path = state.history_path.expect("history path");
        let events = bifrost_agent::persistence::load_conversation_events(std::path::Path::new(
            &history_path,
        ))
        .expect("load final events");
        let assistant_index = events
            .iter()
            .position(|event| {
                event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                    && event
                        .content
                        .get("message")
                        .and_then(|value| value.as_str())
                        == Some("final Traex answer")
            })
            .expect("assistant final event");
        let completed_index = events
            .iter()
            .position(|event| {
                event.event_type == bifrost_agent::persistence::event_types::RUN_STATE_CHANGED
                    && event.content.get("state").and_then(|value| value.as_str())
                        == Some("completed")
            })
            .expect("completed state event");
        assert!(
            assistant_index < completed_index,
            "final assistant message must be visible before completed state"
        );
    }

    #[test]
    fn codex_command_execution_progress_is_recorded_as_exec_command_tool_steps() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "codex-command-execution-history";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(
                session_key,
                serde_json::json!({"source": "admin-api", "adapter": "codex"}),
            )
            .expect("record start");

        let events = crate::im_gateway::external_cli::parse_progress_events(
            r#"{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"/tmp/work\n","exit_code":0,"status":"completed"}}"#,
        );
        assert_eq!(events.len(), 2);
        for event in &events {
            record_external_cli_progress_event_to_timeline(
                &mut recorder,
                session_key,
                "web",
                "codex",
                "codex",
                event,
            );
        }

        let persisted = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load progress events");
        let tool_call_count = persisted
            .iter()
            .filter(|event| {
                event.event_type == bifrost_agent::persistence::event_types::TOOL_CALL
                    && event.content.get("call_id").and_then(|v| v.as_str()) == Some("item_0")
            })
            .count();
        assert_eq!(tool_call_count, 1);
        assert!(persisted.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::TOOL_CALL
                && event.content.get("tool_name").and_then(|v| v.as_str()) == Some("exec_command")
                && event.content.get("call_id").and_then(|v| v.as_str()) == Some("item_0")
                && event
                    .content
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .is_some_and(|value| value.contains("/bin/zsh -lc pwd"))
        }));
        assert!(persisted.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::TOOL_RESULT
                && event.content.get("tool_name").and_then(|v| v.as_str()) == Some("exec_command")
                && event.content.get("call_id").and_then(|v| v.as_str()) == Some("item_0")
                && event.content.get("success").and_then(|v| v.as_bool()) == Some(true)
                && event
                    .content
                    .get("result")
                    .and_then(|v| v.as_str())
                    .is_some_and(|value| value.contains("/tmp/work"))
        }));
    }

    #[test]
    fn external_runner_plan_progress_is_recorded_as_plan_updated_event() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "external-plan-history";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(
                session_key,
                serde_json::json!({"source": "admin-api", "adapter": "codex"}),
            )
            .expect("record start");

        let events = crate::im_gateway::external_cli::parse_progress_events(
            r#"{"type":"item.updated","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":true},{"text":"map parser","completed":false}]}}"#,
        );
        assert_eq!(events.len(), 1);

        let end_index = record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "codex",
            "codex",
            &events[0],
        )
        .expect("plan update changes timeline");

        assert!(end_index > 0);
        let persisted = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load progress events");
        let plan_event = persisted
            .iter()
            .find(|event| event.event_type == bifrost_agent::persistence::event_types::PLAN_UPDATED)
            .expect("plan_updated event");
        let plan = plan_event
            .content
            .get("plan")
            .and_then(serde_json::Value::as_array)
            .expect("plan array");
        assert_eq!(plan.len(), 2);
        assert_eq!(
            plan[0].get("step").and_then(|v| v.as_str()),
            Some("inspect output")
        );
        assert_eq!(
            plan[0].get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert_eq!(
            plan[1].get("status").and_then(|v| v.as_str()),
            Some("pending")
        );
    }

    #[test]
    fn external_runner_plan_progress_payload_includes_steps_for_stream_consumers() {
        let event = crate::im_gateway::external_cli::parse_progress_events(
            r#"{"type":"plan_updated","title":"Runner plan","items":[{"text":"inspect output","status":"completed"},{"text":"map parser","status":"in_progress"}]}"#,
        )
        .pop()
        .expect("plan event");

        let payload = external_cli_progress_event_payload(&event);

        assert_eq!(
            payload.get("eventType").and_then(|value| value.as_str()),
            Some("plan_updated")
        );
        let steps = payload
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .expect("stream steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].get("status").and_then(|value| value.as_str()),
            Some("completed")
        );
        assert_eq!(
            steps[1].get("status").and_then(|value| value.as_str()),
            Some("in_progress")
        );
    }

    #[test]
    fn external_runner_subagent_progress_is_flattened_and_persisted_for_web_replay() {
        let event = crate::im_gateway::external_cli::parse_progress_events(
            r#"{"type":"item.updated","item":{"id":"collab-1","type":"collab_agent_tool_call","tool":"wait","status":"in_progress","prompt":"Review auth","receiver_thread_ids":["agent-7"],"sender_thread_id":"root","agents_states":{"agent-7":{"status":"running","message":"Inspecting guards"}}}}"#,
        )
        .pop()
        .expect("subagent event");
        let payload = external_cli_progress_event_payload(&event);
        assert_eq!(payload["eventType"], "sub_agent_updated");
        assert_eq!(payload["subagent"]["agentId"], "agent-7");
        assert_eq!(payload["subagent"]["task"], "Review auth");
        assert_eq!(payload["subagent"]["status"], "running");

        let temp_dir = tempfile::tempdir().unwrap();
        let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(
            temp_dir.path(),
            "subagent-web-replay",
        );
        let end_index = record_external_cli_progress_event_to_timeline(
            &mut recorder,
            "subagent-web-replay",
            "web",
            "codex",
            "codex",
            &event,
        )
        .expect("subagent event changes timeline");
        assert_eq!(end_index, 1);
        let events =
            bifrost_agent::persistence::load_conversation_events(recorder.file_path()).unwrap();
        assert_eq!(events[0].event_type, "subagent_updated");
        assert_eq!(events[0].content["agentId"], "agent-7");
        assert_eq!(events[0].content["detail"], "Inspecting guards");
    }

    #[test]
    fn external_runner_duplicate_tool_started_is_recorded_once() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "external-duplicate-tool-start";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(
                session_key,
                serde_json::json!({"source": "admin-api", "adapter": "traex"}),
            )
            .expect("record start");

        let event = crate::im_gateway::external_cli::ExternalCliProgressEvent {
            event_type: crate::im_gateway::external_cli::ExternalCliProgressEventType::ToolStarted,
            content: "git diff --stat".to_string(),
            title: Some("exec_command".to_string()),
            raw: serde_json::json!({
                "type": "item.started",
                "item": {
                    "id": "item_duplicate",
                    "type": "command_execution",
                    "command": "git diff --stat"
                },
                "arguments": {"command": "git diff --stat"}
            }),
        };
        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &event,
        );
        record_external_cli_progress_event_to_timeline(
            &mut recorder,
            session_key,
            "web",
            "traex",
            "traex",
            &event,
        );

        let persisted = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load progress events");
        let tool_call_count = persisted
            .iter()
            .filter(|event| {
                event.event_type == bifrost_agent::persistence::event_types::TOOL_CALL
                    && event.content.get("call_id").and_then(|v| v.as_str())
                        == Some("item_duplicate")
            })
            .count();
        assert_eq!(tool_call_count, 1);
    }

    #[test]
    fn external_runner_web_turn_without_history_creates_canonical_timeline() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "active-gpt-web-history";
        let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
            images: Vec::new(),
            files: Vec::new(),
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
            Some("codex"),
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
        assert!(prompt.contains("Current runner: codex"));
        assert!(prompt.contains("Target runner: codex (codex)"));
        assert!(prompt.contains("History path: /tmp/session.jsonl"));
        assert!(prompt.contains("User:\nOriginal question"));
        assert!(prompt.contains("Assistant:\nOriginal answer"));
        assert!(prompt.contains("Review this from another runner"));
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
            "Traex",
            "traex",
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
            "Run with Traex: summarize this context"
        );
        assert_eq!(running.messages[1].role, "assistant");
        assert_eq!(running.messages[1].content, "Runner `Traex` is running...");

        remember_runner_call_result_for_caller(
            &manager,
            &caller_scope,
            "source-session",
            "call-visible",
            "Traex",
            "traex",
            None,
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
            "Runner `Traex` completed this call.\n\ndone"
        );
    }

    #[test]
    fn runner_call_visibility_does_not_create_a_parent_history_from_nothing() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let session_key = "missing-parent-history";

        append_runner_call_started_to_parent_history(
            session_key,
            "codex",
            Some("codex"),
            "run child",
            "running child",
            "traex",
        );

        let data_dir = bifrost_agent::config::agent_home_dir();
        assert!(
            !bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key)
                .exists()
        );
    }

    #[test]
    fn runner_call_visible_messages_are_recorded_in_parent_history() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let manager = std::sync::Arc::new(bifrost_agent::AgentSessionManager::new(60));
        let source_session_key = "source-web-history";
        let caller_scope = (
            "chatgpt_web".to_string(),
            Some("web".to_string()),
            "web".to_string(),
        );
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, source_session_key);
        recorder
            .record_session_start(
                source_session_key,
                serde_json::json!({
                    "source": "admin-api",
                    "runtime": "external_cli",
                    "adapter": "chatgpt_web",
                    "runner_id": "web",
                }),
            )
            .expect("record start");
        recorder
            .record_user_message(source_session_key, "first prompt")
            .expect("record parent user");
        recorder
            .record_assistant_message(source_session_key, "first response")
            .expect("record parent assistant");
        let history_path = recorder.file_path().display().to_string();
        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: source_session_key.to_string(),
                adapter: "chatgpt_web".to_string(),
                runner_id: Some("web".to_string()),
                history_path: Some(history_path.clone()),
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember parent state");

        remember_runner_call_started_for_caller(
            &manager,
            &caller_scope,
            source_session_key,
            "call-visible-history",
            "web",
            "chatgpt_web",
            "generate four images",
            1_000,
        );
        remember_runner_call_result_for_caller(
            &manager,
            &caller_scope,
            source_session_key,
            "call-visible-history",
            "web",
            "chatgpt_web",
            Some("run-visible-history"),
            "generate four images",
            "generated four images",
            1_000,
            2_000,
        );

        let events = bifrost_agent::persistence::load_conversation_events(recorder.file_path())
            .expect("load parent history");
        let finished_state = crate::im_gateway::session_state::load_session_state(
            source_session_key,
            "chatgpt_web",
            Some("web"),
        )
        .expect("finished parent state");
        assert_eq!(
            finished_state.latest_run_id.as_deref(),
            Some("run-visible-history")
        );
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::USER_MESSAGE
                && event
                    .content
                    .get("message")
                    .and_then(|value| value.as_str())
                    == Some("Run with web: generate four images")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                && event
                    .content
                    .get("message")
                    .and_then(|value| value.as_str())
                    == Some("Runner `web` is running...")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                && event
                    .content
                    .get("message")
                    .and_then(|value| value.as_str())
                    == Some("Runner `web` completed this call.\n\ngenerated four images")
        }));
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
            files: Vec::new(),
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

    #[tokio::test]
    async fn local_resume_slash_lists_selects_and_reports_request_errors() {
        struct VarGuard(Option<std::ffi::OsString>);
        impl Drop for VarGuard {
            fn drop(&mut self) {
                if let Some(value) = self.0.take() {
                    std::env::set_var("CODEX_HOME", value);
                } else {
                    std::env::remove_var("CODEX_HOME");
                }
            }
        }

        let _env_lock = crate::im_gateway::external_cli::local_session_test_env_lock()
            .lock()
            .await;
        let home = tempfile::tempdir().expect("codex home");
        let data_dir = tempfile::tempdir().expect("data dir");
        let _data_guard =
            crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(data_dir.path());
        let _home_guard = VarGuard(std::env::var_os("CODEX_HOME"));
        std::env::set_var("CODEX_HOME", home.path());
        let id = "dddddddd-0000-0000-0000-000000000005";
        let session_path = home.path().join("sessions/resume.jsonl");
        std::fs::create_dir_all(session_path.parent().expect("parent")).expect("create sessions");
        std::fs::write(
            &session_path,
            format!(
                "{{\"timestamp\":\"2026-08-07T04:05:06Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\"}}}}\n"
            ),
        )
        .expect("write session");

        let config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
        let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
            &config,
            None,
            Some(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID),
        );
        let request = |message: &str, adapter: &str, session_key: Option<&str>| {
            crate::im_gateway::external_cli::ExternalCliRunRequest {
                message: message.to_string(),
                images: Vec::new(),
                files: Vec::new(),
                operation: "chat".to_string(),
                params: serde_json::Value::Null,
                provider_id: None,
                runner_id: Some(effective.runner_id.clone()),
                session_key: session_key.map(str::to_string),
                runtime: "external_cli".to_string(),
                adapter: adapter.to_string(),
                work_dir: None,
                instructions: None,
                adapter_config: Default::default(),
                allow_work_dirs: Vec::new(),
                inject_bifrost_tools: false,
                skill_paths: Vec::new(),
            }
        };
        async fn body_text(response: Response<BoxBody>) -> String {
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("collect response")
                .to_bytes();
            String::from_utf8(bytes.to_vec()).expect("utf8 response")
        }

        let listed = maybe_external_cli_slash_response(
            &request("/resume", "codex", Some("web:resume-handler")),
            &effective,
            false,
        )
        .await
        .expect("list response");
        assert!(body_text(listed).await.contains(id));

        let picked = maybe_external_cli_resume_slash_response(
            &request("/resume dddddddd", "codex", Some("web:resume-handler")),
            &effective,
            false,
        )
        .await
        .expect("pick response");
        assert!(body_text(picked).await.contains("已选择本地 session"));
        let state = crate::im_gateway::session_state::load_session_state(
            "web:resume-handler",
            "codex",
            Some(&effective.runner_id),
        )
        .expect("selected state");
        assert_eq!(state.external_thread_id.as_deref(), Some(id));

        for invalid in [
            request("/resume too many", "codex", Some("web:resume-handler")),
            request("/resume dddddddd", "mock", Some("web:resume-handler")),
            request("/resume dddddddd", "codex", None),
        ] {
            let response = maybe_external_cli_resume_slash_response(&invalid, &effective, false)
                .await
                .expect("error response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        assert!(maybe_external_cli_resume_slash_response(
            &request("hello", "codex", Some("web:resume-handler")),
            &effective,
            false,
        )
        .await
        .is_none());
    }
}

#[cfg(test)]
mod image_message_tests {
    use super::*;
    use crate::im_gateway::external_cli::ExternalCliImageInput;

    fn image(mime_type: &str, data: &str) -> ExternalCliImageInput {
        ExternalCliImageInput {
            mime_type: mime_type.to_string(),
            data: data.to_string(),
            name: None,
        }
    }

    #[test]
    fn message_image_content_parts_returns_none_when_no_images() {
        let parts = message_image_content_parts("hello", &[]);
        assert!(parts.is_none());

        let images = vec![image("image/png", "   ")];
        assert!(message_image_content_parts("hello", &images).is_none());
    }

    #[test]
    fn message_image_content_parts_wraps_text_and_images_up_to_limit() {
        let images = vec![
            image("image/png", "AAA"),
            image("image/jpeg", "data:image/jpeg;base64,BBBB"),
        ];
        let parts = message_image_content_parts(" hi ", &images).unwrap();
        let arr = parts.as_array().expect("array of parts");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], " hi ");
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(
            arr[1]["image_url"]["url"].as_str(),
            Some("data:image/png;base64,AAA")
        );
        assert_eq!(
            arr[2]["image_url"]["url"].as_str(),
            Some("data:image/jpeg;base64,BBBB")
        );
    }

    #[test]
    fn image_message_preview_uses_message_when_non_empty() {
        let images = vec![image("image/png", "AAA")];
        assert_eq!(
            image_message_preview("  describe image  ", &images, &[]),
            "describe image"
        );
    }

    #[test]
    fn image_message_preview_counts_non_empty_images_when_message_empty() {
        let images = vec![
            image("image/png", "AAA"),
            image("image/png", "BBB"),
            image("image/png", "   "), // ignored
        ];
        assert_eq!(
            image_message_preview("   ", &images[..1], &[]),
            "Attached 1 image"
        );
        assert_eq!(image_message_preview("", &images, &[]), "Attached 2 images");
        let empty_images = vec![image("image/png", "   ")];
        let files = vec![
            crate::im_gateway::external_cli::ExternalCliFileInput {
                mime_type: "text/plain".to_string(),
                data: "Zm9v".to_string(),
                name: Some("note.txt".to_string()),
            },
            crate::im_gateway::external_cli::ExternalCliFileInput {
                mime_type: "application/pdf".to_string(),
                data: "YmFy".to_string(),
                name: Some("report.pdf".to_string()),
            },
        ];
        assert_eq!(
            image_message_preview("", &empty_images, &files[..1]),
            "Attached 1 file"
        );
        assert_eq!(
            image_message_preview("", &empty_images, &files),
            "Attached 2 files"
        );
        assert_eq!(image_message_preview("", &empty_images, &[]), "");
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;

    use bytes::Bytes;
    use http_body_util::BodyExt;
    use tokio::sync::mpsc;

    use crate::im_gateway::external_cli::ExternalCliRunRequest;
    use crate::im_gateway::types::{ImProviderAgentConfig, ImProviderConfig, ImProviderType};
    use crate::test_support::TestAdminState;

    fn sample_run_request() -> ExternalCliRunRequest {
        ExternalCliRunRequest {
            message: "hello".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            operation: "chat".to_string(),
            params: serde_json::Value::Null,
            provider_id: None,
            runner_id: Some("web".to_string()),
            session_key: Some("session-key".to_string()),
            runtime: "local".to_string(),
            adapter: "chatgpt_web".to_string(),
            work_dir: None,
            instructions: None,
            adapter_config: Default::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
        }
    }

    #[test]
    fn runner_call_stream_request_deserializes_defaults() {
        let json = r#"{
            "callerSessionKey": "source",
            "targetRunnerId": "codex",
            "message": "hello"
        }"#;
        let req: RunnerCallStreamRequest = serde_json::from_str(json).expect("parse request");

        assert_eq!(req.caller_session_key, "source");
        assert_eq!(req.target_runner_id, "codex");
        assert_eq!(req.message, "hello");
        assert!(req.caller_runner_id.is_none());
        assert!(req.caller_runner_adapter.is_none());
        assert!(req.images.is_empty());
        assert!(req.work_dir.is_none());
        assert!(req.history_path.is_none());
        assert!(req.caller_messages.is_empty());
    }

    #[test]
    fn runner_call_stream_request_deserializes_full_fields() {
        let json = r#"{
            "callerSessionKey": " source ",
            "callerRunnerId": " web ",
            "callerRunnerAdapter": " chatgpt_web ",
            "targetRunnerId": "codex",
            "message": " hi ",
            "images": [
                {"mimeType": "image/png", "data": "aGVsbG8=", "name": "pasted.png"}
            ],
            "workDir": "/tmp/work",
            "historyPath": " /tmp/history.jsonl ",
            "callerMessages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "second"}
            ]
        }"#;
        let req: RunnerCallStreamRequest = serde_json::from_str(json).expect("parse request");

        assert_eq!(req.caller_session_key, " source ");
        assert_eq!(req.caller_runner_id.as_deref(), Some(" web "));
        assert_eq!(req.caller_runner_adapter.as_deref(), Some(" chatgpt_web "));
        assert_eq!(req.target_runner_id, "codex");
        assert_eq!(req.message, " hi ");
        assert_eq!(req.images.len(), 1);
        assert_eq!(req.images[0].mime_type, "image/png");
        assert_eq!(req.images[0].name.as_deref(), Some("pasted.png"));
        assert_eq!(
            req.work_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .as_deref(),
            Some("/tmp/work")
        );
        assert_eq!(req.history_path.as_deref(), Some(" /tmp/history.jsonl "));
        assert_eq!(req.caller_messages.len(), 2);
        assert_eq!(req.caller_messages[0].role, "user");
        assert_eq!(req.caller_messages[1].content, "second");
    }

    #[test]
    fn session_attachment_base_dir_uses_history_file_stem() {
        let base = session_attachment_base_dir_from_history_path(
            "/tmp/bifrost/agent/sessions/by-key/session-abc123.jsonl",
        )
        .expect("attachment base dir");

        assert_eq!(
            base,
            std::path::PathBuf::from("/tmp/bifrost/agent/sessions/by-key")
                .join("attachments")
                .join("session-abc123")
                .display()
                .to_string()
        );
    }

    #[test]
    fn prepare_attachment_params_overwrites_untrusted_client_base_dir() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let mut request = sample_run_request();
        request.params = serde_json::json!({
            "historyPath": "/tmp/attacker-history.jsonl",
            "attachmentBaseDir": "/tmp/attacker-attachments",
            "attachment_base_dir": "/tmp/attacker-snake-attachments"
        });

        prepare_external_cli_session_attachment_params(&mut request, "web");

        let params = request.params.as_object().expect("params object");
        let attachment_base_dir = params
            .get("attachmentBaseDir")
            .and_then(serde_json::Value::as_str)
            .expect("attachment base dir");
        assert!(!params.contains_key("attachment_base_dir"));
        assert!(!attachment_base_dir.starts_with("/tmp/attacker"));
        let normalized_attachment_base_dir = attachment_base_dir.replace('\\', "/");
        assert!(normalized_attachment_base_dir.contains("/agent/sessions/"));
        assert!(normalized_attachment_base_dir.contains("/attachments/"));
    }

    #[test]
    fn normalize_transcript_role_maps_known_roles_and_defaults() {
        assert_eq!(normalize_transcript_role("assistant"), "Assistant");
        assert_eq!(normalize_transcript_role(" Assistant "), "Assistant");
        assert_eq!(normalize_transcript_role("SYSTEM"), "System");
        assert_eq!(normalize_transcript_role("developer"), "Developer");
        assert_eq!(normalize_transcript_role("user"), "User");
        assert_eq!(normalize_transcript_role("other"), "User");
    }

    #[test]
    fn build_runner_call_prompt_includes_no_prior_visible_stub_when_empty() {
        let prompt = build_runner_call_prompt(
            "session-1",
            Some("codex"),
            "codex",
            "codex",
            None,
            &[],
            "Review this in another runner",
        );

        assert!(prompt.contains("Source session: session-1"));
        assert!(prompt.contains("Current runner: codex"));
        assert!(prompt.contains("Target runner: codex (codex)"));
        assert!(prompt.contains("(No prior visible messages were provided.)"));
        assert!(prompt.contains("Review this in another runner"));
    }

    #[test]
    fn caller_runner_scope_uses_external_default_and_custom_adapter() {
        let (adapter, runner_opt, runner) = caller_runner_scope(None, None);
        assert_eq!(adapter, "external");
        assert_eq!(runner_opt.as_deref(), Some("external"));
        assert_eq!(runner, "external");

        let (adapter, runner_opt, runner) =
            caller_runner_scope(Some(" codex "), Some(" chatgpt_web "));
        assert_eq!(adapter, "chatgpt_web");
        assert_eq!(runner_opt.as_deref(), Some("codex"));
        assert_eq!(runner, "codex");

        let (adapter, runner_opt, runner) = caller_runner_scope(Some("codex"), None);
        assert_eq!(adapter, "codex");
        assert_eq!(runner_opt.as_deref(), Some("codex"));
        assert_eq!(runner, "codex");
    }

    #[test]
    fn runner_call_visible_running_and_completed_messages_trim_inputs() {
        let visible = runner_call_visible_user("codex", "  summarize  ");
        assert_eq!(visible, "Run with codex: summarize");

        let running = runner_call_running_message("codex");
        assert_eq!(running, "Runner `codex` is running...");

        let completed = runner_call_completed_message("codex", "  done ");
        assert_eq!(completed, "Runner `codex` completed this call.\n\ndone");
    }

    #[test]
    fn update_session_runner_call_messages_updates_or_appends() {
        let visible = "Run with web: ask";
        let running = "Runner `web` is running...";
        let completed = "Runner `web` completed this call.\n\nanswer";

        let mut messages = vec![
            crate::im_gateway::session_state::ImAgentSessionMessage {
                role: "user".to_string(),
                content: visible.to_string(),
                timestamp: Some(1),
                content_parts: None,
            },
            crate::im_gateway::session_state::ImAgentSessionMessage {
                role: "assistant".to_string(),
                content: running.to_string(),
                timestamp: Some(2),
                content_parts: None,
            },
        ];
        update_session_runner_call_messages(&mut messages, visible, running, completed, 10, 20);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, completed);
        assert_eq!(messages[1].timestamp, Some(20));

        let mut messages = Vec::new();
        update_session_runner_call_messages(&mut messages, visible, running, completed, 10, 20);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].timestamp, Some(10));
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].timestamp, Some(20));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn runner_call_stream_response_validates_required_fields() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let base = RunnerCallStreamRequest {
            caller_session_key: "caller".to_string(),
            caller_runner_id: None,
            caller_runner_adapter: None,
            target_runner_id: "codex".to_string(),
            message: "hello".to_string(),
            images: Vec::new(),
            work_dir: None,
            history_path: None,
            caller_messages: Vec::new(),
        };

        let err = runner_call_stream_response(
            &service,
            RunnerCallStreamRequest {
                caller_session_key: "  ".to_string(),
                ..base.clone()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err, "callerSessionKey is required");

        let err = runner_call_stream_response(
            &service,
            RunnerCallStreamRequest {
                target_runner_id: "".to_string(),
                ..base.clone()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err, "targetRunnerId is required");

        let err = runner_call_stream_response(
            &service,
            RunnerCallStreamRequest {
                message: "   ".to_string(),
                ..base.clone()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err, "message or images are required");

        let image_only_result = runner_call_stream_response(
            &service,
            RunnerCallStreamRequest {
                message: "   ".to_string(),
                images: vec![crate::im_gateway::external_cli::ExternalCliImageInput {
                    mime_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                    name: Some("only.png".to_string()),
                }],
                ..base
            },
        )
        .await;
        if let Err(error) = image_only_result {
            assert_ne!(error, "message or images are required");
        }
    }

    #[test]
    fn is_clear_session_command_matches_reset_and_clear() {
        assert!(is_clear_session_command("/clear"));
        assert!(is_clear_session_command("  /reset  "));
        assert!(!is_clear_session_command("/CLEAR"));
        assert!(!is_clear_session_command("/clear now"));
    }

    #[test]
    fn first_message_title_preview_trims_and_truncates() {
        assert!(first_message_title_preview("   ").is_none());

        let long = "中".repeat(200);
        let preview = first_message_title_preview(&long).expect("preview");
        assert_eq!(preview.chars().count(), 81);
        assert!(preview.ends_with('…'));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queue_stream_remove_nonexistent_and_parse_error() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let service = ImGatewayService::new(temp_dir.path());

        let not_found_response =
            queue_external_cli_stream_response(&service, "session-1", "/rq 5").await;
        let not_found_body = not_found_response
            .into_body()
            .collect()
            .await
            .expect("collect not_found")
            .to_bytes();
        let not_found: serde_json::Value =
            serde_json::from_slice(&not_found_body).expect("json not_found");
        assert_eq!(not_found["eventType"], "run_finished");
        assert_eq!(not_found["sessionKey"], "session-1");
        assert_eq!(not_found["response"].as_str(), Some("❌ 未找到排队消息 #5"));

        let parse_response =
            queue_external_cli_stream_response(&service, "session-1", "/rq not-a-number").await;
        let parse_body = parse_response
            .into_body()
            .collect()
            .await
            .expect("collect parse_error")
            .to_bytes();
        let parse_error: serde_json::Value =
            serde_json::from_slice(&parse_body).expect("json parse_error");
        assert_eq!(parse_error["eventType"], "run_finished");
        assert_eq!(parse_error["sessionKey"], "session-1");
        assert_eq!(
            parse_error["response"].as_str(),
            Some("用法: /rq <序号>（如 /rq 1）")
        );
    }

    #[test]
    fn query_param_extracts_first_match() {
        let query = Some("runnerId=web&foo=bar&runnerId=extra");
        assert_eq!(query_param(query, "runnerId"), Some("web".to_string()));
        assert_eq!(query_param(query, "missing"), None);
        assert_eq!(query_param(None, "runnerId"), None);
    }

    #[test]
    fn request_history_path_prefers_camel_case_and_filters_empty() {
        let mut req = sample_run_request();
        req.params = serde_json::json!({ "historyPath": " /tmp/history.log " });
        assert_eq!(
            request_history_path(&req),
            Some("/tmp/history.log".to_string())
        );

        req.params = serde_json::json!({ "historyPath": "   " });
        assert!(request_history_path(&req).is_none());

        req.params = serde_json::json!({ "history_path": " /tmp/snake.log " });
        assert_eq!(
            request_history_path(&req),
            Some("/tmp/snake.log".to_string())
        );

        req.params = serde_json::json!({});
        assert!(request_history_path(&req).is_none());
    }

    #[test]
    fn set_request_history_path_initializes_params_map() {
        let mut req = sample_run_request();
        assert!(req.params.is_null());
        set_request_history_path(&mut req, "/tmp/history.log");

        assert_eq!(
            request_history_path(&req),
            Some("/tmp/history.log".to_string())
        );

        // Second call preserves existing value
        set_request_history_path(&mut req, "/other/path");
        assert_eq!(
            request_history_path(&req),
            Some("/tmp/history.log".to_string())
        );
    }

    #[test]
    fn persisted_history_path_for_request_returns_none_without_state() {
        let mut req = sample_run_request();
        req.session_key = Some("no-state-session".to_string());
        assert!(persisted_history_path_for_request(&req, "web").is_none());
    }

    #[test]
    fn apply_provider_work_dir_to_external_cli_request_uses_provider_override() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let provider = ImProviderConfig {
            id: "provider-1".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Main".to_string(),
            enabled: true,
            base_url: None,
            app_id: None,
            secret_ref: None,
            owner_open_id: None,
            event_connection_enabled: false,
            event_types: Vec::new(),
            agent_config: Some(ImProviderAgentConfig {
                runner: None,
                work_dir: Some("/custom/workdir".to_string()),
                base_instructions: None,
                developer_instructions: None,
                user_instructions: None,
            }),
            created_at: 0,
            updated_at: 0,
        };
        service
            .provider_store
            .add(provider.clone())
            .expect("add provider");

        let agent_config = service.agent_config_store.load();
        let stored = service
            .provider_store
            .get(&provider.id)
            .expect("stored provider");
        let expected_work_dir = effective_agent_work_dir_for_provider(&agent_config, &stored)
            .expect("resolved work dir");

        let mut req = sample_run_request();
        req.provider_id = Some(provider.id.clone());
        req.work_dir = None;
        req.allow_work_dirs.clear();

        apply_provider_work_dir_to_external_cli_request(&service, &mut req);

        assert_eq!(req.work_dir.as_ref(), Some(&expected_work_dir));
        assert_eq!(
            req.allow_work_dirs,
            vec![expected_work_dir.display().to_string()]
        );
    }

    #[test]
    fn chatgpt_web_settings_uses_default_runner_and_validates_adapter() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        // Unknown runner id -> not found
        let err = chatgpt_web_settings(&service, Some("missing")).unwrap_err();
        assert!(err.contains("runner 'missing' not found"));

        // Non-chatgpt_web adapter -> error
        let mut runners = std::collections::BTreeMap::new();
        runners.insert(
            "web".to_string(),
            crate::im_gateway::external_cli::ExternalCliAgentSettings {
                enabled: true,
                adapter: "codex".to_string(),
                ..Default::default()
            },
        );
        let config = crate::im_gateway::external_cli::ExternalCliGatewayConfig {
            version: 1,
            default_runner_id: "web".to_string(),
            runners,
            channels: std::collections::BTreeMap::new(),
        };
        service
            .external_cli_config_store
            .save(config)
            .expect("save config");
        let err = chatgpt_web_settings(&service, Some("web")).unwrap_err();
        assert!(err.contains("uses adapter 'codex', not chatgpt_web"));

        // Valid chatgpt_web runner with default id
        let mut runners = std::collections::BTreeMap::new();
        runners.insert(
            "web".to_string(),
            crate::im_gateway::external_cli::ExternalCliAgentSettings {
                enabled: true,
                adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
                ..Default::default()
            },
        );
        let config = crate::im_gateway::external_cli::ExternalCliGatewayConfig {
            version: 1,
            default_runner_id: "web".to_string(),
            runners,
            channels: std::collections::BTreeMap::new(),
        };
        service
            .external_cli_config_store
            .save(config)
            .expect("save config");

        let settings = chatgpt_web_settings(&service, None).expect("settings");
        assert!(settings.enabled);
        assert_eq!(settings.adapter, crate::im_gateway::chatgpt_web::ADAPTER_ID);
    }

    #[test]
    fn external_progress_tool_name_and_arguments_and_call_id() {
        use crate::im_gateway::external_cli::ExternalCliProgressEventType as EventType;

        let mut event = crate::im_gateway::external_cli::ExternalCliProgressEvent {
            event_type: EventType::ToolStarted,
            content: String::new(),
            title: Some(" exec_command ".to_string()),
            raw: serde_json::json!({}),
        };
        assert_eq!(
            external_progress_tool_name(&event, "default"),
            "exec_command"
        );

        event.title = None;
        event.raw = serde_json::json!({ "tool_name": " ls " });
        assert_eq!(external_progress_tool_name(&event, "default"), "ls");

        event.raw = serde_json::Value::Null;
        assert_eq!(external_progress_tool_name(&event, "fallback"), "fallback");

        // Arguments prefer structured fields
        event.content = "ignored".to_string();
        event.raw = serde_json::json!({ "arguments": {"cmd": "pwd"} });
        assert_eq!(
            external_progress_tool_arguments(&event),
            serde_json::json!({"cmd": "pwd"}).to_string()
        );

        // Fallback wraps content
        event.raw = serde_json::Value::Null;
        event.content = "log".to_string();
        let args = external_progress_tool_arguments(&event);
        assert!(args.contains("log"));

        // Call id falls back to adapter + tool name + hash
        event.title = Some("exec_command".to_string());
        event.raw = serde_json::json!({ "other": 1 });
        let call_id = external_progress_call_id("codex", &event);
        assert!(call_id.starts_with("codex-exec_command-"));
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::type_complexity)]
    async fn send_ndjson_event_writes_single_json_line() {
        let (tx, mut rx): (
            mpsc::Sender<Result<hyper::body::Frame<Bytes>, hyper::Error>>,
            mpsc::Receiver<Result<hyper::body::Frame<Bytes>, hyper::Error>>,
        ) = mpsc::channel(1);

        let event = serde_json::json!({"foo": "bar"});
        send_ndjson_event(&tx, &event)
            .await
            .expect("send should succeed");

        let frame = rx.recv().await.expect("frame").expect("ok frame");
        let data = frame.into_data().expect("data frame");
        let text = String::from_utf8(data.to_vec()).expect("utf8");
        assert_eq!(text.trim_end(), "{\"foo\":\"bar\"}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_persisted_state_consumes_imported_contexts_without_session_state() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());

        crate::im_gateway::session_state::push_imported_context(
            "external-import-session-2",
            "codex",
            Some("codex"),
            crate::im_gateway::session_state::ImImportedRunnerContext {
                call_id: "call-import".to_string(),
                source_session_key: "external-import-session-2".to_string(),
                target_runner_id: "web".to_string(),
                target_adapter: "chatgpt_web".to_string(),
                user_message: "ask web".to_string(),
                response: "web answer".to_string(),
                created_at: 1,
            },
        )
        .expect("push imported context");

        let mut req = sample_run_request();
        req.session_key = Some("external-import-session-2".to_string());
        req.adapter = "codex".to_string();
        req.instructions = Some("base instructions".to_string());

        apply_persisted_external_cli_state(&mut req, "codex");

        let instructions = req.instructions.as_deref().expect("instructions");
        assert!(instructions.contains("base instructions"));
        assert!(instructions.contains("Imported Runner Results"));
        assert!(instructions.contains("web answer"));

        let contexts = crate::im_gateway::session_state::take_imported_contexts(
            "external-import-session-2",
            "codex",
            Some("codex"),
        )
        .expect("take contexts");
        assert!(contexts.is_empty());
    }

    #[test]
    fn apply_persisted_state_applies_external_runner_session_model_and_effort_override() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());

        crate::im_gateway::session_state::upsert_session_state(
            "traex-model-session",
            crate::im_gateway::external_cli::TRAEX_ADAPTER,
            Some("Traex"),
            |state| {
                state.model_override = Some("gpt-5.5".to_string());
                state.model_override_source = Some("session slash command".to_string());
                state.reasoning_effort_override = Some("high".to_string());
                state.reasoning_effort_override_source = Some("session slash command".to_string());
            },
        )
        .expect("persist model override");

        let mut req = sample_run_request();
        req.session_key = Some("traex-model-session".to_string());
        req.adapter = crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string();
        req.runner_id = Some("Traex".to_string());

        apply_persisted_external_cli_state(&mut req, "Traex");

        assert_eq!(req.adapter_config.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(req.adapter_config.reasoning_effort.as_deref(), Some("high"));

        let mut runner_default = req.clone();
        runner_default.adapter_config.model = Some("gpt-runner-default".to_string());

        apply_persisted_external_cli_state(&mut runner_default, "Traex");

        assert_eq!(
            runner_default.adapter_config.model.as_deref(),
            Some("gpt-5.5")
        );

        crate::im_gateway::session_state::upsert_session_state(
            "codex-model-session",
            "codex",
            Some("Codex"),
            |state| {
                state.model_override = Some("gpt-5.5".to_string());
                state.model_override_source = Some("session slash command".to_string());
                state.reasoning_effort_override = Some("minimal".to_string());
                state.reasoning_effort_override_source = Some("session slash command".to_string());
            },
        )
        .expect("persist codex model override");

        let mut codex = sample_run_request();
        codex.session_key = Some("codex-model-session".to_string());
        codex.adapter = "codex".to_string();
        codex.runner_id = Some("Codex".to_string());

        apply_persisted_external_cli_state(&mut codex, "Codex");

        assert_eq!(codex.adapter_config.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            codex.adapter_config.reasoning_effort.as_deref(),
            Some("minimal")
        );

        crate::im_gateway::session_state::upsert_session_state(
            "claude-code-model-session",
            crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER,
            Some(crate::im_gateway::external_cli::DEFAULT_CLAUDE_CODE_RUNNER_ID),
            |state| {
                state.model_override = Some("sonnet".to_string());
                state.model_override_source = Some("session slash command".to_string());
                state.reasoning_effort_override = Some("xhigh".to_string());
                state.reasoning_effort_override_source = Some("session slash command".to_string());
            },
        )
        .expect("persist claude code model override");

        let mut claude_code = sample_run_request();
        claude_code.session_key = Some("claude-code-model-session".to_string());
        claude_code.adapter = crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER.to_string();
        claude_code.runner_id =
            Some(crate::im_gateway::external_cli::DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string());

        apply_persisted_external_cli_state(
            &mut claude_code,
            crate::im_gateway::external_cli::DEFAULT_CLAUDE_CODE_RUNNER_ID,
        );

        assert_eq!(claude_code.adapter_config.model.as_deref(), Some("sonnet"));
        assert_eq!(
            claude_code.adapter_config.reasoning_effort.as_deref(),
            Some("xhigh")
        );
    }

    #[test]
    fn timeline_has_tool_call_detects_existing_call_id() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(dir.path(), "session-tool");
        recorder
            .record_session_start("session-tool", serde_json::json!({"source": "admin-api"}))
            .expect("start session");
        recorder
            .record_tool_call_with_id(
                "session-tool",
                "exec_command",
                "{\"cmd\":\"pwd\"}",
                Some("call-123"),
            )
            .expect("tool call");
        recorder.close();

        assert!(timeline_has_tool_call(
            &recorder,
            "session-tool",
            "call-123"
        ));
        assert!(!timeline_has_tool_call(
            &recorder,
            "session-tool",
            "missing"
        ));
    }

    #[test]
    fn message_image_content_parts_respects_max_image_limit() {
        let mut images = Vec::new();
        for i in 0..(MAX_AGENT_ATTACHMENTS_PER_MESSAGE + 2) {
            images.push(crate::im_gateway::external_cli::ExternalCliImageInput {
                mime_type: "image/png".to_string(),
                data: format!("A{i}"),
                name: None,
            });
        }
        let parts = message_image_content_parts("msg", &images).expect("parts");
        let arr = parts.as_array().expect("array");
        assert_eq!(arr.len(), 1 + MAX_AGENT_ATTACHMENTS_PER_MESSAGE);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queue_stream_push_without_prefix_uses_trimmed_message() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let service = ImGatewayService::new(temp_dir.path());

        let response = queue_external_cli_stream_response(
            &service,
            "session-queue",
            "  plain queued message  ",
        )
        .await;
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect queued")
            .to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json payload");

        assert_eq!(payload["eventType"], "run_finished");
        assert_eq!(payload["sessionKey"], "session-queue");
        assert_eq!(payload["queued"], true);
        assert_eq!(payload["queueLength"], 1);
        let items = payload["queueItems"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["message"], "plain queued message");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guide_stream_falls_back_to_queue_without_losing_message() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let service = ImGatewayService::new(temp_dir.path());

        let response = guide_external_cli_stream_response(
            &service,
            "session-guide-fallback",
            "inspect the failing test",
        )
        .await;
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect guide fallback")
            .to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json payload");

        assert_eq!(payload["eventType"], "run_finished");
        assert_eq!(payload["delivery"], "queued");
        assert_eq!(payload["guide"], true);
        assert_eq!(payload["queued"], true);
        assert_eq!(payload["queueLength"], 1);
        assert_eq!(
            payload["queueItems"][0]["message"],
            "inspect the failing test"
        );
        assert!(payload["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no active external runner")));
    }

    #[test]
    fn external_progress_tool_name_reads_item_name_field() {
        use crate::im_gateway::external_cli::ExternalCliProgressEventType as EventType;

        let event = crate::im_gateway::external_cli::ExternalCliProgressEvent {
            event_type: EventType::ToolStarted,
            content: String::new(),
            title: None,
            raw: serde_json::json!({
                "item": { "name": "from_item" }
            }),
        };
        assert_eq!(external_progress_tool_name(&event, "default"), "from_item");
    }

    #[test]
    fn external_progress_tool_arguments_uses_item_args_field() {
        use crate::im_gateway::external_cli::ExternalCliProgressEventType as EventType;

        let event = crate::im_gateway::external_cli::ExternalCliProgressEvent {
            event_type: EventType::ToolStarted,
            content: String::new(),
            title: None,
            raw: serde_json::json!({
                "item": { "args": {"cmd": "ls"} }
            }),
        };
        assert_eq!(
            external_progress_tool_arguments(&event),
            serde_json::json!({"cmd": "ls"}).to_string()
        );
    }

    #[test]
    fn first_message_title_preview_returns_full_when_short() {
        let msg = "short title";
        let preview = first_message_title_preview(msg).expect("preview");
        assert_eq!(preview, msg);
    }

    #[test]
    fn runner_call_visible_user_handles_empty_trailing_whitespace() {
        let visible = runner_call_visible_user("web", " ask  ");
        assert_eq!(visible, "Run with web: ask");
    }
}

#[test]
fn timeline_has_tool_call_detects_existing_call_id() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut recorder =
        bifrost_agent::persistence::ConversationRecorder::new(dir.path(), "session-tool");
    recorder
        .record_session_start("session-tool", serde_json::json!({"source": "admin-api"}))
        .expect("start session");
    recorder
        .record_tool_call_with_id(
            "session-tool",
            "exec_command",
            "{\"cmd\":\"pwd\"}",
            Some("call-123"),
        )
        .expect("tool call");
    recorder.close();

    assert!(timeline_has_tool_call(
        &recorder,
        "session-tool",
        "call-123"
    ));
    assert!(!timeline_has_tool_call(
        &recorder,
        "session-tool",
        "missing"
    ));
}

#[test]
fn message_image_content_parts_respects_max_image_limit() {
    let mut images = Vec::new();
    for i in 0..(MAX_AGENT_ATTACHMENTS_PER_MESSAGE + 2) {
        images.push(crate::im_gateway::external_cli::ExternalCliImageInput {
            mime_type: "image/png".to_string(),
            data: format!("A{i}"),
            name: None,
        });
    }
    let parts = message_image_content_parts("msg", &images).expect("parts");
    let arr = parts.as_array().expect("array");
    assert_eq!(arr.len(), 1 + MAX_AGENT_ATTACHMENTS_PER_MESSAGE);
}

#[tokio::test(flavor = "current_thread")]
async fn queue_stream_push_without_prefix_uses_trimmed_message() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let service = ImGatewayService::new(temp_dir.path());

    let response =
        queue_external_cli_stream_response(&service, "session-queue", "  plain queued message  ")
            .await;
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect queued")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json payload");

    assert_eq!(payload["eventType"], "run_finished");
    assert_eq!(payload["sessionKey"], "session-queue");
    assert_eq!(payload["queued"], true);
    assert_eq!(payload["queueLength"], 1);
    let items = payload["queueItems"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["message"], "plain queued message");
}

#[test]
fn external_progress_tool_name_reads_item_name_field() {
    use crate::im_gateway::external_cli::ExternalCliProgressEventType as EventType;

    let event = crate::im_gateway::external_cli::ExternalCliProgressEvent {
        event_type: EventType::ToolStarted,
        content: String::new(),
        title: None,
        raw: serde_json::json!({
            "item": { "name": "from_item" }
        }),
    };
    assert_eq!(external_progress_tool_name(&event, "default"), "from_item");
}

#[test]
fn external_progress_tool_arguments_uses_item_args_field() {
    use crate::im_gateway::external_cli::ExternalCliProgressEventType as EventType;

    let event = crate::im_gateway::external_cli::ExternalCliProgressEvent {
        event_type: EventType::ToolStarted,
        content: String::new(),
        title: None,
        raw: serde_json::json!({
            "item": { "args": {"cmd": "ls"} }
        }),
    };
    assert_eq!(
        external_progress_tool_arguments(&event),
        serde_json::json!({"cmd": "ls"}).to_string()
    );
}

#[test]
fn first_message_title_preview_returns_full_when_short() {
    let msg = "short title";
    let preview = first_message_title_preview(msg).expect("preview");
    assert_eq!(preview, msg);
}

#[test]
fn runner_call_visible_user_handles_empty_trailing_whitespace() {
    let visible = runner_call_visible_user("web", " ask  ");
    assert_eq!(visible, "Run with web: ask");
}
