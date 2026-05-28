use super::*;
use crate::client::AgentClient;
use crate::config::{AgentConfig, ModelProviderConfig};
use crate::types::ToolCallMessage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

fn test_config_for_base_url(base_url: String) -> AgentConfig {
    let mut model_providers = HashMap::new();
    model_providers.insert(
        "test".to_string(),
        ModelProviderConfig {
            name: Some("test".to_string()),
            base_url: Some(base_url),
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
        ..Default::default()
    }
}

async fn compaction_retry_url(requests: Arc<Mutex<Vec<serde_json::Value>>>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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

            if let Some(pos) = header_end {
                let body = &buffer[pos + 4..];
                let request: serde_json::Value = serde_json::from_slice(body).unwrap();
                requests.lock().unwrap().push(request);
            }

            let (status, body) = if attempt == 0 {
                (
                    "400 Bad Request",
                    serde_json::json!({
                        "error": {
                            "message": "context_length_exceeded: too many tokens"
                        }
                    }),
                )
            } else {
                ("200 OK", chat_text_response("retry summary", 77))
            };
            let body = body.to_string();
            let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(http.as_bytes()).await.unwrap();
        }
    });
    format!("http://{addr}/chat/completions")
}

async fn transient_retry_url(requests: Arc<Mutex<Vec<serde_json::Value>>>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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

            if let Some(pos) = header_end {
                let body = &buffer[pos + 4..];
                let request: serde_json::Value = serde_json::from_slice(body).unwrap();
                requests.lock().unwrap().push(request);
            }

            let (status, body) = if attempt == 0 {
                (
                    "500 Internal Server Error",
                    serde_json::json!({
                        "error": {
                            "message": "temporary server error"
                        }
                    }),
                )
            } else {
                ("200 OK", chat_text_response("transient retry summary", 88))
            };
            let body = body.to_string();
            let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(http.as_bytes()).await.unwrap();
        }
    });
    format!("http://{addr}/chat/completions")
}

async fn compaction_sequence_url(
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    responses: Vec<(&'static str, serde_json::Value)>,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for (status, body) in responses {
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

            if let Some(pos) = header_end {
                let body = &buffer[pos + 4..];
                let request: serde_json::Value = serde_json::from_slice(body).unwrap();
                requests.lock().unwrap().push(request);
            }

            let body = body.to_string();
            let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(http.as_bytes()).await.unwrap();
        }
    });
    format!("http://{addr}/chat/completions")
}

#[test]
fn test_build_compaction_messages_uses_codex_local_request_shape() {
    let history = vec![
        ChatMessage::user("hello"),
        ChatMessage::assistant_with_tool_calls(vec![ToolCallMessage::function_call(
            "call-1".to_string(),
            "read_file".to_string(),
            r#"{"path":"Cargo.toml"}"#.to_string(),
        )]),
        ChatMessage::tool_result("call-1", "workspace = true"),
        ChatMessage::assistant("done"),
    ];
    let messages = build_compaction_messages(&history, Some("base instructions"));

    assert_eq!(messages.len(), history.len() + 2);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[0].content.as_deref(), Some("base instructions"));
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content.as_deref(), Some("hello"));
    assert_eq!(messages[2].role, "assistant");
    assert!(messages[2].tool_calls.is_some());
    assert_eq!(messages[3].role, "tool");
    assert_eq!(messages[3].content.as_deref(), Some("workspace = true"));
    assert_eq!(messages[4].role, "assistant");
    assert_eq!(messages[5].role, "user");
    assert_eq!(messages[5].content.as_deref(), Some(COMPACTION_PROMPT));
    assert!(messages
        .iter()
        .all(|message| message.role != "system"
            || message.content.as_deref() != Some(COMPACTION_PROMPT)));
    assert!(messages.iter().all(|message| {
        !message
            .content
            .as_deref()
            .is_some_and(|content| content.contains("[user]: hello"))
    }));
}

#[test]
fn test_build_compaction_messages_omits_empty_base_instructions() {
    let messages = build_compaction_messages(&[ChatMessage::user("hello")], Some("   "));

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content.as_deref(), Some("hello"));
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content.as_deref(), Some(COMPACTION_PROMPT));
}

#[test]
fn test_build_compaction_messages_does_not_inject_plan_text() {
    let messages = build_compaction_messages(&[ChatMessage::user("hello")], None);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content.as_deref(), Some("hello"));
    assert_eq!(messages[1].content.as_deref(), Some(COMPACTION_PROMPT));
    assert!(messages.iter().all(|message| !message
        .content
        .as_deref()
        .is_some_and(|content| content.contains("Current persisted task plan"))));
}

#[test]
fn test_remove_oldest_history_item_preserves_base_and_compaction_prompt() {
    let mut messages = build_compaction_messages(
        &[
            ChatMessage::user("oldest"),
            ChatMessage::assistant("reply"),
            ChatMessage::user("newest"),
        ],
        Some("base"),
    );

    assert!(remove_oldest_history_item_from_compaction_messages(
        &mut messages,
        1,
        1,
    ));

    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].content.as_deref(), Some("base"));
    assert_eq!(messages[1].content.as_deref(), Some("reply"));
    assert_eq!(messages[2].content.as_deref(), Some("newest"));
    assert_eq!(messages[3].content.as_deref(), Some(COMPACTION_PROMPT));
    assert!(!remove_oldest_history_item_from_compaction_messages(
        &mut vec![
            ChatMessage::system("base"),
            ChatMessage::user(COMPACTION_PROMPT)
        ],
        1,
        1,
    ));
}

#[test]
fn test_remove_oldest_history_item_preserves_compaction_prompt_tail() {
    let mut messages = vec![
        ChatMessage::system("base"),
        ChatMessage::user("oldest"),
        ChatMessage::assistant("reply"),
        ChatMessage::user(COMPACTION_PROMPT),
    ];

    assert!(remove_oldest_history_item_from_compaction_messages(
        &mut messages,
        1,
        1,
    ));
    assert!(remove_oldest_history_item_from_compaction_messages(
        &mut messages,
        1,
        1,
    ));
    assert!(!remove_oldest_history_item_from_compaction_messages(
        &mut messages,
        1,
        1,
    ));
    assert_eq!(messages[1].content.as_deref(), Some(COMPACTION_PROMPT));
}

#[tokio::test]
async fn test_compaction_retries_context_window_error_by_dropping_history_batch() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut config = test_config_for_base_url(compaction_retry_url(Arc::clone(&requests)).await);
    config.model_context_window = Some(4_000);
    let client = AgentClient::new();
    let mut session = AgentSession::new("compact-retry");
    session.history = vec![
        ChatMessage::user("oldest user"),
        ChatMessage::assistant("assistant reply"),
        ChatMessage::user("newest user"),
    ];

    let result = compact_session(
        &client,
        &config,
        &mut session,
        CompactionTrigger::Auto,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        Some("base instructions"),
        InitialContextInjection::DoNotInject,
        Vec::new(),
        &CancellationToken::default_grace(),
    )
    .await
    .unwrap();

    assert!(result.performed);
    assert_eq!(session.compaction_count, 1);
    assert_eq!(session.total_tokens_used, Some(77));
    assert!(session.history.last().is_some_and(is_summary_message));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first_messages = requests[0]["messages"].as_array().unwrap();
    let retry_messages = requests[1]["messages"].as_array().unwrap();

    assert_eq!(first_messages.len(), 5);
    assert_eq!(first_messages[0]["role"], "system");
    assert_eq!(first_messages[0]["content"], "base instructions");
    assert_eq!(first_messages[1]["content"], "oldest user");
    assert_eq!(first_messages[4]["content"], COMPACTION_PROMPT);

    assert!(retry_messages.len() < first_messages.len());
    assert_eq!(retry_messages[0]["content"], "base instructions");
    assert_eq!(retry_messages.last().unwrap()["content"], COMPACTION_PROMPT);
    assert!(retry_messages
        .iter()
        .all(|message| message["content"] != "oldest user"));
}

#[tokio::test]
async fn test_compaction_retries_transient_error_using_provider_budget() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut config = test_config_for_base_url(transient_retry_url(Arc::clone(&requests)).await);
    config
        .model_providers
        .get_mut("test")
        .unwrap()
        .stream_max_retries = Some(1);
    let client = AgentClient::new();
    let mut session = AgentSession::new("compact-transient-retry");
    session.history = vec![ChatMessage::user("keep this user message")];

    let result = compact_session(
        &client,
        &config,
        &mut session,
        CompactionTrigger::Auto,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        Some("base instructions"),
        InitialContextInjection::DoNotInject,
        Vec::new(),
        &CancellationToken::default_grace(),
    )
    .await
    .unwrap();

    assert!(result.performed);
    assert_eq!(session.compaction_count, 1);
    assert_eq!(session.total_tokens_used, Some(88));
    assert!(session.history.last().is_some_and(is_summary_message));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first_messages = requests[0]["messages"].as_array().unwrap();
    let retry_messages = requests[1]["messages"].as_array().unwrap();
    assert_eq!(first_messages.len(), retry_messages.len());
    assert_eq!(retry_messages[0]["content"], "base instructions");
    assert!(retry_messages
        .iter()
        .any(|message| message["content"] == "keep this user message"));
    assert_eq!(retry_messages.last().unwrap()["content"], COMPACTION_PROMPT);
}

#[tokio::test]
async fn test_compaction_caps_transient_failures_at_five_retries() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let responses = (0..=COMPACTION_MAX_TRANSIENT_RETRIES)
        .map(|_| {
            (
                "500 Internal Server Error",
                serde_json::json!({
                    "error": {
                        "message": "temporary overloaded"
                    }
                }),
            )
        })
        .collect();
    let config =
        test_config_for_base_url(compaction_sequence_url(Arc::clone(&requests), responses).await);
    let client = AgentClient::new();
    let mut session = AgentSession::new("compact-transient-fail");
    session.history = vec![ChatMessage::user("keep original history")];

    let error = compact_session(
        &client,
        &config,
        &mut session,
        CompactionTrigger::Auto,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        Some("base instructions"),
        InitialContextInjection::DoNotInject,
        Vec::new(),
        &CancellationToken::default_grace(),
    )
    .await
    .unwrap_err();

    assert!(error.contains("temporary overloaded"));
    assert_eq!(requests.lock().unwrap().len(), 6);
    assert_eq!(session.compaction_count, 0);
    assert_eq!(
        session.history[0].content.as_deref(),
        Some("keep original history")
    );
}

#[tokio::test]
async fn test_compaction_pre_shrinks_oversized_summary_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let config = test_config_for_base_url(
        compaction_sequence_url(
            Arc::clone(&requests),
            vec![("200 OK", chat_text_response("shrunk summary", 64))],
        )
        .await,
    );
    let client = AgentClient::new();
    let mut session = AgentSession::new("compact-pre-shrink");
    session.history = (0..20)
        .map(|idx| ChatMessage::user(&format!("old-{idx} {}", "x".repeat(400))))
        .collect();

    let result = compact_session(
        &client,
        &config,
        &mut session,
        CompactionTrigger::Auto,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        Some("base instructions"),
        InitialContextInjection::DoNotInject,
        Vec::new(),
        &CancellationToken::default_grace(),
    )
    .await
    .unwrap();

    assert!(result.performed);
    assert_eq!(session.compaction_count, 1);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request_messages = requests[0]["messages"].as_array().unwrap();
    assert!(request_messages.len() < 22);
    assert_eq!(request_messages[0]["content"], "base instructions");
    assert_eq!(
        request_messages.last().unwrap()["content"],
        COMPACTION_PROMPT
    );
    assert!(request_messages.iter().all(|message| !message["content"]
        .as_str()
        .unwrap_or("")
        .starts_with("old-0 ")));
    let estimated_tokens = request_messages
        .iter()
        .map(|message| approx_token_count(message["content"].as_str().unwrap_or("")))
        .sum::<usize>();
    assert!(estimated_tokens <= compaction_request_budget_tokens(&config));
}

#[test]
fn test_codex_compaction_templates_are_exact() {
    assert_eq!(
            COMPACTION_PROMPT,
            "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.\n\nInclude:\n- Current progress and key decisions made\n- Important context, constraints, or user preferences\n- What remains to be done (clear next steps)\n- Any critical data, examples, or references needed to continue\n\nBe concise, structured, and focused on helping the next LLM seamlessly continue the work.\n"
        );
    assert_eq!(
            SUMMARY_PREFIX,
            "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:"
        );
}

#[test]
fn test_collect_user_messages_preserves_all_real_users_within_budget() {
    let history = vec![
        ChatMessage::user("first"),
        ChatMessage::assistant("reply1"),
        ChatMessage::user("second"),
        ChatMessage::assistant("reply2"),
        ChatMessage::user("third"),
        ChatMessage::assistant("reply3"),
        ChatMessage::user("fourth"),
    ];
    let recent = collect_user_messages(&history);
    // Should include all real user messages that fit the budget, without the
    // assistant suffix between them.
    assert_eq!(recent.len(), 4);
    assert_eq!(recent[0], "first");
    assert_eq!(recent[1], "second");
    assert_eq!(recent[2], "third");
    assert_eq!(recent[3], "fourth");
}

#[test]
fn test_collect_user_messages_excludes_summary() {
    let history = vec![
        ChatMessage::user("real user question 1"),
        ChatMessage::assistant("reply2"),
        ChatMessage::user("real user question 2"),
        ChatMessage::user(&format!("{SUMMARY_PREFIX}\nprevious compaction summary")),
    ];
    let recent = collect_user_messages(&history);
    assert_eq!(recent, vec!["real user question 1", "real user question 2"]);
}

#[test]
fn test_collect_user_messages_filters_contextual_prompt_entries() {
    let history = vec![
        ChatMessage::user("<user_instructions>\nAGENTS.md\n</user_instructions>"),
        ChatMessage::user("<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"),
        ChatMessage::user("real user message"),
    ];

    let recent = collect_user_messages(&history);

    assert_eq!(recent, vec!["real user message"]);
}

#[test]
fn test_build_compacted_history_places_summary_after_recent_messages() {
    let recent = vec!["recent question".to_string()];

    let compacted = build_compacted_history(&recent, &format!("{SUMMARY_PREFIX}\nhandoff summary"));

    assert_eq!(compacted.len(), 2);
    assert_eq!(compacted[0].content.as_deref(), Some("recent question"));
    assert!(is_summary_message(&compacted[1]));
    assert!(compacted[1]
        .content
        .as_deref()
        .unwrap()
        .contains("handoff summary"));
}

#[test]
fn test_compaction_drops_tool_artifacts_from_replacement_messages() {
    let history = vec![
        ChatMessage::user("first"),
        ChatMessage::assistant("reply"),
        ChatMessage::user("inspect"),
        ChatMessage::assistant_with_tool_calls(vec![crate::types::ToolCallMessage::function_call(
            "call-1".to_string(),
            "read_file".to_string(),
            r#"{"path":"Cargo.toml"}"#.to_string(),
        )]),
        ChatMessage::tool_result("call-1", "content"),
        ChatMessage::assistant("done"),
    ];

    let recent = collect_user_messages(&history);

    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0], "first");
    assert_eq!(recent[1], "inspect");
}

#[test]
fn test_collect_user_messages_caps_preserved_user_budget() {
    let long_message = "x".repeat(10_000);
    let user_messages = vec!["older".to_string(), long_message];

    let compacted = build_compacted_history_with_limit(
        &user_messages,
        &format!("{SUMMARY_PREFIX}\nsummary"),
        16,
    );

    assert_eq!(compacted.len(), 2);
    let content = compacted[0].content.as_deref().unwrap();
    assert!(content.contains("tokens truncated"));
    assert!(approx_token_count(content) <= 32);
    assert!(is_summary_message(&compacted[1]));
}

#[test]
fn test_build_compacted_history_rebuilds_text_only_user_messages() {
    let image = crate::types::ChatImageInput {
        mime_type: "image/png".to_string(),
        data: "abc".to_string(),
    };
    let history = vec![ChatMessage::user_with_images("describe this", &[image])];

    let user_messages = collect_user_messages(&history);
    let compacted = build_compacted_history(&user_messages, &format!("{SUMMARY_PREFIX}\nsummary"));

    assert_eq!(compacted[0].role, "user");
    assert_eq!(compacted[0].content.as_deref(), Some("describe this"));
    assert!(compacted[0].content_parts.is_none());
}

#[test]
fn test_should_compact_adds_items_after_last_model_snapshot() {
    let mut session = AgentSession::new("compact-stale-response");
    let config = AgentConfig {
        model_context_window: Some(1_000),
        model_auto_compact_token_limit: Some(500),
        ..Default::default()
    };

    session
        .history
        .push(ChatMessage::assistant(&"x".repeat(3_000)));
    session.last_response_tokens = Some(100);
    session.last_response_history_len = Some(0);

    assert!(session.effective_token_count() > config.get_compact_threshold_tokens());
    assert!(should_compact(&session, &config));
}

#[test]
fn test_token_usage_snapshot_marks_stale_server_usage_as_mixed() {
    let mut session = AgentSession::new("mixed-token-source");
    session.history.push(ChatMessage::assistant("covered"));
    session.track_token_usage(100, 120);
    session
        .history
        .push(ChatMessage::user("new estimated context"));

    let snapshot = token_usage_snapshot(&session);

    assert!(snapshot.source.is_mixed());
    assert_eq!(
        snapshot.active_context_tokens,
        u64::from(session.effective_token_count())
    );
}

#[test]
fn test_is_summary_message() {
    let summary = ChatMessage::user(&format!("{SUMMARY_PREFIX}\nsome summary"));
    assert!(is_summary_message(&summary));

    let regular = ChatMessage::user("hello");
    assert!(!is_summary_message(&regular));

    let assistant = ChatMessage::assistant("reply");
    assert!(!is_summary_message(&assistant));
}

#[test]
fn test_insert_initial_context_before_last_real_user_message() {
    let history = vec![
        ChatMessage::user("older question"),
        ChatMessage::assistant("reply"),
        ChatMessage::user("recent question"),
        ChatMessage::user(&format!("{SUMMARY_PREFIX}\nsummary text")),
    ];
    let context = vec![ChatMessage::system("[context reminder]")];

    let result = insert_initial_context_before_last_user_message(history, context);

    // Context should be inserted before "recent question" (last real user message)
    assert_eq!(result.len(), 5);
    assert_eq!(result[0].content.as_deref(), Some("older question"));
    assert_eq!(result[1].role, "assistant"); // reply
    assert_eq!(result[2].role, "system"); // injected context
    assert_eq!(result[3].content.as_deref(), Some("recent question"));
    assert!(is_summary_message(&result[4]));
}

#[test]
fn test_insert_initial_context_only_summary() {
    // When only a summary user message exists, insert before it
    let history = vec![ChatMessage::user(&format!("{SUMMARY_PREFIX}\nsummary"))];
    let context = vec![ChatMessage::system("[context]")];

    let result = insert_initial_context_before_last_user_message(history, context);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].role, "system"); // injected context
    assert!(result[1]
        .content
        .as_ref()
        .unwrap()
        .starts_with(SUMMARY_PREFIX));
}

#[test]
fn test_insert_initial_context_empty_context_is_noop() {
    let history = vec![ChatMessage::user("hello")];
    let result = insert_initial_context_before_last_user_message(history.clone(), vec![]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].content.as_deref(), Some("hello"));
}

#[test]
fn test_insert_initial_context_no_user_messages() {
    // When there are no user messages at all, append to the end
    let history = vec![ChatMessage::assistant("reply")];
    let context = vec![ChatMessage::system("[context]")];

    let result = insert_initial_context_before_last_user_message(history, context);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].role, "assistant");
    assert_eq!(result[1].role, "system");
}

#[test]
fn test_insert_initial_context_multiple_context_items() {
    let history = vec![
        ChatMessage::user("question"),
        ChatMessage::user(&format!("{SUMMARY_PREFIX}\nsummary")),
    ];
    let context = vec![ChatMessage::system("[ctx1]"), ChatMessage::system("[ctx2]")];

    let result = insert_initial_context_before_last_user_message(history, context);

    // Both context items inserted before "question"
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].content.as_deref(), Some("[ctx1]"));
    assert_eq!(result[1].content.as_deref(), Some("[ctx2]"));
    assert_eq!(result[2].content.as_deref(), Some("question"));
    assert!(is_summary_message(&result[3]));
}

/// Verify that cancelling the token before compaction starts returns
/// the interrupted error immediately without calling the model.
#[tokio::test]
async fn test_compaction_interrupted_before_model_call() {
    use std::time::Duration;

    // Create a token and cancel it immediately
    let token = CancellationToken::new(Duration::from_secs(60));
    token.cancel();

    let config = AgentConfig {
        model: Some("test-model".to_string()),
        model_context_window: Some(1_000),
        model_auto_compact_token_limit: Some(100),
        ..Default::default()
    };
    let client = AgentClient::new();
    let mut session = AgentSession::new("compact-interrupted");
    session.history = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];

    let result = compact_session(
        &client,
        &config,
        &mut session,
        CompactionTrigger::Auto,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        None,
        InitialContextInjection::DoNotInject,
        Vec::new(),
        &token,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), COMPACTION_INTERRUPTED_ERROR);
    // Session should be unchanged — no compaction performed
    assert_eq!(session.compaction_count, 0);
    assert_eq!(session.history.len(), 2);
}

/// Verify that cancelling the token during a model call (slow server)
/// returns the interrupted error without waiting for completion.
#[tokio::test]
async fn test_compaction_interrupted_during_model_call() {
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    // Start a server that never responds (hangs)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Accept connection but never respond — simulates a slow model
            tokio::time::sleep(Duration::from_secs(30)).await;
            let _ = stream.shutdown().await;
        }
    });

    let mut model_providers = HashMap::new();
    model_providers.insert(
        "test".to_string(),
        ModelProviderConfig {
            name: Some("test".to_string()),
            base_url: Some(format!("http://{addr}/chat/completions")),
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
    let config = AgentConfig {
        model: Some("test-model".to_string()),
        model_provider: Some("test".to_string()),
        model_providers,
        model_context_window: Some(1_000),
        model_auto_compact_token_limit: Some(100),
        ..Default::default()
    };
    let client = AgentClient::new();
    let mut session = AgentSession::new("compact-interrupted-during");
    session.history = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];

    // Create token and cancel it after a short delay
    let token = CancellationToken::new(Duration::from_secs(60));
    let token_clone = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        token_clone.cancel();
    });

    let start = std::time::Instant::now();
    let result = compact_session(
        &client,
        &config,
        &mut session,
        CompactionTrigger::Auto,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        None,
        InitialContextInjection::DoNotInject,
        Vec::new(),
        &token,
    )
    .await;

    let elapsed = start.elapsed();

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), COMPACTION_INTERRUPTED_ERROR);
    // Should return quickly (well under the 30s server timeout)
    assert!(elapsed < Duration::from_secs(5));
    // Session should be unchanged
    assert_eq!(session.compaction_count, 0);
    assert_eq!(session.history.len(), 2);
}
