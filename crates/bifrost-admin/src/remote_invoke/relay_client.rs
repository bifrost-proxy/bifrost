use std::time::Duration;

use bifrost_core::{
    apply_remote_relay_headers, direct_reqwest_client_builder, direct_sse_reqwest_client_builder,
    remote_relay_headers_from_env, BifrostError, Result, REMOTE_RELAY_HEADERS_ENV,
};
use parking_lot::RwLock;
use reqwest::header::{HeaderMap, ACCEPT_ENCODING};
use serde::{Deserialize, Serialize};

use super::types::{
    ClientCallExitRequest, ClientCallFrameRequest, ClientCallStreamFrameRequest,
    ClientHeartbeatRequest, ClientRegistrationChallengeRequest,
    ClientRegistrationChallengeResponse, ClientRegistrationRequest, ClientRegistrationResponse,
    GrantDecisionRequest, PublishPairCodeRequest, SshConnectResultRequest, UpdateGrantRequest,
};

const CALL_FRAME_SEND_RETRY_DELAYS_MS: &[u64] = &[150, 500, 1_000];

#[derive(Debug, Deserialize)]
struct RelayApiResponse<T> {
    code: i32,
    #[serde(default)]
    message: Option<String>,
    data: Option<T>,
}

pub struct RelayClient {
    http: reqwest::Client,
    sse_http: reqwest::Client,
    base_url: RwLock<String>,
    relay_headers: HeaderMap,
    client_auth_token: RwLock<Option<String>>,
    client_instance_id: String,
    device_name: String,
    platform: String,
}

impl RelayClient {
    pub fn new(
        base_url: &str,
        client_instance_id: &str,
        device_name: &str,
        platform: &str,
    ) -> Self {
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build relay http client");
        let sse_http = direct_sse_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build relay sse http client");
        let relay_headers = remote_relay_headers_from_env().unwrap_or_else(|error| {
            tracing::warn!(
                env = REMOTE_RELAY_HEADERS_ENV,
                error = %error,
                "ignoring invalid remote relay header configuration"
            );
            HeaderMap::new()
        });

        Self {
            http,
            sse_http,
            base_url: RwLock::new(base_url.trim_end_matches('/').to_string()),
            relay_headers,
            client_auth_token: RwLock::new(None),
            client_instance_id: client_instance_id.to_string(),
            device_name: device_name.to_string(),
            platform: platform.to_string(),
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
        user_auth_token: Option<&str>,
    ) -> Result<ClientRegistrationResponse> {
        let url = format!("{}/v4/remote-invoke/client/register", self.base_url());
        let mut request = self.relay_post(&url).json(req);
        if let Some(token) = user_auth_token {
            request = request.header("x-bifrost-token", token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("relay register request failed: {e}")))?;
        self.parse_response_with_data::<ClientRegistrationResponse>(response, "register")
            .await
    }

    pub async fn request_registration_challenge(
        &self,
        req: &ClientRegistrationChallengeRequest,
        user_auth_token: Option<&str>,
    ) -> Result<ClientRegistrationChallengeResponse> {
        let url = format!(
            "{}/v4/remote-invoke/client/register/challenge",
            self.base_url()
        );
        let mut request = self.relay_post(&url).json(req);
        if let Some(token) = user_auth_token {
            request = request.header("x-bifrost-token", token);
        }
        let response = request.send().await.map_err(|e| {
            BifrostError::Network(format!("relay register challenge request failed: {e}"))
        })?;
        self.parse_response_with_data::<ClientRegistrationChallengeResponse>(
            response,
            "request_registration_challenge",
        )
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
            .authorized_post_json_with_send_retry(&url, req, "relay post call frame")
            .await?;
        self.parse_response_empty(response, "post_call_frame").await
    }

    /// PR#6c: post a StreamFrame JSON to the relay `/stream-frame` endpoint.
    pub async fn post_call_stream_frame(
        &self,
        call_id: &str,
        req: &ClientCallStreamFrameRequest,
    ) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/calls/{}/stream-frame",
            self.base_url(),
            call_id
        );
        let response = self
            .authorized_post_json_with_send_retry(&url, req, "relay post call stream_frame")
            .await?;
        self.parse_response_empty(response, "post_call_stream_frame")
            .await
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

    pub async fn post_ssh_connect_result(&self, req: &SshConnectResultRequest) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/ssh/connect-result?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
        );
        let response = self
            .authorized_post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay post ssh connect result failed: {e}"))
            })?;
        self.parse_response_empty(response, "post_ssh_connect_result")
            .await
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

    pub async fn delete_grant(&self, grant_id: &str) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/grants/{}",
            self.base_url(),
            grant_id
        );
        let body = serde_json::json!({
            "client_instance_id": self.client_instance_id,
        });
        let response = self
            .authorized_delete(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay delete grant request failed: {e}"))
            })?;
        self.parse_response_empty(response, "delete_grant").await
    }

    pub async fn update_grant(
        &self,
        grant_id: &str,
        req: &UpdateGrantRequest,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/v4/remote-invoke/client/grants/{}",
            self.base_url(),
            grant_id
        );
        let response = self
            .authorized_patch(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("relay update grant request failed: {e}"))
            })?;
        self.parse_response_with_data::<serde_json::Value>(response, "update_grant")
            .await
    }

    pub async fn poll_pending_pairings(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/v4/remote-invoke/client/pending-pairings?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
        );
        let response = self.authorized_get(&url).send().await.map_err(|e| {
            BifrostError::Network(format!("relay poll_pending_pairings request failed: {e}"))
        })?;
        self.parse_response_with_data::<Vec<serde_json::Value>>(response, "poll_pending_pairings")
            .await
    }

    pub async fn cancel_pending_pairings(&self) -> Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/client/pending-pairings?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
        );
        let response = self.authorized_delete(&url).send().await.map_err(|e| {
            BifrostError::Network(format!("relay cancel_pending_pairings request failed: {e}"))
        })?;
        self.parse_response_empty(response, "cancel_pending_pairings")
            .await
    }

    pub async fn fetch_active_grants(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/v4/remote-invoke/client/active-grants?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
        );
        let response = self.authorized_get(&url).send().await.map_err(|e| {
            BifrostError::Network(format!("relay fetch_active_grants request failed: {e}"))
        })?;
        self.parse_response_with_data::<Vec<serde_json::Value>>(response, "fetch_active_grants")
            .await
    }

    pub async fn fetch_client_call(&self, call_id: &str) -> Result<serde_json::Value> {
        let url = format!(
            "{}/v4/remote-invoke/client/calls/{}?client_instance_id={}",
            self.base_url(),
            urlencoding::encode(call_id),
            urlencoding::encode(&self.client_instance_id),
        );
        let response = self.authorized_get(&url).send().await.map_err(|e| {
            BifrostError::Network(format!("relay fetch_client_call request failed: {e}"))
        })?;
        self.parse_response_with_data::<serde_json::Value>(response, "fetch_client_call")
            .await
    }

    pub fn build_stream_url(&self, stream_id: &str) -> String {
        format!(
            "{}/v4/remote-invoke/client/stream?client_instance_id={}&stream_id={}&client_name={}&platform={}",
            self.base_url(),
            urlencoding::encode(&self.client_instance_id),
            urlencoding::encode(stream_id),
            urlencoding::encode(&self.device_name),
            urlencoding::encode(&self.platform),
        )
    }

    pub fn build_sse_request(&self, stream_id: &str) -> reqwest::RequestBuilder {
        let url = self.build_stream_url(stream_id);
        let mut builder = apply_remote_relay_headers(self.sse_http.get(&url), &self.relay_headers)
            .header(ACCEPT_ENCODING, "identity")
            .header(reqwest::header::CACHE_CONTROL, "no-transform");
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    fn relay_get(&self, url: &str) -> reqwest::RequestBuilder {
        apply_remote_relay_headers(self.http.get(url), &self.relay_headers)
    }

    fn relay_post(&self, url: &str) -> reqwest::RequestBuilder {
        apply_remote_relay_headers(self.http.post(url), &self.relay_headers)
    }

    fn relay_delete(&self, url: &str) -> reqwest::RequestBuilder {
        apply_remote_relay_headers(self.http.delete(url), &self.relay_headers)
    }

    fn relay_patch(&self, url: &str) -> reqwest::RequestBuilder {
        apply_remote_relay_headers(self.http.patch(url), &self.relay_headers)
    }

    fn authorized_get(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.relay_get(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    fn authorized_post(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.relay_post(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    async fn authorized_post_json_with_send_retry<T>(
        &self,
        url: &str,
        body: &T,
        label: &str,
    ) -> Result<reqwest::Response>
    where
        T: Serialize + ?Sized,
    {
        let mut attempt = 0usize;
        loop {
            match self.authorized_post(url).json(body).send().await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if attempt >= CALL_FRAME_SEND_RETRY_DELAYS_MS.len() {
                        return Err(BifrostError::Network(format!(
                            "{label} request failed: {error}"
                        )));
                    }
                    let delay_ms = CALL_FRAME_SEND_RETRY_DELAYS_MS[attempt];
                    attempt += 1;
                    tracing::warn!(
                        error = %error,
                        attempt,
                        delay_ms,
                        "{label} request send failed, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }

    fn authorized_delete(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.relay_delete(url);
        if let Some(token) = self.client_auth_token.read().as_deref() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
    }

    fn authorized_patch(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.relay_patch(url);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn post_call_frame_retries_send_failure_once() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock relay");
        let addr = listener.local_addr().expect("mock relay addr");
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_server = Arc::clone(&attempts);
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.expect("accept mock relay connection");
                let attempt = attempts_for_server.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    drop(stream);
                    continue;
                }

                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                let body = r#"{"code":0,"message":null,"data":null}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write mock relay response");
                break;
            }
        });

        let client = RelayClient::new(
            &format!("http://{addr}"),
            "client-retry",
            "device",
            "platform",
        );
        client.set_auth_token("token".to_string());
        client
            .post_call_frame(
                "call-retry",
                &ClientCallFrameRequest {
                    call_id: "call-retry".to_string(),
                    client_instance_id: "client-retry".to_string(),
                    envelope_json: "{}".to_string(),
                },
            )
            .await
            .expect("post_call_frame should retry send failure");

        server.join().expect("mock relay thread");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
