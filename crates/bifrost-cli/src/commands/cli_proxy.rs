use std::path::{Path, PathBuf};

use bifrost_core::{
    create_combined_ca_bundle, BifrostError, CliProxyEnvironmentConfig, CliProxyEnvironmentManager,
    CliProxyEnvironmentResult, CliProxyShell,
};

use crate::cli::{CliProxyCommands, CliProxyShellArg};
use crate::commands::ca::ensure_ca_exists;
use crate::config::get_bifrost_dir;

const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1";

pub fn handle_cli_proxy_command(
    action: CliProxyCommands,
    effective_port: u16,
) -> bifrost_core::Result<()> {
    match action {
        CliProxyCommands::Enable {
            host,
            port,
            no_proxy,
            shell,
            ca_file,
            ca_dir,
        } => {
            let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
            let port = port.unwrap_or(effective_port);
            let no_proxy = no_proxy.unwrap_or_else(|| DEFAULT_NO_PROXY.to_string());
            let result = enable_cli_proxy_environment(
                shell,
                &host,
                port,
                &no_proxy,
                ca_file.clone(),
                ca_dir.clone(),
            );
            if let Err(error) = &result {
                print_manual_enable_fallback(shell, &host, port, &no_proxy, ca_file, ca_dir, error);
            }
            result
        }
        CliProxyCommands::Disable { shell } => {
            let result = disable_cli_proxy_environment(shell);
            if let Err(error) = &result {
                print_manual_disable_fallback(shell, error);
            }
            result
        }
    }
}

fn enable_cli_proxy_environment(
    shell: Option<CliProxyShellArg>,
    host: &str,
    port: u16,
    no_proxy: &str,
    ca_file: Option<PathBuf>,
    ca_dir: Option<PathBuf>,
) -> bifrost_core::Result<()> {
    let shell = resolve_shell(shell)?;
    let data_dir = get_bifrost_dir()?;
    let cert_dir = data_dir.join("certs");
    let using_default_ca = ca_file.is_none();
    let ca_file = ca_file.unwrap_or_else(|| cert_dir.join("ca.crt"));

    if using_default_ca {
        ensure_ca_exists(&ca_file, &cert_dir.join("ca.key"))?;
    }
    let ca_file = canonical_file(&ca_file, "CA file")?;
    let ca_dir = match ca_dir {
        Some(path) => canonical_directory(&path, "CA directory")?,
        None => ca_file
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| BifrostError::Config("CA file has no parent directory".into()))?,
    };

    let ca_bundle = cert_dir.join("cli-proxy-ca-bundle.pem");
    let native_root_count = create_combined_ca_bundle(&ca_file, &ca_bundle)?;
    let ca_bundle = canonical_file(&ca_bundle, "Combined CA bundle")?;
    let config = CliProxyEnvironmentConfig {
        proxy_url: format_proxy_url(host, port)?,
        no_proxy: no_proxy.to_string(),
        ca_file,
        ca_bundle,
        ca_dir,
    };
    let manager = CliProxyEnvironmentManager::new(shell)?;
    let result = manager.enable(&config)?;

    println!("✓ CLI proxy environment enabled for {}", shell.as_str());
    println!("  Proxy: {}", config.proxy_url);
    println!("  No proxy: {}", config.no_proxy);
    println!("  Bifrost CA: {}", config.ca_file.display());
    println!(
        "  Combined CA bundle: {} ({} system roots + Bifrost CA)",
        config.ca_bundle.display(),
        native_root_count
    );
    print_profile_result(&result);
    println!("  Open a new shell or reload the profile above to apply it.");
    Ok(())
}

fn disable_cli_proxy_environment(shell: Option<CliProxyShellArg>) -> bifrost_core::Result<()> {
    let shell = resolve_shell(shell)?;
    let manager = CliProxyEnvironmentManager::new(shell)?;
    let result = manager.disable()?;

    if result.changed_paths.is_empty() {
        println!(
            "✓ CLI proxy environment already disabled for {}",
            shell.as_str()
        );
    } else {
        println!("✓ CLI proxy environment disabled for {}", shell.as_str());
        print_profile_result(&result);
        println!("  Open a new shell or reload the profile above to apply it.");
    }
    Ok(())
}

fn resolve_shell(shell: Option<CliProxyShellArg>) -> bifrost_core::Result<CliProxyShell> {
    match shell {
        Some(CliProxyShellArg::Bash) => Ok(CliProxyShell::Bash),
        Some(CliProxyShellArg::Zsh) => Ok(CliProxyShell::Zsh),
        Some(CliProxyShellArg::Fish) => Ok(CliProxyShell::Fish),
        Some(CliProxyShellArg::PowerShell) => Ok(CliProxyShell::PowerShell),
        None => detect_parent_process_shell()?.map_or_else(CliProxyShell::detect, Ok),
    }
}

fn detect_parent_process_shell() -> bifrost_core::Result<Option<CliProxyShell>> {
    let system = sysinfo::System::new_all();
    let mut pid = sysinfo::get_current_pid()
        .ok()
        .and_then(|pid| system.process(pid))
        .and_then(|process| process.parent());

    // The direct parent is normally the current interactive shell. Walk a few wrapper levels as
    // well so `env bifrost ...`, `cargo run ...`, and similar launchers still reach that shell.
    for _ in 0..8 {
        let Some(current_pid) = pid else {
            return Ok(None);
        };
        let Some(process) = system.process(current_pid) else {
            return Ok(None);
        };
        let process_name = process.name().to_string_lossy();
        if let Some(shell) = shell_from_process_name(&process_name) {
            return Ok(Some(shell));
        }
        if is_known_unsupported_shell(&process_name) {
            return Err(BifrostError::Config(format!(
                "Current shell {process_name:?} is not supported for automatic profile editing. Use --shell bash|zsh|fish|powershell if one of those profiles is appropriate"
            )));
        }
        pid = process.parent();
    }
    Ok(None)
}

fn shell_from_process_name(name: &str) -> Option<CliProxyShell> {
    let file_name = normalized_process_file_name(name);
    match file_name.as_str() {
        "bash" => Some(CliProxyShell::Bash),
        "zsh" => Some(CliProxyShell::Zsh),
        "fish" => Some(CliProxyShell::Fish),
        "powershell" | "pwsh" => Some(CliProxyShell::PowerShell),
        _ => None,
    }
}

fn is_known_unsupported_shell(name: &str) -> bool {
    matches!(
        normalized_process_file_name(name).as_str(),
        "sh" | "dash" | "ksh" | "csh" | "tcsh" | "nu" | "xonsh" | "elvish"
    )
}

fn normalized_process_file_name(name: &str) -> String {
    let normalized = name.replace('\\', "/");
    Path::new(&normalized)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name)
        .trim_end_matches(".exe")
        .to_ascii_lowercase()
}

fn format_proxy_url(host: &str, port: u16) -> bifrost_core::Result<String> {
    let host = host.trim();
    if port == 0
        || host.is_empty()
        || host.contains("//")
        || host.chars().any(|character| character.is_control())
    {
        return Err(BifrostError::Config(format!(
            "Invalid proxy host: {host:?}"
        )));
    }
    let host = if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let proxy_url = format!("http://{host}:{port}");
    let parsed = url::Url::parse(&proxy_url).map_err(|_| {
        BifrostError::Config(format!("Invalid proxy host or port: {host:?}:{port}"))
    })?;
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(BifrostError::Config(format!(
            "Invalid proxy host or port: {host:?}:{port}"
        )));
    }
    Ok(proxy_url)
}

fn canonical_file(path: &Path, label: &str) -> bifrost_core::Result<PathBuf> {
    if !path.is_file() {
        return Err(BifrostError::Config(format!(
            "{label} does not exist or is not a file: {}",
            path.display()
        )));
    }
    std::fs::canonicalize(path).map_err(Into::into)
}

fn canonical_directory(path: &Path, label: &str) -> bifrost_core::Result<PathBuf> {
    if !path.is_dir() {
        return Err(BifrostError::Config(format!(
            "{label} does not exist or is not a directory: {}",
            path.display()
        )));
    }
    std::fs::canonicalize(path).map_err(Into::into)
}

fn print_profile_result(result: &CliProxyEnvironmentResult) {
    if result.changed_paths.is_empty() {
        println!("  Profiles: already up to date");
    } else {
        println!("  Profiles:");
        for path in &result.changed_paths {
            println!("    - {}", path.display());
        }
    }
}

fn print_manual_enable_fallback(
    requested_shell: Option<CliProxyShellArg>,
    host: &str,
    port: u16,
    no_proxy: &str,
    ca_file: Option<PathBuf>,
    ca_dir: Option<PathBuf>,
    error: &BifrostError,
) {
    let data_dir = get_bifrost_dir().unwrap_or_else(|_| PathBuf::from("~/.bifrost"));
    let cert_dir = data_dir.join("certs");
    let ca_file = ca_file.unwrap_or_else(|| cert_dir.join("ca.crt"));
    let intended_bundle = cert_dir.join("cli-proxy-ca-bundle.pem");
    let ca_bundle = intended_bundle.clone();
    let ca_dir =
        ca_dir.unwrap_or_else(|| ca_file.parent().map(Path::to_path_buf).unwrap_or(cert_dir));
    let proxy_url = match format_proxy_url(host, port) {
        Ok(proxy_url) => proxy_url,
        Err(host_error) => {
            eprintln!();
            eprintln!("Automatic CLI proxy environment installation failed: {error}");
            eprintln!("Manual setup cannot be generated safely: {host_error}");
            eprintln!("Fix --host/--port, then retry the command.");
            return;
        }
    };
    let config = CliProxyEnvironmentConfig {
        proxy_url,
        no_proxy: no_proxy.to_string(),
        ca_file: ca_file.clone(),
        ca_bundle: ca_bundle.clone(),
        ca_dir,
    };

    eprintln!();
    eprintln!("Automatic CLI proxy environment installation failed: {error}");
    eprintln!("Manual setup (copy the block for your shell into every profile listed):");
    if !ca_file.is_file() {
        eprintln!("  1. Generate the CA first: bifrost ca generate");
    }
    if !intended_bundle.is_file() {
        eprintln!(
            "  Warning: do not load the block until {} exists. Retry this command after fixing the error so Bifrost can create the combined system+Bifrost CA bundle without replacing system trust.",
            intended_bundle.display()
        );
    }

    for shell in manual_shell_candidates(requested_shell) {
        match CliProxyEnvironmentManager::new(shell) {
            Ok(manager) => {
                eprintln!();
                eprintln!("  {} profiles:", shell.as_str());
                for path in shell.config_paths().unwrap_or_default() {
                    eprintln!("    - {}", path.display());
                }
                eprintln!("  ----- copy below -----");
                match manager.manual_enable_block(&config) {
                    Ok(block) => {
                        eprintln!("{block}");
                        eprintln!("  ----- copy above -----");
                    }
                    Err(render_error) => {
                        eprintln!("  Cannot render a safe block: {render_error}");
                        eprintln!("  Fix the invalid command option or path, then retry.");
                    }
                }
            }
            Err(path_error) => {
                eprintln!(
                    "  {}: could not determine profile paths: {path_error}",
                    shell.as_str()
                );
            }
        }
    }
    eprintln!("  Open a new shell or reload the edited profile afterward.");
}

fn print_manual_disable_fallback(requested_shell: Option<CliProxyShellArg>, error: &BifrostError) {
    let (start_marker, end_marker) = CliProxyEnvironmentManager::environment_markers();
    eprintln!();
    eprintln!("Automatic CLI proxy environment removal failed: {error}");
    eprintln!("Manual removal:");
    eprintln!("  Delete the complete block, including both marker lines:");
    eprintln!("    START: {start_marker}");
    eprintln!("    END:   {end_marker}");
    eprintln!("  Check these profiles:");
    for shell in manual_shell_candidates(requested_shell) {
        match shell.config_paths() {
            Ok(paths) => {
                eprintln!("    {}:", shell.as_str());
                for path in paths {
                    eprintln!("      - {}", path.display());
                }
            }
            Err(path_error) => {
                eprintln!("    {}: {path_error}", shell.as_str());
            }
        }
    }
    eprintln!("  Save the files, then open a new shell or reload the edited profile.");
}

fn manual_shell_candidates(requested_shell: Option<CliProxyShellArg>) -> Vec<CliProxyShell> {
    if let Ok(shell) = resolve_shell(requested_shell) {
        return vec![shell];
    }
    vec![
        CliProxyShell::Bash,
        CliProxyShell::Zsh,
        CliProxyShell::Fish,
        CliProxyShell::PowerShell,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_url_formats_ipv4_hostname_and_ipv6() {
        assert_eq!(
            format_proxy_url("127.0.0.1", 9900).unwrap(),
            "http://127.0.0.1:9900"
        );
        assert_eq!(
            format_proxy_url("localhost", 8811).unwrap(),
            "http://localhost:8811"
        );
        assert_eq!(format_proxy_url("::1", 9900).unwrap(), "http://[::1]:9900");
    }

    #[test]
    fn proxy_url_rejects_scheme_and_control_characters() {
        assert!(format_proxy_url("http://127.0.0.1", 9900).is_err());
        assert!(format_proxy_url("bad\nhost", 9900).is_err());
        assert!(format_proxy_url("bad host", 9900).is_err());
        assert!(format_proxy_url("user@example.com", 9900).is_err());
        assert!(format_proxy_url("example.com/path", 9900).is_err());
        assert!(format_proxy_url("127.0.0.1", 0).is_err());
    }

    #[test]
    fn process_name_detection_accepts_only_supported_shell_executables() {
        assert_eq!(
            shell_from_process_name("/bin/bash"),
            Some(CliProxyShell::Bash)
        );
        assert_eq!(shell_from_process_name("zsh"), Some(CliProxyShell::Zsh));
        assert_eq!(shell_from_process_name("fish"), Some(CliProxyShell::Fish));
        assert_eq!(
            shell_from_process_name(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Some(CliProxyShell::PowerShell)
        );
        assert_eq!(shell_from_process_name("bash-helper"), None);
        assert!(is_known_unsupported_shell("/usr/bin/nu"));
        assert!(is_known_unsupported_shell(r"C:\tools\dash.exe"));
        assert!(!is_known_unsupported_shell("cargo"));
    }
}
