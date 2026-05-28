use super::turn_loop::*;
use super::*;
use crate::config::{AgentConfig, ModelProviderConfig};
use crate::types::{ToolCallMessage, ToolRuntimeEvent};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

fn test_tool_call(id: &str) -> ToolCallMessage {
    ToolCallMessage::function_call(
        id.to_string(),
        "list_directory".to_string(),
        r#"{"path":"."}"#.to_string(),
    )
}

fn count_messages_with_content(messages: &[ChatMessage], content: &str) -> usize {
    messages
        .iter()
        .filter(|message| message.content.as_deref() == Some(content))
        .count()
}

#[test]
fn test_combine_guide_messages_preserves_order() {
    let combined = combine_guide_messages(vec![
        " 第一条 ".to_string(),
        "".to_string(),
        "第二条".to_string(),
    ])
    .unwrap();

    assert!(combined.contains("引导消息 1:\n第一条"));
    assert!(combined.contains("引导消息 2:\n第二条"));
    assert!(
        combined.find("第一条").unwrap() < combined.find("第二条").unwrap(),
        "guide message order should be preserved"
    );
}

#[test]
fn session_detail_uses_stable_title_fallback() {
    let manager = AgentSessionManager::new(3600);
    let mut session = AgentSession::new("detail-title");
    session.add_user_message("first visible user message");
    session.add_assistant_message("answer");
    manager.return_session(session);

    let detail = manager
        .get_session_detail("detail-title")
        .expect("session detail");
    assert_eq!(detail.title.as_deref(), Some("first visible user message"));
}

#[test]
fn session_detail_prefers_explicit_title_over_first_user_message() {
    let manager = AgentSessionManager::new(3600);
    let mut session = AgentSession::new("detail-explicit-title");
    session.add_user_message("first visible user message");
    session.title = Some("Bifrost Edge".to_string());
    manager.return_session(session);

    let detail = manager
        .get_session_detail("detail-explicit-title")
        .expect("session detail");
    assert_eq!(detail.title.as_deref(), Some("Bifrost Edge"));
}

#[test]
fn test_apply_plan_update_snapshot_replaces_completed_history() {
    use crate::tools::update_plan::{PlanStep, PlanStepStatus, UpdatePlanArgs};

    let mut session = AgentSession::new("plan-snapshot");
    session.current_plan = Some(vec![
        PlanStep {
            step: "实现后端".to_string(),
            status: PlanStepStatus::Completed,
        },
        PlanStep {
            step: "实现 WebUI".to_string(),
            status: PlanStepStatus::Completed,
        },
    ]);
    session.plan_repair_attempts = 2;

    let incoming = vec![PlanStep {
        step: "从头复核".to_string(),
        status: PlanStepStatus::InProgress,
    }];
    let mut recorder = None;
    apply_plan_update_snapshot(
        &mut session,
        &mut recorder,
        UpdatePlanArgs {
            explanation: Some("fresh task".to_string()),
            plan: incoming.clone(),
        },
    );

    assert_eq!(session.current_plan, Some(incoming));
    assert_eq!(session.plan_repair_attempts, 0);
}

#[test]
fn test_apply_plan_update_snapshot_allows_completed_step_to_reopen() {
    use crate::tools::update_plan::{PlanStep, PlanStepStatus, UpdatePlanArgs};

    let mut session = AgentSession::new("plan-reopen");
    session.current_plan = Some(vec![PlanStep {
        step: "目标清单".to_string(),
        status: PlanStepStatus::Completed,
    }]);
    let incoming = vec![
        PlanStep {
            step: "目标清单".to_string(),
            status: PlanStepStatus::Pending,
        },
        PlanStep {
            step: "重新审查 diff".to_string(),
            status: PlanStepStatus::InProgress,
        },
    ];
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plan_tx, mut plan_rx) = tokio::sync::mpsc::unbounded_channel();
    (session.progress_sender, session.plan_sender) = (Some(progress_tx), Some(plan_tx));

    let mut recorder = None;
    apply_plan_update_snapshot(
        &mut session,
        &mut recorder,
        UpdatePlanArgs {
            explanation: None,
            plan: incoming.clone(),
        },
    );

    assert_eq!(session.current_plan, Some(incoming.clone()));
    assert!(matches!(
        progress_rx.try_recv(),
        Ok(AgentTurnProgressEvent::PlanUpdated { steps, .. }) if steps == incoming
    ));
    assert_eq!(plan_rx.try_recv().expect("plan update event").0, incoming);
}

#[test]
fn test_apply_plan_update_empty_snapshot_clears_plan() {
    use crate::tools::update_plan::{PlanStep, PlanStepStatus, UpdatePlanArgs};

    let mut session = AgentSession::new("plan-empty-clear");
    session.current_plan = Some(vec![PlanStep {
        step: "旧任务".to_string(),
        status: PlanStepStatus::Completed,
    }]);
    session.plan_repair_attempts = 1;
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plan_tx, mut plan_rx) = tokio::sync::mpsc::unbounded_channel();
    (session.progress_sender, session.plan_sender) = (Some(progress_tx), Some(plan_tx));

    let dir = tempfile::tempdir().unwrap();
    let mut recorder = ConversationRecorder::new(dir.path(), "plan-empty-clear");
    let mut recorder_ref = Some(&mut recorder);
    apply_plan_update_snapshot(
        &mut session,
        &mut recorder_ref,
        UpdatePlanArgs {
            explanation: Some("current plan no longer applies".to_string()),
            plan: Vec::new(),
        },
    );
    recorder.close();

    assert!(session.current_plan.is_none());
    assert_eq!(session.plan_repair_attempts, 0);
    assert!(matches!(
        progress_rx.try_recv(),
        Ok(AgentTurnProgressEvent::PlanUpdated { steps, .. }) if steps.is_empty()
    ));
    assert!(plan_rx.try_recv().expect("plan clear event").0.is_empty());

    let state = persistence::load_session_runtime_state(recorder.file_path()).unwrap();
    assert!(state.current_plan.is_none());
}

#[test]
fn test_apply_plan_update_empty_snapshot_does_not_clear_unfinished_plan() {
    use crate::tools::update_plan::{PlanStep, PlanStepStatus, UpdatePlanArgs};

    let mut session = AgentSession::new("plan-empty-unfinished");
    let existing = vec![PlanStep {
        step: "继续实现".to_string(),
        status: PlanStepStatus::InProgress,
    }];
    session.current_plan = Some(existing.clone());
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plan_tx, mut plan_rx) = tokio::sync::mpsc::unbounded_channel();
    (session.progress_sender, session.plan_sender) = (Some(progress_tx), Some(plan_tx));

    let dir = tempfile::tempdir().unwrap();
    let mut recorder = ConversationRecorder::new(dir.path(), "plan-empty-unfinished");
    let mut recorder_ref = Some(&mut recorder);
    apply_plan_update_snapshot(
        &mut session,
        &mut recorder_ref,
        UpdatePlanArgs {
            explanation: Some("当前消息只包含项目规则".to_string()),
            plan: Vec::new(),
        },
    );
    recorder.close();

    assert_eq!(session.current_plan, Some(existing));
    assert!(progress_rx.try_recv().is_err());
    assert!(plan_rx.try_recv().is_err());
    assert!(!recorder.file_path().exists());
}

#[test]
fn test_clear_completed_plan_for_new_turn_resets_runtime_state() {
    use crate::tools::update_plan::{PlanStep, PlanStepStatus};
    let mut session = AgentSession::new("plan-clear");
    session.current_plan = Some(vec![PlanStep {
        step: "完成旧任务".to_string(),
        status: PlanStepStatus::Completed,
    }]);
    session.plan_repair_attempts = 1;
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plan_tx, mut plan_rx) = tokio::sync::mpsc::unbounded_channel();
    (session.progress_sender, session.plan_sender) = (Some(progress_tx), Some(plan_tx));
    let mut recorder = None;
    clear_completed_plan_for_new_turn(&mut session, &mut recorder);
    assert!(session.current_plan.is_none());
    assert_eq!(session.plan_repair_attempts, 0);
    assert!(matches!(
        progress_rx.try_recv(),
        Ok(AgentTurnProgressEvent::PlanUpdated { steps, .. }) if steps.is_empty()
    ));
    assert!(plan_rx.try_recv().expect("plan clear event").0.is_empty());
}

async fn chat_response_url(responses: Vec<serde_json::Value>) -> String {
    counted_chat_response_url(responses).await.0
}

async fn counted_chat_response_url(
    responses: Vec<serde_json::Value>,
) -> (String, std::sync::Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let request_count = std::sync::Arc::new(AtomicUsize::new(0));
    let request_count_for_server = std::sync::Arc::clone(&request_count);
    tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            request_count_for_server.fetch_add(1, Ordering::SeqCst);
            let mut buffer = Vec::new();
            let mut header_end = None;
            loop {
                let mut chunk = [0_u8; 1024];
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if header_end.is_none() {
                    header_end = buffer.windows(4).position(|w| w == b"\r\n\r\n");
                }
                if let Some(pos) = header_end {
                    let headers = String::from_utf8_lossy(&buffer[..pos]);
                    let content_len = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if buffer.len() >= pos + 4 + content_len {
                        break;
                    }
                }
            }

            let body = response.to_string();
            let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(http.as_bytes()).await.unwrap();
        }
    });
    (format!("http://{addr}/chat/completions"), request_count)
}

async fn slow_chat_response_url(
    delay: std::time::Duration,
) -> (String, tokio::sync::oneshot::Receiver<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = accepted_tx.send(());
        let mut buffer = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let n = stream.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        tokio::time::sleep(delay).await;
        let body = chat_text_response("too late", 11).to_string();
        let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        let _ = stream.write_all(http.as_bytes()).await;
    });
    (format!("http://{addr}/chat/completions"), accepted_rx)
}

fn provider_config_for_url(url: String) -> AgentConfig {
    let mut model_providers = HashMap::new();
    model_providers.insert(
        "test".to_string(),
        ModelProviderConfig {
            name: Some("test".to_string()),
            base_url: Some(url),
            wire_api: Some(crate::config::ModelWireApi::ChatCompletions),
            env_key: None,
            api_key: Some("test-key".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );
    AgentConfig {
        model: Some("test-model".to_string()),
        model_provider: Some("test".to_string()),
        model_providers,
        model_context_window: Some(1_000),
        model_auto_compact_token_limit: Some(100),
        request_timeout_secs: Some(30),
        ..Default::default()
    }
}

fn chat_text_response(content: &str, total_tokens: u64) -> serde_json::Value {
    serde_json::json!({
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": content
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": total_tokens.saturating_sub(1),
            "completion_tokens": 1,
            "total_tokens": total_tokens
        }
    })
}

fn chat_tool_calls_response(
    tool_calls: Vec<ToolCallMessage>,
    total_tokens: u64,
) -> serde_json::Value {
    serde_json::json!({
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "tool_calls": tool_calls
                },
                "finish_reason": "tool_calls"
            }
        ],
        "usage": {
            "prompt_tokens": total_tokens.saturating_sub(1),
            "completion_tokens": 1,
            "total_tokens": total_tokens
        }
    })
}

struct ParallelGate {
    started: AtomicUsize,
    notify: tokio::sync::Notify,
}

struct BarrierTool {
    name: &'static str,
    gate: Arc<ParallelGate>,
}

#[async_trait]
impl ToolHandler for BarrierTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "test-only barrier tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(&self, _arguments: &str, _work_dir: &Path) -> crate::types::ToolResult {
        let started = self.gate.started.fetch_add(1, Ordering::SeqCst) + 1;
        if started >= 2 {
            self.gate.notify.notify_waiters();
        }
        loop {
            let notified = self.gate.notify.notified();
            if self.gate.started.load(Ordering::SeqCst) >= 2 {
                break;
            }
            notified.await;
        }
        crate::types::ToolResult {
            success: true,
            output: format!("done:{}", self.name),
            runtime_events: Vec::new(),
        }
    }
}

async fn provider_config_for_responses(responses: Vec<serde_json::Value>) -> AgentConfig {
    let url = chat_response_url(responses).await;
    provider_config_for_url(url)
}

async fn single_summary_provider_config(total_tokens: u64) -> AgentConfig {
    provider_config_for_responses(vec![chat_text_response("compacted summary", total_tokens)]).await
}

#[test]
fn test_session_new() {
    let session = AgentSession::new("test-user");
    assert_eq!(session.session_key, "test-user");
    assert!(session.history.is_empty());
    assert_eq!(session.compaction_count, 0);
    assert!(session.total_tokens_used.is_none());
}

#[test]
fn test_session_clear_resets_all() {
    let mut session = AgentSession::new("test");
    session.add_user_message("hello");
    session.compaction_count = 3;
    session.total_tokens_used = Some(50000);
    session.current_plan = Some(vec![crate::tools::update_plan::PlanStep {
        step: "Do work".to_string(),
        status: crate::tools::update_plan::PlanStepStatus::InProgress,
    }]);
    session.plan_repair_attempts = 1;
    session.clear();
    assert!(session.history.is_empty());
    assert_eq!(session.compaction_count, 0);
    assert!(session.total_tokens_used.is_none());
    assert!(session.current_plan.is_none());
    assert_eq!(session.plan_repair_attempts, 0);
    assert!(
        session.memory_cleared,
        "memory_cleared should be set after clear"
    );
    assert!(
        session.recorder.is_none(),
        "recorder should be dropped after clear"
    );
}

#[test]
fn reinitialize_work_dir_preserves_current_turn_events() {
    let dir = tempfile::tempdir().expect("work dir");
    let mut session = AgentSession::new("switch-events");
    session.record_turn_event(CodexTurnEventKind::TurnStarted);
    session.record_turn_event(CodexTurnEventKind::ToolBatchStarted);

    session.reinitialize_work_dir(dir.path().to_string_lossy().to_string());

    assert_eq!(session.last_turn_events.len(), 2);
    assert_eq!(
        session.last_turn_events[0].kind,
        CodexTurnEventKind::TurnStarted
    );
    assert_eq!(
        session.last_turn_events[1].kind,
        CodexTurnEventKind::ToolBatchStarted
    );
}

#[test]
fn test_track_token_usage() {
    let mut session = AgentSession::new("test");
    assert!(session.total_tokens_used.is_none());

    session.add_user_message("hello");
    session.track_token_usage(900, 1000);
    assert_eq!(session.total_tokens_used, Some(1000));
    assert_eq!(session.last_response_tokens, Some(900));
    assert_eq!(session.last_response_history_len, Some(1));

    session.add_assistant_message("hi");
    assert_eq!(session.last_response_history_len, Some(2));
    session.track_token_usage(1500, 2000);
    assert_eq!(session.total_tokens_used, Some(3000));
    assert_eq!(session.last_response_tokens, Some(1500));
    assert_eq!(session.last_response_history_len, Some(2));
}

#[test]
fn test_background_token_usage_does_not_update_context_snapshot() {
    let mut session = AgentSession::new("test");

    session.track_token_usage(800, 1000);
    session.track_background_token_usage(2500);

    assert_eq!(session.total_tokens_used, Some(3500));
    assert_eq!(session.last_response_tokens, Some(800));
}

#[test]
fn test_record_compaction_event_includes_emergency_and_total_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let mut recorder = ConversationRecorder::new(dir.path(), "test");
    let file_path = recorder.file_path().to_path_buf();

    let mut session = AgentSession::new("test");
    session.compaction_count = 2;
    session.total_tokens_used = Some(1234);
    let result = compact::CompactionResult {
        performed: true,
        pre_tokens: 900,
        post_tokens: 300,
        tokens_saved: 600,
        messages_removed: 8,
        reason: None,
        trigger: None,
        compaction_reason: None,
        phase: None,
        duration_ms: Some(10),
    };

    {
        let mut recorder_ref = Some(&mut recorder);
        record_compaction_event(
            &mut recorder_ref,
            &session,
            &result,
            "auto",
            "context_limit",
            "mid_turn",
            true,
        );
    }
    recorder.close();

    let events = persistence::load_conversation_events(&file_path).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, persistence::event_types::COMPACTION);
    assert_eq!(events[0].content["emergency"], true);
    assert_eq!(events[0].content["total_tokens"], 1234);
    assert_eq!(events[0].content["compaction_count"], 2);
    assert_eq!(events[0].content["phase"], "mid_turn");
}

#[test]
fn test_history_growth_after_response_is_incrementally_accounted() {
    let mut session = AgentSession::new("test");
    session.track_token_usage(100, 120);
    assert_eq!(session.last_response_tokens, Some(100));

    session.add_user_message(&"x".repeat(1_000));

    assert_eq!(session.last_response_tokens, Some(100));
    assert_eq!(session.effective_token_count(), 350);
}

#[test]
fn test_model_message_advances_incremental_token_boundary() {
    let mut session = AgentSession::new("test");
    session.add_user_message(&"old context ".repeat(100));
    session.track_token_usage(500, 700);
    session.add_assistant_message("covered by usage");
    session.add_user_message(&"new context ".repeat(100));

    assert_eq!(session.last_response_history_len, Some(2));
    assert_eq!(session.effective_token_count(), 800);
}

#[test]
fn test_restore_token_snapshot_keeps_cumulative_tokens_out_of_context() {
    let mut session = AgentSession::new("test");
    session.add_user_message("restored user");
    session.add_assistant_message("restored assistant");
    session.total_tokens_used = Some(50_000);

    session.restore_token_snapshot(Some(180));
    session.add_user_message(&"new turn ".repeat(40));

    assert_eq!(session.total_tokens_used, Some(50_000));
    assert_eq!(session.last_response_tokens, Some(180));
    assert_eq!(session.effective_token_count(), 270);
}

#[test]
fn test_recompute_token_snapshot_includes_base_instructions() {
    let mut session = AgentSession::new("test");
    session.add_user_message("hello");
    let base = "base instructions ".repeat(20);

    session.recompute_token_snapshot_from_history(Some(&base));

    let expected =
        session
            .estimate_tokens()
            .saturating_add(AgentSession::estimate_messages_tokens(&[
                ChatMessage::system(&base),
            ]));
    assert_eq!(session.last_response_tokens, Some(u64::from(expected)));
    assert_eq!(session.effective_token_count(), expected);
}

#[test]
fn test_mid_turn_initial_context_excludes_base_instructions() {
    let session = AgentSession::new("test");
    let prompt_prefix = vec![
        ChatMessage::system("base instructions"),
        ChatMessage::developer("developer context"),
        ChatMessage::user("<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"),
    ];
    let memory_message = ChatMessage::developer("memory context");

    let context = build_mid_turn_initial_context(&session, &prompt_prefix, Some(&memory_message));

    assert_eq!(context.len(), 3);
    assert!(context
        .iter()
        .all(|message| message.content.as_deref() != Some("base instructions")));
    assert_eq!(
        count_messages_with_content(&context, "developer context"),
        1
    );
    assert_eq!(
        count_messages_with_content(
            &context,
            "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"
        ),
        1
    );
    assert_eq!(count_messages_with_content(&context, "memory context"), 1);
}

#[test]
fn test_build_messages_dedupes_injected_mid_turn_context() {
    let mut session = AgentSession::new("test");
    let env_context = "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>";
    session
        .history
        .push(ChatMessage::developer("developer context"));
    session.history.push(ChatMessage::user(env_context));
    session
        .history
        .push(ChatMessage::developer("memory context"));
    session.history.push(ChatMessage::user(&format!(
        "{}\nsummary",
        compact::SUMMARY_PREFIX
    )));
    let prompt_prefix = vec![
        ChatMessage::system("base instructions"),
        ChatMessage::developer("developer context"),
        ChatMessage::user(env_context),
    ];
    let memory_message = ChatMessage::developer("memory context");

    let messages = session.build_messages(&prompt_prefix, Some(&memory_message));

    assert_eq!(
        count_messages_with_content(&messages, "base instructions"),
        1
    );
    assert_eq!(
        count_messages_with_content(&messages, "developer context"),
        1
    );
    assert_eq!(count_messages_with_content(&messages, env_context), 1);
    assert_eq!(count_messages_with_content(&messages, "memory context"), 1);
    assert!(messages[0].role == "system");
}

#[test]
fn test_build_messages_dedupes_context_against_full_history() {
    let mut session = AgentSession::new("test");
    session
        .history
        .push(ChatMessage::developer("developer context"));
    for i in 0..10 {
        session.add_user_message(&format!("msg {i}"));
    }
    let prompt_prefix = vec![
        ChatMessage::system("base instructions"),
        ChatMessage::developer("developer context"),
    ];

    let messages = session.build_messages(&prompt_prefix, None);

    assert_eq!(
        count_messages_with_content(&messages, "base instructions"),
        1
    );
    assert_eq!(
        count_messages_with_content(&messages, "developer context"),
        1
    );
    assert_eq!(messages.len(), 12);
    assert_eq!(messages[2].content.as_deref(), Some("msg 0"));
    assert_eq!(messages[11].content.as_deref(), Some("msg 9"));
}

#[tokio::test]
async fn test_queued_continuation_compacts_before_next_model_request() {
    let mut config = single_summary_provider_config(77).await;
    config.model_auto_compact_token_limit = Some(500);
    let client = AgentClient::new();
    let mut session = AgentSession::new("queued-compaction");
    let status_handle = std::sync::Arc::new(std::sync::Mutex::new(ActiveTurnStatus::new(
        &session.session_key,
    )));
    session.active_turn_status = Some(status_handle.clone());

    session.add_user_message("first");
    session.add_assistant_message("reply");
    session.add_user_message("second");
    session.add_assistant_message("final before queued message");
    session.track_token_usage(17, 30);
    session.add_user_message(&"queued context ".repeat(200));

    assert!(compact::should_compact(&session, &config));
    let mut recorder: Option<&mut ConversationRecorder> = None;
    let prompt_prefix = vec![
        ChatMessage::system("base instructions"),
        ChatMessage::developer("developer context"),
        ChatMessage::user("<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"),
    ];
    let memory_message = ChatMessage::developer("memory context");
    compact_mid_turn_if_needed(
        &client,
        &config,
        &mut session,
        &mut recorder,
        &prompt_prefix,
        Some(&memory_message),
        ActiveTurnProgress {
            state: "model_response",
            current_loop_iteration: 1,
            completed_loop_iterations: 1,
            max_loop_iterations: 8,
            local_tool_count: 0,
            mcp_tool_count: 0,
        },
        &crate::turn_runtime::CancellationToken::default_grace(),
    )
    .await
    .unwrap();

    assert_eq!(session.compaction_count, 1);
    assert_eq!(session.total_tokens_used, Some(107));
    let expected_snapshot_tokens =
        session
            .estimate_tokens()
            .saturating_add(AgentSession::estimate_messages_tokens(&[
                ChatMessage::system("base instructions"),
            ]));
    assert_eq!(
        session.last_response_tokens,
        Some(u64::from(expected_snapshot_tokens))
    );
    assert_eq!(
        session.last_response_history_len,
        Some(session.history.len())
    );
    assert!(session
        .history
        .iter()
        .all(|message| message.content.as_deref() != Some("base instructions")));
    assert_eq!(
        count_messages_with_content(&session.history, "developer context"),
        1
    );
    assert_eq!(
        count_messages_with_content(
            &session.history,
            "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"
        ),
        1
    );
    assert_eq!(
        count_messages_with_content(&session.history, "memory context"),
        1
    );
    assert!(session
        .history
        .last()
        .is_some_and(compact::is_summary_message));
    let messages = session.build_messages(&prompt_prefix, Some(&memory_message));
    assert_eq!(
        count_messages_with_content(&messages, "base instructions"),
        1
    );
    assert_eq!(
        count_messages_with_content(&messages, "developer context"),
        1
    );
    assert_eq!(
        count_messages_with_content(
            &messages,
            "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"
        ),
        1
    );
    assert_eq!(count_messages_with_content(&messages, "memory context"), 1);
    let status = status_handle.lock().unwrap().clone();
    assert_eq!(status.compaction_count, session.compaction_count);
    assert_eq!(status.history_version, session.history_version);
    assert_eq!(
        status.estimated_context_tokens,
        session.effective_token_count()
    );
}

#[tokio::test]
async fn test_pre_sampling_auto_compacts_before_model_request() {
    let mut config = provider_config_for_responses(vec![
        chat_text_response("compacted summary", 77),
        chat_text_response("final answer", 33),
    ])
    .await;
    config.model_auto_compact_token_limit = Some(100);
    let client = AgentClient::new();
    let tools = ToolRegistry::new();
    let mut session = AgentSession::new("pre-sampling-compaction");
    session.add_user_message(&"old context ".repeat(1_000));

    assert!(compact::should_compact(&session, &config));
    let result = run_turn(
        &client,
        &config,
        &mut session,
        &tools,
        "answer without tools",
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.response, "final answer");
    assert_eq!(session.compaction_count, 1);
    assert!(session.history.iter().any(compact::is_summary_message));
    assert!(session
        .history
        .iter()
        .all(|message| message.content.as_deref() != Some("base instructions")));
    assert!(session
        .history
        .iter()
        .any(|message| message.content.as_deref() == Some("answer without tools")));
    assert!(session
        .history
        .iter()
        .any(|message| message.content.as_deref() == Some("final answer")));
}

#[tokio::test]
async fn codex_parallel_tool_batch_preserves_history_order() {
    let tool_calls = vec![
        ToolCallMessage::function_call(
            "call-a".to_string(),
            "read_file".to_string(),
            "{}".to_string(),
        ),
        ToolCallMessage::function_call(
            "call-b".to_string(),
            "list_directory".to_string(),
            "{}".to_string(),
        ),
    ];
    let config = provider_config_for_responses(vec![
        chat_tool_calls_response(tool_calls, 22),
        chat_text_response("final answer", 24),
    ])
    .await;
    let gate = Arc::new(ParallelGate {
        started: AtomicUsize::new(0),
        notify: tokio::sync::Notify::new(),
    });
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(BarrierTool {
        name: "read_file",
        gate: Arc::clone(&gate),
    }));
    tools.register(Arc::new(BarrierTool {
        name: "list_directory",
        gate,
    }));
    let client = AgentClient::new();
    let mut session = AgentSession::new("parallel-tools");

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run_turn(
            &client,
            &config,
            &mut session,
            &tools,
            "run both tools",
            None,
        ),
    )
    .await
    .expect("parallel tools should not block each other")
    .unwrap();

    assert_eq!(result.response, "final answer");
    assert_eq!(
        result
            .tool_calls_log
            .iter()
            .map(|log| log.tool_name.as_str())
            .collect::<Vec<_>>(),
        vec!["read_file", "list_directory"]
    );
    assert_eq!(
        session
            .history
            .iter()
            .filter(|message| message.role == "tool")
            .map(|message| message.content.as_deref().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["done:read_file", "done:list_directory"]
    );
    assert!(session.last_turn_events.iter().any(|event| {
        event.kind == CodexTurnEventKind::ToolBatchStarted
            && event.detail.as_deref() == Some("mode=parallel,count=2")
    }));
    assert!(session
        .last_turn_events
        .iter()
        .any(|event| event.kind == CodexTurnEventKind::TurnCompleted));
}

#[tokio::test]
async fn exec_command_long_task_waits_in_runtime_without_model_polling() {
    let tool_calls = vec![ToolCallMessage::function_call(
        "call-exec".to_string(),
        "exec_command".to_string(),
        r#"{"cmd":"printf start; sleep 0.4; printf done","yield_time_ms":250}"#.to_string(),
    )];
    let (url, request_count) = counted_chat_response_url(vec![
        chat_tool_calls_response(tool_calls, 22),
        chat_text_response("final answer", 24),
    ])
    .await;
    let config = provider_config_for_url(url);
    let client = AgentClient::new();
    let tools = ToolRegistry::with_defaults();
    let mut session = AgentSession::new("long-task-runtime-watch");

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_turn(
            &client,
            &config,
            &mut session,
            &tools,
            "run a long command",
            None,
        ),
    )
    .await
    .expect("runtime watcher should finish after process exits")
    .unwrap();

    assert_eq!(result.response, "final answer");
    assert_eq!(
        request_count.load(Ordering::SeqCst),
        2,
        "runtime watcher should not ask the model to poll write_stdin"
    );
    let tool_output = session
        .history
        .iter()
        .find(|message| message.role == "tool")
        .and_then(|message| message.content.as_deref())
        .expect("tool result");
    assert!(tool_output.contains("process_exited"), "{tool_output}");
    assert!(tool_output.contains("start"), "{tool_output}");
    assert!(tool_output.contains("done"), "{tool_output}");
    assert!(
        tool_output.contains("model_request_count_while_waiting"),
        "{tool_output}"
    );
    assert!(session.last_turn_events.iter().any(|event| {
        event.kind == CodexTurnEventKind::TurnSuspended
            && event.detail.as_deref().is_some_and(|detail| {
                detail.contains("waiting_on_session") && detail.contains("adaptive")
            })
    }));
    assert!(session
        .last_turn_events
        .iter()
        .any(|event| event.kind == CodexTurnEventKind::LongTaskExited));
    assert!(session
        .last_turn_events
        .iter()
        .any(|event| event.kind == CodexTurnEventKind::TurnResumed));
}

#[test]
fn long_task_watch_interval_includes_bounded_jitter() {
    let base = 1_000;
    let first = super::turn_loop::jittered_watch_interval_ms(base, "session-a", 1);
    let second = super::turn_loop::jittered_watch_interval_ms(base, "session-a", 2);

    assert!((900..=1_100).contains(&first));
    assert!((900..=1_100).contains(&second));
    assert_ne!(first, second, "heartbeat should influence jitter");
}

#[test]
fn adaptive_long_task_watch_extends_beyond_background_terminal_timeout() {
    assert_eq!(
        super::turn_loop::long_task_profile_max_wait("adaptive", 300_000),
        std::time::Duration::from_millis(3_900_000)
    );
    assert_eq!(
        super::turn_loop::long_task_profile_max_wait("unclassified", 7_200_000),
        std::time::Duration::from_millis(7_200_000)
    );
}

#[test]
fn long_task_profile_intervals_are_bounded_by_profile_caps() {
    assert_eq!(
        super::turn_loop::long_task_profile_max_interval_ms("adaptive"),
        300_000
    );
    assert_eq!(
        super::turn_loop::long_task_profile_max_interval_ms("unclassified"),
        300_000
    );
    assert_eq!(
        super::turn_loop::next_long_task_interval_ms("adaptive", 300_000),
        300_000
    );
}

#[tokio::test]
async fn test_stop_request_cancels_in_flight_model_request() {
    let (url, accepted_rx) = slow_chat_response_url(std::time::Duration::from_secs(30)).await;
    let config = provider_config_for_url(url);
    let client = AgentClient::new();
    let tools = ToolRegistry::new();
    let manager = std::sync::Arc::new(AgentSessionManager::new(60));
    let mut session = manager
        .try_take_session("stop-model-request")
        .expect("session should be available");
    let manager_for_stop = std::sync::Arc::clone(&manager);

    let handle = tokio::spawn(async move {
        let result = run_turn(
            &client,
            &config,
            &mut session,
            &tools,
            "please wait for a slow model",
            None,
        )
        .await;
        (result, session)
    });

    accepted_rx
        .await
        .expect("slow mock should receive the model request");
    assert!(manager_for_stop.request_stop("stop-model-request"));

    let (result, session) = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("turn should stop promptly")
        .expect("join should succeed");
    let result = result.expect("stop should return a normal turn result");
    assert!(result.response.contains("已收到 /stop"));
    assert!(session
        .history
        .iter()
        .any(|message| message.content.as_deref() == Some(STOPPED_RESPONSE)));
}

#[path = "path_switch_tests.rs"]
mod path_switch_tests;

#[tokio::test]
async fn test_stop_when_idle_does_not_call_model() {
    let config =
        provider_config_for_responses(vec![chat_text_response("should not call", 1)]).await;
    let client = AgentClient::new();
    let tools = ToolRegistry::new();
    let mut session = AgentSession::new("idle-stop");

    let result = run_turn(&client, &config, &mut session, &tools, "/stop", None)
        .await
        .unwrap();

    assert_eq!(result.response, "当前没有正在执行的 Agent loop。");
    assert!(session.history.is_empty());
    assert!(session.total_tokens_used.is_none());
}

#[tokio::test]
async fn test_unknown_slash_input_falls_back_to_normal_chat_message() {
    let (url, request_count) =
        counted_chat_response_url(vec![chat_text_response("slash response", 24)]).await;
    let config = provider_config_for_url(url);
    let client = AgentClient::new();
    let tools = ToolRegistry::new();
    let mut session = AgentSession::new("unknown-slash");

    let result = run_turn(&client, &config, &mut session, &tools, "/demo", None)
        .await
        .unwrap();

    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    assert_eq!(result.response, "slash response");
    assert!(session
        .history
        .iter()
        .any(|message| message.role == "user" && message.content.as_deref() == Some("/demo")));
}

#[tokio::test]
async fn test_manual_compaction_command_does_not_record_user_message() {
    let (url, request_count) =
        counted_chat_response_url(vec![chat_text_response("compacted summary", 24)]).await;
    let config = provider_config_for_url(url);
    let client = AgentClient::new();
    let mut session = AgentSession::new("manual-compact");
    session.add_user_message("请总结一下当前改动");
    session.add_assistant_message("我会先检查当前工程。");

    let dir = tempfile::tempdir().unwrap();
    let mut recorder = ConversationRecorder::new(dir.path(), "manual-compact");
    let result = run_manual_compaction_command(&client, &config, &mut session, Some(&mut recorder))
        .await
        .unwrap();
    recorder.close();

    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    assert_eq!(result.response, "上下文已自动压缩");
    assert_eq!(session.compaction_count, 1);
    assert!(!session
        .history
        .iter()
        .any(|message| message.content.as_deref() == Some("/compact")));

    let raw = std::fs::read_to_string(recorder.file_path()).unwrap();
    assert!(!raw.contains(r#""event_type":"user_message""#));
    assert!(!raw.contains("/compact"));
    assert!(raw.contains(r#""event_type":"compaction""#));
}

#[tokio::test]
async fn test_compaction_recomputes_context_snapshot_and_drops_tool_suffix() {
    let mut config = single_summary_provider_config(77).await;
    config.model_auto_compact_token_limit = Some(500);
    let client = AgentClient::new();
    let mut session = AgentSession::new("tool-output-compaction");

    session.add_user_message("first");
    session.add_assistant_message("reply");
    session.add_user_message("inspect generated output");
    session.track_token_usage(17, 30);
    session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
    session.add_tool_result("call-1", &"tool-output ".repeat(2_000));

    let pre_tokens = session.effective_token_count();
    assert!(compact::should_compact(&session, &config));

    let result = compact::compact_session(
        &client,
        &config,
        &mut session,
        compact::CompactionTrigger::Auto,
        compact::CompactionReason::ContextLimit,
        compact::CompactionPhase::MidTurn,
        None,
        compact::InitialContextInjection::DoNotInject,
        Vec::new(),
        &crate::turn_runtime::CancellationToken::default_grace(),
    )
    .await
    .unwrap();

    assert!(result.performed);
    assert_eq!(result.pre_tokens, pre_tokens);
    assert_eq!(session.compaction_count, 1);
    assert_eq!(session.total_tokens_used, Some(107));
    assert!(session.history.iter().all(|message| message.role == "user"));
    assert!(session
        .history
        .iter()
        .all(|message| message.content.as_deref() != Some("reply")));
    assert!(session.history.iter().all(|message| message.role != "tool"));
    assert!(session
        .history
        .last()
        .is_some_and(compact::is_summary_message));
    assert_eq!(
        session.last_response_tokens,
        Some(u64::from(session.estimate_tokens()))
    );
    assert_eq!(
        session.last_response_history_len,
        Some(session.history.len())
    );
    assert_eq!(result.post_tokens, session.effective_token_count());
    assert!(result.post_tokens < config.get_compact_threshold_tokens());
    assert!(!compact::should_compact(&session, &config));
}

#[tokio::test]
async fn test_compaction_runs_for_first_tool_turn_below_four_messages() {
    let mut config = single_summary_provider_config(77).await;
    config.model_auto_compact_token_limit = Some(500);
    let client = AgentClient::new();
    let mut session = AgentSession::new("first-tool-turn-compaction");

    session.add_user_message("inspect generated output");
    session.track_token_usage(17, 30);
    session.add_assistant_tool_calls(&[test_tool_call("call-1")]);
    session.add_tool_result("call-1", &"tool-output ".repeat(2_000));

    assert_eq!(session.history.len(), 3);
    assert!(compact::should_compact(&session, &config));

    let result = compact::compact_session(
        &client,
        &config,
        &mut session,
        compact::CompactionTrigger::Auto,
        compact::CompactionReason::ContextLimit,
        compact::CompactionPhase::MidTurn,
        None,
        compact::InitialContextInjection::DoNotInject,
        Vec::new(),
        &crate::turn_runtime::CancellationToken::default_grace(),
    )
    .await
    .unwrap();

    assert!(result.performed);
    assert_eq!(session.compaction_count, 1);
    assert!(session.history.iter().all(|message| message.role == "user"));
    assert!(session.history.iter().all(|message| message.role != "tool"));
    assert!(session
        .history
        .last()
        .is_some_and(compact::is_summary_message));
    assert!(result.post_tokens < config.get_compact_threshold_tokens());
    assert!(!compact::should_compact(&session, &config));
}

#[tokio::test]
async fn test_retryable_error_rebuilds_messages_before_retry() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen_request_roles = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let seen_request_roles_server = seen_request_roles.clone();

    tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let mut header_end = None;
            loop {
                let mut chunk = [0_u8; 1024];
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if header_end.is_none() {
                    header_end = buffer.windows(4).position(|w| w == b"\r\n\r\n");
                }
                if let Some(pos) = header_end {
                    let headers = String::from_utf8_lossy(&buffer[..pos]);
                    let content_len = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if buffer.len() >= pos + 4 + content_len {
                        break;
                    }
                }
            }

            let body_start = header_end.unwrap() + 4;
            let body: serde_json::Value = serde_json::from_slice(&buffer[body_start..]).unwrap();
            let roles = body
                .get("messages")
                .and_then(|v| v.as_array())
                .unwrap()
                .iter()
                .map(|message| {
                    message
                        .get("role")
                        .and_then(|role| role.as_str())
                        .unwrap_or("<missing>")
                        .to_string()
                })
                .collect::<Vec<_>>();
            seen_request_roles_server.lock().unwrap().push(roles);

            let response = if attempt == 0 {
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close\r\n\r\ntemporary"
                        .to_string()
            } else {
                let payload = serde_json::json!({
                    "choices": [{
                        "message": {"role": "assistant", "content": "retry ok"},
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 2,
                        "total_tokens": 12
                    }
                })
                .to_string();
                format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    )
            };
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let mut config = AgentConfig {
        model: Some("retry-model".to_string()),
        model_provider: Some("retry-provider".to_string()),
        request_timeout_secs: Some(20),
        max_turn_iterations: Some(2),
        ..AgentConfig::default()
    };
    config.model_providers.insert(
        "retry-provider".to_string(),
        ModelProviderConfig {
            name: Some("retry-provider".to_string()),
            base_url: Some(format!("http://{addr}/chat/completions")),
            wire_api: Some(crate::config::ModelWireApi::ChatCompletions),
            env_key: None,
            api_key: Some("retry-key".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );

    let client = AgentClient::new();
    let tools = ToolRegistry::new();
    let mut session = AgentSession::new("retry-rebuild");
    session
        .history
        .push(ChatMessage::tool_result("stale", "stale tool output"));

    let result = run_turn(&client, &config, &mut session, &tools, "hello retry", None)
        .await
        .unwrap();

    assert_eq!(result.response, "retry ok");
    let seen_roles = seen_request_roles.lock().unwrap();
    assert_eq!(seen_roles.len(), 2);
    assert_eq!(seen_roles[0], seen_roles[1]);
    assert!(!seen_roles[0].iter().any(|role| role == "tool"));
}

#[test]
fn test_context_rewrite_refreshes_active_status_snapshot() {
    let config = AgentConfig {
        model_context_window: Some(1_000),
        ..Default::default()
    };
    let mut session = AgentSession::new("status-refresh");
    let status_handle = std::sync::Arc::new(std::sync::Mutex::new(ActiveTurnStatus::new(
        &session.session_key,
    )));
    session.active_turn_status = Some(status_handle.clone());
    session.add_user_message(&"before ".repeat(800));
    refresh_active_turn_status(
        &session,
        &config,
        ActiveTurnProgress {
            state: "model_request",
            current_loop_iteration: 1,
            completed_loop_iterations: 0,
            max_loop_iterations: 8,
            local_tool_count: 0,
            mcp_tool_count: 0,
        },
    );
    let before = status_handle.lock().unwrap().estimated_context_tokens;

    session.history = vec![ChatMessage::user("compacted summary")];
    session.compaction_count = 1;
    session.history_version = session.history_version.saturating_add(1);
    refresh_active_turn_status(
        &session,
        &config,
        ActiveTurnProgress {
            state: "model_request",
            current_loop_iteration: 1,
            completed_loop_iterations: 0,
            max_loop_iterations: 8,
            local_tool_count: 0,
            mcp_tool_count: 0,
        },
    );

    let status = status_handle.lock().unwrap().clone();
    assert!(before > status.estimated_context_tokens);
    assert_eq!(status.compaction_count, 1);
    assert_eq!(status.history_version, session.history_version);
    assert_eq!(status.estimated_context_tokens, session.estimate_tokens());
}

#[test]
fn test_build_messages_uses_full_sanitized_history() {
    let mut session = AgentSession::new("test");
    for i in 0..10 {
        session.add_user_message(&format!("msg {i}"));
    }
    let prefix = vec![ChatMessage::system("system")];
    let messages = session.build_messages(&prefix, None);
    assert_eq!(messages.len(), 11);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].content.as_deref(), Some("msg 0"));
    assert_eq!(messages[10].content.as_deref(), Some("msg 9"));
}

#[test]
fn test_build_messages_preserves_compaction_summary() {
    let mut session = AgentSession::new("test");
    session.compaction_count = 1; // Mark as compacted
    for i in 0..10 {
        session.add_user_message(&format!("msg {i}"));
    }
    session.add_user_message(&format!("{}\nprevious context", compact::SUMMARY_PREFIX));
    let prefix = vec![ChatMessage::system("system")];
    let messages = session.build_messages(&prefix, None);
    assert_eq!(messages.len(), 12);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].content.as_deref(), Some("msg 0"));
    assert!(messages[11]
        .content
        .as_deref()
        .is_some_and(|content| content.starts_with(compact::SUMMARY_PREFIX)));
}

#[test]
fn test_build_messages_with_full_history() {
    let mut session = AgentSession::new("test");
    for i in 0..5 {
        session.add_user_message(&format!("msg {i}"));
    }
    let prefix = vec![ChatMessage::system("system")];
    let messages = session.build_messages(&prefix, None);
    assert_eq!(messages.len(), 6); // 1 system + 5 history
}

#[test]
fn test_build_messages_sanitizes_malformed_tool_history_without_count_trim() {
    let mut session = AgentSession::new("test");
    session.add_tool_result("call-1", "[file] Cargo.toml");

    let prefix = vec![ChatMessage::system("system")];
    let messages = session.build_messages(&prefix, None);

    assert_eq!(messages.len(), 1);
}

#[test]
fn test_session_info_from_list() {
    let manager = AgentSessionManager::new(3600);
    let mut session = AgentSession::new("user-1");
    session.add_user_message("hello");
    session.total_tokens_used = Some(500);
    manager.return_session(session);

    let sessions = manager.list_sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_key, "user-1");
    assert_eq!(sessions[0].message_count, 1);
    assert_eq!(sessions[0].total_tokens_used, Some(500));
    assert_eq!(sessions[0].title.as_deref(), Some("hello"));
}

#[test]
fn test_running_turns_remain_visible_in_session_list() {
    let manager = AgentSessionManager::new(3600);
    let mut idle = AgentSession::new("idle-session");
    idle.add_user_message("hello");
    manager.return_session(idle);

    let running = manager.take_session("running-session");
    manager.update_active_session_preview(
        "running-session",
        Some("still running".to_string()),
        Some("/tmp/work".to_string()),
        Some("admin-api".to_string()),
        Some("codex".to_string()),
        Some("abc".to_string()),
    );

    let sessions = manager.list_sessions();
    assert_eq!(sessions.len(), 2);
    let running_info = sessions
        .iter()
        .find(|session| session.session_key == "running-session")
        .expect("running session should stay visible while checked out");
    assert!(running_info.running);
    assert_eq!(running_info.title.as_deref(), Some("still running"));
    assert_eq!(running_info.work_dir.as_deref(), Some("/tmp/work"));
    assert_eq!(running_info.runner_type.as_deref(), Some("codex"));
    assert_eq!(running_info.runner_id.as_deref(), Some("abc"));

    let running_statuses = manager.list_active_turn_statuses();
    assert_eq!(running_statuses.len(), 1);
    assert_eq!(running_statuses[0].session_key, "running-session");

    manager.return_session(running);
    assert!(manager.list_active_turn_statuses().is_empty());
}

#[test]
fn test_session_manager_broadcasts_session_list_changes() {
    let manager = AgentSessionManager::new(3600);
    let mut events = manager.subscribe_session_events();

    let session = manager.take_session("event-session");
    let started = events.try_recv().expect("take should broadcast start");
    assert_eq!(started.event_type, "sessions_changed");
    assert_eq!(started.session_key.as_deref(), Some("event-session"));
    assert_eq!(started.reason, "started");

    manager.update_active_session_preview(
        "event-session",
        Some("event title".to_string()),
        None,
        Some("im".to_string()),
        Some("bifrost_agent".to_string()),
        None,
    );
    let updated = events
        .try_recv()
        .expect("preview update should broadcast update");
    assert_eq!(updated.session_key.as_deref(), Some("event-session"));
    assert_eq!(updated.reason, "updated");

    manager.return_session(session);
    let returned = events.try_recv().expect("return should broadcast change");
    assert_eq!(returned.session_key.as_deref(), Some("event-session"));
    assert_eq!(returned.reason, "returned");
}

#[test]
fn test_return_session_clears_runtime_channels() {
    let manager = AgentSessionManager::new(3600);
    let mut session = AgentSession::new("runtime-channels");
    let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plan_tx, _plan_rx) = tokio::sync::mpsc::unbounded_channel();
    session.progress_sender = Some(progress_tx);
    session.plan_sender = Some(plan_tx);

    manager.return_session(session);

    let session = manager
        .try_take_session("runtime-channels")
        .expect("returned session should be available");
    assert!(session.progress_sender.is_none());
    assert!(session.plan_sender.is_none());
}

#[test]
fn test_take_session_with_work_dir_overrides_existing_status_session() {
    let manager = AgentSessionManager::new(3600);
    let mut status_session = manager
        .try_take_session_with_work_dir("status-first", None)
        .expect("first take should create status session");
    status_session.add_user_message("/status");
    manager.return_session(status_session);

    let session = manager
        .try_take_session_with_work_dir("status-first", Some("/tmp/bifrost-work".to_string()))
        .expect("second take should not be busy");

    assert_eq!(session.work_dir.as_deref(), Some("/tmp/bifrost-work"));
    assert!(session.history.is_empty());
    assert!(!session.memory_cleared);
}

#[test]
fn test_rollback_single_turn() {
    let mut session = AgentSession::new("test");
    session.add_user_message("msg1");
    session.add_assistant_message("reply1");
    session.add_user_message("msg2");
    session.add_assistant_message("reply2");
    assert_eq!(session.history.len(), 4);

    let removed = session.rollback(1);
    assert_eq!(removed, 2); // user + assistant
    assert_eq!(session.history.len(), 2);
    assert_eq!(session.history[0].content.as_deref(), Some("msg1"));
    assert_eq!(session.history_version, 1);
}

#[test]
fn test_rollback_multiple_turns() {
    let mut session = AgentSession::new("test");
    for i in 0..3 {
        session.add_user_message(&format!("msg{i}"));
        session.add_assistant_message(&format!("reply{i}"));
    }
    assert_eq!(session.history.len(), 6);

    let removed = session.rollback(2);
    assert_eq!(removed, 4);
    assert_eq!(session.history.len(), 2);
}

#[test]
fn test_rollback_preserves_compaction_summary() {
    let mut session = AgentSession::new("test");
    session.compaction_count = 1; // Marked as compacted
    session.add_user_message("msg1");
    session.add_assistant_message("reply1");
    session.add_user_message(&format!("{}\nsummary", compact::SUMMARY_PREFIX));

    // Roll back more turns than exist
    let removed = session.rollback(999);
    assert_eq!(session.history.len(), 1); // Summary preserved
    assert!(compact::is_summary_message(&session.history[0]));
    assert!(removed > 0);
}

#[test]
fn test_rollback_zero_is_noop() {
    let mut session = AgentSession::new("test");
    session.add_user_message("hello");
    let removed = session.rollback(0);
    assert_eq!(removed, 0);
    assert_eq!(session.history.len(), 1);
    assert_eq!(session.history_version, 0);
}

#[test]
fn test_clear_bumps_history_version() {
    let mut session = AgentSession::new("test");
    session.add_user_message("hello");
    assert_eq!(session.history_version, 0);
    session.clear();
    assert_eq!(session.history_version, 1);
}

#[test]
fn test_clear_sets_memory_cleared_flag() {
    let mut session = AgentSession::new("test");
    assert!(!session.memory_cleared);
    session.add_user_message("hello");
    session.add_assistant_message("hi");
    session.clear();
    assert!(session.memory_cleared, "clear should set memory_cleared");
    assert!(session.history.is_empty());

    // After adding a new message, the flag should still be set
    // (it's only reset by the turn loop, not by adding messages)
    session.add_user_message("new message");
    assert!(session.memory_cleared);
}

#[test]
fn test_is_context_window_error() {
    assert!(is_context_window_error(
        "API error (status 400): context_length_exceeded"
    ));
    assert!(is_context_window_error(
        "This model's maximum context length is 128000"
    ));
    assert!(!is_context_window_error(
        "API error (status 500): internal error"
    ));
}

#[test]
fn test_is_retryable_error() {
    assert!(is_retryable_error("API error (status 429): rate limit"));
    assert!(is_retryable_error("HTTP request failed: timeout"));
    assert!(is_retryable_error("connection reset"));
    assert!(!is_retryable_error("invalid api key"));
}

#[test]
fn test_apply_tool_runtime_events_applies_plan_update() {
    let mut session = AgentSession::new("runtime-event-plan");
    let mut recorder = None;
    apply_tool_runtime_events(
        &mut session,
        &mut recorder,
        "update_plan",
        true,
        vec![ToolRuntimeEvent::PlanUpdate(
            crate::tools::update_plan::UpdatePlanArgs {
                explanation: Some("Progress update".to_string()),
                plan: vec![
                    crate::tools::update_plan::PlanStep {
                        step: "Inspect files".to_string(),
                        status: crate::tools::update_plan::PlanStepStatus::Completed,
                    },
                    crate::tools::update_plan::PlanStep {
                        step: "Write summary".to_string(),
                        status: crate::tools::update_plan::PlanStepStatus::InProgress,
                    },
                ],
            },
        )],
    );

    let plan = session.current_plan.expect("plan should be applied");
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].step, "Inspect files");
    assert_eq!(
        plan[0].status,
        crate::tools::update_plan::PlanStepStatus::Completed
    );
    assert_eq!(
        plan[1].status,
        crate::tools::update_plan::PlanStepStatus::InProgress
    );
}

#[test]
fn test_failed_tool_runtime_events_are_ignored() {
    let mut session = AgentSession::new("runtime-event-failed-plan");
    let mut recorder = None;
    apply_tool_runtime_events(
        &mut session,
        &mut recorder,
        "update_plan",
        false,
        vec![ToolRuntimeEvent::PlanUpdate(
            crate::tools::update_plan::UpdatePlanArgs {
                explanation: Some("should not apply".to_string()),
                plan: vec![crate::tools::update_plan::PlanStep {
                    step: "Invalid failed event".to_string(),
                    status: crate::tools::update_plan::PlanStepStatus::Completed,
                }],
            },
        )],
    );

    assert!(session.current_plan.is_none());
}

#[test]
fn test_plan_has_unfinished_steps_false_when_all_completed() {
    let plan = vec![
        crate::tools::update_plan::PlanStep {
            step: "A".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::Completed,
        },
        crate::tools::update_plan::PlanStep {
            step: "B".to_string(),
            status: crate::tools::update_plan::PlanStepStatus::Completed,
        },
    ];
    assert!(!plan_has_unfinished_steps(&plan));
}

#[test]
fn test_plan_has_unfinished_steps_true_for_pending_or_in_progress() {
    let pending_plan = vec![crate::tools::update_plan::PlanStep {
        step: "A".to_string(),
        status: crate::tools::update_plan::PlanStepStatus::Pending,
    }];
    assert!(plan_has_unfinished_steps(&pending_plan));

    let in_progress_plan = vec![crate::tools::update_plan::PlanStep {
        step: "B".to_string(),
        status: crate::tools::update_plan::PlanStepStatus::InProgress,
    }];
    assert!(plan_has_unfinished_steps(&in_progress_plan));
}

#[test]
fn test_truncate_tool_output_short() {
    let output = "short output";
    let result = truncate_tool_output(output, 100);
    assert_eq!(result, output);
}

#[test]
fn test_truncate_tool_output_long() {
    let output = "a".repeat(1000);
    let result = truncate_tool_output(&output, 100); // 100 tokens = 400 chars limit
    assert!(result.len() < output.len());
    assert!(result.contains("characters truncated"));
}
