use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use bifrost_device::{
    discover_android_devices, discover_android_devices_with_ca, discover_ios_devices,
    generate_ios_mobileconfig, install_android_ca, install_ios_profile_with_configurator,
    read_certificate_der_from_file, AdbDiscovery, AndroidInstallOptions, DeviceStatus, InstallMode,
    InstallSession, IosConfiguratorInstallOptions, IosDiscovery, MobileConfigOptions,
    MobilePlatform,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use qrcode::render::svg;
use qrcode::QrCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    cors_preflight, empty_body, error_response, full_body, json_response,
    json_response_with_status, method_not_allowed, public_response_builder, BoxBody,
};
use crate::state::SharedAdminState;

static INSTALL_SESSIONS: Lazy<Mutex<HashMap<String, InstallSession>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const INSTALL_CONFIRMATION: &str = "push_and_open_mobile_certificate_installer";

#[derive(Debug, Serialize)]
struct MobileDevicesResponse {
    android: AdbDiscovery,
    ios: IosDiscovery,
    ios_profile_url: String,
    ios_profile_qrcode_url: String,
    ordinary_device_notice: &'static str,
    managed_device_notice: &'static str,
}

#[derive(Debug, Deserialize)]
struct InstallCaBody {
    mode: Option<InstallMode>,
    confirmation: Option<String>,
}

pub async fn handle_mobile_devices(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
    peer_addr: Option<SocketAddr>,
) -> Response<BoxBody> {
    if !is_local_peer(peer_addr) {
        return error_response(
            StatusCode::FORBIDDEN,
            "Mobile USB certificate operations are only available from the local admin UI.",
        );
    }

    let method = req.method().clone();
    match (method, path) {
        (Method::GET, "/api/mobile-devices") | (Method::GET, "/api/mobile-devices/") => {
            list_mobile_devices(state)
        }
        (Method::POST, "/api/mobile-devices/refresh") => list_mobile_devices(state),
        (Method::GET, _) if path.starts_with("/api/mobile-devices/install-sessions/") => {
            get_install_session(path)
        }
        (Method::POST, _) if path.ends_with("/install-ca") => install_ca(req, state, path).await,
        _ => {
            if path.starts_with("/api/mobile-devices/") {
                method_not_allowed()
            } else {
                error_response(StatusCode::NOT_FOUND, "Not Found")
            }
        }
    }
}

pub async fn handle_mobile_public(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    match (req.method().clone(), path) {
        (Method::GET, "/public/mobile/ios-profile.mobileconfig")
        | (Method::GET, "/public/mobile/ios-profile.mobileconfig/") => ios_mobileconfig(state),
        (Method::HEAD, "/public/mobile/ios-profile.mobileconfig")
        | (Method::HEAD, "/public/mobile/ios-profile.mobileconfig/") => {
            ios_mobileconfig_head(state)
        }
        (Method::GET, "/public/mobile/ios-profile.mobileconfig/qrcode")
        | (Method::GET, "/public/mobile/ios-profile.mobileconfig/qrcode/") => {
            ios_mobileconfig_qrcode(req, state)
        }
        (Method::HEAD, "/public/mobile/ios-profile.mobileconfig/qrcode")
        | (Method::HEAD, "/public/mobile/ios-profile.mobileconfig/qrcode/") => {
            ios_mobileconfig_qrcode_head(state)
        }
        (Method::OPTIONS, _) => cors_preflight(),
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

fn list_mobile_devices(state: SharedAdminState) -> Response<BoxBody> {
    json_response(&build_mobile_devices_response(&state))
}

pub fn mobile_devices_snapshot(state: &SharedAdminState) -> serde_json::Value {
    serde_json::to_value(build_mobile_devices_response(state)).unwrap_or_else(|_| {
        serde_json::json!({
            "android": { "devices": [] },
            "ios": { "devices": [] },
            "ordinary_device_notice": "Mobile device discovery is unavailable.",
            "managed_device_notice": "Mobile device discovery is unavailable."
        })
    })
}

fn build_mobile_devices_response(state: &SharedAdminState) -> MobileDevicesResponse {
    let port = state.port();
    let ca_cert_path = state.ca_cert_path.as_deref().filter(|path| path.exists());
    MobileDevicesResponse {
        android: discover_android_devices_with_ca(ca_cert_path),
        ios: discover_ios_devices(),
        ios_profile_url: format!(
            "http://127.0.0.1:{port}/_bifrost/public/mobile/ios-profile.mobileconfig"
        ),
        ios_profile_qrcode_url: format!(
            "http://127.0.0.1:{port}/_bifrost/public/mobile/ios-profile.mobileconfig/qrcode"
        ),
        ordinary_device_notice:
            "Personal Android and iOS devices require final confirmation on the phone before the CA is installed or fully trusted.",
        managed_device_notice:
            "Managed Android/iOS devices can support automatic trust only through Device Owner/Profile Owner, MDM, Apple Configurator, or equivalent fleet tooling.",
    }
}

async fn install_ca(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let Some(device_id) = path
        .strip_prefix("/api/mobile-devices/")
        .and_then(|rest| rest.strip_suffix("/install-ca"))
        .and_then(|value| urlencoding::decode(value).ok())
        .map(|value| value.into_owned())
        .filter(|value| !value.trim().is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Missing mobile device id");
    };

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
        InstallCaBody {
            mode: None,
            confirmation: None,
        }
    } else {
        match serde_json::from_slice::<InstallCaBody>(&body) {
            Ok(request) => request,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid install request JSON: {error}"),
                );
            }
        }
    };
    if request.confirmation.as_deref() != Some(INSTALL_CONFIRMATION) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing install confirmation. The UI must confirm that the user understands the phone still requires manual confirmation.",
        );
    }
    let mode = request.mode.unwrap_or_default();

    let Some(cert_path) = state.ca_cert_path.as_ref().filter(|path| path.exists()) else {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    };
    if mode == InstallMode::ManagedAutoTrust {
        return install_ios_configurator_profile(cert_path.clone(), device_id);
    }
    if mode != InstallMode::NormalGuide {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Only normal guided install and macOS Apple Configurator install are supported in this version.",
        );
    }

    let discovery = discover_android_devices();
    let Some(adb_path) = discovery.adb_path.as_ref().map(PathBuf::from) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, &discovery.message);
    };
    let Some(device) = discovery
        .devices
        .iter()
        .find(|device| device.id == device_id)
    else {
        return error_response(StatusCode::NOT_FOUND, "Android device not found");
    };
    if device.status != DeviceStatus::Connected {
        return error_response(
            StatusCode::CONFLICT,
            &format!(
                "Android device is not ready for CA installation: {}",
                device.status_message
            ),
        );
    }

    tracing::info!(
        target: "bifrost_admin::mobile_devices",
        device_id = %device_id,
        "audit: mobile CA install requested in normal guide mode"
    );

    match install_android_ca(AndroidInstallOptions {
        adb_path,
        device_id: device_id.clone(),
        ca_cert_path: cert_path.clone(),
    }) {
        Ok(session) => {
            INSTALL_SESSIONS
                .lock()
                .insert(session.session_id.clone(), session.clone());
            json_response_with_status(StatusCode::CREATED, &session)
        }
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("Failed to start Android CA install flow: {error}"),
        ),
    }
}

fn install_ios_configurator_profile(cert_path: PathBuf, device_id: String) -> Response<BoxBody> {
    let discovery = discover_ios_devices();
    if !discovery.configurator.cfgutil_available {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &discovery.configurator.message,
        );
    }
    let Some(cfgutil_path) = discovery
        .configurator
        .cfgutil_path
        .as_ref()
        .map(PathBuf::from)
    else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cfgutil path is unavailable",
        );
    };
    let Some(device) = discovery
        .devices
        .iter()
        .find(|device| device.id == device_id && device.platform == MobilePlatform::Ios)
    else {
        return error_response(StatusCode::NOT_FOUND, "iOS device not found");
    };
    if device.status != DeviceStatus::Connected {
        return error_response(
            StatusCode::CONFLICT,
            &format!("iOS device is not ready: {}", device.status_message),
        );
    }
    let Some(cfgutil_target) = device.managed_install_target.clone() else {
        return error_response(
            StatusCode::CONFLICT,
            "This iOS device is visible over USB, but Bifrost could not resolve its Apple Configurator ECID target. Refresh devices or use Apple Configurator directly.",
        );
    };

    tracing::info!(
        target: "bifrost_admin::mobile_devices",
        device_id = %device_id,
        cfgutil_target = %cfgutil_target,
        profile_kind = "ca",
        "audit: iOS profile install requested through Apple Configurator"
    );

    let cert_der = match read_certificate_der_from_file(&cert_path) {
        Ok(cert_der) => cert_der,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to prepare iOS profile: {error}"),
            );
        }
    };
    let profile = generate_ios_mobileconfig(&cert_der, &MobileConfigOptions::default());
    let temp_prefix = "bifrost-ca";
    let profile_path =
        std::env::temp_dir().join(format!("{temp_prefix}-{}.mobileconfig", Uuid::new_v4()));
    if let Err(error) = std::fs::write(&profile_path, profile) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to write temporary iOS profile: {error}"),
        );
    }

    let result = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
        cfgutil_path,
        device_id: device_id.clone(),
        cfgutil_target: Some(cfgutil_target),
        profile_path: profile_path.clone(),
    });
    let _ = std::fs::remove_file(&profile_path);

    match result {
        Ok(session) => {
            INSTALL_SESSIONS
                .lock()
                .insert(session.session_id.clone(), session.clone());
            json_response_with_status(StatusCode::CREATED, &session)
        }
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("Failed to install iOS profile through Apple Configurator: {error}"),
        ),
    }
}

fn get_install_session(path: &str) -> Response<BoxBody> {
    let Some(session_id) = path
        .strip_prefix("/api/mobile-devices/install-sessions/")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Missing install session id");
    };
    let Some(session) = INSTALL_SESSIONS.lock().get(session_id).cloned() else {
        return error_response(StatusCode::NOT_FOUND, "Install session not found");
    };
    json_response(&session)
}

fn ios_mobileconfig(state: SharedAdminState) -> Response<BoxBody> {
    let Some(cert_path) = state.ca_cert_path.as_ref().filter(|path| path.exists()) else {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    };
    let cert_der = match read_certificate_der_from_file(cert_path) {
        Ok(cert_der) => cert_der,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to prepare iOS profile: {error}"),
            );
        }
    };
    let profile = generate_ios_mobileconfig(&cert_der, &MobileConfigOptions::default());
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "application/x-apple-aspen-config")
        .header(
            "Content-Disposition",
            "attachment; filename=\"bifrost-ca.mobileconfig\"",
        )
        .body(full_body(profile))
        .unwrap()
}

fn ios_mobileconfig_head(state: SharedAdminState) -> Response<BoxBody> {
    if !ca_cert_available(&state) {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    }
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "application/x-apple-aspen-config")
        .header(
            "Content-Disposition",
            "attachment; filename=\"bifrost-ca.mobileconfig\"",
        )
        .body(empty_body())
        .unwrap()
}

fn ios_mobileconfig_qrcode(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if !ca_cert_available(&state) {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    }

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
        .map(|ip| format!("{ip}:{port}"))
        .unwrap_or_else(|| {
            req.headers()
                .get(hyper::header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("127.0.0.1")
                .to_string()
        });
    let profile_url = format!("http://{host}/_bifrost/public/mobile/ios-profile.mobileconfig");
    let code = match QrCode::new(profile_url.as_bytes()) {
        Ok(code) => code,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to generate QR code: {error}"),
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

fn ios_mobileconfig_qrcode_head(state: SharedAdminState) -> Response<BoxBody> {
    if !ca_cert_available(&state) {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    }
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(empty_body())
        .unwrap()
}

fn ca_cert_available(state: &SharedAdminState) -> bool {
    state
        .ca_cert_path
        .as_ref()
        .map(|path| path.exists())
        .unwrap_or(false)
}

fn is_local_peer(peer_addr: Option<SocketAddr>) -> bool {
    peer_addr
        .map(|addr| addr.ip().is_loopback())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::is_local_peer;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn allows_loopback_or_in_process_mobile_device_api() {
        assert!(is_local_peer(None));
        assert!(is_local_peer(Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            8800
        ))));
    }

    #[test]
    fn rejects_lan_mobile_device_api() {
        assert!(!is_local_peer(Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
            8800
        ))));
    }
}
