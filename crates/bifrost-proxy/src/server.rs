use std::collections::HashMap;
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt;

use regex::Regex;

use bifrost_admin::{
    handle_sync_login_callback, is_cert_public_request, is_valid_admin_request, AdminRouter,
    AdminSecurityConfig, AdminState, SharedPushManager, ADMIN_PATH_PREFIX, CERT_PUBLIC_PATH_PREFIX,
};
use bifrost_core::{BifrostError, Protocol, Result};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::HeaderMap;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, Semaphore};
use tokio_rustls::rustls::ServerConfig as RustlsServerConfig;
use tracing::{debug, error, info, warn};

use crate::dns::DnsResolver;
use crate::proxy::http::{
    handle_connect, handle_http_request, requires_client_app_for_tls_decision, SingleCertResolver,
};
use crate::proxy::socks::{SocksConfig, SocksHandler, SocksServer, UdpRelay};
use crate::unified::{DetectedProtocol, PeekableStream};
use crate::utils::logging::RequestContext;
use crate::utils::process_info::{
    app_policy_process_resolution_retry_config, resolve_client_process_async_for_connection,
    resolve_client_process_async_for_connection_with_retry, spawn_async_process_resolver,
    ClientProcess,
};
use bifrost_core::{AccessControlConfig, AccessDecision, AccessMode, ClientAccessControl};

const NOISY_CONN_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct NoisyConnErrorLogState {
    last_log_at: Option<Instant>,
    suppressed: u64,
}

static INCOMPLETE_MESSAGE_LOG_STATE: LazyLock<Mutex<NoisyConnErrorLogState>> =
    LazyLock::new(|| Mutex::new(NoisyConnErrorLogState::default()));

#[cfg(unix)]
fn is_fd_limit_error(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(code) if code == libc::EMFILE || code == libc::ENFILE)
}

#[cfg(windows)]
fn is_fd_limit_error(err: &std::io::Error) -> bool {
    const WSAEMFILE: i32 = 10024;
    matches!(err.raw_os_error(), Some(WSAEMFILE))
}

#[cfg(not(any(unix, windows)))]
fn is_fd_limit_error(_err: &std::io::Error) -> bool {
    false
}

fn log_connection_serve_error(peer_addr: SocketAddr, err: &hyper::Error) {
    let err_debug = format!("{err:?}");
    let is_noisy_disconnect =
        err_debug.contains("IncompleteMessage") || err.to_string().contains("message completed");

    if !is_noisy_disconnect {
        error!("Error serving connection from {}: {:?}", peer_addr, err);
        return;
    }

    let now = Instant::now();
    let mut state = INCOMPLETE_MESSAGE_LOG_STATE
        .lock()
        .expect("incomplete message log state poisoned");

    match state.last_log_at {
        Some(last_log_at) if now.duration_since(last_log_at) < NOISY_CONN_ERROR_LOG_INTERVAL => {
            state.suppressed += 1;
        }
        _ => {
            let suppressed = std::mem::take(&mut state.suppressed);
            state.last_log_at = Some(now);
            warn!(
                "Noisy connection close from {}: {:?} (suppressed {} similar events in last {:?})",
                peer_addr, err, suppressed, NOISY_CONN_ERROR_LOG_INTERVAL
            );
        }
    }
}

fn should_defer_client_process_resolution(
    method: &Method,
    verbose_logging: bool,
    has_script_manager: bool,
) -> bool {
    // Client process information is part of the policy input for CONNECT/TLS interception.
    // That path must resolve the process before making an interception decision.
    method != Method::CONNECT && !verbose_logging && !has_script_manager
}

#[derive(Debug, Clone)]
pub struct TlsInterceptConfig {
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub unsafe_ssl: bool,
}

impl TlsInterceptConfig {
    pub fn from_proxy_config(config: &ProxyConfig) -> Self {
        Self {
            enable_tls_interception: config.enable_tls_interception,
            intercept_exclude: config.intercept_exclude.clone(),
            intercept_include: config.intercept_include.clone(),
            app_intercept_exclude: config.app_intercept_exclude.clone(),
            app_intercept_include: config.app_intercept_include.clone(),
            unsafe_ssl: config.unsafe_ssl,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub port: u16,
    pub host: String,
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub timeout_secs: u64,
    pub http1_max_header_size: usize,
    pub http2_max_header_list_size: usize,
    pub websocket_handshake_max_header_size: usize,
    pub socks5_port: Option<u16>,
    pub socks5_auth_required: bool,
    pub socks5_username: Option<String>,
    pub socks5_password: Option<String>,
    pub verbose_logging: bool,
    pub access_mode: AccessMode,
    pub client_whitelist: Vec<String>,
    pub allow_lan: bool,
    pub unsafe_ssl: bool,
    pub max_body_buffer_size: usize,
    pub max_body_probe_size: usize,
    pub binary_traffic_performance_mode: bool,
    pub enable_socks: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: 9900,
            host: "127.0.0.1".to_string(),
            enable_tls_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: vec![
                "Google Chrome*".to_string(),
                "Microsoft Edge*".to_string(),
                "*Safari*".to_string(),
                "*Firefox*".to_string(),
                "*Opera*".to_string(),
                "*Brave*".to_string(),
                "*Arc*".to_string(),
                "*Vivaldi*".to_string(),
            ],
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
            socks5_port: None,
            socks5_auth_required: false,
            socks5_username: None,
            socks5_password: None,
            verbose_logging: false,
            access_mode: AccessMode::LocalOnly,
            client_whitelist: Vec::new(),
            allow_lan: false,
            unsafe_ssl: false,
            max_body_buffer_size: 10 * 1024 * 1024, // 10MB
            max_body_probe_size: 64 * 1024,
            binary_traffic_performance_mode: true,
            enable_socks: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuleValue {
    pub pattern: String,
    pub protocol: Protocol,
    pub value: String,
    pub options: HashMap<String, String>,
    pub rule_name: Option<String>,
    pub raw: Option<String>,
    pub line: Option<usize>,
}

#[derive(Clone)]
pub struct RegexReplace {
    pub pattern: Regex,
    pub replacement: String,
    pub global: bool,
}

impl std::fmt::Debug for RegexReplace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegexReplace")
            .field("pattern", &self.pattern.as_str())
            .field("replacement", &self.replacement)
            .field("global", &self.global)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct ResCookieValue {
    pub value: String,
    pub max_age: Option<i64>,
    pub path: Option<String>,
    pub domain: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

impl std::fmt::Display for ResCookieValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl ResCookieValue {
    pub fn simple(value: String) -> Self {
        Self {
            value,
            max_age: None,
            path: None,
            domain: None,
            secure: false,
            http_only: false,
            same_site: None,
        }
    }

    pub fn to_set_cookie_string(&self, name: &str) -> String {
        let escaped_name = escape_cookie_name(name);
        let escaped_value = escape_cookie_value(&self.value);
        let mut parts = vec![format!("{}={}", escaped_name, escaped_value)];

        if let Some(max_age) = self.max_age {
            parts.push(format!("Max-Age={}", max_age));
            let expires = chrono::Utc::now() + chrono::Duration::seconds(max_age);
            parts.push(format!(
                "Expires={}",
                expires.format("%a, %d %b %Y %H:%M:%S GMT")
            ));
        }
        if let Some(ref path) = self.path {
            parts.push(format!("Path={}", path));
        }
        if let Some(ref domain) = self.domain {
            parts.push(format!("Domain={}", domain));
        }
        if self.secure {
            parts.push("Secure".to_string());
        }
        if self.http_only {
            parts.push("HttpOnly".to_string());
        }
        if let Some(ref same_site) = self.same_site {
            parts.push(format!("SameSite={}", same_site));
        }

        parts.join("; ")
    }
}

fn escape_cookie_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.~!$&'()*+,;=".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

fn escape_cookie_value(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii()
                && !c.is_ascii_control()
                && c != '"'
                && c != ','
                && c != ';'
                && c != '\\'
            {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct CorsConfig {
    pub enabled: bool,
    pub origin: Option<String>,
    pub methods: Option<String>,
    pub headers: Option<String>,
    pub expose_headers: Option<String>,
    pub credentials: Option<bool>,
    pub max_age: Option<u64>,
}

impl CorsConfig {
    pub fn enable_all() -> Self {
        Self {
            enabled: true,
            origin: Some("*".to_string()),
            methods: None,
            headers: None,
            expose_headers: None,
            credentials: Some(true),
            max_age: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[derive(Debug, Clone, Default)]
pub struct IgnoredFields {
    pub host: bool,
    pub all: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedRules {
    pub host: Option<String>,
    pub host_protocol: Option<Protocol>,
    pub proxy: Option<String>,
    pub upstream_http3: bool,
    pub req_headers: Vec<(String, String)>,
    pub res_headers: Vec<(String, String)>,
    pub req_body: Option<Bytes>,
    pub res_body: Option<Bytes>,
    pub req_cookies: Vec<(String, String)>,
    pub res_cookies: Vec<(String, ResCookieValue)>,
    pub req_del_cookies: Vec<String>,
    pub res_del_cookies: Vec<String>,
    pub req_delay: Option<u64>,
    pub res_delay: Option<u64>,
    pub status_code: Option<u16>,
    pub method: Option<String>,
    pub ua: Option<String>,
    pub referer: Option<String>,
    pub req_cors: CorsConfig,
    pub res_cors: CorsConfig,
    pub rules: Vec<RuleValue>,

    pub req_prepend: Option<Bytes>,
    pub req_append: Option<Bytes>,
    pub res_prepend: Option<Bytes>,
    pub res_append: Option<Bytes>,
    pub req_replace: Vec<(String, String)>,
    pub res_replace: Vec<(String, String)>,
    pub req_replace_regex: Vec<RegexReplace>,
    pub res_replace_regex: Vec<RegexReplace>,
    pub req_merge: Option<serde_json::Value>,
    pub res_merge: Option<serde_json::Value>,

    pub url_params: Vec<(String, String)>,
    pub url_replace: Vec<(String, String)>,
    pub url_replace_regex: Vec<RegexReplace>,

    pub req_type: Option<String>,
    pub req_charset: Option<String>,

    pub res_type: Option<String>,
    pub res_charset: Option<String>,
    pub replace_status: Option<u16>,
    pub cache: Option<String>,
    pub attachment: Option<String>,

    pub ignored: IgnoredFields,

    pub mock_file: Option<String>,
    pub mock_rawfile: Option<String>,
    pub mock_template: Option<String>,

    pub redirect: Option<String>,
    pub redirect_status: Option<u16>,
    pub location_href: Option<String>,

    pub req_speed: Option<u64>,
    pub res_speed: Option<u64>,

    pub html_append: Option<String>,
    pub html_prepend: Option<String>,
    pub html_body: Option<String>,
    pub js_append: Option<String>,
    pub js_prepend: Option<String>,
    pub js_body: Option<String>,
    pub css_append: Option<String>,
    pub css_prepend: Option<String>,
    pub css_body: Option<String>,

    pub dns_servers: Vec<String>,

    pub tls_intercept: Option<bool>,
    pub tls_options: Option<String>,
    pub sni_callback: Option<String>,

    pub req_scripts: Vec<String>,
    pub res_scripts: Vec<String>,
    pub decode_scripts: Vec<String>,

    pub auth: Option<String>,
    pub delete_req_headers: Vec<String>,
    pub delete_res_headers: Vec<String>,
    pub delete_url_params: Vec<String>,
    pub header_replace: Vec<HeaderReplaceRule>,

    pub values: std::collections::HashMap<String, String>,

    pub trailers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct HeaderReplaceRule {
    pub target: HeaderReplaceTarget,
    pub header_name: String,
    pub pattern: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HeaderReplaceTarget {
    Request,
    Response,
}

pub trait RulesResolver: Send + Sync {
    fn values(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    fn resolve(&self, url: &str, method: &str) -> ResolvedRules {
        self.resolve_with_context(
            url,
            method,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        )
    }

    fn resolve_with_context(
        &self,
        url: &str,
        method: &str,
        req_headers: &std::collections::HashMap<String, String>,
        req_cookies: &std::collections::HashMap<String, String>,
    ) -> ResolvedRules;

    fn has_response_rules_for_host(&self, _host: &str) -> bool {
        false
    }
}

#[derive(Default)]
pub struct NoOpRulesResolver;

impl RulesResolver for NoOpRulesResolver {
    fn resolve_with_context(
        &self,
        _url: &str,
        _method: &str,
        _req_headers: &std::collections::HashMap<String, String>,
        _req_cookies: &std::collections::HashMap<String, String>,
    ) -> ResolvedRules {
        ResolvedRules::default()
    }
}

#[derive(Default)]
pub struct TlsConfig {
    pub ca_cert: Option<Vec<u8>>,
    pub ca_key: Option<Vec<u8>>,
    pub cert_generator: Option<Arc<bifrost_tls::DynamicCertGenerator>>,
    pub sni_resolver: Option<Arc<bifrost_tls::SniResolver>>,
}

impl TlsConfig {
    pub fn resolve_server_config(
        &self,
        server_name: &str,
        alpn_protocols: &[Vec<u8>],
    ) -> Result<Arc<RustlsServerConfig>> {
        if let Some(ref sni_resolver) = self.sni_resolver {
            return sni_resolver.resolve_server_config_with_alpn(server_name, alpn_protocols);
        }

        let certified_key = if let Some(ref cert_generator) = self.cert_generator {
            Arc::new(cert_generator.generate_for_domain(server_name)?)
        } else {
            return Err(BifrostError::Tls(
                "TLS interception enabled but cert generator not configured".to_string(),
            ));
        };

        let mut server_config = RustlsServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleCertResolver(certified_key)));
        server_config.alpn_protocols = alpn_protocols.to_vec();

        Ok(Arc::new(server_config))
    }
}

pub struct ProxyServer {
    config: ProxyConfig,
    rules: Arc<dyn RulesResolver>,
    tls_config: Arc<TlsConfig>,
    admin_state: Option<Arc<AdminState>>,
    push_manager: Option<SharedPushManager>,
    admin_security_config: AdminSecurityConfig,
    access_control: Arc<RwLock<ClientAccessControl>>,
    dns_resolver: Arc<DnsResolver>,
    udp_relay_addr: Arc<RwLock<Option<SocketAddr>>>,
    #[allow(dead_code)]
    udp_relay: Arc<RwLock<Option<UdpRelay>>>,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig) -> Self {
        let admin_security_config = AdminSecurityConfig::new(config.port);
        let access_config = AccessControlConfig {
            mode: config.access_mode,
            whitelist: config.client_whitelist.clone(),
            allow_lan: config.allow_lan,
        };
        let dns_resolver = Arc::new(DnsResolver::new(config.verbose_logging));
        Self {
            config,
            rules: Arc::new(NoOpRulesResolver),
            tls_config: Arc::new(TlsConfig::default()),
            admin_state: None,
            push_manager: None,
            admin_security_config,
            access_control: Arc::new(RwLock::new(ClientAccessControl::new(access_config))),
            dns_resolver,
            udp_relay_addr: Arc::new(RwLock::new(None)),
            udp_relay: Arc::new(RwLock::new(None)),
        }
    }

    pub fn with_rules(mut self, rules: Arc<dyn RulesResolver>) -> Self {
        self.rules = rules;
        self
    }

    pub fn with_tls_config(mut self, tls_config: Arc<TlsConfig>) -> Self {
        self.tls_config = tls_config;
        self
    }

    pub fn with_access_control(mut self, access_control: Arc<RwLock<ClientAccessControl>>) -> Self {
        self.access_control = access_control;
        self
    }

    pub fn with_admin_state(mut self, admin_state: AdminState) -> Self {
        let admin_state = admin_state.with_access_control(Arc::clone(&self.access_control));
        self.admin_state = Some(Arc::new(admin_state));
        self
    }

    pub fn with_admin_state_shared(mut self, admin_state: Arc<AdminState>) -> Self {
        self.admin_state = Some(admin_state);
        self
    }

    pub fn with_push_manager(mut self, push_manager: SharedPushManager) -> Self {
        self.push_manager = Some(push_manager);
        self
    }

    pub fn push_manager(&self) -> Option<&SharedPushManager> {
        self.push_manager.as_ref()
    }

    pub fn config(&self) -> &ProxyConfig {
        &self.config
    }

    pub fn admin_state(&self) -> Option<&Arc<AdminState>> {
        self.admin_state.as_ref()
    }

    pub fn access_control(&self) -> &Arc<RwLock<ClientAccessControl>> {
        &self.access_control
    }

    pub async fn bind(&self, addr: SocketAddr) -> Result<TcpListener> {
        let check_addr = if addr.ip().is_unspecified() {
            SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                addr.port(),
            )
        } else {
            addr
        };
        if let Ok(Ok(_)) = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            tokio::net::TcpStream::connect(check_addr),
        )
        .await
        {
            return Err(BifrostError::Network(format!(
                "Failed to bind to {}: another process is already listening on this port",
                addr
            )));
        }

        TcpListener::bind(addr)
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to bind to {}: {}", addr, e)))
    }

    pub async fn run(&self) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| BifrostError::Config(format!("Invalid address: {}", e)))?;

        let listener = self.bind(addr).await?;
        self.run_with_listener(listener).await
    }

    pub async fn run_with_listener(&self, listener: TcpListener) -> Result<()> {
        let addr = listener
            .local_addr()
            .map_err(|e| BifrostError::Network(format!("Failed to get listener address: {}", e)))?;

        if self.config.enable_socks {
            let udp_addr = SocketAddr::new(addr.ip(), addr.port());
            let mut udp_relay = UdpRelay::new(udp_addr)
                .with_rules(Arc::clone(&self.rules))
                .with_access_control(Arc::clone(&self.access_control));
            let udp_relay_started_addr = udp_relay.start().await?;
            {
                let mut relay_addr = self.udp_relay_addr.write().await;
                *relay_addr = Some(udp_relay_started_addr);
            }
            {
                let mut relay = self.udp_relay.write().await;
                *relay = Some(udp_relay);
            }
            info!(
                "Unified proxy server listening on {} (HTTP/HTTPS/SOCKS5), UDP relay on {}",
                addr, udp_relay_started_addr
            );
        } else {
            info!("Proxy server listening on {} (HTTP/HTTPS only)", addr);
        }

        if let Some(socks5_port) = self.config.socks5_port {
            let socks_config = SocksConfig {
                port: socks5_port,
                host: self.config.host.clone(),
                auth_required: self.config.socks5_auth_required,
                username: self.config.socks5_username.clone(),
                password: self.config.socks5_password.clone(),
                timeout_secs: self.config.timeout_secs,
                access_mode: self.config.access_mode,
                client_whitelist: self.config.client_whitelist.clone(),
                allow_lan: self.config.allow_lan,
                enable_udp: true,
                udp_port: None,
            };
            let tls_intercept_config = TlsInterceptConfig::from_proxy_config(&self.config);
            let socks_server = SocksServer::new(socks_config)
                .with_rules(Arc::clone(&self.rules))
                .with_access_control(Arc::clone(&self.access_control))
                .with_verbose_logging(self.config.verbose_logging)
                .with_unsafe_ssl(self.config.unsafe_ssl)
                .with_dns_resolver(Arc::clone(&self.dns_resolver))
                .with_tls_intercept(
                    Arc::clone(&self.tls_config),
                    tls_intercept_config,
                    self.admin_state.clone(),
                );

            info!(
                "Separate SOCKS5 server also listening on {}:{}",
                self.config.host, socks5_port
            );

            let http_future = self.serve(listener);
            let socks_future = socks_server.run();

            tokio::select! {
                result = http_future => result,
                result = socks_future => result,
            }
        } else {
            self.serve(listener).await
        }
    }

    pub async fn serve(&self, listener: TcpListener) -> Result<()> {
        const MAX_CONNECTIONS: usize = 10000;
        const EMFILE_BACKOFF_MS: u64 = 2000;
        const GENERAL_BACKOFF_MS: u64 = 500;
        const MAX_BACKOFF_MS: u64 = 10000;
        const FATAL_CONSECUTIVE_ERRORS: u32 = 200;

        let conn_semaphore = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut consecutive_errors = 0u32;
        let mut last_emfile_warn = Instant::now() - Duration::from_secs(60);

        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(conn) => {
                    if consecutive_errors > 0 {
                        info!(
                            recovered_after = consecutive_errors,
                            "accept recovered from transient errors"
                        );
                    }
                    consecutive_errors = 0;
                    conn
                }
                Err(e) => {
                    consecutive_errors += 1;
                    let is_emfile = is_fd_limit_error(&e);

                    if is_emfile {
                        if last_emfile_warn.elapsed() >= Duration::from_secs(5) {
                            error!(
                                consecutive_errors,
                                available_permits = conn_semaphore.available_permits(),
                                "accept hit file descriptor limit (EMFILE), backing off"
                            );
                            last_emfile_warn = Instant::now();
                        }
                        let backoff = (EMFILE_BACKOFF_MS * consecutive_errors.min(5) as u64)
                            .min(MAX_BACKOFF_MS);
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                    } else {
                        error!(
                            consecutive_errors,
                            error = %e,
                            "failed to accept connection"
                        );

                        if consecutive_errors >= FATAL_CONSECUTIVE_ERRORS {
                            return Err(BifrostError::Network(format!(
                                "too many consecutive non-EMFILE accept errors ({}): {}",
                                consecutive_errors, e
                            )));
                        }

                        let backoff =
                            (GENERAL_BACKOFF_MS * consecutive_errors as u64).min(MAX_BACKOFF_MS);
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                    }
                    continue;
                }
            };

            let permit = match conn_semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    warn!(
                        peer_addr = %peer_addr,
                        max_connections = MAX_CONNECTIONS,
                        "connection limit reached, dropping new connection"
                    );
                    drop(stream);
                    continue;
                }
            };

            debug!("Accepted connection from {}", peer_addr);

            let decision = {
                let access_control = self.access_control.read().await;
                access_control.check_access(&peer_addr.ip())
            };

            match decision {
                AccessDecision::Allow => {}
                AccessDecision::Deny => {
                    warn!(
                        "Access denied for client {} (not in whitelist)",
                        peer_addr.ip()
                    );
                    continue;
                }
                AccessDecision::Prompt(ip) => {
                    {
                        let access_control = self.access_control.read().await;
                        access_control.add_pending_authorization(ip);
                    }
                    warn!(
                        "Non-whitelisted client {} added to pending authorization. \
                        Approve via admin UI or use `bifrost whitelist add {}`",
                        ip, ip
                    );
                    continue;
                }
            }

            let rules = Arc::clone(&self.rules);
            let tls_config = Arc::clone(&self.tls_config);
            let proxy_config = self.config.clone();
            let admin_state = self.admin_state.clone();
            let push_manager = self.push_manager.clone();
            let admin_security_config = self.admin_security_config.clone();
            let dns_resolver = Arc::clone(&self.dns_resolver);
            let access_control = Arc::clone(&self.access_control);
            let initial_generation = {
                let ac = self.access_control.read().await;
                ac.generation()
            };
            let enable_socks = self.config.enable_socks;
            let socks5_auth_required = self.config.socks5_auth_required;
            let socks5_username = self.config.socks5_username.clone();
            let socks5_password = self.config.socks5_password.clone();
            let timeout_secs = self.config.timeout_secs;
            let udp_relay_addr = Arc::clone(&self.udp_relay_addr);

            tokio::spawn(async move {
                let _permit = permit;
                let connection_task = async {
                    if enable_socks {
                        let mut peekable = PeekableStream::new(stream);
                        match peekable.detect_protocol().await {
                            Ok(DetectedProtocol::Socks5) => {
                                debug!(
                                    "Detected SOCKS5 protocol from {} on unified port",
                                    peer_addr
                                );
                                let stream = peekable.into_inner();
                                let tls_intercept_config =
                                    TlsInterceptConfig::from_proxy_config(&proxy_config);
                                let current_udp_relay_addr = {
                                    let addr = udp_relay_addr.read().await;
                                    *addr
                                };
                                let handler = SocksHandler::new(
                                    stream,
                                    peer_addr,
                                    socks5_auth_required,
                                    socks5_username,
                                    socks5_password,
                                    timeout_secs,
                                    current_udp_relay_addr,
                                )
                                .with_rules(Arc::clone(&rules))
                                .with_verbose_logging(proxy_config.verbose_logging)
                                .with_unsafe_ssl(proxy_config.unsafe_ssl)
                                .with_dns_resolver(Arc::clone(&dns_resolver))
                                .with_tls_intercept(
                                    Arc::clone(&tls_config),
                                    tls_intercept_config,
                                    admin_state.clone(),
                                );
                                if let Err(e) = handler.handle().await {
                                    debug!("SOCKS5 handler error for {}: {}", peer_addr, e);
                                }
                                return;
                            }
                            Ok(DetectedProtocol::Socks4) => {
                                warn!(
                                    "SOCKS4 connection from {} rejected - SOCKS4 protocol is not supported, please use SOCKS5 instead",
                                    peer_addr
                                );
                                return;
                            }
                            Ok(_) | Err(_) => {}
                        }
                        handle_http_connection(
                            peekable.into_inner(),
                            peer_addr,
                            rules,
                            tls_config,
                            proxy_config,
                            admin_state,
                            push_manager,
                            admin_security_config,
                            dns_resolver,
                            access_control,
                            initial_generation,
                        )
                        .await;
                    } else {
                        handle_http_connection(
                            stream,
                            peer_addr,
                            rules,
                            tls_config,
                            proxy_config,
                            admin_state,
                            push_manager,
                            admin_security_config,
                            dns_resolver,
                            access_control,
                            initial_generation,
                        )
                        .await;
                    }
                };

                let result = AssertUnwindSafe(connection_task).catch_unwind().await;

                if let Err(panic_err) = result {
                    let panic_msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic_err.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic".to_string()
                    };
                    error!(
                        "Connection handler for {} panicked: {}",
                        peer_addr, panic_msg
                    );
                }
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_http_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    rules: Arc<dyn RulesResolver>,
    tls_config: Arc<TlsConfig>,
    proxy_config: ProxyConfig,
    admin_state: Option<Arc<AdminState>>,
    push_manager: Option<SharedPushManager>,
    admin_security_config: AdminSecurityConfig,
    dns_resolver: Arc<DnsResolver>,
    access_control: Arc<tokio::sync::RwLock<ClientAccessControl>>,
    initial_generation: u64,
) {
    let local_addr = match stream.local_addr() {
        Ok(addr) => addr,
        Err(err) => {
            warn!(
                peer_addr = %peer_addr,
                error = %err,
                "Failed to get local socket address for client process resolution"
            );
            peer_addr
        }
    };
    let io = TokioIo::new(stream);
    let client_process_cache = Arc::new(Mutex::new(None::<ClientProcess>));
    let client_process_resolution_started = Arc::new(AtomicBool::new(false));

    if peer_addr.ip().is_loopback() {
        let cache = Arc::clone(&client_process_cache);
        let started = Arc::clone(&client_process_resolution_started);
        tokio::spawn(async move {
            let result = resolve_client_process_async_for_connection_with_retry(
                &peer_addr,
                &local_addr,
                20,
                50,
            )
            .await;
            if let Some(process) = result {
                started.store(true, Ordering::Release);
                if let Ok(mut guard) = cache.lock() {
                    *guard = Some(process);
                }
            }
        });
    }
    let http1_max_header_size = if let Some(ref state) = admin_state {
        if let Some(ref config_manager) = state.config_manager {
            config_manager.config().await.server.http1_max_header_size
        } else {
            proxy_config.http1_max_header_size
        }
    } else {
        proxy_config.http1_max_header_size
    };

    let service = service_fn(move |req: Request<Incoming>| {
        let rules = Arc::clone(&rules);
        let tls_config = Arc::clone(&tls_config);
        let proxy_config = proxy_config.clone();
        let admin_state = admin_state.clone();
        let push_manager = push_manager.clone();
        let admin_security_config = admin_security_config.clone();
        let dns_resolver = Arc::clone(&dns_resolver);
        let access_control = Arc::clone(&access_control);
        let client_process_cache = Arc::clone(&client_process_cache);
        let client_process_resolution_started = Arc::clone(&client_process_resolution_started);
        async move {
            handle_request(
                req,
                peer_addr,
                local_addr,
                rules,
                tls_config,
                proxy_config,
                admin_state,
                push_manager,
                admin_security_config,
                dns_resolver,
                access_control,
                initial_generation,
                client_process_cache,
                client_process_resolution_started,
            )
            .await
        }
    });

    let mut builder = http1::Builder::new();
    builder
        .preserve_header_case(true)
        .title_case_headers(true)
        .max_buf_size(http1_max_header_size);

    if let Err(err) = builder.serve_connection(io, service).with_upgrades().await {
        log_connection_serve_error(peer_addr, &err);
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_request(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    rules: Arc<dyn RulesResolver>,
    tls_config: Arc<TlsConfig>,
    proxy_config: ProxyConfig,
    admin_state: Option<Arc<AdminState>>,
    push_manager: Option<SharedPushManager>,
    admin_security_config: AdminSecurityConfig,
    dns_resolver: Arc<DnsResolver>,
    access_control: Arc<tokio::sync::RwLock<ClientAccessControl>>,
    initial_generation: u64,
    client_process_cache: Arc<Mutex<Option<ClientProcess>>>,
    client_process_resolution_started: Arc<AtomicBool>,
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path();
    let verbose_logging = proxy_config.verbose_logging;
    let is_local_client = peer_addr.ip().is_loopback();
    let can_defer_client_process = should_defer_client_process_resolution(
        &method,
        verbose_logging,
        admin_state
            .as_ref()
            .is_some_and(|state| state.script_manager.is_some()),
    );

    let connect_tls_intercept_config = if method == Method::CONNECT {
        Some(if let Some(ref state) = admin_state {
            let runtime_config = state.runtime_config.read().await;
            TlsInterceptConfig {
                enable_tls_interception: runtime_config.enable_tls_interception,
                intercept_exclude: runtime_config.intercept_exclude.clone(),
                intercept_include: runtime_config.intercept_include.clone(),
                app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
                app_intercept_include: runtime_config.app_intercept_include.clone(),
                unsafe_ssl: runtime_config.unsafe_ssl,
            }
        } else {
            TlsInterceptConfig::from_proxy_config(&proxy_config)
        })
    } else {
        None
    };

    let mut ctx = RequestContext::new()
        .with_client_ip(peer_addr.ip().to_string())
        .with_port(proxy_config.port);
    let cached_client_process = client_process_cache
        .lock()
        .ok()
        .and_then(|cache| cache.clone());

    let client_process = if let Some(client_process) = cached_client_process {
        Some(client_process)
    } else if let Some(ref tls_intercept_config) = connect_tls_intercept_config {
        let client_process =
            if is_local_client && requires_client_app_for_tls_decision(tls_intercept_config) {
                let (max_retries, delay_ms) = app_policy_process_resolution_retry_config();
                info!(
                    req_id = ctx.id_str(),
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    max_retries,
                    delay_ms,
                    "CONNECT request requires synchronous client process resolution for app policy"
                );
                resolve_client_process_async_for_connection_with_retry(
                    &peer_addr,
                    &local_addr,
                    max_retries,
                    delay_ms,
                )
                .await
            } else {
                resolve_client_process_async_for_connection(&peer_addr, &local_addr).await
            };
        if is_local_client && requires_client_app_for_tls_decision(tls_intercept_config) {
            match client_process.as_ref() {
                Some(process) => info!(
                    req_id = ctx.id_str(),
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    client_app = %process.name,
                    client_pid = process.pid,
                    "CONNECT synchronous client process resolution succeeded"
                ),
                None => warn!(
                    req_id = ctx.id_str(),
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    "CONNECT synchronous client process resolution returned unknown client"
                ),
            }
        }
        if let Some(ref client_process) = client_process {
            if let Ok(mut cache) = client_process_cache.lock() {
                *cache = Some(client_process.clone());
            }
        }
        client_process
    } else if can_defer_client_process {
        if let Some(state) = admin_state.clone() {
            let should_spawn = !client_process_resolution_started.swap(true, Ordering::AcqRel);
            if should_spawn {
                let record_id = ctx.id_str();
                let client_process_cache = Arc::clone(&client_process_cache);
                spawn_async_process_resolver(
                    peer_addr,
                    local_addr,
                    record_id.to_string(),
                    move |id, process| {
                        if let Ok(mut cache) = client_process_cache.lock() {
                            *cache = Some(process.clone());
                        }
                        state.update_client_process(&id, process.name, process.pid, process.path);
                    },
                );
            }
        }
        None
    } else {
        let client_process =
            resolve_client_process_async_for_connection(&peer_addr, &local_addr).await;
        if let Some(ref client_process) = client_process {
            if let Ok(mut cache) = client_process_cache.lock() {
                *cache = Some(client_process.clone());
            }
        }
        let client_process = if client_process.is_some() {
            client_process
        } else if is_local_client {
            let mut resolved = None;
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let Ok(guard) = client_process_cache.lock() {
                    if guard.is_some() {
                        resolved = guard.clone();
                        break;
                    }
                }
            }
            if resolved.is_none() {
                if let Some(state) = admin_state.clone() {
                    let should_spawn =
                        !client_process_resolution_started.swap(true, Ordering::AcqRel);
                    if should_spawn {
                        let record_id = ctx.id_str();
                        let client_process_cache = Arc::clone(&client_process_cache);
                        spawn_async_process_resolver(
                            peer_addr,
                            local_addr,
                            record_id.to_string(),
                            move |id, process| {
                                if let Ok(mut cache) = client_process_cache.lock() {
                                    *cache = Some(process.clone());
                                }
                                state.update_client_process(
                                    &id,
                                    process.name,
                                    process.pid,
                                    process.path,
                                );
                            },
                        );
                    }
                }
            }
            resolved
        } else {
            None
        };
        client_process
    };
    let (client_app, client_pid, client_path) = client_process
        .as_ref()
        .map(|p| (Some(p.name.clone()), Some(p.pid), p.path.clone()))
        .unwrap_or((None, None, None));

    ctx = ctx.with_client_process(client_app, client_pid, client_path);

    let client_info = client_process
        .as_ref()
        .map(|p| p.name.as_str())
        .unwrap_or_else(|| "unknown");

    if verbose_logging {
        info!(
            "[{}] --> {} {} (from {} - {})",
            ctx.id_str(),
            method,
            uri,
            peer_addr,
            client_info
        );
    } else {
        debug!(
            "Received request: {} {} from {} ({})",
            method, uri, peer_addr, client_info
        );
    }

    let is_public_cert_path = path.starts_with(CERT_PUBLIC_PATH_PREFIX);
    let is_loopback = peer_addr.ip().is_loopback();
    if !is_public_cert_path && !is_loopback {
        let ac = access_control.read().await;
        let current_generation = ac.generation();
        if current_generation != initial_generation {
            let decision = ac.check_access(&peer_addr.ip());
            drop(ac);
            match decision {
                AccessDecision::Allow => {}
                AccessDecision::Deny | AccessDecision::Prompt(_) => {
                    warn!(
                        "[{}] Access denied for {} on existing connection (access control changed)",
                        ctx.id_str(),
                        peer_addr.ip()
                    );
                    return Ok(error_response(
                        403,
                        "Access denied - access control policy changed",
                    ));
                }
            }
        }
    }

    if path.starts_with(ADMIN_PATH_PREFIX) {
        if let Some(state) = admin_state {
            if is_loopback && !req.uri().path().is_empty() {
                debug!(
                    "Loopback admin request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager).await,
                ));
            } else if path.starts_with(CERT_PUBLIC_PATH_PREFIX) && is_cert_public_request(&req) {
                debug!(
                    "Public cert request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager).await,
                ));
            } else if path.starts_with(&format!("{ADMIN_PATH_PREFIX}/public/")) && is_loopback {
                debug!(
                    "Public admin request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager).await,
                ));
            } else if is_valid_admin_request(&req, peer_addr, &admin_security_config) {
                debug!(
                    "Valid admin request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager).await,
                ));
            } else {
                warn!(
                    "Rejected invalid admin request from {}: {} {} (possible forgery attempt)",
                    peer_addr, method, uri
                );
                return Ok(error_response(403, "Forbidden"));
            }
        } else {
            return Ok(error_response(503, "Admin interface not enabled"));
        }
    }

    if path == "/login.html" {
        if let Some(state) = admin_state {
            if is_loopback {
                return Ok(convert_admin_response(
                    handle_sync_login_callback(req, state).await,
                ));
            }
            return Ok(error_response(403, "Forbidden"));
        }
        return Ok(error_response(503, "Admin interface not enabled"));
    }

    if is_direct_browser_access(&req, &proxy_config) && admin_state.is_some() {
        debug!(
            "Redirecting direct browser access from {} to admin UI",
            peer_addr
        );
        return Ok(redirect_response(ADMIN_PATH_PREFIX));
    }

    if let Some(ref state) = admin_state {
        state.metrics_collector.increment_requests();
    }

    if method == Method::CONNECT {
        let tls_intercept_config = connect_tls_intercept_config
            .clone()
            .unwrap_or_else(|| TlsInterceptConfig::from_proxy_config(&proxy_config));

        match handle_connect(
            req,
            peer_addr,
            local_addr,
            rules,
            tls_config,
            &tls_intercept_config,
            &proxy_config,
            verbose_logging,
            &ctx,
            admin_state.clone(),
            Some(dns_resolver),
        )
        .await
        {
            Ok(response) => {
                if verbose_logging {
                    info!(
                        "[{}] <-- CONNECT established ({}ms)",
                        ctx.id_str(),
                        ctx.elapsed_ms()
                    );
                }
                Ok(response)
            }
            Err(e) => {
                error!("[{}] CONNECT error: {}", ctx.id_str(), e);
                Ok(error_response(502, "Bad Gateway"))
            }
        }
    } else {
        let (unsafe_ssl, max_body_buffer_size) = if let Some(ref state) = admin_state {
            (
                state.runtime_config.read().await.unsafe_ssl,
                state.get_max_body_buffer_size(),
            )
        } else {
            (proxy_config.unsafe_ssl, proxy_config.max_body_buffer_size)
        };
        let max_body_probe_size = if let Some(ref state) = admin_state {
            state.get_max_body_probe_size()
        } else {
            proxy_config.max_body_probe_size
        };

        match handle_http_request(
            req,
            rules,
            verbose_logging,
            unsafe_ssl,
            max_body_buffer_size,
            max_body_probe_size,
            &ctx,
            admin_state.clone(),
            Some(dns_resolver),
        )
        .await
        {
            Ok(response) => {
                if verbose_logging {
                    info!(
                        "[{}] <-- {} ({}ms)",
                        ctx.id_str(),
                        response.status(),
                        ctx.elapsed_ms()
                    );
                }
                Ok(response)
            }
            Err(e) => {
                error!("[{}] HTTP proxy error: {}", ctx.id_str(), e);
                Ok(error_response(502, "Bad Gateway"))
            }
        }
    }
}

pub type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

pub fn empty_body() -> BoxBody {
    use http_body_util::{BodyExt, Empty};
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

pub fn full_body(data: impl Into<Bytes>) -> BoxBody {
    use http_body_util::{BodyExt, Full};
    Full::new(data.into())
        .map_err(|never| match never {})
        .boxed()
}

pub fn with_trailers(body: BoxBody, rules: &ResolvedRules) -> BoxBody {
    if rules.trailers.is_empty() {
        return body;
    }
    let mut trailers = HeaderMap::new();
    for (name, value) in &rules.trailers {
        if let (Ok(header_name), Ok(header_value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            trailers.insert(header_name, header_value);
        }
    }
    if trailers.is_empty() {
        return body;
    }
    body.with_trailers(std::future::ready(Some(Ok(trailers))))
        .boxed()
}

fn error_response(status: u16, message: &str) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .body(full_body(message.to_string()))
        .unwrap()
}

fn redirect_response(location: &str) -> Response<BoxBody> {
    Response::builder()
        .status(302)
        .header("Location", location)
        .body(empty_body())
        .unwrap()
}

fn is_direct_browser_access(req: &Request<Incoming>, config: &ProxyConfig) -> bool {
    let uri = req.uri();
    let path = uri.path();

    if path != "/" {
        return false;
    }

    if uri.scheme().is_some() || uri.host().is_some() {
        return false;
    }

    let headers = req.headers();
    let host = match headers.get("host").and_then(|h| h.to_str().ok()) {
        Some(h) => h,
        None => return false,
    };

    let host_without_port = host.split(':').next().unwrap_or(host);
    let is_local = host_without_port == "localhost"
        || host_without_port == "127.0.0.1"
        || host_without_port == config.host;

    if !is_local {
        return false;
    }

    let port = host
        .split(':')
        .nth(1)
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(80);

    if port != config.port {
        return false;
    }

    let accept = headers
        .get("accept")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    accept.contains("text/html")
}

fn convert_admin_response(
    response: Response<http_body_util::combinators::BoxBody<Bytes, hyper::Error>>,
) -> Response<BoxBody> {
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_tls::{generate_root_ca, init_crypto_provider, DynamicCertGenerator, SniResolver};

    #[test]
    fn test_proxy_config_default() {
        let config = ProxyConfig::default();
        assert_eq!(config.port, 9900);
        assert_eq!(config.host, "127.0.0.1");
        assert!(!config.enable_tls_interception);
        assert!(config.intercept_exclude.is_empty());
        assert!(config.intercept_include.is_empty());
        assert_eq!(config.timeout_secs, 30);
        assert!(config.socks5_port.is_none());
        assert!(!config.socks5_auth_required);
        assert!(config.socks5_username.is_none());
        assert!(config.socks5_password.is_none());
    }

    #[test]
    fn test_resolved_rules_default() {
        let rules = ResolvedRules::default();
        assert!(rules.host.is_none());
        assert!(rules.proxy.is_none());
        assert!(rules.req_headers.is_empty());
        assert!(rules.res_headers.is_empty());
        assert!(rules.req_body.is_none());
        assert!(rules.res_body.is_none());
        assert!(rules.status_code.is_none());
        assert!(!rules.req_cors.is_enabled());
        assert!(!rules.res_cors.is_enabled());
        assert!(rules.tls_intercept.is_none());
    }

    #[test]
    fn test_noop_rules_resolver() {
        let resolver = NoOpRulesResolver;
        let rules = resolver.resolve("http://example.com", "GET");
        assert!(rules.host.is_none());
        assert!(rules.rules.is_empty());
    }

    #[test]
    fn test_proxy_server_new() {
        let config = ProxyConfig::default();
        let server = ProxyServer::new(config.clone());
        assert_eq!(server.config().port, config.port);
        assert_eq!(server.config().host, config.host);
    }

    #[test]
    fn test_proxy_server_with_config() {
        let config = ProxyConfig {
            port: 9000,
            host: "0.0.0.0".to_string(),
            enable_tls_interception: true,
            intercept_exclude: vec!["*.example.com".to_string()],
            intercept_include: vec!["*.api.com".to_string()],
            app_intercept_exclude: vec![],
            app_intercept_include: vec![],
            timeout_secs: 60,
            http1_max_header_size: 128 * 1024,
            http2_max_header_list_size: 512 * 1024,
            websocket_handshake_max_header_size: 96 * 1024,
            socks5_port: Some(1080),
            socks5_auth_required: true,
            socks5_username: Some("user".to_string()),
            socks5_password: Some("pass".to_string()),
            verbose_logging: true,
            access_mode: AccessMode::Whitelist,
            client_whitelist: vec!["192.168.1.0/24".to_string()],
            allow_lan: true,
            unsafe_ssl: false,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            binary_traffic_performance_mode: true,
            enable_socks: true,
        };
        let server = ProxyServer::new(config);
        assert_eq!(server.config().port, 9000);
        assert_eq!(server.config().host, "0.0.0.0");
        assert!(server.config().enable_tls_interception);
        assert_eq!(server.config().socks5_port, Some(1080));
        assert!(server.config().socks5_auth_required);
        assert_eq!(server.config().http1_max_header_size, 128 * 1024);
        assert_eq!(server.config().http2_max_header_list_size, 512 * 1024);
        assert_eq!(
            server.config().websocket_handshake_max_header_size,
            96 * 1024
        );
        assert!(server.config().verbose_logging);
        assert_eq!(server.config().access_mode, AccessMode::Whitelist);
        assert!(server.config().allow_lan);
    }

    #[test]
    fn test_should_not_defer_client_process_for_connect() {
        assert!(!should_defer_client_process_resolution(
            &Method::CONNECT,
            false,
            false
        ));
    }

    #[test]
    fn test_should_not_defer_client_process_when_scripts_are_enabled() {
        assert!(!should_defer_client_process_resolution(
            &Method::GET,
            false,
            true
        ));
    }

    #[test]
    fn test_should_defer_client_process_only_for_plain_http_without_scripts() {
        assert!(should_defer_client_process_resolution(
            &Method::GET,
            false,
            false
        ));
        assert!(!should_defer_client_process_resolution(
            &Method::GET,
            true,
            false
        ));
    }

    #[test]
    fn test_empty_body() {
        use hyper::body::Body;
        let body = empty_body();
        assert!(body.is_end_stream());
    }

    #[test]
    fn test_full_body() {
        use hyper::body::Body;
        let body = full_body("test content");
        assert!(!body.is_end_stream());
    }

    #[test]
    fn test_rule_value() {
        let rule = RuleValue {
            pattern: "*.example.com".to_string(),
            protocol: Protocol::Host,
            value: "example.com".to_string(),
            options: HashMap::new(),
            rule_name: None,
            raw: None,
            line: None,
        };
        assert_eq!(rule.protocol, Protocol::Host);
        assert_eq!(rule.value, "example.com");
    }

    #[test]
    fn test_tls_config_default() {
        let config = TlsConfig::default();
        assert!(config.ca_cert.is_none());
        assert!(config.ca_key.is_none());
    }

    #[test]
    fn test_tls_config_resolve_server_config_reuses_sni_cache() {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let config = TlsConfig {
            ca_cert: Some(vec![1, 2, 3]),
            ca_key: Some(vec![4, 5, 6]),
            cert_generator: Some(Arc::new(DynamicCertGenerator::new(ca.clone()))),
            sni_resolver: Some(Arc::new(SniResolver::new(ca))),
        };

        let config1 = config
            .resolve_server_config("example.com", &alpn)
            .expect("Failed to resolve server config");
        let config2 = config
            .resolve_server_config("example.com", &alpn)
            .expect("Failed to resolve cached server config");

        assert!(Arc::ptr_eq(&config1, &config2));
        assert_eq!(config1.alpn_protocols, alpn);
    }
}
