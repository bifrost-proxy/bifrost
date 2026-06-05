use std::net::{IpAddr, SocketAddr};

use bifrost_device::read_certificate_der_from_file;
use bifrost_tls::{CertInstaller, CertStatus};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use qrcode::render::svg;
use qrcode::QrCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    cors_preflight, error_response, full_body, json_response, method_not_allowed,
    public_response_builder, BoxBody,
};
use crate::network;
use crate::state::SharedAdminState;

#[derive(Serialize)]
struct CertInfo {
    available: bool,
    status: String,
    status_label: String,
    installed: bool,
    trusted: bool,
    status_message: String,
    sha256_fingerprint: Option<String>,
    local_ips: Vec<String>,
    download_urls: Vec<String>,
    qrcode_urls: Vec<String>,
}

struct CertStateView {
    status: &'static str,
    status_label: &'static str,
    installed: bool,
    trusted: bool,
    status_message: String,
}

const LOCAL_INSTALL_CONFIRMATION: &str = "install_local_ca_certificate";

#[derive(Debug, Deserialize)]
struct LocalInstallBody {
    confirmation: Option<String>,
}

pub async fn handle_cert(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
    peer_addr: Option<SocketAddr>,
) -> Response<BoxBody> {
    let method = req.method().clone();

    match path {
        "/api/cert" | "/api/cert/" | "/api/cert/info" => match method {
            Method::GET => get_cert_info(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/cert/install" | "/api/cert/install/" => match method {
            Method::POST => install_local_ca(req, state, peer_addr).await,
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

pub async fn handle_cert_public(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    match path {
        "/public/cert" | "/public/cert/" => match method {
            Method::GET => download_ca_cert(state).await,
            Method::OPTIONS => cors_preflight(),
            _ => method_not_allowed(),
        },
        "/public/cert/qrcode" | "/public/cert/qrcode/" => match method {
            Method::GET => get_cert_qrcode(req, state).await,
            Method::OPTIONS => cors_preflight(),
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn download_ca_cert(state: SharedAdminState) -> Response<BoxBody> {
    let cert_path = match &state.ca_cert_path {
        Some(path) => path,
        None => {
            return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
        }
    };

    if !cert_path.exists() {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not found");
    }

    match std::fs::read(cert_path) {
        Ok(cert_data) => public_response_builder(StatusCode::OK)
            .header("Content-Type", "application/x-pem-file")
            .header(
                "Content-Disposition",
                "attachment; filename=\"bifrost-ca.crt\"",
            )
            .body(full_body(cert_data))
            .unwrap(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to read certificate: {}", e),
        ),
    }
}

async fn get_cert_qrcode(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or("");
    let ip_from_query = query.split('&').find_map(|pair| {
        let mut parts = pair.split('=');
        match (parts.next(), parts.next()) {
            (Some("ip"), Some(value)) => Some(urlencoding::decode(value).ok()?.into_owned()),
            _ => None,
        }
    });

    let port = state.port();

    let host = ip_from_query
        .map(|ip| format!("{}:{}", ip, port))
        .unwrap_or_else(|| {
            req.headers()
                .get(hyper::header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("127.0.0.1")
                .to_string()
        });

    let download_url = format!("http://{}/_bifrost/public/cert", host);

    let code = match QrCode::new(download_url.as_bytes()) {
        Ok(code) => code,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to generate QR code: {}", e),
            );
        }
    };

    let svg_string = code
        .render()
        .min_dimensions(200, 200)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();

    public_response_builder(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(full_body(svg_string))
        .unwrap()
}

async fn get_cert_info(_req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    json_response(&build_cert_info(&state))
}

fn build_cert_info(state: &SharedAdminState) -> CertInfo {
    let available = state
        .ca_cert_path
        .as_ref()
        .map(|p| p.exists())
        .unwrap_or(false);
    let cert_state = resolve_cert_state(state.ca_cert_path.as_deref());

    let local_ips: Vec<String> = network::get_local_ips()
        .into_iter()
        .map(|info| info.ip)
        .collect();
    let port = state.port();

    let download_urls: Vec<String> = local_ips
        .iter()
        .map(|ip| format!("http://{}:{}/_bifrost/public/cert", ip, port))
        .collect();

    let qrcode_urls: Vec<String> = local_ips
        .iter()
        .map(|ip| format!("http://{}:{}/_bifrost/public/cert/qrcode", ip, port))
        .collect();

    CertInfo {
        available,
        status: cert_state.status.to_string(),
        status_label: cert_state.status_label.to_string(),
        installed: cert_state.installed,
        trusted: cert_state.trusted,
        status_message: cert_state.status_message,
        sha256_fingerprint: certificate_sha256_fingerprint(state.ca_cert_path.as_deref()),
        local_ips,
        download_urls,
        qrcode_urls,
    }
}

async fn install_local_ca(
    req: Request<Incoming>,
    state: SharedAdminState,
    peer_addr: Option<SocketAddr>,
) -> Response<BoxBody> {
    if !is_local_peer(peer_addr) {
        return error_response(
            StatusCode::FORBIDDEN,
            "Local CA installation is only available from the local admin UI.",
        );
    }

    let body = match req.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {error}"),
            );
        }
    };
    let request = if body.is_empty() {
        LocalInstallBody { confirmation: None }
    } else {
        match serde_json::from_slice::<LocalInstallBody>(&body) {
            Ok(request) => request,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid install request JSON: {error}"),
                );
            }
        }
    };
    if request.confirmation.as_deref() != Some(LOCAL_INSTALL_CONFIRMATION) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing local CA install confirmation.",
        );
    }

    let Some(cert_path) = state
        .ca_cert_path
        .as_ref()
        .filter(|path| path.exists())
        .cloned()
    else {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    };

    let install_result = tokio::task::spawn_blocking(move || {
        let installer = CertInstaller::new(&cert_path);
        installer.install_and_trust_gui()
    })
    .await;

    match install_result {
        Ok(Ok(())) => json_response(&build_cert_info(&state)),
        Ok(Err(error)) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("Failed to install and trust local CA certificate: {error}"),
        ),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Local CA installation task failed: {error}"),
        ),
    }
}

fn is_local_peer(peer_addr: Option<SocketAddr>) -> bool {
    match peer_addr.map(|addr| addr.ip()) {
        Some(IpAddr::V4(ip)) => ip.is_loopback(),
        Some(IpAddr::V6(ip)) => ip.is_loopback(),
        None => true,
    }
}

fn certificate_sha256_fingerprint(cert_path: Option<&std::path::Path>) -> Option<String> {
    let cert_path = cert_path.filter(|path| path.exists())?;
    let cert_data = read_certificate_der_from_file(cert_path).ok()?;
    let digest = Sha256::digest(cert_data);
    Some(
        digest
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

fn resolve_cert_state(cert_path: Option<&std::path::Path>) -> CertStateView {
    let Some(cert_path) = cert_path.filter(|path| path.exists()) else {
        return CertStateView {
            status: "not_installed",
            status_label: "Not installed",
            installed: false,
            trusted: false,
            status_message: "CA certificate file is missing, so system trust is not configured."
                .to_string(),
        };
    };

    let installer = CertInstaller::new(cert_path);
    match installer.check_status() {
        Ok(status) => cert_state_from_status(status),
        Err(error) => CertStateView {
            status: "unknown",
            status_label: "Check failed",
            installed: false,
            trusted: false,
            status_message: format!(
                "Unable to verify whether the CA certificate is trusted: {error}"
            ),
        },
    }
}

fn cert_state_from_status(status: CertStatus) -> CertStateView {
    match status {
        CertStatus::NotInstalled => CertStateView {
            status: "not_installed",
            status_label: "Not installed",
            installed: status.is_installed(),
            trusted: status.is_trusted(),
            status_message: "CA certificate is not installed in the system trust store."
                .to_string(),
        },
        CertStatus::InstalledNotTrusted => CertStateView {
            status: "installed_not_trusted",
            status_label: "Installed, not trusted",
            installed: status.is_installed(),
            trusted: status.is_trusted(),
            status_message: "CA certificate is installed, but the system does not trust it yet."
                .to_string(),
        },
        CertStatus::InstalledAndTrusted => CertStateView {
            status: "installed_and_trusted",
            status_label: "Installed and trusted",
            installed: status.is_installed(),
            trusted: status.is_trusted(),
            status_message: "CA certificate is installed and trusted by the system.".to_string(),
        },
    }
}

pub async fn handle_proxy_public(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    match path {
        "/public/proxy/qrcode" | "/public/proxy/qrcode/" => match method {
            Method::GET => get_proxy_qrcode(req, state).await,
            Method::OPTIONS => cors_preflight(),
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn get_proxy_qrcode(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or("");
    let ip_from_query = query.split('&').find_map(|pair| {
        let mut parts = pair.split('=');
        match (parts.next(), parts.next()) {
            (Some("ip"), Some(value)) => Some(urlencoding::decode(value).ok()?.into_owned()),
            _ => None,
        }
    });

    let host = ip_from_query.unwrap_or_else(|| {
        req.headers()
            .get(hyper::header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h))
            .unwrap_or("127.0.0.1")
            .to_string()
    });

    let proxy_address = format!("{}:{}", host, state.port());

    let code = match QrCode::new(proxy_address.as_bytes()) {
        Ok(code) => code,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to generate QR code: {}", e),
            );
        }
    };

    let svg_string = code
        .render()
        .min_dimensions(200, 200)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();

    public_response_builder(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(full_body(svg_string))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::{cert_state_from_status, CertStatus};

    #[test]
    fn maps_installed_not_trusted_status_without_false_trust() {
        let state = cert_state_from_status(CertStatus::InstalledNotTrusted);

        assert_eq!(state.status, "installed_not_trusted");
        assert!(state.installed);
        assert!(!state.trusted);
    }

    #[test]
    fn maps_installed_and_trusted_status() {
        let state = cert_state_from_status(CertStatus::InstalledAndTrusted);

        assert_eq!(state.status, "installed_and_trusted");
        assert!(state.installed);
        assert!(state.trusted);
    }
}
