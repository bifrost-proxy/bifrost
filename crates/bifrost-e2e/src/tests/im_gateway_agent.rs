//! E2E tests for IM Gateway Agent Admin API endpoints.

use crate::assertions::assert_status;
use crate::{ProxyInstance, TestCase};
use std::net::TcpListener;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "im_gateway_agent_config_get",
            "Validate GET /api/im-gateway/agent returns default config with expected fields",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

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
                if json.get("by_azure").is_none() {
                    return Err("Expected 'by_azure' field in agent config".to_string());
                }
                if json.get("base_url").is_none() {
                    return Err("Expected 'base_url' field in agent config".to_string());
                }
                if json.get("api_key").is_none() {
                    return Err("Expected 'api_key' field in agent config".to_string());
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
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

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

                // Verify the patch was successful
                let patch_json: serde_json::Value = patch_response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse patch response: {}", e))?;
                if patch_json.get("success").and_then(|v| v.as_bool()) != Some(true) {
                    return Err(format!(
                        "Expected success: true in patch response, got: {}",
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
                if json.get("base_url").and_then(|v| v.as_str()) != Some("https://test.example.com")
                {
                    return Err(format!(
                        "Expected base_url: 'https://test.example.com', got: {:?}",
                        json.get("base_url")
                    ));
                }
                if json.get("api_key").and_then(|v| v.as_str()) != Some("test-api-key-e2e") {
                    return Err(format!(
                        "Expected api_key: 'test-api-key-e2e', got: {:?}",
                        json.get("api_key")
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
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

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
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

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
                        "chat_ids": [],
                        "user_ids": []
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
    ]
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {}", e))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {}", e))
}
