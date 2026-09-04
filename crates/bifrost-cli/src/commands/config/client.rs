use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub struct ConfigApiClient {
    base_url: String,
    bearer_token: Option<String>,
}

impl ConfigApiClient {
    pub fn new(host: &str, port: u16) -> Self {
        if let Ok(base_url) = std::env::var("BIFROST_INTERNAL_CLIENT_BASE_URL") {
            // This value is emitted only by the validated Client envelope. If
            // it is malformed, preserve it so the request fails closed; never
            // fall back to this machine's loopback Admin API.
            return Self {
                base_url: format!("{}/_bifrost/api", base_url.trim_end_matches('/')),
                bearer_token: std::env::var("BIFROST_INTERNAL_CLIENT_TOKEN").ok(),
            };
        }
        Self {
            base_url: format!("http://{}:{}/_bifrost/api", host, port),
            bearer_token: None,
        }
    }

    pub fn from_base_url(base_url: &str, bearer_token: Option<String>) -> Result<Self, String> {
        let base_url = base_url.trim_end_matches('/');
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err("Admin API base URL must use http or https".to_string());
        }
        Ok(Self {
            base_url: format!("{base_url}/_bifrost/api"),
            bearer_token,
        })
    }

    pub fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub fn request(&self, method: &str, path: &str) -> ureq::Request {
        let agent = bifrost_core::direct_ureq_agent_builder()
            .redirects(0)
            .build();
        let request = agent.request(method, &self.url(path));
        match self.bearer_token() {
            Some(token) => request.set("Authorization", &format!("Bearer {token}")),
            None => request,
        }
    }

    pub fn get_public<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.read_json(self.request("GET", path).call(), "GET", path)
    }

    pub fn post_public<T: DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: &R,
    ) -> Result<T, String> {
        self.read_json(self.request("POST", path).send_json(body), "POST", path)
    }

    pub fn get_tls_config(&self) -> Result<TlsConfigResponse, String> {
        self.get("/config/tls")
    }

    pub fn get_server_config(&self) -> Result<ServerConfigResponse, String> {
        self.get("/config/server")
    }

    pub fn update_server_config(
        &self,
        req: &UpdateServerConfigRequest,
    ) -> Result<ServerConfigResponse, String> {
        self.put("/config/server", req)
    }

    pub fn update_tls_config(
        &self,
        req: &UpdateTlsConfigRequest,
    ) -> Result<TlsConfigResponse, String> {
        self.put("/config/tls", req)
    }

    pub fn get_performance_config(&self) -> Result<PerformanceConfigResponse, String> {
        self.get("/config/performance")
    }

    pub fn update_performance_config(
        &self,
        req: &UpdatePerformanceConfigRequest,
    ) -> Result<PerformanceConfigResponse, String> {
        self.put("/config/performance", req)
    }

    pub fn clear_cache(&self) -> Result<ClearCacheResponse, String> {
        self.delete("/config/performance/clear-cache")
    }

    pub fn disconnect_by_domain(&self, domain: &str) -> Result<DisconnectResponse, String> {
        self.post(
            "/config/connections/disconnect",
            &DisconnectRequest {
                domain: domain.to_string(),
            },
        )
    }

    pub fn get_whitelist(&self) -> Result<WhitelistResponse, String> {
        self.get("/whitelist")
    }

    pub fn set_allow_lan(&self, allow: bool) -> Result<serde_json::Value, String> {
        self.put(
            "/whitelist/allow-lan",
            &AllowLanRequest { allow_lan: allow },
        )
    }

    pub fn set_userpass(&self, req: &UpdateUserPassRequest) -> Result<serde_json::Value, String> {
        self.put("/whitelist/userpass", req)
    }

    pub fn get_metrics(&self) -> Result<serde_json::Value, String> {
        self.get("/metrics")
    }

    pub fn get_metrics_history(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let path = match limit {
            Some(l) => format!("/metrics/history?limit={}", l),
            None => "/metrics/history".to_string(),
        };
        self.get(&path)
    }

    pub fn get_system_overview(&self) -> Result<serde_json::Value, String> {
        self.get("/system/overview")
    }

    pub fn get_app_metrics(&self) -> Result<Vec<serde_json::Value>, String> {
        self.get("/metrics/apps")
    }

    pub fn get_host_metrics(&self) -> Result<Vec<serde_json::Value>, String> {
        self.get("/metrics/hosts")
    }

    pub fn get_sync_status(&self) -> Result<SyncStatusResponse, String> {
        self.get("/sync/status")
    }

    pub fn update_sync_config(
        &self,
        req: &UpdateSyncConfigRequest,
    ) -> Result<SyncStatusResponse, String> {
        self.put("/sync/config", req)
    }

    pub fn sync_login(
        &self,
        token: Option<&str>,
        remote_base_url: Option<&str>,
    ) -> Result<SyncStatusResponse, String> {
        self.post(
            "/sync/login",
            &serde_json::json!({
                "token": token,
                "remote_base_url": remote_base_url,
            }),
        )
    }

    pub fn sync_logout(&self) -> Result<SyncStatusResponse, String> {
        self.post("/sync/logout", &serde_json::json!({}))
    }

    pub fn sync_run(&self) -> Result<SyncStatusResponse, String> {
        self.post("/sync/run", &serde_json::json!({}))
    }

    pub fn clear_traffic(&self) -> Result<serde_json::Value, String> {
        self.delete("/traffic")
    }

    pub fn delete_traffic_by_ids(&self, ids: &[String]) -> Result<serde_json::Value, String> {
        self.delete_with_body("/traffic", &serde_json::json!({ "ids": ids }))
    }

    pub fn reorder_rules(&self, order: &[String]) -> Result<serde_json::Value, String> {
        self.put("/rules/reorder", &serde_json::json!({ "order": order }))
    }

    pub fn rename_rule(&self, old_name: &str, new_name: &str) -> Result<serde_json::Value, String> {
        let path = format!("/rules/{}/rename", urlencoding::encode(old_name));
        self.put(&path, &serde_json::json!({ "new_name": new_name }))
    }

    pub fn rename_script(
        &self,
        script_type: &str,
        name: &str,
        new_name: &str,
    ) -> Result<serde_json::Value, String> {
        let path = format!(
            "/scripts/rename/{}/{}",
            script_type,
            urlencoding::encode(name)
        );
        self.post(&path, &serde_json::json!({ "new_name": new_name }))
    }

    pub fn get_script(&self, script_type: &str, name: &str) -> Result<serde_json::Value, String> {
        let path = format!("/scripts/{}/{}", script_type, urlencoding::encode(name));
        self.get(&path)
    }

    pub fn save_script(
        &self,
        script_type: &str,
        name: &str,
        content: &str,
    ) -> Result<serde_json::Value, String> {
        let path = format!("/scripts/{}/{}", script_type, urlencoding::encode(name));
        self.put(&path, &serde_json::json!({ "content": content }))
    }

    pub fn delete_script(
        &self,
        script_type: &str,
        name: &str,
    ) -> Result<serde_json::Value, String> {
        let path = format!("/scripts/{}/{}", script_type, urlencoding::encode(name));
        self.delete(&path)
    }

    pub fn upsert_values(
        &self,
        values: &std::collections::HashMap<String, String>,
    ) -> Result<serde_json::Value, String> {
        self.put("/values", &serde_json::json!({ "values": values }))
    }

    pub fn update_value(&self, name: &str, value: &str) -> Result<serde_json::Value, String> {
        let path = format!("/values/{}", urlencoding::encode(name));
        self.put(&path, &serde_json::json!({ "value": value }))
    }

    pub fn delete_value(&self, name: &str) -> Result<serde_json::Value, String> {
        let path = format!("/values/{}", urlencoding::encode(name));
        self.delete(&path)
    }

    pub fn get_access_mode(&self) -> Result<serde_json::Value, String> {
        self.get("/whitelist/mode")
    }

    pub fn set_access_mode(&self, mode: &str) -> Result<serde_json::Value, String> {
        self.put("/whitelist/mode", &serde_json::json!({ "mode": mode }))
    }

    pub fn add_temporary(&self, ip: &str) -> Result<serde_json::Value, String> {
        self.post("/whitelist/temporary", &serde_json::json!({ "ip": ip }))
    }

    pub fn remove_temporary(&self, ip: &str) -> Result<serde_json::Value, String> {
        self.delete_with_body("/whitelist/temporary", &serde_json::json!({ "ip": ip }))
    }

    pub fn get_pending(&self) -> Result<Vec<serde_json::Value>, String> {
        self.get("/whitelist/pending")
    }

    pub fn approve_pending(&self, ip: &str) -> Result<serde_json::Value, String> {
        self.post(
            "/whitelist/pending/approve",
            &serde_json::json!({ "ip": ip }),
        )
    }

    pub fn reject_pending(&self, ip: &str) -> Result<serde_json::Value, String> {
        self.post(
            "/whitelist/pending/reject",
            &serde_json::json!({ "ip": ip }),
        )
    }

    pub fn clear_pending(&self) -> Result<serde_json::Value, String> {
        self.delete("/whitelist/pending")
    }

    pub fn get_sandbox_config(&self) -> Result<serde_json::Value, String> {
        self.get("/config/sandbox")
    }

    pub fn version_check(&self) -> Result<serde_json::Value, String> {
        self.get("/system/version-check?refresh=true")
    }

    pub fn get_websocket_connections(&self) -> Result<serde_json::Value, String> {
        self.get("/websocket/connections")
    }

    pub fn disconnect_by_app(&self, app: &str) -> Result<serde_json::Value, String> {
        self.post(
            "/config/connections/disconnect-by-app",
            &serde_json::json!({ "app": app }),
        )
    }

    pub fn list_connections(&self) -> Result<serde_json::Value, String> {
        self.get("/config/connections")
    }

    pub fn get_memory_diagnostics(&self) -> Result<serde_json::Value, String> {
        self.get("/system/memory")
    }

    pub fn list_remote_invoke_grants(&self) -> Result<serde_json::Value, String> {
        self.get("/remote-invoke/grants")
    }

    pub fn update_remote_invoke_grant<R: Serialize>(
        &self,
        grant_id: &str,
        body: &R,
    ) -> Result<serde_json::Value, String> {
        self.patch(
            &format!("/remote-invoke/grants/{}", urlencoding::encode(grant_id)),
            body,
        )
    }

    pub fn revoke_remote_invoke_grant(&self, grant_id: &str) -> Result<serde_json::Value, String> {
        self.delete(&format!(
            "/remote-invoke/grants/{}",
            urlencoding::encode(grant_id)
        ))
    }

    pub fn get_remote_invoke_ssh_key(&self) -> Result<serde_json::Value, String> {
        self.get("/remote-invoke/ssh-key")
    }

    pub fn create_remote_invoke_ssh_key<R: Serialize>(
        &self,
        body: &R,
    ) -> Result<serde_json::Value, String> {
        self.post("/remote-invoke/ssh-key", body)
    }

    pub fn export_remote_invoke_ssh_key(&self) -> Result<serde_json::Value, String> {
        self.get("/remote-invoke/ssh-key/private-key")
    }

    pub fn revoke_remote_invoke_ssh_key(&self) -> Result<serde_json::Value, String> {
        self.delete("/remote-invoke/ssh-key")
    }

    pub fn bifrost_file_detect(&self, content: &str) -> Result<serde_json::Value, String> {
        self.post_text("/bifrost-file/detect", content)
    }

    pub fn bifrost_file_import(&self, content: &str) -> Result<serde_json::Value, String> {
        self.post_text("/bifrost-file/import", content)
    }

    pub fn bifrost_file_export_rules(
        &self,
        rule_names: &[String],
        description: Option<&str>,
    ) -> Result<String, String> {
        let mut body = serde_json::json!({ "rule_names": rule_names });
        if let Some(desc) = description {
            body["description"] = serde_json::json!(desc);
        }
        self.post_text_response("/bifrost-file/export/rules", &body)
    }

    pub fn bifrost_file_export_values(
        &self,
        value_names: Option<&[String]>,
        description: Option<&str>,
    ) -> Result<String, String> {
        let mut body = serde_json::json!({});
        if let Some(names) = value_names {
            body["value_names"] = serde_json::json!(names);
        }
        if let Some(desc) = description {
            body["description"] = serde_json::json!(desc);
        }
        self.post_text_response("/bifrost-file/export/values", &body)
    }

    pub fn bifrost_file_export_scripts(
        &self,
        script_names: &[String],
        description: Option<&str>,
    ) -> Result<String, String> {
        let mut body = serde_json::json!({ "script_names": script_names });
        if let Some(desc) = description {
            body["description"] = serde_json::json!(desc);
        }
        self.post_text_response("/bifrost-file/export/scripts", &body)
    }

    pub(crate) fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.read_json(self.request("GET", path).call(), "GET", path)
    }

    pub(crate) fn put<T: DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: &R,
    ) -> Result<T, String> {
        self.read_json(self.request("PUT", path).send_json(body), "PUT", path)
    }

    pub(crate) fn post<T: DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: &R,
    ) -> Result<T, String> {
        self.read_json(self.request("POST", path).send_json(body), "POST", path)
    }

    fn patch<T: DeserializeOwned, R: Serialize>(&self, path: &str, body: &R) -> Result<T, String> {
        self.read_json(self.request("PATCH", path).send_json(body), "PATCH", path)
    }

    pub(crate) fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.read_json(self.request("DELETE", path).call(), "DELETE", path)
    }

    fn delete_with_body<T: DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: &R,
    ) -> Result<T, String> {
        self.read_json(self.request("DELETE", path).send_json(body), "DELETE", path)
    }

    pub(crate) fn delete_with_body_public<T: DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: &R,
    ) -> Result<T, String> {
        self.delete_with_body(path, body)
    }

    fn post_text<T: DeserializeOwned>(&self, path: &str, text: &str) -> Result<T, String> {
        let resp = self
            .request("POST", path)
            .set("Content-Type", "text/plain")
            .send_string(text)
            .map_err(|e| self.request_error("POST", path, e))?;
        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn post_text_response<R: Serialize>(&self, path: &str, body: &R) -> Result<String, String> {
        let resp = self
            .request("POST", path)
            .send_json(body)
            .map_err(|e| self.request_error("POST", path, e))?;
        resp.into_string()
            .map_err(|e| format!("Failed to read response: {}", e))
    }

    fn read_json<T: DeserializeOwned>(
        &self,
        response: Result<ureq::Response, ureq::Error>,
        method: &str,
        path: &str,
    ) -> Result<T, String> {
        let response = response.map_err(|error| self.request_error(method, path, error))?;
        if !(200..300).contains(&response.status()) {
            return Err(self.response_error(method, path, response));
        }
        let body = response
            .into_string()
            .map_err(|error| format!("Failed to read Admin API response: {error}"))?;
        serde_json::from_str(&body)
            .map_err(|error| format!("Failed to parse Admin API response: {error}"))
    }

    fn request_error(&self, method: &str, path: &str, error: ureq::Error) -> String {
        match error {
            ureq::Error::Status(_, response) => self.response_error(method, path, response),
            ureq::Error::Transport(error) => format!(
                "Failed to connect to Bifrost admin API for {method} {}: {error}",
                self.url(path)
            ),
        }
    }

    fn response_error(&self, method: &str, path: &str, response: ureq::Response) -> String {
        let status = response.status();
        let body = response.into_string().unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .or_else(|| value.get("message"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| format!("HTTP {status}"));
        if status == 401 {
            format!(
                "Admin authentication failed for {method} {}: {message}. Run `bifrost client target login <target>` again",
                self.url(path)
            )
        } else {
            format!(
                "Failed to connect to Bifrost admin API for {method} {}: HTTP {status}: {message}",
                self.url(path)
            )
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfigResponse {
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub ip_intercept_exclude: Vec<String>,
    pub ip_intercept_include: Vec<String>,
    pub unsafe_ssl: bool,
    pub disconnect_on_config_change: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfigResponse {
    pub timeout_secs: u64,
    pub http1_max_header_size: usize,
    pub http2_max_header_list_size: usize,
    pub websocket_handshake_max_header_size: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateServerConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http1_max_header_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http2_max_header_list_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket_handshake_max_header_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateTlsConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_tls_interception: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intercept_exclude: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intercept_include: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_intercept_exclude: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_intercept_include: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_intercept_exclude: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_intercept_include: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsafe_ssl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disconnect_on_config_change: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfigResponse {
    pub traffic: TrafficConfig,
    pub body_store_stats: Option<BodyStoreStats>,
    pub frame_store_stats: Option<FrameStoreStats>,
    pub ws_payload_store_stats: Option<WsPayloadStoreStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficConfig {
    pub max_records: usize,
    pub max_db_size_bytes: u64,
    pub max_body_memory_size: usize,
    pub max_body_buffer_size: usize,
    pub max_body_probe_size: usize,
    pub super_performance_mode: bool,
    pub binary_traffic_performance_mode: bool,
    pub file_retention_days: u64,
    pub sse_stream_flush_bytes: usize,
    pub sse_stream_flush_interval_ms: u64,
    pub ws_payload_flush_bytes: usize,
    pub ws_payload_flush_interval_ms: u64,
    pub ws_payload_max_open_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyStoreStats {
    pub file_count: usize,
    pub total_size: u64,
    pub temp_dir: String,
    pub max_memory_size: usize,
    pub retention_days: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameStoreStats {
    pub connection_count: usize,
    pub total_size: u64,
    pub frames_dir: String,
    pub retention_hours: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsPayloadStoreStats {
    pub file_count: usize,
    pub total_size: u64,
    pub payload_dir: String,
    pub retention_days: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdatePerformanceConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_records: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_db_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_body_memory_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_body_buffer_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_body_probe_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub super_performance_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_traffic_performance_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_retention_days: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sse_stream_flush_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sse_stream_flush_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_payload_flush_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_payload_flush_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_payload_max_open_files: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearCacheResponse {
    pub body_cache_removed: usize,
    pub traffic_cache_removed: usize,
    pub frame_cache_removed: usize,
    pub ws_payload_cache_removed: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisconnectRequest {
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisconnectResponse {
    pub success: bool,
    pub disconnected_count: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistResponse {
    pub mode: String,
    pub allow_lan: bool,
    pub whitelist: Vec<String>,
    pub temporary_whitelist: Vec<String>,
    pub userpass: UserPassResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserPassResponse {
    pub enabled: bool,
    pub accounts: Vec<UserPassAccountResponse>,
    pub loopback_requires_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPassAccountResponse {
    pub username: String,
    pub enabled: bool,
    pub has_password: bool,
    pub last_connected_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AllowLanRequest {
    pub allow_lan: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateUserPassRequest {
    pub enabled: bool,
    pub accounts: Vec<UpdateUserPassAccountRequest>,
    #[serde(default)]
    pub loopback_requires_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserPassAccountRequest {
    pub username: String,
    pub password: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusResponse {
    pub enabled: bool,
    pub auto_sync: bool,
    pub remote_base_url: String,
    pub has_session: bool,
    pub reachable: bool,
    pub authorized: bool,
    pub syncing: bool,
    pub reason: String,
    pub last_sync_at: Option<String>,
    pub last_sync_action: Option<String>,
    pub last_error: Option<String>,
    pub user: Option<SyncUserInfo>,
    #[serde(default)]
    pub providers: Vec<SyncProviderStatusResponse>,
    #[serde(default)]
    pub first_run_prompt_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncUserInfo {
    pub user_id: String,
    pub nickname: String,
    pub avatar: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProviderCapabilitiesResponse {
    pub rules_sync: bool,
    pub config_sync: bool,
    pub remote_invoke: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProviderStatusResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub remote_base_url: Option<String>,
    pub connected: bool,
    pub enabled: bool,
    pub reachable: bool,
    pub authorized: bool,
    pub user: Option<SyncUserInfo>,
    pub capabilities: SyncProviderCapabilitiesResponse,
    pub remote_invoke_registered: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateSyncConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_sync: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_base_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn config_api_client_get_server_config_uses_wiremock() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "timeout_secs": 30u64,
            "http1_max_header_size": 8192usize,
            "http2_max_header_list_size": 16384usize,
            "websocket_handshake_max_header_size": 4096usize,
        });

        Mock::given(method("GET"))
            .and(path("/_bifrost/api/config/server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .mount(&mock_server)
            .await;

        let base_uri = mock_server.uri(); // e.g. http://127.0.0.1:12345
        let without_scheme = base_uri.trim_start_matches("http://");
        let mut parts = without_scheme.split(':');
        let host = parts.next().expect("mock server host");
        let port: u16 = parts
            .next()
            .expect("mock server port")
            .parse()
            .expect("valid port number");

        let client = ConfigApiClient::new(host, port);
        let resp = client.get_server_config().expect("request should succeed");

        assert_eq!(resp.timeout_secs, 30);
        assert_eq!(resp.http1_max_header_size, 8192);
        assert_eq!(resp.http2_max_header_list_size, 16384);
        assert_eq!(resp.websocket_handshake_max_header_size, 4096);
    }

    #[tokio::test]
    async fn client_base_url_injects_bearer_for_admin_requests() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/metrics"))
            .and(header("authorization", "Bearer client-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"requests": 1})))
            .mount(&mock_server)
            .await;

        let client =
            ConfigApiClient::from_base_url(&mock_server.uri(), Some("client-secret".to_string()))
                .unwrap();
        let response = client.get_metrics().unwrap();

        assert_eq!(response["requests"], json!(1));
    }

    #[tokio::test]
    async fn client_base_url_does_not_follow_redirects_with_bearer() {
        let destination = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stolen"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(0)
            .mount(&destination)
            .await;

        let source = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/_bifrost/api/metrics"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/stolen", destination.uri())),
            )
            .mount(&source)
            .await;

        let client =
            ConfigApiClient::from_base_url(&source.uri(), Some("secret".to_string())).unwrap();
        let error = client.get_metrics().unwrap_err();

        assert!(error.contains("HTTP 302"), "unexpected error: {error}");
    }

    #[tokio::test]
    async fn admin_api_status_errors_preserve_the_legacy_connection_prefix() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/_bifrost/api/sync/login"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(json!({"error": "remote URL must use HTTPS"})),
            )
            .mount(&server)
            .await;

        let client = ConfigApiClient::from_base_url(&server.uri(), None).unwrap();
        let error = client
            .post_public::<serde_json::Value, _>("/sync/login", &json!({"token": "invalid"}))
            .unwrap_err();

        assert!(
            error.starts_with("Failed to connect to Bifrost admin API"),
            "unexpected error: {error}"
        );
        assert!(error.contains("HTTP 400"), "unexpected error: {error}");
        assert!(
            error.contains("remote URL must use HTTPS"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn config_api_client_new_builds_expected_base_url() {
        let client = ConfigApiClient::new("localhost", 8080);
        assert_eq!(client.base_url, "http://localhost:8080/_bifrost/api");
    }

    #[test]
    fn update_server_config_request_serializes_only_present_fields() {
        let req = UpdateServerConfigRequest {
            timeout_secs: Some(42),
            ..Default::default()
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value, json!({"timeout_secs": 42}));
    }

    #[test]
    fn update_tls_config_request_serializes_lists_and_flags() {
        let req = UpdateTlsConfigRequest {
            enable_tls_interception: Some(true),
            intercept_exclude: Some(vec!["example.com".into()]),
            unsafe_ssl: Some(false),
            disconnect_on_config_change: Some(true),
            ..Default::default()
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["enable_tls_interception"], json!(true));
        assert_eq!(value["intercept_exclude"], json!(["example.com"]));
        assert_eq!(value["unsafe_ssl"], json!(false));
        assert_eq!(value["disconnect_on_config_change"], json!(true));
        // Fields left as None should be omitted
        assert!(value.get("intercept_include").is_none());
    }

    #[test]
    fn update_performance_config_request_skips_none_fields() {
        let req = UpdatePerformanceConfigRequest {
            max_records: Some(1000),
            max_db_size_bytes: None,
            max_body_memory_size: Some(512 * 1024),
            super_performance_mode: Some(true),
            ..Default::default()
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["max_records"], json!(1000));
        assert_eq!(value["max_body_memory_size"], json!(512 * 1024));
        assert_eq!(value["super_performance_mode"], json!(true));
        assert!(value.get("max_db_size_bytes").is_none());
    }

    #[test]
    fn allow_lan_request_serializes_boolean_flag() {
        let req = AllowLanRequest { allow_lan: true };
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value, json!({"allow_lan": true}));
    }

    #[test]
    fn update_userpass_request_serializes_accounts_and_flags() {
        let req = UpdateUserPassRequest {
            enabled: true,
            accounts: vec![UpdateUserPassAccountRequest {
                username: "user".into(),
                password: Some("secret".into()),
                enabled: false,
            }],
            loopback_requires_auth: true,
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["enabled"], json!(true));
        assert_eq!(value["loopback_requires_auth"], json!(true));
        assert_eq!(value["accounts"][0]["username"], json!("user"));
        assert_eq!(value["accounts"][0]["password"], json!("secret"));
        assert_eq!(value["accounts"][0]["enabled"], json!(false));
    }

    #[test]
    fn update_sync_config_request_serializes_partial_config() {
        let req = UpdateSyncConfigRequest {
            enabled: Some(true),
            auto_sync: None,
            remote_base_url: Some("https://remote".into()),
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["enabled"], json!(true));
        assert_eq!(value["remote_base_url"], json!("https://remote"));
        assert!(value.get("auto_sync").is_none());
    }

    #[test]
    fn sync_status_response_roundtrips_through_json() {
        let status = SyncStatusResponse {
            enabled: true,
            auto_sync: false,
            remote_base_url: "https://remote".into(),
            has_session: true,
            reachable: true,
            authorized: true,
            syncing: false,
            reason: "ok".into(),
            last_sync_at: Some("2024-01-01T00:00:00Z".into()),
            last_sync_action: Some("full".into()),
            last_error: None,
            user: Some(SyncUserInfo {
                user_id: "u1".into(),
                nickname: "Alice".into(),
                avatar: "avatar.png".into(),
                email: "a@example.com".into(),
            }),
            providers: Vec::new(),
            first_run_prompt_required: false,
        };

        let json = serde_json::to_string(&status).unwrap();
        let decoded: SyncStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.enabled, status.enabled);
        assert_eq!(decoded.user.unwrap().email, "a@example.com");
    }
}
