use std::collections::HashMap;
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::FutureExt;

use regex::Regex;

use bifrost_admin::{
    handle_sync_login_callback, handle_trust_probe_proxy_configured_request,
    is_cert_public_request, is_valid_admin_request, AdminRouter, AdminSecurityConfig, AdminState,
    SharedPushManager, ADMIN_PATH_PREFIX, TRUST_PROBE_PROXY_CONFIG_HOST,
    TRUST_PROBE_PROXY_CONFIG_PATH,
};

pub(crate) const ADMIN_VIRTUAL_HOST: &str = "bifrost.local";
use bifrost_core::{BifrostError, Protocol, Result, UserPassAuthConfig};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue, HOST};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::HeaderMap;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, RwLock, Semaphore};
use tokio_rustls::rustls::ServerConfig as RustlsServerConfig;
use tracing::{debug, error, info, warn};

use socket2::{Domain, Protocol as SocketProtocol, Socket, Type};

use crate::dns::DnsResolver;
use crate::proxy::http::{
    handle_connect_with_process_state as handle_connect, handle_http_request,
    requires_client_app_for_tls_decision, SingleCertResolver,
};
use crate::proxy::socks::{SocksConfig, SocksHandler, SocksServer, UdpRelay};
use crate::unified::{DetectedProtocol, PeekableStream};
use crate::utils::logging::RequestContext;
#[cfg(test)]
use crate::utils::process_info::ClientProcess;
use crate::utils::process_info::{
    app_policy_process_resolution_retry_config, resolve_client_process_async_for_connection_shared,
    resolve_client_process_async_for_connection_with_retry_shared,
    spawn_async_process_resolver_with_finish, ConnectionProcessState, PROCESS_RESOLVER,
};
use bifrost_core::{
    AccessControlConfig, AccessDecision, AccessMode, ClientAccessControl, ProxyAuthRateLimiter,
};

const NOISY_CONN_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(5);
const TRUST_PROBE_ROUTE_PREFIX: &str = "/_bifrost/trust-probe/";
const RUNTIME_CANARY_HOST: &str = "bifrost-runtime-canary.invalid";
const RUNTIME_CANARY_PATH: &str = "/__bifrost_runtime_canary";

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

fn is_udp_relay_fallback_bind_error(err: &BifrostError) -> bool {
    match err {
        BifrostError::Io(io_err) => io_err.kind() == std::io::ErrorKind::AddrInUse,
        BifrostError::Network(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("address already in use")
                || message.contains("addrinuse")
                || message.contains("os error 48")
                || message.contains("os error 98")
                || message.contains("os error 10013")
                || message.contains("only one usage of each socket address")
        }
        _ => false,
    }
}

fn log_connection_serve_error(peer_addr: SocketAddr, err: &hyper::Error) {
    let err_debug = format!("{err:?}");
    let is_noisy_disconnect = is_noisy_connection_close(&err_debug, &err.to_string());

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
            debug!(
                "Noisy connection close from {}: {:?} (suppressed {} similar events in last {:?})",
                peer_addr, err, suppressed, NOISY_CONN_ERROR_LOG_INTERVAL
            );
        }
    }
}

fn is_noisy_connection_close(err_debug: &str, err_display: &str) -> bool {
    err_debug.contains("IncompleteMessage") || err_display.contains("message completed")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientProcessResolutionMode {
    SkipAdmin,
    SyncAppPolicy,
    Normal,
}

fn client_process_resolution_mode(
    routes_to_admin: bool,
    method: &Method,
    tls_intercept_config: Option<&TlsInterceptConfig>,
) -> ClientProcessResolutionMode {
    if routes_to_admin {
        ClientProcessResolutionMode::SkipAdmin
    } else if method == Method::CONNECT
        && tls_intercept_config.is_some_and(requires_client_app_for_tls_decision)
    {
        ClientProcessResolutionMode::SyncAppPolicy
    } else {
        ClientProcessResolutionMode::Normal
    }
}

#[derive(Debug, Clone)]
pub struct TlsInterceptConfig {
    pub enable_tls_interception: bool,
    pub intercept_exclude: Vec<String>,
    pub intercept_include: Vec<String>,
    pub app_intercept_exclude: Vec<String>,
    pub app_intercept_include: Vec<String>,
    pub ip_intercept_exclude: Vec<String>,
    pub ip_intercept_include: Vec<String>,
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
            ip_intercept_exclude: config.ip_intercept_exclude.clone(),
            ip_intercept_include: config.ip_intercept_include.clone(),
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
    pub ip_intercept_exclude: Vec<String>,
    pub ip_intercept_include: Vec<String>,
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
    pub userpass_auth: Option<UserPassAuthConfig>,
    pub userpass_last_connected_at: HashMap<String, u64>,
    pub unsafe_ssl: bool,
    pub max_body_buffer_size: usize,
    pub max_body_probe_size: usize,
    pub super_performance_mode: bool,
    pub binary_traffic_performance_mode: bool,
    pub inject_bifrost_badge: bool,
    pub enable_socks: bool,
}

pub const DEFAULT_APP_INTERCEPT_INCLUDE: &[&str] = &[
    "Google Chrome*",
    "Microsoft Edge*",
    "*Safari*",
    "*Firefox*",
    "*Opera*",
    "*Brave*",
    "*Arc*",
    "*Vivaldi*",
];

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: 9900,
            host: "127.0.0.1".to_string(),
            enable_tls_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: DEFAULT_APP_INTERCEPT_INCLUDE
                .iter()
                .map(|app| (*app).to_string())
                .collect(),
            ip_intercept_exclude: Vec::new(),
            ip_intercept_include: Vec::new(),
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
            socks5_port: None,
            socks5_auth_required: false,
            socks5_username: None,
            socks5_password: None,
            verbose_logging: false,
            access_mode: AccessMode::Interactive,
            client_whitelist: Vec::new(),
            allow_lan: false,
            userpass_auth: None,
            userpass_last_connected_at: HashMap::new(),
            unsafe_ssl: false,
            max_body_buffer_size: 10 * 1024 * 1024, // 10MB
            max_body_probe_size: 64 * 1024,
            super_performance_mode: false,
            binary_traffic_performance_mode: true,
            inject_bifrost_badge: true,
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
    pub auto_tls_intercept: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DevtoolsMode {
    Read,
    #[default]
    Control,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DevtoolsInjectMode {
    #[default]
    Auto,
    Bridge,
    Off,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevtoolsRule {
    pub mode: DevtoolsMode,
    pub inject: DevtoolsInjectMode,
    pub deny: bool,
    pub evaluate_allowlist: Vec<String>,
    pub raw_value: String,
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
    pub upstream_unsafe_ssl: bool,
    pub sni_callback: Option<String>,

    pub req_scripts: Vec<String>,
    pub res_scripts: Vec<String>,
    pub res_stream_scripts: Vec<String>,
    pub decode_scripts: Vec<String>,
    pub bp_scripts: Vec<String>,

    pub auth: Option<String>,
    pub forwarded_for: Option<String>,
    pub response_for: Option<String>,
    pub delete_req_headers: Vec<String>,
    pub delete_res_headers: Vec<String>,
    pub delete_url_params: Vec<String>,
    pub header_replace: Vec<HeaderReplaceRule>,

    pub values: std::collections::HashMap<String, String>,

    pub trailers: Vec<(String, String)>,

    pub devtools: Option<DevtoolsRule>,
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

    /// Re-resolve once the upstream response is available, so response-dependent
    /// filters (`includeFilter://s:NNN`, `includeFilter://resH:...`) can be evaluated
    /// against the real response. The default implementation ignores the response data
    /// and falls back to request-phase resolution.
    fn resolve_with_response_context(
        &self,
        url: &str,
        method: &str,
        req_headers: &std::collections::HashMap<String, String>,
        req_cookies: &std::collections::HashMap<String, String>,
        _res_status: u16,
        _res_headers: &std::collections::HashMap<String, String>,
    ) -> ResolvedRules {
        self.resolve_with_context(url, method, req_headers, req_cookies)
    }

    fn has_response_rules_for_host(&self, _host: &str) -> bool {
        false
    }

    fn has_breakpoint_rules_for_host(&self, _host: &str) -> bool {
        false
    }

    fn has_tls_auto_intercept_route_rules_for_host(&self, _host: &str) -> bool {
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
    proxy_auth_rate_limiter: Arc<ProxyAuthRateLimiter>,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig) -> Self {
        let admin_security_config = AdminSecurityConfig::new(config.port);
        let access_config = AccessControlConfig {
            mode: config.access_mode,
            whitelist: config.client_whitelist.clone(),
            allow_lan: config.allow_lan,
            userpass: config.userpass_auth.clone(),
        };
        let access_control = ClientAccessControl::new(access_config);
        access_control.set_userpass_last_connected_at(config.userpass_last_connected_at.clone());
        let access_control = Arc::new(RwLock::new(access_control));
        let dns_resolver = Arc::new(DnsResolver::new(config.verbose_logging));
        Self {
            config,
            rules: Arc::new(NoOpRulesResolver),
            tls_config: Arc::new(TlsConfig::default()),
            admin_state: None,
            push_manager: None,
            admin_security_config,
            access_control,
            dns_resolver,
            udp_relay_addr: Arc::new(RwLock::new(None)),
            udp_relay: Arc::new(RwLock::new(None)),
            proxy_auth_rate_limiter: Arc::new(ProxyAuthRateLimiter::new()),
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
        admin_state.set_process_resolver_diagnostics(PROCESS_RESOLVER.diagnostics());
        self.admin_state = Some(Arc::new(admin_state));
        self
    }

    pub fn with_admin_state_shared(mut self, admin_state: Arc<AdminState>) -> Self {
        admin_state.set_process_resolver_diagnostics(PROCESS_RESOLVER.diagnostics());
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

        // IMPORTANT:
        // We want the proxy to be restart-friendly (especially for E2E retries).
        // Binding without SO_REUSEADDR can fail on Linux when the previous process
        // recently had many connections and the port is stuck in TIME_WAIT.
        let domain = match addr {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };
        let socket = Socket::new(domain, Type::STREAM, Some(SocketProtocol::TCP))
            .map_err(|e| BifrostError::Network(format!("Failed to create listen socket: {}", e)))?;
        // NOTE:
        // - On Unix, SO_REUSEADDR is needed to make quick restarts reliable (E2E retries)
        //   when the port is stuck behind TIME_WAIT.
        // - On Windows, SO_REUSEADDR can enable port hijacking semantics; keep the default
        //   exclusive bind behavior for safety.
        #[cfg(unix)]
        socket
            .set_reuse_address(true)
            .map_err(|e| BifrostError::Network(format!("Failed to set SO_REUSEADDR: {}", e)))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| BifrostError::Network(format!("Failed to set nonblocking: {}", e)))?;
        socket
            .bind(&addr.into())
            .map_err(|e| BifrostError::Network(format!("Failed to bind to {}: {}", addr, e)))?;
        // Backlog doesn't need to be huge here; we also cap active connections separately.
        socket
            .listen(1024)
            .map_err(|e| BifrostError::Network(format!("Failed to listen on {}: {}", addr, e)))?;

        let std_listener: std::net::TcpListener = socket.into();
        std_listener
            .set_nonblocking(true)
            .map_err(|e| BifrostError::Network(format!("Failed to set nonblocking: {}", e)))?;
        TcpListener::from_std(std_listener)
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
        self.run_with_listener_ready(listener, None).await
    }

    pub async fn run_with_listener_ready(
        &self,
        listener: TcpListener,
        ready_tx: Option<oneshot::Sender<()>>,
    ) -> Result<()> {
        let addr = listener
            .local_addr()
            .map_err(|e| BifrostError::Network(format!("Failed to get listener address: {}", e)))?;

        if self.config.enable_socks {
            let udp_addr = SocketAddr::new(addr.ip(), addr.port());
            let mut udp_relay = UdpRelay::new(udp_addr)
                .with_rules(Arc::clone(&self.rules))
                .with_access_control(Arc::clone(&self.access_control));
            let udp_relay_started_addr = match udp_relay.start().await {
                Ok(started_addr) => started_addr,
                Err(error) if is_udp_relay_fallback_bind_error(&error) => {
                    let fallback_addr = SocketAddr::new(addr.ip(), 0);
                    warn!(
                        "UDP relay failed to bind on {}; retrying with an ephemeral port: {}",
                        udp_addr, error
                    );
                    udp_relay = UdpRelay::new(fallback_addr)
                        .with_rules(Arc::clone(&self.rules))
                        .with_access_control(Arc::clone(&self.access_control));
                    udp_relay.start().await?
                }
                Err(error) => return Err(error),
            };
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

        if let Some(ready_tx) = ready_tx {
            let _ = ready_tx.send(());
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
                .with_inject_bifrost_badge(self.config.inject_bifrost_badge)
                .with_dns_resolver(Arc::clone(&self.dns_resolver))
                .with_proxy_auth_rate_limiter(Arc::clone(&self.proxy_auth_rate_limiter))
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

            let (decision, should_defer_userpass) = {
                let access_control = self.access_control.read().await;
                let decision = access_control.check_access(&peer_addr.ip());
                let should_defer_userpass = access_control.should_defer_userpass(&decision);
                (decision, should_defer_userpass)
            };

            let allow_remote_admin_bypass = self
                .admin_state
                .as_ref()
                .map(|s| bifrost_admin::is_remote_access_enabled(s))
                .unwrap_or(false);

            match decision {
                AccessDecision::Allow => {}
                AccessDecision::Deny => {
                    if should_defer_userpass || allow_remote_admin_bypass {
                        debug!(
                            "Deferring access denial for {} (userpass={}, remote_admin={})",
                            peer_addr.ip(),
                            should_defer_userpass,
                            allow_remote_admin_bypass
                        );
                    } else {
                        debug!(
                            "Deferring access denial for {} until the HTTP request path is known",
                            peer_addr.ip()
                        );
                    }
                }
                AccessDecision::Prompt(ip) => {
                    if should_defer_userpass || allow_remote_admin_bypass {
                        debug!(
                            "Deferring interactive authorization for {} (userpass={}, remote_admin={})",
                            ip,
                            should_defer_userpass,
                            allow_remote_admin_bypass
                        );
                    } else {
                        debug!(
                            "Deferring interactive authorization for {} until the HTTP request path is known",
                            ip
                        );
                    }
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
            let proxy_auth_rate_limiter = Arc::clone(&self.proxy_auth_rate_limiter);

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
                                .with_access_control(Arc::clone(&access_control))
                                .with_dns_resolver(Arc::clone(&dns_resolver))
                                .with_proxy_auth_rate_limiter(Arc::clone(&proxy_auth_rate_limiter))
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
                            Arc::clone(&proxy_auth_rate_limiter),
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
                            proxy_auth_rate_limiter,
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
    proxy_auth_rate_limiter: Arc<ProxyAuthRateLimiter>,
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
    let client_process_state = Arc::new(ConnectionProcessState::default());

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
        let client_process_state = Arc::clone(&client_process_state);
        let proxy_auth_rate_limiter = Arc::clone(&proxy_auth_rate_limiter);
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
                client_process_state,
                proxy_auth_rate_limiter,
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
    mut req: Request<Incoming>,
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
    client_process_state: Arc<ConnectionProcessState>,
    proxy_auth_rate_limiter: Arc<ProxyAuthRateLimiter>,
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path();
    let verbose_logging = proxy_config.verbose_logging;
    let is_local_client = peer_addr.ip().is_loopback();
    let admin_routing = admin_routing_decision(&req, proxy_config.port, &proxy_config.host);

    if is_runtime_data_plane_canary(&req) {
        return Ok(if is_local_client {
            Response::builder()
                .status(hyper::StatusCode::NO_CONTENT)
                .header("cache-control", "no-store")
                .body(empty_body())
                .expect("static runtime canary response must be valid")
        } else {
            error_response(403, "Runtime canary is loopback-only")
        });
    }

    let connect_tls_intercept_config = if method == Method::CONNECT {
        Some(if let Some(ref state) = admin_state {
            let runtime_config = state.runtime_config.read().await;
            TlsInterceptConfig {
                enable_tls_interception: runtime_config.enable_tls_interception,
                intercept_exclude: runtime_config.intercept_exclude.clone(),
                intercept_include: runtime_config.intercept_include.clone(),
                app_intercept_exclude: runtime_config.app_intercept_exclude.clone(),
                app_intercept_include: runtime_config.app_intercept_include.clone(),
                ip_intercept_exclude: runtime_config.ip_intercept_exclude.clone(),
                ip_intercept_include: runtime_config.ip_intercept_include.clone(),
                unsafe_ssl: runtime_config.unsafe_ssl,
            }
        } else {
            TlsInterceptConfig::from_proxy_config(&proxy_config)
        })
    } else {
        None
    };
    let process_resolution_mode = client_process_resolution_mode(
        admin_routing.routes_to_admin,
        &method,
        connect_tls_intercept_config.as_ref(),
    );
    let mut ctx = RequestContext::new()
        .with_client_ip(peer_addr.ip().to_string())
        .with_port(proxy_config.port);
    let cached_client_process = if process_resolution_mode == ClientProcessResolutionMode::SkipAdmin
    {
        None
    } else {
        client_process_state.cached()
    };

    let client_process = if process_resolution_mode == ClientProcessResolutionMode::SkipAdmin {
        None
    } else if let Some(client_process) = cached_client_process {
        Some(client_process)
    } else if connect_tls_intercept_config.is_some() {
        let client_process = client_process_state
            .resolve(|| async {
                if is_local_client
                    && process_resolution_mode == ClientProcessResolutionMode::SyncAppPolicy
                {
                    let (max_retries, delay_ms) = app_policy_process_resolution_retry_config();
                    debug!(
                        req_id = ctx.id_str(),
                        peer_addr = %peer_addr,
                        local_addr = %local_addr,
                        max_retries,
                        delay_ms,
                        "CONNECT request requires synchronous client process resolution for app policy"
                    );
                    resolve_client_process_async_for_connection_with_retry_shared(
                        &peer_addr,
                        &local_addr,
                        max_retries,
                        delay_ms,
                    )
                    .await
                } else {
                    resolve_client_process_async_for_connection_shared(&peer_addr, &local_addr)
                        .await
                }
            })
            .await;
        if is_local_client && process_resolution_mode == ClientProcessResolutionMode::SyncAppPolicy
        {
            match client_process.as_ref() {
                Some(process) => debug!(
                    req_id = ctx.id_str(),
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    client_app = %process.name,
                    client_pid = process.pid,
                    "CONNECT synchronous client process resolution succeeded"
                ),
                None => debug!(
                    req_id = ctx.id_str(),
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    "CONNECT synchronous client process resolution returned unknown client"
                ),
            }
        }
        client_process
    } else {
        let client_process = client_process_state
            .resolve(|| async {
                resolve_client_process_async_for_connection_shared(&peer_addr, &local_addr).await
            })
            .await;
        let client_process = if client_process.is_some() {
            client_process
        } else if is_local_client {
            let mut resolved = None;
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let Some(process) = client_process_state.cached() {
                    resolved = Some(process);
                    break;
                }
            }
            if resolved.is_none() {
                if let Some(state) = admin_state.clone() {
                    let should_spawn = client_process_state.try_start_background_resolution();
                    if should_spawn {
                        let record_id = ctx.id_str();
                        let state_for_success = Arc::clone(&client_process_state);
                        let state_for_finish = Arc::clone(&client_process_state);
                        spawn_async_process_resolver_with_finish(
                            peer_addr,
                            local_addr,
                            record_id.to_string(),
                            move |id, process| {
                                let process = state_for_success.store(Arc::new(process));
                                state.update_client_process(
                                    &id,
                                    process.name.clone(),
                                    process.pid,
                                    process.path.clone(),
                                );
                            },
                            move || {
                                state_for_finish.finish_background_resolution();
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

    if is_trust_probe_proxy_configured_request(&req) {
        return Ok(handle_trust_probe_proxy_configured_request(
            req,
            peer_addr,
            push_manager.as_ref(),
        )
        .await);
    }

    if is_proxy_request_to_trust_probe_direct_target(&req) {
        warn!(
            "[{}] Rejected proxy-routed availability probe from {}: {} {}; trust probe must connect directly to the probe port",
            ctx.id_str(),
            peer_addr,
            method,
            uri
        );
        return Ok(trust_probe_proxy_bypass_required_response(&method));
    }

    let is_public_cert_path = is_cert_public_request(&req);
    let is_loopback = peer_addr.ip().is_loopback();
    if !is_public_cert_path && !is_loopback {
        let ac = access_control.read().await;
        let current_generation = ac.generation();
        if current_generation != initial_generation {
            let decision = ac.check_access(&peer_addr.ip());
            let should_defer_userpass = ac.should_defer_userpass(&decision);
            drop(ac);

            let allow_remote_admin_bypass = admin_state
                .as_ref()
                .map(|s| bifrost_admin::is_remote_access_enabled(s))
                .unwrap_or(false);

            match decision {
                AccessDecision::Allow => {}
                AccessDecision::Prompt(_) => {
                    if !should_defer_userpass && !allow_remote_admin_bypass {
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
                AccessDecision::Deny => {
                    if should_defer_userpass || allow_remote_admin_bypass {
                        debug!(
                            "[{}] Deferring access policy re-check for {} to HTTP userpass auth",
                            ctx.id_str(),
                            peer_addr.ip()
                        );
                    } else {
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
    }

    let is_admin_virtual_host = admin_routing.is_admin_virtual_host;
    let is_proxy_request_to_other_server = admin_routing.is_proxy_request_to_other_server;

    if is_devtools_bridge_admin_path(path) {
        if let Some(state) = admin_state {
            if let Ok(value) = hyper::header::HeaderValue::from_str(&peer_addr.ip().to_string()) {
                req.headers_mut().insert(
                    hyper::header::HeaderName::from_static("x-bifrost-peer-ip"),
                    value,
                );
            }
            debug!(
                "Routing page bridge request from {}: {} {}",
                peer_addr, method, path
            );
            return Ok(convert_admin_response(
                AdminRouter::handle(req, state, push_manager, Some(peer_addr)).await,
            ));
        }
        return Ok(error_response(503, "Admin interface not enabled"));
    }

    if path.starts_with(ADMIN_PATH_PREFIX) && !is_proxy_request_to_other_server {
        if let Some(state) = admin_state {
            if let Ok(value) = hyper::header::HeaderValue::from_str(&peer_addr.ip().to_string()) {
                req.headers_mut().insert(
                    hyper::header::HeaderName::from_static("x-bifrost-peer-ip"),
                    value,
                );
            }
            let allow_remote_admin = bifrost_admin::is_remote_access_enabled(&state);
            if is_admin_virtual_host {
                if is_loopback {
                    debug!(
                        "Serving admin UI asset/API via {} from {}: {} {}",
                        ADMIN_VIRTUAL_HOST, peer_addr, method, path
                    );
                    let req = rewrite_virtual_host_request(req);
                    return Ok(convert_admin_response(
                        AdminRouter::handle(req, state, push_manager, Some(peer_addr)).await,
                    ));
                }
                return Ok(error_response(403, "Forbidden"));
            } else if is_cert_public_request(&req) {
                debug!(
                    "Public cert request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager, Some(peer_addr)).await,
                ));
            } else if path.starts_with(&format!("{ADMIN_PATH_PREFIX}/public/"))
                && (is_loopback || allow_remote_admin)
            {
                debug!(
                    "Public admin request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager, Some(peer_addr)).await,
                ));
            } else if is_valid_admin_request(
                &req,
                peer_addr,
                &admin_security_config,
                allow_remote_admin,
            ) {
                debug!(
                    "Valid admin request from {}: {} {}",
                    peer_addr, method, path
                );
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager, Some(peer_addr)).await,
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

    if path == "/login.html" && !is_proxy_request_to_other_server {
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

    if is_admin_virtual_host && !is_proxy_request_to_other_server {
        if let Some(state) = admin_state {
            if is_loopback {
                debug!(
                    "Serving admin UI via {} from {}: {} {}",
                    ADMIN_VIRTUAL_HOST, peer_addr, method, path
                );
                let req = rewrite_virtual_host_request(req);
                return Ok(convert_admin_response(
                    AdminRouter::handle(req, state, push_manager, Some(peer_addr)).await,
                ));
            }
            return Ok(error_response(403, "Forbidden"));
        }
        return Ok(error_response(503, "Admin interface not enabled"));
    }

    let allow_remote_admin = admin_state
        .as_ref()
        .map(|s| bifrost_admin::is_remote_access_enabled(s))
        .unwrap_or(false);

    if is_direct_browser_access(&req, &proxy_config, allow_remote_admin) && admin_state.is_some() {
        debug!(
            "Redirecting direct browser access from {} to admin UI",
            peer_addr
        );
        return Ok(redirect_response(ADMIN_PATH_PREFIX));
    }

    let has_userpass = {
        let ac = access_control.read().await;
        ac.has_userpass_auth()
    };

    if !is_loopback || has_userpass {
        match authorize_proxy_request(
            &req,
            peer_addr,
            &access_control,
            admin_state.as_ref(),
            &ctx,
            &proxy_auth_rate_limiter,
        )
        .await
        {
            Ok(account_name) => {
                if account_name.is_some() {
                    ctx = ctx.with_account_name(account_name);
                }
            }
            Err(response) => return Ok(response),
        }
    }
    if has_userpass || !is_loopback {
        req.headers_mut().remove("proxy-authorization");
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
            push_manager.clone(),
            Arc::clone(&client_process_state),
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
        let (unsafe_ssl, max_body_buffer_size, inject_bifrost_badge) =
            if let Some(ref state) = admin_state {
                let unsafe_ssl = state.runtime_config.read().await.unsafe_ssl;
                let inject_bifrost_badge = state
                    .config_manager
                    .as_ref()
                    .and_then(|cm| cm.try_config())
                    .map(|config| config.traffic.inject_bifrost_badge)
                    .unwrap_or(proxy_config.inject_bifrost_badge);

                (
                    unsafe_ssl,
                    state.get_max_body_buffer_size(),
                    inject_bifrost_badge,
                )
            } else {
                (
                    proxy_config.unsafe_ssl,
                    proxy_config.max_body_buffer_size,
                    proxy_config.inject_bifrost_badge,
                )
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
            inject_bifrost_badge,
            &ctx,
            admin_state.clone(),
            push_manager.clone(),
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

enum ProxyAuthResult {
    Authorized(Option<String>),
    Unauthorized,
}

async fn authorize_proxy_request(
    req: &Request<Incoming>,
    peer_addr: SocketAddr,
    access_control: &Arc<tokio::sync::RwLock<ClientAccessControl>>,
    admin_state: Option<&Arc<AdminState>>,
    ctx: &RequestContext,
    rate_limiter: &Arc<ProxyAuthRateLimiter>,
) -> std::result::Result<Option<String>, Response<BoxBody>> {
    let (decision, has_userpass, loopback_requires_auth) = {
        let access_control = access_control.read().await;
        let decision = access_control.check_access(&peer_addr.ip());
        let has_userpass = access_control.has_userpass_auth();
        let loopback_requires_auth = access_control.loopback_requires_auth();
        (decision, has_userpass, loopback_requires_auth)
    };

    if matches!(decision, AccessDecision::Allow) && !loopback_requires_auth {
        return Ok(None);
    }

    if has_userpass {
        if rate_limiter.is_banned(&peer_addr.ip()) {
            warn!(
                "[{}] Proxy auth rejected for {} — IP is temporarily banned",
                ctx.id_str(),
                peer_addr.ip()
            );
            return Err(proxy_auth_banned_response());
        }

        match authenticate_proxy_credentials(req.headers(), access_control, admin_state, ctx).await
        {
            ProxyAuthResult::Authorized(account_name) => {
                rate_limiter.record_success(&peer_addr.ip());
                return Ok(account_name);
            }
            ProxyAuthResult::Unauthorized => {
                let failures = rate_limiter.record_failure(peer_addr.ip());
                warn!(
                    "[{}] Proxy auth failed for {} (attempt #{})",
                    ctx.id_str(),
                    peer_addr.ip(),
                    failures
                );
                return Err(proxy_auth_required_response());
            }
        }
    }

    if matches!(decision, AccessDecision::Allow) {
        return Ok(None);
    }

    if matches!(decision, AccessDecision::Prompt(_)) {
        let access_control = access_control.read().await;
        access_control.add_pending_authorization(peer_addr.ip());
    }

    Err(proxy_auth_required_response())
}

async fn authenticate_proxy_credentials(
    headers: &HeaderMap<HeaderValue>,
    access_control: &Arc<tokio::sync::RwLock<ClientAccessControl>>,
    admin_state: Option<&Arc<AdminState>>,
    ctx: &RequestContext,
) -> ProxyAuthResult {
    let Some(encoded) = headers
        .get("proxy-authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_proxy_basic_auth_header)
    else {
        return ProxyAuthResult::Unauthorized;
    };

    let decoded = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(decoded) => decoded,
        Err(_) => return ProxyAuthResult::Unauthorized,
    };
    let decoded = match String::from_utf8(decoded) {
        Ok(decoded) => decoded,
        Err(_) => return ProxyAuthResult::Unauthorized,
    };
    let Some((username, password)) = decoded.split_once(':') else {
        return ProxyAuthResult::Unauthorized;
    };

    let matched_username = {
        let access_control = access_control.read().await;
        access_control.verify_userpass(username, password)
    };

    let Some(matched_username) = matched_username else {
        return ProxyAuthResult::Unauthorized;
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    {
        let access_control = access_control.read().await;
        access_control.record_userpass_success(&matched_username, timestamp);
    }
    if let Some(admin_state) = admin_state {
        if let Some(config_manager) = admin_state.config_manager.as_ref() {
            if let Err(error) = config_manager
                .record_userpass_last_connected_at(&matched_username, timestamp)
                .await
            {
                warn!(
                    "[{}] Failed to persist userpass auth timestamp for {}: {}",
                    ctx.id_str(),
                    matched_username,
                    error
                );
            }
        }
    }

    ProxyAuthResult::Authorized(Some(matched_username))
}

fn parse_proxy_basic_auth_header(header: &str) -> Option<&str> {
    let (scheme, encoded) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    Some(encoded)
}

fn proxy_auth_required_response() -> Response<BoxBody> {
    Response::builder()
        .status(407)
        .header("Proxy-Authenticate", "Basic realm=\"Bifrost\"")
        .body(full_body(
            "Proxy authentication required. Provide valid proxy username and password.".to_string(),
        ))
        .unwrap()
}

fn proxy_auth_banned_response() -> Response<BoxBody> {
    Response::builder()
        .status(429)
        .header("Retry-After", "300")
        .body(full_body(
            "Too many failed authentication attempts. Your IP has been temporarily banned."
                .to_string(),
        ))
        .unwrap()
}

fn redirect_response(location: &str) -> Response<BoxBody> {
    Response::builder()
        .status(302)
        .header("Location", location)
        .body(empty_body())
        .unwrap()
}

fn is_admin_virtual_host_request<B>(req: &Request<B>) -> bool {
    if req.method() == hyper::Method::CONNECT {
        return false;
    }

    if let Some(uri_host) = req.uri().host() {
        if uri_host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
            return true;
        }
    }

    if let Some(host_val) = req.headers().get("host").and_then(|h| h.to_str().ok()) {
        let host_without_port = host_val.split(':').next().unwrap_or(host_val);
        if host_without_port.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
            return true;
        }
    }

    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdminRoutingDecision {
    is_admin_virtual_host: bool,
    is_proxy_request_to_other_server: bool,
    routes_to_admin: bool,
}

fn admin_routing_decision<B>(
    req: &Request<B>,
    self_port: u16,
    self_host: &str,
) -> AdminRoutingDecision {
    let is_admin_virtual_host = is_admin_virtual_host_request(req);
    let is_proxy_request_to_other_server =
        is_proxy_request_to_other_for_admin_routing(req, self_port, self_host);
    let path = req.uri().path();
    let routes_to_admin = is_devtools_bridge_admin_path(path)
        || (!is_proxy_request_to_other_server
            && (path.starts_with(ADMIN_PATH_PREFIX) || is_admin_virtual_host));

    AdminRoutingDecision {
        is_admin_virtual_host,
        is_proxy_request_to_other_server,
        routes_to_admin,
    }
}

fn is_proxy_request_to_other_for_admin_routing<B>(
    req: &Request<B>,
    self_port: u16,
    self_host: &str,
) -> bool {
    if is_admin_virtual_host_request(req) {
        return false;
    }
    is_proxy_request_targeting_other(req, self_port, self_host)
}

fn is_proxy_request_targeting_other<B>(req: &Request<B>, self_port: u16, self_host: &str) -> bool {
    let uri = req.uri();
    if uri.scheme().is_none() && uri.host().is_none() {
        return false;
    }

    let target_host = match uri.host() {
        Some(h) => h,
        None => return false,
    };
    let target_port = uri.port_u16().unwrap_or(80);

    if target_port != self_port {
        return true;
    }

    let is_loopback_target =
        target_host == "127.0.0.1" || target_host == "localhost" || target_host == "[::1]";
    let self_is_loopback =
        self_host == "127.0.0.1" || self_host == "localhost" || self_host == "[::1]";
    let self_is_wildcard = self_host == "0.0.0.0" || self_host == "[::]";

    if target_host == self_host {
        return false;
    }
    if is_loopback_target && (self_is_loopback || self_is_wildcard) {
        return false;
    }
    if self_is_wildcard {
        return false;
    }

    true
}

fn is_devtools_bridge_admin_path(path: &str) -> bool {
    path.strip_prefix(ADMIN_PATH_PREFIX)
        .is_some_and(|rest| rest.starts_with("/api/devtools/bridge/"))
}

fn is_trust_probe_proxy_configured_request<B>(req: &Request<B>) -> bool {
    let uri = req.uri();
    let absolute_form_match = uri.scheme_str() == Some("http")
        && uri.host() == Some(TRUST_PROBE_PROXY_CONFIG_HOST)
        && uri.path() == TRUST_PROBE_PROXY_CONFIG_PATH;
    if absolute_form_match {
        return true;
    }
    uri.scheme_str().is_none()
        && uri.path() == TRUST_PROBE_PROXY_CONFIG_PATH
        && request_host_matches(req, TRUST_PROBE_PROXY_CONFIG_HOST)
}

fn is_runtime_data_plane_canary<B>(req: &Request<B>) -> bool {
    if req.method() != Method::GET || req.uri().path() != RUNTIME_CANARY_PATH {
        return false;
    }
    let uri = req.uri();
    (uri.scheme_str() == Some("http")
        && uri
            .host()
            .is_some_and(|host| host.eq_ignore_ascii_case(RUNTIME_CANARY_HOST)))
        || (uri.scheme_str().is_none() && request_host_matches(req, RUNTIME_CANARY_HOST))
}

fn request_host_matches<B>(req: &Request<B>, expected_host: &str) -> bool {
    req.headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next())
        .is_some_and(|host| host.eq_ignore_ascii_case(expected_host))
}

fn proxy_request_trust_probe_target<B>(req: &Request<B>) -> Option<(String, u16)> {
    let uri = req.uri();
    let scheme = uri.scheme_str()?;
    let host = uri.host()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    if !uri.path().starts_with(TRUST_PROBE_ROUTE_PREFIX) {
        return None;
    }
    if scheme == "http" && host == TRUST_PROBE_PROXY_CONFIG_HOST {
        return None;
    }

    let default_port = if scheme == "https" { 443 } else { 80 };
    Some((host.to_string(), uri.port_u16().unwrap_or(default_port)))
}

fn is_proxy_request_to_trust_probe_direct_target<B>(req: &Request<B>) -> bool {
    proxy_request_trust_probe_target(req).is_some()
}

fn trust_probe_proxy_bypass_required_response(method: &Method) -> Response<BoxBody> {
    let mut builder = Response::builder()
        .status(if *method == Method::OPTIONS { 204 } else { 409 })
        .header("access-control-allow-origin", "*")
        .header("access-control-allow-methods", "GET, OPTIONS")
        .header("access-control-allow-headers", "content-type");

    if *method != Method::OPTIONS {
        builder = builder.header("content-type", "application/json");
    }

    builder
        .body(if *method == Method::OPTIONS {
            empty_body()
        } else {
            full_body(
                r#"{"error":"trust_probe_must_bypass_proxy","message":"Availability trust probe must connect directly to the probe port; only bifrost-proxy-check.invalid is allowed through the configured proxy."}"#,
            )
        })
        .unwrap()
}

fn rewrite_virtual_host_request(req: Request<Incoming>) -> Request<Incoming> {
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

fn is_direct_browser_access<B>(req: &Request<B>, config: &ProxyConfig, allow_remote: bool) -> bool {
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

    if !is_local && !allow_remote {
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
    use hyper::header::{CONTENT_TYPE, LOCATION};
    use hyper::StatusCode;

    #[derive(Clone)]
    struct StaticStatusRules;

    impl RulesResolver for StaticStatusRules {
        fn resolve_with_context(
            &self,
            _url: &str,
            _method: &str,
            _req_headers: &HashMap<String, String>,
            _req_cookies: &HashMap<String, String>,
        ) -> ResolvedRules {
            ResolvedRules {
                status_code: Some(204),
                ..Default::default()
            }
        }
    }

    #[test]
    fn test_incomplete_message_is_treated_as_noisy_connection_close() {
        assert!(is_noisy_connection_close(
            "hyper::Error(IncompleteMessage)",
            "connection closed before message completed"
        ));
        assert!(!is_noisy_connection_close(
            "hyper::Error(Parse(Version))",
            "invalid HTTP version"
        ));
    }

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
    fn test_proxy_config_default_app_intercept_include_excludes_codex() {
        let config = ProxyConfig::default();
        assert_eq!(config.app_intercept_include, DEFAULT_APP_INTERCEPT_INCLUDE);
        assert!(!config
            .app_intercept_include
            .iter()
            .any(|app| app.to_ascii_lowercase().contains("codex")));
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
            ip_intercept_exclude: vec![],
            ip_intercept_include: vec![],
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
            super_performance_mode: false,
            binary_traffic_performance_mode: true,
            inject_bifrost_badge: true,
            enable_socks: true,
            userpass_auth: None,
            userpass_last_connected_at: HashMap::new(),
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
    fn test_client_process_resolution_mode_uses_admin_routing_decision() {
        assert_eq!(
            client_process_resolution_mode(true, &Method::GET, None),
            ClientProcessResolutionMode::SkipAdmin
        );
        assert_eq!(
            client_process_resolution_mode(false, &Method::GET, None),
            ClientProcessResolutionMode::Normal
        );
    }

    #[tokio::test]
    async fn connection_process_state_shares_one_successful_resolution() {
        let state = Arc::new(ConnectionProcessState::default());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(12));
        let tasks = (0..12)
            .map(|_| {
                let state = Arc::clone(&state);
                let calls = Arc::clone(&calls);
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await;
                    state
                        .resolve(|| async {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            Some(Arc::new(ClientProcess {
                                pid: 42,
                                name: "SharedBrowser".to_string(),
                                path: Some("/Applications/SharedBrowser.app".to_string()),
                            }))
                        })
                        .await
                })
            })
            .collect::<Vec<_>>();

        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap().pid, 42);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(state.cached().unwrap().name, "SharedBrowser");
    }

    #[tokio::test]
    async fn connection_process_state_does_not_cache_unknown() {
        let state = ConnectionProcessState::default();
        assert!(state.resolve(|| async { None }).await.is_none());
        let resolved = state
            .resolve(|| async {
                Some(Arc::new(ClientProcess {
                    pid: 84,
                    name: "LateBrowser".to_string(),
                    path: None,
                }))
            })
            .await
            .unwrap();

        assert_eq!(resolved.pid, 84);
        assert_eq!(state.cached().unwrap().pid, 84);
    }

    #[tokio::test]
    async fn connection_process_state_uses_background_result_without_reinitializing() {
        let state = ConnectionProcessState::default();
        state.store(Arc::new(ClientProcess {
            pid: 126,
            name: "BackgroundBrowser".to_string(),
            path: None,
        }));

        let resolved = state
            .resolve(|| async { panic!("cached background result must bypass resolver") })
            .await
            .unwrap();
        assert_eq!(resolved.pid, 126);
    }

    #[tokio::test]
    async fn connection_process_state_waits_for_inflight_background_resolution() {
        let state = Arc::new(ConnectionProcessState::default());
        assert!(state.try_start_background_resolution());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let state_for_waiter = Arc::clone(&state);
        let calls_for_waiter = Arc::clone(&calls);
        let waiter = tokio::spawn(async move {
            state_for_waiter
                .resolve(|| async move {
                    calls_for_waiter.fetch_add(1, Ordering::SeqCst);
                    None
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        state.store(Arc::new(ClientProcess {
            pid: 168,
            name: "BackgroundWinner".to_string(),
            path: None,
        }));
        state.finish_background_resolution();

        assert_eq!(waiter.await.unwrap().unwrap().pid, 168);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn connection_process_state_releases_inflight_after_cancellation() {
        let state = Arc::new(ConnectionProcessState::default());
        let state_for_cancelled = Arc::clone(&state);
        let cancelled = tokio::spawn(async move {
            state_for_cancelled
                .resolve(|| async { std::future::pending::<Option<Arc<ClientProcess>>>().await })
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancelled.abort();
        let _ = cancelled.await;

        let resolved = state
            .resolve(|| async {
                Some(Arc::new(ClientProcess {
                    pid: 210,
                    name: "RecoveredBrowser".to_string(),
                    path: None,
                }))
            })
            .await
            .unwrap();
        assert_eq!(resolved.pid, 210);
    }

    #[test]
    fn test_client_process_resolution_mode_keeps_connect_app_policy_synchronous() {
        let config = TlsInterceptConfig {
            enable_tls_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: vec!["*Chrome*".to_string()],
            ip_intercept_exclude: Vec::new(),
            ip_intercept_include: Vec::new(),
            unsafe_ssl: false,
        };
        assert_eq!(
            client_process_resolution_mode(false, &Method::CONNECT, Some(&config)),
            ClientProcessResolutionMode::SyncAppPolicy
        );
        assert_eq!(
            client_process_resolution_mode(false, &Method::GET, Some(&config)),
            ClientProcessResolutionMode::Normal
        );
    }

    #[test]
    fn runtime_data_plane_canary_accepts_only_exact_get_target() {
        let absolute = Request::builder()
            .method(Method::GET)
            .uri(format!("http://{RUNTIME_CANARY_HOST}{RUNTIME_CANARY_PATH}"))
            .body(())
            .unwrap();
        assert!(is_runtime_data_plane_canary(&absolute));

        let origin = Request::builder()
            .method(Method::GET)
            .uri(RUNTIME_CANARY_PATH)
            .header(HOST, format!("{RUNTIME_CANARY_HOST}:80"))
            .body(())
            .unwrap();
        assert!(is_runtime_data_plane_canary(&origin));

        let wrong_method = Request::builder()
            .method(Method::POST)
            .uri(format!("http://{RUNTIME_CANARY_HOST}{RUNTIME_CANARY_PATH}"))
            .body(())
            .unwrap();
        assert!(!is_runtime_data_plane_canary(&wrong_method));

        let wrong_host = Request::builder()
            .method(Method::GET)
            .uri(RUNTIME_CANARY_PATH)
            .header(HOST, "example.com")
            .body(())
            .unwrap();
        assert!(!is_runtime_data_plane_canary(&wrong_host));
    }

    #[test]
    fn test_admin_routing_decision_does_not_misclassify_external_admin_like_path() {
        let external = Request::builder()
            .method(Method::GET)
            .uri("http://example.com/_bifrost/api/proxy/address")
            .header("host", "example.com")
            .body(())
            .unwrap();
        let external_decision = admin_routing_decision(&external, 9900, "127.0.0.1");
        assert!(external_decision.is_proxy_request_to_other_server);
        assert!(!external_decision.routes_to_admin);

        let local = Request::builder()
            .method(Method::GET)
            .uri("/_bifrost/api/proxy/address")
            .header("host", "127.0.0.1:9900")
            .body(())
            .unwrap();
        assert!(admin_routing_decision(&local, 9900, "127.0.0.1").routes_to_admin);

        let virtual_host = Request::builder()
            .method(Method::GET)
            .uri("http://bifrost.local/settings")
            .header("host", "bifrost.local")
            .body(())
            .unwrap();
        let virtual_host_decision = admin_routing_decision(&virtual_host, 9900, "127.0.0.1");
        assert!(virtual_host_decision.is_admin_virtual_host);
        assert!(virtual_host_decision.routes_to_admin);
    }

    #[test]
    fn test_admin_virtual_host_absolute_uri_without_port_routes_to_admin() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://bifrost.local/")
            .body(())
            .unwrap();

        assert!(is_admin_virtual_host_request(&req));
        assert!(is_proxy_request_targeting_other(&req, 9900, "0.0.0.0"));
        assert!(!is_proxy_request_to_other_for_admin_routing(
            &req, 9900, "0.0.0.0"
        ));
    }

    #[test]
    fn test_admin_virtual_host_absolute_uri_with_self_port_routes_to_admin() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://bifrost.local:9900/_bifrost/")
            .body(())
            .unwrap();

        assert!(is_admin_virtual_host_request(&req));
        assert!(!is_proxy_request_to_other_for_admin_routing(
            &req, 9900, "0.0.0.0"
        ));
    }

    #[test]
    fn test_admin_virtual_host_absolute_asset_uri_routes_to_admin() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://bifrost.local/_bifrost/assets/index.js")
            .body(())
            .unwrap();

        assert!(is_admin_virtual_host_request(&req));
        assert!(!is_proxy_request_to_other_for_admin_routing(
            &req, 9900, "0.0.0.0"
        ));
    }

    #[test]
    fn test_external_absolute_admin_asset_uri_still_routes_to_proxy_target() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://evil.example/_bifrost/assets/index.js")
            .body(())
            .unwrap();

        assert!(!is_admin_virtual_host_request(&req));
        assert!(is_proxy_request_to_other_for_admin_routing(
            &req, 9900, "0.0.0.0"
        ));
    }

    #[test]
    fn test_external_absolute_uri_still_routes_to_proxy_target() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("http://example.test/")
            .body(())
            .unwrap();

        assert!(!is_admin_virtual_host_request(&req));
        assert!(is_proxy_request_to_other_for_admin_routing(
            &req, 9900, "0.0.0.0"
        ));
    }

    #[test]
    fn test_connect_to_admin_virtual_host_is_not_admin_ui_request() {
        let req = Request::builder()
            .method(Method::CONNECT)
            .uri("http://bifrost.local/")
            .body(())
            .unwrap();

        assert!(!is_admin_virtual_host_request(&req));
        assert!(is_proxy_request_to_other_for_admin_routing(
            &req, 9900, "0.0.0.0"
        ));
    }

    #[test]
    fn test_proxy_request_trust_probe_target_extracts_absolute_probe_url() {
        let req = Request::builder()
            .uri("http://10.0.0.2:8802/_bifrost/trust-probe/netcheck?sid=s1&deviceId=d1")
            .body(())
            .unwrap();

        assert_eq!(
            proxy_request_trust_probe_target(&req),
            Some(("10.0.0.2".to_string(), 8802))
        );
    }

    #[test]
    fn test_proxy_request_trust_probe_target_uses_scheme_default_port() {
        let req = Request::builder()
            .uri("https://10.0.0.2/_bifrost/trust-probe/check?sid=s1&deviceId=d1")
            .body(())
            .unwrap();

        assert_eq!(
            proxy_request_trust_probe_target(&req),
            Some(("10.0.0.2".to_string(), 443))
        );
    }

    #[test]
    fn test_proxy_request_trust_probe_target_ignores_origin_form_and_proxy_config_probe() {
        let origin_form_req = Request::builder()
            .uri("/_bifrost/trust-probe/netcheck?sid=s1")
            .body(())
            .unwrap();
        let proxy_config_req = Request::builder()
            .uri("http://bifrost-proxy-check.invalid/_bifrost/trust-probe/proxy-configured?sid=s1")
            .body(())
            .unwrap();

        assert_eq!(proxy_request_trust_probe_target(&origin_form_req), None);
        assert_eq!(proxy_request_trust_probe_target(&proxy_config_req), None);
    }

    #[test]
    fn test_trust_probe_proxy_configured_request_accepts_absolute_and_host_header_forms() {
        let absolute_form_req = Request::builder()
            .uri("http://bifrost-proxy-check.invalid/_bifrost/trust-probe/proxy-configured?sid=s1")
            .body(())
            .unwrap();
        let origin_form_req = Request::builder()
            .uri("/_bifrost/trust-probe/proxy-configured?sid=s1")
            .header(HOST, "bifrost-proxy-check.invalid")
            .body(())
            .unwrap();
        let wrong_host_req = Request::builder()
            .uri("/_bifrost/trust-probe/proxy-configured?sid=s1")
            .header(HOST, "example.invalid")
            .body(())
            .unwrap();

        assert!(is_trust_probe_proxy_configured_request(&absolute_form_req));
        assert!(is_trust_probe_proxy_configured_request(&origin_form_req));
        assert!(!is_trust_probe_proxy_configured_request(&wrong_host_req));
    }

    #[test]
    fn test_proxy_request_to_trust_probe_direct_target_rejects_stale_probe_ports() {
        let req = Request::builder()
            .uri("http://127.0.0.1:8802/_bifrost/trust-probe/netcheck?sid=s1")
            .body(())
            .unwrap();

        assert!(is_proxy_request_to_trust_probe_direct_target(&req));
    }

    #[test]
    fn test_devtools_bridge_admin_path_is_narrow_exception() {
        assert!(is_devtools_bridge_admin_path(
            "/_bifrost/api/devtools/bridge/pg_123/ws"
        ));
        assert!(is_devtools_bridge_admin_path(
            "/_bifrost/api/devtools/bridge/pg_123/hello"
        ));
        assert!(!is_devtools_bridge_admin_path("/_bifrost/api/push"));
        assert!(!is_devtools_bridge_admin_path("/_bifrost/api/rules"));
        assert!(!is_devtools_bridge_admin_path(
            "/api/devtools/bridge/pg_123/ws"
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
            auto_tls_intercept: false,
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

    #[test]
    fn udp_fallback_detection_covers_io_network_and_unrelated_errors() {
        assert!(is_udp_relay_fallback_bind_error(&BifrostError::Io(
            std::io::Error::new(std::io::ErrorKind::AddrInUse, "busy")
        )));
        for message in [
            "Address already in use",
            "AddrInUse",
            "bind failed: os error 48",
            "bind failed: os error 98",
            "bind failed: os error 10013",
            "Only one usage of each socket address is normally permitted",
        ] {
            assert!(is_udp_relay_fallback_bind_error(&BifrostError::Network(
                message.to_string()
            )));
        }
        assert!(!is_udp_relay_fallback_bind_error(&BifrostError::Network(
            "connection refused".to_string()
        )));
        assert!(!is_udp_relay_fallback_bind_error(&BifrostError::Parse(
            "bad config".to_string()
        )));
    }

    #[test]
    fn response_cookie_serialization_escapes_and_emits_all_attributes() {
        let cookie = ResCookieValue {
            value: "quoted,semi;slash\\unicode-雪".to_string(),
            max_age: Some(60),
            path: Some("/api".to_string()),
            domain: Some("example.test".to_string()),
            secure: true,
            http_only: true,
            same_site: Some("Strict".to_string()),
        };
        let serialized = cookie.to_set_cookie_string("bad name");
        assert!(serialized.starts_with("bad%20name="));
        assert!(serialized.contains("%2C"));
        assert!(serialized.contains("%3B"));
        assert!(serialized.contains("%5C"));
        assert!(serialized.contains("Max-Age=60"));
        assert!(serialized.contains("Expires="));
        assert!(serialized.contains("Path=/api"));
        assert!(serialized.contains("Domain=example.test"));
        assert!(serialized.contains("Secure"));
        assert!(serialized.contains("HttpOnly"));
        assert!(serialized.contains("SameSite=Strict"));
        assert_eq!(cookie.to_string(), cookie.value);

        let simple = ResCookieValue::simple("value".to_string());
        assert_eq!(simple.to_set_cookie_string("name"), "name=value");
    }

    #[test]
    fn direct_browser_access_covers_host_port_path_scheme_remote_and_accept_matrix() {
        let config = ProxyConfig {
            host: "127.0.0.1".to_string(),
            port: 9900,
            ..ProxyConfig::default()
        };
        let make = |uri: &str, host: Option<&str>, accept: Option<&str>| {
            let mut builder = Request::builder().uri(uri);
            if let Some(host) = host {
                builder = builder.header(HOST, host);
            }
            if let Some(accept) = accept {
                builder = builder.header("accept", accept);
            }
            builder.body(()).unwrap()
        };
        assert!(is_direct_browser_access(
            &make("/", Some("127.0.0.1:9900"), Some("text/html,*/*")),
            &config,
            false
        ));
        assert!(is_direct_browser_access(
            &make("/", Some("remote.test:9900"), Some("text/html")),
            &config,
            true
        ));
        assert!(!is_direct_browser_access(
            &make("/other", Some("127.0.0.1:9900"), Some("text/html")),
            &config,
            false
        ));
        assert!(!is_direct_browser_access(
            &make(
                "http://127.0.0.1:9900/",
                Some("127.0.0.1:9900"),
                Some("text/html")
            ),
            &config,
            false
        ));
        assert!(!is_direct_browser_access(
            &make("/", None, Some("text/html")),
            &config,
            false
        ));
        assert!(!is_direct_browser_access(
            &make("/", Some("remote.test:9900"), Some("text/html")),
            &config,
            false
        ));
        assert!(!is_direct_browser_access(
            &make("/", Some("127.0.0.1:8800"), Some("text/html")),
            &config,
            false
        ));
        assert!(!is_direct_browser_access(
            &make("/", Some("127.0.0.1:9900"), Some("application/json")),
            &config,
            false
        ));
    }

    #[test]
    fn proxy_target_routing_covers_self_loopback_wildcard_and_remote_hosts() {
        let request = |uri: &str| Request::builder().uri(uri).body(()).unwrap();
        assert!(!is_proxy_request_targeting_other(
            &request("/relative"),
            9900,
            "127.0.0.1"
        ));
        assert!(is_proxy_request_targeting_other(
            &request("http://example.test:8800/"),
            9900,
            "127.0.0.1"
        ));
        assert!(!is_proxy_request_targeting_other(
            &request("http://127.0.0.1:9900/"),
            9900,
            "0.0.0.0"
        ));
        assert!(!is_proxy_request_targeting_other(
            &request("http://example.test:9900/"),
            9900,
            "0.0.0.0"
        ));
        assert!(is_proxy_request_targeting_other(
            &request("http://example.test:9900/"),
            9900,
            "proxy.test"
        ));
    }

    #[tokio::test]
    async fn trust_probe_bypass_and_response_helpers_cover_all_status_paths() {
        use http_body_util::BodyExt;

        let options = trust_probe_proxy_bypass_required_response(&Method::OPTIONS);
        assert_eq!(options.status(), StatusCode::NO_CONTENT);
        assert_eq!(options.headers()["access-control-allow-origin"], "*");
        assert!(options
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty());

        let get = trust_probe_proxy_bypass_required_response(&Method::GET);
        assert_eq!(get.status(), StatusCode::CONFLICT);
        assert_eq!(get.headers()[CONTENT_TYPE], "application/json");
        let body = get.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("trust_probe_must_bypass_proxy"));

        let redirect = redirect_response("https://example.test/next");
        assert_eq!(redirect.status(), StatusCode::FOUND);
        assert_eq!(redirect.headers()[LOCATION], "https://example.test/next");

        let required = proxy_auth_required_response();
        assert_eq!(required.status(), StatusCode::PROXY_AUTHENTICATION_REQUIRED);
        assert!(required.headers().contains_key("proxy-authenticate"));
        let banned = proxy_auth_banned_response();
        assert_eq!(banned.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(banned.headers()["retry-after"], "300");
    }

    #[test]
    fn tls_config_without_resolver_or_generator_returns_explicit_error() {
        let error = TlsConfig::default()
            .resolve_server_config("example.test", &[b"h2".to_vec()])
            .unwrap_err();
        assert!(error.to_string().contains("cert generator not configured"));
    }

    #[test]
    fn proxy_server_builder_accessors_cover_optional_state() {
        let state = AdminState::new(19900);
        let access = Arc::new(RwLock::new(ClientAccessControl::new(
            AccessControlConfig::default(),
        )));
        let server = ProxyServer::new(ProxyConfig::default())
            .with_access_control(access.clone())
            .with_admin_state(state)
            .with_tls_config(Arc::new(TlsConfig::default()))
            .with_rules(Arc::new(NoOpRulesResolver));
        assert!(server.admin_state().is_some());
        assert!(Arc::ptr_eq(server.access_control(), &access));
        assert!(server.push_manager().is_none());
    }

    #[test]
    fn coverage_90_debug_fd_limit_and_noop_values_helpers() {
        let replacement = RegexReplace {
            pattern: Regex::new("old.+").unwrap(),
            replacement: "new".to_string(),
            global: true,
        };
        let debug = format!("{replacement:?}");
        assert!(debug.contains("old.+"));
        assert!(debug.contains("replacement"));
        assert!(debug.contains("global: true"));

        #[cfg(unix)]
        {
            assert!(is_fd_limit_error(&std::io::Error::from_raw_os_error(
                libc::EMFILE
            )));
            assert!(is_fd_limit_error(&std::io::Error::from_raw_os_error(
                libc::ENFILE
            )));
            assert!(!is_fd_limit_error(&std::io::Error::from_raw_os_error(
                libc::EINVAL
            )));
        }
        let resolver = NoOpRulesResolver;
        assert!(RulesResolver::values(&resolver).is_empty());
    }

    #[tokio::test]
    async fn coverage_90_real_http_connection_spawns_deferred_process_resolution() {
        use http_body_util::{BodyExt, Full};
        use hyper::client::conn::http1 as client_http1;
        use hyper::server::conn::http1 as server_http1;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;

        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let peer: SocketAddr = "127.0.0.1:54340".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:19920".parse().unwrap();
        let access = Arc::new(RwLock::new(ClientAccessControl::new(
            AccessControlConfig::default(),
        )));
        let initial_generation = access.read().await.generation();
        let service_access = Arc::clone(&access);
        let service = service_fn(move |request: Request<Incoming>| {
            let access = Arc::clone(&service_access);
            async move {
                handle_request(
                    request,
                    peer,
                    local,
                    Arc::new(StaticStatusRules),
                    Arc::new(TlsConfig::default()),
                    ProxyConfig {
                        port: 19920,
                        ..Default::default()
                    },
                    Some(Arc::new(AdminState::new(19920))),
                    None,
                    AdminSecurityConfig::new(19920),
                    Arc::new(DnsResolver::new(false)),
                    access,
                    initial_generation,
                    Arc::new(ConnectionProcessState::default()),
                    Arc::new(ProxyAuthRateLimiter::new()),
                )
                .await
            }
        });
        tokio::spawn(async move {
            server_http1::Builder::new()
                .serve_connection(TokioIo::new(server_io), service)
                .await
                .unwrap();
        });
        let (mut sender, connection) = client_http1::handshake(TokioIo::new(client_io))
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let response = sender
            .send_request(
                Request::builder()
                    .uri("http://coverage.test/deferred-process")
                    .header(hyper::header::HOST, "coverage.test")
                    .body(Full::new(Bytes::new()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty());

        for (uri, host) in [
            ("/login.html", "127.0.0.1:19920"),
            ("http://bifrost.local/", "bifrost.local"),
        ] {
            let response = sender
                .send_request(
                    Request::builder()
                        .uri(uri)
                        .header(hyper::header::HOST, host)
                        .body(Full::new(Bytes::new()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let _ = response.into_body().collect().await.unwrap();
        }

        let (remote_client_io, remote_server_io) = tokio::io::duplex(64 * 1024);
        let remote_peer: SocketAddr = "192.0.2.44:54341".parse().unwrap();
        let remote_access = Arc::new(RwLock::new(ClientAccessControl::new(
            AccessControlConfig::default(),
        )));
        let remote_generation = remote_access.read().await.generation();
        let service_access = Arc::clone(&remote_access);
        let remote_service = service_fn(move |request: Request<Incoming>| {
            let access = Arc::clone(&service_access);
            async move {
                handle_request(
                    request,
                    remote_peer,
                    local,
                    Arc::new(StaticStatusRules),
                    Arc::new(TlsConfig::default()),
                    ProxyConfig {
                        port: 19920,
                        ..Default::default()
                    },
                    Some(Arc::new(AdminState::new(19920))),
                    None,
                    AdminSecurityConfig::new(19920),
                    Arc::new(DnsResolver::new(false)),
                    access,
                    remote_generation,
                    Arc::new(ConnectionProcessState::default()),
                    Arc::new(ProxyAuthRateLimiter::new()),
                )
                .await
            }
        });
        tokio::spawn(async move {
            server_http1::Builder::new()
                .serve_connection(TokioIo::new(remote_server_io), remote_service)
                .await
                .unwrap();
        });
        let (mut remote_sender, remote_connection) =
            client_http1::handshake(TokioIo::new(remote_client_io))
                .await
                .unwrap();
        tokio::spawn(async move {
            let _ = remote_connection.await;
        });
        for (uri, host) in [
            ("/login.html", "127.0.0.1:19920"),
            ("http://bifrost.local/", "bifrost.local"),
        ] {
            let response = remote_sender
                .send_request(
                    Request::builder()
                        .uri(uri)
                        .header(hyper::header::HOST, host)
                        .body(Full::new(Bytes::new()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let _ = response.into_body().collect().await.unwrap();
        }
    }
}
