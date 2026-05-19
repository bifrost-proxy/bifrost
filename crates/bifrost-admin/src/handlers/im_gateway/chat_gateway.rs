use super::*;
use std::path::PathBuf;

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
                if !effective.settings.enabled {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("runner '{}' is not enabled", effective.runner_id),
                    );
                }
                apply_provider_work_dir_to_external_cli_request(_service, &mut request);
                let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
                    crate::im_gateway::external_cli::default_runs_root(),
                );
                let (tx, rx) = tokio::sync::mpsc::channel::<
                    Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>,
                >(16);
                let session_key_for_stop = request.session_key.clone();
                let runs_root_for_stop = crate::im_gateway::external_cli::default_runs_root();
                tokio::spawn(async move {
                    let started =
                        serde_json::json!({"eventType":"run_started","content":"started"});
                    if tx
                        .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(format!(
                            "{}\n",
                            started
                        )))))
                        .await
                        .is_err()
                    {
                        // Client already disconnected before run even started
                        return;
                    }
                    // Race between the run completing and the client disconnecting.
                    // tx.closed() resolves when the receiver (HTTP response body) is dropped,
                    // which happens when the client closes the connection.
                    let run_result = tokio::select! {
                        result = runtime.run(request) => Some(result),
                        _ = tx.closed() => {
                            // Client disconnected while run is in progress — stop it
                            tracing::info!("SSE client disconnected, stopping active session");
                            if let Some(ref sk) = session_key_for_stop {
                                let _ =
                                    crate::im_gateway::external_cli::request_session_stop(
                                        &runs_root_for_stop,
                                        sk,
                                    )
                                    .await;
                            }
                            None
                        }
                    };
                    let Some(run_result) = run_result else {
                        return;
                    };
                    match run_result {
                        Ok(result) => {
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
                                if tx
                                    .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                                        format!("{line}\n"),
                                    ))))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            let finished = serde_json::json!({"eventType":"run_finished","runId":result.run_id,"status":result.status,"response":result.response});
                            let _ = tx
                                .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(format!(
                                    "{}\n",
                                    finished
                                )))))
                                .await;
                        }
                        Err(error) => {
                            let failed =
                                serde_json::json!({"eventType":"run_failed","error":error});
                            let _ = tx
                                .send(Ok(hyper::body::Frame::data(bytes::Bytes::from(format!(
                                    "{}\n",
                                    failed
                                )))))
                                .await;
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
                if !effective.settings.enabled {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("runner '{}' is not enabled", effective.runner_id),
                    );
                }
                apply_provider_work_dir_to_external_cli_request(_service, &mut request);
                let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
                    crate::im_gateway::external_cli::default_runs_root(),
                );
                match runtime.run(request).await {
                    Ok(result) => json_response(&result),
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
