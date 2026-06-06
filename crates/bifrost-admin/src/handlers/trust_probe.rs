use std::cmp::Reverse;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bifrost_core::AccessDecision;
use bifrost_tls::{init_crypto_provider, load_root_ca, DynamicCertGenerator};
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use qrcode::render::svg;
use qrcode::QrCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use uuid::Uuid;

use super::{
    cors_preflight, error_response, full_body, json_response, json_response_with_status,
    method_not_allowed, public_response_builder, BoxBody,
};
use crate::network;
use crate::push::{SharedPushManager, SETTINGS_SCOPE_TRUST_PROBE};
use crate::state::SharedAdminState;

static TRUST_PROBE_MANAGER: Lazy<TrustProbeManager> = Lazy::new(TrustProbeManager::new);

const DEFAULT_TTL_SECONDS: i64 = 600;
const MAX_TTL_SECONDS: i64 = 1800;
const MAX_ACTIVE_SESSIONS: usize = 32;
const TOKEN_QUERY_KEY: &str = "t";
pub const TRUST_PROBE_PROXY_CONFIG_HOST: &str = "bifrost-proxy-check.invalid";
pub const TRUST_PROBE_PROXY_CONFIG_PATH: &str = "/_bifrost/trust-probe/proxy-configured";

pub fn list_active_sessions() -> Vec<TrustProbeSessionView> {
    TRUST_PROBE_MANAGER.list_sessions()
}

fn broadcast_trust_probe_update(push_manager: Option<&SharedPushManager>) {
    let Some(push_manager) = push_manager.cloned() else {
        return;
    };
    tokio::spawn(async move {
        push_manager
            .broadcast_settings_scope(SETTINGS_SCOPE_TRUST_PROBE)
            .await;
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustProbeStatus {
    Created,
    PageOpened,
    NetworkReachable,
    TlsTrusted,
    TlsFailed,
    NetworkFailed,
    Expired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustProbeEvent {
    #[serde(rename = "type")]
    event_type: String,
    at: DateTime<Utc>,
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustProbeDeviceView {
    device_id: String,
    status: TrustProbeStatus,
    opened: bool,
    proxy_access_status: Option<TrustProbeProxyAccessStatus>,
    proxy_access_allowed: Option<bool>,
    proxy_access_message: Option<String>,
    proxy_configured: bool,
    proxy_configuration_message: Option<String>,
    network_reachable: bool,
    tls_trusted: bool,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    last_error: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    events: Vec<TrustProbeEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustProbeSessionView {
    session_id: String,
    status: TrustProbeStatus,
    opened: bool,
    proxy_access_status: Option<TrustProbeProxyAccessStatus>,
    proxy_access_allowed: Option<bool>,
    proxy_access_message: Option<String>,
    proxy_configured: bool,
    proxy_configuration_message: Option<String>,
    suggested_wifi_ssid: Option<String>,
    suggested_wifi_ssid_message: Option<String>,
    network_reachable: bool,
    tls_trusted: bool,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    last_error: Option<String>,
    devices: Vec<TrustProbeDeviceView>,
    events: Vec<TrustProbeEvent>,
    expires_at: DateTime<Utc>,
    host: String,
    admin_port: u16,
    probe_port: u16,
    landing_url: String,
    qr_code_url: String,
    ca_download_url: String,
    proxy_qr_code_url: String,
    ca_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTrustProbeSessionRequest {
    host: String,
    ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustProbeReport {
    #[serde(rename = "type")]
    event_type: String,
    message: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    status: Option<u16>,
    #[serde(alias = "wifiSsid")]
    wifi_ssid: Option<String>,
    #[serde(alias = "deviceId")]
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTrustProbeSessionRequest {
    wifi_ssid: Option<String>,
}

struct TrustProbeEventInput {
    event_type: String,
    message: Option<String>,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    device_id: Option<String>,
}

#[derive(Debug, Clone)]
struct TrustProbeDeviceState {
    device_id: String,
    status: TrustProbeStatus,
    opened: bool,
    proxy_access_status: Option<TrustProbeProxyAccessStatus>,
    proxy_access_allowed: Option<bool>,
    proxy_access_message: Option<String>,
    proxy_configured: bool,
    proxy_configuration_message: Option<String>,
    network_reachable: bool,
    tls_trusted: bool,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    last_error: Option<String>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    events: Vec<TrustProbeEvent>,
}

#[derive(Debug, Clone)]
struct TrustProbeSession {
    id: Uuid,
    token_hash: String,
    host: String,
    admin_port: u16,
    probe_port: u16,
    ca_fingerprint_sha256: Option<String>,
    status: TrustProbeStatus,
    opened: bool,
    proxy_access_status: Option<TrustProbeProxyAccessStatus>,
    proxy_access_allowed: Option<bool>,
    proxy_access_message: Option<String>,
    proxy_configured: bool,
    proxy_configuration_message: Option<String>,
    suggested_wifi_ssid: Option<String>,
    suggested_wifi_ssid_message: Option<String>,
    network_reachable: bool,
    tls_trusted: bool,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    last_error: Option<String>,
    devices: HashMap<String, TrustProbeDeviceState>,
    events: Vec<TrustProbeEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustProbeProxyAccessStatus {
    Allowed,
    Pending,
    Denied,
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrustProbeProxyAccessView {
    status: TrustProbeProxyAccessStatus,
    authorized: bool,
    client_ip: Option<String>,
    message: String,
}

#[derive(Debug)]
struct ProbeServerHandle {
    host: String,
    ca_fingerprint_sha256: Option<String>,
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

#[derive(Debug)]
pub struct TrustProbeManager {
    sessions: Mutex<HashMap<Uuid, TrustProbeSession>>,
    server: Mutex<Option<ProbeServerHandle>>,
}

impl TrustProbeManager {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            server: Mutex::new(None),
        }
    }

    async fn create_session(
        &self,
        state: &SharedAdminState,
        request: CreateTrustProbeSessionRequest,
    ) -> Result<TrustProbeSessionView, String> {
        self.cleanup_expired_sessions();
        let host = validate_probe_host(&request.host)?;
        let ttl_seconds = request
            .ttl_seconds
            .unwrap_or(DEFAULT_TTL_SECONDS)
            .clamp(60, MAX_TTL_SECONDS);
        let admin_port = state.port();
        let ca_cert_path = state
            .ca_cert_path
            .as_ref()
            .filter(|path| path.exists())
            .cloned()
            .ok_or_else(|| "CA certificate is not configured.".to_string())?;
        let ca_key_path = ca_key_path_from_cert_path(&ca_cert_path);
        if !ca_key_path.exists() {
            return Err("CA private key is not configured, so the trust probe cannot sign its HTTPS certificate.".to_string());
        }
        let ca_fingerprint_sha256 = certificate_sha256_fingerprint(&ca_cert_path);
        {
            let sessions = self.sessions.lock();
            if let Some(session) = sessions
                .values()
                .filter(|session| {
                    !session.is_expired()
                        && session.host == host
                        && session.admin_port == admin_port
                        && session.ca_fingerprint_sha256 == ca_fingerprint_sha256
                })
                .max_by_key(|session| session.created_at)
            {
                return Ok(session.to_view(""));
            }
        }
        let probe_port = self
            .ensure_probe_server(
                &host,
                admin_port.saturating_add(2),
                &ca_cert_path,
                &ca_key_path,
                ca_fingerprint_sha256.clone(),
            )
            .await?;

        let id = Uuid::new_v4();
        let token = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());
        let token_hash = hash_token(&token);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl_seconds);
        let wifi_detection = current_wifi_ssid_detection();
        let session = TrustProbeSession {
            id,
            token_hash,
            host,
            admin_port,
            probe_port,
            ca_fingerprint_sha256,
            status: TrustProbeStatus::Created,
            opened: false,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: wifi_detection.ssid,
            suggested_wifi_ssid_message: wifi_detection.message,
            network_reachable: false,
            tls_trusted: false,
            created_at: now,
            expires_at,
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: vec![TrustProbeEvent {
                event_type: "created".to_string(),
                at: now,
                message: None,
            }],
        };

        let mut sessions = self.sessions.lock();
        if sessions.len() >= MAX_ACTIVE_SESSIONS {
            if let Some(oldest_id) = sessions
                .values()
                .min_by_key(|session| session.created_at)
                .map(|session| session.id)
            {
                sessions.remove(&oldest_id);
            }
        }
        sessions.insert(id, session);
        let session = sessions
            .get(&id)
            .expect("newly inserted trust probe session");
        Ok(session.to_view(&token))
    }

    async fn get_or_create_public_session(
        &self,
        state: &SharedAdminState,
        host: &str,
    ) -> Result<TrustProbeSessionView, String> {
        self.cleanup_expired_sessions();
        let host = validate_probe_host(host)?;
        {
            let sessions = self.sessions.lock();
            if let Some(session) = sessions
                .values()
                .filter(|session| !session.is_expired() && session.host == host)
                .max_by_key(|session| session.created_at)
            {
                return Ok(session.to_view(""));
            }
        }
        self.create_session(
            state,
            CreateTrustProbeSessionRequest {
                host,
                ttl_seconds: Some(MAX_TTL_SECONDS),
            },
        )
        .await
    }

    fn get_session(&self, session_id: Uuid) -> Option<TrustProbeSessionView> {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
        sessions.get(&session_id).map(|session| session.to_view(""))
    }

    fn list_sessions(&self) -> Vec<TrustProbeSessionView> {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
        let mut views: Vec<_> = sessions
            .values()
            .filter(|session| !session.is_expired())
            .map(|session| session.to_view(""))
            .collect();
        views.sort_by_key(|session| Reverse(session.expires_at));
        views
    }

    fn get_public_session(&self, session_id: Uuid, token: &str) -> Option<TrustProbeSessionView> {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
        let session = sessions.get(&session_id)?;
        if !session.token_matches(token) || session.is_expired() {
            return None;
        }
        Some(session.to_view(token))
    }

    fn update_session(
        &self,
        session_id: Uuid,
        request: UpdateTrustProbeSessionRequest,
    ) -> Option<TrustProbeSessionView> {
        self.cleanup_expired_sessions();
        let mut sessions = self.sessions.lock();
        if sessions
            .get(&session_id)
            .map(|session| session.is_expired())
            .unwrap_or(true)
        {
            return None;
        }
        if let Some(ssid) = request.wifi_ssid {
            Self::apply_wifi_ssid_to_active_sessions(&mut sessions, ssid);
        }
        sessions.get(&session_id).map(|session| session.to_view(""))
    }

    fn render_landing_page(&self, session_id: Uuid, token: &str) -> Option<String> {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
        let session = sessions.get(&session_id)?;
        if !session.token_matches(token) || session.is_expired() {
            return None;
        }
        Some(render_landing_page(session, token))
    }

    fn record_report(
        &self,
        session_id: Uuid,
        token: &str,
        report: TrustProbeReport,
        client_ip: Option<String>,
        user_agent_header: Option<String>,
    ) -> bool {
        let TrustProbeReport {
            event_type,
            message,
            user_agent,
            platform_hint,
            status,
            wifi_ssid,
            device_id,
        } = report;
        let recorded = self.record_event(
            session_id,
            token,
            TrustProbeEventInput {
                event_type,
                message: message.or_else(|| {
                    status.map(|status| format!("Probe request returned HTTP {status}"))
                }),
                client_ip,
                user_agent: user_agent.or(user_agent_header),
                platform_hint,
                device_id,
            },
        );
        if recorded {
            if let Some(ssid) = wifi_ssid {
                let mut sessions = self.sessions.lock();
                Self::apply_wifi_ssid_to_active_sessions(&mut sessions, ssid);
            }
        }
        recorded
    }

    fn record_event(&self, session_id: Uuid, token: &str, input: TrustProbeEventInput) -> bool {
        self.cleanup_expired_sessions();
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get_mut(&session_id) else {
            return false;
        };
        if !session.token_matches(token) || session.is_expired() {
            return false;
        }
        session.apply_event(
            &input.event_type,
            input.message,
            input.client_ip,
            input.user_agent,
            input.platform_hint,
            input.device_id,
        );
        true
    }

    fn record_proxy_access(
        &self,
        session_id: Uuid,
        token: &str,
        status: TrustProbeProxyAccessStatus,
        client_ip: Option<String>,
        message: String,
        device_id: Option<String>,
    ) -> bool {
        self.cleanup_expired_sessions();
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get_mut(&session_id) else {
            return false;
        };
        if !session.token_matches(token) || session.is_expired() {
            return false;
        }
        session.apply_proxy_access(status, client_ip, message, device_id);
        true
    }

    async fn ensure_probe_server(
        &self,
        host: &str,
        preferred_port: u16,
        ca_cert_path: &Path,
        ca_key_path: &Path,
        ca_fingerprint_sha256: Option<String>,
    ) -> Result<u16, String> {
        {
            let server = self.server.lock();
            if let Some(server) = server.as_ref() {
                if server.host == host && server.ca_fingerprint_sha256 == ca_fingerprint_sha256 {
                    return Ok(server.port);
                }
            }
        }

        let old = self.server.lock().take();
        if let Some(mut old) = old {
            if let Some(tx) = old.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }

        init_crypto_provider();
        let ca = load_root_ca(ca_cert_path, ca_key_path)
            .map_err(|error| format!("Failed to load Bifrost CA for trust probe: {error}"))?;
        let generator = DynamicCertGenerator::new(Arc::new(ca));
        let certified_key = generator
            .generate_for_domain(host)
            .map_err(|error| format!("Failed to generate trust probe certificate: {error}"))?;
        let server_config = bifrost_tls::TlsConfig::build_server_config(&certified_key)
            .map_err(|error| format!("Failed to build trust probe TLS config: {error}"))?;

        let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, preferred_port)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
                .await
                .map_err(|error| format!("Failed to bind trust probe port: {error}"))?,
        };
        let port = listener
            .local_addr()
            .map_err(|error| format!("Failed to inspect trust probe port: {error}"))?
            .port();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(run_probe_server(listener, server_config, shutdown_rx));
        *self.server.lock() = Some(ProbeServerHandle {
            host: host.to_string(),
            ca_fingerprint_sha256,
            port,
            shutdown_tx: Some(shutdown_tx),
        });
        Ok(port)
    }

    fn cleanup_expired_sessions(&self) {
        let now = Utc::now();
        let has_active_session = {
            let mut sessions = self.sessions.lock();
            for session in sessions.values_mut() {
                if session.expires_at <= now && session.status != TrustProbeStatus::Expired {
                    session.status = TrustProbeStatus::Expired;
                    session.last_error = Some("Availability check session expired.".to_string());
                    session.events.push(TrustProbeEvent {
                        event_type: "expired".to_string(),
                        at: now,
                        message: None,
                    });
                }
            }
            sessions.values().any(|session| session.expires_at > now)
        };
        if !has_active_session {
            self.stop_probe_server();
        }
    }

    fn stop_probe_server(&self) {
        let old = self.server.lock().take();
        if let Some(mut old) = old {
            if let Some(tx) = old.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    fn apply_wifi_ssid_to_active_sessions(
        sessions: &mut HashMap<Uuid, TrustProbeSession>,
        ssid: String,
    ) {
        let ssid = ssid.trim();
        if ssid.is_empty() || ssid.len() > 128 {
            return;
        }
        for session in sessions
            .values_mut()
            .filter(|session| !session.is_expired())
        {
            session.apply_wifi_ssid(ssid.to_string());
        }
    }
}

impl TrustProbeSession {
    fn token_matches(&self, token: &str) -> bool {
        token.is_empty() || self.token_hash == hash_token(token)
    }

    fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    fn to_view(&self, _token: &str) -> TrustProbeSessionView {
        let mut devices: Vec<_> = self
            .devices
            .values()
            .map(TrustProbeDeviceState::to_view)
            .collect();
        devices.sort_by_key(|device| Reverse(device.last_seen));
        TrustProbeSessionView {
            session_id: self.id.to_string(),
            status: if self.is_expired() {
                TrustProbeStatus::Expired
            } else {
                self.status
            },
            opened: self.opened,
            proxy_access_status: self.proxy_access_status,
            proxy_access_allowed: self.proxy_access_allowed,
            proxy_access_message: self.proxy_access_message.clone(),
            proxy_configured: self.proxy_configured,
            proxy_configuration_message: self.proxy_configuration_message.clone(),
            suggested_wifi_ssid: self.suggested_wifi_ssid.clone(),
            suggested_wifi_ssid_message: self.suggested_wifi_ssid_message.clone(),
            network_reachable: self.network_reachable,
            tls_trusted: self.tls_trusted,
            client_ip: self.client_ip.clone(),
            user_agent: self.user_agent.clone(),
            platform_hint: self.platform_hint.clone(),
            last_error: self.last_error.clone(),
            devices,
            events: self.events.clone(),
            expires_at: self.expires_at,
            host: self.host.clone(),
            admin_port: self.admin_port,
            probe_port: self.probe_port,
            landing_url: format!(
                "http://{}:{}/_bifrost/public/trust-probe",
                self.host, self.admin_port
            ),
            qr_code_url: format!(
                "http://{}:{}/_bifrost/public/trust-probe/qrcode?host={}",
                self.host,
                self.admin_port,
                urlencoding::encode(&self.host)
            ),
            ca_download_url: format!(
                "http://{}:{}/_bifrost/public/cert",
                self.host, self.admin_port
            ),
            proxy_qr_code_url: format!(
                "http://{}:{}/_bifrost/public/proxy/qrcode?ip={}",
                self.host,
                self.admin_port,
                urlencoding::encode(&self.host)
            ),
            ca_fingerprint_sha256: self.ca_fingerprint_sha256.clone(),
        }
    }

    fn apply_proxy_access(
        &mut self,
        status: TrustProbeProxyAccessStatus,
        client_ip: Option<String>,
        message: String,
        device_id: Option<String>,
    ) {
        if let Some(client_ip) = client_ip {
            self.client_ip = Some(client_ip);
        }
        self.proxy_access_status = Some(status);
        self.proxy_access_allowed = Some(matches!(status, TrustProbeProxyAccessStatus::Allowed));
        self.proxy_access_message = Some(message.clone());
        self.events.push(TrustProbeEvent {
            event_type: match status {
                TrustProbeProxyAccessStatus::Allowed => "proxy_access_allowed",
                TrustProbeProxyAccessStatus::Pending => "proxy_access_pending",
                TrustProbeProxyAccessStatus::Denied => "proxy_access_denied",
                TrustProbeProxyAccessStatus::Unavailable => "proxy_access_unavailable",
            }
            .to_string(),
            at: Utc::now(),
            message: Some(message),
        });
        let device_client_ip = self.client_ip.clone();
        let device_message = self.proxy_access_message.clone().unwrap_or_default();
        if let Some(device_id) = normalize_device_id(device_id) {
            self.device_mut(&device_id).apply_proxy_access(
                status,
                device_client_ip,
                device_message,
            );
        }
    }

    fn apply_event(
        &mut self,
        event_type: &str,
        message: Option<String>,
        client_ip: Option<String>,
        user_agent: Option<String>,
        platform_hint: Option<String>,
        device_id: Option<String>,
    ) {
        if let Some(client_ip) = client_ip {
            self.client_ip = Some(client_ip);
        }
        if let Some(user_agent) = user_agent.filter(|value| !value.trim().is_empty()) {
            self.user_agent = Some(user_agent);
        }
        if let Some(platform_hint) = platform_hint.filter(|value| !value.trim().is_empty()) {
            self.platform_hint = Some(platform_hint);
        }

        match event_type {
            "page_opened" => {
                self.opened = true;
                if !matches!(
                    self.status,
                    TrustProbeStatus::NetworkReachable
                        | TrustProbeStatus::TlsTrusted
                        | TrustProbeStatus::TlsFailed
                        | TrustProbeStatus::NetworkFailed
                ) {
                    self.status = TrustProbeStatus::PageOpened;
                }
            }
            "netcheck_ok" => {
                self.opened = true;
                self.network_reachable = true;
                if !matches!(
                    self.status,
                    TrustProbeStatus::TlsTrusted | TrustProbeStatus::TlsFailed
                ) {
                    self.status = TrustProbeStatus::NetworkReachable;
                }
            }
            "tls_ok" | "tls_check_ok" => {
                self.opened = true;
                self.network_reachable = true;
                self.tls_trusted = true;
                self.last_error = None;
                self.status = TrustProbeStatus::TlsTrusted;
            }
            "proxy_configured_ok" => {
                self.opened = true;
                self.proxy_configured = true;
                self.proxy_configuration_message = Some(
                    message
                        .clone()
                        .unwrap_or_else(|| "This browser is using the Bifrost proxy.".to_string()),
                );
            }
            "proxy_config_failed" => {
                self.opened = true;
                if !self.proxy_configured {
                    self.proxy_configuration_message = Some(message.clone().unwrap_or_else(|| {
                        "This browser is not using the Bifrost proxy yet.".to_string()
                    }));
                }
            }
            "network_failed" => {
                self.opened = true;
                if !self.tls_trusted {
                    self.status = TrustProbeStatus::NetworkFailed;
                    self.last_error = message.clone();
                }
            }
            "tls_failed" => {
                self.opened = true;
                self.network_reachable = true;
                if !self.tls_trusted {
                    self.status = TrustProbeStatus::TlsFailed;
                    self.last_error = message.clone();
                }
            }
            _ => {}
        }

        self.events.push(TrustProbeEvent {
            event_type: event_type.to_string(),
            at: Utc::now(),
            message: message.clone(),
        });
        if self.events.len() > 64 {
            self.events.remove(0);
        }
        let device_client_ip = self.client_ip.clone();
        let device_user_agent = self.user_agent.clone();
        let device_platform_hint = self.platform_hint.clone();
        if let Some(device_id) = normalize_device_id(device_id) {
            self.device_mut(&device_id).apply_event(
                event_type,
                message.clone(),
                device_client_ip,
                device_user_agent,
                device_platform_hint,
            );
        }
    }

    fn apply_wifi_ssid(&mut self, ssid: String) {
        let ssid = ssid.trim();
        if ssid.is_empty() || ssid.len() > 128 {
            return;
        }
        if self.suggested_wifi_ssid.as_deref() == Some(ssid) {
            return;
        }
        self.suggested_wifi_ssid = Some(ssid.to_string());
        self.suggested_wifi_ssid_message =
            Some("Wi-Fi name was provided by the user for this availability check.".to_string());
        self.events.push(TrustProbeEvent {
            event_type: "wifi_ssid_updated".to_string(),
            at: Utc::now(),
            message: Some("Wi-Fi name updated for iOS proxy profile generation.".to_string()),
        });
        if self.events.len() > 64 {
            self.events.remove(0);
        }
    }

    fn device_mut(&mut self, device_id: &str) -> &mut TrustProbeDeviceState {
        let now = Utc::now();
        self.devices
            .entry(device_id.to_string())
            .or_insert_with(|| TrustProbeDeviceState::new(device_id.to_string(), now))
    }
}

impl TrustProbeDeviceState {
    fn new(device_id: String, now: DateTime<Utc>) -> Self {
        Self {
            device_id,
            status: TrustProbeStatus::Created,
            opened: false,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            network_reachable: false,
            tls_trusted: false,
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            first_seen: now,
            last_seen: now,
            events: Vec::new(),
        }
    }

    fn to_view(&self) -> TrustProbeDeviceView {
        TrustProbeDeviceView {
            device_id: self.device_id.clone(),
            status: self.status,
            opened: self.opened,
            proxy_access_status: self.proxy_access_status,
            proxy_access_allowed: self.proxy_access_allowed,
            proxy_access_message: self.proxy_access_message.clone(),
            proxy_configured: self.proxy_configured,
            proxy_configuration_message: self.proxy_configuration_message.clone(),
            network_reachable: self.network_reachable,
            tls_trusted: self.tls_trusted,
            client_ip: self.client_ip.clone(),
            user_agent: self.user_agent.clone(),
            platform_hint: self.platform_hint.clone(),
            last_error: self.last_error.clone(),
            first_seen: self.first_seen,
            last_seen: self.last_seen,
            events: self.events.clone(),
        }
    }

    fn apply_proxy_access(
        &mut self,
        status: TrustProbeProxyAccessStatus,
        client_ip: Option<String>,
        message: String,
    ) {
        self.last_seen = Utc::now();
        self.opened = true;
        self.client_ip = client_ip.or_else(|| self.client_ip.clone());
        self.proxy_access_status = Some(status);
        self.proxy_access_allowed = Some(matches!(status, TrustProbeProxyAccessStatus::Allowed));
        self.proxy_access_message = Some(message.clone());
        self.push_event(
            match status {
                TrustProbeProxyAccessStatus::Allowed => "proxy_access_allowed",
                TrustProbeProxyAccessStatus::Pending => "proxy_access_pending",
                TrustProbeProxyAccessStatus::Denied => "proxy_access_denied",
                TrustProbeProxyAccessStatus::Unavailable => "proxy_access_unavailable",
            },
            Some(message),
        );
    }

    fn apply_event(
        &mut self,
        event_type: &str,
        message: Option<String>,
        client_ip: Option<String>,
        user_agent: Option<String>,
        platform_hint: Option<String>,
    ) {
        self.last_seen = Utc::now();
        self.client_ip = client_ip.or_else(|| self.client_ip.clone());
        self.user_agent = user_agent.or_else(|| self.user_agent.clone());
        self.platform_hint = platform_hint.or_else(|| self.platform_hint.clone());
        match event_type {
            "page_opened" => {
                self.opened = true;
                if !matches!(
                    self.status,
                    TrustProbeStatus::NetworkReachable
                        | TrustProbeStatus::TlsTrusted
                        | TrustProbeStatus::TlsFailed
                        | TrustProbeStatus::NetworkFailed
                ) {
                    self.status = TrustProbeStatus::PageOpened;
                }
            }
            "netcheck_ok" => {
                self.opened = true;
                self.network_reachable = true;
                if !matches!(
                    self.status,
                    TrustProbeStatus::TlsTrusted | TrustProbeStatus::TlsFailed
                ) {
                    self.status = TrustProbeStatus::NetworkReachable;
                }
            }
            "tls_ok" | "tls_check_ok" => {
                self.opened = true;
                self.network_reachable = true;
                self.tls_trusted = true;
                self.last_error = None;
                self.status = TrustProbeStatus::TlsTrusted;
            }
            "tls_failed" => {
                self.opened = true;
                self.network_reachable = true;
                if !self.tls_trusted {
                    self.status = TrustProbeStatus::TlsFailed;
                    self.last_error = message.clone();
                }
            }
            "network_failed" => {
                self.opened = true;
                if !self.tls_trusted {
                    self.status = TrustProbeStatus::NetworkFailed;
                    self.last_error = message.clone();
                }
            }
            "proxy_configured_ok" => {
                self.opened = true;
                self.proxy_configured = true;
                self.proxy_configuration_message = Some(
                    message
                        .clone()
                        .unwrap_or_else(|| "This browser is using the Bifrost proxy.".to_string()),
                );
            }
            "proxy_config_failed" => {
                self.opened = true;
                if !self.proxy_configured {
                    self.proxy_configuration_message = Some(message.clone().unwrap_or_else(|| {
                        "This browser is not using the Bifrost proxy yet.".to_string()
                    }));
                }
            }
            _ => {}
        }
        self.push_event(event_type, message);
    }

    fn push_event(&mut self, event_type: &str, message: Option<String>) {
        self.events.push(TrustProbeEvent {
            event_type: event_type.to_string(),
            at: Utc::now(),
            message,
        });
        if self.events.len() > 32 {
            self.events.remove(0);
        }
    }
}

pub async fn handle_trust_probe_proxy_configured_request(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
) -> Response<BoxBody> {
    if req.method() == Method::OPTIONS {
        return probe_json_response(StatusCode::NO_CONTENT, serde_json::json!({}));
    }
    if req.method() != Method::GET {
        return probe_json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            serde_json::json!({ "error": "method not allowed" }),
        );
    }
    let Some(session_id) =
        query_param(req.uri().query(), "sid").and_then(|value| value.parse::<Uuid>().ok())
    else {
        return probe_json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "missing sid" }),
        );
    };
    let token = query_param(req.uri().query(), TOKEN_QUERY_KEY).unwrap_or_default();
    let device_id = query_param(req.uri().query(), "deviceId");
    let user_agent = req
        .headers()
        .get(hyper::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let client_ip = Some(peer_addr.ip().to_string());
    let message = format!(
        "This browser reached Bifrost through the configured proxy from {}.",
        peer_addr.ip()
    );
    if !TRUST_PROBE_MANAGER.record_event(
        session_id,
        &token,
        TrustProbeEventInput {
            event_type: "proxy_configured_ok".to_string(),
            message: Some(message.clone()),
            client_ip,
            user_agent,
            platform_hint: None,
            device_id,
        },
    ) {
        return probe_json_response(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "Availability check session not found" }),
        );
    }
    probe_json_response(
        StatusCode::OK,
        serde_json::json!({
            "configured": true,
            "message": message,
        }),
    )
}

pub async fn handle_trust_probe_api(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    match (req.method().clone(), path) {
        (Method::POST, "/api/trust-probe/sessions")
        | (Method::POST, "/api/trust-probe/sessions/") => {
            create_probe_session(req, state, push_manager.as_ref()).await
        }
        (Method::GET, "/api/trust-probe/sessions")
        | (Method::GET, "/api/trust-probe/sessions/") => list_probe_sessions(),
        (Method::GET, _) if path.starts_with("/api/trust-probe/sessions/") => {
            get_probe_session(path)
        }
        (Method::PATCH, _) if path.starts_with("/api/trust-probe/sessions/") => {
            update_probe_session(req, push_manager.as_ref(), path).await
        }
        _ => {
            if path.starts_with("/api/trust-probe/") {
                method_not_allowed()
            } else {
                error_response(StatusCode::NOT_FOUND, "Not Found")
            }
        }
    }
}

fn list_probe_sessions() -> Response<BoxBody> {
    json_response(&TRUST_PROBE_MANAGER.list_sessions())
}

async fn update_probe_session(
    req: Request<Incoming>,
    push_manager: Option<&SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let Some(session_id) = path
        .strip_prefix("/api/trust-probe/sessions/")
        .and_then(|value| value.trim_end_matches('/').parse::<Uuid>().ok())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid trust probe session id");
    };
    let body = match req.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read trust probe session update body: {error}"),
            );
        }
    };
    let request = match serde_json::from_slice::<UpdateTrustProbeSessionRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid trust probe session update JSON: {error}"),
            );
        }
    };
    match TRUST_PROBE_MANAGER.update_session(session_id, request) {
        Some(session) => {
            broadcast_trust_probe_update(push_manager);
            json_response(&session)
        }
        None => error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        ),
    }
}

pub async fn handle_trust_probe_public(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    if req.method() == Method::OPTIONS {
        return cors_preflight();
    }
    if path.trim_end_matches('/') == "/public/trust-probe" {
        return render_fixed_probe_landing(req, state, push_manager.as_ref()).await;
    }
    if path.trim_end_matches('/') == "/public/trust-probe/qrcode" {
        return render_fixed_probe_qrcode(req, state, push_manager.as_ref()).await;
    }
    let Some((session_id, action)) = parse_public_probe_path(path) else {
        return error_response(StatusCode::NOT_FOUND, "Not Found");
    };
    let token = query_param(req.uri().query(), TOKEN_QUERY_KEY).unwrap_or_default();

    match (req.method().clone(), action.as_deref()) {
        (Method::GET, None) => {
            let Some(html) = TRUST_PROBE_MANAGER.render_landing_page(session_id, &token) else {
                return error_response(
                    StatusCode::NOT_FOUND,
                    "Availability check session not found",
                );
            };
            public_response_builder(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(full_body(html))
                .unwrap()
        }
        (Method::HEAD, None) => {
            if TRUST_PROBE_MANAGER
                .render_landing_page(session_id, &token)
                .is_none()
            {
                return error_response(
                    StatusCode::NOT_FOUND,
                    "Availability check session not found",
                );
            }
            public_response_builder(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(super::empty_body())
                .unwrap()
        }
        (Method::GET, Some("qrcode")) => render_probe_qrcode(session_id, &token),
        (Method::HEAD, Some("qrcode")) => render_probe_qrcode_head(session_id, &token),
        (Method::GET, Some("session")) => render_probe_public_session(session_id, &token),
        (Method::GET, Some("proxy-access")) => {
            check_proxy_access(req, state, push_manager.as_ref(), session_id, token).await
        }
        (Method::POST, Some("report")) => {
            report_probe_result(req, push_manager.as_ref(), session_id, token).await
        }
        _ => method_not_allowed(),
    }
}

async fn render_fixed_probe_landing(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<&SharedPushManager>,
) -> Response<BoxBody> {
    let host = match public_probe_host_from_request(&req) {
        Some(host) => host,
        None => return error_response(StatusCode::BAD_REQUEST, "Invalid availability check host"),
    };
    match TRUST_PROBE_MANAGER
        .get_or_create_public_session(&state, &host)
        .await
    {
        Ok(session) => {
            broadcast_trust_probe_update(push_manager);
            if req.method() == Method::HEAD {
                return public_response_builder(StatusCode::OK)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .body(super::empty_body())
                    .unwrap();
            }
            let Ok(session_id) = session.session_id.parse::<Uuid>() else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Invalid availability check session id",
                );
            };
            let html = TRUST_PROBE_MANAGER
                .render_landing_page(session_id, "")
                .unwrap_or_else(|| "Availability check session expired.".to_string());
            public_response_builder(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(full_body(html))
                .unwrap()
        }
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn render_fixed_probe_qrcode(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<&SharedPushManager>,
) -> Response<BoxBody> {
    let host = query_param(req.uri().query(), "host")
        .or_else(|| public_probe_host_from_request(&req))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    if req.method() == Method::HEAD {
        return public_response_builder(StatusCode::OK)
            .header("Content-Type", "image/svg+xml")
            .body(super::empty_body())
            .unwrap();
    }
    let session = match TRUST_PROBE_MANAGER
        .get_or_create_public_session(&state, &host)
        .await
    {
        Ok(session) => {
            broadcast_trust_probe_update(push_manager);
            session
        }
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    render_qrcode_for_url(&session.landing_url)
}

async fn create_probe_session(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<&SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {error}"),
            );
        }
    };
    let request = match serde_json::from_slice::<CreateTrustProbeSessionRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid trust probe request JSON: {error}"),
            );
        }
    };

    match TRUST_PROBE_MANAGER.create_session(&state, request).await {
        Ok(session) => {
            broadcast_trust_probe_update(push_manager);
            json_response_with_status(StatusCode::CREATED, &session)
        }
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn get_probe_session(path: &str) -> Response<BoxBody> {
    let Some(session_id) = path
        .strip_prefix("/api/trust-probe/sessions/")
        .and_then(|value| value.trim_end_matches('/').parse::<Uuid>().ok())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid trust probe session id");
    };
    match TRUST_PROBE_MANAGER.get_session(session_id) {
        Some(session) => json_response(&session),
        None => error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        ),
    }
}

fn render_probe_public_session(session_id: Uuid, token: &str) -> Response<BoxBody> {
    match TRUST_PROBE_MANAGER.get_public_session(session_id, token) {
        Some(session) => public_response_builder(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(
                serde_json::json!({
                    "sessionId": session.session_id,
                    "suggestedWifiSsid": session.suggested_wifi_ssid,
                    "suggestedWifiSsidMessage": session.suggested_wifi_ssid_message,
                    "proxyConfigured": session.proxy_configured,
                    "proxyConfigurationMessage": session.proxy_configuration_message,
                })
                .to_string(),
            ))
            .unwrap(),
        None => error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        ),
    }
}

async fn report_probe_result(
    req: Request<Incoming>,
    push_manager: Option<&SharedPushManager>,
    session_id: Uuid,
    token: String,
) -> Response<BoxBody> {
    let client_ip = request_client_ip(&req);
    let user_agent_header = req
        .headers()
        .get(hyper::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let body = match req.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read report body: {error}"),
            );
        }
    };
    let report = match serde_json::from_slice::<TrustProbeReport>(&body) {
        Ok(report) => report,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid trust probe report JSON: {error}"),
            );
        }
    };
    if !TRUST_PROBE_MANAGER.record_report(session_id, &token, report, client_ip, user_agent_header)
    {
        return error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        );
    }
    broadcast_trust_probe_update(push_manager);
    json_response(&serde_json::json!({ "ok": true }))
}

async fn check_proxy_access(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<&SharedPushManager>,
    session_id: Uuid,
    token: String,
) -> Response<BoxBody> {
    let device_id = query_param(req.uri().query(), "deviceId");
    let Some(client_ip) = request_client_ip_addr(&req) else {
        let message = "Bifrost could not determine this device IP address.".to_string();
        if !TRUST_PROBE_MANAGER.record_proxy_access(
            session_id,
            &token,
            TrustProbeProxyAccessStatus::Unavailable,
            None,
            message.clone(),
            device_id.clone(),
        ) {
            return error_response(
                StatusCode::NOT_FOUND,
                "Availability check session not found",
            );
        }
        broadcast_trust_probe_update(push_manager);
        return json_response(&TrustProbeProxyAccessView {
            status: TrustProbeProxyAccessStatus::Unavailable,
            authorized: false,
            client_ip: None,
            message,
        });
    };

    let Some(access_control) = state.access_control.as_ref() else {
        let message =
            "Proxy access control is not available for this Bifrost instance.".to_string();
        if !TRUST_PROBE_MANAGER.record_proxy_access(
            session_id,
            &token,
            TrustProbeProxyAccessStatus::Unavailable,
            Some(client_ip.to_string()),
            message.clone(),
            device_id.clone(),
        ) {
            return error_response(
                StatusCode::NOT_FOUND,
                "Availability check session not found",
            );
        }
        broadcast_trust_probe_update(push_manager);
        return json_response(&TrustProbeProxyAccessView {
            status: TrustProbeProxyAccessStatus::Unavailable,
            authorized: false,
            client_ip: Some(client_ip.to_string()),
            message,
        });
    };

    let ac = access_control.read().await;
    let decision = ac.check_access(&client_ip);
    let (status, authorized, message) = match decision {
        AccessDecision::Allow => (
            TrustProbeProxyAccessStatus::Allowed,
            true,
            format!("This device ({client_ip}) is authorized to use the Bifrost proxy."),
        ),
        AccessDecision::Prompt(ip) => {
            ac.add_pending_authorization(ip);
            (
                TrustProbeProxyAccessStatus::Pending,
                false,
                format!(
                    "This device ({client_ip}) is waiting for proxy access approval in Bifrost."
                ),
            )
        }
        AccessDecision::Deny => (
            TrustProbeProxyAccessStatus::Denied,
            false,
            format!("This device ({client_ip}) is not allowed to use the Bifrost proxy."),
        ),
    };
    drop(ac);

    if !TRUST_PROBE_MANAGER.record_proxy_access(
        session_id,
        &token,
        status,
        Some(client_ip.to_string()),
        message.clone(),
        device_id,
    ) {
        return error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        );
    }
    broadcast_trust_probe_update(push_manager);

    json_response(&TrustProbeProxyAccessView {
        status,
        authorized,
        client_ip: Some(client_ip.to_string()),
        message,
    })
}

fn render_probe_qrcode(session_id: Uuid, token: &str) -> Response<BoxBody> {
    let Some(session) = TRUST_PROBE_MANAGER.get_session(session_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        );
    };
    let url = if token.is_empty() {
        session.landing_url
    } else {
        format!(
            "{}?{TOKEN_QUERY_KEY}={}",
            session.landing_url,
            urlencoding::encode(token)
        )
    };
    render_qrcode_for_url(&url)
}

fn render_qrcode_for_url(url: &str) -> Response<BoxBody> {
    let code = match QrCode::new(url.as_bytes()) {
        Ok(code) => code,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to generate trust probe QR code: {error}"),
            );
        }
    };
    let svg_string = code
        .render()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(full_body(svg_string))
        .unwrap()
}

fn render_probe_qrcode_head(session_id: Uuid, token: &str) -> Response<BoxBody> {
    if TRUST_PROBE_MANAGER
        .render_landing_page(session_id, token)
        .is_none()
    {
        return error_response(
            StatusCode::NOT_FOUND,
            "Availability check session not found",
        );
    }
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(super::empty_body())
        .unwrap()
}

async fn run_probe_server(
    listener: TcpListener,
    server_config: Arc<bifrost_tls::rustls::ServerConfig>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let acceptor = TlsAcceptor::from(server_config);
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            accepted = listener.accept() => {
                let Ok((stream, peer_addr)) = accepted else {
                    continue;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    handle_probe_connection(stream, peer_addr, acceptor).await;
                });
            }
        }
    }
}

async fn handle_probe_connection(stream: TcpStream, peer_addr: SocketAddr, acceptor: TlsAcceptor) {
    let mut first = [0u8; 1];
    let is_tls = match tokio::time::timeout(Duration::from_secs(5), stream.peek(&mut first)).await {
        Ok(Ok(size)) if size > 0 => first[0] == 0x16,
        _ => false,
    };

    if is_tls {
        let Ok(stream) = acceptor.accept(stream).await else {
            return;
        };
        let io = TokioIo::new(stream);
        let service = service_fn(move |req| handle_probe_request(req, peer_addr, true));
        let _ = http1::Builder::new().serve_connection(io, service).await;
    } else {
        let io = TokioIo::new(stream);
        let service = service_fn(move |req| handle_probe_request(req, peer_addr, false));
        let _ = http1::Builder::new().serve_connection(io, service).await;
    }
}

async fn handle_probe_request(
    req: Request<Incoming>,
    peer_addr: SocketAddr,
    is_tls: bool,
) -> Result<Response<BoxBody>, hyper::Error> {
    if req.method() == Method::OPTIONS {
        return Ok(probe_response(StatusCode::NO_CONTENT, ""));
    }
    let path = req.uri().path();
    let Some(session_id) =
        query_param(req.uri().query(), "sid").and_then(|value| value.parse::<Uuid>().ok())
    else {
        return Ok(probe_response(StatusCode::BAD_REQUEST, "missing sid"));
    };
    let token = query_param(req.uri().query(), TOKEN_QUERY_KEY).unwrap_or_default();
    let device_id = query_param(req.uri().query(), "deviceId");
    let user_agent = req
        .headers()
        .get(hyper::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let client_ip = Some(peer_addr.ip().to_string());

    let response = match (req.method(), path, is_tls) {
        (&Method::GET, "/_bifrost/trust-probe/netcheck", false) => {
            TRUST_PROBE_MANAGER.record_event(
                session_id,
                &token,
                TrustProbeEventInput {
                    event_type: "netcheck_ok".to_string(),
                    message: None,
                    client_ip: client_ip.clone(),
                    user_agent: user_agent.clone(),
                    platform_hint: None,
                    device_id: device_id.clone(),
                },
            );
            probe_json_response(StatusCode::OK, serde_json::json!({ "ok": true }))
        }
        (&Method::GET, "/_bifrost/trust-probe/check", true) => {
            TRUST_PROBE_MANAGER.record_event(
                session_id,
                &token,
                TrustProbeEventInput {
                    event_type: "tls_ok".to_string(),
                    message: None,
                    client_ip: client_ip.clone(),
                    user_agent: user_agent.clone(),
                    platform_hint: None,
                    device_id: device_id.clone(),
                },
            );
            probe_json_response(StatusCode::OK, serde_json::json!({ "trusted": true }))
        }
        (&Method::GET, "/_bifrost/trust-probe/check", false) => probe_response(
            StatusCode::BAD_REQUEST,
            "trust check must be requested over HTTPS",
        ),
        _ => probe_response(StatusCode::NOT_FOUND, "not found"),
    };
    Ok(response)
}

fn probe_response(status: StatusCode, body: &str) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, OPTIONS")
        .header("Access-Control-Allow-Headers", "Content-Type")
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(full_body(body.to_string()))
        .unwrap()
}

fn probe_json_response(status: StatusCode, body: serde_json::Value) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, OPTIONS")
        .header("Access-Control-Allow-Headers", "Content-Type")
        .header("Content-Type", "application/json")
        .body(full_body(body.to_string()))
        .unwrap()
}

fn render_landing_page(session: &TrustProbeSession, token: &str) -> String {
    let view = session.to_view(token);
    let token_query = if token.is_empty() {
        String::new()
    } else {
        format!("?{TOKEN_QUERY_KEY}={}", urlencoding::encode(token))
    };
    let token_suffix = if token.is_empty() {
        String::new()
    } else {
        format!("&{TOKEN_QUERY_KEY}={}", urlencoding::encode(token))
    };
    let netcheck_url = format!(
        "http://{}:{}/_bifrost/trust-probe/netcheck?sid={}{}",
        view.host, view.probe_port, view.session_id, token_suffix
    );
    let tls_check_url = format!(
        "https://{}:{}/_bifrost/trust-probe/check?sid={}{}",
        view.host, view.probe_port, view.session_id, token_suffix
    );
    let report_url = format!(
        "http://{}:{}/_bifrost/public/trust-probe/{}/report{}",
        view.host, view.admin_port, view.session_id, token_query
    );
    let proxy_access_url = format!(
        "http://{}:{}/_bifrost/public/trust-probe/{}/proxy-access{}",
        view.host, view.admin_port, view.session_id, token_query
    );
    let session_public_url = format!(
        "http://{}:{}/_bifrost/public/trust-probe/{}/session{}",
        view.host, view.admin_port, view.session_id, token_query
    );
    let ios_wifi_proxy_profile_url = format!(
        "http://{}:{}/_bifrost/public/mobile/ios-wifi-proxy.mobileconfig",
        view.host, view.admin_port
    );
    let proxy_configured_url = format!(
        "http://{}{}?sid={}{}",
        TRUST_PROBE_PROXY_CONFIG_HOST, TRUST_PROBE_PROXY_CONFIG_PATH, view.session_id, token_suffix
    );
    let config = serde_json::json!({
        "sessionId": view.session_id,
        "token": token,
        "host": view.host,
        "adminPort": view.admin_port,
        "probePort": view.probe_port,
        "caFingerprintSha256": view.ca_fingerprint_sha256,
        "caDownloadUrl": view.ca_download_url,
        "iosWifiProxyProfileUrl": ios_wifi_proxy_profile_url,
        "proxyQrCodeUrl": view.proxy_qr_code_url,
        "proxyConfiguredUrl": proxy_configured_url,
        "netcheckUrl": netcheck_url,
        "tlsCheckUrl": tls_check_url,
        "proxyAccessUrl": proxy_access_url,
        "reportUrl": report_url,
        "sessionPublicUrl": session_public_url,
        "suggestedWifiSsid": view.suggested_wifi_ssid,
        "suggestedWifiSsidMessage": view.suggested_wifi_ssid_message,
    });
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Bifrost Availability Check</title>
  <style>
    :root {{ color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    body {{ margin: 0; padding: 24px; background: Canvas; color: CanvasText; }}
    main {{ max-width: 640px; margin: 0 auto; }}
    h1 {{ font-size: 24px; margin: 0 0 12px; }}
    .status {{ border: 1px solid color-mix(in srgb, CanvasText 18%, transparent); border-radius: 8px; padding: 14px; margin: 16px 0; }}
    .ok {{ color: #16833a; }}
    .warn {{ color: #9a6500; }}
    .bad {{ color: #c7352f; }}
    .warning-box {{ border: 1px solid #d6a100; border-radius: 8px; padding: 10px 12px; background: color-mix(in srgb, #ffd666 18%, Canvas); margin: 10px 0; }}
    code {{ word-break: break-all; }}
    a, button {{ font: inherit; }}
    button, .button {{ display: inline-block; margin: 6px 8px 6px 0; padding: 10px 12px; border-radius: 6px; border: 1px solid currentColor; background: transparent; color: inherit; text-decoration: none; }}
    .button-disabled {{ opacity: 0.45; pointer-events: none; }}
    label {{ display: block; margin: 10px 0 4px; font-weight: 600; }}
    input {{ box-sizing: border-box; width: 100%; padding: 10px 12px; border-radius: 6px; border: 1px solid color-mix(in srgb, CanvasText 24%, transparent); background: Canvas; color: CanvasText; font: inherit; }}
    small {{ color: color-mix(in srgb, CanvasText 72%, transparent); }}
    ol {{ padding-left: 20px; }}
  </style>
</head>
<body>
<main>
  <h1>Bifrost Availability Check</h1>
  <p>This page checks whether this device can use Bifrost and whether this browser trusts the current Bifrost CA.</p>
  <p>CA SHA-256 fingerprint:<br><code>{}</code></p>
  <section class="status" id="proxy-access">Checking proxy access authorization...</section>
  <section class="status" id="result">Preparing trust check...</section>
  <section class="status" id="proxy-configuration">Checking whether this browser is using the Bifrost proxy...</section>
  <section class="status" id="ios-wifi-proxy-tools"></section>
  <section id="next"></section>
</main>
<script>
window.__BIFROST_TRUST_PROBE__ = {};
function getDeviceId() {{
  const key = "bifrostAvailabilityDeviceId";
  try {{
    let value = localStorage.getItem(key);
    if (!value) {{
      value = "dev-" + randomSuffix();
      localStorage.setItem(key, value);
    }}
    return value;
  }} catch (_) {{
    if (!window.__BIFROST_FALLBACK_DEVICE_ID__) {{
      window.__BIFROST_FALLBACK_DEVICE_ID__ = "dev-" + randomSuffix();
    }}
    return window.__BIFROST_FALLBACK_DEVICE_ID__;
  }}
}}
function withDevice(url) {{
  const separator = url.indexOf("?") >= 0 ? "&" : "?";
  return url + separator + "deviceId=" + encodeURIComponent(window.__BIFROST_TRUST_PROBE__.deviceId || getDeviceId());
}}
function detectPlatform() {{
  const ua = navigator.userAgent || "";
  if (/iPhone|iPad|iPod/i.test(ua)) return "ios";
  if (/Android/i.test(ua)) return "android";
  return "unknown";
}}
function randomSuffix() {{
  if (window.crypto && crypto.randomUUID) return crypto.randomUUID();
  return String(Date.now()) + Math.random().toString(16).slice(2);
}}
function setHtmlIfChanged(id, html) {{
  const element = document.getElementById(id);
  if (!element || element.innerHTML === html) return;
  element.innerHTML = html;
}}
function show(html) {{ setHtmlIfChanged("result", html); }}
function showNext(html) {{ setHtmlIfChanged("next", html); }}
function showProxyAccess(html) {{ setHtmlIfChanged("proxy-access", html); }}
function showProxyConfiguration(html) {{ setHtmlIfChanged("proxy-configuration", html); }}
let probeLoopRunning = false;
let probeHasRun = false;
async function postReport(type, extra) {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  try {{
    await fetch(withDevice(cfg.reportUrl), {{
      method: "POST",
      headers: {{ "Content-Type": "application/json" }},
      body: JSON.stringify(Object.assign({{
        deviceId: cfg.deviceId,
        type,
        userAgent: navigator.userAgent,
        platformHint: detectPlatform()
      }}, extra || {{}}))
    }});
  }} catch (_) {{}}
}}
async function syncProbeConfig() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const active = document.activeElement;
  if (active && active.id === "ios-wifi-ssid-input") return;
  const risk = document.getElementById("ios-wifi-managed-risk");
  if (risk) cfg.managedWifiRiskAccepted = !!risk.checked;
  try {{
    const response = await fetch(withDevice(cfg.sessionPublicUrl) + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store"
    }});
    if (!response.ok) return;
    const data = await response.json();
    cfg.suggestedWifiSsid = data.suggestedWifiSsid || "";
    cfg.suggestedWifiSsidMessage = data.suggestedWifiSsidMessage || "";
    renderIosWifiProxyTools();
  }} catch (_) {{}}
}}
async function runProbe() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  await postReport("page_opened");
  await checkProxyAccess();
  await checkCertificateTrust();
  await checkProxyConfiguration();
}}
async function runProbeLoop() {{
  if (probeLoopRunning) return;
  probeLoopRunning = true;
  try {{
    await runProbe();
    probeHasRun = true;
  }} finally {{
    probeLoopRunning = false;
  }}
}}
async function checkCertificateTrust() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  if (!probeHasRun) {{
    show("Device opened the probe page. Checking probe port...");
  }}
  try {{
    const net = await fetch(withDevice(cfg.netcheckUrl) + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store",
      mode: "cors"
    }});
    if (!net.ok) {{
      await postReport("network_failed", {{ status: net.status }});
      show('<span class="bad">Probe port is not reachable.</span>');
      showNext("<p>Check that this phone and computer are on the same network, the selected IP is correct, and firewall rules allow the probe port.</p><button onclick='runProbeLoop()'>Retry</button>");
      return false;
    }}
    await postReport("netcheck_ok");
    if (!cfg.tlsFailed && !cfg.tlsTrusted) {{
      show('<span class="ok">Network check passed.</span> Checking HTTPS trust...');
    }}
  }} catch (error) {{
    await postReport("network_failed", {{ message: String(error) }});
    show('<span class="bad">Probe port is not reachable.</span>');
    showNext("<p>Check that this phone and computer are on the same network, the selected IP is correct, and firewall rules allow the probe port.</p><button onclick='runProbeLoop()'>Retry</button>");
    return false;
  }}
  try {{
    const tls = await fetch(withDevice(cfg.tlsCheckUrl) + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store",
      mode: "cors"
    }});
    if (tls.ok) {{
      await postReport("tls_ok");
      cfg.tlsFailed = false;
      cfg.tlsTrusted = true;
      show('<span class="ok">Trust check passed. This browser trusts Bifrost CA.</span>');
      showProxyConfig();
      return true;
    }} else {{
      await postReport("tls_failed", {{ status: tls.status }});
      showTlsFailed();
      return false;
    }}
  }} catch (error) {{
    await postReport("tls_failed", {{ message: String(error) }});
    showTlsFailed();
    return false;
  }}
}}
async function checkProxyAccess() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  if (!probeHasRun) {{
    showProxyAccess("Checking whether this device is authorized to use the Bifrost proxy...");
  }}
  try {{
    const response = await fetch(withDevice(cfg.proxyAccessUrl) + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store",
      mode: "cors"
    }});
    const data = await response.json();
    if (data.status === "allowed") {{
      showProxyAccess('<span class="ok">Proxy access is authorized.</span><br><small>' + escapeText(data.message || "") + '</small>');
    }} else if (data.status === "pending") {{
      showProxyAccess('<span class="warn">Proxy access is waiting for approval.</span><br><small>' + escapeText(data.message || "") + '</small>');
    }} else if (data.status === "denied") {{
      showProxyAccess('<span class="bad">Proxy access is denied.</span><br><small>' + escapeText(data.message || "") + '</small>');
    }} else {{
      showProxyAccess('<span class="warn">Proxy access status is unavailable.</span><br><small>' + escapeText(data.message || "") + '</small>');
    }}
  }} catch (error) {{
    showProxyAccess('<span class="warn">Proxy access check failed.</span><br><small>' + escapeText(String(error)) + '</small>');
  }}
}}
async function checkProxyConfiguration() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  if (!probeHasRun) {{
    showProxyConfiguration("Checking whether this browser is actually using the Bifrost proxy...");
  }}
  try {{
    const response = await fetch(withDevice(cfg.proxyConfiguredUrl) + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store",
      mode: "cors"
    }});
    if (response.ok) {{
      const data = await response.json().catch(function() {{ return {{}}; }});
      showProxyConfiguration('<span class="ok">Proxy is configured.</span><br><small>' + escapeText(data.message || "This browser reached Bifrost through the configured proxy.") + '</small>');
      return;
    }}
    await postReport("proxy_config_failed", {{ status: response.status }});
    showProxyConfigurationMissing("Proxy check returned HTTP " + response.status + ".");
  }} catch (error) {{
    await postReport("proxy_config_failed", {{ message: String(error) }});
    showProxyConfigurationMissing("This browser did not reach Bifrost through the proxy.");
  }}
}}
function showProxyConfigurationMissing(message) {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  showProxyConfiguration(
    '<span class="bad">Proxy is not configured yet.</span><br>' +
    '<small>' + escapeText(message || "") + '</small>' +
    '<ol>' +
    '<li>Recommended for iPhone: configure it manually in Settings &gt; Wi-Fi &gt; current network &gt; Configure Proxy &gt; Manual.</li>' +
    '<li>Set Server to <code>' + escapeText(cfg.host) + '</code> and Port to <code>' + escapeText(String(cfg.adminPort)) + '</code>, then rerun this check.</li>' +
    '<li>Use the experimental Wi-Fi profile only if you accept that uninstalling it may remove the managed Wi-Fi network entry.</li>' +
    '</ol>' +
    '<button onclick="focusIosWifiSsid()">Show experimental profile option</button>'
  );
}}
function escapeText(value) {{
  return String(value || "").replace(/[&<>"']/g, function(ch) {{
    return ({{ "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;" }})[ch];
  }});
}}
function buildIosWifiProxyProfileUrl() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const ssid = String(cfg.suggestedWifiSsid || "").trim();
  if (!ssid) return "";
  return cfg.iosWifiProxyProfileUrl +
    "?ssid=" + encodeURIComponent(ssid) +
    "&ip=" + encodeURIComponent(cfg.host) +
    "&port=" + encodeURIComponent(String(cfg.adminPort));
}}
function updateIosWifiProxyProfileLink() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const link = document.getElementById("ios-wifi-proxy-profile-link");
  const hint = document.getElementById("ios-wifi-proxy-profile-hint");
  if (!link || !hint) return;
  const url = buildIosWifiProxyProfileUrl();
  if (!url) {{
    link.removeAttribute("href");
    link.classList.add("button-disabled");
    hint.textContent = cfg.suggestedWifiSsidMessage || "Bifrost could not detect this Mac's current Wi-Fi SSID. Use manual Wi-Fi proxy setup for now.";
    return;
  }}
  const risk = document.getElementById("ios-wifi-managed-risk");
  if (risk) cfg.managedWifiRiskAccepted = !!risk.checked;
  if (!risk || !risk.checked) {{
    link.removeAttribute("href");
    link.classList.add("button-disabled");
    hint.textContent = "Confirm the managed Wi-Fi removal risk before downloading this experimental profile.";
    return;
  }}
  link.href = url;
  link.classList.remove("button-disabled");
  hint.textContent = "This profile configures Wi-Fi proxy " + window.__BIFROST_TRUST_PROBE__.host + ":" + window.__BIFROST_TRUST_PROBE__.adminPort + " for Wi-Fi \"" + window.__BIFROST_TRUST_PROBE__.suggestedWifiSsid + "\".";
}}
function renderIosWifiProxyTools() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const target = document.getElementById("ios-wifi-proxy-tools");
  if (!target) return;
  const ssid = String(cfg.suggestedWifiSsid || "").trim();
  const riskChecked = cfg.managedWifiRiskAccepted ? " checked" : "";
  const ssidHtml = ssid ? "<p>Wi-Fi name for this check: <code>" + escapeText(ssid) + "</code></p>" : "<p><span class='warn'>" + escapeText(cfg.suggestedWifiSsidMessage || "Bifrost could not detect this Mac's current Wi-Fi SSID.") + "</span></p>";
  target.innerHTML =
    "<h2>iOS Proxy Setup</h2>" +
    "<p><strong>Recommended:</strong> set the proxy manually in Settings &gt; Wi-Fi &gt; current network &gt; Configure Proxy &gt; Manual. Set Server to <code>" +
    escapeText(cfg.host) +
    "</code> and Port to <code>" +
    escapeText(String(cfg.adminPort)) +
    "</code>. Turn it back to Off when finished.</p>" +
    "<p><strong>Experimental profile:</strong> Bifrost can generate a managed Wi-Fi profile that sets the current Wi-Fi network proxy to <strong>" +
    escapeText(cfg.host + ":" + cfg.adminPort) +
    "</strong>. It does not contain or ask for the Wi-Fi password, but iOS may remove the managed Wi-Fi network entry when you uninstall this profile.</p>" +
    ssidHtml +
    "<label for='ios-wifi-ssid-input'>Wi-Fi name</label>" +
    "<input id='ios-wifi-ssid-input' autocomplete='off' value='" + escapeText(ssid) + "' placeholder='Enter the exact Wi-Fi name shown on this iPhone'>" +
    "<p><button onclick='submitWifiSsid()'>Use this Wi-Fi name</button></p>" +
    "<div class='warning-box'><strong>Experimental managed Wi-Fi profile</strong><br><small>Bifrost's iOS Wi-Fi proxy profile uses Apple's managed Wi-Fi payload. It does not contain a Wi-Fi password, but uninstalling the profile can remove that managed Wi-Fi network entry from iOS. Manual Wi-Fi proxy setup is the safe cleanup path.</small></div>" +
    "<label><input id='ios-wifi-managed-risk' type='checkbox' onchange='window.__BIFROST_TRUST_PROBE__.managedWifiRiskAccepted = this.checked; updateIosWifiProxyProfileLink()'" + riskChecked + "> I understand removing this profile may remove this Wi-Fi entry.</label>" +
    "<p><a id='ios-wifi-proxy-profile-link' class='button button-disabled'>Download Experimental Wi-Fi Proxy Profile</a></p>" +
    "<small id='ios-wifi-proxy-profile-hint'>Preparing Wi-Fi proxy profile link...</small>" +
    "<p><small>If iOS installs the profile but the proxy does not take effect, disconnect and reconnect Wi-Fi, then run this Availability Check again. If you later remove the profile and Wi-Fi disappears, reconnect to the Wi-Fi network manually.</small></p>";
  updateIosWifiProxyProfileLink();
}}
async function submitWifiSsid() {{
  const input = document.getElementById("ios-wifi-ssid-input");
  const ssid = input ? input.value.trim() : "";
  if (!ssid) {{
    const hint = document.getElementById("ios-wifi-proxy-profile-hint");
    if (hint) hint.textContent = "Enter the exact Wi-Fi name first.";
    return;
  }}
  window.__BIFROST_TRUST_PROBE__.suggestedWifiSsid = ssid;
  window.__BIFROST_TRUST_PROBE__.suggestedWifiSsidMessage = "Wi-Fi name was provided on this device.";
  updateIosWifiProxyProfileLink();
  await postReport("wifi_ssid_updated", {{ wifiSsid: ssid, message: "Wi-Fi name was provided on the availability check page." }});
  await syncProbeConfig();
}}
function focusIosWifiSsid() {{
  const target = document.getElementById("ios-wifi-proxy-tools");
  if (target) {{
    target.scrollIntoView({{ behavior: "smooth", block: "center" }});
  }}
}}
async function copyProxyAddress() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const value = cfg.host + ":" + cfg.adminPort;
  try {{
    await navigator.clipboard.writeText(value);
    document.getElementById("copy-status").textContent = "Copied";
  }} catch (_) {{
    document.getElementById("copy-status").textContent = "Select and copy manually";
  }}
}}
function showProxyConfig() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const proxyAddress = cfg.host + ":" + cfg.adminPort;
  showNext("<p>Next configure this device proxy to:</p><button onclick='copyProxyAddress()'><strong>" + proxyAddress + "</strong></button><span id='copy-status'></span><p><a class='button' href='" + cfg.proxyQrCodeUrl + "'>Open proxy QR code</a></p>");
}}
function showTlsFailed() {{
  window.__BIFROST_TRUST_PROBE__.tlsFailed = true;
  window.__BIFROST_TRUST_PROBE__.tlsTrusted = false;
  const platform = detectPlatform();
  show('<span class="bad">HTTPS trust check failed.</span><p><a class="button" href="' + window.__BIFROST_TRUST_PROBE__.caDownloadUrl + '">Download Bifrost CA</a></p>');
  let restartHint = "<p>If you just installed or trusted the CA, fully quit and restart this browser, then open this page again. Some browsers keep old certificate trust decisions until restart.</p>";
  let steps = "<p>Install and trust Bifrost CA, then return here and retry.</p>" + restartHint;
  if (platform === "ios") {{
    steps = "<ol><li>Install the Bifrost CA profile.</li><li>Open Settings &gt; General &gt; About &gt; Certificate Trust Settings.</li><li>Turn on full trust for Bifrost CA.</li><li>Fully quit and restart this browser, then retry.</li></ol>" + restartHint;
  }} else if (platform === "android") {{
    steps = "<ol><li>Install the Bifrost CA certificate.</li><li>Fully quit and restart this browser, then retry.</li><li>For Android apps, remember that some apps ignore user CAs or use certificate pinning.</li></ol>" + restartHint;
  }}
  showNext(steps + "<button onclick='runProbeLoop()'>Retry</button>");
}}
renderIosWifiProxyTools();
window.__BIFROST_TRUST_PROBE__.deviceId = getDeviceId();
setInterval(syncProbeConfig, 1000);
runProbeLoop();
setInterval(runProbeLoop, 1000);
</script>
</body>
</html>"#,
        escape_html(
            view.ca_fingerprint_sha256
                .as_deref()
                .unwrap_or("Unavailable")
        ),
        config
    )
}

fn parse_public_probe_path(path: &str) -> Option<(Uuid, Option<String>)> {
    let rest = path
        .strip_prefix("/public/trust-probe/")?
        .trim_end_matches('/');
    let mut parts = rest.split('/');
    let session_id = parts.next()?.parse::<Uuid>().ok()?;
    let action = parts.next().map(|value| value.to_string());
    if parts.next().is_some() {
        return None;
    }
    Some((session_id, action))
}

fn validate_probe_host(host: &str) -> Result<String, String> {
    let host = host.trim();
    let ip = host.parse::<IpAddr>().map_err(|_| {
        "Availability check host must be one of this computer's local IP addresses.".to_string()
    })?;
    let allowed: Vec<IpAddr> = network::get_local_ips()
        .into_iter()
        .filter_map(|info| info.ip.parse::<IpAddr>().ok())
        .chain(
            ["127.0.0.1", "::1"]
                .into_iter()
                .filter_map(|ip| ip.parse().ok()),
        )
        .collect();
    if allowed.contains(&ip) {
        Ok(ip.to_string())
    } else {
        Err("Availability check host must be selected from the local IP list.".to_string())
    }
}

fn public_probe_host_from_request(req: &Request<Incoming>) -> Option<String> {
    let raw_host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|value| value.to_str().ok())?
        .trim();
    if raw_host.is_empty() {
        return None;
    }
    let host = if raw_host.starts_with('[') {
        raw_host
            .strip_prefix('[')
            .and_then(|value| value.split(']').next())
            .unwrap_or(raw_host)
    } else {
        raw_host.split(':').next().unwrap_or(raw_host)
    };
    if host.trim().is_empty() {
        None
    } else {
        Some(host.trim().to_string())
    }
}

fn normalize_device_id(device_id: Option<String>) -> Option<String> {
    let device_id = device_id?.trim().to_string();
    if device_id.is_empty() || device_id.len() > 96 {
        return None;
    }
    if !device_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return None;
    }
    Some(device_id)
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|query| {
        query.split('&').find_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(k), Some(value)) if k == key => urlencoding::decode(value)
                    .ok()
                    .map(|value| value.into_owned()),
                _ => None,
            }
        })
    })
}

fn request_client_ip(req: &Request<Incoming>) -> Option<String> {
    req.headers()
        .get("x-bifrost-peer-ip")
        .or_else(|| req.headers().get("x-forwarded-for"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn request_client_ip_addr(req: &Request<Incoming>) -> Option<IpAddr> {
    request_client_ip(req).and_then(|value| value.parse::<IpAddr>().ok())
}

struct WifiSsidDetection {
    ssid: Option<String>,
    message: Option<String>,
}

fn current_wifi_ssid_detection() -> WifiSsidDetection {
    if !cfg!(target_os = "macos") {
        return WifiSsidDetection {
            ssid: None,
            message: Some(
                "Automatic Wi-Fi name detection is currently only supported on macOS.".to_string(),
            ),
        };
    }
    let Some(device) = current_wifi_device() else {
        return WifiSsidDetection {
            ssid: None,
            message: Some("Bifrost could not find the macOS Wi-Fi interface.".to_string()),
        };
    };
    if let Some(ssid) = current_wifi_ssid_from_networksetup(&device) {
        return WifiSsidDetection {
            ssid: Some(ssid),
            message: None,
        };
    }
    match current_wifi_ssid_from_ipconfig(&device) {
        Some(ssid) if ssid == "<redacted>" => WifiSsidDetection {
            ssid: None,
            message: Some(
                "macOS is hiding the current Wi-Fi name from this Bifrost process. Grant location permission to the app or use manual Wi-Fi proxy setup for now.".to_string(),
            ),
        },
        Some(ssid) => WifiSsidDetection {
            ssid: Some(ssid),
            message: None,
        },
        None => WifiSsidDetection {
            ssid: None,
            message: Some("Bifrost could not detect the current Wi-Fi name from macOS.".to_string()),
        },
    }
}

fn current_wifi_device() -> Option<String> {
    let output = Command::new("networksetup")
        .arg("-listallhardwareports")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut in_wifi_block = false;
    let mut device = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("Hardware Port: ") {
            in_wifi_block = name == "Wi-Fi" || name == "AirPort";
            continue;
        }
        if in_wifi_block {
            if let Some(value) = line.strip_prefix("Device: ") {
                device = Some(value.trim().to_string());
                break;
            }
        }
    }
    device
}

fn current_wifi_ssid_from_networksetup(device: &str) -> Option<String> {
    let output = Command::new("networksetup")
        .arg("-getairportnetwork")
        .arg(device)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_once(':')
        .map(|(_, ssid)| ssid.trim().to_string())
        .filter(|ssid| !ssid.is_empty() && ssid != "<redacted>")
}

fn current_wifi_ssid_from_ipconfig(device: &str) -> Option<String> {
    let output = Command::new("ipconfig")
        .arg("getsummary")
        .arg(device)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("SSID :")?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn ca_key_path_from_cert_path(ca_cert_path: &Path) -> PathBuf {
    ca_cert_path.with_file_name("ca.key")
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn certificate_sha256_fingerprint(cert_path: &Path) -> Option<String> {
    let data = bifrost_device::read_certificate_der_from_file(cert_path).ok()?;
    let digest = Sha256::digest(data);
    Some(
        digest
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session(id: Uuid, token: &str, now: DateTime<Utc>) -> TrustProbeSession {
        TrustProbeSession {
            id,
            token_hash: hash_token(token),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::Created,
            opened: false,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: None,
            suggested_wifi_ssid_message: None,
            network_reachable: false,
            tls_trusted: false,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: Vec::new(),
        }
    }

    #[test]
    fn token_hash_does_not_match_plain_token() {
        let token = "secret";
        assert_ne!(hash_token(token), token);
    }

    #[test]
    fn session_status_tracks_tls_failure_after_network_success() {
        let id = Uuid::new_v4();
        let token = "token";
        let mut session = TrustProbeSession {
            id,
            token_hash: hash_token(token),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::Created,
            opened: false,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: None,
            suggested_wifi_ssid_message: None,
            network_reachable: false,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: Vec::new(),
        };

        session.apply_event(
            "page_opened",
            None,
            None,
            None,
            Some("ios".to_string()),
            None,
        );
        session.apply_event("netcheck_ok", None, None, None, None, None);
        session.apply_event(
            "tls_failed",
            Some("Failed to fetch".to_string()),
            None,
            None,
            None,
            None,
        );

        assert!(session.opened);
        assert!(session.network_reachable);
        assert!(!session.tls_trusted);
        assert_eq!(session.status, TrustProbeStatus::TlsFailed);
        assert_eq!(session.platform_hint.as_deref(), Some("ios"));
    }

    #[test]
    fn tls_success_wins_over_prior_failure() {
        let id = Uuid::new_v4();
        let token = "token";
        let mut session = TrustProbeSession {
            id,
            token_hash: hash_token(token),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::TlsFailed,
            opened: true,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: None,
            suggested_wifi_ssid_message: None,
            network_reachable: true,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: Some("old error".to_string()),
            devices: HashMap::new(),
            events: Vec::new(),
        };

        session.apply_event("tls_ok", None, None, None, None, None);

        assert_eq!(session.status, TrustProbeStatus::TlsTrusted);
        assert!(session.tls_trusted);
        assert!(session.last_error.is_none());
    }

    #[test]
    fn proxy_access_status_is_recorded_in_view() {
        let id = Uuid::new_v4();
        let token = "token";
        let mut session = TrustProbeSession {
            id,
            token_hash: hash_token(token),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::PageOpened,
            opened: true,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: None,
            suggested_wifi_ssid_message: None,
            network_reachable: false,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: Vec::new(),
        };

        session.apply_proxy_access(
            TrustProbeProxyAccessStatus::Pending,
            Some("192.168.1.20".to_string()),
            "Waiting for proxy access approval.".to_string(),
            None,
        );

        let view = session.to_view(token);
        assert_eq!(
            view.proxy_access_status,
            Some(TrustProbeProxyAccessStatus::Pending)
        );
        assert_eq!(view.proxy_access_allowed, Some(false));
        assert_eq!(
            view.proxy_access_message.as_deref(),
            Some("Waiting for proxy access approval.")
        );
        assert_eq!(view.client_ip.as_deref(), Some("192.168.1.20"));
        assert_eq!(
            view.events.last().map(|event| event.event_type.as_str()),
            Some("proxy_access_pending")
        );
    }

    #[test]
    fn proxy_configuration_status_is_recorded_in_view() {
        let id = Uuid::new_v4();
        let token = "token";
        let mut session = TrustProbeSession {
            id,
            token_hash: hash_token(token),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::PageOpened,
            opened: true,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: Some("Office Wi-Fi".to_string()),
            suggested_wifi_ssid_message: None,
            network_reachable: false,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: Vec::new(),
        };

        session.apply_event(
            "proxy_configured_ok",
            Some("Proxy reached Bifrost.".to_string()),
            Some("192.168.1.20".to_string()),
            None,
            None,
            None,
        );

        let view = session.to_view(token);
        assert!(view.proxy_configured);
        assert_eq!(
            view.proxy_configuration_message.as_deref(),
            Some("Proxy reached Bifrost.")
        );
        assert_eq!(view.suggested_wifi_ssid.as_deref(), Some("Office Wi-Fi"));
        assert_eq!(view.client_ip.as_deref(), Some("192.168.1.20"));
        assert_eq!(
            view.events.last().map(|event| event.event_type.as_str()),
            Some("proxy_configured_ok")
        );
    }

    #[test]
    fn device_status_is_tracked_per_local_storage_device_id() {
        let token = "token";
        let mut session = test_session(Uuid::new_v4(), token, Utc::now());

        session.apply_event(
            "tls_ok",
            None,
            Some("192.168.1.20".to_string()),
            Some("Mobile Safari".to_string()),
            Some("ios".to_string()),
            Some("dev-ios".to_string()),
        );
        session.apply_proxy_access(
            TrustProbeProxyAccessStatus::Pending,
            Some("192.168.1.21".to_string()),
            "Waiting for proxy access approval.".to_string(),
            Some("dev-android".to_string()),
        );

        let view = session.to_view("");
        assert_eq!(view.devices.len(), 2);
        let ios = view
            .devices
            .iter()
            .find(|device| device.device_id == "dev-ios")
            .expect("ios device state");
        assert!(ios.tls_trusted);
        assert_eq!(ios.platform_hint.as_deref(), Some("ios"));
        assert_eq!(ios.client_ip.as_deref(), Some("192.168.1.20"));

        let android = view
            .devices
            .iter()
            .find(|device| device.device_id == "dev-android")
            .expect("android device state");
        assert_eq!(
            android.proxy_access_status,
            Some(TrustProbeProxyAccessStatus::Pending)
        );
        assert_eq!(android.proxy_access_allowed, Some(false));
        assert_eq!(android.client_ip.as_deref(), Some("192.168.1.21"));
    }

    #[test]
    fn tls_failure_is_not_downgraded_by_next_network_check() {
        let token = "token";
        let mut session = test_session(Uuid::new_v4(), token, Utc::now());

        session.apply_event(
            "tls_failed",
            Some("Failed to fetch".to_string()),
            Some("192.168.1.20".to_string()),
            Some("Mobile Safari".to_string()),
            Some("ios".to_string()),
            Some("dev-ios".to_string()),
        );
        session.apply_event(
            "netcheck_ok",
            None,
            Some("192.168.1.20".to_string()),
            Some("Mobile Safari".to_string()),
            Some("ios".to_string()),
            Some("dev-ios".to_string()),
        );

        let view = session.to_view("");
        assert_eq!(view.status, TrustProbeStatus::TlsFailed);
        let device = view
            .devices
            .iter()
            .find(|device| device.device_id == "dev-ios")
            .expect("device state");
        assert_eq!(device.status, TrustProbeStatus::TlsFailed);
        assert!(device.network_reachable);
        assert!(!device.tls_trusted);
    }

    #[test]
    fn user_wifi_ssid_is_recorded_in_view() {
        let id = Uuid::new_v4();
        let token = "token";
        let mut session = TrustProbeSession {
            id,
            token_hash: hash_token(token),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::Created,
            opened: false,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: None,
            suggested_wifi_ssid_message: None,
            network_reachable: false,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: Vec::new(),
        };

        session.apply_wifi_ssid("Office Wi-Fi".to_string());

        let view = session.to_view(token);
        assert_eq!(view.suggested_wifi_ssid.as_deref(), Some("Office Wi-Fi"));
        assert!(view
            .suggested_wifi_ssid_message
            .as_deref()
            .unwrap_or_default()
            .contains("provided by the user"));
        assert_eq!(
            view.events.last().map(|event| event.event_type.as_str()),
            Some("wifi_ssid_updated")
        );
    }

    #[test]
    fn user_wifi_ssid_is_synced_to_all_active_sessions() {
        let now = Utc::now();
        let session = |id: Uuid, expires_at: DateTime<Utc>| TrustProbeSession {
            id,
            token_hash: hash_token("token"),
            host: "127.0.0.1".to_string(),
            admin_port: 8800,
            probe_port: 8802,
            ca_fingerprint_sha256: None,
            status: TrustProbeStatus::Created,
            opened: false,
            proxy_access_status: None,
            proxy_access_allowed: None,
            proxy_access_message: None,
            proxy_configured: false,
            proxy_configuration_message: None,
            suggested_wifi_ssid: None,
            suggested_wifi_ssid_message: None,
            network_reachable: false,
            tls_trusted: false,
            created_at: now,
            expires_at,
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            devices: HashMap::new(),
            events: Vec::new(),
        };
        let active_a = Uuid::new_v4();
        let active_b = Uuid::new_v4();
        let expired = Uuid::new_v4();
        let mut sessions = HashMap::from([
            (
                active_a,
                session(active_a, now + chrono::Duration::minutes(10)),
            ),
            (
                active_b,
                session(active_b, now + chrono::Duration::minutes(10)),
            ),
            (
                expired,
                session(expired, now - chrono::Duration::minutes(1)),
            ),
        ]);

        TrustProbeManager::apply_wifi_ssid_to_active_sessions(
            &mut sessions,
            "Office Wi-Fi".to_string(),
        );

        assert_eq!(
            sessions
                .get(&active_a)
                .and_then(|session| session.suggested_wifi_ssid.as_deref()),
            Some("Office Wi-Fi")
        );
        assert_eq!(
            sessions
                .get(&active_b)
                .and_then(|session| session.suggested_wifi_ssid.as_deref()),
            Some("Office Wi-Fi")
        );
        assert!(sessions
            .get(&expired)
            .and_then(|session| session.suggested_wifi_ssid.as_deref())
            .is_none());
    }
}
