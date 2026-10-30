//! Retired browser Runner configuration must fail closed while preserving history.
use crate::{ProxyInstance, TestCase};
use bifrost_admin::{AdminState, ImGatewayService};
use std::net::TcpListener;
use std::sync::Arc;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![TestCase::standalone(
        "im_gateway_retired_web_runner_preserves_history",
        "Reject retired Runner config/execution/login without migrating providers or deleting data",
        "admin",
        || async move {
            let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
            let data_dir = temp.path();
            let _guard = DataDirEnvGuard::new(data_dir);
            let config_path = data_dir.join("admin/im_gateway_external_cli_agent.json");
            std::fs::create_dir_all(config_path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(
                &config_path,
                serde_json::to_vec(&serde_json::json!({
                    "version":2, "defaultRunnerId":"web", "runners": {"web": {
                        "enabled":true, "adapter":"chatgpt_web", "adapterConfig":{}
                    }}, "channels":{}
                }))
                .unwrap(),
            )
            .map_err(|e| e.to_string())?;
            let history = data_dir.join("legacy-history.jsonl");
            let credentials = data_dir.join("auth_state.json");
            std::fs::write(&history, "old conversation").map_err(|e| e.to_string())?;
            std::fs::write(&credentials, "old credentials").map_err(|e| e.to_string())?;
            let port = TcpListener::bind("127.0.0.1:0")
                .map_err(|e| e.to_string())?
                .local_addr()
                .map_err(|e| e.to_string())?
                .port();
            let (_proxy, state): (ProxyInstance, Arc<AdminState>) =
                ProxyInstance::start_with_admin(port, vec![], false, true)
                    .await
                    .map_err(|error| error.to_string())?;
            state.set_im_gateway_service(Arc::new(ImGatewayService::new(data_dir)));
            let client = reqwest::Client::builder()
                .no_proxy()
                .build()
                .map_err(|e| e.to_string())?;
            let base = format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat");
            let config: serde_json::Value = client
                .get(format!("{base}/config"))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            if config["runners"]["web"]["adapter"] != "chatgpt_web" {
                return Err(format!("legacy Runner changed: {config}"));
            }
            let response = client
                .patch(format!("{base}/config"))
                .json(&config)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if response.status().as_u16() != 400 {
                return Err("retired config save must fail".into());
            }
            for stream in [false, true] {
                let url = if stream {
                    format!("{base}/stream")
                } else {
                    base.clone()
                };
                let response = client
                    .post(url)
                    .json(&serde_json::json!({
                        "runnerId":"web", "sessionKey":"old-session", "message":"hello"
                    }))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = response.status().as_u16();
                let body = response.text().await.map_err(|e| e.to_string())?;
                if status != 400 || !body.contains("removed") {
                    return Err(format!("expected retirement error, got {status}: {body}"));
                }
            }
            for path in ["status", "open", "stop"] {
                let response = client
                    .post(format!("{base}/adapters/chatgpt-web/auth/{path}"))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if response.status().as_u16() != 410 {
                    return Err(format!("login endpoint {path} remains available"));
                }
            }
            if std::fs::read_to_string(history).map_err(|e| e.to_string())? != "old conversation"
                || std::fs::read_to_string(credentials).map_err(|e| e.to_string())?
                    != "old credentials"
            {
                return Err("legacy data changed".into());
            }
            Ok(())
        },
    )
    .serial()]
}

struct DataDirEnvGuard {
    previous: Option<std::ffi::OsString>,
}
impl DataDirEnvGuard {
    fn new(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("BIFROST_DATA_DIR");
        std::env::set_var("BIFROST_DATA_DIR", path);
        Self { previous }
    }
}
impl Drop for DataDirEnvGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
    }
}
