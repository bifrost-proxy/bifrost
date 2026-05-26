use std::collections::HashMap;
use std::error::Error as StdError;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use crate::dns::DnsResolver;
use crate::ensure_crypto_provider;
use crate::server::BoxBody;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};
use hyper::Request;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::Error as ClientError;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use pin_project_lite::pin_project;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tower::Service;

pub(super) type PooledHttpsConnector =
    hyper_rustls::HttpsConnector<HttpConnector<ProxyDnsResolver>>;
type HttpsPooledClient = Client<PooledHttpsConnector, BoxBody>;

#[derive(Debug, Clone)]
pub(in crate::proxy::http) struct UpstreamRequestErrorInfo {
    pub error_type: &'static str,
    pub error_message: String,
    pub source_chain: Vec<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ClientCacheKey {
    unsafe_ssl: bool,
    dns_servers: Vec<String>,
    pool_partition: String,
    protocol: ClientProtocolPreference,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum ClientProtocolPreference {
    Auto,
    Http1Only,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct Http1FallbackKey {
    unsafe_ssl: bool,
    dns_servers: Vec<String>,
    pool_partition: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UpstreamConcurrencyKey {
    unsafe_ssl: bool,
    dns_servers: Vec<String>,
    pool_partition: String,
}

#[derive(Clone)]
pub(super) struct ProxyDnsResolver {
    dns_servers: Arc<Vec<String>>,
    resolver: Arc<DnsResolver>,
}

type ResolveAddrs = std::vec::IntoIter<SocketAddr>;
type ResolveFuture = Pin<Box<dyn Future<Output = io::Result<ResolveAddrs>> + Send>>;

static HTTPS_CLIENTS: LazyLock<RwLock<HashMap<ClientCacheKey, Arc<HttpsPooledClient>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static HTTP1_FALLBACKS: LazyLock<RwLock<HashMap<Http1FallbackKey, std::time::Instant>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static HTTP1_FALLBACK_LAST_CLEANUP: LazyLock<RwLock<Option<std::time::Instant>>> =
    LazyLock::new(|| RwLock::new(None));
static UPSTREAM_LIMITERS: LazyLock<RwLock<HashMap<UpstreamConcurrencyKey, Arc<Semaphore>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
const HTTP1_FALLBACK_TTL: Duration = Duration::from_secs(120);
const HTTP1_FALLBACK_CLEANUP_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum number of cached HTTPS client pools before eviction is triggered.
const MAX_CACHED_HTTPS_CLIENTS: usize = 256;
/// Maximum number of upstream limiter entries before eviction is triggered.
const MAX_CACHED_UPSTREAM_LIMITERS: usize = 512;
static MAX_UPSTREAM_INFLIGHT_PER_PARTITION: LazyLock<usize> = LazyLock::new(|| {
    std::env::var("BIFROST_UPSTREAM_MAX_INFLIGHT_PER_PARTITION")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(64)
});

pin_project! {
    struct UpstreamPermitBody<B> {
        #[pin]
        inner: B,
        permit: Option<OwnedSemaphorePermit>,
    }
}

impl<B> UpstreamPermitBody<B> {
    fn new(inner: B, permit: OwnedSemaphorePermit) -> Self {
        Self {
            inner,
            permit: Some(permit),
        }
    }
}

impl<B> Body for UpstreamPermitBody<B>
where
    B: Body<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        match this.inner.as_mut().poll_frame(cx) {
            Poll::Ready(None) => {
                this.permit.take();
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

fn build_root_cert_store() -> RootCertStore {
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    root_store
}

impl ProxyDnsResolver {
    fn new(dns_servers: Vec<String>) -> Self {
        Self {
            dns_servers: Arc::new(dns_servers),
            resolver: Arc::new(DnsResolver::new(false)),
        }
    }
}

impl Service<Name> for ProxyDnsResolver {
    type Response = ResolveAddrs;
    type Error = io::Error;
    type Future = ResolveFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let dns_servers = Arc::clone(&self.dns_servers);
        let resolver = Arc::clone(&self.resolver);
        let host = name.as_str().to_string();

        Box::pin(async move {
            if dns_servers.is_empty() {
                let addrs: Vec<SocketAddr> =
                    tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
                return Ok(addrs.into_iter());
            }

            match resolver.resolve(&host, dns_servers.as_slice()).await {
                Ok(Some(ip)) => Ok(vec![SocketAddr::new(ip, 0)].into_iter()),
                Ok(None) => {
                    let addrs: Vec<SocketAddr> =
                        tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
                    Ok(addrs.into_iter())
                }
                Err(err) => Err(io::Error::other(err.to_string())),
            }
        })
    }
}

#[derive(Debug)]
struct NoVerifier;

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::ServerCertVerified,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        vec![
            tokio_rustls::rustls::SignatureScheme::RSA_PKCS1_SHA256,
            tokio_rustls::rustls::SignatureScheme::RSA_PKCS1_SHA384,
            tokio_rustls::rustls::SignatureScheme::RSA_PKCS1_SHA512,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            tokio_rustls::rustls::SignatureScheme::RSA_PSS_SHA256,
            tokio_rustls::rustls::SignatureScheme::RSA_PSS_SHA384,
            tokio_rustls::rustls::SignatureScheme::RSA_PSS_SHA512,
            tokio_rustls::rustls::SignatureScheme::ED25519,
        ]
    }
}

fn build_https_client(
    unsafe_ssl: bool,
    dns_servers: &[String],
    protocol: ClientProtocolPreference,
) -> HttpsPooledClient {
    ensure_crypto_provider();

    let config = if unsafe_ssl {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(build_root_cert_store())
            .with_no_client_auth()
    };

    let resolver = ProxyDnsResolver::new(dns_servers.to_vec());
    let mut http_connector = HttpConnector::new_with_resolver(resolver);
    http_connector.enforce_http(false);
    http_connector.set_nodelay(true);
    http_connector.set_keepalive(Some(Duration::from_secs(60)));
    http_connector.set_connect_timeout(Some(Duration::from_secs(10)));

    let https_connector = match protocol {
        ClientProtocolPreference::Auto => hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_all_versions()
            .wrap_connector(http_connector),
        ClientProtocolPreference::Http1Only => hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_http1()
            .wrap_connector(http_connector),
    };

    let mut builder = Client::builder(TokioExecutor::new());
    builder.timer(TokioTimer::new());
    builder.pool_timer(TokioTimer::new());
    builder.pool_idle_timeout(Duration::from_secs(90));
    builder.pool_max_idle_per_host(32);
    builder.http2_adaptive_window(true);
    builder.http2_keep_alive_interval(Some(Duration::from_secs(15)));
    builder.http2_keep_alive_timeout(Duration::from_secs(20));
    builder.http2_keep_alive_while_idle(true);
    builder.build(https_connector)
}

fn get_https_client(
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
    protocol: ClientProtocolPreference,
) -> Arc<HttpsPooledClient> {
    let key = ClientCacheKey {
        unsafe_ssl,
        dns_servers: dns_servers.to_vec(),
        pool_partition: pool_partition.to_string(),
        protocol,
    };

    if let Ok(clients) = HTTPS_CLIENTS.read() {
        if let Some(client) = clients.get(&key) {
            return Arc::clone(client);
        }
    }

    let client = Arc::new(build_https_client(
        unsafe_ssl,
        &key.dns_servers,
        key.protocol,
    ));
    if let Ok(mut clients) = HTTPS_CLIENTS.write() {
        // Evict idle pools when cache grows too large.
        // An entry with strong_count == 1 means only the cache holds it (no active requests).
        if clients.len() >= MAX_CACHED_HTTPS_CLIENTS {
            clients.retain(|_, v| Arc::strong_count(v) > 1);
        }
        let entry = clients.entry(key).or_insert_with(|| Arc::clone(&client));
        return Arc::clone(entry);
    }
    client
}

fn fallback_key(
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> Http1FallbackKey {
    Http1FallbackKey {
        unsafe_ssl,
        dns_servers: dns_servers.to_vec(),
        pool_partition: pool_partition.to_string(),
    }
}

fn concurrency_key(
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> UpstreamConcurrencyKey {
    UpstreamConcurrencyKey {
        unsafe_ssl,
        dns_servers: dns_servers.to_vec(),
        pool_partition: pool_partition.to_string(),
    }
}

fn get_upstream_limiter(
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> Arc<Semaphore> {
    let key = concurrency_key(unsafe_ssl, dns_servers, pool_partition);

    if let Ok(limiters) = UPSTREAM_LIMITERS.read() {
        if let Some(limiter) = limiters.get(&key) {
            return Arc::clone(limiter);
        }
    }

    let limiter = Arc::new(Semaphore::new(*MAX_UPSTREAM_INFLIGHT_PER_PARTITION));
    if let Ok(mut limiters) = UPSTREAM_LIMITERS.write() {
        // Evict unused limiters when map grows too large.
        // strong_count == 1 means only the cache holds it (no active permits outstanding).
        if limiters.len() >= MAX_CACHED_UPSTREAM_LIMITERS {
            limiters.retain(|_, v| Arc::strong_count(v) > 1);
        }
        let entry = limiters.entry(key).or_insert_with(|| Arc::clone(&limiter));
        return Arc::clone(entry);
    }
    limiter
}

pub(super) fn should_prefer_http1_upstream(
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> bool {
    let key = fallback_key(unsafe_ssl, dns_servers, pool_partition);
    let now = std::time::Instant::now();

    if let Ok(fallbacks) = HTTP1_FALLBACKS.read() {
        if let Some(expires_at) = fallbacks.get(&key) {
            return *expires_at > now;
        }
    }

    cleanup_http1_fallbacks_if_needed(now);
    false
}

fn cleanup_http1_fallbacks_if_needed(now: std::time::Instant) {
    if let Ok(last_cleanup) = HTTP1_FALLBACK_LAST_CLEANUP.read() {
        if last_cleanup
            .as_ref()
            .is_some_and(|last| now.duration_since(*last) < HTTP1_FALLBACK_CLEANUP_INTERVAL)
        {
            return;
        }
    }

    if let Ok(mut last_cleanup) = HTTP1_FALLBACK_LAST_CLEANUP.write() {
        if last_cleanup
            .as_ref()
            .is_some_and(|last| now.duration_since(*last) < HTTP1_FALLBACK_CLEANUP_INTERVAL)
        {
            return;
        }
        *last_cleanup = Some(now);
        if let Ok(mut fallbacks) = HTTP1_FALLBACKS.write() {
            fallbacks.retain(|_, expires_at| *expires_at > now);
        }
    }
}

pub(super) fn mark_http1_fallback(unsafe_ssl: bool, dns_servers: &[String], pool_partition: &str) {
    if let Ok(mut fallbacks) = HTTP1_FALLBACKS.write() {
        fallbacks.insert(
            fallback_key(unsafe_ssl, dns_servers, pool_partition),
            std::time::Instant::now() + HTTP1_FALLBACK_TTL,
        );
    }
}

pub(super) fn is_retryable_http2_error(err: &ClientError) -> bool {
    let source_text = collect_error_source_chain(err)
        .join(" | ")
        .to_ascii_lowercase();
    let err_text = err.to_string().to_ascii_lowercase();
    let err_debug = format!("{err:?}").to_ascii_lowercase();
    source_text.contains("http2 error")
        || source_text.contains("connection closed before message completed")
        || source_text.contains("stream error")
        || err_text.contains("http2")
        || err_debug.contains("http2")
        || err_debug.contains("reset(streamid")
}

async fn send_pooled_request_with_protocol(
    request: Request<BoxBody>,
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
    protocol: ClientProtocolPreference,
) -> Result<hyper::Response<BoxBody>, ClientError> {
    let permit = get_upstream_limiter(unsafe_ssl, dns_servers, pool_partition)
        .acquire_owned()
        .await
        .expect("upstream limiter should not be closed");

    let response = get_https_client(unsafe_ssl, dns_servers, pool_partition, protocol)
        .request(request)
        .await?;
    Ok(response.map(|body| UpstreamPermitBody::new(body, permit).boxed()))
}

pub(super) async fn send_pooled_request(
    request: Request<BoxBody>,
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> Result<hyper::Response<BoxBody>, ClientError> {
    let protocol = if should_prefer_http1_upstream(unsafe_ssl, dns_servers, pool_partition) {
        ClientProtocolPreference::Http1Only
    } else {
        ClientProtocolPreference::Auto
    };
    send_pooled_request_with_protocol(request, unsafe_ssl, dns_servers, pool_partition, protocol)
        .await
}

pub(super) async fn send_pooled_request_http1_only(
    request: Request<BoxBody>,
    unsafe_ssl: bool,
    dns_servers: &[String],
    pool_partition: &str,
) -> Result<hyper::Response<BoxBody>, ClientError> {
    send_pooled_request_with_protocol(
        request,
        unsafe_ssl,
        dns_servers,
        pool_partition,
        ClientProtocolPreference::Http1Only,
    )
    .await
}

pub(super) fn classify_request_error(err: &ClientError) -> UpstreamRequestErrorInfo {
    let source_chain = collect_error_source_chain(err);
    let io_kind = find_io_error_kind(err);
    let source_text = source_chain.join(" | ").to_ascii_lowercase();
    let error_type = classify_error_type(err, io_kind, &source_text);

    let error_message = match source_chain.first() {
        Some(root_cause) => format!("Request Failed: {} | cause: {}", err, root_cause),
        None => format!("Request Failed: {}", err),
    };

    UpstreamRequestErrorInfo {
        error_type,
        error_message,
        source_chain,
    }
}

fn classify_error_type(
    err: &ClientError,
    io_kind: Option<io::ErrorKind>,
    source_text: &str,
) -> &'static str {
    classify_error_type_inner(err.is_connect(), io_kind, source_text)
}

fn classify_error_type_inner(
    is_connect: bool,
    io_kind: Option<io::ErrorKind>,
    source_text: &str,
) -> &'static str {
    if is_connect {
        if is_dns_failure(source_text) {
            return "REQUEST_DNS_FAILED";
        }
        if is_tls_failure(source_text) {
            return "REQUEST_TLS_FAILED";
        }
        if is_resource_exhaustion(io_kind, source_text) {
            return "REQUEST_CONNECT_RESOURCE_EXHAUSTED";
        }
        return match io_kind {
            Some(io::ErrorKind::TimedOut) => "REQUEST_CONNECT_TIMEOUT",
            Some(io::ErrorKind::ConnectionRefused) => "REQUEST_CONNECT_REFUSED",
            Some(io::ErrorKind::ConnectionReset) => "REQUEST_CONNECT_RESET",
            Some(io::ErrorKind::ConnectionAborted) => "REQUEST_CONNECT_ABORTED",
            Some(io::ErrorKind::AddrInUse) => "REQUEST_CONNECT_ADDR_IN_USE",
            Some(io::ErrorKind::AddrNotAvailable) => "REQUEST_CONNECT_ADDR_NOT_AVAILABLE",
            Some(io::ErrorKind::NotFound) => "REQUEST_CONNECT_NOT_FOUND",
            Some(io::ErrorKind::NetworkUnreachable) => "REQUEST_CONNECT_NETWORK_UNREACHABLE",
            Some(io::ErrorKind::HostUnreachable) => "REQUEST_CONNECT_HOST_UNREACHABLE",
            _ => "REQUEST_CONNECT_FAILED",
        };
    }

    if is_tls_failure(source_text) {
        return "REQUEST_TLS_FAILED";
    }
    if is_resource_exhaustion(io_kind, source_text) {
        return "REQUEST_RESOURCE_EXHAUSTED";
    }
    "REQUEST_FAILED"
}

fn collect_error_source_chain(err: &ClientError) -> Vec<String> {
    let mut source = err.source();
    let mut chain = Vec::new();
    while let Some(err) = source {
        chain.push(err.to_string());
        source = err.source();
    }
    chain
}

fn find_io_error_kind(err: &ClientError) -> Option<io::ErrorKind> {
    let mut source = err.source();
    while let Some(inner) = source {
        if let Some(io_err) = inner.downcast_ref::<io::Error>() {
            return Some(io_err.kind());
        }
        source = inner.source();
    }
    None
}

fn is_dns_failure(source_text: &str) -> bool {
    source_text.contains("dns error")
        || source_text.contains("failed to lookup address information")
        || source_text.contains("no such host")
        || source_text.contains("name or service not known")
        || source_text.contains("nodename nor servname provided")
        || source_text.contains("temporary failure in name resolution")
        || source_text.contains("resolve")
}

fn is_tls_failure(source_text: &str) -> bool {
    source_text.contains("tls")
        || source_text.contains("ssl")
        || source_text.contains("certificate")
        || source_text.contains("handshake")
        || source_text.contains("peer sent")
        || source_text.contains("invalid peer certificate")
        || source_text.contains("unknown issuer")
}

fn is_resource_exhaustion(io_kind: Option<io::ErrorKind>, source_text: &str) -> bool {
    matches!(io_kind, Some(io::ErrorKind::OutOfMemory))
        || source_text.contains("too many open files")
        || source_text.contains("cannot assign requested address")
        || source_text.contains("address not available")
        || source_text.contains("resource temporarily unavailable")
        || source_text.contains("no buffer space available")
        || source_text.contains("os error 24")
        || source_text.contains("os error 49")
        || source_text.contains("os error 55")
}

pub(super) fn get_tls_client_config(unsafe_ssl: bool) -> Arc<ClientConfig> {
    ensure_crypto_provider();

    // 允许 TLS 上游通过 ALPN 协商到 HTTP/2，从而避免被强制降级到 HTTP/1.1 造成大文件下载吞吐下降。
    // 这里显式打开 h2 + http/1.1，后续会根据协商结果选择对应的 Hyper handshake。
    let mut config = if unsafe_ssl {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(build_root_cert_store())
            .with_no_client_auth()
    };

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Arc::new(config)
}

pub(super) fn get_tls_client_config_http1_only(unsafe_ssl: bool) -> Arc<ClientConfig> {
    ensure_crypto_provider();

    let mut config = if unsafe_ssl {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(build_root_cert_store())
            .with_no_client_auth()
    };

    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

pub(super) fn get_tls_client_config_without_alpn(unsafe_ssl: bool) -> Arc<ClientConfig> {
    ensure_crypto_provider();

    let config = if unsafe_ssl {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(build_root_cert_store())
            .with_no_client_auth()
    };

    Arc::new(config)
}

pub(super) fn sanitize_upstream_headers(headers: &mut hyper::HeaderMap) {
    use hyper::header;

    // RFC7540: HTTP/2 禁止 hop-by-hop headers。
    // 同时移除 Connection 指定的额外 header。
    if let Some(connection) = headers.remove(header::CONNECTION) {
        if let Ok(connection) = connection.to_str() {
            for token in connection
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
            {
                headers.remove(token);
            }
        }
    }
    headers.remove("proxy-connection");
    headers.remove("keep-alive");
    headers.remove("transfer-encoding");
    headers.remove("upgrade");
    headers.remove("trailer");

    // TE 在 HTTP/2 仅允许 "trailers"。
    if let Some(te) = headers.get(header::TE).and_then(|v| v.to_str().ok()) {
        if !te.trim().eq_ignore_ascii_case("trailers") {
            headers.remove(header::TE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_dns_failure_before_generic_connect() {
        assert_eq!(
            classify_error_type_inner(
                true,
                Some(io::ErrorKind::Other),
                "dns error: failed to lookup address information",
            ),
            "REQUEST_DNS_FAILED"
        );
    }

    #[test]
    fn classifies_connect_timeout() {
        assert_eq!(
            classify_error_type_inner(true, Some(io::ErrorKind::TimedOut), "connection timeout",),
            "REQUEST_CONNECT_TIMEOUT"
        );
    }

    #[test]
    fn classifies_resource_exhaustion() {
        assert_eq!(
            classify_error_type_inner(
                true,
                Some(io::ErrorKind::AddrNotAvailable),
                "cannot assign requested address",
            ),
            "REQUEST_CONNECT_RESOURCE_EXHAUSTED"
        );
    }

    #[test]
    fn classifies_tls_failure() {
        assert_eq!(
            classify_error_type_inner(true, Some(io::ErrorKind::Other), "tls handshake eof",),
            "REQUEST_TLS_FAILED"
        );
    }

    #[test]
    fn marks_http1_fallback_for_partition() {
        let pool_partition = "orig=example.com|target=https://example.com:443";
        assert!(!should_prefer_http1_upstream(false, &[], pool_partition));
        mark_http1_fallback(false, &[], pool_partition);
        assert!(should_prefer_http1_upstream(false, &[], pool_partition));
        assert!(!should_prefer_http1_upstream(
            false,
            &[],
            "orig=other|target=https://other:443"
        ));
    }

    #[tokio::test]
    async fn releases_upstream_permit_when_body_drops() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let body = UpstreamPermitBody::new(http_body_util::Empty::<Bytes>::new(), permit);

        assert!(semaphore.try_acquire().is_err());
        drop(body);
        assert!(semaphore.try_acquire().is_ok());
    }
}
