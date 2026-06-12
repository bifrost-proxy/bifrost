use std::time::Duration;

use bifrost_core::{
    direct_reqwest_client_builder, text::truncate_chars_with_suffix, BifrostError, Result,
};
use bifrost_storage::SyncConfig;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::types::{RemoteEnv, RemoteUser};

fn truncate_for_log(s: &str, max_len: usize) -> String {
    let suffix = format!("...(truncated, total {} bytes)", s.len());
    truncate_chars_with_suffix(s, max_len, &suffix)
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    code: i32,
    #[serde(rename = "message")]
    _message: String,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize, Default)]
struct EnvList {
    list: Vec<RemoteEnv>,
}

#[derive(Debug, Deserialize, Default)]
struct UserEnvelope {
    user_id: String,
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    avatar: String,
    #[serde(default)]
    email: String,
}

#[derive(Debug, Serialize)]
struct CreateEnvRequest<'a> {
    user_id: &'a str,
    name: &'a str,
    rule: &'a str,
}

#[derive(Debug, Serialize)]
struct UpdateEnvRequest<'a> {
    id: &'a str,
    user_id: &'a str,
    name: &'a str,
    rule: &'a str,
}

#[derive(Clone)]
pub struct SyncHttpClient {
    http: reqwest::Client,
}

impl SyncHttpClient {
    pub fn new(config: &SyncConfig) -> Result<Self> {
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_millis(config.connect_timeout_ms.max(500)))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("failed to build sync client: {e}")))?;
        Ok(Self { http })
    }

    pub async fn probe_reachable(&self, config: &SyncConfig) -> bool {
        let url = format!(
            "{}/v4/sso/check",
            config.remote_base_url.trim_end_matches('/')
        );
        match self.http.get(url).send().await {
            Ok(response) => response.status().is_success() || response.status().as_u16() == 401,
            Err(_) => false,
        }
    }

    pub fn login_url(&self, config: &SyncConfig, callback_url: &str) -> String {
        format!(
            "{}/v4/sso/login?next={}",
            config.remote_base_url.trim_end_matches('/'),
            urlencoding::encode(callback_url)
        )
    }

    pub fn login_url_with_reauth(&self, config: &SyncConfig, callback_url: &str) -> String {
        let login_url = self.login_url(config, callback_url);
        format!(
            "{}/v4/sso/logout?next={}",
            config.remote_base_url.trim_end_matches('/'),
            urlencoding::encode(&login_url)
        )
    }

    pub async fn get_user_info(
        &self,
        config: &SyncConfig,
        token: &str,
    ) -> Result<Option<RemoteUser>> {
        let url = format!(
            "{}/v4/sso/info",
            config.remote_base_url.trim_end_matches('/')
        );
        let response = self
            .http
            .get(url)
            .header("x-bifrost-token", token)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("sync user info request failed: {e}")))?;

        if response.status().as_u16() == 401 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(BifrostError::Network(format!(
                "sync user info returned HTTP {}",
                response.status()
            )));
        }
        let body = response
            .json::<ApiEnvelope<UserEnvelope>>()
            .await
            .map_err(|e| BifrostError::Network(format!("invalid sync user info response: {e}")))?;
        if body.code != 0 {
            return Err(BifrostError::Network(format!(
                "sync user info returned error code: {}",
                body.code
            )));
        }
        let Some(data) = body.data else {
            return Ok(None);
        };
        Ok(Some(RemoteUser {
            user_id: data.user_id,
            nickname: data.nickname,
            avatar: data.avatar,
            email: data.email,
        }))
    }

    pub async fn logout(&self, config: &SyncConfig, token: &str) -> Result<()> {
        let url = format!(
            "{}/v4/sso/logout",
            config.remote_base_url.trim_end_matches('/')
        );
        let _ = self
            .http
            .get(url)
            .header("x-bifrost-token", token)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("sync logout request failed: {e}")))?;
        Ok(())
    }

    pub async fn search_envs(
        &self,
        config: &SyncConfig,
        token: &str,
        user_id: &str,
    ) -> Result<Vec<RemoteEnv>> {
        let url = format!(
            "{}/v4/env?user_id={}&offset=0&limit=500",
            config.remote_base_url.trim_end_matches('/'),
            urlencoding::encode(user_id)
        );
        let response: ApiEnvelope<EnvList> = self
            .request_json(reqwest::Method::GET, &url, token, None::<&()>, None::<&()>)
            .await?;
        Ok(response.data.map(|data| data.list).unwrap_or_default())
    }

    pub async fn create_env(
        &self,
        config: &SyncConfig,
        token: &str,
        user_id: &str,
        name: &str,
        rule: &str,
    ) -> Result<RemoteEnv> {
        let url = format!("{}/v4/env", config.remote_base_url.trim_end_matches('/'));
        let response: ApiEnvelope<RemoteEnv> = self
            .request_json(
                reqwest::Method::POST,
                &url,
                token,
                None::<&()>,
                Some(&CreateEnvRequest {
                    user_id,
                    name,
                    rule,
                }),
            )
            .await?;
        response
            .data
            .ok_or_else(|| BifrostError::Network("sync create env returned empty data".to_string()))
    }

    pub async fn update_env(
        &self,
        config: &SyncConfig,
        token: &str,
        env: &RemoteEnv,
        rule: &str,
    ) -> Result<RemoteEnv> {
        let url = format!(
            "{}/v4/env/{}",
            config.remote_base_url.trim_end_matches('/'),
            env.id
        );
        let response: ApiEnvelope<RemoteEnv> = self
            .request_json(
                reqwest::Method::PATCH,
                &url,
                token,
                None::<&()>,
                Some(&UpdateEnvRequest {
                    id: &env.id,
                    user_id: &env.user_id,
                    name: &env.name,
                    rule,
                }),
            )
            .await?;
        response
            .data
            .ok_or_else(|| BifrostError::Network("sync update env returned empty data".to_string()))
    }

    pub async fn delete_env_by_id(
        &self,
        config: &SyncConfig,
        token: &str,
        remote_id: &str,
        remote_user_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/v4/env/{}?user_id={}",
            config.remote_base_url.trim_end_matches('/'),
            remote_id,
            urlencoding::encode(remote_user_id)
        );
        let response = self
            .http
            .delete(&url)
            .header("x-bifrost-token", token)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("sync request failed: {e}")))?;

        let status = response.status();
        if status.as_u16() == 401 {
            return Err(BifrostError::Network("sync unauthorized".to_string()));
        }
        if status.as_u16() == 404 {
            return Ok(());
        }

        let response_text = response.text().await.map_err(|e| {
            BifrostError::Network(format!(
                "sync response body read failed: {e} (method=DELETE url={url} status={status})"
            ))
        })?;

        if !status.is_success() {
            let preview = truncate_for_log(&response_text, 500);
            return Err(BifrostError::Network(format!(
                "sync delete failed with status {status} (url={url}): {preview}"
            )));
        }

        let _: ApiEnvelope<serde_json::Value> = serde_json::from_str(&response_text).map_err(|e| {
            let preview = truncate_for_log(&response_text, 500);
            BifrostError::Network(format!(
                "invalid sync response: {e} (method=DELETE url={url} status={status} body_preview={preview})"
            ))
        })?;
        Ok(())
    }

    pub async fn proxy_forward(
        &self,
        config: &SyncConfig,
        token: &str,
        method: reqwest::Method,
        path: &str,
        query: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, String, Vec<u8>)> {
        let mut url = format!("{}{}", config.remote_base_url.trim_end_matches('/'), path);
        if let Some(q) = query {
            if !q.is_empty() {
                url.push('?');
                url.push_str(q);
            }
        }
        let mut request = self
            .http
            .request(method, &url)
            .header("x-bifrost-token", token)
            .header("Content-Type", "application/json");
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("sync proxy request failed: {e}")))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let body = response
            .bytes()
            .await
            .map_err(|e| BifrostError::Network(format!("sync proxy response read failed: {e}")))?
            .to_vec();
        Ok((status, content_type, body))
    }

    async fn request_json<Q, B, T>(
        &self,
        method: reqwest::Method,
        url: &str,
        token: &str,
        query: Option<&Q>,
        body: Option<&B>,
    ) -> Result<T>
    where
        Q: Serialize + ?Sized,
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let method_str = method.to_string();
        let mut request = self
            .http
            .request(method, url)
            .header("x-bifrost-token", token)
            .header("Content-Type", "application/json");
        if let Some(query) = query {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("sync request failed: {e}")))?;
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(BifrostError::Network("sync unauthorized".to_string()));
        }

        let response_text = response.text().await.map_err(|e| {
            BifrostError::Network(format!(
                "sync response body read failed: {e} (method={method_str} url={url} status={status})"
            ))
        })?;

        if !status.is_success() {
            let preview = truncate_for_log(&response_text, 500);
            tracing::error!(
                target: "bifrost_sync::client",
                %method_str,
                %url,
                status = status.as_u16(),
                response_body = %preview,
                "sync request returned non-success status"
            );
            return Err(BifrostError::Network(format!(
                "sync request failed with status {status} (method={method_str} url={url}): {preview}"
            )));
        }

        serde_json::from_str::<T>(&response_text).map_err(|e| {
            let preview = truncate_for_log(&response_text, 500);
            tracing::error!(
                target: "bifrost_sync::client",
                %method_str,
                %url,
                status = status.as_u16(),
                error = %e,
                response_body = %preview,
                "failed to decode sync response JSON"
            );
            BifrostError::Network(format!(
                "invalid sync response: {e} (method={method_str} url={url} status={status} body_preview={preview})"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An address guaranteed to fail to connect quickly: TEST-NET-1 (RFC 5737)
    // is non-routable, so connect attempts fail fast without any listener.
    fn unreachable_config() -> SyncConfig {
        SyncConfig {
            remote_base_url: "http://192.0.2.1:9".to_string(),
            connect_timeout_ms: 500,
            ..SyncConfig::default()
        }
    }

    fn client() -> SyncHttpClient {
        SyncHttpClient::new(&unreachable_config()).expect("client builds")
    }

    #[test]
    fn truncate_for_log_passes_short_and_truncates_long() {
        // Short strings are returned unchanged.
        assert_eq!(truncate_for_log("hello", 100), "hello");
        // Long strings get truncated with a byte-count suffix.
        let long = "x".repeat(50);
        let out = truncate_for_log(&long, 10);
        assert!(out.contains("truncated"));
        assert!(out.contains("total 50 bytes"));
    }

    #[test]
    fn new_clamps_tiny_connect_timeout() {
        // connect_timeout_ms below 500 is clamped to 500 (no panic, builds ok).
        let cfg = SyncConfig {
            connect_timeout_ms: 1,
            ..SyncConfig::default()
        };
        assert!(SyncHttpClient::new(&cfg).is_ok());
    }

    #[test]
    fn login_url_encodes_callback_and_trims_slash() {
        let client = client();
        let cfg = SyncConfig {
            remote_base_url: "https://example.com/".to_string(),
            ..SyncConfig::default()
        };
        let url = client.login_url(&cfg, "http://127.0.0.1/cb?x=1");
        assert_eq!(
            url,
            "https://example.com/v4/sso/login?next=http%3A%2F%2F127.0.0.1%2Fcb%3Fx%3D1"
        );
    }

    #[test]
    fn login_url_with_reauth_wraps_logout_then_login() {
        let client = client();
        let cfg = SyncConfig {
            remote_base_url: "https://example.com".to_string(),
            ..SyncConfig::default()
        };
        let url = client.login_url_with_reauth(&cfg, "http://cb");
        assert!(url.starts_with("https://example.com/v4/sso/logout?next="));
        // The encoded inner login URL is present.
        assert!(url.contains("sso%2Flogin"));
    }

    #[tokio::test]
    async fn probe_reachable_is_false_when_unreachable() {
        let client = client();
        assert!(!client.probe_reachable(&unreachable_config()).await);
    }

    #[tokio::test]
    async fn get_user_info_errors_on_connection_failure() {
        let client = client();
        let err = client
            .get_user_info(&unreachable_config(), "tok")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn logout_errors_on_connection_failure() {
        let client = client();
        let err = client
            .logout(&unreachable_config(), "tok")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn search_envs_errors_on_connection_failure() {
        let client = client();
        let err = client
            .search_envs(&unreachable_config(), "tok", "user/id")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn create_env_errors_on_connection_failure() {
        let client = client();
        let err = client
            .create_env(&unreachable_config(), "tok", "u", "name", "rule")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn update_env_errors_on_connection_failure() {
        let client = client();
        let env = RemoteEnv {
            id: "e1".into(),
            user_id: "u1".into(),
            name: "n".into(),
            ..RemoteEnv::default()
        };
        let err = client
            .update_env(&unreachable_config(), "tok", &env, "rule")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn delete_env_by_id_errors_on_connection_failure() {
        let client = client();
        let err = client
            .delete_env_by_id(&unreachable_config(), "tok", "id", "uid")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn proxy_forward_errors_on_connection_failure() {
        let client = client();
        let err = client
            .proxy_forward(
                &unreachable_config(),
                "tok",
                reqwest::Method::POST,
                "/v4/env",
                Some("a=1"),
                Some(b"{}".to_vec()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn proxy_forward_empty_query_is_ignored() {
        // Exercises the `q.is_empty()` branch (query present but empty).
        let client = client();
        let err = client
            .proxy_forward(
                &unreachable_config(),
                "tok",
                reqwest::Method::GET,
                "/v4/env",
                Some(""),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    // --- Loopback server fixtures exercising response-decoding branches ---

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Spawn a one-shot loopback server that replies to a single request with a
    /// fixed raw HTTP response (status line + headers + body), then returns the
    /// base URL pointing at it.
    async fn spawn_canned_server(status_line: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0_u8; 4096];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });
        format!("http://{addr}")
    }

    fn config_for(base: &str) -> SyncConfig {
        SyncConfig {
            remote_base_url: base.to_string(),
            connect_timeout_ms: 500,
            ..SyncConfig::default()
        }
    }

    #[tokio::test]
    async fn get_user_info_returns_user_on_success() {
        let base = spawn_canned_server(
            "200 OK",
            r#"{"code":0,"message":"ok","data":{"user_id":"u1","nickname":"N","avatar":"a","email":"e@x"}}"#,
        )
        .await;
        let client = client();
        let user = client
            .get_user_info(&config_for(&base), "tok")
            .await
            .unwrap();
        assert_eq!(user.unwrap().user_id, "u1");
    }

    #[tokio::test]
    async fn get_user_info_returns_none_on_401() {
        let base = spawn_canned_server("401 Unauthorized", "").await;
        let client = client();
        let user = client
            .get_user_info(&config_for(&base), "tok")
            .await
            .unwrap();
        assert!(user.is_none());
    }

    #[tokio::test]
    async fn get_user_info_errors_on_non_success_status() {
        let base = spawn_canned_server("500 Internal Server Error", "boom").await;
        let client = client();
        let err = client
            .get_user_info(&config_for(&base), "tok")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn get_user_info_errors_on_nonzero_code() {
        let base =
            spawn_canned_server("200 OK", r#"{"code":7,"message":"nope","data":null}"#).await;
        let client = client();
        let err = client
            .get_user_info(&config_for(&base), "tok")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn get_user_info_returns_none_when_data_null() {
        let base = spawn_canned_server("200 OK", r#"{"code":0,"message":"ok","data":null}"#).await;
        let client = client();
        let user = client
            .get_user_info(&config_for(&base), "tok")
            .await
            .unwrap();
        assert!(user.is_none());
    }

    #[tokio::test]
    async fn search_envs_decodes_list_on_success() {
        let base = spawn_canned_server(
            "200 OK",
            r#"{"code":0,"message":"ok","data":{"list":[{"id":"e1","user_id":"u1","name":"n","rule":"r","create_time":"t","update_time":"t"}]}}"#,
        )
        .await;
        let client = client();
        let envs = client
            .search_envs(&config_for(&base), "tok", "u1")
            .await
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "e1");
    }

    #[tokio::test]
    async fn request_json_errors_on_401() {
        let base = spawn_canned_server("401 Unauthorized", "").await;
        let client = client();
        let err = client
            .search_envs(&config_for(&base), "tok", "u1")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn request_json_errors_on_invalid_json() {
        let base = spawn_canned_server("200 OK", "not-json").await;
        let client = client();
        let err = client
            .search_envs(&config_for(&base), "tok", "u1")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn request_json_errors_on_non_success_status() {
        let base = spawn_canned_server("503 Service Unavailable", "down").await;
        let client = client();
        let err = client
            .search_envs(&config_for(&base), "tok", "u1")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn delete_env_by_id_succeeds_on_ok() {
        let base = spawn_canned_server("200 OK", r#"{"code":0,"message":"ok","data":null}"#).await;
        let client = client();
        client
            .delete_env_by_id(&config_for(&base), "tok", "e1", "u1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_env_by_id_treats_404_as_success() {
        let base = spawn_canned_server("404 Not Found", "missing").await;
        let client = client();
        client
            .delete_env_by_id(&config_for(&base), "tok", "e1", "u1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_env_by_id_errors_on_401() {
        let base = spawn_canned_server("401 Unauthorized", "").await;
        let client = client();
        let err = client
            .delete_env_by_id(&config_for(&base), "tok", "e1", "u1")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn delete_env_by_id_errors_on_non_success_status() {
        let base = spawn_canned_server("500 Internal Server Error", "boom").await;
        let client = client();
        let err = client
            .delete_env_by_id(&config_for(&base), "tok", "e1", "u1")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn delete_env_by_id_errors_on_invalid_json() {
        let base = spawn_canned_server("200 OK", "not-json").await;
        let client = client();
        let err = client
            .delete_env_by_id(&config_for(&base), "tok", "e1", "u1")
            .await
            .unwrap_err();
        assert!(matches!(err, BifrostError::Network(_)));
    }

    #[tokio::test]
    async fn proxy_forward_returns_status_and_body() {
        let base = spawn_canned_server("200 OK", r#"{"ok":true}"#).await;
        let client = client();
        let (status, content_type, body) = client
            .proxy_forward(
                &config_for(&base),
                "tok",
                reqwest::Method::GET,
                "/v4/env",
                Some("a=1"),
                Some(b"{}".to_vec()),
            )
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert!(content_type.contains("application/json"));
        assert_eq!(body, br#"{"ok":true}"#);
    }

    #[tokio::test]
    async fn logout_succeeds_against_server() {
        let base = spawn_canned_server("200 OK", "").await;
        let client = client();
        client.logout(&config_for(&base), "tok").await.unwrap();
    }

    #[tokio::test]
    async fn probe_reachable_true_on_success_and_401() {
        let ok_base = spawn_canned_server("200 OK", "").await;
        let client = client();
        assert!(client.probe_reachable(&config_for(&ok_base)).await);

        let unauth_base = spawn_canned_server("401 Unauthorized", "").await;
        assert!(client.probe_reachable(&config_for(&unauth_base)).await);
    }
}
