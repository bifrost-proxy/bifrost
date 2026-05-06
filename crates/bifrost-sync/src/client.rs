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
