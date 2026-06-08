use bifrost_core::error::{BifrostError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "macos")]
use security_framework::authorization::{
    Authorization, AuthorizationItemSetBuilder, Flags as AuthorizationFlags,
};
#[cfg(target_os = "windows")]
use sha1::{Digest as Sha1Digest, Sha1};

#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::mem::size_of;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{CloseHandle, HWND};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn install_cert_with_uac(cert_path: &Path) -> bool {
    let verb = to_wide("runas");
    let file = to_wide("certutil");
    let params = to_wide(format!("-addstore Root \"{}\"", cert_path.display()).as_str());
    let mut exec_info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: HWND(std::ptr::null_mut()),
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOW.0,
        ..Default::default()
    };
    let launched = unsafe { ShellExecuteExW(&mut exec_info) }.is_ok();
    if !launched || exec_info.hProcess.is_invalid() {
        return false;
    }
    unsafe {
        WaitForSingleObject(exec_info.hProcess, INFINITE);
    }
    let mut exit_code: u32 = 1;
    let got_exit = unsafe { GetExitCodeProcess(exec_info.hProcess, &mut exit_code) }.is_ok();
    unsafe {
        let _ = CloseHandle(exec_info.hProcess);
    }
    got_exit && exit_code == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertStatus {
    NotInstalled,
    InstalledNotTrusted,
    InstalledAndTrusted,
}

impl std::fmt::Display for CertStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertStatus::NotInstalled => write!(f, "Not installed"),
            CertStatus::InstalledNotTrusted => write!(f, "Installed but not trusted"),
            CertStatus::InstalledAndTrusted => write!(f, "Installed and trusted"),
        }
    }
}

impl CertStatus {
    pub fn is_installed(self) -> bool {
        !matches!(self, Self::NotInstalled)
    }

    pub fn is_trusted(self) -> bool {
        matches!(self, Self::InstalledAndTrusted)
    }
}

#[derive(Debug, Clone)]
pub struct CertSystemInfo {
    pub status: CertStatus,
    pub keychain_location: Option<String>,
    pub system_cert_path: Option<PathBuf>,
    pub fingerprint_match: Option<bool>,
}

pub struct CertInstaller {
    cert_path: std::path::PathBuf,
    #[allow(dead_code)]
    cert_name: String,
}

impl CertInstaller {
    pub fn new<P: AsRef<Path>>(cert_path: P) -> Self {
        Self {
            cert_path: cert_path.as_ref().to_path_buf(),
            cert_name: "Bifrost CA".to_string(),
        }
    }

    pub fn check_status(&self) -> Result<CertStatus> {
        #[cfg(target_os = "macos")]
        return self.check_status_macos();

        #[cfg(target_os = "linux")]
        return self.check_status_linux();

        #[cfg(target_os = "windows")]
        return self.check_status_windows();

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return Err(BifrostError::Config(
            "Unsupported operating system".to_string(),
        ));
    }

    pub fn get_detailed_status(&self) -> Result<CertSystemInfo> {
        #[cfg(target_os = "macos")]
        return self.get_detailed_status_macos();

        #[cfg(target_os = "linux")]
        return self.get_detailed_status_linux();

        #[cfg(target_os = "windows")]
        return self.get_detailed_status_windows();

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return Err(BifrostError::Config(
            "Unsupported operating system".to_string(),
        ));
    }

    pub fn install_and_trust(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        return self.install_macos();

        #[cfg(target_os = "linux")]
        return self.install_linux();

        #[cfg(target_os = "windows")]
        return self.install_windows();

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return Err(BifrostError::Config(
            "Unsupported operating system".to_string(),
        ));
    }

    pub fn install_and_trust_gui(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        return self.install_macos_gui();

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        return self.install_and_trust();

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return Err(BifrostError::Config(
            "Unsupported operating system".to_string(),
        ));
    }

    pub fn get_install_instructions(&self) -> String {
        #[cfg(target_os = "macos")]
        return format!(
            "To manually install the certificate on macOS:\n\
             1. Double-click the certificate file: {}\n\
             2. Add it to the 'System' keychain\n\
             3. Open Keychain Access, find 'Bifrost CA'\n\
             4. Double-click it, expand 'Trust', set 'Always Trust'",
            self.cert_path.display()
        );

        #[cfg(target_os = "linux")]
        return format!(
            "To manually install the certificate on Linux:\n\
             1. Copy the certificate:\n\
                sudo cp {} /usr/local/share/ca-certificates/bifrost-ca.crt\n\
             2. Update CA certificates:\n\
                sudo update-ca-certificates",
            self.cert_path.display()
        );

        #[cfg(target_os = "windows")]
        return format!(
            "To manually install the certificate on Windows:\n\
             1. Open Command Prompt as Administrator\n\
             2. Run: certutil -addstore Root \"{}\"",
            self.cert_path.display()
        );

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return "Manual certificate installation required for your platform.".to_string();
    }

    #[cfg(target_os = "macos")]
    fn check_status_macos(&self) -> Result<CertStatus> {
        let current_fingerprint = self.current_cert_fingerprint_macos()?;
        let system_match = keychain_contains_fingerprint_macos(
            "/Library/Keychains/System.keychain",
            &self.cert_name,
            &current_fingerprint,
        )?;

        if system_match {
            if self.check_trust_macos()? {
                Ok(CertStatus::InstalledAndTrusted)
            } else {
                Ok(CertStatus::InstalledNotTrusted)
            }
        } else {
            Ok(CertStatus::NotInstalled)
        }
    }

    #[cfg(target_os = "macos")]
    fn check_trust_macos(&self) -> Result<bool> {
        if !self.cert_path.exists() {
            return Ok(false);
        }

        let output = Command::new("security")
            .args([
                "verify-cert",
                "-c",
                self.cert_path.to_str().unwrap_or(""),
                "-p",
                "ssl",
            ])
            .output();

        match output {
            Ok(out) => Ok(out.status.success()),
            Err(_) => Ok(false),
        }
    }

    #[cfg(target_os = "macos")]
    fn get_detailed_status_macos(&self) -> Result<CertSystemInfo> {
        let current_fingerprint = self.current_cert_fingerprint_macos()?;
        let system_keychain = "/Library/Keychains/System.keychain";
        let system_fingerprints =
            list_macos_keychain_fingerprints(system_keychain, &self.cert_name)?;
        let system_match = system_fingerprints.contains(&current_fingerprint);
        let trusted = self.check_trust_macos()?;

        if system_match {
            return Ok(CertSystemInfo {
                status: if trusted {
                    CertStatus::InstalledAndTrusted
                } else {
                    CertStatus::InstalledNotTrusted
                },
                keychain_location: Some("System Keychain".to_string()),
                system_cert_path: Some(PathBuf::from(system_keychain)),
                fingerprint_match: Some(true),
            });
        }

        if !system_fingerprints.is_empty() {
            return Ok(CertSystemInfo {
                status: CertStatus::NotInstalled,
                keychain_location: Some("System Keychain".to_string()),
                system_cert_path: Some(PathBuf::from(system_keychain)),
                fingerprint_match: Some(false),
            });
        }

        Ok(CertSystemInfo {
            status: CertStatus::NotInstalled,
            keychain_location: None,
            system_cert_path: None,
            fingerprint_match: None,
        })
    }

    #[cfg(target_os = "macos")]
    fn current_cert_fingerprint_macos(&self) -> Result<String> {
        let output = Command::new("openssl")
            .args([
                "x509",
                "-in",
                self.cert_path.to_str().unwrap_or(""),
                "-noout",
                "-fingerprint",
                "-sha256",
            ])
            .output()
            .map_err(|e| BifrostError::Tls(format!("Failed to execute openssl: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BifrostError::Tls(format!(
                "openssl fingerprint failed: {}",
                stderr.trim()
            )));
        }

        parse_openssl_sha256_fingerprint(&String::from_utf8_lossy(&output.stdout)).ok_or_else(
            || BifrostError::Tls("Failed to parse current certificate fingerprint".to_string()),
        )
    }

    #[cfg(target_os = "macos")]
    fn install_macos(&self) -> Result<()> {
        if !self.cert_path.exists() {
            return Err(BifrostError::NotFound(format!(
                "Certificate file not found: {}",
                self.cert_path.display()
            )));
        }

        println!("Installing CA certificate to System keychain...");
        println!("This requires administrator privileges.");
        purge_macos_named_certificates(&self.cert_name, &resolve_macos_login_keychain()?, false)?;
        install_macos_cert_to_system_keychain(&self.cert_path)?;
        println!("✓ CA certificate installed and trusted successfully.");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn install_macos_gui(&self) -> Result<()> {
        if !self.cert_path.exists() {
            return Err(BifrostError::NotFound(format!(
                "Certificate file not found: {}",
                self.cert_path.display()
            )));
        }

        println!("Installing CA certificate to System keychain...");
        println!("macOS will prompt for administrator authorization.");
        purge_macos_named_certificates(&self.cert_name, &resolve_macos_login_keychain()?, false)?;
        run_macos_security_add_trusted_cert_gui(&self.cert_path)?;
        println!("✓ CA certificate installed and trusted successfully.");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn check_status_linux(&self) -> Result<CertStatus> {
        let system_cert_path = Path::new("/usr/local/share/ca-certificates/bifrost-ca.crt");
        let trusted_cert_link = Path::new("/etc/ssl/certs/bifrost-ca.pem");

        if system_cert_path.exists() {
            if trusted_cert_link.exists() || self.check_cert_in_bundle_linux() {
                Ok(CertStatus::InstalledAndTrusted)
            } else {
                Ok(CertStatus::InstalledNotTrusted)
            }
        } else {
            Ok(CertStatus::NotInstalled)
        }
    }

    #[cfg(target_os = "linux")]
    fn check_cert_in_bundle_linux(&self) -> bool {
        let ca_bundle = Path::new("/etc/ssl/certs/ca-certificates.crt");
        if ca_bundle.exists() {
            if let Ok(content) = std::fs::read_to_string(ca_bundle) {
                return content.contains("Bifrost CA");
            }
        }
        false
    }

    #[cfg(target_os = "linux")]
    fn get_detailed_status_linux(&self) -> Result<CertSystemInfo> {
        let system_cert_path = PathBuf::from("/usr/local/share/ca-certificates/bifrost-ca.crt");
        let trusted_cert_link = Path::new("/etc/ssl/certs/bifrost-ca.pem");

        if system_cert_path.exists() {
            let is_trusted = trusted_cert_link.exists() || self.check_cert_in_bundle_linux();
            Ok(CertSystemInfo {
                status: if is_trusted {
                    CertStatus::InstalledAndTrusted
                } else {
                    CertStatus::InstalledNotTrusted
                },
                keychain_location: Some("System CA Store".to_string()),
                system_cert_path: Some(system_cert_path),
                fingerprint_match: Some(true),
            })
        } else {
            Ok(CertSystemInfo {
                status: CertStatus::NotInstalled,
                keychain_location: None,
                system_cert_path: None,
                fingerprint_match: None,
            })
        }
    }

    #[cfg(target_os = "linux")]
    fn install_linux(&self) -> Result<()> {
        if !self.cert_path.exists() {
            return Err(BifrostError::NotFound(format!(
                "Certificate file not found: {}",
                self.cert_path.display()
            )));
        }

        println!("Installing CA certificate to system trust store...");
        println!("This requires administrator privileges.");

        let copy_status = Command::new("sudo")
            .args([
                "cp",
                self.cert_path.to_str().unwrap_or(""),
                "/usr/local/share/ca-certificates/bifrost-ca.crt",
            ])
            .status();

        match copy_status {
            Ok(status) if status.success() => {}
            Ok(_) => {
                return Err(BifrostError::Tls(
                    "Failed to copy certificate to system directory".to_string(),
                ))
            }
            Err(e) => {
                return Err(BifrostError::Tls(format!(
                    "Failed to execute copy command: {}",
                    e
                )))
            }
        }

        let update_status = Command::new("sudo")
            .args(["update-ca-certificates"])
            .status();

        match update_status {
            Ok(status) => {
                if status.success() {
                    println!("✓ CA certificate installed and trusted successfully.");
                    Ok(())
                } else {
                    Err(BifrostError::Tls(
                        "Failed to update CA certificates".to_string(),
                    ))
                }
            }
            Err(e) => Err(BifrostError::Tls(format!(
                "Failed to execute update-ca-certificates: {}",
                e
            ))),
        }
    }

    #[cfg(target_os = "windows")]
    fn check_status_windows(&self) -> Result<CertStatus> {
        let current_thumbprint = self.current_cert_thumbprint_windows()?;
        let machine_match =
            windows_store_contains_thumbprint(None, "Root", &self.cert_name, &current_thumbprint)?;
        let user_match = windows_store_contains_thumbprint(
            Some("user"),
            "Root",
            &self.cert_name,
            &current_thumbprint,
        )?;

        if machine_match || user_match {
            Ok(CertStatus::InstalledAndTrusted)
        } else {
            Ok(CertStatus::NotInstalled)
        }
    }

    #[cfg(target_os = "windows")]
    fn get_detailed_status_windows(&self) -> Result<CertSystemInfo> {
        let current_thumbprint = self.current_cert_thumbprint_windows()?;
        let machine_thumbprints = list_windows_store_thumbprints(None, "Root", &self.cert_name)?;
        let user_thumbprints =
            list_windows_store_thumbprints(Some("user"), "Root", &self.cert_name)?;
        let machine_match = machine_thumbprints.contains(&current_thumbprint);
        let user_match = user_thumbprints.contains(&current_thumbprint);

        let location = match (user_match, machine_match) {
            (true, true) => Some("Current User + Local Machine Root Store".to_string()),
            (true, false) => Some("Current User Root Store".to_string()),
            (false, true) => Some("Local Machine Root Store".to_string()),
            (false, false) => None,
        };

        if user_match || machine_match {
            return Ok(CertSystemInfo {
                status: CertStatus::InstalledAndTrusted,
                keychain_location: location,
                system_cert_path: None,
                fingerprint_match: Some(true),
            });
        }

        if !machine_thumbprints.is_empty() || !user_thumbprints.is_empty() {
            return Ok(CertSystemInfo {
                status: CertStatus::NotInstalled,
                keychain_location: match (
                    !user_thumbprints.is_empty(),
                    !machine_thumbprints.is_empty(),
                ) {
                    (true, true) => Some("Current User + Local Machine Root Store".to_string()),
                    (true, false) => Some("Current User Root Store".to_string()),
                    (false, true) => Some("Local Machine Root Store".to_string()),
                    (false, false) => None,
                },
                system_cert_path: None,
                fingerprint_match: Some(false),
            });
        }

        Ok(CertSystemInfo {
            status: CertStatus::NotInstalled,
            keychain_location: None,
            system_cert_path: None,
            fingerprint_match: None,
        })
    }

    #[cfg(target_os = "windows")]
    fn current_cert_thumbprint_windows(&self) -> Result<String> {
        let cert_bytes = std::fs::read(&self.cert_path).map_err(|e| {
            BifrostError::Tls(format!(
                "Failed to read certificate file {}: {}",
                self.cert_path.display(),
                e
            ))
        })?;
        let mut reader = std::io::BufReader::new(cert_bytes.as_slice());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| BifrostError::Tls(format!("Failed to parse PEM certificate: {}", e)))?;
        let cert = certs
            .first()
            .ok_or_else(|| BifrostError::Tls("No certificate found in PEM file".to_string()))?;

        Ok(hex_uppercase(&Sha1::digest(cert.as_ref())))
    }

    #[cfg(target_os = "windows")]
    fn install_windows(&self) -> Result<()> {
        if !self.cert_path.exists() {
            return Err(BifrostError::NotFound(format!(
                "Certificate file not found: {}",
                self.cert_path.display()
            )));
        }

        println!("Installing CA certificate to Windows Root certificate store...");
        println!("Trying Current User Root first.");

        let cert_path = self.cert_path.to_str().unwrap_or("");
        let output = Command::new("certutil")
            .args(["-user", "-addstore", "Root", cert_path])
            .status();

        match output {
            Ok(status) => {
                if status.success() {
                    println!("✓ CA certificate installed and trusted successfully.");
                    Ok(())
                } else {
                    println!("Current User Root install did not succeed.");
                    println!("Requesting administrator approval to install the certificate...");
                    if install_cert_with_uac(&self.cert_path) {
                        println!("✓ CA certificate installed and trusted successfully.");
                        Ok(())
                    } else {
                        println!();
                        println!(
                            "Failed to install certificate. Please try running as Administrator:"
                        );
                        println!("  certutil -addstore Root \"{}\"", self.cert_path.display());
                        Err(BifrostError::Tls(
                            "Failed to install CA certificate. Administrator privileges required."
                                .to_string(),
                        ))
                    }
                }
            }
            Err(e) => Err(BifrostError::Tls(format!(
                "Failed to execute certutil: {}",
                e
            ))),
        }
    }
}

#[cfg(target_os = "macos")]
fn install_macos_cert_to_system_keychain(cert_path: &Path) -> Result<()> {
    run_macos_security_add_trusted_cert(
        cert_path,
        Path::new("/Library/Keychains/System.keychain"),
        true,
    )
}

#[cfg(target_os = "macos")]
fn resolve_macos_login_keychain() -> Result<PathBuf> {
    if let Ok(output) = Command::new("security")
        .args(["default-keychain", "-d", "user"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let trimmed = stdout.trim().trim_matches('"');
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed));
            }
        }
    }

    let home = std::env::var("HOME").map_err(|e| {
        BifrostError::Tls(format!("Failed to resolve HOME for login keychain: {}", e))
    })?;
    Ok(PathBuf::from(home).join("Library/Keychains/login.keychain-db"))
}

#[cfg(target_os = "macos")]
fn run_macos_security_add_trusted_cert(
    cert_path: &Path,
    keychain: &Path,
    use_sudo: bool,
) -> Result<()> {
    let cert_path = cert_path.to_str().ok_or_else(|| {
        BifrostError::Tls(format!(
            "Certificate path is not valid UTF-8: {}",
            cert_path.display()
        ))
    })?;
    let keychain = keychain.to_str().ok_or_else(|| {
        BifrostError::Tls(format!(
            "Keychain path is not valid UTF-8: {}",
            keychain.display()
        ))
    })?;

    let mut command = if use_sudo {
        let mut cmd = Command::new("sudo");
        cmd.arg("security");
        cmd
    } else {
        Command::new("security")
    };
    let output = command
        .args([
            "add-trusted-cert",
            "-d",
            "-r",
            "trustRoot",
            "-k",
            keychain,
            cert_path,
        ])
        .output()
        .map_err(|e| BifrostError::Tls(format!("Failed to execute security command: {}", e)))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(BifrostError::Tls(format!(
        "security add-trusted-cert failed for {}: {} {}",
        keychain,
        stdout.trim(),
        stderr.trim()
    )))
}

#[cfg(target_os = "macos")]
fn run_macos_security_add_trusted_cert_gui(cert_path: &Path) -> Result<()> {
    let cert_path = cert_path.to_str().ok_or_else(|| {
        BifrostError::Tls(format!(
            "Certificate path is not valid UTF-8: {}",
            cert_path.display()
        ))
    })?;
    let args = [
        "add-trusted-cert",
        "-d",
        "-r",
        "trustRoot",
        "-k",
        "/Library/Keychains/System.keychain",
        cert_path,
    ];
    match run_macos_security_with_authorization(&args) {
        Ok(()) => Ok(()),
        Err(error) => {
            if is_macos_user_cancelled(&error.to_string()) {
                return Err(error);
            }
            run_macos_security_add_trusted_cert_osascript(cert_path).map_err(|fallback_error| {
                BifrostError::Tls(format!(
                    "{error}; AppleScript administrator authorization fallback failed: {fallback_error}"
                ))
            })
        }
    }
}

#[cfg(target_os = "macos")]
fn run_macos_security_add_trusted_cert_osascript(cert_path: &str) -> Result<()> {
    let command = format!(
        "/usr/bin/security add-trusted-cert -d -r trustRoot -k {} {}",
        shell_quote("/Library/Keychains/System.keychain"),
        shell_quote(cert_path)
    );
    let script = format!(
        "do shell script {} with administrator privileges",
        applescript_string_literal(&command)
    );
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .map_err(|error| {
            BifrostError::Tls(format!(
                "Failed to execute osascript administrator authorization: {error}"
            ))
        })?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_macos_user_cancelled(&stderr) {
        return Err(BifrostError::Tls(format!(
            "UserCancelled: {}",
            stderr.trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(BifrostError::Tls(format!(
        "osascript administrator authorization failed: {} {}",
        stdout.trim(),
        stderr.trim()
    )))
}

#[cfg(any(test, target_os = "macos"))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(test, target_os = "macos"))]
fn applescript_string_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn list_macos_keychain_fingerprints(keychain: &str, cert_name: &str) -> Result<Vec<String>> {
    let output = Command::new("security")
        .args(["find-certificate", "-c", cert_name, "-a", "-Z", keychain])
        .output()
        .map_err(|e| {
            BifrostError::Tls(format!(
                "Failed to execute security find-certificate: {}",
                e
            ))
        })?;

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_security_sha256_fingerprint)
        .collect())
}

#[cfg(target_os = "macos")]
fn keychain_contains_fingerprint_macos(
    keychain: &str,
    cert_name: &str,
    fingerprint: &str,
) -> Result<bool> {
    Ok(list_macos_keychain_fingerprints(keychain, cert_name)?
        .iter()
        .any(|candidate| candidate == fingerprint))
}

#[cfg(target_os = "macos")]
fn purge_macos_named_certificates(cert_name: &str, keychain: &Path, use_sudo: bool) -> Result<()> {
    let keychain = keychain.to_str().ok_or_else(|| {
        BifrostError::Tls(format!(
            "Keychain path is not valid UTF-8: {}",
            keychain.display()
        ))
    })?;

    let mut command = if use_sudo {
        let mut cmd = Command::new("sudo");
        cmd.arg("security");
        cmd
    } else {
        Command::new("security")
    };

    let output = command
        .args(["delete-certificate", "-c", cert_name, keychain])
        .output()
        .map_err(|e| {
            BifrostError::Tls(format!(
                "Failed to execute security delete-certificate: {}",
                e
            ))
        })?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_macos_delete_certificate_not_found(&stderr) {
        return Ok(());
    }

    Err(BifrostError::Tls(format!(
        "security delete-certificate failed for {}: {}",
        keychain,
        stderr.trim()
    )))
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn run_macos_security_with_authorization(args: &[&str]) -> Result<()> {
    let rights = AuthorizationItemSetBuilder::new()
        .add_right("system.privilege.admin")
        .map_err(|error| BifrostError::Tls(format!("Failed to build auth rights: {error}")))?
        .build();
    let auth = Authorization::new(
        Some(rights),
        None,
        AuthorizationFlags::INTERACTION_ALLOWED
            | AuthorizationFlags::EXTEND_RIGHTS
            | AuthorizationFlags::PREAUTHORIZE,
    )
    .map_err(|error| {
        map_macos_authorization_error("Failed to request macOS authorization", &error.to_string())
    })?;

    auth.execute_with_privileges(
        "/usr/bin/security",
        args.iter().copied(),
        AuthorizationFlags::DEFAULTS,
    )
    .map_err(|error| {
        map_macos_authorization_error("security authorization command failed", &error.to_string())
    })
}

#[cfg(any(test, target_os = "macos"))]
fn is_macos_delete_certificate_not_found(message: &str) -> bool {
    message.contains("could not find item")
        || message.contains("The specified item could not be found")
        || message.contains("Unable to delete certificate matching")
}

#[cfg(target_os = "macos")]
fn map_macos_authorization_error(context: &str, message: &str) -> BifrostError {
    if is_macos_user_cancelled(message) {
        return BifrostError::Tls(format!("UserCancelled: {message}"));
    }

    BifrostError::Tls(format!("{context}: {message}"))
}

#[cfg(any(test, target_os = "macos"))]
fn is_macos_user_cancelled(message: &str) -> bool {
    let lowercase = message.to_ascii_lowercase();
    lowercase.contains("user canceled")
        || lowercase.contains("user cancelled")
        || lowercase.contains("canceled by the user")
        || lowercase.contains("cancelled by the user")
        || lowercase.contains("errauthorizationcanceled")
}

#[cfg(any(test, target_os = "macos"))]
fn parse_security_sha256_fingerprint(line: &str) -> Option<String> {
    let (label, value) = line.split_once(':')?;
    if !label.trim().eq_ignore_ascii_case("SHA-256 hash") {
        return None;
    }

    let fingerprint = normalize_thumbprint(value);
    (!fingerprint.is_empty()).then_some(fingerprint)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_openssl_sha256_fingerprint(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        if !line.to_ascii_lowercase().contains("fingerprint") {
            return None;
        }

        let (_, value) = line.split_once('=')?;
        let fingerprint = normalize_thumbprint(value);
        (!fingerprint.is_empty()).then_some(fingerprint)
    })
}

#[cfg(target_os = "windows")]
fn list_windows_store_thumbprints(
    store_scope: Option<&str>,
    store_name: &str,
    cert_name: &str,
) -> Result<Vec<String>> {
    let mut command = Command::new("certutil");
    if let Some(scope) = store_scope {
        command.arg(format!("-{scope}"));
    }
    let output = command
        .args(["-store", store_name, cert_name])
        .output()
        .map_err(|e| BifrostError::Tls(format!("Failed to execute certutil: {}", e)))?;

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_windows_certutil_thumbprint)
        .collect())
}

#[cfg(target_os = "windows")]
fn windows_store_contains_thumbprint(
    store_scope: Option<&str>,
    store_name: &str,
    cert_name: &str,
    thumbprint: &str,
) -> Result<bool> {
    Ok(
        list_windows_store_thumbprints(store_scope, store_name, cert_name)?
            .iter()
            .any(|candidate| candidate == thumbprint),
    )
}

#[cfg(any(target_os = "windows", test))]
fn parse_windows_certutil_thumbprint(line: &str) -> Option<String> {
    line.split_once("(sha1):")
        .map(|(_, value)| normalize_thumbprint(value))
        .filter(|value| value.len() == 40)
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn normalize_thumbprint(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .map(|character| character.to_ascii_uppercase())
        .collect()
}

#[cfg(target_os = "windows")]
fn hex_uppercase(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

pub fn get_platform_name() -> &'static str {
    #[cfg(target_os = "macos")]
    return "macOS";

    #[cfg(target_os = "linux")]
    return "Linux";

    #[cfg(target_os = "windows")]
    return "Windows";

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return "Unknown";
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_cert_status_display() {
        assert_eq!(CertStatus::NotInstalled.to_string(), "Not installed");
        assert_eq!(
            CertStatus::InstalledNotTrusted.to_string(),
            "Installed but not trusted"
        );
        assert_eq!(
            CertStatus::InstalledAndTrusted.to_string(),
            "Installed and trusted"
        );
    }

    #[test]
    fn test_cert_status_helpers() {
        assert!(!CertStatus::NotInstalled.is_installed());
        assert!(!CertStatus::NotInstalled.is_trusted());

        assert!(CertStatus::InstalledNotTrusted.is_installed());
        assert!(!CertStatus::InstalledNotTrusted.is_trusted());

        assert!(CertStatus::InstalledAndTrusted.is_installed());
        assert!(CertStatus::InstalledAndTrusted.is_trusted());
    }

    #[test]
    fn test_cert_installer_new() {
        let dir = tempdir().expect("Failed to create temp dir");
        let cert_path = dir.path().join("test.crt");
        let installer = CertInstaller::new(&cert_path);
        assert_eq!(installer.cert_path, cert_path);
        assert_eq!(installer.cert_name, "Bifrost CA");
    }

    #[test]
    fn test_get_install_instructions() {
        let dir = tempdir().expect("Failed to create temp dir");
        let cert_path = dir.path().join("test.crt");
        let installer = CertInstaller::new(&cert_path);
        let instructions = installer.get_install_instructions();
        assert!(!instructions.is_empty());
    }

    #[test]
    fn test_get_platform_name() {
        let name = get_platform_name();
        assert!(!name.is_empty());
        #[cfg(target_os = "macos")]
        assert_eq!(name, "macOS");
        #[cfg(target_os = "linux")]
        assert_eq!(name, "Linux");
        #[cfg(target_os = "windows")]
        assert_eq!(name, "Windows");
    }

    #[test]
    fn test_is_macos_delete_certificate_not_found() {
        assert!(is_macos_delete_certificate_not_found(
            "Unable to delete certificate matching \"Bifrost CA\""
        ));
        assert!(is_macos_delete_certificate_not_found(
            "The specified item could not be found in the keychain."
        ));
        assert!(!is_macos_delete_certificate_not_found("some other failure"));
    }

    #[test]
    fn test_is_macos_user_cancelled() {
        assert!(is_macos_user_cancelled("User canceled."));
        assert!(is_macos_user_cancelled("errAuthorizationCanceled"));
        assert!(!is_macos_user_cancelled("authorization denied"));
    }

    #[test]
    fn test_parse_security_sha256_fingerprint() {
        assert_eq!(
            parse_security_sha256_fingerprint("SHA-256 hash: ABCDEF1234"),
            Some("ABCDEF1234".to_string())
        );
        assert_eq!(
            parse_security_sha256_fingerprint("sha-256 hash: ab:cd ef"),
            Some("ABCDEF".to_string())
        );
    }

    #[test]
    fn test_macos_authorization_fallback_quoting_helpers() {
        assert_eq!(
            shell_quote("/tmp/Bifrost CA's cert.crt"),
            "'/tmp/Bifrost CA'\\''s cert.crt'"
        );
        assert_eq!(
            applescript_string_literal("/usr/bin/security \"quoted\" \\\\ path"),
            "\"/usr/bin/security \\\"quoted\\\" \\\\\\\\ path\""
        );
    }

    #[test]
    fn test_parse_openssl_sha256_fingerprint() {
        assert_eq!(
            parse_openssl_sha256_fingerprint("SHA256 Fingerprint=AA:BB:CC:DD:EE:FF\n"),
            Some("AABBCCDDEEFF".to_string())
        );
        assert_eq!(
            parse_openssl_sha256_fingerprint("sha256 Fingerprint = aa bb cc dd\n"),
            Some("AABBCCDD".to_string())
        );
        assert_eq!(
            parse_openssl_sha256_fingerprint(
                "subject=CN = Test Cert\nSHA256 Fingerprint=01:23:45:67\n"
            ),
            Some("01234567".to_string())
        );
    }

    #[test]
    fn test_normalize_thumbprint() {
        assert_eq!(normalize_thumbprint("aa bb:cc-dd"), "AABBCCDD".to_string());
    }

    #[test]
    fn test_parse_windows_certutil_thumbprint() {
        assert_eq!(
            parse_windows_certutil_thumbprint(
                "  Cert Hash(sha1): f7 c6 cb 00 d3 7d ee a9 9e 42 6f 16 a7 e4 13 c3 93 47 75 06  "
            ),
            Some("F7C6CB00D37DEEA99E426F16A7E413C393477506".to_string())
        );
        assert_eq!(
            parse_windows_certutil_thumbprint(
                "  证书哈希(sha1): f7 c6 cb 00 d3 7d ee a9 9e 42 6f 16 a7 e4 13 c3 93 47 75 06  "
            ),
            Some("F7C6CB00D37DEEA99E426F16A7E413C393477506".to_string())
        );
        let gbk_prefix: &[u8] = b"  \xd6\xa4\xca\xe9\xb9\xfe\xcf\xa3(sha1): f7 c6 cb 00 d3 7d ee a9 9e 42 6f 16 a7 e4 13 c3 93 47 75 06  ";
        let lossy_line = String::from_utf8_lossy(gbk_prefix);
        assert_eq!(
            parse_windows_certutil_thumbprint(&lossy_line),
            Some("F7C6CB00D37DEEA99E426F16A7E413C393477506".to_string())
        );
        assert_eq!(parse_windows_certutil_thumbprint("  Other line  "), None);
        assert_eq!(
            parse_windows_certutil_thumbprint("  Cert Hash(sha1): aa bb  "),
            None
        );
    }
}
