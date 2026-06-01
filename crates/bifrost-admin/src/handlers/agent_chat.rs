use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};

use super::{error_response, method_not_allowed, BoxBody};
use crate::state::SharedAdminState;

const MAX_AGENT_IMAGES_PER_MESSAGE: usize = 6;

#[derive(Debug, Deserialize)]
struct AgentChatRequest {
    message: String,
    #[serde(default)]
    images: Vec<AgentChatImageRequest>,
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    work_dir: Option<String>,
    #[serde(default)]
    history_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentChatImageRequest {
    #[serde(default = "default_chat_image_mime_type")]
    mime_type: String,
    /// Base64 image bytes or a data URL.
    data: String,
}

fn default_chat_image_mime_type() -> String {
    "image/png".to_string()
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

pub async fn handle_agent_chat(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let suffix = path.strip_prefix("/api/agent/chat").unwrap_or("");
    match (req.method(), suffix.trim_end_matches('/')) {
        (&Method::POST, "") => {
            let Some(service) = state.im_gateway_service() else {
                return error_response(StatusCode::SERVICE_UNAVAILABLE, "Agent is not configured");
            };
            crate::handlers::im_gateway::handle_im_gateway(
                req,
                Some(service),
                "/api/im-gateway/agent/chat",
            )
            .await
        }
        (&Method::POST, "/stream") => handle_stream(req, state).await,
        (&Method::POST, _) => {
            error_response(StatusCode::NOT_FOUND, "Agent chat endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

async fn handle_stream(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let body: AgentChatRequest = match read_body_json(req).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if body.message.trim().is_empty() && body.images.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "message must not be empty");
    }

    let Some(service) = state.im_gateway_service() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Agent is not configured");
    };
    let config = service.agent_config_store.load();
    if !config.enabled {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Agent is disabled");
    }

    let session_key = body
        .session_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "admin-chat".to_string());

    // Keep session-free commands synchronous enough to avoid taking the session.
    if let Some(response) =
        handle_session_free_stream_command(&body, &session_key, &service, &config).await
    {
        return sse_response(move |tx| async move {
            let _ = send_sse_event(&tx, "run_finished", response).await;
        });
    }

    let mut session = match service
        .agent_session_manager
        .try_take_session_with_work_dir(&session_key, body.work_dir.clone())
    {
        Some(session) => session,
        None => {
            return sse_response(move |tx| async move {
                let payload =
                    handle_builtin_busy_stream_input(&service, &session_key, body.message.trim());
                let _ = send_sse_event(&tx, "run_finished", payload).await;
            });
        }
    };
    session.source = "web".to_string();
    session.mark_bifrost_agent_runtime();
    let is_manual_compaction = body.message.trim() == "/compact";
    session.guide_channel = Some(
        service
            .queue_manager
            .get_or_create_guide_channel(&session_key),
    );
    if let Some(history_path) = body
        .history_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if session.history.is_empty() {
            if let Err(error) = restore_session_from_history_path(
                &mut session,
                history_path,
                &session_key,
                config
                    .history
                    .as_ref()
                    .and_then(|history| history.max_bytes),
            ) {
                service.agent_session_manager.return_session(session);
                return error_response(StatusCode::BAD_REQUEST, &error);
            }
        }
    }
    service.agent_session_manager.update_active_session_preview(
        &session_key,
        if is_manual_compaction {
            session.title.clone()
        } else {
            first_message_title_preview(&body.message)
        },
        session.work_dir.clone(),
        Some(session.source.clone()),
        session.runner_type.clone(),
        session.runner_id.clone(),
    );
    service
        .progress_registry
        .restart_existing(
            &session_key,
            if is_manual_compaction {
                "上下文正在自动压缩"
            } else {
                body.message.trim()
            },
        )
        .await;

    let (progress_tx, progress_rx) = mpsc::unbounded_channel();
    session.progress_sender = Some(progress_tx);

    sse_response(move |tx| async move {
        run_agent_stream(service, config, session_key, session, body, progress_rx, tx).await;
    })
}

fn restore_session_from_history_path(
    session: &mut bifrost_agent::AgentSession,
    history_path: &str,
    expected_session_key: &str,
    max_bytes: Option<usize>,
) -> Result<(), String> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let path =
        bifrost_agent::persistence::validate_conversation_path(&data_dir, Path::new(history_path))?;
    let report = bifrost_agent::persistence::load_conversation_lossy(&path)?;
    if let Some(restored_key) = report.session_key.as_deref() {
        if restored_key != expected_session_key {
            return Err("history session_key does not match the requested session_key".to_string());
        }
    }
    if report.messages.is_empty() {
        return Err("history file does not contain restorable chat messages".to_string());
    }

    let summary = bifrost_agent::persistence::scan_session_summary(&path);
    session.history = report.messages;
    session.history_version = session.history_version.saturating_add(1);
    session.last_response_tokens = None;
    session.last_response_history_len = None;
    session.memory_cleared = false;
    session.title = summary.title;
    if session.work_dir.is_none() {
        session.work_dir = summary.work_dir;
    }
    if !summary.source.is_empty() {
        session.source = summary.source;
    }
    match bifrost_agent::persistence::load_session_runtime_state(&path) {
        Ok(runtime_state) => {
            session.current_goal = runtime_state.current_goal;
            session.current_plan = runtime_state.current_plan;
            session.total_tokens_used = runtime_state.total_tokens_used;
            session.restore_token_snapshot(runtime_state.last_response_tokens);
            session.compaction_count = runtime_state.compaction_count;
            session.resolved_base_instructions = runtime_state.base_instructions;
            bifrost_agent::tools::goal::goal_runtime_apply(
                session,
                bifrost_agent::tools::goal::GoalRuntimeEvent::ThreadResumed,
            );
        }
        Err(error) => {
            warn!(history_path = %path.display(), error = %error, "restored chat history without runtime state");
            session.total_tokens_used = Some(summary.total_tokens);
            session.compaction_count = summary.compaction_count;
        }
    }
    if report.skipped_lines > 0 {
        warn!(
            history_path = %path.display(),
            skipped_lines = report.skipped_lines,
            "restored chat history with skipped JSONL lines"
        );
    }
    session.recorder =
        Some(bifrost_agent::persistence::ConversationRecorder::from_existing_file(path, max_bytes));
    Ok(())
}

async fn run_agent_stream(
    service: crate::handlers::im_gateway::SharedImGatewayService,
    config: bifrost_agent::AgentConfig,
    session_key: String,
    mut session: bifrost_agent::AgentSession,
    body: AgentChatRequest,
    mut progress_rx: mpsc::UnboundedReceiver<bifrost_agent::AgentTurnProgressEvent>,
    tx: mpsc::Sender<Result<hyper::body::Frame<Bytes>, hyper::Error>>,
) {
    if !send_sse_event(
        &tx,
        "run_started",
        json!({
            "eventType": "run_started",
            "sessionKey": session_key,
        }),
    )
    .await
    {
        service.agent_session_manager.return_session(session);
        return;
    }
    let initial_context = bifrost_agent::snapshot_agent_context(&session, &config);
    if !send_sse_event(
        &tx,
        "context_updated",
        json!({
            "eventType": "context_updated",
            "sessionKey": session_key,
            "context": initial_context,
        }),
    )
    .await
    {
        service.agent_session_manager.return_session(session);
        return;
    }

    let mut worker_request = crate::im_gateway::agent_worker::build_run_request(
        session_key.clone(),
        body.message.clone(),
        normalize_images(&body.images),
        &config,
        session.work_dir.clone(),
        body.history_path.clone(),
        Some("web".to_string()),
    );
    worker_request.system_prompt = body.system_prompt.clone();
    let mut worker =
        crate::im_gateway::agent_worker::AgentWorkerClient::spawn_or_fallback(worker_request).await;
    let (stop_tx, mut stop_rx) = mpsc::unbounded_channel::<()>();
    let worker_pid = worker.child_id().unwrap_or(0);
    crate::im_gateway::agent_worker::register_active_worker(&session_key, worker_pid, stop_tx);

    let mut progress_closed = false;
    loop {
        tokio::select! {
            _ = stop_rx.recv() => {
                let _ = worker.terminate().await;
                crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                service.agent_session_manager.return_session(session);
                let payload = json!({
                    "eventType": "run_finished",
                    "sessionKey": session_key,
                    "response": "已收到 /stop，Agent worker 子进程已停止。",
                    "stopped": true,
                });
                let _ = send_sse_event(&tx, "run_finished", payload).await;
                return;
            }
            _ = tx.closed() => {
                info!(
                    session_key = %session_key,
                    "agent chat stream client disconnected; stopping isolated worker"
                );
                let _ = worker.terminate().await;
                crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                service.agent_session_manager.return_session(session);
                return;
            }
            maybe_event = progress_rx.recv(), if !progress_closed => {
                match maybe_event {
                    Some(event) => {
                        apply_worker_progress_event(&service, &session_key, &event).await;
                        let (event_name, payload) = progress_event_payload(&session_key, event);
                        if !send_sse_event(&tx, event_name, payload).await {
                            info!(
                                session_key = %session_key,
                                "agent chat stream receiver closed while sending progress; stopping isolated worker"
                            );
                            let _ = worker.terminate().await;
                            crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                            service.agent_session_manager.return_session(session);
                            return;
                        }
                    }
                    None => {
                        progress_closed = true;
                    }
                }
            }
            event = worker.next_event() => {
                while let Ok(event) = progress_rx.try_recv() {
                    apply_worker_progress_event(&service, &session_key, &event).await;
                    let (event_name, payload) = progress_event_payload(&session_key, event);
                    if !send_sse_event(&tx, event_name, payload).await {
                        info!(
                            session_key = %session_key,
                            "agent chat stream receiver closed while flushing progress; stopping isolated worker"
                        );
                        let _ = worker.terminate().await;
                        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                        service.agent_session_manager.return_session(session);
                        return;
                    }
                }
                match event {
                    Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Started { .. })) => {}
                    Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Progress { event })) => {
                        apply_worker_progress_event(&service, &session_key, &event).await;
                        let (event_name, payload) = progress_event_payload(&session_key, event);
                        if !send_sse_event(&tx, event_name, payload).await {
                            let _ = worker.terminate().await;
                            crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                            service.agent_session_manager.return_session(session);
                            return;
                        }
                    }
                    Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Finished { result: turn_result })) => {
                        info!(
                            session_key = %session_key,
                            response_len = turn_result.response.len(),
                            tool_call_count = turn_result.tool_calls_log.len(),
                            "agent chat stream completed"
                        );
                        let payload = json!({
                            "eventType": "run_finished",
                            "sessionKey": session_key,
                            "response": turn_result.response,
                            "toolCalls": turn_result.tool_calls_log,
                            "planSteps": turn_result.plan_steps,
                        });
                        let _ = send_sse_event(&tx, "run_finished", payload).await;
                        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                        refresh_session_from_worker_history(&mut session, &session_key, &turn_result.history_path, &config);
                        service.agent_session_manager.return_session(session);
                        return;
                    }
                    Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Failed { error })) => {
                        error!(session_key = %session_key, error = %error, "agent chat stream failed");
                        let payload = json!({
                            "eventType": "run_failed",
                            "sessionKey": session_key,
                            "error": error,
                        });
                        let _ = send_sse_event(&tx, "run_failed", payload).await;
                        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                        service.agent_session_manager.return_session(session);
                        return;
                    }
                    Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Stopped)) => {
                        let payload = json!({
                            "eventType": "run_finished",
                            "sessionKey": session_key,
                            "response": "已收到 /stop，Agent worker 子进程已停止。",
                            "stopped": true,
                        });
                        let _ = send_sse_event(&tx, "run_finished", payload).await;
                        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                        service.agent_session_manager.return_session(session);
                        return;
                    }
                    Ok(None) => {
                        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                        service.agent_session_manager.return_session(session);
                        return;
                    }
                    Err(error) => {
                        error!(session_key = %session_key, error = %error, "agent worker stream failed");
                        let payload = json!({
                            "eventType": "run_failed",
                            "sessionKey": session_key,
                            "error": format!("agent worker failed: {error}"),
                        });
                        let _ = send_sse_event(&tx, "run_failed", payload).await;
                        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
                        service.agent_session_manager.return_session(session);
                        return;
                    }
                }
            }
        }
    }
}

async fn apply_worker_progress_event(
    service: &crate::handlers::im_gateway::SharedImGatewayService,
    session_key: &str,
    event: &bifrost_agent::AgentTurnProgressEvent,
) {
    match event {
        bifrost_agent::AgentTurnProgressEvent::TitleUpdated { title } => {
            service.agent_session_manager.update_active_session_preview(
                session_key,
                Some(title.clone()),
                None,
                None,
                None,
                None,
            );
        }
        bifrost_agent::AgentTurnProgressEvent::Status(status) => {
            service
                .agent_session_manager
                .update_active_turn_status_from_worker((**status).clone());
        }
        _ => {}
    }
    service
        .progress_registry
        .apply_event(session_key, event.clone())
        .await;
}

fn refresh_session_from_worker_history(
    session: &mut bifrost_agent::AgentSession,
    session_key: &str,
    history_path: &Option<String>,
    config: &bifrost_agent::AgentConfig,
) {
    let Some(history_path) = history_path.as_deref() else {
        return;
    };
    if let Err(error) = restore_session_from_history_path(
        session,
        history_path,
        session_key,
        config
            .history
            .as_ref()
            .and_then(|history| history.max_bytes),
    ) {
        warn!(
            session_key = %session_key,
            history_path = %history_path,
            error = %error,
            "failed to refresh main-process session from isolated worker history"
        );
    }
}

fn handle_builtin_busy_stream_input(
    service: &crate::handlers::im_gateway::SharedImGatewayService,
    session_key: &str,
    message: &str,
) -> Value {
    let trimmed = message.trim();
    if let Some(rest) = trimmed.strip_prefix("/q ") {
        let queued = rest.trim();
        if queued.is_empty() {
            return json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": "用法: /q <排队消息>",
            });
        }
        return match service
            .queue_manager
            .push_queue(session_key, queued.to_string())
        {
            Ok(items) => json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": format!("✅ 消息已收到，将在当前任务完成后处理（排队 {} 条）", items.len()),
                "queued": true,
                "queueLength": items.len(),
                "queueItems": items,
            }),
            Err(error) => json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": format!("排队失败: {error}"),
            }),
        };
    }

    if let Some(rest) = trimmed.strip_prefix("/rq ") {
        let rest = rest.trim();
        return match rest.parse::<u64>() {
            Ok(seq) if service.queue_manager.remove_queue(session_key, seq) => {
                let items = service.queue_manager.queue_status(session_key);
                json!({
                    "eventType": "run_finished",
                    "sessionKey": session_key,
                    "response": format!("🗑️ 已删除排队消息 #{seq}"),
                    "queued": true,
                    "queueLength": items.len(),
                    "queueItems": items,
                })
            }
            Ok(seq) => json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": format!("❌ 未找到排队消息 #{seq}"),
            }),
            Err(_) => json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": "用法: /rq <序号>（如 /rq 1）",
            }),
        };
    }

    let guide_text = trimmed
        .strip_prefix("/g ")
        .map(str::trim)
        .unwrap_or(trimmed);
    if guide_text.is_empty() {
        return json!({
            "eventType": "run_finished",
            "sessionKey": session_key,
            "response": "消息内容不能为空",
        });
    }
    let pending_count = service
        .queue_manager
        .inject_guide(session_key, guide_text.to_string());
    json!({
        "eventType": "run_finished",
        "sessionKey": session_key,
        "response": if pending_count > 1 {
            format!("🔀 已追加引导消息（当前 {pending_count} 条尚未进入 loop，将合并后生效）")
        } else {
            "🔀 已注入引导消息，将在当前工具调用完成后生效".to_string()
        },
        "guide": true,
        "pendingGuideCount": pending_count,
    })
}

pub(crate) fn queue_snapshot_payload(
    queue_manager: &crate::im_gateway::SessionQueueManager,
    session_key: &str,
) -> Value {
    let items = queue_manager.queue_status(session_key);
    json!({
        "queueLength": items.len(),
        "queueItems": items,
    })
}

async fn handle_session_free_stream_command(
    body: &AgentChatRequest,
    session_key: &str,
    service: &crate::handlers::im_gateway::SharedImGatewayService,
    config: &bifrost_agent::AgentConfig,
) -> Option<Value> {
    let trimmed = body.message.trim();
    if trimmed == "/status" {
        if let Some(status) = service
            .agent_session_manager
            .get_active_turn_status(session_key)
        {
            return Some(json!({
                "eventType": "run_finished",
                "sessionKey": session_key,
                "response": bifrost_agent::format_active_turn_status_text(&status),
                "activeStatus": status,
            }));
        }
        let detail = service
            .agent_session_manager
            .get_session_detail(session_key);
        return Some(json!({
            "eventType": "run_finished",
            "sessionKey": session_key,
            "response": detail
                .as_ref()
                .map(format_session_detail_status)
                .unwrap_or_else(|| "会话不存在或尚未开始。".to_string()),
            "session": detail,
        }));
    }

    if trimmed == "/stop" {
        let internal_stopped = service.agent_session_manager.request_stop(session_key);
        let worker_stopped =
            crate::im_gateway::agent_worker::request_session_stop(session_key).await;
        let stopped = internal_stopped || worker_stopped;
        return Some(json!({
            "eventType": "run_finished",
            "sessionKey": session_key,
            "response": if stopped {
                "已请求停止当前 Agent loop。"
            } else {
                "当前没有正在执行的 Agent loop。"
            },
            "stopped": stopped,
        }));
    }

    bifrost_agent::handle_session_free_command(session_key, &body.message, config).map(|response| {
        json!({
            "eventType": "run_finished",
            "sessionKey": session_key,
            "response": response,
        })
    })
}

fn format_session_detail_status(detail: &bifrost_agent::SessionDetail) -> String {
    format!(
        "会话状态:\n- Session: {}\n- 消息数: {}\n- 用户轮次: {}\n- 工作目录: {}",
        detail.session_key,
        detail.message_count,
        detail.user_turn_count,
        detail.work_dir.as_deref().unwrap_or("N/A")
    )
}

fn normalize_images(images: &[AgentChatImageRequest]) -> Vec<bifrost_agent::ChatImageInput> {
    if images.len() > MAX_AGENT_IMAGES_PER_MESSAGE {
        warn!(
            image_count = images.len(),
            max_images = MAX_AGENT_IMAGES_PER_MESSAGE,
            "too many admin agent chat images in one request; truncating images"
        );
    }
    images
        .iter()
        .take(MAX_AGENT_IMAGES_PER_MESSAGE)
        .filter(|image| !image.data.trim().is_empty())
        .map(|image| bifrost_agent::ChatImageInput {
            mime_type: image.mime_type.clone(),
            data: image.data.clone(),
        })
        .collect()
}

fn progress_event_payload(
    session_key: &str,
    event: bifrost_agent::AgentTurnProgressEvent,
) -> (&'static str, Value) {
    match event {
        bifrost_agent::AgentTurnProgressEvent::Status(status) => (
            "status",
            json!({
                "eventType": "status",
                "sessionKey": session_key,
                "status": status,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::ContextUpdated { context } => (
            "context_updated",
            json!({
                "eventType": "context_updated",
                "sessionKey": session_key,
                "context": context,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::CompactionStarted { progress } => (
            "compaction_started",
            json!({
                "eventType": "compaction_started",
                "sessionKey": session_key,
                "compaction": progress,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::CompactionFinished { progress } => (
            "compaction_finished",
            json!({
                "eventType": "compaction_finished",
                "sessionKey": session_key,
                "compaction": progress,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::CompactionFailed { progress, error } => (
            "compaction_failed",
            json!({
                "eventType": "compaction_failed",
                "sessionKey": session_key,
                "compaction": progress,
                "error": error,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::ToolStarted {
            tool_name,
            arguments,
        } => (
            "tool_started",
            json!({
                "eventType": "tool_started",
                "sessionKey": session_key,
                "toolName": tool_name,
                "arguments": arguments,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::ToolFinished { log, duration_ms } => (
            "tool_finished",
            json!({
                "eventType": "tool_finished",
                "sessionKey": session_key,
                "log": log,
                "durationMs": duration_ms,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::LongTaskStatus {
            session_key: event_session_key,
            session_id,
            profile,
            state,
            elapsed_ms,
            last_output_preview,
            next_check_at_ms,
            unchanged_heartbeats,
        } => (
            "long_task_status",
            json!({
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
        ),
        bifrost_agent::AgentTurnProgressEvent::PlanUpdated { steps, title } => (
            "plan_updated",
            json!({
                "eventType": "plan_updated",
                "sessionKey": session_key,
                "steps": steps,
                "title": title,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::TitleUpdated { title } => (
            "title_updated",
            json!({
                "eventType": "title_updated",
                "sessionKey": session_key,
                "title": title,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::AssistantDelta { content } => (
            "assistant_delta",
            json!({
                "eventType": "assistant_delta",
                "sessionKey": session_key,
                "content": content,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::AssistantFinal { content } => (
            "assistant_final",
            json!({
                "eventType": "assistant_final",
                "sessionKey": session_key,
                "content": content,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::TurnFinished { content } => (
            "turn_finished",
            json!({
                "eventType": "turn_finished",
                "sessionKey": session_key,
                "content": content,
            }),
        ),
        bifrost_agent::AgentTurnProgressEvent::TurnFailed { error } => (
            "turn_failed",
            json!({
                "eventType": "turn_failed",
                "sessionKey": session_key,
                "error": error,
            }),
        ),
    }
}

fn format_sse_event(event_name: &str, payload: Value) -> String {
    let data = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    format!("event: {event_name}\ndata: {data}\n\n")
}

async fn send_sse_event(
    tx: &mpsc::Sender<Result<hyper::body::Frame<Bytes>, hyper::Error>>,
    event_name: &str,
    payload: Value,
) -> bool {
    tx.send(Ok(hyper::body::Frame::data(Bytes::from(format_sse_event(
        event_name, payload,
    )))))
    .await
    .is_ok()
}

fn sse_response<F, Fut>(run: F) -> Response<BoxBody>
where
    F: FnOnce(mpsc::Sender<Result<hyper::body::Frame<Bytes>, hyper::Error>>) -> Fut
        + Send
        + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<hyper::body::Frame<Bytes>, hyper::Error>>(32);
    tokio::spawn(run(tx));
    let body_stream = http_body_util::StreamBody::new(ReceiverStream::new(rx));
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

async fn read_body_json<T: for<'de> Deserialize<'de>>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = req
        .into_body()
        .collect()
        .await
        .map_err(|error| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        })?
        .to_bytes();
    serde_json::from_slice::<T>(&body)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::SessionQueueManager;

    #[test]
    fn formats_named_sse_event_with_json_payload() {
        let event = format_sse_event(
            "assistant_delta",
            json!({"eventType":"assistant_delta","content":"hello"}),
        );

        assert_eq!(
            event,
            "event: assistant_delta\ndata: {\"content\":\"hello\",\"eventType\":\"assistant_delta\"}\n\n"
        );
    }

    #[test]
    fn maps_progress_events_to_stable_event_names() {
        let (name, payload) = progress_event_payload(
            "s1",
            bifrost_agent::AgentTurnProgressEvent::AssistantDelta {
                content: "chunk".to_string(),
            },
        );

        assert_eq!(name, "assistant_delta");
        assert_eq!(payload["eventType"], "assistant_delta");
        assert_eq!(payload["sessionKey"], "s1");
        assert_eq!(payload["content"], "chunk");
    }

    #[test]
    fn maps_context_progress_event_to_context_updated_sse() {
        let context = bifrost_agent::AgentContextSnapshot {
            estimated_context_tokens: 123,
            context_window_tokens: Some(1000),
            context_usage_percent: Some(12.3),
            compaction_count: 2,
            history_version: 7,
            message_count: 9,
            user_turn_count: 4,
            last_response_tokens: Some(50),
            total_tokens_used: Some(500),
        };
        let (name, payload) = progress_event_payload(
            "s1",
            bifrost_agent::AgentTurnProgressEvent::ContextUpdated {
                context: context.clone(),
            },
        );

        assert_eq!(name, "context_updated");
        assert_eq!(payload["eventType"], "context_updated");
        assert_eq!(payload["sessionKey"], "s1");
        assert_eq!(payload["context"]["estimatedContextTokens"], 123);
        assert_eq!(payload["context"]["compactionCount"], 2);
    }

    #[test]
    fn maps_compaction_progress_events_to_named_sse() {
        let context = bifrost_agent::AgentContextSnapshot {
            estimated_context_tokens: 250,
            context_window_tokens: Some(1000),
            context_usage_percent: Some(25.0),
            compaction_count: 3,
            history_version: 11,
            message_count: 5,
            user_turn_count: 2,
            last_response_tokens: None,
            total_tokens_used: Some(900),
        };
        let progress = bifrost_agent::AgentCompactionProgress {
            trigger: "auto".to_string(),
            reason: "context_limit".to_string(),
            phase: "pre_turn".to_string(),
            pre_tokens: 900,
            post_tokens: Some(250),
            tokens_saved: Some(650),
            messages_removed: Some(8),
            duration_ms: Some(42),
            compaction_count: 3,
            history_version: 11,
            context,
        };
        let (name, payload) = progress_event_payload(
            "s1",
            bifrost_agent::AgentTurnProgressEvent::CompactionFinished { progress },
        );

        assert_eq!(name, "compaction_finished");
        assert_eq!(payload["eventType"], "compaction_finished");
        assert_eq!(payload["compaction"]["trigger"], "auto");
        assert_eq!(payload["compaction"]["tokensSaved"], 650);
        assert_eq!(
            payload["compaction"]["context"]["estimatedContextTokens"],
            250
        );
    }

    #[test]
    fn queue_snapshot_payload_exposes_backend_queue_items() {
        let queue_manager = SessionQueueManager::new();
        queue_manager
            .push_queue("web-session", "first queued".to_string())
            .unwrap();
        queue_manager
            .push_queue("web-session", "second queued".to_string())
            .unwrap();

        let payload = queue_snapshot_payload(&queue_manager, "web-session");

        assert_eq!(payload["queueLength"], 2);
        assert_eq!(payload["queueItems"][0]["seq"], 1);
        assert_eq!(payload["queueItems"][0]["message"], "first queued");
        assert_eq!(payload["queueItems"][1]["seq"], 2);
        assert_eq!(payload["queueItems"][1]["message"], "second queued");
    }
}
