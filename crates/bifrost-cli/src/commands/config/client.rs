use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub struct ConfigApiClient {
    base_url: String,
}

impl ConfigApiClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            base_url: format!("http://{}:{}/_bifrost/api", host, port),
        }
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

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent().get(&url).call().map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;

        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn put<T: DeserializeOwned, R: Serialize>(&self, path: &str, body: &R) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .put(&url)
            .send_json(body)
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;

        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn post<T: DeserializeOwned, R: Serialize>(&self, path: &str, body: &R) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .post(&url)
            .send_json(body)
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;

        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn patch<T: DeserializeOwned, R: Serialize>(&self, path: &str, body: &R) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .request("PATCH", &url)
            .send_json(body)
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;

        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .delete(&url)
            .call()
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;

        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn delete_with_body<T: DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: &R,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .delete(&url)
            .send_json(body)
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;
        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn post_text<T: DeserializeOwned>(&self, path: &str, text: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .post(&url)
            .set("Content-Type", "text/plain")
            .send_string(text)
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;
        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn post_text_response<R: Serialize>(&self, path: &str, body: &R) -> Result<String, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = bifrost_core::direct_ureq_agent()
            .post(&url)
            .send_json(body)
            .map_err(|e| {
            format!(
                "Failed to connect to Bifrost admin API at {}\nIs the proxy server running?\n\nHint: Start the proxy with: bifrost start\n\nError: {}",
                url, e
            )
        })?;
        resp.into_string()
            .map_err(|e| format!("Failed to read response: {}", e))
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncUserInfo {
    pub user_id: String,
    pub nickname: String,
    pub avatar: String,
    pub email: String,
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
            ..Default::default()
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["max_records"], json!(1000));
        assert_eq!(value["max_body_memory_size"], json!(512 * 1024));
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
        };

        let json = serde_json::to_string(&status).unwrap();
        let decoded: SyncStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.enabled, status.enabled);
        assert_eq!(decoded.user.unwrap().email, "a@example.com");
    }
}
