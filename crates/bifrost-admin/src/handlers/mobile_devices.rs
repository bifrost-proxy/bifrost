use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::Command;

use bifrost_device::{
    discover_android_devices, discover_android_devices_with_ca, discover_ios_devices,
    generate_ios_mobileconfig, generate_ios_wifi_proxy_mobileconfig, install_android_ca,
    install_ios_profile_with_configurator, read_certificate_der_from_file, AdbDiscovery,
    AndroidInstallOptions, DeviceStatus, InstallMode, InstallSession,
    IosConfiguratorInstallOptions, IosDiscovery, IosWifiProxyProfileOptions, MobileConfigOptions,
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
use crate::network;
use crate::state::SharedAdminState;

static INSTALL_SESSIONS: Lazy<Mutex<HashMap<String, InstallSession>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const INSTALL_CONFIRMATION: &str = "push_and_open_mobile_certificate_installer";
const PROXY_CONFIG_CONFIRMATION: &str = "install_ios_wifi_proxy_profile";

#[derive(Debug, Serialize)]
struct MobileDevicesResponse {
    android: AdbDiscovery,
    ios: IosDiscovery,
    ios_profile_url: String,
    ios_profile_qrcode_url: String,
    ios_wifi_proxy_profile_url: String,
    ios_wifi_proxy_profile_qrcode_url: String,
    suggested_wifi_ssid: Option<String>,
    suggested_wifi_ssid_message: Option<String>,
    ordinary_device_notice: &'static str,
    managed_device_notice: &'static str,
}

#[derive(Debug, Deserialize)]
struct InstallCaBody {
    mode: Option<InstallMode>,
    confirmation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InstallIosProxyBody {
    confirmation: Option<String>,
    ssid: String,
    proxy_host: Option<String>,
    proxy_port: Option<u16>,
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
        (Method::POST, _) if path.ends_with("/install-ios-proxy-profile") => {
            install_ios_proxy_profile(req, state, path).await
        }
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
        (Method::GET, "/public/mobile/ios-wifi-proxy.mobileconfig")
        | (Method::GET, "/public/mobile/ios-wifi-proxy.mobileconfig/") => {
            ios_wifi_proxy_mobileconfig(req, state)
        }
        (Method::HEAD, "/public/mobile/ios-wifi-proxy.mobileconfig")
        | (Method::HEAD, "/public/mobile/ios-wifi-proxy.mobileconfig/") => {
            ios_wifi_proxy_mobileconfig_head(req, state)
        }
        (Method::GET, "/public/mobile/ios-wifi-proxy.mobileconfig/qrcode")
        | (Method::GET, "/public/mobile/ios-wifi-proxy.mobileconfig/qrcode/") => {
            ios_wifi_proxy_mobileconfig_qrcode(req, state)
        }
        (Method::HEAD, "/public/mobile/ios-wifi-proxy.mobileconfig/qrcode")
        | (Method::HEAD, "/public/mobile/ios-wifi-proxy.mobileconfig/qrcode/") => {
            ios_wifi_proxy_mobileconfig_qrcode_head(req, state)
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
    let wifi_detection = current_wifi_ssid_detection();
    MobileDevicesResponse {
        android: discover_android_devices_with_ca(ca_cert_path),
        ios: discover_ios_devices(),
        ios_profile_url: format!(
            "http://127.0.0.1:{port}/_bifrost/public/mobile/ios-profile.mobileconfig"
        ),
        ios_profile_qrcode_url: format!(
            "http://127.0.0.1:{port}/_bifrost/public/mobile/ios-profile.mobileconfig/qrcode"
        ),
        ios_wifi_proxy_profile_url: format!(
            "http://127.0.0.1:{port}/_bifrost/public/mobile/ios-wifi-proxy.mobileconfig"
        ),
        ios_wifi_proxy_profile_qrcode_url: format!(
            "http://127.0.0.1:{port}/_bifrost/public/mobile/ios-wifi-proxy.mobileconfig/qrcode"
        ),
        suggested_wifi_ssid: wifi_detection.ssid,
        suggested_wifi_ssid_message: wifi_detection.message,
        ordinary_device_notice:
            "Personal Android and iOS devices require final confirmation on the phone before the CA is installed or fully trusted.",
        managed_device_notice:
            "Managed Android/iOS devices can support automatic trust only through Device Owner/Profile Owner, MDM, Apple Configurator, or equivalent fleet tooling.",
    }
}

async fn install_ios_proxy_profile(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let Some(device_id) = path
        .strip_prefix("/api/mobile-devices/")
        .and_then(|rest| rest.strip_suffix("/install-ios-proxy-profile"))
        .and_then(|value| urlencoding::decode(value).ok())
        .map(|value| value.into_owned())
        .filter(|value| !value.trim().is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "Missing iOS device id");
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
    let request = match serde_json::from_slice::<InstallIosProxyBody>(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid iOS proxy profile request JSON: {error}"),
            );
        }
    };
    if request.confirmation.as_deref() != Some(PROXY_CONFIG_CONFIRMATION) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing proxy profile confirmation.",
        );
    }

    let ssid = match normalize_ssid(&request.ssid) {
        Ok(ssid) => ssid,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let proxy_host = match normalize_proxy_host(request.proxy_host.as_deref()) {
        Ok(proxy_host) => proxy_host,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let proxy_port = match normalize_proxy_port(request.proxy_port, state.port()) {
        Ok(proxy_port) => proxy_port,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    let Some(cert_path) = state.ca_cert_path.as_ref().filter(|path| path.exists()) else {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    };

    install_ios_configurator_profile(
        cert_path.clone(),
        device_id,
        IosProfileKind::WifiProxy {
            ssid,
            proxy_host,
            proxy_port,
        },
    )
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
        return install_ios_configurator_profile(cert_path.clone(), device_id, IosProfileKind::Ca);
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

enum IosProfileKind {
    Ca,
    WifiProxy {
        ssid: String,
        proxy_host: String,
        proxy_port: u16,
    },
}

fn install_ios_configurator_profile(
    cert_path: PathBuf,
    device_id: String,
    profile_kind: IosProfileKind,
) -> Response<BoxBody> {
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
        profile_kind = %profile_kind.audit_name(),
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
    let (profile, temp_prefix) = match &profile_kind {
        IosProfileKind::Ca => (
            generate_ios_mobileconfig(&cert_der, &MobileConfigOptions::default()),
            "bifrost-ca",
        ),
        IosProfileKind::WifiProxy {
            ssid,
            proxy_host,
            proxy_port,
        } => (
            generate_ios_wifi_proxy_mobileconfig(
                &cert_der,
                &IosWifiProxyProfileOptions::new(ssid.clone(), proxy_host.clone(), *proxy_port),
            ),
            "bifrost-ios-wifi-proxy",
        ),
    };
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
        Ok(mut session) => {
            if let IosProfileKind::WifiProxy {
                ssid,
                proxy_host,
                proxy_port,
            } = profile_kind
            {
                session.mode = InstallMode::ProxyConfig;
                session.summary = if session.completed && !session.requires_user_confirmation {
                    format!(
                        "Apple Configurator installed the Bifrost Wi-Fi proxy profile for SSID {ssid}. The iPhone should use {proxy_host}:{proxy_port} when connected to that Wi-Fi network."
                    )
                } else if session.requires_user_confirmation {
                    format!(
                        "Apple Configurator opened the Bifrost Wi-Fi proxy profile install flow on the iPhone. Confirm it on the phone to configure SSID {ssid} to use proxy {proxy_host}:{proxy_port}."
                    )
                } else {
                    "Apple Configurator could not install the Bifrost Wi-Fi proxy profile. Unlock the iPhone, tap Trust for this Mac if prompted, then retry.".to_string()
                };
            }
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

impl IosProfileKind {
    fn audit_name(&self) -> &'static str {
        match self {
            IosProfileKind::Ca => "ca",
            IosProfileKind::WifiProxy { .. } => "wifi_proxy",
        }
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

fn ios_wifi_proxy_mobileconfig(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let Some(cert_path) = state.ca_cert_path.as_ref().filter(|path| path.exists()) else {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    };
    let options = match wifi_proxy_options_from_query(req.uri().query(), state.port()) {
        Ok(options) => options,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let cert_der = match read_certificate_der_from_file(cert_path) {
        Ok(cert_der) => cert_der,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to prepare iOS Wi-Fi proxy profile: {error}"),
            );
        }
    };
    let profile = generate_ios_wifi_proxy_mobileconfig(&cert_der, &options);
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "application/x-apple-aspen-config")
        .header(
            "Content-Disposition",
            "attachment; filename=\"bifrost-ios-wifi-proxy.mobileconfig\"",
        )
        .body(full_body(profile))
        .unwrap()
}

fn ios_wifi_proxy_mobileconfig_head(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    if !ca_cert_available(&state) {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    }
    if let Err(message) = wifi_proxy_options_from_query(req.uri().query(), state.port()) {
        return error_response(StatusCode::BAD_REQUEST, &message);
    }
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "application/x-apple-aspen-config")
        .header(
            "Content-Disposition",
            "attachment; filename=\"bifrost-ios-wifi-proxy.mobileconfig\"",
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

fn ios_wifi_proxy_mobileconfig_qrcode(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    if !ca_cert_available(&state) {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    }
    let options = match wifi_proxy_options_from_query(req.uri().query(), state.port()) {
        Ok(options) => options,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let profile_url = ios_wifi_proxy_profile_url(&options);
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

fn ios_wifi_proxy_mobileconfig_qrcode_head(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    if !ca_cert_available(&state) {
        return error_response(StatusCode::NOT_FOUND, "CA certificate not configured");
    }
    if let Err(message) = wifi_proxy_options_from_query(req.uri().query(), state.port()) {
        return error_response(StatusCode::BAD_REQUEST, &message);
    }
    public_response_builder(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(empty_body())
        .unwrap()
}

fn wifi_proxy_options_from_query(
    query: Option<&str>,
    default_port: u16,
) -> Result<IosWifiProxyProfileOptions, String> {
    let ssid = normalize_ssid(
        &query_param(query, "ssid")
            .or_else(|| current_wifi_ssid_detection().ssid)
            .unwrap_or_default(),
    )?;
    let proxy_host = normalize_proxy_host(query_param(query, "ip").as_deref())?;
    let proxy_port = normalize_proxy_port(
        query_param(query, "port")
            .map(|value| {
                value
                    .parse::<u16>()
                    .map_err(|_| "Proxy port must be a valid TCP port.".to_string())
            })
            .transpose()?,
        default_port,
    )?;
    Ok(IosWifiProxyProfileOptions::new(
        ssid, proxy_host, proxy_port,
    ))
}

fn ios_wifi_proxy_profile_url(options: &IosWifiProxyProfileOptions) -> String {
    format!(
        "http://{}:{}/_bifrost/public/mobile/ios-wifi-proxy.mobileconfig?ssid={}&ip={}&port={}",
        options.proxy_host,
        options.proxy_port,
        urlencoding::encode(&options.ssid),
        urlencoding::encode(&options.proxy_host),
        options.proxy_port
    )
}

fn normalize_ssid(ssid: &str) -> Result<String, String> {
    let ssid = ssid.trim();
    if ssid.is_empty() {
        return Err("Wi-Fi SSID is required for the iOS proxy profile.".to_string());
    }
    if ssid.len() > 128 {
        return Err("Wi-Fi SSID is too long.".to_string());
    }
    Ok(ssid.to_string())
}

fn normalize_proxy_host(host: Option<&str>) -> Result<String, String> {
    let host = host
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| network::get_local_ips().first().map(|info| info.ip.clone()))
        .ok_or_else(|| "Select a local network address for the iOS proxy profile.".to_string())?;
    let ip = host
        .parse::<IpAddr>()
        .map_err(|_| "Proxy host must be one of this computer's local IP addresses.".to_string())?;
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
        Err("Proxy host must be selected from the local IP list.".to_string())
    }
}

fn normalize_proxy_port(port: Option<u16>, current_port: u16) -> Result<u16, String> {
    match port {
        Some(port) if port == current_port => Ok(port),
        Some(_) => Err("Proxy port must match the current Bifrost proxy port.".to_string()),
        None => Ok(current_port),
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

fn ca_cert_available(state: &SharedAdminState) -> bool {
    state
        .ca_cert_path
        .as_ref()
        .map(|path| path.exists())
        .unwrap_or(false)
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
    if let Some(ssid) = ssid_for_wifi_device_networksetup(&device) {
        return WifiSsidDetection {
            ssid: Some(ssid),
            message: None,
        };
    }
    match ssid_for_wifi_device_ipconfig(&device) {
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut saw_wifi = false;
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(port) = line.strip_prefix("Hardware Port:") {
            saw_wifi = port.trim() == "Wi-Fi" || port.trim() == "AirPort";
            continue;
        }
        if saw_wifi {
            if let Some(device) = line.strip_prefix("Device:").map(str::trim) {
                return Some(device.to_string());
            }
        }
    }
    None
}

fn ssid_for_wifi_device_networksetup(device: &str) -> Option<String> {
    let output = Command::new("networksetup")
        .args(["-getairportnetwork", device])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .map(|(_, ssid)| ssid.trim().to_string())
        })
        .filter(|ssid| !ssid.is_empty())
}

fn ssid_for_wifi_device_ipconfig(device: &str) -> Option<String> {
    let output = Command::new("ipconfig")
        .arg("getsummary")
        .arg(device)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("SSID :")?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
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
