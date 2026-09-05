//! Traffic export (curl / fetch / HAR) and JSON Patch based replay.
//!
//! 该模块不依赖 json-patch crate，按 RFC 6902 实现 `replace` / `add` / `remove` 子集。
//!
//! 公共 API:
//! - [`ExportFormat`], [`ExportOptions`], [`export_request`]
//! - [`JsonPatchOp`], [`ReplayOptions`], [`ReplayResult`], [`HttpClient`], [`ReqwestClient`],
//!   [`replay_request`]
//!
//! 设计原则:
//! 1. 导出使用原始捕获数据；本期不做脱敏，后续单独设计完整方案。
//! 2. 重放使用注入式 `HttpClient` trait，便于单元测试不需要 wiremock。
//! 3. JSON Patch 仅实现 `replace` / `add` / `remove` 三种，path 走 JSON Pointer (RFC 6901)。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::replay_content_encoding::{content_encoding_value, encode_content_encoded_body};

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Curl,
    Fetch,
    Har,
}

impl ExportFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "curl" => Some(Self::Curl),
            "fetch" => Some(Self::Fetch),
            "har" => Some(Self::Har),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    pub format: ExportFormat,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Curl,
        }
    }
}

/// 导出请求模板。
///
/// `body` 是原始请求体字节（已 decode 的，或调用方愿意展示的形式）。
/// 二进制体在 curl/fetch 中以 `--data-binary @-` / `<binary N bytes>` 占位呈现，避免破坏文本输出。
pub fn export_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    opts: &ExportOptions,
) -> String {
    let headers_owned: Vec<(String, String)> = headers.to_vec();

    let body_owned: Option<Vec<u8>> = body.map(|b| b.to_vec());

    match opts.format {
        ExportFormat::Curl => export_curl(method, url, &headers_owned, body_owned.as_deref()),
        ExportFormat::Fetch => export_fetch(method, url, &headers_owned, body_owned.as_deref()),
        ExportFormat::Har => export_har(method, url, &headers_owned, body_owned.as_deref()),
    }
}

fn shell_single_quote(s: &str) -> String {
    // POSIX 单引号转义：' -> '\''
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn export_curl(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> String {
    let mut out = String::new();
    out.push_str("curl -X ");
    out.push_str(&method.to_ascii_uppercase());
    out.push(' ');
    out.push_str(&shell_single_quote(url));
    for (k, v) in headers {
        out.push_str(" \\\n  -H ");
        out.push_str(&shell_single_quote(&format!("{k}: {v}")));
    }
    if let Some(b) = body {
        if !b.is_empty() {
            if let Ok(text) = std::str::from_utf8(b) {
                out.push_str(" \\\n  --data-binary ");
                out.push_str(&shell_single_quote(text));
            } else {
                out.push_str(&format!(
                    " \\\n  --data-binary @- # <binary {} bytes>",
                    b.len()
                ));
            }
        }
    }
    out
}

fn export_fetch(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> String {
    // 输出为 JS fetch(...) 调用片段
    let mut headers_obj = serde_json::Map::new();
    for (k, v) in headers {
        headers_obj.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    let mut init = serde_json::Map::new();
    init.insert(
        "method".to_string(),
        serde_json::Value::String(method.to_ascii_uppercase()),
    );
    init.insert(
        "headers".to_string(),
        serde_json::Value::Object(headers_obj),
    );
    if let Some(b) = body {
        if !b.is_empty() {
            match std::str::from_utf8(b) {
                Ok(text) => {
                    init.insert(
                        "body".to_string(),
                        serde_json::Value::String(text.to_string()),
                    );
                }
                Err(_) => {
                    init.insert(
                        "body".to_string(),
                        serde_json::Value::String(format!("<binary {} bytes>", b.len())),
                    );
                }
            }
        }
    }
    let init_str = serde_json::to_string_pretty(&serde_json::Value::Object(init))
        .unwrap_or_else(|_| "{}".to_string());
    format!(
        "fetch({}, {});",
        serde_json::Value::String(url.to_string()),
        init_str
    )
}

fn export_har(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> String {
    let har_headers: Vec<serde_json::Value> = headers
        .iter()
        .map(|(k, v)| serde_json::json!({"name": k, "value": v}))
        .collect();
    let post_data = body.and_then(|b| {
        if b.is_empty() {
            None
        } else {
            match std::str::from_utf8(b) {
                Ok(text) => Some(serde_json::json!({
                    "mimeType": "application/octet-stream",
                    "text": text,
                })),
                Err(_) => Some(serde_json::json!({
                    "mimeType": "application/octet-stream",
                    "encoding": "base64",
                    "text": base64::engine::general_purpose::STANDARD.encode(b),
                })),
            }
        }
    });
    let mut request = serde_json::json!({
        "method": method.to_ascii_uppercase(),
        "url": url,
        "httpVersion": "HTTP/1.1",
        "headers": har_headers,
        "queryString": [],
        "cookies": [],
        "headersSize": -1,
        "bodySize": body.map(|b| b.len() as i64).unwrap_or(-1),
    });
    if let Some(pd) = post_data {
        request["postData"] = pd;
    }
    let har = serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "bifrost", "version": env!("CARGO_PKG_VERSION")},
            "entries": [{
                "startedDateTime": "1970-01-01T00:00:00Z",
                "time": 0,
                "request": request,
                "response": {
                    "status": 0,
                    "statusText": "",
                    "httpVersion": "HTTP/1.1",
                    "headers": [],
                    "cookies": [],
                    "content": {"size": 0, "mimeType": ""},
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": -1,
                },
                "cache": {},
                "timings": {"send": 0, "wait": 0, "receive": 0},
            }],
        }
    });
    serde_json::to_string_pretty(&har).unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------
// JSON Patch (RFC 6902 subset: replace / add / remove)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum JsonPatchOp {
    Replace {
        path: String,
        value: serde_json::Value,
    },
    Add {
        path: String,
        value: serde_json::Value,
    },
    Remove {
        path: String,
    },
}

/// 解析 RFC 6901 JSON Pointer 为 token 数组（已做 ~1/~0 解码）。
fn parse_json_pointer(path: &str) -> Result<Vec<String>, String> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    if !path.starts_with('/') {
        return Err(format!(
            "invalid JSON Pointer (must start with '/'): {path}"
        ));
    }
    Ok(path[1..]
        .split('/')
        .map(|seg| seg.replace("~1", "/").replace("~0", "~"))
        .collect())
}

/// 应用一组 JSON Patch 操作。
pub fn apply_json_patch(value: &mut serde_json::Value, ops: &[JsonPatchOp]) -> Result<(), String> {
    for op in ops {
        match op {
            JsonPatchOp::Replace { path, value: v } => {
                patch_set(value, path, v.clone(), false)?;
            }
            JsonPatchOp::Add { path, value: v } => {
                patch_set(value, path, v.clone(), true)?;
            }
            JsonPatchOp::Remove { path } => {
                patch_remove(value, path)?;
            }
        }
    }
    Ok(())
}

fn patch_set(
    root: &mut serde_json::Value,
    path: &str,
    new_value: serde_json::Value,
    is_add: bool,
) -> Result<(), String> {
    let tokens = parse_json_pointer(path)?;
    if tokens.is_empty() {
        *root = new_value;
        return Ok(());
    }
    let (last, parents) = tokens.split_last().unwrap();
    let mut cursor = root;
    for tok in parents {
        cursor = step_into(cursor, tok)?;
    }
    match cursor {
        serde_json::Value::Object(map) => {
            map.insert(last.clone(), new_value);
            Ok(())
        }
        serde_json::Value::Array(arr) => {
            if last == "-" {
                if !is_add {
                    return Err(format!("replace on '-' is not allowed: {path}"));
                }
                arr.push(new_value);
                return Ok(());
            }
            let idx: usize = last
                .parse()
                .map_err(|_| format!("invalid array index '{last}' at {path}"))?;
            if is_add {
                if idx > arr.len() {
                    return Err(format!("array index {idx} out of bounds at {path}"));
                }
                arr.insert(idx, new_value);
            } else {
                if idx >= arr.len() {
                    return Err(format!("array index {idx} out of bounds at {path}"));
                }
                arr[idx] = new_value;
            }
            Ok(())
        }
        _ => Err(format!("cannot set into non-object/array at {path}")),
    }
}

fn patch_remove(root: &mut serde_json::Value, path: &str) -> Result<(), String> {
    let tokens = parse_json_pointer(path)?;
    if tokens.is_empty() {
        *root = serde_json::Value::Null;
        return Ok(());
    }
    let (last, parents) = tokens.split_last().unwrap();
    let mut cursor = root;
    for tok in parents {
        cursor = step_into(cursor, tok)?;
    }
    match cursor {
        serde_json::Value::Object(map) => {
            if map.remove(last).is_none() {
                return Err(format!("missing key '{last}' at {path}"));
            }
            Ok(())
        }
        serde_json::Value::Array(arr) => {
            let idx: usize = last
                .parse()
                .map_err(|_| format!("invalid array index '{last}' at {path}"))?;
            if idx >= arr.len() {
                return Err(format!("array index {idx} out of bounds at {path}"));
            }
            arr.remove(idx);
            Ok(())
        }
        _ => Err(format!("cannot remove from non-object/array at {path}")),
    }
}

fn step_into<'a>(
    cursor: &'a mut serde_json::Value,
    token: &str,
) -> Result<&'a mut serde_json::Value, String> {
    match cursor {
        serde_json::Value::Object(map) => map
            .get_mut(token)
            .ok_or_else(|| format!("missing key '{token}'")),
        serde_json::Value::Array(arr) => {
            let idx: usize = token
                .parse()
                .map_err(|_| format!("invalid array index '{token}'"))?;
            arr.get_mut(idx)
                .ok_or_else(|| format!("array index {idx} out of bounds"))
        }
        _ => Err(format!("cannot descend into non-object/array at '{token}'")),
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplayOptions {
    #[serde(default)]
    pub patch_json: Vec<JsonPatchOp>,
    /// 兼容字段：早期版本传 `"latest"` 启用 refresh-auth。任意非空字符串等价于
    /// `refresh_auth = true`。新代码应使用 [`Self::refresh_auth`]。
    #[serde(default)]
    pub refresh_auth_from: Option<String>,
    /// 是否启用 refresh-auth：从历史记录中找最新的认证 header 套到 replay 请求。
    #[serde(default)]
    pub refresh_auth: bool,
    /// 限定从哪个 host 取 auth header，缺省时使用 replay 请求自身的 host。
    #[serde(default)]
    pub auth_source_host: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

impl ReplayOptions {
    /// 兼容旧字段：`refresh_auth_from` 非空 → 启用 refresh-auth。
    pub fn refresh_auth_enabled(&self) -> bool {
        self.refresh_auth
            || self
                .refresh_auth_from
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false)
    }
}

fn default_timeout_ms() -> u64 {
    30_000
}

/// refresh-auth 候选记录的最小抽象（host + 请求 header），便于单测注入。
#[derive(Debug, Clone)]
pub struct AuthCandidate {
    pub id: String,
    pub host: String,
    pub headers: Vec<(String, String)>,
    /// 历史时间戳，越大越新（ms / epoch / sequence 均可；只用于排序）。
    pub recency: u64,
}

/// refresh-auth 命中后的元信息，回传给调用方。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthRefreshInfo {
    pub applied: bool,
    pub source_traffic_id: Option<String>,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// base64 编码的响应体。
    pub body_b64: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_refresh: Option<AuthRefreshInfo>,
}

/// 抽象 HTTP client，便于单元测试注入。
#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(
        &self,
        method: &str,
        url: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<RawResponse, String>;
}

/// trait 的返回原始 response。
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// 基于 `reqwest` 的默认实现。
pub struct ReqwestClient;

#[async_trait::async_trait]
impl HttpClient for ReqwestClient {
    async fn execute(
        &self,
        method: &str,
        url: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<RawResponse, String> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| format!("invalid method: {e}"))?;
        let client = bifrost_core::outbound_reqwest_client_builder()
            .timeout(timeout)
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| format!("build client: {e}"))?;
        let mut req = client.request(method, url);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if !body.is_empty() {
            req = req.body(body);
        }
        let resp = req.send().await.map_err(|e| format!("send: {e}"))?;
        let status = resp.status().as_u16();
        let mut header_vec: Vec<(String, String)> = Vec::new();
        for (k, v) in resp.headers().iter() {
            header_vec.push((k.as_str().to_string(), v.to_str().unwrap_or("").to_string()));
        }
        let body_bytes = resp.bytes().await.map_err(|e| format!("body: {e}"))?;
        Ok(RawResponse {
            status,
            headers: header_vec,
            body: body_bytes.to_vec(),
        })
    }
}

// ---------------------------------------------------------------------------
// Refresh-auth helpers
// ---------------------------------------------------------------------------

/// 把一个 header 名归类为 refresh-auth 关心的字段。
/// 命中规则：authorization / cookie / x-tt-* (x-tt-token / x-tt-passport-* 等)。
pub fn classify_auth_header_field(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower == "authorization" {
        return Some("authorization");
    }
    if lower == "cookie" {
        return Some("cookie");
    }
    if lower.starts_with("x-tt-") {
        return Some("x-tt-*");
    }
    None
}

/// 从一组候选历史记录中找一条能作为 refresh-auth 源的记录。
///
/// 规则：
/// 1. host 完全匹配（大小写不敏感）。
/// 2. 至少包含一条 [`classify_auth_header_field`] 命中的 header。
/// 3. recency 最大者优先。
///
/// 命中后返回 `(traffic_id, auth_headers)`，其中 auth_headers 仅保留被识别的那几条。
pub fn find_refresh_auth_source<'a>(
    candidates: &'a [AuthCandidate],
    target_host: &str,
) -> Option<(String, Vec<(String, String)>)> {
    let host = target_host.to_ascii_lowercase();
    let mut best: Option<&'a AuthCandidate> = None;
    for c in candidates {
        if c.host.to_ascii_lowercase() != host {
            continue;
        }
        let has_auth = c
            .headers
            .iter()
            .any(|(k, _)| classify_auth_header_field(k).is_some());
        if !has_auth {
            continue;
        }
        match best {
            None => best = Some(c),
            Some(prev) if c.recency > prev.recency => best = Some(c),
            _ => {}
        }
    }
    let pick = best?;
    let auth_headers: Vec<(String, String)> = pick
        .headers
        .iter()
        .filter(|(k, _)| classify_auth_header_field(k).is_some())
        .cloned()
        .collect();
    Some((pick.id.clone(), auth_headers))
}

/// 把 `auth_headers` 应用到 `target_headers`：相同 header 名（大小写不敏感）会被覆盖；
/// 不存在则追加。返回实际被覆盖/新增的 header 名（保留原大小写）的 unique 列表。
pub fn apply_refresh_auth(
    target_headers: &mut Vec<(String, String)>,
    auth_headers: &[(String, String)],
) -> Vec<String> {
    let mut applied: Vec<String> = Vec::new();
    for (k, v) in auth_headers {
        let lower = k.to_ascii_lowercase();
        let mut replaced = false;
        for (existing_k, existing_v) in target_headers.iter_mut() {
            if existing_k.to_ascii_lowercase() == lower {
                *existing_v = v.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            target_headers.push((k.clone(), v.clone()));
        }
        if !applied.iter().any(|x| x.eq_ignore_ascii_case(k)) {
            applied.push(k.clone());
        }
    }
    applied
}

/// 重放一个请求。
///
/// 步骤：
/// 1. 如 patch_json 非空，先按 Content-Encoding 严格解压，再把明文 body 当 JSON 解析并
///    应用 patch，最后按原编码链重新压缩；非 JSON、未知编码或损坏压缩流均返回错误。
/// 2. 移除 Bifrost 控制类 / hop-by-hop / Content-Length 等 header，让 http client 自己重新计算。
/// 3. 若 `opts.refresh_auth_enabled()`，从 `auth_candidates` 找最新的同 host 认证记录，
///    把 Authorization / Cookie / X-Tt-* header 覆盖到 replay 请求上。
/// 4. 用 client 执行，返回 [`ReplayResult`]。
pub async fn replay_request(
    client: &dyn HttpClient,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
    opts: &ReplayOptions,
    auth_candidates: &[AuthCandidate],
) -> Result<ReplayResult, String> {
    let mut body_bytes: Vec<u8> = body.to_vec();
    if !opts.patch_json.is_empty() {
        let content_encoding = content_encoding_value(headers);
        let decoded_body = match content_encoding.as_deref() {
            Some(encoding) => crate::handlers::network_body::decompress_with_limit(
                &body_bytes,
                encoding,
                crate::handlers::network_body::DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
            )
            .map_err(|error| {
                format!("patch_json failed to decode {encoding} request body: {error}")
            })?,
            None => body_bytes,
        };
        let mut value: serde_json::Value = serde_json::from_slice(&decoded_body)
            .map_err(|e| format!("patch_json requires JSON body: {e}"))?;
        apply_json_patch(&mut value, &opts.patch_json)?;
        let patched_body = serde_json::to_vec(&value).map_err(|e| format!("re-serialize: {e}"))?;
        body_bytes = match content_encoding.as_deref() {
            Some(encoding) => {
                encode_content_encoded_body(&patched_body, encoding).map_err(|error| {
                    format!("patch_json failed to encode {encoding} request body: {error}")
                })?
            }
            None => patched_body,
        };
    }

    let strip: HashSet<&str> = [
        "content-length",
        "host",
        "connection",
        "transfer-encoding",
        "keep-alive",
        "proxy-connection",
        "te",
        "trailer",
        "upgrade",
    ]
    .into_iter()
    .collect();
    let mut filtered_headers: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| !strip.contains(k.to_ascii_lowercase().as_str()))
        .cloned()
        .collect();

    // refresh-auth：找历史记录里同 host 的最新认证 header，覆盖到 replay 请求。
    let mut auth_refresh: Option<AuthRefreshInfo> = None;
    if opts.refresh_auth_enabled() {
        let target_host = opts
            .auth_source_host
            .clone()
            .or_else(|| extract_host_from_url(url))
            .unwrap_or_default();
        if let Some((source_id, auth_headers)) =
            find_refresh_auth_source(auth_candidates, &target_host)
        {
            let applied = apply_refresh_auth(&mut filtered_headers, &auth_headers);
            auth_refresh = Some(AuthRefreshInfo {
                applied: !applied.is_empty(),
                source_traffic_id: Some(source_id),
                fields: applied,
            });
        } else {
            auth_refresh = Some(AuthRefreshInfo {
                applied: false,
                source_traffic_id: None,
                fields: Vec::new(),
            });
        }
    }

    let timeout = Duration::from_millis(opts.timeout_ms.max(1));
    let started = Instant::now();
    let resp = client
        .execute(method, url, filtered_headers, body_bytes, timeout)
        .await?;
    let duration_ms = started.elapsed().as_millis() as u64;

    Ok(ReplayResult {
        status: resp.status,
        headers: resp.headers,
        body_b64: base64::engine::general_purpose::STANDARD.encode(&resp.body),
        duration_ms,
        auth_refresh,
    })
}

/// 从 URL 中解析 host（不带端口）。失败返回 None。
fn extract_host_from_url(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host_with_port = rest.split('/').next().unwrap_or("");
    let host = host_with_port
        .split('@')
        .next_back()
        .unwrap_or(host_with_port);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_headers() -> Vec<(String, String)> {
        vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Authorization".to_string(),
                "Bearer abc.def.ghi".to_string(),
            ),
            ("X-Trace-Token".to_string(), "secret-trace".to_string()),
            ("X-Request-Id".to_string(), "req-123".to_string()),
        ]
    }

    fn sample_body() -> Vec<u8> {
        br#"{"user":"alice","password":"p@ss","payload":{"jwt":"xx.yy.zz","other":1}}"#.to_vec()
    }

    #[test]
    fn export_curl_contains_method_url_headers_and_body() {
        let body = sample_body();
        let opts = ExportOptions {
            format: ExportFormat::Curl,
        };
        let s = export_request(
            "POST",
            "https://example.com/api/v1/x",
            &sample_headers(),
            Some(&body),
            &opts,
        );
        assert!(s.starts_with("curl -X POST"));
        assert!(s.contains("'https://example.com/api/v1/x'"));
        assert!(s.contains("'Authorization: Bearer abc.def.ghi'"));
        assert!(s.contains("'X-Trace-Token: secret-trace'"));
        assert!(s.contains("--data-binary"));
        assert!(s.contains("\"password\":\"p@ss\""));
    }

    #[test]
    fn export_fetch_emits_js_snippet() {
        let body = sample_body();
        let opts = ExportOptions {
            format: ExportFormat::Fetch,
        };
        let s = export_request(
            "POST",
            "https://example.com/api/v1/x",
            &sample_headers(),
            Some(&body),
            &opts,
        );
        assert!(s.starts_with("fetch("));
        assert!(s.contains("\"method\": \"POST\""));
        assert!(s.contains("\"Authorization\": \"Bearer abc.def.ghi\""));
        assert!(s.contains("\"body\":"));
    }

    #[test]
    fn export_har_parses_as_json_with_entry() {
        let body = sample_body();
        let opts = ExportOptions {
            format: ExportFormat::Har,
        };
        let s = export_request(
            "POST",
            "https://example.com/api/v1/x",
            &sample_headers(),
            Some(&body),
            &opts,
        );
        let v: serde_json::Value = serde_json::from_str(&s).expect("HAR must be valid JSON");
        assert_eq!(v["log"]["version"], "1.2");
        let entry = &v["log"]["entries"][0];
        assert_eq!(entry["request"]["method"], "POST");
        assert_eq!(entry["request"]["url"], "https://example.com/api/v1/x");
        let req_headers = entry["request"]["headers"].as_array().unwrap();
        let auth = req_headers
            .iter()
            .find(|h| h["name"] == "Authorization")
            .unwrap();
        assert_eq!(auth["value"], "Bearer abc.def.ghi");
        assert!(entry["request"]["postData"]["text"]
            .as_str()
            .unwrap()
            .contains("\"password\":\"p@ss\""));
    }

    #[test]
    fn apply_json_patch_replace_add_remove() {
        let mut v: serde_json::Value = serde_json::json!({
            "user": "alice",
            "tags": ["a", "b", "c"],
            "meta": {"k": 1}
        });
        let ops = vec![
            JsonPatchOp::Replace {
                path: "/user".to_string(),
                value: serde_json::json!("bob"),
            },
            JsonPatchOp::Add {
                path: "/meta/v".to_string(),
                value: serde_json::json!(2),
            },
            JsonPatchOp::Remove {
                path: "/tags/1".to_string(),
            },
        ];
        apply_json_patch(&mut v, &ops).unwrap();
        assert_eq!(v["user"], "bob");
        assert_eq!(v["meta"]["k"], 1);
        assert_eq!(v["meta"]["v"], 2);
        assert_eq!(v["tags"], serde_json::json!(["a", "c"]));
    }

    #[test]
    fn apply_json_patch_add_to_array_with_dash_appends() {
        let mut v = serde_json::json!({"xs": [1, 2]});
        apply_json_patch(
            &mut v,
            &[JsonPatchOp::Add {
                path: "/xs/-".to_string(),
                value: serde_json::json!(3),
            }],
        )
        .unwrap();
        assert_eq!(v["xs"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn apply_json_patch_invalid_pointer_errors() {
        let mut v = serde_json::json!({});
        let err = apply_json_patch(
            &mut v,
            &[JsonPatchOp::Replace {
                path: "no-leading-slash".to_string(),
                value: serde_json::json!(1),
            }],
        )
        .unwrap_err();
        assert!(err.contains("JSON Pointer"));
    }

    #[test]
    fn apply_json_patch_escape_tokens() {
        let mut v = serde_json::json!({"a/b": 1, "c~d": 2});
        apply_json_patch(
            &mut v,
            &[
                JsonPatchOp::Replace {
                    path: "/a~1b".to_string(),
                    value: serde_json::json!(10),
                },
                JsonPatchOp::Replace {
                    path: "/c~0d".to_string(),
                    value: serde_json::json!(20),
                },
            ],
        )
        .unwrap();
        assert_eq!(v["a/b"], 10);
        assert_eq!(v["c~d"], 20);
    }

    // ---- Replay tests with mock client ----

    struct MockClient {
        expected_url: String,
        expected_method: String,
        captured_body: std::sync::Mutex<Option<Vec<u8>>>,
        captured_headers: std::sync::Mutex<Option<Vec<(String, String)>>>,
        response: RawResponse,
    }

    #[async_trait::async_trait]
    impl HttpClient for MockClient {
        async fn execute(
            &self,
            method: &str,
            url: &str,
            headers: Vec<(String, String)>,
            body: Vec<u8>,
            _timeout: Duration,
        ) -> Result<RawResponse, String> {
            assert_eq!(method, self.expected_method);
            assert_eq!(url, self.expected_url);
            *self.captured_body.lock().unwrap() = Some(body);
            *self.captured_headers.lock().unwrap() = Some(headers);
            Ok(RawResponse {
                status: self.response.status,
                headers: self.response.headers.clone(),
                body: self.response.body.clone(),
            })
        }
    }

    #[tokio::test]
    async fn replay_request_applies_patch_and_strips_content_length() {
        let client = MockClient {
            expected_url: "https://example.com/echo".to_string(),
            expected_method: "POST".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 200,
                headers: vec![("x-mock".to_string(), "1".to_string())],
                body: b"{\"ok\":true}".to_vec(),
            },
        };
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Content-Length".to_string(), "9999".to_string()),
            ("Host".to_string(), "should-be-stripped".to_string()),
        ];
        let body = b"{\"a\":1,\"b\":2}";
        let opts = ReplayOptions {
            patch_json: vec![
                JsonPatchOp::Replace {
                    path: "/a".to_string(),
                    value: serde_json::json!(10),
                },
                JsonPatchOp::Add {
                    path: "/c".to_string(),
                    value: serde_json::json!(3),
                },
                JsonPatchOp::Remove {
                    path: "/b".to_string(),
                },
            ],
            refresh_auth_from: None,
            refresh_auth: false,
            auth_source_host: None,
            timeout_ms: 5000,
        };
        let result = replay_request(
            &client,
            "POST",
            "https://example.com/echo",
            &headers,
            body,
            &opts,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(result.status, 200);
        let captured = client.captured_body.lock().unwrap().clone().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&captured).unwrap();
        assert_eq!(v["a"], 10);
        assert_eq!(v["c"], 3);
        assert!(v.get("b").is_none());
        // 校验 Content-Length / Host 被剥离
        let captured_headers = client.captured_headers.lock().unwrap().clone().unwrap();
        let names: Vec<String> = captured_headers
            .iter()
            .map(|(k, _)| k.to_ascii_lowercase())
            .collect();
        assert!(!names.contains(&"content-length".to_string()));
        assert!(!names.contains(&"host".to_string()));
        assert!(names.contains(&"content-type".to_string()));
        // body 应能 base64 解码
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&result.body_b64)
            .unwrap();
        assert_eq!(decoded, b"{\"ok\":true}");
    }

    #[tokio::test]
    async fn replay_request_patches_gzip_body_and_preserves_encoding() {
        let original = br#"{"limit":1,"nested":{"keep":true}}"#;
        let compressed = encode_content_encoded_body(original, "gzip").unwrap();
        let client = MockClient {
            expected_url: "https://example.com/compressed".to_string(),
            expected_method: "POST".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 200,
                headers: vec![],
                body: vec![],
            },
        };
        let opts = ReplayOptions {
            patch_json: vec![JsonPatchOp::Replace {
                path: "/limit".to_string(),
                value: serde_json::json!(5),
            }],
            ..ReplayOptions::default()
        };
        let headers = vec![
            ("Content-Encoding".to_string(), "gzip".to_string()),
            ("Content-Length".to_string(), compressed.len().to_string()),
        ];

        replay_request(
            &client,
            "POST",
            "https://example.com/compressed",
            &headers,
            &compressed,
            &opts,
            &[],
        )
        .await
        .unwrap();

        let captured = client.captured_body.lock().unwrap().clone().unwrap();
        let decoded =
            crate::handlers::network_body::decompress_with_limit(&captured, "gzip", 1024).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"limit": 5, "nested": {"keep": true}})
        );

        let captured_headers = client.captured_headers.lock().unwrap().clone().unwrap();
        assert!(captured_headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("content-encoding") && value == "gzip"));
        assert!(!captured_headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length")));
    }

    #[tokio::test]
    async fn replay_request_patches_stacked_content_encoding_chain() {
        let original = br#"{"enabled":false}"#;
        let compressed = encode_content_encoded_body(original, "gzip, br").unwrap();
        let client = MockClient {
            expected_url: "https://example.com/stacked".to_string(),
            expected_method: "PATCH".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 204,
                headers: vec![],
                body: vec![],
            },
        };
        let opts = ReplayOptions {
            patch_json: vec![JsonPatchOp::Replace {
                path: "/enabled".to_string(),
                value: serde_json::json!(true),
            }],
            ..ReplayOptions::default()
        };
        let headers = vec![
            ("Content-Encoding".to_string(), "gzip".to_string()),
            ("content-encoding".to_string(), "br".to_string()),
        ];

        replay_request(
            &client,
            "PATCH",
            "https://example.com/stacked",
            &headers,
            &compressed,
            &opts,
            &[],
        )
        .await
        .unwrap();

        let captured = client.captured_body.lock().unwrap().clone().unwrap();
        let decoded =
            crate::handlers::network_body::decompress_with_limit(&captured, "gzip, br", 1024)
                .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&decoded).unwrap(),
            serde_json::json!({"enabled": true})
        );
    }

    #[tokio::test]
    async fn replay_request_patch_rejects_invalid_or_unknown_encoding() {
        let opts = ReplayOptions {
            patch_json: vec![JsonPatchOp::Replace {
                path: "/value".to_string(),
                value: serde_json::json!(2),
            }],
            ..ReplayOptions::default()
        };

        for (encoding, body, expected_error) in [
            ("gzip", b"not-gzip".as_slice(), "failed to decode gzip"),
            (
                "x-private",
                br#"{"value":1}"#.as_slice(),
                "unsupported content-encoding: x-private",
            ),
        ] {
            let client = MockClient {
                expected_url: "https://example.com/error".to_string(),
                expected_method: "POST".to_string(),
                captured_body: std::sync::Mutex::new(None),
                captured_headers: std::sync::Mutex::new(None),
                response: RawResponse {
                    status: 200,
                    headers: vec![],
                    body: vec![],
                },
            };
            let error = replay_request(
                &client,
                "POST",
                "https://example.com/error",
                &[("Content-Encoding".to_string(), encoding.to_string())],
                body,
                &opts,
                &[],
            )
            .await
            .unwrap_err();
            assert!(error.contains(expected_error), "{error}");
            assert!(client.captured_body.lock().unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn replay_request_without_patch_passes_body_through() {
        let client = MockClient {
            expected_url: "https://example.com/x".to_string(),
            expected_method: "GET".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 204,
                headers: vec![],
                body: vec![],
            },
        };
        let opts = ReplayOptions::default();
        let r = replay_request(
            &client,
            "GET",
            "https://example.com/x",
            &[],
            b"",
            &opts,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(r.status, 204);
        assert_eq!(r.body_b64, "");
    }

    #[tokio::test]
    async fn replay_request_without_patch_preserves_compressed_wire_body() {
        let compressed = encode_content_encoded_body(br#"{"value":1}"#, "gzip").unwrap();
        let client = MockClient {
            expected_url: "https://example.com/wire".to_string(),
            expected_method: "POST".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 200,
                headers: vec![],
                body: vec![],
            },
        };

        replay_request(
            &client,
            "POST",
            "https://example.com/wire",
            &[("Content-Encoding".to_string(), "gzip".to_string())],
            &compressed,
            &ReplayOptions::default(),
            &[],
        )
        .await
        .unwrap();

        assert_eq!(
            client.captured_body.lock().unwrap().as_deref(),
            Some(compressed.as_slice())
        );
        assert!(client
            .captured_headers
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("content-encoding") && value == "gzip"));
    }

    // ---- refresh-auth tests ----

    #[test]
    fn classify_auth_header_field_recognises_authorization_cookie_xtt() {
        assert_eq!(
            classify_auth_header_field("Authorization"),
            Some("authorization")
        );
        assert_eq!(
            classify_auth_header_field("authorization"),
            Some("authorization")
        );
        assert_eq!(classify_auth_header_field("Cookie"), Some("cookie"));
        assert_eq!(classify_auth_header_field("X-Tt-Token"), Some("x-tt-*"));
        assert_eq!(
            classify_auth_header_field("x-tt-passport-csrf"),
            Some("x-tt-*")
        );
        assert_eq!(classify_auth_header_field("Content-Type"), None);
        assert_eq!(classify_auth_header_field("X-Request-Id"), None);
    }

    #[test]
    fn find_refresh_auth_source_picks_latest_matching_host() {
        let candidates = vec![
            AuthCandidate {
                id: "old".into(),
                host: "api.example.com".into(),
                headers: vec![("Authorization".into(), "Bearer old".into())],
                recency: 100,
            },
            AuthCandidate {
                id: "new".into(),
                host: "api.example.com".into(),
                headers: vec![
                    ("Authorization".into(), "Bearer new".into()),
                    ("Cookie".into(), "sid=abc".into()),
                    ("X-Request-Id".into(), "req-1".into()),
                ],
                recency: 200,
            },
            AuthCandidate {
                id: "other-host".into(),
                host: "other.example".into(),
                headers: vec![("Authorization".into(), "Bearer x".into())],
                recency: 999,
            },
        ];
        let (id, hs) = find_refresh_auth_source(&candidates, "api.example.com").unwrap();
        assert_eq!(id, "new");
        let names: Vec<String> = hs.iter().map(|(k, _)| k.clone()).collect();
        assert!(names.contains(&"Authorization".to_string()));
        assert!(names.contains(&"Cookie".to_string()));
        assert!(!names.contains(&"X-Request-Id".to_string()));
    }

    #[test]
    fn find_refresh_auth_source_returns_none_when_no_match() {
        let candidates = vec![AuthCandidate {
            id: "x".into(),
            host: "other.example".into(),
            headers: vec![("Authorization".into(), "Bearer x".into())],
            recency: 1,
        }];
        assert!(find_refresh_auth_source(&candidates, "target.example").is_none());

        let candidates = vec![AuthCandidate {
            id: "y".into(),
            host: "target.example".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            recency: 1,
        }];
        assert!(find_refresh_auth_source(&candidates, "target.example").is_none());
    }

    #[test]
    fn apply_refresh_auth_overrides_or_appends_headers() {
        let mut target = vec![
            ("Authorization".to_string(), "Bearer stale".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let auth = vec![
            ("authorization".to_string(), "Bearer fresh".to_string()),
            ("Cookie".to_string(), "sid=z".to_string()),
        ];
        let applied = apply_refresh_auth(&mut target, &auth);
        let map: std::collections::HashMap<String, String> = target
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        assert_eq!(map["authorization"], "Bearer fresh");
        assert_eq!(map["cookie"], "sid=z");
        assert_eq!(map["content-type"], "application/json");
        assert!(applied
            .iter()
            .any(|x| x.eq_ignore_ascii_case("authorization")));
        assert!(applied.iter().any(|x| x.eq_ignore_ascii_case("Cookie")));
    }

    #[test]
    fn extract_host_from_url_handles_scheme_port_path_userinfo() {
        assert_eq!(
            extract_host_from_url("https://example.com/api/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            extract_host_from_url("https://user:pw@host:8443/x").as_deref(),
            Some("host")
        );
        assert_eq!(
            extract_host_from_url("http://[::1]:80").as_deref(),
            Some("[")
        );
        assert_eq!(
            extract_host_from_url("example.com/p").as_deref(),
            Some("example.com")
        );
    }

    #[tokio::test]
    async fn replay_request_with_refresh_auth_applies_authorization_and_cookie() {
        let client = MockClient {
            expected_url: "https://api.example.com/api/foo".to_string(),
            expected_method: "GET".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 200,
                headers: vec![],
                body: b"ok".to_vec(),
            },
        };
        let candidates = vec![AuthCandidate {
            id: "src-1".into(),
            host: "api.example.com".into(),
            headers: vec![
                ("Authorization".into(), "Bearer FRESH".into()),
                ("Cookie".into(), "sid=z".into()),
                ("X-Other".into(), "ignored".into()),
            ],
            recency: 10,
        }];
        let opts = ReplayOptions {
            refresh_auth: true,
            ..Default::default()
        };
        let result = replay_request(
            &client,
            "GET",
            "https://api.example.com/api/foo",
            &[("Authorization".into(), "Bearer STALE".into())],
            b"",
            &opts,
            &candidates,
        )
        .await
        .unwrap();
        let info = result.auth_refresh.expect("auth_refresh info");
        assert!(info.applied);
        assert_eq!(info.source_traffic_id.as_deref(), Some("src-1"));
        let captured = client.captured_headers.lock().unwrap().clone().unwrap();
        let map: std::collections::HashMap<String, String> = captured
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        assert_eq!(map["authorization"], "Bearer FRESH");
        assert_eq!(map["cookie"], "sid=z");
        assert!(!map.contains_key("x-other"));
    }

    #[tokio::test]
    async fn replay_request_with_refresh_auth_no_match_returns_not_applied() {
        let client = MockClient {
            expected_url: "https://example.com/x".to_string(),
            expected_method: "GET".to_string(),
            captured_body: std::sync::Mutex::new(None),
            captured_headers: std::sync::Mutex::new(None),
            response: RawResponse {
                status: 200,
                headers: vec![],
                body: vec![],
            },
        };
        let opts = ReplayOptions {
            refresh_auth_from: Some("latest".into()),
            ..Default::default()
        };
        let result = replay_request(
            &client,
            "GET",
            "https://example.com/x",
            &[],
            b"",
            &opts,
            &[],
        )
        .await
        .unwrap();
        let info = result.auth_refresh.expect("info");
        assert!(!info.applied);
        assert!(info.source_traffic_id.is_none());
        assert!(info.fields.is_empty());
    }

    #[test]
    fn refresh_auth_enabled_treats_legacy_string_as_truthy() {
        let opts = ReplayOptions {
            refresh_auth_from: Some("latest".into()),
            ..Default::default()
        };
        assert!(opts.refresh_auth_enabled());
        let opts = ReplayOptions {
            refresh_auth_from: Some(String::new()),
            ..Default::default()
        };
        assert!(!opts.refresh_auth_enabled());
        let opts = ReplayOptions {
            refresh_auth: true,
            ..Default::default()
        };
        assert!(opts.refresh_auth_enabled());
    }
}
