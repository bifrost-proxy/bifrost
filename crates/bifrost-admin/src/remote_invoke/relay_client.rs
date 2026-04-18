use std::time::Duration;

use bifrost_core::{direct_reqwest_client_builder, BifrostError, Result};
use parking_lot::RwLock;
use serde::Deserialize;

use super::types::{
    ClientCallExitRequest, ClientCallFrameRequest, ClientHeartbeatRequest,
    ClientRegistrationRequest, ClientRegistrationResponse, GrantDecisionRequest,
    PublishPairCodeRequest,
};

#[derive(Debug, Deserialize)]
struct RelayApiResponse<T> {
    code: i32,
    #[serde(default)]
    message: Option<String>,
    data: Option<T>,
}

pub struct RelayClient {
    http: reqwest::Client,
    base_url: RwLock<String>,
    client_auth_token: RwLock<Option<String>>,
    client_instance_id: String,
}

impl RelayClient {
    pub fn new(base_url: &str, client_instance_id: &str) -> Self {
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build relay http client");

        Self {
            http,
            base_url: RwLock::new(base_url.trim_end_matches('/').to_string()),
            client_auth_token: RwLock::new(None),
            client_instance_id: client_instance_id.to_string(),
        }
    }

    pub fn update_base_url(&self, new_url: &str) {
        let normalized = new_url.trim_end_matches('/').to_string();
        *self.base_url.write() = normalized;
        *self.client_auth_token.write() = None;
    }

    pub fn base_url(&self) -> String {
        self.base_url.read().clone()
    }

    pub async fn register(
        &self,
        req: &ClientRegistrationRequest,
    ) -> Result<ClientRegistrationResponse> {
        let url = format!("{}/v4/remote-invoke/client/register", self.base_url());
        let response =
            self.http.post(&url).json(req).send().await.map_err(|e| {
                BifrostError::Network(format!("relay register request failed: {e}"))
            })?;
        self.parse_response_with_data::<ClientRegistrationResponse>(response, "register")
            .await
    }

    pub fn set_auth_token(&self, token: String) {
        *self.client_auth_token.write() = Some(token);
    }

    pub fn auth_token(&self) -> Option<String> {
        self.client_auth_token.read().clone()
    }

    pub async fn heartbeat(&self, req: &ClientHeartbeatRequest) -> Result<()> {
        let url = format!("{}/v4/remote-invoke/client/heartbeat", self.base_url());
        let response = self
            .authorized_post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("relay heartbeat request failed: {e}")))?;
        self.parse_response_empty(response, "heartbeat").await
    }

    pub async fn publish_pair_code(&self, req: &PublishPairCodeRequest) -> Result<()> {
        let url = format!("{}/v4/remote-invoke/client/pair-code", self.base_url());
        let response = self
            .authorized_post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay publish pair code request failed: {e}"))
            })?;
        self.parse_response_empty(response, "publish_pair_code")
            .await
    }

    pub async fn close_discovery_session(&self, session_id: &str) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/discovery-session/{}?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(session_id),
            urlencoding::encode(&self.client_instance_id),
        );
        let response = self.authorized_delete(&url).send().await.map_err(|e| {
            BifrostError::Network(format!("relay close discovery session request failed: {e}"))
        })?;
        self.parse_response_empty(response, "close_discovery_session")
            .await
    }

    pub async fn submit_grant_decision(
        &self,
        pairing_id: &str,
        req: &GrantDecisionRequest,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/v4/remote-invoke/client/grants/{}/decision",
            self.base_url(),
            pairing_id
        );
        let response = self
            .authorized_post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay submit grant decision request failed: {e}"))
            })?;
        self.parse_response_with_data::<serde_json::Value>(response, "submit_grant_decision")
            .await
    }

    pub async fn post_call_frame(&self, call_id: &str, req: &ClientCallFrameRequest) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/calls/{}/frame",
            self.base_url(),
            call_id
        );
        let response = self
            .authorized_post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay post call frame request failed: {e}"))
            })?;
        self.parse_response_empty(response, "post_call_frame").await
    }

    pub async fn post_call_exit(&self, call_id: &str, req: &ClientCallExitRequest) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/calls/{}/exit",
            self.base_url(),
            call_id
        );
        let response = self
            .authorized_post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay post call exit request failed: {e}"))
            })?;
        self.parse_response_empty(response, "post_call_exit").await
    }

    pub async fn revoke_ack(&self, grant_id: &str) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/grants/{}/revoke-ack",
            self.base_url(),
            grant_id
        );
        let body = serde_json::json!({
            "client_instance_id": self.client_instance_id,
        });
        let response = self
            .authorized_post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("relay revoke ack request failed: {e}")))?;
        self.parse_response_empty(response, "revoke_ack").await
    }

    pub async fn list_grants(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/v4/remote-invoke/grants?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
        );
        let response =
            self.authorized_get(&url).send().await.map_err(|e| {
                BifrostError::Network(format!("relay list grants request failed: {e}"))
            })?;
        let wrapper: serde_json::Value = self
            .parse_response_with_data(response, "list_grants")
            .await?;
        match wrapper.get("list").and_then(|v| v.as_array()) {
            Some(arr) => Ok(arr.clone()),
            None => Ok(vec![]),
        }
    }

    pub async fn update_grant(
        &self,
        grant_id: &str,
        updates: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/v4/remote-invoke/grants/{}", self.base_url(), grant_id);
        let response = self
            .authorized_patch(&url)
            .json(updates)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay update grant request failed: {e}"))
            })?;
        self.parse_response_with_data::<serde_json::Value>(response, "update_grant")
            .await
    }

    pub async fn delete_grant(&self, grant_id: &str) -> Result<()> {
        let url = format!("{}/v4/remote-invoke/grants/{}", self.base_url(), grant_id);
        let response = self.authorized_delete(&url).send().await.map_err(|e| {
            BifrostError::Network(format!("relay delete grant request failed: {e}"))
        })?;
        self.parse_response_empty(response, "delete_grant").await
    }

    pub async fn list_calls(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/v4/remote-invoke/calls?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
        );
        let response =
            self.authorized_get(&url).send().await.map_err(|e| {
                BifrostError::Network(format!("relay list calls request failed: {e}"))
            })?;
        let wrapper: serde_json::Value = self
            .parse_response_with_data(response, "list_calls")
            .await?;
        match wrapper.get("list").and_then(|v| v.as_array()) {
            Some(arr) => Ok(arr.clone()),
            None => Ok(vec![]),
        }
    }

    pub async fn get_call(&self, call_id: &str) -> Result<serde_json::Value> {
        let url = format!("{}/v4/remote-invoke/calls/{}", self.base_url(), call_id);
        let response =
            self.authorized_get(&url).send().await.map_err(|e| {
                BifrostError::Network(format!("relay get call request failed: {e}"))
            })?;
        self.parse_response_with_data::<serde_json::Value>(response, "get_call")
            .await
    }

    pub fn build_stream_url(&self, stream_id: &str) -> String {
        let token_part = self
            .client_auth_token
            .read()
            .as_deref()
            .map(|t| format!("&client_auth_token={}", urlencoding::encode(t)))
            .unwrap_or_default();
        format!(
            "{}/v4/remote-invoke/client/stream?client_instance_id={}&stream_id={}{}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
            urlencoding::encode(stream_id),
            token_part,
        )
    }

    pub fn build_sse_request(&self, stream_id: &str) -> reqwest::RequestBuilder {
        let url = self.build_stream_url(stream_id);
        self.authorized_get(&url)
    }

    fn authorized_get(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.http.get(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    fn authorized_post(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.http.post(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    fn authorized_patch(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.http.patch(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    fn authorized_delete(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.http.delete(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    async fn parse_response_with_data<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> Result<T> {
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(BifrostError::Network(format!(
                "relay {operation} unauthorized"
            )));
        }

        let body = response.text().await.map_err(|e| {
            BifrostError::Network(format!("relay {operation} response read failed: {e}"))
        })?;

        if !status.is_success() {
            let preview = truncate_for_log(&body, 500);
            tracing::error!(
                target: "bifrost_admin::relay_client",
                %operation,
                status = status.as_u16(),
                response_body = %preview,
                "relay request returned non-success status"
            );
            return Err(BifrostError::Network(format!(
                "relay {operation} failed with status {status}: {preview}"
            )));
        }

        let envelope: RelayApiResponse<T> = serde_json::from_str(&body).map_err(|e| {
            let preview = truncate_for_log(&body, 500);
            BifrostError::Network(format!(
                "relay {operation} invalid response JSON: {e} body={preview}"
            ))
        })?;

        if envelope.code != 0 {
            let msg = envelope.message.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "relay {operation} returned error code {}: {msg}",
                envelope.code
            )));
        }

        envelope
            .data
            .ok_or_else(|| BifrostError::Network(format!("relay {operation} returned empty data")))
    }

    async fn parse_response_empty(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> Result<()> {
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(BifrostError::Network(format!(
                "relay {operation} unauthorized"
            )));
        }

        let body = response.text().await.map_err(|e| {
            BifrostError::Network(format!("relay {operation} response read failed: {e}"))
        })?;

        if !status.is_success() {
            let preview = truncate_for_log(&body, 500);
            tracing::error!(
                target: "bifrost_admin::relay_client",
                %operation,
                status = status.as_u16(),
                response_body = %preview,
                "relay request returned non-success status"
            );
            return Err(BifrostError::Network(format!(
                "relay {operation} failed with status {status}: {preview}"
            )));
        }

        if body.is_empty() {
            return Ok(());
        }

        let envelope: RelayApiResponse<serde_json::Value> =
            serde_json::from_str(&body).map_err(|e| {
                let preview = truncate_for_log(&body, 500);
                BifrostError::Network(format!(
                    "relay {operation} invalid response JSON: {e} body={preview}"
                ))
            })?;

        if envelope.code != 0 {
            let msg = envelope.message.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "relay {operation} returned error code {}: {msg}",
                envelope.code
            )));
        }

        Ok(())
    }
}

fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...(truncated, total {} bytes)", s.len())
    }
}
