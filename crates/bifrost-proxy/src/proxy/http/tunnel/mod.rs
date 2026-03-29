use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crate::ensure_crypto_provider;
#[cfg(feature = "http3")]
use crate::http3::Http3Client;
use crate::protocol::{ProtocolDetector, TransportProtocol};
use bifrost_admin::{AdminState, ConnectionInfo, RequestTiming, TrafficRecord, TrafficType};
use bifrost_core::{BifrostError, Protocol, Result};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as AutoServerBuilder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
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

pub use self::bidirectional::{
    tunnel_bidirectional, tunnel_bidirectional_with_cancel, TunnelStats,
};
pub use self::cert::SingleCertResolver;
use self::host_rule::parse_host_rule;
use self::io::{BufferedIo, CombinedAsyncRw};

use super::handler::{
    build_connection_error_response, build_overridden_error_response, needs_body_processing,
    needs_request_body_processing, needs_response_override, parse_and_record_sse_events,
    ConnectionErrorInfo,
};
use super::ws_handshake::{
    header_values, negotiate_extensions, negotiate_protocol, read_http1_response_with_leftover,
};
use crate::dns::DnsResolver;
use crate::server::{
    empty_body, full_body, with_trailers, BoxBody, ProxyConfig, ResolvedRules, RulesResolver,
    TlsConfig, TlsInterceptConfig,
};
use crate::transform::apply_res_rules;
use crate::transform::decompress::get_content_encoding;
use crate::transform::{apply_body_rules, Phase};
use crate::utils::bounded::{read_body_bounded, BoundedBody};
use crate::utils::http_size::{
    calculate_request_size, calculate_response_headers_size, calculate_response_size,
};
use crate::utils::logging::{format_rules_summary, RequestContext};
use crate::utils::process_info::spawn_async_process_resolver;
use crate::utils::tee::{
    create_metrics_body, create_request_tee_body, create_sse_tee_body, create_tee_body_with_store,
    store_request_body, BodyCaptureHandle,
};
use crate::utils::throttle::wrap_throttled_body;

fn maybe_backfill_tunnel_client_process(
    state: &Arc<AdminState>,
    req_id: &str,
    client_app: Option<&str>,
    client_pid: Option<u32>,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
) {
    if client_app.is_some() && client_pid.is_some() {
        debug!(
            req_id,
            client_app = ?client_app,
            client_pid = ?client_pid,
            "Skipping tunnel client process backfill because client metadata is already present"
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

    info!(
        req_id,
        peer_addr = %peer_addr,
        local_addr = %local_addr,
        existing_client_app = ?client_app,
        existing_client_pid = ?client_pid,
        "Scheduling tunnel client process backfill"
    );

    let state = Arc::clone(state);
    spawn_async_process_resolver(
        peer_addr,
        local_addr,
        req_id.to_string(),
        move |id, process| {
            info!(
                req_id = %id,
                client_app = %process.name,
                client_pid = process.pid,
                client_path = ?process.path,
                "Applying tunnel client process backfill to traffic record"
            );
            state.update_client_process(&id, process.name.clone(), process.pid, process.path);
            state
                .connection_registry
                .update_client_app(&id, process.name.clone());
        },
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
    !resolved_rules.res_headers.is_empty()
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
        || resolved_rules.html_append.is_some()
        || resolved_rules.html_prepend.is_some()
        || resolved_rules.html_body.is_some()
        || resolved_rules.js_append.is_some()
        || resolved_rules.js_prepend.is_some()
        || resolved_rules.js_body.is_some()
        || resolved_rules.css_append.is_some()
        || resolved_rules.css_prepend.is_some()
        || resolved_rules.css_body.is_some()
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

fn build_upstream_pool_partition(
    original_host: &str,
    target_host: &str,
    target_port: u16,
    use_http: bool,
    rules: &ResolvedRules,
) -> String {
    format!(
        "orig={original_host}|target={}://{}:{}|host={:?}|proxy={:?}|proto={:?}|ignored_host={}",
        if use_http { "http" } else { "https" },
        target_host,
        target_port,
        rules.host,
        rules.proxy,
        rules.host_protocol,
        rules.ignored.host
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

    let url = format!("https://{}:{}", host, port);
    let tunnel_url = format!("tunnel://{}:{}", host, port);
    let mut resolved_rules = rules.resolve(&url, "CONNECT");
    let tunnel_rules = rules.resolve(&tunnel_url, "CONNECT");
    if tunnel_rules.host.is_some()
        || tunnel_rules.tls_options.is_some()
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
        warn!(
            req_id = ctx.id_str(),
            host,
            port,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "CONNECT app-based TLS decision fell back because client process is unknown"
        );
    }

    let mut intercept = should_intercept_tls_for_client(
        &host,
        ctx.client_app.as_deref(),
        is_local_client,
        tls_intercept_config,
        &tls_config,
        &resolved_rules,
    );

    if !intercept
        && tls_config.ca_cert.is_some()
        && !matches!(resolved_rules.tls_intercept, Some(false))
        && (requires_tls_interception_for_rules(&resolved_rules)
            || rules.has_response_rules_for_host(&host))
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
            ctx,
            admin_state,
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

    let has_rules = resolved_rules.host.is_some() || !resolved_rules.rules.is_empty();
    if verbose_logging && has_rules {
        info!(
            "[{}] CONNECT tunnel rules matched: {}",
            ctx.id_str(),
            format_rules_summary(&resolved_rules)
        );
    }

    let (target_host, target_port) = if let Some(ref host_rule) = resolved_rules.host {
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

    let connect_host = if !resolved_rules.dns_servers.is_empty() {
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

    let target_stream = TcpStream::connect(format!("{}:{}", connect_host, target_port))
        .await
        .map_err(|e| {
            BifrostError::Network(format!(
                "Failed to connect to {}:{}: {}",
                connect_host, target_port, e
            ))
        })?;

    if let Err(e) = target_stream.set_nodelay(true) {
        warn!(
            "[{}] Failed to set TCP_NODELAY on tunnel connection: {}",
            ctx.id_str(),
            e
        );
    }

    if verbose_logging {
        info!(
            "[{}] CONNECT tunnel established to {}:{}",
            ctx.id_str(),
            target_host,
            target_port
        );
    }

    let req_id = ctx.id_str().to_string();
    let verbose = verbose_logging;
    let client_ip = ctx.client_ip.clone();
    let client_app = ctx.client_app.clone();
    let client_pid = ctx.client_pid;
    let client_path = ctx.client_path.clone();

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
        record.client_ip = client_ip.clone();
        record.client_app = client_app.clone();
        record.client_pid = client_pid;
        record.client_path = client_path.clone();
        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
        state.record_traffic(record);
        maybe_backfill_tunnel_client_process(
            state,
            &req_id,
            client_app.as_deref(),
            client_pid,
            peer_addr,
            local_addr,
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
    ctx: &RequestContext,
    admin_state: Option<Arc<AdminState>>,
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
            &req_id,
            admin_state.clone(),
            cancel_rx,
            client_ip,
            client_app,
            client_pid,
            client_path,
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
#[allow(dead_code)]
async fn tls_intercept_tunnel(
    upgraded: Upgraded,
    server_config: Arc<ServerConfig>,
    original_host: &str,
    original_port: u16,
    rules: Arc<dyn RulesResolver>,
    verbose_logging: bool,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    unsafe_ssl: bool,
    req_id: &str,
    admin_state: Option<Arc<AdminState>>,
    client_ip: String,
    client_app: Option<String>,
    client_pid: Option<u32>,
    client_path: Option<String>,
) -> Result<()> {
    let acceptor = TlsAcceptor::from(server_config);
    let client_tls = acceptor
        .accept(TokioIo::new(upgraded))
        .await
        .map_err(|e| BifrostError::Tls(format!("TLS accept failed: {e}")))?;

    if verbose_logging {
        debug!("[{}] TLS handshake with client completed", req_id);
    }

    let original_host_for_requests = original_host.to_string();
    let original_port_for_requests = original_port;
    let admin_state_clone = admin_state.clone();
    let rules_clone = rules.clone();
    let verbose = verbose_logging;
    let client_ip_clone = client_ip.clone();
    let client_app_clone = client_app.clone();
    let client_path_clone = client_path.clone();

    let service = service_fn(move |req: Request<Incoming>| {
        let original_host = original_host_for_requests.clone();
        let original_port = original_port_for_requests;
        let req_id = crate::utils::logging::generate_request_id();
        let admin_state = admin_state_clone.clone();
        let rules = rules_clone.clone();
        let client_ip = client_ip_clone.clone();
        let client_app = client_app_clone.clone();
        let client_pid = client_pid;
        let client_path = client_path_clone.clone();
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
            )
            .await
        }
    });

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

    if let Err(e) = conn.await {
        if verbose_logging {
            debug!("[{}] HTTP connection ended: {}", req_id, e);
        }
    }

    Ok(())
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
    req_id: &str,
    admin_state: Option<Arc<AdminState>>,
    cancel_rx: oneshot::Receiver<()>,
    client_ip: String,
    client_app: Option<String>,
    client_pid: Option<u32>,
    client_path: Option<String>,
) -> Result<bool> {
    let acceptor = TlsAcceptor::from(server_config);
    let mut client_tls = acceptor
        .accept(TokioIo::new(upgraded))
        .await
        .map_err(|e| BifrostError::Tls(format!("TLS accept failed: {e}")))?;
    let client_alpn = client_tls.get_ref().1.alpn_protocol().map(|p| p.to_vec());
    let should_sniff_payload = !is_standard_tls_intercept_port(original_port);

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

    if !is_http_alpn(client_alpn.as_deref())
        || (should_sniff_payload && !looks_like_http_payload(&initial_payload))
    {
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

async fn tunnel_intercepted_non_http_tls_with_cancel(
    client_tls: tokio_rustls::server::TlsStream<TokioIo<Upgraded>>,
    initial_payload: BytesMut,
    ctx: RawTlsTunnelContext,
) -> Result<bool> {
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

    let target_stream = TcpStream::connect(format!("{}:{}", original_host, original_port))
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

#[allow(clippy::too_many_arguments)]
async fn handle_intercepted_request_with_protocol(
    req: Request<Incoming>,
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
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
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
        )
        .await;
    }

    let start_time = Instant::now();
    let method = req.method().clone();
    let method_str = method.to_string();
    let uri = req.uri().clone();
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let query_string = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();

    let original_uri = format!("https://{}{}", original_host, path);

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

    let incoming_cookies: std::collections::HashMap<String, String> = req
        .headers()
        .get(hyper::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .filter_map(|part| {
                    let mut iter = part.trim().splitn(2, '=');
                    match (iter.next(), iter.next()) {
                        (Some(k), Some(v)) => Some((k.to_string(), v.to_string())),
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

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

    let has_rules = !resolved_rules.rules.is_empty()
        || resolved_rules.host.is_some()
        || !resolved_rules.req_headers.is_empty()
        || !resolved_rules.res_headers.is_empty();

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
            let use_http_override = match resolved_rules.host_protocol {
                Some(Protocol::Http) | Some(Protocol::Ws) => true,
                Some(Protocol::Host) | Some(Protocol::XHost) => p != 443 && p != 8443,
                _ => false,
            };
            let target_path = parsed_path.unwrap_or_else(|| path.to_string());
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
                path.to_string(),
            )
        };

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

    debug!("[{}] Intercepted: {} {}", req_id, method_str, target_uri);

    if let Some(ref redirect_url) = resolved_rules.redirect {
        let status = resolved_rules.redirect_status.unwrap_or(302);
        if verbose_logging {
            info!(
                "[{}] [REDIRECT] {} -> {} ({})",
                req_id, original_uri, redirect_url, status
            );
        }
        return Ok(build_redirect_response(status, redirect_url));
    }

    if let Some(ref mock_file) = resolved_rules.mock_file {
        if verbose_logging {
            info!("[{}] [MOCK_FILE] Serving file: {}", req_id, mock_file);
        }
        let status_code = resolved_rules.status_code.unwrap_or(200);
        return Ok(serve_mock_file(mock_file, status_code, None).await);
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
        return Ok(serve_mock_file(mock_template, status_code, Some(&template_vars)).await);
    }

    if let Some(ref mock_rawfile) = resolved_rules.mock_rawfile {
        if verbose_logging {
            info!(
                "[{}] [MOCK_RAWFILE] Serving raw file: {}",
                req_id, mock_rawfile
            );
        }
        let status_code = resolved_rules.status_code.unwrap_or(200);
        return Ok(serve_mock_file(mock_rawfile, status_code, None).await);
    }

    let (parts, body) = req.into_parts();

    let actual_method = if let Some(ref method_override) = resolved_rules.method {
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

    let original_req_headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let req_headers = original_req_headers.clone();

    let req_content_encoding = get_content_encoding(&req_headers);

    let req_content_length = parts
        .headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());
    let has_transfer_encoding = parts.headers.contains_key(hyper::header::TRANSFER_ENCODING);

    let needs_req_processing = needs_request_body_processing(&resolved_rules);
    let has_req_body_override = resolved_rules.req_body.is_some();
    let needs_req_body_read = needs_req_processing && !has_req_body_override;

    let mut streaming_body: Option<BoxBody> = None;
    let mut req_body_capture: Option<BodyCaptureHandle> = None;
    let body_bytes = if needs_req_body_read {
        if let Some(len) = req_content_length {
            if len > max_body_buffer_size {
                warn!(
                    "[{}] [REQ_BODY] body too large ({} bytes > {} limit), skipping body rules and streaming forward",
                    req_id,
                    len,
                    max_body_buffer_size
                );
                if admin_state.is_some() {
                    let (tee_body, capture) =
                        create_request_tee_body(body, admin_state.clone(), req_id.to_string());
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
                        if admin_state.is_some() {
                            let (tee_body, capture) = create_request_tee_body(
                                replay_body,
                                admin_state.clone(),
                                req_id.to_string(),
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
                    if admin_state.is_some() {
                        let (tee_body, capture) = create_request_tee_body(
                            replay_body,
                            admin_state.clone(),
                            req_id.to_string(),
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
            let (tee_body, capture) =
                create_request_tee_body(body, admin_state.clone(), req_id.to_string());
            streaming_body = Some(tee_body);
            req_body_capture = Some(capture);
        } else {
            streaming_body = Some(body.boxed());
        }
        Vec::new()
    };
    let request_body_size = if !body_bytes.is_empty() {
        body_bytes.len()
    } else {
        req_content_length.unwrap_or(0)
    };

    let upstream_uri: hyper::Uri = match target_uri.parse() {
        Ok(u) => u,
        Err(e) => {
            error!("[{}] Failed to parse upstream URI: {}", req_id, e);
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"Bad Gateway".to_vec()))
                .unwrap());
        }
    };

    let mut new_req = Request::builder().method(actual_method).uri(&upstream_uri);

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
        if name == hyper::header::COOKIE && !resolved_rules.req_cookies.is_empty() {
            continue;
        }
        new_req = new_req.header(name, value);
    }
    let host_header_value = if actual_use_http {
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
    new_req = new_req.header(hyper::header::HOST, &host_header_value);
    if streaming_body.is_none() {
        new_req = new_req.header(hyper::header::CONTENT_LENGTH, body_bytes.len());
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

    for (name, value) in &resolved_rules.req_headers {
        if verbose_logging {
            info!("[{}] [REQ_HEADER] {} = {}", req_id, name, value);
        }
        new_req = new_req.header(name.as_str(), value.as_str());
    }

    if !resolved_rules.req_cookies.is_empty() {
        let existing_cookies: Vec<(String, String)> = parts
            .headers
            .get(hyper::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                s.split(';')
                    .filter_map(|part| {
                        let mut iter = part.trim().splitn(2, '=');
                        match (iter.next(), iter.next()) {
                            (Some(k), Some(v)) => Some((k.to_string(), v.to_string())),
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut cookie_map: std::collections::HashMap<String, String> =
            existing_cookies.into_iter().collect();

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
    }

    let request_body_is_streaming = streaming_body.is_some();
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
    sanitize_upstream_headers(outgoing_req.headers_mut());
    outgoing_req.headers_mut().remove(hyper::header::HOST);

    let final_req_headers: Vec<(String, String)> = outgoing_req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

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

    let build_conn_error_record_and_response =
        |error_type: &'static str, error_msg: String, tls_ms: Option<u64>| {
            let error_info = ConnectionErrorInfo {
                error_type,
                error_message: error_msg.clone(),
                host: actual_target_host.clone(),
                request_url: original_uri.clone(),
            };
            let total_ms = start_time.elapsed().as_millis() as u64;
            if let Some(ref state) = admin_state {
                let mut record = TrafficRecord::new(
                    req_id.to_string(),
                    method_str.clone(),
                    original_uri.clone(),
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
                record.host = original_host.to_string();
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
                record.original_request_headers = Some(original_req_headers.clone());
                record.has_rule_hit = has_rules;
                record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                record.error_message = Some(error_msg);
                record.request_body_ref = if let Some(ref capture) = req_body_capture {
                    capture.take()
                } else {
                    store_request_body(
                        &admin_state,
                        req_id,
                        &body_bytes,
                        req_content_encoding.as_deref(),
                    )
                };
                state.record_traffic(record);
            }
            if needs_response_override(&resolved_rules) {
                if verbose_logging {
                    info!(
                        "[{}] [CONN_ERROR] {}, applying response override rules",
                        req_id, error_type
                    );
                }
                build_overridden_error_response(&resolved_rules, 502, &error_info)
            } else {
                build_connection_error_response(502, &error_info)
            }
        };
    let (mut upstream_parts, upstream_body) = outgoing_req.into_parts();
    upstream_parts.uri = upstream_uri.clone();
    upstream_parts.headers.remove(hyper::header::HOST);
    sanitize_upstream_headers(&mut upstream_parts.headers);

    #[cfg(feature = "http3")]
    let req_headers_for_h3: Vec<(String, String)> = upstream_parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    #[cfg(feature = "http3")]
    let h3_dns_resolver = DnsResolver::new(verbose_logging);

    #[cfg(feature = "http3")]
    let should_try_http3_upstream = !actual_use_http
        && resolved_rules.upstream_http3
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
                    unsafe_ssl,
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
        };
        let wait_ms = send_start.elapsed().as_millis() as u64;
        let (parts, body) = response.into_parts();
        (parts, Some(body), None, wait_ms, None)
    };

    let (mut res_parts, res_body, tls_ms, wait_ms, h3_buffered_body) = upstream_result;

    let target_status = resolved_rules.replace_status.or(resolved_rules.status_code);
    if let Some(status_code) = target_status {
        if let Ok(status) = hyper::StatusCode::from_u16(status_code) {
            if verbose_logging {
                info!(
                    "[{}] [RES_STATUS] {} -> {}",
                    req_id,
                    res_parts.status.as_u16(),
                    status_code
                );
            }
            res_parts.status = status;
        }
    }

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
        .with_client_process(client_app.clone(), client_pid, client_path.clone());
    apply_res_rules(&mut res_parts, &resolved_rules, verbose_logging, &ctx);

    let res_headers: Vec<(String, String)> = res_parts
        .headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let needs_processing = needs_body_processing(&resolved_rules);
    let has_res_body_override = resolved_rules.res_body.is_some();
    let needs_res_body_read = needs_processing && !has_res_body_override;

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
    let binary_traffic_performance_mode = admin_state
        .as_ref()
        .map(|state| state.get_binary_traffic_performance_mode())
        .unwrap_or(false);
    let skip_binary_recording =
        should_use_binary_performance_mode(&res_parts, binary_traffic_performance_mode)
            && !is_websocket
            && !is_sse;

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
            req_id,
            size_display,
            res_body_limit
        );
    }

    if skip_body_processing {
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
                record.response_headers = Some(original_res_headers.clone());
                if res_headers != original_res_headers {
                    record.actual_response_headers = Some(res_headers.clone());
                }
                record.original_request_headers = Some(original_req_headers.clone());
                if actual_target_host != original_host || actual_target_port != original_port {
                    let actual_scheme = if actual_use_http { "http" } else { "https" };
                    let actual_url = if (actual_use_http && actual_target_port == 80)
                        || (!actual_use_http && actual_target_port == 443)
                    {
                        format!("{}://{}{}", actual_scheme, actual_target_host, path)
                    } else {
                        format!(
                            "{}://{}:{}{}",
                            actual_scheme, actual_target_host, actual_target_port, path
                        )
                    };
                    record.actual_url = Some(actual_url);
                    record.actual_host = Some(actual_target_host.clone());
                }
                record.request_content_type = final_req_headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone());
                record.client_ip = client_ip.clone();
                record.client_app = client_app.clone();
                record.client_pid = client_pid;
                record.client_path = client_path.clone();

                if is_websocket {
                    record.protocol = "wss".to_string();
                }

                if is_sse {
                    record.set_sse();
                    state.sse_hub.register(&record_id);
                }

                record.has_rule_hit = has_rules;
                record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);

                record.request_body_ref = if let Some(ref capture) = req_body_capture {
                    capture.take()
                } else {
                    store_request_body(
                        &admin_state,
                        &record_id,
                        &body_bytes,
                        req_content_encoding.as_deref(),
                    )
                };

                if is_sse {
                    if let Some(ref body_store) = state.body_store {
                        match body_store.read().start_stream(&record_id, "sse_raw") {
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

        if is_sse {
            let res_body = res_body_incoming.take().unwrap();
            let tee_body = create_sse_tee_body(
                res_body,
                admin_state.clone(),
                record_id,
                Some(traffic_type),
                sse_stream_writer,
                max_body_buffer_size,
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
        record.response_size = calculate_response_size(
            res_parts.status.as_u16(),
            &res_headers,
            original_res_body_len,
        );
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
        record.response_headers = Some(original_res_headers.clone());
        if res_headers != original_res_headers {
            record.actual_response_headers = Some(res_headers.clone());
        }
        record.original_request_headers = Some(original_req_headers.clone());
        if actual_target_host != original_host || actual_target_port != original_port {
            let actual_scheme = if actual_use_http { "http" } else { "https" };
            let actual_url = if (actual_use_http && actual_target_port == 80)
                || (!actual_use_http && actual_target_port == 443)
            {
                format!("{}://{}{}", actual_scheme, actual_target_host, path)
            } else {
                format!(
                    "{}://{}:{}{}",
                    actual_scheme, actual_target_host, actual_target_port, path
                )
            };
            record.actual_url = Some(actual_url);
            record.actual_host = Some(actual_target_host.clone());
        }
        record.request_content_type = final_req_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone());
        record.client_ip = client_ip.clone();
        record.client_app = client_app.clone();
        record.client_pid = client_pid;
        record.client_path = client_path.clone();

        if is_websocket {
            record.protocol = "wss".to_string();
        }

        if is_sse {
            record.set_sse();
            state.sse_hub.register(req_id);
        }

        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);

        record.request_body_ref = if let Some(ref capture) = req_body_capture {
            capture.take()
        } else {
            store_request_body(
                &admin_state,
                req_id,
                &body_bytes,
                req_content_encoding.as_deref(),
            )
        };

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
            record.response_body_ref = store.store(req_id, "res", decompressed_res_body.as_ref());
        }

        state.record_traffic(record);

        if is_sse {
            let event_count = parse_and_record_sse_events(&res_body_bytes);
            let response_size = res_body_bytes.len();
            state.update_traffic_by_id(req_id, move |record| {
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
            });
            state.sse_hub.unregister(req_id);
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

    let res_content_type = res_parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let final_body = apply_body_rules(
        Bytes::from(res_body_bytes.clone()),
        &resolved_rules,
        Phase::Response,
        res_content_type,
        verbose_logging,
        &ctx,
    );

    if original_res_body_len != final_body.len() {
        res_parts.headers.remove(hyper::header::CONTENT_LENGTH);
        res_parts.headers.insert(
            hyper::header::CONTENT_LENGTH,
            hyper::header::HeaderValue::from_str(&final_body.len().to_string()).unwrap(),
        );
        if verbose_logging {
            info!(
                "[{}] Updated Content-Length: {} -> {}",
                req_id,
                original_res_body_len,
                final_body.len()
            );
        }
    }

    if let Some(ref state) = admin_state {
        if let Some(ref body_store) = state.body_store {
            let max_decompress_output_bytes = if let Some(cm) = state.config_manager.as_ref() {
                cm.config().await.sandbox.limits.max_decompress_output_bytes
            } else {
                10 * 1024 * 1024
            };

            let store = body_store.read();
            let decompressed_res = crate::transform::decompress_body_with_limit(
                &final_body,
                res_content_encoding.as_deref(),
                max_decompress_output_bytes,
            );
            if let Some(body_ref) = store.store(req_id, "res", decompressed_res.as_ref()) {
                state.update_traffic_by_id(req_id, move |record| {
                    record.response_body_ref = Some(body_ref.clone());
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
    req: Request<Incoming>,
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
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    use tokio_rustls::rustls::pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    let start_time = Instant::now();
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

    let incoming_cookies: std::collections::HashMap<String, String> = req
        .headers()
        .get(hyper::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .filter_map(|part| {
                    let mut iter = part.trim().splitn(2, '=');
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

    let has_rules = !resolved_rules.rules.is_empty()
        || resolved_rules.host.is_some()
        || !resolved_rules.req_headers.is_empty()
        || !resolved_rules.res_headers.is_empty();

    let (target_host, target_port, use_http) = if resolved_rules.ignored.host {
        debug!(
            "[{}] [WS] Passthrough rule applied: WebSocket will be forwarded to original target {}:{}",
            req_id, original_host, original_port
        );
        (original_host.to_string(), original_port, false)
    } else if let Some(ref host_rule) = resolved_rules.host {
        let host_rule_clean = host_rule.trim_end_matches('/');
        let parts: Vec<&str> = host_rule_clean.split(':').collect();
        let h = parts[0].to_string();
        let p = if parts.len() > 1 {
            parts[1].parse().unwrap_or(original_port)
        } else {
            match resolved_rules.host_protocol {
                Some(Protocol::Http) | Some(Protocol::Ws) => 80,
                Some(Protocol::Https) | Some(Protocol::Wss) => 443,
                _ => original_port,
            }
        };
        let use_http_flag = match resolved_rules.host_protocol {
            Some(Protocol::Http) | Some(Protocol::Ws) => true,
            Some(Protocol::Host) | Some(Protocol::XHost) => p != 443 && p != 8443,
            _ => false,
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
        (h, p, use_http_flag)
    } else {
        (original_host.to_string(), original_port, false)
    };

    let connect_start = Instant::now();
    let target_stream = match TcpStream::connect(format!("{}:{}", target_host, target_port)).await {
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
        let handshake = build_websocket_handshake_request(&req, &target_host, target_port);
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

        if upstream_resp.status_code != 101 {
            error!(
                "[{}] WebSocket handshake failed: {} {}",
                req_id, upstream_resp.status_code, upstream_resp.status_text
            );
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"WebSocket handshake failed".to_vec()))
                .unwrap());
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
        let handshake = build_websocket_handshake_request(&req, &target_host, target_port);
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

        if upstream_resp.status_code != 101 {
            error!(
                "[{}] WebSocket handshake failed: {} {}",
                req_id, upstream_resp.status_code, upstream_resp.status_text
            );
            return Ok(Response::builder()
                .status(502)
                .body(full_body(b"WebSocket handshake failed".to_vec()))
                .unwrap());
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

    let req_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    if let Some(ref state) = admin_state {
        state
            .metrics_collector
            .increment_requests_by_type(TrafficType::Wss);

        let ws_url = format!("wss://{}{}", original_host, path);
        let mut record = TrafficRecord::new(req_id.to_string(), "GET".to_string(), ws_url);
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
        record.client_ip = client_ip.clone();
        record.client_app = client_app.clone();
        record.client_pid = client_pid;
        record.client_path = client_path.clone();
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
        .with_client_process(client_app.clone(), client_pid, client_path.clone());

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

    for (name, value) in upstream_headers {
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

fn build_websocket_handshake_request(
    req: &Request<Incoming>,
    target_host: &str,
    target_port: u16,
) -> String {
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

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
            || n.eq_ignore_ascii_case("origin")
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
        tls_intercept_config,
        tls_config,
        resolved_rules,
    )
}

pub fn should_intercept_tls_for_client(
    host: &str,
    client_app: Option<&str>,
    is_local_client: bool,
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

    if is_local_client
        && requires_client_app_for_tls_decision(tls_intercept_config)
        && !matches!(client_app, Some(app) if !app.is_empty())
    {
        if is_domain_included(host, &tls_intercept_config.intercept_include) {
            return true;
        }

        if is_domain_excluded(host, &tls_intercept_config.intercept_exclude) {
            return false;
        }

        return false;
    }

    if is_local_client {
        if is_app_included(client_app, &tls_intercept_config.app_intercept_include) {
            return true;
        }

        if is_app_excluded(client_app, &tls_intercept_config.app_intercept_exclude) {
            return false;
        }
    }

    if is_domain_included(host, &tls_intercept_config.intercept_include) {
        return true;
    }

    if is_domain_excluded(host, &tls_intercept_config.intercept_exclude) {
        return false;
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

async fn serve_mock_file(
    file_path: &str,
    status_code: u16,
    template_vars: Option<&TemplateVars>,
) -> Response<BoxBody> {
    match tokio::fs::read_to_string(file_path).await {
        Ok(content) => {
            let body = if let Some(vars) = template_vars {
                process_template(&content, vars)
            } else {
                content
            };

            let content_type = if file_path.ends_with(".json") || body.trim_start().starts_with('{')
            {
                "application/json"
            } else if file_path.ends_with(".html") {
                "text/html; charset=utf-8"
            } else if file_path.ends_with(".xml") {
                "application/xml"
            } else {
                "text/plain; charset=utf-8"
            };

            Response::builder()
                .status(status_code)
                .header(hyper::header::CONTENT_TYPE, content_type)
                .body(full_body(body.into_bytes()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_admin::{FrameDirection, TrafficDbStore};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    fn test_should_intercept_include_has_highest_priority() {
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
            result,
            "Include list should have higher priority than exclude list"
        );
        println!("✓ Include > Exclude priority: intercept={}", result);
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
            "nextoncall-bd.byteintl.net",
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
            "nextoncall-bd.byteintl.net",
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
            "nextoncall-bd.byteintl.net",
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
    fn test_should_intercept_app_include_has_higher_priority_than_app_exclude() {
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
            result,
            "App include should have higher priority than app exclude"
        );
    }

    #[test]
    fn test_should_intercept_app_has_higher_priority_than_domain() {
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
            !result,
            "App exclude should have higher priority than domain include"
        );
    }
}
