use super::*;

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowsMsiScope {
    PerUser,
    Machine,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WindowsMsiRegistration {
    pub(super) product_code: String,
    pub(super) scope: WindowsMsiScope,
}

#[cfg(target_os = "windows")]
pub(super) fn run_windows_installer(
    package: &Path,
    args: &[&str],
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    let mut command = Command::new(package);
    command.args(args);
    let status = run_desktop_install_command(command, target_version, progress_source)?;
    if status.success() {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "desktop installer exited with status {status}"
        )))
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn run_windows_installer(
    _package: &Path,
    _args: &[&str],
    _target_version: &str,
    _progress_source: &str,
) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "Windows desktop packages can only be installed on Windows".to_string(),
    ))
}

#[cfg(target_os = "windows")]
pub(super) fn run_windows_msi(
    package: &Path,
    install_dir: &Path,
    target_version: &str,
    progress_source: &str,
) -> Result<(), BifrostError> {
    let log_path = windows_msi_log_path(package);
    let scope = find_windows_msi_registration_for_install_dir(install_dir)
        .map(|registration| registration.scope)
        .unwrap_or(WindowsMsiScope::PerUser);
    let args = windows_msi_install_args(package, install_dir, &log_path, scope);
    let command = windows_msi_command(&args, scope);
    let status = run_desktop_install_command(command, target_version, progress_source)?;
    if status.success() {
        let _ = fs::remove_file(&log_path);
        Ok(())
    } else {
        let log_summary = read_windows_msi_log_summary(&log_path);
        Err(BifrostError::Config(format!(
            "msiexec exited with status {status}; log: {}{}",
            log_path.display(),
            log_summary
                .map(|summary| format!("; {summary}"))
                .unwrap_or_default()
        )))
    }
}

#[cfg(target_os = "windows")]
pub(super) fn run_windows_msi_uninstall(
    product_code: &str,
    scope: WindowsMsiScope,
) -> Result<(), BifrostError> {
    let log_path = env::temp_dir().join(format!(
        "bifrost-desktop-msi-uninstall-{}-{}.log",
        std::process::id(),
        product_code.trim_matches(|ch| ch == '{' || ch == '}')
    ));
    let args = windows_msi_uninstall_args(product_code, &log_path, scope);
    let status = windows_msi_command(&args, scope)
        .stdin(Stdio::null())
        .status()
        .map_err(BifrostError::Io)?;
    if status.success() {
        let _ = fs::remove_file(&log_path);
        Ok(())
    } else {
        let log_summary = read_windows_msi_log_summary(&log_path);
        Err(BifrostError::Config(format!(
            "msiexec uninstall exited with status {status}; log: {}{}",
            log_path.display(),
            log_summary
                .map(|summary| format!("; {summary}"))
                .unwrap_or_default()
        )))
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn run_windows_msi(
    _package: &Path,
    _install_dir: &Path,
    _target_version: &str,
    _progress_source: &str,
) -> Result<(), BifrostError> {
    Err(BifrostError::Config(
        "MSI desktop packages can only be installed on Windows".to_string(),
    ))
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_msi_install_args(
    package: &Path,
    install_dir: &Path,
    log_path: &Path,
    scope: WindowsMsiScope,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("/i"),
        package.as_os_str().to_os_string(),
        OsString::from("/qn"),
        OsString::from("/norestart"),
    ];
    match scope {
        WindowsMsiScope::PerUser => {
            // Tauri's WiX bundle sets ALLUSERS=1 by default. New installs made
            // from the CLI live in LocalAppData and must not request elevation.
            args.push(OsString::from("ALLUSERS=2"));
            args.push(OsString::from("MSIINSTALLPERUSER=1"));
        }
        WindowsMsiScope::Machine => {
            // Preserve an existing machine-wide install instead of silently
            // creating a second per-user registration and failing on HKLM.
            args.push(OsString::from("ALLUSERS=1"));
        }
    }
    // A machine-wide upgrade is launched from the interactive administrator's
    // token so UAC stays visible. WiX may otherwise resolve INSTALLDIR back to
    // that user's LocalAppData even while ALLUSERS=1 writes an HKLM product
    // registration. Pin the already-discovered install directory so scope and
    // files remain together for both machine-wide and per-user upgrades.
    let mut install_dir_property = OsString::from("INSTALLDIR=");
    install_dir_property.push(install_dir.as_os_str());
    args.push(install_dir_property);
    args.extend([OsString::from("/l*v"), log_path.as_os_str().to_os_string()]);
    args
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_msi_uninstall_args(
    product_code: &str,
    log_path: &Path,
    scope: WindowsMsiScope,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("/x"),
        OsString::from(product_code),
        OsString::from("/qn"),
        OsString::from("/norestart"),
    ];
    match scope {
        WindowsMsiScope::PerUser => {
            args.push(OsString::from("ALLUSERS=2"));
            args.push(OsString::from("MSIINSTALLPERUSER=1"));
        }
        WindowsMsiScope::Machine => args.push(OsString::from("ALLUSERS=1")),
    }
    args.extend([OsString::from("/l*v"), log_path.as_os_str().to_os_string()]);
    args
}

#[cfg(target_os = "windows")]
fn windows_msi_command(args: &[OsString], scope: WindowsMsiScope) -> Command {
    if scope == WindowsMsiScope::PerUser {
        let mut command = Command::new("msiexec");
        command.args(args);
        return command;
    }

    // ShellExecute's `runas` verb is most reliably exposed through
    // Start-Process. Keep the UAC prompt attached to the interactive user,
    // wait for the elevated msiexec, and propagate its real exit code.
    let script = windows_msi_elevation_script(args);
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg(script);
    command
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_msi_elevation_script(args: &[OsString]) -> String {
    let argument_list = windows_msi_powershell_argument_list(args);
    format!(
        "$ErrorActionPreference='Stop'; $p=Start-Process -FilePath 'msiexec.exe' -Verb RunAs -ArgumentList {} -Wait -PassThru; exit $p.ExitCode",
        argument_list
    )
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_msi_powershell_argument_list(args: &[OsString]) -> String {
    // Start-Process joins ArgumentList elements before launching msiexec.
    // Preserve literal quotes inside each path/property element: msiexec's
    // parser does not treat one pre-rendered command-line string equivalently.
    let args = args
        .iter()
        .map(|arg| {
            let value = arg.to_string_lossy();
            powershell_single_quoted(&windows_msi_start_process_arg(&value))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("@({args})")
}

#[cfg(any(target_os = "windows", test))]
fn windows_msi_start_process_arg(value: &str) -> String {
    if let Some((name, property_value)) = value.split_once('=') {
        let is_msi_property = !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_');
        if is_msi_property
            && property_value
                .chars()
                .any(|ch| matches!(ch, ' ' | '\t' | '"'))
        {
            return format!("{name}={}", windows_quote_command_line_arg(property_value));
        }
    }
    windows_quote_command_line_arg(value)
}

#[cfg(any(target_os = "windows", test))]
fn windows_quote_command_line_arg(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|ch| matches!(ch, ' ' | '\t' | '"')) {
        return value.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_msi_log_path(package: &Path) -> PathBuf {
    let package_name = package
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("desktop")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    env::temp_dir().join(format!(
        "bifrost-desktop-msi-{}-{package_name}.log",
        std::process::id()
    ))
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn read_windows_msi_log_summary(log_path: &Path) -> Option<String> {
    let contents = fs::read_to_string(log_path).ok()?;
    let interesting = contents
        .lines()
        .rev()
        .find(|line| {
            line.contains("Error ")
                || line.contains("Return value 3")
                || line.contains("Installation failed")
                || line.contains("Product: Bifrost")
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())?;
    Some(format!("MSI detail: {interesting}"))
}

#[cfg(target_os = "windows")]
pub(super) fn find_windows_msi_registration_for_install_dir(
    install_dir: &Path,
) -> Option<WindowsMsiRegistration> {
    const UNINSTALL_HIVES: [(&str, WindowsMsiScope); 2] = [
        (
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall",
            WindowsMsiScope::PerUser,
        ),
        (
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall",
            WindowsMsiScope::Machine,
        ),
    ];

    let expected_install_dir = normalize_windows_path_for_compare(install_dir);
    for (hive, scope) in UNINSTALL_HIVES {
        let output = Command::new("reg")
            .args(["query", hive, "/s"])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(product_code) =
            parse_windows_msi_product_code_for_install_dir(&stdout, &expected_install_dir)
        {
            return Some(WindowsMsiRegistration {
                product_code,
                scope,
            });
        }
    }
    None
}

#[cfg(all(not(target_os = "windows"), test))]
pub(super) fn find_windows_msi_registration_for_install_dir(
    _install_dir: &Path,
) -> Option<WindowsMsiRegistration> {
    None
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn parse_windows_msi_product_code_for_install_dir(
    reg_output: &str,
    expected_install_dir: &str,
) -> Option<String> {
    let mut display_name = None;
    let mut uninstall_string = None;
    let mut install_location = None;

    for line in reg_output.lines().chain(std::iter::once("")) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("HKEY_") {
            if display_name.as_deref() == Some(WINDOWS_APP_NAME)
                && install_location
                    .as_deref()
                    .map(normalize_windows_path_for_compare_str)
                    .as_deref()
                    == Some(expected_install_dir)
            {
                if let Some(product_code) = uninstall_string
                    .as_deref()
                    .and_then(extract_msi_product_code)
                {
                    return Some(product_code);
                }
            }
            display_name = None;
            uninstall_string = None;
            install_location = None;
            continue;
        }

        if let Some(value) = parse_reg_value(trimmed, "DisplayName") {
            display_name = Some(value.to_string());
        } else if let Some(value) = parse_reg_value(trimmed, "UninstallString") {
            uninstall_string = Some(value.to_string());
        } else if let Some(value) = parse_reg_value(trimmed, "InstallLocation") {
            install_location = Some(value.to_string());
        }
    }

    None
}

#[cfg(any(target_os = "windows", test))]
fn parse_reg_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest
        .strip_prefix("REG_EXPAND_SZ")
        .or_else(|| rest.strip_prefix("REG_SZ"))?
        .trim_start();
    Some(rest.trim())
}

#[cfg(any(target_os = "windows", test))]
fn extract_msi_product_code(uninstall_string: &str) -> Option<String> {
    let start = uninstall_string.find('{')?;
    let end = uninstall_string[start..].find('}')? + start;
    let product_code = &uninstall_string[start..=end];
    if product_code.len() == 38 {
        Some(product_code.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod argument_line_tests {
    use super::*;

    #[test]
    fn msi_elevation_script_uses_literal_quotes_in_a_powershell_argument_array() {
        let args = vec![
            OsString::from("/i"),
            OsString::from(r"C:\Program Files\Bifrost\bifrost desktop.msi"),
            OsString::from("/qn"),
            OsString::from("/norestart"),
            OsString::from("ALLUSERS=1"),
            OsString::from(r"INSTALLDIR=C:\Program Files\Bifrost"),
            OsString::from("/l*v"),
            OsString::from(r"C:\Temp Files\bifrost msi.log"),
        ];

        let argument_list = windows_msi_powershell_argument_list(&args);
        assert_eq!(
            argument_list,
            r#"@('/i', '"C:\Program Files\Bifrost\bifrost desktop.msi"', '/qn', '/norestart', 'ALLUSERS=1', 'INSTALLDIR="C:\Program Files\Bifrost"', '/l*v', '"C:\Temp Files\bifrost msi.log"')"#
        );
        let script = windows_msi_elevation_script(&args);
        assert!(script.contains(&format!("-ArgumentList {argument_list}")));
        assert!(!script.contains("-ArgumentList '/i "));
    }

    #[test]
    fn windows_argument_quoting_handles_empty_quotes_and_trailing_backslashes() {
        assert_eq!(windows_quote_command_line_arg(""), r#""""#);
        assert_eq!(windows_quote_command_line_arg("plain"), "plain");
        assert_eq!(windows_quote_command_line_arg(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(
            windows_quote_command_line_arg(r#"C:\Program Files\Bifrost\"#),
            r#""C:\Program Files\Bifrost\\""#
        );
    }
}
