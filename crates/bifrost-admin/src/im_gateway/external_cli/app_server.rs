use super::*;

const HANDSHAKE_TIMEOUT_SECS: u64 = 30;
const CAPACITY_MAX_RETRIES: u32 = 3;
const CAPACITY_RETRY_BASE_DELAY_MS: u64 = 1_000;
const APP_SERVER_SPAWN_MAX_ATTEMPTS: u32 = 8;
const APP_SERVER_SPAWN_RETRY_BASE_DELAY_MS: u64 = 5;
#[cfg(unix)]
const TEXT_FILE_BUSY_RAW_OS_ERROR: i32 = 26;

fn is_retryable_app_server_spawn_error(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(TEXT_FILE_BUSY_RAW_OS_ERROR)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

async fn spawn_app_server_with_retry<T>(
    mut spawn: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut attempt = 1;
    loop {
        match spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if is_retryable_app_server_spawn_error(&error)
                    && attempt < APP_SERVER_SPAWN_MAX_ATTEMPTS =>
            {
                let delay = Duration::from_millis(
                    APP_SERVER_SPAWN_RETRY_BASE_DELAY_MS.saturating_mul(u64::from(attempt)),
                );
                tracing::warn!(
                    attempt,
                    max_attempts = APP_SERVER_SPAWN_MAX_ATTEMPTS,
                    "app-server executable is temporarily busy; retrying spawn"
                );
                sleep(delay).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

struct AppServerRunCleanup {
    run_id: String,
    session_key: Option<String>,
    pid: u32,
    armed: bool,
}

impl AppServerRunCleanup {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AppServerRunCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(session_key) = self.session_key.as_deref() {
            remove_active_app_server_session(session_key, &self.run_id);
        }
        if self.pid != 0 {
            let _ = terminate_process(self.pid);
        }
        ACTIVE_RUNS.remove(&self.run_id);
        remove_active_sessions_for_run(&self.run_id);
    }
}

pub(super) fn resolved_transport(
    request: &ExternalCliRunRequest,
) -> Result<ExternalCliTransport, String> {
    let config = &request.adapter_config;
    match config.transport {
        Some(ExternalCliTransport::Exec) => Ok(ExternalCliTransport::Exec),
        Some(ExternalCliTransport::StreamJson) => {
            validate_stream_json_transport(request)?;
            Ok(ExternalCliTransport::StreamJson)
        }
        Some(ExternalCliTransport::AppServer) => {
            validate_app_server_transport(request)?;
            Ok(ExternalCliTransport::AppServer)
        }
        None if is_default_app_server_candidate(request) => Ok(ExternalCliTransport::AppServer),
        None if is_default_stream_json_candidate(request) => Ok(ExternalCliTransport::StreamJson),
        None => Ok(ExternalCliTransport::Exec),
    }
}

fn is_default_stream_json_candidate(request: &ExternalCliRunRequest) -> bool {
    request.adapter == CLAUDE_CODE_ADAPTER && request.adapter_config.args.is_empty()
}

fn validate_stream_json_transport(request: &ExternalCliRunRequest) -> Result<(), String> {
    if request.adapter != CLAUDE_CODE_ADAPTER {
        return Err(format!(
            "adapter '{}' does not support stream_json transport",
            request.adapter
        ));
    }
    if !request.adapter_config.args.is_empty() {
        return Err("stream_json transport cannot be combined with adapterConfig.args".to_string());
    }
    Ok(())
}

fn is_default_app_server_candidate(request: &ExternalCliRunRequest) -> bool {
    matches!(request.adapter.as_str(), DEFAULT_ADAPTER | TRAEX_ADAPTER)
        && default_executable_supports_app_server(request)
        && request.adapter_config.args.is_empty()
        && request.adapter_config.profile.is_none()
        && request.adapter_config.profile_v2.is_none()
        && !request.adapter_config.oss.unwrap_or(false)
        && request.adapter_config.local_provider.is_none()
        && !request.adapter_config.ignore_user_config.unwrap_or(false)
        && !request.adapter_config.ignore_rules.unwrap_or(false)
}

fn default_executable_supports_app_server(request: &ExternalCliRunRequest) -> bool {
    let Some(executable) = request.adapter_config.executable.as_deref() else {
        return true;
    };
    let executable_name = Path::new(executable)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .trim_end_matches(".exe");
    match request.adapter.as_str() {
        DEFAULT_ADAPTER => executable_name == "codex",
        TRAEX_ADAPTER => matches!(executable_name, "traex" | "traecli"),
        _ => false,
    }
}

fn validate_app_server_transport(request: &ExternalCliRunRequest) -> Result<(), String> {
    if !matches!(request.adapter.as_str(), DEFAULT_ADAPTER | TRAEX_ADAPTER) {
        return Err(format!(
            "adapter '{}' does not support app_server transport",
            request.adapter
        ));
    }
    if !request.adapter_config.args.is_empty() {
        return Err("app_server transport cannot be combined with adapterConfig.args".to_string());
    }
    if request.adapter_config.profile.is_some() || request.adapter_config.profile_v2.is_some() {
        return Err(
            "app_server transport does not support profile overrides; use transport=exec"
                .to_string(),
        );
    }
    if request.adapter_config.oss.unwrap_or(false)
        || request.adapter_config.local_provider.is_some()
        || request.adapter_config.ignore_user_config.unwrap_or(false)
        || request.adapter_config.ignore_rules.unwrap_or(false)
    {
        return Err(
            "app_server transport does not support the configured exec-only flags; use transport=exec"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn request_session_guide(
    session_key: &str,
    guide_id: String,
    message: String,
) -> ExternalCliGuideResult {
    live_guide::request_session_guide(session_key, guide_id, message).await
}

fn register_active_app_server_session(
    session_key: &str,
    run_id: &str,
    handle: live_guide::ActiveGuideHandle,
) -> bool {
    live_guide::register_session(session_key, run_id, handle)
}

fn active_session_is_owned_by(session_key: &str, run_id: &str) -> bool {
    live_guide::active_session_is_owned_by(session_key, run_id)
}

fn remove_active_app_server_session(session_key: &str, run_id: &str) {
    live_guide::remove_session(session_key, run_id);
}

fn rejected_guide(
    guide_id: String,
    thread_id: Option<String>,
    turn_id: Option<String>,
    reason: String,
) -> ExternalCliGuideResult {
    live_guide::rejected_guide(guide_id, thread_id, turn_id, reason)
}

fn app_server_command(spec: &CommandSpec) -> Command {
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    if let Some(work_dir) = spec.work_dir.as_ref() {
        command.current_dir(work_dir);
    }
    apply_command_environment(&mut command, &spec.env);
    command
}

async fn spawn_app_server(spec: &CommandSpec) -> std::io::Result<tokio::process::Child> {
    spawn_app_server_with_retry(|| app_server_command(spec).spawn()).await
}

pub(super) async fn run_command(
    run_id: &str,
    session_key: Option<&str>,
    request: &ExternalCliRunRequest,
    prompt: String,
    stop_marker_path: PathBuf,
    progress_tx: Option<mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<CommandOutput, String> {
    let (stdout_path, stderr_path) = external_cli_log_paths(&stop_marker_path);
    validate_app_server_transport(request)?;
    let spec = build_command_spec(request);
    let mut child = spawn_app_server(&spec).await.map_err(|error| {
        format!(
            "spawn {} app-server failed for executable '{}': {error}",
            request.adapter, spec.executable
        )
    })?;
    let pid = child.id().unwrap_or(0);
    if pid != 0 {
        ACTIVE_RUNS.insert(run_id.to_string(), pid);
    }
    if let Some(session_key) = session_key {
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
    }
    let mut cleanup = AppServerRunCleanup {
        run_id: run_id.to_string(),
        session_key: session_key.map(str::to_string),
        pid,
        armed: true,
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "app-server stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "app-server stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "app-server stderr unavailable".to_string())?;
    let (stdout, stdout_tee_task) =
        tee_external_cli_output(stdout, stdout_path, "app-server stdout").await?;
    let (stderr, stderr_tee_task) =
        tee_external_cli_output(stderr, stderr_path, "app-server stderr").await?;
    let stderr_task = tokio::spawn(read_stderr_lines(stderr));
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut stdout_bytes = Vec::new();
    let mut events = Vec::new();

    send_jsonrpc_request(
        &mut stdin,
        1,
        "initialize",
        serde_json::json!({
            "clientInfo": {
                "name": "bifrost",
                "title": "Bifrost External Runner",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "experimentalApi": true }
        }),
    )
    .await?;
    read_handshake_response(
        &mut lines,
        1,
        None,
        &mut stdout_bytes,
        &mut events,
        progress_tx.as_ref(),
    )
    .await?;
    send_jsonrpc_notification(&mut stdin, "initialized", serde_json::json!({})).await?;

    let existing_thread_id = command_spec::codex_thread_id_from_params(request);
    let (thread_method, thread_params) =
        build_thread_request(request, existing_thread_id.as_deref());
    send_jsonrpc_request(&mut stdin, 2, thread_method, thread_params).await?;
    let thread_response = read_handshake_response(
        &mut lines,
        2,
        existing_thread_id.as_deref(),
        &mut stdout_bytes,
        &mut events,
        progress_tx.as_ref(),
    )
    .await?;
    let response_thread_id = thread_response
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let thread_id = if matches!(request.operation.as_str(), "fork" | "checkpoint_fork") {
        response_thread_id.ok_or_else(|| {
            "app-server thread/fork response missing derived thread.id".to_string()
        })?
    } else {
        response_thread_id
            .or(existing_thread_id)
            .ok_or_else(|| "app-server thread response missing thread.id".to_string())?
    };

    if request.operation == "checkpoint_fork" {
        let event = ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::Status,
            content: "thread checkpoint created".to_string(),
            title: Some("thread_checkpoint".to_string()),
            raw: serde_json::json!({ "threadId": thread_id }),
        };
        if let Some(progress_tx) = progress_tx.as_ref() {
            let _ = progress_tx.try_send(event.clone());
        }
        events.push(event);
        if pid != 0 {
            let _ = terminate_process(pid);
        }
        let _ = timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), child.wait()).await;
        ACTIVE_RUNS.remove(run_id);
        remove_active_sessions_for_run(run_id);
        cleanup.disarm();
        let stderr = stderr_task
            .await
            .map_err(|error| format!("join app-server stderr task failed: {error}"))??;
        join_external_cli_tee(stdout_tee_task, "app-server stdout").await?;
        join_external_cli_tee(stderr_tee_task, "app-server stderr").await?;
        return Ok(CommandOutput {
            status: ExternalCliRunStatus::Succeeded,
            exit_code: Some(0),
            stdout: stdout_bytes,
            stderr,
            events,
        });
    }

    let client_user_message_id = format!("bifrost-{run_id}");
    send_jsonrpc_request(
        &mut stdin,
        3,
        "turn/start",
        build_turn_start_request(request, &thread_id, prompt.clone(), &client_user_message_id),
    )
    .await?;
    let turn_response = read_handshake_response(
        &mut lines,
        3,
        Some(&thread_id),
        &mut stdout_bytes,
        &mut events,
        progress_tx.as_ref(),
    )
    .await?;
    let mut turn_id = turn_response
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "app-server turn response missing turn.id".to_string())?;
    let turn_started_event = ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "turn started".to_string(),
        title: Some("Codex turn".to_string()),
        raw: serde_json::json!({
            "type": "turn_started",
            "threadId": thread_id.clone(),
            "turnId": turn_id.clone(),
        }),
    };
    if let Some(progress_tx) = progress_tx.as_ref() {
        let _ = progress_tx.try_send(turn_started_event.clone());
    }
    events.push(turn_started_event);

    let (guide_tx, mut guide_rx) = mpsc::unbounded_channel::<live_guide::LiveGuideCommand>();
    if let Some(session_key) = session_key {
        register_active_app_server_session(
            session_key,
            run_id,
            live_guide::ActiveGuideHandle {
                run_id: run_id.to_string(),
                thread_id: Some(thread_id.clone()),
                turn_id: Some(turn_id.clone()),
                guide_tx: guide_tx.clone(),
            },
        );
    }
    const RATE_LIMIT_REQUEST_ID: u64 = 90;
    if request.adapter == DEFAULT_ADAPTER {
        send_jsonrpc_request(
            &mut stdin,
            RATE_LIMIT_REQUEST_ID,
            "account/rateLimits/read",
            serde_json::Value::Null,
        )
        .await?;
    }

    let timeout_secs = request.adapter_config.timeout_secs;
    let timeout_sleep = async move {
        match timeout_secs {
            Some(seconds) => sleep(Duration::from_secs(seconds)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timeout_sleep);
    let mut next_request_id = 100u64;
    let mut pending_guides = HashMap::<u64, live_guide::LiveGuideCommand>::new();
    let mut status = ExternalCliRunStatus::Failed;
    let mut exit_code = Some(1);
    let mut terminal = false;
    let mut terminal_error = None;
    let mut capacity_retries = 0u32;
    let mut turn_has_side_effects = false;
    let mut stop_request_id = None::<u64>;
    let mut stop_deadline = None::<std::pin::Pin<Box<tokio::time::Sleep>>>;

    while !terminal {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.map_err(|error| format!("read app-server stdout failed: {error}"))? else {
                    terminal_error = Some("app-server exited before turn/completed".to_string());
                    break;
                };
                record_stdout_line(&mut stdout_bytes, &line);
                let frame: serde_json::Value = serde_json::from_str(&line)
                    .map_err(|error| format!("parse app-server frame failed: {error}; line={line}"))?;
                if let Some(id) = frame.get("id").and_then(serde_json::Value::as_u64) {
                    if stop_request_id == Some(id) {
                        if let Some(error) = frame.get("error") {
                            terminal_error = Some(format!(
                                "app-server rejected turn/interrupt: {}",
                                jsonrpc_error_message(error)
                            ));
                        }
                        status = ExternalCliRunStatus::Stopped;
                        exit_code = None;
                        terminal = true;
                        continue;
                    }
                    if id == RATE_LIMIT_REQUEST_ID {
                        if let Some(response) = frame.get("result").cloned() {
                            let event = account_rate_limits_event(response);
                            if let Some(progress_tx) = progress_tx.as_ref() {
                                let _ = progress_tx.try_send(event.clone());
                            }
                            events.push(event);
                        } else if let Some(error) = frame.get("error") {
                            tracing::debug!(
                                error = %jsonrpc_error_message(error),
                                "Codex app-server does not provide an account rate-limit snapshot"
                            );
                        }
                        continue;
                    }
                    if let Some(command) = pending_guides.remove(&id) {
                        let result = guide_result_from_response(
                            command.guide_id,
                            &thread_id,
                            &turn_id,
                            &frame,
                        );
                        let _ = command.ack_tx.send(result);
                    }
                    continue;
                }
                if !app_server_frame_belongs_to_active_turn(&frame, &thread_id, &turn_id) {
                    tracing::debug!(
                        method = frame.get("method").and_then(serde_json::Value::as_str),
                        root_thread_id = %thread_id,
                        root_turn_id = %turn_id,
                        "ignoring app-server notification outside the active root turn"
                    );
                    continue;
                }
                let frame_events = progress_events_from_app_server_frame(&frame);
                if !frame_events.is_empty() {
                    let can_retry_capacity = should_retry_capacity_error(
                        &frame,
                        turn_has_side_effects,
                        !pending_guides.is_empty(),
                        capacity_retries,
                    );
                    if can_retry_capacity {
                        capacity_retries = capacity_retries.saturating_add(1);
                        let delay = capacity_retry_delay(capacity_retries);
                        let retry_event = capacity_retry_status_event(
                            capacity_retries,
                            CAPACITY_MAX_RETRIES,
                            delay,
                            &frame,
                        );
                        if let Some(progress_tx) = progress_tx.as_ref() {
                            let _ = progress_tx.try_send(retry_event.clone());
                        }
                        events.push(retry_event);
                        if let Some(session_key) = session_key {
                            remove_active_app_server_session(session_key, run_id);
                        }
                        tokio::select! {
                            _ = sleep(delay) => {}
                            _ = wait_for_stop_marker(stop_marker_path.clone()) => {
                                status = ExternalCliRunStatus::Stopped;
                                exit_code = None;
                                terminal = true;
                            }
                        }
                        if terminal {
                            continue;
                        }
                        let retry_request_id = 3u64.saturating_add(capacity_retries as u64);
                        send_jsonrpc_request(
                            &mut stdin,
                            retry_request_id,
                            "turn/start",
                            build_turn_start_request(
                                request,
                                &thread_id,
                                prompt.clone(),
                                &client_user_message_id,
                            ),
                        )
                        .await?;
                        let retry_turn_response = read_handshake_response(
                            &mut lines,
                            retry_request_id,
                            Some(&thread_id),
                            &mut stdout_bytes,
                            &mut events,
                            progress_tx.as_ref(),
                        )
                        .await?;
                        turn_id = retry_turn_response
                            .get("turn")
                            .and_then(|turn| turn.get("id"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| {
                                "app-server retry response missing turn.id".to_string()
                            })?;
                        if let Some(session_key) = session_key {
                            register_active_app_server_session(
                                session_key,
                                run_id,
                                live_guide::ActiveGuideHandle {
                                    run_id: run_id.to_string(),
                                    thread_id: Some(thread_id.clone()),
                                    turn_id: Some(turn_id.clone()),
                                    guide_tx: guide_tx.clone(),
                                },
                            );
                        }
                        turn_has_side_effects = false;
                        continue;
                    }
                    for event in frame_events {
                        turn_has_side_effects |= progress_event_has_retry_side_effect(&event);
                        if let Some(progress_tx) = progress_tx.as_ref() {
                            let _ = progress_tx.try_send(event.clone());
                        }
                        if event.event_type == ExternalCliProgressEventType::RunFinished
                            || event.event_type == ExternalCliProgressEventType::RunFailed
                        {
                            let turn_status = frame
                                .get("params")
                                .and_then(|params| params.get("turn"))
                                .and_then(|turn| turn.get("status"))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default();
                            match turn_status {
                                "completed" => {
                                    status = ExternalCliRunStatus::Succeeded;
                                    exit_code = Some(0);
                                }
                                "interrupted" | "cancelled" => {
                                    status = ExternalCliRunStatus::Stopped;
                                    exit_code = None;
                                }
                                _ => {
                                    status = ExternalCliRunStatus::Failed;
                                    exit_code = Some(1);
                                }
                            }
                            terminal = true;
                        }
                        events.push(event);
                    }
                }
            }
            command = guide_rx.recv() => {
                let Some(command) = command else { continue; };
                if stop_request_id.is_some() {
                    let _ = command.ack_tx.send(rejected_guide(
                        command.guide_id,
                        Some(thread_id.clone()),
                        Some(turn_id.clone()),
                        "app-server turn is stopping".to_string(),
                    ));
                    continue;
                }
                if session_key.is_some_and(|session_key| {
                    !active_session_is_owned_by(session_key, run_id)
                }) {
                    let _ = command.ack_tx.send(rejected_guide(
                        command.guide_id,
                        Some(thread_id.clone()),
                        Some(turn_id.clone()),
                        "active session was replaced before guide delivery".to_string(),
                    ));
                    continue;
                }
                next_request_id = next_request_id.saturating_add(1);
                let request_id = next_request_id;
                let params = build_turn_steer_request(
                    &thread_id,
                    &turn_id,
                    &command.guide_id,
                    &command.message,
                );
                match send_jsonrpc_request(&mut stdin, request_id, "turn/steer", params).await {
                    Ok(()) => {
                        pending_guides.insert(request_id, command);
                    }
                    Err(error) => {
                        let _ = command.ack_tx.send(rejected_guide(
                            command.guide_id,
                            Some(thread_id.clone()),
                            Some(turn_id.clone()),
                            error,
                        ));
                    }
                }
            }
            _ = wait_for_stop_marker(stop_marker_path.clone()), if stop_request_id.is_none() => {
                const STOP_REQUEST_ID: u64 = 99;
                match send_jsonrpc_request(
                    &mut stdin,
                    STOP_REQUEST_ID,
                    "turn/interrupt",
                    serde_json::json!({"threadId": thread_id, "turnId": turn_id}),
                ).await {
                    Ok(()) => {
                        stop_request_id = Some(STOP_REQUEST_ID);
                        stop_deadline = Some(Box::pin(sleep(Duration::from_millis(
                            WORKER_TRANSPORT_STOP_GRACE_MS,
                        ))));
                    }
                    Err(error) => {
                        terminal_error = Some(format!(
                            "failed to send app-server turn/interrupt: {error}"
                        ));
                        status = ExternalCliRunStatus::Stopped;
                        exit_code = None;
                        terminal = true;
                    }
                }
            }
            _ = &mut timeout_sleep, if stop_request_id.is_none() => {
                if let Err(error) = send_jsonrpc_request(
                    &mut stdin,
                    98,
                    "turn/interrupt",
                    serde_json::json!({"threadId": thread_id, "turnId": turn_id}),
                ).await {
                    tracing::warn!(
                        thread_id,
                        turn_id,
                        error = %error,
                        "failed to interrupt timed-out app-server turn"
                    );
                }
                status = ExternalCliRunStatus::TimedOut;
                exit_code = None;
                terminal_error = Some(format!(
                    "app-server turn timed out after {} seconds",
                    timeout_secs.unwrap_or_default(),
                ));
                terminal = true;
            }
            _ = async {
                match stop_deadline.as_mut() {
                    Some(deadline) => deadline.await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                terminal_error = Some(
                    "app-server did not acknowledge turn/interrupt before termination".to_string()
                );
                status = ExternalCliRunStatus::Stopped;
                exit_code = None;
                terminal = true;
            }
        }
    }

    if let Some(session_key) = session_key {
        remove_active_app_server_session(session_key, run_id);
    }
    for (_, command) in pending_guides {
        let _ = command.ack_tx.send(rejected_guide(
            command.guide_id,
            Some(thread_id.clone()),
            Some(turn_id.clone()),
            "turn completed before guide acknowledgement".to_string(),
        ));
    }
    while let Ok(command) = guide_rx.try_recv() {
        let _ = command.ack_tx.send(rejected_guide(
            command.guide_id,
            Some(thread_id.clone()),
            Some(turn_id.clone()),
            "turn is no longer active".to_string(),
        ));
    }

    if pid != 0 {
        let _ = terminate_process(pid);
    }
    let _ = timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), child.wait()).await;
    ACTIVE_RUNS.remove(run_id);
    remove_active_sessions_for_run(run_id);
    cleanup.disarm();
    let mut stderr = stderr_task
        .await
        .map_err(|error| format!("join app-server stderr task failed: {error}"))??;
    join_external_cli_tee(stdout_tee_task, "app-server stdout").await?;
    join_external_cli_tee(stderr_tee_task, "app-server stderr").await?;
    if let Some(error) = terminal_error {
        if !stderr.is_empty() && !stderr.ends_with(b"\n") {
            stderr.push(b'\n');
        }
        stderr.extend_from_slice(error.as_bytes());
        stderr.push(b'\n');
    }
    Ok(CommandOutput {
        status,
        exit_code,
        stdout: stdout_bytes,
        stderr,
        events,
    })
}

fn is_capacity_error_frame(frame: &serde_json::Value) -> bool {
    if frame.get("method").and_then(serde_json::Value::as_str) != Some("error") {
        return false;
    }
    let params = frame.get("params");
    let overloaded = params
        .and_then(|params| params.get("error"))
        .and_then(|error| error.get("codexErrorInfo"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("serverOverloaded"));
    let cli_will_retry = params
        .and_then(|params| params.get("willRetry"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    overloaded && !cli_will_retry
}

fn should_retry_capacity_error(
    frame: &serde_json::Value,
    turn_has_side_effects: bool,
    has_pending_guides: bool,
    retries_used: u32,
) -> bool {
    is_capacity_error_frame(frame)
        && !turn_has_side_effects
        && !has_pending_guides
        && retries_used < CAPACITY_MAX_RETRIES
}

fn progress_event_has_retry_side_effect(event: &ExternalCliProgressEvent) -> bool {
    matches!(
        event.event_type,
        ExternalCliProgressEventType::AssistantDelta
            | ExternalCliProgressEventType::AssistantFinal
            | ExternalCliProgressEventType::ToolStarted
            | ExternalCliProgressEventType::ToolFinished
            | ExternalCliProgressEventType::SubAgentUpdated
    )
}

fn app_server_frame_belongs_to_active_turn(
    frame: &serde_json::Value,
    root_thread_id: &str,
    root_turn_id: &str,
) -> bool {
    let Some(method) = frame.get("method").and_then(serde_json::Value::as_str) else {
        return false;
    };
    if method == "account/rateLimits/updated" {
        return true;
    }
    if !app_server_frame_belongs_to_root_thread(frame, root_thread_id) {
        return false;
    }
    let params = frame.get("params").expect("root thread scope has params");
    let turn_id = params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(serde_json::Value::as_str)
        });
    if method == "turn/completed" {
        return turn_id == Some(root_turn_id);
    }
    turn_id.is_none_or(|turn_id| turn_id == root_turn_id)
}

fn app_server_frame_belongs_to_root_thread(
    frame: &serde_json::Value,
    root_thread_id: &str,
) -> bool {
    if frame.get("method").and_then(serde_json::Value::as_str) == Some("account/rateLimits/updated")
    {
        return true;
    }
    let Some(params) = frame.get("params") else {
        return false;
    };
    let thread_id = params
        .get("threadId")
        .or_else(|| params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            params
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(serde_json::Value::as_str)
        });
    thread_id == Some(root_thread_id)
}

fn capacity_retry_delay(retry_attempt: u32) -> Duration {
    if cfg!(test) {
        return Duration::from_millis(10);
    }
    let exponent = retry_attempt.saturating_sub(1).min(2);
    Duration::from_millis(CAPACITY_RETRY_BASE_DELAY_MS.saturating_mul(1u64 << exponent))
}

fn capacity_retry_status_event(
    retry_attempt: u32,
    max_retries: u32,
    delay: Duration,
    error_frame: &serde_json::Value,
) -> ExternalCliProgressEvent {
    ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: format!(
            "Selected model is at capacity; retrying in {} ms ({retry_attempt}/{max_retries})",
            delay.as_millis()
        ),
        title: Some("Codex capacity retry".to_string()),
        raw: serde_json::json!({
            "type": "capacity_retry",
            "retryAttempt": retry_attempt,
            "maxRetries": max_retries,
            "delayMs": delay.as_millis(),
            "error": error_frame,
        }),
    }
}

pub(super) fn build_command_spec(request: &ExternalCliRunRequest) -> CommandSpec {
    let config = &request.adapter_config;
    let mut args = if request.adapter == TRAEX_ADAPTER {
        vec![
            "app-server".to_string(),
            "--listen".to_string(),
            "stdio://".to_string(),
        ]
    } else {
        vec!["app-server".to_string(), "--stdio".to_string()]
    };
    if request.adapter == DEFAULT_ADAPTER && config.strict_config.unwrap_or(false) {
        args.push("--strict-config".to_string());
    }
    let overrides = config.config_overrides.clone();
    if config.search == Some(true)
        && !config
            .enable_features
            .iter()
            .any(|value| value == "web_search")
    {
        args.push("--enable".to_string());
        args.push("web_search".to_string());
    }
    for value in overrides {
        args.push("--config".to_string());
        args.push(value);
    }
    for value in &config.enable_features {
        args.push("--enable".to_string());
        args.push(value.clone());
    }
    for value in &config.disable_features {
        args.push("--disable".to_string());
        args.push(value.clone());
    }
    CommandSpec {
        executable: config
            .executable
            .clone()
            .unwrap_or_else(|| request.adapter.clone()),
        args,
        env: config.env.clone(),
        work_dir: request.work_dir.clone(),
        timeout_secs: config.timeout_secs,
    }
}

fn build_thread_request(
    request: &ExternalCliRunRequest,
    existing_thread_id: Option<&str>,
) -> (&'static str, serde_json::Value) {
    if matches!(request.operation.as_str(), "fork" | "checkpoint_fork") {
        let mut params = serde_json::Map::new();
        if let Some(thread_id) = existing_thread_id {
            params.insert("threadId".to_string(), serde_json::json!(thread_id));
        }
        if request.adapter == DEFAULT_ADAPTER {
            if let Some(last_turn_id) = request
                .params
                .get("lastTurnId")
                .or_else(|| request.params.get("last_turn_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                params.insert("lastTurnId".to_string(), serde_json::json!(last_turn_id));
            }
        }
        return ("thread/fork", serde_json::Value::Object(params));
    }
    let config = &request.adapter_config;
    let danger_full_access = effective_danger_full_access(request);
    let mut params = serde_json::Map::new();
    if let Some(thread_id) = existing_thread_id {
        params.insert("threadId".to_string(), serde_json::json!(thread_id));
    }
    if let Some(work_dir) = request.work_dir.as_ref() {
        params.insert(
            "cwd".to_string(),
            serde_json::json!(work_dir.display().to_string()),
        );
    }
    if let Some(model) = config.model.as_deref() {
        params.insert("model".to_string(), serde_json::json!(model));
    }
    if danger_full_access {
        params.insert(
            "sandbox".to_string(),
            serde_json::json!("danger-full-access"),
        );
        params.insert(
            "approvalPolicy".to_string(),
            serde_json::json!(config.approval_policy.as_deref().unwrap_or("never")),
        );
    } else {
        if let Some(sandbox) = config.sandbox.as_deref() {
            params.insert("sandbox".to_string(), serde_json::json!(sandbox));
        }
        if let Some(approval) = config.approval_policy.as_deref() {
            params.insert("approvalPolicy".to_string(), serde_json::json!(approval));
        }
    }
    if let Some(service_tier) = resolved_service_tier(request) {
        params.insert("serviceTier".to_string(), serde_json::json!(service_tier));
    }
    if existing_thread_id.is_none() && config.ephemeral.unwrap_or(false) {
        params.insert("ephemeral".to_string(), serde_json::json!(true));
    }
    (
        if existing_thread_id.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        },
        serde_json::Value::Object(params),
    )
}

fn build_turn_start_request(
    request: &ExternalCliRunRequest,
    thread_id: &str,
    prompt: String,
    client_user_message_id: &str,
) -> serde_json::Value {
    let config = &request.adapter_config;
    let mut params = serde_json::Map::from_iter([
        ("threadId".to_string(), serde_json::json!(thread_id)),
        (
            "input".to_string(),
            serde_json::json!([{ "type": "text", "text": prompt }]),
        ),
        (
            "clientUserMessageId".to_string(),
            serde_json::json!(client_user_message_id),
        ),
    ]);
    if let Some(work_dir) = request.work_dir.as_ref() {
        params.insert(
            "cwd".to_string(),
            serde_json::json!(work_dir.display().to_string()),
        );
    }
    if let Some(model) = config.model.as_deref() {
        params.insert("model".to_string(), serde_json::json!(model));
    }
    if let Some(effort) = config.reasoning_effort.as_deref() {
        params.insert("effort".to_string(), serde_json::json!(effort));
    }
    if let Some(summary) = config.reasoning_summary.as_deref() {
        params.insert("summary".to_string(), serde_json::json!(summary));
    }
    if let Some(service_tier) = resolved_service_tier(request) {
        params.insert("serviceTier".to_string(), serde_json::json!(service_tier));
    }
    if !config.add_dirs.is_empty() {
        let roots = config
            .add_dirs
            .iter()
            .map(|path| serde_json::json!(path))
            .collect::<Vec<_>>();
        params.insert(
            "runtimeWorkspaceRoots".to_string(),
            serde_json::json!(roots),
        );
    }
    serde_json::Value::Object(params)
}

fn build_turn_steer_request(
    thread_id: &str,
    turn_id: &str,
    guide_id: &str,
    message: &str,
) -> serde_json::Value {
    serde_json::json!({
        "threadId": thread_id,
        "expectedTurnId": turn_id,
        "clientUserMessageId": guide_id,
        "input": [{ "type": "text", "text": message }]
    })
}

fn effective_danger_full_access(request: &ExternalCliRunRequest) -> bool {
    request
        .adapter_config
        .danger_full_access
        .unwrap_or_else(|| {
            if request.adapter == TRAEX_ADAPTER {
                request
                    .adapter_config
                    .permission_mode
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && *value != "default")
                    .unwrap_or("bypass_permissions")
                    == "bypass_permissions"
            } else {
                request.adapter_config.sandbox.is_none()
                    && request.adapter_config.approval_policy.is_none()
            }
        })
}

fn resolved_service_tier(request: &ExternalCliRunRequest) -> Option<String> {
    resolve_external_cli_service_tier(&request.adapter, &request.adapter_config).0
}

async fn send_jsonrpc_request(
    stdin: &mut tokio::process::ChildStdin,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<(), String> {
    write_jsonrpc_frame(
        stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await
}

async fn send_jsonrpc_notification(
    stdin: &mut tokio::process::ChildStdin,
    method: &str,
    params: serde_json::Value,
) -> Result<(), String> {
    write_jsonrpc_frame(
        stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }),
    )
    .await
}

async fn write_jsonrpc_frame(
    stdin: &mut tokio::process::ChildStdin,
    frame: &serde_json::Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| format!("serialize app-server request failed: {error}"))?;
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|error| format!("write app-server request failed: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush app-server request failed: {error}"))
}

async fn read_until_response(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<DuplexStream>>,
    expected_id: u64,
    root_thread_id: Option<&str>,
    stdout_bytes: &mut Vec<u8>,
    events: &mut Vec<ExternalCliProgressEvent>,
    progress_tx: Option<&mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<serde_json::Value, String> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|error| format!("read app-server response failed: {error}"))?
            .ok_or_else(|| "app-server exited during handshake".to_string())?;
        record_stdout_line(stdout_bytes, &line);
        let frame: serde_json::Value = serde_json::from_str(&line)
            .map_err(|error| format!("parse app-server response failed: {error}; line={line}"))?;
        if frame.get("id").and_then(serde_json::Value::as_u64) == Some(expected_id) {
            if let Some(error) = frame.get("error") {
                return Err(jsonrpc_error_message(error));
            }
            return frame
                .get("result")
                .cloned()
                .ok_or_else(|| format!("app-server response {expected_id} missing result"));
        }
        if root_thread_id.is_some_and(|root_thread_id| {
            !app_server_frame_belongs_to_root_thread(&frame, root_thread_id)
        }) {
            continue;
        }
        for event in progress_events_from_app_server_frame(&frame) {
            if let Some(progress_tx) = progress_tx {
                let _ = progress_tx.try_send(event.clone());
            }
            events.push(event);
        }
    }
}

async fn read_handshake_response(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<DuplexStream>>,
    expected_id: u64,
    root_thread_id: Option<&str>,
    stdout_bytes: &mut Vec<u8>,
    events: &mut Vec<ExternalCliProgressEvent>,
    progress_tx: Option<&mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<serde_json::Value, String> {
    timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_until_response(
            lines,
            expected_id,
            root_thread_id,
            stdout_bytes,
            events,
            progress_tx,
        ),
    )
    .await
    .map_err(|_| {
        format!("app-server request {expected_id} timed out after {HANDSHAKE_TIMEOUT_SECS} seconds")
    })?
}

pub(super) fn record_stdout_line(bytes: &mut Vec<u8>, line: &str) {
    append_tail(bytes, line.as_bytes(), MAX_CAPTURED_STREAM_BYTES);
    append_tail(bytes, b"\n", MAX_CAPTURED_STREAM_BYTES);
}

fn guide_result_from_response(
    guide_id: String,
    thread_id: &str,
    turn_id: &str,
    frame: &serde_json::Value,
) -> ExternalCliGuideResult {
    if let Some(error) = frame.get("error") {
        return rejected_guide(
            guide_id,
            Some(thread_id.to_string()),
            Some(turn_id.to_string()),
            jsonrpc_error_message(error),
        );
    }
    let accepted_turn_id = frame
        .get("result")
        .and_then(|result| result.get("turnId"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(turn_id)
        .to_string();
    ExternalCliGuideResult {
        guide_id,
        accepted: true,
        thread_id: Some(thread_id.to_string()),
        turn_id: Some(accepted_turn_id),
        reason: None,
    }
}

fn jsonrpc_error_message(error: &serde_json::Value) -> String {
    error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error.to_string())
}

fn progress_event_from_app_server_frame(
    frame: &serde_json::Value,
) -> Option<ExternalCliProgressEvent> {
    let method = frame.get("method")?.as_str()?;
    let params = frame.get("params").cloned().unwrap_or_default();
    let mut raw = frame.clone();
    if method == "thread/started" {
        if let Some(thread_id) = params
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .or_else(|| params.get("threadId"))
            .and_then(serde_json::Value::as_str)
        {
            if let Some(object) = raw.as_object_mut() {
                object.insert("thread_id".to_string(), serde_json::json!(thread_id));
            }
        }
    }
    match method {
        "thread/started" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::RunStarted,
            content: params
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .or_else(|| params.get("threadId"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("thread started")
                .to_string(),
            title: Some("Codex thread".to_string()),
            raw,
        }),
        "turn/started" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::Status,
            content: "turn started".to_string(),
            title: Some("Codex turn".to_string()),
            raw,
        }),
        "turn/completed" => {
            let status = params
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("completed");
            Some(ExternalCliProgressEvent {
                event_type: if status == "completed" {
                    ExternalCliProgressEventType::RunFinished
                } else {
                    ExternalCliProgressEventType::RunFailed
                },
                content: params
                    .get("turn")
                    .and_then(|turn| turn.get("error"))
                    .and_then(|error| error.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(status)
                    .to_string(),
                title: Some("Codex turn".to_string()),
                raw,
            })
        }
        "item/agentMessage/delta" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantDelta,
            content: params
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: Some("agent_message".to_string()),
            raw,
        }),
        "turn/plan/updated" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::PlanUpdated,
            content: "plan updated".to_string(),
            title: None,
            raw: serde_json::json!({
                "items": params.get("plan").cloned().unwrap_or_default(),
                "appServerFrame": raw,
            }),
        }),
        "thread/tokenUsage/updated" => {
            let total = params
                .get("tokenUsage")
                .and_then(|usage| usage.get("total"))
                .cloned()
                .unwrap_or_default();
            if let Some(object) = raw.as_object_mut() {
                object.insert(
                    "usage".to_string(),
                    serde_json::json!({
                        "input_tokens": total.get("inputTokens").cloned().unwrap_or_default(),
                        "cached_input_tokens": total.get("cachedInputTokens").cloned().unwrap_or_default(),
                        "output_tokens": total.get("outputTokens").cloned().unwrap_or_default(),
                        "reasoning_output_tokens": total.get("reasoningOutputTokens").cloned().unwrap_or_default(),
                        "total_tokens": total.get("totalTokens").cloned().unwrap_or_default(),
                    }),
                );
            }
            Some(ExternalCliProgressEvent {
                event_type: ExternalCliProgressEventType::Status,
                content: "token usage updated".to_string(),
                title: Some("token_usage".to_string()),
                raw,
            })
        }
        "account/rateLimits/updated" => Some(account_rate_limits_event(params)),
        "item/started" | "item/updated" | "item/completed" => {
            progress_event_from_app_server_item(method, &params, raw)
        }
        "error" => {
            let will_retry = params
                .get("willRetry")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some(ExternalCliProgressEvent {
                event_type: if will_retry {
                    ExternalCliProgressEventType::Status
                } else {
                    ExternalCliProgressEventType::RunFailed
                },
                content: params
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("app-server error")
                    .to_string(),
                title: Some(if will_retry {
                    "Codex reconnecting".to_string()
                } else {
                    "Codex error".to_string()
                }),
                raw,
            })
        }
        _ => None,
    }
}

fn progress_events_from_app_server_frame(raw: &serde_json::Value) -> Vec<ExternalCliProgressEvent> {
    progress_event_from_app_server_frame(raw)
        .map(expand_subagent_progress_event)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "app_server_retry_tests.rs"]
mod retry_tests;

fn account_rate_limits_event(response: serde_json::Value) -> ExternalCliProgressEvent {
    let weekly = codex_weekly_rate_limit_window(&response)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    ExternalCliProgressEvent {
        event_type: ExternalCliProgressEventType::Status,
        content: "usage updated".to_string(),
        title: Some("rate_limits".to_string()),
        raw: serde_json::json!({
            "type": "account_rate_limits",
            "weekly": weekly,
        }),
    }
}

fn codex_weekly_rate_limit_window(value: &serde_json::Value) -> Option<&serde_json::Value> {
    const WEEKLY_WINDOW_MINUTES: u64 = 7 * 24 * 60;
    let snapshots = [
        value
            .get("rateLimitsByLimitId")
            .and_then(serde_json::Value::as_object)
            .and_then(|limits| limits.get("codex")),
        value.get("rateLimits"),
        Some(value),
    ];
    snapshots.into_iter().flatten().find_map(|snapshot| {
        [snapshot.get("primary"), snapshot.get("secondary")]
            .into_iter()
            .flatten()
            .find(|window| {
                window
                    .get("windowDurationMins")
                    .and_then(serde_json::Value::as_u64)
                    == Some(WEEKLY_WINDOW_MINUTES)
            })
    })
}

fn progress_event_from_app_server_item(
    method: &str,
    params: &serde_json::Value,
    mut raw: serde_json::Value,
) -> Option<ExternalCliProgressEvent> {
    let item = params.get("item")?;
    let item_type = item.get("type")?.as_str()?;
    let completed = method == "item/completed";
    if is_codex_subagent_activity_item_type(item_type) {
        return None;
    }
    if is_codex_collaboration_tool_item_type(item_type) {
        if method == "item/updated" {
            return None;
        }
        return codex_collaboration_tool_event(
            &serde_json::json!({
                "type": method.replace('/', "."),
                "item": item,
            }),
            completed,
        );
    }
    match item_type {
        "agentMessage" if completed => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantFinal,
            content: item
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: Some("agent_message".to_string()),
            raw,
        }),
        "reasoning" => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantDelta,
            content: item
                .get("summary")
                .or_else(|| item.get("content"))
                .and_then(string_or_string_array)
                .unwrap_or_default(),
            title: Some("reasoning".to_string()),
            raw,
        }),
        "plan" if completed => Some(ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantDelta,
            content: item
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: Some("plan".to_string()),
            raw,
        }),
        "commandExecution" => {
            let command = item
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let output = item
                .get("aggregatedOutput")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(object) = raw.as_object_mut() {
                object.insert("tool_name".to_string(), serde_json::json!("exec_command"));
                object.insert(
                    "arguments".to_string(),
                    serde_json::json!({"command": command}),
                );
                object.insert(
                    "success".to_string(),
                    serde_json::json!(item
                        .get("exitCode")
                        .and_then(serde_json::Value::as_i64)
                        .is_none_or(|code| code == 0)),
                );
                if let Some(duration) = item.get("durationMs") {
                    object.insert("durationMs".to_string(), duration.clone());
                }
            }
            Some(ExternalCliProgressEvent {
                event_type: if completed {
                    ExternalCliProgressEventType::ToolFinished
                } else {
                    ExternalCliProgressEventType::ToolStarted
                },
                content: if completed { output } else { command },
                title: Some("exec_command".to_string()),
                raw,
            })
        }
        "mcpToolCall" | "dynamicToolCall" | "fileChange" => {
            let tool_name = item
                .get("tool")
                .or_else(|| item.get("type"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let content = if completed && item_type == "fileChange" {
                file_change_detail_from_value(item).unwrap_or_default()
            } else if completed {
                item.get("result")
                    .or_else(|| item.get("error"))
                    .map(serde_json::Value::to_string)
                    .unwrap_or_default()
            } else {
                item.get("arguments")
                    .or_else(|| item.get("changes"))
                    .map(serde_json::Value::to_string)
                    .unwrap_or_default()
            };
            if let Some(object) = raw.as_object_mut() {
                object.insert("tool_name".to_string(), serde_json::json!(tool_name));
                object.insert(
                    "arguments".to_string(),
                    item.get("arguments").cloned().unwrap_or_default(),
                );
                if let Some(duration) = item.get("durationMs") {
                    object.insert("durationMs".to_string(), duration.clone());
                }
                object.insert(
                    "success".to_string(),
                    serde_json::json!(item.get("error").is_none_or(serde_json::Value::is_null)),
                );
            }
            Some(ExternalCliProgressEvent {
                event_type: if completed {
                    ExternalCliProgressEventType::ToolFinished
                } else {
                    ExternalCliProgressEventType::ToolStarted
                },
                content,
                title: Some(tool_name),
                raw,
            })
        }
        _ => None,
    }
}

fn string_or_string_array(value: &serde_json::Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        return Some(value.to_string());
    }
    value.as_array().map(|values| {
        values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_spawn_retries_text_file_busy_and_then_succeeds() {
        let mut attempts = 0;
        let result = spawn_app_server_with_retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::Error::from_raw_os_error(
                    TEXT_FILE_BUSY_RAW_OS_ERROR,
                ))
            } else {
                Ok("spawned")
            }
        })
        .await;

        assert_eq!(result.unwrap(), "spawned");
        assert_eq!(attempts, 3);
    }

    #[tokio::test]
    async fn app_server_spawn_does_not_retry_non_transient_errors() {
        let mut attempts = 0;
        let error = spawn_app_server_with_retry(|| -> std::io::Result<()> {
            attempts += 1;
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "missing app-server executable",
            ))
        })
        .await
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(attempts, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_spawn_stops_after_retry_limit() {
        let mut attempts = 0;
        let error = spawn_app_server_with_retry(|| -> std::io::Result<()> {
            attempts += 1;
            Err(std::io::Error::from_raw_os_error(
                TEXT_FILE_BUSY_RAW_OS_ERROR,
            ))
        })
        .await
        .unwrap_err();

        assert_eq!(error.raw_os_error(), Some(TEXT_FILE_BUSY_RAW_OS_ERROR));
        assert_eq!(attempts, APP_SERVER_SPAWN_MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn app_server_run_reports_spawn_executable_context() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let executable = temp_dir.path().join("missing").join("codex");
        let mut request = request(DEFAULT_ADAPTER);
        request.adapter_config.executable = Some(executable.display().to_string());

        let error = run_command(
            "spawn-error-run",
            Some("spawn-error-session"),
            &request,
            "hello".to_string(),
            temp_dir.path().join("stop"),
            None,
        )
        .await
        .expect_err("missing executable should fail before creating app-server state");

        assert!(error.contains("spawn codex app-server failed for executable"));
        assert!(error.contains(executable.to_string_lossy().as_ref()));
        assert!(!ACTIVE_RUNS.contains_key("spawn-error-run"));
        assert!(!ACTIVE_SESSIONS.contains_key("spawn-error-session"));
    }

    fn request(adapter: &str) -> ExternalCliRunRequest {
        ExternalCliRunRequest {
            message: "hello".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            operation: "ask".to_string(),
            params: serde_json::json!({}),
            provider_id: None,
            runner_id: Some(adapter.to_string()),
            session_key: Some(format!("session:{adapter}")),
            runtime: DEFAULT_RUNTIME.to_string(),
            adapter: adapter.to_string(),
            adapter_config: ExternalCliAdapterConfig::default(),
            allow_work_dirs: Vec::new(),
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            instructions: None,
            work_dir: None,
        }
    }

    #[test]
    fn codex_and_traex_default_to_app_server_transport() {
        assert_eq!(
            resolved_transport(&request(DEFAULT_ADAPTER)).unwrap(),
            ExternalCliTransport::AppServer
        );
        assert_eq!(
            resolved_transport(&request(TRAEX_ADAPTER)).unwrap(),
            ExternalCliTransport::AppServer
        );
        assert_eq!(
            resolved_transport(&request(CLAUDE_CODE_ADAPTER)).unwrap(),
            ExternalCliTransport::StreamJson
        );
        assert_eq!(
            build_command_spec(&request(DEFAULT_ADAPTER)).args[..2],
            ["app-server", "--stdio"]
        );
        assert_eq!(
            build_command_spec(&request(TRAEX_ADAPTER)).args[..3],
            ["app-server", "--listen", "stdio://"]
        );
    }

    #[test]
    fn codex_app_server_omits_service_tier_without_explicit_configuration() {
        let request = request(DEFAULT_ADAPTER);
        let command = build_command_spec(&request);
        let (_, start_params) = build_thread_request(&request, None);
        let (_, resume_params) = build_thread_request(&request, Some("thread-default-mode"));
        let turn_params = build_turn_start_request(
            &request,
            "thread-default-mode",
            "hello".to_string(),
            "msg-default",
        );

        assert!(!command.args.windows(2).any(|pair| {
            pair[0] == "--config" && pair[1].trim_start().starts_with("service_tier=")
        }));
        assert!(start_params.get("serviceTier").is_none(), "{start_params}");
        assert!(
            resume_params.get("serviceTier").is_none(),
            "{resume_params}"
        );
        assert!(turn_params.get("serviceTier").is_none(), "{turn_params}");
    }

    #[test]
    fn codex_app_server_thread_and_turn_use_session_service_tier_override() {
        let mut request = request(DEFAULT_ADAPTER);
        request.adapter_config.config_overrides = vec![
            "service_tier=\"fast\"".to_string(),
            "service_tier=\"default\"".to_string(),
        ];

        let (start_method, start_params) = build_thread_request(&request, None);
        let (resume_method, resume_params) =
            build_thread_request(&request, Some("thread-fast-mode"));
        let turn_params =
            build_turn_start_request(&request, "thread-fast-mode", "hello".to_string(), "msg-1");

        assert_eq!(start_method, "thread/start");
        assert_eq!(resume_method, "thread/resume");
        assert_eq!(start_params["serviceTier"], "default");
        assert_eq!(resume_params["serviceTier"], "default");
        assert_eq!(turn_params["serviceTier"], "default");
    }

    #[test]
    fn thread_fork_is_precise_for_codex_and_omits_turn_for_traex() {
        let mut codex = request(DEFAULT_ADAPTER);
        codex.operation = "fork".to_string();
        codex.params = serde_json::json!({
            "threadId": "thread-source",
            "lastTurnId": "turn-source",
        });
        let (method, params) = build_thread_request(&codex, Some("thread-source"));
        assert_eq!(method, "thread/fork");
        assert_eq!(params["threadId"], "thread-source");
        assert_eq!(params["lastTurnId"], "turn-source");

        let mut traex = codex;
        traex.adapter = TRAEX_ADAPTER.to_string();
        let (method, params) = build_thread_request(&traex, Some("thread-source"));
        assert_eq!(method, "thread/fork");
        assert!(params.get("lastTurnId").is_none());
    }

    #[test]
    fn explicit_exec_and_custom_args_keep_exec_transport() {
        let mut explicit = request(DEFAULT_ADAPTER);
        explicit.adapter_config.transport = Some(ExternalCliTransport::Exec);
        assert_eq!(
            resolved_transport(&explicit).unwrap(),
            ExternalCliTransport::Exec
        );

        let mut custom = request(DEFAULT_ADAPTER);
        custom.adapter_config.args = vec!["exec".to_string(), "--json".to_string()];
        assert_eq!(
            resolved_transport(&custom).unwrap(),
            ExternalCliTransport::Exec
        );

        let mut custom_executable = request(DEFAULT_ADAPTER);
        custom_executable.adapter_config.executable = Some("/tmp/mock-codex".to_string());
        assert_eq!(
            resolved_transport(&custom_executable).unwrap(),
            ExternalCliTransport::Exec
        );

        let mut custom_claude = request(CLAUDE_CODE_ADAPTER);
        custom_claude.adapter_config.args = vec![
            "-p".to_string(),
            "--input-format".to_string(),
            "text".to_string(),
        ];
        assert_eq!(
            resolved_transport(&custom_claude).unwrap(),
            ExternalCliTransport::Exec
        );
    }

    #[test]
    fn explicit_app_server_rejects_unsupported_adapter_and_custom_args() {
        let mut unsupported = request(CLAUDE_CODE_ADAPTER);
        unsupported.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        assert!(resolved_transport(&unsupported)
            .unwrap_err()
            .contains("does not support app_server"));

        let mut custom = request(DEFAULT_ADAPTER);
        custom.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        custom.adapter_config.args = vec!["exec".to_string()];
        assert!(resolved_transport(&custom)
            .unwrap_err()
            .contains("adapterConfig.args"));
    }

    #[test]
    fn explicit_stream_json_validates_adapter_and_custom_args() {
        let mut claude = request(CLAUDE_CODE_ADAPTER);
        claude.adapter_config.transport = Some(ExternalCliTransport::StreamJson);
        assert_eq!(
            resolved_transport(&claude).unwrap(),
            ExternalCliTransport::StreamJson
        );

        let mut unsupported = request(DEFAULT_ADAPTER);
        unsupported.adapter_config.transport = Some(ExternalCliTransport::StreamJson);
        assert!(resolved_transport(&unsupported)
            .unwrap_err()
            .contains("does not support stream_json"));

        claude.adapter_config.args = vec!["--custom".to_string()];
        assert!(resolved_transport(&claude)
            .unwrap_err()
            .contains("adapterConfig.args"));
    }

    #[test]
    fn turn_steer_request_contains_active_turn_precondition_and_client_id() {
        let value = build_turn_steer_request("thread-1", "turn-1", "guide-1", "focus tests");
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["expectedTurnId"], "turn-1");
        assert_eq!(value["clientUserMessageId"], "guide-1");
        assert_eq!(value["input"][0]["text"], "focus tests");
    }

    #[test]
    fn app_server_agent_message_and_command_notifications_map_to_progress() {
        let message = serde_json::json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {"id":"item-1","type":"agentMessage","text":"done"}
            }
        });
        let message_event = progress_event_from_app_server_frame(&message).unwrap();
        assert_eq!(
            message_event.event_type,
            ExternalCliProgressEventType::AssistantFinal
        );
        assert_eq!(message_event.content, "done");

        let command = serde_json::json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "id":"item-2",
                    "type":"commandExecution",
                    "command":"pwd",
                    "aggregatedOutput":"/tmp\n",
                    "exitCode":0,
                    "durationMs":12
                }
            }
        });
        let command_event = progress_event_from_app_server_frame(&command).unwrap();
        assert_eq!(
            command_event.event_type,
            ExternalCliProgressEventType::ToolFinished
        );
        assert_eq!(command_event.title.as_deref(), Some("exec_command"));
        assert_eq!(command_event.raw["success"], true);
        assert_eq!(command_event.raw["durationMs"], 12);
    }

    #[test]
    fn app_server_scope_requires_the_active_root_thread_and_turn() {
        assert!(!app_server_frame_belongs_to_active_turn(
            &serde_json::json!({}),
            "root-thread",
            "root-turn"
        ));

        let root_item = serde_json::json!({
            "method": "item/completed",
            "params": {"threadId": "root-thread", "turnId": "root-turn", "item": {}}
        });
        assert!(app_server_frame_belongs_to_active_turn(
            &root_item,
            "root-thread",
            "root-turn"
        ));

        let child_item = serde_json::json!({
            "method": "item/completed",
            "params": {"threadId": "child-thread", "turnId": "child-turn", "item": {}}
        });
        assert!(!app_server_frame_belongs_to_active_turn(
            &child_item,
            "root-thread",
            "root-turn"
        ));

        let stale_root_turn = serde_json::json!({
            "method": "turn/completed",
            "params": {"threadId": "root-thread", "turn": {"id": "old-turn"}}
        });
        assert!(!app_server_frame_belongs_to_active_turn(
            &stale_root_turn,
            "root-thread",
            "root-turn"
        ));

        let missing_turn = serde_json::json!({
            "method": "turn/completed",
            "params": {"threadId": "root-thread", "turn": {"status": "completed"}}
        });
        assert!(!app_server_frame_belongs_to_active_turn(
            &missing_turn,
            "root-thread",
            "root-turn"
        ));

        let account_event = serde_json::json!({
            "method": "account/rateLimits/updated",
            "params": {"rateLimits": {}}
        });
        assert!(app_server_frame_belongs_to_active_turn(
            &account_event,
            "root-thread",
            "root-turn"
        ));
        assert!(app_server_frame_belongs_to_root_thread(
            &account_event,
            "root-thread"
        ));

        let missing_params = serde_json::json!({"method": "item/completed"});
        assert!(!app_server_frame_belongs_to_root_thread(
            &missing_params,
            "root-thread"
        ));

        let nested_root_thread = serde_json::json!({
            "method": "item/completed",
            "params": {"thread": {"id": "root-thread"}, "item": {}}
        });
        assert!(app_server_frame_belongs_to_root_thread(
            &nested_root_thread,
            "root-thread"
        ));
        assert!(app_server_frame_belongs_to_active_turn(
            &nested_root_thread,
            "root-thread",
            "root-turn"
        ));
    }

    #[test]
    fn app_server_collaboration_notifications_are_plain_tool_events() {
        let started = serde_json::json!({
            "method": "item/started",
            "params": {
                "threadId": "root-thread",
                "turnId": "turn-1",
                "item": {
                    "id": "collab-1",
                    "type": "collabAgentToolCall",
                    "tool": "spawnAgent",
                    "status": "inProgress",
                    "prompt": "Inspect the API boundary",
                    "arguments": {
                        "prompt": "Inspect the API boundary",
                        "receiverThreadIds": ["agent-hidden"],
                        "agentsStates": {"agent-hidden": {"status": "starting"}}
                    },
                    "senderThreadId": "root-thread",
                    "receiverThreadIds": [],
                    "agentsStates": {}
                }
            }
        });
        let started_event = progress_event_from_app_server_frame(&started).unwrap();
        assert_eq!(
            started_event.event_type,
            ExternalCliProgressEventType::ToolStarted
        );
        assert_eq!(started_event.title.as_deref(), Some("spawnAgent"));
        assert_eq!(
            started_event.raw["arguments"]["prompt"],
            "Inspect the API boundary"
        );
        assert!(started_event.raw["arguments"]
            .get("receiverThreadIds")
            .is_none());
        assert!(started_event.raw["arguments"].get("agentsStates").is_none());
        assert!(!started_event.raw.to_string().contains("agent-hidden"));

        let updated = serde_json::json!({
            "method": "item/updated",
            "params": {
                "threadId": "root-thread",
                "turnId": "turn-1",
                "item": {
                    "id": "collab-1",
                    "type": "collabAgentToolCall",
                    "tool": "wait",
                    "status": "inProgress",
                    "prompt": null,
                    "senderThreadId": "root-thread",
                    "receiverThreadIds": ["agent-1"],
                    "agentsStates": {
                        "agent-1": {"status": "running", "message": "Reading handlers"}
                    }
                }
            }
        });
        assert!(progress_event_from_app_server_frame(&updated).is_none());

        let activity = serde_json::json!({
            "method": "item/completed",
            "params": {
                "threadId": "root-thread",
                "turnId": "turn-1",
                "item": {
                    "id": "activity-1",
                    "type": "subAgentActivity",
                    "agentThreadId": "agent-1",
                    "kind": "interrupted"
                }
            }
        });
        assert!(progress_event_from_app_server_frame(&activity).is_none());

        let ordinary_tool_update = serde_json::json!({
            "method": "item/updated",
            "params": {
                "threadId": "root-thread",
                "turnId": "turn-1",
                "item": {
                    "id": "mcp-1",
                    "type": "mcpToolCall",
                    "tool": "lookup",
                    "arguments": {"query": "root-only"}
                }
            }
        });
        assert_eq!(
            progress_event_from_app_server_frame(&ordinary_tool_update)
                .expect("ordinary tool updates must keep their existing behavior")
                .event_type,
            ExternalCliProgressEventType::ToolStarted
        );

        let multi = serde_json::json!({
            "method": "item/completed",
            "params": {
                "threadId": "root-thread",
                "turnId": "turn-1",
                "item": {
                    "id": "wait-2",
                    "type": "collabAgentToolCall",
                    "tool": "wait",
                    "status": "completed",
                    "prompt": null,
                    "result": "Wait completed",
                    "senderThreadId": "root-thread",
                    "receiverThreadIds": ["agent-1", "agent-2"],
                    "agentsStates": {
                        "agent-1": {"status": "completed", "message": "Done"},
                        "agent-2": {"status": "running", "message": "Still working"}
                    }
                }
            }
        });
        let multi_events = progress_events_from_app_server_frame(&multi);
        assert_eq!(multi_events.len(), 1);
        assert_eq!(
            multi_events[0].event_type,
            ExternalCliProgressEventType::ToolFinished
        );
        assert_eq!(multi_events[0].content, "Wait completed");
        assert!(!multi_events[0].content.contains("Still working"));
        assert!(!multi_events[0].raw.to_string().contains("agent-1"));
        assert!(!multi_events[0].raw.to_string().contains("Still working"));
    }

    #[test]
    fn app_server_rate_limit_notification_keeps_only_weekly_display_fields() {
        let notification = serde_json::json!({
            "method": "account/rateLimits/updated",
            "params": {
                "rateLimits": {
                    "limitId": "codex",
                    "credits": {"hasCredits": true, "balance": "private"},
                    "primary": {
                        "usedPercent": 64,
                        "windowDurationMins": 10080,
                        "resetsAt": 1784490086
                    },
                    "secondary": {
                        "usedPercent": 5,
                        "windowDurationMins": 300,
                        "resetsAt": 1784000000
                    }
                }
            }
        });

        let event = progress_event_from_app_server_frame(&notification).expect("rate limit event");

        assert_eq!(event.title.as_deref(), Some("rate_limits"));
        assert_eq!(event.raw["weekly"]["usedPercent"], 64);
        assert_eq!(event.raw["weekly"]["windowDurationMins"], 10_080);
        assert_eq!(event.raw["weekly"]["resetsAt"], 1_784_490_086u64);
        assert!(event.raw.get("rateLimits").is_none());
        assert!(!event.raw.to_string().contains("private"));
    }

    #[test]
    fn app_server_file_change_notification_includes_paths_and_line_stats() {
        let file_change = serde_json::json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "id":"item-3",
                    "type":"fileChange",
                    "status":"completed",
                    "changes":[{
                        "path":"/workspace/src/main.rs",
                        "kind":{"type":"update","move_path":null},
                        "diff":"@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n context\n"
                    }]
                }
            }
        });

        let event = progress_event_from_app_server_frame(&file_change).unwrap();
        assert_eq!(event.event_type, ExternalCliProgressEventType::ToolFinished);
        assert_eq!(event.title.as_deref(), Some("fileChange"));
        assert!(event.content.contains("/workspace/src/main.rs"));
        assert!(event.content.contains("修改 1 行"));
        assert!(event.content.contains("新增 1 行"));
        assert!(!event.content.contains("暂无工具详情"));

        let agent_event = external_progress_to_agent_turn_event(
            "session-1",
            DEFAULT_ADAPTER,
            ExternalCliProgressStatusContext::new(
                Some("Codex"),
                None,
                None,
                None,
                None,
                Some(std::path::Path::new("/workspace")),
            ),
            &event,
        )
        .expect("tool event");
        let bifrost_agent::AgentTurnProgressEvent::ToolFinished { log, .. } = agent_event else {
            panic!("expected tool finished event");
        };
        assert_eq!(log.tool_name, "文件变更");
        assert!(log.result.contains("src/main.rs"));
        assert!(!log.result.contains("/workspace/src/main.rs"));
        assert!(log.result.contains("修改 1 行"));
        assert!(log.result.contains("新增 1 行"));
    }

    #[test]
    fn rejected_and_accepted_guide_responses_preserve_ids() {
        let accepted = guide_result_from_response(
            "guide-1".to_string(),
            "thread-1",
            "turn-1",
            &serde_json::json!({"id":101,"result":{"turnId":"turn-1"}}),
        );
        assert!(accepted.accepted);
        assert_eq!(accepted.guide_id, "guide-1");
        assert_eq!(accepted.turn_id.as_deref(), Some("turn-1"));

        let rejected = guide_result_from_response(
            "guide-2".to_string(),
            "thread-1",
            "turn-1",
            &serde_json::json!({"id":102,"error":{"code":-32600,"message":"no active turn to steer"}}),
        );
        assert!(!rejected.accepted);
        assert_eq!(rejected.reason.as_deref(), Some("no active turn to steer"));
    }

    #[test]
    fn capacity_retry_classification_is_strict_and_side_effect_aware() {
        let overloaded = serde_json::json!({
            "method": "error",
            "params": {
                "error": {
                    "message": "Selected model is at capacity.",
                    "codexErrorInfo": "serverOverloaded"
                },
                "willRetry": false
            }
        });
        assert!(is_capacity_error_frame(&overloaded));
        let mut internally_retried = overloaded.clone();
        internally_retried["params"]["willRetry"] = serde_json::json!(true);
        assert!(!is_capacity_error_frame(&internally_retried));
        assert!(should_retry_capacity_error(&overloaded, false, false, 0));
        assert!(!should_retry_capacity_error(&overloaded, true, false, 0));
        assert!(!should_retry_capacity_error(&overloaded, false, true, 0));
        assert!(!should_retry_capacity_error(
            &overloaded,
            false,
            false,
            CAPACITY_MAX_RETRIES
        ));

        let ordinary_error = serde_json::json!({
            "method": "error",
            "params": {"error": {"message": "invalid request", "codexErrorInfo": "other"}}
        });
        assert!(!is_capacity_error_frame(&ordinary_error));
        assert!(!is_capacity_error_frame(&serde_json::json!({
            "method": "warning",
            "params": {"error": {"codexErrorInfo": "serverOverloaded"}}
        })));

        let assistant = ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::AssistantDelta,
            content: "partial".to_string(),
            title: None,
            raw: serde_json::json!({}),
        };
        let status = ExternalCliProgressEvent {
            event_type: ExternalCliProgressEventType::Status,
            content: "turn started".to_string(),
            title: None,
            raw: serde_json::json!({}),
        };
        assert!(progress_event_has_retry_side_effect(&assistant));
        assert!(!progress_event_has_retry_side_effect(&status));
        assert_eq!(capacity_retry_delay(1), Duration::from_millis(10));
    }

    #[test]
    fn stale_app_server_cleanup_preserves_replacement_session_owner() {
        let session_key = format!("session-replacement-{}", uuid::Uuid::new_v4());
        let old_run_id = format!("run-old-{}", uuid::Uuid::new_v4());
        let new_run_id = format!("run-new-{}", uuid::Uuid::new_v4());
        let (old_guide_tx, _old_guide_rx) = mpsc::unbounded_channel();
        let (new_guide_tx, _new_guide_rx) = mpsc::unbounded_channel();

        ACTIVE_SESSIONS.insert(session_key.clone(), old_run_id.clone());
        assert!(register_active_app_server_session(
            &session_key,
            &old_run_id,
            live_guide::ActiveGuideHandle {
                run_id: old_run_id.clone(),
                thread_id: Some("thread-old".to_string()),
                turn_id: Some("turn-old".to_string()),
                guide_tx: old_guide_tx,
            },
        ));

        ACTIVE_SESSIONS.insert(session_key.clone(), new_run_id.clone());
        assert!(register_active_app_server_session(
            &session_key,
            &new_run_id,
            live_guide::ActiveGuideHandle {
                run_id: new_run_id.clone(),
                thread_id: Some("thread-new".to_string()),
                turn_id: Some("turn-new".to_string()),
                guide_tx: new_guide_tx,
            },
        ));

        remove_active_app_server_session(&session_key, &old_run_id);
        let active = live_guide::active_handle(&session_key)
            .expect("replacement app-server handle must remain registered");
        assert_eq!(active.run_id, new_run_id);
        assert_eq!(active.thread_id.as_deref(), Some("thread-new"));
        assert_eq!(active.turn_id.as_deref(), Some("turn-new"));

        remove_active_app_server_session(&session_key, &active.run_id);
        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[test]
    fn app_server_registration_rejects_stale_run_ownership() {
        let session_key = format!("session-stale-{}", uuid::Uuid::new_v4());
        let current_run_id = format!("run-current-{}", uuid::Uuid::new_v4());
        let stale_run_id = format!("run-stale-{}", uuid::Uuid::new_v4());
        let (guide_tx, _guide_rx) = mpsc::unbounded_channel();
        ACTIVE_SESSIONS.insert(session_key.clone(), current_run_id);

        assert!(!register_active_app_server_session(
            &session_key,
            &stale_run_id,
            live_guide::ActiveGuideHandle {
                run_id: stale_run_id.clone(),
                thread_id: Some("thread-stale".to_string()),
                turn_id: Some("turn-stale".to_string()),
                guide_tx,
            },
        ));
        assert!(live_guide::active_handle(&session_key).is_none());

        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn app_server_spawn_retries_linux_text_file_busy() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let executable = temp_dir.path().join("mock-busy-app-server");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write mock executable");
        let mut permissions = std::fs::metadata(&executable)
            .expect("mock executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).expect("chmod mock executable");

        let writable_handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&executable)
            .expect("hold mock executable open for writing");
        let release_handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(writable_handle);
        });
        let spec = CommandSpec {
            executable: executable.display().to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            work_dir: None,
            timeout_secs: None,
        };

        let mut child = spawn_app_server(&spec)
            .await
            .expect("ETXTBSY should clear within the bounded retry window");
        let status = child.wait().await.expect("wait for mock app-server");
        release_handle.join().expect("release writable handle");
        assert!(status.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_and_traex_ignore_child_turn_completion_until_root_turn_finishes() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-collaboration-app-server");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method in ("thread/start", "thread/resume"):
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":"root-thread"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"root-thread"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"child-thread","turnId":"child-turn","item":{"id":"early-child-message","type":"agentMessage","text":"EARLY_CHILD_FINAL_MUST_NOT_ESCAPE"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"child-thread","turn":{"id":"child-turn","status":"completed"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"root-turn"}}})
        send({"jsonrpc":"2.0","method":"item/started","params":{"threadId":"root-thread","turnId":"root-turn","item":{"id":"collab-1","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","prompt":"inspect the boundary","receiverThreadIds":[],"agentsStates":{}}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"child-thread","turnId":"child-turn","item":{"id":"child-message","type":"agentMessage","text":"CHILD_FINAL_MUST_NOT_ESCAPE"}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"child-thread","turnId":"child-turn","item":{"id":"child-tool","type":"commandExecution","command":"false","aggregatedOutput":"CHILD_TOOL_MUST_NOT_ESCAPE","exitCode":1}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"child-thread","turn":{"id":"child-turn","status":"completed"}}})
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "threadId": "root-thread",
                "turnId": "root-turn",
                "item": {
                    "id": "collab-1",
                    "type": "collabAgentToolCall",
                    "tool": "spawnAgent",
                    "status": "completed",
                    "prompt": "inspect the boundary",
                    "result": "child result received",
                    "receiverThreadIds": ["child-thread"],
                    "agentsStates": {
                        "child-thread": {
                            "status": "completed",
                            "message": "CHILD_INTERNAL_STATE_MUST_NOT_ESCAPE"
                        }
                    }
                }
            }
        })
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"root-thread","turnId":"root-turn","item":{"id":"root-message","type":"agentMessage","text":"ROOT_FINAL_OK"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"root-thread","turn":{"id":"root-turn","status":"completed"}}})
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        for adapter in [DEFAULT_ADAPTER, TRAEX_ADAPTER] {
            let mut request = request(adapter);
            request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
            request.adapter_config.executable = Some(executable.display().to_string());
            let run_id = format!("mock-{adapter}-subagent-boundary-run");
            let output = run_command(
                &run_id,
                None,
                &request,
                "delegate and finish".to_string(),
                temp_dir.path().join(format!("stop-{adapter}")),
                None,
            )
            .await
            .unwrap();

            assert_eq!(output.status, ExternalCliRunStatus::Succeeded, "{adapter}");
            assert_eq!(output.exit_code, Some(0), "{adapter}");
            assert!(output.events.iter().any(|event| {
                event.event_type == ExternalCliProgressEventType::AssistantFinal
                    && event.content == "ROOT_FINAL_OK"
            }));
            assert!(!output.events.iter().any(|event| {
                event.content.contains("CHILD_FINAL_MUST_NOT_ESCAPE")
                    || event.content.contains("EARLY_CHILD_FINAL_MUST_NOT_ESCAPE")
                    || event.content.contains("CHILD_TOOL_MUST_NOT_ESCAPE")
                    || event
                        .content
                        .contains("CHILD_INTERNAL_STATE_MUST_NOT_ESCAPE")
                    || event.event_type == ExternalCliProgressEventType::SubAgentUpdated
            }));
            assert_eq!(
                output
                    .events
                    .iter()
                    .filter(|event| event.event_type == ExternalCliProgressEventType::RunFinished)
                    .count(),
                1,
                "{adapter} must finish exactly once on the root turn"
            );
            assert_eq!(
                output
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event.event_type,
                            ExternalCliProgressEventType::ToolStarted
                                | ExternalCliProgressEventType::ToolFinished
                        ) && event.title.as_deref() == Some("spawnAgent")
                    })
                    .count(),
                2,
                "{adapter} collaboration must be one plain tool input/output pair"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_app_server_accepts_live_guide_and_completes_same_turn() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-codex");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/start":
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":"thread-mock"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"thread-mock"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"turn-mock"}}})
    elif method == "account/rateLimits/read":
        send({"jsonrpc":"2.0","id":request_id,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":63,"windowDurationMins":10080,"resetsAt":1784490086},"secondary":None}}})
        send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"method not found after snapshot"}})
    elif method == "turn/steer":
        assert frame["params"]["expectedTurnId"] == "turn-mock"
        assert frame["params"]["input"][0]["text"] == "focus on tests"
        send({"jsonrpc":"2.0","id":request_id,"result":{"turnId":"turn-mock"}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-mock","turnId":"turn-mock","item":{"id":"message-1","type":"agentMessage","text":"guided result"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-mock","turn":{"id":"turn-mock","status":"completed"}}})
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let mut request = request(DEFAULT_ADAPTER);
        request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        request.adapter_config.executable = Some(executable.display().to_string());
        let session_key = "mock-app-server-session";
        let (progress_tx, mut progress_rx) = mpsc::channel(EXTERNAL_CLI_PROGRESS_CHANNEL_CAPACITY);
        let run_task = tokio::spawn({
            let stop_marker = temp_dir.path().join("stop");
            async move {
                run_command(
                    "mock-app-server-run",
                    Some(session_key),
                    &request,
                    "initial prompt".to_string(),
                    stop_marker,
                    Some(progress_tx),
                )
                .await
            }
        });

        let guided = request_session_guide(
            session_key,
            "guide-mock".to_string(),
            "focus on tests".to_string(),
        )
        .await;
        if !guided.accepted {
            let run_result = timeout(Duration::from_secs(3), run_task)
                .await
                .expect("mock app-server run should exit after setup failure")
                .expect("join mock app-server run");
            panic!("guide rejected: {guided:?}; run={run_result:?}");
        }
        assert_eq!(guided.thread_id.as_deref(), Some("thread-mock"));
        assert_eq!(guided.turn_id.as_deref(), Some("turn-mock"));

        let output = run_task.await.unwrap().unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::Succeeded);
        assert!(output.events.iter().any(|event| {
            event.event_type == ExternalCliProgressEventType::RunStarted
                && event.raw["thread_id"] == "thread-mock"
        }));
        assert!(output.events.iter().any(|event| {
            event.event_type == ExternalCliProgressEventType::AssistantFinal
                && event.content == "guided result"
        }));
        assert!(output.events.iter().any(|event| {
            event.title.as_deref() == Some("rate_limits")
                && event.raw["weekly"]["windowDurationMins"] == 10_080
                && event.raw.get("rateLimits").is_none()
        }));
        let mut streamed_events = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            streamed_events.push(event);
        }
        assert!(streamed_events.iter().any(|event| {
            event.title.as_deref() == Some("rate_limits")
                && event.raw["weekly"]["windowDurationMins"] == 10_080
        }));
        assert!(!ACTIVE_RUNS.contains_key("mock-app-server-run"));
        assert!(!ACTIVE_SESSIONS.contains_key(session_key));
        assert!(live_guide::active_handle(session_key).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_app_server_stop_protocol_covers_ack_rejection_deadline_and_timeout() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-codex-stop-protocol");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

mode = os.environ["STOP_MODE"]
marker = os.environ["STOP_SEEN"]

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"stop-thread"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"stop-turn"}}})
    elif method == "account/rateLimits/read":
        send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
    elif method == "turn/interrupt":
        with open(marker, "w", encoding="utf-8") as handle:
            handle.write(str(request_id))
        if mode == "reject":
            time.sleep(0.2)
            send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32000,"message":"not active"}})
        elif mode == "ack":
            send({"jsonrpc":"2.0","id":request_id,"result":{}})
        elif mode == "ignore":
            pass
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        for mode in ["ack", "reject", "ignore"] {
            let session_key = format!("app-server-stop-{mode}");
            let stop_marker = temp_dir.path().join(format!("stop-{mode}"));
            let stop_seen = temp_dir.path().join(format!("seen-{mode}"));
            let mut request = request(DEFAULT_ADAPTER);
            request.session_key = Some(session_key.clone());
            request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
            request.adapter_config.executable = Some(executable.display().to_string());
            request.adapter_config.env = BTreeMap::from([
                ("STOP_MODE".to_string(), mode.to_string()),
                ("STOP_SEEN".to_string(), stop_seen.display().to_string()),
            ]);
            request.adapter_config.timeout_secs = Some(10);
            let stop_marker_for_run = stop_marker.clone();
            let run = tokio::spawn(async move {
                run_command(
                    &format!("app-server-stop-{mode}-run"),
                    Some(&session_key),
                    &request,
                    "wait".to_string(),
                    stop_marker_for_run,
                    None,
                )
                .await
            });
            timeout(Duration::from_secs(5), async {
                while live_guide::active_handle(&format!("app-server-stop-{mode}")).is_none() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("app-server active guide registration");
            tokio::fs::write(&stop_marker, b"stop").await.unwrap();
            timeout(Duration::from_secs(5), async {
                while !stop_seen.is_file() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("turn/interrupt should be written");

            if mode == "reject" {
                let rejected = request_session_guide(
                    &format!("app-server-stop-{mode}"),
                    "late-guide".to_string(),
                    "too late".to_string(),
                )
                .await;
                assert_eq!(
                    rejected.reason.as_deref(),
                    Some("app-server turn is stopping")
                );
            }
            let output = timeout(Duration::from_secs(8), run)
                .await
                .expect("stopped app-server should finish")
                .unwrap()
                .unwrap();
            assert_eq!(output.status, ExternalCliRunStatus::Stopped);
            let stderr = String::from_utf8_lossy(&output.stderr);
            match mode {
                "ack" => assert!(!stderr.contains("turn/interrupt"), "{stderr}"),
                "reject" => assert!(stderr.contains("rejected turn/interrupt"), "{stderr}"),
                "ignore" => assert!(stderr.contains("did not acknowledge"), "{stderr}"),
                _ => unreachable!(),
            }
        }

        let timeout_seen = temp_dir.path().join("timeout-seen");
        let mut request = request(DEFAULT_ADAPTER);
        request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        request.adapter_config.executable = Some(executable.display().to_string());
        request.adapter_config.env = BTreeMap::from([
            ("STOP_MODE".to_string(), "ignore".to_string()),
            ("STOP_SEEN".to_string(), timeout_seen.display().to_string()),
        ]);
        request.adapter_config.timeout_secs = Some(1);
        let output = run_command(
            "app-server-timeout-interrupt-run",
            None,
            &request,
            "wait".to_string(),
            temp_dir.path().join("unused-timeout-stop"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::TimedOut);
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("app-server turn timed out after 1 seconds"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_stop_and_timeout_report_closed_interrupt_stdin() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-codex-closed-interrupt");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

ready = os.environ["CLOSED_STDIN_READY"]
for line in sys.stdin:
    frame = json.loads(line)
    method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize":
        print(json.dumps({"jsonrpc":"2.0","id":request_id,"result":{}}), flush=True)
    elif method == "thread/start":
        print(json.dumps({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"closed-thread"}}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"closed-turn"}}}), flush=True)
    elif method == "account/rateLimits/read":
        print(json.dumps({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}}), flush=True)
        with open(ready, "w", encoding="utf-8") as handle:
            handle.write("ready")
        os.close(0)
        time.sleep(30)
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        for trigger in ["stop", "timeout"] {
            let ready = temp_dir.path().join(format!("ready-{trigger}"));
            let stop_marker = temp_dir.path().join(format!("stop-{trigger}"));
            let mut request = request(DEFAULT_ADAPTER);
            request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
            request.adapter_config.executable = Some(executable.display().to_string());
            request.adapter_config.env = BTreeMap::from([(
                "CLOSED_STDIN_READY".to_string(),
                ready.display().to_string(),
            )]);
            request.adapter_config.timeout_secs = (trigger == "timeout").then_some(1);
            let run_id = format!("app-server-closed-{trigger}-run");
            let stop_marker_for_run = stop_marker.clone();
            let run = tokio::spawn(async move {
                run_command(
                    &run_id,
                    None,
                    &request,
                    "wait".to_string(),
                    stop_marker_for_run,
                    None,
                )
                .await
            });
            timeout(Duration::from_secs(5), async {
                while !ready.is_file() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("mock should close stdin after handshake");
            if trigger == "stop" {
                tokio::fs::write(&stop_marker, b"stop").await.unwrap();
            }
            let output = timeout(Duration::from_secs(8), run)
                .await
                .expect("closed-stdin app-server should finish")
                .unwrap()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if trigger == "stop" {
                assert_eq!(output.status, ExternalCliRunStatus::Stopped);
                assert!(
                    stderr.contains("failed to send app-server turn/interrupt"),
                    "{stderr}"
                );
            } else {
                assert_eq!(output.status, ExternalCliRunStatus::TimedOut);
                assert!(stderr.contains("app-server turn timed out"), "{stderr}");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_app_server_forks_at_codex_turn_before_starting_new_turn() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-codex-fork");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, sys
def send(value): print(json.dumps(value, separators=(",", ":")), flush=True)
for line in sys.stdin:
    frame = json.loads(line)
    method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize": send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/fork":
        assert frame["params"] == {"threadId":"thread-source","lastTurnId":"turn-source"}
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"thread-derived"}}})
    elif method == "turn/start":
        assert frame["params"]["threadId"] == "thread-derived"
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"turn-derived"}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-derived","turnId":"turn-derived","item":{"id":"message","type":"agentMessage","text":"derived result"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-derived","turn":{"id":"turn-derived","status":"completed"}}})
    elif method == "account/rateLimits/read": send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
"#,
        ).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let mut request = request(DEFAULT_ADAPTER);
        request.operation = "fork".to_string();
        request.params = serde_json::json!({
            "threadId": "thread-source",
            "lastTurnId": "turn-source",
        });
        request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        request.adapter_config.executable = Some(executable.display().to_string());
        let output = run_command(
            "mock-codex-fork-run",
            None,
            &request,
            "continue".to_string(),
            temp_dir.path().join("stop"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::Succeeded);
        assert!(output
            .events
            .iter()
            .any(|event| event.content == "derived result"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_app_server_rejects_fork_response_without_derived_thread_id() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-codex-invalid-fork");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, sys
def send(value): print(json.dumps(value, separators=(",", ":")), flush=True)
for line in sys.stdin:
    frame = json.loads(line); method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize": send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/fork": send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "turn/start": raise RuntimeError("invalid fork must not start a turn")
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let mut request = request(DEFAULT_ADAPTER);
        request.operation = "fork".to_string();
        request.params = serde_json::json!({ "threadId": "thread-source" });
        request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        request.adapter_config.executable = Some(executable.display().to_string());
        let error = run_command(
            "mock-codex-invalid-fork-run",
            None,
            &request,
            "continue".to_string(),
            temp_dir.path().join("stop"),
            None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("thread/fork response missing derived thread.id"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_traex_checkpoint_fork_finishes_without_starting_turn() {
        use std::os::unix::fs::PermissionsExt;
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-traex-checkpoint");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, sys
def send(value): print(json.dumps(value, separators=(",", ":")), flush=True)
for line in sys.stdin:
    frame = json.loads(line); method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize": send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/fork":
        assert frame["params"] == {"threadId":"traex-source"}
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"traex-checkpoint"}}})
    elif method == "turn/start": raise RuntimeError("checkpoint must not start a turn")
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let mut request = request(TRAEX_ADAPTER);
        request.operation = "checkpoint_fork".to_string();
        request.params = serde_json::json!({ "threadId": "traex-source" });
        request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        request.adapter_config.executable = Some(executable.display().to_string());
        let output = run_command(
            "mock-traex-checkpoint-run",
            None,
            &request,
            String::new(),
            temp_dir.path().join("stop"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::Succeeded);
        assert!(output.events.iter().any(|event| {
            event.title.as_deref() == Some("thread_checkpoint")
                && event.raw["threadId"] == "traex-checkpoint"
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_app_server_retries_capacity_error_on_same_thread() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("codex");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import sys

turn_attempt = 0
client_user_message_id = None

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/start":
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":"thread-capacity"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"thread-capacity"}}})
    elif method == "turn/start":
        turn_attempt += 1
        turn_id = f"turn-{turn_attempt}"
        assert frame["params"]["threadId"] == "thread-capacity"
        if client_user_message_id is None:
            client_user_message_id = frame["params"]["clientUserMessageId"]
        else:
            assert frame["params"]["clientUserMessageId"] == client_user_message_id
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":turn_id}}})
        if turn_attempt == 1:
            send({"jsonrpc":"2.0","method":"error","params":{"threadId":"thread-capacity","turnId":turn_id,"error":{"message":"Selected model is at capacity. Please try a different model.","codexErrorInfo":"serverOverloaded","additionalDetails":None},"willRetry":False}})
        else:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-capacity","turnId":turn_id,"item":{"id":"message-1","type":"agentMessage","text":"recovered"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-capacity","turn":{"id":turn_id,"status":"completed"}}})
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let mut request = request(DEFAULT_ADAPTER);
        request.adapter_config.transport = Some(ExternalCliTransport::AppServer);
        request.adapter_config.executable = Some(executable.display().to_string());
        let session_key = "mock-capacity-retry-session";
        let output = run_command(
            "mock-capacity-retry-run",
            Some(session_key),
            &request,
            "retry me".to_string(),
            temp_dir.path().join("stop"),
            None,
        )
        .await
        .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::Succeeded);
        assert_eq!(output.exit_code, Some(0));
        assert!(output.events.iter().any(|event| {
            event.raw["type"] == "capacity_retry"
                && event.raw["retryAttempt"] == 1
                && event.raw["maxRetries"] == CAPACITY_MAX_RETRIES
        }));
        assert!(!output
            .events
            .iter()
            .any(|event| { event.event_type == ExternalCliProgressEventType::RunFailed }));
        assert!(output.events.iter().any(|event| {
            event.event_type == ExternalCliProgressEventType::AssistantFinal
                && event.content == "recovered"
        }));
        assert!(!ACTIVE_RUNS.contains_key("mock-capacity-retry-run"));
        assert!(!ACTIVE_SESSIONS.contains_key(session_key));
        assert!(live_guide::active_handle(session_key).is_none());
    }
}
