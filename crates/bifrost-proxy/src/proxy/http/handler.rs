use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bifrost_admin::{AdminState, RequestTiming, TrafficRecord, TrafficType};
use bifrost_core::{protocol::Protocol, BifrostError, Result};
use bifrost_script::{RequestData, ResponseData};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::HeaderValue;
use hyper::http::response::Parts as ResponseParts;
use hyper::{Request, Response, StatusCode, Uri};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::dns::DnsResolver;
#[cfg(feature = "http3")]
use crate::http3::Http3Client;
use crate::protocol::ProtocolDetector;

use super::tunnel::{
    classify_request_error, is_retryable_http2_error as is_retryable_http2_upstream_error,
    mark_http1_fallback as mark_http1_upstream_fallback, sanitize_upstream_headers,
    send_pooled_request, send_pooled_request_http1_only,
};
use super::ws_handshake::{
    header_values, negotiate_extensions, negotiate_protocol, read_http1_response_with_leftover,
};
use crate::server::{full_body, with_trailers, BoxBody, ResolvedRules, RulesResolver};
use crate::transform::apply_req_rules;
use crate::transform::apply_res_rules;
use crate::transform::decompress_body_with_limit;
use crate::transform::{apply_body_rules, apply_content_injection, Phase};
use crate::utils::bounded::{read_body_bounded, BoundedBody};
use crate::utils::http_size::{
    calculate_request_size, calculate_response_headers_size, calculate_response_size,
};
use crate::utils::logging::{format_rules_detail, format_rules_summary, RequestContext};
use crate::utils::mock::{generate_mock_response, should_intercept_response};
use crate::utils::tee::{
    create_metrics_body, create_request_tee_body, create_sse_tee_body, create_tee_body_with_store,
    store_request_body, store_response_body, BodyCaptureHandle,
};
use crate::utils::throttle::wrap_throttled_body;
use crate::utils::url::apply_url_rules;

mod content_type;
mod decode;
mod scripts;

use self::content_type::{
    get_content_type, is_likely_text_content_type, is_sse_response, is_streaming_response,
    should_use_binary_performance_mode,
};
use self::decode::{
    apply_decode_scripts_for_storage, get_values_from_state, parse_url_parts,
    DecodeForStorageResult,
};
use self::scripts::{execute_request_scripts, execute_response_scripts, headers_to_hashmap};

trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite> AsyncReadWrite for T {}
fn get_traffic_type_from_url(url: &str) -> TrafficType {
    if url.starts_with("https://") {
        TrafficType::Https
    } else {
        TrafficType::Http
    }
}

fn headers_to_pairs(headers: &hyper::HeaderMap) -> Vec<(String, String)> {
    let mut pairs = Vec::with_capacity(headers.len());
    for (key, value) in headers {
        pairs.push((key.to_string(), value.to_str().unwrap_or("").to_string()));
    }
    pairs
}

fn header_map_to_hashmap(headers: &hyper::HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(headers.len());
    for (key, value) in headers {
        map.insert(key.to_string(), value.to_str().unwrap_or("").to_string());
    }
    map
}

fn cloned_headers_hashmap(
    cache: &mut Option<HashMap<String, String>>,
    headers: &[(String, String)],
) -> HashMap<String, String> {
    if let Some(map) = cache.as_ref() {
        return map.clone();
    }

    let map = headers_to_hashmap(headers);
    *cache = Some(map.clone());
    map
}

fn response_content_encoding(parts: &ResponseParts) -> Option<String> {
    parts
        .headers
        .get(hyper::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn header_content_encoding(headers: &hyper::HeaderMap) -> Option<String> {
    headers
        .get(hyper::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn build_upstream_pool_partition(
    original_host: &str,
    target_host: &str,
    target_port: u16,
    use_tls: bool,
    rules: &ResolvedRules,
) -> String {
    let mut partition = String::with_capacity(
        original_host.len()
            + target_host.len()
            + rules.host.as_ref().map_or(4, |value| value.len())
            + rules.proxy.as_ref().map_or(4, |value| value.len())
            + 96,
    );
    partition.push_str("orig=");
    partition.push_str(original_host);
    partition.push_str("|target=");
    partition.push_str(if use_tls { "https" } else { "http" });
    partition.push_str("://");
    partition.push_str(target_host);
    partition.push(':');
    partition.push_str(&target_port.to_string());
    partition.push_str("|host=");
    partition.push_str(rules.host.as_deref().unwrap_or("-"));
    partition.push_str("|proxy=");
    partition.push_str(rules.proxy.as_deref().unwrap_or("-"));
    partition.push_str("|proto=");
    partition.push_str(match rules.host_protocol {
        Some(Protocol::Http) => "http",
        Some(Protocol::Https) => "https",
        Some(Protocol::Ws) => "ws",
        Some(Protocol::Wss) => "wss",
        Some(Protocol::Host) => "host",
        Some(Protocol::XHost) => "xhost",
        Some(_) => "other",
        None => "-",
    });
    partition.push_str("|ignored_host=");
    partition.push(if rules.ignored.host { '1' } else { '0' });
    partition
}

#[cfg(feature = "http3")]
async fn try_send_http3_upstream(
    host: &str,
    port: u16,
    req: Request<Bytes>,
    unsafe_ssl: bool,
    dns_resolver: &DnsResolver,
    dns_servers: &[String],
) -> Result<Response<Bytes>> {
    let addr = Http3Client::resolve_target_addr(host, port, dns_resolver, dns_servers).await?;
    let client = Http3Client::new_with_options(unsafe_ssl)?;
    client.request_to_addr(host, addr, req).await
}

pub fn needs_body_processing(rules: &ResolvedRules) -> bool {
    rules.res_body.is_some()
        || !rules.res_replace.is_empty()
        || !rules.res_replace_regex.is_empty()
        || rules.res_prepend.is_some()
        || rules.res_append.is_some()
        || rules.res_merge.is_some()
        || rules.html_append.is_some()
        || rules.html_prepend.is_some()
        || rules.html_body.is_some()
        || rules.js_append.is_some()
        || rules.js_prepend.is_some()
        || rules.js_body.is_some()
        || rules.css_append.is_some()
        || rules.css_prepend.is_some()
        || rules.css_body.is_some()
        || !rules.res_scripts.is_empty()
}

pub fn needs_response_override(rules: &ResolvedRules) -> bool {
    rules.res_body.is_some() || rules.status_code.is_some() || rules.replace_status.is_some()
}

enum BodyMode {
    Known(usize),
    Stream,
    StreamWithLength(usize),
    StreamWithTrailers,
}

#[derive(Clone)]
struct RetryableRequestBlueprint {
    method: hyper::Method,
    uri: Uri,
    version: hyper::Version,
    headers: hyper::HeaderMap<HeaderValue>,
    body: Bytes,
}

impl RetryableRequestBlueprint {
    fn build(&self) -> Result<Request<BoxBody>> {
        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone())
            .version(self.version);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder.body(full_body(self.body.clone())).map_err(|e| {
            BifrostError::Network(format!("Failed to rebuild request for retry: {}", e))
        })
    }
}

fn is_no_body_response(status: StatusCode, method: &str) -> bool {
    status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || method.eq_ignore_ascii_case("HEAD")
}

fn should_use_metrics_only_forwarding_mode(
    skip_binary_recording: bool,
    _has_rules: bool,
    needs_processing: bool,
    is_websocket: bool,
    is_sse: bool,
) -> bool {
    skip_binary_recording && !needs_processing && !is_websocket && !is_sse
}

fn normalize_req_headers(parts: &mut hyper::http::request::Parts, mode: BodyMode) {
    match mode {
        BodyMode::Known(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
        BodyMode::Stream | BodyMode::StreamWithTrailers => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
        }
        BodyMode::StreamWithLength(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
    }
}

fn normalize_res_headers(parts: &mut ResponseParts, mode: BodyMode, method: &str) {
    if is_no_body_response(parts.status, method) {
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        return;
    }
    match mode {
        BodyMode::Known(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
        BodyMode::Stream | BodyMode::StreamWithTrailers => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
        }
        BodyMode::StreamWithLength(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
    }
}

pub struct ConnectionErrorInfo {
    pub error_type: &'static str,
    pub error_message: String,
    pub host: String,
    pub request_url: String,
}

fn build_error_body(status_code: u16, error_info: &ConnectionErrorInfo) -> Bytes {
    let hostname = gethostname::gethostname().to_string_lossy().to_string();
    let now = chrono::Local::now();
    let date_str = now.format("%m/%d/%Y, %I:%M:%S %p").to_string();

    Bytes::from(format!(
        "Status: {}\nError: {}\nFrom: Bifrost@{}\nHost: {}\nDate: {}\nURL: {}",
        status_code,
        error_info.error_message,
        hostname,
        error_info.host,
        date_str,
        error_info.request_url,
    ))
}

pub fn build_connection_error_response(
    status_code: u16,
    error_info: &ConnectionErrorInfo,
) -> Response<BoxBody> {
    let hostname = gethostname::gethostname().to_string_lossy().to_string();
    let now = chrono::Local::now();
    let date_str = now.format("%m/%d/%Y, %I:%M:%S %p").to_string();

    let body = format!(
        "Status: {}\nError: {}\nFrom: Bifrost@{}\nHost: {}\nDate: {}\nURL: {}",
        status_code,
        error_info.error_message,
        hostname,
        error_info.host,
        date_str,
        error_info.request_url,
    );

    Response::builder()
        .status(hyper::StatusCode::from_u16(status_code).unwrap_or(hyper::StatusCode::BAD_GATEWAY))
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("X-Bifrost-Error", error_info.error_type)
        .body(full_body(body.into_bytes()))
        .unwrap()
}

pub fn build_overridden_error_response(
    rules: &ResolvedRules,
    default_status: u16,
    error_info: &ConnectionErrorInfo,
) -> Response<BoxBody> {
    let status_code = rules
        .status_code
        .or(rules.replace_status)
        .unwrap_or(default_status);

    let body = if let Some(ref res_body) = rules.res_body {
        res_body.clone()
    } else {
        let hostname = gethostname::gethostname().to_string_lossy().to_string();
        let now = chrono::Local::now();
        let date_str = now.format("%m/%d/%Y, %I:%M:%S %p").to_string();

        let body_str = format!(
            "Status: {}\nError: {}\nFrom: Bifrost@{}\nHost: {}\nDate: {}\nURL: {}",
            status_code,
            error_info.error_message,
            hostname,
            error_info.host,
            date_str,
            error_info.request_url,
        );
        Bytes::from(body_str)
    };

    let mut response = Response::builder()
        .status(hyper::StatusCode::from_u16(status_code).unwrap_or(hyper::StatusCode::BAD_GATEWAY));

    for (name, value) in &rules.res_headers {
        if let (Ok(header_name), Ok(header_value)) = (
            hyper::header::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response = response.header(header_name, header_value);
        }
    }

    if rules.res_body.is_none() {
        response = response.header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8");
        response = response.header("X-Bifrost-Error", error_info.error_type);
    }

    response.body(full_body(body.to_vec())).unwrap()
}

pub fn needs_request_body_processing(rules: &ResolvedRules) -> bool {
    rules.req_body.is_some()
        || rules.req_prepend.is_some()
        || rules.req_append.is_some()
        || !rules.req_replace.is_empty()
        || !rules.req_replace_regex.is_empty()
        || rules.req_merge.is_some()
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_http_request(
    req: Request<Incoming>,
    rules: Arc<dyn RulesResolver>,
    verbose_logging: bool,
    unsafe_ssl: bool,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
) -> Result<Response<BoxBody>> {
    if is_websocket_upgrade(&req) {
        return handle_http_websocket(req, rules, ctx, admin_state, unsafe_ssl).await;
    }

    let uri = req.uri().clone();
    let method = req.method().to_string();
    let url = uri.to_string();
    let record_url = if ctx.url.is_empty() {
        url.clone()
    } else {
        ctx.url.clone()
    };
    let start_time = std::time::Instant::now();
    let incoming_headers: HashMap<String, String> = req
        .headers()
        .iter()
        .map(|(key, value)| {
            (
                key.to_string().to_lowercase(),
                value.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let incoming_cookies: HashMap<String, String> = req
        .headers()
        .get(hyper::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .filter_map(|part| {
                    let mut iter = part.trim().splitn(2, '=');
                    match (iter.next(), iter.next()) {
                        (Some(key), Some(val)) => Some((key.to_string(), val.to_string())),
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let rule_match_url = if ctx.url.is_empty() { &url } else { &ctx.url };
    let resolved_rules = rules.resolve_with_context(
        rule_match_url,
        &method,
        &incoming_headers,
        &incoming_cookies,
    );

    // 解压输出上限：用于防御压缩炸弹。优先读取配置，否则使用默认 10MiB。
    let max_decompress_output_bytes = if let Some(ref state) = admin_state {
        if let Some(cm) = state.config_manager.as_ref() {
            cm.config().await.sandbox.limits.max_decompress_output_bytes
        } else {
            10 * 1024 * 1024
        }
    } else {
        10 * 1024 * 1024
    };

    let has_rules = !resolved_rules.rules.is_empty()
        || resolved_rules.host.is_some()
        || resolved_rules.proxy.is_some()
        || !resolved_rules.req_headers.is_empty()
        || !resolved_rules.res_headers.is_empty()
        || resolved_rules.status_code.is_some()
        || should_intercept_response(&resolved_rules);

    if verbose_logging {
        if has_rules {
            info!(
                "[{}] [RULES] matched: {}",
                ctx.id_str(),
                format_rules_summary(&resolved_rules)
            );
            debug!(
                "[{}] [RULES] details:\n{}",
                ctx.id_str(),
                format_rules_detail(&resolved_rules)
            );
        } else {
            debug!("[{}] [RULES] none matched", ctx.id_str());
        }
    }

    if let Some(mock_response) =
        generate_mock_response(&resolved_rules, &uri, verbose_logging, ctx).await
    {
        if verbose_logging {
            info!("[{}] [MOCK] returning mock response", ctx.id_str());
        }
        return Ok(mock_response);
    }

    if let Some(ref redirect_url) = resolved_rules.redirect {
        let status = resolved_rules.redirect_status.unwrap_or(302);
        if verbose_logging {
            info!(
                "[{}] [REDIRECT] {} -> {} ({})",
                ctx.id_str(),
                url,
                redirect_url,
                status
            );
        }
        return Ok(build_redirect_response(status, redirect_url));
    }

    if let Some(ref location) = resolved_rules.location_href {
        if verbose_logging {
            info!("[{}] [LOCATION] {} -> {}", ctx.id_str(), url, location);
        }
        return Ok(build_redirect_response(301, location));
    }

    let processed_uri = apply_url_rules(&uri, &resolved_rules, verbose_logging, ctx);

    let original_host = uri.host().unwrap_or("unknown").to_string();
    let is_https = uri.scheme_str() == Some("https") || uri.scheme_str() == Some("wss");
    let default_port = if is_https { 443 } else { 80 };
    let original_port = uri.port_u16().unwrap_or(default_port);
    let (host, port) = extract_host_port(&processed_uri, &resolved_rules, is_https)?;

    if verbose_logging {
        if resolved_rules.host.is_some() {
            info!(
                "[{}] [FORWARD] {}:{} -> {}:{} (redirected by host rule)",
                ctx.id_str(),
                original_host,
                original_port,
                host,
                port
            );
        } else {
            info!("[{}] [FORWARD] {}:{}", ctx.id_str(), host, port);
        }
    } else {
        debug!("Proxying HTTP request to {}:{}", host, port);
    }

    let (mut parts, body) = req.into_parts();

    let original_req_headers = admin_state
        .as_ref()
        .map(|_| headers_to_pairs(&parts.headers));

    let req_content_encoding = header_content_encoding(&parts.headers);

    apply_req_rules(&mut parts, &resolved_rules, verbose_logging, ctx);

    let content_length = parts
        .headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());
    let has_transfer_encoding = parts.headers.contains_key(hyper::header::TRANSFER_ENCODING);

    let needs_req_processing = needs_request_body_processing(&resolved_rules);
    let has_req_body_override = resolved_rules.req_body.is_some();
    let has_req_scripts = !resolved_rules.req_scripts.is_empty();
    let needs_req_body_read = !has_req_body_override && (needs_req_processing || has_req_scripts);

    let mut skip_req_scripts = false;
    let mut streaming_body: Option<BoxBody> = None;
    let mut req_body_capture: Option<BodyCaptureHandle> = None;
    let (body_bytes, mut final_body) = if needs_req_body_read {
        if let Some(len) = content_length {
            if len > max_body_buffer_size {
                warn!(
                    "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and scripts",
                    ctx.id_str(),
                    len,
                    max_body_buffer_size
                );
                skip_req_scripts = true;
                if admin_state.is_some() {
                    let (tee_body, capture) = create_request_tee_body(
                        body,
                        admin_state.clone(),
                        ctx.id_str().to_string(),
                    );
                    streaming_body = Some(tee_body);
                    req_body_capture = Some(capture);
                } else {
                    streaming_body = Some(body.boxed());
                }
                (Bytes::new(), Bytes::new())
            } else {
                let req_content_type = parts
                    .headers
                    .get(hyper::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_lowercase();
                let limit = if !is_likely_text_content_type(&req_content_type) {
                    let probe = max_body_probe_size.min(max_body_buffer_size);
                    if probe == 0 {
                        max_body_buffer_size
                    } else {
                        probe
                    }
                } else {
                    max_body_buffer_size
                };
                match read_body_bounded(body, limit).await {
                    Ok(BoundedBody::Complete(bytes)) => {
                        let req_content_type = parts
                            .headers
                            .get(hyper::header::CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok());
                        let processed = apply_body_rules(
                            bytes.clone(),
                            &resolved_rules,
                            Phase::Request,
                            req_content_type,
                            verbose_logging,
                            ctx,
                        );
                        (bytes, processed)
                    }
                    Ok(BoundedBody::Exceeded(replay_body)) => {
                        let size_display = content_length
                            .map(|len| len.to_string())
                            .unwrap_or_else(|| format!(">{}", limit));
                        warn!(
                            "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and scripts",
                            ctx.id_str(),
                            size_display,
                            limit
                        );
                        skip_req_scripts = true;
                        if admin_state.is_some() {
                            let (tee_body, capture) = create_request_tee_body(
                                replay_body,
                                admin_state.clone(),
                                ctx.id_str().to_string(),
                            );
                            streaming_body = Some(tee_body);
                            req_body_capture = Some(capture);
                        } else {
                            streaming_body = Some(replay_body.boxed());
                        }
                        (Bytes::new(), Bytes::new())
                    }
                    Err(e) => {
                        return Err(BifrostError::Network(format!(
                            "Failed to read request body: {}",
                            e
                        )))
                    }
                }
            }
        } else {
            let req_content_type = parts
                .headers
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_lowercase();
            let limit = if !is_likely_text_content_type(&req_content_type) {
                let probe = max_body_probe_size.min(max_body_buffer_size);
                if probe == 0 {
                    max_body_buffer_size
                } else {
                    probe
                }
            } else {
                max_body_buffer_size
            };
            match read_body_bounded(body, limit).await {
                Ok(BoundedBody::Complete(bytes)) => {
                    let req_content_type = parts
                        .headers
                        .get(hyper::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok());
                    let processed = apply_body_rules(
                        bytes.clone(),
                        &resolved_rules,
                        Phase::Request,
                        req_content_type,
                        verbose_logging,
                        ctx,
                    );
                    (bytes, processed)
                }
                Ok(BoundedBody::Exceeded(replay_body)) => {
                    let size_display = content_length
                        .map(|len| len.to_string())
                        .unwrap_or_else(|| format!(">{}", limit));
                    warn!(
                    "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and scripts",
                    ctx.id_str(),
                    size_display,
                    limit
                );
                    skip_req_scripts = true;
                    if admin_state.is_some() {
                        let (tee_body, capture) = create_request_tee_body(
                            replay_body,
                            admin_state.clone(),
                            ctx.id_str().to_string(),
                        );
                        streaming_body = Some(tee_body);
                        req_body_capture = Some(capture);
                    } else {
                        streaming_body = Some(replay_body.boxed());
                    }
                    (Bytes::new(), Bytes::new())
                }
                Err(e) => {
                    return Err(BifrostError::Network(format!(
                        "Failed to read request body: {}",
                        e
                    )))
                }
            }
        }
    } else if let Some(ref new_body) = resolved_rules.req_body {
        if verbose_logging {
            info!(
                "[{}] [REQ_BODY] replaced: {} bytes -> {} bytes",
                ctx.id_str(),
                content_length.unwrap_or(0),
                new_body.len()
            );
        }
        let mut body = body;
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
        let req_content_type = parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());
        let processed = apply_body_rules(
            new_body.clone(),
            &resolved_rules,
            Phase::Request,
            req_content_type,
            verbose_logging,
            ctx,
        );
        (Bytes::new(), processed)
    } else if content_length.unwrap_or(0) == 0 && !has_transfer_encoding {
        (Bytes::new(), Bytes::new())
    } else {
        if admin_state.is_some() {
            let (tee_body, capture) =
                create_request_tee_body(body, admin_state.clone(), ctx.id_str().to_string());
            streaming_body = Some(tee_body);
            req_body_capture = Some(capture);
        } else {
            streaming_body = Some(body.boxed());
        }
        (Bytes::new(), Bytes::new())
    };
    let has_res_scripts = !resolved_rules.res_scripts.is_empty();
    let has_decode_scripts = !resolved_rules.decode_scripts.is_empty();
    let mut values = HashMap::new();
    if has_req_scripts || has_res_scripts || has_decode_scripts {
        values = resolved_rules.values.clone();
        let state_values = get_values_from_state(&admin_state).await;
        for (k, v) in state_values {
            values.entry(k).or_insert(v);
        }
    }

    let req_script_results = if has_req_scripts && !skip_req_scripts {
        let mut script_method = method.clone();
        let mut script_headers = header_map_to_hashmap(&parts.headers);
        let mut script_body = if !final_body.is_empty() {
            String::from_utf8(final_body.to_vec()).ok()
        } else {
            None
        };

        let results = execute_request_scripts(
            &admin_state,
            &resolved_rules.req_scripts,
            ctx,
            &resolved_rules,
            &url,
            &mut script_method,
            &mut script_headers,
            &mut script_body,
            &values,
        )
        .await;

        if results.iter().any(|r| r.success) {
            if let Ok(new_method) = script_method.parse() {
                parts.method = new_method;
            }

            let mut new_headers = hyper::HeaderMap::new();
            for (key, value) in &script_headers {
                if let (Ok(name), Ok(val)) = (
                    hyper::header::HeaderName::from_bytes(key.as_bytes()),
                    hyper::header::HeaderValue::from_str(value),
                ) {
                    new_headers.insert(name, val);
                }
            }
            parts.headers = new_headers;

            if let Some(ref new_body) = script_body {
                final_body = Bytes::from(new_body.clone());
            }
        }

        results
    } else {
        Vec::new()
    };

    let req_body_mode = if streaming_body.is_some() {
        if let Some(len) = content_length {
            BodyMode::StreamWithLength(len)
        } else {
            BodyMode::Stream
        }
    } else {
        BodyMode::Known(final_body.len())
    };
    normalize_req_headers(&mut parts, req_body_mode);
    let req_headers = headers_to_pairs(&parts.headers);
    let mut req_headers_hashmap_cache: Option<HashMap<String, String>> = None;
    let request_body_size = if !final_body.is_empty() {
        final_body.len()
    } else {
        content_length.unwrap_or(0)
    };
    let request_body_is_streaming = streaming_body.is_some();
    let outgoing_body = match streaming_body {
        Some(body) => body,
        None => full_body(final_body.clone()),
    };
    let outgoing_body = wrap_throttled_body(outgoing_body, resolved_rules.req_speed);

    let dns_ms = None;

    let use_tls = if resolved_rules.ignored.host {
        is_https
    } else {
        match resolved_rules.host_protocol {
            Some(Protocol::Http) | Some(Protocol::Ws) => false,
            Some(Protocol::Https) | Some(Protocol::Wss) => true,
            Some(Protocol::Host) | Some(Protocol::XHost) => port == 443 || port == 8443,
            _ => is_https,
        }
    };
    let retry_blueprint =
        if use_tls && matches!(method.as_str(), "GET" | "HEAD") && !request_body_is_streaming {
            Some(RetryableRequestBlueprint {
                method: parts.method.clone(),
                uri: parts.uri.clone(),
                version: parts.version,
                headers: parts.headers.clone(),
                body: final_body.clone(),
            })
        } else {
            None
        };

    let build_conn_error_and_record =
        |error_type: &'static str, error_msg: String, err_tls_ms: Option<u64>| {
            let error_info = ConnectionErrorInfo {
                error_type,
                error_message: error_msg.clone(),
                host: host.clone(),
                request_url: url.clone(),
            };
            let total_ms = start_time.elapsed().as_millis() as u64;
            if let Some(ref state) = admin_state {
                let mut record = TrafficRecord::new(
                    ctx.id_str().to_string(),
                    method.clone(),
                    record_url.clone(),
                );
                record.status = if needs_response_override(&resolved_rules) {
                    resolved_rules
                        .status_code
                        .or(resolved_rules.replace_status)
                        .unwrap_or(502)
                } else {
                    502
                };
                record.duration_ms = total_ms;
                record.host = original_host.clone();
                record.timing = Some(RequestTiming {
                    dns_ms,
                    connect_ms: None,
                    tls_ms: err_tls_ms,
                    send_ms: None,
                    wait_ms: None,
                    first_byte_ms: None,
                    receive_ms: None,
                    total_ms,
                });
                record.original_request_headers = Some(
                    original_req_headers
                        .as_ref()
                        .expect("request headers captured when admin state is enabled")
                        .clone(),
                );
                record.has_rule_hit = has_rules;
                record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                record.error_message = Some(error_msg.clone());
                record.request_body_ref = if let Some(ref capture) = req_body_capture {
                    capture.take()
                } else if let Some(ref body_store) = state.body_store {
                    let store = body_store.read();
                    let decompressed_req_body = decompress_body_with_limit(
                        &final_body,
                        req_content_encoding.as_deref(),
                        max_decompress_output_bytes,
                    );
                    store.store(ctx.id_str(), "req", decompressed_req_body.as_ref())
                } else {
                    store_request_body(
                        &admin_state,
                        ctx.id_str(),
                        &final_body,
                        req_content_encoding.as_deref(),
                    )
                };

                let response_body = if needs_response_override(&resolved_rules) {
                    if let Some(ref res_body) = resolved_rules.res_body {
                        res_body.clone()
                    } else {
                        build_error_body(record.status, &error_info)
                    }
                } else {
                    build_error_body(502, &error_info)
                };
                record.response_body_ref = if let Some(ref body_store) = state.body_store {
                    let store = body_store.read();
                    store.store(ctx.id_str(), "res", response_body.as_ref())
                } else {
                    store_response_body(&admin_state, ctx.id_str(), &response_body)
                };
                state.record_traffic(record);
            }
            if needs_response_override(&resolved_rules) {
                if verbose_logging {
                    info!(
                        "[{}] [CONN_ERROR] {}, applying response override rules",
                        ctx.id_str(),
                        error_type
                    );
                }
                build_overridden_error_response(&resolved_rules, 502, &error_info)
            } else {
                build_connection_error_response(502, &error_info)
            }
        };

    let path = processed_uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let upstream_authority = if (use_tls && port == 443) || (!use_tls && port == 80) {
        host.clone()
    } else {
        format!("{}:{}", host, port)
    };
    let upstream_uri: Uri = format!(
        "{}://{}{}",
        if use_tls { "https" } else { "http" },
        upstream_authority,
        path
    )
    .parse()
    .map_err(|e| BifrostError::Network(format!("Invalid URI: {}", e)))?;

    parts.uri = upstream_uri.clone();
    sanitize_upstream_headers(&mut parts.headers);
    parts.headers.remove(hyper::header::HOST);

    #[cfg(feature = "http3")]
    let req_headers_for_h3: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    #[cfg(feature = "http3")]
    let should_try_http3_upstream = use_tls
        && resolved_rules.upstream_http3
        && !request_body_is_streaming
        && dns_resolver.is_some()
        && resolved_rules.proxy.is_none()
        && !ProtocolDetector::is_websocket_upgrade(&req_headers_for_h3)
        && !ProtocolDetector::is_sse_request(&req_headers_for_h3);

    #[cfg(feature = "http3")]
    let h3_attempt = if should_try_http3_upstream {
        let mut builder = Request::builder()
            .method(parts.method.clone())
            .uri(upstream_uri.clone());
        for (key, value) in parts.headers.iter() {
            builder = builder.header(key, value);
        }
        builder = builder.header("host", upstream_authority.clone());
        match builder.body(final_body.clone()) {
            Ok(h3_req) => {
                let start = Instant::now();
                match try_send_http3_upstream(
                    &host,
                    port,
                    h3_req,
                    unsafe_ssl,
                    dns_resolver.as_ref().unwrap().as_ref(),
                    &resolved_rules.dns_servers,
                )
                .await
                {
                    Ok(resp) => {
                        info!(
                            "[{}] Upstream negotiated HTTP/3 for {}:{}",
                            ctx.id_str(),
                            host,
                            port
                        );
                        Some((resp, start.elapsed().as_millis() as u64))
                    }
                    Err(err) => {
                        warn!(
                            "[{}] Upstream HTTP/3 attempt failed for {}:{}: {}, falling back to HTTP/1.1/2",
                            ctx.id_str(),
                            host,
                            port,
                            err
                        );
                        None
                    }
                }
            }
            Err(err) => {
                warn!(
                    "[{}] Failed to build upstream HTTP/3 request for {}:{}: {}",
                    ctx.id_str(),
                    host,
                    port,
                    err
                );
                None
            }
        }
    } else {
        None
    };

    let outgoing_req = Request::from_parts(parts, outgoing_body);
    let pool_partition =
        build_upstream_pool_partition(&original_host, &host, port, use_tls, &resolved_rules);

    #[cfg(feature = "http3")]
    let upstream_result = if let Some((res, wait_ms)) = h3_attempt {
        let (parts, body) = res.into_parts();
        (
            parts,
            None,
            Some(full_body(body.clone())),
            Some((body, 0)),
            wait_ms,
        )
    } else {
        let send_start = Instant::now();
        let res = match send_pooled_request(
            outgoing_req,
            unsafe_ssl,
            &resolved_rules.dns_servers,
            &pool_partition,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let retryable_upstream_h2 = use_tls
                    && matches!(method.as_str(), "GET" | "HEAD")
                    && retry_blueprint.is_some()
                    && (!e.is_connect() || is_retryable_http2_upstream_error(&e));

                if retryable_upstream_h2 {
                    warn!(
                        "[{}] Upstream HTTP/2 request failed; retrying with HTTP/1.1 fallback",
                        ctx.id_str()
                    );
                    mark_http1_upstream_fallback(
                        unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    );
                    let retry_request = retry_blueprint
                        .as_ref()
                        .expect("retry blueprint exists for retryable request")
                        .build()?;
                    match send_pooled_request_http1_only(
                        retry_request,
                        unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    )
                    .await
                    {
                        Ok(response) => {
                            info!(
                                "[{}] Upstream request recovered via HTTP/1.1 fallback",
                                ctx.id_str()
                            );
                            response
                        }
                        Err(retry_err) => {
                            let classified = classify_request_error(&retry_err);
                            error!(
                                "[{}] {} ({})",
                                ctx.id_str(),
                                classified.error_message,
                                classified.error_type
                            );
                            for source in &classified.source_chain {
                                error!("[{}] Request failure source: {}", ctx.id_str(), source);
                            }
                            return Ok(build_conn_error_and_record(
                                classified.error_type,
                                classified.error_message,
                                None,
                            ));
                        }
                    }
                } else {
                    let classified = classify_request_error(&e);
                    error!(
                        "[{}] {} ({})",
                        ctx.id_str(),
                        classified.error_message,
                        classified.error_type
                    );
                    for source in &classified.source_chain {
                        error!("[{}] Request failure source: {}", ctx.id_str(), source);
                    }
                    return Ok(build_conn_error_and_record(
                        classified.error_type,
                        classified.error_message,
                        None,
                    ));
                }
            }
        };
        let wait_ms = send_start.elapsed().as_millis() as u64;
        let (parts, body) = res.into_parts();
        (parts, Some(body), None, None, wait_ms)
    };

    #[cfg(not(feature = "http3"))]
    let upstream_result = {
        let send_start = Instant::now();
        let res = match send_pooled_request(
            outgoing_req,
            unsafe_ssl,
            &resolved_rules.dns_servers,
            &pool_partition,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let classified = classify_request_error(&e);
                error!(
                    "[{}] {} ({})",
                    ctx.id_str(),
                    classified.error_message,
                    classified.error_type
                );
                for source in &classified.source_chain {
                    error!("[{}] Request failure source: {}", ctx.id_str(), source);
                }
                return Ok(build_conn_error_and_record(
                    classified.error_type,
                    classified.error_message,
                    None,
                ));
            }
        };
        let wait_ms = send_start.elapsed().as_millis() as u64;
        let (parts, body) = res.into_parts();
        (parts, Some(body), None, None, wait_ms)
    };

    let (mut res_parts, mut res_body_incoming, mut res_body_stream, mut pre_read_res, wait_ms) =
        upstream_result;

    let original_res_headers = admin_state
        .as_ref()
        .map(|_| headers_to_pairs(&res_parts.headers));
    let res_content_encoding = response_content_encoding(&res_parts);

    apply_res_rules(&mut res_parts, &resolved_rules, verbose_logging, ctx);

    let needs_processing = needs_body_processing(&resolved_rules);
    let has_res_body_override = resolved_rules.res_body.is_some();
    let needs_res_body_read = needs_processing && !has_res_body_override;

    let is_websocket = res_parts.status == StatusCode::SWITCHING_PROTOCOLS
        && res_parts
            .headers
            .get(hyper::header::UPGRADE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);

    let res_content_length = res_parts
        .headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());

    let res_content_type = get_content_type(&res_parts);
    let is_sse = is_sse_response(&res_parts);
    let binary_traffic_performance_mode = admin_state
        .as_ref()
        .map(|state| state.get_binary_traffic_performance_mode())
        .unwrap_or(false);
    let skip_binary_recording =
        should_use_binary_performance_mode(&res_parts, binary_traffic_performance_mode)
            && !is_websocket
            && !is_sse;
    let metrics_only_forwarding = should_use_metrics_only_forwarding_mode(
        skip_binary_recording,
        has_rules,
        needs_processing,
        is_websocket,
        is_sse,
    );
    let mut res_body_too_large = false;
    let mut res_body_limit = max_body_buffer_size;
    if !is_sse && res_body_stream.is_none() {
        res_body_stream = Some(res_body_incoming.take().unwrap().boxed());
    }
    if needs_res_body_read && needs_processing && !is_sse && !skip_binary_recording {
        if let Some(len) = res_content_length {
            if len > max_body_buffer_size {
                res_body_too_large = true;
                res_body_limit = max_body_buffer_size;
            } else {
                let receive_start = Instant::now();
                let body = res_body_stream.take().unwrap();
                let limit = if !is_likely_text_content_type(&res_content_type) {
                    let probe = max_body_probe_size.min(max_body_buffer_size);
                    if probe == 0 {
                        max_body_buffer_size
                    } else {
                        probe
                    }
                } else {
                    max_body_buffer_size
                };
                res_body_limit = limit;
                match read_body_bounded(body, limit).await {
                    Ok(BoundedBody::Complete(bytes)) => {
                        let receive_ms = receive_start.elapsed().as_millis() as u64;
                        pre_read_res = Some((bytes, receive_ms));
                    }
                    Ok(BoundedBody::Exceeded(replay_body)) => {
                        res_body_too_large = true;
                        res_body_stream = Some(replay_body.boxed());
                    }
                    Err(e) => {
                        return Err(BifrostError::Network(format!(
                            "Failed to read response body: {}",
                            e
                        )))
                    }
                }
            }
        } else {
            let receive_start = Instant::now();
            let body = res_body_stream.take().unwrap();
            let limit = if !is_likely_text_content_type(&res_content_type) {
                let probe = max_body_probe_size.min(max_body_buffer_size);
                if probe == 0 {
                    max_body_buffer_size
                } else {
                    probe
                }
            } else {
                max_body_buffer_size
            };
            res_body_limit = limit;
            match read_body_bounded(body, limit).await {
                Ok(BoundedBody::Complete(bytes)) => {
                    let receive_ms = receive_start.elapsed().as_millis() as u64;
                    pre_read_res = Some((bytes, receive_ms));
                }
                Ok(BoundedBody::Exceeded(replay_body)) => {
                    res_body_too_large = true;
                    res_body_stream = Some(replay_body.boxed());
                }
                Err(e) => {
                    return Err(BifrostError::Network(format!(
                        "Failed to read response body: {}",
                        e
                    )))
                }
            }
        }
    }

    let skip_body_processing = skip_binary_recording
        || is_sse
        || !needs_processing
        || (res_body_too_large && needs_res_body_read);

    if needs_res_body_read && res_body_too_large {
        let size_display = res_content_length
            .map(|len| len.to_string())
            .unwrap_or_else(|| format!(">{}", res_body_limit));
        warn!(
            "[{}] [RES_BODY] body too large ({} bytes > {} limit), skipping body rules and streaming forward",
            ctx.id_str(),
            size_display,
            res_body_limit
        );
    }

    if let Some(delay_ms) = resolved_rules.res_delay {
        if verbose_logging {
            info!("[{}] [RES_DELAY] Sleeping {}ms", ctx.id_str(), delay_ms);
        }
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }

    if let Some(speed) = resolved_rules.res_speed {
        if verbose_logging {
            info!(
                "[{}] [RES_SPEED] Speed limit: {} bytes/s",
                ctx.id_str(),
                speed
            );
        }
    }

    if skip_body_processing {
        let is_streaming =
            is_streaming_response(&res_parts, res_content_length, max_body_buffer_size);
        let res_body_mode = if resolved_rules.trailers.is_empty() {
            BodyMode::Stream
        } else {
            BodyMode::StreamWithTrailers
        };
        normalize_res_headers(&mut res_parts, res_body_mode, &method);
        if verbose_logging && !res_body_too_large {
            if is_sse {
                info!(
                    "[{}] [SSE] detected SSE response, forwarding with event capture",
                    ctx.id_str()
                );
            } else if is_streaming {
                info!(
                    "[{}] [STREAMING] detected streaming response, forwarding directly with tee",
                    ctx.id_str()
                );
            } else {
                debug!(
                    "[{}] No body processing needed, streaming forward with tee",
                    ctx.id_str()
                );
            }
        }

        let total_ms = start_time.elapsed().as_millis() as u64;
        let record_id = ctx.id_str();
        let traffic_type = get_traffic_type_from_url(&record_url);
        let mut sse_stream_writer: Option<bifrost_admin::BodyStreamWriter> = None;

        if let Some(ref state) = admin_state {
            if !metrics_only_forwarding {
                state
                    .metrics_collector
                    .add_bytes_sent_by_type(traffic_type, request_body_size as u64);
                state
                    .metrics_collector
                    .increment_requests_by_type(traffic_type);
            }
            if !metrics_only_forwarding {
                let res_headers = headers_to_pairs(&res_parts.headers);
                let original_res_headers = original_res_headers
                    .as_ref()
                    .expect("response headers captured when admin state is enabled");
                let mut record =
                    TrafficRecord::new(record_id.to_string(), method.clone(), record_url.clone());
                record.status = res_parts.status.as_u16();
                record.content_type = res_parts
                    .headers
                    .get(hyper::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                record.request_size =
                    calculate_request_size(&method, &record_url, &req_headers, request_body_size);
                record.response_size = 0;
                record.duration_ms = total_ms;
                record.timing = Some(RequestTiming {
                    dns_ms,
                    connect_ms: None,
                    tls_ms: None,
                    send_ms: None,
                    wait_ms: Some(wait_ms),
                    first_byte_ms: None,
                    receive_ms: None,
                    total_ms,
                });
                record.request_headers = Some(req_headers.clone());
                record.response_headers = Some(original_res_headers.clone());
                if res_headers != *original_res_headers {
                    record.actual_response_headers = Some(res_headers.clone());
                }
                record.has_rule_hit = has_rules;
                record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                record.request_content_type = req_headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone());
                record.client_ip = ctx.client_ip.clone();
                record.client_app = ctx.client_app.clone();
                record.client_pid = ctx.client_pid;
                record.client_path = ctx.client_path.clone();

                if is_websocket {
                    record.protocol = "ws".to_string();
                    record.set_websocket();
                    state.connection_monitor.register_connection(record_id);
                } else if is_sse {
                    record.set_sse();
                    state.sse_hub.register(record_id);
                } else if is_streaming {
                    record.set_streaming();
                    state.connection_monitor.register_connection(record_id);
                }

                record.request_body_ref = if let Some(ref capture) = req_body_capture {
                    capture.take()
                } else {
                    store_request_body(
                        &admin_state,
                        record_id,
                        &body_bytes,
                        req_content_encoding.as_deref(),
                    )
                };

                if !req_script_results.is_empty() {
                    record.req_script_results = Some(req_script_results.clone());
                }

                if is_sse {
                    if let Some(ref body_store) = state.body_store {
                        match body_store.read().start_stream(record_id, "sse_raw") {
                            Ok(writer) => {
                                record.response_body_ref = Some(writer.body_ref());
                                sse_stream_writer = Some(writer);
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, record_id = %record_id, "failed to start sse raw stream writer");
                            }
                        }
                    }
                }

                state.record_traffic(record);
            }
        }

        if is_sse {
            let res_body = res_body_incoming.take().unwrap();
            let tee_body = create_sse_tee_body(
                res_body,
                admin_state.clone(),
                record_id.to_string(),
                Some(traffic_type),
                sse_stream_writer,
                max_body_buffer_size,
            );
            let final_body = wrap_throttled_body(tee_body.boxed(), resolved_rules.res_speed);
            let body = with_trailers(final_body, &resolved_rules);
            return Ok(Response::from_parts(res_parts, body));
        } else {
            let res_body = res_body_stream.take().unwrap();
            let tee_body = if metrics_only_forwarding {
                res_body
            } else if skip_binary_recording {
                create_metrics_body(res_body, admin_state.clone(), Some(traffic_type))
            } else {
                let res_headers = headers_to_pairs(&res_parts.headers);
                let response_headers_size =
                    calculate_response_headers_size(res_parts.status.as_u16(), &res_headers);
                create_tee_body_with_store(
                    res_body,
                    admin_state.clone(),
                    record_id.to_string(),
                    Some(max_body_buffer_size),
                    res_content_encoding.clone(),
                    Some(traffic_type),
                    response_headers_size,
                )
            };
            let final_body = wrap_throttled_body(tee_body, resolved_rules.res_speed);
            let body = with_trailers(final_body, &resolved_rules);
            return Ok(Response::from_parts(res_parts, body));
        }
    }

    let (res_body_bytes, receive_ms) = if let Some(v) = pre_read_res.take() {
        v
    } else if needs_res_body_read {
        let receive_start = Instant::now();
        let res_body = res_body_stream.take().unwrap();
        let res_body_bytes = res_body
            .collect()
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to read response body: {}", e)))?
            .to_bytes();
        let receive_ms = receive_start.elapsed().as_millis() as u64;
        (res_body_bytes, receive_ms)
    } else {
        (Bytes::new(), 0)
    };

    let content_type = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let original_res_body_len = res_content_length.unwrap_or(res_body_bytes.len());
    let mut final_res_body = if let Some(ref new_body) = resolved_rules.res_body {
        if verbose_logging {
            info!(
                "[{}] [RES_BODY] replaced: {} bytes -> {} bytes",
                ctx.id_str(),
                original_res_body_len,
                new_body.len()
            );
        }
        new_body.clone()
    } else {
        let body_processed = apply_body_rules(
            res_body_bytes.clone(),
            &resolved_rules,
            Phase::Response,
            Some(&content_type),
            verbose_logging,
            ctx,
        );

        apply_content_injection(
            body_processed,
            &content_type,
            &resolved_rules,
            verbose_logging,
            ctx,
        )
    };

    let res_script_results = if has_res_scripts {
        let mut res_script_status = res_parts.status.as_u16();
        let mut res_script_status_text = res_parts
            .status
            .canonical_reason()
            .unwrap_or("OK")
            .to_string();
        let mut res_script_headers = header_map_to_hashmap(&res_parts.headers);
        let mut res_script_body = String::from_utf8(final_res_body.to_vec()).ok();
        let req_script_headers =
            cloned_headers_hashmap(&mut req_headers_hashmap_cache, &req_headers);

        let results = execute_response_scripts(
            &admin_state,
            &resolved_rules.res_scripts,
            ctx,
            &resolved_rules,
            &url,
            &method,
            &req_script_headers,
            &mut res_script_status,
            &mut res_script_status_text,
            &mut res_script_headers,
            &mut res_script_body,
            &values,
        )
        .await;

        if results.iter().any(|r| r.success) {
            if let Ok(new_status) = hyper::StatusCode::from_u16(res_script_status) {
                res_parts.status = new_status;
            }

            let mut new_headers = hyper::HeaderMap::new();
            for (key, value) in &res_script_headers {
                if let (Ok(name), Ok(val)) = (
                    hyper::header::HeaderName::from_bytes(key.as_bytes()),
                    hyper::header::HeaderValue::from_str(value),
                ) {
                    new_headers.insert(name, val);
                }
            }
            res_parts.headers = new_headers;

            if let Some(ref new_body) = res_script_body {
                final_res_body = Bytes::from(new_body.clone());
            }
        }

        results
    } else {
        Vec::new()
    };
    normalize_res_headers(
        &mut res_parts,
        BodyMode::Known(final_res_body.len()),
        &method,
    );

    let total_ms = start_time.elapsed().as_millis() as u64;

    if let Some(ref state) = admin_state {
        let traffic_type = get_traffic_type_from_url(&record_url);
        state
            .metrics_collector
            .add_bytes_sent_by_type(traffic_type, request_body_size as u64);
        state
            .metrics_collector
            .add_bytes_received_by_type(traffic_type, final_res_body.len() as u64);
        state
            .metrics_collector
            .increment_requests_by_type(traffic_type);

        let mut record =
            TrafficRecord::new(ctx.id_str().to_string(), method.clone(), record_url.clone());
        record.status = res_parts.status.as_u16();
        record.content_type = res_parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let res_headers = headers_to_pairs(&res_parts.headers);
        let original_res_headers = original_res_headers
            .as_ref()
            .expect("response headers captured when admin state is enabled");
        record.request_size =
            calculate_request_size(&method, &record_url, &req_headers, request_body_size);
        record.response_size = calculate_response_size(
            res_parts.status.as_u16(),
            &res_headers,
            final_res_body.len(),
        );
        record.duration_ms = total_ms;
        record.timing = Some(RequestTiming {
            dns_ms,
            connect_ms: None,
            tls_ms: None,
            send_ms: None,
            wait_ms: Some(wait_ms),
            first_byte_ms: Some(total_ms),
            receive_ms: Some(receive_ms),
            total_ms,
        });
        record.request_headers = Some(req_headers.clone());
        record.response_headers = Some(original_res_headers.clone());
        if res_headers != *original_res_headers {
            record.actual_response_headers = Some(res_headers.clone());
        }
        record.original_request_headers = Some(
            original_req_headers
                .as_ref()
                .expect("request headers captured when admin state is enabled")
                .clone(),
        );
        if host != original_host || port != original_port {
            let actual_scheme = if use_tls { "https" } else { "http" };
            let actual_url = if (use_tls && port == 443) || (!use_tls && port == 80) {
                format!(
                    "{}://{}{}",
                    actual_scheme,
                    host,
                    processed_uri
                        .path_and_query()
                        .map(|pq| pq.as_str())
                        .unwrap_or("/")
                )
            } else {
                format!(
                    "{}://{}:{}{}",
                    actual_scheme,
                    host,
                    port,
                    processed_uri
                        .path_and_query()
                        .map(|pq| pq.as_str())
                        .unwrap_or("/")
                )
            };
            record.actual_url = Some(actual_url);
            record.actual_host = Some(host.clone());
        }
        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
        record.request_content_type = req_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone());
        record.client_ip = ctx.client_ip.clone();
        record.client_app = ctx.client_app.clone();
        record.client_pid = ctx.client_pid;
        record.client_path = ctx.client_path.clone();

        if is_websocket {
            record.protocol = "ws".to_string();
            record.set_websocket();
            state.connection_monitor.register_connection(ctx.id_str());
        }

        let is_sse = is_sse_response(&res_parts);
        if is_sse {
            record.set_sse();
        }

        if let Some(ref body_store) = state.body_store {
            // decode://script：在落库前进行解码（请求/响应两阶段）
            let (req_host, req_path, req_proto) = parse_url_parts(&record_url);
            let request_data = RequestData {
                url: record_url.clone(),
                method: method.clone(),
                host: req_host,
                path: req_path,
                protocol: req_proto,
                client_ip: ctx.client_ip.clone(),
                client_app: ctx.client_app.clone(),
                headers: cloned_headers_hashmap(&mut req_headers_hashmap_cache, &req_headers),
                body: None,
            };

            let decompressed_req_body = decompress_body_with_limit(
                &final_body,
                req_content_encoding.as_deref(),
                max_decompress_output_bytes,
            );
            let raw_req_body = decompressed_req_body.clone();
            let decoded_req_body = apply_decode_scripts_for_storage(
                &admin_state,
                &resolved_rules.decode_scripts,
                "request",
                ctx,
                &resolved_rules,
                &request_data,
                &ResponseData {
                    request: request_data.clone(),
                    ..Default::default()
                },
                &values,
                decompressed_req_body,
            )
            .await;
            let DecodeForStorageResult {
                output: decoded_req_output,
                results: decoded_req_results,
                ..
            } = decoded_req_body;

            let decompressed_res_body = decompress_body_with_limit(
                &final_res_body,
                res_content_encoding.as_deref(),
                max_decompress_output_bytes,
            );
            let raw_res_body = decompressed_res_body.clone();
            let res_headers_hashmap = headers_to_hashmap(&res_headers);
            let response_data = ResponseData {
                status: res_parts.status.as_u16(),
                status_text: res_parts
                    .status
                    .canonical_reason()
                    .unwrap_or("OK")
                    .to_string(),
                headers: res_headers_hashmap,
                body: None,
                request: request_data,
            };
            let decoded_res_body = apply_decode_scripts_for_storage(
                &admin_state,
                &resolved_rules.decode_scripts,
                "response",
                ctx,
                &resolved_rules,
                &response_data.request,
                &response_data,
                &values,
                decompressed_res_body,
            )
            .await;
            let DecodeForStorageResult {
                output: decoded_res_output,
                results: decoded_res_results,
                ..
            } = decoded_res_body;

            let store = body_store.read();

            if !resolved_rules.decode_scripts.is_empty() {
                record.raw_request_body_ref =
                    store.store(ctx.id_str(), "req_raw", raw_req_body.as_ref());
                record.raw_response_body_ref =
                    store.store(ctx.id_str(), "res_raw", raw_res_body.as_ref());

                if !decoded_req_results.is_empty() {
                    record.decode_req_script_results = Some(decoded_req_results.clone());
                }
                if !decoded_res_results.is_empty() {
                    record.decode_res_script_results = Some(decoded_res_results.clone());
                }
            }

            record.request_body_ref = store.store(ctx.id_str(), "req", decoded_req_output.as_ref());
            record.response_body_ref =
                store.store(ctx.id_str(), "res", decoded_res_output.as_ref());
        }

        if !req_script_results.is_empty() {
            record.req_script_results = Some(req_script_results.clone());
        }
        if !res_script_results.is_empty() {
            record.res_script_results = Some(res_script_results.clone());
        }

        if is_sse {
            let event_count = parse_and_record_sse_events(&final_res_body);
            let response_size = final_res_body.len();
            record.response_size = response_size;
            record.frame_count = event_count;
            record.last_frame_id = event_count as u64;
            record.socket_status = Some(bifrost_admin::SocketStatus {
                is_open: false,
                send_count: 0,
                receive_count: event_count as u64,
                send_bytes: 0,
                receive_bytes: response_size as u64,
                frame_count: event_count,
                close_code: None,
                close_reason: Some("SSE stream completed".to_string()),
            });
        }

        state.record_traffic(record);
    }

    let response_body = wrap_throttled_body(full_body(final_res_body), resolved_rules.res_speed);
    let body = with_trailers(response_body, &resolved_rules);
    Ok(Response::from_parts(res_parts, body))
}

fn build_redirect_response(status_code: u16, location: &str) -> Response<BoxBody> {
    let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::FOUND);
    let body = format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Redirect</title></head>
<body><a href="{}">Redirecting...</a></body>
</html>"#,
        location
    );

    Response::builder()
        .status(status)
        .header(hyper::header::LOCATION, location)
        .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(full_body(bytes::Bytes::from(body)))
        .unwrap()
}

fn extract_host_port(uri: &Uri, rules: &ResolvedRules, is_https: bool) -> Result<(String, u16)> {
    let default_port = get_default_port(&rules.host_protocol, is_https);

    if !rules.ignored.host {
        if let Some(ref host_rule) = rules.host {
            let host_without_path = host_rule.split('/').next().unwrap_or(host_rule);
            let parts: Vec<&str> = host_without_path.split(':').collect();
            let host = parts[0].to_string();
            let port = if parts.len() > 1 {
                parts[1].parse().unwrap_or(default_port)
            } else {
                default_port
            };
            return Ok((host, port));
        }
    }

    if let Some(ref proxy_rule) = rules.proxy {
        if proxy_rule.starts_with("http://") || proxy_rule.starts_with("https://") {
            if let Ok(url) = Url::parse(proxy_rule) {
                if let Some(host) = url.host_str() {
                    let port = url.port().unwrap_or(default_port);
                    return Ok((host.to_string(), port));
                }
            }
        }
        let host_without_path = proxy_rule.split('/').next().unwrap_or(proxy_rule);
        let parts: Vec<&str> = host_without_path.split(':').collect();
        let host = parts[0].to_string();
        let port = if parts.len() > 1 {
            parts[1].parse().unwrap_or(default_port)
        } else {
            default_port
        };
        return Ok((host, port));
    }

    let host = uri
        .host()
        .ok_or_else(|| BifrostError::Network("Missing host in URI".to_string()))?
        .to_string();

    let port = uri.port_u16().unwrap_or(default_port);

    Ok((host, port))
}

fn get_default_port(host_protocol: &Option<Protocol>, is_https: bool) -> u16 {
    match host_protocol {
        Some(Protocol::Http) | Some(Protocol::Ws) => 80,
        Some(Protocol::Https) | Some(Protocol::Wss) => 443,
        None | Some(Protocol::Host) => {
            if is_https {
                443
            } else {
                80
            }
        }
        _ => 80,
    }
}

async fn handle_http_websocket(
    req: Request<Incoming>,
    rules: Arc<dyn RulesResolver>,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    unsafe_ssl: bool,
) -> Result<Response<BoxBody>> {
    use super::websocket::websocket_bidirectional_generic_with_capture;
    use crate::server::empty_body;
    use tokio::io::AsyncWriteExt;
    use tokio_rustls::rustls::pki_types::ServerName;

    let start_time = Instant::now();
    let uri = req.uri().clone();
    let method = req.method().to_string();

    let forwarded_proto = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|p| p.split(',').next().unwrap_or(p).trim().to_ascii_lowercase());

    let host_header = req
        .headers()
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(',').next().unwrap_or(h).trim().to_string())
        .or_else(|| uri.host().map(|h| h.to_string()))
        .or_else(|| {
            req.headers()
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(|h| h.trim().to_string())
        })
        .ok_or_else(|| BifrostError::Network("Missing host in WebSocket request".to_string()))?;

    let (host, host_port_from_header) = if let Some((h, p)) = host_header.rsplit_once(':') {
        if let Ok(p) = p.parse::<u16>() {
            (h.to_string(), Some(p))
        } else {
            (host_header.clone(), None)
        }
    } else {
        (host_header.clone(), None)
    };

    let is_wss = matches!(uri.scheme_str(), Some("wss" | "https"))
        || matches!(forwarded_proto.as_deref(), Some("wss" | "https"))
        || matches!(uri.port_u16(), Some(443 | 8443))
        || matches!(host_port_from_header, Some(443 | 8443));

    let port = uri
        .port_u16()
        .or(host_port_from_header)
        .unwrap_or(if is_wss { 443 } else { 80 });

    let ws_scheme = if is_wss { "wss" } else { "ws" };
    let ws_url = if let Some(authority) = uri.authority().map(|a| a.as_str()) {
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        format!("{}://{}{}", ws_scheme, authority, path)
    } else {
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        format!("{}://{}{}", ws_scheme, host_header, path)
    };

    let http_scheme = if is_wss { "https" } else { "http" };
    let http_url = if let Some(authority) = uri.authority().map(|a| a.as_str()) {
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        format!("{}://{}{}", http_scheme, authority, path)
    } else {
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        format!("{}://{}{}", http_scheme, host_header, path)
    };

    let mut resolved_rules = rules.resolve(&ws_url, "GET");
    if resolved_rules.rules.is_empty() && resolved_rules.host.is_none() {
        resolved_rules = rules.resolve(&http_url, "GET");
    }
    let has_rules = !resolved_rules.rules.is_empty() || resolved_rules.host.is_some();

    let req_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let (target_host, target_port) = if let Some(ref host_rule) = resolved_rules.host {
        let parts: Vec<&str> = host_rule.split(':').collect();
        let h = parts[0].to_string();
        let p = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(port);
        (h, p)
    } else {
        (host.to_string(), port)
    };

    debug!(
        "[{}] WebSocket upgrade via HTTP proxy to {}:{}",
        ctx.id_str(),
        target_host,
        target_port
    );

    let connect_start = Instant::now();
    let target_stream = TcpStream::connect(format!("{}:{}", target_host, target_port))
        .await
        .map_err(|e| {
            BifrostError::Network(format!(
                "Failed to connect to {}:{}: {}",
                target_host, target_port, e
            ))
        })?;
    let tcp_connect_ms = connect_start.elapsed().as_millis() as u64;

    if let Err(e) = target_stream.set_nodelay(true) {
        debug!("Failed to set TCP_NODELAY on WebSocket connection: {}", e);
    }

    let use_tls = match resolved_rules.host_protocol {
        Some(Protocol::Http) | Some(Protocol::Ws) => false,
        Some(Protocol::Https) | Some(Protocol::Wss) => true,
        Some(Protocol::Host) | Some(Protocol::XHost) => target_port == 443 || target_port == 8443,
        _ => is_wss,
    };
    let mut target_stream: Box<dyn AsyncReadWrite + Unpin + Send> = if use_tls {
        let tls_config = super::tunnel::get_tls_client_config_http1_only(unsafe_ssl);
        let connector = TlsConnector::from(tls_config);

        let server_name = ServerName::try_from(target_host.clone()).map_err(|_| {
            BifrostError::Network(format!("Invalid server name for TLS: {}", target_host))
        })?;

        let tls_stream = connector
            .connect(server_name, target_stream)
            .await
            .map_err(|e| BifrostError::Network(format!("TLS handshake failed: {}", e)))?;

        Box::new(tls_stream)
    } else {
        Box::new(target_stream)
    };

    let upgrade_request = build_http_websocket_handshake(&req, &target_host, target_port)?;
    target_stream
        .write_all(upgrade_request.as_bytes())
        .await
        .map_err(|e| BifrostError::Network(format!("Failed to send WS handshake: {}", e)))?;

    let websocket_handshake_max_header_size = if let Some(ref state) = admin_state {
        if let Some(ref config_manager) = state.config_manager {
            config_manager
                .config()
                .await
                .server
                .websocket_handshake_max_header_size
        } else {
            64 * 1024
        }
    } else {
        64 * 1024
    };

    let (upstream_resp, upstream_leftover) =
        read_http1_response_with_leftover(&mut target_stream, websocket_handshake_max_header_size)
            .await?;
    if upstream_resp.status_code != 101 {
        return Err(BifrostError::Network(format!(
            "WebSocket handshake failed: {} {}",
            upstream_resp.status_code, upstream_resp.status_text
        )));
    }

    let response_headers = upstream_resp.headers.clone();
    let sec_accept = upstream_resp
        .header("Sec-WebSocket-Accept")
        .map(|v| v.to_string());

    let upstream_protocol = upstream_resp.header("Sec-WebSocket-Protocol");
    let upstream_extensions = header_values(&upstream_resp, "Sec-WebSocket-Extensions");

    let client_protocol = req
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok());
    let client_extensions = req
        .headers()
        .get("Sec-WebSocket-Extensions")
        .and_then(|v| v.to_str().ok());

    let negotiated_protocol = negotiate_protocol(client_protocol, upstream_protocol);
    let negotiated_extensions = negotiate_extensions(client_extensions, &upstream_extensions);
    let compression_cfg = negotiated_extensions
        .as_deref()
        .and_then(crate::protocol::parse_permessage_deflate_config);
    let _compression_enabled = compression_cfg.is_some();
    let ws_meta = super::ws_decode::WsHandshakeMeta {
        negotiated_protocol: negotiated_protocol.clone(),
        negotiated_extensions: negotiated_extensions.clone(),
    };

    let total_ms = start_time.elapsed().as_millis() as u64;
    let record_id = ctx.id_str().to_string();

    if let Some(ref state) = admin_state {
        state
            .metrics_collector
            .increment_requests_by_type(bifrost_admin::TrafficType::Ws);

        let record_protocol = if use_tls { "wss" } else { "ws" };
        let ws_url = format!(
            "{}://{}:{}{}",
            record_protocol,
            host,
            port,
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
        );

        let mut record =
            bifrost_admin::TrafficRecord::new(record_id.to_string(), method.clone(), ws_url);
        record.status = 101;
        record.protocol = record_protocol.to_string();
        record.duration_ms = total_ms;
        record.timing = Some(bifrost_admin::RequestTiming {
            dns_ms: None,
            connect_ms: Some(tcp_connect_ms),
            tls_ms: None,
            send_ms: None,
            wait_ms: Some(total_ms.saturating_sub(tcp_connect_ms)),
            first_byte_ms: Some(total_ms),
            receive_ms: None,
            total_ms,
        });
        record.request_headers = Some(req_headers.clone());
        record.response_headers = Some(response_headers.clone());
        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
        record.client_ip = ctx.client_ip.clone();
        record.client_app = ctx.client_app.clone();
        record.client_pid = ctx.client_pid;
        record.client_path = ctx.client_path.clone();
        record.set_websocket();

        state.connection_monitor.register_connection(&record_id);
        state.record_traffic(record);
    }

    let record_id_clone = record_id.clone();
    let admin_state_clone = admin_state.clone();
    let ws_ctx = ctx.clone();
    let ws_rules = resolved_rules.clone();
    let ws_req_url = ws_url.clone();
    let ws_req_method = method.clone();
    let ws_req_headers = req_headers.clone();
    let ws_decode_scripts = ws_rules.decode_scripts.clone();
    let ws_compression_cfg = compression_cfg.clone();
    let ws_meta_spawn = ws_meta.clone();
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                if let Err(e) = websocket_bidirectional_generic_with_capture(
                    upgraded,
                    target_stream,
                    &record_id_clone,
                    admin_state_clone.clone(),
                    ws_compression_cfg,
                    upstream_leftover,
                    ws_ctx,
                    ws_rules,
                    ws_req_url,
                    ws_req_method,
                    ws_req_headers,
                    ws_meta_spawn,
                    ws_decode_scripts,
                )
                .await
                {
                    error!("[{}] WebSocket tunnel error: {}", record_id_clone, e);
                }

                if let Some(ref state) = admin_state_clone {
                    state.connection_monitor.set_connection_closed(
                        &record_id_clone,
                        None,
                        None,
                        state.frame_store.as_ref(),
                        state.ws_payload_store.as_ref(),
                    );
                }
            }
            Err(e) => {
                error!("[{}] WebSocket upgrade error: {}", record_id_clone, e);
            }
        }
    });

    let mut response = Response::builder()
        .status(101)
        .header(hyper::header::UPGRADE, "websocket")
        .header(hyper::header::CONNECTION, "Upgrade");

    if let Some(accept) = sec_accept {
        response = response.header("Sec-WebSocket-Accept", accept);
    }

    if let Some(protocol) = negotiated_protocol {
        response = response.header("Sec-WebSocket-Protocol", protocol);
    }

    if let Some(extensions) = negotiated_extensions {
        response = response.header("Sec-WebSocket-Extensions", extensions);
    }

    for (name, value) in response_headers {
        let lower = name.to_ascii_lowercase();
        if lower != "upgrade"
            && lower != "connection"
            && lower != "sec-websocket-accept"
            && lower != "sec-websocket-protocol"
            && lower != "sec-websocket-extensions"
        {
            response = response.header(name, value);
        }
    }

    Ok(response.body(empty_body()).unwrap())
}

fn build_http_websocket_handshake(
    req: &Request<Incoming>,
    target_host: &str,
    target_port: u16,
) -> Result<String> {
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let host_header = if target_port == 80 {
        target_host.to_string()
    } else {
        format!("{}:{}", target_host, target_port)
    };

    let ws_key = req
        .headers()
        .get("Sec-WebSocket-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let ws_version = req
        .headers()
        .get("Sec-WebSocket-Version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("13");

    let mut handshake = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: {}\r\n",
        path, host_header, ws_key, ws_version
    );

    for (name, value) in req.headers().iter() {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || n.eq_ignore_ascii_case("upgrade")
            || n.eq_ignore_ascii_case("connection")
            || n.eq_ignore_ascii_case("sec-websocket-key")
            || n.eq_ignore_ascii_case("sec-websocket-version")
            || n.eq_ignore_ascii_case("sec-websocket-protocol")
            || n.eq_ignore_ascii_case("sec-websocket-extensions")
            || n.eq_ignore_ascii_case("content-length")
            || n.eq_ignore_ascii_case("transfer-encoding")
            || n.eq_ignore_ascii_case("proxy-connection")
            || n.eq_ignore_ascii_case("keep-alive")
            || n.eq_ignore_ascii_case("te")
            || n.eq_ignore_ascii_case("trailer")
        {
            continue;
        }

        if let Ok(v) = value.to_str() {
            handshake.push_str(&format!("{}: {}\r\n", n, v));
        }
    }

    if let Some(protocol) = req.headers().get("Sec-WebSocket-Protocol") {
        if let Ok(protocol_str) = protocol.to_str() {
            handshake.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", protocol_str));
        }
    }

    if let Some(extensions) = req.headers().get("Sec-WebSocket-Extensions") {
        if let Ok(ext_str) = extensions.to_str() {
            handshake.push_str(&format!("Sec-WebSocket-Extensions: {}\r\n", ext_str));
        }
    }

    handshake.push_str("\r\n");
    Ok(handshake)
}

pub fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
    if req.version() == hyper::Version::HTTP_2
        && req.method() == hyper::Method::CONNECT
        && req
            .extensions()
            .get::<hyper::ext::Protocol>()
            .is_some_and(|protocol| protocol.as_str().eq_ignore_ascii_case("websocket"))
    {
        return true;
    }

    let connection = req
        .headers()
        .get(hyper::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let upgrade = req
        .headers()
        .get(hyper::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    connection.to_lowercase().contains("upgrade") && upgrade.to_lowercase() == "websocket"
}

pub fn get_request_url(req: &Request<Incoming>) -> String {
    let uri = req.uri();
    if uri.scheme().is_some() {
        uri.to_string()
    } else {
        let host = req
            .headers()
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost");
        format!(
            "http://{}{}",
            host,
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
        )
    }
}

pub fn parse_and_record_sse_events(body: &[u8]) -> usize {
    let body_str = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let mut current_event = String::new();
    let mut count = 0usize;
    for line in body_str.lines() {
        if line.is_empty() {
            if !current_event.is_empty() {
                current_event.clear();
                count += 1;
            }
        } else {
            if !current_event.is_empty() {
                current_event.push('\n');
            }
            current_event.push_str(line);
        }
    }

    if !current_event.is_empty() {
        count += 1;
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Method;
    use hyper::Uri;
    use hyper::Version;

    #[test]
    fn test_extract_host_port_from_uri() {
        let uri: Uri = "http://example.com:8080/path".parse().unwrap();
        let rules = ResolvedRules::default();
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);
    }

    #[test]
    fn test_is_websocket_upgrade_accepts_http11_upgrade() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://example.com/socket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header(hyper::header::UPGRADE, "websocket")
            .body(())
            .unwrap();

        assert!(is_websocket_upgrade(&req));
    }

    #[test]
    fn test_is_websocket_upgrade_accepts_http2_extended_connect() {
        let req = Request::builder()
            .method(Method::CONNECT)
            .uri("https://example.com/socket")
            .version(Version::HTTP_2)
            .extension(hyper::ext::Protocol::from_static("websocket"))
            .body(())
            .unwrap();

        assert!(is_websocket_upgrade(&req));
    }

    #[test]
    fn test_is_websocket_upgrade_rejects_plain_http2_connect() {
        let req = Request::builder()
            .method(Method::CONNECT)
            .uri("https://example.com/socket")
            .version(Version::HTTP_2)
            .body(())
            .unwrap();

        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn test_extract_host_port_default_port() {
        let uri: Uri = "http://example.com/path".parse().unwrap();
        let rules = ResolvedRules::default();
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_extract_host_port_with_rule_override() {
        let uri: Uri = "http://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            host: Some("override.com:9000".to_string()),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "override.com");
        assert_eq!(port, 9000);
    }

    #[test]
    fn test_extract_host_port_rule_without_port() {
        let uri: Uri = "http://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            host: Some("override.com".to_string()),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "override.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_extract_host_port_rule_with_path() {
        let uri: Uri = "http://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            host: Some("127.0.0.1:3020/ws".to_string()),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 3020);
    }

    #[test]
    fn test_extract_host_port_https_default_port() {
        let uri: Uri = "https://example.com/path".parse().unwrap();
        let rules = ResolvedRules::default();
        let (host, port) = extract_host_port(&uri, &rules, true).unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_extract_host_port_https_rule_without_port() {
        let uri: Uri = "https://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            host: Some("override.com".to_string()),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, true).unwrap();
        assert_eq!(host, "override.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_extract_host_port_http_protocol_forces_port_80() {
        let uri: Uri = "https://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            host: Some("override.com".to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, true).unwrap();
        assert_eq!(host, "override.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_extract_host_port_https_protocol_forces_port_443() {
        let uri: Uri = "http://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            host: Some("override.com".to_string()),
            host_protocol: Some(Protocol::Https),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "override.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_get_default_port() {
        assert_eq!(get_default_port(&None, false), 80);
        assert_eq!(get_default_port(&None, true), 443);
        assert_eq!(get_default_port(&Some(Protocol::Host), false), 80);
        assert_eq!(get_default_port(&Some(Protocol::Host), true), 443);
        assert_eq!(get_default_port(&Some(Protocol::Http), false), 80);
        assert_eq!(get_default_port(&Some(Protocol::Http), true), 80);
        assert_eq!(get_default_port(&Some(Protocol::Https), false), 443);
        assert_eq!(get_default_port(&Some(Protocol::Https), true), 443);
        assert_eq!(get_default_port(&Some(Protocol::Ws), false), 80);
        assert_eq!(get_default_port(&Some(Protocol::Wss), true), 443);
    }

    #[test]
    fn test_upstream_pool_partition_separates_different_route_rules() {
        let host_rules = ResolvedRules {
            host: Some("127.0.0.1:3000".to_string()),
            ..Default::default()
        };
        let proxy_rules = ResolvedRules {
            proxy: Some("127.0.0.1:9999".to_string()),
            ..Default::default()
        };

        let host_partition =
            build_upstream_pool_partition("example.com", "127.0.0.1", 3000, false, &host_rules);
        let proxy_partition =
            build_upstream_pool_partition("example.com", "127.0.0.1", 9999, false, &proxy_rules);

        assert_ne!(host_partition, proxy_partition);
    }

    #[test]
    fn test_metrics_only_forwarding_mode_only_for_binary_fast_path() {
        assert!(!should_use_metrics_only_forwarding_mode(
            false, false, false, false, false
        ));
        assert!(should_use_metrics_only_forwarding_mode(
            true, false, false, false, false
        ));
        assert!(should_use_metrics_only_forwarding_mode(
            true, true, false, false, false
        ));
        assert!(!should_use_metrics_only_forwarding_mode(
            true, false, true, false, false
        ));
    }
}
