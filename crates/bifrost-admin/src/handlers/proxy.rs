use std::time::Duration;

use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use super::{
    error_response, json_response, json_response_with_status, method_not_allowed, BoxBody,
};
use crate::state::SharedAdminState;
use bifrost_core::ShellProxyManager;
use bifrost_core::SystemProxyManager;
use bifrost_storage::SystemProxyConfigUpdate;

#[derive(Serialize)]
struct SystemProxyStatus {
    supported: bool,
    enabled: bool,
    host: String,
    port: u16,
    bypass: String,
    managed_by_bifrost: bool,
}

impl SystemProxyStatus {
    fn from_proxy(proxy: bifrost_core::ProxyBackup, managed_by_bifrost: bool) -> Self {
        Self {
            supported: true,
            enabled: proxy.enable,
            host: proxy.host,
            port: proxy.port,
            bypass: proxy.bypass,
            managed_by_bifrost,
        }
    }
}

#[derive(Serialize)]
struct SystemProxySupportStatus {
    supported: bool,
    platform: String,
}

#[derive(Serialize)]
struct SystemProxyLaunchdApiStatus {
    supported: bool,
    installed: bool,
    loaded: bool,
    label: String,
    plist_path: String,
    program: Option<String>,
    data_dir: Option<String>,
    installed_version: Option<String>,
    current_version: String,
    needs_upgrade: bool,
    message: Option<String>,
}

impl From<bifrost_core::SystemProxyLaunchdStatus> for SystemProxyLaunchdApiStatus {
    fn from(status: bifrost_core::SystemProxyLaunchdStatus) -> Self {
        Self {
            supported: status.supported,
            installed: status.installed,
            loaded: status.loaded,
            label: status.label,
            plist_path: status.plist_path.display().to_string(),
            program: status.program.map(|path| path.display().to_string()),
            data_dir: status.data_dir.map(|path| path.display().to_string()),
            installed_version: status.installed_version,
            current_version: status.current_version,
            needs_upgrade: status.needs_upgrade,
            message: status.message,
        }
    }
}

#[derive(Serialize)]
struct CliProxyStatus {
    enabled: bool,
    shell: String,
    config_files: Vec<String>,
    proxy_url: String,
}

#[derive(Deserialize)]
struct SetSystemProxyRequest {
    enabled: bool,
    bypass: Option<String>,
}

#[derive(Deserialize)]
struct SetSystemProxyLaunchdRequest {
    enabled: bool,
}

#[derive(Serialize)]
struct ProxyAddressInfo {
    port: u16,
    local_ips: Vec<String>,
    addresses: Vec<ProxyAddress>,
}

#[derive(Serialize)]
struct ProxyAddress {
    ip: String,
    address: String,
    qrcode_url: String,
    is_preferred: bool,
}

const SYSTEM_PROXY_VERIFY_DELAYS_MS: [u64; 4] = [200, 400, 800, 1600];
#[cfg(target_os = "macos")]
const SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV: &str =
    "BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL";

pub async fn handle_proxy(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    match path {
        "/api/proxy/system" | "/api/proxy/system/" => match method {
            Method::GET => get_system_proxy_status(state).await,
            Method::PUT => set_system_proxy(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/proxy/cli" | "/api/proxy/cli/" => match method {
            Method::GET => get_cli_proxy_status(state).await,
            _ => method_not_allowed(),
        },
        "/api/proxy/system/support" => match method {
            Method::GET => get_system_proxy_support().await,
            _ => method_not_allowed(),
        },
        "/api/proxy/system/launchd" | "/api/proxy/system/launchd/" => match method {
            Method::GET => get_system_proxy_launchd_status(state).await,
            Method::PUT => set_system_proxy_launchd(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/proxy/address" | "/api/proxy/address/" => match method {
            Method::GET => get_proxy_address_info(state).await,
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn get_cli_proxy_status(state: SharedAdminState) -> Response<BoxBody> {
    let Some(ref config_manager) = state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };

    let data_dir = config_manager.data_dir().to_path_buf();
    let manager = ShellProxyManager::new(data_dir);
    let status = manager.status();

    let resp = CliProxyStatus {
        enabled: status.has_persistent_config,
        shell: status.shell_type.as_str().to_string(),
        config_files: status
            .config_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        proxy_url: format!("http://127.0.0.1:{}", state.port()),
    };
    json_response(&resp)
}

async fn get_system_proxy_status(state: SharedAdminState) -> Response<BoxBody> {
    if !SystemProxyManager::is_supported() {
        let status = SystemProxyStatus {
            supported: false,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            managed_by_bifrost: false,
        };
        return json_response(&status);
    }

    match SystemProxyManager::get_current() {
        Ok(proxy) => {
            let managed_by_bifrost = if let Some(manager) = &state.system_proxy_manager {
                manager.read().await.is_current_managed(&proxy)
            } else {
                false
            };
            let status = SystemProxyStatus::from_proxy(proxy, managed_by_bifrost);
            json_response(&status)
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to get system proxy: {}", e),
        ),
    }
}

fn read_system_proxy_status(
    expected_host: &str,
    expected_port: u16,
) -> Result<SystemProxyStatus, String> {
    if !SystemProxyManager::is_supported() {
        return Ok(SystemProxyStatus {
            supported: false,
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
            managed_by_bifrost: false,
        });
    }

    let proxy = SystemProxyManager::get_current()
        .map_err(|e| format!("Failed to get system proxy: {}", e))?;

    let managed_by_bifrost = proxy.target_matches(expected_host, expected_port);
    Ok(SystemProxyStatus::from_proxy(proxy, managed_by_bifrost))
}

async fn wait_for_system_proxy_status(
    expected_enabled: bool,
    expected_host: &str,
    expected_port: u16,
) -> Result<SystemProxyStatus, String> {
    let mut latest = read_system_proxy_status(expected_host, expected_port)?;
    if matches_expected_system_proxy(&latest, expected_enabled, expected_host, expected_port) {
        return Ok(latest);
    }

    for delay_ms in SYSTEM_PROXY_VERIFY_DELAYS_MS {
        sleep(Duration::from_millis(delay_ms)).await;
        latest = read_system_proxy_status(expected_host, expected_port)?;
        if matches_expected_system_proxy(&latest, expected_enabled, expected_host, expected_port) {
            return Ok(latest);
        }
    }

    Ok(latest)
}

fn matches_expected_system_proxy(
    status: &SystemProxyStatus,
    expected_enabled: bool,
    expected_host: &str,
    expected_port: u16,
) -> bool {
    if expected_enabled {
        return status.enabled && status.host == expected_host && status.port == expected_port;
    }

    !status.enabled || !status.managed_by_bifrost
}

async fn set_system_proxy(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    if !SystemProxyManager::is_supported() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "System proxy is not supported on this platform",
        );
    }

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: SetSystemProxyRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let bypass = request
        .bypass
        .unwrap_or_else(|| "localhost,127.0.0.1,::1,*.local".to_string());

    if let Some(ref manager) = state.system_proxy_manager {
        let host = "127.0.0.1";
        let target_port = state.port();

        let final_result = {
            let mut manager = manager.write().await;

            let result = if request.enabled {
                manager.enable(host, state.port(), Some(&bypass))
            } else {
                manager.disable_managed().map(|_| ())
            };

            match &result {
                Ok(()) => result,
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("RequiresAdmin") {
                        tracing::info!("Permission denied, trying GUI authorization...");
                        #[cfg(target_os = "macos")]
                        {
                            if request.enabled {
                                manager.enable_with_gui_auth(host, state.port(), Some(&bypass))
                            } else {
                                manager
                                    .disable_if_matches_with_gui_auth(host, state.port())
                                    .map(|_| ())
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            result
                        }
                    } else {
                        result
                    }
                }
            }
        };

        match final_result {
            Ok(()) => {
                let status =
                    match wait_for_system_proxy_status(request.enabled, host, target_port).await {
                        Ok(status) => status,
                        Err(e) => {
                            return error_response(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                &format!("Failed to verify system proxy: {}", e),
                            )
                        }
                    };

                if let Some(ref config_manager) = state.config_manager {
                    let enabled_by_bifrost = status.enabled && status.managed_by_bifrost;
                    let update = SystemProxyConfigUpdate {
                        enabled: Some(enabled_by_bifrost),
                        bypass: if enabled_by_bifrost {
                            Some(status.bypass.clone())
                        } else {
                            None
                        },
                        auto_enable: None,
                    };
                    if let Err(e) = config_manager.update_system_proxy_config(update).await {
                        tracing::error!("Failed to persist system proxy config: {}", e);
                    } else {
                        tracing::info!("System proxy config persisted: enabled={}", status.enabled);
                    }

                    if enabled_by_bifrost {
                        spawn_system_proxy_launchd_install_task_from_config(config_manager);
                    }
                }

                json_response(&status)
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("UserCancelled") {
                    #[derive(Serialize)]
                    struct UserCancelledError {
                        error: &'static str,
                        message: &'static str,
                    }
                    let body = UserCancelledError {
                        error: "user_cancelled",
                        message: "Authorization was cancelled by user.",
                    };
                    json_response_with_status(StatusCode::FORBIDDEN, &body)
                } else if msg.contains("RequiresAdmin") {
                    #[derive(Serialize)]
                    struct AdminError {
                        error: &'static str,
                        message: &'static str,
                    }
                    let body = AdminError {
                        error: "requires_admin",
                        message: "System proxy requires administrator privileges. Please run the CLI with sudo or grant permission.",
                    };
                    json_response_with_status(StatusCode::FORBIDDEN, &body)
                } else {
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to set system proxy: {}", e),
                    )
                }
            }
        }
    } else {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "System proxy manager not initialized",
        )
    }
}

async fn get_system_proxy_support() -> Response<BoxBody> {
    let status = SystemProxySupportStatus {
        supported: SystemProxyManager::is_supported(),
        platform: get_platform_name(),
    };
    json_response(&status)
}

async fn get_system_proxy_launchd_status(state: SharedAdminState) -> Response<BoxBody> {
    let Some(config_manager) = &state.config_manager else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };
    let config = match bifrost_core::SystemProxyLaunchdConfig::new(
        None,
        None,
        config_manager.data_dir().to_path_buf(),
        None,
    ) {
        Ok(config) => config,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "Failed to prepare system proxy LaunchDaemon config: {}",
                    error
                ),
            );
        }
    };
    match bifrost_core::launchd_status_for_config(&config) {
        Ok(status) => json_response(&SystemProxyLaunchdApiStatus::from(status)),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to get system proxy LaunchDaemon status: {}", error),
        ),
    }
}

async fn set_system_proxy_launchd(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    use http_body_util::BodyExt;

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: SetSystemProxyLaunchdRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let result = if request.enabled {
        install_system_proxy_launchd_from_state(&state)
    } else {
        match bifrost_core::uninstall_launchd_cleanup(None, None) {
            Ok(status) => Ok(status),
            Err(error) if error.to_string().contains("RequiresAdmin") => {
                bifrost_core::uninstall_launchd_cleanup_with_gui_auth(None, None, None)
            }
            Err(error) => Err(error),
        }
    };

    match result {
        Ok(status) => json_response(&SystemProxyLaunchdApiStatus::from(status)),
        Err(error) if error.to_string().contains("RequiresAdmin") => {
            #[derive(Serialize)]
            struct AdminError {
                error: &'static str,
                message: String,
                suggested_command: Option<String>,
            }
            let suggested_command = if request.enabled {
                suggested_launchd_install_command(&state)
            } else {
                std::env::current_exe()
                    .ok()
                    .map(|exe| format!("sudo {} system-proxy launchd uninstall", exe.display()))
            };
            json_response_with_status(
                StatusCode::FORBIDDEN,
                &AdminError {
                    error: "requires_admin",
                    message: "Installing or uninstalling the macOS system proxy cleanup LaunchDaemon requires administrator privileges.".to_string(),
                    suggested_command,
                },
            )
        }
        Err(error) if error.to_string().contains("UserCancelled") => {
            #[derive(Serialize)]
            struct UserCancelledError {
                error: &'static str,
                message: &'static str,
            }
            json_response_with_status(
                StatusCode::FORBIDDEN,
                &UserCancelledError {
                    error: "user_cancelled",
                    message: "Authorization was cancelled by user.",
                },
            )
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update system proxy LaunchDaemon: {}", error),
        ),
    }
}

fn install_system_proxy_launchd_from_state(
    state: &SharedAdminState,
) -> bifrost_core::Result<bifrost_core::SystemProxyLaunchdStatus> {
    let Some(config_manager) = &state.config_manager else {
        return Err(bifrost_core::BifrostError::Config(
            "Config manager not available".to_string(),
        ));
    };
    let config = bifrost_core::SystemProxyLaunchdConfig::new(
        None,
        None,
        config_manager.data_dir().to_path_buf(),
        None,
    )?;
    match bifrost_core::install_launchd_cleanup(&config) {
        Ok(status) => Ok(status),
        Err(error) if error.to_string().contains("RequiresAdmin") => {
            bifrost_core::install_launchd_cleanup_with_gui_auth(&config)
        }
        Err(error) => Err(error),
    }
}

fn suggested_launchd_install_command(state: &SharedAdminState) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let data_dir = state.config_manager.as_ref()?.data_dir();
    Some(format!(
        "sudo {} system-proxy launchd install --data-dir {} --program {}",
        exe.display(),
        data_dir.display(),
        exe.display()
    ))
}

#[cfg(any(target_os = "macos", test))]
fn system_proxy_launchd_needs_auto_install(
    installed: bool,
    loaded: bool,
    needs_upgrade: bool,
) -> bool {
    !installed || !loaded || needs_upgrade
}

#[cfg(target_os = "macos")]
fn spawn_system_proxy_launchd_install_task_from_config(
    config_manager: &bifrost_storage::ConfigManager,
) {
    if std::env::var(SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        tracing::info!(
            target: "bifrost_admin::proxy",
            env = SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV,
            "system proxy LaunchDaemon cleanup install disabled by environment"
        );
        return;
    }

    let config = match bifrost_core::SystemProxyLaunchdConfig::new(
        None,
        None,
        config_manager.data_dir().to_path_buf(),
        None,
    ) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                target: "bifrost_admin::proxy",
                error = %error,
                "failed to prepare system proxy LaunchDaemon cleanup install after system proxy enable"
            );
            return;
        }
    };

    let status = match bifrost_core::launchd_status_for_config(&config) {
        Ok(status) => status,
        Err(error) => {
            tracing::warn!(
                target: "bifrost_admin::proxy",
                error = %error,
                "failed to inspect system proxy LaunchDaemon cleanup status after system proxy enable"
            );
            return;
        }
    };

    if !system_proxy_launchd_needs_auto_install(
        status.installed,
        status.loaded,
        status.needs_upgrade,
    ) {
        tracing::info!(
            target: "bifrost_admin::proxy",
            installed_version = status.installed_version.as_deref().unwrap_or(""),
            current_version = status.current_version,
            "system proxy LaunchDaemon cleanup already installed and current after system proxy enable"
        );
        return;
    }

    std::thread::spawn(move || {
        tracing::info!(
            target: "bifrost_admin::proxy",
            installed = status.installed,
            loaded = status.loaded,
            needs_upgrade = status.needs_upgrade,
            "system proxy LaunchDaemon cleanup install starting asynchronously after system proxy enable"
        );
        match bifrost_core::install_launchd_cleanup_with_gui_auth(&config) {
            Ok(status) => tracing::info!(
                target: "bifrost_admin::proxy",
                installed_version = status.installed_version.as_deref().unwrap_or(""),
                current_version = status.current_version,
                "system proxy LaunchDaemon cleanup installed asynchronously after system proxy enable"
            ),
            Err(error) if error.to_string().contains("UserCancelled") => tracing::info!(
                target: "bifrost_admin::proxy",
                "system proxy LaunchDaemon cleanup install cancelled by user after system proxy enable"
            ),
            Err(error) => tracing::warn!(
                target: "bifrost_admin::proxy",
                error = %error,
                "system proxy LaunchDaemon cleanup install failed after system proxy enable"
            ),
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn spawn_system_proxy_launchd_install_task_from_config(
    _config_manager: &bifrost_storage::ConfigManager,
) {
}

fn get_platform_name() -> String {
    #[cfg(target_os = "macos")]
    {
        "macOS".to_string()
    }
    #[cfg(target_os = "windows")]
    {
        "Windows".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        "Linux".to_string()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "Unknown".to_string()
    }
}

async fn get_proxy_address_info(state: SharedAdminState) -> Response<BoxBody> {
    let ip_infos = crate::network::get_local_ips();
    let port = state.port();

    let local_ips: Vec<String> = ip_infos.iter().map(|i| i.ip.clone()).collect();

    let addresses: Vec<ProxyAddress> = ip_infos
        .iter()
        .map(|info| ProxyAddress {
            ip: info.ip.clone(),
            address: format!("{}:{}", info.ip, port),
            qrcode_url: format!(
                "/_bifrost/public/proxy/qrcode?ip={}",
                urlencoding::encode(&info.ip)
            ),
            is_preferred: info.is_preferred,
        })
        .collect();

    let info = ProxyAddressInfo {
        port,
        local_ips,
        addresses,
    };

    json_response(&info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_verification_accepts_external_proxy_left_enabled() {
        let status = SystemProxyStatus {
            supported: true,
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 6152,
            bypass: String::new(),
            managed_by_bifrost: false,
        };

        assert!(matches_expected_system_proxy(
            &status,
            false,
            "127.0.0.1",
            8800
        ));
    }

    #[test]
    fn disable_verification_rejects_bifrost_proxy_still_enabled() {
        let status = SystemProxyStatus {
            supported: true,
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 8800,
            bypass: String::new(),
            managed_by_bifrost: true,
        };

        assert!(!matches_expected_system_proxy(
            &status,
            false,
            "127.0.0.1",
            8800
        ));
    }

    #[test]
    fn launchd_auto_install_needed_when_missing_unloaded_or_stale() {
        assert!(system_proxy_launchd_needs_auto_install(false, false, false));
        assert!(system_proxy_launchd_needs_auto_install(true, false, false));
        assert!(system_proxy_launchd_needs_auto_install(true, true, true));
    }

    #[test]
    fn launchd_auto_install_skips_current_loaded_daemon() {
        assert!(!system_proxy_launchd_needs_auto_install(true, true, false));
    }
}
