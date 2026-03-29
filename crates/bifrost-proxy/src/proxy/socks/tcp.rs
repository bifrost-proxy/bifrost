use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ensure_crypto_provider;
use bifrost_admin::{AdminState, ConnectionInfo, FrameDirection, TrafficRecord, TrafficType};
use bifrost_core::{BifrostError, Result};

use futures_util::FutureExt;
use hyper::body::Incoming;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use hyper::{Request, Response, Uri};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, Semaphore};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

use crate::dns::DnsResolver;
use crate::protocol::ProtocolDetector;
use crate::proxy::http::requires_client_app_for_tls_decision;
use crate::server::{
    full_body, BoxBody, NoOpRulesResolver, RulesResolver, TlsConfig, TlsInterceptConfig,
};
use crate::utils::logging::RequestContext;
use crate::utils::process_info::{
    app_policy_process_resolution_retry_config, resolve_client_process_async_for_connection,
    resolve_client_process_async_for_connection_with_retry,
};
use crate::utils::tee::store_request_body;
use bifrost_core::{AccessControlConfig, AccessDecision, AccessMode, ClientAccessControl};

use super::super::http::handle_http_request;
use super::udp::UdpRelay;

use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

const SOCKS5_VERSION: u8 = 0x05;

static SOCKS5_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

static SOCKS5_PROCESS_START_TS: LazyLock<u64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
});

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

fn generate_socks5_request_id() -> String {
    let seq = SOCKS5_REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("SOCKS-{:x}-{:06}", *SOCKS5_PROCESS_START_TS, seq)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthMethod {
    NoAuth = 0x00,
    GssApi = 0x01,
    UsernamePassword = 0x02,
    NoAcceptable = 0xFF,
}

impl From<u8> for AuthMethod {
    fn from(value: u8) -> Self {
        match value {
            0x00 => AuthMethod::NoAuth,
            0x01 => AuthMethod::GssApi,
            0x02 => AuthMethod::UsernamePassword,
            _ => AuthMethod::NoAcceptable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SocksCommand {
    Connect = 0x01,
    Bind = 0x02,
    UdpAssociate = 0x03,
}

impl TryFrom<u8> for SocksCommand {
    type Error = BifrostError;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0x01 => Ok(SocksCommand::Connect),
            0x02 => Ok(SocksCommand::Bind),
            0x03 => Ok(SocksCommand::UdpAssociate),
            _ => Err(BifrostError::Parse(format!(
                "Invalid SOCKS5 command: {}",
                value
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AddressType {
    IPv4 = 0x01,
    DomainName = 0x03,
    IPv6 = 0x04,
}

impl TryFrom<u8> for AddressType {
    type Error = BifrostError;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0x01 => Ok(AddressType::IPv4),
            0x03 => Ok(AddressType::DomainName),
            0x04 => Ok(AddressType::IPv6),
            _ => Err(BifrostError::Parse(format!(
                "Invalid SOCKS5 address type: {}",
                value
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SocksReply {
    Succeeded = 0x00,
    GeneralFailure = 0x01,
    ConnectionNotAllowed = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    TtlExpired = 0x06,
    CommandNotSupported = 0x07,
    AddressTypeNotSupported = 0x08,
}

#[derive(Debug, Clone)]
pub enum SocksAddress {
    IPv4(Ipv4Addr),
    IPv6(Ipv6Addr),
    DomainName(String),
}

impl SocksAddress {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            SocksAddress::IPv4(addr) => {
                let mut bytes = vec![AddressType::IPv4 as u8];
                bytes.extend_from_slice(&addr.octets());
                bytes
            }
            SocksAddress::IPv6(addr) => {
                let mut bytes = vec![AddressType::IPv6 as u8];
                bytes.extend_from_slice(&addr.octets());
                bytes
            }
            SocksAddress::DomainName(domain) => {
                let mut bytes = vec![AddressType::DomainName as u8];
                bytes.push(domain.len() as u8);
                bytes.extend_from_slice(domain.as_bytes());
                bytes
            }
        }
    }

    pub fn parse_from_bytes(atyp: u8, data: &[u8]) -> Result<(Self, u16, usize)> {
        match atyp {
            0x01 => {
                if data.len() < 6 {
                    return Err(BifrostError::Parse("IPv4 address too short".to_string()));
                }
                let addr = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
                let port = u16::from_be_bytes([data[4], data[5]]);
                Ok((SocksAddress::IPv4(addr), port, 6))
            }
            0x03 => {
                if data.is_empty() {
                    return Err(BifrostError::Parse(
                        "Domain name length missing".to_string(),
                    ));
                }
                let len = data[0] as usize;
                if data.len() < 1 + len + 2 {
                    return Err(BifrostError::Parse("Domain name too short".to_string()));
                }
                let domain = String::from_utf8(data[1..1 + len].to_vec())
                    .map_err(|e| BifrostError::Parse(format!("Invalid domain encoding: {}", e)))?;
                let port = u16::from_be_bytes([data[1 + len], data[2 + len]]);
                Ok((SocksAddress::DomainName(domain), port, 1 + len + 2))
            }
            0x04 => {
                if data.len() < 18 {
                    return Err(BifrostError::Parse("IPv6 address too short".to_string()));
                }
                let mut addr_bytes = [0u8; 16];
                addr_bytes.copy_from_slice(&data[0..16]);
                let addr = Ipv6Addr::from(addr_bytes);
                let port = u16::from_be_bytes([data[16], data[17]]);
                Ok((SocksAddress::IPv6(addr), port, 18))
            }
            _ => Err(BifrostError::Parse(format!(
                "Invalid address type: {}",
                atyp
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SocksConfig {
    pub port: u16,
    pub host: String,
    pub auth_required: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub timeout_secs: u64,
    pub access_mode: AccessMode,
    pub client_whitelist: Vec<String>,
    pub allow_lan: bool,
    pub enable_udp: bool,
    pub udp_port: Option<u16>,
}

impl Default for SocksConfig {
    fn default() -> Self {
        Self {
            port: 1080,
            host: "127.0.0.1".to_string(),
            auth_required: false,
            username: None,
            password: None,
            timeout_secs: 30,
            access_mode: AccessMode::LocalOnly,
            client_whitelist: Vec::new(),
            allow_lan: false,
            enable_udp: true,
            udp_port: None,
        }
    }
}

pub struct SocksServer {
    config: SocksConfig,
    rules: Arc<dyn RulesResolver>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    udp_relay_addr: Arc<RwLock<Option<SocketAddr>>>,
    #[allow(dead_code)]
    udp_relay: Arc<RwLock<Option<UdpRelay>>>,
    tls_config: Option<Arc<TlsConfig>>,
    tls_intercept_config: Option<TlsInterceptConfig>,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
    verbose_logging: bool,
    unsafe_ssl: bool,
}

impl SocksServer {
    pub fn new(config: SocksConfig) -> Self {
        let access_config = AccessControlConfig {
            mode: config.access_mode,
            whitelist: config.client_whitelist.clone(),
            allow_lan: config.allow_lan,
        };
        Self {
            config,
            rules: Arc::new(crate::server::NoOpRulesResolver),
            access_control: Arc::new(RwLock::new(ClientAccessControl::new(access_config))),
            udp_relay_addr: Arc::new(RwLock::new(None)),
            udp_relay: Arc::new(RwLock::new(None)),
            tls_config: None,
            tls_intercept_config: None,
            admin_state: None,
            dns_resolver: None,
            verbose_logging: false,
            unsafe_ssl: true,
        }
    }

    pub fn with_rules(mut self, rules: Arc<dyn RulesResolver>) -> Self {
        self.rules = rules;
        self
    }

    pub fn with_access_control(mut self, access_control: Arc<RwLock<ClientAccessControl>>) -> Self {
        self.access_control = access_control;
        self
    }

    pub fn with_tls_intercept(
        mut self,
        tls_config: Arc<TlsConfig>,
        tls_intercept_config: TlsInterceptConfig,
        admin_state: Option<Arc<AdminState>>,
    ) -> Self {
        self.tls_config = Some(tls_config);
        self.tls_intercept_config = Some(tls_intercept_config);
        self.admin_state = admin_state;
        self
    }

    pub fn with_dns_resolver(mut self, dns_resolver: Arc<DnsResolver>) -> Self {
        self.dns_resolver = Some(dns_resolver);
        self
    }

    pub fn with_verbose_logging(mut self, verbose: bool) -> Self {
        self.verbose_logging = verbose;
        self
    }

    pub fn with_unsafe_ssl(mut self, unsafe_ssl: bool) -> Self {
        self.unsafe_ssl = unsafe_ssl;
        self
    }

    pub fn config(&self) -> &SocksConfig {
        &self.config
    }

    pub fn access_control(&self) -> &Arc<RwLock<ClientAccessControl>> {
        &self.access_control
    }

    pub async fn bind(&self, addr: SocketAddr) -> Result<TcpListener> {
        TcpListener::bind(addr)
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to bind to {}: {}", addr, e)))
    }

    pub async fn run(&self) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| BifrostError::Config(format!("Invalid address: {}", e)))?;

        let listener = self.bind(addr).await?;
        info!("SOCKS5 server listening on {}", addr);

        if self.config.enable_udp {
            let udp_port = self.config.udp_port.unwrap_or(0);
            let udp_addr: SocketAddr = format!("{}:{}", self.config.host, udp_port)
                .parse()
                .map_err(|e| BifrostError::Config(format!("Invalid UDP address: {}", e)))?;

            let mut udp_relay = UdpRelay::new(udp_addr)
                .with_rules(Arc::clone(&self.rules))
                .with_admin_state(self.admin_state.clone())
                .with_access_control(Arc::clone(&self.access_control));
            let relay_addr = udp_relay.start().await?;
            info!("SOCKS5 UDP relay started on {}", relay_addr);

            {
                let mut addr_guard = self.udp_relay_addr.write().await;
                *addr_guard = Some(relay_addr);
            }
            {
                let mut relay_guard = self.udp_relay.write().await;
                *relay_guard = Some(udp_relay);
            }
        }

        self.serve(listener).await
    }

    pub async fn get_udp_relay_addr(&self) -> Option<SocketAddr> {
        let addr = self.udp_relay_addr.read().await;
        *addr
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
                            "SOCKS5: accept recovered from transient errors"
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
                                "SOCKS5: accept hit file descriptor limit (EMFILE), backing off"
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
                            "SOCKS5: failed to accept connection"
                        );

                        if consecutive_errors >= FATAL_CONSECUTIVE_ERRORS {
                            return Err(BifrostError::Network(format!(
                                "SOCKS5: too many consecutive non-EMFILE accept errors ({}): {}",
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
                        "SOCKS5: connection limit reached, dropping new connection"
                    );
                    drop(stream);
                    continue;
                }
            };

            debug!("SOCKS5: Accepted connection from {}", peer_addr);

            let decision = {
                let access_control = self.access_control.read().await;
                access_control.check_access(&peer_addr.ip())
            };

            match decision {
                AccessDecision::Allow => {}
                AccessDecision::Deny => {
                    warn!(
                        "SOCKS5: Access denied for client {} (not in whitelist)",
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
                        "SOCKS5: Non-whitelisted client {} added to pending authorization. \
                        Approve via admin UI or use `bifrost whitelist add {}`",
                        ip, ip
                    );
                    continue;
                }
            }

            let config = self.config.clone();
            let udp_relay_addr = Arc::clone(&self.udp_relay_addr);
            let rules = Arc::clone(&self.rules);
            let tls_config = self.tls_config.clone();
            let tls_intercept_config = self.tls_intercept_config.clone();
            let admin_state = self.admin_state.clone();
            let dns_resolver = self.dns_resolver.clone();
            let verbose_logging = self.verbose_logging;
            let unsafe_ssl = self.unsafe_ssl;

            tokio::spawn(async move {
                let _permit = permit;
                let handler_task = async {
                    let udp_addr = {
                        let addr = udp_relay_addr.read().await;
                        *addr
                    };
                    let mut handler = SocksHandler::from_config_with_rules(
                        stream, peer_addr, &config, udp_addr, rules,
                    );
                    handler = handler
                        .with_verbose_logging(verbose_logging)
                        .with_unsafe_ssl(unsafe_ssl);
                    if let Some(dns) = dns_resolver {
                        handler = handler.with_dns_resolver(dns);
                    }
                    if let (Some(tls_cfg), Some(intercept_cfg)) = (tls_config, tls_intercept_config)
                    {
                        handler = handler.with_tls_intercept(tls_cfg, intercept_cfg, admin_state);
                    }
                    if let Err(e) = handler.handle().await {
                        error!("SOCKS5 error for {}: {}", peer_addr, e);
                    }
                };

                let result = AssertUnwindSafe(handler_task).catch_unwind().await;

                if let Err(panic_err) = result {
                    let panic_msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic_err.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic".to_string()
                    };
                    error!("SOCKS5 handler for {} panicked: {}", peer_addr, panic_msg);
                }
            });
        }
    }
}

pub struct SocksHandler {
    stream: Option<TcpStream>,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    auth_required: bool,
    username: Option<String>,
    password: Option<String>,
    timeout_secs: u64,
    udp_relay_addr: Option<SocketAddr>,
    rules: Option<Arc<dyn RulesResolver>>,
    tls_config: Option<Arc<TlsConfig>>,
    tls_intercept_config: Option<TlsInterceptConfig>,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
    verbose_logging: bool,
    unsafe_ssl: bool,
}

impl SocksHandler {
    pub fn new(
        stream: TcpStream,
        peer_addr: SocketAddr,
        auth_required: bool,
        username: Option<String>,
        password: Option<String>,
        timeout_secs: u64,
        udp_relay_addr: Option<SocketAddr>,
    ) -> Self {
        let local_addr = stream.local_addr().unwrap_or(peer_addr);
        Self {
            stream: Some(stream),
            peer_addr,
            local_addr,
            auth_required,
            username,
            password,
            timeout_secs,
            udp_relay_addr,
            rules: None,
            tls_config: None,
            tls_intercept_config: None,
            admin_state: None,
            dns_resolver: None,
            verbose_logging: false,
            unsafe_ssl: true,
        }
    }

    fn stream(&mut self) -> &mut TcpStream {
        self.stream.as_mut().expect("Stream should be available")
    }

    pub fn with_rules(mut self, rules: Arc<dyn RulesResolver>) -> Self {
        self.rules = Some(rules);
        self
    }

    pub fn with_tls_intercept(
        mut self,
        tls_config: Arc<TlsConfig>,
        tls_intercept_config: TlsInterceptConfig,
        admin_state: Option<Arc<AdminState>>,
    ) -> Self {
        self.tls_config = Some(tls_config);
        self.tls_intercept_config = Some(tls_intercept_config);
        self.admin_state = admin_state;
        self
    }

    pub fn with_dns_resolver(mut self, dns_resolver: Arc<DnsResolver>) -> Self {
        self.dns_resolver = Some(dns_resolver);
        self
    }

    pub fn with_verbose_logging(mut self, verbose: bool) -> Self {
        self.verbose_logging = verbose;
        self
    }

    pub fn with_unsafe_ssl(mut self, unsafe_ssl: bool) -> Self {
        self.unsafe_ssl = unsafe_ssl;
        self
    }

    pub fn from_config(
        stream: TcpStream,
        peer_addr: SocketAddr,
        config: &SocksConfig,
        udp_relay_addr: Option<SocketAddr>,
    ) -> Self {
        Self::new(
            stream,
            peer_addr,
            config.auth_required,
            config.username.clone(),
            config.password.clone(),
            config.timeout_secs,
            udp_relay_addr,
        )
    }

    pub fn from_config_with_rules(
        stream: TcpStream,
        peer_addr: SocketAddr,
        config: &SocksConfig,
        udp_relay_addr: Option<SocketAddr>,
        rules: Arc<dyn RulesResolver>,
    ) -> Self {
        Self::from_config(stream, peer_addr, config, udp_relay_addr).with_rules(rules)
    }

    pub async fn handle(mut self) -> Result<()> {
        self.handle_client().await
    }

    async fn handle_client(&mut self) -> Result<()> {
        let auth_method = self.handle_handshake().await?;

        if auth_method == AuthMethod::UsernamePassword {
            self.handle_auth().await?;
        }

        let (command, address, port) = self.handle_request().await?;

        match command {
            SocksCommand::Connect => self.connect_and_relay(address, port).await,
            SocksCommand::UdpAssociate => self.handle_udp_associate(address, port).await,
            SocksCommand::Bind => {
                self.send_reply(SocksReply::CommandNotSupported, None)
                    .await?;
                Err(BifrostError::Network(
                    "BIND command not supported".to_string(),
                ))
            }
        }
    }

    async fn handle_handshake(&mut self) -> Result<AuthMethod> {
        let mut header = [0u8; 2];
        self.stream().read_exact(&mut header).await?;

        let version = header[0];
        let nmethods = header[1];

        if version != SOCKS5_VERSION {
            return Err(BifrostError::Parse(format!(
                "Invalid SOCKS version: {}",
                version
            )));
        }

        let mut methods = vec![0u8; nmethods as usize];
        self.stream().read_exact(&mut methods).await?;

        let selected_method = self.select_auth_method(&methods);

        let response = [SOCKS5_VERSION, selected_method as u8];
        self.stream().write_all(&response).await?;

        if selected_method == AuthMethod::NoAcceptable {
            return Err(BifrostError::Network(
                "No acceptable authentication method".to_string(),
            ));
        }

        Ok(selected_method)
    }

    fn select_auth_method(&self, methods: &[u8]) -> AuthMethod {
        if self.auth_required {
            if methods.contains(&(AuthMethod::UsernamePassword as u8)) {
                return AuthMethod::UsernamePassword;
            }
        } else if methods.contains(&(AuthMethod::NoAuth as u8)) {
            return AuthMethod::NoAuth;
        }

        if methods.contains(&(AuthMethod::UsernamePassword as u8))
            && self.username.is_some()
            && self.password.is_some()
        {
            return AuthMethod::UsernamePassword;
        }

        if methods.contains(&(AuthMethod::NoAuth as u8)) && !self.auth_required {
            return AuthMethod::NoAuth;
        }

        AuthMethod::NoAcceptable
    }

    async fn handle_auth(&mut self) -> Result<()> {
        let mut version = [0u8; 1];
        self.stream().read_exact(&mut version).await?;

        if version[0] != 0x01 {
            return Err(BifrostError::Parse(format!(
                "Invalid auth version: {}",
                version[0]
            )));
        }

        let mut ulen = [0u8; 1];
        self.stream().read_exact(&mut ulen).await?;
        let mut username = vec![0u8; ulen[0] as usize];
        self.stream().read_exact(&mut username).await?;

        let mut plen = [0u8; 1];
        self.stream().read_exact(&mut plen).await?;
        let mut password = vec![0u8; plen[0] as usize];
        self.stream().read_exact(&mut password).await?;

        let username = String::from_utf8_lossy(&username).to_string();
        let password = String::from_utf8_lossy(&password).to_string();

        let auth_success = self.verify_credentials(&username, &password);

        let response = if auth_success {
            [0x01, 0x00]
        } else {
            [0x01, 0x01]
        };

        self.stream().write_all(&response).await?;

        if !auth_success {
            return Err(BifrostError::Network("Authentication failed".to_string()));
        }

        debug!("SOCKS5: User '{}' authenticated successfully", username);
        Ok(())
    }

    fn verify_credentials(&self, username: &str, password: &str) -> bool {
        match (&self.username, &self.password) {
            (Some(expected_user), Some(expected_pass)) => {
                username == expected_user && password == expected_pass
            }
            _ => false,
        }
    }

    async fn handle_request(&mut self) -> Result<(SocksCommand, SocksAddress, u16)> {
        let mut header = [0u8; 4];
        self.stream().read_exact(&mut header).await?;

        let version = header[0];
        let cmd = header[1];
        let atyp = header[3];

        if version != SOCKS5_VERSION {
            return Err(BifrostError::Parse(format!(
                "Invalid SOCKS version in request: {}",
                version
            )));
        }

        let command = SocksCommand::try_from(cmd)?;
        let addr_type = AddressType::try_from(atyp)?;

        let address = self.read_address(addr_type).await?;

        let mut port_bytes = [0u8; 2];
        self.stream().read_exact(&mut port_bytes).await?;
        let port = u16::from_be_bytes(port_bytes);

        debug!("SOCKS5: Request {:?} to {:?}:{}", command, address, port);

        Ok((command, address, port))
    }

    async fn read_address(&mut self, addr_type: AddressType) -> Result<SocksAddress> {
        match addr_type {
            AddressType::IPv4 => {
                let mut addr = [0u8; 4];
                self.stream().read_exact(&mut addr).await?;
                Ok(SocksAddress::IPv4(Ipv4Addr::from(addr)))
            }
            AddressType::IPv6 => {
                let mut addr = [0u8; 16];
                self.stream().read_exact(&mut addr).await?;
                Ok(SocksAddress::IPv6(Ipv6Addr::from(addr)))
            }
            AddressType::DomainName => {
                let mut len = [0u8; 1];
                self.stream().read_exact(&mut len).await?;
                let mut domain = vec![0u8; len[0] as usize];
                self.stream().read_exact(&mut domain).await?;
                let domain_str = String::from_utf8(domain).map_err(|e| {
                    BifrostError::Parse(format!("Invalid domain name encoding: {}", e))
                })?;
                Ok(SocksAddress::DomainName(domain_str))
            }
        }
    }

    async fn connect_and_relay(&mut self, address: SocksAddress, port: u16) -> Result<()> {
        let url = match &address {
            SocksAddress::IPv4(ip) => format!("socks5://{}:{}", ip, port),
            SocksAddress::IPv6(ip) => format!("socks5://[{}]:{}", ip, port),
            SocksAddress::DomainName(domain) => format!("socks5://{}:{}", domain, port),
        };

        let host_str = match &address {
            SocksAddress::IPv4(ip) => ip.to_string(),
            SocksAddress::IPv6(ip) => ip.to_string(),
            SocksAddress::DomainName(domain) => domain.clone(),
        };

        let (target_host, target_port) = if let Some(ref rules) = self.rules {
            debug!("SOCKS5: Resolving rules for URL: {}", url);
            let resolved = rules.resolve(&url, "CONNECT");
            debug!("SOCKS5: Resolved host rule: {:?}", resolved.host);
            if let Some(ref host_rule) = resolved.host {
                let host_rule = host_rule.trim_end_matches('/');
                let parts: Vec<&str> = host_rule.split(':').collect();
                let h = parts[0].to_string();
                let p = if parts.len() > 1 {
                    parts[1].parse().unwrap_or(port)
                } else {
                    port
                };
                debug!("SOCKS5: Rule applied - {} -> {}:{}", url, h, p);
                (h, p)
            } else {
                let scheme = if port == 443 { "https" } else { "http" };
                let scheme_url = format!("{}://{}:{}", scheme, host_str, port);
                let scheme_resolved = rules.resolve(&scheme_url, "CONNECT");
                if let Some(ref host_rule) = scheme_resolved.host {
                    let host_rule = host_rule.trim_end_matches('/');
                    let parts: Vec<&str> = host_rule.split(':').collect();
                    let h = parts[0].to_string();
                    let p = if parts.len() > 1 {
                        parts[1].parse().unwrap_or(port)
                    } else {
                        match scheme_resolved.host_protocol {
                            Some(bifrost_core::Protocol::Http)
                            | Some(bifrost_core::Protocol::Ws) => 80,
                            Some(bifrost_core::Protocol::Https)
                            | Some(bifrost_core::Protocol::Wss) => 443,
                            _ => port,
                        }
                    };
                    debug!(
                        "SOCKS5: Rule applied via {}:// URL - {} -> {}:{}",
                        scheme, url, h, p
                    );
                    (h, p)
                } else {
                    (host_str.clone(), port)
                }
            }
        } else {
            (host_str.clone(), port)
        };

        let target_addr = format!("{}:{}", target_host, target_port);
        match TcpStream::connect(&target_addr).await {
            Ok(target_stream) => {
                if let Err(e) = target_stream.set_nodelay(true) {
                    debug!("Failed to set TCP_NODELAY on SOCKS5 connection: {}", e);
                }
                let local_addr = target_stream.local_addr().ok();
                self.send_reply(SocksReply::Succeeded, local_addr).await?;
                debug!("SOCKS5: Connected to {}", target_addr);
                self.relay_data(target_stream, &target_host, target_port, &url)
                    .await
            }
            Err(e) => {
                let reply = match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => SocksReply::ConnectionRefused,
                    std::io::ErrorKind::AddrNotAvailable => SocksReply::HostUnreachable,
                    _ => SocksReply::GeneralFailure,
                };
                self.send_reply(reply, None).await?;
                Err(BifrostError::Network(format!(
                    "Failed to connect to {}: {}",
                    target_addr, e
                )))
            }
        }
    }

    async fn handle_udp_associate(&mut self, _address: SocksAddress, _port: u16) -> Result<()> {
        let udp_relay_addr = match self.udp_relay_addr {
            Some(addr) => addr,
            None => {
                self.send_reply(SocksReply::CommandNotSupported, None)
                    .await?;
                return Err(BifrostError::Network(
                    "UDP ASSOCIATE not enabled on this server".to_string(),
                ));
            }
        };

        info!("SOCKS5: UDP ASSOCIATE request, relay at {}", udp_relay_addr);

        self.send_reply(SocksReply::Succeeded, Some(udp_relay_addr))
            .await?;

        debug!("SOCKS5: UDP ASSOCIATE established, keeping TCP connection alive");

        let mut buf = [0u8; 1];
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_secs(self.timeout_secs * 10),
                self.stream().read(&mut buf),
            )
            .await
            {
                Ok(Ok(0)) => {
                    debug!("SOCKS5: UDP ASSOCIATE TCP connection closed by client");
                    break;
                }
                Ok(Ok(_)) => {
                    continue;
                }
                Ok(Err(e)) => {
                    debug!("SOCKS5: UDP ASSOCIATE TCP connection error: {}", e);
                    break;
                }
                Err(_) => {
                    debug!("SOCKS5: UDP ASSOCIATE timeout, closing connection");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn send_reply(
        &mut self,
        reply: SocksReply,
        bound_addr: Option<SocketAddr>,
    ) -> Result<()> {
        let mut response = vec![SOCKS5_VERSION, reply as u8, 0x00];

        match bound_addr {
            Some(SocketAddr::V4(addr)) => {
                response.push(AddressType::IPv4 as u8);
                response.extend_from_slice(&addr.ip().octets());
                response.extend_from_slice(&addr.port().to_be_bytes());
            }
            Some(SocketAddr::V6(addr)) => {
                response.push(AddressType::IPv6 as u8);
                response.extend_from_slice(&addr.ip().octets());
                response.extend_from_slice(&addr.port().to_be_bytes());
            }
            None => {
                response.push(AddressType::IPv4 as u8);
                response.extend_from_slice(&[0, 0, 0, 0]);
                response.extend_from_slice(&[0, 0]);
            }
        }

        self.stream().write_all(&response).await?;
        Ok(())
    }

    async fn relay_data(
        &mut self,
        target_stream: TcpStream,
        target_host: &str,
        target_port: u16,
        original_url: &str,
    ) -> Result<()> {
        let resolved_rules = if let Some(ref rules) = self.rules {
            let resolved = rules.resolve(original_url, "CONNECT");
            debug!(
                "SOCKS5: Rule tls_intercept={:?} for {}",
                resolved.tls_intercept, original_url
            );
            resolved
        } else {
            crate::server::ResolvedRules::default()
        };

        let mut peek_buf = [0u8; 16];
        let peek_len = self.stream().peek(&mut peek_buf).await.unwrap_or(0);

        if peek_len > 0 {
            let protocol = ProtocolDetector::detect_protocol_type(&peek_buf[..peek_len]);
            debug!(
                "SOCKS5: Detected protocol {:?} for {}:{}",
                protocol, target_host, target_port
            );

            match protocol {
                Some(crate::protocol::TransportProtocol::Tls) => {
                    let original_host_port = original_url
                        .strip_prefix("socks5://")
                        .unwrap_or(original_url);
                    let https_url = format!("https://{}", original_host_port);

                    let tls_resolved_rules = if let Some(ref rules) = self.rules {
                        let https_resolved = rules.resolve(&https_url, "CONNECT");
                        debug!(
                            "SOCKS5: Re-resolved rules via https:// URL {}: host={:?}, tls_intercept={:?}, rules_count={}",
                            https_url, https_resolved.host, https_resolved.tls_intercept, https_resolved.rules.len()
                        );
                        if https_resolved.host.is_some()
                            || https_resolved.tls_intercept.is_some()
                            || !https_resolved.rules.is_empty()
                            || !https_resolved.res_headers.is_empty()
                            || !https_resolved.req_headers.is_empty()
                        {
                            https_resolved
                        } else {
                            resolved_rules.clone()
                        }
                    } else {
                        resolved_rules.clone()
                    };

                    let original_host = original_host_port
                        .trim_start_matches('[')
                        .split([']', ':'])
                        .next()
                        .unwrap_or(target_host);

                    if let Some(ref tls_config) = &self.tls_config {
                        let tls_intercept_config: Option<TlsInterceptConfig> =
                            if let Some(ref state) = self.admin_state {
                                let runtime_config = state.runtime_config.read().await;
                                Some(TlsInterceptConfig {
                                    enable_tls_interception: runtime_config.enable_tls_interception,
                                    intercept_exclude: runtime_config.intercept_exclude.clone(),
                                    intercept_include: runtime_config.intercept_include.clone(),
                                    app_intercept_exclude: runtime_config
                                        .app_intercept_exclude
                                        .clone(),
                                    app_intercept_include: runtime_config
                                        .app_intercept_include
                                        .clone(),
                                    unsafe_ssl: runtime_config.unsafe_ssl,
                                })
                            } else {
                                self.tls_intercept_config.clone()
                            };
                        if let Some(ref tls_intercept_config) = tls_intercept_config {
                            let is_local_client = self.peer_addr.ip().is_loopback();
                            let requires_client_app = is_local_client
                                && requires_client_app_for_tls_decision(tls_intercept_config);
                            let client_process = if requires_client_app {
                                let (max_retries, delay_ms) =
                                    app_policy_process_resolution_retry_config();
                                info!(
                                    target_host,
                                    target_port,
                                    peer_addr = %self.peer_addr,
                                    local_addr = %self.local_addr,
                                    max_retries,
                                    delay_ms,
                                    "SOCKS5 request requires synchronous client process resolution for app policy"
                                );
                                resolve_client_process_async_for_connection_with_retry(
                                    &self.peer_addr,
                                    &self.local_addr,
                                    max_retries,
                                    delay_ms,
                                )
                                .await
                            } else {
                                resolve_client_process_async_for_connection(
                                    &self.peer_addr,
                                    &self.local_addr,
                                )
                                .await
                            };
                            let client_app = client_process.as_ref().map(|p| p.name.as_str());

                            debug!(
                                "SOCKS5: Client process for {}:{} - app={:?}",
                                target_host, target_port, client_app
                            );

                            if requires_client_app {
                                match client_process.as_ref() {
                                    Some(process) => info!(
                                        target_host,
                                        target_port,
                                        peer_addr = %self.peer_addr,
                                        local_addr = %self.local_addr,
                                        client_app = %process.name,
                                        client_pid = process.pid,
                                        "SOCKS5 synchronous client process resolution succeeded"
                                    ),
                                    None => {
                                        if let Some(ref state) = self.admin_state {
                                            state
                                                .metrics_collector
                                                .increment_client_process_resolution_failure();
                                            state
                                                .metrics_collector
                                                .increment_client_process_policy_unknown_decision();
                                        }
                                        warn!(
                                            target_host,
                                            target_port,
                                            peer_addr = %self.peer_addr,
                                            local_addr = %self.local_addr,
                                            "SOCKS5 app-based TLS decision fell back because client process is unknown"
                                        );
                                    }
                                }
                            }

                            let do_intercept = crate::proxy::http::should_intercept_tls_for_client(
                                original_host,
                                client_app,
                                is_local_client,
                                tls_intercept_config,
                                tls_config,
                                &tls_resolved_rules,
                            ) || (tls_config.ca_cert.is_some()
                                && !matches!(tls_resolved_rules.tls_intercept, Some(false))
                                && (crate::proxy::http::requires_tls_interception_for_rules(
                                    &tls_resolved_rules,
                                ) || (tls_resolved_rules.host.is_some()
                                    && !tls_resolved_rules.ignored.host)
                                    || self.rules.as_ref().is_some_and(|r| {
                                        r.has_response_rules_for_host(original_host)
                                    })));
                            if do_intercept {
                                debug!(
                                    "SOCKS5: TLS interception enabled for {}:{} (original_host={}, client_app={:?}, rule={:?}, global={})",
                                    target_host, target_port, original_host, client_app, tls_resolved_rules.tls_intercept, tls_intercept_config.enable_tls_interception
                                );
                                return self
                                    .relay_with_tls_intercept(
                                        target_stream,
                                        target_host,
                                        target_port,
                                        original_host,
                                    )
                                    .await;
                            } else {
                                debug!(
                                    "SOCKS5: TLS passthrough for {}:{} (client_app={:?})",
                                    target_host, target_port, client_app
                                );
                            }
                        }
                    }
                }
                Some(crate::protocol::TransportProtocol::Http1) => {
                    if self.rules.is_some() {
                        debug!(
                            "SOCKS5: HTTP interception for {}:{}",
                            target_host, target_port
                        );
                        return self
                            .relay_with_http_intercept(target_stream, target_host, target_port)
                            .await;
                    }
                }
                _ => {}
            }
        }

        self.relay_raw(target_stream, target_host, target_port)
            .await
    }

    async fn relay_raw(
        &mut self,
        target_stream: TcpStream,
        target_host: &str,
        target_port: u16,
    ) -> Result<()> {
        let start_time = Instant::now();
        let req_id = generate_socks5_request_id();
        let peer_addr = self.peer_addr;
        let admin_state = self.admin_state.clone();

        let original_url = format!("socks5://{}:{}", target_host, target_port);
        let resolved_rules = if let Some(ref rules) = self.rules {
            rules.resolve(&original_url, "CONNECT")
        } else {
            crate::server::ResolvedRules::default()
        };

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

        let client_process =
            resolve_client_process_async_for_connection(&peer_addr, &self.local_addr).await;
        let (client_app, client_pid, client_path) = client_process
            .as_ref()
            .map(|p| (Some(p.name.clone()), Some(p.pid), p.path.clone()))
            .unwrap_or((None, None, None));

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .increment_connections_by_type(TrafficType::Socks5);
            state
                .metrics_collector
                .increment_requests_by_type(TrafficType::Socks5);

            let conn_info = ConnectionInfo::new(
                req_id.clone(),
                target_host.to_string(),
                target_port,
                false,
                client_app.clone(),
                cancel_tx,
            );
            state.connection_registry.register(conn_info);

            let mut record = TrafficRecord::new(
                req_id.clone(),
                "CONNECT".to_string(),
                format!("socks5://{}:{}", target_host, target_port),
            );
            record.status = 200;
            record.protocol = "socks5".to_string();
            record.host = target_host.to_string();
            record.is_tunnel = true;
            record.client_ip = peer_addr.ip().to_string();
            record.client_app = client_app;
            record.client_pid = client_pid;
            record.client_path = client_path;
            record.has_rule_hit = !resolved_rules.rules.is_empty()
                || resolved_rules.host.is_some()
                || resolved_rules.proxy.is_some();
            record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);

            state.record_traffic(record);
            state.connection_monitor.register_connection(&req_id);

            info!(
                "[{}] SOCKS5 tunnel established to {}:{} (from {})",
                req_id, target_host, target_port, peer_addr
            );
        }

        let (mut client_read, mut client_write) = self.stream().split();
        let (mut target_read, mut target_write) = target_stream.into_split();

        let bytes_sent = Arc::new(AtomicU64::new(0));
        let bytes_received = Arc::new(AtomicU64::new(0));
        let bytes_sent_clone = Arc::clone(&bytes_sent);
        let bytes_received_clone = Arc::clone(&bytes_received);
        let admin_state_send = admin_state.clone();
        let admin_state_recv = admin_state.clone();
        let req_id_send = req_id.clone();
        let req_id_recv = req_id.clone();

        let client_to_target = async move {
            let mut buf = vec![0u8; 8192];
            loop {
                let n = client_read.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                bytes_sent_clone.fetch_add(n as u64, Ordering::Relaxed);
                if let Some(ref state) = admin_state_send {
                    state
                        .metrics_collector
                        .add_bytes_sent_by_type(TrafficType::Socks5, n as u64);
                    state.connection_monitor.update_traffic(
                        &req_id_send,
                        FrameDirection::Send,
                        n as u64,
                    );
                }
                target_write.write_all(&buf[..n]).await?;
                target_write.flush().await?;
            }
            target_write.shutdown().await?;
            Ok::<_, std::io::Error>(())
        };

        let target_to_client = async move {
            let mut buf = vec![0u8; 8192];
            loop {
                let n = target_read.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                bytes_received_clone.fetch_add(n as u64, Ordering::Relaxed);
                if let Some(ref state) = admin_state_recv {
                    state
                        .metrics_collector
                        .add_bytes_received_by_type(TrafficType::Socks5, n as u64);
                    state.connection_monitor.update_traffic(
                        &req_id_recv,
                        FrameDirection::Receive,
                        n as u64,
                    );
                }
                client_write.write_all(&buf[..n]).await?;
                client_write.flush().await?;
            }
            Ok::<_, std::io::Error>(())
        };

        let cancel_future = async {
            let _ = cancel_rx.await;
            Err::<(), std::io::Error>(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Connection cancelled",
            ))
        };

        let relay_future = async { tokio::try_join!(client_to_target, target_to_client) };

        let result = tokio::select! {
            res = relay_future => res.map(|_| ()),
            _ = cancel_future => Ok(()),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let total_sent = bytes_sent.load(Ordering::Relaxed);
        let total_received = bytes_received.load(Ordering::Relaxed);

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .decrement_connections_by_type(TrafficType::Socks5);

            state.connection_registry.unregister(&req_id);
            state.connection_monitor.unregister_connection(&req_id);

            state.update_traffic_by_id(&req_id, move |record| {
                record.request_size = total_sent as usize;
                record.response_size = total_received as usize;
                record.duration_ms = duration_ms;
            });

            debug!(
                "[{}] SOCKS5 tunnel closed: sent={} bytes, received={} bytes, duration={}ms",
                req_id, total_sent, total_received, duration_ms
            );
        }

        match result {
            Ok(_) => {
                debug!("SOCKS5: Connection closed normally");
                Ok(())
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::ConnectionReset
                    || e.kind() == std::io::ErrorKind::BrokenPipe
                    || e.kind() == std::io::ErrorKind::Interrupted
                {
                    debug!("SOCKS5: Connection closed: {}", e);
                    Ok(())
                } else {
                    Err(BifrostError::Network(format!("Relay error: {}", e)))
                }
            }
        }
    }

    async fn relay_with_tls_intercept(
        &mut self,
        _target_stream: TcpStream,
        target_host: &str,
        target_port: u16,
        cert_host: &str,
    ) -> Result<()> {
        ensure_crypto_provider();

        let tls_config = match &self.tls_config {
            Some(c) => Arc::clone(c),
            None => return Err(BifrostError::Tls("TLS config not available".to_string())),
        };
        let no_alpn_protocols: Vec<Vec<u8>> = Vec::new();
        let server_config = tls_config.resolve_server_config(cert_host, &no_alpn_protocols)?;

        let req_id = generate_socks5_request_id();
        let admin_state = self.admin_state.clone();

        debug!(
            "[{}] SOCKS5: Starting TLS MITM handshake for {}:{}",
            req_id, target_host, target_port
        );

        let acceptor = TlsAcceptor::from(server_config);

        let client_stream = self
            .stream
            .take()
            .ok_or_else(|| BifrostError::Network("Stream already taken".to_string()))?;

        let client_tls = acceptor
            .accept(client_stream)
            .await
            .map_err(|e| BifrostError::Tls(format!("TLS accept failed: {e}")))?;

        debug!(
            "[{}] SOCKS5: TLS handshake completed for {}:{}",
            req_id, target_host, target_port
        );

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

        let client_process =
            resolve_client_process_async_for_connection(&self.peer_addr, &self.local_addr).await;
        let client_app = client_process.as_ref().map(|p| p.name.clone());

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .increment_connections_by_type(TrafficType::Socks5);

            let conn_info = ConnectionInfo::new(
                req_id.clone(),
                cert_host.to_string(),
                target_port,
                true,
                client_app.clone(),
                cancel_tx,
            );
            state.connection_registry.register(conn_info);

            info!(
                "[{}] SOCKS5 TLS intercept tunnel established to {}:{} (from {})",
                req_id, cert_host, target_port, self.peer_addr
            );
        }

        let rules = self.rules.clone();
        let admin_state_for_service = self.admin_state.clone();
        let dns_resolver = self.dns_resolver.clone();
        let verbose_logging = self.verbose_logging;
        let unsafe_ssl = if let Some(ref state) = admin_state {
            state.runtime_config.read().await.unsafe_ssl
        } else {
            self.unsafe_ssl
        };
        let peer_addr = self.peer_addr;
        let target_host = target_host.to_string();
        let original_host = cert_host.to_string();
        let max_body_buffer_size = admin_state
            .as_ref()
            .map(|s| s.get_max_body_buffer_size())
            .unwrap_or(10 * 1024 * 1024);
        let max_body_probe_size = admin_state
            .as_ref()
            .map(|s| s.get_max_body_probe_size())
            .unwrap_or(64 * 1024);
        let http1_max_header_size = if let Some(ref state) = admin_state {
            if let Some(ref config_manager) = state.config_manager {
                config_manager.config().await.server.http1_max_header_size
            } else {
                64 * 1024
            }
        } else {
            64 * 1024
        };
        let local_addr = self.local_addr;

        let service = service_fn(move |req: Request<Incoming>| {
            let target_host = target_host.clone();
            let original_host = original_host.clone();
            let rules = rules.clone();
            let admin_state = admin_state_for_service.clone();
            let dns_resolver = dns_resolver.clone();
            async move {
                handle_socks5_intercepted_request(
                    req,
                    &target_host,
                    target_port,
                    &original_host,
                    rules,
                    admin_state,
                    dns_resolver,
                    max_body_buffer_size,
                    max_body_probe_size,
                    verbose_logging,
                    unsafe_ssl,
                    peer_addr,
                    local_addr,
                )
                .await
            }
        });

        let client_io = TokioIo::new(client_tls);

        let mut builder = ServerBuilder::new();
        builder
            .preserve_header_case(true)
            .title_case_headers(true)
            .max_buf_size(http1_max_header_size);

        let conn = builder.serve_connection(client_io, service).with_upgrades();

        let mut conn = std::pin::pin!(conn);

        let result = tokio::select! {
            result = conn.as_mut() => {
                match result {
                    Ok(_) => Ok(false),
                    Err(e) => {
                        debug!("[{}] SOCKS5 TLS: HTTP connection ended: {}", req_id, e);
                        Ok(false)
                    }
                }
            }
            _ = cancel_rx => {
                debug!("[{}] SOCKS5 TLS intercept cancelled by config change, initiating graceful shutdown", req_id);
                conn.as_mut().graceful_shutdown();
                let _ = conn.await;
                Ok(true)
            }
        };

        if let Some(ref state) = admin_state {
            state.connection_registry.unregister(&req_id);
            state
                .metrics_collector
                .decrement_connections_by_type(TrafficType::Socks5);

            match result {
                Ok(true) => {
                    info!(
                        "[{}] SOCKS5 TLS intercept tunnel {}:{} closed due to config change",
                        req_id, cert_host, target_port
                    );
                }
                Ok(false) => {
                    debug!(
                        "[{}] SOCKS5 TLS intercept tunnel {}:{} closed normally",
                        req_id, cert_host, target_port
                    );
                }
                Err(ref e) => {
                    debug!(
                        "[{}] SOCKS5 TLS intercept tunnel {}:{} error: {}",
                        req_id, cert_host, target_port, e
                    );
                }
            }
        }

        result.map(|_| ())
    }

    async fn relay_with_http_intercept(
        &mut self,
        mut target_stream: TcpStream,
        target_host: &str,
        target_port: u16,
    ) -> Result<()> {
        let rules = match &self.rules {
            Some(r) => Arc::clone(r),
            None => {
                return self
                    .relay_raw(target_stream, target_host, target_port)
                    .await
            }
        };

        let start_time = Instant::now();
        let req_id = generate_socks5_request_id();
        let peer_addr = self.peer_addr;
        let admin_state = self.admin_state.clone();

        let mut request_buf = vec![0u8; 8192];
        let n = self.stream().read(&mut request_buf).await?;
        if n == 0 {
            return Ok(());
        }

        let request_data = &request_buf[..n];
        let request_str = String::from_utf8_lossy(request_data);

        let first_line = request_str.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        let method = parts.first().unwrap_or(&"GET");
        let path = parts.get(1).unwrap_or(&"/");

        let url = format!("http://{}:{}{}", target_host, target_port, path);
        debug!("[{}] SOCKS5 HTTP: {} {}", req_id, method, url);

        let resolved_rules = rules.resolve(&url, method);
        let has_rules = !resolved_rules.rules.is_empty()
            || resolved_rules.host.is_some()
            || resolved_rules.proxy.is_some();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

        let client_process =
            resolve_client_process_async_for_connection(&peer_addr, &self.local_addr).await;
        let (client_app, client_pid, client_path) = client_process
            .as_ref()
            .map(|p| (Some(p.name.clone()), Some(p.pid), p.path.clone()))
            .unwrap_or((None, None, None));

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .increment_connections_by_type(TrafficType::Socks5);
            state
                .metrics_collector
                .increment_requests_by_type(TrafficType::Socks5);

            let conn_info = ConnectionInfo::new(
                req_id.clone(),
                target_host.to_string(),
                target_port,
                true,
                client_app.clone(),
                cancel_tx,
            );
            state.connection_registry.register(conn_info);

            let mut record = TrafficRecord::new(req_id.clone(), method.to_string(), url.clone());
            record.status = 200;
            record.protocol = "socks5-http".to_string();
            record.host = target_host.to_string();
            record.is_tunnel = true;
            record.client_ip = peer_addr.ip().to_string();
            record.client_app = client_app;
            record.client_pid = client_pid;
            record.client_path = client_path;

            let body_start = request_str
                .find("\r\n\r\n")
                .map(|i| i + 4)
                .or_else(|| request_str.find("\n\n").map(|i| i + 2));
            if let Some(body_offset) = body_start {
                if body_offset < request_data.len() {
                    let body_data = &request_data[body_offset..];
                    record.request_body_ref =
                        store_request_body(&admin_state, &req_id, body_data, None);
                }
            }

            record.has_rule_hit = has_rules;
            record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
            state.record_traffic(record);
            state.connection_monitor.register_connection(&req_id);

            info!(
                "[{}] SOCKS5 HTTP intercept established to {}:{} (from {})",
                req_id, target_host, target_port, peer_addr
            );
        }

        if let Some(ref host_rule) = resolved_rules.host {
            let new_host = host_rule.split(':').next().unwrap_or(host_rule).to_string();
            let new_port: u16 = host_rule
                .split(':')
                .nth(1)
                .and_then(|p| p.parse().ok())
                .unwrap_or(target_port);

            debug!(
                "[{}] SOCKS5 HTTP: Redirecting to {}:{}",
                req_id, new_host, new_port
            );

            if let Some(ref state) = admin_state {
                let actual_url = format!("http://{}:{}{}", new_host, new_port, path);
                let actual_host = new_host.clone();
                state.update_traffic_by_id(&req_id, move |record| {
                    record.actual_url = Some(actual_url.clone());
                    record.actual_host = Some(actual_host.clone());
                });
            }

            drop(target_stream);
            target_stream = TcpStream::connect(format!("{}:{}", new_host, new_port)).await?;

            let modified_request = self.rewrite_http_host(request_data, target_host, &new_host)?;
            target_stream.write_all(&modified_request).await?;
        } else {
            target_stream.write_all(request_data).await?;
        }
        target_stream.flush().await?;

        let (mut client_read, mut client_write) = self.stream().split();
        let (mut target_read, mut target_write) = target_stream.into_split();

        let bytes_sent = Arc::new(AtomicU64::new(n as u64));
        let bytes_received = Arc::new(AtomicU64::new(0));
        let bytes_sent_clone = Arc::clone(&bytes_sent);
        let bytes_received_clone = Arc::clone(&bytes_received);
        let admin_state_send = admin_state.clone();
        let admin_state_recv = admin_state.clone();
        let req_id_send = req_id.clone();
        let req_id_recv = req_id.clone();

        let client_to_target = async move {
            let mut buf = vec![0u8; 8192];
            loop {
                let n = client_read.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                bytes_sent_clone.fetch_add(n as u64, Ordering::Relaxed);
                if let Some(ref state) = admin_state_send {
                    state
                        .metrics_collector
                        .add_bytes_sent_by_type(TrafficType::Socks5, n as u64);
                    state.connection_monitor.update_traffic(
                        &req_id_send,
                        FrameDirection::Send,
                        n as u64,
                    );
                }
                target_write.write_all(&buf[..n]).await?;
                target_write.flush().await?;
            }
            target_write.shutdown().await?;
            Ok::<_, std::io::Error>(())
        };

        let target_to_client = async move {
            let mut buf = vec![0u8; 8192];
            loop {
                let n = target_read.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                bytes_received_clone.fetch_add(n as u64, Ordering::Relaxed);
                if let Some(ref state) = admin_state_recv {
                    state
                        .metrics_collector
                        .add_bytes_received_by_type(TrafficType::Socks5, n as u64);
                    state.connection_monitor.update_traffic(
                        &req_id_recv,
                        FrameDirection::Receive,
                        n as u64,
                    );
                }
                client_write.write_all(&buf[..n]).await?;
                client_write.flush().await?;
            }
            Ok::<_, std::io::Error>(())
        };

        let relay_future = async { tokio::try_join!(client_to_target, target_to_client) };

        let (relay_result, was_cancelled) = tokio::select! {
            res = relay_future => (res, false),
            _ = cancel_rx => (Ok(((), ())), true),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let total_sent = bytes_sent.load(Ordering::Relaxed);
        let total_received = bytes_received.load(Ordering::Relaxed);

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .decrement_connections_by_type(TrafficType::Socks5);

            state.connection_registry.unregister(&req_id);
            state.connection_monitor.unregister_connection(&req_id);

            state.update_traffic_by_id(&req_id, move |record| {
                record.request_size = total_sent as usize;
                record.response_size = total_received as usize;
                record.duration_ms = duration_ms;
            });

            if was_cancelled {
                info!(
                    "[{}] SOCKS5 HTTP intercept {}:{} closed due to config change",
                    req_id, target_host, target_port
                );
            } else {
                debug!(
                    "[{}] SOCKS5 HTTP: sent={} bytes, received={} bytes, duration={}ms",
                    req_id, total_sent, total_received, duration_ms
                );
            }
        }

        relay_result
            .map(|_| ())
            .map_err(|e| BifrostError::Network(e.to_string()))
    }

    fn rewrite_http_host(&self, request: &[u8], old_host: &str, new_host: &str) -> Result<Vec<u8>> {
        let request_str = String::from_utf8_lossy(request);
        let modified = request_str.replace(
            &format!("Host: {}", old_host),
            &format!("Host: {}", new_host),
        );
        Ok(modified.into_bytes())
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_socks5_intercepted_request(
    req: Request<Incoming>,
    target_host: &str,
    target_port: u16,
    original_host: &str,
    rules: Option<Arc<dyn RulesResolver>>,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
    max_body_buffer_size: usize,
    max_body_probe_size: usize,
    verbose_logging: bool,
    unsafe_ssl: bool,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    let method = req.method().to_string();
    let original_uri = req.uri().clone();
    let path = original_uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let full_url = format!("https://{}{}", original_host, path);

    debug!(
        "SOCKS5 TLS intercepted: {} {} (target: {}:{})",
        method, full_url, target_host, target_port
    );

    let rules_resolver: Arc<dyn RulesResolver> =
        rules.clone().unwrap_or_else(|| Arc::new(NoOpRulesResolver));

    let resolved = rules_resolver.resolve(&full_url, &method);

    let request_url = if resolved.ignored.host {
        full_url.clone()
    } else if let Some(ref host_rule) = resolved.host {
        let host_rule_clean = host_rule.trim_end_matches('/');
        let (h, p) = if let Some(idx) = host_rule_clean.find(':') {
            let host_part = &host_rule_clean[..idx];
            let port_part = host_rule_clean[idx + 1..]
                .parse::<u16>()
                .unwrap_or(target_port);
            (host_part.to_string(), port_part)
        } else {
            let default_port = match resolved.host_protocol {
                Some(bifrost_core::Protocol::Http) | Some(bifrost_core::Protocol::Ws) => 80,
                _ => 443,
            };
            (host_rule_clean.to_string(), default_port)
        };

        let target_protocol = resolved
            .host_protocol
            .unwrap_or(bifrost_core::Protocol::Https);
        let use_http = match target_protocol {
            bifrost_core::Protocol::Http | bifrost_core::Protocol::Ws => true,
            bifrost_core::Protocol::Host | bifrost_core::Protocol::XHost => p != 443 && p != 8443,
            _ => false,
        };
        if use_http {
            if p == 80 {
                format!("http://{}{}", h, path)
            } else {
                format!("http://{}:{}{}", h, p, path)
            }
        } else if p == 443 {
            format!("https://{}{}", h, path)
        } else {
            format!("https://{}:{}{}", h, p, path)
        }
    } else if target_port == 443 {
        format!("https://{}{}", target_host, path)
    } else {
        format!("https://{}:{}{}", target_host, target_port, path)
    };

    debug!(
        "SOCKS5 TLS: Forwarding to {} (ignored.host: {})",
        request_url, resolved.ignored.host
    );

    let new_uri: Uri = request_url.parse().unwrap_or_else(|_| original_uri.clone());

    let (parts, body) = req.into_parts();
    let mut new_req = Request::from_parts(parts, body);
    *new_req.uri_mut() = new_uri;

    let client_process = resolve_client_process_async_for_connection(&peer_addr, &local_addr).await;
    let (client_app, client_pid, client_path) = client_process
        .as_ref()
        .map(|p| (Some(p.name.clone()), Some(p.pid), p.path.clone()))
        .unwrap_or((None, None, None));

    let ctx = RequestContext::new()
        .with_client_ip(peer_addr.ip().to_string())
        .with_client_process(client_app, client_pid, client_path)
        .with_request_info(
            full_url.clone(),
            method.clone(),
            original_host.to_string(),
            path.to_string(),
            String::new(),
            peer_addr.ip().to_string(),
        );

    match handle_http_request(
        new_req,
        rules_resolver,
        verbose_logging,
        unsafe_ssl,
        max_body_buffer_size,
        max_body_probe_size,
        &ctx,
        admin_state,
        dns_resolver,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(e) => {
            error!("SOCKS5 TLS: Request handling failed: {}", e);
            Ok(Response::builder()
                .status(502)
                .body(full_body(format!("Request handling failed: {}", e)))
                .unwrap())
        }
    }
}

pub async fn parse_socks5_handshake(data: &[u8]) -> Result<(u8, Vec<AuthMethod>)> {
    if data.len() < 2 {
        return Err(BifrostError::Parse(
            "Insufficient data for SOCKS5 handshake".to_string(),
        ));
    }

    let version = data[0];
    let nmethods = data[1] as usize;

    if data.len() < 2 + nmethods {
        return Err(BifrostError::Parse(
            "Insufficient data for auth methods".to_string(),
        ));
    }

    let methods: Vec<AuthMethod> = data[2..2 + nmethods]
        .iter()
        .map(|&m| AuthMethod::from(m))
        .collect();

    Ok((version, methods))
}

pub fn build_handshake_response(method: AuthMethod) -> Vec<u8> {
    vec![SOCKS5_VERSION, method as u8]
}

pub fn build_reply(reply: SocksReply, addr: Option<&SocksAddress>, port: u16) -> Vec<u8> {
    let mut response = vec![SOCKS5_VERSION, reply as u8, 0x00];

    if let Some(address) = addr {
        response.extend(address.to_bytes());
    } else {
        response.push(AddressType::IPv4 as u8);
        response.extend_from_slice(&[0, 0, 0, 0]);
    }

    response.extend_from_slice(&port.to_be_bytes());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_method_from_u8() {
        assert_eq!(AuthMethod::from(0x00), AuthMethod::NoAuth);
        assert_eq!(AuthMethod::from(0x01), AuthMethod::GssApi);
        assert_eq!(AuthMethod::from(0x02), AuthMethod::UsernamePassword);
        assert_eq!(AuthMethod::from(0xFF), AuthMethod::NoAcceptable);
        assert_eq!(AuthMethod::from(0x10), AuthMethod::NoAcceptable);
    }

    #[test]
    fn test_socks_command_try_from() {
        assert_eq!(SocksCommand::try_from(0x01).unwrap(), SocksCommand::Connect);
        assert_eq!(SocksCommand::try_from(0x02).unwrap(), SocksCommand::Bind);
        assert_eq!(
            SocksCommand::try_from(0x03).unwrap(),
            SocksCommand::UdpAssociate
        );
        assert!(SocksCommand::try_from(0x04).is_err());
        assert!(SocksCommand::try_from(0x00).is_err());
    }

    #[test]
    fn test_address_type_try_from() {
        assert_eq!(AddressType::try_from(0x01).unwrap(), AddressType::IPv4);
        assert_eq!(
            AddressType::try_from(0x03).unwrap(),
            AddressType::DomainName
        );
        assert_eq!(AddressType::try_from(0x04).unwrap(), AddressType::IPv6);
        assert!(AddressType::try_from(0x02).is_err());
        assert!(AddressType::try_from(0x05).is_err());
    }

    #[test]
    fn test_socks_address_ipv4_to_bytes() {
        let addr = SocksAddress::IPv4(Ipv4Addr::new(192, 168, 1, 1));
        let bytes = addr.to_bytes();
        assert_eq!(bytes, vec![0x01, 192, 168, 1, 1]);
    }

    #[test]
    fn test_socks_address_ipv6_to_bytes() {
        let addr = SocksAddress::IPv6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));
        let bytes = addr.to_bytes();
        assert_eq!(bytes.len(), 17);
        assert_eq!(bytes[0], 0x04);
    }

    #[test]
    fn test_socks_address_domain_to_bytes() {
        let addr = SocksAddress::DomainName("example.com".to_string());
        let bytes = addr.to_bytes();
        assert_eq!(bytes[0], 0x03);
        assert_eq!(bytes[1], 11);
        assert_eq!(&bytes[2..], b"example.com");
    }

    #[test]
    fn test_socks_config_default() {
        let config = SocksConfig::default();
        assert_eq!(config.port, 1080);
        assert_eq!(config.host, "127.0.0.1");
        assert!(!config.auth_required);
        assert!(config.username.is_none());
        assert!(config.password.is_none());
        assert_eq!(config.timeout_secs, 30);
        assert!(config.enable_udp);
        assert!(config.udp_port.is_none());
    }

    #[test]
    fn test_socks_server_new() {
        let config = SocksConfig::default();
        let server = SocksServer::new(config);
        assert_eq!(server.config().port, 1080);
        assert_eq!(server.config().host, "127.0.0.1");
    }

    #[test]
    fn test_socks_server_custom_config() {
        let config = SocksConfig {
            port: 9050,
            host: "0.0.0.0".to_string(),
            auth_required: true,
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
            timeout_secs: 60,
            access_mode: AccessMode::Whitelist,
            client_whitelist: vec!["10.0.0.0/8".to_string()],
            allow_lan: true,
            enable_udp: true,
            udp_port: Some(1081),
        };
        let server = SocksServer::new(config);
        assert_eq!(server.config().port, 9050);
        assert_eq!(server.config().host, "0.0.0.0");
        assert!(server.config().auth_required);
        assert_eq!(server.config().access_mode, AccessMode::Whitelist);
        assert!(server.config().allow_lan);
        assert!(server.config().enable_udp);
        assert_eq!(server.config().udp_port, Some(1081));
    }

    #[tokio::test]
    async fn test_parse_socks5_handshake_no_auth() {
        let data = [0x05, 0x01, 0x00];
        let (version, methods) = parse_socks5_handshake(&data).await.unwrap();
        assert_eq!(version, 0x05);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0], AuthMethod::NoAuth);
    }

    #[tokio::test]
    async fn test_parse_socks5_handshake_multiple_methods() {
        let data = [0x05, 0x03, 0x00, 0x01, 0x02];
        let (version, methods) = parse_socks5_handshake(&data).await.unwrap();
        assert_eq!(version, 0x05);
        assert_eq!(methods.len(), 3);
        assert_eq!(methods[0], AuthMethod::NoAuth);
        assert_eq!(methods[1], AuthMethod::GssApi);
        assert_eq!(methods[2], AuthMethod::UsernamePassword);
    }

    #[tokio::test]
    async fn test_parse_socks5_handshake_insufficient_data() {
        let data = [0x05];
        let result = parse_socks5_handshake(&data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parse_socks5_handshake_insufficient_methods() {
        let data = [0x05, 0x03, 0x00];
        let result = parse_socks5_handshake(&data).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_build_handshake_response() {
        let response = build_handshake_response(AuthMethod::NoAuth);
        assert_eq!(response, vec![0x05, 0x00]);

        let response = build_handshake_response(AuthMethod::UsernamePassword);
        assert_eq!(response, vec![0x05, 0x02]);

        let response = build_handshake_response(AuthMethod::NoAcceptable);
        assert_eq!(response, vec![0x05, 0xFF]);
    }

    #[test]
    fn test_build_reply_success() {
        let addr = SocksAddress::IPv4(Ipv4Addr::new(127, 0, 0, 1));
        let reply = build_reply(SocksReply::Succeeded, Some(&addr), 8080);
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x00);
        assert_eq!(reply[2], 0x00);
        assert_eq!(reply[3], 0x01);
        assert_eq!(&reply[4..8], &[127, 0, 0, 1]);
        assert_eq!(&reply[8..10], &[0x1F, 0x90]);
    }

    #[test]
    fn test_build_reply_failure() {
        let reply = build_reply(SocksReply::ConnectionRefused, None, 0);
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x05);
        assert_eq!(reply[2], 0x00);
        assert_eq!(reply[3], 0x01);
        assert_eq!(&reply[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_build_reply_with_domain() {
        let addr = SocksAddress::DomainName("test.com".to_string());
        let reply = build_reply(SocksReply::Succeeded, Some(&addr), 443);
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x00);
        assert_eq!(reply[2], 0x00);
        assert_eq!(reply[3], 0x03);
        assert_eq!(reply[4], 8);
        assert_eq!(&reply[5..13], b"test.com");
    }

    #[test]
    fn test_build_reply_with_ipv6() {
        let addr = SocksAddress::IPv6(Ipv6Addr::LOCALHOST);
        let reply = build_reply(SocksReply::Succeeded, Some(&addr), 8080);
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x00);
        assert_eq!(reply[2], 0x00);
        assert_eq!(reply[3], 0x04);
        assert_eq!(reply.len(), 22);
    }

    #[test]
    fn test_socks_reply_values() {
        assert_eq!(SocksReply::Succeeded as u8, 0x00);
        assert_eq!(SocksReply::GeneralFailure as u8, 0x01);
        assert_eq!(SocksReply::ConnectionNotAllowed as u8, 0x02);
        assert_eq!(SocksReply::NetworkUnreachable as u8, 0x03);
        assert_eq!(SocksReply::HostUnreachable as u8, 0x04);
        assert_eq!(SocksReply::ConnectionRefused as u8, 0x05);
        assert_eq!(SocksReply::TtlExpired as u8, 0x06);
        assert_eq!(SocksReply::CommandNotSupported as u8, 0x07);
        assert_eq!(SocksReply::AddressTypeNotSupported as u8, 0x08);
    }

    #[test]
    fn test_socks_config_with_auth() {
        let config = SocksConfig {
            port: 1080,
            host: "127.0.0.1".to_string(),
            auth_required: true,
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
            timeout_secs: 30,
            access_mode: AccessMode::LocalOnly,
            client_whitelist: Vec::new(),
            allow_lan: false,
            enable_udp: false,
            udp_port: None,
        };
        assert!(config.auth_required);
        assert_eq!(config.username, Some("admin".to_string()));
        assert_eq!(config.password, Some("secret".to_string()));
    }

    struct MockSocksHandler {
        config: SocksConfig,
    }

    impl MockSocksHandler {
        fn select_auth_method(&self, methods: &[u8]) -> AuthMethod {
            if self.config.auth_required {
                if methods.contains(&(AuthMethod::UsernamePassword as u8)) {
                    return AuthMethod::UsernamePassword;
                }
            } else if methods.contains(&(AuthMethod::NoAuth as u8)) {
                return AuthMethod::NoAuth;
            }

            if methods.contains(&(AuthMethod::UsernamePassword as u8))
                && self.config.username.is_some()
                && self.config.password.is_some()
            {
                return AuthMethod::UsernamePassword;
            }

            if methods.contains(&(AuthMethod::NoAuth as u8)) && !self.config.auth_required {
                return AuthMethod::NoAuth;
            }

            AuthMethod::NoAcceptable
        }

        fn verify_credentials(&self, username: &str, password: &str) -> bool {
            match (&self.config.username, &self.config.password) {
                (Some(expected_user), Some(expected_pass)) => {
                    username == expected_user && password == expected_pass
                }
                _ => false,
            }
        }
    }

    #[test]
    fn test_select_auth_method_no_auth_required() {
        let handler = MockSocksHandler {
            config: SocksConfig::default(),
        };
        let methods = vec![0x00, 0x02];
        assert_eq!(handler.select_auth_method(&methods), AuthMethod::NoAuth);
    }

    #[test]
    fn test_select_auth_method_auth_required() {
        let handler = MockSocksHandler {
            config: SocksConfig {
                auth_required: true,
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                ..Default::default()
            },
        };
        let methods = vec![0x00, 0x02];
        assert_eq!(
            handler.select_auth_method(&methods),
            AuthMethod::UsernamePassword
        );
    }

    #[test]
    fn test_select_auth_method_no_acceptable() {
        let handler = MockSocksHandler {
            config: SocksConfig {
                auth_required: true,
                username: None,
                password: None,
                ..Default::default()
            },
        };
        let methods = vec![0x00];
        assert_eq!(
            handler.select_auth_method(&methods),
            AuthMethod::NoAcceptable
        );
    }

    #[test]
    fn test_verify_credentials_success() {
        let handler = MockSocksHandler {
            config: SocksConfig {
                username: Some("admin".to_string()),
                password: Some("secret".to_string()),
                ..Default::default()
            },
        };
        assert!(handler.verify_credentials("admin", "secret"));
    }

    #[test]
    fn test_verify_credentials_wrong_username() {
        let handler = MockSocksHandler {
            config: SocksConfig {
                username: Some("admin".to_string()),
                password: Some("secret".to_string()),
                ..Default::default()
            },
        };
        assert!(!handler.verify_credentials("wrong", "secret"));
    }

    #[test]
    fn test_verify_credentials_wrong_password() {
        let handler = MockSocksHandler {
            config: SocksConfig {
                username: Some("admin".to_string()),
                password: Some("secret".to_string()),
                ..Default::default()
            },
        };
        assert!(!handler.verify_credentials("admin", "wrong"));
    }

    #[test]
    fn test_verify_credentials_no_config() {
        let handler = MockSocksHandler {
            config: SocksConfig::default(),
        };
        assert!(!handler.verify_credentials("admin", "secret"));
    }

    #[test]
    fn test_constants() {
        assert_eq!(SOCKS5_VERSION, 0x05);
    }
}
