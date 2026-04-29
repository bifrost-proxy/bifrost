use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use bifrost_admin::{
    devtools::{DevtoolsMode as AdminDevtoolsMode, MatchedDevtoolsRule, RegisterPageInput},
    AdminRouter, AdminState, RequestTiming, SharedPushManager, TrafficRecord, TrafficType,
    ADMIN_PATH_PREFIX,
};
use bifrost_core::{protocol::Protocol, BifrostError, Result};
use bifrost_script::{RequestData, ResponseData};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::header::HeaderValue;
use hyper::http::response::Parts as ResponseParts;
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
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
use crate::server::{
    full_body, with_trailers, BoxBody, DevtoolsInjectMode, DevtoolsMode, DevtoolsRule,
    ResolvedRules, RulesResolver, ADMIN_VIRTUAL_HOST,
};
use crate::transform::apply_req_rules;
use crate::transform::apply_res_rules;
use crate::transform::collect_all_cookies_from_headers;
use crate::transform::decompress_body_with_limit;
use crate::transform::{
    apply_body_rules, apply_content_injection, apply_content_injection_preserving_encoding,
    compress_body, maybe_inject_bifrost_badge_html, try_decompress_body_with_limit,
    ContentInjectionEncoding, Phase,
};
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
use crate::utils::url::{
    apply_url_rules, extract_target_path_from_host_rule, find_host_rule_source_path,
    rewrite_path_with_prefix,
};

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

#[allow(clippy::too_many_arguments)]
fn record_http_mock_traffic(
    state: &Arc<AdminState>,
    ctx: &RequestContext,
    method: &str,
    record_url: &str,
    uri: &Uri,
    start_time: &Instant,
    has_rules: bool,
    resolved_rules: &ResolvedRules,
    response: &Response<BoxBody>,
    request: &Request<Incoming>,
) {
    let total_ms = start_time.elapsed().as_millis() as u64;
    let mock_host = uri.host().unwrap_or("unknown").to_string();
    let req_headers_pairs = headers_to_pairs(request.headers());
    let mock_status = response.status().as_u16();
    let mock_res_headers = headers_to_pairs(response.headers());

    let traffic_type = get_traffic_type_from_url(record_url);
    state
        .metrics_collector
        .add_bytes_sent_by_type(traffic_type, 0);
    state
        .metrics_collector
        .increment_requests_by_type(traffic_type);

    let mut record = TrafficRecord::new(
        ctx.id_str().to_string(),
        method.to_string(),
        record_url.to_string(),
    );
    record.status = mock_status;
    record.duration_ms = total_ms;
    record.host = mock_host;
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
    record.client_ip = ctx.client_ip.clone();
    record.client_app = ctx.client_app.clone();
    record.client_pid = ctx.client_pid;
    record.client_path = ctx.client_path.clone();
    record.response_size = calculate_response_size(
        mock_status,
        record.original_response_headers.as_deref().unwrap_or(&[]),
        0,
    );
    state.record_traffic(record);
}

trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite> AsyncReadWrite for T {}
fn get_traffic_type_from_url(url: &str) -> TrafficType {
    if url.starts_with("https://") {
        TrafficType::Https
    } else {
        TrafficType::Http
    }
}

pub(crate) fn headers_pairs_equal_ignore_order(
    a: &[(String, String)],
    b: &[(String, String)],
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a_sorted: Vec<(&str, &str)> = a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut b_sorted: Vec<(&str, &str)> = b.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    a_sorted.sort();
    b_sorted.sort();
    a_sorted == b_sorted
}

pub(crate) fn headers_to_pairs(headers: &hyper::HeaderMap) -> Vec<(String, String)> {
    let mut pairs = Vec::with_capacity(headers.len());
    let mut cookie_values: Vec<&str> = Vec::new();
    let mut cookie_insert_pos: Option<usize> = None;

    for (key, value) in headers {
        if key == hyper::header::COOKIE {
            if cookie_insert_pos.is_none() {
                cookie_insert_pos = Some(pairs.len());
                pairs.push(("cookie".to_string(), String::new()));
            }
            cookie_values.push(value.to_str().unwrap_or(""));
        } else {
            pairs.push((key.to_string(), value.to_str().unwrap_or("").to_string()));
        }
    }

    if let Some(pos) = cookie_insert_pos {
        pairs[pos].1 = cookie_values.join("; ");
    }

    pairs
}

fn build_proxy_rule_url(proxy_rule: &str) -> Result<Url> {
    let normalized = if proxy_rule.starts_with("http://") || proxy_rule.starts_with("https://") {
        proxy_rule.to_string()
    } else {
        format!("http://{}", proxy_rule)
    };

    Url::parse(&normalized)
        .map_err(|e| BifrostError::Parse(format!("Invalid proxy rule '{}': {}", proxy_rule, e)))
}

fn proxy_authority(host: &str, port: u16) -> String {
    if port == 80 {
        host.to_string()
    } else {
        format!("{}:{}", host, port)
    }
}

fn build_upstream_proxy_auth_value(proxy_url: &Url) -> Option<String> {
    if proxy_url.username().is_empty() {
        return None;
    }

    let credentials = format!(
        "{}:{}",
        proxy_url.username(),
        proxy_url.password().unwrap_or_default()
    );
    Some(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(credentials)
    ))
}

fn build_proxy_forward_uri(
    processed_uri: &Uri,
    original_host: &str,
    original_port: u16,
    is_https: bool,
) -> Result<Uri> {
    if processed_uri.scheme().is_some() && processed_uri.host().is_some() {
        return Ok(processed_uri.clone());
    }

    let path = processed_uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let authority = if (is_https && original_port == 443) || (!is_https && original_port == 80) {
        original_host.to_string()
    } else {
        format!("{}:{}", original_host, original_port)
    };

    format!(
        "{}://{}{}",
        if is_https { "https" } else { "http" },
        authority,
        path
    )
    .parse()
    .map_err(|e| BifrostError::Network(format!("Invalid upstream proxy target URI: {}", e)))
}

async fn send_request_via_upstream_proxy(
    proxy_rule: &str,
    target_uri: Uri,
    mut parts: hyper::http::request::Parts,
    outgoing_body: BoxBody,
) -> Result<Response<BoxBody>> {
    let proxy_url = build_proxy_rule_url(proxy_rule)?;
    let proxy_host = proxy_url
        .host_str()
        .ok_or_else(|| BifrostError::Parse(format!("Missing proxy host in '{}'", proxy_rule)))?;
    let proxy_port = proxy_url.port().unwrap_or(80);
    let target_authority = target_uri
        .authority()
        .map(|authority| authority.as_str().to_string())
        .ok_or_else(|| {
            BifrostError::Network("Missing target authority for upstream proxy".to_string())
        })?;

    parts.uri = target_uri;
    sanitize_upstream_headers(&mut parts.headers);
    parts.headers.remove(hyper::header::HOST);
    parts.headers.insert(
        hyper::header::HOST,
        HeaderValue::from_str(&target_authority).map_err(|e| {
            BifrostError::Parse(format!(
                "Invalid target host header '{}': {}",
                target_authority, e
            ))
        })?,
    );

    if let Some(auth_value) = build_upstream_proxy_auth_value(&proxy_url) {
        parts.headers.insert(
            "proxy-authorization",
            HeaderValue::from_str(&auth_value).map_err(|e| {
                BifrostError::Parse(format!("Invalid upstream proxy auth header: {}", e))
            })?,
        );
    }

    let stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|e| {
            BifrostError::Network(format!(
                "Failed to connect to upstream proxy {}: {}",
                proxy_authority(proxy_host, proxy_port),
                e
            ))
        })?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = http1::handshake(io)
        .await
        .map_err(|e| BifrostError::Network(format!("Upstream proxy handshake failed: {}", e)))?;

    tokio::spawn(async move {
        if let Err(err) = conn.await {
            warn!("Upstream proxy connection closed with error: {}", err);
        }
    });

    sender
        .send_request(Request::from_parts(parts, outgoing_body))
        .await
        .map(|response| response.map(|body| body.boxed()))
        .map_err(|e| BifrostError::Network(format!("Upstream proxy request failed: {}", e)))
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

pub(super) fn devtools_bridge_requested(rules: &ResolvedRules) -> bool {
    effective_devtools_rule(rules)
        .as_ref()
        .map(|rule| {
            !rule.deny
                && matches!(
                    rule.inject,
                    DevtoolsInjectMode::Auto | DevtoolsInjectMode::Bridge
                )
        })
        .unwrap_or(false)
}

fn effective_devtools_rule(rules: &ResolvedRules) -> Option<DevtoolsRule> {
    if let Some(rule) = rules.devtools.clone() {
        return Some(rule);
    }
    let matched = rules
        .rules
        .iter()
        .rev()
        .find(|rule| rule.protocol == Protocol::DevTools)?;
    Some(parse_devtools_rule_value(&matched.value))
}

fn parse_devtools_rule_value(value: &str) -> DevtoolsRule {
    let mut rule = DevtoolsRule {
        raw_value: value.to_string(),
        ..Default::default()
    };

    for part in value.split([',', '&']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, raw_value) = part.split_once('=').unwrap_or((part, "true"));
        let key = key.trim();
        let raw_value = raw_value.trim();
        match key {
            "mode" if raw_value.eq_ignore_ascii_case("control") => {
                rule.mode = DevtoolsMode::Control;
            }
            "mode" if raw_value.eq_ignore_ascii_case("read") => {
                rule.mode = DevtoolsMode::Read;
            }
            "inject" if raw_value.eq_ignore_ascii_case("bridge") => {
                rule.inject = DevtoolsInjectMode::Bridge;
            }
            "inject" if raw_value.eq_ignore_ascii_case("off") => {
                rule.inject = DevtoolsInjectMode::Off;
            }
            "inject" if raw_value.eq_ignore_ascii_case("auto") => {
                rule.inject = DevtoolsInjectMode::Auto;
            }
            "deny" => {
                rule.deny = matches!(
                    raw_value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            _ => {}
        }
    }

    rule
}

fn admin_devtools_mode(rule: &crate::server::DevtoolsRule) -> AdminDevtoolsMode {
    match rule.mode {
        crate::server::DevtoolsMode::Read => AdminDevtoolsMode::Read,
        crate::server::DevtoolsMode::Control => AdminDevtoolsMode::Control,
    }
}

fn devtools_matched_rule(rules: &ResolvedRules) -> Option<MatchedDevtoolsRule> {
    rules
        .rules
        .iter()
        .rev()
        .find(|rule| rule.protocol == Protocol::DevTools)
        .map(|rule| MatchedDevtoolsRule {
            pattern: rule.pattern.clone(),
            raw: rule.raw.clone(),
            line: rule.line,
        })
}

fn origin_from_url(url: &str) -> String {
    Url::parse(url)
        .map(|parsed| {
            let scheme = parsed.scheme();
            let host = parsed.host_str().unwrap_or_default();
            if let Some(port) = parsed.port() {
                format!("{scheme}://{host}:{port}")
            } else {
                format!("{scheme}://{host}")
            }
        })
        .unwrap_or_default()
}

fn insert_devtools_bridge_script(html: &str, script: &str) -> String {
    let mut html = html.to_string();
    let lower = html.to_lowercase();

    if let Some(head_start) = lower.find("<head") {
        if let Some(head_end_offset) = lower[head_start..].find('>') {
            let insert_at = head_start + head_end_offset + 1;
            html.insert_str(insert_at, script);
            return html;
        }
    }

    if let Some(html_start) = lower.find("<html") {
        if let Some(html_end_offset) = lower[html_start..].find('>') {
            let insert_at = html_start + html_end_offset + 1;
            html.insert_str(insert_at, script);
            return html;
        }
    }

    format!("{script}{html}")
}

fn devtools_bridge_script(page_id: &str, token: &str) -> String {
    let endpoint = format!("/_bifrost/api/devtools/bridge/{page_id}");
    let endpoint_json = serde_json::to_string(&endpoint).unwrap_or_else(|_| "\"\"".to_string());
    let page_id_json = serde_json::to_string(page_id).unwrap_or_else(|_| "\"\"".to_string());
    let token_json = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r##"<script id="__bifrost_devtools_bridge__">
(function() {{
  if (window.__BIFROST_DEVTOOLS_BRIDGE__) return;
  const endpoint = {endpoint_json};
  const token = {token_json};
  const pageId = {page_id_json};
  const rawFetch = window.fetch ? window.fetch.bind(window) : null;
  const tabWindowNamePrefix = "__bifrost_devtools_tab_id__:";
  const tabStorageKey = "__bifrost_devtools_tab_id__";
  let tabId = "";
  try {{
    if (window.name && window.name.indexOf(tabWindowNamePrefix) === 0) {{
      tabId = window.name.slice(tabWindowNamePrefix.length);
    }}
    if (!tabId) {{
      tabId = "tab_" + Math.random().toString(36).slice(2) + Date.now().toString(36);
      window.name = tabWindowNamePrefix + tabId;
      window.sessionStorage.setItem(tabStorageKey, tabId);
    }}
  }} catch (_) {{
    tabId = "tab_" + Math.random().toString(36).slice(2) + Date.now().toString(36);
    try {{ window.name = tabWindowNamePrefix + tabId; }} catch (_) {{}}
  }}
  const post = function(path, payload) {{
    try {{
      if (!rawFetch) return;
      rawFetch(endpoint + path, {{
        method: "POST",
        headers: {{"Content-Type": "application/json"}},
        body: JSON.stringify(Object.assign({{token: token}}, payload || {{}}))
      }}).catch(function() {{}});
    }} catch (_) {{}}
  }};
  const bridge = {{
    page_id: pageId,
    tab_id: tabId,
    state: "connecting",
    post: post
  }};
  window.__BIFROST_DEVTOOLS_BRIDGE__ = bridge;
  let nextNodeId = 1;
  let nodeMap = Object.create(null);
  let domRefreshTimer = 0;
  let lastDomSignature = "";
  let lastStorageSnapshot = "";
  let highlightedNode = null;
  let highlightOverlay = null;
  const observedResources = Object.create(null);
  const internalNodeIds = {{
    "__bifrost_devtools_bridge__": true,
    "__bifrost_devtools_highlight__": true
  }};
  const isBridgeInternalNode = function(node) {{
    if (!node) return false;
    if (node.nodeType !== Node.ELEMENT_NODE) node = node.parentElement;
    if (!node) return false;
    if (internalNodeIds[node.id]) return true;
    try {{
      return !!(node.closest && node.closest("#__bifrost_devtools_bridge__,#__bifrost_devtools_highlight__"));
    }} catch (_) {{
      return false;
    }}
  }};
  const externalChildNodes = function(node) {{
    return Array.prototype.slice.call(node.childNodes || []).filter(function(child) {{
      return !isBridgeInternalNode(child);
    }});
  }};
  const sanitizedOuterHTML = function() {{
    try {{
      if (!document.documentElement) return "";
      const clone = document.documentElement.cloneNode(true);
      Array.prototype.slice.call(clone.querySelectorAll("#__bifrost_devtools_bridge__,#__bifrost_devtools_highlight__")).forEach(function(node) {{
        if (node.parentNode) node.parentNode.removeChild(node);
      }});
      return clone.outerHTML.slice(0, 1048576);
    }} catch (_) {{
      return document.documentElement ? document.documentElement.outerHTML.slice(0, 1048576) : "";
    }}
  }};
  const attrList = function(node) {{
    const attrs = [];
    if (!node || !node.attributes) return attrs;
    for (let i = 0; i < node.attributes.length; i++) {{
      attrs.push(node.attributes[i].name, node.attributes[i].value);
    }}
    return attrs;
  }};
  const serializeNode = function(node) {{
    const id = nextNodeId++;
    nodeMap[id] = node;
    const children = externalChildNodes(node);
    if (node.nodeType === Node.DOCUMENT_NODE) {{
      return {{
        nodeId: id,
        backendNodeId: id,
        nodeType: 9,
        nodeName: "#document",
        localName: "",
        nodeValue: "",
        documentURL: location.href,
        baseURL: document.baseURI || location.href,
        xmlVersion: "",
        compatibilityMode: document.compatMode === "BackCompat" ? "QuirksMode" : "NoQuirksMode",
        children: children.map(serializeNode)
      }};
    }}
    if (node.nodeType === Node.TEXT_NODE) {{
      return {{
        nodeId: id,
        backendNodeId: id,
        nodeType: 3,
        nodeName: "#text",
        localName: "",
        nodeValue: (node.nodeValue || "").slice(0, 4096)
      }};
    }}
    if (node.nodeType === Node.COMMENT_NODE) {{
      return {{
        nodeId: id,
        backendNodeId: id,
        nodeType: 8,
        nodeName: "#comment",
        localName: "",
        nodeValue: (node.nodeValue || "").slice(0, 4096)
      }};
    }}
    return {{
      nodeId: id,
      backendNodeId: id,
      nodeType: node.nodeType,
      nodeName: node.nodeName || "",
      localName: node.localName || "",
      nodeValue: node.nodeValue || "",
      attributes: attrList(node),
      childNodeCount: children.length,
      children: children.slice(0, 2500).map(serializeNode)
    }};
  }};
  const storageSnapshot = function() {{
    const collect = function(storage) {{
      const entries = [];
      try {{
        for (let i = 0; i < storage.length; i++) {{
          const key = storage.key(i);
          entries.push([key, storage.getItem(key)]);
        }}
      }} catch (_) {{}}
      return entries;
    }};
    const cookies = [];
    try {{
      if (document.cookie) {{
        document.cookie.split(";").forEach(function(part) {{
          const idx = part.indexOf("=");
          if (idx > -1) cookies.push([part.slice(0, idx).trim(), part.slice(idx + 1)]);
        }});
      }}
    }} catch (_) {{}}
    return {{
      local_storage: collect(window.localStorage),
      session_storage: collect(window.sessionStorage),
      cookies: cookies
    }};
  }};
  const performanceNetwork = function() {{
    try {{
      return performance.getEntriesByType("resource").filter(function(entry) {{
        return entry.name && entry.name.indexOf("/_bifrost/api/devtools/bridge/") === -1;
      }}).slice(-100).filter(function(entry) {{
        const key = entry.name + "::" + entry.startTime;
        if (observedResources[key]) return false;
        observedResources[key] = true;
        return true;
      }}).map(function(entry) {{
        return {{
          url: entry.name,
          method: "GET",
          status: 0,
          type: entry.initiatorType || "Other"
        }};
      }});
    }} catch (_) {{
      return [];
    }}
  }};
  const hello = function(includeDom, networkEvents) {{
    if (includeDom) {{
      nextNodeId = 1;
      nodeMap = Object.create(null);
    }}
    bridge.state = "connected";
    const payload = {{
      tab_id: tabId,
      title: document.title || null,
      url: location.href,
      user_agent: navigator.userAgent,
      storage: storageSnapshot(),
      network: networkEvents || []
    }};
    if (includeDom) {{
      const domTree = serializeNode(document);
      const domSignature = JSON.stringify(domTree);
      if (domSignature !== lastDomSignature) {{
        lastDomSignature = domSignature;
        payload.dom_snapshot = sanitizedOuterHTML();
        payload.dom_tree = domTree;
      }}
    }}
    post("/hello", payload);
  }};
  const sendStorageIfChanged = function() {{
    try {{
      const snapshot = storageSnapshot();
      const serialized = JSON.stringify(snapshot);
      if (serialized === lastStorageSnapshot) return;
      lastStorageSnapshot = serialized;
      hello(false, []);
    }} catch (_) {{}}
  }};
  const scheduleDomRefresh = function(delay) {{
    if (domRefreshTimer) return;
    domRefreshTimer = window.setTimeout(function() {{
      domRefreshTimer = 0;
      hello(true, performanceNetwork());
    }}, delay || 250);
  }};
  const isExternalStructuralMutation = function(mutation) {{
    if (!mutation || mutation.type !== "childList") return false;
    if (isBridgeInternalNode(mutation.target)) return false;
    const added = Array.prototype.slice.call(mutation.addedNodes || []);
    const removed = Array.prototype.slice.call(mutation.removedNodes || []);
    return added.concat(removed).some(function(node) {{
      return !isBridgeInternalNode(node);
    }});
  }};
  const ensureHighlightOverlay = function() {{
    if (highlightOverlay && document.documentElement.contains(highlightOverlay)) return highlightOverlay;
    highlightOverlay = document.createElement("div");
    highlightOverlay.id = "__bifrost_devtools_highlight__";
    highlightOverlay.style.cssText = [
      "position:fixed",
      "z-index:2147483647",
      "pointer-events:none",
      "border:2px solid #1677ff",
      "box-shadow:0 0 0 99999px rgba(22,119,255,0.08),0 0 0 1px rgba(255,255,255,0.85) inset",
      "border-radius:2px",
      "box-sizing:border-box",
      "display:none"
    ].join(";");
    (document.documentElement || document.body).appendChild(highlightOverlay);
    return highlightOverlay;
  }};
  const updateHighlightOverlay = function() {{
    if (!highlightedNode || !highlightedNode.getBoundingClientRect) return;
    const rect = highlightedNode.getBoundingClientRect();
    const overlay = ensureHighlightOverlay();
    if (rect.width <= 0 && rect.height <= 0) {{
      overlay.style.display = "none";
      return;
    }}
    overlay.style.display = "block";
    overlay.style.left = Math.max(0, rect.left) + "px";
    overlay.style.top = Math.max(0, rect.top) + "px";
    overlay.style.width = Math.max(1, rect.width) + "px";
    overlay.style.height = Math.max(1, rect.height) + "px";
  }};
  const highlightNode = function(nodeId) {{
    const node = nodeMap[nodeId];
    highlightedNode = node && node.nodeType === Node.TEXT_NODE ? node.parentElement : node;
    updateHighlightOverlay();
  }};
  const hideHighlight = function() {{
    highlightedNode = null;
    if (highlightOverlay) highlightOverlay.style.display = "none";
  }};
  const pollOverlay = function() {{
    try {{
      if (!rawFetch) return;
      rawFetch(endpoint + "/overlay-next", {{
        method: "POST",
        headers: {{"Content-Type": "application/json"}},
        body: JSON.stringify({{token: token}})
      }}).then(function(response) {{
        return response.ok ? response.json() : null;
      }}).then(function(payload) {{
        const command = payload && payload.command;
        if (!command) return;
        if (command.type === "highlight_node") highlightNode(command.node_id);
        if (command.type === "hide_highlight") hideHighlight();
      }}).catch(function() {{}});
    }} catch (_) {{}}
  }};
  const stringifyArgs = function(args) {{
    return Array.prototype.slice.call(args).map(function(value) {{
      try {{
        return typeof value === "string" ? value : JSON.stringify(value);
      }} catch (_) {{
        return String(value);
      }}
    }}).join(" ");
  }};
  ["log", "info", "warn", "error", "debug"].forEach(function(level) {{
    const original = console[level];
    console[level] = function() {{
      try {{
        post("/console", {{level: level, text: stringifyArgs(arguments)}});
      }} catch (_) {{}}
      return original.apply(console, arguments);
    }};
  }});
  const recordNetwork = function(event) {{
    if (!event || !event.url || event.url.indexOf("/_bifrost/api/devtools/bridge/") !== -1) return;
    post("/network", {{event: event}});
  }};
  try {{
    if (window.PerformanceObserver) {{
      const resourceObserver = new PerformanceObserver(function(list) {{
        list.getEntries().forEach(function(entry) {{
          if (!entry || !entry.name || entry.name.indexOf("/_bifrost/api/devtools/bridge/") !== -1) return;
          const key = entry.name + "::" + entry.startTime;
          if (observedResources[key]) return;
          observedResources[key] = true;
          recordNetwork({{url: entry.name, method: "GET", status: 0, type: entry.initiatorType || "Other"}});
        }});
      }});
      resourceObserver.observe({{type: "resource", buffered: true}});
    }}
  }} catch (_) {{}}
  try {{
    if (window.Storage && window.Storage.prototype) {{
      ["setItem", "removeItem", "clear"].forEach(function(method) {{
        const original = window.Storage.prototype[method];
        if (typeof original !== "function") return;
        window.Storage.prototype[method] = function() {{
          const result = original.apply(this, arguments);
          sendStorageIfChanged();
          return result;
        }};
      }});
    }}
  }} catch (_) {{}}
  window.addEventListener("storage", sendStorageIfChanged, true);
  const remoteObject = function(value) {{
    if (value === undefined) return {{type: "undefined", description: "undefined"}};
    if (value === null) return {{type: "object", subtype: "null", value: null, description: "null"}};
    const valueType = typeof value;
    if (valueType === "string" || valueType === "boolean" || valueType === "number") {{
      return {{type: valueType, value: value, description: String(value)}};
    }}
    if (valueType === "bigint") {{
      return {{type: "bigint", unserializableValue: String(value) + "n", description: String(value) + "n"}};
    }}
    if (valueType === "function") {{
      return {{type: "function", description: String(value).slice(0, 4096)}};
    }}
    let description = "";
    try {{ description = JSON.stringify(value); }} catch (_) {{ description = String(value); }}
    return {{type: "object", description: (description || String(value)).slice(0, 4096)}};
  }};
  const pollEval = function() {{
    try {{
      if (!rawFetch) return;
      rawFetch(endpoint + "/eval-next", {{
        method: "POST",
        headers: {{"Content-Type": "application/json"}},
        body: JSON.stringify({{token: token}})
      }}).then(function(response) {{
        return response.ok ? response.json() : null;
      }}).then(function(payload) {{
        const command = payload && payload.command;
        if (!command) return;
        try {{
          const value = (0, eval)(command.expression);
          Promise.resolve(value).then(function(resolved) {{
            post("/eval-result", {{eval_id: command.eval_id, result: remoteObject(resolved)}});
          }}, function(error) {{
            post("/eval-result", {{eval_id: command.eval_id, exception: String(error && error.stack || error)}});
          }});
        }} catch (error) {{
          post("/eval-result", {{eval_id: command.eval_id, exception: String(error && error.stack || error)}});
        }}
      }}).catch(function() {{}});
    }} catch (_) {{}}
  }};
  const originalFetch = window.fetch;
  if (originalFetch) {{
    window.fetch = function(input, init) {{
      const url = typeof input === "string" ? input : (input && input.url) || "";
      const method = (init && init.method) || (input && input.method) || "GET";
      return originalFetch.apply(this, arguments).then(function(response) {{
        try {{ recordNetwork({{url: response.url || url, method: method, status: response.status, type: "Fetch"}}); }} catch (_) {{}}
        return response;
      }});
    }};
  }}
  const OriginalXHR = window.XMLHttpRequest;
  if (OriginalXHR) {{
    window.XMLHttpRequest = function() {{
      const xhr = new OriginalXHR();
      let url = "";
      let method = "GET";
      const open = xhr.open;
      xhr.open = function(m, u) {{
        method = m || "GET";
        url = u || "";
        return open.apply(xhr, arguments);
      }};
      xhr.addEventListener("loadend", function() {{
        try {{ recordNetwork({{url: xhr.responseURL || url, method: method, status: xhr.status, type: "XHR"}}); }} catch (_) {{}}
      }});
      return xhr;
    }};
  }}
  try {{
    const observer = new MutationObserver(function(mutations) {{
      if (mutations.some(isExternalStructuralMutation)) scheduleDomRefresh(250);
    }});
    observer.observe(document.documentElement || document, {{
      childList: true,
      subtree: true
    }});
  }} catch (_) {{}}
  window.addEventListener("resize", updateHighlightOverlay, true);
  window.addEventListener("scroll", updateHighlightOverlay, true);
  window.setInterval(pollEval, 250);
  window.setInterval(pollOverlay, 100);
  if (document.readyState === "loading") {{
    document.addEventListener("DOMContentLoaded", function() {{
      lastStorageSnapshot = JSON.stringify(storageSnapshot());
      hello(true, performanceNetwork());
    }}, {{once: true}});
  }} else {{
    lastStorageSnapshot = JSON.stringify(storageSnapshot());
    hello(true, performanceNetwork());
  }}
}})();
</script>"##
    )
}

pub(super) fn maybe_inject_devtools_bridge_html(
    body: Bytes,
    content_type: &str,
    rules: &ResolvedRules,
    state: Option<&AdminState>,
    record_url: &str,
    traffic_id: &str,
) -> Bytes {
    if !content_type.to_ascii_lowercase().starts_with("text/html")
        || !devtools_bridge_requested(rules)
    {
        return body;
    }
    let Some(state) = state else {
        return body;
    };
    let Some(devtools_rule) = effective_devtools_rule(rules) else {
        return body;
    };

    let input = RegisterPageInput {
        url: record_url.to_string(),
        origin: origin_from_url(record_url),
        traffic_id: traffic_id.to_string(),
        mode: admin_devtools_mode(&devtools_rule),
        matched_rule: devtools_matched_rule(rules),
    };
    let (page_id, token) = state.devtools_broker.register_page_candidate(input);
    let script = devtools_bridge_script(&page_id, &token);
    let html = String::from_utf8_lossy(&body);
    Bytes::from(insert_devtools_bridge_script(&html, &script))
}

pub fn needs_response_override(rules: &ResolvedRules) -> bool {
    rules.res_body.is_some() || rules.status_code.is_some() || rules.replace_status.is_some()
}

async fn apply_immediate_response_body_rules(
    response: Response<BoxBody>,
    rules: &ResolvedRules,
    method: &str,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Result<(Response<BoxBody>, Bytes)> {
    let (mut parts, body) = response.into_parts();
    let content_type = parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body_bytes = body
        .collect()
        .await
        .map_err(|e| BifrostError::Network(format!("Failed to read immediate response: {}", e)))?
        .to_bytes();
    let body_processed = apply_body_rules(
        body_bytes,
        rules,
        Phase::Response,
        Some(&content_type),
        verbose_logging,
        ctx,
    );
    let final_body =
        apply_content_injection(body_processed, &content_type, rules, verbose_logging, ctx);
    normalize_res_headers(&mut parts, BodyMode::Known(final_body.len()), method);

    Ok((
        Response::from_parts(parts, full_body(final_body.clone())),
        final_body,
    ))
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

fn normalize_req_headers(
    parts: &mut hyper::http::request::Parts,
    mode: BodyMode,
    had_content_length: bool,
) {
    match mode {
        BodyMode::Known(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            if len > 0 || had_content_length {
                parts.headers.insert(
                    hyper::header::CONTENT_LENGTH,
                    HeaderValue::from_str(&len.to_string()).unwrap(),
                );
            }
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

pub fn build_error_body(status_code: u16, error_info: &ConnectionErrorInfo) -> Bytes {
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
    inject_bifrost_badge: bool,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    push_manager: Option<SharedPushManager>,
    dns_resolver: Option<Arc<DnsResolver>>,
) -> Result<Response<BoxBody>> {
    if is_websocket_upgrade(&req) {
        return handle_http_websocket(req, rules, ctx, admin_state, push_manager, unsafe_ssl).await;
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
    let incoming_cookies: HashMap<String, String> = collect_all_cookies_from_headers(req.headers());

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

    if let Some(mut mock_response) =
        generate_mock_response(&resolved_rules, &uri, verbose_logging, ctx).await
    {
        let transformed_mock_body = if needs_body_processing(&resolved_rules) {
            let (response, body) = apply_immediate_response_body_rules(
                mock_response,
                &resolved_rules,
                &method,
                verbose_logging,
                ctx,
            )
            .await?;
            mock_response = response;
            Some(body)
        } else {
            None
        };

        if verbose_logging {
            info!("[{}] [MOCK] returning mock response", ctx.id_str());
        }

        if let Some(ref state) = admin_state {
            let total_ms = start_time.elapsed().as_millis() as u64;
            let mock_host = uri.host().unwrap_or("unknown").to_string();
            let req_headers_pairs = headers_to_pairs(req.headers());
            let mock_status = mock_response.status().as_u16();
            let mock_res_headers = headers_to_pairs(mock_response.headers());
            let mock_res_body = transformed_mock_body
                .clone()
                .or_else(|| resolved_rules.res_body.clone())
                .unwrap_or_else(|| {
                    Bytes::from(
                        hyper::StatusCode::from_u16(mock_status)
                            .ok()
                            .and_then(|s| s.canonical_reason())
                            .unwrap_or(""),
                    )
                });
            let mock_body_len = mock_res_body.len();

            let traffic_type = get_traffic_type_from_url(&record_url);
            state
                .metrics_collector
                .add_bytes_sent_by_type(traffic_type, 0);
            state
                .metrics_collector
                .increment_requests_by_type(traffic_type);

            let mut record =
                TrafficRecord::new(ctx.id_str().to_string(), method.clone(), record_url.clone());
            record.status = mock_status;
            record.duration_ms = total_ms;
            record.host = mock_host;
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
            record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
            record.client_ip = ctx.client_ip.clone();
            record.client_app = ctx.client_app.clone();
            record.client_pid = ctx.client_pid;
            record.client_path = ctx.client_path.clone();
            record.response_body_ref = if let Some(ref body_store) = state.body_store {
                let store = body_store.read();
                store.store(ctx.id_str(), "res", mock_res_body.as_ref())
            } else {
                store_response_body(&admin_state, ctx.id_str(), &mock_res_body)
            };
            record.response_size = calculate_response_size(
                mock_status,
                record.original_response_headers.as_deref().unwrap_or(&[]),
                mock_body_len,
            );
            state.record_traffic(record);
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
        let response = build_redirect_response(status, redirect_url);
        if let Some(ref state) = admin_state {
            record_http_mock_traffic(
                state,
                ctx,
                &method,
                &record_url,
                &uri,
                &start_time,
                has_rules,
                &resolved_rules,
                &response,
                &req,
            );
        }
        return Ok(response);
    }

    if let Some(ref location) = resolved_rules.location_href {
        if verbose_logging {
            info!("[{}] [LOCATION] {} -> {}", ctx.id_str(), url, location);
        }
        let response = build_redirect_response(301, location);
        if let Some(ref state) = admin_state {
            record_http_mock_traffic(
                state,
                ctx,
                &method,
                &record_url,
                &uri,
                &start_time,
                has_rules,
                &resolved_rules,
                &response,
                &req,
            );
        }
        return Ok(response);
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

    let request_origin = parts
        .headers
        .get(hyper::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let original_req_headers = admin_state
        .as_ref()
        .map(|_| headers_to_pairs(&parts.headers));

    let req_content_encoding = header_content_encoding(&parts.headers);

    apply_req_rules(&mut parts, &resolved_rules, verbose_logging, ctx);

    if parts.headers.get_all(hyper::header::COOKIE).iter().count() > 1 {
        let merged = crate::transform::merge_cookie_header_values(&parts.headers);
        parts.headers.remove(hyper::header::COOKIE);
        if !merged.is_empty() {
            if let Ok(v) = merged.parse::<HeaderValue>() {
                parts.headers.insert(hyper::header::COOKIE, v);
            }
        }
    }

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
    normalize_req_headers(&mut parts, req_body_mode, content_length.is_some());
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
                {
                    let orig = original_req_headers
                        .as_ref()
                        .expect("request headers captured when admin state is enabled");
                    if !headers_pairs_equal_ignore_order(orig, &req_headers) {
                        record.original_request_headers = Some(orig.clone());
                    }
                }
                record.request_headers = Some(req_headers.clone());
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

                {
                    let mut res_header_pairs: Vec<(String, String)> = Vec::new();
                    if needs_response_override(&resolved_rules) {
                        for (name, value) in &resolved_rules.res_headers {
                            res_header_pairs.push((name.clone(), value.clone()));
                        }
                        if resolved_rules.res_body.is_none() {
                            res_header_pairs.push((
                                "content-type".to_string(),
                                "text/plain; charset=utf-8".to_string(),
                            ));
                            res_header_pairs
                                .push(("x-bifrost-error".to_string(), error_type.to_string()));
                        }
                    } else {
                        res_header_pairs.push((
                            "content-type".to_string(),
                            "text/plain; charset=utf-8".to_string(),
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

    let path = {
        let original_path = processed_uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        if let Some(ref host_rule) = resolved_rules.host {
            if let Some(target_path) =
                crate::utils::url::extract_target_path_from_host_rule(host_rule)
            {
                let source_path = crate::utils::url::find_host_rule_source_path(
                    &resolved_rules.rules,
                    resolved_rules.host_protocol.unwrap_or(Protocol::Host),
                    host_rule,
                );
                crate::utils::url::rewrite_path_with_prefix(
                    original_path,
                    source_path.as_deref(),
                    &target_path,
                )
            } else {
                original_path.to_string()
            }
        } else {
            original_path.to_string()
        }
    };

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
    let req_headers_for_h3: Vec<(String, String)> = headers_to_pairs(&parts.headers);

    #[cfg(feature = "http3")]
    let use_upstream_proxy = should_use_upstream_proxy(&resolved_rules);

    let should_try_http3_upstream = use_tls
        && resolved_rules.upstream_http3
        && !request_body_is_streaming
        && dns_resolver.is_some()
        && !use_upstream_proxy
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
    let upstream_result =
        if let Some(proxy_rule) = resolved_rules.proxy.as_ref().filter(|_| use_upstream_proxy) {
            let send_start = Instant::now();
            let proxy_target_uri =
                build_proxy_forward_uri(&processed_uri, &original_host, original_port, is_https)?;
            let (outgoing_parts, outgoing_body) = outgoing_req.into_parts();
            let res = match send_request_via_upstream_proxy(
                proxy_rule,
                proxy_target_uri,
                outgoing_parts,
                outgoing_body,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    let error_message = e.to_string();
                    error!("[{}] {}", ctx.id_str(), error_message);
                    return Ok(build_conn_error_and_record(
                        "REQUEST_PROXY_FAILED",
                        error_message,
                        None,
                    ));
                }
            };
            let wait_ms = send_start.elapsed().as_millis() as u64;
            let (parts, body) = res.into_parts();
            (parts, Some(body), None, None, wait_ms)
        } else if let Some((res, wait_ms)) = h3_attempt {
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
    let upstream_result =
        if let Some(proxy_rule) = resolved_rules.proxy.as_ref().filter(|_| use_upstream_proxy) {
            let send_start = Instant::now();
            let proxy_target_uri =
                build_proxy_forward_uri(&processed_uri, &original_host, original_port, is_https)?;
            let (outgoing_parts, outgoing_body) = outgoing_req.into_parts();
            let res = match send_request_via_upstream_proxy(
                proxy_rule,
                proxy_target_uri,
                outgoing_parts,
                outgoing_body,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    let error_message = e.to_string();
                    error!("[{}] {}", ctx.id_str(), error_message);
                    return Ok(build_conn_error_and_record(
                        "REQUEST_PROXY_FAILED",
                        error_message,
                        None,
                    ));
                }
            };
            let wait_ms = send_start.elapsed().as_millis() as u64;
            let (parts, body) = res.into_parts();
            (parts, Some(body), None, None, wait_ms)
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

    apply_res_rules(
        &mut res_parts,
        &resolved_rules,
        verbose_logging,
        ctx,
        request_origin.as_deref(),
    );
    let output_res_content_encoding = response_content_encoding(&res_parts);

    let res_content_type = get_content_type(&res_parts);
    let force_body_processing_for_badge =
        inject_bifrost_badge && res_content_type.starts_with("text/html");
    let force_body_processing_for_devtools =
        devtools_bridge_requested(&resolved_rules) && res_content_type.starts_with("text/html");
    let needs_processing = needs_body_processing(&resolved_rules)
        || force_body_processing_for_badge
        || force_body_processing_for_devtools;
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

    let is_sse = is_sse_response(&res_parts);
    let binary_traffic_performance_mode = admin_state
        .as_ref()
        .map(|state| state.get_binary_traffic_performance_mode())
        .unwrap_or(false);
    let skip_binary_recording =
        should_use_binary_performance_mode(&res_parts, binary_traffic_performance_mode)
            && !is_websocket
            && !is_sse
            && !needs_processing;
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
        let skip_detail = if force_body_processing_for_badge {
            "skipping body rules and badge injection"
        } else if force_body_processing_for_devtools {
            "skipping body rules and DevTools bridge injection"
        } else {
            "skipping body rules"
        };
        warn!(
            "[{}] [RES_BODY] body too large ({} bytes > {} limit), {}, streaming forward",
            ctx.id_str(),
            size_display,
            res_body_limit,
            skip_detail
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
                record.original_response_headers = Some(original_res_headers.clone());
                if res_headers != *original_res_headers {
                    record.response_headers = Some(res_headers.clone());
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

        let injection_result = apply_content_injection_preserving_encoding(
            body_processed,
            &content_type,
            ContentInjectionEncoding {
                source: res_content_encoding.as_deref(),
                output: output_res_content_encoding.as_deref(),
                max_decompress_output_bytes,
            },
            &resolved_rules,
            verbose_logging,
            ctx,
        );
        res_parts.headers.remove(hyper::header::CONTENT_ENCODING);
        if let Some(content_encoding) = injection_result.content_encoding.as_deref() {
            if let Ok(value) = hyper::header::HeaderValue::from_str(content_encoding) {
                res_parts
                    .headers
                    .insert(hyper::header::CONTENT_ENCODING, value);
            }
        }
        injection_result.body
    };

    if content_type.to_ascii_lowercase().starts_with("text/html")
        && devtools_bridge_requested(&resolved_rules)
    {
        res_parts.headers.insert(
            hyper::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
        );
        res_parts
            .headers
            .insert(hyper::header::PRAGMA, HeaderValue::from_static("no-cache"));
        let final_content_encoding = response_content_encoding(&res_parts);
        if let Some(content_encoding) = final_content_encoding.as_deref() {
            match try_decompress_body_with_limit(
                final_res_body.as_ref(),
                content_encoding,
                max_decompress_output_bytes,
            ) {
                Ok(decompressed) => {
                    let injected_body = maybe_inject_devtools_bridge_html(
                        Bytes::from(decompressed),
                        &content_type,
                        &resolved_rules,
                        admin_state.as_deref(),
                        &record_url,
                        ctx.id_str(),
                    );
                    match compress_body(injected_body.as_ref(), content_encoding) {
                        Ok(compressed) => {
                            final_res_body = Bytes::from(compressed);
                        }
                        Err(e) => {
                            tracing::debug!(
                                "[{}] [DEVTOOLS] Failed to recompress response body ({}), fallback to identity",
                                ctx.id_str(),
                                e
                            );
                            res_parts.headers.remove(hyper::header::CONTENT_ENCODING);
                            final_res_body = injected_body;
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        "[{}] [DEVTOOLS] Skip bridge injection: failed to decompress response body ({}).",
                        ctx.id_str(),
                        e
                    );
                }
            }
        } else {
            final_res_body = maybe_inject_devtools_bridge_html(
                final_res_body,
                &content_type,
                &resolved_rules,
                admin_state.as_deref(),
                &record_url,
                ctx.id_str(),
            );
        }
    }

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

    if inject_bifrost_badge {
        let badge_rules_json = build_badge_rules_json(admin_state.as_deref());
        let final_res_content_type = get_content_type(&res_parts);
        if final_res_content_type.starts_with("text/html") {
            if let Some(content_encoding) = response_content_encoding(&res_parts) {
                match try_decompress_body_with_limit(
                    final_res_body.as_ref(),
                    &content_encoding,
                    max_decompress_output_bytes,
                ) {
                    Ok(decompressed) => {
                        let (injected_body, injected) = maybe_inject_bifrost_badge_html(
                            Bytes::from(decompressed),
                            &badge_rules_json,
                        );
                        if injected {
                            match compress_body(injected_body.as_ref(), &content_encoding) {
                                Ok(compressed) => {
                                    final_res_body = Bytes::from(compressed);
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "[{}] [BADGE] Failed to recompress response body ({}), fallback to identity",
                                        ctx.id_str(),
                                        e
                                    );
                                    res_parts.headers.remove(hyper::header::CONTENT_ENCODING);
                                    final_res_body = injected_body;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "[{}] [BADGE] Skip badge injection: failed to decompress response body ({}).",
                            ctx.id_str(),
                            e
                        );
                    }
                }
            } else {
                let (injected_body, injected) =
                    maybe_inject_bifrost_badge_html(final_res_body.clone(), &badge_rules_json);
                if injected {
                    final_res_body = injected_body;
                }
            }
        }
    }

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
        record.original_response_headers = Some(original_res_headers.clone());
        if res_headers != *original_res_headers {
            record.response_headers = Some(res_headers.clone());
        }
        {
            let orig = original_req_headers
                .as_ref()
                .expect("request headers captured when admin state is enabled");
            if !headers_pairs_equal_ignore_order(orig, &req_headers) {
                record.original_request_headers = Some(orig.clone());
            }
        }
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
        if let Ok(url) = build_proxy_rule_url(proxy_rule) {
            if let Some(host) = url.host_str() {
                let port = url.port().unwrap_or(80);
                return Ok((host.to_string(), port));
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

fn should_use_upstream_proxy(rules: &ResolvedRules) -> bool {
    rules.proxy.is_some() && (rules.ignored.host || rules.host.is_none())
}

fn get_default_port(host_protocol: &Option<Protocol>, is_https: bool) -> u16 {
    match host_protocol {
        Some(Protocol::Http) | Some(Protocol::Ws) => 80,
        Some(Protocol::Https) | Some(Protocol::Wss) => 443,
        None | Some(Protocol::Host) if is_https => 443,
        None | Some(Protocol::Host) => 80,
        _ => 80,
    }
}

async fn handle_http_websocket(
    req: Request<Incoming>,
    rules: Arc<dyn RulesResolver>,
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
    push_manager: Option<SharedPushManager>,
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

    if should_route_websocket_to_local_admin(&host, port, uri.path(), ctx.port) {
        if let (Some(state), Some(push_manager)) = (admin_state.clone(), push_manager.clone()) {
            let req = rewrite_local_admin_websocket_request(req, &host);
            let peer_addr = peer_addr_from_client_ip(&ctx.client_ip);
            return Ok(AdminRouter::handle(req, state, Some(push_manager), peer_addr).await);
        }
    }

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

    let req_headers: Vec<(String, String)> = headers_to_pairs(req.headers());

    let (target_host, target_port, target_path) = if let Some(ref host_rule) = resolved_rules.host {
        let host_without_path = host_rule.split('/').next().unwrap_or(host_rule);
        let parts: Vec<&str> = host_without_path.split(':').collect();
        let h = parts[0].to_string();
        let p = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(port);
        let path = if let Some(target_path) = extract_target_path_from_host_rule(host_rule) {
            let source_path = find_host_rule_source_path(
                &resolved_rules.rules,
                resolved_rules.host_protocol.unwrap_or(Protocol::Host),
                host_rule,
            );
            rewrite_path_with_prefix(
                req.uri()
                    .path_and_query()
                    .map(|pq| pq.as_str())
                    .unwrap_or("/"),
                source_path.as_deref(),
                &target_path,
            )
        } else {
            req.uri()
                .path_and_query()
                .map(|pq| pq.as_str())
                .unwrap_or("/")
                .to_string()
        };
        (h, p, path)
    } else {
        (
            host.to_string(),
            port,
            req.uri()
                .path_and_query()
                .map(|pq| pq.as_str())
                .unwrap_or("/")
                .to_string(),
        )
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

    let upgrade_request =
        build_http_websocket_handshake(&req, &target_host, target_port, &target_path)?;
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
        record.original_response_headers = Some(response_headers.clone());
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

fn should_route_websocket_to_local_admin(
    host: &str,
    port: u16,
    path: &str,
    listener_port: u16,
) -> bool {
    if host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
        return true;
    }

    if port != listener_port || !path.starts_with(ADMIN_PATH_PREFIX) {
        return false;
    }

    let host = host.trim_matches(|c| c == '[' || c == ']');
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn rewrite_local_admin_websocket_request<T>(req: Request<T>, host: &str) -> Request<T> {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path();

    if host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) && !path.starts_with(ADMIN_PATH_PREFIX) {
        let new_path = if path == "/" {
            format!("{ADMIN_PATH_PREFIX}/")
        } else {
            format!("{ADMIN_PATH_PREFIX}{path}")
        };
        let new_uri = if let Some(query) = parts.uri.query() {
            format!("{new_path}?{query}")
        } else {
            new_path
        };
        if let Ok(uri) = new_uri.parse() {
            parts.uri = uri;
        }
    }

    Request::from_parts(parts, body)
}

fn peer_addr_from_client_ip(client_ip: &str) -> Option<SocketAddr> {
    client_ip
        .parse::<IpAddr>()
        .ok()
        .map(|ip| SocketAddr::new(ip, 0))
}

fn build_http_websocket_handshake(
    req: &Request<Incoming>,
    target_host: &str,
    target_port: u16,
    target_path: &str,
) -> Result<String> {
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

pub(crate) fn build_badge_rules_json(admin_state: Option<&AdminState>) -> String {
    match admin_state {
        Some(s) => s.badge_rules_json(),
        None => r#"{"rules":[],"merged_content":"","admin_port":0}"#.to_string(),
    }
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
    fn test_extract_host_port_proxy_rule_with_auth() {
        let uri: Uri = "http://example.com/path".parse().unwrap();
        let rules = ResolvedRules {
            proxy: Some("user:pass@127.0.0.1:9090".to_string()),
            ..Default::default()
        };
        let (host, port) = extract_host_port(&uri, &rules, false).unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9090);
    }

    #[test]
    fn test_should_use_upstream_proxy_when_only_proxy_rule_exists() {
        let rules = ResolvedRules {
            proxy: Some("127.0.0.1:9090".to_string()),
            ..Default::default()
        };
        assert!(should_use_upstream_proxy(&rules));
    }

    #[test]
    fn test_should_not_use_upstream_proxy_when_host_rule_also_exists() {
        let rules = ResolvedRules {
            host: Some("127.0.0.1:3000".to_string()),
            proxy: Some("127.0.0.1:9090".to_string()),
            ..Default::default()
        };
        assert!(!should_use_upstream_proxy(&rules));
    }

    #[test]
    fn test_should_use_upstream_proxy_when_host_rule_is_ignored() {
        let rules = ResolvedRules {
            host: Some("127.0.0.1:3000".to_string()),
            proxy: Some("127.0.0.1:9090".to_string()),
            ignored: crate::server::IgnoredFields {
                host: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(should_use_upstream_proxy(&rules));
    }

    #[test]
    fn test_build_upstream_proxy_auth_value() {
        let url = build_proxy_rule_url("user:pass@127.0.0.1:8080").unwrap();
        assert_eq!(
            build_upstream_proxy_auth_value(&url).as_deref(),
            Some("Basic dXNlcjpwYXNz")
        );
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

    #[test]
    fn test_headers_to_pairs_merges_cookie_entries() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(hyper::header::HOST, "example.com".parse().unwrap());
        headers.append(hyper::header::COOKIE, "session=abc123".parse().unwrap());
        headers.append(hyper::header::COOKIE, "user=test".parse().unwrap());
        headers.append(hyper::header::COOKIE, "lang=en".parse().unwrap());
        headers.append(hyper::header::ACCEPT, "*/*".parse().unwrap());

        let pairs = headers_to_pairs(&headers);

        let cookie_entries: Vec<_> = pairs.iter().filter(|(k, _)| k == "cookie").collect();
        assert_eq!(cookie_entries.len(), 1);
        assert_eq!(cookie_entries[0].1, "session=abc123; user=test; lang=en");

        assert!(pairs.iter().any(|(k, v)| k == "host" && v == "example.com"));
        assert!(pairs.iter().any(|(k, v)| k == "accept" && v == "*/*"));
    }

    #[test]
    fn test_headers_to_pairs_single_cookie_unchanged() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::COOKIE,
            "session=abc123; user=test".parse().unwrap(),
        );

        let pairs = headers_to_pairs(&headers);

        let cookie_entries: Vec<_> = pairs.iter().filter(|(k, _)| k == "cookie").collect();
        assert_eq!(cookie_entries.len(), 1);
        assert_eq!(cookie_entries[0].1, "session=abc123; user=test");
    }

    #[test]
    fn test_headers_to_pairs_no_cookie() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(hyper::header::HOST, "example.com".parse().unwrap());
        headers.append(hyper::header::ACCEPT, "text/html".parse().unwrap());

        let pairs = headers_to_pairs(&headers);

        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|(k, _)| k != "cookie"));
    }

    #[test]
    fn test_headers_pairs_equal_ignore_order_same_content_different_order() {
        let a = vec![
            ("content-length".to_string(), "100".to_string()),
            ("accept".to_string(), "*/*".to_string()),
            ("host".to_string(), "example.com".to_string()),
        ];
        let b = vec![
            ("accept".to_string(), "*/*".to_string()),
            ("host".to_string(), "example.com".to_string()),
            ("content-length".to_string(), "100".to_string()),
        ];
        assert!(headers_pairs_equal_ignore_order(&a, &b));
    }

    #[test]
    fn test_headers_pairs_equal_ignore_order_identical() {
        let a = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        assert!(headers_pairs_equal_ignore_order(&a, &a));
    }

    #[test]
    fn test_headers_pairs_equal_ignore_order_different_values() {
        let a = vec![
            ("host".to_string(), "foo.com".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        let b = vec![
            ("host".to_string(), "bar.com".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        assert!(!headers_pairs_equal_ignore_order(&a, &b));
    }

    #[test]
    fn test_headers_pairs_equal_ignore_order_different_lengths() {
        let a = vec![("host".to_string(), "example.com".to_string())];
        let b = vec![
            ("host".to_string(), "example.com".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        assert!(!headers_pairs_equal_ignore_order(&a, &b));
    }

    #[test]
    fn test_headers_pairs_equal_ignore_order_empty() {
        let a: Vec<(String, String)> = vec![];
        let b: Vec<(String, String)> = vec![];
        assert!(headers_pairs_equal_ignore_order(&a, &b));
    }

    #[test]
    fn test_headers_pairs_equal_ignore_order_different_keys() {
        let a = vec![
            ("x-custom".to_string(), "value".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        let b = vec![
            ("x-other".to_string(), "value".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        assert!(!headers_pairs_equal_ignore_order(&a, &b));
    }

    #[test]
    fn test_should_route_websocket_to_local_admin_for_loopback_admin_path() {
        assert!(should_route_websocket_to_local_admin(
            "localhost",
            8811,
            "/_bifrost/api/push",
            8811
        ));
        assert!(should_route_websocket_to_local_admin(
            "127.0.0.1",
            8811,
            "/_bifrost/api/push",
            8811
        ));
    }

    #[test]
    fn test_should_not_route_websocket_to_local_admin_for_non_admin_path_or_port() {
        assert!(!should_route_websocket_to_local_admin(
            "localhost",
            8811,
            "/socket.io",
            8811
        ));
        assert!(!should_route_websocket_to_local_admin(
            "localhost",
            8812,
            "/_bifrost/api/push",
            8811
        ));
        assert!(!should_route_websocket_to_local_admin(
            "example.com",
            8811,
            "/_bifrost/api/push",
            8811
        ));
    }

    #[test]
    fn test_rewrite_local_admin_websocket_request_rewrites_virtual_host_path() {
        let req = Request::builder()
            .uri("/api/push?need_overview=true")
            .body(())
            .unwrap();

        let req = rewrite_local_admin_websocket_request(req, ADMIN_VIRTUAL_HOST);
        assert_eq!(
            req.uri(),
            &"/_bifrost/api/push?need_overview=true"
                .parse::<Uri>()
                .unwrap()
        );
    }
}
