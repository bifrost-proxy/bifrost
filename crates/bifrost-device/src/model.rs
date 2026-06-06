use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobilePlatform {
    Android,
    Ios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTrustCapability {
    GuideOnly,
    PushAndOpenInstaller,
    ManagedAutoTrust,
    RootedTestDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Connected,
    Unauthorized,
    Offline,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCertificateState {
    Unknown,
    NotInstalled,
    PushedToDevice,
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCertificateStatus {
    pub state: DeviceCertificateState,
    pub trusted: Option<bool>,
    pub fingerprint_match: Option<bool>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDevice {
    pub id: String,
    pub name: Option<String>,
    pub managed_install_target: Option<String>,
    pub platform: MobilePlatform,
    pub status: DeviceStatus,
    pub capability: DeviceTrustCapability,
    pub certificate_status: Option<DeviceCertificateStatus>,
    pub status_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallCaRequest {
    pub device_id: String,
    pub ca_cert_path: PathBuf,
    pub mode: InstallMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    #[default]
    NormalGuide,
    ManagedAutoTrust,
    ProxyConfig,
    LabRootMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallStep {
    pub name: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSession {
    pub session_id: String,
    pub device_id: String,
    pub platform: MobilePlatform,
    pub mode: InstallMode,
    pub capability: DeviceTrustCapability,
    pub completed: bool,
    pub requires_user_confirmation: bool,
    pub summary: String,
    pub steps: Vec<InstallStep>,
}
