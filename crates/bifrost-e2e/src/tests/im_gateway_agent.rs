//! E2E tests for IM Gateway Agent Admin API endpoints.

use crate::assertions::assert_status;
use crate::{ProxyInstance, TestCase};
use bifrost_admin::{AdminState, ImGatewayService};
use bifrost_agent::config::{AgentConfig, MemoriesConfig, ModelProviderConfig};
use bifrost_agent::memory_prompts::{CONSOLIDATION_SYSTEM_PROMPT, EXTRACT_SYSTEM_PROMPT};
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
    let tests = vec![
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
                if json
                    .get("default_base_instructions")
                    .and_then(|v| v.as_str())
                    .filter(|v| v.contains("Bifrost Agent"))
                    .is_none()
                {
                    return Err(format!(
                        "Expected default_base_instructions to include built-in prompt, got: {:?}",
                        json.get("default_base_instructions")
                    ));
                }
                if json
                    .get("effective_base_instructions")
                    .and_then(|v| v.as_str())
                    .filter(|v| v.contains("Bifrost Agent"))
                    .is_none()
                {
                    return Err(format!(
                        "Expected effective_base_instructions to fall back to built-in prompt, got: {:?}",
                        json.get("effective_base_instructions")
                    ));
                }
                if json.get("request_timeout_secs").and_then(|v| v.as_u64()) != Some(600) {
                    return Err(format!(
                        "Expected request_timeout_secs: 600, got: {:?}",
                        json.get("request_timeout_secs")
                    ));
                }
                if json.get("max_turn_iterations").and_then(|v| v.as_u64()) != Some(1000) {
                    return Err(format!(
                        "Expected max_turn_iterations: 1000, got: {:?}",
                        json.get("max_turn_iterations")
                    ));
                }
                if json
                    .get("background_terminal_max_timeout")
                    .and_then(|v| v.as_u64())
                    != Some(300_000)
                {
                    return Err(format!(
                        "Expected background_terminal_max_timeout: 300000, got: {:?}",
                        json.get("background_terminal_max_timeout")
                    ));
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
                    "api_key": "test-api-key-e2e",
                    "model_reasoning_effort": "none",
                    "model_reasoning_summary": "none",
                    "base_instructions": "Base prompt e2e",
                    "developer_instructions": "Developer prompt e2e",
                    "user_instructions": "User prompt e2e"
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
                if json.get("base_instructions").and_then(|v| v.as_str())
                    != Some("Base prompt e2e")
                {
                    return Err(format!("Expected base_instructions to be patched, got: {json}"));
                }
                if json.get("developer_instructions").and_then(|v| v.as_str())
                    != Some("Developer prompt e2e")
                {
                    return Err(format!(
                        "Expected developer_instructions to be patched, got: {json}"
                    ));
                }
                if json.get("user_instructions").and_then(|v| v.as_str())
                    != Some("User prompt e2e")
                {
                    return Err(format!("Expected user_instructions to be patched, got: {json}"));
                }
                if json.get("model_reasoning_effort").and_then(|v| v.as_str()) != Some("none") {
                    return Err(format!(
                        "Expected model_reasoning_effort none to be patched, got: {json}"
                    ));
                }
                if json.get("model_reasoning_summary").and_then(|v| v.as_str()) != Some("none") {
                    return Err(format!(
                        "Expected model_reasoning_summary none to be patched, got: {json}"
                    ));
                }
                if json
                    .get("effective_base_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Base prompt e2e")
                {
                    return Err(format!(
                        "Expected effective_base_instructions to use override, got: {json}"
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

                let clear_response = client
                    .patch(format!(
                        "http://127.0.0.1:{}/_bifrost/api/im-gateway/agent",
                        port
                    ))
                    .json(&serde_json::json!({
                        "base_instructions": "",
                        "developer_instructions": "",
                        "user_instructions": ""
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH clear agent instructions failed: {e}"))?;
                assert_status(&clear_response, 200)?;
                let cleared: serde_json::Value = clear_response
                    .json()
                    .await
                    .map_err(|e| format!("parse cleared agent config failed: {e}"))?;
                if !cleared.get("base_instructions").is_none_or(|v| v.is_null())
                    || !cleared
                        .get("developer_instructions")
                        .is_none_or(|v| v.is_null())
                    || !cleared.get("user_instructions").is_none_or(|v| v.is_null())
                    || !cleared
                        .get("effective_base_instructions")
                        .and_then(|v| v.as_str())
                        .is_some_and(|v| v.contains("Bifrost Agent"))
                {
                    return Err(format!(
                        "Expected empty strings to clear instruction overrides, got: {cleared}"
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_provider_agent_config_overrides",
            "Validate IM Provider create/PATCH persists provider-level agent work_dir and instructions",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;
                let base = format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway");

                let create_body = serde_json::json!({
                    "id": "agent-config-provider",
                    "provider_type": "feishu",
                    "display_name": "Agent Config Provider",
                    "enabled": true,
                    "event_connection_enabled": true,
                    "event_types": [],
                    "agent_config": {
                        "work_dir": "/tmp/provider-workdir",
                        "base_instructions": "Provider prompt",
                        "developer_instructions": "Provider developer prompt",
                        "user_instructions": "Provider user prompt"
                    }
                });
                let create_response = client
                    .post(format!("{base}/providers"))
                    .json(&create_body)
                    .send()
                    .await
                    .map_err(|e| format!("POST provider failed: {e}"))?;
                assert_status(&create_response, 200)?;

                let get_response = client
                    .get(format!("{base}/providers/agent-config-provider"))
                    .send()
                    .await
                    .map_err(|e| format!("GET provider failed: {e}"))?;
                assert_status(&get_response, 200)?;
                let created: serde_json::Value = get_response
                    .json()
                    .await
                    .map_err(|e| format!("parse provider JSON failed: {e}"))?;
                if created.get("secret_ref").is_some() {
                    return Err(format!("Provider response leaked secret_ref: {created}"));
                }
                if created
                    .pointer("/agent_config/work_dir")
                    .and_then(|v| v.as_str())
                    != Some("/tmp/provider-workdir")
                {
                    return Err(format!(
                        "Expected provider work_dir override, got: {created}"
                    ));
                }
                if created
                    .pointer("/agent_config/base_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Provider prompt")
                {
                    return Err(format!(
                        "Expected provider instructions override, got: {created}"
                    ));
                }
                if created
                    .pointer("/agent_config/developer_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Provider developer prompt")
                {
                    return Err(format!(
                        "Expected provider developer instructions override, got: {created}"
                    ));
                }
                if created
                    .pointer("/agent_config/user_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Provider user prompt")
                {
                    return Err(format!(
                        "Expected provider user instructions override, got: {created}"
                    ));
                }

                let patch_body = serde_json::json!({
                    "agent_config": {
                        "work_dir": "/tmp/provider-patched",
                        "base_instructions": "Provider prompt patched",
                        "developer_instructions": "Provider developer patched",
                        "user_instructions": "Provider user patched"
                    }
                });
                let patch_response = client
                    .patch(format!("{base}/providers/agent-config-provider"))
                    .json(&patch_body)
                    .send()
                    .await
                    .map_err(|e| format!("PATCH provider failed: {e}"))?;
                assert_status(&patch_response, 200)?;

                let patched: serde_json::Value = client
                    .get(format!("{base}/providers/agent-config-provider"))
                    .send()
                    .await
                    .map_err(|e| format!("GET patched provider failed: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("parse patched provider failed: {e}"))?;
                if patched
                    .pointer("/agent_config/work_dir")
                    .and_then(|v| v.as_str())
                    != Some("/tmp/provider-patched")
                {
                    return Err(format!("Expected patched work_dir, got: {patched}"));
                }
                if patched
                    .pointer("/agent_config/base_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Provider prompt patched")
                {
                    return Err(format!("Expected patched instructions, got: {patched}"));
                }
                if patched
                    .pointer("/agent_config/developer_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Provider developer patched")
                {
                    return Err(format!("Expected patched developer instructions, got: {patched}"));
                }
                if patched
                    .pointer("/agent_config/user_instructions")
                    .and_then(|v| v.as_str())
                    != Some("Provider user patched")
                {
                    return Err(format!("Expected patched user instructions, got: {patched}"));
                }

                let clear_base_response = client
                    .patch(format!("{base}/providers/agent-config-provider"))
                    .json(&serde_json::json!({
                        "agent_config": {
                            "base_instructions": null
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH provider clear base failed: {e}"))?;
                assert_status(&clear_base_response, 200)?;
                let cleared_base: serde_json::Value = client
                    .get(format!("{base}/providers/agent-config-provider"))
                    .send()
                    .await
                    .map_err(|e| format!("GET provider after base clear failed: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("parse provider after base clear failed: {e}"))?;
                if cleared_base.pointer("/agent_config/base_instructions").is_some()
                    || cleared_base
                        .pointer("/agent_config/developer_instructions")
                        .and_then(|v| v.as_str())
                        != Some("Provider developer patched")
                    || cleared_base
                        .pointer("/agent_config/user_instructions")
                        .and_then(|v| v.as_str())
                        != Some("Provider user patched")
                {
                    return Err(format!(
                        "Expected clearing provider base to preserve developer/user, got: {cleared_base}"
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_chat_codex_prompt_layers",
            "Validate POST /api/im-gateway/agent/chat sends system/developer/user prompt layers",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;
                let base = format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway");

                let patch_response = client
                    .patch(format!("{base}/agent"))
                    .json(&serde_json::json!({
                        "enabled": true,
                        "model": "mock-model",
                        "model_provider": "mock",
                        "base_instructions": "PROMPT_SPLIT_BASE_OK",
                        "developer_instructions": "PROMPT_SPLIT_DEVELOPER_OK",
                        "user_instructions": "PROMPT_SPLIT_USER_OK",
                        "max_turn_iterations": 4,
                        "request_timeout_secs": 20,
                        "memories": {
                            "use_memories": false,
                            "generate_memories": false
                        },
                        "model_providers": {
                            "mock": {
                                "name": "Mock",
                                "base_url": mock.url(),
                                "api_key": "test"
                            }
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH agent config failed: {e}"))?;
                assert_status(&patch_response, 200)?;

                let chat_response = client
                    .post(format!("{base}/agent/chat"))
                    .json(&serde_json::json!({
                        "session_key": "prompt-split-e2e",
                        "message": "PROMPT_SPLIT_CHAT_E2E",
                        "work_dir": std::env::current_dir()
                            .map_err(|e| format!("current_dir failed: {e}"))?
                            .display()
                            .to_string()
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST agent chat failed: {e}"))?;
                assert_status(&chat_response, 200)?;
                let chat_json: serde_json::Value = chat_response
                    .json()
                    .await
                    .map_err(|e| format!("parse chat response failed: {e}"))?;
                if chat_json.get("success").and_then(|v| v.as_bool()) != Some(true)
                    || !chat_json
                        .get("response")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .contains("PROMPT_SPLIT_CHAT_E2E_OK")
                {
                    return Err(format!("Expected prompt split chat success, got: {chat_json}"));
                }

                let sessions: serde_json::Value = client
                    .get(format!("{base}/agent/sessions"))
                    .send()
                    .await
                    .map_err(|e| format!("GET sessions failed: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("parse sessions failed: {e}"))?;
                let session = sessions
                    .get("sessions")
                    .and_then(|v| v.as_array())
                    .and_then(|items| {
                        items.iter().find(|item| {
                            item.get("session_key").and_then(|v| v.as_str())
                                == Some("prompt-split-e2e")
                        })
                    })
                    .ok_or_else(|| format!("prompt split session missing: {sessions}"))?;
                if session.get("message_count").and_then(|v| v.as_u64()) != Some(2) {
                    return Err(format!("Expected chat session to record 2 messages, got: {session}"));
                }

                let requests = mock.requests.lock();
                let request = requests
                    .last()
                    .ok_or_else(|| "mock did not receive chat request".to_string())?;
                let messages = request
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| format!("mock request missing messages: {request}"))?;
                let roles: Vec<&str> = messages
                    .iter()
                    .filter_map(|message| message.get("role").and_then(|value| value.as_str()))
                    .collect();
                if roles.get(0..4) != Some(&["system", "developer", "user", "user"][..]) {
                    return Err(format!("Expected layered prompt roles, got: {roles:?}"));
                }
                let joined = messages
                    .iter()
                    .filter_map(|message| message.get("content").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                for needle in [
                    "PROMPT_SPLIT_BASE_OK",
                    "PROMPT_SPLIT_DEVELOPER_OK",
                    "PROMPT_SPLIT_USER_OK",
                    "PROMPT_SPLIT_CHAT_E2E",
                ] {
                    if !joined.contains(needle) {
                        return Err(format!("Prompt split request missing {needle}: {messages:?}"));
                    }
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_chat_stop_active_loop",
            "Validate /agent/chat accepts /stop and cancels an active agent loop",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;
                let base = format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway");

                let patch_response = client
                    .patch(format!("{base}/agent"))
                    .json(&serde_json::json!({
                        "enabled": true,
                        "model": "mock-model",
                        "model_provider": "mock",
                        "max_turn_iterations": 4,
                        "request_timeout_secs": 60,
                        "memories": {
                            "use_memories": false,
                            "generate_memories": false
                        },
                        "model_providers": {
                            "mock": {
                                "name": "Mock",
                                "base_url": mock.url(),
                                "api_key": "test"
                            }
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH agent config failed: {e}"))?;
                assert_status(&patch_response, 200)?;

                let chat_client = client.clone();
                let chat_base = base.clone();
                let chat_handle = tokio::spawn(async move {
                    chat_client
                        .post(format!("{chat_base}/agent/chat"))
                        .json(&serde_json::json!({
                            "session_key": "stop-loop-e2e",
                            "message": "AGENT_STOP_E2E please keep this model request open"
                        }))
                        .send()
                        .await
                        .map_err(|e| format!("POST slow agent chat failed: {e}"))
                });

                let started_at = std::time::Instant::now();
                loop {
                    let seen = mock
                        .requests
                        .lock()
                        .iter()
                        .any(|body| request_messages_contain(body, "AGENT_STOP_E2E"));
                    if seen {
                        break;
                    }
                    if started_at.elapsed() > std::time::Duration::from_secs(5) {
                        return Err("mock did not receive the slow stop E2E request".to_string());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }

                let status_response = client
                    .post(format!("{base}/agent/chat"))
                    .json(&serde_json::json!({
                        "session_key": "stop-loop-e2e",
                        "message": "/status"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST active status failed: {e}"))?;
                assert_status(&status_response, 200)?;
                let status_json: serde_json::Value = status_response
                    .json()
                    .await
                    .map_err(|e| format!("parse active status failed: {e}"))?;
                if status_json.get("active_status").is_none() {
                    return Err(format!("Expected active_status while loop is running, got: {status_json}"));
                }

                let stop_response = client
                    .post(format!("{base}/agent/chat"))
                    .json(&serde_json::json!({
                        "session_key": "stop-loop-e2e",
                        "message": "/stop"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST stop failed: {e}"))?;
                assert_status(&stop_response, 200)?;
                let stop_json: serde_json::Value = stop_response
                    .json()
                    .await
                    .map_err(|e| format!("parse stop response failed: {e}"))?;
                if stop_json.get("stopped").and_then(|value| value.as_bool()) != Some(true) {
                    return Err(format!("Expected /stop to signal active loop, got: {stop_json}"));
                }

                let chat_response =
                    tokio::time::timeout(std::time::Duration::from_secs(5), chat_handle)
                        .await
                        .map_err(|_| "slow chat did not return promptly after /stop".to_string())?
                        .map_err(|e| format!("slow chat task join failed: {e}"))??;
                assert_status(&chat_response, 200)?;
                let chat_json: serde_json::Value = chat_response
                    .json()
                    .await
                    .map_err(|e| format!("parse stopped chat response failed: {e}"))?;
                if chat_json.get("success").and_then(|value| value.as_bool()) != Some(true)
                    || !chat_json
                        .get("response")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .contains("/stop")
                {
                    return Err(format!("Expected stopped chat response, got: {chat_json}"));
                }

                let followup_response = client
                    .post(format!("{base}/agent/chat"))
                    .json(&serde_json::json!({
                        "session_key": "stop-loop-e2e",
                        "message": "after stop, respond normally"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST follow-up chat failed: {e}"))?;
                assert_status(&followup_response, 200)?;
                let followup_json: serde_json::Value = followup_response
                    .json()
                    .await
                    .map_err(|e| format!("parse follow-up chat failed: {e}"))?;
                if followup_json.get("success").and_then(|value| value.as_bool()) != Some(true)
                {
                    return Err(format!("Expected follow-up after /stop to succeed, got: {followup_json}"));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_chat_queue_state_persists_for_refresh",
            "Validate Web Agent Chat queue state is served from backend sessions APIs",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;
                let base = format!("http://127.0.0.1:{port}/_bifrost/api");
                let gateway_base = format!("{base}/im-gateway");
                let session_key = "web-queue-refresh-e2e";

                let patch_response = client
                    .patch(format!("{gateway_base}/agent"))
                    .json(&serde_json::json!({
                        "enabled": true,
                        "model": "mock-model",
                        "model_provider": "mock",
                        "max_turn_iterations": 2,
                        "request_timeout_secs": 60,
                        "memories": {
                            "use_memories": false,
                            "generate_memories": false
                        },
                        "model_providers": {
                            "mock": {
                                "name": "Mock",
                                "base_url": mock.url(),
                                "api_key": "test"
                            }
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH agent config failed: {e}"))?;
                assert_status(&patch_response, 200)?;

                let chat_client = client.clone();
                let chat_base = base.clone();
                let chat_handle = tokio::spawn(async move {
                    chat_client
                        .post(format!("{chat_base}/agent/chat/stream"))
                        .json(&serde_json::json!({
                            "session_key": session_key,
                            "message": "AGENT_QUEUE_REFRESH_E2E keep this model request open"
                        }))
                        .send()
                        .await
                        .map_err(|e| format!("POST slow agent chat stream failed: {e}"))
                });

                let started_at = std::time::Instant::now();
                loop {
                    let seen = mock
                        .requests
                        .lock()
                        .iter()
                        .any(|body| request_messages_contain(body, "AGENT_QUEUE_REFRESH_E2E"));
                    if seen {
                        break;
                    }
                    if started_at.elapsed() > std::time::Duration::from_secs(5) {
                        return Err("mock did not receive queue refresh request".to_string());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }

                let queue_response = client
                    .post(format!("{base}/agent/chat/stream"))
                    .json(&serde_json::json!({
                        "session_key": session_key,
                        "message": "/q queued follow-up from web ui"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST queue stream failed: {e}"))?;
                assert_status(&queue_response, 200)?;
                let queue_text = queue_response
                    .text()
                    .await
                    .map_err(|e| format!("read queue stream failed: {e}"))?;
                if !queue_text.contains("\"queueLength\":1")
                    || !queue_text.contains("queued follow-up from web ui")
                {
                    return Err(format!("Expected queue SSE to include backend queue items, got: {queue_text}"));
                }

                let sessions_response = client
                    .get(format!("{gateway_base}/agent/sessions/all"))
                    .send()
                    .await
                    .map_err(|e| format!("GET sessions/all failed: {e}"))?;
                assert_status(&sessions_response, 200)?;
                let sessions_json: serde_json::Value = sessions_response
                    .json()
                    .await
                    .map_err(|e| format!("parse sessions/all failed: {e}"))?;
                let session = sessions_json
                    .get("sessions")
                    .and_then(|value| value.as_array())
                    .and_then(|items| {
                        items.iter().find(|item| {
                            item.get("session_key").and_then(|value| value.as_str())
                                == Some(session_key)
                        })
                    })
                    .ok_or_else(|| format!("queued active session missing: {sessions_json}"))?;
                assert_queue_item(session, "queued follow-up from web ui")?;

                let detail_response = client
                    .get(format!(
                        "{gateway_base}/agent/sessions/{}",
                        urlencoding::encode(session_key)
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET session detail failed: {e}"))?;
                assert_status(&detail_response, 200)?;
                let detail_json: serde_json::Value = detail_response
                    .json()
                    .await
                    .map_err(|e| format!("parse session detail failed: {e}"))?;
                assert_queue_item(&detail_json, "queued follow-up from web ui")?;

                let stop_response = client
                    .post(format!("{gateway_base}/agent/chat"))
                    .json(&serde_json::json!({
                        "session_key": session_key,
                        "message": "/stop"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST stop for queue refresh failed: {e}"))?;
                assert_status(&stop_response, 200)?;
                let chat_response =
                    tokio::time::timeout(std::time::Duration::from_secs(5), chat_handle)
                        .await
                        .map_err(|_| "slow queue refresh chat did not return after /stop".to_string())?
                        .map_err(|e| format!("queue refresh chat task join failed: {e}"))??;
                assert_status(&chat_response, 200)?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_long_task_runtime_watch",
            "Validate exec_command long tasks are watched by runtime without model polling",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start_long_exec().await?;
                let mut model_providers = HashMap::new();
                model_providers.insert(
                    "mock".to_string(),
                    ModelProviderConfig {
                        name: Some("Mock".to_string()),
                        base_url: Some(mock.url()),
                        wire_api: Some(bifrost_agent::ModelWireApi::ChatCompletions),
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
                    enabled: true,
                    model: Some("mock-model".to_string()),
                    model_provider: Some("mock".to_string()),
                    model_providers,
                    memories: Some(MemoriesConfig {
                        generate_memories: Some(false),
                        use_memories: Some(false),
                        ..Default::default()
                    }),
                    request_timeout_secs: Some(30),
                    ..Default::default()
                };
                let client = bifrost_agent::AgentClient::new();
                let tools = ToolRegistry::with_defaults();
                let mut session = AgentSession::new("long-task-runtime-watch-e2e");

                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    run_turn(
                        &client,
                        &config,
                        &mut session,
                        &tools,
                        "LONG_TASK_RUNTIME_WATCH_E2E",
                        None,
                    ),
                )
                .await
                .map_err(|_| "long task runtime watch timed out".to_string())?
                .map_err(|e| format!("run_turn failed: {e}"))?;

                if !result.response.contains("LONG_TASK_RUNTIME_WATCH_E2E_OK") {
                    return Err(format!("unexpected final response: {}", result.response));
                }
                let request_count = mock.requests.lock().len();
                if request_count != 2 {
                    return Err(format!(
                        "expected exactly 2 model requests, got {request_count}"
                    ));
                }
                let tool_output = session
                    .history
                    .iter()
                    .find(|message| message.role == "tool")
                    .and_then(|message| message.content.as_deref())
                    .ok_or_else(|| "missing exec_command tool result".to_string())?;
                if !tool_output.contains("process_exited")
                    || !tool_output.contains("start")
                    || !tool_output.contains("done")
                    || !tool_output.contains("model_request_count_while_waiting")
                {
                    return Err(format!("unexpected long task tool output: {tool_output}"));
                }
                if !session.last_turn_events.iter().any(|event| {
                    event.kind == bifrost_agent::turn_runtime::CodexTurnEventKind::TurnSuspended
                }) {
                    return Err("missing TurnSuspended event".to_string());
                }
                if !session.last_turn_events.iter().any(|event| {
                    event.kind == bifrost_agent::turn_runtime::CodexTurnEventKind::LongTaskExited
                }) {
                    return Err("missing LongTaskExited event".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_streaming_progress_card_renderer",
            "Validate IM Agent progress snapshot renders Feishu streaming card sections and guide/queue footer state",
            "admin",
            || async move {
                use bifrost_admin::im_gateway::progress_card::{
                    build_feishu_progress_card, ImAgentProgressSnapshot,
                };
                use bifrost_admin::im_gateway::queue_manager::QueueItem;
                use bifrost_agent::tools::update_plan::{PlanStep, PlanStepStatus};

                let mut snapshot =
                    ImAgentProgressSnapshot::new("provider:owner", "run streaming card e2e");
                snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::AssistantDelta {
                    content: "checking progress card sections".to_string(),
                });
                snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::ToolStarted {
                    tool_name: "list_directory".to_string(),
                    arguments: "{\"path\":\".\"}".to_string(),
                });
                snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::ToolFinished {
                    log: bifrost_agent::ToolCallLog {
                        tool_name: "list_directory".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                        result: "Cargo.toml\ncrates".to_string(),
                        success: true,
                    },
                    duration_ms: 37,
                });
                snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::PlanUpdated {
                    title: Some("Streaming Progress".to_string()),
                    steps: vec![PlanStep {
                        step: "Render latest status card".to_string(),
                        status: PlanStepStatus::InProgress,
                    }],
                });
                snapshot.update_queue_state(
                    vec![QueueItem {
                        seq: 1,
                        message: "next queued message".to_string(),
                    }],
                    true,
                    Some("已收到引导：check latest logs".to_string()),
                );
                snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                    content: "streaming card done".to_string(),
                });

                let card = build_feishu_progress_card(&snapshot, true);
                if card["schema"] != "2.0" {
                    return Err(format!("expected card JSON 2.0, got {card}"));
                }
                if card["config"]["streaming_mode"] != true {
                    return Err(format!("expected streaming_mode=true, got {card}"));
                }
                let body = card["body"]["elements"].to_string();
                for needle in [
                    "agent_output",
                    "agent_plan_panel",
                    "agent_plan",
                    "agent_tool_panel",
                    "agent_tool_log",
                    "agent_status_panel",
                    "agent_footer",
                    "agent_thinking_panel",
                    "agent_thinking",
                    "list_directory",
                    "37ms",
                    "1 条排队消息",
                    "有待处理引导消息",
                    "已收到引导：check latest logs",
                    "任务计划：Render latest status card",
                    "思考过程：checking progress card sections",
                    "checking progress card sections",
                ] {
                    if !body.contains(needle) {
                        return Err(format!("streaming card body missing {needle}: {body}"));
                    }
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
            "im_gateway_agent_chat_multimodal_image_parts",
            "Validate POST /api/im-gateway/agent/chat forwards image attachments as multimodal content parts",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) = start_im_gateway_admin(port).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;
                let base = format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway");

                let patch_response = client
                    .patch(format!("{base}/agent"))
                    .json(&serde_json::json!({
                        "enabled": true,
                        "model": "mock-vision-model",
                        "model_provider": "mock",
                        "base_instructions": "You can understand images.",
                        "max_turn_iterations": 1,
                        "request_timeout_secs": 20,
                        "memories": {
                            "use_memories": false,
                            "generate_memories": false
                        },
                        "model_providers": {
                            "mock": {
                                "name": "Mock",
                                "base_url": mock.url(),
                                "api_key": "test"
                            }
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH agent config failed: {e}"))?;
                assert_status(&patch_response, 200)?;

                let chat_response = client
                    .post(format!("{base}/agent/chat"))
                    .json(&serde_json::json!({
                        "session_key": "multimodal-image-e2e",
                        "message": "MULTIMODAL_IMAGE_E2E 请描述这张图片",
                        "images": [{
                            "mime_type": "image/png",
                            "data": "iVBORw0KGgo="
                        }]
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST multimodal agent chat failed: {e}"))?;
                assert_status(&chat_response, 200)?;
                let chat_json: serde_json::Value = chat_response
                    .json()
                    .await
                    .map_err(|e| format!("parse chat response failed: {e}"))?;
                if chat_json.get("success").and_then(|v| v.as_bool()) != Some(true)
                    || !chat_json
                        .get("response")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .contains("MULTIMODAL_IMAGE_E2E_OK")
                {
                    return Err(format!("Expected multimodal chat success, got: {chat_json}"));
                }

                {
                    let requests = mock.requests.lock();
                    let request = requests
                        .last()
                        .ok_or_else(|| "mock did not receive chat request".to_string())?;
                    if !request_contains_image_url(request) {
                        return Err(format!("mock request missing image_url part: {request}"));
                    }
                }

                let session_detail: serde_json::Value = client
                    .get(format!("{base}/agent/sessions/multimodal-image-e2e"))
                    .send()
                    .await
                    .map_err(|e| format!("GET multimodal session detail failed: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("parse multimodal session detail failed: {e}"))?;
                let user_message = session_detail
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .and_then(|messages| messages.iter().find(|message| {
                        message.get("role").and_then(|value| value.as_str()) == Some("user")
                    }))
                    .ok_or_else(|| format!("multimodal user message missing: {session_detail}"))?;
                if !user_message
                    .get("content_parts")
                    .and_then(|value| value.as_array())
                    .map(|parts| parts.iter().any(|part| {
                        part.get("type").and_then(|value| value.as_str()) == Some("image_url")
                    }))
                    .unwrap_or(false)
                {
                    return Err(format!(
                        "session detail missing persisted image content parts: {session_detail}"
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
            "im_gateway_agent_model_request_uses_bifrost_proxy",
            "Validate Agent model HTTP requests are proxied through the running Bifrost port and recorded in Traffic",
            "admin",
            || async move {
                let proxy_port = pick_unused_port()?;
                let mock = ChatCompletionMock::start().await?;
                let mock_host = "model-proxy.test";
                let mock_base_url = format!("http://{mock_host}/chat/completions");
                let host_rule = format!("{mock_host} host://127.0.0.1:{}", mock.port);
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(proxy_port, vec![host_rule.as_str()], false, true)
                        .await
                        .map_err(|e| format!("failed to start Bifrost proxy: {e}"))?;

                let mut config = AgentConfig {
                    model: Some("mock-model".to_string()),
                    model_provider: Some("mock".to_string()),
                    request_timeout_secs: Some(20),
                    ..AgentConfig::default()
                };
                config.model_providers.insert(
                    "mock".to_string(),
                    ModelProviderConfig {
                        name: Some("Mock".to_string()),
                        base_url: Some(mock_base_url),
                        wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
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

                let client = bifrost_agent::AgentClient::new_with_bifrost_proxy(proxy_port);
                let expected_proxy_url = format!("http://127.0.0.1:{proxy_port}");
                if client.model_proxy_url() != Some(expected_proxy_url.as_str()) {
                    return Err(format!(
                        "expected agent model proxy to target Bifrost port {proxy_port}, got {:?}",
                        client.model_proxy_url()
                    ));
                }

                let tools = ToolRegistry::new();
                let mut session = AgentSession::new("model-proxy-e2e");
                let result = run_turn(
                    &client,
                    &config,
                    &mut session,
                    &tools,
                    "hello through bifrost proxy",
                    None,
                )
                .await
                .map_err(|e| format!("proxied turn failed: {e}"))?;
                if result.response.is_empty() {
                    return Err("expected model response from proxied request".to_string());
                }

                let direct = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("failed to create direct client: {e}"))?;
                let host_filter = mock_host.to_string();
                let list_url = format!(
                    "http://127.0.0.1:{proxy_port}/_bifrost/api/traffic?limit=20&host_contains={}",
                    urlencoding::encode(&host_filter)
                );
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let traffic_json: serde_json::Value = direct
                        .get(&list_url)
                        .send()
                        .await
                        .map_err(|e| format!("traffic list request failed: {e}"))?
                        .json()
                        .await
                        .map_err(|e| format!("traffic list decode failed: {e}"))?;
                    let records = traffic_json
                        .get("records")
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| format!("traffic response missing records: {traffic_json}"))?;
                    let found = records.iter().any(|record| {
                        record
                            .get("h")
                            .and_then(|v| v.as_str())
                            .map(|host| host.contains(&host_filter))
                            .unwrap_or(false)
                            && record
                                .get("m")
                                .and_then(|v| v.as_str())
                                .map(|method| method == "POST")
                                .unwrap_or(false)
                    });
                    if found {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(format!(
                            "timed out waiting for proxied model request traffic record; host_filter={host_filter}; traffic={traffic_json}"
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "im_gateway_agent_long_term_memory_remember_recall",
            "Validate file memories inject read-path instructions from the current data-dir layout",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start().await?;
                let temp_dir = tempfile::tempdir()
                    .map_err(|e| format!("failed to create temp dir: {e}"))?;
                let data_dir = temp_dir.path().to_string_lossy().to_string();
                tokio::task::spawn_blocking(move || {
                    temp_env::with_vars(
                    [
                        ("BIFROST_DATA_DIR", Some(data_dir.as_str())),
                    ],
                    || {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("temp env runtime")
                            .block_on(async move {
                let memory_root = temp_dir.path().join("agent").join("memory");
                std::fs::create_dir_all(&memory_root)
                    .map_err(|e| format!("failed to create memory root: {e}"))?;
                std::fs::write(
                    memory_root.join("memory_summary.md"),
                    "Bifrost should use on-demand memory loading.",
                )
                .map_err(|e| format!("failed to write memory summary: {e}"))?;
                std::fs::write(
                    memory_root.join("MEMORY.md"),
                    "# Memory\n\n- Memory evidence lives here.\n",
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
                        wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
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
                                && content.contains("Bifrost should use on-demand memory loading.")
                                && content.contains("<oai-mem-citation>")
                        })
                        .unwrap_or(false)
                });
                if !injected {
                    return Err(format!("memory read-path instructions were not injected: {messages:?}"));
                }
                Ok(())
                            })
                    },
                )
                })
                .await
                .map_err(|error| format!("memory env task join failed: {error}"))?
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
                let data_dir = temp_dir.path().to_string_lossy().to_string();
                tokio::task::spawn_blocking(move || {
                    temp_env::with_vars(
                    [
                        ("BIFROST_DATA_DIR", Some(data_dir.as_str())),
                    ],
                    || {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("temp env runtime")
                            .block_on(async move {
                let memory_root = temp_dir.path().join("agent").join("memory");

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
                        wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
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
                // auto_extract_after_turn runs as a fire-and-forget tokio::spawn
                // task. Wait up to 10s for the background write to land before
                // asserting on the on-disk state (avoids a test-only race while
                // keeping the production assistant reply non-blocking).
                let summary_path = memory_root.join("memory_summary.md");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if let Ok(content) = std::fs::read_to_string(&summary_path) {
                        if content.contains("MEM-AUTO-42") {
                            break;
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
                        "auto memory was not persisted to memory files; summary={summary:?}; memory={memory:?}; raw={raw:?}; rollout_count={rollout_count}"
                    ));
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
                            })
                    },
                )
                })
                .await
                .map_err(|error| format!("memory env task join failed: {error}"))?
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
                        wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
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
                let tools = ToolRegistry::with_defaults();
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
        TestCase::standalone(
            "im_gateway_agent_retry_sanitizes_orphan_tool_history",
            "Validate transient retry rebuilds and sanitizes malformed tool history before Chat Completions retry",
            "admin",
            || async move {
                let mock = ChatCompletionMock::start_fail_once_before_tool_loop().await?;

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
                        wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
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
                let tools = ToolRegistry::with_defaults();
                let mut session = AgentSession::new("retry-sanitize-e2e");
                session.history.push(bifrost_agent::types::ChatMessage::tool_result(
                    "stale-tool-call",
                    "stale tool output",
                ));

                let result = run_turn(
                    &client,
                    &config,
                    &mut session,
                    &tools,
                    "list the current directory after retry",
                    None,
                )
                .await
                .map_err(|e| format!("retry turn failed: {e}"))?;
                if result.tool_calls_log.is_empty() {
                    return Err("expected retry flow to execute a tool call".to_string());
                }

                let requests = mock.requests.lock();
                if requests.len() < 3 {
                    return Err(format!(
                        "expected retry scenario to emit at least 3 model requests, got {}",
                        requests.len()
                    ));
                }
                for (idx, request) in requests.iter().enumerate() {
                    if let Some(error) = validate_chat_messages_json(request) {
                        return Err(format!(
                            "mock observed malformed message history on request {}: {}",
                            idx + 1,
                            error
                        ));
                    }
                }

                let first_roles = requests[0]
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| "first request missing messages".to_string())?
                    .iter()
                    .filter_map(|message| message.get("role").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>();
                let retry_roles = requests[1]
                    .get("messages")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| "retry request missing messages".to_string())?
                    .iter()
                    .filter_map(|message| message.get("role").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>();
                if first_roles != retry_roles {
                    return Err(format!(
                        "retry request roles diverged from first request: first={first_roles:?}, retry={retry_roles:?}"
                    ));
                }
                if retry_roles.contains(&"tool") {
                    return Err(format!(
                        "retry request still contained orphan tool role: {retry_roles:?}"
                    ));
                }

                Ok(())
            },
        ),
    ];
    tests.into_iter().map(TestCase::serial).collect()
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

                            let is_memory_extract =
                                request_messages_contain(&body, EXTRACT_SYSTEM_PROMPT);
                            let is_memory_consolidation =
                                request_messages_contain(&body, CONSOLIDATION_SYSTEM_PROMPT);
                            let consumes_auto_memory = request_messages_contain(&body, "## Memory")
                                && request_messages_contain(&body, "MEM-AUTO-42")
                                && request_messages_contain(
                                    &body,
                                    "新的对话。请根据长期记忆回答我的 Bifrost 项目代号",
                                );
                            let is_prompt_split_e2e =
                                request_messages_contain(&body, "PROMPT_SPLIT_BASE_OK")
                                    && request_messages_contain(&body, "PROMPT_SPLIT_DEVELOPER_OK")
                                    && request_messages_contain(&body, "PROMPT_SPLIT_USER_OK")
                                    && request_messages_contain(&body, "PROMPT_SPLIT_CHAT_E2E")
                                    && body
                                        .get("messages")
                                        .and_then(|value| value.as_array())
                                        .map(|messages| {
                                            messages
                                                .first()
                                                .and_then(|m| m.get("role"))
                                                .and_then(|r| r.as_str())
                                                == Some("system")
                                                && messages
                                                    .get(1)
                                                    .and_then(|m| m.get("role"))
                                                    .and_then(|r| r.as_str())
                                                    == Some("developer")
                                                && messages
                                                    .get(2)
                                                    .and_then(|m| m.get("role"))
                                                    .and_then(|r| r.as_str())
                                                    == Some("user")
                                                && messages
                                                    .get(3)
                                                    .and_then(|m| m.get("role"))
                                                    .and_then(|r| r.as_str())
                                                    == Some("user")
                                        })
                                        .unwrap_or(false);
                            let is_multimodal_e2e =
                                request_messages_contain(&body, "MULTIMODAL_IMAGE_E2E")
                                    && request_contains_image_url(&body);
                            let is_stop_e2e = (request_messages_contain(&body, "AGENT_STOP_E2E")
                                || request_messages_contain(&body, "AGENT_QUEUE_REFRESH_E2E"))
                                && !request_messages_contain(&body, "已收到 /stop");
                            if is_stop_e2e {
                                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            }
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
                                let content = json!({
                                    "rollout_summary": "# Auto memory source\n\n## Task 1: remember project code\nOutcome: success\n\nReusable knowledge:\n- User's Bifrost project code is MEM-AUTO-42.",
                                    "rollout_slug": "auto-memory-source",
                                    "raw_memory": "---\ndescription: User's Bifrost project code is MEM-AUTO-42.\ntask: remember_project_code\ntask_group: /home/user/work/project\ntask_outcome: success\ncwd: /home/user/work/project\nkeywords: MEM-AUTO-42, auto memory, bifrost\n---\n\n### Task 1: remember project code\n\ntask: remember_project_code\ntask_group: bifrost agent memory\ntask_outcome: success\n\nReusable knowledge:\n- User's Bifrost project code is MEM-AUTO-42.\n"
                                })
                                .to_string();
                                json!({
                                    "role": "assistant",
                                    "content": content
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
                            } else if is_prompt_split_e2e {
                                json!({
                                    "role": "assistant",
                                    "content": "PROMPT_SPLIT_CHAT_E2E_OK"
                                })
                            } else if is_multimodal_e2e {
                                json!({
                                    "role": "assistant",
                                    "content": "MULTIMODAL_IMAGE_E2E_OK: image understood"
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

    async fn start_long_exec() -> Result<Self, String> {
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

                            let last_role = body
                                .get("messages")
                                .and_then(|value| value.as_array())
                                .and_then(|messages| messages.last())
                                .and_then(|message| message.get("role"))
                                .and_then(|role| role.as_str());
                            let should_call_tool = current_call == 1 && last_role != Some("tool");
                            let message = if should_call_tool {
                                json!({
                                    "role": "assistant",
                                    "content": null,
                                    "tool_calls": [{
                                        "id": "call-long-exec",
                                        "type": "function",
                                        "function": {
                                            "name": "exec_command",
                                            "arguments": "{\"cmd\":\"printf start; sleep 0.4; printf done # cargo test\",\"yield_time_ms\":250}"
                                        }
                                    }]
                                })
                            } else {
                                json!({
                                    "role": "assistant",
                                    "content": "LONG_TASK_RUNTIME_WATCH_E2E_OK"
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

    async fn start_fail_once_before_tool_loop() -> Result<Self, String> {
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

                            if current_call == 1 {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                                        .header("Content-Type", "text/plain")
                                        .body(Full::new(Bytes::from("temporary")))
                                        .unwrap(),
                                );
                            }

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
                            let should_call_tool = has_tools && last_role != Some("tool");
                            let message = if should_call_tool {
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
                let Some(content) = message.get("content") else {
                    return false;
                };
                if let Some(text) = content.as_str() {
                    return text.contains(needle);
                }
                content
                    .as_array()
                    .map(|parts| {
                        parts.iter().any(|part| {
                            part.get("text")
                                .and_then(|value| value.as_str())
                                .map(|text| text.contains(needle))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn assert_queue_item(value: &serde_json::Value, expected_message: &str) -> Result<(), String> {
    if value
        .get("queue_length")
        .or_else(|| value.get("queueLength"))
        .and_then(|item| item.as_u64())
        != Some(1)
    {
        return Err(format!("Expected queue length 1, got: {value}"));
    }
    let has_expected_item = value
        .get("queue_items")
        .or_else(|| value.get("queueItems"))
        .and_then(|item| item.as_array())
        .map(|items| {
            items.iter().any(|item| {
                item.get("seq").and_then(|seq| seq.as_u64()) == Some(1)
                    && item.get("message").and_then(|message| message.as_str())
                        == Some(expected_message)
            })
        })
        .unwrap_or(false);
    if !has_expected_item {
        return Err(format!(
            "Expected queue item {expected_message:?}, got: {value}"
        ));
    }
    Ok(())
}

fn request_contains_image_url(body: &serde_json::Value) -> bool {
    body.get("messages")
        .and_then(|value| value.as_array())
        .map(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(|value| value.as_array())
                    .map(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(|value| value.as_str()) == Some("image_url")
                                && part
                                    .pointer("/image_url/url")
                                    .and_then(|value| value.as_str())
                                    .is_some_and(|url| url.starts_with("data:image/png;base64,"))
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

async fn start_im_gateway_admin(port: u16) -> Result<(ProxyInstance, Arc<AdminState>), String> {
    let data_dir = std::env::temp_dir().join(format!("bifrost_e2e_im_gateway_agent_{port}"));
    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)
            .map_err(|e| format!("Failed to clean IM Gateway E2E data dir: {e}"))?;
    }
    std::env::set_var("BIFROST_DATA_DIR", &data_dir);

    let (proxy, admin_state) = ProxyInstance::start_with_admin(port, vec![], false, true)
        .await
        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;
    admin_state.set_im_gateway_service(Arc::new(ImGatewayService::new_with_agent_proxy_port(
        &data_dir,
        Some(port),
    )));
    Ok((proxy, admin_state))
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {}", e))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {}", e))
}
