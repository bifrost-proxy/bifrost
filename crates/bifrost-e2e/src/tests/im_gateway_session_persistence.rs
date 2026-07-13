//! E2E tests for IM Gateway session persistence across service restarts.

use crate::assertions::assert_status;
use crate::{ProxyInstance, TestCase};
use bifrost_admin::im_gateway::session_state::{self, ImAgentSessionState};
use bifrost_admin::{AdminState, ImGatewayService};
use std::net::TcpListener;
use std::sync::Arc;

pub fn get_all_tests() -> Vec<TestCase> {
    let tests = vec![
        TestCase::standalone(
            "im_gateway_chatgpt_web_restores_conversation_after_service_restart",
            "Validate Chat Gateway chatgpt_web restores persisted conversationId after service restart and does not reuse it after /reset",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let data_dir =
                    std::env::temp_dir().join(format!("bifrost_e2e_chatgpt_web_resume_{port}"));
                let _env_guard = DataDirEnvGuard::new(&data_dir);
                let _mock_guard = EnvVarGuard::set("BIFROST_CHATGPT_WEB_E2E_MOCK", "1");
                let (_proxy, admin_state) =
                    start_im_gateway_admin_with_data_dir(port, data_dir.clone()).await?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {e}"))?;
                let base = format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat");
                let runner_id = "chatgpt-web-resume-e2e";
                let session_key = "chatgpt-web-restart-resume-e2e";
                let conversation_id = "conv-chatgpt-web-e2e-1";
                let mut runners = serde_json::Map::new();
                runners.insert(
                    runner_id.to_string(),
                    serde_json::json!({
                        "enabled": true,
                        "adapter": "chatgpt_web",
                        "adapterConfig": {
                            "timeoutSecs": 1
                        },
                        "injectBifrostTools": false,
                        "deliveryMode": "no_im"
                    }),
                );

                let config_response = client
                    .patch(format!("{base}/config"))
                    .json(&serde_json::json!({
                        "version": 1,
                        "defaultRunnerId": runner_id,
                        "runners": runners,
                        "channels": {}
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("PATCH chat gateway config failed: {e}"))?;
                assert_status(&config_response, 200)?;

                session_state::remember_session_state(ImAgentSessionState {
                    session_key: session_key.to_string(),
                    adapter: "chatgpt_web".to_string(),
                    runner_id: Some(runner_id.to_string()),
                    external_conversation_id: Some(conversation_id.to_string()),
                    updated_at: 1,
                    ..ImAgentSessionState::default()
                })
                .map_err(|error| format!("seed chatgpt_web session state failed: {error}"))?;

                admin_state.set_im_gateway_service(Arc::new(
                    ImGatewayService::new_with_agent_proxy_port(&data_dir, Some(port)),
                ));

                let resume_response = client
                    .post(&base)
                    .json(&serde_json::json!({
                        "sessionKey": session_key,
                        "runnerId": runner_id,
                        "adapter": "chatgpt_web",
                        "message": "resume chatgpt web e2e"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST chatgpt_web resume request failed: {e}"))?;
                assert_status(&resume_response, 200)?;

                let resume_snapshot = latest_chat_run_snapshot(&data_dir)?;
                let restored_conversation = resume_snapshot
                    .get("params")
                    .and_then(|params| params.get("conversationId"))
                    .and_then(|value| value.as_str());
                if restored_conversation != Some(conversation_id) {
                    return Err(format!(
                        "Expected restored chatgpt_web conversationId {conversation_id:?}, got snapshot: {resume_snapshot}"
                    ));
                }

                let reset_response = client
                    .post(&base)
                    .json(&serde_json::json!({
                        "sessionKey": session_key,
                        "runnerId": runner_id,
                        "adapter": "chatgpt_web",
                        "message": "/reset"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST chatgpt_web reset failed: {e}"))?;
                assert_status(&reset_response, 200)?;

                admin_state.set_im_gateway_service(Arc::new(
                    ImGatewayService::new_with_agent_proxy_port(&data_dir, Some(port)),
                ));

                let fresh_response = client
                    .post(&base)
                    .json(&serde_json::json!({
                        "sessionKey": session_key,
                        "runnerId": runner_id,
                        "adapter": "chatgpt_web",
                        "message": "fresh chatgpt web e2e"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("POST chatgpt_web fresh request failed: {e}"))?;
                assert_status(&fresh_response, 200)?;

                let fresh_snapshot = latest_chat_run_snapshot(&data_dir)?;
                let fresh_conversation = fresh_snapshot
                    .get("params")
                    .and_then(|params| params.get("conversationId"))
                    .and_then(|value| value.as_str());
                if fresh_conversation.is_some() {
                    return Err(format!(
                        "Expected /reset to prevent chatgpt_web conversationId restore, got snapshot: {fresh_snapshot}"
                    ));
                }

                Ok(())
            },
        ),
    ];
    tests.into_iter().map(TestCase::serial).collect()
}

fn latest_chat_run_snapshot(data_dir: &std::path::Path) -> Result<serde_json::Value, String> {
    let runs_dir = data_dir.join("agent/im_gateway/chat_runs");
    let mut entries = std::fs::read_dir(&runs_dir)
        .map_err(|error| format!("read chat runs dir {} failed: {error}", runs_dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let latest = entries
        .pop()
        .ok_or_else(|| format!("no chat run directories under {}", runs_dir.display()))?;
    let snapshot_path = latest.path().join("runtime_snapshot.json");
    let content = std::fs::read_to_string(&snapshot_path)
        .map_err(|error| format!("read snapshot {} failed: {error}", snapshot_path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("parse snapshot {} failed: {error}", snapshot_path.display()))
}

async fn start_im_gateway_admin_with_data_dir(
    port: u16,
    data_dir: std::path::PathBuf,
) -> Result<(ProxyInstance, Arc<AdminState>), String> {
    let (proxy, admin_state) = ProxyInstance::start_with_admin(port, vec![], false, true)
        .await
        .map_err(|e| format!("Failed to start proxy with admin: {e}"))?;
    admin_state.set_im_gateway_service(Arc::new(ImGatewayService::new_with_agent_proxy_port(
        &data_dir,
        Some(port),
    )));
    Ok((proxy, admin_state))
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {e}"))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {e}"))
}

struct DataDirEnvGuard {
    old_data_dir: Option<String>,
}

impl DataDirEnvGuard {
    fn new(path: &std::path::Path) -> Self {
        let old_data_dir = std::env::var("BIFROST_DATA_DIR").ok();
        std::env::set_var("BIFROST_DATA_DIR", path);
        Self { old_data_dir }
    }
}

impl Drop for DataDirEnvGuard {
    fn drop(&mut self) {
        match self.old_data_dir.take() {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
    }
}

struct EnvVarGuard {
    key: &'static str,
    old_value: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let old_value = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, old_value }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.old_value.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}
