use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        CaCommands::Install => {
            ensure_ca_exists(&ca_cert_path, &ca_key_path)?;

            let installer = CertInstaller::new(&ca_cert_path);
            installer.install_and_trust()?;
            println!("CA certificate installed and trusted successfully.");
            println!("Certificate: {}", ca_cert_path.display());
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

fn ensure_ca_exists(ca_cert_path: &Path, ca_key_path: &Path) -> bifrost_core::Result<()> {
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
}
