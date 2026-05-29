use std::sync::OnceLock;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use super::{AuthState, BrowserCookie, RuntimeConfig, ACCOUNT_CHECK_PATH_PREFIX};

const NATIVE_HTTP_TIMEOUT_SECS: u64 = 20;

/// Reuse a single reqwest::Client across all ChatGPT API requests to avoid
/// repeated DNS lookups, TLS handshakes and connection pool recreation.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        bifrost_core::direct_reqwest_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(NATIVE_HTTP_TIMEOUT_SECS))
            .pool_max_idle_per_host(4)
            .build()
            .expect("build shared reqwest client")
    })
}

pub(super) async fn native_json(
    config: &RuntimeConfig,
    auth: &AuthState,
    path: &str,
) -> Result<Value, String> {
    let client = shared_client();
    let response = client
        .get(format!("{}{}", config.chatgpt.base_url, path))
        .headers(build_headers(auth)?)
        .timeout(std::time::Duration::from_secs(NATIVE_HTTP_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|error| format!("ChatGPT native HTTP request failed: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("read ChatGPT native HTTP response failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "auth_or_challenge: ChatGPT native HTTP {} for {}: {}",
            status.as_u16(),
            path,
            text.chars().take(300).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|error| format!("parse ChatGPT JSON failed: {error}"))
}

pub(super) struct NativeBinary {
    pub(super) bytes: bytes::Bytes,
    pub(super) content_type: Option<String>,
}

pub(super) async fn native_binary(auth: &AuthState, url: &str) -> Result<NativeBinary, String> {
    // Use a separate client that follows redirects — estuary/content may 302 to CDN.
    static BINARY_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = BINARY_CLIENT.get_or_init(|| {
        bifrost_core::direct_reqwest_client_builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(2)
            .build()
            .expect("build binary download client")
    });
    let response = client
        .get(url)
        .headers(build_headers(auth)?)
        .send()
        .await
        .map_err(|error| format!("ChatGPT native binary request failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    if !status.is_success() {
        let text = response
            .text()
            .await
            .unwrap_or_else(|error| format!("read error body failed: {error}"));
        return Err(format!(
            "auth_or_challenge: ChatGPT native binary HTTP {} for {}: {}",
            status.as_u16(),
            url,
            text.chars().take(300).collect::<String>()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("read ChatGPT native binary response failed: {error}"))?;
    Ok(NativeBinary {
        bytes,
        content_type,
    })
}

pub(super) struct AccountProbe {
    pub(super) status: u16,
    pub(super) logged_in: bool,
}

pub(super) async fn native_account_probe(
    config: &RuntimeConfig,
    auth: &AuthState,
) -> Result<AccountProbe, String> {
    let timezone_offset = chrono::Local::now().offset().local_minus_utc() / -60;
    let path =
        format!("{ACCOUNT_CHECK_PATH_PREFIX}/v4-2023-04-27?timezone_offset_min={timezone_offset}");
    let response = shared_client()
        .get(format!("{}{}", config.chatgpt.base_url, path))
        .headers(build_headers(auth)?)
        .timeout(std::time::Duration::from_secs(NATIVE_HTTP_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|error| format!("accounts/check request failed: {error}"))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|error| format!("accounts/check read failed: {error}"))?;
    if !(200..300).contains(&status) {
        return Ok(AccountProbe {
            status,
            logged_in: false,
        });
    }
    let json = serde_json::from_str::<Value>(&text)
        .map_err(|error| format!("accounts/check parse failed: {error}"))?;
    Ok(AccountProbe {
        status,
        logged_in: account_check_logged_in(&json),
    })
}

pub(super) fn account_check_logged_in(json: &Value) -> bool {
    let Some(accounts) = json.get("accounts").and_then(Value::as_object) else {
        return false;
    };
    accounts.values().any(|entry| {
        let account = entry.get("account");
        let account_id = account
            .and_then(|account| account.get("account_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let user_id = account
            .and_then(|account| account.get("account_user_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        !account_id.is_empty()
            && user_id.starts_with("user-")
            && entry
                .get("can_access_with_session")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            && !account
                .and_then(|account| account.get("is_deactivated"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
    })
}

fn build_headers(auth: &AuthState) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert("accept", HeaderValue::from_static("application/json"));
    headers.insert(
        "user-agent",
        HeaderValue::from_str(
            auth.captured_auth_headers
                .get("user-agent")
                .map(String::as_str)
                .unwrap_or(&auth.user_agent),
        )
        .map_err(|error| format!("invalid user-agent header: {error}"))?,
    );
    let cookie = cookie_header(&auth.cookies);
    if !cookie.is_empty() {
        headers.insert(
            "cookie",
            HeaderValue::from_str(&cookie)
                .map_err(|error| format!("invalid cookie header: {error}"))?,
        );
    }
    for (name, value) in &auth.captured_auth_headers {
        if name == "cookie" {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("invalid captured header name: {error}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|error| format!("invalid captured header value for {name}: {error}"))?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}

fn cookie_header(cookies: &[BrowserCookie]) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    cookies
        .iter()
        .filter(|cookie| cookie.domain.ends_with("chatgpt.com"))
        .filter(|cookie| !cookie.name.is_empty() && !cookie.value.is_empty())
        .filter(|cookie| cookie.expires.is_none_or(|exp| exp > now))
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; ")
}
