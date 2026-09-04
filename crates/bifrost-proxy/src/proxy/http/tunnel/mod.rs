use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::ensure_crypto_provider;
#[cfg(feature = "http3")]
use crate::http3::Http3Client;
use crate::protocol::{HttpResponse, ProtocolDetector, TransportProtocol};
use bifrost_admin::{
    AdminRouter, AdminState, ConnectionInfo, RequestTiming, SharedPushManager, TrafficRecord,
    TrafficType, ADMIN_PATH_PREFIX,
};
use bifrost_core::{
    rule_share::{encode_rule_share_payload, extract_rule_share_query, RULE_SHARE_QUERY_PARAM},
    BifrostError, Protocol, Result,
};
use bifrost_script::{RequestData, ResponseData};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::HeaderMap;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as AutoServerBuilder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, error, info, warn};

mod bidirectional;
mod cert;
mod client;
mod host_rule;
mod io;

pub use self::bidirectional::{tunnel_bidirectional_with_cancel, TunnelStats};
pub use self::cert::SingleCertResolver;
use self::host_rule::parse_host_rule;
use self::io::{BufferedIo, CombinedAsyncRw};

use super::body_metadata::{
    buffered_res_body_mode, content_encoding_is_identity, normalize_req_headers,
    normalize_res_headers, response_content_encoding, set_content_encoding_header,
    streaming_res_body_mode, BodyMode,
};
use super::breakpoint::breakpoint_tls_interception_required as bp_tls;
use super::breakpoint::{
    apply_edited_response_status, apply_edited_response_status_and_body, body_read_error_response,
    response_breakpoint_can_buffer_body,
};
use super::devtools::{
    attach_devtools_client_req_id, devtools_bridge_requested, is_devtools_client_req_id_header,
    maybe_inject_devtools_bridge_html, take_devtools_client_req_id,
    take_devtools_client_req_id_from_uri,
};
use super::handler::decode::{apply_decode_scripts_for_storage, DecodeForStorageResult};
use super::handler::{
    apply_immediate_response_body_rules, apply_websocket_request_header_rules,
    apply_websocket_response_header_rules, build_connection_error_body,
    build_connection_error_response_from_body, build_overridden_error_response_from_body,
    configure_stream_script_response_headers, connect_via_upstream_http_proxy_tunnel,
    merge_websocket_header_rule_candidates, needs_body_processing, needs_request_body_processing,
    needs_response_override, needs_response_phase_resolve, parse_and_record_sse_events,
    request_explicitly_accepts_html, ConnectionErrorInfo,
};
use super::scripts::{
    apply_script_headers_to_header_map, body_to_script_string, create_response_stream_script_body,
    execute_request_scripts, execute_response_scripts, header_map_to_hashmap,
    header_pairs_to_hashmap, initialize_response_stream_script, parse_url_parts,
    script_string_to_body,
};
use super::ws_handshake::{
    header_values, negotiate_extensions, negotiate_protocol, read_http1_response_with_leftover,
};
use crate::dns::DnsResolver;
use crate::server::ADMIN_VIRTUAL_HOST;
use crate::server::{
    empty_body, full_body, with_trailers, BoxBody, ProxyConfig, ResolvedRules, RulesResolver,
    TlsConfig, TlsInterceptConfig,
};
use crate::transform::collect_all_cookies_from_headers;
use crate::transform::decompress::get_content_encoding;
use crate::transform::merge_cookie_header_values;
use crate::transform::{
    apply_body_rules_preserving_encoding, apply_content_injection_preserving_encoding,
    ContentInjectionEncoding, Phase,
};
use crate::transform::{apply_req_rules, apply_res_rules};
use crate::transform::{compress_body, maybe_inject_bifrost_badge_html};
use crate::utils::bounded::{read_body_bounded, BoundedBody};
use crate::utils::http_size::{
    calculate_request_size, calculate_response_headers_size, calculate_response_size,
};
use crate::utils::logging::{format_rules_summary, RequestContext};
use crate::utils::process_info::{
    spawn_async_process_resolver_with_finish, ClientProcess, ConnectionProcessState,
};
use crate::utils::tee::{
    create_metrics_body, create_request_tee_body, create_sse_tee_body, create_tee_body_with_store,
    store_request_body, store_response_body, BodyCaptureHandle, SseTeeOptions,
    TeeBodyCaptureOptions,
};
use crate::utils::throttle::wrap_throttled_body;
use crate::utils::upstream_stability::connect_tcp;
use crate::utils::url::apply_url_rules;

fn websocket_handshake_rejection_category(status_code: u16) -> Option<&'static str> {
    (status_code != 101).then_some("upstream_handshake_rejected")
}

const WEBSOCKET_REJECTION_LOG_WINDOW: Duration = Duration::from_secs(30);
const WEBSOCKET_REJECTION_LOG_MAX_KEYS: usize = 128;

#[derive(Debug)]
struct WebSocketRejectionLogEntry {
    last_logged_at: Instant,
    suppressed: u64,
}

#[derive(Debug, Default)]
struct WebSocketRejectionLogLimiter {
    entries: HashMap<(String, u16), WebSocketRejectionLogEntry>,
}

impl WebSocketRejectionLogLimiter {
    fn record(&mut self, host: &str, status_code: u16, now: Instant) -> Option<u64> {
        let key = (host.to_ascii_lowercase(), status_code);
        if let Some(entry) = self.entries.get_mut(&key) {
            if now.duration_since(entry.last_logged_at) < WEBSOCKET_REJECTION_LOG_WINDOW {
                entry.suppressed = entry.suppressed.saturating_add(1);
                return None;
            }
            let suppressed = entry.suppressed;
            entry.last_logged_at = now;
            entry.suppressed = 0;
            return Some(suppressed);
        }

        if self.entries.len() >= WEBSOCKET_REJECTION_LOG_MAX_KEYS {
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_logged_at)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(
            key,
            WebSocketRejectionLogEntry {
                last_logged_at: now,
                suppressed: 0,
            },
        );
        Some(0)
    }
}

static WEBSOCKET_REJECTION_LOG_LIMITER: LazyLock<Mutex<WebSocketRejectionLogLimiter>> =
    LazyLock::new(|| Mutex::new(WebSocketRejectionLogLimiter::default()));

pub(in crate::proxy::http) fn log_upstream_websocket_rejection(
    req_id: &str,
    target_host: &str,
    status_code: u16,
    status_text: &str,
) {
    let suppressed = WEBSOCKET_REJECTION_LOG_LIMITER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record(target_host, status_code, Instant::now());
    if let Some(suppressed) = suppressed {
        warn!(
            target: "bifrost_proxy::websocket",
            request_id = %req_id,
            target_host = %target_host,
            error_category = "upstream_handshake_rejected",
            upstream_status = status_code,
            upstream_status_text = %status_text,
            suppressed_since_last_log = suppressed,
            "upstream rejected WebSocket handshake"
        );
    }
}

fn websocket_rejection_response(
    req_id: &str,
    target_host: &str,
    upstream_response: &HttpResponse,
) -> Option<Response<BoxBody>> {
    websocket_handshake_rejection_category(upstream_response.status_code)?;
    log_upstream_websocket_rejection(
        req_id,
        target_host,
        upstream_response.status_code,
        &upstream_response.status_text,
    );
    Some(
        Response::builder()
            .status(502)
            .body(full_body(b"WebSocket handshake failed".to_vec()))
            .unwrap(),
    )
}

fn apply_listener_context(
    record: &mut TrafficRecord,
    listener_port: u16,
    client_ip: &str,
    client_app: &Option<String>,
    client_pid: Option<u32>,
    client_path: &Option<String>,
    account_name: &Option<String>,
) {
    record.listener_port = listener_port;
    record.client_ip = client_ip.to_string();
    record.client_app = client_app.clone();
    record.client_pid = client_pid;
    record.client_path = client_path.clone();
    record.account_name = account_name.clone();
}

async fn get_values_from_state(admin_state: &Option<Arc<AdminState>>) -> HashMap<String, String> {
    use bifrost_core::ValueStore;
    if let Some(state) = admin_state {
        if let Some(values_storage) = &state.values_storage {
            let storage = values_storage.read();
            return storage.as_hashmap();
        }
    }
    HashMap::new()
}

fn apply_tunnel_client_process_backfill(
    state: &AdminState,
    connection_process_state: &ConnectionProcessState,
    req_id: String,
    process: ClientProcess,
) {
    let process = connection_process_state.store(Arc::new(process));
    info!(
        req_id,
        client_app = %process.name,
        client_pid = process.pid,
        client_path = ?process.path,
        "Applying tunnel client process backfill to traffic record"
    );
    state.update_client_process(
        &req_id,
        process.name.clone(),
        process.pid,
        process.path.clone(),
    );
    state
        .connection_registry
        .update_client_app(&req_id, process.name.clone());
}

fn maybe_backfill_tunnel_client_process(
    state: &Arc<AdminState>,
    connection_process_state: &Arc<ConnectionProcessState>,
    req_id: &str,
    has_client_process: bool,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    skip_unknown_backfill: bool,
) {
    if has_client_process {
        debug!(
            req_id,
            "Skipping tunnel client process backfill because client metadata is already present"
        );
        return;
    }

    if skip_unknown_backfill {
        debug!(
            req_id,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "Skipping tunnel client process backfill after synchronous resolution miss"
        );
        return;
    }

    if !peer_addr.ip().is_loopback() {
        debug!(
            req_id,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "Skipping tunnel client process backfill for non-loopback client"
        );
        return;
    }

    debug!(
        req_id,
        peer_addr = %peer_addr,
        local_addr = %local_addr,
        "Scheduling tunnel client process backfill"
    );

    if !connection_process_state.try_start_background_resolution() {
        debug!(
            req_id,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "Skipping tunnel client process backfill because this connection already has a resolver in flight"
        );
        return;
    }

    let state = Arc::clone(state);
    let state_for_success = Arc::clone(connection_process_state);
    let state_for_finish = Arc::clone(connection_process_state);
    spawn_async_process_resolver_with_finish(
        peer_addr,
        local_addr,
        req_id.to_string(),
        move |id, process| {
            apply_tunnel_client_process_backfill(&state, &state_for_success, id, process);
        },
        move || state_for_finish.finish_background_resolution(),
    );
}

fn finalize_tunnel_tracking(state: &Arc<AdminState>, req_id: &str) {
    state
        .metrics_collector
        .decrement_connections_by_type(TrafficType::Tunnel);
    state.connection_registry.unregister(req_id);

    let socket_status = state.connection_monitor.close_connection(
        req_id,
        None,
        None,
        state.frame_store.as_ref(),
        state.ws_payload_store.as_ref(),
    );

    if let Some(socket_status) = socket_status {
        let req_id = req_id.to_string();
        state.update_traffic_by_id(&req_id, move |record| {
            record.upload_bytes = record.upload_bytes.max(socket_status.send_bytes as usize);
            record.download_bytes = record
                .download_bytes
                .max(socket_status.receive_bytes as usize);
            record.request_size = record.request_size.max(socket_status.send_bytes as usize);
            record.response_size = record
                .response_size
                .max(socket_status.receive_bytes as usize);
            record.socket_status = Some(socket_status.clone());
        });
    }
}

pub fn get_tls_client_config(unsafe_ssl: bool) -> Arc<ClientConfig> {
    client::get_tls_client_config(unsafe_ssl)
}

pub fn get_tls_client_config_http1_only(unsafe_ssl: bool) -> Arc<ClientConfig> {
    client::get_tls_client_config_http1_only(unsafe_ssl)
}

pub fn get_tls_client_config_without_alpn(unsafe_ssl: bool) -> Arc<ClientConfig> {
    client::get_tls_client_config_without_alpn(unsafe_ssl)
}

fn is_standard_tls_intercept_port(port: u16) -> bool {
    matches!(port, 443 | 8443)
}

fn should_sniff_tls_payload(client_alpn: Option<&[u8]>, port: u16) -> bool {
    client_alpn.is_none() || !is_standard_tls_intercept_port(port)
}

fn tls_authority(host: &str, port: u16, include_default_port: bool) -> String {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if port == 443 && !include_default_port {
        host.to_string()
    } else {
        super::handler::format_connection_endpoint(host, port)
    }
}

fn tls_request_url(scheme: &str, host: &str, port: u16, path: &str) -> String {
    format!("{scheme}://{}{path}", tls_authority(host, port, false))
}

fn is_explicit_tls_intercept_override(
    host: &str,
    client_app: Option<&str>,
    tls_intercept_config: &TlsInterceptConfig,
    resolved_rules: &ResolvedRules,
) -> bool {
    resolved_rules.tls_intercept == Some(true)
        || is_domain_included(host, &tls_intercept_config.intercept_include)
        || is_app_included(client_app, &tls_intercept_config.app_intercept_include)
}

fn requires_tls_interception_for_host_rewrite(resolved_rules: &ResolvedRules) -> bool {
    resolved_rules.host.is_some()
        && matches!(
            resolved_rules.host_protocol,
            Some(Protocol::Http | Protocol::Ws)
        )
}

pub fn requires_tls_interception_for_rules(resolved_rules: &ResolvedRules) -> bool {
    let has_interceptable_fields = !resolved_rules.res_headers.is_empty()
        || !resolved_rules.req_headers.is_empty()
        || !resolved_rules.delete_res_headers.is_empty()
        || !resolved_rules.delete_req_headers.is_empty()
        || resolved_rules.res_body.is_some()
        || resolved_rules.req_body.is_some()
        || resolved_rules.status_code.is_some()
        || resolved_rules.replace_status.is_some()
        || resolved_rules.mock_file.is_some()
        || resolved_rules.mock_rawfile.is_some()
        || resolved_rules.mock_template.is_some()
        || !resolved_rules.res_replace.is_empty()
        || !resolved_rules.res_replace_regex.is_empty()
        || !resolved_rules.req_replace.is_empty()
        || !resolved_rules.req_replace_regex.is_empty()
        || resolved_rules.res_prepend.is_some()
        || resolved_rules.res_append.is_some()
        || resolved_rules.req_prepend.is_some()
        || resolved_rules.req_append.is_some()
        || !resolved_rules.res_cookies.is_empty()
        || !resolved_rules.req_cookies.is_empty()
        || !resolved_rules.header_replace.is_empty()
        || !resolved_rules.req_scripts.is_empty()
        || !resolved_rules.res_scripts.is_empty()
        || !resolved_rules.res_stream_scripts.is_empty()
        || !resolved_rules.decode_scripts.is_empty()
        || resolved_rules.html_append.is_some()
        || resolved_rules.html_prepend.is_some()
        || resolved_rules.html_body.is_some()
        || resolved_rules.js_append.is_some()
        || resolved_rules.js_prepend.is_some()
        || resolved_rules.js_body.is_some()
        || resolved_rules.css_append.is_some()
        || resolved_rules.css_prepend.is_some()
        || resolved_rules.css_body.is_some()
        || resolved_rules.req_type.is_some()
        || resolved_rules.req_charset.is_some()
        || resolved_rules.res_type.is_some()
        || resolved_rules.res_charset.is_some()
        || !resolved_rules.url_params.is_empty()
        || !resolved_rules.delete_url_params.is_empty()
        || !resolved_rules.url_replace.is_empty()
        || !resolved_rules.url_replace_regex.is_empty()
        || resolved_rules.forwarded_for.is_some()
        || resolved_rules.response_for.is_some()
        || resolved_rules.method.is_some()
        || resolved_rules.auth.is_some()
        || resolved_rules.referer.is_some()
        || resolved_rules.cache.is_some()
        || resolved_rules.attachment.is_some()
        || resolved_rules.req_merge.is_some()
        || resolved_rules.res_merge.is_some();

    has_interceptable_fields
        && resolved_rules.rules.iter().any(|rule| {
            rule.auto_tls_intercept && protocol_requires_tls_interception(rule.protocol)
        })
}

fn protocol_requires_tls_interception(protocol: Protocol) -> bool {
    matches!(
        protocol,
        Protocol::ReqHeaders
            | Protocol::ResHeaders
            | Protocol::ReqBody
            | Protocol::ResBody
            | Protocol::ReqPrepend
            | Protocol::ResPrepend
            | Protocol::ReqAppend
            | Protocol::ResAppend
            | Protocol::ReqCookies
            | Protocol::ResCookies
            | Protocol::ReqCors
            | Protocol::ResCors
            | Protocol::ReqSpeed
            | Protocol::ResSpeed
            | Protocol::ReqType
            | Protocol::ResType
            | Protocol::ReqCharset
            | Protocol::ResCharset
            | Protocol::ReqReplace
            | Protocol::ResReplace
            | Protocol::ForwardedFor
            | Protocol::ResponseFor
            | Protocol::Method
            | Protocol::Auth
            | Protocol::Ua
            | Protocol::Referer
            | Protocol::UrlParams
            | Protocol::Params
            | Protocol::UrlReplace
            | Protocol::ReplaceStatus
            | Protocol::StatusCode
            | Protocol::Cache
            | Protocol::Attachment
            | Protocol::ResMerge
            | Protocol::HeaderReplace
            | Protocol::HtmlAppend
            | Protocol::HtmlPrepend
            | Protocol::HtmlBody
            | Protocol::JsAppend
            | Protocol::JsPrepend
            | Protocol::JsBody
            | Protocol::CssAppend
            | Protocol::CssPrepend
            | Protocol::CssBody
            | Protocol::ReqScript
            | Protocol::ResScript
            | Protocol::ResStreamScript
            | Protocol::Decode
            | Protocol::File
            | Protocol::Tpl
            | Protocol::RawFile
            | Protocol::Delete
    )
}

pub fn requires_tls_interception_for_connect_rules(resolved_rules: &ResolvedRules) -> bool {
    requires_tls_interception_for_rules(resolved_rules)
        || requires_tls_interception_for_host_rewrite(resolved_rules)
}

fn should_use_connect_upstream_proxy(resolved_rules: &ResolvedRules) -> bool {
    resolved_rules.proxy.is_some() && (resolved_rules.ignored.host || resolved_rules.host.is_none())
}

fn has_request_body_rules(rules: &ResolvedRules) -> bool {
    rules.req_body.is_some()
        || rules.req_prepend.is_some()
        || rules.req_append.is_some()
        || !rules.req_replace.is_empty()
        || !rules.req_replace_regex.is_empty()
        || rules.req_merge.is_some()
}

fn has_response_body_rules(rules: &ResolvedRules) -> bool {
    rules.res_body.is_some()
        || rules.res_prepend.is_some()
        || rules.res_append.is_some()
        || !rules.res_replace.is_empty()
        || !rules.res_replace_regex.is_empty()
        || rules.res_merge.is_some()
}

pub(super) fn sanitize_upstream_headers(headers: &mut hyper::HeaderMap) {
    client::sanitize_upstream_headers(headers)
}

pub(super) fn classify_request_error(
    err: &hyper_util::client::legacy::Error,
) -> client::UpstreamRequestErrorInfo {
    client::classify_request_error(err)
}

pub(super) fn is_retryable_http2_error(err: &hyper_util::client::legacy::Error) -> bool {
    client::is_retryable_http2_error(err)
}

pub(super) fn mark_http1_fallback(unsafe_ssl: bool, dns_servers: &[String], pool_partition: &str) {
    client::mark_http1_fallback(unsafe_ssl, dns_servers, pool_partition)
}

pub(super) async fn send_pooled_request(
    request: Request<BoxBody>,
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> std::result::Result<Response<BoxBody>, hyper_util::client::legacy::Error> {
    client::send_pooled_request(request, unsafe_ssl, dns_servers, pool_partition).await
}

pub(super) async fn send_pooled_request_http1_only(
    request: Request<BoxBody>,
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> std::result::Result<Response<BoxBody>, hyper_util::client::legacy::Error> {
    client::send_pooled_request_http1_only(request, unsafe_ssl, dns_servers, pool_partition).await
}

#[derive(Clone)]
struct RetryableRequestBlueprint {
    method: hyper::Method,
    uri: hyper::Uri,
    headers: hyper::HeaderMap<HeaderValue>,
    body: Bytes,
}

impl RetryableRequestBlueprint {
    fn build(&self) -> Result<Request<BoxBody>> {
        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone())
            .version(hyper::Version::HTTP_11);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder.body(full_body(self.body.clone())).map_err(|e| {
            BifrostError::Network(format!(
                "Failed to rebuild request for HTTP/1.1 retry: {}",
                e
            ))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum H2BodyRecoveryAction {
    Probe,
    RetryHttp1,
    Stream,
}

fn h2_body_recovery_action(
    response_version: hyper::Version,
    status: hyper::StatusCode,
    method: &str,
    content_type: &str,
    content_length: Option<usize>,
    max_body_buffer_size: usize,
    retryable: bool,
) -> H2BodyRecoveryAction {
    if !retryable
        || response_version != hyper::Version::HTTP_2
        || status.is_informational()
        || status == hyper::StatusCode::NO_CONTENT
        || status == hyper::StatusCode::NOT_MODIFIED
        || method.eq_ignore_ascii_case("HEAD")
        || content_type
            .to_ascii_lowercase()
            .starts_with("text/event-stream")
    {
        return H2BodyRecoveryAction::Stream;
    }

    if let Some(len) = content_length {
        if len <= max_body_buffer_size {
            H2BodyRecoveryAction::Probe
        } else if !is_likely_text_content_type(content_type) {
            H2BodyRecoveryAction::RetryHttp1
        } else {
            H2BodyRecoveryAction::Stream
        }
    } else if is_likely_text_content_type(content_type) {
        H2BodyRecoveryAction::Probe
    } else {
        H2BodyRecoveryAction::RetryHttp1
    }
}

fn build_upstream_pool_partition(
    original_host: &str,
    target_host: &str,
    target_port: u16,
    use_http: bool,
    rules: &ResolvedRules,
) -> String {
    format!(
        "orig={original_host}|target={}://{}:{}|host={:?}|proxy={:?}|proto={:?}|ignored_host={}|upstream_unsafe_ssl={}",
        if use_http { "http" } else { "https" },
        target_host,
        target_port,
        rules.host,
        rules.proxy,
        rules.host_protocol,
        rules.ignored.host,
        rules.upstream_unsafe_ssl
    )
}

fn merge_connect_resolved_rules(
    mut base: ResolvedRules,
    tunnel_specific: ResolvedRules,
) -> ResolvedRules {
    if tunnel_specific.host.is_some() && !base.ignored.host {
        base.host = tunnel_specific.host;
        base.host_protocol = tunnel_specific.host_protocol;
    }

    if tunnel_specific.tls_intercept.is_some() {
        base.tls_intercept = tunnel_specific.tls_intercept;
    }
    if tunnel_specific.tls_options.is_some() {
        base.tls_options = tunnel_specific.tls_options;
    }
    if tunnel_specific.sni_callback.is_some() {
        base.sni_callback = tunnel_specific.sni_callback;
    }
    base.upstream_unsafe_ssl = base.upstream_unsafe_ssl || tunnel_specific.upstream_unsafe_ssl;

    if !tunnel_specific.rules.is_empty() {
        base.rules.extend(tunnel_specific.rules);
    }

    base
}

fn parse_sni_callback_spec(value: &str) -> (&str, Option<&str>) {
    if let Some((plugin, raw_arg)) = value.split_once('(') {
        let plugin = plugin.trim();
        let arg = raw_arg.trim_end_matches(')').trim();
        return (plugin, (!arg.is_empty()).then_some(arg));
    }

    (value.trim(), None)
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

fn build_tls_intercept_server_builder(
    http2_max_header_list_size: usize,
) -> AutoServerBuilder<TokioExecutor> {
    let http2_max_header_list_size = u32::try_from(http2_max_header_list_size).unwrap_or(u32::MAX);
    let mut builder = AutoServerBuilder::new(TokioExecutor::new())
        .preserve_header_case(true)
        .title_case_headers(true);
    builder
        .http2()
        .adaptive_window(true)
        .enable_connect_protocol()
        // Browser-originated HTTP/2 requests can carry large cookie/header sets
        // (for example chatgpt.com session cookies). Hyper's default 16KB limit
        // is too small and surfaces as a proxy-generated 431 before our handler runs.
        .max_header_list_size(http2_max_header_list_size)
        .max_concurrent_streams(512)
        .keep_alive_interval(Some(std::time::Duration::from_secs(15)))
        .keep_alive_timeout(std::time::Duration::from_secs(20))
        .timer(TokioTimer::new());
    builder
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_connect(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    rules: Arc<dyn RulesResolver>,
    tls_config: Arc<TlsConfig>,
    tls_intercept_config: &TlsInterceptConfig,
    proxy_config: &ProxyConfig,
    verbose_logging: bool,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
    push_manager: Option<SharedPushManager>,
) -> Result<Response<BoxBody>> {
    handle_connect_with_process_state(
        req,
        peer_addr,
        local_addr,
        rules,
        tls_config,
        tls_intercept_config,
        proxy_config,
        verbose_logging,
        ctx,
        admin_state,
        dns_resolver,
        push_manager,
        Arc::new(ConnectionProcessState::default()),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_connect_with_process_state(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    rules: Arc<dyn RulesResolver>,
    tls_config: Arc<TlsConfig>,
    tls_intercept_config: &TlsInterceptConfig,
    proxy_config: &ProxyConfig,
    verbose_logging: bool,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
    push_manager: Option<SharedPushManager>,
    connection_process_state: Arc<ConnectionProcessState>,
) -> Result<Response<BoxBody>> {
    let uri = req.uri().clone();
    let authority = uri
        .authority()
        .ok_or_else(|| BifrostError::Network("CONNECT request missing authority".to_string()))?;

    let host = authority.host().to_string();
    let port = authority.port_u16().unwrap_or(443);

    if verbose_logging {
        debug!(
            "[{}] CONNECT tunnel request to {}:{}",
            ctx.id_str(),
            host,
            port
        );
    } else {
        debug!("CONNECT tunnel to {}:{}", host, port);
    }

    let authority = tls_authority(&host, port, true);
    let url = format!("https://{authority}");
    let tunnel_url = format!("tunnel://{authority}");
    let mut resolved_rules = rules.resolve(&url, "CONNECT");
    let tunnel_rules = rules.resolve(&tunnel_url, "CONNECT");
    if tunnel_rules.host.is_some()
        || tunnel_rules.tls_intercept.is_some()
        || tunnel_rules.tls_options.is_some()
        || tunnel_rules.upstream_unsafe_ssl
        || tunnel_rules.sni_callback.is_some()
        || !tunnel_rules.rules.is_empty()
    {
        resolved_rules = merge_connect_resolved_rules(resolved_rules, tunnel_rules);
    }

    if let Some(ref tls_options) = resolved_rules.tls_options {
        info!(
            "[{}] CONNECT TLS options matched for {}:{} => {}",
            ctx.id_str(),
            host,
            port,
            tls_options
        );
    }
    if let Some(ref sni_callback) = resolved_rules.sni_callback {
        let (plugin, sni_value) = parse_sni_callback_spec(sni_callback);
        info!(
            "[{}] CONNECT SNI callback matched for {}:{} => plugin={}, sniValue={}",
            ctx.id_str(),
            host,
            port,
            plugin,
            sni_value.unwrap_or("<none>")
        );
    }
    let is_local_client = ctx
        .client_ip
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback());
    let requires_client_app =
        is_local_client && requires_client_app_for_tls_decision(tls_intercept_config);

    if requires_client_app && !matches!(ctx.client_app.as_deref(), Some(app) if !app.is_empty()) {
        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .increment_client_process_resolution_failure();
            state
                .metrics_collector
                .increment_client_process_policy_unknown_decision();
        }
        debug!(
            req_id = ctx.id_str(),
            host,
            port,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "CONNECT app-based TLS decision fell back because client process is unknown"
        );
    }

    let client_ip_str = peer_addr.ip().to_string();
    let mut intercept = should_intercept_tls_for_client(
        &host,
        ctx.client_app.as_deref(),
        is_local_client,
        Some(&client_ip_str),
        tls_intercept_config,
        &tls_config,
        &resolved_rules,
    );
    let force_trust_probe_passthrough = bifrost_admin::is_active_trust_probe_target(&host, port);
    if force_trust_probe_passthrough {
        intercept = false;
        debug!(
            "[{}] TLS interception skipped for active trust probe target {}:{}",
            ctx.id_str(),
            host,
            port
        );
    }

    if !is_local_client {
        if let Some(ref state) = admin_state {
            if let Some(ref ip_tls_mgr) = state.ip_tls_pending_manager {
                let peer_ip = peer_addr.ip();
                if !is_ip_included(&client_ip_str, &tls_intercept_config.ip_intercept_include)
                    && !is_ip_excluded(&client_ip_str, &tls_intercept_config.ip_intercept_exclude)
                    && !ip_tls_mgr.is_pending_or_decided(&peer_ip)
                {
                    ip_tls_mgr.check_and_add_pending(peer_ip);
                }
            }
        }
    }

    let state = &admin_state;
    let resolved = &resolved_rules;
    let scoped_rules = Some(rules.as_ref());
    let bp = bp_tls(state, resolved, scoped_rules, &host, port);
    debug!(
        req_id = ctx.id_str(),
        host,
        port,
        breakpoint_tls_required = bp,
        breakpoint_enabled = state
            .as_ref()
            .is_some_and(|state| state.breakpoint_manager.is_enabled()),
        ca_available = tls_config.ca_cert.is_some(),
        explicit_tls_passthrough = matches!(resolved_rules.tls_intercept, Some(false)),
        force_trust_probe_passthrough,
        "CONNECT scoped TLS interception decision"
    );
    if !intercept
        && !force_trust_probe_passthrough
        && is_local_client
        && host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST)
        && tls_config.ca_cert.is_some()
    {
        intercept = true;
        debug!(
            "[{}] Forced TLS interception for admin virtual host {}",
            ctx.id_str(),
            host
        );
    }

    if !intercept
        && !force_trust_probe_passthrough
        && tls_config.ca_cert.is_some()
        && !matches!(resolved_rules.tls_intercept, Some(false))
        && bp
    {
        intercept = true;
    }

    if !intercept
        && !force_trust_probe_passthrough
        && tls_config.ca_cert.is_some()
        && !matches!(resolved_rules.tls_intercept, Some(false))
        && (requires_tls_interception_for_connect_rules(&resolved_rules)
            || rules.has_response_rules_for_host(&host)
            || rules.has_tls_auto_intercept_route_rules_for_host(&host))
    {
        intercept = true;
    }

    if intercept
        && !is_standard_tls_intercept_port(port)
        && !is_explicit_tls_intercept_override(
            &host,
            ctx.client_app.as_deref(),
            tls_intercept_config,
            &resolved_rules,
        )
    {
        intercept = false;
        if verbose_logging {
            debug!(
                "[{}] TLS interception skipped for {}:{} (non-standard TLS port without explicit override)",
                ctx.id_str(),
                host,
                port
            );
        }
    }

    if intercept {
        if verbose_logging {
            let reason = if resolved_rules.tls_intercept.is_some() {
                "rule override"
            } else if is_app_included(
                ctx.client_app.as_deref(),
                &tls_intercept_config.app_intercept_include,
            ) {
                "app in include list (force intercept)"
            } else if is_domain_included(&host, &tls_intercept_config.intercept_include) {
                "in include list (force intercept)"
            } else {
                "global interception enabled (not excluded)"
            };
            debug!(
                "[{}] TLS interception enabled for {} ({})",
                ctx.id_str(),
                host,
                reason
            );
        }
        let max_body_buffer_size = admin_state
            .as_ref()
            .map(|s| s.get_max_body_buffer_size())
            .unwrap_or(proxy_config.max_body_buffer_size);
        let max_body_probe_size = admin_state
            .as_ref()
            .map(|s| s.get_max_body_probe_size())
            .unwrap_or(proxy_config.max_body_probe_size);
        let inject_bifrost_badge = admin_state
            .as_ref()
            .and_then(|s| s.config_manager.as_ref())
            .and_then(|cm| cm.try_config())
            .map(|config| config.traffic.inject_bifrost_badge)
            .unwrap_or(true);
        return handle_tls_interception(
            req,
            &host,
            port,
            rules,
            tls_config,
            verbose_logging,
            max_body_buffer_size,
            max_body_probe_size,
            tls_intercept_config.unsafe_ssl,
            inject_bifrost_badge,
            ctx,
            admin_state,
            push_manager,
        )
        .await;
    } else if tls_config.ca_cert.is_some() && verbose_logging {
        let reason = if let Some(false) = resolved_rules.tls_intercept {
            "rule override (passthrough)"
        } else if is_app_excluded(
            ctx.client_app.as_deref(),
            &tls_intercept_config.app_intercept_exclude,
        ) {
            "app in exclude list"
        } else if is_domain_excluded(&host, &tls_intercept_config.intercept_exclude) {
            "in exclude list"
        } else {
            "global interception disabled"
        };
        debug!(
            "[{}] TLS interception skipped for {} ({})",
            ctx.id_str(),
            host,
            reason
        );
    }

    let has_rules = resolved_rules.host.is_some()
        || resolved_rules.proxy.is_some()
        || !resolved_rules.rules.is_empty();
    if verbose_logging && has_rules {
        info!(
            "[{}] CONNECT tunnel rules matched: {}",
            ctx.id_str(),
            format_rules_summary(&resolved_rules)
        );
    }

    let (target_host, target_port) = if host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
        debug!(
            "[{}] CONNECT admin virtual host routed to local admin listener",
            ctx.id_str()
        );
        ("127.0.0.1".to_string(), proxy_config.port)
    } else if let Some(ref host_rule) = resolved_rules.host {
        let (h, parsed_port) = match parse_host_rule(host_rule) {
            Some((h, p, _path)) => (h, p),
            None => (host_rule.trim_end_matches('/').to_string(), None),
        };

        let p = parsed_port.unwrap_or(match resolved_rules.host_protocol {
            Some(Protocol::Http) | Some(Protocol::Ws) => 80,
            Some(Protocol::Https) | Some(Protocol::Wss) | Some(Protocol::Tunnel) => 443,
            _ => port,
        });
        debug!(
            "[{}] CONNECT tunnel target redirected: {}:{} -> {}:{} (protocol={:?})",
            ctx.id_str(),
            host,
            port,
            h,
            p,
            resolved_rules.host_protocol
        );
        (h, p)
    } else {
        (host.clone(), port)
    };

    let upstream_proxy_rule = if should_use_connect_upstream_proxy(&resolved_rules) {
        resolved_rules.proxy.clone()
    } else {
        None
    };

    let connect_host = if upstream_proxy_rule.is_some() {
        target_host.clone()
    } else if !resolved_rules.dns_servers.is_empty() {
        if let Some(ref resolver) = dns_resolver {
            if verbose_logging {
                info!(
                    "[{}] [DNS] resolving {} with custom servers: {:?}",
                    ctx.id_str(),
                    target_host,
                    resolved_rules.dns_servers
                );
            }
            match resolver
                .resolve(&target_host, &resolved_rules.dns_servers)
                .await
            {
                Ok(Some(ip)) => {
                    if verbose_logging {
                        info!(
                            "[{}] [DNS] resolved {} -> {}",
                            ctx.id_str(),
                            target_host,
                            ip
                        );
                    }
                    ip.to_string()
                }
                Ok(None) | Err(_) => target_host.clone(),
            }
        } else {
            target_host.clone()
        }
    } else {
        target_host.clone()
    };

    let target_stream = if let Some(ref proxy_rule) = upstream_proxy_rule {
        connect_via_upstream_http_proxy_tunnel(proxy_rule, &target_host, target_port).await?
    } else {
        connect_tcp(format!("{}:{}", connect_host, target_port))
            .await
            .map_err(|e| {
                BifrostError::Network(format!(
                    "Failed to connect to {}:{}: {}",
                    connect_host, target_port, e
                ))
            })?
    };

    if let Err(e) = target_stream.set_nodelay(true) {
        warn!(
            "[{}] Failed to set TCP_NODELAY on tunnel connection: {}",
            ctx.id_str(),
            e
        );
    }

    if verbose_logging {
        info!(
            "[{}] CONNECT tunnel established to {}:{}{}",
            ctx.id_str(),
            target_host,
            target_port,
            upstream_proxy_rule
                .as_ref()
                .map(|proxy| format!(" via upstream proxy {proxy}"))
                .unwrap_or_default()
        );
    }

    let req_id = ctx.id_str().to_string();
    let verbose = verbose_logging;
    let client_ip = ctx.client_ip.clone();
    let client_app = ctx.client_app.clone();
    let client_pid = ctx.client_pid;
    let client_path = ctx.client_path.clone();
    let account_name = ctx.account_name.clone();
    let listener_port = ctx.port;

    // cancel_rx 用于在配置变更时优雅关闭 tunnel。
    // 注意：若 admin_state 为空，必须保留 cancel_tx 的生命周期，否则 Sender 被提前 drop 会导致
    // cancel_rx 立即完成，从而把连接误判为“配置变更”并立刻关闭。
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let mut cancel_tx_keepalive = Some(cancel_tx);

    if let Some(ref state) = admin_state {
        state
            .metrics_collector
            .increment_connections_by_type(TrafficType::Tunnel);
        state
            .metrics_collector
            .increment_requests_by_type(TrafficType::Tunnel);

        let conn_info = ConnectionInfo::new(
            req_id.to_string(),
            host.clone(),
            port,
            false,
            client_app.clone(),
            cancel_tx_keepalive
                .take()
                .expect("cancel_tx should be available when registering connection"),
        );
        state.connection_registry.register(conn_info);

        let mut record = TrafficRecord::new(
            req_id.to_string(),
            "CONNECT".to_string(),
            format!("tunnel://{}:{}", host, port),
        );
        record.status = 200;
        record.protocol = "tunnel".to_string();
        record.host = host.clone();
        record.is_tunnel = true;
        apply_listener_context(
            &mut record,
            listener_port,
            &client_ip,
            &client_app,
            client_pid,
            &client_path,
            &account_name,
        );
        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
        state.record_traffic(record);
        maybe_backfill_tunnel_client_process(
            state,
            &connection_process_state,
            &req_id,
            client_app.is_some() && client_pid.is_some(),
            peer_addr,
            local_addr,
            requires_client_app,
        );

        state.connection_monitor.register_tunnel_connection(&req_id);
    }

    let host_for_unregister = host.clone();
    tokio::spawn(async move {
        // keep cancel sender alive when admin_state is None
        // （避免编译器因为未使用而提前 drop，导致 cancel_rx 立刻完成）
        let cancel_tx_keepalive = cancel_tx_keepalive;

        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let result = tunnel_bidirectional_with_cancel(
                    upgraded,
                    target_stream,
                    verbose,
                    &req_id,
                    admin_state.as_ref(),
                    cancel_rx,
                )
                .await;
                if let Some(ref state) = admin_state {
                    finalize_tunnel_tracking(state, &req_id);
                }
                match result {
                    Ok(stats) if stats.cancelled => {
                        info!(
                            "[{}] Tunnel {}:{} closed due to config change",
                            req_id, host_for_unregister, port
                        );
                    }
                    Err(e) => {
                        error!(
                            "[{}] Tunnel error to {}:{} client_ip={} client_app={:?} client_pid={:?} client_path={:?} error={}",
                            req_id,
                            host_for_unregister,
                            port,
                            client_ip,
                            client_app,
                            client_pid,
                            client_path,
                            e
                        );
                    }
                    _ => {}
                }
            }
            Err(e) => {
                if let Some(ref state) = admin_state {
                    finalize_tunnel_tracking(state, &req_id);
                }
                error!(
                    "[{}] Upgrade error for {}:{} client_ip={} client_app={:?} client_pid={:?} client_path={:?} error={}",
                    req_id,
                    host_for_unregister,
                    port,
                    client_ip,
                    client_app,
                    client_pid,
                    client_path,
                    e
                );
            }
        }

        // 确保 keepalive 不会被编译器过早 drop（会导致 cancel_rx 立刻完成）。
        std::hint::black_box(&cancel_tx_keepalive);
        drop(cancel_tx_keepalive);
    });

    Ok(Response::builder().status(200).body(empty_body()).unwrap())
}

#[allow(clippy::too_many_arguments)]
async fn handle_tls_interception(
    req: Request<Incoming>,
    original_host: &str,
    original_port: u16,
    rules: Arc<dyn RulesResolver>,
    tls_config: Arc<TlsConfig>,
    verbose_logging: bool,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    unsafe_ssl: bool,
    inject_bifrost_badge: bool,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    push_manager: Option<SharedPushManager>,
) -> Result<Response<BoxBody>> {
    ensure_crypto_provider();
    let alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let server_config = tls_config.resolve_server_config(original_host, &alpn_protocols)?;

    let req_id = ctx.id_str().to_string();
    let verbose = verbose_logging;
    let original_host_owned = original_host.to_string();
    let client_ip = ctx.client_ip.clone();
    let client_app = ctx.client_app.clone();
    let client_pid = ctx.client_pid;
    let client_path = ctx.client_path.clone();
    let account_name = ctx.account_name.clone();
    let listener_port = ctx.port;

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let mut cancel_tx_keepalive = Some(cancel_tx);

    if let Some(ref state) = admin_state {
        state
            .metrics_collector
            .increment_connections_by_type(TrafficType::Https);

        let conn_info = ConnectionInfo::new(
            req_id.to_string(),
            original_host_owned.clone(),
            original_port,
            true,
            client_app.clone(),
            cancel_tx_keepalive
                .take()
                .expect("cancel_tx should be available when registering TLS intercept connection"),
        );
        state.connection_registry.register(conn_info);
    }

    let host_for_log = original_host_owned.clone();
    tokio::spawn(async move {
        let cancel_tx_keepalive = cancel_tx_keepalive;
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(u) => u,
            Err(e) => {
                if let Some(ref state) = admin_state {
                    state
                        .metrics_collector
                        .decrement_connections_by_type(TrafficType::Https);
                    state.connection_registry.unregister(&req_id);
                }
                error!("[{}] TLS interception upgrade error: {}", req_id, e);
                return;
            }
        };

        let result = tls_intercept_tunnel_with_cancel(
            upgraded,
            server_config,
            &original_host_owned,
            original_port,
            rules,
            verbose,
            max_body_buffer_size,
            max_body_probe_size,
            unsafe_ssl,
            inject_bifrost_badge,
            &req_id,
            admin_state.clone(),
            cancel_rx,
            client_ip,
            client_app,
            client_pid,
            client_path,
            account_name,
            listener_port,
            push_manager,
        )
        .await;

        std::hint::black_box(&cancel_tx_keepalive);
        drop(cancel_tx_keepalive);

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .decrement_connections_by_type(TrafficType::Https);
            state.connection_registry.unregister(&req_id);
        }

        match result {
            Ok(cancelled) if cancelled => {
                info!(
                    "[{}] TLS intercept tunnel {}:{} closed due to config change",
                    req_id, host_for_log, original_port
                );
            }
            Err(e) => {
                if verbose {
                    warn!("[{}] TLS interception error: {}", req_id, e);
                } else {
                    debug!("TLS interception error: {}", e);
                }
            }
            _ => {}
        }
    });

    Ok(Response::builder().status(200).body(empty_body()).unwrap())
}

#[allow(clippy::too_many_arguments)]
async fn tls_intercept_tunnel_with_cancel(
    upgraded: Upgraded,
    server_config: Arc<ServerConfig>,
    original_host: &str,
    original_port: u16,
    rules: Arc<dyn RulesResolver>,
    verbose_logging: bool,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    unsafe_ssl: bool,
    inject_bifrost_badge: bool,
    req_id: &str,
    admin_state: Option<Arc<AdminState>>,
    cancel_rx: oneshot::Receiver<()>,
    client_ip: String,
    client_app: Option<String>,
    client_pid: Option<u32>,
    client_path: Option<String>,
    account_name: Option<String>,
    listener_port: u16,
    push_manager: Option<SharedPushManager>,
) -> Result<bool> {
    let acceptor = TlsAcceptor::from(server_config);
    let mut client_tls = match acceptor.accept(TokioIo::new(upgraded)).await {
        Ok(tls) => {
            if let Some(ref state) = admin_state {
                if let Some(ref tracker) = state.client_trust_tracker {
                    tracker.record_handshake_success(
                        &client_ip,
                        client_app.as_deref(),
                        original_host,
                    );
                }
            }
            tls
        }
        Err(e) => {
            if let Some(ref state) = admin_state {
                if let Some(ref tracker) = state.client_trust_tracker {
                    let reason = bifrost_admin::classify_tls_accept_error(&e);
                    tracker.record_handshake_failure(
                        &client_ip,
                        client_app.as_deref(),
                        original_host,
                        &reason,
                    );
                }
            }
            return Err(BifrostError::Tls(format!("TLS accept failed: {e}")));
        }
    };
    #[rustfmt::skip] let (client_alpn, should_sniff_payload) = { let alpn = client_tls.get_ref().1.alpn_protocol().map(|p| p.to_vec()); let sniff = should_sniff_tls_payload(alpn.as_deref(), original_port); (alpn, sniff) }; // HTTP/1.1 may omit ALPN, notably Schannel for IP targets.

    if verbose_logging {
        debug!(
            "[{}] TLS handshake with client completed (alpn={})",
            req_id,
            format_tls_alpn(client_alpn.as_deref())
        );
    }

    let initial_payload = if should_sniff_payload {
        sniff_tls_client_payload(&mut client_tls, req_id, verbose_logging).await?
    } else {
        BytesMut::new()
    };

    if !should_serve_intercepted_http(
        client_alpn.as_deref(),
        should_sniff_payload,
        &initial_payload,
    ) {
        return tunnel_intercepted_non_http_tls_with_cancel(
            client_tls,
            initial_payload,
            RawTlsTunnelContext {
                original_host: original_host.to_string(),
                original_port,
                unsafe_ssl,
                verbose_logging,
                req_id: req_id.to_string(),
                admin_state,
                cancel_rx,
            },
        )
        .await;
    }

    let original_host_for_requests = original_host.to_string();
    let original_port_for_requests = original_port;
    let admin_state_clone = admin_state.clone();
    let rules_clone = rules.clone();
    let verbose = verbose_logging;
    let client_ip_clone = client_ip.clone();
    let client_app_clone = client_app.clone();
    let client_path_clone2 = client_path.clone();
    let account_name_clone = account_name.clone();
    let push_manager_clone = push_manager.clone();

    let service = service_fn(move |req: Request<Incoming>| {
        let original_host = original_host_for_requests.clone();
        let original_port = original_port_for_requests;
        let req_id = crate::utils::logging::generate_request_id();
        let admin_state = admin_state_clone.clone();
        let rules = rules_clone.clone();
        let client_ip = client_ip_clone.clone();
        let client_app = client_app_clone.clone();
        let client_pid = client_pid;
        let client_path = client_path_clone2.clone();
        let account_name = account_name_clone.clone();
        let push_manager = push_manager_clone.clone();
        async move {
            handle_intercepted_request_with_protocol(
                req,
                &original_host,
                original_port,
                &req_id,
                admin_state,
                rules,
                verbose,
                max_body_buffer_size,
                max_body_probe_size,
                unsafe_ssl,
                client_ip,
                client_app,
                client_pid,
                client_path,
                account_name,
                listener_port,
                push_manager,
                inject_bifrost_badge,
            )
            .await
        }
    });

    let client_tls = BufferedIo::new(client_tls, initial_payload);
    let (client_read, client_write) = tokio::io::split(client_tls);
    let client_io = TokioIo::new(CombinedAsyncRw::new(client_read, client_write));

    let http2_max_header_list_size = if let Some(ref state) = admin_state {
        if let Some(ref config_manager) = state.config_manager {
            config_manager
                .config()
                .await
                .server
                .http2_max_header_list_size
        } else {
            256 * 1024
        }
    } else {
        256 * 1024
    };
    let builder = build_tls_intercept_server_builder(http2_max_header_list_size);
    let conn = builder.serve_connection_with_upgrades(client_io, service);

    tokio::pin!(conn);

    tokio::select! {
        result = conn.as_mut() => {
            if let Err(e) = result {
                if verbose_logging {
                    debug!("[{}] HTTP connection ended: {}", req_id, e);
                }
            }
            Ok(false)
        }
        _ = cancel_rx => {
            if verbose_logging {
                debug!("[{}] TLS intercept tunnel cancelled by config change, initiating graceful shutdown", req_id);
            }
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
            Ok(true)
        }
    }
}

fn is_http_alpn(alpn: Option<&[u8]>) -> bool {
    matches!(alpn, Some(b"h2") | Some(b"http/1.1"))
}

fn should_serve_intercepted_http(
    alpn: Option<&[u8]>,
    payload_was_sniffed: bool,
    initial_payload: &BytesMut,
) -> bool {
    match alpn {
        Some(protocol) if is_http_alpn(Some(protocol)) => {
            !payload_was_sniffed || looks_like_http_payload(initial_payload)
        }
        None => payload_was_sniffed && looks_like_http_payload(initial_payload),
        Some(_) => false,
    }
}

fn format_tls_alpn(alpn: Option<&[u8]>) -> String {
    match alpn {
        Some(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        None => "none".to_string(),
    }
}

struct RawTlsTunnelContext {
    original_host: String,
    original_port: u16,
    unsafe_ssl: bool,
    verbose_logging: bool,
    req_id: String,
    admin_state: Option<Arc<AdminState>>,
    cancel_rx: oneshot::Receiver<()>,
}

async fn tunnel_intercepted_non_http_tls_with_cancel<C>(
    client_tls: C,
    initial_payload: BytesMut,
    ctx: RawTlsTunnelContext,
) -> Result<bool>
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let RawTlsTunnelContext {
        original_host,
        original_port,
        unsafe_ssl,
        verbose_logging,
        req_id,
        admin_state,
        cancel_rx,
    } = ctx;

    if verbose_logging {
        info!(
            "[{}] Intercepted TLS payload is not HTTP; forwarding as raw TLS stream to {}:{}",
            req_id, original_host, original_port
        );
    }

    let target_stream = connect_tcp(format!("{}:{}", original_host, original_port))
        .await
        .map_err(|e| {
            BifrostError::Network(format!(
                "Failed to connect raw TLS upstream {}:{}: {}",
                original_host, original_port, e
            ))
        })?;
    if let Err(err) = target_stream.set_nodelay(true) {
        debug!(
            "[{}] Failed to set TCP_NODELAY on raw TLS upstream {}:{}: {}",
            req_id, original_host, original_port, err
        );
    }

    let server_name = ServerName::try_from(original_host.clone())
        .map_err(|e| BifrostError::Tls(format!("Invalid server name {original_host}: {e}")))?;
    let connector = TlsConnector::from(get_tls_client_config_without_alpn(unsafe_ssl));
    let upstream_tls = connector
        .connect(server_name, target_stream)
        .await
        .map_err(|e| {
            BifrostError::Tls(format!(
                "Failed to establish raw TLS upstream {}:{}: {}",
                original_host, original_port, e
            ))
        })?;

    relay_raw_tls_streams_with_cancel(
        client_tls,
        upstream_tls,
        initial_payload,
        verbose_logging,
        req_id,
        admin_state,
        cancel_rx,
    )
    .await
}

async fn relay_raw_tls_streams_with_cancel<C, U>(
    client_tls: C,
    upstream_tls: U,
    initial_payload: BytesMut,
    verbose_logging: bool,
    req_id: String,
    admin_state: Option<Arc<AdminState>>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<bool>
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut client_read, mut client_write) = tokio::io::split(client_tls);
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream_tls);

    let admin_state_send = admin_state.clone();
    let admin_state_recv = admin_state.clone();
    let req_id_send = req_id.to_string();
    let req_id_recv = req_id.to_string();

    let client_to_upstream = async move {
        let mut buf = [0u8; 16 * 1024];
        if !initial_payload.is_empty() {
            upstream_write.write_all(&initial_payload).await?;
            if let Some(ref state) = admin_state_send {
                state
                    .metrics_collector
                    .add_bytes_sent_by_type(TrafficType::Https, initial_payload.len() as u64);
                state.connection_monitor.update_traffic(
                    &req_id_send,
                    bifrost_admin::FrameDirection::Send,
                    initial_payload.len() as u64,
                );
            }
        }
        loop {
            let n = client_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            upstream_write.write_all(&buf[..n]).await?;
            if let Some(ref state) = admin_state_send {
                state
                    .metrics_collector
                    .add_bytes_sent_by_type(TrafficType::Https, n as u64);
                state.connection_monitor.update_traffic(
                    &req_id_send,
                    bifrost_admin::FrameDirection::Send,
                    n as u64,
                );
            }
        }
        upstream_write.shutdown().await?;
        Ok::<(), std::io::Error>(())
    };

    let upstream_to_client = async move {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = upstream_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            client_write.write_all(&buf[..n]).await?;
            if let Some(ref state) = admin_state_recv {
                state
                    .metrics_collector
                    .add_bytes_received_by_type(TrafficType::Https, n as u64);
                state.connection_monitor.update_traffic(
                    &req_id_recv,
                    bifrost_admin::FrameDirection::Receive,
                    n as u64,
                );
            }
        }
        client_write.shutdown().await?;
        Ok::<(), std::io::Error>(())
    };

    tokio::pin!(client_to_upstream);
    tokio::pin!(upstream_to_client);

    tokio::select! {
        result = &mut client_to_upstream => {
            match result {
                Ok(()) => Ok(false),
                Err(err) if matches!(err.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof) => Ok(false),
                Err(err) => Err(BifrostError::Network(format!("Raw TLS client->upstream forwarding error: {err}"))),
            }
        }
        result = &mut upstream_to_client => {
            match result {
                Ok(()) => Ok(false),
                Err(err) if matches!(err.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof) => Ok(false),
                Err(err) => Err(BifrostError::Network(format!("Raw TLS upstream->client forwarding error: {err}"))),
            }
        }
        _ = cancel_rx => {
            if verbose_logging {
                debug!("[{}] Raw TLS intercept tunnel cancelled by config change", req_id);
            }
            Ok(true)
        }
    }
}

async fn sniff_tls_client_payload<T>(
    client_tls: &mut T,
    req_id: &str,
    verbose_logging: bool,
) -> Result<BytesMut>
where
    T: tokio::io::AsyncRead + Unpin,
{
    let mut sniff_buf = [0u8; 24];
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client_tls.read(&mut sniff_buf),
    )
    .await
    {
        Ok(Ok(n)) if n > 0 => Ok(BytesMut::from(&sniff_buf[..n])),
        Ok(Ok(_)) => Ok(BytesMut::new()),
        Ok(Err(err)) => Err(BifrostError::Network(format!(
            "Failed to sniff intercepted TLS payload: {err}"
        ))),
        Err(_) => {
            if verbose_logging {
                debug!(
                    "[{}] Timed out while sniffing intercepted TLS payload; treating as non-HTTP on non-standard port",
                    req_id
                );
            }
            Ok(BytesMut::new())
        }
    }
}

fn looks_like_http_payload(payload: &BytesMut) -> bool {
    if payload.is_empty() {
        return false;
    }

    matches!(
        ProtocolDetector::detect_protocol_type(payload.as_ref()),
        Some(
            TransportProtocol::Http1
                | TransportProtocol::Http2
                | TransportProtocol::WebSocket
                | TransportProtocol::Sse
                | TransportProtocol::Grpc
        )
    )
}

fn rewrite_intercepted_virtual_host_request(req: Request<Incoming>) -> Request<Incoming> {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path();
    if !path.starts_with(ADMIN_PATH_PREFIX) {
        let new_path = if path == "/" {
            format!("{}/", ADMIN_PATH_PREFIX)
        } else {
            format!("{}{}", ADMIN_PATH_PREFIX, path)
        };
        let new_uri = if let Some(query) = parts.uri.query() {
            format!("{}?{}", new_path, query)
        } else {
            new_path
        };
        if let Ok(uri) = new_uri.parse() {
            parts.uri = uri;
        }
    }
    Request::from_parts(parts, body)
}

fn convert_intercepted_admin_response(resp: Response<BoxBody>) -> Response<BoxBody> {
    resp
}

fn is_websocket_upgrade_request(req: &Request<Incoming>) -> bool {
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

fn is_likely_text_content_type(content_type: &str) -> bool {
    let ct = content_type.trim();
    if ct.is_empty() {
        return false;
    }
    if ct.starts_with("text/") {
        return true;
    }
    if ct.starts_with("application/json") {
        return true;
    }
    if ct.contains("+json") {
        return true;
    }
    if ct.starts_with("application/xml") || ct.contains("+xml") {
        return true;
    }
    if ct.starts_with("application/javascript")
        || ct.starts_with("application/x-javascript")
        || ct.starts_with("application/ecmascript")
    {
        return true;
    }
    if ct.starts_with("application/x-www-form-urlencoded") {
        return true;
    }
    false
}

fn is_likely_binary_content_type(content_type: &str) -> bool {
    let ct = content_type.trim();
    if ct.is_empty() || is_likely_text_content_type(ct) {
        return false;
    }

    ct.starts_with("application/octet-stream")
        || ct.starts_with("application/pdf")
        || ct.starts_with("application/zip")
        || ct.starts_with("application/gzip")
        || ct.starts_with("application/x-gzip")
        || ct.starts_with("application/x-tar")
        || ct.starts_with("application/x-rar")
        || ct.starts_with("application/x-7z")
        || ct.starts_with("application/vnd.rar")
        || ct.starts_with("application/vnd.ms-cab-compressed")
        || ct.starts_with("application/x-bittorrent")
        || ct.starts_with("application/wasm")
        || ct.starts_with("application/font-")
        || ct.starts_with("application/vnd.ms-fontobject")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct.starts_with("font/")
        || ct.contains("protobuf")
        || ct.contains("grpc")
}

fn should_use_binary_performance_mode(
    res_parts: &hyper::http::response::Parts,
    binary_traffic_performance_mode: bool,
) -> bool {
    if !binary_traffic_performance_mode {
        return false;
    }

    let content_type_lower = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if content_type_lower.starts_with("image/") {
        return false;
    }
    let has_attachment = res_parts
        .headers
        .get(hyper::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("attachment"))
        .unwrap_or(false);
    if !has_attachment && !is_likely_binary_content_type(&content_type_lower) {
        return false;
    }

    has_attachment || is_likely_binary_content_type(&content_type_lower)
}

fn breakpoint_upstream_target(uri: &hyper::Uri) -> Option<(bool, String, u16, String, String)> {
    let use_http = uri.scheme_str() == Some("http");
    let host = uri.host()?.to_string();
    let default_port = if use_http { 80 } else { 443 };
    let port = uri.port_u16().unwrap_or(default_port);
    let path = uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let authority = uri.authority()?.as_str().to_string();
    Some((use_http, host, port, path, authority))
}

#[allow(clippy::too_many_arguments)]
async fn handle_intercepted_request_with_protocol(
    mut req: Request<Incoming>,
    original_host: &str,
    original_port: u16,
    req_id: &str,
    admin_state: Option<Arc<AdminState>>,
    rules: Arc<dyn RulesResolver>,
    verbose_logging: bool,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    unsafe_ssl: bool,
    client_ip: String,
    client_app: Option<String>,
    client_pid: Option<u32>,
    client_path: Option<String>,
    account_name: Option<String>,
    listener_port: u16,
    push_manager: Option<SharedPushManager>,
    inject_bifrost_badge_default: bool,
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    // Re-read inject_bifrost_badge from persisted config on every request,
    // so that toggling the setting in Web UI takes effect immediately
    // even for long-lived CONNECT tunnels.
    let inject_bifrost_badge = admin_state
        .as_ref()
        .and_then(|s| s.config_manager.as_ref())
        .and_then(|cm| cm.try_config())
        .map(|config| config.traffic.inject_bifrost_badge)
        .unwrap_or(inject_bifrost_badge_default);

    if original_host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
        if let Some(state) = admin_state.clone() {
            let req = rewrite_intercepted_virtual_host_request(req);
            let resp = AdminRouter::handle(req, state, push_manager.clone(), None).await;
            return Ok(convert_intercepted_admin_response(resp));
        }
    }

    if req
        .uri()
        .path()
        .starts_with(&format!("{ADMIN_PATH_PREFIX}/api/devtools/bridge/"))
    {
        if let Some(state) = admin_state.clone() {
            let resp = AdminRouter::handle(req, state, push_manager.clone(), None).await;
            return Ok(convert_intercepted_admin_response(resp));
        }
    }

    if is_websocket_upgrade_request(&req) {
        return handle_intercepted_websocket(
            req,
            original_host,
            original_port,
            req_id,
            admin_state,
            rules,
            verbose_logging,
            unsafe_ssl,
            client_ip,
            client_app,
            client_pid,
            client_path,
            account_name,
            listener_port,
            push_manager,
        )
        .await;
    }

    let devtools_client_req_id_from_uri = take_devtools_client_req_id_from_uri(req.uri_mut());
    let devtools_client_req_id =
        take_devtools_client_req_id(req.headers_mut()).or(devtools_client_req_id_from_uri);
    let start_time = Instant::now();
    let method = req.method().clone();
    let mut method_str = method.to_string();
    let uri = req.uri().clone();
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let query_string = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();

    let mut original_uri = tls_request_url("https", original_host, original_port, &path);
    match handle_intercepted_rule_share_query(
        &mut req,
        &original_uri,
        req_id,
        admin_state.as_ref(),
        push_manager.as_ref(),
    )
    .await
    {
        InterceptedRuleShareAction::None => {}
        InterceptedRuleShareAction::Redirect(clean_url) => {
            return Ok(build_redirect_response(302, &clean_url));
        }
    }

    let incoming_headers: std::collections::HashMap<String, String> = req
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string().to_lowercase(),
                v.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    let incoming_cookies: std::collections::HashMap<String, String> =
        collect_all_cookies_from_headers(req.headers());

    let query_params: std::collections::HashMap<String, String> = uri
        .query()
        .map(|q| {
            q.split('&')
                .filter_map(|part| {
                    let mut iter = part.splitn(2, '=');
                    match (iter.next(), iter.next()) {
                        (Some(k), Some(v)) => Some((k.to_string(), v.to_string())),
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let resolved_rules = rules.resolve_with_context(
        &original_uri,
        &method_str,
        &incoming_headers,
        &incoming_cookies,
    );
    let upstream_unsafe_ssl = unsafe_ssl || resolved_rules.upstream_unsafe_ssl;

    let has_rules = !resolved_rules.rules.is_empty()
        || resolved_rules.host.is_some()
        || !resolved_rules.req_headers.is_empty()
        || !resolved_rules.res_headers.is_empty()
        || !resolved_rules.delete_req_headers.is_empty()
        || !resolved_rules.delete_res_headers.is_empty()
        || !resolved_rules.header_replace.is_empty()
        || resolved_rules.status_code.is_some()
        || resolved_rules.replace_status.is_some();

    if verbose_logging {
        if has_rules {
            info!(
                "[{}] [RULES] matched: {}",
                req_id,
                format_rules_summary(&resolved_rules)
            );
        } else {
            info!("[{}] [RULES] matched: none", req_id);
        }
    }

    let (actual_target_host, actual_target_port, actual_use_http, actual_target_path) =
        if resolved_rules.ignored.host {
            debug!(
                "[{}] Passthrough rule applied: request will be forwarded to original target {}:{}",
                req_id, original_host, original_port
            );
            (
                original_host.to_string(),
                original_port,
                false,
                path.clone(),
            )
        } else if let Some(ref host_rule) = resolved_rules.host {
            let (h, parsed_port, parsed_path) = match parse_host_rule(host_rule) {
                Some((h, p, path_and_query)) => (h, p, path_and_query),
                None => (host_rule.trim_end_matches('/').to_string(), None, None),
            };

            let p = parsed_port.unwrap_or(match resolved_rules.host_protocol {
                Some(Protocol::Http) | Some(Protocol::Ws) => 80,
                Some(Protocol::Https) | Some(Protocol::Wss) => 443,
                _ => original_port,
            });
            let use_http_override = match resolved_rules.host_protocol {
                Some(Protocol::Http) | Some(Protocol::Ws) => true,
                Some(Protocol::Host) | Some(Protocol::XHost) => p != 443 && p != 8443,
                _ => false,
            };
            let target_path = if let Some(ref rule_path) = parsed_path {
                let host_protocol = resolved_rules.host_protocol.unwrap_or(Protocol::Host);
                if crate::utils::url::host_rule_uses_exact_target_path(
                    &resolved_rules.rules,
                    host_protocol,
                    host_rule,
                ) {
                    rule_path.clone()
                } else {
                    let source_path = crate::utils::url::find_host_rule_source_path(
                        &resolved_rules.rules,
                        host_protocol,
                        host_rule,
                    );
                    crate::utils::url::rewrite_path_with_prefix(
                        &path,
                        source_path.as_deref(),
                        rule_path,
                    )
                }
            } else {
                path.clone()
            };
            debug!(
                "[{}] Host rule applied: original={}:{} -> target={}:{}, host_protocol={:?}, use_http={}",
                req_id, original_host, original_port, h, p, resolved_rules.host_protocol, use_http_override
            );
            (h, p, use_http_override, target_path)
        } else {
            (
                original_host.to_string(),
                original_port,
                false,
                path.clone(),
            )
        };
    let mut actual_target_host = actual_target_host;
    let mut actual_target_port = actual_target_port;
    let mut actual_use_http = actual_use_http;
    let mut actual_target_path = actual_target_path;

    let target_uri = if actual_use_http {
        if actual_target_port == 80 {
            format!("http://{}{}", actual_target_host, actual_target_path)
        } else {
            format!(
                "http://{}:{}{}",
                actual_target_host, actual_target_port, actual_target_path
            )
        }
    } else if actual_target_port == 443 {
        format!("https://{}{}", actual_target_host, actual_target_path)
    } else {
        format!(
            "https://{}:{}{}",
            actual_target_host, actual_target_port, actual_target_path
        )
    };

    let rule_ctx = RequestContext::new()
        .with_request_info(
            original_uri.clone(),
            method_str.clone(),
            actual_target_host.clone(),
            path.clone(),
            query_string.clone(),
            client_ip.clone(),
        )
        .with_headers(incoming_headers.clone())
        .with_cookies(incoming_cookies.clone())
        .with_query_params(query_params.clone())
        .with_port(listener_port);

    let mut upstream_uri: hyper::Uri = match target_uri.parse() {
        Ok(uri) => apply_url_rules(&uri, &resolved_rules, verbose_logging, &rule_ctx),
        Err(e) => {
            error!("[{}] Failed to parse upstream URI: {}", req_id, e);
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"Bad Gateway".to_vec()))
                .unwrap());
        }
    };

    debug!("[{}] Intercepted: {} {}", req_id, method_str, upstream_uri);

    if let Some(ref redirect_url) = resolved_rules.redirect {
        let status = resolved_rules.redirect_status.unwrap_or(302);
        if verbose_logging {
            info!(
                "[{}] [REDIRECT] {} -> {} ({})",
                req_id, original_uri, redirect_url, status
            );
        }
        let response = build_redirect_response(status, redirect_url);
        if let Some(ref state) = admin_state {
            record_mock_traffic(
                state,
                req_id,
                &method_str,
                &original_uri,
                original_host,
                &start_time,
                has_rules,
                &resolved_rules,
                &response,
                &req,
                &client_ip,
                client_app.as_deref(),
                client_pid,
                client_path.as_deref(),
                account_name.as_deref(),
                listener_port,
                &devtools_client_req_id,
            );
        }
        return Ok(response);
    }

    if let Some(ref mock_file) = resolved_rules.mock_file {
        if verbose_logging {
            info!("[{}] [MOCK_FILE] Serving file: {}", req_id, mock_file);
        }
        let status_code = resolved_rules.status_code.unwrap_or(200);
        let response = serve_mock_file(mock_file, status_code, None).await;
        if let Some(ref state) = admin_state {
            record_mock_traffic(
                state,
                req_id,
                &method_str,
                &original_uri,
                original_host,
                &start_time,
                has_rules,
                &resolved_rules,
                &response,
                &req,
                &client_ip,
                client_app.as_deref(),
                client_pid,
                client_path.as_deref(),
                account_name.as_deref(),
                listener_port,
                &devtools_client_req_id,
            );
        }
        return Ok(response);
    }

    if let Some(ref mock_template) = resolved_rules.mock_template {
        if verbose_logging {
            info!(
                "[{}] [MOCK_TPL] Serving template: {}",
                req_id, mock_template
            );
        }
        let template_vars = TemplateVars {
            url: original_uri.clone(),
            method: method_str.clone(),
            host: actual_target_host.clone(),
            pathname: path.to_string(),
            search: uri.query().map(|q| format!("?{}", q)).unwrap_or_default(),
            client_ip: "127.0.0.1".to_string(),
            req_id: req_id.to_string(),
        };
        let status_code = resolved_rules.status_code.unwrap_or(200);
        let response = serve_mock_file(mock_template, status_code, Some(&template_vars)).await;
        if let Some(ref state) = admin_state {
            record_mock_traffic(
                state,
                req_id,
                &method_str,
                &original_uri,
                original_host,
                &start_time,
                has_rules,
                &resolved_rules,
                &response,
                &req,
                &client_ip,
                client_app.as_deref(),
                client_pid,
                client_path.as_deref(),
                account_name.as_deref(),
                listener_port,
                &devtools_client_req_id,
            );
        }
        return Ok(response);
    }

    if let Some(ref mock_rawfile) = resolved_rules.mock_rawfile {
        if verbose_logging {
            info!(
                "[{}] [MOCK_RAWFILE] Serving raw file: {}",
                req_id, mock_rawfile
            );
        }
        let status_code = resolved_rules.status_code.unwrap_or(200);
        let response = serve_mock_file(mock_rawfile, status_code, None).await;
        if let Some(ref state) = admin_state {
            record_mock_traffic(
                state,
                req_id,
                &method_str,
                &original_uri,
                original_host,
                &start_time,
                has_rules,
                &resolved_rules,
                &response,
                &req,
                &client_ip,
                client_app.as_deref(),
                client_pid,
                client_path.as_deref(),
                account_name.as_deref(),
                listener_port,
                &devtools_client_req_id,
            );
        }
        return Ok(response);
    }

    let (mut parts, body) = req.into_parts();

    let mut actual_method = if let Some(ref method_override) = resolved_rules.method {
        if verbose_logging {
            info!(
                "[{}] [METHOD] {} -> {}",
                req_id, method_str, method_override
            );
        }
        hyper::Method::from_bytes(method_override.as_bytes()).unwrap_or(method)
    } else {
        method
    };

    let accepts_html_error = request_explicitly_accepts_html(&parts.headers);
    let original_req_headers: Vec<(String, String)> = super::headers_to_pairs(&parts.headers);

    let req_headers = original_req_headers.clone();

    let req_content_encoding = get_content_encoding(&req_headers);
    let max_decompress_output_bytes = if let Some(state) = admin_state.as_ref() {
        if let Some(cm) = state.config_manager.as_ref() {
            cm.config().await.sandbox.limits.max_decompress_output_bytes
        } else {
            10 * 1024 * 1024
        }
    } else {
        10 * 1024 * 1024
    };

    apply_req_rules(&mut parts, &resolved_rules, verbose_logging, &rule_ctx);
    let output_req_headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let output_req_content_encoding = get_content_encoding(&output_req_headers);

    let req_content_length = parts
        .headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());
    let has_transfer_encoding = parts.headers.contains_key(hyper::header::TRANSFER_ENCODING);

    let needs_req_processing = needs_request_body_processing(&resolved_rules);
    let has_req_body_override = resolved_rules.req_body.is_some();
    let has_req_scripts = !resolved_rules.req_scripts.is_empty();
    let has_res_scripts = !resolved_rules.res_scripts.is_empty();
    let has_res_stream_scripts = !resolved_rules.res_stream_scripts.is_empty();
    let has_decode_scripts = !resolved_rules.decode_scripts.is_empty();
    let needs_req_body_read = !has_req_body_override
        && (needs_req_processing
            || has_req_scripts
            || (has_res_scripts && resolved_rules.status_code.is_some()));

    let mut skip_req_scripts = false;
    let mut streaming_body: Option<BoxBody> = None;
    let mut req_body_capture: Option<BodyCaptureHandle> = None;
    let mut body_bytes = if needs_req_body_read {
        if let Some(len) = req_content_length {
            if len > max_body_buffer_size {
                warn!(
                    "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and streaming forward",
                    req_id,
                    len,
                    max_body_buffer_size
                );
                skip_req_scripts = true;
                if admin_state.is_some() {
                    let (tee_body, capture) = create_request_tee_body(
                        body,
                        admin_state.clone(),
                        req_id.to_string(),
                        output_req_content_encoding.clone(),
                    );
                    streaming_body = Some(tee_body);
                    req_body_capture = Some(capture);
                } else {
                    streaming_body = Some(body.boxed());
                }
                Vec::new()
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
                    Ok(BoundedBody::Complete(bytes)) => bytes.to_vec(),
                    Ok(BoundedBody::Exceeded(replay_body)) => {
                        let size_display = req_content_length
                            .map(|len| len.to_string())
                            .unwrap_or_else(|| format!(">{}", limit));
                        warn!(
                            "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and streaming forward",
                            req_id,
                            size_display,
                            limit
                        );
                        skip_req_scripts = true;
                        if admin_state.is_some() {
                            let (tee_body, capture) = create_request_tee_body(
                                replay_body,
                                admin_state.clone(),
                                req_id.to_string(),
                                output_req_content_encoding.clone(),
                            );
                            streaming_body = Some(tee_body);
                            req_body_capture = Some(capture);
                        } else {
                            streaming_body = Some(replay_body.boxed());
                        }
                        Vec::new()
                    }
                    Err(e) => {
                        error!("[{}] Failed to read request body: {}", req_id, e);
                        return Ok(Response::builder()
                            .status(502)
                            .body(full_body(b"Bad Gateway".to_vec()))
                            .unwrap());
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
                Ok(BoundedBody::Complete(bytes)) => bytes.to_vec(),
                Ok(BoundedBody::Exceeded(replay_body)) => {
                    let size_display = req_content_length
                        .map(|len| len.to_string())
                        .unwrap_or_else(|| format!(">{}", limit));
                    warn!(
                        "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and streaming forward",
                        req_id,
                        size_display,
                        limit
                    );
                    skip_req_scripts = true;
                    if admin_state.is_some() {
                        let (tee_body, capture) = create_request_tee_body(
                            replay_body,
                            admin_state.clone(),
                            req_id.to_string(),
                            output_req_content_encoding.clone(),
                        );
                        streaming_body = Some(tee_body);
                        req_body_capture = Some(capture);
                    } else {
                        streaming_body = Some(replay_body.boxed());
                    }
                    Vec::new()
                }
                Err(e) => {
                    error!("[{}] Failed to read request body: {}", req_id, e);
                    return Ok(Response::builder()
                        .status(502)
                        .body(full_body(b"Bad Gateway".to_vec()))
                        .unwrap());
                }
            }
        }
    } else if let Some(ref new_body) = resolved_rules.req_body {
        if verbose_logging {
            info!(
                "[{}] [REQ_BODY] replaced: {} bytes -> {} bytes",
                req_id,
                req_content_length.unwrap_or(0),
                new_body.len()
            );
        }
        let mut body = body;
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
        new_body.to_vec()
    } else if req_content_length.unwrap_or(0) == 0 && !has_transfer_encoding {
        Vec::new()
    } else {
        if admin_state.is_some() {
            let (tee_body, capture) = create_request_tee_body(
                body,
                admin_state.clone(),
                req_id.to_string(),
                output_req_content_encoding.clone(),
            );
            streaming_body = Some(tee_body);
            req_body_capture = Some(capture);
        } else {
            streaming_body = Some(body.boxed());
        }
        Vec::new()
    };
    if skip_req_scripts && resolved_rules.status_code.is_none() {
        set_content_encoding_header(&mut parts.headers, req_content_encoding.as_deref());
        if let Some(capture) = req_body_capture.as_ref() {
            capture.set_content_encoding(req_content_encoding.clone());
        }
    }
    if streaming_body.is_none() && has_request_body_rules(&resolved_rules) {
        let req_content_type = parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());
        let processed = apply_body_rules_preserving_encoding(
            Bytes::from(body_bytes),
            &resolved_rules,
            Phase::Request,
            req_content_type,
            ContentInjectionEncoding {
                source: if has_req_body_override {
                    None
                } else {
                    req_content_encoding.as_deref()
                },
                output: output_req_content_encoding.as_deref(),
                max_decompress_output_bytes,
            },
            verbose_logging,
            &rule_ctx,
        );
        set_content_encoding_header(&mut parts.headers, processed.content_encoding.as_deref());
        body_bytes = processed.body.to_vec();
    }
    let mut values = HashMap::new();
    if has_req_scripts || has_res_scripts || has_res_stream_scripts || has_decode_scripts {
        values = resolved_rules.values.clone();
        let state_values = get_values_from_state(&admin_state).await;
        for (key, value) in state_values {
            values.entry(key).or_insert(value);
        }
    }

    let req_script_results = if has_req_scripts && !skip_req_scripts {
        let mut script_method = actual_method.to_string();
        let mut script_headers = header_map_to_hashmap(&parts.headers);
        let original_script_headers = script_headers.clone();
        let mut script_body = body_to_script_string(
            &Bytes::from(body_bytes.clone()),
            parts
                .headers
                .get(hyper::header::CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            max_decompress_output_bytes,
        );

        let results = execute_request_scripts(
            &admin_state,
            &resolved_rules.req_scripts,
            &rule_ctx,
            &resolved_rules,
            &original_uri,
            &mut script_method,
            &mut script_headers,
            &mut script_body,
            &values,
        )
        .await;

        if results.iter().any(|result| result.success) {
            if let Ok(new_method) = script_method.parse() {
                actual_method = new_method;
            }
            parts.headers = apply_script_headers_to_header_map(
                &parts.headers,
                &original_script_headers,
                &script_headers,
            );
            if let Some(ref new_body) = script_body {
                let encoded = script_string_to_body(
                    new_body,
                    get_content_encoding(
                        &parts
                            .headers
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect::<Vec<_>>(),
                    )
                    .as_deref(),
                );
                set_content_encoding_header(
                    &mut parts.headers,
                    encoded.content_encoding.as_deref(),
                );
                body_bytes = encoded.body.to_vec();
            }
        }

        results
    } else {
        Vec::new()
    };
    let mut host_header_value = if actual_use_http {
        if actual_target_port == 80 {
            actual_target_host.clone()
        } else {
            format!("{}:{}", actual_target_host, actual_target_port)
        }
    } else if actual_target_port == 443 {
        actual_target_host.clone()
    } else {
        format!("{}:{}", actual_target_host, actual_target_port)
    };

    let request_hook_enabled = admin_state
        .as_ref()
        .map(|state| {
            state.breakpoint_manager.is_enabled()
                && super::breakpoint::breakpoint_request_rule_enabled(&resolved_rules)
        })
        .unwrap_or(false);
    let mut request_body_omitted_for_breakpoint = false;

    if let Some(ref state) = admin_state {
        if request_hook_enabled {
            if body_bytes.is_empty() {
                if let Some(body) = streaming_body.take() {
                    let should_collect = req_content_length
                        .map(|len| state.breakpoint_manager.body_within_capture_limit(len))
                        .unwrap_or(true);
                    if should_collect {
                        match read_body_bounded(body, state.breakpoint_manager.max_body_bytes())
                            .await
                        {
                            Ok(BoundedBody::Complete(bytes)) => {
                                body_bytes = bytes.to_vec();
                            }
                            Ok(BoundedBody::Exceeded(replay_body)) => {
                                request_body_omitted_for_breakpoint = true;
                                streaming_body = Some(replay_body.boxed());
                            }
                            Err(error) => return Ok(body_read_error_response(error)),
                        }
                    } else {
                        request_body_omitted_for_breakpoint = true;
                        streaming_body = Some(body);
                    }
                }
            }
            let mut pending =
                TrafficRecord::new(req_id.to_string(), method_str.clone(), original_uri.clone());
            attach_devtools_client_req_id(&mut pending, &devtools_client_req_id);
            pending.host = original_host.to_string();
            pending.request_headers = Some(super::handler::headers_to_pairs(&parts.headers));
            if let Some(ref current) = pending.request_headers {
                if !super::headers_pairs_equal_ignore_order(&original_req_headers, current) {
                    pending.original_request_headers = Some(original_req_headers.clone());
                }
            }
            pending.request_size = if !body_bytes.is_empty() {
                body_bytes.len()
            } else {
                req_content_length.unwrap_or(0)
            };
            pending.upload_bytes = pending.request_size;
            pending.request_content_type = parts
                .headers
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            apply_listener_context(
                &mut pending,
                listener_port,
                &client_ip,
                &client_app,
                client_pid,
                &client_path,
                &account_name,
            );
            pending.has_rule_hit = has_rules;
            pending.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
            if !body_bytes.is_empty() {
                store_request_body(
                    &admin_state,
                    req_id,
                    &body_bytes,
                    get_content_encoding(pending.request_headers.as_deref().unwrap_or_default())
                        .as_deref(),
                )
                .apply_to(&mut pending);
            } else if let Some(ref capture) = req_body_capture {
                capture.apply_to(&mut pending);
            }
            if !req_script_results.is_empty() {
                pending.req_script_results = Some(req_script_results.clone());
            }
            state.record_traffic(pending);
        }
    }

    if request_hook_enabled {
        let hook_body = Bytes::from(body_bytes.clone());
        let mut edited_body = hook_body.clone();
        let outcome = super::breakpoint::breakpoint_request_hook(
            &admin_state,
            &push_manager,
            req_id,
            actual_method.as_str(),
            &upstream_uri.to_string(),
            &mut parts.headers,
            hook_body,
            req_content_length,
            request_body_omitted_for_breakpoint,
            &mut edited_body,
        )
        .await;
        if let Some(edited_method) = outcome.method.as_deref() {
            actual_method =
                hyper::Method::from_bytes(edited_method.as_bytes()).unwrap_or(actual_method);
            method_str = actual_method.to_string();
        }
        if let Some(edited_url) = outcome.url.as_deref() {
            if let Ok(uri) = edited_url.parse::<hyper::Uri>() {
                if let Some((use_http, host, port, path, authority)) =
                    breakpoint_upstream_target(&uri)
                {
                    upstream_uri = uri;
                    actual_use_http = use_http;
                    actual_target_host = host;
                    actual_target_port = port;
                    actual_target_path = path;
                    host_header_value = authority;
                    original_uri = edited_url.to_string();
                }
            }
        }
        if outcome.body_replaced {
            body_bytes = edited_body.to_vec();
        }
    }
    let req_body_mode = if streaming_body.is_some() {
        if let Some(len) = req_content_length {
            BodyMode::StreamWithLength(len)
        } else {
            BodyMode::Stream
        }
    } else {
        BodyMode::Known(body_bytes.len())
    };
    normalize_req_headers(&mut parts, req_body_mode, req_content_length.is_some());
    let request_body_is_streaming = streaming_body.is_some();
    let request_body_size = if !body_bytes.is_empty() {
        body_bytes.len()
    } else {
        req_content_length.unwrap_or(0)
    };

    if let Some(status) = resolved_rules.status_code {
        if resolved_rules.mock_file.is_none()
            && resolved_rules.mock_rawfile.is_none()
            && resolved_rules.mock_template.is_none()
            && resolved_rules.location_href.is_none()
        {
            if verbose_logging {
                info!("[{}] [MOCK] status code: {}", req_id, status);
            }
            let mut response = crate::utils::mock::build_status_response(status, &resolved_rules);
            let mut response_body = None;
            if needs_body_processing(&resolved_rules) {
                match apply_immediate_response_body_rules(
                    response,
                    &resolved_rules,
                    &method_str,
                    max_decompress_output_bytes,
                    verbose_logging,
                    &rule_ctx,
                )
                .await
                {
                    Ok((processed_response, processed_body)) => {
                        response = processed_response;
                        response_body = Some(processed_body);
                    }
                    Err(e) => {
                        error!(
                            "[{}] Failed to process statusCode response body: {}",
                            req_id, e
                        );
                        return Ok(Response::builder()
                            .status(502)
                            .body(full_body(b"Bad Gateway".to_vec()))
                            .unwrap());
                    }
                }
            }

            // FIX: run resScript on intercepted-HTTPS statusCode mock responses.
            // The plaintext path (handler.rs) already runs res scripts in its statusCode branch;
            // #188 "preserve statusCode rule pipeline" wired it up for plaintext but missed this
            // tunnel branch. Mirror the forward path below (~4449-4518) / handler.rs (~1494-1562).
            if has_res_scripts {
                let (mut sc_parts, sc_body) = response.into_parts();
                let mut sc_final_body = match response_body.take() {
                    Some(b) => b,
                    None => match sc_body.collect().await {
                        Ok(c) => c.to_bytes(),
                        Err(_) => Bytes::new(),
                    },
                };
                let mut res_script_status = sc_parts.status.as_u16();
                let mut res_script_status_text = sc_parts
                    .status
                    .canonical_reason()
                    .unwrap_or("OK")
                    .to_string();
                let mut res_script_headers = header_map_to_hashmap(&sc_parts.headers);
                let original_script_headers = res_script_headers.clone();
                let current_res_headers: Vec<(String, String)> = sc_parts
                    .headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                let mut res_script_body = body_to_script_string(
                    &sc_final_body,
                    get_content_encoding(&current_res_headers).as_deref(),
                    max_decompress_output_bytes,
                );
                let req_script_headers = header_map_to_hashmap(&parts.headers);

                let results = execute_response_scripts(
                    &admin_state,
                    &resolved_rules.res_scripts,
                    &rule_ctx,
                    &resolved_rules,
                    &original_uri,
                    &method_str,
                    &req_script_headers,
                    Some(String::from_utf8_lossy(&body_bytes).to_string()),
                    &mut res_script_status,
                    &mut res_script_status_text,
                    &mut res_script_headers,
                    &mut res_script_body,
                    &values,
                )
                .await;

                if results.iter().any(|r| r.success) {
                    if let Ok(new_status) = hyper::StatusCode::from_u16(res_script_status) {
                        sc_parts.status = new_status;
                    }
                    sc_parts.headers = apply_script_headers_to_header_map(
                        &sc_parts.headers,
                        &original_script_headers,
                        &res_script_headers,
                    );
                    if let Some(ref new_body) = res_script_body {
                        let cur: Vec<(String, String)> = sc_parts
                            .headers
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        let encoded =
                            script_string_to_body(new_body, get_content_encoding(&cur).as_deref());
                        set_content_encoding_header(
                            &mut sc_parts.headers,
                            encoded.content_encoding.as_deref(),
                        );
                        sc_final_body = encoded.body;
                    }
                }

                // Re-derive Content-Length / body-mode headers for the (possibly) rewritten body,
                // matching the plaintext statusCode path (handler.rs). Without this the response
                // keeps the original status body's Content-Length and the scripted body is truncated.
                normalize_res_headers(
                    &mut sc_parts,
                    buffered_res_body_mode(
                        sc_final_body.len(),
                        !resolved_rules.trailers.is_empty(),
                    ),
                    &method_str,
                );
                response_body = Some(sc_final_body.clone());
                response = Response::from_parts(sc_parts, full_body(sc_final_body));
            }

            if let Some(ref state) = admin_state {
                let final_req_headers = super::handler::headers_to_pairs(&parts.headers);
                let final_req_content_encoding = get_content_encoding(&final_req_headers);
                if let Some(ref capture) = req_body_capture {
                    capture.set_content_encoding(final_req_content_encoding.clone());
                }
                record_direct_status_traffic(
                    state,
                    req_id,
                    &method_str,
                    &original_uri,
                    original_host,
                    &start_time,
                    has_rules,
                    &resolved_rules,
                    &response,
                    &final_req_headers,
                    &original_req_headers,
                    &body_bytes,
                    final_req_content_encoding.as_deref(),
                    &req_body_capture,
                    response_body,
                    &req_script_results,
                    &client_ip,
                    &client_app,
                    client_pid,
                    &client_path,
                    &account_name,
                    listener_port,
                    &devtools_client_req_id,
                );
            }

            return Ok(response);
        }
    }

    let mut new_req = Request::builder()
        .method(actual_method.clone())
        .uri(&upstream_uri);
    let mut skip_referer = false;
    let mut skip_ua = false;
    for (name, value) in parts.headers.iter() {
        if name == hyper::header::HOST {
            continue;
        }
        if name == hyper::header::CONTENT_LENGTH {
            continue;
        }
        if name == hyper::header::REFERER && resolved_rules.referer.is_some() {
            skip_referer = true;
            continue;
        }
        if name == hyper::header::USER_AGENT && resolved_rules.ua.is_some() {
            skip_ua = true;
            continue;
        }
        if name == hyper::header::COOKIE {
            continue;
        }
        new_req = new_req.header(name, value);
    }

    if !resolved_rules.req_cookies.is_empty() {
        let mut cookie_map: std::collections::HashMap<String, String> =
            collect_all_cookies_from_headers(&parts.headers);

        for (name, value) in &resolved_rules.req_cookies {
            if verbose_logging {
                info!("[{}] [REQ_COOKIE] {} = {}", req_id, name, value);
            }
            cookie_map.insert(name.clone(), value.clone());
        }

        let cookie_str: String = cookie_map
            .into_iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ");

        new_req = new_req.header(hyper::header::COOKIE, cookie_str);
    } else {
        let merged = merge_cookie_header_values(&parts.headers);
        if !merged.is_empty() {
            new_req = new_req.header(hyper::header::COOKIE, merged);
        }
    }

    new_req = new_req.header(hyper::header::HOST, &host_header_value);
    if streaming_body.is_none() {
        if !body_bytes.is_empty() || req_content_length.is_some() {
            new_req = new_req.header(hyper::header::CONTENT_LENGTH, body_bytes.len());
        }
    } else if let Some(content_length) = req_content_length {
        new_req = new_req.header(hyper::header::CONTENT_LENGTH, content_length);
    }

    if let Some(ref referer) = resolved_rules.referer {
        if !referer.is_empty() {
            if verbose_logging {
                info!("[{}] [REFERER] -> {}", req_id, referer);
            }
            new_req = new_req.header(hyper::header::REFERER, referer);
        } else if verbose_logging && skip_referer {
            info!("[{}] [REFERER] Removed", req_id);
        }
    }

    if let Some(ref ua) = resolved_rules.ua {
        if !ua.is_empty() {
            if verbose_logging {
                info!("[{}] [USER-AGENT] -> {}", req_id, ua);
            }
            new_req = new_req.header(hyper::header::USER_AGENT, ua);
        } else if verbose_logging && skip_ua {
            info!("[{}] [USER-AGENT] Removed", req_id);
        }
    }

    let outgoing_body = match streaming_body {
        Some(body) => body,
        None => full_body(Bytes::from(body_bytes.clone())),
    };
    let outgoing_body = wrap_throttled_body(outgoing_body, resolved_rules.req_speed);
    let mut outgoing_req = match new_req.body(outgoing_body) {
        Ok(r) => r,
        Err(e) => {
            error!("[{}] Failed to build request: {}", req_id, e);
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"Bad Gateway".to_vec()))
                .unwrap());
        }
    };
    if let Err(err) = apply_resolved_req_headers_to_outgoing_request(
        req_id,
        &mut outgoing_req,
        &resolved_rules.req_headers,
        verbose_logging,
    ) {
        error!("[{}] Failed to apply request headers: {}", req_id, err);
        return Ok(Response::builder()
            .status(502)
            .body(full_body(b"Bad Gateway".to_vec()))
            .unwrap());
    }
    if skip_req_scripts {
        set_content_encoding_header(outgoing_req.headers_mut(), req_content_encoding.as_deref());
    }
    sanitize_upstream_headers(outgoing_req.headers_mut());
    outgoing_req.headers_mut().remove(hyper::header::HOST);

    let final_req_headers: Vec<(String, String)> = super::headers_to_pairs(outgoing_req.headers());
    let final_req_content_encoding = get_content_encoding(&final_req_headers);
    if let Some(ref capture) = req_body_capture {
        capture.set_content_encoding(final_req_content_encoding.clone());
    }

    if let Some(delay_ms) = resolved_rules.req_delay {
        if verbose_logging {
            info!("[{}] [REQ_DELAY] Sleeping {}ms", req_id, delay_ms);
        }
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }

    if let Some(speed) = resolved_rules.req_speed {
        if verbose_logging {
            info!("[{}] [REQ_SPEED] Speed limit: {} bytes/s", req_id, speed);
        }
    }

    // Pooled upstream client already owns DNS resolution and connection reuse.
    // Pre-resolving here only adds duplicate lookup cost and stretches H2 tail latency.
    let dns_ms = None;

    let error_badge_rules_json = if inject_bifrost_badge && accepts_html_error {
        Some(super::handler::build_badge_rules_json(admin_state.as_deref(), listener_port).await)
    } else {
        None
    };
    let build_conn_error_record_and_response =
        |error_type: &'static str, error_msg: String, tls_ms: Option<u64>| {
            let error_info = ConnectionErrorInfo {
                error_type,
                error_message: error_msg.clone(),
                host: super::handler::format_connection_endpoint(
                    &actual_target_host,
                    actual_target_port,
                ),
                request_url: original_uri.clone(),
            };
            let response_status = if needs_response_override(&resolved_rules) {
                resolved_rules
                    .status_code
                    .or(resolved_rules.replace_status)
                    .unwrap_or(502)
            } else {
                502
            };
            let (response_body, default_content_type) =
                if let Some(ref res_body) = resolved_rules.res_body {
                    (res_body.clone(), None)
                } else {
                    let (body, content_type) = build_connection_error_body(
                        response_status,
                        &error_info,
                        error_badge_rules_json.as_deref(),
                    );
                    (body, Some(content_type))
                };
            let total_ms = start_time.elapsed().as_millis() as u64;
            if let Some(ref state) = admin_state {
                let mut record = TrafficRecord::new(
                    req_id.to_string(),
                    method_str.clone(),
                    original_uri.clone(),
                );
                attach_devtools_client_req_id(&mut record, &devtools_client_req_id);
                record.status = response_status;
                record.duration_ms = total_ms;
                record.host = original_host.to_string();
                apply_listener_context(
                    &mut record,
                    listener_port,
                    &client_ip,
                    &client_app,
                    client_pid,
                    &client_path,
                    &account_name,
                );
                record.timing = Some(RequestTiming {
                    dns_ms,
                    connect_ms: None,
                    tls_ms,
                    send_ms: None,
                    wait_ms: None,
                    first_byte_ms: None,
                    receive_ms: None,
                    total_ms,
                });
                record.request_headers = Some(final_req_headers.clone());
                if !super::headers_pairs_equal_ignore_order(
                    &original_req_headers,
                    &final_req_headers,
                ) {
                    record.original_request_headers = Some(original_req_headers.clone());
                }
                record.has_rule_hit = has_rules;
                record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                record.error_message = Some(error_msg);
                if !body_bytes.is_empty() {
                    store_request_body(
                        &admin_state,
                        req_id,
                        &body_bytes,
                        final_req_content_encoding.as_deref(),
                    )
                    .apply_to(&mut record);
                } else if let Some(ref capture) = req_body_capture {
                    capture.apply_to(&mut record);
                }

                record.response_body_ref = if state.get_super_performance_mode() {
                    None
                } else if let Some(ref body_store) = state.body_store {
                    let store = body_store.read();
                    store.store(req_id, "res", response_body.as_ref())
                } else {
                    store_response_body(&admin_state, req_id, &response_body)
                };

                {
                    let mut res_header_pairs: Vec<(String, String)> = Vec::new();
                    if needs_response_override(&resolved_rules) {
                        for (name, value) in &resolved_rules.res_headers {
                            res_header_pairs.push((name.clone(), value.clone()));
                        }
                        if let Some(content_type) = default_content_type {
                            res_header_pairs
                                .push(("content-type".to_string(), content_type.to_string()));
                            res_header_pairs
                                .push(("x-bifrost-error".to_string(), error_type.to_string()));
                        }
                    } else {
                        res_header_pairs.push((
                            "content-type".to_string(),
                            default_content_type
                                .unwrap_or("text/plain; charset=utf-8")
                                .to_string(),
                        ));
                        res_header_pairs
                            .push(("x-bifrost-error".to_string(), error_type.to_string()));
                    }
                    if !res_header_pairs.is_empty() {
                        record.original_response_headers = Some(res_header_pairs);
                    }
                }

                record.response_size = calculate_response_size(
                    record.status,
                    record.original_response_headers.as_deref().unwrap_or(&[]),
                    response_body.len(),
                );
                record.download_bytes = response_body.len();

                state.record_traffic(record);
            }
            if needs_response_override(&resolved_rules) {
                if verbose_logging {
                    info!(
                        "[{}] [CONN_ERROR] {}, applying response override rules",
                        req_id, error_type
                    );
                }
                build_overridden_error_response_from_body(
                    &resolved_rules,
                    response_status,
                    &error_info,
                    response_body,
                    default_content_type,
                )
            } else {
                build_connection_error_response_from_body(
                    502,
                    &error_info,
                    response_body,
                    default_content_type.unwrap_or("text/plain; charset=utf-8"),
                )
            }
        };

    let (mut upstream_parts, upstream_body) = outgoing_req.into_parts();
    upstream_parts.uri = upstream_uri.clone();
    upstream_parts.headers.remove(hyper::header::HOST);
    sanitize_upstream_headers(&mut upstream_parts.headers);
    if !resolved_rules.res_stream_scripts.is_empty() {
        upstream_parts.headers.insert(
            hyper::header::ACCEPT_ENCODING,
            hyper::header::HeaderValue::from_static("identity"),
        );
    }

    #[cfg(feature = "http3")]
    let req_headers_for_h3: Vec<(String, String)> =
        super::headers_to_pairs(&upstream_parts.headers);

    #[cfg(feature = "http3")]
    let h3_dns_resolver = DnsResolver::new(verbose_logging);

    #[cfg(feature = "http3")]
    let should_try_http3_upstream = !actual_use_http
        && resolved_rules.upstream_http3
        && resolved_rules.res_stream_scripts.is_empty()
        && !request_body_is_streaming
        && resolved_rules.proxy.is_none()
        && !ProtocolDetector::is_websocket_upgrade(&req_headers_for_h3)
        && !ProtocolDetector::is_sse_request(&req_headers_for_h3);

    #[cfg(feature = "http3")]
    let h3_attempt = if should_try_http3_upstream {
        let upstream_authority = if (!actual_use_http && actual_target_port == 443)
            || (actual_use_http && actual_target_port == 80)
        {
            actual_target_host.clone()
        } else {
            format!("{}:{}", actual_target_host, actual_target_port)
        };
        let mut builder = Request::builder()
            .method(upstream_parts.method.clone())
            .uri(upstream_uri.clone());
        for (key, value) in upstream_parts.headers.iter() {
            builder = builder.header(key, value);
        }
        builder = builder.header("host", upstream_authority);
        match builder.body(Bytes::from(body_bytes.clone())) {
            Ok(h3_req) => {
                let start = Instant::now();
                match try_send_http3_upstream(
                    &actual_target_host,
                    actual_target_port,
                    h3_req,
                    upstream_unsafe_ssl,
                    &h3_dns_resolver,
                    &resolved_rules.dns_servers,
                )
                .await
                {
                    Ok(resp) => {
                        info!(
                            "[{}] Upstream negotiated HTTP/3 for {}:{}",
                            req_id, actual_target_host, actual_target_port
                        );
                        Some((resp, start.elapsed().as_millis() as u64))
                    }
                    Err(err) => {
                        warn!(
                            "[{}] Upstream HTTP/3 attempt failed for {}:{}: {}, falling back to HTTP/1.1/2",
                            req_id,
                            actual_target_host,
                            actual_target_port,
                            err
                        );
                        None
                    }
                }
            }
            Err(err) => {
                warn!(
                    "[{}] Failed to build upstream HTTP/3 request for {}:{}: {}",
                    req_id, actual_target_host, actual_target_port, err
                );
                None
            }
        }
    } else {
        None
    };

    let retry_blueprint = if !actual_use_http
        && matches!(method_str.as_str(), "GET" | "HEAD")
        && !request_body_is_streaming
    {
        Some(RetryableRequestBlueprint {
            method: upstream_parts.method.clone(),
            uri: upstream_parts.uri.clone(),
            headers: upstream_parts.headers.clone(),
            body: Bytes::from(body_bytes.clone()),
        })
    } else {
        None
    };
    let upstream_req = Request::from_parts(upstream_parts, upstream_body);

    let pool_partition = build_upstream_pool_partition(
        original_host,
        &actual_target_host,
        actual_target_port,
        actual_use_http,
        &resolved_rules,
    );
    #[cfg(feature = "http3")]
    let upstream_result = if let Some((response, wait_ms)) = h3_attempt {
        let (parts, body) = response.into_parts();
        (parts, None, None, wait_ms, Some(body))
    } else {
        let send_start = Instant::now();
        let response = match send_pooled_request(
            upstream_req,
            upstream_unsafe_ssl,
            &resolved_rules.dns_servers,
            &pool_partition,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let retryable_upstream_h2 = !actual_use_http
                    && retry_blueprint.is_some()
                    && (!e.is_connect() || is_retryable_http2_error(&e));

                if retryable_upstream_h2 {
                    warn!(
                        "[{}] Upstream HTTP/2 request failed; retrying with HTTP/1.1 fallback",
                        req_id
                    );
                    mark_http1_fallback(
                        upstream_unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    );
                    let retry_request = match retry_blueprint
                        .as_ref()
                        .expect("retry blueprint exists for retryable request")
                        .build()
                    {
                        Ok(request) => request,
                        Err(err) => {
                            return Ok(build_conn_error_record_and_response(
                                "REQUEST_BUILD_FAILED",
                                err.to_string(),
                                None,
                            ));
                        }
                    };
                    match send_pooled_request_http1_only(
                        retry_request,
                        upstream_unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(retry_err) => {
                            let classified = classify_request_error(&retry_err);
                            error!(
                                "[{}] {} ({})",
                                req_id, classified.error_message, classified.error_type
                            );
                            for source in &classified.source_chain {
                                error!("[{}] Request failure source: {}", req_id, source);
                            }
                            return Ok(build_conn_error_record_and_response(
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
                        req_id, classified.error_message, classified.error_type
                    );
                    for source in &classified.source_chain {
                        error!("[{}] Request failure source: {}", req_id, source);
                    }
                    return Ok(build_conn_error_record_and_response(
                        classified.error_type,
                        classified.error_message,
                        None,
                    ));
                }
            }
        };
        let wait_ms = send_start.elapsed().as_millis() as u64;
        let (parts, body) = response.into_parts();
        (parts, Some(body), None, wait_ms, None)
    };

    #[cfg(not(feature = "http3"))]
    let upstream_result = {
        let send_start = Instant::now();
        let response = match send_pooled_request(
            upstream_req,
            upstream_unsafe_ssl,
            &resolved_rules.dns_servers,
            &pool_partition,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let retryable_upstream_h2 = !actual_use_http
                    && retry_blueprint.is_some()
                    && (!e.is_connect() || is_retryable_http2_error(&e));

                if retryable_upstream_h2 {
                    warn!(
                        "[{}] Upstream HTTP/2 request failed; retrying with HTTP/1.1 fallback",
                        req_id
                    );
                    mark_http1_fallback(
                        upstream_unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    );
                    let retry_request = match retry_blueprint
                        .as_ref()
                        .expect("retry blueprint exists for retryable request")
                        .build()
                    {
                        Ok(request) => request,
                        Err(err) => {
                            return Ok(build_conn_error_record_and_response(
                                "REQUEST_BUILD_FAILED",
                                err.to_string(),
                                None,
                            ));
                        }
                    };
                    match send_pooled_request_http1_only(
                        retry_request,
                        upstream_unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(retry_err) => {
                            let classified = classify_request_error(&retry_err);
                            error!(
                                "[{}] {} ({})",
                                req_id, classified.error_message, classified.error_type
                            );
                            for source in &classified.source_chain {
                                error!("[{}] Request failure source: {}", req_id, source);
                            }
                            return Ok(build_conn_error_record_and_response(
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
                        req_id, classified.error_message, classified.error_type
                    );
                    for source in &classified.source_chain {
                        error!("[{}] Request failure source: {}", req_id, source);
                    }
                    return Ok(build_conn_error_record_and_response(
                        classified.error_type,
                        classified.error_message,
                        None,
                    ));
                }
            }
        };
        let wait_ms = send_start.elapsed().as_millis() as u64;
        let (parts, body) = response.into_parts();
        (parts, Some(body), None, wait_ms, None)
    };

    let (mut res_parts, mut res_body, tls_ms, mut wait_ms, mut h3_buffered_body) = upstream_result;

    if h3_buffered_body.is_none() {
        let early_content_type = res_parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let early_content_length = res_parts
            .headers
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok());
        let recovery_action = h2_body_recovery_action(
            res_parts.version,
            res_parts.status,
            &method_str,
            &early_content_type,
            early_content_length,
            max_body_buffer_size,
            retry_blueprint.is_some(),
        );

        if matches!(recovery_action, H2BodyRecoveryAction::Probe) {
            let body = res_body
                .take()
                .expect("upstream response body should exist");
            match read_body_bounded(body, max_body_buffer_size).await {
                Ok(BoundedBody::Complete(bytes)) => {
                    h3_buffered_body = Some(bytes);
                }
                Ok(BoundedBody::Exceeded(replay_body)) => {
                    res_body = Some(replay_body.boxed());
                }
                Err(e) => {
                    warn!(
                        "[{}] Upstream HTTP/2 response body failed while probing response; retrying with HTTP/1.1 fallback: {}",
                        req_id, e
                    );
                    mark_http1_fallback(
                        upstream_unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    );
                    let retry_request = match retry_blueprint
                        .as_ref()
                        .expect("retry blueprint exists for H2 body fallback")
                        .build()
                    {
                        Ok(request) => request,
                        Err(err) => {
                            return Ok(build_conn_error_record_and_response(
                                "REQUEST_BUILD_FAILED",
                                err.to_string(),
                                None,
                            ));
                        }
                    };
                    let retry_start = Instant::now();
                    match send_pooled_request_http1_only(
                        retry_request,
                        upstream_unsafe_ssl,
                        &resolved_rules.dns_servers,
                        &pool_partition,
                    )
                    .await
                    {
                        Ok(response) => {
                            info!(
                                "[{}] Upstream response body recovered via HTTP/1.1 fallback",
                                req_id
                            );
                            wait_ms = retry_start.elapsed().as_millis() as u64;
                            let (parts, body) = response.into_parts();
                            res_parts = parts;
                            res_body = Some(body);
                        }
                        Err(err) => {
                            let classified = classify_request_error(&err);
                            error!(
                                "[{}] {} ({})",
                                req_id, classified.error_message, classified.error_type
                            );
                            for source in &classified.source_chain {
                                error!("[{}] Request failure source: {}", req_id, source);
                            }
                            return Ok(build_conn_error_record_and_response(
                                classified.error_type,
                                classified.error_message,
                                None,
                            ));
                        }
                    }
                }
            }
        } else if matches!(recovery_action, H2BodyRecoveryAction::RetryHttp1) {
            warn!(
                "[{}] Upstream HTTP/2 response is a large or unknown-size binary body; retrying with HTTP/1.1 fallback before streaming",
                req_id
            );
            mark_http1_fallback(
                upstream_unsafe_ssl,
                &resolved_rules.dns_servers,
                &pool_partition,
            );
            let retry_request = match retry_blueprint
                .as_ref()
                .expect("retry blueprint exists for H2 body fallback")
                .build()
            {
                Ok(request) => request,
                Err(err) => {
                    return Ok(build_conn_error_record_and_response(
                        "REQUEST_BUILD_FAILED",
                        err.to_string(),
                        None,
                    ));
                }
            };
            let retry_start = Instant::now();
            match send_pooled_request_http1_only(
                retry_request,
                upstream_unsafe_ssl,
                &resolved_rules.dns_servers,
                &pool_partition,
            )
            .await
            {
                Ok(response) => {
                    info!(
                        "[{}] Upstream response switched to HTTP/1.1 fallback before streaming",
                        req_id
                    );
                    wait_ms = retry_start.elapsed().as_millis() as u64;
                    let (parts, body) = response.into_parts();
                    res_parts = parts;
                    res_body = Some(body);
                }
                Err(err) => {
                    let classified = classify_request_error(&err);
                    error!(
                        "[{}] {} ({})",
                        req_id, classified.error_message, classified.error_type
                    );
                    for source in &classified.source_chain {
                        error!("[{}] Request failure source: {}", req_id, source);
                    }
                    return Ok(build_conn_error_record_and_response(
                        classified.error_type,
                        classified.error_message,
                        None,
                    ));
                }
            }
        }
    }

    // NOTE: the response status override (replace_status / status_code) is applied below
    // by `apply_res_rules`, using the response-phase-resolved rules so that
    // response-dependent filters gate it. Applying it here from the request-phase
    // `resolved_rules` would be both ungated and would corrupt the status that the
    // response-phase re-resolve reads, so it is intentionally not done here.

    let original_res_headers: Vec<(String, String)> = res_parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let res_content_encoding = get_content_encoding(&original_res_headers);

    let ctx = RequestContext::new()
        .with_request_info(
            original_uri.clone(),
            method_str.clone(),
            actual_target_host.clone(),
            path.to_string(),
            query_string.clone(),
            client_ip.clone(),
        )
        .with_headers(incoming_headers.clone())
        .with_cookies(incoming_cookies.clone())
        .with_query_params(query_params.clone())
        .with_port(listener_port)
        .with_client_process(client_app.clone(), client_pid, client_path.clone())
        .with_account_name(account_name.clone());
    let request_origin = incoming_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
        .map(|(_, v)| v.as_str());
    let res_ctx = ctx.with_response_data(res_parts.status.as_u16(), &res_parts.headers);
    // Re-resolve with the upstream response available so response-dependent filters
    // (s:/resH:) are evaluated against the real response; response-modification ops are
    // applied from this set. See the HTTP path in handler.rs for the rationale.
    let response_resolved = if needs_response_phase_resolve(&resolved_rules) {
        let res_header_map: std::collections::HashMap<String, String> = res_parts
            .headers
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (k.as_str().to_string(), s.to_string()))
            })
            .collect();
        rules.resolve_with_response_context(
            &original_uri,
            &method_str,
            &incoming_headers,
            &incoming_cookies,
            res_parts.status.as_u16(),
            &res_header_map,
        )
    } else {
        resolved_rules.clone()
    };
    if !response_resolved.res_stream_scripts.is_empty() {
        let state_values = get_values_from_state(&admin_state).await;
        for (key, value) in state_values {
            values.entry(key).or_insert(value);
        }
        for (key, value) in &response_resolved.values {
            values.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    apply_res_rules(
        &mut res_parts,
        &response_resolved,
        verbose_logging,
        &res_ctx,
        request_origin,
    );

    let res_headers: Vec<(String, String)> = res_parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let output_res_content_encoding = get_content_encoding(&res_headers);

    let needs_body_rules_processing = needs_body_processing(&response_resolved);
    let res_content_type_str = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let force_body_processing_for_badge =
        inject_bifrost_badge && res_content_type_str.starts_with("text/html");
    let force_body_processing_for_devtools =
        devtools_bridge_requested(&resolved_rules) && res_content_type_str.starts_with("text/html");
    let is_websocket = res_parts.status == hyper::StatusCode::SWITCHING_PROTOCOLS
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

    let res_content_type = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let is_sse = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().starts_with("text/event-stream"))
        .unwrap_or(false);
    if !response_resolved.res_stream_scripts.is_empty() {
        if !response_resolved.res_scripts.is_empty() {
            return Ok(Response::builder()
                .status(hyper::StatusCode::BAD_GATEWAY)
                .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(full_body(
                    "resScript and resStreamScript cannot be combined on one response",
                ))
                .unwrap());
        }
        if !is_sse {
            return Ok(Response::builder()
                .status(hyper::StatusCode::BAD_GATEWAY)
                .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(full_body(
                    "resStreamScript requires a text/event-stream response",
                ))
                .unwrap());
        }
        if response_content_encoding(&res_parts)
            .is_some_and(|encoding| !content_encoding_is_identity(&encoding))
        {
            return Ok(Response::builder()
                .status(hyper::StatusCode::BAD_GATEWAY)
                .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(full_body(
                    "resStreamScript does not support encoded upstream SSE responses",
                ))
                .unwrap());
        }
    }
    let binary_traffic_performance_mode = admin_state
        .as_ref()
        .map(|state| state.get_binary_traffic_performance_mode())
        .unwrap_or(false);
    let skip_binary_recording =
        should_use_binary_performance_mode(&res_parts, binary_traffic_performance_mode)
            && !is_websocket
            && !is_sse;
    let response_breakpoint_enabled = admin_state
        .as_ref()
        .map(|state| {
            state.breakpoint_manager.is_enabled()
                && super::breakpoint::breakpoint_response_rule_enabled(&resolved_rules)
        })
        .unwrap_or(false);
    let breakpoint_max_body_bytes = admin_state
        .as_ref()
        .map_or(0, |state| state.breakpoint_manager.max_body_bytes());
    #[rustfmt::skip] let response_breakpoint_can_buffer_body = response_breakpoint_can_buffer_body(response_breakpoint_enabled, is_websocket, is_sse, skip_binary_recording, res_content_length, breakpoint_max_body_bytes);
    let response_breakpoint_header_only = response_breakpoint_enabled
        && !is_websocket
        && !is_sse
        && !skip_binary_recording
        && !response_breakpoint_can_buffer_body;
    let needs_processing = needs_body_rules_processing
        || force_body_processing_for_badge
        || force_body_processing_for_devtools
        || response_breakpoint_can_buffer_body;
    let has_res_body_override = response_resolved.res_body.is_some();
    let needs_res_body_read = needs_processing && !has_res_body_override;

    let mut res_body_too_large = false;
    let mut res_body_limit = max_body_buffer_size;
    let mut res_body_incoming = res_body;
    let mut res_body_stream: Option<BoxBody> = None;
    if !is_sse {
        if let Some(ref body) = h3_buffered_body {
            res_body_stream = Some(full_body(body.clone()));
        } else {
            res_body_stream = Some(res_body_incoming.take().unwrap().boxed());
        }
    }

    let mut pre_read_res: Option<(Vec<u8>, u64)> = None;
    if let Some(body) = h3_buffered_body.clone() {
        pre_read_res = Some((body.to_vec(), 0));
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
                        pre_read_res = Some((bytes.to_vec(), receive_ms));
                    }
                    Ok(BoundedBody::Exceeded(replay_body)) => {
                        res_body_too_large = true;
                        res_body_stream = Some(replay_body.boxed());
                    }
                    Err(e) => {
                        error!("[{}] Failed to read response body: {}", req_id, e);
                        return Ok(Response::builder()
                            .status(502)
                            .body(full_body(b"Bad Gateway".to_vec()))
                            .unwrap());
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
                    pre_read_res = Some((bytes.to_vec(), receive_ms));
                }
                Ok(BoundedBody::Exceeded(replay_body)) => {
                    res_body_too_large = true;
                    res_body_stream = Some(replay_body.boxed());
                }
                Err(e) => {
                    error!("[{}] Failed to read response body: {}", req_id, e);
                    return Ok(Response::builder()
                        .status(502)
                        .body(full_body(b"Bad Gateway".to_vec()))
                        .unwrap());
                }
            }
        }
    }

    let response_breakpoint_header_only = response_breakpoint_header_only
        || (response_breakpoint_enabled && res_body_too_large && !is_websocket && !is_sse);
    let skip_body_processing = skip_binary_recording
        || is_sse
        || !needs_processing
        || (res_body_too_large && needs_res_body_read);

    if needs_res_body_read && res_body_too_large {
        let size_display = res_content_length
            .map(|len| len.to_string())
            .unwrap_or_else(|| format!(">{}", res_body_limit));
        let skip_detail = if force_body_processing_for_badge {
            "skipping body rules and badge injection"
        } else if force_body_processing_for_devtools {
            "skipping body rules and DevTools bridge injection"
        } else {
            "skipping body rules"
        };
        warn!(
            "[{}] [RES_BODY] body too large ({} bytes > {} limit), {}, streaming forward",
            req_id, size_display, res_body_limit, skip_detail
        );
    }

    if skip_body_processing {
        let res_body_mode =
            streaming_res_body_mode(res_content_length, !resolved_rules.trailers.is_empty());
        normalize_res_headers(&mut res_parts, res_body_mode, &method_str);
        // `sanitize_upstream_headers` strips the hop-by-hop `Trailer` header,
        // but when the proxy announces response trailers (via a `trailers://`
        // rule) the client must keep the `Trailer` header to know which trailing
        // fields to expect. Preserve and restore it across the sanitize call.
        let preserved_trailer = res_parts.headers.get(hyper::header::TRAILER).cloned();
        sanitize_upstream_headers(&mut res_parts.headers);
        if let Some(trailer) = preserved_trailer {
            res_parts.headers.insert(hyper::header::TRAILER, trailer);
        }

        let total_ms = start_time.elapsed().as_millis() as u64;
        let record_id = req_id.to_string();
        let traffic_type = if is_websocket {
            TrafficType::Wss
        } else {
            TrafficType::Https
        };
        let mut sse_stream_writer: Option<bifrost_admin::BodyStreamWriter> = None;

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .add_bytes_sent_by_type(traffic_type, request_body_size as u64);
            state
                .metrics_collector
                .increment_requests_by_type(traffic_type);

            if !skip_binary_recording {
                let mut record =
                    TrafficRecord::new(record_id.clone(), method_str.clone(), original_uri.clone());
                attach_devtools_client_req_id(&mut record, &devtools_client_req_id);
                record.status = res_parts.status.as_u16();
                record.content_type = res_parts
                    .headers
                    .get(hyper::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let res_headers: Vec<(String, String)> = res_parts
                    .headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                record.request_size = calculate_request_size(
                    &method_str,
                    &original_uri,
                    &req_headers,
                    request_body_size,
                );
                record.upload_bytes = request_body_size;
                record.response_size = 0;
                record.duration_ms = total_ms;
                record.host = original_host.to_string();
                record.timing = Some(RequestTiming {
                    dns_ms,
                    connect_ms: None,
                    tls_ms,
                    send_ms: None,
                    wait_ms: Some(wait_ms),
                    first_byte_ms: None,
                    receive_ms: None,
                    total_ms,
                });
                record.request_headers = Some(final_req_headers.clone());
                record.original_response_headers = Some(original_res_headers.clone());
                if res_headers != original_res_headers {
                    record.response_headers = Some(res_headers.clone());
                }
                if !super::headers_pairs_equal_ignore_order(
                    &original_req_headers,
                    &final_req_headers,
                ) {
                    record.original_request_headers = Some(original_req_headers.clone());
                }
                if actual_target_host != original_host
                    || actual_target_port != original_port
                    || actual_target_path != path
                {
                    let actual_scheme = if actual_use_http { "http" } else { "https" };
                    let actual_url = if (actual_use_http && actual_target_port == 80)
                        || (!actual_use_http && actual_target_port == 443)
                    {
                        format!(
                            "{}://{}{}",
                            actual_scheme, actual_target_host, actual_target_path
                        )
                    } else {
                        format!(
                            "{}://{}:{}{}",
                            actual_scheme,
                            actual_target_host,
                            actual_target_port,
                            actual_target_path
                        )
                    };
                    record.actual_url = Some(actual_url);
                    record.actual_host = Some(actual_target_host.clone());
                }
                record.request_content_type = final_req_headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone());
                apply_listener_context(
                    &mut record,
                    listener_port,
                    &client_ip,
                    &client_app,
                    client_pid,
                    &client_path,
                    &account_name,
                );

                if is_websocket {
                    record.protocol = "wss".to_string();
                }

                if is_sse {
                    record.set_sse();
                    state.sse_hub.register(&record_id);
                }

                record.has_rule_hit = has_rules;
                record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                if !req_script_results.is_empty() {
                    record.req_script_results = Some(req_script_results.clone());
                }

                if !body_bytes.is_empty() {
                    store_request_body(
                        &admin_state,
                        &record_id,
                        &body_bytes,
                        final_req_content_encoding.as_deref(),
                    )
                    .apply_to(&mut record);
                } else if let Some(ref capture) = req_body_capture {
                    capture.apply_to(&mut record);
                }

                if is_sse && !state.get_super_performance_mode() {
                    if let Some(ref body_store) = state.body_store {
                        match body_store.read().start_stream(&record_id, "sse_raw") {
                            Ok(writer) => {
                                record.response_body_ref = Some(writer.body_ref());
                                record.set_response_body_content_encoding(
                                    res_content_encoding.as_deref(),
                                );
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

        if let Some(delay_ms) = resolved_rules.res_delay {
            if verbose_logging {
                info!("[{}] [RES_DELAY] Sleeping {}ms", req_id, delay_ms);
            }
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }

        if let Some(speed) = resolved_rules.res_speed {
            if verbose_logging {
                info!("[{}] [RES_SPEED] Speed limit: {} bytes/s", req_id, speed);
            }
        }

        if response_breakpoint_header_only {
            let mut ignored_body = Bytes::new();
            let outcome = super::breakpoint::breakpoint_response_hook(
                &admin_state,
                &push_manager,
                req_id,
                &method_str,
                &original_uri,
                res_parts.status.as_u16(),
                &mut res_parts.headers,
                Bytes::new(),
                res_content_length,
                true,
                &mut ignored_body,
            )
            .await;
            let no_body = apply_edited_response_status(&mut res_parts, &method_str, outcome.status);
            if let Some(ref state) = admin_state {
                let final_status = res_parts.status.as_u16();
                let final_headers = super::headers_to_pairs(&res_parts.headers);
                let final_content_type = res_parts
                    .headers
                    .get(hyper::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                #[rustfmt::skip] let update_record = move |record: &mut TrafficRecord| super::breakpoint::apply_response_breakpoint_record_state(record, final_status, final_content_type.clone(), final_headers.clone(), no_body);
                state.update_traffic_by_id(&record_id, update_record);
            }
            if no_body {
                return Ok(Response::from_parts(res_parts, full_body(Bytes::new())));
            }
        }

        if is_sse {
            let hook_enabled = response_breakpoint_enabled;
            let can_buffer_sse_for_breakpoint = hook_enabled
                && admin_state
                    .as_ref()
                    .and_then(|s| {
                        res_content_length
                            .map(|len| s.breakpoint_manager.body_within_capture_limit(len))
                    })
                    .unwrap_or(false);

            if hook_enabled && !can_buffer_sse_for_breakpoint {
                warn!(
                    "[{}] Breakpoint: skipping SSE response hook because content length is unknown or exceeds the breakpoint body limit",
                    req_id
                );
            }

            if can_buffer_sse_for_breakpoint {
                let res_body = res_body_incoming.take().unwrap();
                let collected = res_body.collect().await;
                if let Ok(collected) = collected {
                    let res_bytes = collected.to_bytes();
                    let mut final_body = res_bytes.clone();
                    let pause_event_count = parse_and_record_sse_events(&final_body);

                    if let Some(ref state) = admin_state {
                        state.sse_hub.set_closed(&record_id);
                        state.sse_hub.unregister(&record_id);

                        let pause_content_type = res_parts
                            .headers
                            .get(hyper::header::CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let pause_body_ref = if state.get_super_performance_mode() {
                            None
                        } else {
                            state.body_store.as_ref().and_then(|body_store| {
                                let store = body_store.read();
                                store.store(req_id, "sse_raw", final_body.as_ref())
                            })
                        };
                        let pause_derived_body_ref = if state.get_super_performance_mode() {
                            None
                        } else {
                            state.body_store.as_ref().and_then(|body_store| {
                                bifrost_admin::assemble_openai_like_response_body_from_text(
                                    std::str::from_utf8(&final_body).ok()?,
                                )
                                .and_then(|assembled| {
                                    body_store.read().store(
                                        req_id,
                                        "res_openai_like",
                                        assembled.as_bytes(),
                                    )
                                })
                            })
                        };
                        let pause_response_size = final_body.len();
                        let pause_total_ms = total_ms;

                        state.update_traffic_by_id(&record_id, move |record| {
                            record.status = res_parts.status.as_u16();
                            record.content_type = pause_content_type.clone();
                            record.response_size = pause_response_size;
                            record.download_bytes = pause_response_size;
                            record.duration_ms = record.duration_ms.max(pause_total_ms);
                            record.response_body_ref = pause_body_ref.clone();
                            record.derived_response_body_ref = pause_derived_body_ref.clone();
                            record.frame_count = pause_event_count;
                            record.last_frame_id = pause_event_count as u64;
                            record.socket_status = Some(bifrost_admin::SocketStatus {
                                is_open: false,
                                send_count: 0,
                                receive_count: pause_event_count as u64,
                                send_bytes: 0,
                                receive_bytes: pause_response_size as u64,
                                frame_count: pause_event_count,
                                close_code: None,
                                close_reason: Some("SSE stream completed".to_string()),
                            });
                        });
                    }

                    let outcome = super::breakpoint::breakpoint_response_hook(
                        &admin_state,
                        &push_manager,
                        req_id,
                        &method_str,
                        &original_uri,
                        res_parts.status.as_u16(),
                        &mut res_parts.headers,
                        final_body.clone(),
                        Some(final_body.len()),
                        false,
                        &mut final_body,
                    )
                    .await;

                    #[rustfmt::skip] let no_body = apply_edited_response_status_and_body(&mut res_parts, &method_str, outcome.status, &mut final_body);
                    if !no_body && outcome.body_replaced {
                        normalize_res_headers(
                            &mut res_parts,
                            buffered_res_body_mode(
                                final_body.len(),
                                !resolved_rules.trailers.is_empty(),
                            ),
                            &method_str,
                        );
                    }

                    if let Some(ref state) = admin_state {
                        let mut record = TrafficRecord::new(
                            record_id.clone(),
                            method_str.clone(),
                            original_uri.clone(),
                        );
                        attach_devtools_client_req_id(&mut record, &devtools_client_req_id);
                        record.status = res_parts.status.as_u16();
                        record.content_type = res_parts
                            .headers
                            .get(hyper::header::CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        record.response_size = final_body.len();
                        record.download_bytes = final_body.len();
                        record.duration_ms = total_ms;
                        record.host = original_host.to_string();
                        record.timing = Some(RequestTiming {
                            dns_ms,
                            connect_ms: None,
                            tls_ms,
                            send_ms: None,
                            wait_ms: Some(wait_ms),
                            first_byte_ms: Some(total_ms),
                            receive_ms: Some(0u64),
                            total_ms,
                        });
                        record.request_headers = Some(final_req_headers.clone());
                        record.original_response_headers = Some(original_res_headers.clone());
                        if !body_bytes.is_empty() {
                            store_request_body(
                                &admin_state,
                                req_id,
                                &body_bytes,
                                final_req_content_encoding.as_deref(),
                            )
                            .apply_to(&mut record);
                        } else if let Some(ref capture) = req_body_capture {
                            capture.apply_to(&mut record);
                        }
                        if !state.get_super_performance_mode() {
                            if let Some(ref body_store) = state.body_store {
                                let store = body_store.read();
                                record.response_body_ref =
                                    store.store(req_id, "sse_raw", final_body.as_ref());
                                if let Ok(body_text) = std::str::from_utf8(&final_body) {
                                    record.derived_response_body_ref =
                                    bifrost_admin::assemble_openai_like_response_body_from_text(
                                        body_text,
                                    )
                                    .and_then(|assembled| {
                                        store.store(req_id, "res_openai_like", assembled.as_bytes())
                                    });
                                }
                            }
                        }
                        apply_listener_context(
                            &mut record,
                            listener_port,
                            &client_ip,
                            &client_app,
                            client_pid,
                            &client_path,
                            &account_name,
                        );
                        record.has_rule_hit = has_rules;
                        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                        record.set_sse();
                        if let Some(ref mut status) = record.socket_status {
                            status.receive_bytes = final_body.len() as u64;
                            let event_count = std::str::from_utf8(&final_body)
                                .map(|s| {
                                    s.split("\n\n")
                                        .filter(|chunk| !chunk.trim().is_empty())
                                        .count() as u64
                                })
                                .unwrap_or(0);
                            status.receive_count = event_count;
                            status.frame_count = event_count as usize;
                        }
                        state.update_traffic_by_id(&record_id, move |r| {
                            *r = record.clone();
                        });
                    }

                    let response_body = wrap_throttled_body(
                        full_body(final_body.to_vec()),
                        resolved_rules.res_speed,
                    );
                    let body = with_trailers(response_body, &resolved_rules);
                    return Ok(Response::from_parts(res_parts, body));
                }
            }

            let res_body = res_body_incoming.take().unwrap().boxed();
            let res_body = if response_resolved.res_stream_scripts.is_empty() {
                res_body
            } else {
                let worker = match initialize_response_stream_script(
                    &admin_state,
                    &response_resolved.res_stream_scripts,
                    &ctx,
                    &response_resolved,
                    &original_uri,
                    &method_str,
                    &header_pairs_to_hashmap(&final_req_headers),
                    res_parts.status.as_u16(),
                    res_parts
                        .status
                        .canonical_reason()
                        .unwrap_or("OK")
                        .to_string(),
                    header_map_to_hashmap(&res_parts.headers),
                    &values,
                )
                .await
                {
                    Ok(worker) => worker,
                    Err(error) => {
                        if let Some(ref state) = admin_state {
                            state.sse_hub.unregister(&record_id);
                            state.update_traffic_by_id(&record_id, |record| {
                                record.status = hyper::StatusCode::BAD_GATEWAY.as_u16();
                            });
                        }
                        return Ok(Response::builder()
                            .status(hyper::StatusCode::BAD_GATEWAY)
                            .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                            .body(full_body(format!(
                                "stream script initialization failed: {error}"
                            )))
                            .unwrap());
                    }
                };
                configure_stream_script_response_headers(&mut res_parts.headers);
                create_response_stream_script_body(Some(res_body), worker)
            };
            let tee_body = create_sse_tee_body(
                res_body,
                admin_state.clone(),
                record_id,
                SseTeeOptions {
                    traffic_type: Some(traffic_type),
                    file_writer: sse_stream_writer,
                    content_encoding: res_content_encoding.clone(),
                    max_buffer_size: max_body_buffer_size,
                    max_decompress_output_bytes,
                },
            );
            let final_body = wrap_throttled_body(tee_body.boxed(), resolved_rules.res_speed);
            let body = with_trailers(final_body, &resolved_rules);
            return Ok(Response::from_parts(res_parts, body));
        } else {
            let res_body = res_body_stream.take().unwrap();
            let tee_body = if skip_binary_recording {
                create_metrics_body(res_body, admin_state.clone(), Some(traffic_type))
            } else {
                let response_headers_size =
                    calculate_response_headers_size(res_parts.status.as_u16(), &res_headers);
                create_tee_body_with_store(
                    res_body,
                    admin_state.clone(),
                    record_id,
                    TeeBodyCaptureOptions {
                        max_body_size: Some(max_body_buffer_size),
                        content_encoding: res_content_encoding.clone(),
                        traffic_type: Some(traffic_type),
                        monitor_connection: false,
                        response_headers_size,
                    },
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
        let res_body_bytes = match res_body.collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(e) => {
                error!("[{}] Failed to read response body: {}", req_id, e);
                return Ok(Response::builder()
                    .status(502)
                    .body(full_body(b"Bad Gateway".to_vec()))
                    .unwrap());
            }
        };
        let receive_ms = receive_start.elapsed().as_millis() as u64;
        (res_body_bytes, receive_ms)
    } else {
        (Vec::new(), 0)
    };
    let original_res_body_len = res_content_length.unwrap_or(res_body_bytes.len());

    let total_ms = start_time.elapsed().as_millis() as u64;

    if let Some(ref state) = admin_state {
        let traffic_type = if is_websocket {
            TrafficType::Wss
        } else {
            TrafficType::Https
        };
        state
            .metrics_collector
            .add_bytes_sent_by_type(traffic_type, request_body_size as u64);
        state
            .metrics_collector
            .add_bytes_received_by_type(traffic_type, original_res_body_len as u64);
        state
            .metrics_collector
            .increment_requests_by_type(traffic_type);

        let mut record =
            TrafficRecord::new(req_id.to_string(), method_str.clone(), original_uri.clone());
        attach_devtools_client_req_id(&mut record, &devtools_client_req_id);
        record.status = res_parts.status.as_u16();
        record.content_type = res_parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let res_headers: Vec<(String, String)> = res_parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        record.request_size =
            calculate_request_size(&method_str, &original_uri, &req_headers, request_body_size);
        record.upload_bytes = request_body_size;
        record.response_size = calculate_response_size(
            res_parts.status.as_u16(),
            &res_headers,
            original_res_body_len,
        );
        record.download_bytes = original_res_body_len;
        record.duration_ms = total_ms;
        record.host = original_host.to_string();
        record.timing = Some(RequestTiming {
            dns_ms,
            connect_ms: None,
            tls_ms,
            send_ms: None,
            wait_ms: Some(wait_ms),
            first_byte_ms: Some(total_ms),
            receive_ms: Some(receive_ms),
            total_ms,
        });
        record.request_headers = Some(final_req_headers.clone());
        record.original_response_headers = Some(original_res_headers.clone());
        if res_headers != original_res_headers {
            record.response_headers = Some(res_headers.clone());
        }
        if !super::headers_pairs_equal_ignore_order(&original_req_headers, &final_req_headers) {
            record.original_request_headers = Some(original_req_headers.clone());
        }
        if actual_target_host != original_host
            || actual_target_port != original_port
            || actual_target_path != path
        {
            let actual_scheme = if actual_use_http { "http" } else { "https" };
            let actual_url = if (actual_use_http && actual_target_port == 80)
                || (!actual_use_http && actual_target_port == 443)
            {
                format!(
                    "{}://{}{}",
                    actual_scheme, actual_target_host, actual_target_path
                )
            } else {
                format!(
                    "{}://{}:{}{}",
                    actual_scheme, actual_target_host, actual_target_port, actual_target_path
                )
            };
            record.actual_url = Some(actual_url);
            record.actual_host = Some(actual_target_host.clone());
        }
        record.request_content_type = final_req_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone());
        apply_listener_context(
            &mut record,
            listener_port,
            &client_ip,
            &client_app,
            client_pid,
            &client_path,
            &account_name,
        );

        if is_websocket {
            record.protocol = "wss".to_string();
        }

        if is_sse {
            record.set_sse();
            state.sse_hub.register(req_id);
        }

        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
        if !req_script_results.is_empty() {
            record.req_script_results = Some(req_script_results.clone());
        }

        if !body_bytes.is_empty() {
            store_request_body(
                &admin_state,
                req_id,
                &body_bytes,
                final_req_content_encoding.as_deref(),
            )
            .apply_to(&mut record);
        } else if let Some(ref capture) = req_body_capture {
            capture.apply_to(&mut record);
        }

        if !state.get_super_performance_mode() {
            if let Some(ref body_store) = state.body_store {
                let max_decompress_output_bytes = if let Some(cm) = state.config_manager.as_ref() {
                    cm.config().await.sandbox.limits.max_decompress_output_bytes
                } else {
                    10 * 1024 * 1024
                };

                let store = body_store.read();
                let decompressed_res_body = crate::transform::decompress_body_with_limit(
                    &res_body_bytes,
                    res_content_encoding.as_deref(),
                    max_decompress_output_bytes,
                );
                record.response_body_ref =
                    store.store(req_id, "res", decompressed_res_body.as_ref());
            }
        }

        state.record_traffic(record);
    }

    if let Some(delay_ms) = resolved_rules.res_delay {
        if verbose_logging {
            info!("[{}] [RES_DELAY] Sleeping {}ms", req_id, delay_ms);
        }
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }

    if let Some(speed) = resolved_rules.res_speed {
        if verbose_logging {
            info!("[{}] [RES_SPEED] Speed limit: {} bytes/s", req_id, speed);
        }
    }

    let res_content_type = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let res_content_type = res_content_type.unwrap_or("").to_string();
    let (body_for_injection, injection_source_encoding, injection_output_encoding) =
        if has_response_body_rules(&response_resolved) {
            let body_rule_input = response_resolved
                .res_body
                .clone()
                .unwrap_or_else(|| Bytes::from(res_body_bytes.clone()));
            let body_processed = apply_body_rules_preserving_encoding(
                body_rule_input,
                &response_resolved,
                Phase::Response,
                Some(&res_content_type),
                ContentInjectionEncoding {
                    source: if response_resolved.res_body.is_some() {
                        None
                    } else {
                        res_content_encoding.as_deref()
                    },
                    output: output_res_content_encoding.as_deref(),
                    max_decompress_output_bytes,
                },
                verbose_logging,
                &res_ctx,
            );
            (
                body_processed.body,
                body_processed.content_encoding.clone(),
                body_processed.content_encoding,
            )
        } else {
            (
                Bytes::from(res_body_bytes.clone()),
                res_content_encoding.clone(),
                output_res_content_encoding.clone(),
            )
        };
    let injection_result = apply_content_injection_preserving_encoding(
        body_for_injection,
        &res_content_type,
        ContentInjectionEncoding {
            source: injection_source_encoding.as_deref(),
            output: injection_output_encoding.as_deref(),
            max_decompress_output_bytes,
        },
        &resolved_rules,
        verbose_logging,
        &ctx,
    );
    set_content_encoding_header(
        &mut res_parts.headers,
        injection_result.content_encoding.as_deref(),
    );
    let final_body = injection_result.body;

    let mut final_body = if devtools_bridge_requested(&resolved_rules)
        && res_content_type
            .to_ascii_lowercase()
            .starts_with("text/html")
    {
        res_parts.headers.insert(
            hyper::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
        );
        res_parts
            .headers
            .insert(hyper::header::PRAGMA, HeaderValue::from_static("no-cache"));
        let final_res_headers: Vec<(String, String)> = res_parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        if let Some(content_encoding) = get_content_encoding(&final_res_headers) {
            match crate::transform::try_decompress_body_with_limit(
                final_body.as_ref(),
                &content_encoding,
                10 * 1024 * 1024,
            ) {
                Ok(decompressed) => {
                    let injected_body = maybe_inject_devtools_bridge_html(
                        Bytes::from(decompressed),
                        &res_content_type,
                        &resolved_rules,
                        admin_state.as_deref(),
                        &original_uri,
                        req_id,
                    );
                    match compress_body(injected_body.as_ref(), &content_encoding) {
                        Ok(compressed) => Bytes::from(compressed),
                        Err(_) => {
                            res_parts.headers.remove(hyper::header::CONTENT_ENCODING);
                            injected_body
                        }
                    }
                }
                Err(_) => final_body,
            }
        } else {
            maybe_inject_devtools_bridge_html(
                final_body,
                &res_content_type,
                &resolved_rules,
                admin_state.as_deref(),
                &original_uri,
                req_id,
            )
        }
    } else {
        final_body
    };

    let res_script_results = if has_res_scripts {
        let mut res_script_status = res_parts.status.as_u16();
        let mut res_script_status_text = res_parts
            .status
            .canonical_reason()
            .unwrap_or("OK")
            .to_string();
        let mut res_script_headers = header_map_to_hashmap(&res_parts.headers);
        let original_script_headers = res_script_headers.clone();
        let current_res_headers: Vec<(String, String)> = res_parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let mut res_script_body = body_to_script_string(
            &final_body,
            get_content_encoding(&current_res_headers).as_deref(),
            max_decompress_output_bytes,
        );
        let req_script_headers = header_pairs_to_hashmap(&final_req_headers);

        let results = execute_response_scripts(
            &admin_state,
            &resolved_rules.res_scripts,
            &ctx,
            &resolved_rules,
            &original_uri,
            &method_str,
            &req_script_headers,
            None,
            &mut res_script_status,
            &mut res_script_status_text,
            &mut res_script_headers,
            &mut res_script_body,
            &values,
        )
        .await;

        if results.iter().any(|result| result.success) {
            if let Ok(new_status) = hyper::StatusCode::from_u16(res_script_status) {
                res_parts.status = new_status;
            }

            res_parts.headers = apply_script_headers_to_header_map(
                &res_parts.headers,
                &original_script_headers,
                &res_script_headers,
            );

            if let Some(ref new_body) = res_script_body {
                let current_res_headers: Vec<(String, String)> = res_parts
                    .headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                let encoded = script_string_to_body(
                    new_body,
                    get_content_encoding(&current_res_headers).as_deref(),
                );
                set_content_encoding_header(
                    &mut res_parts.headers,
                    encoded.content_encoding.as_deref(),
                );
                final_body = encoded.body;
            }
        }

        results
    } else {
        Vec::new()
    };

    let mut final_body = if inject_bifrost_badge {
        let badge_rules_json = match error_badge_rules_json.as_ref() {
            Some(rules_json) => rules_json.clone(),
            None => {
                super::handler::build_badge_rules_json(admin_state.as_deref(), listener_port).await
            }
        };
        let final_res_content_type = res_parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        if final_res_content_type.starts_with("text/html") {
            if let Some(content_encoding) = response_content_encoding(&res_parts) {
                match crate::transform::try_decompress_body_with_limit(
                    final_body.as_ref(),
                    &content_encoding,
                    10 * 1024 * 1024,
                ) {
                    Ok(decompressed) => {
                        let (injected_body, injected) = maybe_inject_bifrost_badge_html(
                            Bytes::from(decompressed),
                            &badge_rules_json,
                        );
                        if injected {
                            match compress_body(injected_body.as_ref(), &content_encoding) {
                                Ok(compressed) => Bytes::from(compressed),
                                Err(_) => {
                                    res_parts.headers.remove(hyper::header::CONTENT_ENCODING);
                                    injected_body
                                }
                            }
                        } else {
                            final_body
                        }
                    }
                    Err(_) => final_body,
                }
            } else {
                let (injected_body, injected) =
                    maybe_inject_bifrost_badge_html(final_body.clone(), &badge_rules_json);
                if injected {
                    injected_body
                } else {
                    final_body
                }
            }
        } else {
            final_body
        }
    } else {
        final_body
    };

    if let Some(ref state) = admin_state {
        if response_breakpoint_enabled {
            let final_status = res_parts.status.as_u16();
            let final_content_type = res_parts
                .headers
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let final_res_headers: Vec<(String, String)> = res_parts
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            let final_response_size =
                calculate_response_size(final_status, &final_res_headers, final_body.len());
            let pause_total_ms = start_time.elapsed().as_millis() as u64;
            let original_res_headers_for_pause = original_res_headers.clone();
            let pause_body_ref = if state.get_super_performance_mode() {
                None
            } else {
                state.body_store.as_ref().and_then(|body_store| {
                    let max_decompress_output_bytes = state
                        .config_manager
                        .as_ref()
                        .and_then(|cm| cm.try_config())
                        .map(|cfg| cfg.sandbox.limits.max_decompress_output_bytes)
                        .unwrap_or(10 * 1024 * 1024);
                    let store = body_store.read();
                    let decompressed_res = crate::transform::decompress_body_with_limit(
                        &final_body,
                        response_content_encoding(&res_parts).as_deref(),
                        max_decompress_output_bytes,
                    );
                    store.store(req_id, "res", decompressed_res.as_ref())
                })
            };

            let pause_download_bytes = final_body.len();
            state.update_traffic_by_id(req_id, move |record| {
                record.status = final_status;
                record.content_type = final_content_type.clone();
                record.response_size = final_response_size;
                record.download_bytes = pause_download_bytes;
                record.duration_ms = record.duration_ms.max(pause_total_ms);
                record.response_headers = if final_res_headers != original_res_headers_for_pause {
                    Some(final_res_headers.clone())
                } else {
                    None
                };
                record.response_body_ref = pause_body_ref.clone();
                if let Some(ref mut timing) = record.timing {
                    timing.total_ms = record.duration_ms;
                    if timing.first_byte_ms.is_none() {
                        timing.first_byte_ms = Some(record.duration_ms);
                    }
                }
            });
        }
    }

    let response_hook_enabled = response_breakpoint_enabled;
    if response_hook_enabled {
        let outcome = super::breakpoint::breakpoint_response_hook(
            &admin_state,
            &push_manager,
            req_id,
            &method_str,
            &original_uri,
            res_parts.status.as_u16(),
            &mut res_parts.headers,
            final_body.clone(),
            Some(final_body.len()),
            false,
            &mut final_body,
        )
        .await;
        #[rustfmt::skip] apply_edited_response_status_and_body(&mut res_parts, &method_str, outcome.status, &mut final_body);
    }

    normalize_res_headers(
        &mut res_parts,
        buffered_res_body_mode(final_body.len(), !resolved_rules.trailers.is_empty()),
        &method_str,
    );
    if verbose_logging && original_res_body_len != final_body.len() {
        info!(
            "[{}] Updated Content-Length: {} -> {}",
            req_id,
            original_res_body_len,
            final_body.len()
        );
    }

    if !res_script_results.is_empty() {
        let final_status = res_parts.status.as_u16();
        let final_content_type = res_parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let final_res_headers: Vec<(String, String)> = res_parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let final_response_size =
            calculate_response_size(final_status, &final_res_headers, final_body.len());
        let original_res_headers_for_update = original_res_headers.clone();
        let res_script_results_for_update = res_script_results.clone();
        let final_download_bytes = final_body.len();
        let final_is_sse = res_parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
        let final_sse_event_count = if final_is_sse {
            parse_and_record_sse_events(&final_body)
        } else {
            0
        };
        if let Some(ref state) = admin_state {
            state.update_traffic_by_id(req_id, move |record| {
                record.status = final_status;
                record.content_type = final_content_type.clone();
                record.response_size = final_response_size;
                record.download_bytes = final_download_bytes;
                record.response_headers = if final_res_headers != original_res_headers_for_update {
                    Some(final_res_headers.clone())
                } else {
                    None
                };
                record.res_script_results = Some(res_script_results_for_update.clone());
                if final_is_sse {
                    record.set_sse();
                    record.frame_count = final_sse_event_count;
                    record.last_frame_id = final_sse_event_count as u64;
                    record.socket_status = Some(bifrost_admin::SocketStatus {
                        is_open: false,
                        send_count: 0,
                        receive_count: final_sse_event_count as u64,
                        send_bytes: 0,
                        receive_bytes: final_download_bytes as u64,
                        frame_count: final_sse_event_count,
                        close_code: None,
                        close_reason: Some("SSE stream completed".to_string()),
                    });
                }
            });
        }
    }

    if let Some(ref state) = admin_state {
        if !state.get_super_performance_mode() {
            if let Some(ref body_store) = state.body_store {
                let max_decompress_output_bytes = if let Some(cm) = state.config_manager.as_ref() {
                    cm.config().await.sandbox.limits.max_decompress_output_bytes
                } else {
                    10 * 1024 * 1024
                };

                let final_req_content_encoding = get_content_encoding(&final_req_headers);
                let decompressed_req = crate::transform::decompress_body_with_limit(
                    &Bytes::from(body_bytes.clone()),
                    final_req_content_encoding.as_deref(),
                    max_decompress_output_bytes,
                );
                let raw_req_body = decompressed_req.clone();
                let req_headers_hashmap = header_pairs_to_hashmap(&final_req_headers);
                let (req_host, req_path, req_proto) = parse_url_parts(&original_uri);
                let request_data = RequestData {
                    url: original_uri.clone(),
                    method: actual_method.to_string(),
                    host: req_host,
                    path: req_path,
                    protocol: req_proto,
                    client_ip: client_ip.clone(),
                    client_app: client_app.clone(),
                    headers: req_headers_hashmap,
                    body: None,
                };

                let final_res_content_encoding = response_content_encoding(&res_parts);
                let decompressed_res = crate::transform::decompress_body_with_limit(
                    &final_body,
                    final_res_content_encoding.as_deref(),
                    max_decompress_output_bytes,
                );
                let raw_res_body = decompressed_res.clone();
                let final_res_headers: Vec<(String, String)> = res_parts
                    .headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                let response_data = ResponseData {
                    status: res_parts.status.as_u16(),
                    status_text: res_parts
                        .status
                        .canonical_reason()
                        .unwrap_or("OK")
                        .to_string(),
                    headers: header_pairs_to_hashmap(&final_res_headers),
                    body: None,
                    request: request_data.clone(),
                };

                let decoded_req_body = apply_decode_scripts_for_storage(
                    &admin_state,
                    &resolved_rules.decode_scripts,
                    "request",
                    &ctx,
                    &resolved_rules,
                    &request_data,
                    &response_data,
                    &values,
                    decompressed_req,
                )
                .await;
                let DecodeForStorageResult {
                    output: decoded_req_output,
                    results: decoded_req_results,
                } = decoded_req_body;

                let decoded_res_body = apply_decode_scripts_for_storage(
                    &admin_state,
                    &resolved_rules.decode_scripts,
                    "response",
                    &ctx,
                    &resolved_rules,
                    &request_data,
                    &response_data,
                    &values,
                    decompressed_res,
                )
                .await;
                let DecodeForStorageResult {
                    output: decoded_res_output,
                    results: decoded_res_results,
                } = decoded_res_body;

                let store = body_store.read();
                let raw_request_body_ref = if has_decode_scripts && !raw_req_body.is_empty() {
                    store.store(req_id, "req_raw", raw_req_body.as_ref())
                } else {
                    None
                };
                let raw_response_body_ref = if has_decode_scripts && !raw_res_body.is_empty() {
                    store.store(req_id, "res_raw", raw_res_body.as_ref())
                } else {
                    None
                };
                let request_body_ref = if !decoded_req_output.is_empty() {
                    store.store(req_id, "req", decoded_req_output.as_ref())
                } else {
                    None
                };
                let final_is_sse = res_parts
                    .headers
                    .get(hyper::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| {
                        value.to_ascii_lowercase().starts_with("text/event-stream")
                    });
                let response_body_ref = store.store(
                    req_id,
                    if final_is_sse { "sse_raw" } else { "res" },
                    decoded_res_output.as_ref(),
                );
                let derived_response_body_ref = final_is_sse
                    .then(|| std::str::from_utf8(decoded_res_output.as_ref()).ok())
                    .flatten()
                    .and_then(bifrost_admin::assemble_openai_like_response_body_from_text)
                    .and_then(|assembled| {
                        store.store(req_id, "res_openai_like", assembled.as_bytes())
                    });
                let has_decode_scripts_for_update = has_decode_scripts;
                state.update_traffic_by_id(req_id, move |record| {
                    if let Some(body_ref) = request_body_ref.clone() {
                        record.request_body_ref = Some(body_ref);
                    }
                    if let Some(body_ref) = response_body_ref.clone() {
                        record.response_body_ref = Some(body_ref);
                    }
                    if let Some(body_ref) = derived_response_body_ref.clone() {
                        record.derived_response_body_ref = Some(body_ref);
                    }
                    if has_decode_scripts_for_update {
                        record.raw_request_body_ref = raw_request_body_ref.clone();
                        record.raw_response_body_ref = raw_response_body_ref.clone();
                        if !decoded_req_results.is_empty() {
                            record.decode_req_script_results = Some(decoded_req_results.clone());
                        }
                        if !decoded_res_results.is_empty() {
                            record.decode_res_script_results = Some(decoded_res_results.clone());
                        }
                    }
                });
            }
        }
    }

    let downstream_first_byte_ms = start_time.elapsed().as_millis() as u64;
    if let Some(ref state) = admin_state {
        state.update_traffic_by_id(req_id, move |record| {
            record.duration_ms = record.duration_ms.max(downstream_first_byte_ms);
            if let Some(ref mut timing) = record.timing {
                timing.first_byte_ms = Some(downstream_first_byte_ms);
                timing.total_ms = record.duration_ms;
                if timing.receive_ms.is_some() {
                    timing.receive_ms =
                        Some(record.duration_ms.saturating_sub(downstream_first_byte_ms));
                }
            }
        });
    }

    let response_body =
        wrap_throttled_body(full_body(final_body.to_vec()), resolved_rules.res_speed);
    let body = with_trailers(response_body, &resolved_rules);
    Ok(Response::from_parts(res_parts, body))
}

#[allow(clippy::too_many_arguments)]
async fn handle_intercepted_websocket(
    mut req: Request<Incoming>,
    original_host: &str,
    original_port: u16,
    req_id: &str,
    admin_state: Option<Arc<AdminState>>,
    rules: Arc<dyn RulesResolver>,
    verbose_logging: bool,
    unsafe_ssl: bool,
    client_ip: String,
    client_app: Option<String>,
    client_pid: Option<u32>,
    client_path: Option<String>,
    account_name: Option<String>,
    listener_port: u16,
    push_manager: Option<SharedPushManager>,
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    if original_host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
        if let Some(state) = admin_state.clone() {
            let req = rewrite_intercepted_virtual_host_request(req);
            let resp = AdminRouter::handle(req, state, push_manager, None).await;
            return Ok(convert_intercepted_admin_response(resp));
        }
    }

    use tokio_rustls::rustls::pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    let start_time = Instant::now();
    let devtools_client_req_id_from_uri = take_devtools_client_req_id_from_uri(req.uri_mut());
    let devtools_client_req_id =
        take_devtools_client_req_id(req.headers_mut()).or(devtools_client_req_id_from_uri);
    let is_h2_websocket_connect = req.version() == hyper::Version::HTTP_2
        && req.method() == hyper::Method::CONNECT
        && req
            .extensions()
            .get::<hyper::ext::Protocol>()
            .is_some_and(|protocol| protocol.as_str().eq_ignore_ascii_case("websocket"));
    let uri = req.uri().clone();
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let method_str = "GET".to_string();

    if verbose_logging {
        info!("[{}] WebSocket upgrade request detected: {}", req_id, path);
    }

    let original_uri = format!("wss://{}{}", original_host, path);
    let incoming_headers: std::collections::HashMap<String, String> = req
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string().to_lowercase(),
                v.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    let incoming_cookies: std::collections::HashMap<String, String> =
        collect_all_cookies_from_headers(req.headers());

    let mut resolved_rules = rules.resolve_with_context(
        &original_uri,
        &method_str,
        &incoming_headers,
        &incoming_cookies,
    );
    let candidate_urls = vec![
        format!("https://{}{}", original_host, path),
        original_host.to_string(),
        format!("{}:{}", original_host, original_port),
    ];
    resolved_rules = merge_websocket_header_rule_candidates(
        resolved_rules,
        rules.as_ref(),
        &candidate_urls,
        &method_str,
        &incoming_headers,
        &incoming_cookies,
    );

    let has_rules = !resolved_rules.rules.is_empty()
        || resolved_rules.host.is_some()
        || !resolved_rules.req_headers.is_empty()
        || !resolved_rules.res_headers.is_empty()
        || !resolved_rules.delete_req_headers.is_empty()
        || !resolved_rules.delete_res_headers.is_empty()
        || !resolved_rules.header_replace.is_empty();

    let (target_host, target_port, use_http, target_path) = if resolved_rules.ignored.host {
        debug!(
            "[{}] [WS] Passthrough rule applied: WebSocket will be forwarded to original target {}:{}",
            req_id, original_host, original_port
        );
        (
            original_host.to_string(),
            original_port,
            false,
            path.to_string(),
        )
    } else if let Some(ref host_rule) = resolved_rules.host {
        let (h, parsed_port, parsed_path) = match parse_host_rule(host_rule) {
            Some((h, p, path_and_query)) => (h, p, path_and_query),
            None => (host_rule.trim_end_matches('/').to_string(), None, None),
        };
        let p = parsed_port.unwrap_or(match resolved_rules.host_protocol {
            Some(Protocol::Http) | Some(Protocol::Ws) => 80,
            Some(Protocol::Https) | Some(Protocol::Wss) => 443,
            _ => original_port,
        });
        let use_http_flag = match resolved_rules.host_protocol {
            Some(Protocol::Http) | Some(Protocol::Ws) => true,
            Some(Protocol::Host) | Some(Protocol::XHost) => p != 443 && p != 8443,
            _ => false,
        };
        let target_path = if let Some(ref rule_path) = parsed_path {
            let host_protocol = resolved_rules.host_protocol.unwrap_or(Protocol::Host);
            if crate::utils::url::host_rule_uses_exact_target_path(
                &resolved_rules.rules,
                host_protocol,
                host_rule,
            ) {
                rule_path.clone()
            } else {
                let source_path = crate::utils::url::find_host_rule_source_path(
                    &resolved_rules.rules,
                    host_protocol,
                    host_rule,
                );
                crate::utils::url::rewrite_path_with_prefix(path, source_path.as_deref(), rule_path)
            }
        } else {
            path.to_string()
        };
        if verbose_logging {
            info!(
                "[{}] [WS] WebSocket target resolved: wss://{}:{} -> {}://{}:{}",
                req_id,
                original_host,
                original_port,
                if use_http_flag { "ws" } else { "wss" },
                h,
                p
            );
        }
        (h, p, use_http_flag, target_path)
    } else {
        (
            original_host.to_string(),
            original_port,
            false,
            path.to_string(),
        )
    };

    apply_websocket_request_header_rules(req.headers_mut(), &resolved_rules);

    let connect_start = Instant::now();
    let target_stream = match connect_tcp(format!("{}:{}", target_host, target_port)).await {
        Ok(s) => s,
        Err(e) => {
            error!(
                "[{}] Failed to connect to WebSocket target {}:{}: {}",
                req_id, target_host, target_port, e
            );
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"Bad Gateway".to_vec()))
                .unwrap());
        }
    };
    let tcp_connect_ms = connect_start.elapsed().as_millis() as u64;
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

    let upstream_handshake = if use_http {
        let stream: Box<dyn AsyncReadWrite + Send + Unpin> = Box::new(target_stream);
        let handshake =
            build_websocket_handshake_request(&req, &target_host, target_port, &target_path);
        let (mut stream_read, mut stream_write) = tokio::io::split(stream);

        if let Err(e) = stream_write.write_all(handshake.as_bytes()).await {
            error!("[{}] Failed to send WebSocket handshake: {}", req_id, e);
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"Bad Gateway".to_vec()))
                .unwrap());
        }

        let (upstream_resp, upstream_leftover) = match read_http1_response_with_leftover(
            &mut stream_read,
            websocket_handshake_max_header_size,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                error!(
                    "[{}] Failed to read WebSocket handshake response: {}",
                    req_id, e
                );
                return Ok(Response::builder()
                    .status(502)
                    .body(full_body(b"Bad Gateway".to_vec()))
                    .unwrap());
            }
        };

        if let Some(response) = websocket_rejection_response(req_id, &target_host, &upstream_resp) {
            return Ok(response);
        }

        UpstreamWebSocketHandshake {
            stream: Box::new(stream_read.unsplit(stream_write)),
            leftover: upstream_leftover,
            headers: upstream_resp.headers.clone(),
            sec_accept: upstream_resp
                .header("Sec-WebSocket-Accept")
                .map(|v| v.to_string()),
            protocol: upstream_resp
                .header("Sec-WebSocket-Protocol")
                .map(ToOwned::to_owned),
            extensions: header_values(&upstream_resp, "Sec-WebSocket-Extensions"),
        }
    } else {
        // Real-world WSS endpoints commonly expect the classic HTTP/1.1 Upgrade flow even when
        // the TLS endpoint also advertises h2. Forcing HTTP/1.1 here matches browser behavior
        // more closely and avoids hanging on servers that do not implement RFC 8441.
        let tls_config = get_tls_client_config_http1_only(unsafe_ssl);
        let connector = TlsConnector::from(tls_config);

        let server_name = match ServerName::try_from(target_host.to_string()) {
            Ok(name) => name,
            Err(_) => {
                error!("[{}] Invalid server name for TLS: {}", req_id, target_host);
                return Ok(Response::builder()
                    .status(502)
                    .body(full_body(b"Bad Gateway".to_vec()))
                    .unwrap());
            }
        };

        let tls_stream = match connector.connect(server_name, target_stream).await {
            Ok(tls_stream) => tls_stream,
            Err(e) => {
                error!("[{}] TLS handshake failed: {}", req_id, e);
                return Ok(Response::builder()
                    .status(502)
                    .body(full_body(b"Bad Gateway".to_vec()))
                    .unwrap());
            }
        };

        let stream: Box<dyn AsyncReadWrite + Send + Unpin> = Box::new(tls_stream);
        let handshake =
            build_websocket_handshake_request(&req, &target_host, target_port, &target_path);
        let (mut stream_read, mut stream_write) = tokio::io::split(stream);

        if let Err(e) = stream_write.write_all(handshake.as_bytes()).await {
            error!("[{}] Failed to send WebSocket handshake: {}", req_id, e);
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"Bad Gateway".to_vec()))
                .unwrap());
        }

        let (upstream_resp, upstream_leftover) = match read_http1_response_with_leftover(
            &mut stream_read,
            websocket_handshake_max_header_size,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                error!(
                    "[{}] Failed to read WebSocket handshake response: {}",
                    req_id, e
                );
                return Ok(Response::builder()
                    .status(502)
                    .body(full_body(b"Bad Gateway".to_vec()))
                    .unwrap());
            }
        };

        if let Some(response) = websocket_rejection_response(req_id, &target_host, &upstream_resp) {
            return Ok(response);
        }

        UpstreamWebSocketHandshake {
            stream: Box::new(stream_read.unsplit(stream_write)),
            leftover: upstream_leftover,
            headers: upstream_resp.headers.clone(),
            sec_accept: upstream_resp
                .header("Sec-WebSocket-Accept")
                .map(|v| v.to_string()),
            protocol: upstream_resp
                .header("Sec-WebSocket-Protocol")
                .map(ToOwned::to_owned),
            extensions: header_values(&upstream_resp, "Sec-WebSocket-Extensions"),
        }
    };

    let UpstreamWebSocketHandshake {
        stream,
        leftover: upstream_leftover,
        headers: upstream_headers,
        sec_accept: upstream_sec_accept,
        protocol: upstream_protocol_owned,
        extensions: upstream_extensions,
    } = upstream_handshake;
    let upstream_protocol = upstream_protocol_owned.as_deref();
    let mut passthrough_response_headers = HeaderMap::new();
    for (name, value) in &upstream_headers {
        let lower = name.to_ascii_lowercase();
        if lower != "upgrade"
            && lower != "connection"
            && lower != "sec-websocket-accept"
            && lower != "sec-websocket-protocol"
            && lower != "sec-websocket-extensions"
        {
            if let (Ok(header_name), Ok(header_value)) =
                (name.parse::<HeaderName>(), value.parse::<HeaderValue>())
            {
                passthrough_response_headers.insert(header_name, header_value);
            }
        }
    }
    apply_websocket_response_header_rules(&mut passthrough_response_headers, &resolved_rules);

    let client_protocol = req
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok());
    let client_extensions = req
        .headers()
        .get("Sec-WebSocket-Extensions")
        .and_then(|v| v.to_str().ok());

    let sec_protocol = negotiate_protocol(client_protocol, upstream_protocol);
    let negotiated_extensions = negotiate_extensions(client_extensions, &upstream_extensions);
    let compression_cfg = negotiated_extensions
        .as_deref()
        .and_then(crate::protocol::parse_permessage_deflate_config);
    let _compression_enabled = compression_cfg.is_some();
    let ws_meta = super::ws_decode::WsHandshakeMeta {
        negotiated_protocol: sec_protocol.clone(),
        negotiated_extensions: negotiated_extensions.clone(),
    };
    let sec_accept = if is_h2_websocket_connect {
        upstream_sec_accept
    } else {
        req.headers()
            .get("Sec-WebSocket-Key")
            .and_then(|v| v.to_str().ok())
            .map(crate::protocol::compute_accept_key)
            .or(upstream_sec_accept)
    };

    let total_ms = start_time.elapsed().as_millis() as u64;

    let req_headers: Vec<(String, String)> = super::headers_to_pairs(req.headers());

    if let Some(ref state) = admin_state {
        state
            .metrics_collector
            .increment_requests_by_type(TrafficType::Wss);

        let ws_url = format!("wss://{}{}", original_host, path);
        let mut record = TrafficRecord::new(req_id.to_string(), "GET".to_string(), ws_url);
        attach_devtools_client_req_id(&mut record, &devtools_client_req_id);
        record.status = 101;
        record.protocol = "wss".to_string();
        record.duration_ms = total_ms;
        record.timing = Some(RequestTiming {
            dns_ms: None,
            connect_ms: Some(tcp_connect_ms),
            tls_ms: if use_http {
                None
            } else {
                Some(total_ms.saturating_sub(tcp_connect_ms))
            },
            send_ms: None,
            wait_ms: None,
            first_byte_ms: Some(total_ms),
            receive_ms: None,
            total_ms,
        });
        record.request_headers = Some(req_headers.clone());
        record.request_content_type = req_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone());
        record.host = original_host.to_string();
        apply_listener_context(
            &mut record,
            listener_port,
            &client_ip,
            &client_app,
            client_pid,
            &client_path,
            &account_name,
        );
        record.set_websocket();

        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);

        state.connection_monitor.register_connection(req_id);
        state.record_traffic(record);
    }

    if verbose_logging {
        info!(
            "[{}] WebSocket connection established to {}:{}",
            req_id, target_host, target_port
        );
    }

    let req_id_owned = req_id.to_string();
    let admin_state_clone = admin_state.clone();
    let ws_rules = resolved_rules.clone();
    let ws_req_url = format!("wss://{}{}", original_host, path);
    let ws_req_method = "GET".to_string();
    let ws_req_headers = req_headers.clone();
    let ws_decode_scripts = ws_rules.decode_scripts.clone();
    let ws_ctx = RequestContext::new()
        .with_request_info(
            ws_req_url.clone(),
            ws_req_method.clone(),
            original_host.to_string(),
            path.to_string(),
            String::new(),
            client_ip.clone(),
        )
        .with_client_process(client_app.clone(), client_pid, client_path.clone())
        .with_port(listener_port);

    let ws_compression_cfg = compression_cfg.clone();
    let ws_meta_spawn = ws_meta.clone();
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                if let Err(e) = super::websocket::websocket_bidirectional_generic_with_capture(
                    upgraded,
                    stream,
                    &req_id_owned,
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
                    if verbose_logging {
                        debug!("[{}] WebSocket tunnel closed: {}", req_id_owned, e);
                    }
                }

                if let Some(ref state) = admin_state_clone {
                    state.connection_monitor.set_connection_closed(
                        &req_id_owned,
                        None,
                        None,
                        state.frame_store.as_ref(),
                        state.ws_payload_store.as_ref(),
                    );
                }
            }
            Err(e) => {
                error!("[{}] WebSocket upgrade error: {}", req_id_owned, e);
            }
        }
    });

    let mut response = if is_h2_websocket_connect {
        Response::builder().status(200)
    } else {
        let mut response = Response::builder()
            .status(101)
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade");
        if let Some(accept) = sec_accept {
            response = response.header("Sec-WebSocket-Accept", accept);
        }
        response
    };

    if let Some(protocol) = sec_protocol {
        response = response.header("Sec-WebSocket-Protocol", protocol);
    }

    if let Some(extensions) = negotiated_extensions {
        response = response.header("Sec-WebSocket-Extensions", extensions);
    }

    for (name, value) in passthrough_response_headers {
        if let Some(name) = name {
            response = response.header(name, value);
        }
    }

    Ok(response.body(empty_body()).unwrap())
}

fn build_websocket_handshake_request(
    req: &Request<Incoming>,
    target_host: &str,
    target_port: u16,
    target_path: &str,
) -> String {
    let ws_key = req
        .headers()
        .get("Sec-WebSocket-Key")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(crate::protocol::generate_sec_websocket_key);

    let authority_host = if target_host.contains(':') && !target_host.starts_with('[') {
        format!("[{}]", target_host)
    } else {
        target_host.to_string()
    };

    let host_header = match target_port {
        80 | 443 => authority_host,
        _ => format!("{authority_host}:{target_port}"),
    };

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
        target_path, host_header, ws_key, ws_version
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
            || n.eq_ignore_ascii_case("origin")
            || n.eq_ignore_ascii_case("content-length")
            || n.eq_ignore_ascii_case("transfer-encoding")
            || n.eq_ignore_ascii_case("proxy-connection")
            || n.eq_ignore_ascii_case("keep-alive")
            || n.eq_ignore_ascii_case("te")
            || n.eq_ignore_ascii_case("trailer")
            || is_devtools_client_req_id_header(n)
        {
            continue;
        }

        if let Ok(v) = value.to_str() {
            handshake.push_str(&format!("{}: {}\r\n", n, v));
        }
    }

    if let Some(protocol) = req
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
    {
        handshake.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", protocol));
    }

    if let Some(extensions) = req
        .headers()
        .get("Sec-WebSocket-Extensions")
        .and_then(|v| v.to_str().ok())
    {
        handshake.push_str(&format!("Sec-WebSocket-Extensions: {}\r\n", extensions));
    }

    if let Some(origin) = req.headers().get("Origin").and_then(|v| v.to_str().ok()) {
        handshake.push_str(&format!("Origin: {}\r\n", origin));
    }

    handshake.push_str("\r\n");

    handshake
}

trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite> AsyncReadWrite for T {}

struct UpstreamWebSocketHandshake {
    stream: Box<dyn AsyncReadWrite + Send + Unpin>,
    leftover: BytesMut,
    headers: Vec<(String, String)>,
    sec_accept: Option<String>,
    protocol: Option<String>,
    extensions: Vec<String>,
}

enum InterceptedRuleShareAction {
    None,
    Redirect(String),
}

async fn handle_intercepted_rule_share_query(
    _req: &mut Request<Incoming>,
    request_url: &str,
    req_id: &str,
    admin_state: Option<&Arc<AdminState>>,
    _push_manager: Option<&SharedPushManager>,
) -> InterceptedRuleShareAction {
    if !request_url.contains(RULE_SHARE_QUERY_PARAM) {
        return InterceptedRuleShareAction::None;
    }

    let parts = match extract_rule_share_query(request_url) {
        Ok(parts) => parts,
        Err(error) => {
            warn!(
                target: "bifrost_proxy::rule_share",
                req_id,
                error = %error,
                url = %request_url,
                "failed to decode intercepted rule share query"
            );
            return InterceptedRuleShareAction::None;
        }
    };

    let Some(payload) = parts.payload else {
        return InterceptedRuleShareAction::None;
    };

    if let Some(state) = admin_state {
        match build_rule_share_confirm_url(state.port(), &payload, &parts.clean_url) {
            Ok(confirm_url) => {
                info!(
                    target: "bifrost_proxy::rule_share",
                    req_id,
                    target_url = %parts.clean_url,
                    confirm_url = %confirm_url,
                    "redirecting intercepted rule share query to confirmation page"
                );
                return InterceptedRuleShareAction::Redirect(confirm_url);
            }
            Err(error) => {
                warn!(
                    target: "bifrost_proxy::rule_share",
                    req_id,
                    error = %error,
                    "failed to build intercepted rule share confirmation URL"
                );
                return InterceptedRuleShareAction::None;
            }
        }
    }

    warn!(
        target: "bifrost_proxy::rule_share",
        req_id,
        "intercepted rule share query was present but admin state is unavailable"
    );
    InterceptedRuleShareAction::Redirect(parts.clean_url)
}

fn build_rule_share_confirm_url(
    admin_port: u16,
    payload: &bifrost_core::rule_share::RuleSharePayload,
    clean_url: &str,
) -> Result<String> {
    let encoded = encode_rule_share_payload(payload)?;
    let mut confirm = url::Url::parse(&format!(
        "http://127.0.0.1:{admin_port}{ADMIN_PATH_PREFIX}/share/rule"
    ))
    .map_err(|error| BifrostError::Proxy(format!("invalid rule share confirm URL: {error}")))?;
    confirm
        .query_pairs_mut()
        .append_pair("payload", &encoded)
        .append_pair("target", clean_url);
    Ok(confirm.to_string())
}

#[cfg(test)]
fn apply_clean_url_to_intercepted_request(
    req: &mut Request<Incoming>,
    clean_url: &str,
) -> Result<()> {
    let clean = url::Url::parse(clean_url)
        .map_err(|error| BifrostError::Proxy(format!("invalid clean rule share URL: {error}")))?;
    let mut path_and_query = clean.path().to_string();
    if path_and_query.is_empty() {
        path_and_query.push('/');
    }
    if let Some(query) = clean.query() {
        path_and_query.push('?');
        path_and_query.push_str(query);
    }
    let uri = path_and_query
        .parse::<hyper::Uri>()
        .map_err(|error| BifrostError::Proxy(format!("invalid clean rule share path: {error}")))?;
    *req.uri_mut() = uri;
    Ok(())
}

fn build_redirect_response(status_code: u16, location: &str) -> Response<BoxBody> {
    let body = format!(
        r#"<!DOCTYPE html><html>
<head><title>Redirect</title></head>
<body><a href="{}">Redirecting...</a></body>
</html>"#,
        location
    );

    Response::builder()
        .status(status_code)
        .header(hyper::header::LOCATION, location)
        .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(full_body(body.into_bytes()))
        .unwrap()
}

fn is_domain_matched(host: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }

    let host_lower = host.to_lowercase();
    for pattern in patterns {
        let pattern_lower = pattern.to_lowercase();

        if let Some(base_domain) = pattern_lower.strip_prefix("*.") {
            let suffix = format!(".{}", base_domain);
            if host_lower.ends_with(&suffix) || host_lower == base_domain {
                return true;
            }
        } else if host_lower == pattern_lower
            || host_lower.ends_with(&format!(".{}", pattern_lower))
        {
            return true;
        }
    }

    false
}

fn is_domain_excluded(host: &str, exclude_list: &[String]) -> bool {
    is_domain_matched(host, exclude_list)
}

fn is_domain_included(host: &str, include_list: &[String]) -> bool {
    is_domain_matched(host, include_list)
}

fn is_app_matched(client_app: Option<&str>, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }

    let app = match client_app {
        Some(a) if !a.is_empty() => a,
        _ => return false,
    };

    let app_lower = app.to_lowercase();
    for pattern in patterns {
        let pattern_lower = pattern.to_lowercase();

        if let Some(suffix) = pattern_lower.strip_prefix('*') {
            if app_lower.ends_with(&suffix) {
                return true;
            }
        } else if let Some(prefix) = pattern_lower.strip_suffix('*') {
            if app_lower.starts_with(prefix) {
                return true;
            }
        } else if app_lower == pattern_lower {
            return true;
        }
    }

    false
}

fn is_app_excluded(client_app: Option<&str>, exclude_list: &[String]) -> bool {
    is_app_matched(client_app, exclude_list)
}

fn is_app_included(client_app: Option<&str>, include_list: &[String]) -> bool {
    is_app_matched(client_app, include_list)
}

fn is_ip_matched(client_ip: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }

    let parsed_ip: std::net::IpAddr = match client_ip.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };

    for pattern in patterns {
        if let Ok(network) = pattern.parse::<ipnet::IpNet>() {
            if network.contains(&parsed_ip) {
                return true;
            }
        } else if let Ok(single_ip) = pattern.parse::<std::net::IpAddr>() {
            if parsed_ip == single_ip {
                return true;
            }
        }
    }

    false
}

fn is_ip_excluded(client_ip: &str, exclude_list: &[String]) -> bool {
    is_ip_matched(client_ip, exclude_list)
}

fn is_ip_included(client_ip: &str, include_list: &[String]) -> bool {
    is_ip_matched(client_ip, include_list)
}

pub fn requires_client_app_for_tls_decision(tls_intercept_config: &TlsInterceptConfig) -> bool {
    !tls_intercept_config.app_intercept_include.is_empty()
        || !tls_intercept_config.app_intercept_exclude.is_empty()
}

pub fn should_intercept_tls(
    host: &str,
    client_app: Option<&str>,
    tls_intercept_config: &TlsInterceptConfig,
    tls_config: &TlsConfig,
    resolved_rules: &ResolvedRules,
) -> bool {
    should_intercept_tls_for_client(
        host,
        client_app,
        true,
        None,
        tls_intercept_config,
        tls_config,
        resolved_rules,
    )
}

pub fn should_intercept_tls_for_client(
    host: &str,
    client_app: Option<&str>,
    is_local_client: bool,
    client_ip: Option<&str>,
    tls_intercept_config: &TlsInterceptConfig,
    tls_config: &TlsConfig,
    resolved_rules: &ResolvedRules,
) -> bool {
    if tls_config.ca_cert.is_none() {
        return false;
    }

    if let Some(rule_intercept) = resolved_rules.tls_intercept {
        return rule_intercept;
    }

    if requires_tls_interception_for_host_rewrite(resolved_rules) {
        return true;
    }

    if is_domain_excluded(host, &tls_intercept_config.intercept_exclude) {
        return false;
    }

    if is_domain_included(host, &tls_intercept_config.intercept_include) {
        return true;
    }

    if is_local_client
        && requires_client_app_for_tls_decision(tls_intercept_config)
        && !matches!(client_app, Some(app) if !app.is_empty())
    {
        return false;
    }

    if is_local_client {
        if is_app_excluded(client_app, &tls_intercept_config.app_intercept_exclude) {
            return false;
        }

        if is_app_included(client_app, &tls_intercept_config.app_intercept_include) {
            return true;
        }
    }

    if let Some(ip) = client_ip {
        if is_ip_excluded(ip, &tls_intercept_config.ip_intercept_exclude) {
            return false;
        }

        if is_ip_included(ip, &tls_intercept_config.ip_intercept_include) {
            return true;
        }
    }

    tls_intercept_config.enable_tls_interception
}

pub fn parse_connect_authority(authority: &str) -> Result<(String, u16)> {
    let parts: Vec<&str> = authority.split(':').collect();
    match parts.len() {
        1 => Ok((parts[0].to_string(), 443)),
        2 => {
            let port = parts[1]
                .parse()
                .map_err(|_| BifrostError::Parse(format!("Invalid port: {}", parts[1])))?;
            Ok((parts[0].to_string(), port))
        }
        _ => Err(BifrostError::Parse(format!(
            "Invalid authority: {}",
            authority
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn record_mock_traffic(
    state: &Arc<AdminState>,
    req_id: &str,
    method: &str,
    url: &str,
    host: &str,
    start_time: &Instant,
    has_rules: bool,
    resolved_rules: &ResolvedRules,
    response: &Response<BoxBody>,
    request: &Request<Incoming>,
    client_ip: &str,
    client_app: Option<&str>,
    client_pid: Option<u32>,
    client_path: Option<&str>,
    account_name: Option<&str>,
    listener_port: u16,
    devtools_client_req_id: &Option<String>,
) {
    let total_ms = start_time.elapsed().as_millis() as u64;
    let traffic_type = TrafficType::Https;

    state
        .metrics_collector
        .add_bytes_sent_by_type(traffic_type, 0);
    state
        .metrics_collector
        .increment_requests_by_type(traffic_type);

    let req_headers_pairs = super::headers_to_pairs(request.headers());
    let mock_status = response.status().as_u16();
    let mock_res_headers = super::headers_to_pairs(response.headers());

    let mut record = TrafficRecord::new(req_id.to_string(), method.to_string(), url.to_string());
    attach_devtools_client_req_id(&mut record, devtools_client_req_id);
    record.status = mock_status;
    record.duration_ms = total_ms;
    record.host = host.to_string();
    record.timing = Some(RequestTiming {
        dns_ms: None,
        connect_ms: None,
        tls_ms: None,
        send_ms: None,
        wait_ms: Some(total_ms),
        first_byte_ms: None,
        receive_ms: None,
        total_ms,
    });
    record.request_headers = Some(req_headers_pairs);
    record.original_response_headers = Some(mock_res_headers);
    record.has_rule_hit = has_rules;
    record.matched_rules = crate::utils::build_matched_rules(resolved_rules);
    record.client_ip = client_ip.to_string();
    record.client_app = client_app.map(|s| s.to_string());
    record.client_pid = client_pid;
    record.client_path = client_path.map(|s| s.to_string());
    record.account_name = account_name.map(|s| s.to_string());
    record.listener_port = listener_port;
    record.response_size = calculate_response_size(
        mock_status,
        record.original_response_headers.as_deref().unwrap_or(&[]),
        0,
    );
    record.upload_bytes = 0;
    record.download_bytes = 0;
    state.record_traffic(record);
}

#[allow(clippy::too_many_arguments)]
fn record_direct_status_traffic(
    state: &Arc<AdminState>,
    req_id: &str,
    method: &str,
    url: &str,
    host: &str,
    start_time: &Instant,
    has_rules: bool,
    resolved_rules: &ResolvedRules,
    response: &Response<BoxBody>,
    request_headers: &[(String, String)],
    original_request_headers: &[(String, String)],
    request_body: &[u8],
    request_content_encoding: Option<&str>,
    req_body_capture: &Option<BodyCaptureHandle>,
    response_body: Option<Bytes>,
    req_script_results: &[bifrost_script::ScriptExecutionResult],
    client_ip: &str,
    client_app: &Option<String>,
    client_pid: Option<u32>,
    client_path: &Option<String>,
    account_name: &Option<String>,
    listener_port: u16,
    devtools_client_req_id: &Option<String>,
) {
    let total_ms = start_time.elapsed().as_millis() as u64;
    let traffic_type = TrafficType::Https;

    state
        .metrics_collector
        .add_bytes_sent_by_type(traffic_type, request_body.len() as u64);

    let mock_status = response.status().as_u16();
    let mock_res_headers = super::headers_to_pairs(response.headers());
    let mock_res_body = response_body
        .or_else(|| resolved_rules.res_body.clone())
        .unwrap_or_else(|| {
            Bytes::from(
                hyper::StatusCode::from_u16(mock_status)
                    .ok()
                    .and_then(|status| status.canonical_reason())
                    .unwrap_or(""),
            )
        });
    let mock_body_len = mock_res_body.len();
    state
        .metrics_collector
        .add_bytes_received_by_type(traffic_type, mock_body_len as u64);
    state
        .metrics_collector
        .increment_requests_by_type(traffic_type);

    let mut record = TrafficRecord::new(req_id.to_string(), method.to_string(), url.to_string());
    attach_devtools_client_req_id(&mut record, devtools_client_req_id);
    record.status = mock_status;
    record.duration_ms = total_ms;
    record.host = host.to_string();
    record.timing = Some(RequestTiming {
        dns_ms: None,
        connect_ms: None,
        tls_ms: None,
        send_ms: None,
        wait_ms: Some(total_ms),
        first_byte_ms: None,
        receive_ms: None,
        total_ms,
    });
    record.request_headers = Some(request_headers.to_vec());
    if !super::headers_pairs_equal_ignore_order(original_request_headers, request_headers) {
        record.original_request_headers = Some(original_request_headers.to_vec());
    }
    record.request_size = request_body.len();
    record.upload_bytes = request_body.len();
    if !request_body.is_empty() {
        store_request_body(
            &Some(Arc::clone(state)),
            req_id,
            request_body,
            request_content_encoding,
        )
        .apply_to(&mut record);
    } else if let Some(capture) = req_body_capture {
        capture.apply_to(&mut record);
    }
    record.original_response_headers = Some(mock_res_headers);
    record.has_rule_hit = has_rules;
    record.matched_rules = crate::utils::build_matched_rules(resolved_rules);
    apply_listener_context(
        &mut record,
        listener_port,
        client_ip,
        client_app,
        client_pid,
        client_path,
        account_name,
    );
    if !req_script_results.is_empty() {
        record.req_script_results = Some(req_script_results.to_vec());
    }
    record.response_body_ref =
        store_response_body(&Some(Arc::clone(state)), req_id, &mock_res_body);
    record.response_size = calculate_response_size(
        mock_status,
        record.original_response_headers.as_deref().unwrap_or(&[]),
        mock_body_len,
    );
    record.download_bytes = mock_body_len;
    state.record_traffic(record);
}

struct TemplateVars {
    url: String,
    method: String,
    host: String,
    pathname: String,
    search: String,
    client_ip: String,
    req_id: String,
}

fn process_template(content: &str, vars: &TemplateVars) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();

    let random: u64 = rand::random();

    content
        .replace("${url}", &vars.url)
        .replace("${method}", &vars.method)
        .replace("${host}", &vars.host)
        .replace("${pathname}", &vars.pathname)
        .replace("${path}", &vars.pathname)
        .replace("${search}", &vars.search)
        .replace("${query}", &vars.search)
        .replace("${clientIp}", &vars.client_ip)
        .replace("${reqId}", &vars.req_id)
        .replace("${now}", &now)
        .replace("${timestamp}", &now)
        .replace("${random}", &random.to_string())
}

use crate::utils::mock::{guess_content_type, is_text_mime};

async fn serve_mock_file(
    file_path: &str,
    status_code: u16,
    template_vars: Option<&TemplateVars>,
) -> Response<BoxBody> {
    match tokio::fs::read(file_path).await {
        Ok(raw_bytes) => {
            let content_type = guess_content_type(file_path);

            let should_process_as_text = template_vars.is_some() || is_text_mime(&content_type);

            let body_bytes = if should_process_as_text {
                match String::from_utf8(raw_bytes) {
                    Ok(text) => {
                        let processed = if let Some(vars) = template_vars {
                            process_template(&text, vars)
                        } else {
                            text
                        };
                        processed.into_bytes()
                    }
                    Err(e) => e.into_bytes(),
                }
            } else {
                raw_bytes
            };

            let effective_content_type = if template_vars.is_some() && !is_text_mime(&content_type)
            {
                "application/json; charset=utf-8".to_string()
            } else {
                content_type
            };

            Response::builder()
                .status(status_code)
                .header(hyper::header::CONTENT_TYPE, &effective_content_type)
                .body(full_body(body_bytes))
                .unwrap()
        }
        Err(e) => {
            error!("Failed to read mock file {}: {}", file_path, e);
            Response::builder()
                .status(500)
                .body(full_body(
                    format!("Failed to read file: {}", e).into_bytes(),
                ))
                .unwrap()
        }
    }
}

fn apply_resolved_req_headers_to_outgoing_request<B>(
    req_id: &str,
    outgoing_req: &mut Request<B>,
    req_headers: &[(String, String)],
    verbose_logging: bool,
) -> std::result::Result<(), String> {
    for (name, value) in req_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid request header name: {name}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|_| format!("invalid request header value for header: {name}"))?;
        if verbose_logging {
            info!("[{}] [REQ_HEADER] {} = {}", req_id, name, value);
        }
        outgoing_req.headers_mut().insert(header_name, header_value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_admin::{FrameDirection, TrafficDbStore};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn websocket_handshake_status_classifies_upstream_rejection_without_transport_error() {
        assert_eq!(websocket_handshake_rejection_category(101), None);
        assert_eq!(
            websocket_handshake_rejection_category(200),
            Some("upstream_handshake_rejected")
        );
        assert_eq!(
            websocket_handshake_rejection_category(401),
            Some("upstream_handshake_rejected")
        );
    }

    #[test]
    fn websocket_handshake_rejection_logs_are_rate_limited_and_bounded() {
        let started = Instant::now();
        let mut limiter = WebSocketRejectionLogLimiter::default();

        assert_eq!(limiter.record("example.com", 401, started), Some(0));
        assert_eq!(
            limiter.record("example.com", 401, started + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            limiter.record("example.com", 401, started + Duration::from_secs(2)),
            None
        );
        assert_eq!(
            limiter.record("EXAMPLE.COM", 401, started + WEBSOCKET_REJECTION_LOG_WINDOW),
            Some(2)
        );

        for index in 0..(WEBSOCKET_REJECTION_LOG_MAX_KEYS + 10) {
            let host = format!("host-{index}.example.com");
            assert_eq!(limiter.record(&host, 400, started), Some(0));
        }
        assert_eq!(limiter.entries.len(), WEBSOCKET_REJECTION_LOG_MAX_KEYS);
    }

    #[tokio::test]
    async fn websocket_rejection_response_preserves_compatibility_status_and_body() {
        assert!(websocket_rejection_response(
            "req-accepted",
            "accepted.example",
            &HttpResponse::new(101, "Switching Protocols"),
        )
        .is_none());

        let response = websocket_rejection_response(
            "req-rejected",
            "rejected.example",
            &HttpResponse::new(401, "Unauthorized"),
        )
        .unwrap();
        assert_eq!(response.status(), 502);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"WebSocket handshake failed");
    }

    fn create_test_dir() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!(
            "bifrost_tunnel_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            counter
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn cleanup_test_dir(dir: &PathBuf) {
        let _ = fs::remove_dir_all(dir);
    }

    fn auto_tls_rule(protocol: Protocol) -> crate::server::RuleValue {
        crate::server::RuleValue {
            pattern: "auto-tls.local".to_string(),
            protocol,
            value: "test".to_string(),
            options: std::collections::HashMap::new(),
            rule_name: None,
            raw: None,
            line: None,
            auto_tls_intercept: true,
        }
    }

    #[test]
    fn test_h2_body_recovery_policy_probes_bounded_responses() {
        assert_eq!(
            h2_body_recovery_action(
                hyper::Version::HTTP_2,
                hyper::StatusCode::OK,
                "GET",
                "image/png",
                Some(1024),
                2048,
                true,
            ),
            H2BodyRecoveryAction::Probe
        );
        assert_eq!(
            h2_body_recovery_action(
                hyper::Version::HTTP_2,
                hyper::StatusCode::OK,
                "GET",
                "text/html",
                None,
                2048,
                true,
            ),
            H2BodyRecoveryAction::Probe
        );
    }

    #[test]
    fn test_h2_body_recovery_policy_retries_large_or_unknown_binary() {
        assert_eq!(
            h2_body_recovery_action(
                hyper::Version::HTTP_2,
                hyper::StatusCode::OK,
                "GET",
                "image/png",
                Some(4096),
                2048,
                true,
            ),
            H2BodyRecoveryAction::RetryHttp1
        );
        assert_eq!(
            h2_body_recovery_action(
                hyper::Version::HTTP_2,
                hyper::StatusCode::OK,
                "GET",
                "application/octet-stream",
                None,
                2048,
                true,
            ),
            H2BodyRecoveryAction::RetryHttp1
        );
    }

    #[test]
    fn test_h2_body_recovery_policy_skips_non_retryable_or_streaming() {
        assert_eq!(
            h2_body_recovery_action(
                hyper::Version::HTTP_2,
                hyper::StatusCode::OK,
                "POST",
                "image/png",
                Some(1024),
                2048,
                false,
            ),
            H2BodyRecoveryAction::Stream
        );
        assert_eq!(
            h2_body_recovery_action(
                hyper::Version::HTTP_2,
                hyper::StatusCode::OK,
                "GET",
                "text/event-stream",
                None,
                2048,
                true,
            ),
            H2BodyRecoveryAction::Stream
        );
    }

    #[test]
    fn test_parse_connect_authority_with_port() {
        let (host, port) = parse_connect_authority("example.com:443").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_parse_connect_authority_custom_port() {
        let (host, port) = parse_connect_authority("example.com:8443").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8443);
    }

    #[test]
    fn test_parse_connect_authority_default_port() {
        let (host, port) = parse_connect_authority("example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_parse_connect_authority_invalid_port() {
        let result = parse_connect_authority("example.com:invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_connect_authority_multiple_colons() {
        let result = parse_connect_authority("example.com:443:extra");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_http_alpn_matches_supported_http_protocols() {
        assert!(is_http_alpn(Some(b"h2")));
        assert!(is_http_alpn(Some(b"http/1.1")));
        assert!(!is_http_alpn(None));
        assert!(!is_http_alpn(Some(b"stun.turn")));
    }

    #[test]
    fn test_alpn_less_http1_payload_is_served_by_interceptor() {
        let http1 = BytesMut::from(&b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"[..]);
        assert!(should_serve_intercepted_http(None, true, &http1));
        assert!(!should_serve_intercepted_http(
            None,
            true,
            &BytesMut::from(&b"\x01\x02\x03"[..])
        ));
        assert!(!should_serve_intercepted_http(None, false, &http1));
        assert!(should_serve_intercepted_http(
            Some(b"http/1.1"),
            false,
            &BytesMut::new()
        ));
        assert!(!should_serve_intercepted_http(
            Some(b"stun.turn"),
            true,
            &http1
        ));
    }

    #[test]
    fn test_looks_like_http_payload_detects_http_preface() {
        assert!(looks_like_http_payload(&BytesMut::from(
            &b"GET / HTTP/1.1\r\nHost: example.com\r\n"[..]
        )));
        assert!(looks_like_http_payload(&BytesMut::from(
            &b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"[..]
        )));
        assert!(!looks_like_http_payload(&BytesMut::from(
            &b"\x01\x02\x03\x04"[..]
        )));
        assert!(!looks_like_http_payload(&BytesMut::new()));
    }

    #[test]
    fn test_is_domain_excluded_exact_match() {
        let exclude = vec!["example.com".to_string()];
        assert!(is_domain_excluded("example.com", &exclude));
        assert!(!is_domain_excluded("other.com", &exclude));
    }

    #[test]
    fn test_is_domain_excluded_subdomain_match() {
        let exclude = vec!["example.com".to_string()];
        assert!(is_domain_excluded("sub.example.com", &exclude));
        assert!(is_domain_excluded("deep.sub.example.com", &exclude));
        assert!(!is_domain_excluded("notexample.com", &exclude));
    }

    #[test]
    fn test_is_domain_excluded_wildcard() {
        let exclude = vec!["*.example.com".to_string()];
        assert!(is_domain_excluded("sub.example.com", &exclude));
        assert!(is_domain_excluded("example.com", &exclude));
        assert!(!is_domain_excluded("other.com", &exclude));
    }

    #[test]
    fn test_is_domain_excluded_case_insensitive() {
        let exclude = vec!["Example.COM".to_string()];
        assert!(is_domain_excluded("example.com", &exclude));
        assert!(is_domain_excluded("EXAMPLE.COM", &exclude));
        assert!(is_domain_excluded("Sub.Example.Com", &exclude));
    }

    #[test]
    fn test_is_domain_excluded_empty_list() {
        let exclude: Vec<String> = vec![];
        assert!(!is_domain_excluded("example.com", &exclude));
    }

    #[test]
    fn test_is_domain_excluded_multiple_patterns() {
        let exclude = vec![
            "example.com".to_string(),
            "*.google.com".to_string(),
            "internal.corp".to_string(),
        ];
        assert!(is_domain_excluded("example.com", &exclude));
        assert!(is_domain_excluded("maps.google.com", &exclude));
        assert!(is_domain_excluded("api.internal.corp", &exclude));
        assert!(!is_domain_excluded("other.com", &exclude));
    }

    #[test]
    fn finalize_tunnel_tracking_persists_closed_socket_status() {
        let dir = create_test_dir();
        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(9913).with_traffic_db_store_shared(store.clone()));

        let req_id = "tunnel-close-1";
        state.connection_monitor.register_tunnel_connection(req_id);
        state
            .connection_monitor
            .update_traffic(req_id, FrameDirection::Send, 128);
        state
            .connection_monitor
            .update_traffic(req_id, FrameDirection::Receive, 64);

        let mut record = TrafficRecord::new(
            req_id.to_string(),
            "CONNECT".to_string(),
            "tunnel://example.test:443".to_string(),
        );
        record.status = 200;
        record.is_tunnel = true;
        state.record_traffic(record);

        std::thread::sleep(std::time::Duration::from_millis(100));
        finalize_tunnel_tracking(&state, req_id);
        std::thread::sleep(std::time::Duration::from_millis(100));

        let persisted = store.get_by_id(req_id).expect("record should exist");
        let socket_status = persisted
            .socket_status
            .expect("socket status should be persisted");
        assert!(!socket_status.is_open);
        assert_eq!(socket_status.send_bytes, 128);
        assert_eq!(socket_status.receive_bytes, 64);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn apply_resolved_req_headers_replaces_existing_header_values() {
        let mut req = Request::builder()
            .uri("http://example.com")
            .header("x-same-key", "client-original")
            .body(())
            .unwrap();

        apply_resolved_req_headers_to_outgoing_request(
            "test-req",
            &mut req,
            &[("x-same-key".to_string(), "rule-value".to_string())],
            false,
        )
        .expect("request headers should be applied");

        let values: Vec<_> = req
            .headers()
            .get_all("x-same-key")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect();
        assert_eq!(values, vec!["rule-value".to_string()]);
    }

    #[test]
    fn decode_scripts_require_tls_interception() {
        let rules = ResolvedRules {
            decode_scripts: vec!["decode_script".to_string()],
            rules: vec![auto_tls_rule(Protocol::Decode)],
            ..ResolvedRules::default()
        };

        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn bp_scripts_alone_do_not_require_tls_interception() {
        let rules = ResolvedRules {
            bp_scripts: vec!["local_echo".to_string()],
            ..ResolvedRules::default()
        };

        assert!(!requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn connect_host_rule_alone_does_not_require_tls_interception() {
        let rules = ResolvedRules {
            host: Some("127.0.0.1:3443".to_string()),
            host_protocol: Some(Protocol::Host),
            ..ResolvedRules::default()
        };

        assert!(!requires_tls_interception_for_connect_rules(&rules));
    }

    #[test]
    fn connect_proxy_rule_alone_does_not_require_tls_interception() {
        let rules = ResolvedRules {
            proxy: Some("127.0.0.1:8888".to_string()),
            ..ResolvedRules::default()
        };

        assert!(!requires_tls_interception_for_connect_rules(&rules));
        assert!(should_use_connect_upstream_proxy(&rules));
    }

    #[test]
    fn connect_proxy_rule_with_host_rewrite_does_not_use_upstream_proxy() {
        let rules = ResolvedRules {
            host: Some("127.0.0.1:3443".to_string()),
            host_protocol: Some(Protocol::Host),
            proxy: Some("127.0.0.1:8888".to_string()),
            ..ResolvedRules::default()
        };

        assert!(!requires_tls_interception_for_connect_rules(&rules));
        assert!(!should_use_connect_upstream_proxy(&rules));
    }

    #[test]
    fn connect_plaintext_upstream_rewrite_requires_tls_interception() {
        let http_rules = ResolvedRules {
            host: Some("127.0.0.1:3000".to_string()),
            host_protocol: Some(Protocol::Http),
            ..ResolvedRules::default()
        };
        let ws_rules = ResolvedRules {
            host: Some("127.0.0.1:3001".to_string()),
            host_protocol: Some(Protocol::Ws),
            ..ResolvedRules::default()
        };

        assert!(requires_tls_interception_for_connect_rules(&http_rules));
        assert!(requires_tls_interception_for_connect_rules(&ws_rules));
    }

    #[test]
    fn connect_content_mutation_requires_tls_interception_even_with_proxy_rule() {
        let rules = ResolvedRules {
            proxy: Some("127.0.0.1:8888".to_string()),
            res_headers: vec![(
                "X-Bifrost-Test".to_string(),
                "tls-intercept-required".to_string(),
            )],
            rules: vec![auto_tls_rule(Protocol::ResHeaders)],
            ..ResolvedRules::default()
        };

        assert!(requires_tls_interception_for_connect_rules(&rules));
    }

    #[test]
    fn connect_content_mutation_without_host_scope_does_not_require_tls_interception() {
        let rules = ResolvedRules {
            res_headers: vec![(
                "X-Bifrost-Test".to_string(),
                "tls-intercept-not-allowed".to_string(),
            )],
            rules: vec![crate::server::RuleValue {
                pattern: "*".to_string(),
                protocol: Protocol::ResHeaders,
                value: "X-Bifrost-Test=tls-intercept-not-allowed".to_string(),
                options: std::collections::HashMap::new(),
                rule_name: None,
                raw: Some("* resHeaders://X-Bifrost-Test=tls-intercept-not-allowed".to_string()),
                line: None,
                auto_tls_intercept: false,
            }],
            ..ResolvedRules::default()
        };

        assert!(!requires_tls_interception_for_connect_rules(&rules));
    }

    #[test]
    fn connect_delete_rule_with_host_scope_requires_tls_interception() {
        let rules = ResolvedRules {
            delete_res_headers: vec!["X-Remove-Me".to_string()],
            rules: vec![auto_tls_rule(Protocol::Delete)],
            ..ResolvedRules::default()
        };

        assert!(requires_tls_interception_for_connect_rules(&rules));
    }

    fn make_tls_config_with_ca() -> TlsConfig {
        TlsConfig {
            ca_cert: Some(vec![1, 2, 3]),
            ca_key: Some(vec![1, 2, 3]),
            cert_generator: None,
            sni_resolver: None,
        }
    }

    fn make_tls_config_without_ca() -> TlsConfig {
        TlsConfig {
            ca_cert: None,
            ca_key: None,
            cert_generator: None,
            sni_resolver: None,
        }
    }

    fn make_tls_intercept_config(
        enable: bool,
        exclude: Vec<String>,
        include: Vec<String>,
    ) -> TlsInterceptConfig {
        TlsInterceptConfig {
            enable_tls_interception: enable,
            intercept_exclude: exclude,
            intercept_include: include,
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        }
    }

    #[test]
    fn test_should_intercept_no_ca_cert() {
        let tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        let tls_config = make_tls_config_without_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "Should NOT intercept when CA cert is not available"
        );
        println!("✓ No CA cert: intercept={}", result);
    }

    #[test]
    fn test_should_intercept_enabled_default() {
        let tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(result, "Should intercept when enabled with empty lists");
        println!("✓ Enabled (empty lists): intercept={}", result);
    }

    #[test]
    fn test_should_intercept_excluded_domains() {
        let tls_intercept_config = make_tls_intercept_config(
            true,
            vec!["*.apple.com".to_string(), "example.com".to_string()],
            vec![],
        );
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result1 = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(!result1, "Should NOT intercept excluded domain");
        println!("✓ Excluded (example.com): intercept={}", result1);

        let result2 = should_intercept_tls(
            "api.apple.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(!result2, "Should NOT intercept wildcard excluded domain");
        println!("✓ Excluded (*.apple.com): intercept={}", result2);

        let result3 = should_intercept_tls(
            "other.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(result3, "Should intercept non-excluded domain");
        println!("✓ Not excluded (other.com): intercept={}", result3);
    }

    #[test]
    fn test_should_intercept_include_force_intercept() {
        let tls_intercept_config = make_tls_intercept_config(
            false,
            vec![],
            vec!["*.api.example.com".to_string(), "secure.local".to_string()],
        );
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result1 = should_intercept_tls(
            "secure.local",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result1,
            "Should intercept included domain even when globally disabled"
        );
        println!(
            "✓ Included (secure.local, global disabled): intercept={}",
            result1
        );

        let result2 = should_intercept_tls(
            "test.api.example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result2,
            "Should intercept wildcard included domain even when globally disabled"
        );
        println!(
            "✓ Included (*.api.example.com, global disabled): intercept={}",
            result2
        );

        let result3 = should_intercept_tls(
            "other.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result3,
            "Should NOT intercept non-included domain when globally disabled"
        );
        println!(
            "✓ Not included (other.com, global disabled): intercept={}",
            result3
        );
    }

    #[test]
    fn test_should_passthrough_domain_exclude_before_domain_include() {
        let tls_intercept_config = make_tls_intercept_config(
            true,
            vec!["secure.local".to_string()],
            vec!["secure.local".to_string()],
        );
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "secure.local",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "Domain passthrough should have higher priority than force intercept"
        );
        println!("✓ Domain passthrough > force intercept: intercept={result}");
    }

    #[test]
    fn test_should_intercept_rule_override_intercept() {
        let tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            tls_intercept: Some(true),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "any.domain.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Rule override (tlsIntercept://) should force interception"
        );
        println!("✓ Rule override tlsIntercept://: intercept={}", result);
    }

    #[test]
    fn test_should_intercept_rule_override_passthrough() {
        let tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            tls_intercept: Some(false),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "any.domain.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "Rule override (tlsPassthrough://) should force passthrough"
        );
        println!("✓ Rule override tlsPassthrough://: intercept={}", result);
    }

    #[test]
    fn test_should_intercept_disabled_globally() {
        let tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(!result, "Should NOT intercept when globally disabled");
        println!("✓ Global disabled: intercept={}", result);
    }

    #[test]
    fn test_should_intercept_rule_overrides_global_disable() {
        let tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            tls_intercept: Some(true),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Rule override should work even when globally disabled"
        );
        println!("✓ Rule override with global disabled: intercept={}", result);
    }

    #[test]
    fn test_should_intercept_http_host_rewrite_even_when_global_disable() {
        let tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            host: Some("localhost:8000".to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "nextoncall-bd.bifrost.local",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "HTTPS CONNECT rewritten to HTTP upstream should force interception"
        );
    }

    #[test]
    fn test_should_intercept_ws_host_rewrite_even_when_global_disable() {
        let tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            host: Some("localhost:8000".to_string()),
            host_protocol: Some(Protocol::Ws),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "nextoncall-bd.bifrost.local",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "WSS CONNECT rewritten to WS upstream should force interception"
        );
    }

    #[test]
    fn test_tls_passthrough_rule_still_overrides_http_host_rewrite() {
        let tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            host: Some("localhost:8000".to_string()),
            host_protocol: Some(Protocol::Http),
            tls_intercept: Some(false),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "nextoncall-bd.bifrost.local",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "Explicit tlsPassthrough:// should keep higher priority than auto interception"
        );
    }

    #[test]
    fn test_is_app_matched() {
        let patterns = vec![
            "Safari".to_string(),
            "Chrome*".to_string(),
            "*Firefox".to_string(),
        ];
        assert!(is_app_matched(Some("Safari"), &patterns));
        assert!(is_app_matched(Some("safari"), &patterns));
        assert!(is_app_matched(Some("Chrome"), &patterns));
        assert!(is_app_matched(Some("Chrome Beta"), &patterns));
        assert!(is_app_matched(Some("Firefox"), &patterns));
        assert!(is_app_matched(Some("Mozilla Firefox"), &patterns));
        assert!(!is_app_matched(Some("Edge"), &patterns));
        assert!(!is_app_matched(None, &patterns));
        assert!(!is_app_matched(Some(""), &patterns));
    }

    #[test]
    fn test_should_intercept_app_exclude() {
        let mut tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        tls_intercept_config.app_intercept_exclude = vec!["Safari".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result1 = should_intercept_tls(
            "example.com",
            Some("Safari"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(!result1, "Should NOT intercept traffic from excluded app");

        let result2 = should_intercept_tls(
            "example.com",
            Some("Chrome"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(result2, "Should intercept traffic from non-excluded app");
    }

    #[test]
    fn test_should_intercept_app_include() {
        let mut tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        tls_intercept_config.app_intercept_include = vec!["Safari".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result1 = should_intercept_tls(
            "example.com",
            Some("Safari"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result1,
            "Should intercept traffic from included app even when globally disabled"
        );

        let result2 = should_intercept_tls(
            "example.com",
            Some("Chrome"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result2,
            "Should NOT intercept traffic from non-included app when globally disabled"
        );
    }

    #[test]
    fn test_should_not_intercept_when_app_policy_configured_but_client_app_unknown() {
        let mut tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        tls_intercept_config.app_intercept_exclude = vec!["Postman".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "Should default to passthrough when app policy is configured but client app is unknown"
        );
    }

    #[test]
    fn test_should_intercept_domain_include_even_when_client_app_unknown() {
        let mut tls_intercept_config =
            make_tls_intercept_config(false, vec![], vec!["example.com".to_string()]);
        tls_intercept_config.app_intercept_exclude = vec!["Postman".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Explicit domain include should still force interception when client app is unknown"
        );
    }

    #[test]
    fn test_should_intercept_rule_override_even_when_client_app_unknown() {
        let mut tls_intercept_config = make_tls_intercept_config(false, vec![], vec![]);
        tls_intercept_config.app_intercept_exclude = vec!["Postman".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            tls_intercept: Some(true),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "example.com",
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Rule override should still win when client app is unknown"
        );
    }

    #[test]
    fn test_should_ignore_app_policy_for_non_local_client() {
        let mut tls_intercept_config =
            make_tls_intercept_config(false, vec![], vec!["example.com".to_string()]);
        tls_intercept_config.app_intercept_exclude = vec!["Postman".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            None,
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Non-local client traffic should skip app policy and follow domain/global rules"
        );
    }

    #[test]
    fn test_should_passthrough_app_exclude_before_app_include() {
        let mut tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        tls_intercept_config.app_intercept_exclude = vec!["Safari".to_string()];
        tls_intercept_config.app_intercept_include = vec!["Safari".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            Some("Safari"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "App passthrough should have higher priority than app force intercept"
        );
    }

    #[test]
    fn test_should_intercept_domain_include_before_app_exclude() {
        let mut tls_intercept_config = make_tls_intercept_config(true, vec![], vec![]);
        tls_intercept_config.app_intercept_exclude = vec!["Safari".to_string()];
        tls_intercept_config.intercept_include = vec!["example.com".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "example.com",
            Some("Safari"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Domain force intercept should have higher priority than app passthrough"
        );
    }

    #[test]
    fn test_should_passthrough_domain_exclude_before_app_include() {
        let mut tls_intercept_config =
            make_tls_intercept_config(true, vec!["chatgpt.com".to_string()], vec![]);
        tls_intercept_config.app_intercept_include = vec!["Microsoft Edge*".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls(
            "chatgpt.com",
            Some("Microsoft Edge Helper"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "Domain passthrough should have higher priority than app force intercept"
        );
    }

    #[test]
    fn test_should_intercept_rule_override_before_domain_and_app_passthrough() {
        let mut tls_intercept_config =
            make_tls_intercept_config(false, vec!["example.com".to_string()], vec![]);
        tls_intercept_config.app_intercept_exclude = vec!["Safari".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules {
            tls_intercept: Some(true),
            ..Default::default()
        };

        let result = should_intercept_tls(
            "example.com",
            Some("Safari"),
            &tls_intercept_config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Rule force intercept should have higher priority than domain and app passthrough"
        );
    }

    #[test]
    fn test_should_passthrough_app_exclude_before_ip_include() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.app_intercept_exclude = vec!["Safari".to_string()];
        config.ip_intercept_include = vec!["127.0.0.1".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            Some("Safari"),
            true,
            Some("127.0.0.1"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "App passthrough should have higher priority than IP force intercept"
        );
    }

    #[test]
    fn test_should_intercept_app_include_before_ip_exclude() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.app_intercept_include = vec!["Safari".to_string()];
        config.ip_intercept_exclude = vec!["127.0.0.1".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            Some("Safari"),
            true,
            Some("127.0.0.1"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "App force intercept should have higher priority than IP passthrough"
        );
    }

    #[test]
    fn test_ip_intercept_include_match() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.ip_intercept_include = vec!["192.168.1.100".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("192.168.1.100"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(result, "IP in include list should force interception");
    }

    #[test]
    fn test_ip_intercept_exclude_match() {
        let mut config = make_tls_intercept_config(true, vec![], vec![]);
        config.ip_intercept_exclude = vec!["10.0.0.50".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("10.0.0.50"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "IP in exclude list should prevent interception even with global enabled"
        );
    }

    #[test]
    fn test_ip_intercept_cidr_match() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.ip_intercept_include = vec!["10.0.0.0/8".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("10.1.2.3"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(result, "IP matching CIDR range should force interception");

        let result2 = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("192.168.1.1"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result2,
            "IP not in CIDR range should not match include list"
        );
    }

    #[test]
    fn test_ip_tls_priority_below_domain_include() {
        let mut config = make_tls_intercept_config(false, vec![], vec!["example.com".to_string()]);
        config.ip_intercept_exclude = vec!["192.168.1.100".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("192.168.1.100"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "Domain include should override IP exclude (higher priority)"
        );
    }

    #[test]
    fn test_ip_tls_priority_above_global() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.ip_intercept_include = vec!["192.168.1.100".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("192.168.1.100"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "IP include should override global disabled (IP priority > global)"
        );
    }

    #[test]
    fn test_ip_exclude_priority_above_ip_include() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.ip_intercept_include = vec!["192.168.1.100".to_string()];
        config.ip_intercept_exclude = vec!["192.168.1.100".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("192.168.1.100"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "IP passthrough should have higher priority than IP force intercept"
        );
    }

    #[test]
    fn test_ip_no_match_falls_to_global() {
        let mut config = make_tls_intercept_config(true, vec![], vec![]);
        config.ip_intercept_include = vec!["10.0.0.1".to_string()];
        config.ip_intercept_exclude = vec!["10.0.0.2".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("172.16.0.1"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            result,
            "IP not in any list should fall through to global toggle"
        );
    }

    #[test]
    fn test_ip_none_skips_ip_check() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.ip_intercept_include = vec!["192.168.1.100".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            true,
            None,
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(
            !result,
            "When client_ip is None, IP check should be skipped and fall to global"
        );
    }

    #[test]
    fn test_ip_intercept_ipv6() {
        let mut config = make_tls_intercept_config(false, vec![], vec![]);
        config.ip_intercept_include = vec!["::1".to_string()];
        let tls_config = make_tls_config_with_ca();
        let resolved_rules = ResolvedRules::default();

        let result = should_intercept_tls_for_client(
            "example.com",
            None,
            false,
            Some("::1"),
            &config,
            &tls_config,
            &resolved_rules,
        );
        assert!(result, "IPv6 loopback should match");
    }

    #[test]
    fn test_ip_matched_helper() {
        assert!(is_ip_matched(
            "192.168.1.1",
            &["192.168.1.0/24".to_string()]
        ));
        assert!(!is_ip_matched("10.0.0.1", &["192.168.1.0/24".to_string()]));
        assert!(is_ip_matched("10.0.0.1", &["10.0.0.1".to_string()]));
        assert!(!is_ip_matched("10.0.0.2", &["10.0.0.1".to_string()]));
        assert!(!is_ip_matched("invalid-ip", &["10.0.0.1".to_string()]));
        assert!(is_ip_matched("fe80::1", &["fe80::/10".to_string()]));
        assert!(!is_ip_matched("::1", &["fe80::/10".to_string()]));
    }

    #[test]
    fn test_guess_content_type_common_extensions() {
        assert!(guess_content_type("photo.png").starts_with("image/png"));
        assert!(guess_content_type("photo.jpg").starts_with("image/jpeg"));
        assert!(guess_content_type("style.css").starts_with("text/css"));
        assert!(guess_content_type("data.json").starts_with("application/json"));
        assert!(guess_content_type("page.html").starts_with("text/html"));
        assert!(guess_content_type("app.js").contains("javascript"));
        assert!(guess_content_type("doc.pdf").starts_with("application/pdf"));
        assert!(guess_content_type("archive.zip").starts_with("application/zip"));
        assert_eq!(
            guess_content_type("unknown.xyz123"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_guess_content_type_text_has_charset() {
        let ct = guess_content_type("page.html");
        assert!(
            ct.contains("charset=utf-8"),
            "text types should include charset: {}",
            ct
        );
        let ct = guess_content_type("style.css");
        assert!(
            ct.contains("charset=utf-8"),
            "text types should include charset: {}",
            ct
        );
    }

    #[test]
    fn test_guess_content_type_binary_no_charset() {
        let ct = guess_content_type("photo.png");
        assert!(
            !ct.contains("charset"),
            "binary types should not include charset: {}",
            ct
        );
        let ct = guess_content_type("video.mp4");
        assert!(
            !ct.contains("charset"),
            "binary types should not include charset: {}",
            ct
        );
    }

    #[test]
    fn test_is_text_mime_classification() {
        assert!(is_text_mime("text/plain; charset=utf-8"));
        assert!(is_text_mime("text/html; charset=utf-8"));
        assert!(is_text_mime("application/json"));
        assert!(is_text_mime("application/xml"));
        assert!(is_text_mime("application/javascript"));
        assert!(is_text_mime("application/vnd.api+json"));
        assert!(is_text_mime("application/atom+xml"));

        assert!(!is_text_mime("image/png"));
        assert!(!is_text_mime("application/octet-stream"));
        assert!(!is_text_mime("application/pdf"));
        assert!(!is_text_mime("video/mp4"));
    }

    #[tokio::test]
    async fn test_serve_mock_file_binary_png() {
        let dir = create_test_dir();
        let png_path = dir.join("test.png");
        let fake_png_bytes: Vec<u8> =
            vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0x00];
        fs::write(&png_path, &fake_png_bytes).unwrap();

        let resp = serve_mock_file(png_path.to_str().unwrap(), 200, None).await;
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.starts_with("image/png"),
            "expected image/png, got: {}",
            ct
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), &fake_png_bytes);
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn test_serve_mock_file_text_json() {
        let dir = create_test_dir();
        let json_path = dir.join("data.json");
        fs::write(&json_path, r#"{"key":"value"}"#).unwrap();

        let resp = serve_mock_file(json_path.to_str().unwrap(), 200, None).await;
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.starts_with("application/json"),
            "expected application/json, got: {}",
            ct
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), br#"{"key":"value"}"#);
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn test_serve_mock_file_template_substitution() {
        let dir = create_test_dir();
        let tpl_path = dir.join("tpl.json");
        fs::write(&tpl_path, r#"{"host":"${host}","method":"${method}"}"#).unwrap();

        let vars = TemplateVars {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            pathname: "/api".to_string(),
            search: "".to_string(),
            client_ip: "127.0.0.1".to_string(),
            req_id: "test-123".to_string(),
        };
        let resp = serve_mock_file(tpl_path.to_str().unwrap(), 200, Some(&vars)).await;
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            body_str.contains("example.com"),
            "template host not substituted: {}",
            body_str
        );
        assert!(
            body_str.contains("GET"),
            "template method not substituted: {}",
            body_str
        );
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn test_serve_mock_file_template_substitution_tpl_ext() {
        let dir = create_test_dir();
        let tpl_path = dir.join("response.tpl");
        fs::write(
            &tpl_path,
            r#"{"host":"${host}","method":"${method}","url":"${url}"}"#,
        )
        .unwrap();

        let vars = TemplateVars {
            url: "https://example.com/api".to_string(),
            method: "POST".to_string(),
            host: "example.com".to_string(),
            pathname: "/api".to_string(),
            search: "".to_string(),
            client_ip: "127.0.0.1".to_string(),
            req_id: "test-456".to_string(),
        };
        let resp = serve_mock_file(tpl_path.to_str().unwrap(), 200, Some(&vars)).await;

        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            ct.contains("json"),
            ".tpl with template_vars should get json content type, got: {}",
            ct
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !body_str.contains("${"),
            "template variables should be substituted in .tpl file: {}",
            body_str
        );
        assert!(
            body_str.contains("example.com"),
            "template host not substituted: {}",
            body_str
        );
        assert!(
            body_str.contains("POST"),
            "template method not substituted: {}",
            body_str
        );
        cleanup_test_dir(&dir);
    }

    #[tokio::test]
    async fn test_serve_mock_file_not_found() {
        let resp = serve_mock_file("/nonexistent/path/file.txt", 200, None).await;
        assert_eq!(resp.status(), 500);
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;

    use bytes::{Bytes, BytesMut};
    use hyper::Response;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, ReadBuf};

    #[test]
    fn test_apply_listener_context_sets_all_fields() {
        let mut record = TrafficRecord::new(
            "id-1".to_string(),
            "GET".to_string(),
            "https://example.com".to_string(),
        );
        let client_app = Some("UnitTestApp".to_string());
        let client_path = Some("/tmp/app".to_string());
        let account_name = Some("alice".to_string());
        apply_listener_context(
            &mut record,
            8080,
            "127.0.0.1",
            &client_app,
            Some(1234),
            &client_path,
            &account_name,
        );
        assert_eq!(record.listener_port, 8080);
        assert_eq!(record.client_ip, "127.0.0.1");
        assert_eq!(record.client_app, client_app);
        assert_eq!(record.client_pid, Some(1234));
        assert_eq!(record.client_path, client_path);
        assert_eq!(record.account_name, account_name);
    }

    #[test]
    fn test_is_standard_tls_intercept_port_only_standard_ports() {
        assert!(is_standard_tls_intercept_port(443));
        assert!(is_standard_tls_intercept_port(8443));
        assert!(!is_standard_tls_intercept_port(80));
        assert!(!is_standard_tls_intercept_port(0));
        assert!(should_sniff_tls_payload(None, 443));
        assert!(!should_sniff_tls_payload(Some(b"h2"), 443));
        assert!(should_sniff_tls_payload(Some(b"http/1.1"), 9443));
    }

    #[test]
    fn breakpoint_tls_request_url_preserves_non_default_port_and_ipv6_authority() {
        assert_eq!(
            tls_request_url("https", "example.com", 443, "/api?q=1"),
            "https://example.com/api?q=1"
        );
        assert_eq!(
            tls_request_url("https", "127.0.0.1", 8443, "/api"),
            "https://127.0.0.1:8443/api"
        );
        assert_eq!(
            tls_request_url("wss", "::1", 9443, "/socket"),
            "wss://[::1]:9443/socket"
        );
        assert_eq!(tls_authority("::1", 443, true), "[::1]:443");
    }

    #[test]
    fn test_is_explicit_tls_intercept_override_checks_domain_and_app() {
        let mut cfg = TlsInterceptConfig {
            enable_tls_interception: false,
            intercept_exclude: vec![],
            intercept_include: vec!["secure.example.com".to_string()],
            app_intercept_exclude: vec![],
            app_intercept_include: vec!["MyBrowser".to_string()],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        let mut rules = ResolvedRules {
            tls_intercept: Some(true),
            ..Default::default()
        };
        assert!(is_explicit_tls_intercept_override(
            "other.example.com",
            Some("SomeApp"),
            &cfg,
            &rules,
        ));

        // Without rule override, domain or app include should still force interception
        rules.tls_intercept = None;
        assert!(is_explicit_tls_intercept_override(
            "secure.example.com",
            None,
            &cfg,
            &rules,
        ));
        assert!(is_explicit_tls_intercept_override(
            "other.example.com",
            Some("MyBrowser"),
            &cfg,
            &rules,
        ));

        // No match -> no explicit override
        cfg.intercept_include.clear();
        cfg.app_intercept_include.clear();
        assert!(!is_explicit_tls_intercept_override(
            "other.example.com",
            Some("OtherApp"),
            &cfg,
            &rules,
        ));
    }

    #[test]
    fn test_requires_tls_interception_for_host_rewrite_only_plaintext_protocols() {
        let mut rules = ResolvedRules::default();
        assert!(!requires_tls_interception_for_host_rewrite(&rules));

        rules.host = Some("example.com".to_string());
        rules.host_protocol = Some(Protocol::Https);
        assert!(!requires_tls_interception_for_host_rewrite(&rules));

        rules.host_protocol = Some(Protocol::Http);
        assert!(requires_tls_interception_for_host_rewrite(&rules));
        rules.host_protocol = Some(Protocol::Ws);
        assert!(requires_tls_interception_for_host_rewrite(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_needs_interceptable_fields_and_auto_flag() {
        let mut rules = ResolvedRules::default();
        // No interceptable fields => false even with auto_tls_intercept
        rules.rules.push(crate::server::RuleValue {
            pattern: "*".to_string(),
            protocol: Protocol::ResHeaders,
            value: "X-Test: 1".to_string(),
            options: std::collections::HashMap::new(),
            rule_name: None,
            raw: None,
            line: None,
            auto_tls_intercept: true,
        });
        assert!(!requires_tls_interception_for_rules(&rules));

        // With matching content mutation + auto flag => true
        rules
            .res_headers
            .push(("X-Test".to_string(), "1".to_string()));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_should_use_connect_upstream_proxy_requires_proxy_and_ignored_or_no_host() {
        let mut rules = ResolvedRules::default();
        assert!(!should_use_connect_upstream_proxy(&rules));

        rules.proxy = Some("127.0.0.1:8888".to_string());
        // proxy but no host override -> uses upstream proxy
        assert!(should_use_connect_upstream_proxy(&rules));

        rules.host = Some("override.example".to_string());
        rules.ignored.host = false;
        assert!(!should_use_connect_upstream_proxy(&rules));

        rules.ignored.host = true;
        assert!(should_use_connect_upstream_proxy(&rules));
    }

    #[test]
    fn test_has_request_and_response_body_rules_detection() {
        let mut rules = ResolvedRules::default();
        assert!(!has_request_body_rules(&rules));
        assert!(!has_response_body_rules(&rules));

        rules.req_body = Some(Bytes::from_static(b"req"));
        assert!(has_request_body_rules(&rules));

        rules.req_body = None;
        rules.req_prepend = Some(Bytes::from_static(b"p"));
        assert!(has_request_body_rules(&rules));

        rules = ResolvedRules::default();
        rules.res_body = Some(Bytes::from_static(b"res"));
        assert!(has_response_body_rules(&rules));
    }

    #[test]
    fn test_build_upstream_pool_partition_includes_key_fields() {
        let rules = ResolvedRules {
            host: Some("override.example".to_string()),
            proxy: Some("127.0.0.1:8888".to_string()),
            host_protocol: Some(Protocol::Https),
            ignored: crate::server::IgnoredFields {
                host: true,
                ..Default::default()
            },
            upstream_unsafe_ssl: true,
            ..Default::default()
        };

        let partition =
            build_upstream_pool_partition("orig.example", "target.example", 443, false, &rules);

        assert!(partition.contains("orig=orig.example"));
        assert!(partition.contains("target=https://target.example:443"));
        assert!(partition.contains("host=Some(\"override.example\")"));
        assert!(partition.contains("proxy=Some(\"127.0.0.1:8888\")"));
        assert!(partition.contains("ignored_host=true"));
        assert!(partition.contains("upstream_unsafe_ssl=true"));
    }

    #[test]
    fn test_merge_connect_resolved_rules_merges_host_and_tls_fields() {
        let base = ResolvedRules {
            host: Some("base.example".to_string()),
            host_protocol: Some(Protocol::Https),
            tls_intercept: Some(false),
            tls_options: None,
            upstream_unsafe_ssl: false,
            sni_callback: None,
            rules: vec![crate::server::RuleValue {
                pattern: "*".to_string(),
                protocol: Protocol::ResHeaders,
                value: "X-Base: 1".to_string(),
                options: std::collections::HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: false,
            }],
            ..Default::default()
        };

        let tunnel_specific = ResolvedRules {
            host: Some("tunnel.example".to_string()),
            host_protocol: Some(Protocol::Http),
            tls_intercept: Some(true),
            tls_options: Some("opt".to_string()),
            upstream_unsafe_ssl: true,
            sni_callback: Some("plugin(arg)".to_string()),
            rules: vec![crate::server::RuleValue {
                pattern: "*".to_string(),
                protocol: Protocol::ReqHeaders,
                value: "X-Tunnel: 1".to_string(),
                options: std::collections::HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: true,
            }],
            ..Default::default()
        };

        let merged = merge_connect_resolved_rules(base, tunnel_specific);
        assert_eq!(merged.host.as_deref(), Some("tunnel.example"));
        assert_eq!(merged.host_protocol, Some(Protocol::Http));
        assert_eq!(merged.tls_intercept, Some(true));
        assert_eq!(merged.tls_options.as_deref(), Some("opt"));
        assert!(merged.upstream_unsafe_ssl);
        assert_eq!(merged.rules.len(), 2);
    }

    #[test]
    fn test_merge_connect_resolved_rules_respects_ignored_host() {
        let base = ResolvedRules {
            host: Some("base.example".to_string()),
            host_protocol: Some(Protocol::Https),
            ignored: crate::server::IgnoredFields {
                host: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let tunnel_specific = ResolvedRules {
            host: Some("tunnel.example".to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };

        let merged = merge_connect_resolved_rules(base, tunnel_specific);
        // Host rewrite should be ignored when base.ignored.host is true
        assert_eq!(merged.host.as_deref(), Some("base.example"));
        assert_eq!(merged.host_protocol, Some(Protocol::Https));
    }

    #[test]
    fn test_parse_sni_callback_spec_variants() {
        let (plugin, arg) = parse_sni_callback_spec("myPlugin(arg1)");
        assert_eq!(plugin, "myPlugin");
        assert_eq!(arg, Some("arg1"));

        let (plugin, arg) = parse_sni_callback_spec("  other_plugin (  spaced arg  ) ");
        assert_eq!(plugin, "other_plugin");
        assert_eq!(arg, Some("spaced arg  )"));

        let (plugin, arg) = parse_sni_callback_spec("simple_plugin");
        assert_eq!(plugin, "simple_plugin");
        assert_eq!(arg, None);

        let (plugin, arg) = parse_sni_callback_spec("empty() ");
        assert_eq!(plugin, "empty");
        assert_eq!(arg, Some(")"));
    }

    #[test]
    fn test_format_tls_alpn_some_and_none() {
        assert_eq!(format_tls_alpn(Some(b"h2")), "h2");
        assert_eq!(format_tls_alpn(Some(b"http/1.1")), "http/1.1");
        assert_eq!(format_tls_alpn(None), "none");
    }

    #[test]
    fn test_is_likely_text_and_binary_content_type_classification() {
        assert!(is_likely_text_content_type("text/plain"));
        assert!(is_likely_text_content_type(
            "application/json; charset=utf-8"
        ));
        assert!(is_likely_text_content_type("application/vnd.api+json"));
        assert!(is_likely_text_content_type("application/xml"));
        assert!(is_likely_text_content_type("application/javascript"));
        assert!(is_likely_text_content_type(
            "application/x-www-form-urlencoded"
        ));

        assert!(!is_likely_text_content_type(""));
        assert!(!is_likely_text_content_type("application/octet-stream"));

        assert!(is_likely_binary_content_type("application/octet-stream"));
        assert!(is_likely_binary_content_type("application/pdf"));
        assert!(is_likely_binary_content_type("audio/ogg"));
        assert!(is_likely_binary_content_type("video/mp4"));
        assert!(is_likely_binary_content_type("font/woff2"));
        assert!(is_likely_binary_content_type("application/grpc"));

        // Text types should not be considered binary
        assert!(!is_likely_binary_content_type("text/html"));
        assert!(!is_likely_binary_content_type("application/json"));
    }

    #[test]
    fn test_should_use_binary_performance_mode_behaviour() {
        let res = Response::builder()
            .status(200)
            .header(hyper::header::CONTENT_TYPE, "application/pdf")
            .body(empty_body())
            .unwrap();
        let (mut parts, _body) = res.into_parts();

        // Disabled completely
        assert!(!should_use_binary_performance_mode(&parts, false));

        // Binary content-type without attachment: should still be eligible
        assert!(should_use_binary_performance_mode(&parts, true));

        // Image type is explicitly skipped even in binary mode
        parts.headers.insert(
            hyper::header::CONTENT_TYPE,
            HeaderValue::from_static("image/png"),
        );
        assert!(!should_use_binary_performance_mode(&parts, true));

        // Attachment header should force binary mode even for non-binary type
        let res2 = Response::builder()
            .status(200)
            .header(
                hyper::header::CONTENT_DISPOSITION,
                "attachment; filename=export.bin",
            )
            .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
            .body(empty_body())
            .unwrap();
        let (parts2, _body2) = res2.into_parts();
        assert!(should_use_binary_performance_mode(&parts2, true));
    }

    #[test]
    fn test_process_template_replaces_basic_placeholders() {
        let vars = TemplateVars {
            url: "https://example.com/path?query=1".to_string(),
            method: "POST".to_string(),
            host: "example.com".to_string(),
            pathname: "/path".to_string(),
            search: "?query=1".to_string(),
            client_ip: "127.0.0.1".to_string(),
            req_id: "req-123".to_string(),
        };
        let tpl = "${method} ${url} ${host} ${pathname} ${search} ${clientIp} ${reqId}";
        let out = process_template(tpl, &vars);
        assert!(out.contains("POST"));
        assert!(out.contains("https://example.com/path?query=1"));
        assert!(out.contains("example.com"));
        assert!(out.contains("/path"));
        assert!(out.contains("?query=1"));
        assert!(out.contains("127.0.0.1"));
        assert!(out.contains("req-123"));
    }

    #[test]
    fn test_build_redirect_response_sets_status_and_headers() {
        let resp = build_redirect_response(301, "https://example.com/target");
        assert_eq!(resp.status(), 301);
        assert_eq!(
            resp.headers()
                .get(hyper::header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "https://example.com/target",
        );
        assert_eq!(
            resp.headers()
                .get(hyper::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/html; charset=utf-8",
        );
    }

    #[test]
    fn test_requires_client_app_for_tls_decision_detects_app_policy() {
        let mut cfg = TlsInterceptConfig {
            enable_tls_interception: true,
            intercept_exclude: vec![],
            intercept_include: vec![],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        assert!(!requires_client_app_for_tls_decision(&cfg));

        cfg.app_intercept_include.push("Chrome".to_string());
        assert!(requires_client_app_for_tls_decision(&cfg));

        cfg.app_intercept_include.clear();
        cfg.app_intercept_exclude.push("Postman".to_string());
        assert!(requires_client_app_for_tls_decision(&cfg));
    }

    #[test]
    fn test_is_domain_matched_and_app_matched_helpers() {
        let patterns = vec!["Example.COM".to_string(), "*.internal.local".to_string()];
        assert!(is_domain_matched("example.com", &patterns));
        assert!(is_domain_matched("foo.internal.local", &patterns));
        assert!(!is_domain_matched("other.com", &patterns));

        let app_patterns = vec![
            "Chrome".to_string(),
            "Safari*".to_string(),
            "*Firefox".to_string(),
        ];
        assert!(is_app_matched(Some("Chrome"), &app_patterns));
        assert!(is_app_matched(
            Some("Safari Technology Preview"),
            &app_patterns
        ));
        assert!(is_app_matched(Some("Mozilla Firefox"), &app_patterns));
        assert!(!is_app_matched(Some("Edge"), &app_patterns));
        assert!(!is_app_matched(None, &app_patterns));
    }

    #[test]
    fn test_is_ip_matched_helpers() {
        let patterns = vec![
            "192.168.1.0/24".to_string(),
            "10.0.0.1".to_string(),
            "fe80::/10".to_string(),
        ];
        assert!(is_ip_matched("192.168.1.23", &patterns));
        assert!(is_ip_matched("10.0.0.1", &patterns));
        assert!(is_ip_matched("fe80::1", &patterns));
        assert!(!is_ip_matched("172.16.0.1", &patterns));
    }

    #[test]
    fn test_requires_tls_interception_for_connect_rules_delegates_to_helpers() {
        let rules = ResolvedRules {
            res_headers: vec![("X-Test".to_string(), "1".to_string())],
            rules: vec![crate::server::RuleValue {
                pattern: "*".to_string(),
                protocol: Protocol::ResHeaders,
                value: "X-Test: 1".to_string(),
                options: std::collections::HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: true,
            }],
            ..Default::default()
        };
        assert!(requires_tls_interception_for_connect_rules(&rules));

        let host_rewrite_rules = ResolvedRules {
            host: Some("127.0.0.1:3000".to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };
        assert!(requires_tls_interception_for_connect_rules(
            &host_rewrite_rules
        ));
    }

    #[test]
    fn test_parse_connect_authority_invalid_and_empty_authority() {
        // Multiple colons should be rejected as invalid authority
        assert!(parse_connect_authority("too:many:colons").is_err());

        // Empty authority today is treated as an empty host with default HTTPS port
        let (host, port) =
            parse_connect_authority("").expect("empty authority should default to :443");
        assert_eq!(host, "");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_apply_resolved_req_headers_to_outgoing_request_appends_and_overwrites() {
        let mut req = hyper::Request::builder()
            .uri("http://example.com")
            .header("X-Existing", "client")
            .body(())
            .unwrap();

        apply_resolved_req_headers_to_outgoing_request(
            "req-1",
            &mut req,
            &[
                ("X-New".to_string(), "value".to_string()),
                ("X-Existing".to_string(), "rule".to_string()),
            ],
            false,
        )
        .expect("headers should be applied");

        let values: Vec<_> = req
            .headers()
            .get_all("X-Existing")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(values, vec!["rule".to_string()]);
        assert_eq!(
            req.headers().get("X-New").unwrap().to_str().unwrap(),
            "value",
        );
    }

    #[test]
    fn test_apply_resolved_req_headers_to_outgoing_request_invalid_name_and_value() {
        let mut req = hyper::Request::builder()
            .uri("http://example.com")
            .body(())
            .unwrap();

        let err = apply_resolved_req_headers_to_outgoing_request(
            "req-2",
            &mut req,
            &[("bad\nname".to_string(), "ok".to_string())],
            false,
        )
        .unwrap_err();
        assert!(err.contains("invalid request header name"));

        let err = apply_resolved_req_headers_to_outgoing_request(
            "req-3",
            &mut req,
            &[("X-Ok".to_string(), "bad\nvalue".to_string())],
            false,
        )
        .unwrap_err();
        assert!(err.contains("invalid request header value"));
    }

    #[test]
    fn test_tls_client_config_wrapper_functions_do_not_panic() {
        let _cfg1 = get_tls_client_config(false);
        let _cfg2 = get_tls_client_config_http1_only(true);
        let _cfg3 = get_tls_client_config_without_alpn(false);
    }

    struct StubReader {
        result: Option<std::result::Result<Vec<u8>, std::io::Error>>,
    }

    impl StubReader {
        fn with_bytes(bytes: &[u8]) -> Self {
            Self {
                result: Some(Ok(bytes.to_vec())),
            }
        }

        fn with_zero() -> Self {
            Self {
                result: Some(Ok(Vec::new())),
            }
        }

        fn with_error(msg: &str) -> Self {
            Self {
                result: Some(Err(std::io::Error::other(msg))),
            }
        }
    }

    impl AsyncRead for StubReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::result::Result<(), std::io::Error>> {
            match self.result.take() {
                Some(Ok(bytes)) => {
                    let len = bytes.len().min(buf.remaining());
                    buf.put_slice(&bytes[..len]);
                    Poll::Ready(Ok(()))
                }
                Some(Err(e)) => Poll::Ready(Err(e)),
                None => {
                    // Subsequent reads behave like EOF
                    Poll::Ready(Ok(()))
                }
            }
        }
    }

    impl Unpin for StubReader {}

    #[tokio::test]
    async fn test_sniff_tls_client_payload_reads_bytes() {
        let mut reader = StubReader::with_bytes(b"GET / HTTP/1.1\r\n");
        let buf = sniff_tls_client_payload(&mut reader, "req-1", false)
            .await
            .expect("sniff should succeed");
        assert_eq!(buf, BytesMut::from(&b"GET / HTTP/1.1\r\n"[..]));
    }

    #[tokio::test]
    async fn test_sniff_tls_client_payload_zero_length_returns_empty() {
        let mut reader = StubReader::with_zero();
        let buf = sniff_tls_client_payload(&mut reader, "req-2", false)
            .await
            .expect("sniff should succeed");
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn test_sniff_tls_client_payload_error_maps_to_bifrost_error() {
        let mut reader = StubReader::with_error("boom");
        let err = sniff_tls_client_payload(&mut reader, "req-3", false)
            .await
            .unwrap_err();
        match err {
            BifrostError::Network(msg) => {
                assert!(msg.contains("Failed to sniff intercepted TLS payload"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_h2_body_recovery_action_streams_large_text_bodies() {
        let action = h2_body_recovery_action(
            hyper::Version::HTTP_2,
            hyper::StatusCode::OK,
            "GET",
            "text/html",
            Some(10_000),
            2048,
            true,
        );
        assert_eq!(action, H2BodyRecoveryAction::Stream);
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;

    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bifrost_storage::ValuesStorage;
    use bytes::{Bytes, BytesMut};
    use hyper::header::{HeaderName, HeaderValue};
    use hyper::HeaderMap;
    use parking_lot::RwLock as ParkingRwLock;
    use serde_json::json;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

    // -------------------------- get_values_from_state --------------------------

    #[tokio::test]
    async fn test_get_values_from_state_none_returns_empty() {
        let values = get_values_from_state(&None).await;
        assert!(values.is_empty());
    }

    #[tokio::test]
    async fn test_get_values_from_state_without_storage_returns_empty() {
        let state = Arc::new(AdminState::new(0));
        let values = get_values_from_state(&Some(state)).await;
        assert!(values.is_empty());
    }

    #[tokio::test]
    async fn test_get_values_from_state_reads_from_values_storage() {
        let base_dir = std::env::temp_dir().join("bifrost_values_test_v2");
        let _ = std::fs::remove_dir_all(&base_dir);
        let mut storage = ValuesStorage::with_dir(base_dir.clone()).unwrap();
        storage.set_value("key1", "value1").unwrap();
        storage.set_value("key2", "value2").unwrap();

        let mut state = AdminState::new(1234);
        state.values_storage = Some(Arc::new(ParkingRwLock::new(storage)));
        let state = Arc::new(state);

        let values = get_values_from_state(&Some(state)).await;
        assert_eq!(values.get("key1"), Some(&"value1".to_string()));
        assert_eq!(values.get("key2"), Some(&"value2".to_string()));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // ------------------------------ parse_host_rule ------------------------------

    #[test]
    fn test_parse_host_rule_plain_host_without_port_or_path() {
        let parsed = parse_host_rule("example.com").expect("host should parse");
        assert_eq!(parsed.0, "example.com");
        assert_eq!(parsed.1, None);
        assert_eq!(parsed.2, None);
    }

    #[test]
    fn test_parse_host_rule_with_scheme_port_and_path() {
        let parsed = parse_host_rule("https://example.com:8443/api/v1?x=1").expect("parse");
        assert_eq!(parsed.0, "example.com");
        assert_eq!(parsed.1, Some(8443));
        assert_eq!(parsed.2.as_deref(), Some("/api/v1?x=1"));
    }

    #[test]
    fn test_parse_host_rule_strips_known_prefixes() {
        for prefix in [
            "http://", "https://", "ws://", "wss://", "host://", "xhost://", "proxy://", "pac://",
        ] {
            let rule = format!("{}example.org:8080/path", prefix);
            let parsed = parse_host_rule(&rule).expect("parse");
            assert_eq!(parsed.0, "example.org");
            assert_eq!(parsed.1, Some(8080));
            assert_eq!(parsed.2.as_deref(), Some("/path"));
        }
    }

    #[test]
    fn test_parse_host_rule_ignores_empty_or_whitespace() {
        assert!(parse_host_rule("").is_none());
        assert!(parse_host_rule("   ").is_none());
    }

    #[test]
    fn test_parse_host_rule_filters_root_path() {
        let parsed = parse_host_rule("https://example.com/").expect("parse");
        assert_eq!(parsed.0, "example.com");
        assert_eq!(parsed.1, None);
        assert_eq!(parsed.2, None, "root path should be treated as none");
    }

    #[test]
    fn test_parse_host_rule_invalid_uri_returns_none() {
        // space in host -> invalid URI
        assert!(parse_host_rule("bad host").is_none());
    }

    // ------------------------- sanitize_upstream_headers -------------------------

    #[test]
    fn test_sanitize_upstream_headers_removes_standard_hop_by_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            hyper::header::CONNECTION,
            HeaderValue::from_static("keep-alive"),
        );
        headers.insert(
            HeaderName::from_static("proxy-connection"),
            HeaderValue::from_static("a"),
        );
        headers.insert(
            HeaderName::from_static("keep-alive"),
            HeaderValue::from_static("b"),
        );
        headers.insert(
            hyper::header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        headers.insert(
            hyper::header::UPGRADE,
            HeaderValue::from_static("websocket"),
        );
        headers.insert(
            HeaderName::from_static("trailer"),
            HeaderValue::from_static("X-Trailer"),
        );

        sanitize_upstream_headers(&mut headers);

        assert!(!headers.contains_key(hyper::header::CONNECTION));
        assert!(!headers.contains_key("proxy-connection"));
        assert!(!headers.contains_key("keep-alive"));
        assert!(!headers.contains_key(hyper::header::TRANSFER_ENCODING));
        assert!(!headers.contains_key(hyper::header::UPGRADE));
        assert!(!headers.contains_key("trailer"));
    }

    #[test]
    fn test_sanitize_upstream_headers_removes_headers_listed_in_connection() {
        let mut headers = HeaderMap::new();
        headers.insert(
            hyper::header::CONNECTION,
            HeaderValue::from_static("Foo, Bar , baz"),
        );
        headers.insert(
            HeaderName::from_static("foo"),
            HeaderValue::from_static("1"),
        );
        headers.insert(
            HeaderName::from_static("bar"),
            HeaderValue::from_static("2"),
        );
        headers.insert(
            HeaderName::from_static("baz"),
            HeaderValue::from_static("3"),
        );

        sanitize_upstream_headers(&mut headers);

        assert!(!headers.contains_key(hyper::header::CONNECTION));
        assert!(!headers.contains_key("foo"));
        assert!(!headers.contains_key("bar"));
        assert!(!headers.contains_key("baz"));
    }

    #[test]
    fn test_sanitize_upstream_headers_preserves_te_trailers_only() {
        let mut headers = HeaderMap::new();
        headers.insert(hyper::header::TE, HeaderValue::from_static("trailers"));

        sanitize_upstream_headers(&mut headers);

        assert!(headers.contains_key(hyper::header::TE));
    }

    #[test]
    fn test_sanitize_upstream_headers_drops_te_when_not_trailers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            hyper::header::TE,
            HeaderValue::from_static("trailers, deflate"),
        );

        sanitize_upstream_headers(&mut headers);

        assert!(!headers.contains_key(hyper::header::TE));
    }

    // ------------------------- CombinedAsyncRw & BufferedIo -------------------------

    struct TestReader {
        data: Bytes,
        pos: usize,
    }

    impl AsyncRead for TestReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.pos >= self.data.len() {
                return Poll::Ready(Ok(()));
            }
            let remaining = &self.data[self.pos..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.pos += to_copy;
            Poll::Ready(Ok(()))
        }
    }

    struct RecordingWriter {
        written: Arc<ParkingRwLock<Vec<u8>>>,
    }

    impl AsyncWrite for RecordingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.written.write().extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn test_combined_async_rw_delegates_read_and_write() {
        let reader = TestReader {
            data: Bytes::from_static(b"hello"),
            pos: 0,
        };
        let written = Arc::new(ParkingRwLock::new(Vec::new()));
        let writer = RecordingWriter {
            written: Arc::clone(&written),
        };

        let mut combined = CombinedAsyncRw::new(reader, writer);

        let mut buf = [0u8; 5];
        combined.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        combined.write_all(b"world").await.unwrap();
        combined.flush().await.unwrap();

        let recorded = written.read().clone();
        assert_eq!(recorded, b"world");
    }

    #[tokio::test]
    async fn test_buffered_io_reads_from_prefilled_buffer_then_inner() {
        let inner = TestReader {
            data: Bytes::from_static(b"DEF"),
            pos: 0,
        };
        let buffer = BytesMut::from(&b"ABC"[..]);
        let mut io = BufferedIo::new(inner, buffer);

        let mut buf = [0u8; 3];
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ABC");

        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"DEF");
    }

    #[tokio::test]
    async fn test_buffered_io_partial_reads_across_multiple_calls() {
        let inner = TestReader {
            data: Bytes::from_static(b"XYZ"),
            pos: 0,
        };
        let buffer = BytesMut::from(&b"ABCDE"[..]);
        let mut io = BufferedIo::new(inner, buffer);

        let mut buf = [0u8; 2];
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"AB");
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"CD");

        let mut buf3 = [0u8; 3];
        io.read_exact(&mut buf3).await.unwrap();
        assert_eq!(&buf3, b"EXY".as_ref());
    }

    #[tokio::test]
    async fn test_buffered_io_without_buffer_reads_from_inner_only() {
        let inner = TestReader {
            data: Bytes::from_static(b"payload"),
            pos: 0,
        };
        let buffer = BytesMut::new();
        let mut io = BufferedIo::new(inner, buffer);

        let mut buf = [0u8; 7];
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"payload");
    }

    // ---------------------- build_tls_intercept_server_builder ----------------------

    #[tokio::test]
    async fn test_build_tls_intercept_server_builder_accepts_small_limits() {
        // Ensure builder can be constructed with a normal header limit without panicking.
        let _builder = build_tls_intercept_server_builder(128 * 1024);
    }

    #[tokio::test]
    async fn test_build_tls_intercept_server_builder_clamps_large_limits() {
        // usize::MAX should be clamped to u32::MAX internally without panic.
        let _builder = build_tls_intercept_server_builder(usize::MAX);
    }

    // --------------------------- requires_tls_interception_for_rules ---------------------------

    fn auto_tls_rule(protocol: Protocol) -> crate::server::RuleValue {
        crate::server::RuleValue {
            pattern: "*".to_string(),
            protocol,
            value: "test".to_string(),
            options: HashMap::new(),
            rule_name: None,
            raw: None,
            line: None,
            auto_tls_intercept: true,
        }
    }

    #[test]
    fn test_requires_tls_interception_for_rules_with_mock_template() {
        let mut rules = ResolvedRules {
            mock_template: Some("template.json".to_string()),
            ..Default::default()
        };
        rules.rules.push(auto_tls_rule(Protocol::ResHeaders));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_with_html_body_rule() {
        let mut rules = ResolvedRules {
            html_body: Some("<html></html>".to_string()),
            ..Default::default()
        };
        rules.rules.push(auto_tls_rule(Protocol::HtmlBody));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_with_url_params_rule() {
        let mut rules = ResolvedRules::default();
        rules
            .url_params
            .push(("foo".to_string(), "bar".to_string()));
        rules.rules.push(auto_tls_rule(Protocol::UrlParams));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_with_merge_fields() {
        let mut rules = ResolvedRules {
            req_merge: Some(json!({"k": "v"})),
            ..Default::default()
        };
        rules.rules.push(auto_tls_rule(Protocol::ResMerge));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    // ------------------------ has_request_body_rules / has_response_body_rules ------------------------

    #[test]
    fn test_has_request_body_rules_for_append_and_replace_variants() {
        let mut rules = ResolvedRules::default();
        assert!(!has_request_body_rules(&rules));

        rules.req_append = Some(Bytes::from_static(b"suffix"));
        assert!(has_request_body_rules(&rules));

        rules.req_append = None;
        rules.req_replace.push(("a".to_string(), "b".to_string()));
        assert!(has_request_body_rules(&rules));

        rules.req_replace.clear();
        rules.req_replace_regex.push(crate::server::RegexReplace {
            pattern: regex::Regex::new("a").unwrap(),
            replacement: "b".to_string(),
            global: false,
        });
        assert!(has_request_body_rules(&rules));

        rules.req_replace_regex.clear();
        rules.req_merge = Some(json!({"k": "v"}));
        assert!(has_request_body_rules(&rules));
    }

    #[test]
    fn test_has_response_body_rules_for_append_and_merge_variants() {
        let mut rules = ResolvedRules::default();
        assert!(!has_response_body_rules(&rules));

        rules.res_append = Some(Bytes::from_static(b"tail"));
        assert!(has_response_body_rules(&rules));

        rules.res_append = None;
        rules.res_replace.push(("a".to_string(), "b".to_string()));
        assert!(has_response_body_rules(&rules));

        rules.res_replace.clear();
        rules.res_merge = Some(json!({"k": "v"}));
        assert!(has_response_body_rules(&rules));
    }

    // ------------------------------ h2_body_recovery_action extra cases ------------------------------

    #[test]
    fn test_h2_body_recovery_action_streams_for_non_h2_or_head() {
        let action = h2_body_recovery_action(
            hyper::Version::HTTP_11,
            hyper::StatusCode::OK,
            "GET",
            "application/json",
            Some(1024),
            2048,
            true,
        );
        assert_eq!(action, H2BodyRecoveryAction::Stream);

        let action_head = h2_body_recovery_action(
            hyper::Version::HTTP_2,
            hyper::StatusCode::OK,
            "HEAD",
            "application/json",
            Some(1024),
            2048,
            true,
        );
        assert_eq!(action_head, H2BodyRecoveryAction::Stream);
    }

    #[test]
    fn test_h2_body_recovery_action_streams_for_no_content_or_not_modified() {
        let action_204 = h2_body_recovery_action(
            hyper::Version::HTTP_2,
            hyper::StatusCode::NO_CONTENT,
            "GET",
            "application/json",
            Some(0),
            2048,
            true,
        );
        assert_eq!(action_204, H2BodyRecoveryAction::Stream);

        let action_304 = h2_body_recovery_action(
            hyper::Version::HTTP_2,
            hyper::StatusCode::NOT_MODIFIED,
            "GET",
            "application/json",
            None,
            2048,
            true,
        );
        assert_eq!(action_304, H2BodyRecoveryAction::Stream);
    }

    // ------------------------------ process_template & serve_mock_file ------------------------------

    #[allow(dead_code)]
    fn create_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{}_{}_{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn test_process_template_replaces_dynamic_placeholders() {
        let vars = TemplateVars {
            url: "https://example.com/path?query=1".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            pathname: "/path".to_string(),
            search: "?query=1".to_string(),
            client_ip: "127.0.0.1".to_string(),
            req_id: "req-xyz".to_string(),
        };
        let tpl = "now=${now}, ts=${timestamp}, rand=${random}";
        let out = process_template(tpl, &vars);
        assert!(!out.contains("${now}"));
        assert!(!out.contains("${timestamp}"));
        assert!(!out.contains("${random}"));
    }

    // ---------------- apply_resolved_req_headers_to_outgoing_request (verbose logging) ----------------

    #[test]
    fn test_apply_resolved_req_headers_with_verbose_logging_inserts_headers() {
        let mut req = hyper::Request::builder()
            .uri("https://example.com")
            .body(())
            .unwrap();

        apply_resolved_req_headers_to_outgoing_request(
            "req-verbose-1",
            &mut req,
            &[
                ("X-Verbose-One".to_string(), "1".to_string()),
                ("X-Verbose-Two".to_string(), "2".to_string()),
            ],
            true,
        )
        .expect("headers should be applied");

        assert_eq!(
            req.headers()
                .get("X-Verbose-One")
                .unwrap()
                .to_str()
                .unwrap(),
            "1"
        );
        assert_eq!(
            req.headers()
                .get("X-Verbose-Two")
                .unwrap()
                .to_str()
                .unwrap(),
            "2"
        );
    }

    #[test]
    fn test_apply_resolved_req_headers_with_verbose_logging_overwrites_existing() {
        let mut req = hyper::Request::builder()
            .uri("https://example.com")
            .header("X-Existing", "client")
            .body(())
            .unwrap();

        apply_resolved_req_headers_to_outgoing_request(
            "req-verbose-2",
            &mut req,
            &[("X-Existing".to_string(), "rule".to_string())],
            true,
        )
        .expect("headers should be applied");

        let values: Vec<_> = req
            .headers()
            .get_all("X-Existing")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(values, vec!["rule".to_string()]);
    }
}

#[cfg(test)]
mod coverage_boost_v3 {
    use super::*;

    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use bifrost_admin::AdminState;
    use bifrost_core::rule_share::{append_rule_share_query, new_rule_share_payload};
    use bifrost_core::Protocol;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty, Full};
    use hyper::body::Incoming;
    use hyper::client::conn::http1 as client_http1;
    use hyper::header::HeaderValue;
    use hyper::server::conn::http1 as server_http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use tokio::io::duplex;
    use tokio::net::TcpListener;

    use wiremock::matchers::{method as wm_method, path as wm_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::server::{ResolvedRules, TlsInterceptConfig};

    #[derive(Clone)]
    struct EmptyRulesResolver;

    impl RulesResolver for EmptyRulesResolver {
        fn resolve_with_context(
            &self,
            _url: &str,
            _method: &str,
            _req_headers: &HashMap<String, String>,
            _req_cookies: &HashMap<String, String>,
        ) -> ResolvedRules {
            ResolvedRules::default()
        }
    }

    // ---------------- maybe_backfill_tunnel_client_process ----------------

    #[test]
    fn test_maybe_backfill_skips_when_metadata_present() {
        let state = Arc::new(AdminState::new(0));
        let peer_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 12345));
        let local_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 443));

        // Should early-return without spawning background resolver
        maybe_backfill_tunnel_client_process(
            &state,
            &Arc::new(ConnectionProcessState::default()),
            "req-meta",
            true,
            peer_addr,
            local_addr,
            false,
        );
    }

    #[test]
    fn test_maybe_backfill_skips_when_unknown_and_flag_set() {
        let state = Arc::new(AdminState::new(0));
        let peer_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 12345));
        let local_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 443));

        maybe_backfill_tunnel_client_process(
            &state,
            &Arc::new(ConnectionProcessState::default()),
            "req-skip",
            false,
            peer_addr,
            local_addr,
            true,
        );
    }

    #[test]
    fn test_maybe_backfill_skips_for_non_loopback_peers() {
        let state = Arc::new(AdminState::new(0));
        let peer_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 12345));
        let local_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 443));

        maybe_backfill_tunnel_client_process(
            &state,
            &Arc::new(ConnectionProcessState::default()),
            "req-nonloop",
            false,
            peer_addr,
            local_addr,
            false,
        );
    }

    #[test]
    fn test_maybe_backfill_joins_existing_connection_resolution() {
        let state = Arc::new(AdminState::new(0));
        let process_state = Arc::new(ConnectionProcessState::default());
        let peer_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 12346));
        let local_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 443));
        assert!(process_state.try_start_background_resolution());

        maybe_backfill_tunnel_client_process(
            &state,
            &process_state,
            "req-inflight",
            false,
            peer_addr,
            local_addr,
            false,
        );

        assert!(!process_state.try_start_background_resolution());
        process_state.finish_background_resolution();
        assert!(process_state.try_start_background_resolution());
        process_state.finish_background_resolution();
    }

    #[test]
    fn test_apply_tunnel_client_process_backfill_updates_connection_state_and_registry() {
        let state = AdminState::new(0);
        let process_state = ConnectionProcessState::default();
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        state.connection_registry.register(ConnectionInfo::new(
            "req-backfill".to_string(),
            "example.com".to_string(),
            443,
            false,
            None,
            cancel_tx,
        ));

        apply_tunnel_client_process_backfill(
            &state,
            &process_state,
            "req-backfill".to_string(),
            ClientProcess {
                pid: 4242,
                name: "coverage-client".to_string(),
                path: Some("/tmp/coverage-client".to_string()),
            },
        );

        let cached = process_state
            .cached()
            .expect("process state must be filled");
        assert_eq!(cached.pid, 4242);
        assert_eq!(cached.name, "coverage-client");
        assert_eq!(
            state.connection_registry.list_connections_full()[0].4,
            Some("coverage-client".to_string())
        );
    }

    #[tokio::test]
    async fn test_handle_connect_public_wrapper_rejects_missing_authority() {
        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(|req: Request<Incoming>| async move {
                let proxy_config = ProxyConfig::default();
                let tls_intercept_config = TlsInterceptConfig::from_proxy_config(&proxy_config);
                let result = handle_connect(
                    req,
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 12347)),
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 9900)),
                    Arc::new(EmptyRulesResolver),
                    Arc::new(TlsConfig {
                        ca_cert: None,
                        ca_key: None,
                        cert_generator: None,
                        sni_resolver: None,
                    }),
                    &tls_intercept_config,
                    &proxy_config,
                    false,
                    &RequestContext::new(),
                    None,
                    None,
                    None,
                )
                .await;
                assert!(result
                    .expect_err("origin-form URI must not be accepted as CONNECT")
                    .to_string()
                    .contains("missing authority"));
                Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(b"ok"))))
            });
            server_http1::Builder::new()
                .serve_connection(io, service)
                .await
                .unwrap();
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);
        let response = sender
            .send_request(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Empty::<Bytes>::new())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    // ---------------- domain / app / ip matching helpers ----------------

    #[test]
    fn test_is_domain_matched_empty_patterns_false() {
        let patterns: Vec<String> = Vec::new();
        assert!(!is_domain_matched("example.com", &patterns));
    }

    #[test]
    fn test_is_domain_matched_exact_and_wildcard_variants() {
        let patterns = vec!["example.com".to_string(), "*.internal.local".to_string()];
        assert!(is_domain_matched("example.com", &patterns));
        assert!(is_domain_matched("api.internal.local", &patterns));
        assert!(!is_domain_matched("other.com", &patterns));
    }

    #[test]
    fn test_is_app_matched_empty_and_none() {
        let patterns: Vec<String> = Vec::new();
        assert!(!is_app_matched(Some("Chrome"), &patterns));
        assert!(!is_app_matched(None, &patterns));
    }

    #[test]
    fn test_is_ip_matched_invalid_and_single_ip() {
        let patterns = vec!["192.168.1.0/24".to_string(), "10.0.0.1".to_string()];
        assert!(!is_ip_matched("not-an-ip", &patterns));
        assert!(is_ip_matched("10.0.0.1", &patterns));
    }

    #[test]
    fn test_ip_include_and_exclude_helpers() {
        let include = vec!["10.0.0.1".to_string()];
        let exclude = vec!["192.168.0.0/16".to_string()];
        assert!(is_ip_included("10.0.0.1", &include));
        assert!(is_ip_excluded("192.168.1.20", &exclude));
    }

    // ---------------- requires_client_app_for_tls_decision ----------------

    #[test]
    fn test_requires_client_app_for_tls_decision_when_lists_non_empty() {
        let mut cfg = TlsInterceptConfig {
            enable_tls_interception: true,
            intercept_exclude: vec![],
            intercept_include: vec![],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        assert!(!requires_client_app_for_tls_decision(&cfg));
        cfg.app_intercept_include.push("Chrome".to_string());
        assert!(requires_client_app_for_tls_decision(&cfg));
    }

    // ---------------- build_upstream_pool_partition ----------------

    #[test]
    fn test_build_upstream_pool_partition_http_and_https() {
        let rules = ResolvedRules {
            host: Some("override.example".to_string()),
            proxy: Some("127.0.0.1:8888".to_string()),
            host_protocol: Some(Protocol::Http),
            upstream_unsafe_ssl: true,
            ..Default::default()
        };

        let http_partition =
            build_upstream_pool_partition("orig.example", "target.example", 80, true, &rules);
        assert!(http_partition.contains("target=http://target.example:80"));

        let https_partition =
            build_upstream_pool_partition("orig.example", "target.example", 443, false, &rules);
        assert!(https_partition.contains("target=https://target.example:443"));
    }

    // ---------------- send_pooled_request (with real loopback listener) ----------------

    #[allow(dead_code)]
    async fn start_simple_http_server(port: u16) {
        let listener = TcpListener::bind((IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .await
            .expect("bind loopback listener");

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                let service = service_fn(|_req: Request<Incoming>| async move {
                    let mut resp: Response<BoxBody> = Response::builder()
                        .status(StatusCode::OK)
                        .header("x-echo", HeaderValue::from_static("ok"))
                        .body(full_body(Bytes::from_static(b"hello")))
                        .unwrap();
                    resp.headers_mut()
                        .insert(hyper::header::CONTENT_LENGTH, HeaderValue::from_static("5"));
                    Ok::<_, hyper::Error>(resp)
                });
                let _ = server_http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            }
        });
    }

    #[tokio::test]
    async fn test_send_pooled_request_basic_get_v3() {
        // Bind to an ephemeral port on loopback using std listener to satisfy requirements
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind std listener");
        let port = std_listener.local_addr().unwrap().port();
        // Hand std listener over to Tokio
        std_listener.set_nonblocking(true).unwrap();
        let listener = TcpListener::from_std(std_listener).expect("from_std");

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                let service = service_fn(|_req: Request<Incoming>| async move {
                    let mut resp: Response<BoxBody> = Response::builder()
                        .status(StatusCode::OK)
                        .header("x-echo", HeaderValue::from_static("ok"))
                        .body(full_body(Bytes::from_static(b"hello")))
                        .unwrap();
                    resp.headers_mut()
                        .insert(hyper::header::CONTENT_LENGTH, HeaderValue::from_static("5"));
                    Ok::<_, hyper::Error>(resp)
                });
                let _ = server_http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            }
        });

        let uri: hyper::Uri = format!("http://127.0.0.1:{port}/test").parse().unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(full_body(Bytes::new()))
            .unwrap();

        let resp = send_pooled_request(req, false, &[], "partition-basic")
            .await
            .expect("send_pooled_request should succeed");
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::OK);
        assert_eq!(parts.headers.get("x-echo").unwrap(), "ok");
        let bytes = body.collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn test_send_pooled_request_http1_only_v3() {
        let server = MockServer::start().await;
        Mock::given(wm_method("GET"))
            .and(wm_path("/pool"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes("ok-http1"))
            .mount(&server)
            .await;

        let uri: hyper::Uri = format!("{}/pool", server.uri()).parse().unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(full_body(Bytes::new()))
            .unwrap();

        let resp = send_pooled_request_http1_only(req, false, &[], "partition-http1")
            .await
            .expect("http1_only request should succeed");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"ok-http1");
    }

    // ---------------- rewrite_intercepted_virtual_host_request ----------------

    #[tokio::test]
    async fn test_rewrite_intercepted_virtual_host_request_prefixes_path() {
        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(|req: Request<Incoming>| async move {
                let rewritten = rewrite_intercepted_virtual_host_request(req);
                let path = rewritten.uri().path().to_string();
                let body = Full::new(Bytes::from(path));
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(body)
                        .unwrap(),
                )
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/foo")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes, Bytes::from("/_bifrost/foo"));

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    // ---------------- handle_intercepted_rule_share_query ----------------

    #[tokio::test]
    async fn test_handle_intercepted_rule_share_query_redirects_get() {
        let payload = new_rule_share_payload("demo", "example.com bp://127.0.0.1:3000").unwrap();
        let shared = append_rule_share_query("https://example.com/path?a=1", &payload).unwrap();
        let shared_for_server = shared.clone();

        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(move |mut req: Request<Incoming>| {
                let shared_cloned = shared_for_server.clone();
                async move {
                    let request_url = shared_cloned;
                    let action = handle_intercepted_rule_share_query(
                        &mut req,
                        &request_url,
                        "req-rule-share",
                        None,
                        None,
                    )
                    .await;

                    let marker = match action {
                        InterceptedRuleShareAction::Redirect(clean) => {
                            let body = Full::new(Bytes::from(clean));
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(body)
                                    .unwrap(),
                            );
                        }
                        _ => "none".to_string(),
                    };

                    let body = Full::new(Bytes::from(marker));
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(body)
                            .unwrap(),
                    )
                }
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        // Only path and query matter for the server; host is irrelevant here.
        let url: hyper::Uri = shared.parse().unwrap();
        let path_and_query = url.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

        let req = Request::builder()
            .method(Method::GET)
            .uri(path_and_query)
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = std::str::from_utf8(&bytes).unwrap();
        // Body should be the clean URL without the rule share query param.
        assert!(body_str.starts_with("https://example.com/path"));
        assert!(!body_str.contains("__bifrost_rule"));

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }
}

#[cfg(test)]
mod coverage_boost_v4 {
    use super::*;

    use bifrost_core::rule_share::{append_rule_share_query, new_rule_share_payload};
    use bifrost_core::Protocol;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty, Full};
    use hyper::body::Incoming;
    use hyper::client::conn::http1 as client_http1;
    use hyper::server::conn::http1 as server_http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use tokio::io::duplex;

    use crate::server::{ResolvedRules, RuleValue, TlsConfig, TlsInterceptConfig};

    // ---------------- is_websocket_upgrade_request ----------------

    #[tokio::test]
    async fn test_is_websocket_upgrade_request_true_for_http1_headers_v4() {
        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(|req: Request<Incoming>| async move {
                let is_ws = is_websocket_upgrade_request(&req);
                let body = Full::new(Bytes::from(if is_ws { "ws" } else { "no" }));
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(body)
                        .unwrap(),
                )
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/chat")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header(hyper::header::UPGRADE, "websocket")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"ws");

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_is_websocket_upgrade_request_false_without_headers_v4() {
        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(|req: Request<Incoming>| async move {
                let is_ws = is_websocket_upgrade_request(&req);
                let body = Full::new(Bytes::from(if is_ws { "ws" } else { "no" }));
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(body)
                        .unwrap(),
                )
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/chat")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"no");

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    // ---------------- apply_clean_url_to_intercepted_request ----------------

    #[tokio::test]
    async fn test_apply_clean_url_to_intercepted_request_sets_path_and_query_v4() {
        let (client_side, server_side) = duplex(16 * 1024);
        let clean_url = "https://example.com/clean/path?x=1&y=2".to_string();

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(move |mut req: Request<Incoming>| {
                let clean_url = clean_url.clone();
                async move {
                    apply_clean_url_to_intercepted_request(&mut req, &clean_url).unwrap();
                    let path_and_query = req
                        .uri()
                        .path_and_query()
                        .map(|pq| pq.as_str().to_string())
                        .unwrap_or_default();
                    let body = Full::new(Bytes::from(path_and_query));
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(body)
                            .unwrap(),
                    )
                }
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/original?foo=bar")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"/clean/path?x=1&y=2");

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_apply_clean_url_to_intercepted_request_invalid_url_v4() {
        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(|mut req: Request<Incoming>| async move {
                let result = apply_clean_url_to_intercepted_request(&mut req, "://not-a-valid-url");
                let body = match result {
                    Ok(()) => Full::new(Bytes::from_static(b"ok")),
                    Err(_) => Full::new(Bytes::from_static(b"err")),
                };
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(body)
                        .unwrap(),
                )
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/original")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"err");

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    // ------------- additional handle_intercepted_rule_share_query tests -------------

    #[tokio::test]
    async fn test_handle_intercepted_rule_share_query_redirects_post_to_confirm_v4() {
        let payload = new_rule_share_payload("demo2", "example.com bp://127.0.0.1:4000").unwrap();
        let shared = append_rule_share_query("https://example.com/old?flag=1", &payload).unwrap();
        let shared_for_server = shared.clone();
        let admin_state = Arc::new(AdminState::new(9900));

        let (client_side, server_side) = duplex(16 * 1024);

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(move |mut req: Request<Incoming>| {
                let shared_url = shared_for_server.clone();
                let admin_state = admin_state.clone();
                async move {
                    let action = handle_intercepted_rule_share_query(
                        &mut req,
                        &shared_url,
                        "req-rule-share-post-v4",
                        Some(&admin_state),
                        None,
                    )
                    .await;

                    match action {
                        InterceptedRuleShareAction::Redirect(confirm_url) => {
                            let body = Full::new(Bytes::from(confirm_url));
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(body)
                                    .unwrap(),
                            )
                        }
                        _ => {
                            let body = Full::new(Bytes::from_static(b"unexpected"));
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(body)
                                    .unwrap(),
                            )
                        }
                    }
                }
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let uri: hyper::Uri = shared.parse().unwrap();
        let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

        let req = Request::builder()
            .method(Method::POST)
            .uri(path_and_query)
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = std::str::from_utf8(&bytes).unwrap();
        assert!(body_str.starts_with("http://127.0.0.1:9900/_bifrost/share/rule?"));
        assert!(body_str.contains("payload="));
        assert!(body_str.contains("target=https%3A%2F%2Fexample.com%2Fold%3Fflag%3D1"));
        assert!(!body_str.contains("__bifrost_rule"));

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_handle_intercepted_rule_share_query_invalid_payload_returns_none_v4() {
        let (client_side, server_side) = duplex(16 * 1024);

        let request_url = "https://example.com/path?__bifrost_rule=not-valid-base64".to_string();

        let server_task = tokio::spawn(async move {
            let io = TokioIo::new(server_side);
            let service = service_fn(move |mut req: Request<Incoming>| {
                let request_url = request_url.clone();
                async move {
                    let action = handle_intercepted_rule_share_query(
                        &mut req,
                        &request_url,
                        "req-bad-rule-share-v4",
                        None,
                        None,
                    )
                    .await;
                    let marker = match action {
                        InterceptedRuleShareAction::None => "none",
                        _ => "other",
                    };
                    let body = Full::new(Bytes::from(marker));
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(body)
                            .unwrap(),
                    )
                }
            });
            let _ = server_http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });

        let io = TokioIo::new(client_side);
        let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
        let client_task = tokio::spawn(conn);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/path?__bifrost_rule=not-valid-base64")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"none");

        drop(sender);
        client_task.await.unwrap().unwrap();
        server_task.await.unwrap();
    }

    // ------------- content-type helpers --------------

    #[test]
    fn test_is_likely_text_content_type_empty_and_binary_v4() {
        assert!(!is_likely_text_content_type(""));
        assert!(!is_likely_text_content_type("application/octet-stream"));
        assert!(is_likely_text_content_type("text/plain; charset=utf-8"));
    }

    #[test]
    fn test_is_likely_text_content_type_json_and_xml_variants_v4() {
        assert!(is_likely_text_content_type("application/json"));
        assert!(is_likely_text_content_type("application/vnd.api+json"));
        assert!(is_likely_text_content_type("application/atom+xml"));
    }

    #[test]
    fn test_is_likely_binary_content_type_for_grpc_and_fonts_v4() {
        assert!(is_likely_binary_content_type("application/grpc+proto"));
        assert!(is_likely_binary_content_type("application/font-woff"));
        assert!(is_likely_binary_content_type("font/woff2"));
    }

    #[test]
    fn test_is_likely_binary_content_type_false_for_text_with_charset_v4() {
        assert!(!is_likely_binary_content_type("text/html; charset=utf-8"));
        assert!(!is_likely_binary_content_type(
            "application/json; charset=utf-8"
        ));
    }

    // ------------- should_use_binary_performance_mode extra cases -------------

    #[test]
    fn test_should_use_binary_performance_mode_disabled_flag_always_false_v4() {
        let res = Response::builder()
            .status(200)
            .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
            .body(empty_body())
            .unwrap();
        let (parts, _body) = res.into_parts();
        assert!(!should_use_binary_performance_mode(&parts, false));
    }

    #[test]
    fn test_should_use_binary_performance_mode_inline_binary_still_true_v4() {
        let res = Response::builder()
            .status(200)
            .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
            .body(empty_body())
            .unwrap();
        let (parts, _body) = res.into_parts();
        assert!(should_use_binary_performance_mode(&parts, true));
    }

    #[test]
    fn test_should_use_binary_performance_mode_skips_images_even_with_flag_v4() {
        let res = Response::builder()
            .status(200)
            .header(hyper::header::CONTENT_TYPE, "image/jpeg")
            .body(empty_body())
            .unwrap();
        let (parts, _body) = res.into_parts();
        assert!(!should_use_binary_performance_mode(&parts, true));
    }

    // ------------- h2 alpn formatting helpers -------------

    #[test]
    fn test_format_tls_alpn_unknown_bytes_v4() {
        assert_eq!(format_tls_alpn(Some(b"custom/1.0")), "custom/1.0");
    }

    #[test]
    fn test_is_http_alpn_recognises_h2_and_http11_v4() {
        assert!(is_http_alpn(Some(b"h2")));
        assert!(is_http_alpn(Some(b"http/1.1")));
        assert!(!is_http_alpn(Some(b"stun")));
        assert!(!is_http_alpn(None));
    }

    // ------------- additional TLS interception rules coverage -------------

    fn auto_tls_rule_v4(protocol: Protocol) -> RuleValue {
        RuleValue {
            pattern: "*".to_string(),
            protocol,
            value: "test".to_string(),
            options: std::collections::HashMap::new(),
            rule_name: None,
            raw: None,
            line: None,
            auto_tls_intercept: true,
        }
    }

    #[test]
    fn test_requires_tls_interception_for_rules_res_body_rule_v4() {
        let mut rules = ResolvedRules {
            res_body: Some(Bytes::from_static(b"body")),
            ..Default::default()
        };
        rules.rules.push(auto_tls_rule_v4(Protocol::ResBody));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_res_replace_regex_rule_v4() {
        let mut rules = ResolvedRules::default();
        rules.res_replace_regex.push(crate::server::RegexReplace {
            pattern: regex::Regex::new("foo").unwrap(),
            replacement: "bar".to_string(),
            global: true,
        });
        rules.rules.push(auto_tls_rule_v4(Protocol::ResReplace));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_req_scripts_rule_v4() {
        let mut rules = ResolvedRules::default();
        rules.req_scripts.push("script1".to_string());
        rules.rules.push(auto_tls_rule_v4(Protocol::ReqScript));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    #[test]
    fn test_requires_tls_interception_for_rules_res_scripts_rule_v4() {
        let mut rules = ResolvedRules::default();
        rules.res_scripts.push("script-res".to_string());
        rules.rules.push(auto_tls_rule_v4(Protocol::ResScript));
        assert!(requires_tls_interception_for_rules(&rules));
    }

    // ------------- IP helper extra coverage -------------

    #[test]
    fn test_is_ip_matched_ipv6_and_cidr_v4() {
        let patterns = vec!["fe80::/10".to_string(), "::1".to_string()];
        assert!(is_ip_matched("fe80::1", &patterns));
        assert!(is_ip_matched("::1", &patterns));
        assert!(!is_ip_matched("::2", &["::3".to_string()]));
    }

    #[test]
    fn test_is_ip_included_and_excluded_shortcuts_v4() {
        let include = vec!["10.1.1.0/24".to_string()];
        let exclude = vec!["10.1.2.0/24".to_string()];
        assert!(is_ip_included("10.1.1.10", &include));
        assert!(is_ip_excluded("10.1.2.10", &exclude));
    }

    // ------------- domain and TLS decision helpers -------------

    #[test]
    fn test_is_domain_included_and_excluded_shortcuts_v4() {
        let include = vec!["*.example.com".to_string()];
        let exclude = vec!["blocked.com".to_string()];
        assert!(is_domain_included("api.example.com", &include));
        assert!(!is_domain_included("foo.com", &include));
        assert!(is_domain_excluded("blocked.com", &exclude));
    }

    #[test]
    fn test_should_intercept_tls_for_client_respects_rule_override_true_v4() {
        let tls_config = TlsConfig {
            ca_cert: Some(vec![1]),
            ca_key: Some(vec![2]),
            cert_generator: None,
            sni_resolver: None,
        };
        let tls_intercept_config = TlsInterceptConfig {
            enable_tls_interception: false,
            intercept_exclude: vec![],
            intercept_include: vec![],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        let rules = ResolvedRules {
            tls_intercept: Some(true),
            ..Default::default()
        };
        assert!(should_intercept_tls_for_client(
            "example.com",
            None,
            true,
            None,
            &tls_intercept_config,
            &tls_config,
            &rules,
        ));
    }

    #[test]
    fn test_should_intercept_tls_for_client_respects_rule_override_false_v4() {
        let tls_config = TlsConfig {
            ca_cert: Some(vec![1]),
            ca_key: Some(vec![2]),
            cert_generator: None,
            sni_resolver: None,
        };
        let tls_intercept_config = TlsInterceptConfig {
            enable_tls_interception: true,
            intercept_exclude: vec![],
            intercept_include: vec![],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        let rules = ResolvedRules {
            tls_intercept: Some(false),
            ..Default::default()
        };
        assert!(!should_intercept_tls_for_client(
            "example.com",
            None,
            true,
            None,
            &tls_intercept_config,
            &tls_config,
            &rules,
        ));
    }

    #[test]
    fn test_should_intercept_tls_for_client_domain_include_without_app_policy_v4() {
        let tls_config = TlsConfig {
            ca_cert: Some(vec![1]),
            ca_key: Some(vec![2]),
            cert_generator: None,
            sni_resolver: None,
        };
        let tls_intercept_config = TlsInterceptConfig {
            enable_tls_interception: false,
            intercept_exclude: vec![],
            intercept_include: vec!["example.com".to_string()],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        let rules = ResolvedRules::default();
        assert!(should_intercept_tls_for_client(
            "example.com",
            None,
            true,
            None,
            &tls_intercept_config,
            &tls_config,
            &rules,
        ));
    }

    #[test]
    fn test_should_intercept_tls_for_client_skips_when_no_ca_cert_v4() {
        let tls_config = TlsConfig {
            ca_cert: None,
            ca_key: None,
            cert_generator: None,
            sni_resolver: None,
        };
        let tls_intercept_config = TlsInterceptConfig {
            enable_tls_interception: true,
            intercept_exclude: vec![],
            intercept_include: vec!["example.com".to_string()],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
            unsafe_ssl: false,
        };
        let rules = ResolvedRules::default();
        assert!(!should_intercept_tls_for_client(
            "example.com",
            None,
            true,
            None,
            &tls_intercept_config,
            &tls_config,
            &rules,
        ));
    }
}

#[cfg(test)]
mod coverage_90_wave {
    use super::*;
    use crate::server::{HeaderReplaceRule, HeaderReplaceTarget};
    use http_body_util::{BodyExt, Empty, Full, StreamBody};
    use hyper::client::conn::http1 as client_http1;
    use hyper::server::conn::http1 as server_http1;
    use hyper::service::service_fn;
    use hyper::{header, Method, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct StaticResolver(ResolvedRules);

    impl RulesResolver for StaticResolver {
        fn resolve_with_context(
            &self,
            _url: &str,
            _method: &str,
            _req_headers: &HashMap<String, String>,
            _req_cookies: &HashMap<String, String>,
        ) -> ResolvedRules {
            self.0.clone()
        }

        fn has_breakpoint_rules_for_host(&self, host: &str) -> bool {
            host.starts_with("source.test")
                && self
                    .0
                    .rules
                    .iter()
                    .any(|rule| rule.protocol == Protocol::Breakpoint)
        }
    }

    fn temp_path(extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bifrost-tunnel-coverage-{}-{}.{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            extension
        ))
    }

    async fn run_intercepted_request<B>(
        rules: ResolvedRules,
        admin_state: Option<Arc<AdminState>>,
        request: Request<B>,
        max_body_buffer_size: usize,
        max_body_probe_size: usize,
        verbose: bool,
    ) -> Response<Incoming>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        run_intercepted_request_config(
            rules,
            admin_state,
            request,
            max_body_buffer_size,
            max_body_probe_size,
            verbose,
            "source.test",
            443,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_intercepted_request_config<B>(
        rules: ResolvedRules,
        admin_state: Option<Arc<AdminState>>,
        request: Request<B>,
        max_body_buffer_size: usize,
        max_body_probe_size: usize,
        verbose: bool,
        original_host: &str,
        original_port: u16,
        inject_badge: bool,
    ) -> Response<Incoming>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        run_intercepted_request_config_result(
            rules,
            admin_state,
            request,
            max_body_buffer_size,
            max_body_probe_size,
            verbose,
            original_host,
            original_port,
            inject_badge,
        )
        .await
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_intercepted_request_config_result<B>(
        rules: ResolvedRules,
        admin_state: Option<Arc<AdminState>>,
        request: Request<B>,
        max_body_buffer_size: usize,
        max_body_probe_size: usize,
        verbose: bool,
        original_host: &str,
        original_port: u16,
        inject_badge: bool,
    ) -> std::result::Result<Response<Incoming>, hyper::Error>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let (client_io, server_io) = tokio::io::duplex(128 * 1024);
        let (mut sender, client_conn) = client_http1::handshake(TokioIo::new(client_io))
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = client_conn.await;
        });

        let resolver: Arc<dyn RulesResolver> = Arc::new(StaticResolver(rules));
        let original_host = original_host.to_string();
        let service = service_fn(move |request: Request<Incoming>| {
            let resolver = resolver.clone();
            let admin_state = admin_state.clone();
            let original_host = original_host.clone();
            async move {
                handle_intercepted_request_with_protocol(
                    request,
                    &original_host,
                    original_port,
                    "REQ-tunnel-coverage",
                    admin_state,
                    resolver,
                    verbose,
                    max_body_buffer_size,
                    max_body_probe_size,
                    true,
                    "127.0.0.1".to_string(),
                    Some("coverage-client".to_string()),
                    Some(42),
                    Some("/tmp/coverage-client".to_string()),
                    Some("coverage-account".to_string()),
                    19443,
                    None,
                    inject_badge,
                )
                .await
            }
        });
        tokio::spawn(async move {
            let _ = server_http1::Builder::new()
                .serve_connection(TokioIo::new(server_io), service)
                .await;
        });
        sender.send_request(request).await
    }

    async fn body_bytes(response: Response<Incoming>) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    #[tokio::test]
    async fn connect_breakpoint_rule_enables_scoped_tls_interception() {
        use bifrost_admin::breakpoint::BreakpointSettings;

        let state = Arc::new(AdminState::new(19443));
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 1024,
            });
        let resolver: Arc<dyn RulesResolver> = Arc::new(StaticResolver(ResolvedRules {
            rules: vec![breakpoint_rule("request")],
            ..Default::default()
        }));
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let service = service_fn(move |request: Request<Incoming>| {
            let state = state.clone();
            let resolver = resolver.clone();
            async move {
                let proxy_config = ProxyConfig::default();
                let tls_intercept_config = TlsInterceptConfig::from_proxy_config(&proxy_config);
                handle_connect(
                    request,
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 12348)),
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 9900)),
                    resolver,
                    Arc::new(TlsConfig {
                        ca_cert: Some(vec![1]),
                        ca_key: Some(vec![2]),
                        cert_generator: None,
                        sni_resolver: None,
                    }),
                    &tls_intercept_config,
                    &proxy_config,
                    false,
                    &RequestContext::new(),
                    Some(state),
                    None,
                    None,
                )
                .await
            }
        });
        let server = tokio::spawn(async move {
            let _ = server_http1::Builder::new()
                .serve_connection(TokioIo::new(server_io), service)
                .with_upgrades()
                .await;
        });
        let (mut sender, connection) = client_http1::handshake(TokioIo::new(client_io))
            .await
            .unwrap();
        let client = tokio::spawn(connection.with_upgrades());
        let response = sender
            .send_request(
                Request::builder()
                    .method(Method::CONNECT)
                    .uri("source.test:443")
                    .body(Full::new(Bytes::new()))
                    .unwrap(),
            )
            .await;
        assert!(
            response.is_err(),
            "missing cert generator proves the TLS interception path ran"
        );
        server.abort();
        client.abort();
    }

    fn breakpoint_rule(value: &str) -> crate::server::RuleValue {
        crate::server::RuleValue {
            pattern: "source.test".to_string(),
            protocol: Protocol::Breakpoint,
            value: value.to_string(),
            options: HashMap::new(),
            rule_name: Some("coverage-breakpoint".to_string()),
            raw: None,
            line: None,
            auto_tls_intercept: true,
        }
    }

    async fn wait_for_breakpoint(state: &AdminState) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !state.breakpoint_manager.has_pending("REQ-tunnel-coverage") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("breakpoint should become pending");
    }

    async fn chunked_http_fixture() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(|request: Request<Incoming>| async move {
                assert!(!request.headers().contains_key(header::CONTENT_ENCODING));
                let request_body = request.into_body().collect().await.unwrap().to_bytes();
                assert_eq!(request_body, Bytes::from_static(b"tunnel-new-body"));
                let frames = futures_util::stream::iter(vec![
                    Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"chunked"))),
                    Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"-tunnel"))),
                ]);
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/plain")
                        .body(StreamBody::new(frames))
                        .unwrap(),
                )
            });
            server_http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });
        (address, task)
    }

    #[tokio::test]
    async fn intercepted_status_covers_request_and_response_body_pipelines() {
        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19443)
            .build();
        let rules = ResolvedRules {
            status_code: Some(213),
            req_prepend: Some(Bytes::from_static(b"pre-")),
            req_append: Some(Bytes::from_static(b"-post")),
            req_replace: vec![("old".to_string(), "new".to_string())],
            res_body: Some(Bytes::from_static(b"old-response")),
            res_replace: vec![("old".to_string(), "new".to_string())],
            res_append: Some(Bytes::from_static(b"-tail")),
            method: Some("PUT".to_string()),
            req_headers: vec![
                ("X-Tunnel-Request".to_string(), "yes".to_string()),
                ("content-encoding".to_string(), "gzip".to_string()),
            ],
            res_headers: vec![("X-Tunnel-Response".to_string(), "yes".to_string())],
            ..Default::default()
        };
        let compressed = compress_body(b"old-body", "gzip").unwrap();
        let frames = futures_util::stream::iter(vec![Ok::<_, Infallible>(
            hyper::body::Frame::data(Bytes::from(compressed)),
        )]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/status?one=1&two=2")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::COOKIE, "a=1")
            .header(header::COOKIE, "b=2")
            .body(StreamBody::new(frames))
            .unwrap();
        let response =
            run_intercepted_request(rules, Some(harness.state()), request, 4, 2, true).await;
        assert_eq!(response.status().as_u16(), 213);
        assert_eq!(response.headers()["x-tunnel-response"], "yes");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"new-response-tail")
        );
        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("direct status traffic record");
        assert!(record
            .request_headers
            .as_ref()
            .is_some_and(|headers| headers.iter().any(|(name, value)| name
                .eq_ignore_ascii_case("content-encoding")
                && value == "gzip")));
    }

    #[tokio::test]
    async fn intercepted_streaming_rule_added_encoding_is_plaintext_in_search() {
        use base64::Engine as _;
        use bifrost_admin::search::{
            FilterCondition, SearchEngine, SearchFilters, SearchInclude, SearchRequest, SearchScope,
        };

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19444)
            .build();
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/streaming-rule-added-encoding"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&upstream)
            .await;

        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            req_headers: vec![("content-encoding".to_string(), "gzip".to_string())],
            ..Default::default()
        };
        let plaintext = br#"{"message":"needle-tunnel-528"}"#;
        let compressed = compress_body(plaintext, "gzip").unwrap();
        let frames = futures_util::stream::iter(vec![Ok::<_, Infallible>(
            hyper::body::Frame::data(Bytes::from(compressed)),
        )]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/streaming-rule-added-encoding")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "application/json")
            .body(StreamBody::new(frames))
            .unwrap();

        let response =
            run_intercepted_request(rules, Some(harness.state()), request, 4, 2, true).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(body_bytes(response).await.is_empty());

        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("streaming traffic record");
        assert_eq!(
            record.request_body_content_encoding().as_deref(),
            Some("gzip")
        );
        let stored = harness
            .body_store
            .read()
            .load_bytes(record.request_body_ref.as_ref().expect("request body ref"))
            .expect("stored request body");
        assert_eq!(
            crate::transform::decompress_body_with_limit(&stored, Some("gzip"), 4096).as_ref(),
            plaintext
        );

        let search =
            SearchEngine::new(harness.traffic_db.clone(), Some(harness.body_store.clone()));
        let result = search.search(&SearchRequest {
            keyword: "needle-tunnel-528".to_string(),
            scope: SearchScope {
                request_body: true,
                all: false,
                ..Default::default()
            },
            filters: SearchFilters {
                conditions: vec![FilterCondition {
                    field: "req.body.$.message".to_string(),
                    operator: "equals".to_string(),
                    value: "needle-tunnel-528".to_string(),
                }],
                ..Default::default()
            },
            include: SearchInclude {
                request_body: true,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(result.total_matched, 1);
        let included = result.results[0]
            .bodies
            .as_ref()
            .and_then(|bodies| bodies.request.as_ref())
            .expect("included decoded request body");
        let included = base64::engine::general_purpose::STANDARD
            .decode(&included.bytes_b64)
            .expect("base64 request body");
        assert_eq!(included, plaintext);
    }

    #[tokio::test]
    async fn intercepted_mock_files_cover_plain_template_raw_and_missing_paths() {
        let plain = temp_path("txt");
        let template = temp_path("tpl");
        let raw = temp_path("bin");
        tokio::fs::write(&plain, "plain mock").await.unwrap();
        tokio::fs::write(&template, "${host}|${pathname}|${method}|${query}")
            .await
            .unwrap();
        tokio::fs::write(&raw, [0_u8, 1, 2, 3]).await.unwrap();

        let variants = [
            ResolvedRules {
                mock_file: Some(plain.to_string_lossy().into_owned()),
                status_code: Some(214),
                ..Default::default()
            },
            ResolvedRules {
                mock_template: Some(template.to_string_lossy().into_owned()),
                status_code: Some(215),
                ..Default::default()
            },
            ResolvedRules {
                mock_rawfile: Some(raw.to_string_lossy().into_owned()),
                status_code: Some(216),
                ..Default::default()
            },
            ResolvedRules {
                mock_file: Some(temp_path("missing").to_string_lossy().into_owned()),
                ..Default::default()
            },
        ];
        for (index, rules) in variants.into_iter().enumerate() {
            let request = Request::builder()
                .method(Method::POST)
                .uri(format!("/mock/{index}?q=yes"))
                .header(header::HOST, "source.test")
                .body(Full::new(Bytes::new()))
                .unwrap();
            let response = run_intercepted_request(
                rules,
                Some(Arc::new(AdminState::new(19444))),
                request,
                1024,
                64,
                true,
            )
            .await;
            if index == 3 {
                assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            } else {
                assert!(response.status().is_success());
            }
            let _ = body_bytes(response).await;
        }
        for file in [plain, template, raw] {
            tokio::fs::remove_file(file).await.unwrap();
        }
    }

    #[tokio::test]
    async fn intercepted_http_upstream_covers_host_rewrite_body_rules_and_response_rules() {
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PATCH"))
            .and(wiremock::matchers::body_string("new-body"))
            .respond_with(
                wiremock::ResponseTemplate::new(201)
                    .insert_header("Content-Type", "text/plain")
                    .set_body_string("old-upstream"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(format!("{}/target/path", upstream.address())),
            host_protocol: Some(Protocol::Http),
            method: Some("PATCH".to_string()),
            req_body: Some(Bytes::from_static(b"new-body")),
            req_headers: vec![("X-Forwarded-Test".to_string(), "yes".to_string())],
            res_replace: vec![("old".to_string(), "new".to_string())],
            res_append: Some(Bytes::from_static(b"-tail")),
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/original")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_LENGTH, "8")
            .body(Full::new(Bytes::from_static(b"original")))
            .unwrap();
        let response = run_intercepted_request(
            rules,
            Some(Arc::new(AdminState::new(19445))),
            request,
            1024,
            64,
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"new-upstream-tail")
        );
    }

    #[tokio::test]
    async fn intercepted_oversized_request_streams_original_body_to_upstream() {
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string("0123456789abcdef"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            req_replace: vec![("0".to_string(), "x".to_string())],
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/oversized")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, "16")
            .body(Full::new(Bytes::from_static(b"0123456789abcdef")))
            .unwrap();
        let response = run_intercepted_request(rules, None, request, 4, 2, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, Bytes::from_static(b"ok"));
    }

    #[tokio::test]
    async fn intercepted_forward_covers_request_and_response_metadata_rules() {
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::header(
                "referer",
                "https://referer.test/",
            ))
            .and(wiremock::matchers::header("user-agent", "coverage-agent"))
            .and(wiremock::matchers::header("x-added", "request"))
            .respond_with(
                wiremock::ResponseTemplate::new(202)
                    .insert_header("content-type", "text/plain")
                    .insert_header("x-remove", "gone")
                    .insert_header("x-rewrite", "old-value")
                    .set_body_string("metadata-response"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            method: Some("PUT".to_string()),
            req_headers: vec![("x-added".to_string(), "request".to_string())],
            req_cookies: vec![("added".to_string(), "two".to_string())],
            referer: Some("https://referer.test/".to_string()),
            ua: Some("coverage-agent".to_string()),
            req_delay: Some(1),
            res_delay: Some(1),
            req_speed: Some(1024 * 1024),
            res_speed: Some(1024 * 1024),
            res_headers: vec![("x-response-added".to_string(), "yes".to_string())],
            delete_res_headers: vec!["x-remove".to_string()],
            header_replace: vec![HeaderReplaceRule {
                target: HeaderReplaceTarget::Response,
                header_name: "x-rewrite".to_string(),
                pattern: "old".to_string(),
                replacement: "new".to_string(),
            }],
            res_type: Some("txt".to_string()),
            res_charset: Some("utf-8".to_string()),
            cache: Some("15".to_string()),
            attachment: Some("coverage.txt".to_string()),
            response_for: Some("coverage-upstream".to_string()),
            trailers: vec![("x-checksum".to_string(), "done".to_string())],
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/metadata?x=1")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::CONTENT_LENGTH, "4")
            .header(header::COOKIE, "original=one")
            .header(header::REFERER, "https://old.test/")
            .header(header::USER_AGENT, "old-agent")
            .body(Full::new(Bytes::from_static(b"body")))
            .unwrap();
        let response = run_intercepted_request(rules, None, request, 1024, 64, true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()["x-response-added"], "yes");
        assert_eq!(response.headers()["x-rewrite"], "new-value");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "max-age=15");
        assert_eq!(
            response.headers()["x-bifrost-response-for"],
            "coverage-upstream"
        );
        assert!(!response.headers().contains_key("x-remove"));
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"metadata-response")
        );
        let requests = upstream.received_requests().await.unwrap();
        let cookie = requests[0]
            .headers
            .get(header::COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("original=one"));
        assert!(cookie.contains("added=two"));
    }

    #[tokio::test]
    async fn intercepted_connection_failure_applies_response_override() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unavailable = listener.local_addr().unwrap();
        drop(listener);
        let rules = ResolvedRules {
            host: Some(unavailable.to_string()),
            host_protocol: Some(Protocol::Http),
            replace_status: Some(521),
            res_body: Some(Bytes::from_static(b"custom-connect-error")),
            res_headers: vec![("x-connect-override".to_string(), "yes".to_string())],
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::GET)
            .uri("/unavailable")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = run_intercepted_request(
            rules,
            Some(Arc::new(AdminState::new(19446))),
            request,
            1024,
            64,
            true,
        )
        .await;
        assert_eq!(response.status().as_u16(), 521);
        assert_eq!(response.headers()["x-connect-override"], "yes");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"custom-connect-error")
        );
    }

    #[tokio::test]
    async fn intercepted_html_response_injects_badge_for_plain_and_gzip_bodies() {
        for compressed in [false, true] {
            let upstream = wiremock::MockServer::start().await;
            let html = b"<!doctype html><html><body>coverage</body></html>";
            let mut template = wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8");
            if compressed {
                let encoded = crate::transform::compress_body(html, "gzip").unwrap();
                template = template
                    .insert_header("content-encoding", "gzip")
                    .set_body_bytes(encoded);
            } else {
                template = template.set_body_bytes(html);
            }
            wiremock::Mock::given(wiremock::matchers::method("GET"))
                .respond_with(template)
                .mount(&upstream)
                .await;
            let rules = ResolvedRules {
                host: Some(upstream.address().to_string()),
                host_protocol: Some(Protocol::Http),
                ..Default::default()
            };
            let request = Request::builder()
                .method(Method::GET)
                .uri("/badge")
                .header(header::HOST, "source.test")
                .body(Full::new(Bytes::new()))
                .unwrap();
            let response = run_intercepted_request_config(
                rules,
                None,
                request,
                1024 * 1024,
                64,
                true,
                "source.test",
                443,
                true,
            )
            .await;
            let encoding = response
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let bytes = body_bytes(response).await;
            let decoded = crate::transform::decompress_body(&bytes, encoding.as_deref());
            assert!(String::from_utf8_lossy(&decoded).contains("__bifrost_badge__"));
        }
    }

    #[tokio::test]
    async fn intercepted_admin_virtual_host_routes_to_admin_router() {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/_bifrost/api/proxy/address")
            .header(header::HOST, ADMIN_VIRTUAL_HOST)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = run_intercepted_request_config(
            ResolvedRules::default(),
            Some(Arc::new(AdminState::new(19447))),
            request,
            1024,
            64,
            true,
            ADMIN_VIRTUAL_HOST,
            443,
            false,
        )
        .await;
        assert_ne!(response.status(), StatusCode::BAD_GATEWAY);
        let _ = body_bytes(response).await;
    }

    #[tokio::test]
    async fn intercepted_forward_with_fully_wired_admin_persists_bodies_and_traffic() {
        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19448)
            .build();
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/persist"))
            .and(wiremock::matchers::body_string("request-payload"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("{\"source\":\"upstream\"}"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            req_append: Some(Bytes::new()),
            res_replace: vec![("upstream".to_string(), "stored".to_string())],
            decode_scripts: vec!["utf8".to_string()],
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/persist?source=coverage")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::CONTENT_LENGTH, "15")
            .body(Full::new(Bytes::from_static(b"request-payload")))
            .unwrap();
        let response =
            run_intercepted_request(rules, Some(harness.state()), request, 1024, 64, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"{\"source\":\"stored\"}")
        );

        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("traffic record should be persisted");
        assert!(record.request_body_ref.is_some());
        assert!(record.response_body_ref.is_some());

        harness.state().set_binary_traffic_performance_mode(true);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/binary"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/octet-stream")
                    .set_body_bytes(vec![0x5a; 256]),
            )
            .mount(&upstream)
            .await;
        let binary_rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };
        let binary_request = Request::builder()
            .method(Method::GET)
            .uri("/binary")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let binary_response = run_intercepted_request(
            binary_rules,
            Some(harness.state()),
            binary_request,
            32,
            16,
            true,
        )
        .await;
        assert_eq!(body_bytes(binary_response).await.len(), 256);

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/events"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: one\n\ndata: two\n\n"),
            )
            .mount(&upstream)
            .await;
        let event_rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };
        let event_request = Request::builder()
            .method(Method::GET)
            .uri("/events")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let event_response = run_intercepted_request(
            event_rules,
            Some(harness.state()),
            event_request,
            8,
            8,
            true,
        )
        .await;
        assert_eq!(
            body_bytes(event_response).await,
            Bytes::from_static(b"data: one\n\ndata: two\n\n")
        );

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/large-request"))
            .and(wiremock::matchers::body_string("0123456789abcdef"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&upstream)
            .await;
        let large_request_rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            req_replace: vec![("0".to_string(), "x".to_string())],
            ..Default::default()
        };
        let large_request = Request::builder()
            .method(Method::POST)
            .uri("/large-request")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, "16")
            .body(Full::new(Bytes::from_static(b"0123456789abcdef")))
            .unwrap();
        let large_response = run_intercepted_request(
            large_request_rules,
            Some(harness.state()),
            large_request,
            4,
            2,
            true,
        )
        .await;
        assert_eq!(large_response.status(), StatusCode::NO_CONTENT);
        let _ = body_bytes(large_response).await;
    }

    #[tokio::test]
    async fn intercepted_forward_executes_request_response_and_decode_scripts() {
        use bifrost_admin::ScriptManager;
        use bifrost_script::ScriptType;

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19449)
            .build();
        let scripts_dir = harness.data_dir().join("scripts");
        let manager = ScriptManager::new(scripts_dir);
        manager.init().await.unwrap();
        manager
            .engine()
            .save_script(
                ScriptType::Request,
                "coverage-request",
                r#"request.method = "PUT"; request.body = "script-request";"#,
            )
            .await
            .unwrap();
        manager
            .engine()
            .save_script(
                ScriptType::Response,
                "coverage-response",
                r#"response.status = 207; response.headers["content-type"] = "text/event-stream"; response.body = ["data: script-response", "", "data: [DONE]", "", ""].join(String.fromCharCode(10));"#,
            )
            .await
            .unwrap();
        manager
            .engine()
            .save_script(
                ScriptType::Decode,
                "coverage-decode",
                r#"ctx.output = { code: "0", data: "decoded-storage", msg: "" };"#,
            )
            .await
            .unwrap();

        let state = Arc::new(
            AdminState::new(19449)
                .with_traffic_db_store_shared(harness.traffic_db.clone())
                .with_body_store(harness.body_store.clone())
                .with_config_manager_shared(harness.config_manager.clone())
                .with_script_manager(manager),
        );
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::body_string("script-request"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("upstream-response"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            req_scripts: vec!["coverage-request".to_string()],
            res_scripts: vec!["coverage-response".to_string()],
            decode_scripts: vec!["coverage-decode".to_string()],
            values: HashMap::from([("coverage".to_string(), "yes".to_string())]),
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/scripts")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::CONTENT_LENGTH, "8")
            .body(Full::new(Bytes::from_static(b"original")))
            .unwrap();
        let response =
            run_intercepted_request(rules, Some(state.clone()), request, 1024, 64, true).await;
        assert_eq!(response.status().as_u16(), 207);
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"data: script-response\n\ndata: [DONE]\n\n")
        );

        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("scripted traffic should be persisted");
        assert!(record
            .req_script_results
            .as_ref()
            .is_some_and(|items| !items.is_empty()));
        assert!(record
            .res_script_results
            .as_ref()
            .is_some_and(|items| !items.is_empty()));
        assert!(record
            .decode_req_script_results
            .as_ref()
            .is_some_and(|items| !items.is_empty()));
        assert!(record
            .decode_res_script_results
            .as_ref()
            .is_some_and(|items| !items.is_empty()));
        assert_eq!(record.frame_count, 2);

        let direct_rules = ResolvedRules {
            status_code: Some(202),
            res_body: Some(Bytes::from_static(b"direct-before-script")),
            res_scripts: vec!["coverage-response".to_string()],
            values: HashMap::from([("coverage".to_string(), "direct".to_string())]),
            ..Default::default()
        };
        let direct_request = Request::builder()
            .method(Method::GET)
            .uri("/direct-script")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let direct_response = run_intercepted_request(
            direct_rules,
            Some(state.clone()),
            direct_request,
            1024,
            64,
            true,
        )
        .await;
        assert_eq!(direct_response.status().as_u16(), 207);
        assert_eq!(
            body_bytes(direct_response).await,
            Bytes::from_static(b"data: script-response\n\ndata: [DONE]\n\n")
        );
    }

    #[tokio::test]
    async fn intercepted_connection_failure_applies_response_overrides() {
        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19454)
            .build();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unavailable = listener.local_addr().unwrap();
        drop(listener);
        let rules = ResolvedRules {
            host: Some(unavailable.to_string()),
            host_protocol: Some(Protocol::Http),
            req_headers: vec![("content-encoding".to_string(), "gzip".to_string())],
            req_append: Some(Bytes::new()),
            replace_status: Some(521),
            res_body: Some(Bytes::from_static(b"tunnel-connect-error")),
            res_headers: vec![("x-tunnel-error".into(), "overridden".into())],
            ..Default::default()
        };
        let body = Bytes::from_static(b"tunnel-request");
        let request = Request::builder()
            .method(Method::POST)
            .uri("/unavailable")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_LENGTH, body.len())
            .body(Full::new(body))
            .unwrap();
        let response =
            run_intercepted_request(rules, Some(harness.state()), request, 4096, 64, true).await;
        assert_eq!(response.status().as_u16(), 521);
        assert_eq!(response.headers()["x-tunnel-error"], "overridden");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"tunnel-connect-error")
        );
        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("connection failure traffic record");
        assert_eq!(record.status, 521);
        assert_eq!(
            record.request_body_content_encoding().as_deref(),
            Some("gzip")
        );
    }

    #[tokio::test]
    async fn intercepted_chunked_request_and_response_cover_unknown_lengths() {
        let (address, upstream_task) = chunked_http_fixture().await;
        let rules = ResolvedRules {
            host: Some(address.to_string()),
            host_protocol: Some(Protocol::Http),
            req_replace: vec![("old".into(), "new".into())],
            res_replace: vec![("chunked".into(), "streamed".into())],
            ..Default::default()
        };
        let frames = futures_util::stream::iter(vec![
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"tunnel-old-"))),
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"body"))),
        ]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/chunked")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(StreamBody::new(frames))
            .unwrap();
        let response = run_intercepted_request(rules, None, request, 4096, 64, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"streamed-tunnel")
        );
        upstream_task.abort();
    }

    #[tokio::test]
    async fn intercepted_breakpoints_edit_request_and_buffered_sse_response() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19450)
            .build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 4096,
            });
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/breakpoint-sse"))
            .and(wiremock::matchers::body_string("edited-request"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(b"data: original\n\ndata: [DONE]\n\n".to_vec())
                    .insert_header("content-type", "text/event-stream")
                    .insert_header("content-length", "30"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            rules: vec![breakpoint_rule("both")],
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/breakpoint-sse")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::CONTENT_LENGTH, "8")
            .body(Full::new(Bytes::from_static(b"original")))
            .unwrap();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            run_intercepted_request(rules, Some(task_state), request, 4096, 64, true).await
        });

        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "request",
                BreakpointEdit {
                    headers: Some(vec![("x-breakpoint-request".into(), "yes".into())]),
                    body: Some("edited-request".into()),
                    ..Default::default()
                },
            )
            .is_ok());
        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "response",
                BreakpointEdit {
                    headers: Some(vec![("x-breakpoint-response".into(), "yes".into())]),
                    body: Some("data: edited\n\ndata: [DONE]\n\n".into()),
                    status: Some(220),
                    ..Default::default()
                },
            )
            .is_ok());

        let response = task.await.unwrap();
        assert_eq!(response.status().as_u16(), 220);
        assert_eq!(response.headers()["x-breakpoint-response"], "yes");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"data: edited\n\ndata: [DONE]\n\n")
        );
        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("breakpoint traffic record");
        assert_eq!(record.status, 220);
        assert!(record.request_body_ref.is_some());
        assert!(record.response_body_ref.is_some());
    }

    #[tokio::test]
    async fn intercepted_breakpoints_edit_regular_response_and_record_final_state() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19455)
            .build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 4096,
            });
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/breakpoint-regular-edited"))
            .and(wiremock::matchers::body_string("regular-edited-request"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(b"regular-upstream".to_vec())
                    .insert_header("content-type", "text/plain"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            rules: vec![breakpoint_rule("both")],
            ..Default::default()
        };
        let frames = futures_util::stream::iter(vec![
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"orig"))),
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"inal"))),
        ]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/breakpoint-regular")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(StreamBody::new(frames))
            .unwrap();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            run_intercepted_request(rules, Some(task_state), request, 4096, 64, true).await
        });

        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "request",
                BreakpointEdit {
                    headers: Some(vec![("x-regular-request".into(), "yes".into())]),
                    body: Some("regular-edited-request".into()),
                    method: Some("PUT".into()),
                    url: Some(format!("{}/breakpoint-regular-edited", upstream.uri())),
                    ..Default::default()
                },
            )
            .is_ok());
        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "response",
                BreakpointEdit {
                    headers: Some(vec![("x-regular-response".into(), "yes".into())]),
                    body: Some("regular-edited-response".into()),
                    status: Some(218),
                    ..Default::default()
                },
            )
            .is_ok());
        let response = task.await.unwrap();
        assert_eq!(response.status().as_u16(), 218);
        assert_eq!(response.headers()["x-regular-response"], "yes");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"regular-edited-response")
        );
        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("regular breakpoint traffic record");
        assert_eq!(record.status, 218);
        assert_eq!(record.method, "PUT");
        assert_eq!(record.path, "/breakpoint-regular-edited");
        assert!(record.response_body_ref.is_some());
    }

    async fn websocket_upstream_response(status: u16) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while request.len() < 16 * 1024 {
                if stream.read_exact(&mut byte).await.is_err() {
                    return;
                }
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            assert!(String::from_utf8_lossy(&request)
                .to_ascii_lowercase()
                .contains("x-coverage: request"));
            let response = if status == 101 {
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: upstream-accept\r\nSec-WebSocket-Protocol: chat.v1\r\nSec-WebSocket-Extensions: permessage-deflate\r\nX-Upstream: present\r\n\r\n"
                    .to_string()
            } else {
                format!("HTTP/1.1 {status} Forbidden\r\nContent-Length: 0\r\n\r\n")
            };
            stream.write_all(response.as_bytes()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        (address, task)
    }

    async fn websocket_tls_upstream_response() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        bifrost_tls::init_crypto_provider();
        let ca = Arc::new(bifrost_tls::generate_root_ca().unwrap());
        let cert = bifrost_tls::DynamicCertGenerator::new(ca)
            .generate_for_domain("127.0.0.1")
            .unwrap();
        let mut config = (*bifrost_tls::TlsConfig::build_server_config(&cert).unwrap()).clone();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while request.len() < 16 * 1024 {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            assert!(String::from_utf8_lossy(&request)
                .to_ascii_lowercase()
                .contains("x-coverage: request"));
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: tls-upstream\r\nSec-WebSocket-Protocol: chat.v1\r\nSec-WebSocket-Extensions: permessage-deflate\r\nX-TLS-Upstream: yes\r\n\r\n")
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        (address, task)
    }

    fn websocket_request() -> Request<Full<Bytes>> {
        Request::builder()
            .method(Method::GET)
            .uri("/socket?coverage=1")
            .header(header::HOST, "source.test")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-protocol", "chat.v1, other")
            .header("sec-websocket-extensions", "permessage-deflate")
            .header("origin", "https://source.test")
            .header("cookie", "session=coverage")
            .body(Full::new(Bytes::new()))
            .unwrap()
    }

    #[tokio::test]
    async fn raw_tls_relay_forwards_both_directions_and_honors_cancel() {
        let (proxy_client, mut client_peer) = tokio::io::duplex(1024);
        let (proxy_upstream, mut upstream_peer) = tokio::io::duplex(1024);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let state = Arc::new(AdminState::new(19452));
        state
            .connection_monitor
            .register_connection("REQ-raw-tls-coverage");
        let task = tokio::spawn(relay_raw_tls_streams_with_cancel(
            proxy_client,
            proxy_upstream,
            BytesMut::from(&b"initial-"[..]),
            true,
            "REQ-raw-tls-coverage".to_string(),
            Some(state),
            cancel_rx,
        ));

        client_peer.write_all(b"client").await.unwrap();
        let mut upstream_received = [0_u8; 14];
        upstream_peer
            .read_exact(&mut upstream_received)
            .await
            .unwrap();
        assert_eq!(&upstream_received, b"initial-client");

        upstream_peer.write_all(b"upstream").await.unwrap();
        let mut client_received = [0_u8; 8];
        client_peer.read_exact(&mut client_received).await.unwrap();
        assert_eq!(&client_received, b"upstream");

        cancel_tx.send(()).unwrap();
        assert!(task.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn intercepted_non_http_tls_connects_and_relays_through_tls_upstream() {
        bifrost_tls::init_crypto_provider();
        let ca = Arc::new(bifrost_tls::generate_root_ca().unwrap());
        let cert = bifrost_tls::DynamicCertGenerator::new(ca)
            .generate_for_domain("127.0.0.1")
            .unwrap();
        let acceptor =
            TlsAcceptor::from(bifrost_tls::TlsConfig::build_server_config(&cert).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut received = [0_u8; 14];
            stream.read_exact(&mut received).await.unwrap();
            assert_eq!(&received, b"initial-client");
            stream.write_all(b"upstream").await.unwrap();
            let mut closed = [0_u8; 1];
            let _ = stream.read(&mut closed).await;
        });

        let (proxy_client, mut client_peer) = tokio::io::duplex(1024);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let relay_task = tokio::spawn(tunnel_intercepted_non_http_tls_with_cancel(
            proxy_client,
            BytesMut::from(&b"initial-"[..]),
            RawTlsTunnelContext {
                original_host: "127.0.0.1".to_string(),
                original_port: upstream_address.port(),
                unsafe_ssl: true,
                verbose_logging: true,
                req_id: "REQ-intercepted-raw-tls".to_string(),
                admin_state: None,
                cancel_rx,
            },
        ));

        client_peer.write_all(b"client").await.unwrap();
        let mut response = [0_u8; 8];
        client_peer.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"upstream");
        cancel_tx.send(()).unwrap();
        assert!(relay_task.await.unwrap().unwrap());
        upstream_task.await.unwrap();
    }

    #[tokio::test]
    async fn raw_tls_relay_treats_clean_half_closes_as_completion() {
        let (proxy_client, client_peer) = tokio::io::duplex(128);
        let (proxy_upstream, _upstream_peer) = tokio::io::duplex(128);
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        drop(client_peer);
        assert!(!relay_raw_tls_streams_with_cancel(
            proxy_client,
            proxy_upstream,
            BytesMut::new(),
            false,
            "REQ-raw-client-close".to_string(),
            None,
            cancel_rx,
        )
        .await
        .unwrap());

        let (proxy_client, _client_peer) = tokio::io::duplex(128);
        let (proxy_upstream, upstream_peer) = tokio::io::duplex(128);
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        drop(upstream_peer);
        assert!(!relay_raw_tls_streams_with_cancel(
            proxy_client,
            proxy_upstream,
            BytesMut::new(),
            false,
            "REQ-raw-upstream-close".to_string(),
            None,
            cancel_rx,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn intercepted_html_covers_devtools_and_badge_injection_encodings() {
        let upstream = wiremock::MockServer::start().await;
        let html = b"<html><head></head><body><script src=\"/app.js\"></script></body></html>";
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/devtools"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(html.to_vec())
                    .insert_header("content-type", "text/html; charset=utf-8"),
            )
            .mount(&upstream)
            .await;
        let compressed = compress_body(html, "gzip").unwrap();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/devtools-gzip"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(compressed)
                    .insert_header("content-type", "text/html")
                    .insert_header("content-encoding", "gzip"),
            )
            .mount(&upstream)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/badge"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(html.to_vec())
                    .insert_header("content-type", "text/html"),
            )
            .mount(&upstream)
            .await;
        let badge_gzip = compress_body(html, "gzip").unwrap();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/badge-gzip"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(badge_gzip)
                    .insert_header("content-type", "text/html")
                    .insert_header("content-encoding", "gzip"),
            )
            .mount(&upstream)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/badge-invalid"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(b"not-gzip".to_vec())
                    .insert_header("content-type", "text/html")
                    .insert_header("content-encoding", "gzip"),
            )
            .mount(&upstream)
            .await;

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19453)
            .build();
        for (path, gzip) in [("/devtools", false), ("/devtools-gzip", true)] {
            let rules = ResolvedRules {
                host: Some(upstream.address().to_string()),
                host_protocol: Some(Protocol::Http),
                devtools: Some(crate::server::DevtoolsRule::default()),
                ..Default::default()
            };
            let request = Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::HOST, "source.test")
                .body(Full::new(Bytes::new()))
                .unwrap();
            let response =
                run_intercepted_request(rules, Some(harness.state()), request, 4096, 64, true)
                    .await;
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "no-store, no-cache, must-revalidate, max-age=0"
            );
            assert_eq!(response.headers()[header::PRAGMA], "no-cache");
            let body = body_bytes(response).await;
            let decoded = if gzip {
                crate::transform::decompress_body_with_limit(&body, Some("gzip"), 10 * 1024 * 1024)
            } else {
                body
            };
            let text = String::from_utf8(decoded.to_vec()).unwrap();
            assert!(text.contains("__bifrost_devtools_bridge__"));
            assert!(text.contains("__bifrost_client_req_id="));
        }

        for (path, encoding, expected) in [
            ("/badge", None, true),
            ("/badge-gzip", Some("gzip"), true),
            ("/badge-invalid", Some("gzip"), false),
        ] {
            let badge_rules = ResolvedRules {
                host: Some(upstream.address().to_string()),
                host_protocol: Some(Protocol::Http),
                ..Default::default()
            };
            let badge_request = Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::HOST, "source.test")
                .body(Full::new(Bytes::new()))
                .unwrap();
            let badge_response = run_intercepted_request_config(
                badge_rules,
                None,
                badge_request,
                4096,
                64,
                true,
                "source.test",
                443,
                true,
            )
            .await;
            let badge_body = body_bytes(badge_response).await;
            let decoded = crate::transform::decompress_body_with_limit(
                &badge_body,
                encoding,
                10 * 1024 * 1024,
            );
            assert_eq!(
                String::from_utf8_lossy(&decoded).contains("__bifrost_badge__"),
                expected
            );
        }
    }

    #[tokio::test]
    async fn intercepted_websocket_covers_plain_upstream_success_and_rejection() {
        for status in [101_u16, 403] {
            let (address, upstream_task) = websocket_upstream_response(status).await;
            let rules = ResolvedRules {
                host: Some(address.to_string()),
                host_protocol: Some(Protocol::Http),
                req_headers: vec![("x-coverage".into(), "request".into())],
                res_headers: vec![("x-coverage-response".into(), "yes".into())],
                ..Default::default()
            };
            let response = run_intercepted_request(
                rules,
                Some(Arc::new(AdminState::new(19451))),
                websocket_request(),
                4096,
                64,
                true,
            )
            .await;
            if status == 101 {
                assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
                assert_eq!(response.headers()["x-upstream"], "present");
                assert_eq!(response.headers()["x-coverage-response"], "yes");
                assert_eq!(response.headers()["sec-websocket-protocol"], "chat.v1");
            } else {
                assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            }
            let _ = body_bytes(response).await;
            upstream_task.await.unwrap();
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unavailable = listener.local_addr().unwrap();
        drop(listener);
        let rules = ResolvedRules {
            host: Some(unavailable.to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };
        let response =
            run_intercepted_request(rules, None, websocket_request(), 4096, 64, true).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn intercepted_websocket_covers_tls_upstream_handshake() {
        let (address, upstream_task) = websocket_tls_upstream_response().await;
        let rules = ResolvedRules {
            host: Some(address.to_string()),
            host_protocol: Some(Protocol::Https),
            upstream_unsafe_ssl: true,
            req_headers: vec![("x-coverage".into(), "request".into())],
            res_headers: vec![("x-tls-response".into(), "yes".into())],
            ..Default::default()
        };
        let response = run_intercepted_request(
            rules,
            Some(Arc::new(AdminState::new(19456))),
            websocket_request(),
            4096,
            64,
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(response.headers()["x-tls-upstream"], "yes");
        assert_eq!(response.headers()["x-tls-response"], "yes");
        assert_eq!(response.headers()["sec-websocket-protocol"], "chat.v1");
        let _ = body_bytes(response).await;
        upstream_task.await.unwrap();
    }

    #[tokio::test]
    async fn intercepted_request_breakpoint_header_only_preserves_large_chunked_body() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19459)
            .build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 8,
            });
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/large-request-breakpoint"))
            .and(wiremock::matchers::body_string("tunnel-large-request-body"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            rules: vec![breakpoint_rule("request")],
            ..Default::default()
        };
        let frames = futures_util::stream::iter(vec![
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(
                b"tunnel-large-",
            ))),
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(
                b"request-body",
            ))),
        ]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/large-request-breakpoint")
            .header(header::HOST, "source.test")
            .body(StreamBody::new(frames))
            .unwrap();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            run_intercepted_request(rules, Some(task_state), request, 4096, 64, true).await
        });
        wait_for_breakpoint(&state).await;
        let pending = state.breakpoint_manager.pending();
        assert!(pending[0].body_omitted);
        assert!(state
            .breakpoint_manager
            .resume("REQ-tunnel-coverage", "request", BreakpointEdit::default(),)
            .is_ok());
        let response = task.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn intercepted_breakpoint_request_url_routes_to_edited_upstream() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};

        let harness = bifrost_admin::test_support::TestAdminState::builder().build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 1024,
            });
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/edited-upstream"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("edited"))
            .expect(1)
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            rules: vec![breakpoint_rule("request")],
            ..Default::default()
        };
        let request = Request::builder()
            .uri("/default-port")
            .header(header::HOST, "source.test")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            run_intercepted_request(rules, Some(task_state), request, 4096, 64, true).await
        });
        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "request",
                BreakpointEdit {
                    url: Some(format!("{}/edited-upstream", upstream.uri())),
                    ..Default::default()
                },
            )
            .is_ok());
        let response = task.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn breakpoint_upstream_target_uses_scheme_defaults_and_preserves_authority() {
        let http = "http://example.test/path?value=1".parse().unwrap();
        assert_eq!(
            breakpoint_upstream_target(&http),
            Some((
                true,
                "example.test".into(),
                80,
                "/path?value=1".into(),
                "example.test".into(),
            ))
        );

        let https = "https://[::1]:9443".parse().unwrap();
        assert_eq!(
            breakpoint_upstream_target(&https),
            Some((false, "[::1]".into(), 9443, "/".into(), "[::1]:9443".into(),))
        );
        assert!(breakpoint_upstream_target(&"/relative".parse().unwrap()).is_none());
    }

    #[tokio::test]
    async fn intercepted_request_breakpoint_reports_stream_read_errors() {
        use bifrost_admin::breakpoint::BreakpointSettings;

        let harness = bifrost_admin::test_support::TestAdminState::builder().build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 8,
            });
        let rules = ResolvedRules {
            rules: vec![breakpoint_rule("request")],
            ..Default::default()
        };
        let frames = futures_util::stream::iter(vec![Err::<hyper::body::Frame<Bytes>, _>(
            std::io::Error::other("broken breakpoint body"),
        )]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/broken-breakpoint")
            .header(header::HOST, "source.test")
            .body(StreamBody::new(frames))
            .unwrap();
        let response = run_intercepted_request_config_result(
            rules,
            Some(state),
            request,
            4096,
            64,
            true,
            "source.test",
            443,
            false,
        )
        .await;
        assert!(
            response.is_err(),
            "broken body must terminate the client transport"
        );
    }

    #[tokio::test]
    async fn intercepted_response_breakpoint_header_only_preserves_large_body() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19459)
            .build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 8,
            });
        let upstream = wiremock::MockServer::start().await;
        let large_body = "tunnel-large-response-body";
        wiremock::Mock::given(wiremock::matchers::path("/large-breakpoint"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string(large_body),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            rules: vec![breakpoint_rule("response")],
            ..Default::default()
        };
        let request = Request::builder()
            .uri("/large-breakpoint")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            run_intercepted_request(rules, Some(task_state), request, 4096, 64, true).await
        });
        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "response",
                BreakpointEdit {
                    headers: Some(vec![("x-header-only".into(), "yes".into())]),
                    status: Some(219),
                    body: None,
                    ..Default::default()
                },
            )
            .is_ok());
        let response = task.await.unwrap();
        assert_eq!(response.status().as_u16(), 219);
        assert_eq!(response.headers()["x-header-only"], "yes");
        assert_eq!(body_bytes(response).await, Bytes::from(large_body));
    }

    #[tokio::test]
    async fn intercepted_header_only_response_breakpoint_no_content_clears_body_and_record() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19461)
            .build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 4,
            });
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/large-no-content"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("large-response-must-be-removed"),
            )
            .mount(&upstream)
            .await;
        let rules = ResolvedRules {
            host: Some(upstream.address().to_string()),
            host_protocol: Some(Protocol::Http),
            rules: vec![breakpoint_rule("response")],
            ..Default::default()
        };
        let request = Request::builder()
            .uri("/large-no-content")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            run_intercepted_request(rules, Some(task_state), request, 4096, 64, true).await
        });
        wait_for_breakpoint(&state).await;
        assert!(state
            .breakpoint_manager
            .resume(
                "REQ-tunnel-coverage",
                "response",
                BreakpointEdit {
                    status: Some(204),
                    ..Default::default()
                },
            )
            .is_ok());
        let response = task.await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
        assert!(!response.headers().contains_key(header::TRANSFER_ENCODING));
        assert!(body_bytes(response).await.is_empty());
        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("tunnel no-content traffic record");
        assert_eq!(record.status, 204);
        assert_eq!(record.response_size, 0);
        assert_eq!(record.download_bytes, 0);
        assert!(record.response_body_ref.is_none());
    }

    #[test]
    fn retryable_request_blueprint_rebuilds_http1_request() {
        let blueprint = RetryableRequestBlueprint {
            method: Method::PATCH,
            uri: "https://retry.test/resource".parse().unwrap(),
            headers: hyper::HeaderMap::from_iter([(
                header::HeaderName::from_static("x-retry"),
                header::HeaderValue::from_static("yes"),
            )]),
            body: Bytes::from_static(b"retry-body"),
        };
        let request = blueprint.build().unwrap();
        assert_eq!(request.version(), hyper::Version::HTTP_11);
        assert_eq!(request.method(), Method::PATCH);
        assert_eq!(request.uri(), "https://retry.test/resource");
        assert_eq!(request.headers()["x-retry"], "yes");
    }

    #[cfg(feature = "http3")]
    #[tokio::test]
    async fn intercepted_http3_attempt_falls_back_after_unavailable_quic_origin() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unavailable = listener.local_addr().unwrap();
        drop(listener);
        let rules = ResolvedRules {
            host: Some(unavailable.to_string()),
            host_protocol: Some(Protocol::Https),
            upstream_http3: true,
            upstream_unsafe_ssl: true,
            ..Default::default()
        };
        let request = Request::builder()
            .uri("/http3-fallback")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = run_intercepted_request(rules, None, request, 4096, 64, true).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn intercepted_direct_status_request_script_mutates_recorded_request() {
        use bifrost_admin::ScriptManager;
        use bifrost_script::ScriptType;

        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19460)
            .build();
        let manager = ScriptManager::new(harness.data_dir().join("tunnel-direct-request-script"));
        manager.init().await.unwrap();
        manager
            .engine()
            .save_script(
                ScriptType::Request,
                "tunnel-direct-request",
                r#"request.method = "PATCH"; request.headers["x-tunnel-scripted"] = "yes"; request.body = "tunnel-scripted-body";"#,
            )
            .await
            .unwrap();
        let state = Arc::new(
            AdminState::new(19460)
                .with_traffic_db_store_shared(harness.traffic_db.clone())
                .with_body_store(harness.body_store.clone())
                .with_config_manager_shared(harness.config_manager.clone())
                .with_script_manager(manager),
        );
        let rules = ResolvedRules {
            status_code: Some(219),
            req_scripts: vec!["tunnel-direct-request".to_string()],
            ..Default::default()
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/direct-script")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from_static(b"original")))
            .unwrap();
        let response = run_intercepted_request(rules, Some(state), request, 1024, 64, true).await;
        assert_eq!(response.status().as_u16(), 219);
        let record = harness
            .traffic_db
            .get_by_id("REQ-tunnel-coverage")
            .expect("tunnel direct scripted traffic");
        assert_eq!(record.method, "POST");
        assert!(record
            .req_script_results
            .as_ref()
            .is_some_and(|results| results.iter().any(|result| result.success)));
    }

    #[tokio::test]
    async fn intercepted_invalid_upstream_uri_and_host_prefix_rewrite_cover_errors_and_paths() {
        let invalid = ResolvedRules {
            host: Some("[invalid-host".to_string()),
            host_protocol: Some(Protocol::Http),
            ..Default::default()
        };
        let request = Request::builder()
            .uri("/invalid")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        assert_eq!(
            run_intercepted_request(invalid, None, request, 1024, 64, true)
                .await
                .status(),
            StatusCode::BAD_GATEWAY
        );

        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("tunnel-rewritten"))
            .mount(&upstream)
            .await;
        let target = format!("{}/base", upstream.address());
        let rule = crate::server::RuleValue {
            pattern: "source.test/api".to_string(),
            protocol: Protocol::Http,
            value: target.clone(),
            options: HashMap::new(),
            rule_name: Some("tunnel-prefix".to_string()),
            raw: None,
            line: Some(1),
            auto_tls_intercept: true,
        };
        let rules = ResolvedRules {
            host: Some(target),
            host_protocol: Some(Protocol::Http),
            rules: vec![rule],
            ..Default::default()
        };
        let request = Request::builder()
            .uri("/api/item")
            .header(header::HOST, "source.test")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = run_intercepted_request(rules, None, request, 1024, 64, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"tunnel-rewritten")
        );
        let requests = upstream.received_requests().await.unwrap();
        let path = requests[0].url.path();
        assert!(path.contains("base") && path.contains("item"), "{path}");
    }

    #[tokio::test]
    async fn intercepted_unknown_length_oversized_bodies_use_admin_streaming_tees() {
        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19461)
            .build();
        let (address, upstream_task) = chunked_http_fixture().await;
        let rules = ResolvedRules {
            host: Some(address.to_string()),
            host_protocol: Some(Protocol::Http),
            req_headers: vec![("content-encoding".to_string(), "gzip".to_string())],
            req_replace: vec![("old".to_string(), "new".to_string())],
            res_replace: vec![("chunked".to_string(), "new".to_string())],
            ..Default::default()
        };
        let frames = futures_util::stream::iter(vec![
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"tunnel-new-"))),
            Ok::<_, Infallible>(hyper::body::Frame::data(Bytes::from_static(b"body"))),
        ]);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/unknown-oversized")
            .header(header::HOST, "source.test")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(StreamBody::new(frames))
            .unwrap();
        let response =
            run_intercepted_request(rules, Some(harness.state()), request, 4, 2, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"chunked-tunnel")
        );
        upstream_task.abort();
    }
}
