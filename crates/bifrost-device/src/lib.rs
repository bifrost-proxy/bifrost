pub mod adb;
pub mod ios;
pub mod mobileconfig;
pub mod model;

pub use adb::{
    discover_android_devices, install_android_ca, parse_adb_devices, AdbDiscovery,
    AndroidInstallOptions,
};
pub use ios::{
    cfgutil_install_profile_args, discover_configurator, discover_ios_devices,
    install_ios_profile_with_configurator, parse_ioreg_ios_devices, ConfiguratorDiscovery,
    IosConfiguratorError, IosConfiguratorInstallOptions, IosDiscovery,
};
pub use mobileconfig::{
    generate_ios_mobileconfig, read_certificate_der_from_file, MobileConfigOptions,
};
pub use model::{
    DeviceStatus, DeviceTrustCapability, InstallCaRequest, InstallMode, InstallSession,
    InstallStep, MobileDevice, MobilePlatform,
};
