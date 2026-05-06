//! HTTP client for Chat Completions API with tool calling support.

use crate::config::AgentConfig;
use crate::types::{ChatMessage, ModelResponse, TokenUsage, ToolCallMessage, ToolDefinition};
use bifrost_core::text::truncate_bytes_with_suffix;
use std::path::Path;
use tracing::{info, warn};

/// Environment variable that disables the default Bifrost model-request proxy
/// for embedded Bifrost services.
pub const AGENT_PROXY_DISABLE_ENV: &str = "BIFROST_AGENT_DISABLE_MODEL_PROXY";

/// HTTP client that calls a Chat Completions endpoint with tool support.
/// HTTP client that calls a Chat Completions endpoint with tool support.
#[derive(Clone)]
pub struct AgentClient {
    http: reqwest::Client,
    model_proxy_url: Option<String>,
}

impl Default for AgentClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder().build().unwrap_or_default(),
            model_proxy_url: None,
        }
    }

    /// Create a client whose model requests are routed through the running
    /// Bifrost HTTP proxy so Traffic can capture request/response details.
    pub fn new_with_bifrost_proxy(port: u16) -> Self {
        Self::new_with_bifrost_proxy_and_ca(port, None)
    }

    /// Create a proxied client and trust the running Bifrost CA when provided.
    pub fn new_with_bifrost_proxy_and_ca(port: u16, ca_cert_path: Option<&Path>) -> Self {
        if agent_proxy_disabled_by_env() {
            info!(
                env = AGENT_PROXY_DISABLE_ENV,
                "agent model proxy disabled by environment"
            );
            return Self::new();
        }

        let proxy_url = format!("http://127.0.0.1:{port}");
        Self::new_with_model_proxy_url_and_ca(proxy_url, ca_cert_path)
    }

    pub fn new_with_model_proxy_url(proxy_url: impl Into<String>) -> Self {
        Self::new_with_model_proxy_url_and_ca(proxy_url, None)
    }

    pub fn new_with_model_proxy_url_and_ca(
        proxy_url: impl Into<String>,
        ca_cert_path: Option<&Path>,
    ) -> Self {
        let proxy_url = proxy_url.into();
        let mut builder = reqwest::Client::builder();
        match reqwest::Proxy::all(&proxy_url) {
            Ok(proxy) => {
                builder = builder.proxy(proxy);
            }
            Err(error) => {
                warn!(
                    proxy_url = %proxy_url,
                    error = %error,
                    "invalid agent model proxy URL; falling back to direct model requests"
                );
                return Self::new();
            }
        }
        if let Some(path) = ca_cert_path {
            match load_reqwest_certificate(path) {
                Ok(cert) => {
                    builder = builder.add_root_certificate(cert);
                    info!(
                        proxy_url = %proxy_url,
                        ca_cert_path = %path.display(),
                        "agent model proxy trusts Bifrost CA"
                    );
                }
                Err(error) => {
                    warn!(
                        proxy_url = %proxy_url,
                        ca_cert_path = %path.display(),
                        error = %error,
                        "agent model proxy could not load Bifrost CA; TLS-intercepted model requests may fail"
                    );
                }
            }
        }

        let http = match builder.build() {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    proxy_url = %proxy_url,
                    error = %error,
                    "failed to build proxied agent HTTP client; falling back to direct model requests"
                );
                return Self::new();
            }
        };

        Self {
            http,
            model_proxy_url: Some(proxy_url),
        }
    }

    pub fn model_proxy_url(&self) -> Option<&str> {
        self.model_proxy_url.as_deref()
    }

    /// Send a chat completion request with optional tool definitions.
    /// Returns a structured ModelResponse with content and/or tool_calls.
    pub async fn chat_completion(
        &self,
        config: &AgentConfig,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ModelResponse, String> {
        self.chat_completion_with_schema(config, messages, tools, None)
            .await
    }

    /// Send a chat completion request with optional JSON Schema structured output constraint.
    /// When `output_schema` is provided, the model response is constrained to conform to the schema.
    /// Aligned with Codex Phase 1 structured output pattern.
    pub async fn chat_completion_with_schema(
        &self,
        config: &AgentConfig,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        output_schema: Option<&serde_json::Value>,
    ) -> Result<ModelResponse, String> {
        let effective = config.resolve_effective_config()?;
        let url = effective.base_url.trim_end_matches('/').to_string();

        // Build request body
        let mut body = serde_json::json!({
            "model": effective.model,
            "messages": messages,
            "max_completion_tokens": effective.max_completion_tokens,
            "stream": false,
        });

        if !tools.is_empty() {
            body["tools"] =
                serde_json::to_value(tools).map_err(|e| format!("serialize tools: {e}"))?;
        }

        // P2-2: JSON Schema structured output constraint (Codex-aligned)
        if let Some(schema) = output_schema {
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "structured_output",
                    "strict": true,
                    "schema": schema
                }
            });
        }

        if let Some(ref effort) = effective.reasoning_effort {
            body["reasoning_effort"] = serde_json::json!(effort);
        }
        if let Some(ref summary) = effective.reasoning_summary {
            body["reasoning_summary"] = serde_json::json!(summary);
        }

        info!(
            url = %url,
            model = %effective.model,
            message_count = messages.len(),
            tool_count = tools.len(),
            api_key_len = effective.api_key.len(),
            extra_headers_count = effective.extra_headers.len(),
            use_azure_auth = effective.use_azure_auth,
            model_proxy_url = self.model_proxy_url.as_deref().unwrap_or("direct"),
            "sending chat completion request"
        );

        // Build HTTP request
        let mut request = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(
                effective.request_timeout_secs,
            ));

        if effective.use_azure_auth {
            request = request.header("api-key", &effective.api_key);
        } else {
            request = request.header("Authorization", format!("Bearer {}", effective.api_key));
        }
        for (key, value) in &effective.extra_headers {
            request = request.header(key.as_str(), value.as_str());
        }

        // Send & parse
        let response = request.json(&body).send().await.map_err(|e| {
            // Unfold the reqwest error source chain so operators can see the
            // underlying cause (TLS handshake, DNS resolve, connect refused,
            // etc.) instead of the generic top-level message.
            let mut chain = format!("HTTP request failed: {e}");
            let mut src: &dyn std::error::Error = &e;
            let mut i = 0usize;
            while let Some(next) = src.source() {
                use std::fmt::Write as _;
                let _ = write!(chain, " | cause[{i}]: {next}");
                src = next;
                i += 1;
                if i >= 8 {
                    break;
                }
            }
            chain
        })?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(format!(
                "API error (status {}): {}",
                status,
                truncate(&error_body, 500)
            ));
        }

        let resp_text = response
            .text()
            .await
            .map_err(|e| format!("failed to read response: {e}"))?;

        let resp: serde_json::Value =
            serde_json::from_str(&resp_text).map_err(|e| format!("failed to parse JSON: {e}"))?;

        self.parse_response(&resp)
    }

    fn parse_response(&self, resp: &serde_json::Value) -> Result<ModelResponse, String> {
        let choice = resp
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or("no choices in response")?;

        let message = choice.get("message").ok_or("no message in choice")?;

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("stop")
            .to_string();

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        let reasoning_content = message
            .get("reasoning_content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        // Parse function tool calls.
        let tool_calls: Vec<ToolCallMessage> = message
            .get("tool_calls")
            .and_then(|tc| serde_json::from_value(tc.clone()).ok())
            .unwrap_or_default();

        // Parse usage
        let usage = resp.get("usage").map(|u| TokenUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            completion_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        });

        if let Some(ref u) = usage {
            info!(
                prompt_tokens = u.prompt_tokens,
                completion_tokens = u.completion_tokens,
                total_tokens = u.total_tokens,
                finish_reason = %finish_reason,
                tool_calls_count = tool_calls.len(),
                "model response received"
            );
        }

        Ok(ModelResponse {
            content,
            reasoning_content,
            tool_calls,
            finish_reason,
            usage,
        })
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    truncate_bytes_with_suffix(s, max_len, "...")
}

fn load_reqwest_certificate(path: &Path) -> Result<reqwest::Certificate, String> {
    let pem = std::fs::read(path).map_err(|error| format!("read CA certificate: {error}"))?;
    reqwest::Certificate::from_pem(&pem).map_err(|error| format!("parse CA certificate: {error}"))
}

fn agent_proxy_disabled_by_env() -> bool {
    std::env::var(AGENT_PROXY_DISABLE_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_client_uses_direct_model_requests() {
        let client = AgentClient::new();
        assert_eq!(client.model_proxy_url(), None);
    }

    #[test]
    fn bifrost_proxy_client_targets_local_started_port() {
        let client = AgentClient::new_with_bifrost_proxy(18888);
        assert_eq!(client.model_proxy_url(), Some("http://127.0.0.1:18888"));
    }

    #[test]
    fn bifrost_proxy_client_accepts_bifrost_ca_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cert_path = dir.path().join("ca.crt");
        std::fs::write(
            &cert_path,
            include_bytes!("../../bifrost-tls/testdata/test-rsa-ca.crt"),
        )
        .expect("write cert");

        let client = AgentClient::new_with_bifrost_proxy_and_ca(18889, Some(&cert_path));
        assert_eq!(client.model_proxy_url(), Some("http://127.0.0.1:18889"));
    }
}
