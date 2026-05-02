//! E2E tests for IM Gateway Agent Admin API endpoints.

use crate::assertions::assert_status;
use crate::{ProxyInstance, TestCase};
use bifrost_admin::{AdminState, ImGatewayService};
use bifrost_agent::config::{AgentConfig, ModelProviderConfig};
use bifrost_agent::persistence::{load_conversation, ConversationRecorder};
use bifrost_agent::session::{run_turn, run_turn_with_mcp, AgentSession};
use bifrost_agent::ToolRegistry;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "im_gateway_agent_config_get",
            "Validate GET /api/im-gateway/agent returns default config with expected fields",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/agent",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET agent config failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse agent config JSON: {}", e))?;

                // Verify expected fields exist
                if json.get("enabled").is_none() {
                    return Err("Expected 'enabled' field in agent config".to_string());
                }
                if json.get("model").is_none() {
                    return Err("Expected 'model' field in agent config".to_string());
                }
                if json.get("model_provider").is_none() {
                    return Err("Expected 'model_provider' field in agent config".to_string());
                }
                if json.get("model_providers").is_none() {
                    return Err("Expected 'model_providers' field in agent config".to_string());
                }
                if json.get("request_timeout_secs").is_none() {
                    return Err("Expected 'request_timeout_secs' field in agent config".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_config_patch",
            "Validate PATCH /api/im-gateway/agent updates config and persists changes",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                // First, patch the config
                let patch_body = serde_json::json!({
                    "enabled": false,
                    "model": "test-model-e2e",
                    "base_url": "https://test.example.com",
                    "api_key": "test-api-key-e2e"
                });

                let patch_response = client
                    .patch(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/agent",
                        port
                    ))
                    .json(&patch_body)
                    .send()
                    .await
                    .map_err(|e| format!("PATCH agent config failed: {}", e))?;

                assert_status(&patch_response, 200)?;

                // PATCH returns the updated full AgentConfig.
                let patch_json: serde_json::Value = patch_response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse patch response: {}", e))?;
                if patch_json.get("enabled").and_then(|v| v.as_bool()) != Some(false) {
                    return Err(format!(
                        "Expected updated config in patch response, got: {}",
                        serde_json::to_string_pretty(&patch_json).unwrap_or_default()
                    ));
                }

                // Now GET to verify the update persisted
                let get_response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/agent",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET agent config after patch failed: {}", e))?;

                assert_status(&get_response, 200)?;

                let json: serde_json::Value = get_response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse agent config JSON: {}", e))?;

                // Verify the patched values
                if json.get("enabled").and_then(|v| v.as_bool()) != Some(false) {
                    return Err(format!(
                        "Expected enabled: false, got: {:?}",
                        json.get("enabled")
                    ));
                }
                if json.get("model").and_then(|v| v.as_str()) != Some("test-model-e2e") {
                    return Err(format!(
                        "Expected model: 'test-model-e2e', got: {:?}",
                        json.get("model")
                    ));
                }
                let provider = json
                    .get("model_providers")
                    .and_then(|v| v.get("aidp_crawl"))
                    .ok_or("Expected aidp_crawl provider in model_providers")?;
                if provider.get("base_url").and_then(|v| v.as_str())
                    != Some("https://test.example.com")
                {
                    return Err(format!(
                        "Expected base_url: 'https://test.example.com', got: {:?}",
                        provider.get("base_url")
                    ));
                }
                if provider
                    .get("http_headers")
                    .and_then(|v| v.get("api-key"))
                    .and_then(|v| v.as_str())
                    != Some("test-api-key-e2e")
                {
                    return Err(format!(
                        "Expected api_key: 'test-api-key-e2e', got: {:?}",
                        provider.get("http_headers")
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_sessions_empty",
            "Validate GET /api/im-gateway/agent/sessions returns empty sessions list",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/agent/sessions",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET agent sessions failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse sessions JSON: {}", e))?;

                // Verify response has sessions array
                let sessions = json
                    .get("sessions")
                    .and_then(|v| v.as_array())
                    .ok_or("Expected 'sessions' array in response")?;

                if !sessions.is_empty() {
                    return Err(format!(
                        "Expected empty sessions array for fresh instance, got {} sessions",
                        sessions.len()
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_route_create",
            "Validate POST /api/im-gateway/routes creates an AgentChat route and GET verifies it",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                // Create an AgentChat route
                let route_body = serde_json::json!({
                    "id": "test-agent-route-1",
                    "provider_id": "test-provider",
                    "name": "Test Agent Route",
                    "enabled": true,
                    "event_type": "message_receive",
                    "matcher": {
                        "keyword": "agent"
                    },
                    "action": {
                        "type": "agent_chat",
                        "system_prompt": "You are a helpful assistant.",
                        "reply_target": "original_chat"
                    }
                });

                let create_response = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/routes",
                        port
                    ))
                    .json(&route_body)
                    .send()
                    .await
                    .map_err(|e| format!("POST route failed: {}", e))?;

                assert_status(&create_response, 200)?;

                // Verify the create response
                let create_json: serde_json::Value = create_response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse create response: {}", e))?;
                if create_json.get("success").and_then(|v| v.as_bool()) != Some(true) {
                    return Err(format!(
                        "Expected success: true in create response, got: {}",
                        serde_json::to_string_pretty(&create_json).unwrap_or_default()
                    ));
                }

                // GET routes to verify the route was created with correct action type
                let list_response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/routes",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET routes failed: {}", e))?;

                assert_status(&list_response, 200)?;

                let routes: serde_json::Value = list_response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse routes JSON: {}", e))?;

                let routes_array = routes.as_array().ok_or("Expected routes to be an array")?;

                // Find our created route
                let found_route = routes_array
                    .iter()
                    .find(|r| r.get("id").and_then(|v| v.as_str()) == Some("test-agent-route-1"));

                let route = found_route.ok_or("Created route not found in routes list")?;

                // Verify action type is "agent_chat"
                let action_type = route
                    .get("action")
                    .and_then(|a| a.get("type"))
                    .and_then(|v| v.as_str())
                    .ok_or("Expected action.type field in route")?;

                if action_type != "agent_chat" {
                    return Err(format!(
                        "Expected action.type: 'agent_chat', got: '{}'",
                        action_type
                    ));
                }

                // Verify other route fields
                if route.get("name").and_then(|v| v.as_str()) != Some("Test Agent Route") {
                    return Err(format!(
                        "Expected name: 'Test Agent Route', got: {:?}",
                        route.get("name")
                    ));
                }

                if route.get("provider_id").and_then(|v| v.as_str()) != Some("test-provider") {
                    return Err(format!(
                        "Expected provider_id: 'test-provider', got: {:?}",
                        route.get("provider_id")
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_long_term_memory_remember_recall",
            "Validate Codex-style file memories inject read-path instructions without SQLite",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let temp_dir = tempfile::tempdir()
                    .map_err(|e| format!("failed to create temp dir: {e}"))?;
                let _agent_home_guard = EnvVarGuard::set("BIFROST_AGENT_HOME", temp_dir.path());
                let memory_root = temp_dir.path().join("memory");
                std::fs::create_dir_all(&memory_root)
                    .map_err(|e| format!("failed to create memory root: {e}"))?;
                std::fs::write(
                    memory_root.join("memory_summary.md"),
                    "Bifrost should use Codex-style on-demand memory loading.",
                )
                .map_err(|e| format!("failed to write memory summary: {e}"))?;
                std::fs::write(
                    memory_root.join("MEMORY.md"),
                    "# Memory\n\n- Codex-style memory evidence lives here.\n",
                )
                .map_err(|e| format!("failed to write MEMORY.md: {e}"))?;

                let mut config = AgentConfig {
                    model: Some("mock-model".to_string()),
                    model_provider: Some("mock".to_string()),
                    work_dir: Some(std::env::current_dir().unwrap().display().to_string()),
                    memories: Some(bifrost_agent::config::MemoriesConfig {
                        use_memories: Some(true),
                        generate_memories: Some(false),
                        ..Default::default()
                    }),
                    ..AgentConfig::default()
                };
                config.model_providers.insert(
                    "mock".to_string(),
                    ModelProviderConfig {
                        name: Some("Mock".to_string()),
                        base_url: Some(mock.url()),
                        env_key: None,
                        api_key: None,
                        http_headers: Some(HashMap::from([(
                            "Authorization".to_string(),
                            "Bearer test".to_string(),
                        )])),
                        env_http_headers: None,
                        request_max_retries: None,
                        stream_idle_timeout_ms: None,
                        stream_max_retries: None,
                    },
                );

                let client = bifrost_agent::AgentClient::new();
                let tools = ToolRegistry::new();
                let mut second_session = AgentSession::new("session-file-memory");
                let result = run_turn(
                    &client,
                    &config,
                    &mut second_session,
                    &tools,
                    "需要时应该如何读取长期记忆？",
                    None,
                )
                .await
                .map_err(|e| format!("recall turn failed: {e}"))?;
                if result.response.is_empty() {
                    return Err("expected model response after recall".to_string());
                }

                let requests = mock.requests.lock();
                let recall_request = requests
                    .last()
                    .ok_or_else(|| "mock did not receive recall request".to_string())?;
                let messages = recall_request
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| "recall request missing messages".to_string())?;
                let injected = messages.iter().any(|message| {
                    message
                        .get("content")
                        .and_then(|value| value.as_str())
                        .map(|content| {
                            content.contains("## Memory")
                                && content.contains("memory_summary.md (already provided below; do NOT open again)")
                                && content.contains("MEMORY.md (searchable registry; primary file to query)")
                                && content.contains("Bifrost should use Codex-style on-demand memory loading.")
                                && content.contains("<oai-mem-citation>")
                        })
                        .unwrap_or(false)
                });
                if !injected {
                    return Err(format!("memory read-path instructions were not injected: {messages:?}"));
                }
                if memory_root.join("memories.sqlite").exists() {
                    return Err("file-backed memory path created memories.sqlite".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_auto_memory_new_session_consumes",
            "Validate generated file memory is loaded and consumed by a later fresh session",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let temp_dir = tempfile::tempdir()
                    .map_err(|e| format!("failed to create temp dir: {e}"))?;
                let _agent_home_guard = EnvVarGuard::set("BIFROST_AGENT_HOME", temp_dir.path());
                let memory_root = temp_dir.path().join("memory");

                let mut config = AgentConfig {
                    model: Some("mock-model".to_string()),
                    model_provider: Some("mock".to_string()),
                    work_dir: Some(std::env::current_dir().unwrap().display().to_string()),
                    memories: Some(bifrost_agent::config::MemoriesConfig {
                        use_memories: Some(true),
                        generate_memories: Some(true),
                        ..Default::default()
                    }),
                    ..AgentConfig::default()
                };
                config.model_providers.insert(
                    "mock".to_string(),
                    ModelProviderConfig {
                        name: Some("Mock".to_string()),
                        base_url: Some(mock.url()),
                        env_key: None,
                        api_key: None,
                        http_headers: Some(HashMap::from([(
                            "Authorization".to_string(),
                            "Bearer test".to_string(),
                        )])),
                        env_http_headers: None,
                        request_max_retries: None,
                        stream_idle_timeout_ms: None,
                        stream_max_retries: None,
                    },
                );

                let client = bifrost_agent::AgentClient::new();
                let tools = ToolRegistry::new();
                let mut first_session = AgentSession::new("auto-memory-source");
                let first = run_turn(
                    &client,
                    &config,
                    &mut first_session,
                    &tools,
                    "请记住：我的 Bifrost 项目代号是 MEM-AUTO-42。",
                    None,
                )
                .await
                .map_err(|e| format!("source turn failed: {e}"))?;
                if first.response.is_empty() {
                    return Err("expected source turn response".to_string());
                }

                let summary = std::fs::read_to_string(memory_root.join("memory_summary.md"))
                    .map_err(|e| format!("read generated memory_summary.md: {e}"))?;
                let memory = std::fs::read_to_string(memory_root.join("MEMORY.md"))
                    .map_err(|e| format!("read generated MEMORY.md: {e}"))?;
                let raw = std::fs::read_to_string(memory_root.join("raw_memories.md"))
                    .map_err(|e| format!("read generated raw_memories.md: {e}"))?;
                let rollout_count = std::fs::read_dir(memory_root.join("rollout_summaries"))
                    .map_err(|e| format!("read rollout_summaries: {e}"))?
                    .filter_map(Result::ok)
                    .count();
                if !summary.contains("MEM-AUTO-42")
                    || !memory.contains("MEM-AUTO-42")
                    || !raw.contains("MEM-AUTO-42")
                    || rollout_count == 0
                {
                    return Err(format!(
                        "auto memory was not persisted to Codex files; summary={summary:?}; memory={memory:?}; raw={raw:?}; rollout_count={rollout_count}"
                    ));
                }
                if memory_root.join("memories.sqlite").exists() {
                    return Err("auto memory path created memories.sqlite".to_string());
                }

                let recall_config = AgentConfig {
                    memories: Some(bifrost_agent::config::MemoriesConfig {
                        use_memories: Some(true),
                        generate_memories: Some(false),
                        ..Default::default()
                    }),
                    ..config.clone()
                };
                let mut second_session = AgentSession::new("auto-memory-consumer");
                let second = run_turn(
                    &client,
                    &recall_config,
                    &mut second_session,
                    &tools,
                    "这是新的对话。请根据长期记忆回答我的 Bifrost 项目代号。",
                    None,
                )
                .await
                .map_err(|e| format!("consumer turn failed: {e}"))?;
                if !second.response.contains("MEM-AUTO-42") {
                    return Err(format!(
                        "new session did not consume auto memory; response={:?}",
                        second.response
                    ));
                }

                let requests = mock.requests.lock();
                let consumer_request = requests
                    .last()
                    .ok_or_else(|| "mock did not receive consumer request".to_string())?;
                let consumer_messages = consumer_request
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| "consumer request missing messages".to_string())?;
                let loaded = consumer_messages.iter().any(|message| {
                    message
                        .get("content")
                        .and_then(|value| value.as_str())
                        .map(|content| {
                            content.contains("## Memory")
                                && content.contains("MEM-AUTO-42")
                                && content.contains("MEMORY.md (searchable registry; primary file to query)")
                        })
                        .unwrap_or(false)
                });
                if !loaded {
                    return Err(format!(
                        "new session request did not include generated memory instructions: {consumer_messages:?}"
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_tool_history_resume_regression",
            "Validate tool-call session persistence reloads into a valid Chat Completions message sequence",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let temp_dir = tempfile::tempdir()
                    .map_err(|e| format!("failed to create temp dir: {e}"))?;

                let mut config = AgentConfig {
                    model: Some("mock-model".to_string()),
                    model_provider: Some("mock".to_string()),
                    work_dir: Some(std::env::current_dir().unwrap().display().to_string()),
                    max_turn_iterations: Some(4),
                    request_timeout_secs: Some(20),
                    ..AgentConfig::default()
                };
                config.model_providers.insert(
                    "mock".to_string(),
                    ModelProviderConfig {
                        name: Some("Mock".to_string()),
                        base_url: Some(mock.url()),
                        env_key: None,
                        api_key: None,
                        http_headers: Some(HashMap::from([(
                            "Authorization".to_string(),
                            "Bearer test".to_string(),
                        )])),
                        env_http_headers: None,
                        request_max_retries: None,
                        stream_idle_timeout_ms: None,
                        stream_max_retries: None,
                    },
                );

                let client = bifrost_agent::AgentClient::new();
                let tools = ToolRegistry::with_defaults(5);
                let mut session = AgentSession::new("resume-tool-e2e");
                let mut recorder = ConversationRecorder::new(temp_dir.path(), "resume-tool-e2e");
                recorder
                    .record_session_start(
                        "resume-tool-e2e",
                        json!({"model": "mock-model", "provider": "mock", "source": "e2e"}),
                    )
                    .map_err(|e| format!("record session start failed: {e}"))?;

                let first = run_turn_with_mcp(
                    &client,
                    &config,
                    &mut session,
                    &tools,
                    None,
                    "list the current directory",
                    None,
                    Some(&mut recorder),
                )
                .await
                .map_err(|e| format!("first tool loop failed: {e}"))?;
                if first.tool_calls_log.is_empty() {
                    return Err("expected first turn to execute a tool call".to_string());
                }
                recorder.close();

                let restored = load_conversation(recorder.file_path())
                    .map_err(|e| format!("failed to reload conversation: {e}"))?;
                if !bifrost_agent::history::is_valid_chat_history(&restored) {
                    return Err("reloaded conversation history is malformed".to_string());
                }
                if !restored.iter().any(|m| m.role == "tool") {
                    return Err("expected restored history to include a legal tool result".to_string());
                }

                let mut resumed_session = AgentSession::new("resume-tool-e2e");
                resumed_session.history = restored;
                let second = run_turn(
                    &client,
                    &config,
                    &mut resumed_session,
                    &tools,
                    "continue and list the directory again",
                    None,
                )
                .await
                .map_err(|e| format!("resumed tool loop failed: {e}"))?;
                if second.tool_calls_log.is_empty() {
                    return Err("expected resumed turn to execute a tool call".to_string());
                }

                let requests = mock.requests.lock();
                if requests.len() < 4 {
                    return Err(format!(
                        "expected at least 4 model requests, got {}",
                        requests.len()
                    ));
                }
                if let Some(error) = requests.iter().find_map(validate_chat_messages_json) {
                    return Err(format!("mock observed malformed message history: {error}"));
                }

                Ok(())
            },
        ),
    ]
}

struct ChatCompletionMock {
    port: u16,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl ChatCompletionMock {
    async fn start() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("bind mock chat server: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("mock local addr: {e}"))?
            .port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_count = Arc::new(AtomicUsize::new(0));
        let requests_for_server = Arc::clone(&requests);
        let request_count_for_server = Arc::clone(&request_count);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let requests = Arc::clone(&requests_for_server);
                let request_count = Arc::clone(&request_count_for_server);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let requests = Arc::clone(&requests);
                        let request_count = Arc::clone(&request_count);
                        async move {
                            let current_call = request_count.fetch_add(1, Ordering::SeqCst) + 1;
                            let body_bytes = req
                                .into_body()
                                .collect()
                                .await
                                .map(|b| b.to_bytes())
                                .unwrap_or_else(|_| Bytes::new());
                            let body: serde_json::Value =
                                serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}));
                            requests.lock().push(body.clone());

                            if let Some(error) = validate_chat_messages_json(&body) {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::BAD_REQUEST)
                                        .header("Content-Type", "application/json")
                                        .body(Full::new(Bytes::from(
                                            json!({"error": {"message": error}}).to_string(),
                                        )))
                                        .unwrap(),
                                );
                            }

                            let is_memory_extract = request_messages_contain(
                                &body,
                                "You extract durable memories from a Bifrost Agent conversation",
                            );
                            let is_memory_consolidation = request_messages_contain(
                                &body,
                                "Bifrost memory consolidation agent",
                            );
                            let consumes_auto_memory = request_messages_contain(&body, "## Memory")
                                && request_messages_contain(&body, "MEM-AUTO-42")
                                && request_messages_contain(
                                    &body,
                                    "新的对话。请根据长期记忆回答我的 Bifrost 项目代号",
                                );
                            let has_tools = body
                                .get("tools")
                                .and_then(|value| value.as_array())
                                .map(|items| !items.is_empty())
                                .unwrap_or(false);
                            let last_role = body
                                .get("messages")
                                .and_then(|value| value.as_array())
                                .and_then(|messages| messages.last())
                                .and_then(|message| message.get("role"))
                                .and_then(|role| role.as_str());
                            let should_call_tool = has_tools
                                && last_role != Some("tool")
                                && !is_memory_extract
                                && !is_memory_consolidation;
                            let message = if is_memory_extract {
                                json!({
                                    "role": "assistant",
                                    "content": "{\"memories\":[\"User's Bifrost project code is MEM-AUTO-42.\"]}"
                                })
                            } else if is_memory_consolidation {
                                json!({
                                    "role": "assistant",
                                    "content": "{\"memory_summary\":\"- User's Bifrost project code is MEM-AUTO-42.\",\"memory\":\"# Memory\\n\\n- User's Bifrost project code is MEM-AUTO-42.\\n  source: phase2_consolidated\",\"skills\":[]}"
                                })
                            } else if consumes_auto_memory {
                                json!({
                                    "role": "assistant",
                                    "content": "我从长期记忆中读取到项目代号是 MEM-AUTO-42。"
                                })
                            } else if should_call_tool {
                                json!({
                                    "role": "assistant",
                                    "content": null,
                                    "tool_calls": [{
                                        "id": format!("call-{current_call}"),
                                        "type": "function",
                                        "function": {
                                            "name": "list_directory",
                                            "arguments": "{\"path\":\".\"}"
                                        }
                                    }]
                                })
                            } else {
                                json!({
                                    "role": "assistant",
                                    "content": format!("tool loop complete after request {current_call}")
                                })
                            };
                            let response = json!({
                                "choices": [{
                                    "message": message,
                                    "finish_reason": if should_call_tool { "tool_calls" } else { "stop" }
                                }],
                                "usage": {
                                    "prompt_tokens": 10,
                                    "completion_tokens": 5,
                                    "total_tokens": 15
                                }
                            });

                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("Content-Type", "application/json")
                                    .body(Full::new(Bytes::from(response.to_string())))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        Ok(Self { port, requests })
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/chat/completions", self.port)
    }
}

fn validate_chat_messages_json(body: &serde_json::Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;
    let mut pending: Vec<String> = Vec::new();
    for (idx, message) in messages.iter().enumerate() {
        match message.get("role").and_then(|v| v.as_str()) {
            Some("assistant") => {
                pending.clear();
                if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                    pending = tool_calls
                        .iter()
                        .filter_map(|tc| {
                            tc.get("id").and_then(|id| id.as_str()).map(str::to_string)
                        })
                        .collect();
                }
            }
            Some("tool") => {
                let Some(id) = message.get("tool_call_id").and_then(|v| v.as_str()) else {
                    return Some(format!("messages.[{idx}].tool_call_id missing"));
                };
                let Some(pos) = pending.iter().position(|pending_id| pending_id == id) else {
                    return Some(format!(
                        "messages.[{idx}].role=tool has no preceding assistant tool_calls"
                    ));
                };
                pending.remove(pos);
            }
            Some(_) => pending.clear(),
            None => return Some(format!("messages.[{idx}].role missing")),
        }
    }
    if !pending.is_empty() {
        return Some("assistant tool_calls were not followed by tool results".to_string());
    }
    None
}

fn request_messages_contain(body: &serde_json::Value, needle: &str) -> bool {
    body.get("messages")
        .and_then(|value| value.as_array())
        .map(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(|value| value.as_str())
                    .map(|content| content.contains(needle))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

async fn start_im_gateway_admin(port: u16) -> Result<(ProxyInstance, Arc<AdminState>), String> {
    let (proxy, admin_state) = ProxyInstance::start_with_admin(port, vec![], false, true)
        .await
        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;
    let data_dir = std::env::temp_dir().join(format!("bifrost_e2e_im_gateway_agent_{port}"));
    admin_state.set_im_gateway_service(Arc::new(ImGatewayService::new(&data_dir)));
    Ok((proxy, admin_state))
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {}", e))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {}", e))
}

struct EnvVarGuard {
    key: String,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self {
            key: key.to_string(),
            previous,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(&self.key, value),
            None => std::env::remove_var(&self.key),
        }
    }
}
