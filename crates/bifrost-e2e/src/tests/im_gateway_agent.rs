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

                            let message = if current_call % 2 == 1 {
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
                                    "finish_reason": if current_call % 2 == 1 { "tool_calls" } else { "stop" }
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
