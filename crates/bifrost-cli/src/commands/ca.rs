use std::path::{Path, PathBuf};
use std::sync::Arc;

use bifrost_device::{
    discover_android_devices_with_ca, discover_ios_devices, generate_ios_mobileconfig,
    install_android_ca, install_ios_profile_with_configurator, read_certificate_der_from_file,
    AdbDiscovery, AndroidInstallOptions, DeviceCertificateState, DeviceStatus, InstallSession,
    IosConfiguratorInstallOptions, MobileConfigOptions, MobileDevice,
};
use bifrost_proxy::{ProxyConfig, TlsConfig};
use bifrost_tls::{
    ensure_valid_ca, generate_root_ca, get_platform_name, load_root_ca, parse_cert_info,
    save_root_ca, CertInstaller, CertStatus, DynamicCertGenerator, SniResolver,
};
use dialoguer::{Confirm, Select};

use crate::cli::CaCommands;
use crate::config::get_bifrost_dir;

#[derive(Debug, Clone, Copy)]
pub struct CertificateCheckOptions {
    pub auto_yes: bool,
    pub allow_prompt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CertificateResolution {
    AlreadyTrusted,
    AutoInstallAndTrust,
    PromptInstall,
    PromptTrust,
    BlockNonInteractive,
}

fn certificate_resolution(
    status: &CertStatus,
    options: CertificateCheckOptions,
) -> CertificateResolution {
    match status {
        CertStatus::InstalledAndTrusted => CertificateResolution::AlreadyTrusted,
        CertStatus::NotInstalled | CertStatus::InstalledNotTrusted if options.auto_yes => {
            CertificateResolution::AutoInstallAndTrust
        }
        CertStatus::NotInstalled if options.allow_prompt => CertificateResolution::PromptInstall,
        CertStatus::InstalledNotTrusted if options.allow_prompt => {
            CertificateResolution::PromptTrust
        }
        CertStatus::NotInstalled | CertStatus::InstalledNotTrusted => {
            CertificateResolution::BlockNonInteractive
        }
    }
}

pub fn handle_ca_command(action: CaCommands) -> bifrost_core::Result<()> {
    let cert_dir = get_bifrost_dir()?.join("certs");
    std::fs::create_dir_all(&cert_dir)?;

    let ca_key_path = cert_dir.join("ca.key");
    let ca_cert_path = cert_dir.join("ca.crt");

    match action {
        CaCommands::Install {
            mobile,
            ios,
            configurator,
            device,
            yes,
        } => {
            ensure_ca_exists(&ca_cert_path, &ca_key_path)?;

            if ios || configurator {
                handle_ios_ca_install(&ca_cert_path, device.as_deref(), configurator, yes)?;
            } else if mobile || device.is_some() {
                handle_mobile_ca_install(&ca_cert_path, device.as_deref(), yes)?;
            } else {
                let installer = CertInstaller::new(&ca_cert_path);
                installer.install_and_trust()?;
                println!("CA certificate installed and trusted successfully.");
                println!("Certificate: {}", ca_cert_path.display());
            }
        }
        CaCommands::Generate { force } => {
            if ca_cert_path.exists() && !force {
                println!("CA certificate already exists.");
                println!("Use --force to regenerate.");
                return Ok(());
            }

            let ca = generate_root_ca()?;
            save_root_ca(&ca_cert_path, &ca_key_path, &ca)?;
            println!("CA certificate generated successfully.");
            println!("Certificate: {}", ca_cert_path.display());
            println!("Private key: {}", ca_key_path.display());
            println!();
            println!(
                "To use HTTPS interception, install the CA certificate in your browser or system."
            );
        }
        CaCommands::Export { output } => {
            if !ca_cert_path.exists() {
                return Err(bifrost_core::BifrostError::NotFound(
                    "CA certificate not found. Run 'bifrost ca generate' first.".to_string(),
                ));
            }

            let output_path = output.unwrap_or_else(|| PathBuf::from("bifrost-ca.crt"));
            std::fs::copy(&ca_cert_path, &output_path)?;
            println!("CA certificate exported to: {}", output_path.display());
        }
        CaCommands::Info => {
            if !ca_cert_path.exists() {
                return Err(bifrost_core::BifrostError::NotFound(
                    "CA certificate not found. Run 'bifrost ca generate' first.".to_string(),
                ));
            }

            let _ca = load_root_ca(&ca_cert_path, &ca_key_path)?;

            println!("CA Certificate Information");
            println!("==========================");
            println!();

            match parse_cert_info(&ca_cert_path) {
                Ok(cert_info) => {
                    println!("📜 Certificate Details");
                    println!("  Subject:           {}", cert_info.subject);
                    println!("  Issuer:            {}", cert_info.issuer);
                    println!("  Serial Number:     {}", cert_info.serial_number);
                    println!("  Signature Algo:    {}", cert_info.signature_algorithm);
                    println!(
                        "  Is CA:             {}",
                        if cert_info.is_ca { "Yes" } else { "No" }
                    );
                    println!();

                    println!("🔑 Key Information");
                    print!("  Algorithm:         {}", cert_info.key_type);
                    if let Some(size) = cert_info.key_size {
                        print!(" ({} bits)", size);
                    }
                    println!();
                    if !cert_info.key_usages.is_empty() {
                        println!("  Key Usage:         {}", cert_info.key_usages.join(", "));
                    }
                    if !cert_info.extended_key_usages.is_empty() {
                        println!(
                            "  Extended Usage:    {}",
                            cert_info.extended_key_usages.join(", ")
                        );
                    }
                    println!();

                    println!("📅 Validity Period");
                    println!(
                        "  Not Before:        {}",
                        cert_info.not_before.format("%Y-%m-%d %H:%M:%S UTC")
                    );
                    println!(
                        "  Not After:         {}",
                        cert_info.not_after.format("%Y-%m-%d %H:%M:%S UTC")
                    );

                    let days = cert_info.days_remaining();
                    if cert_info.is_expired() {
                        println!("  Status:            ❌ EXPIRED ({} days ago)", -days);
                    } else if cert_info.is_not_yet_valid() {
                        println!("  Status:            ⏳ Not yet valid");
                    } else {
                        let years = days / 365;
                        let remaining_days = days % 365;
                        if years > 0 {
                            println!(
                                "  Remaining:         {} days ({} years, {} days)",
                                days, years, remaining_days
                            );
                        } else {
                            println!("  Remaining:         {} days", days);
                        }
                        if days < 30 {
                            println!("  ⚠️  Certificate will expire soon!");
                        }
                    }
                    println!();

                    println!("🔐 Fingerprint");
                    println!("  SHA-256:           {}", cert_info.fingerprint_sha256);
                    println!();
                }
                Err(e) => {
                    println!("⚠️  Could not parse certificate details: {}", e);
                    println!();
                }
            }

            println!("📂 File Paths");
            println!("  Certificate:       {}", ca_cert_path.display());
            println!("  Private Key:       {}", ca_key_path.display());
            let cert_meta = std::fs::metadata(&ca_cert_path)?;
            if let Ok(modified) = cert_meta.modified() {
                if let Ok(duration) = modified.elapsed() {
                    let days = duration.as_secs() / 86400;
                    if days == 0 {
                        let hours = duration.as_secs() / 3600;
                        if hours == 0 {
                            let mins = duration.as_secs() / 60;
                            println!("  File Modified:     {} minutes ago", mins);
                        } else {
                            println!("  File Modified:     {} hours ago", hours);
                        }
                    } else {
                        println!("  File Modified:     {} days ago", days);
                    }
                }
            }
            println!();

            println!("💻 System Trust Status ({})", get_platform_name());
            let installer = CertInstaller::new(&ca_cert_path);
            match installer.get_detailed_status() {
                Ok(system_info) => {
                    let status_icon = match system_info.status {
                        CertStatus::InstalledAndTrusted => "✓",
                        CertStatus::InstalledNotTrusted => "⚠",
                        CertStatus::NotInstalled => "✗",
                    };
                    println!(
                        "  Status:            {} {}",
                        status_icon, system_info.status
                    );
                    if let Some(location) = system_info.keychain_location {
                        println!("  Location:          {}", location);
                    }
                    if let Some(path) = system_info.system_cert_path {
                        println!("  System Path:       {}", path.display());
                    }

                    if system_info.status != CertStatus::InstalledAndTrusted {
                        println!();
                        println!(
                            "  💡 Run 'bifrost ca install' to install and trust the certificate."
                        );
                    }
                }
                Err(e) => {
                    println!("  Could not check trust status: {}", e);
                }
            }
        }
    }

    Ok(())
}

fn handle_ios_ca_install(
    ca_cert_path: &Path,
    requested_device_id: Option<&str>,
    use_configurator: bool,
    non_interactive: bool,
) -> bifrost_core::Result<()> {
    let profile_path = ca_cert_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("bifrost-ca.mobileconfig");
    let cert_der = read_certificate_der_from_file(ca_cert_path)
        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
    let profile = generate_ios_mobileconfig(&cert_der, &MobileConfigOptions::default());
    std::fs::write(&profile_path, profile)?;

    println!("iOS CA install guide");
    println!("====================");
    println!("Profile: {}", profile_path.display());
    println!("Certificate: {}", ca_cert_path.display());
    println!();
    println!("Default path for personal iPhone/iPad:");
    println!("  1. Open the .mobileconfig profile on the iPhone and install it.");
    println!("  2. Open Settings > General > About > Certificate Trust Settings.");
    println!("  3. Turn on full trust for Bifrost CA.");
    println!();
    println!("Important: profiles opened from a website, QR code, AirDrop, email, or Files do not enable SSL/TLS trust automatically.");
    println!();

    let discovery = discover_ios_devices();
    println!("USB detection: {}", discovery.message);
    println!("Apple Configurator: {}", discovery.configurator.message);

    if !use_configurator {
        println!();
        if discovery.configurator.cfgutil_available {
            println!("To try the official computer-side install path on macOS, re-run:");
        } else {
            println!("To try the official computer-side install path on macOS, install Apple Configurator and re-run:");
        }
        println!("  bifrost ca install --ios --configurator");
        println!();
        println!("Configurator-installed certificate payloads are trusted for SSL/TLS automatically. Unsupervised devices may still ask you to Trust this Mac, unlock the phone, or follow onscreen prompts.");
        return Ok(());
    }

    let Some(cfgutil_path) = discovery
        .configurator
        .cfgutil_path
        .as_ref()
        .map(PathBuf::from)
    else {
        return Err(bifrost_core::BifrostError::Config(
            discovery.configurator.message,
        ));
    };
    let device = select_single_ios_device_for_configurator(
        &discovery.devices,
        requested_device_id,
        non_interactive,
    )?;

    println!();
    println!(
        "Installing through Apple Configurator to: {}",
        device_display_name(&device)
    );
    println!("If the iPhone is locked or has not trusted this Mac, unlock it and tap Trust when prompted.");
    println!();

    let session = install_ios_profile_with_configurator(IosConfiguratorInstallOptions {
        cfgutil_path,
        device_id: device.id.clone(),
        cfgutil_target: device.managed_install_target.clone(),
        profile_path,
    })
    .map_err(|error| {
        bifrost_core::BifrostError::Config(format!(
            "Failed to install iOS profile through Apple Configurator: {error}"
        ))
    })?;
    print_install_session(&session);
    if session.completed {
        println!(
            "Configurator-installed certificate payloads are trusted for SSL/TLS automatically."
        );
    }
    Ok(())
}

fn handle_mobile_ca_install(
    ca_cert_path: &Path,
    requested_device_id: Option<&str>,
    non_interactive: bool,
) -> bifrost_core::Result<()> {
    let discovery = discover_android_devices_with_ca(Some(ca_cert_path));
    let Some(adb_path) = discovery.adb_path.as_ref().map(PathBuf::from) else {
        return Err(bifrost_core::BifrostError::Config(discovery.message));
    };

    let device = select_android_device(&discovery, requested_device_id, non_interactive)?;

    println!("Mobile CA install guide");
    println!("=======================");
    println!("Target Android device: {}", device_display_name(&device));
    println!("Certificate: {}", ca_cert_path.display());
    println!();
    println!("Bifrost will push the CA certificate and open Android's installer flow.");
    println!("Personal phones still require final confirmation on the phone.");
    println!("Android apps that do not trust user CAs or use certificate pinning may still reject interception.");
    print_android_certificate_status(&device);
    println!();

    let session = install_android_ca(AndroidInstallOptions {
        adb_path,
        device_id: device.id.clone(),
        ca_cert_path: ca_cert_path.to_path_buf(),
    })
    .map_err(|error| {
        bifrost_core::BifrostError::Config(format!("Failed to start mobile CA install: {error}"))
    })?;
    print_install_session(&session);
    let refreshed = discover_android_devices_with_ca(Some(ca_cert_path));
    if let Some(device) = refreshed
        .devices
        .iter()
        .find(|candidate| candidate.id == device.id)
    {
        print_android_certificate_status(device);
    }
    Ok(())
}

fn print_android_certificate_status(device: &MobileDevice) {
    if let Some(status) = &device.certificate_status {
        let state = match status.state {
            DeviceCertificateState::Unknown => "unknown",
            DeviceCertificateState::NotInstalled => "not installed",
            DeviceCertificateState::PushedToDevice => "pushed to device",
            DeviceCertificateState::Installed => "installed",
        };
        println!("Android CA status: {state}");
        println!("  {}", status.message);
    } else {
        println!("Android CA status: not checked");
    }
}

fn select_android_device(
    discovery: &AdbDiscovery,
    requested_device_id: Option<&str>,
    non_interactive: bool,
) -> bifrost_core::Result<MobileDevice> {
    if let Some(device_id) = requested_device_id {
        let Some(device) = discovery
            .devices
            .iter()
            .find(|device| device.id == device_id)
        else {
            return Err(bifrost_core::BifrostError::NotFound(format!(
                "Android device not found: {device_id}"
            )));
        };
        if device.status != DeviceStatus::Connected {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Android device is not ready: {}",
                device.status_message
            )));
        }
        return Ok(device.clone());
    }

    let connected = connected_android_devices(&discovery.devices);
    match connected.as_slice() {
        [] => Err(bifrost_core::BifrostError::Config(format!(
            "{} Enable USB debugging, approve this computer on the phone, then re-run `bifrost ca install --mobile`.",
            discovery.message
        ))),
        [device] => Ok((*device).clone()),
        devices if non_interactive => Err(bifrost_core::BifrostError::Config(format!(
            "Multiple ready Android devices detected ({}). Pass --device <serial> to select one.",
            devices.len()
        ))),
        devices => {
            let options = devices
                .iter()
                .map(device_display_name)
                .collect::<Vec<_>>();
            let selection = Select::new()
                .with_prompt("Select the Android device to install the CA on")
                .items(&options)
                .default(0)
                .interact()
                .map_err(|error| {
                    bifrost_core::BifrostError::Config(format!(
                        "Unable to select Android device interactively: {error}. Pass --device <serial> to run non-interactively."
                    ))
                })?;
            Ok(devices[selection].clone())
        }
    }
}

fn connected_android_devices(devices: &[MobileDevice]) -> Vec<MobileDevice> {
    devices
        .iter()
        .filter(|device| device.status == DeviceStatus::Connected)
        .cloned()
        .collect()
}

fn select_single_ios_device_for_configurator(
    devices: &[MobileDevice],
    requested_device_id: Option<&str>,
    non_interactive: bool,
) -> bifrost_core::Result<MobileDevice> {
    let connected = devices
        .iter()
        .filter(|device| device.status == DeviceStatus::Connected)
        .cloned()
        .collect::<Vec<_>>();

    if let Some(device_id) = requested_device_id {
        let Some(device) = connected.iter().find(|device| {
            device.id == device_id || device.managed_install_target.as_deref() == Some(device_id)
        }) else {
            return Err(bifrost_core::BifrostError::NotFound(format!(
                "Connected iOS device not found: {device_id}"
            )));
        };
        return Ok(device.clone());
    }

    match connected.as_slice() {
        [] => Err(bifrost_core::BifrostError::Config(
            "No connected iPhone/iPad was detected. Connect the device over USB, unlock it, tap Trust for this Mac if prompted, then re-run `bifrost ca install --ios --configurator`."
                .to_string(),
        )),
        [device] => Ok((*device).clone()),
        devices if non_interactive => Err(bifrost_core::BifrostError::Config(format!(
            "Multiple iPhone/iPad devices are connected ({}). Pass --device <id> to choose which device receives the profile.",
            devices.len()
        ))),
        devices => {
            let labels = devices.iter().map(device_display_name).collect::<Vec<_>>();
            let selection = Select::new()
                .with_prompt("Select the iPhone/iPad to install the CA profile on")
                .items(&labels)
                .default(0)
                .interact()
                .map_err(|error| {
                    bifrost_core::BifrostError::Config(format!(
                        "Unable to select iOS device interactively: {error}. Pass --device <id> to run non-interactively."
                    ))
                })?;
            Ok(devices[selection].clone())
        }
    }
}

fn device_display_name(device: &MobileDevice) -> String {
    match device.name.as_deref() {
        Some(name) if !name.trim().is_empty() => format!("{name} ({})", device.id),
        _ => device.id.clone(),
    }
}

fn print_install_session(session: &InstallSession) {
    println!("Install session: {}", session.session_id);
    for step in &session.steps {
        let marker = if step.success { "✓" } else { "✗" };
        println!("  {marker} {}: {}", step.name, step.message);
    }
    println!();
    println!("{}", session.summary);
    if session.requires_user_confirmation {
        println!("Next step: finish installation and trust confirmation on the phone.");
    }
}

pub fn load_tls_config(config: &ProxyConfig) -> bifrost_core::Result<Arc<TlsConfig>> {
    let cert_dir = get_bifrost_dir()?.join("certs");
    let ca_key_path = cert_dir.join("ca.key");
    let ca_cert_path = cert_dir.join("ca.crt");

    let ca_valid = ensure_valid_ca(&ca_cert_path, &ca_key_path)?;
    if !ca_valid {
        if config.enable_tls_interception {
            println!("TLS interception enabled but valid CA certificate not found.");
        } else {
            println!("Preparing CA certificate for runtime TLS interception...");
        }
        println!("Generating CA certificate...");
        std::fs::create_dir_all(&cert_dir)?;
        let ca = generate_root_ca()?;
        save_root_ca(&ca_cert_path, &ca_key_path, &ca)?;
        println!("✓ CA certificate generated: {}", ca_cert_path.display());
    }

    let ca = load_root_ca(&ca_cert_path, &ca_key_path)?;
    let ca_cert_bytes = std::fs::read(&ca_cert_path)?;
    let ca_key_bytes = std::fs::read(&ca_key_path)?;
    let ca_arc = Arc::new(ca);
    let sni_resolver = SniResolver::new(ca_arc.clone());
    let cert_generator = DynamicCertGenerator::new(ca_arc);

    if config.enable_tls_interception {
        println!("✓ TLS interception enabled");
    } else {
        println!("✓ CA certificate ready (TLS interception can be enabled at runtime)");
    }

    Ok(Arc::new(TlsConfig {
        ca_cert: Some(ca_cert_bytes),
        ca_key: Some(ca_key_bytes),
        cert_generator: Some(Arc::new(cert_generator)),
        sni_resolver: Some(Arc::new(sni_resolver)),
    }))
}

pub fn check_and_install_certificate(options: CertificateCheckOptions) -> bifrost_core::Result<()> {
    let cert_dir = get_bifrost_dir()?.join("certs");
    let ca_key_path = cert_dir.join("ca.key");
    let ca_cert_path = cert_dir.join("ca.crt");

    ensure_ca_exists(&ca_cert_path, &ca_key_path)?;

    let installer = CertInstaller::new(&ca_cert_path);
    let status = installer.check_status()?;

    match certificate_resolution(&status, options) {
        CertificateResolution::AlreadyTrusted => {
            println!("✓ CA certificate is installed and trusted.");
            Ok(())
        }
        CertificateResolution::AutoInstallAndTrust => {
            println!("> yes (auto-confirmed by --yes)");
            installer.install_and_trust()?;
            println!("✓ CA certificate installed and trusted.");
            Ok(())
        }
        CertificateResolution::PromptTrust => {
            println!("⚠ CA certificate is installed but not trusted.");
            println!();
            prompt_trust_certificate(&installer)
        }
        CertificateResolution::PromptInstall => {
            println!("⚠ CA certificate is not installed in system trust store.");
            println!();
            prompt_install_certificate(&installer)
        }
        CertificateResolution::BlockNonInteractive => {
            let state = match status {
                CertStatus::NotInstalled => "not installed in the system trust store",
                CertStatus::InstalledNotTrusted => "installed but not trusted by the system",
                CertStatus::InstalledAndTrusted => {
                    unreachable!("trusted status is handled earlier")
                }
            };
            Err(bifrost_core::BifrostError::Config(format!(
                "CA certificate is {state}, and no interactive terminal is available. \
                 Re-run with --yes to install and trust it automatically, run 'bifrost ca install' first, \
                 or pass --skip-cert-check if you intentionally want to start without a trusted CA."
            )))
        }
    }
}

pub(crate) fn ensure_ca_exists(
    ca_cert_path: &Path,
    ca_key_path: &Path,
) -> bifrost_core::Result<()> {
    let ca_valid = ensure_valid_ca(ca_cert_path, ca_key_path)?;
    if !ca_valid {
        println!("Valid CA certificate not found. Generating...");
        if let Some(parent) = ca_cert_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let ca = generate_root_ca()?;
        save_root_ca(ca_cert_path, ca_key_path, &ca)?;
        println!("✓ CA certificate generated.");
        println!("  Certificate: {}", ca_cert_path.display());
        println!();
    }

    Ok(())
}

fn prompt_install_certificate(installer: &CertInstaller) -> bifrost_core::Result<()> {
    println!("HTTPS interception requires the CA certificate to be trusted by the system.");
    println!("Without it, browsers will show security warnings for HTTPS sites.");
    println!();
    println!("Platform: {}", get_platform_name());
    #[cfg(target_os = "macos")]
    println!("macOS will install the certificate into the System keychain.");
    println!();

    let options = vec![
        "Yes, install and trust",
        "No, skip (HTTPS interception may not work properly)",
        "Show manual installation instructions",
    ];

    let selection = Select::new()
        .with_prompt("Would you like to install and trust the CA certificate?")
        .items(&options)
        .default(0)
        .interact();

    match selection {
        Ok(0) => {
            installer.install_and_trust()?;
            Ok(())
        }
        Ok(1) => {
            println!("Skipping certificate installation.");
            println!("You can install it later using 'bifrost ca install' or manually.");
            Ok(())
        }
        Ok(2) => {
            println!();
            println!("{}", installer.get_install_instructions());
            println!();

            let proceed = Confirm::new()
                .with_prompt("Continue without installing?")
                .default(true)
                .interact();

            match proceed {
                Ok(true) => Ok(()),
                Ok(false) => prompt_install_certificate(installer),
                Err(_) => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

fn prompt_trust_certificate(installer: &CertInstaller) -> bifrost_core::Result<()> {
    println!("The CA certificate is installed but not trusted by the system.");
    println!("HTTPS interception may not work properly without trust.");
    println!();
    #[cfg(target_os = "macos")]
    println!("macOS will install the certificate into the System keychain.");
    #[cfg(target_os = "macos")]
    println!();

    let proceed = Confirm::new()
        .with_prompt("Would you like to trust the CA certificate now?")
        .default(true)
        .interact();

    match proceed {
        Ok(true) => {
            installer.install_and_trust()?;
            Ok(())
        }
        Ok(false) => {
            println!("Skipping. You can trust it later manually.");
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(auto_yes: bool, allow_prompt: bool) -> CertificateCheckOptions {
        CertificateCheckOptions {
            auto_yes,
            allow_prompt,
        }
    }

    #[test]
    fn certificate_resolution_blocks_non_interactive_missing_ca_without_yes() {
        assert_eq!(
            certificate_resolution(&CertStatus::NotInstalled, opts(false, false)),
            CertificateResolution::BlockNonInteractive
        );
    }

    #[test]
    fn certificate_resolution_auto_installs_when_yes() {
        assert_eq!(
            certificate_resolution(&CertStatus::NotInstalled, opts(true, false)),
            CertificateResolution::AutoInstallAndTrust
        );
        assert_eq!(
            certificate_resolution(&CertStatus::InstalledNotTrusted, opts(true, false)),
            CertificateResolution::AutoInstallAndTrust
        );
    }

    #[test]
    fn certificate_resolution_prompts_when_terminal_available() {
        assert_eq!(
            certificate_resolution(&CertStatus::NotInstalled, opts(false, true)),
            CertificateResolution::PromptInstall
        );
        assert_eq!(
            certificate_resolution(&CertStatus::InstalledNotTrusted, opts(false, true)),
            CertificateResolution::PromptTrust
        );
    }

    #[test]
    fn certificate_resolution_keeps_trusted_status_ready() {
        assert_eq!(
            certificate_resolution(&CertStatus::InstalledAndTrusted, opts(false, false)),
            CertificateResolution::AlreadyTrusted
        );
    }

    fn android_device(id: &str, status: DeviceStatus) -> MobileDevice {
        MobileDevice {
            id: id.to_string(),
            name: None,
            managed_install_target: None,
            platform: bifrost_device::MobilePlatform::Android,
            status,
            capability: bifrost_device::DeviceTrustCapability::PushAndOpenInstaller,
            certificate_status: None,
            status_message: "status message".to_string(),
        }
    }

    fn discovery(devices: Vec<MobileDevice>) -> AdbDiscovery {
        AdbDiscovery {
            adb_available: true,
            adb_path: Some("/tmp/adb".to_string()),
            devices,
            message: "ADB found devices.".to_string(),
        }
    }

    #[test]
    fn mobile_ca_install_auto_selects_single_connected_device() {
        let selected = select_android_device(
            &discovery(vec![android_device("android-1", DeviceStatus::Connected)]),
            None,
            true,
        )
        .expect("single connected device should be selected");

        assert_eq!(selected.id, "android-1");
    }

    #[test]
    fn mobile_ca_install_requires_device_for_multiple_non_interactive_devices() {
        let err = select_android_device(
            &discovery(vec![
                android_device("android-1", DeviceStatus::Connected),
                android_device("android-2", DeviceStatus::Connected),
            ]),
            None,
            true,
        )
        .expect_err("multiple devices should require --device in non-interactive mode");

        assert!(err.to_string().contains("--device <serial>"));
    }

    #[test]
    fn mobile_ca_install_rejects_requested_unauthorized_device() {
        let err = select_android_device(
            &discovery(vec![android_device(
                "android-1",
                DeviceStatus::Unauthorized,
            )]),
            Some("android-1"),
            true,
        )
        .expect_err("unauthorized device cannot be installed");

        assert!(err.to_string().contains("not ready"));
    }

    #[test]
    fn connected_android_devices_filters_by_connected_status() {
        let devices = vec![
            android_device("connected", DeviceStatus::Connected),
            android_device("disconnected", DeviceStatus::Offline),
            android_device("unauthorized", DeviceStatus::Unauthorized),
        ];
        let connected = connected_android_devices(&devices);
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].id, "connected");
    }

    fn ios_device(id: &str, status: DeviceStatus, target: Option<&str>) -> MobileDevice {
        MobileDevice {
            id: id.to_string(),
            name: None,
            managed_install_target: target.map(|t| t.to_string()),
            platform: bifrost_device::MobilePlatform::Ios,
            status,
            capability: bifrost_device::DeviceTrustCapability::PushAndOpenInstaller,
            certificate_status: None,
            status_message: "status".to_string(),
        }
    }

    #[test]
    fn select_single_ios_device_resolves_by_id_or_target() {
        let devices = vec![
            ios_device("ios-1", DeviceStatus::Connected, Some("target-1")),
            ios_device("ios-2", DeviceStatus::Connected, Some("target-2")),
        ];

        let selected = select_single_ios_device_for_configurator(&devices, Some("ios-2"), true)
            .expect("select by id");
        assert_eq!(selected.id, "ios-2");

        let selected = select_single_ios_device_for_configurator(&devices, Some("target-1"), true)
            .expect("select by managed target");
        assert_eq!(selected.id, "ios-1");
    }

    #[test]
    fn select_single_ios_device_requires_device_when_multiple_non_interactive() {
        let devices = vec![
            ios_device("ios-1", DeviceStatus::Connected, None),
            ios_device("ios-2", DeviceStatus::Connected, None),
        ];
        let err = select_single_ios_device_for_configurator(&devices, None, true)
            .expect_err("multiple devices should require --device");
        assert!(err.to_string().contains("Pass --device <id>"));
    }

    #[test]
    fn device_display_name_uses_friendly_name_when_available() {
        let named = MobileDevice {
            id: "abc123".to_string(),
            name: Some("My Phone".to_string()),
            managed_install_target: None,
            platform: bifrost_device::MobilePlatform::Android,
            status: DeviceStatus::Connected,
            capability: bifrost_device::DeviceTrustCapability::PushAndOpenInstaller,
            certificate_status: None,
            status_message: String::new(),
        };
        let unnamed = MobileDevice {
            id: "xyz456".to_string(),
            name: None,
            managed_install_target: None,
            platform: bifrost_device::MobilePlatform::Android,
            status: DeviceStatus::Connected,
            capability: bifrost_device::DeviceTrustCapability::PushAndOpenInstaller,
            certificate_status: None,
            status_message: String::new(),
        };

        assert_eq!(device_display_name(&named), "My Phone (abc123)");
        assert_eq!(device_display_name(&unnamed), "xyz456");
    }
}
