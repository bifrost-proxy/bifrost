use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
use crate::state::SharedAdminState;

static TRUST_PROBE_MANAGER: Lazy<TrustProbeManager> = Lazy::new(TrustProbeManager::new);

const DEFAULT_TTL_SECONDS: i64 = 600;
const MAX_TTL_SECONDS: i64 = 1800;
const MAX_ACTIVE_SESSIONS: usize = 32;
const TOKEN_QUERY_KEY: &str = "t";

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
pub struct TrustProbeSessionView {
    session_id: String,
    status: TrustProbeStatus,
    opened: bool,
    network_reachable: bool,
    tls_trusted: bool,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    last_error: Option<String>,
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
}

struct TrustProbeEventInput {
    event_type: String,
    message: Option<String>,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
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
    network_reachable: bool,
    tls_trusted: bool,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    client_ip: Option<String>,
    user_agent: Option<String>,
    platform_hint: Option<String>,
    last_error: Option<String>,
    events: Vec<TrustProbeEvent>,
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
        let session = TrustProbeSession {
            id,
            token_hash,
            host,
            admin_port,
            probe_port,
            ca_fingerprint_sha256,
            status: TrustProbeStatus::Created,
            opened: false,
            network_reachable: false,
            tls_trusted: false,
            created_at: now,
            expires_at,
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
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

    fn get_session(&self, session_id: Uuid) -> Option<TrustProbeSessionView> {
        self.cleanup_expired_sessions();
        let sessions = self.sessions.lock();
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
        self.record_event(
            session_id,
            token,
            TrustProbeEventInput {
                event_type: report.event_type,
                message: report.message.or_else(|| {
                    report
                        .status
                        .map(|status| format!("Probe request returned HTTP {status}"))
                }),
                client_ip,
                user_agent: report.user_agent.or(user_agent_header),
                platform_hint: report.platform_hint,
            },
        )
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
        );
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
                    session.last_error = Some("Trust probe session expired.".to_string());
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
}

impl TrustProbeSession {
    fn token_matches(&self, token: &str) -> bool {
        !token.is_empty() && self.token_hash == hash_token(token)
    }

    fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    fn to_view(&self, token: &str) -> TrustProbeSessionView {
        let token_query = if token.is_empty() {
            String::new()
        } else {
            format!("?{TOKEN_QUERY_KEY}={}", urlencoding::encode(token))
        };
        TrustProbeSessionView {
            session_id: self.id.to_string(),
            status: if self.is_expired() {
                TrustProbeStatus::Expired
            } else {
                self.status
            },
            opened: self.opened,
            network_reachable: self.network_reachable,
            tls_trusted: self.tls_trusted,
            client_ip: self.client_ip.clone(),
            user_agent: self.user_agent.clone(),
            platform_hint: self.platform_hint.clone(),
            last_error: self.last_error.clone(),
            events: self.events.clone(),
            expires_at: self.expires_at,
            host: self.host.clone(),
            admin_port: self.admin_port,
            probe_port: self.probe_port,
            landing_url: format!(
                "http://{}:{}/_bifrost/public/trust-probe/{}{}",
                self.host, self.admin_port, self.id, token_query
            ),
            qr_code_url: format!(
                "http://{}:{}/_bifrost/public/trust-probe/{}/qrcode{}",
                self.host, self.admin_port, self.id, token_query
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

    fn apply_event(
        &mut self,
        event_type: &str,
        message: Option<String>,
        client_ip: Option<String>,
        user_agent: Option<String>,
        platform_hint: Option<String>,
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
                if self.status != TrustProbeStatus::TlsTrusted {
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
            message,
        });
        if self.events.len() > 64 {
            self.events.remove(0);
        }
    }
}

pub async fn handle_trust_probe_api(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    match (req.method().clone(), path) {
        (Method::POST, "/api/trust-probe/sessions")
        | (Method::POST, "/api/trust-probe/sessions/") => create_probe_session(req, state).await,
        (Method::GET, _) if path.starts_with("/api/trust-probe/sessions/") => {
            get_probe_session(path)
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

pub async fn handle_trust_probe_public(
    req: Request<Incoming>,
    _state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    if req.method() == Method::OPTIONS {
        return cors_preflight();
    }
    let Some((session_id, action)) = parse_public_probe_path(path) else {
        return error_response(StatusCode::NOT_FOUND, "Not Found");
    };
    let Some(token) = query_param(req.uri().query(), TOKEN_QUERY_KEY) else {
        return error_response(StatusCode::UNAUTHORIZED, "Missing trust probe token");
    };

    match (req.method().clone(), action.as_deref()) {
        (Method::GET, None) => {
            let Some(html) = TRUST_PROBE_MANAGER.render_landing_page(session_id, &token) else {
                return error_response(StatusCode::NOT_FOUND, "Trust probe session not found");
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
                return error_response(StatusCode::NOT_FOUND, "Trust probe session not found");
            }
            public_response_builder(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(super::empty_body())
                .unwrap()
        }
        (Method::GET, Some("qrcode")) => render_probe_qrcode(session_id, &token),
        (Method::HEAD, Some("qrcode")) => render_probe_qrcode_head(session_id, &token),
        (Method::POST, Some("report")) => report_probe_result(req, session_id, token).await,
        _ => method_not_allowed(),
    }
}

async fn create_probe_session(
    req: Request<Incoming>,
    state: SharedAdminState,
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
        Ok(session) => json_response_with_status(StatusCode::CREATED, &session),
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
        None => error_response(StatusCode::NOT_FOUND, "Trust probe session not found"),
    }
}

async fn report_probe_result(
    req: Request<Incoming>,
    session_id: Uuid,
    token: String,
) -> Response<BoxBody> {
    let client_ip = req
        .headers()
        .get("x-bifrost-peer-ip")
        .or_else(|| req.headers().get("x-forwarded-for"))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
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
        return error_response(StatusCode::NOT_FOUND, "Trust probe session not found");
    }
    json_response(&serde_json::json!({ "ok": true }))
}

fn render_probe_qrcode(session_id: Uuid, token: &str) -> Response<BoxBody> {
    let Some(session) = TRUST_PROBE_MANAGER.get_session(session_id) else {
        return error_response(StatusCode::NOT_FOUND, "Trust probe session not found");
    };
    let url = format!(
        "{}?{TOKEN_QUERY_KEY}={}",
        session
            .landing_url
            .split('?')
            .next()
            .unwrap_or(&session.landing_url),
        urlencoding::encode(token)
    );
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
        return error_response(StatusCode::NOT_FOUND, "Trust probe session not found");
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
    let Some(token) = query_param(req.uri().query(), TOKEN_QUERY_KEY) else {
        return Ok(probe_response(StatusCode::UNAUTHORIZED, "missing token"));
    };
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
                    client_ip,
                    user_agent,
                    platform_hint: None,
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
                    client_ip,
                    user_agent,
                    platform_hint: None,
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
    let netcheck_url = format!(
        "http://{}:{}/_bifrost/trust-probe/netcheck?sid={}&{}={}",
        view.host,
        view.probe_port,
        view.session_id,
        TOKEN_QUERY_KEY,
        urlencoding::encode(token)
    );
    let tls_check_url = format!(
        "https://{}:{}/_bifrost/trust-probe/check?sid={}&{}={}",
        view.host,
        view.probe_port,
        view.session_id,
        TOKEN_QUERY_KEY,
        urlencoding::encode(token)
    );
    let report_url = format!(
        "http://{}:{}/_bifrost/public/trust-probe/{}/report?{}={}",
        view.host,
        view.admin_port,
        view.session_id,
        TOKEN_QUERY_KEY,
        urlencoding::encode(token)
    );
    let config = serde_json::json!({
        "sessionId": view.session_id,
        "token": token,
        "host": view.host,
        "adminPort": view.admin_port,
        "probePort": view.probe_port,
        "caFingerprintSha256": view.ca_fingerprint_sha256,
        "caDownloadUrl": view.ca_download_url,
        "proxyQrCodeUrl": view.proxy_qr_code_url,
        "netcheckUrl": netcheck_url,
        "tlsCheckUrl": tls_check_url,
        "reportUrl": report_url,
    });
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Bifrost Trust Probe</title>
  <style>
    :root {{ color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    body {{ margin: 0; padding: 24px; background: Canvas; color: CanvasText; }}
    main {{ max-width: 640px; margin: 0 auto; }}
    h1 {{ font-size: 24px; margin: 0 0 12px; }}
    .status {{ border: 1px solid color-mix(in srgb, CanvasText 18%, transparent); border-radius: 8px; padding: 14px; margin: 16px 0; }}
    .ok {{ color: #16833a; }}
    .bad {{ color: #c7352f; }}
    code {{ word-break: break-all; }}
    a, button {{ font: inherit; }}
    button, .button {{ display: inline-block; margin: 6px 8px 6px 0; padding: 10px 12px; border-radius: 6px; border: 1px solid currentColor; background: transparent; color: inherit; text-decoration: none; }}
    ol {{ padding-left: 20px; }}
  </style>
</head>
<body>
<main>
  <h1>Bifrost Trust Probe</h1>
  <p>This page checks whether this device browser trusts the current Bifrost CA.</p>
  <p>CA SHA-256 fingerprint:<br><code>{}</code></p>
  <section class="status" id="result">Preparing trust check...</section>
  <section id="next"></section>
</main>
<script>
window.__BIFROST_TRUST_PROBE__ = {};
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
function show(html) {{ document.getElementById("result").innerHTML = html; }}
function showNext(html) {{ document.getElementById("next").innerHTML = html; }}
async function postReport(type, extra) {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  try {{
    await fetch(cfg.reportUrl, {{
      method: "POST",
      headers: {{ "Content-Type": "application/json" }},
      body: JSON.stringify(Object.assign({{
        type,
        userAgent: navigator.userAgent,
        platformHint: detectPlatform()
      }}, extra || {{}}))
    }});
  }} catch (_) {{}}
}}
async function runProbe() {{
  const cfg = window.__BIFROST_TRUST_PROBE__;
  await postReport("page_opened");
  show("Device opened the probe page. Checking probe port...");
  try {{
    const net = await fetch(cfg.netcheckUrl + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store",
      mode: "cors"
    }});
    if (!net.ok) {{
      await postReport("network_failed", {{ status: net.status }});
      show('<span class="bad">Probe port is not reachable.</span>');
      showNext("<p>Check that this phone and computer are on the same network, the selected IP is correct, and firewall rules allow the probe port.</p><button onclick='runProbe()'>Retry</button>");
      return;
    }}
    await postReport("netcheck_ok");
    show('<span class="ok">Network check passed.</span> Checking HTTPS trust...');
  }} catch (error) {{
    await postReport("network_failed", {{ message: String(error) }});
    show('<span class="bad">Probe port is not reachable.</span>');
    showNext("<p>Check that this phone and computer are on the same network, the selected IP is correct, and firewall rules allow the probe port.</p><button onclick='runProbe()'>Retry</button>");
    return;
  }}
  try {{
    const tls = await fetch(cfg.tlsCheckUrl + "&r=" + encodeURIComponent(randomSuffix()), {{
      cache: "no-store",
      mode: "cors"
    }});
    if (tls.ok) {{
      await postReport("tls_ok");
      show('<span class="ok">Trust check passed. This browser trusts Bifrost CA.</span>');
      showNext("<p>Next configure this device proxy to:<br><strong>" + cfg.host + ":" + cfg.adminPort + "</strong></p><a class='button' href='" + cfg.proxyQrCodeUrl + "'>Open proxy QR code</a>");
    }} else {{
      await postReport("tls_failed", {{ status: tls.status }});
      showTlsFailed();
    }}
  }} catch (error) {{
    await postReport("tls_failed", {{ message: String(error) }});
    showTlsFailed();
  }}
}}
function showTlsFailed() {{
  const platform = detectPlatform();
  show('<span class="bad">HTTPS trust check failed.</span>');
  let steps = "<p>Install and trust Bifrost CA, then return here and retry.</p>";
  if (platform === "ios") {{
    steps = "<ol><li>Install the Bifrost CA profile.</li><li>Open Settings &gt; General &gt; About &gt; Certificate Trust Settings.</li><li>Turn on full trust for Bifrost CA.</li></ol>";
  }} else if (platform === "android") {{
    steps = "<ol><li>Install the Bifrost CA certificate.</li><li>Retry in this browser.</li><li>For Android apps, remember that some apps ignore user CAs or use certificate pinning.</li></ol>";
  }}
  showNext(steps + "<a class='button' href='" + window.__BIFROST_TRUST_PROBE__.caDownloadUrl + "'>Download CA</a><button onclick='runProbe()'>Retry</button>");
}}
runProbe();
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
        "Trust probe host must be one of this computer's local IP addresses.".to_string()
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
        Err("Trust probe host must be selected from the local IP list.".to_string())
    }
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
            network_reachable: false,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: None,
            events: Vec::new(),
        };

        session.apply_event("page_opened", None, None, None, Some("ios".to_string()));
        session.apply_event("netcheck_ok", None, None, None, None);
        session.apply_event(
            "tls_failed",
            Some("Failed to fetch".to_string()),
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
            network_reachable: true,
            tls_trusted: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            client_ip: None,
            user_agent: None,
            platform_hint: None,
            last_error: Some("old error".to_string()),
            events: Vec::new(),
        };

        session.apply_event("tls_ok", None, None, None, None);

        assert_eq!(session.status, TrustProbeStatus::TlsTrusted);
        assert!(session.tls_trusted);
        assert!(session.last_error.is_none());
    }
}
