pub mod adb;
pub mod ios;
pub mod mobileconfig;
pub mod model;

pub use adb::{
    check_android_ca_status, discover_android_devices, discover_android_devices_with_ca,
    install_android_ca, parse_adb_devices, AdbDiscovery, AndroidCaStatusOptions,
    AndroidInstallOptions,
};
pub use ios::{
    cfgutil_install_profile_args, discover_configurator, discover_ios_devices,
    install_ios_profile_with_configurator, parse_ioreg_ios_devices, ConfiguratorDiscovery,
    IosConfiguratorError, IosConfiguratorInstallOptions, IosDiscovery,
};
pub use mobileconfig::{
    generate_ios_mobileconfig, generate_ios_wifi_proxy_mobileconfig,
    read_certificate_der_from_file, IosWifiProxyProfileOptions, MobileConfigOptions,
};
pub use model::{
    DeviceCertificateState, DeviceCertificateStatus, DeviceStatus, DeviceTrustCapability,
    InstallCaRequest, InstallMode, InstallSession, InstallStep, MobileDevice, MobilePlatform,
};
