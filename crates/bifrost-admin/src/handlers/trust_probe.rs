use std::cmp::Reverse;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI64, Ordering};
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
use tokio::sync::{oneshot, Mutex as AsyncMutex};
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
const PROBE_SERVER_IDLE_TTL: Duration = Duration::from_secs(60);
const PROBE_SERVER_IDLE_REAPER_INTERVAL: Duration = Duration::from_secs(15);
const TOKEN_QUERY_KEY: &str = "t";
pub const TRUST_PROBE_PROXY_CONFIG_HOST: &str = "bifrost-proxy-check.invalid";
pub const TRUST_PROBE_PROXY_CONFIG_PATH: &str = "/_bifrost/trust-probe/proxy-configured";

pub fn list_active_sessions() -> Vec<TrustProbeSessionView> {
    TRUST_PROBE_MANAGER.list_sessions()
}

pub fn is_active_trust_probe_target(host: &str, port: u16) -> bool {
    TRUST_PROBE_MANAGER.is_active_probe_target(host, port)
}

pub fn infer_device_platform_hint(user_agent: &str) -> Option<String> {
    let ua = user_agent.trim();
    if ua.is_empty() {
        return None;
    }
    let lower = ua.to_ascii_lowercase();
    let os = if lower.contains("iphone") || lower.contains("ipad") || lower.contains("ipod") {
        Some("ios")
    } else if lower.contains("harmonyos") || lower.contains("openharmony") {
        Some("harmonyos")
    } else if lower.contains("android") {
        Some("android")
    } else if lower.contains("windows phone") {
        Some("windows phone")
    } else if lower.contains("windows nt") {
        Some("windows")
    } else if lower.contains("macintosh") || lower.contains("mac os x") {
        Some("macos")
    } else if lower.contains("cros") {
        Some("chromeos")
    } else if lower.contains("linux") || lower.contains("x11") {
        Some("linux")
    } else {
        None
    };

    let app = if lower.contains("micromessenger") {
        Some("wechat")
    } else if lower.contains("alipayclient") {
        Some("alipay")
    } else if lower.contains("dingtalk") {
        Some("dingtalk")
    } else if lower.contains("lark") || lower.contains("feishu") {
        Some("lark")
    } else if lower.contains("mqqbrowser") || lower.contains("qqbrowser") {
        Some("qqbrowser")
    } else if lower.contains("samsungbrowser") {
        Some("samsung browser")
    } else if lower.contains("huaweibrowser") {
        Some("huawei browser")
    } else if lower.contains("miuibrowser") {
        Some("miui browser")
    } else if lower.contains("ucbrowser") {
        Some("uc browser")
    } else if lower.contains("quark") {
        Some("quark")
    } else if lower.contains("baidubrowser") {
        Some("baidu browser")
    } else if lower.contains("sogoumobilebrowser") || lower.contains("metasr") {
        Some("sogou browser")
    } else if lower.contains("edga")
        || lower.contains("edgios")
        || lower.contains("edg/")
        || lower.contains("edge/")
    {
        Some("edge")
    } else if lower.contains("opr/") || lower.contains("opera") {
        Some("opera")
    } else if lower.contains("fxios") || lower.contains("firefox") {
        Some("firefox")
    } else if lower.contains("crios") || lower.contains("chrome/") {
        Some("chrome")
    } else if lower.contains("safari/") {
        Some("safari")
    } else {
        None
    };

    match (os, app) {
        (Some(os), Some(app)) => Some(format!("{os} {app}")),
        (Some(os), None) => Some(os.to_string()),
        (None, Some(app)) => Some(app.to_string()),
        (None, None) => Some("browser".to_string()),
    }
}

pub async fn get_or_create_terminal_probe_session(
    state: &SharedAdminState,
    host: &str,
) -> Result<TrustProbeSessionView, String> {
    TRUST_PROBE_MANAGER
        .get_or_create_public_session(state, host)
        .await
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

impl TrustProbeDeviceView {
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn status(&self) -> TrustProbeStatus {
        self.status
    }

    pub fn opened(&self) -> bool {
        self.opened
    }

    pub fn proxy_access_status(&self) -> Option<TrustProbeProxyAccessStatus> {
        self.proxy_access_status
    }

    pub fn proxy_configured(&self) -> bool {
        self.proxy_configured
    }

    pub fn network_reachable(&self) -> bool {
        self.network_reachable
    }

    pub fn tls_trusted(&self) -> bool {
        self.tls_trusted
    }

    pub fn client_ip(&self) -> Option<&str> {
        self.client_ip.as_deref()
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    pub fn platform_hint(&self) -> Option<&str> {
        self.platform_hint.as_deref()
    }

    pub fn last_seen(&self) -> DateTime<Utc> {
        self.last_seen
    }
}

impl TrustProbeSessionView {
    pub fn devices(&self) -> &[TrustProbeDeviceView] {
        &self.devices
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn landing_url(&self) -> &str {
        &self.landing_url
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProbeServerKey {
    host: String,
    ca_fingerprint_sha256: Option<String>,
}

#[derive(Debug)]
struct ProbeServerHandle {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
    last_activity_ms: Arc<AtomicI64>,
}

#[derive(Debug)]
pub struct TrustProbeManager {
    sessions: Mutex<HashMap<Uuid, TrustProbeSession>>,
    servers: Arc<Mutex<HashMap<ProbeServerKey, ProbeServerHandle>>>,
    ensure_lock: AsyncMutex<()>,
}

impl TrustProbeManager {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            servers: Arc::new(Mutex::new(HashMap::new())),
            ensure_lock: AsyncMutex::new(()),
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
        if let Some(session_id) = {
            let sessions = self.sessions.lock();
            sessions
                .values()
                .filter(|session| {
                    !session.is_expired()
                        && session.host == host
                        && session.admin_port == admin_port
                        && session.ca_fingerprint_sha256 == ca_fingerprint_sha256
                })
                .max_by_key(|session| session.created_at)
                .map(|session| session.id)
        } {
            self.ensure_probe_server_for_group(
                &host,
                admin_port,
                &ca_cert_path,
                &ca_key_path,
                ca_fingerprint_sha256.clone(),
            )
            .await?;
            let sessions = self.sessions.lock();
            if let Some(session) = sessions.get(&session_id) {
                return Ok(session.to_view(""));
            }
        }
        let probe_port = self
            .ensure_probe_server_for_group(
                &host,
                admin_port,
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
        if let Some((session_id, admin_port, ca_fingerprint_sha256)) = {
            let sessions = self.sessions.lock();
            sessions
                .values()
                .filter(|session| {
                    !session.is_expired()
                        && session.host == host
                        && session.admin_port == admin_port
                        && session.ca_fingerprint_sha256 == ca_fingerprint_sha256
                })
                .max_by_key(|session| session.created_at)
                .map(|session| {
                    (
                        session.id,
                        session.admin_port,
                        session.ca_fingerprint_sha256.clone(),
                    )
                })
        } {
            self.ensure_probe_server_for_group(
                &host,
                admin_port,
                &ca_cert_path,
                &ca_key_path,
                ca_fingerprint_sha256,
            )
            .await?;
            let sessions = self.sessions.lock();
            if let Some(session) = sessions.get(&session_id) {
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

    fn is_active_probe_target(&self, host: &str, port: u16) -> bool {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
        sessions
            .values()
            .filter(|session| !session.is_expired())
            .any(|session| {
                session.probe_port == port && probe_target_hosts_match(&session.host, host)
            })
    }

    fn get_public_session(&self, session_id: Uuid, token: &str) -> Option<TrustProbeSessionView> {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
        let session = sessions.get(&session_id)?;
        if !session.token_matches(token) || session.is_expired() {
            return None;
        }
        self.touch_probe_server_activity(&ProbeServerKey {
            host: session.host.clone(),
            ca_fingerprint_sha256: session.ca_fingerprint_sha256.clone(),
        });
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

    async fn ensure_session_probe_server(
        &self,
        state: &SharedAdminState,
        session_id: Uuid,
        token: &str,
    ) -> Result<TrustProbeSessionView, String> {
        self.cleanup_expired_sessions();
        let (host, admin_port, ca_fingerprint_sha256) = {
            let sessions = self.sessions.lock();
            let session = sessions
                .get(&session_id)
                .ok_or_else(|| "Availability check session not found".to_string())?;
            if !session.token_matches(token) || session.is_expired() {
                return Err("Availability check session not found".to_string());
            }
            (
                session.host.clone(),
                session.admin_port,
                session.ca_fingerprint_sha256.clone(),
            )
        };

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
        self.ensure_probe_server_for_group(
            &host,
            admin_port,
            &ca_cert_path,
            &ca_key_path,
            ca_fingerprint_sha256,
        )
        .await?;
        let sessions = self.sessions.lock();
        sessions
            .get(&session_id)
            .map(|session| session.to_view(token))
            .ok_or_else(|| "Availability check session not found".to_string())
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
        let key = {
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
            ProbeServerKey {
                host: session.host.clone(),
                ca_fingerprint_sha256: session.ca_fingerprint_sha256.clone(),
            }
        };
        self.touch_probe_server_activity(&key);
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
        let key = {
            let mut sessions = self.sessions.lock();
            let Some(session) = sessions.get_mut(&session_id) else {
                return false;
            };
            if !session.token_matches(token) || session.is_expired() {
                return false;
            }
            session.apply_proxy_access(status, client_ip, message, device_id);
            ProbeServerKey {
                host: session.host.clone(),
                ca_fingerprint_sha256: session.ca_fingerprint_sha256.clone(),
            }
        };
        self.touch_probe_server_activity(&key);
        true
    }

    async fn ensure_probe_server_for_group(
        &self,
        host: &str,
        admin_port: u16,
        ca_cert_path: &Path,
        ca_key_path: &Path,
        ca_fingerprint_sha256: Option<String>,
    ) -> Result<u16, String> {
        let probe_port = self
            .ensure_probe_server(
                host,
                admin_port.saturating_add(2),
                ca_cert_path,
                ca_key_path,
                ca_fingerprint_sha256.clone(),
            )
            .await?;
        self.update_probe_port_for_group(host, admin_port, &ca_fingerprint_sha256, probe_port);
        Ok(probe_port)
    }

    fn update_probe_port_for_group(
        &self,
        host: &str,
        admin_port: u16,
        ca_fingerprint_sha256: &Option<String>,
        probe_port: u16,
    ) {
        let mut sessions = self.sessions.lock();
        for session in sessions.values_mut().filter(|session| {
            !session.is_expired()
                && session.host == host
                && session.admin_port == admin_port
                && &session.ca_fingerprint_sha256 == ca_fingerprint_sha256
        }) {
            session.probe_port = probe_port;
        }
    }

    async fn ensure_probe_server(
        &self,
        host: &str,
        preferred_port: u16,
        ca_cert_path: &Path,
        ca_key_path: &Path,
        ca_fingerprint_sha256: Option<String>,
    ) -> Result<u16, String> {
        let key = ProbeServerKey {
            host: host.to_string(),
            ca_fingerprint_sha256: ca_fingerprint_sha256.clone(),
        };
        let existing_port = { self.servers.lock().get(&key).map(|server| server.port) };
        if let Some(port) = existing_port {
            if probe_server_port_is_listening(port).await {
                self.touch_probe_server_activity(&key);
                return Ok(port);
            }
        }

        let _ensure_guard = self.ensure_lock.lock().await;
        let existing_port = { self.servers.lock().get(&key).map(|server| server.port) };
        if let Some(port) = existing_port {
            if probe_server_port_is_listening(port).await {
                self.touch_probe_server_activity(&key);
                return Ok(port);
            }
            let mut servers = self.servers.lock();
            if let Some(server) = servers.get(&key) {
                if server.port != port {
                    return Ok(server.port);
                }
                servers.remove(&key);
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
        let last_activity_ms = Arc::new(AtomicI64::new(now_epoch_millis()));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(run_probe_server(
            listener,
            server_config,
            shutdown_rx,
            last_activity_ms.clone(),
        ));
        tokio::spawn(reap_idle_probe_server(
            self.servers.clone(),
            key.clone(),
            port,
            last_activity_ms.clone(),
        ));
        let mut servers = self.servers.lock();
        if let Some(existing) = servers.get(&key) {
            let _ = shutdown_tx.send(());
            return Ok(existing.port);
        }
        servers.insert(
            key,
            ProbeServerHandle {
                port,
                shutdown_tx: Some(shutdown_tx),
                last_activity_ms,
            },
        );
        Ok(port)
    }

    fn touch_probe_server_activity(&self, key: &ProbeServerKey) -> bool {
        let servers = self.servers.lock();
        let Some(server) = servers.get(key) else {
            return false;
        };
        touch_probe_activity(&server.last_activity_ms);
        true
    }

    fn cleanup_expired_sessions(&self) {
        let now = Utc::now();
        let active_server_keys = {
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
            sessions
                .values()
                .filter(|session| session.expires_at > now)
                .map(|session| ProbeServerKey {
                    host: session.host.clone(),
                    ca_fingerprint_sha256: session.ca_fingerprint_sha256.clone(),
                })
                .collect::<std::collections::HashSet<_>>()
        };
        let mut servers = self.servers.lock();
        servers.retain(|key, server| {
            if active_server_keys.contains(key) {
                return true;
            }
            if let Some(tx) = server.shutdown_tx.take() {
                let _ = tx.send(());
            }
            false
        });
        if active_server_keys.is_empty() {
            servers.clear();
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

async fn probe_server_port_is_listening(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_millis(250),
        TcpStream::connect((Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

fn now_epoch_millis() -> i64 {
    Utc::now().timestamp_millis()
}

fn touch_probe_activity(last_activity_ms: &AtomicI64) {
    last_activity_ms.store(now_epoch_millis(), Ordering::Relaxed);
}

fn normalize_platform_hint(
    platform_hint: Option<String>,
    user_agent: Option<&str>,
) -> Option<String> {
    let platform_hint = platform_hint
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty() && !is_unknown_platform_hint(value));
    platform_hint.or_else(|| user_agent.and_then(infer_device_platform_hint))
}

fn is_unknown_platform_hint(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.is_empty()
        || value == "unknown"
        || value == "unknown browser"
        || value == "unknown device"
        || value == "unknown platform"
}

async fn reap_idle_probe_server(
    servers: Arc<Mutex<HashMap<ProbeServerKey, ProbeServerHandle>>>,
    key: ProbeServerKey,
    port: u16,
    last_activity_ms: Arc<AtomicI64>,
) {
    loop {
        tokio::time::sleep(PROBE_SERVER_IDLE_REAPER_INTERVAL).await;
        if shutdown_idle_probe_server_if_due(
            &servers,
            &key,
            port,
            &last_activity_ms,
            PROBE_SERVER_IDLE_TTL,
        ) {
            return;
        }
    }
}

fn shutdown_idle_probe_server_if_due(
    servers: &Arc<Mutex<HashMap<ProbeServerKey, ProbeServerHandle>>>,
    key: &ProbeServerKey,
    port: u16,
    last_activity_ms: &Arc<AtomicI64>,
    idle_ttl: Duration,
) -> bool {
    let idle_for_ms = now_epoch_millis() - last_activity_ms.load(Ordering::Relaxed);
    if idle_for_ms < idle_ttl.as_millis() as i64 {
        return false;
    }

    let mut servers = servers.lock();
    let Some(server) = servers.get_mut(key) else {
        return true;
    };
    if server.port != port || !Arc::ptr_eq(&server.last_activity_ms, last_activity_ms) {
        return true;
    }
    let idle_for_ms = now_epoch_millis() - server.last_activity_ms.load(Ordering::Relaxed);
    if idle_for_ms < idle_ttl.as_millis() as i64 {
        return false;
    }
    if let Some(tx) = server.shutdown_tx.take() {
        let _ = tx.send(());
    }
    servers.remove(key);
    true
}

impl TrustProbeSession {
    fn token_matches(&self, token: &str) -> bool {
        // NOTE: an empty token is still accepted on purpose — the public
        // "fixed landing" flow (`render_fixed_probe_landing`) renders sessions
        // tokenless via `render_landing_page(session_id, "")`, and session IDs
        // are unguessable v4 UUIDs (122 bits of entropy). Tightening this into
        // a hard rejection would break that anonymous flow, so it needs a
        // product decision (tracked as P0-2). We do, however, compare the
        // non-empty case in constant time to avoid a token-hash timing oracle.
        if token.is_empty() {
            return true;
        }
        constant_time_eq(self.token_hash.as_bytes(), hash_token(token).as_bytes())
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
        devices.sort_by(compare_trust_probe_device_views);
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
        let user_agent = user_agent.filter(|value| !value.trim().is_empty());
        let platform_hint = normalize_platform_hint(platform_hint, user_agent.as_deref());
        if let Some(user_agent) = user_agent {
            self.user_agent = Some(user_agent);
        }
        if let Some(platform_hint) = platform_hint {
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

fn compare_trust_probe_device_views(
    left: &TrustProbeDeviceView,
    right: &TrustProbeDeviceView,
) -> std::cmp::Ordering {
    compare_optional_ip(left.client_ip.as_deref(), right.client_ip.as_deref())
        .then_with(|| left.device_id.cmp(&right.device_id))
}

fn compare_optional_ip(left: Option<&str>, right: Option<&str>) -> std::cmp::Ordering {
    match (left.and_then(parse_ip_addr), right.and_then(parse_ip_addr)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.unwrap_or_default().cmp(right.unwrap_or_default()),
    }
}

fn parse_ip_addr(value: &str) -> Option<IpAddr> {
    value.parse::<IpAddr>().ok()
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
        let user_agent = user_agent
            .filter(|value| !value.trim().is_empty())
            .or_else(|| self.user_agent.clone());
        self.platform_hint =
            normalize_platform_hint(platform_hint, user_agent.as_deref()).or_else(|| {
                self.platform_hint
                    .clone()
                    .filter(|value| !is_unknown_platform_hint(value))
            });
        self.user_agent = user_agent;
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
    let normalized_path = path.trim_end_matches('/');
    if normalized_path == "/public/trust-probe" || normalized_path == "/tp" {
        return render_fixed_probe_landing(req, state, push_manager.as_ref()).await;
    }
    if normalized_path == "/public/trust-probe/qrcode" {
        return render_fixed_probe_qrcode(req, state, push_manager.as_ref()).await;
    }
    let Some((session_id, action)) = parse_public_probe_path(path) else {
        return error_response(StatusCode::NOT_FOUND, "Not Found");
    };
    let token = query_param(req.uri().query(), TOKEN_QUERY_KEY).unwrap_or_default();

    match (req.method().clone(), action.as_deref()) {
        (Method::GET, None) => {
            if let Err(error) = TRUST_PROBE_MANAGER
                .ensure_session_probe_server(&state, session_id, &token)
                .await
            {
                return error_response(StatusCode::BAD_REQUEST, &error);
            }
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
        (Method::GET, Some("session")) => {
            render_probe_public_session(state, session_id, &token).await
        }
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
            if let Err(error) = TRUST_PROBE_MANAGER
                .ensure_session_probe_server(&state, session_id, "")
                .await
            {
                return error_response(StatusCode::BAD_REQUEST, &error);
            }
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

async fn render_probe_public_session(
    state: SharedAdminState,
    session_id: Uuid,
    token: &str,
) -> Response<BoxBody> {
    if let Err(error) = TRUST_PROBE_MANAGER
        .ensure_session_probe_server(&state, session_id, token)
        .await
    {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
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
    last_activity_ms: Arc<AtomicI64>,
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
                touch_probe_activity(&last_activity_ms);
                let acceptor = acceptor.clone();
                let activity = last_activity_ms.clone();
                tokio::spawn(async move {
                    handle_probe_connection(stream, peer_addr, acceptor, activity).await;
                });
            }
        }
    }
}

async fn handle_probe_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    acceptor: TlsAcceptor,
    last_activity_ms: Arc<AtomicI64>,
) {
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
        let activity = last_activity_ms.clone();
        let service = service_fn(move |req| {
            touch_probe_activity(&activity);
            handle_probe_request(req, peer_addr, true)
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    } else {
        let io = TokioIo::new(stream);
        let activity = last_activity_ms.clone();
        let service = service_fn(move |req| {
            touch_probe_activity(&activity);
            handle_probe_request(req, peer_addr, false)
        });
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
        "proxyConfigured": view.proxy_configured,
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
    .proxy-target {{ border: 1px solid #1677ff; border-radius: 10px; padding: 14px; margin: 16px 0; background: color-mix(in srgb, #1677ff 12%, Canvas); }}
    .proxy-target strong {{ display: block; margin-bottom: 6px; }}
    .proxy-target code {{ display: inline-block; font-size: 20px; font-weight: 700; padding: 4px 6px; border-radius: 6px; background: color-mix(in srgb, #1677ff 18%, Canvas); }}
    .hidden {{ display: none !important; }}
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
  <p>This page checks proxy access, direct probe reachability, and whether this browser can complete the Bifrost HTTPS probe. It does not prove every Android app trusts the CA.</p>
  <section class="proxy-target" id="target-proxy-service">
    <strong>Target proxy service</strong>
    <code id="target-proxy-address"></code>
  </section>
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
  const os = detectPlatformOs(ua);
  const app = detectPlatformApp(ua);
  if (os && app) return os + " " + app;
  return os || app || "browser";
}}
function detectPlatformOs(ua) {{
  if (/iPhone|iPad|iPod/i.test(ua)) return "ios";
  if (/HarmonyOS|OpenHarmony/i.test(ua)) return "harmonyos";
  if (/Android/i.test(ua)) return "android";
  if (/Windows Phone/i.test(ua)) return "windows phone";
  if (/Windows NT/i.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/i.test(ua)) return "macos";
  if (/CrOS/i.test(ua)) return "chromeos";
  if (/Linux|X11/i.test(ua)) return "linux";
  return "";
}}
function detectPlatformApp(ua) {{
  const lower = String(ua || "").toLowerCase();
  if (/MicroMessenger/i.test(ua)) return "wechat";
  if (/AlipayClient/i.test(ua)) return "alipay";
  if (/DingTalk/i.test(ua)) return "dingtalk";
  if (/Lark|Feishu/i.test(ua)) return "lark";
  if (/MQQBrowser|QQBrowser/i.test(ua)) return "qqbrowser";
  if (/SamsungBrowser/i.test(ua)) return "samsung browser";
  if (/HuaweiBrowser/i.test(ua)) return "huawei browser";
  if (/MiuiBrowser/i.test(ua)) return "miui browser";
  if (/UCBrowser/i.test(ua)) return "uc browser";
  if (/Quark/i.test(ua)) return "quark";
  if (/BaiduBrowser/i.test(ua)) return "baidu browser";
  if (/SogouMobileBrowser|MetaSr/i.test(ua)) return "sogou browser";
  if (/EdgA|EdgiOS/i.test(ua) || lower.includes("edg/") || lower.includes("edge/")) return "edge";
  if (lower.includes("opr/") || /Opera/i.test(ua)) return "opera";
  if (/FxiOS|Firefox/i.test(ua)) return "firefox";
  if (/CriOS/i.test(ua) || lower.includes("chrome/")) return "chrome";
  if (lower.includes("safari/")) return "safari";
  return "";
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
function currentPageHost() {{
  try {{
    return window.location && window.location.hostname ? window.location.hostname : "";
  }} catch (_) {{
    return "";
  }}
}}
function currentPageOrigin() {{
  try {{
    if (window.location && window.location.origin) return window.location.origin;
  }} catch (_) {{}}
  const cfg = window.__BIFROST_TRUST_PROBE__;
  return "http://" + effectiveProxyHost() + ":" + cfg.adminPort;
}}
function effectiveProxyHost() {{
  const host = currentPageHost();
  return host || window.__BIFROST_TRUST_PROBE__.host;
}}
function targetProxyAddress() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  return effectiveProxyHost() + ":" + cfg.adminPort;
}}
function renderTargetProxyAddress() {{
  const target = document.getElementById("target-proxy-address");
  if (target) target.textContent = targetProxyAddress();
}}
function isIosDevice() {{
  return detectPlatformOs(navigator.userAgent || "") === "ios";
}}
function setIosProxySetupVisible(visible) {{
  const target = document.getElementById("ios-wifi-proxy-tools");
  if (!target) return;
  target.hidden = !visible;
  target.classList.toggle("hidden", !visible);
  if (!visible) target.innerHTML = "";
}}
function shouldShowIosProxySetup() {{
  return isIosDevice() && !window.__BIFROST_TRUST_PROBE__.proxyConfigured;
}}
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
    if (typeof data.proxyConfigured === "boolean") cfg.proxyConfigured = data.proxyConfigured;
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
    let netcheckRoutedThroughProxy = false;
    if (!net.ok) {{
      const bypassRequired = await isTrustProbeProxyBypassRequired(net);
      if (bypassRequired) {{
        netcheckRoutedThroughProxy = true;
        show('<span class="ok">Proxy path detected.</span> Checking browser HTTPS probe...');
      }} else {{
        await postReport("network_failed", {{ status: net.status }});
        show('<span class="bad">Probe port is not reachable.</span>');
        showNext("<p>Check that this phone and computer are on the same network, the selected IP is correct, and firewall rules allow the probe port.</p><button onclick='runProbeLoop()'>Retry</button>");
        return false;
      }}
    }} else {{
      await postReport("netcheck_ok");
    }}
    if (netcheckRoutedThroughProxy) {{
      showNext("<p>This browser routed the HTTP reachability check through the configured proxy. Bifrost will still validate CA trust with the HTTPS probe.</p>");
    }} else if (!cfg.tlsFailed && !cfg.tlsTrusted) {{
      show('<span class="ok">Network check passed.</span> Checking browser HTTPS probe...');
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
      show('<span class="ok">Browser HTTPS probe passed. Some Android apps may still reject Bifrost CA.</span>');
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
async function isTrustProbeProxyBypassRequired(response) {{
  if (response.status !== 409) return false;
  try {{
    const data = await response.clone().json();
    return data && data.error === "trust_probe_must_bypass_proxy";
  }} catch (_) {{
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
      cfg.proxyConfigured = true;
      showProxyConfiguration('<span class="ok">Proxy is configured.</span><br><small>' + escapeText(data.message || "This browser reached Bifrost through the configured proxy.") + '</small>');
      renderIosWifiProxyTools();
      return;
    }}
    await postReport("proxy_config_failed", {{ status: response.status }});
    cfg.proxyConfigured = false;
    showProxyConfigurationMissing("Proxy check returned HTTP " + response.status + ".");
  }} catch (error) {{
    await postReport("proxy_config_failed", {{ message: String(error) }});
    cfg.proxyConfigured = false;
    showProxyConfigurationMissing("This browser did not reach Bifrost through the proxy.");
  }}
}}
function showProxyConfigurationMissing(message) {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const proxyAddress = targetProxyAddress();
  const iosSteps = isIosDevice()
    ? '<li>Recommended for iPhone: configure it manually in Settings &gt; Wi-Fi &gt; current network &gt; Configure Proxy &gt; Manual.</li>' +
      '<li>Use the experimental Wi-Fi profile only if you accept that uninstalling it may remove the managed Wi-Fi network entry.</li>'
    : '<li>Configure this device or browser to use the target HTTP proxy service.</li>';
  showProxyConfiguration(
    '<span class="bad">Proxy is not configured yet.</span><br>' +
    '<small>' + escapeText(message || "") + '</small>' +
    '<ol>' +
    iosSteps +
    '<li>Set the proxy target to <code>' + escapeText(proxyAddress) + '</code>, then rerun this check.</li>' +
    '</ol>' +
    (isIosDevice() ? '<button onclick="focusIosWifiSsid()">Show experimental profile option</button>' : '')
  );
  renderIosWifiProxyTools();
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
  return currentPageOrigin() + "/_bifrost/public/mobile/ios-wifi-proxy.mobileconfig" +
    "?ssid=" + encodeURIComponent(ssid) +
    "&ip=" + encodeURIComponent(effectiveProxyHost()) +
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
  hint.textContent = "This profile configures Wi-Fi proxy " + targetProxyAddress() + " for Wi-Fi \"" + window.__BIFROST_TRUST_PROBE__.suggestedWifiSsid + "\".";
}}
function renderIosWifiProxyTools() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  const target = document.getElementById("ios-wifi-proxy-tools");
  if (!target) return;
  if (!shouldShowIosProxySetup()) {{
    setIosProxySetupVisible(false);
    return;
  }}
  setIosProxySetupVisible(true);
  const ssid = String(cfg.suggestedWifiSsid || "").trim();
  const riskChecked = cfg.managedWifiRiskAccepted ? " checked" : "";
  const ssidHtml = ssid ? "<p>Wi-Fi name for this check: <code>" + escapeText(ssid) + "</code></p>" : "<p><span class='warn'>" + escapeText(cfg.suggestedWifiSsidMessage || "Bifrost could not detect this Mac's current Wi-Fi SSID.") + "</span></p>";
  const proxyHost = effectiveProxyHost();
  const proxyAddress = targetProxyAddress();
  target.innerHTML =
    "<h2>iOS Proxy Setup</h2>" +
    "<p><strong>Recommended:</strong> set the proxy manually in Settings &gt; Wi-Fi &gt; current network &gt; Configure Proxy &gt; Manual. Set Server to <code>" +
    escapeText(proxyHost) +
    "</code> and Port to <code>" +
    escapeText(String(cfg.adminPort)) +
    "</code>. Turn it back to Off when finished.</p>" +
    "<p><strong>Experimental profile:</strong> Bifrost can generate a managed Wi-Fi profile that sets the current Wi-Fi network proxy to <strong>" +
    escapeText(proxyAddress) +
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
  const value = targetProxyAddress();
  try {{
    await navigator.clipboard.writeText(value);
    document.getElementById("copy-status").textContent = "Copied";
  }} catch (_) {{
    document.getElementById("copy-status").textContent = "Select and copy manually";
  }}
}}
function showProxyConfig() {{
  const proxyAddress = targetProxyAddress();
  const proxyQrCodeUrl = currentPageOrigin() + "/_bifrost/public/proxy/qrcode?ip=" + encodeURIComponent(effectiveProxyHost());
  showNext("<p>Next configure this device proxy to:</p><button onclick='copyProxyAddress()'><strong>" + proxyAddress + "</strong></button><span id='copy-status'></span><p><a class='button' href='" + proxyQrCodeUrl + "'>Open proxy QR code</a></p>");
}}
function showTlsFailed() {{
  window.__BIFROST_TRUST_PROBE__.tlsFailed = true;
  window.__BIFROST_TRUST_PROBE__.tlsTrusted = false;
  const platform = detectPlatformOs(navigator.userAgent || "");
  show('<span class="bad">Browser HTTPS probe failed.</span><p><a class="button" href="' + window.__BIFROST_TRUST_PROBE__.caDownloadUrl + '">Download Bifrost CA</a></p>');
  let restartHint = "<p>If you just installed or trusted the CA, fully quit and restart this browser, then open this page again. Some browsers keep old certificate trust decisions until restart.</p>";
  let steps = "<p>Install and trust Bifrost CA, then return here and retry.</p>" + restartHint;
  if (platform === "ios") {{
    steps = "<ol><li>Install the Bifrost CA profile.</li><li>Open Settings &gt; General &gt; About &gt; Certificate Trust Settings.</li><li>Turn on full trust for Bifrost CA.</li><li>Fully quit and restart this browser, then retry.</li></ol>" + restartHint;
  }} else if (platform === "android") {{
    steps = "<ol><li>Install the Bifrost CA certificate.</li><li>Fully quit and restart this browser, then retry.</li><li>For Android apps, remember that some apps ignore user CAs or use certificate pinning.</li></ol>" + restartHint;
  }}
  showNext(steps + "<button onclick='runProbeLoop()'>Retry</button>");
}}
renderTargetProxyAddress();
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

fn is_loopback_probe_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn probe_target_hosts_match(session_host: &str, request_host: &str) -> bool {
    session_host == request_host
        || (is_loopback_probe_host(session_host) && is_loopback_probe_host(request_host))
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

/// Constant-time byte comparison to avoid leaking the token hash via timing.
/// Both inputs are fixed-length SHA-256 hex digests, but we still compare in
/// constant time to keep the auth path free of early-exit side channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

    fn test_probe_server_handle(port: u16) -> ProbeServerHandle {
        ProbeServerHandle {
            port,
            shutdown_tx: None,
            last_activity_ms: Arc::new(AtomicI64::new(now_epoch_millis())),
        }
    }

    fn test_probe_server_handle_with_activity(
        port: u16,
        last_activity_ms: Arc<AtomicI64>,
    ) -> ProbeServerHandle {
        ProbeServerHandle {
            port,
            shutdown_tx: None,
            last_activity_ms,
        }
    }

    #[test]
    fn token_hash_does_not_match_plain_token() {
        let token = "secret";
        assert_ne!(hash_token(token), token);
    }

    #[test]
    fn token_match_uses_constant_time_compare() {
        let session = test_session(Uuid::new_v4(), "real-token", Utc::now());
        // Wrong token rejected, correct token accepted (constant-time path).
        assert!(!session.token_matches("wrong"));
        assert!(session.token_matches("real-token"));
        // Empty token is intentionally still accepted for the public flow.
        assert!(session.token_matches(""));
    }

    #[test]
    fn constant_time_eq_behaves_like_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn infer_device_platform_hint_covers_common_os_browser_and_apps() {
        assert_eq!(
            infer_device_platform_hint("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36 Edg/149.0.0.0").as_deref(),
            Some("macos edge")
        );
        assert_eq!(
            infer_device_platform_hint("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36").as_deref(),
            Some("windows chrome")
        );
        assert_eq!(
            infer_device_platform_hint("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1").as_deref(),
            Some("ios safari")
        );
        assert_eq!(
            infer_device_platform_hint("Mozilla/5.0 (Linux; Android 15; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Mobile Safari/537.36 MicroMessenger/8.0.50").as_deref(),
            Some("android wechat")
        );
        assert_eq!(
            infer_device_platform_hint("Mozilla/5.0 (Linux; Android 15; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/28.0 Chrome/122.0 Mobile Safari/537.36").as_deref(),
            Some("android samsung browser")
        );
        assert_eq!(
            infer_device_platform_hint("CustomClient/1.0").as_deref(),
            Some("browser")
        );
    }

    #[test]
    fn landing_page_platform_detection_avoids_regex_slash_escaping() {
        let session = test_session(Uuid::new_v4(), "token", Utc::now());
        let html = render_landing_page(&session, "token");
        assert!(html.contains("lower.includes(\"edg/\")"));
        assert!(html.contains("lower.includes(\"chrome/\")"));
        assert!(!html.contains("\\\\/"));
    }

    #[test]
    fn landing_page_highlights_proxy_target_and_gates_ios_proxy_setup() {
        let session = test_session(Uuid::new_v4(), "token", Utc::now());
        let html = render_landing_page(&session, "token");

        assert!(html.contains("Target proxy service"));
        assert!(html.contains("id=\"target-proxy-address\""));
        assert!(html.contains("function currentPageHost()"));
        assert!(html.contains("function currentPageOrigin()"));
        assert!(html.contains("function effectiveProxyHost()"));
        assert!(html.contains("function targetProxyAddress()"));
        assert!(html.contains("return effectiveProxyHost() + \":\" + cfg.adminPort;"));
        assert!(html.contains("function renderTargetProxyAddress()"));
        assert!(html.contains("function shouldShowIosProxySetup()"));
        assert!(html
            .contains("return isIosDevice() && !window.__BIFROST_TRUST_PROBE__.proxyConfigured;"));
        assert!(html.contains("setIosProxySetupVisible(false);"));
        assert!(html.contains("cfg.proxyConfigured = true;"));
        assert!(html.contains("cfg.proxyConfigured = false;"));
        assert!(html.contains(
            "if (typeof data.proxyConfigured === \"boolean\") cfg.proxyConfigured = data.proxyConfigured;"
        ));
        assert!(html.contains("Set the proxy target to <code>"));
        assert!(html.contains("let netcheckRoutedThroughProxy = false;"));
        assert!(html.contains("Proxy path detected."));
        assert!(html.contains("Bifrost will still validate CA trust with the HTTPS probe."));
        assert!(html.contains(
            "currentPageOrigin() + \"/_bifrost/public/mobile/ios-wifi-proxy.mobileconfig\""
        ));
        assert!(html.contains("\"&ip=\" + encodeURIComponent(effectiveProxyHost())"));
        assert!(html.contains(
            "currentPageOrigin() + \"/_bifrost/public/proxy/qrcode?ip=\" + encodeURIComponent(effectiveProxyHost())"
        ));
        assert!(!html.contains("Direct probe request went through the configured proxy."));
    }

    #[test]
    fn landing_page_preserves_initial_proxy_configured_state() {
        let mut session = test_session(Uuid::new_v4(), "token", Utc::now());
        session.proxy_configured = true;

        let html = render_landing_page(&session, "token");

        assert!(html.contains("\"proxyConfigured\":true"));
    }

    #[test]
    fn manager_matches_only_active_probe_target() {
        let manager = TrustProbeManager::new();
        let now = Utc::now();
        let session = test_session(Uuid::new_v4(), "token", now);
        manager.sessions.lock().insert(session.id, session);

        assert!(manager.is_active_probe_target("127.0.0.1", 8802));
        assert!(manager.is_active_probe_target("localhost", 8802));
        assert!(manager.is_active_probe_target("[::1]", 8802));
        assert!(!manager.is_active_probe_target("127.0.0.1", 8803));
        assert!(!manager.is_active_probe_target("10.0.0.8", 8802));
    }

    #[test]
    fn cleanup_keeps_probe_server_for_each_active_host() {
        let manager = TrustProbeManager::new();
        let now = Utc::now();
        let active = test_session(Uuid::new_v4(), "active", now);
        let mut expired = test_session(Uuid::new_v4(), "expired", now);
        expired.host = "10.0.0.8".to_string();
        expired.expires_at = now - chrono::Duration::seconds(1);

        manager.sessions.lock().insert(active.id, active);
        manager.sessions.lock().insert(expired.id, expired);
        manager.servers.lock().insert(
            ProbeServerKey {
                host: "127.0.0.1".to_string(),
                ca_fingerprint_sha256: None,
            },
            test_probe_server_handle(8802),
        );
        manager.servers.lock().insert(
            ProbeServerKey {
                host: "10.0.0.8".to_string(),
                ca_fingerprint_sha256: None,
            },
            test_probe_server_handle(8802),
        );

        manager.cleanup_expired_sessions();
        let servers = manager.servers.lock();
        assert!(servers.contains_key(&ProbeServerKey {
            host: "127.0.0.1".to_string(),
            ca_fingerprint_sha256: None,
        }));
        assert!(!servers.contains_key(&ProbeServerKey {
            host: "10.0.0.8".to_string(),
            ca_fingerprint_sha256: None,
        }));
    }

    #[test]
    fn update_probe_port_for_group_updates_only_matching_active_sessions() {
        let manager = TrustProbeManager::new();
        let now = Utc::now();
        let mut matching = test_session(Uuid::new_v4(), "matching", now);
        matching.admin_port = 8800;
        matching.probe_port = 8802;
        matching.ca_fingerprint_sha256 = Some("current-ca".to_string());

        let mut same_group = test_session(Uuid::new_v4(), "same-group", now);
        same_group.admin_port = 8800;
        same_group.probe_port = 8802;
        same_group.ca_fingerprint_sha256 = Some("current-ca".to_string());

        let mut different_ca = test_session(Uuid::new_v4(), "different-ca", now);
        different_ca.admin_port = 8800;
        different_ca.probe_port = 8802;
        different_ca.ca_fingerprint_sha256 = Some("old-ca".to_string());

        let mut expired = test_session(Uuid::new_v4(), "expired", now);
        expired.admin_port = 8800;
        expired.probe_port = 8802;
        expired.ca_fingerprint_sha256 = Some("current-ca".to_string());
        expired.expires_at = now - chrono::Duration::seconds(1);

        let matching_id = matching.id;
        let same_group_id = same_group.id;
        let different_ca_id = different_ca.id;
        let expired_id = expired.id;
        manager.sessions.lock().insert(matching_id, matching);
        manager.sessions.lock().insert(same_group_id, same_group);
        manager
            .sessions
            .lock()
            .insert(different_ca_id, different_ca);
        manager.sessions.lock().insert(expired_id, expired);

        manager.update_probe_port_for_group(
            "127.0.0.1",
            8800,
            &Some("current-ca".to_string()),
            49152,
        );

        let sessions = manager.sessions.lock();
        assert_eq!(sessions.get(&matching_id).unwrap().probe_port, 49152);
        assert_eq!(sessions.get(&same_group_id).unwrap().probe_port, 49152);
        assert_eq!(sessions.get(&different_ca_id).unwrap().probe_port, 8802);
        assert_eq!(sessions.get(&expired_id).unwrap().probe_port, 8802);
    }

    #[tokio::test]
    async fn probe_server_port_listening_probe_tracks_local_tcp_listener() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind test listener");
        let port = listener.local_addr().expect("test listener address").port();

        assert!(probe_server_port_is_listening(port).await);
        assert!(!probe_server_port_is_listening(0).await);
    }

    #[tokio::test]
    async fn ensure_probe_server_reuses_healthy_existing_port_without_rebinding() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind test listener");
        let port = listener.local_addr().expect("test listener address").port();
        let manager = TrustProbeManager::new();
        manager.servers.lock().insert(
            ProbeServerKey {
                host: "127.0.0.1".to_string(),
                ca_fingerprint_sha256: None,
            },
            test_probe_server_handle(port),
        );

        let ensured = manager
            .ensure_probe_server(
                "127.0.0.1",
                port.saturating_add(1),
                Path::new("/missing-ca-cert.pem"),
                Path::new("/missing-ca-key.pem"),
                None,
            )
            .await
            .expect("healthy existing listener should be reused before CA loading");

        assert_eq!(ensured, port);
        assert_eq!(manager.servers.lock().len(), 1);
    }

    #[tokio::test]
    async fn ensure_probe_server_removes_stale_existing_port_before_rebinding() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind test listener");
        let port = listener.local_addr().expect("test listener address").port();
        drop(listener);
        let manager = TrustProbeManager::new();
        manager.servers.lock().insert(
            ProbeServerKey {
                host: "127.0.0.1".to_string(),
                ca_fingerprint_sha256: None,
            },
            test_probe_server_handle(port),
        );

        let result = manager
            .ensure_probe_server(
                "127.0.0.1",
                port,
                Path::new("/missing-ca-cert.pem"),
                Path::new("/missing-ca-key.pem"),
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(manager.servers.lock().is_empty());
    }

    #[test]
    fn idle_probe_reaper_removes_matching_idle_listener() {
        let servers = Arc::new(Mutex::new(HashMap::new()));
        let key = ProbeServerKey {
            host: "127.0.0.1".to_string(),
            ca_fingerprint_sha256: None,
        };
        let activity = Arc::new(AtomicI64::new(
            now_epoch_millis() - PROBE_SERVER_IDLE_TTL.as_millis() as i64 - 1,
        ));
        servers.lock().insert(
            key.clone(),
            test_probe_server_handle_with_activity(8802, activity.clone()),
        );

        assert!(shutdown_idle_probe_server_if_due(
            &servers,
            &key,
            8802,
            &activity,
            PROBE_SERVER_IDLE_TTL
        ));
        assert!(servers.lock().is_empty());
    }

    #[test]
    fn idle_probe_reaper_does_not_remove_replaced_listener() {
        let servers = Arc::new(Mutex::new(HashMap::new()));
        let key = ProbeServerKey {
            host: "127.0.0.1".to_string(),
            ca_fingerprint_sha256: None,
        };
        let old_activity = Arc::new(AtomicI64::new(
            now_epoch_millis() - PROBE_SERVER_IDLE_TTL.as_millis() as i64 - 1,
        ));
        let replacement_activity = Arc::new(AtomicI64::new(now_epoch_millis()));
        servers.lock().insert(
            key.clone(),
            test_probe_server_handle_with_activity(8802, replacement_activity),
        );

        assert!(shutdown_idle_probe_server_if_due(
            &servers,
            &key,
            8802,
            &old_activity,
            PROBE_SERVER_IDLE_TTL
        ));
        assert!(servers.lock().contains_key(&key));
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
        assert_eq!(
            view.devices
                .iter()
                .map(|device| device.client_ip.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["192.168.1.20", "192.168.1.21"]
        );
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

    #[test]
    fn parse_public_probe_path_parses_session_and_optional_action() {
        let session_id = Uuid::new_v4();
        let with_action = format!("/public/trust-probe/{}/open", session_id);
        let without_action = format!("/public/trust-probe/{}", session_id);

        let parsed_with_action = parse_public_probe_path(&with_action).expect("with action");
        assert_eq!(parsed_with_action.0, session_id);
        assert_eq!(parsed_with_action.1.as_deref(), Some("open"));

        let parsed_without_action =
            parse_public_probe_path(&without_action).expect("without action");
        assert_eq!(parsed_without_action.0, session_id);
        assert!(parsed_without_action.1.is_none());

        assert!(parse_public_probe_path("/public/trust-probe/not-a-uuid").is_none());
        assert!(parse_public_probe_path("/public/trust-probe/1234/extra/segment").is_none());
    }

    #[test]
    fn normalize_device_id_validates_format_and_length() {
        assert_eq!(
            normalize_device_id(Some(" dev-123_Abc ".to_string())).as_deref(),
            Some("dev-123_Abc")
        );
        assert!(normalize_device_id(Some("".to_string())).is_none());
        assert!(normalize_device_id(Some("   ".to_string())).is_none());
        let long_id = "x".repeat(97);
        assert!(normalize_device_id(Some(long_id)).is_none());
        assert!(normalize_device_id(Some("invalid id!".to_string())).is_none());
        assert!(normalize_device_id(None).is_none());
    }

    #[test]
    fn query_param_decodes_urlencoded_values() {
        let query = "t=hello%20world&other=x";
        assert_eq!(
            query_param(Some(query), "t").as_deref(),
            Some("hello world")
        );
        assert_eq!(query_param(Some(query), "missing"), None);
        assert_eq!(query_param(None, "t"), None);
    }

    #[test]
    fn probe_target_hosts_match_treats_loopback_hosts_as_equivalent() {
        assert!(probe_target_hosts_match("127.0.0.1", "127.0.0.1"));
        assert!(probe_target_hosts_match("localhost", "127.0.0.1"));
        assert!(probe_target_hosts_match("127.0.0.1", "localhost"));
        assert!(probe_target_hosts_match("::1", "127.0.0.1"));
        assert!(probe_target_hosts_match("[::1]", "localhost"));
        assert!(!probe_target_hosts_match("127.0.0.1", "10.0.0.1"));
    }

    #[test]
    fn escape_html_escapes_special_characters() {
        let raw = "<a href=\"x&y\">O'Reilly</a>";
        let escaped = escape_html(raw);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(escaped.contains("&lt;a href=&quot;x&amp;y&quot;"));
        assert!(escaped.contains("O&#39;Reilly"));
    }

    #[test]
    fn validate_probe_host_handles_invalid_and_loopback_hosts() {
        let err = validate_probe_host("not-an-ip").unwrap_err();
        assert!(err.contains(
            "Availability check host must be one of this computer's local IP addresses."
        ));

        let ok = validate_probe_host("127.0.0.1").unwrap();
        assert_eq!(ok, "127.0.0.1");
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use bytes::Bytes;
    use chrono::Utc;
    use http_body_util::BodyExt;
    use hyper::StatusCode;
    use std::cmp::Ordering;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicI64;
    use std::sync::Arc;

    fn make_session(now: chrono::DateTime<Utc>) -> TrustProbeSession {
        TrustProbeSession {
            id: Uuid::new_v4(),
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
    fn normalize_platform_hint_prefers_non_unknown_hint() {
        let hint = normalize_platform_hint(Some("  android wechat  ".to_string()), None);
        assert_eq!(hint.as_deref(), Some("android wechat"));
    }

    #[test]
    fn normalize_platform_hint_uses_user_agent_when_hint_unknown() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        let hint = normalize_platform_hint(Some(" unknown ".to_string()), Some(ua));
        assert_eq!(hint.as_deref(), Some("ios safari"));
    }

    #[test]
    fn normalize_platform_hint_returns_none_for_empty_hint_and_no_user_agent() {
        let hint = normalize_platform_hint(Some("   ".to_string()), None);
        assert!(hint.is_none());
    }

    #[test]
    fn is_unknown_platform_hint_recognizes_variants() {
        assert!(is_unknown_platform_hint("unknown"));
        assert!(is_unknown_platform_hint("Unknown Browser"));
        assert!(is_unknown_platform_hint(" unknown device "));
        assert!(!is_unknown_platform_hint("android"));
    }

    #[test]
    fn compare_optional_ip_orders_real_ips() {
        assert_eq!(
            compare_optional_ip(Some("10.0.0.1"), Some("127.0.0.1")),
            Ordering::Less
        );
        assert_eq!(
            compare_optional_ip(Some("127.0.0.1"), Some("10.0.0.1")),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_optional_ip_treats_some_as_less_than_none() {
        assert_eq!(compare_optional_ip(Some("127.0.0.1"), None), Ordering::Less);
        assert_eq!(
            compare_optional_ip(None, Some("127.0.0.1")),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_optional_ip_uses_string_order_for_invalid_ips() {
        let left = "zzz";
        let right = "aaa";
        assert_eq!(
            compare_optional_ip(Some(left), Some(right)),
            left.cmp(right)
        );
    }

    #[test]
    fn device_events_are_bounded_to_32_entries() {
        let now = Utc::now();
        let mut device = TrustProbeDeviceState::new("dev".to_string(), now);
        for i in 0..40 {
            device.apply_event("page_opened", Some(format!("event-{i}")), None, None, None);
        }
        assert_eq!(device.events.len(), 32);
        assert_eq!(
            device.events[0].message.as_deref(),
            Some("event-8"),
            "oldest events should be evicted when more than 32 are stored",
        );
        assert_eq!(
            device.events.last().unwrap().message.as_deref(),
            Some("event-39"),
        );
    }

    #[test]
    fn session_events_are_bounded_to_64_entries() {
        let now = Utc::now();
        let mut session = make_session(now);
        for i in 0..80 {
            session.apply_event(
                "page_opened",
                Some(format!("event-{i}")),
                None,
                None,
                None,
                None,
            );
        }
        assert_eq!(session.events.len(), 64);
        assert_eq!(session.events[0].message.as_deref(), Some("event-16"),);
        assert_eq!(
            session.events.last().unwrap().message.as_deref(),
            Some("event-79"),
        );
    }

    #[test]
    fn session_apply_event_does_not_override_proxy_configuration_on_failure() {
        let mut session = make_session(Utc::now());
        session.apply_event(
            "proxy_configured_ok",
            Some("Proxy OK".to_string()),
            None,
            None,
            None,
            None,
        );
        session.apply_event(
            "proxy_config_failed",
            Some("Should be ignored".to_string()),
            None,
            None,
            None,
            None,
        );

        assert!(session.proxy_configured);
        assert_eq!(
            session.proxy_configuration_message.as_deref(),
            Some("Proxy OK"),
        );
    }

    #[test]
    fn session_apply_event_network_failed_does_not_override_tls_trusted() {
        let mut session = make_session(Utc::now());
        session.status = TrustProbeStatus::TlsTrusted;
        session.tls_trusted = true;
        session.last_error = Some("existing".to_string());

        session.apply_event(
            "network_failed",
            Some("new network error".to_string()),
            None,
            None,
            None,
            None,
        );

        assert_eq!(session.status, TrustProbeStatus::TlsTrusted);
        assert_eq!(session.last_error.as_deref(), Some("existing"));
    }

    #[test]
    fn manager_get_public_session_rejects_wrong_token_and_expired() {
        let manager = TrustProbeManager::new();
        let now = Utc::now();
        let mut active = make_session(now);
        active.token_hash = hash_token("good-token");
        active.expires_at = now + chrono::Duration::minutes(5);
        let active_id = active.id;
        manager.sessions.lock().insert(active_id, active);

        assert!(manager
            .get_public_session(active_id, "good-token")
            .is_some());
        assert!(manager.get_public_session(active_id, "bad-token").is_none());

        let mut expired = make_session(now);
        expired.id = Uuid::new_v4();
        expired.token_hash = hash_token("other-token");
        expired.expires_at = now - chrono::Duration::minutes(1);
        let expired_id = expired.id;
        manager.sessions.lock().insert(expired_id, expired);

        assert!(manager
            .get_public_session(expired_id, "other-token")
            .is_none());
    }

    #[test]
    fn manager_list_sessions_sorts_by_expiry_and_omits_expired() {
        let manager = TrustProbeManager::new();
        let now = Utc::now();
        let mut a = make_session(now);
        a.id = Uuid::new_v4();
        a.expires_at = now + chrono::Duration::seconds(30);
        let mut b = make_session(now);
        b.id = Uuid::new_v4();
        b.expires_at = now + chrono::Duration::seconds(60);
        let mut expired = make_session(now);
        expired.id = Uuid::new_v4();
        expired.expires_at = now - chrono::Duration::seconds(1);

        manager.sessions.lock().insert(a.id, a);
        manager.sessions.lock().insert(b.id, b);
        manager.sessions.lock().insert(expired.id, expired);

        let views = manager.list_sessions();
        assert_eq!(views.len(), 2);
        assert!(views[0].expires_at >= views[1].expires_at);
        assert_ne!(views[0].session_id, views[1].session_id);
    }

    #[test]
    fn manager_record_report_populates_default_message_and_wifi_ssid() {
        let manager = TrustProbeManager::new();
        let now = Utc::now();
        let mut session = make_session(now);
        session.id = Uuid::new_v4();
        session.token_hash = hash_token("report-token");
        let session_id = session.id;
        manager.sessions.lock().insert(session_id, session);

        let ok = manager.record_report(
            session_id,
            "report-token",
            TrustProbeReport {
                event_type: "network_failed".to_string(),
                message: None,
                user_agent: Some("UA-string".to_string()),
                platform_hint: Some("android".to_string()),
                status: Some(503),
                wifi_ssid: Some("Office Wi-Fi".to_string()),
                device_id: Some("dev-1".to_string()),
            },
            Some("192.168.1.10".to_string()),
            Some("Header-UA".to_string()),
        );
        assert!(ok);

        let sessions = manager.sessions.lock();
        let s = sessions.get(&session_id).unwrap();
        assert_eq!(s.suggested_wifi_ssid.as_deref(), Some("Office Wi-Fi"));
        assert!(s
            .suggested_wifi_ssid_message
            .as_deref()
            .unwrap_or_default()
            .contains("provided by the user"));
        assert_eq!(s.client_ip.as_deref(), Some("192.168.1.10"));
        assert_eq!(s.user_agent.as_deref(), Some("UA-string"));
        assert!(s.events.iter().any(|e| e.event_type == "network_failed"
            && e.message.as_deref() == Some("Probe request returned HTTP 503")));
    }

    #[test]
    fn render_landing_page_with_empty_token_does_not_append_query() {
        let mut session = make_session(Utc::now());
        session.id = Uuid::new_v4();
        let html = render_landing_page(&session, "");
        assert!(!html.contains("?t="));
        assert!(!html.contains("&t="));
    }

    #[test]
    fn render_landing_page_includes_ca_fingerprint_when_present() {
        let mut session = make_session(Utc::now());
        session.ca_fingerprint_sha256 = Some("AA:BB:CC".to_string());
        let html = render_landing_page(&session, "token");
        assert!(html.contains("AA:BB:CC"));
    }

    #[tokio::test]
    async fn render_qrcode_for_url_returns_svg_response() {
        let resp = render_qrcode_for_url("https://example.invalid/path");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok()),
            Some("image/svg+xml"),
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let svg = String::from_utf8_lossy(&body);
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn render_probe_qrcode_head_returns_not_found_without_session() {
        let session_id = Uuid::new_v4();
        let resp = render_probe_qrcode_head(session_id, "");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn touch_probe_activity_updates_timestamp() {
        let last_activity = AtomicI64::new(0);
        touch_probe_activity(&last_activity);
        assert!(last_activity.load(std::sync::atomic::Ordering::Relaxed) > 0);
    }

    #[test]
    fn ca_key_path_from_cert_path_changes_filename() {
        let cert = PathBuf::from("/tmp/custom/ca.crt");
        let key = ca_key_path_from_cert_path(&cert);
        assert_eq!(key.file_name().unwrap().to_string_lossy(), "ca.key");
    }

    #[test]
    fn certificate_sha256_fingerprint_returns_none_for_missing_file() {
        let path = PathBuf::from("this-file-should-not-exist-for-test.pem");
        let fingerprint = certificate_sha256_fingerprint(&path);
        assert!(fingerprint.is_none());
    }

    #[test]
    fn current_wifi_ssid_detection_non_macos_uses_fallback_message() {
        if cfg!(target_os = "macos") {
            // On macOS we only verify that the detection call does not panic.
            let _ = current_wifi_ssid_detection();
            return;
        }
        let detection = current_wifi_ssid_detection();
        assert!(detection.ssid.is_none());
        assert!(detection
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("macOS"));
    }

    #[test]
    fn trust_probe_status_serde_uses_snake_case() {
        let json = serde_json::to_string(&TrustProbeStatus::PageOpened).unwrap();
        assert_eq!(json, "\"page_opened\"");
    }

    #[test]
    fn trust_probe_proxy_access_status_serde_uses_snake_case() {
        let json = serde_json::to_string(&TrustProbeProxyAccessStatus::Allowed).ok();
        // If the enum layout ever changes this will catch it.
        assert!(json.as_deref().is_some_and(|s| s == "\"allowed\""));
    }

    #[test]
    fn trust_probe_report_deserializes_alias_fields() {
        let json = r#"{
            "type": "tls_failed",
            "message": "failed",
            "userAgent": "UA",
            "platformHint": "android",
            "status": 418,
            "wifiSsid": "Office Wi-Fi",
            "deviceId": "dev-123"
        }"#;
        let report: TrustProbeReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.event_type, "tls_failed");
        assert_eq!(report.message.as_deref(), Some("failed"));
        assert_eq!(report.user_agent.as_deref(), Some("UA"));
        assert_eq!(report.platform_hint.as_deref(), Some("android"));
        assert_eq!(report.status, Some(418));
        assert_eq!(report.wifi_ssid.as_deref(), Some("Office Wi-Fi"));
        assert_eq!(report.device_id.as_deref(), Some("dev-123"));
    }

    #[test]
    fn create_trust_probe_session_request_deserializes_ttl_seconds() {
        let json = r#"{ "host": "127.0.0.1", "ttlSeconds": 120 }"#;
        let req: CreateTrustProbeSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.host, "127.0.0.1");
        assert_eq!(req.ttl_seconds, Some(120));
    }

    #[test]
    fn update_trust_probe_session_request_deserializes_wifi_ssid() {
        let json = r#"{ "wifiSsid": "Office Wi-Fi" }"#;
        let req: UpdateTrustProbeSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.wifi_ssid.as_deref(), Some("Office Wi-Fi"));
    }

    #[test]
    fn device_view_accessors_expose_inner_state() {
        let now = Utc::now();
        let mut state = TrustProbeDeviceState::new("dev-123".to_string(), now);
        state.status = TrustProbeStatus::NetworkReachable;
        state.opened = true;
        state.proxy_access_status = Some(TrustProbeProxyAccessStatus::Allowed);
        state.proxy_configured = true;
        state.network_reachable = true;
        state.tls_trusted = true;
        state.client_ip = Some("192.168.1.20".to_string());
        state.user_agent = Some("UA".to_string());
        state.platform_hint = Some("android".to_string());

        let view = state.to_view();
        assert_eq!(view.device_id(), "dev-123");
        assert_eq!(view.status(), TrustProbeStatus::NetworkReachable);
        assert!(view.opened());
        assert_eq!(
            view.proxy_access_status(),
            Some(TrustProbeProxyAccessStatus::Allowed)
        );
        assert!(view.proxy_configured());
        assert!(view.network_reachable());
        assert!(view.tls_trusted());
        assert_eq!(view.client_ip(), Some("192.168.1.20"));
        assert_eq!(view.user_agent(), Some("UA"));
        assert_eq!(view.platform_hint(), Some("android"));
    }

    #[test]
    fn session_view_accessors_expose_inner_state() {
        let now = Utc::now();
        let mut session = make_session(now);
        session.host = "10.0.0.8".to_string();
        session.admin_port = 8801;
        let view = session.to_view("token");

        assert_eq!(view.host(), "10.0.0.8");
        assert!(view.landing_url().contains("10.0.0.8"));
        assert!(view.landing_url().contains(":8801/"));
    }

    #[test]
    fn hash_token_is_deterministic_and_case_sensitive() {
        let a = hash_token("secret");
        let b = hash_token("secret");
        let c = hash_token("Secret");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;

    use chrono::Utc;
    use hyper::{body::Incoming, Method, Request, StatusCode};
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use tokio::net::TcpListener;

    use crate::test_support::TestAdminState;

    async fn spawn_trust_probe_api_server(
        state: SharedAdminState,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind trust-probe api listener");
        let addr = listener.local_addr().expect("api listener addr");
        let base = format!("http://{}", addr);
        let state_clone = state.clone();

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                let state_inner = state_clone.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let state = state_inner.clone();
                        async move {
                            let path = req.uri().path().to_string();
                            let resp =
                                handle_trust_probe_api(req, state, None, &path).await;
                            Ok::<_, hyper::Error>(resp)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        (base, handle)
    }

    async fn spawn_trust_probe_public_server(
        state: SharedAdminState,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind trust-probe public listener");
        let addr = listener.local_addr().expect("public listener addr");
        let base = format!("http://{}", addr);
        let state_clone = state.clone();

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                let state_inner = state_clone.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let state = state_inner.clone();
                        async move {
                            let path = req.uri().path().to_string();
                            let resp =
                                handle_trust_probe_public(req, state, None, &path).await;
                            Ok::<_, hyper::Error>(resp)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        (base, handle)
    }

    async fn spawn_proxy_configured_server(
        peer_addr: SocketAddr,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind proxy-configured listener");
        let addr = listener.local_addr().expect("proxy listener addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                let peer = peer_addr;
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let peer = peer;
                        async move {
                            let resp =
                                handle_trust_probe_proxy_configured_request(req, peer).await;
                            Ok::<_, hyper::Error>(resp)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        (base, handle)
    }

    async fn spawn_probe_request_server(
        peer_addr: SocketAddr,
        is_tls: bool,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind probe-request listener");
        let addr = listener.local_addr().expect("probe listener addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                let peer = peer_addr;
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let peer = peer;
                        async move { handle_probe_request(req, peer, is_tls).await }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        (base, handle)
    }

    #[tokio::test]
    async fn api_create_session_without_ca_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_api_server(state).await;

        let client = reqwest::Client::new();
        let body = serde_json::json!({ "host": "127.0.0.1", "ttlSeconds": 60 });
        let resp = client
            .post(format!("{}/api/trust-probe/sessions", base))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn api_list_sessions_returns_ok() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_api_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/api/trust-probe/sessions", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        handle.abort();
    }

    #[tokio::test]
    async fn api_get_session_with_invalid_id_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_api_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/api/trust-probe/sessions/not-a-uuid", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn api_update_unknown_session_returns_not_found() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_api_server(state).await;

        let client = reqwest::Client::new();
        let session_id = Uuid::new_v4();
        let body = serde_json::json!({ "wifiSsid": "Office Wi-Fi" });
        let resp = client
            .patch(format!(
                "{}/api/trust-probe/sessions/{}",
                base, session_id
            ))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_options_returns_cors_preflight() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .request(Method::OPTIONS, format!("{}/public/trust-probe", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_fixed_landing_without_ca_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/public/trust-probe", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_invalid_session_path_returns_not_found() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/public/trust-probe/not-a-uuid", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        handle.abort();
    }

    #[tokio::test]
    async fn proxy_configured_missing_sid_returns_bad_request() {
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 8080));
        let (base, handle) = spawn_proxy_configured_server(peer).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/_bifrost/trust-probe/proxy-configured", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn proxy_configured_unknown_session_returns_not_found() {
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 8080));
        let (base, handle) = spawn_proxy_configured_server(peer).await;
        let session_id = Uuid::new_v4();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!(
                "{}/_bifrost/trust-probe/proxy-configured?sid={}&t=tok",
                base, session_id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        handle.abort();
    }

    #[tokio::test]
    async fn probe_request_netcheck_over_http_returns_ok() {
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 9000));
        let (base, handle) = spawn_probe_request_server(peer, false).await;
        let session_id = Uuid::new_v4();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!(
                "{}/_bifrost/trust-probe/netcheck?sid={}",
                base, session_id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        handle.abort();
    }

    #[tokio::test]
    async fn probe_request_check_without_tls_returns_bad_request() {
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 9001));
        let (base, handle) = spawn_probe_request_server(peer, false).await;
        let session_id = Uuid::new_v4();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!(
                "{}/_bifrost/trust-probe/check?sid={}",
                base, session_id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn probe_request_check_with_tls_flag_returns_trusted_true() {
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 9002));
        let (base, handle) = spawn_probe_request_server(peer, true).await;
        let session_id = Uuid::new_v4();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!(
                "{}/_bifrost/trust-probe/check?sid={}",
                base, session_id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["trusted"], true);

        handle.abort();
    }

    #[tokio::test]
    async fn probe_request_missing_sid_returns_bad_request() {
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 9003));
        let (base, handle) = spawn_probe_request_server(peer, false).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/_bifrost/trust-probe/netcheck", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn public_probe_host_from_request_parses_ipv6_and_ipv4() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind helper listener");
        let addr = listener.local_addr().expect("helper addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| async move {
                        let host = public_probe_host_from_request(&req);
                        let body = serde_json::json!({ "host": host });
                        Ok::<_, hyper::Error>(probe_json_response(StatusCode::OK, body))
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client = reqwest::Client::new();
        // IPv4 host without brackets
        let resp = client
            .get(format!("{}/ipv4", base))
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["host"].as_str(), Some("127.0.0.1"));

        // IPv6 literal encoded in the Host header while connecting over IPv4.
        let resp = client
            .get(format!("{}/ipv6", base))
            .header(hyper::header::HOST, "[::1]:8080")
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["host"].as_str(), Some("::1"));

        handle.abort();
    }

    #[tokio::test]
    async fn request_client_ip_prefers_bifrost_header() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind ip helper listener");
        let addr = listener.local_addr().expect("ip helper addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| async move {
                        let ip = request_client_ip(&req);
                        let body = serde_json::json!({ "ip": ip });
                        Ok::<_, hyper::Error>(probe_json_response(StatusCode::OK, body))
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/ip", base))
            .header("x-bifrost-peer-ip", "192.168.1.10")
            .header("x-forwarded-for", "10.0.0.1")
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["ip"].as_str(), Some("192.168.1.10"));

        handle.abort();
    }

    #[tokio::test]
    async fn request_client_ip_uses_first_forwarded_entry() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind ip helper listener");
        let addr = listener.local_addr().expect("ip helper addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| async move {
                        let ip = request_client_ip(&req);
                        let body = serde_json::json!({ "ip": ip });
                        Ok::<_, hyper::Error>(probe_json_response(StatusCode::OK, body))
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/ip", base))
            .header("x-forwarded-for", "10.0.0.1, 10.0.0.2")
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["ip"].as_str(), Some("10.0.0.1"));

        handle.abort();
    }

    #[tokio::test]
    async fn request_client_ip_addr_parses_valid_ip_and_rejects_invalid() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind ip-addr helper listener");
        let addr = listener.local_addr().expect("ip-addr helper addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| async move {
                        let ip = request_client_ip_addr(&req);
                        let body = serde_json::json!({ "ip": ip.map(|v| v.to_string()) });
                        Ok::<_, hyper::Error>(probe_json_response(StatusCode::OK, body))
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client = reqwest::Client::new();
        // Valid IP
        let resp = client
            .get(format!("{}/ip", base))
            .header("x-bifrost-peer-ip", "127.0.0.1")
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["ip"].as_str(), Some("127.0.0.1"));

        // Invalid IP should yield null
        let resp = client
            .get(format!("{}/ip-invalid", base))
            .header("x-bifrost-peer-ip", "not-an-ip")
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert!(json["ip"].is_null());

        handle.abort();
    }

    #[test]
    fn is_loopback_probe_host_matches_variants() {
        assert!(is_loopback_probe_host("localhost"));
        assert!(is_loopback_probe_host("127.0.0.1"));
        assert!(is_loopback_probe_host("::1"));
        assert!(is_loopback_probe_host("[::1]"));
        assert!(!is_loopback_probe_host("10.0.0.1"));
    }

    #[test]
    fn probe_target_hosts_match_handles_loopback_equivalence() {
        assert!(probe_target_hosts_match("127.0.0.1", "localhost"));
        assert!(probe_target_hosts_match("localhost", "127.0.0.1"));
        assert!(probe_target_hosts_match("::1", "127.0.0.1"));
        assert!(!probe_target_hosts_match("127.0.0.1", "10.0.0.1"));
    }

    #[test]
    fn now_epoch_millis_is_close_to_utc_now() {
        let now_fn = now_epoch_millis();
        let now_real = Utc::now().timestamp_millis();
        assert!((now_real - now_fn).abs() < 5_000);
    }

    #[tokio::test]
    async fn api_unknown_trust_probe_path_returns_method_not_allowed() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_api_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/trust-probe/unknown", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);

        handle.abort();
    }

    #[tokio::test]
    async fn api_non_trust_probe_path_returns_not_found() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_api_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/api/other", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_fixed_landing_head_without_ca_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .request(Method::HEAD, format!("{}/public/trust-probe", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_fixed_qrcode_without_ca_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/public/trust-probe/qrcode", base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_qrcode_head_unknown_session_returns_not_found() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;
        let session_id = Uuid::new_v4();

        let client = reqwest::Client::new();
        let resp = client
            .request(
                Method::HEAD,
                format!("{}/public/trust-probe/{}/qrcode", base, session_id),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        handle.abort();
    }

    #[tokio::test]
    async fn public_trust_probe_session_unknown_session_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (base, handle) = spawn_trust_probe_public_server(state).await;
        let session_id = Uuid::new_v4();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/public/trust-probe/{}/session", base, session_id))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        handle.abort();
    }

    #[test]
    fn probe_response_sets_expected_cors_headers() {
        let resp = probe_response(StatusCode::OK, "body");
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers
                .get("Access-Control-Allow-Origin")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
        );
        assert_eq!(
            headers
                .get("Content-Type")
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; charset=utf-8"),
        );
    }

    #[test]
    fn probe_json_response_sets_expected_cors_and_json_headers() {
        let resp = probe_json_response(StatusCode::OK, serde_json::json!({ "ok": true }));
        let headers = resp.headers();
        assert_eq!(
            headers
                .get("Access-Control-Allow-Methods")
                .and_then(|v| v.to_str().ok()),
            Some("GET, OPTIONS"),
        );
        assert_eq!(
            headers
                .get("Content-Type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json"),
        );
    }

    #[tokio::test]
    async fn request_client_ip_returns_none_when_headers_missing() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind ip-missing helper listener");
        let addr = listener.local_addr().expect("ip-missing helper addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| async move {
                        let ip = request_client_ip(&req);
                        let body = serde_json::json!({ "ip": ip });
                        Ok::<_, hyper::Error>(probe_json_response(StatusCode::OK, body))
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/ip-none", base))
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert!(json["ip"].is_null());

        handle.abort();
    }

    #[tokio::test]
    async fn request_client_ip_trims_whitespace_and_uses_first_entry() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind ip-trim helper listener");
        let addr = listener.local_addr().expect("ip-trim helper addr");
        let base = format!("http://{}", addr);

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| async move {
                        let ip = request_client_ip(&req);
                        let body = serde_json::json!({ "ip": ip });
                        Ok::<_, hyper::Error>(probe_json_response(StatusCode::OK, body))
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/ip-trim", base))
            .header("x-forwarded-for", " 10.0.0.1 , 10.0.0.2 ")
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(json["ip"].as_str(), Some("10.0.0.1"));

        handle.abort();
    }
}
