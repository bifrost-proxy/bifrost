use super::*;

const NODE_INSTALL_SOURCE_ENV: &str = "BIFROST_CLI_INSTALL_SOURCE";
const SCRIPT_INSTALL_SOURCE_MARKER_SUFFIX: &str = ".install-source";
const NODE_PACKAGE_NAME: &str = "@bifrost-proxy/bifrost";
const NODE_PACKAGE_MANAGER_TIMEOUT_SECS: u64 = 600;
const NODE_PACKAGE_METADATA_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum InstallMethod {
    Homebrew,
    Npm,
    Pnpm,
    Script,
    Manual(PathBuf),
    Unknown,
}

impl InstallMethod {
    pub(super) fn is_node_package_manager(&self) -> bool {
        matches!(self, Self::Npm | Self::Pnpm)
    }

    fn program_for_platform(&self, windows: bool) -> Option<&'static str> {
        match self {
            Self::Npm if windows => Some("npm.cmd"),
            Self::Pnpm if windows => Some("pnpm.cmd"),
            Self::Npm => Some("npm"),
            Self::Pnpm => Some("pnpm"),
            _ => None,
        }
    }

    fn program(&self) -> Option<&'static str> {
        self.program_for_platform(cfg!(windows))
    }
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Homebrew => write!(f, "Homebrew"),
            Self::Npm => write!(f, "npm"),
            Self::Pnpm => write!(f, "pnpm"),
            Self::Script => write!(f, "Install script"),
            Self::Manual(path) => write!(f, "Manual ({})", path.display()),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

pub(super) fn detect_install_method() -> InstallMethod {
    if upgrade_test_overrides_enabled() {
        if let Some(path) = env::var_os(UPGRADE_TEST_INSTALL_TARGET_ENV) {
            return InstallMethod::Manual(PathBuf::from(path));
        }
    }

    let exe_path = env::current_exe().ok();
    let source_marker = exe_path
        .as_deref()
        .and_then(read_script_install_source_marker);
    detect_install_method_from(
        exe_path.as_deref(),
        env::var(NODE_INSTALL_SOURCE_ENV).ok().as_deref(),
        source_marker.as_deref(),
    )
}

fn detect_install_method_from(
    exe_path: Option<&Path>,
    source_hint: Option<&str>,
    source_marker: Option<&str>,
) -> InstallMethod {
    let Some(exe_path) = exe_path else {
        return InstallMethod::Unknown;
    };
    let normalized = normalized_path(exe_path);

    // A Homebrew-installed Node commonly places npm global packages below
    // /opt/homebrew. The package structure is more specific than that prefix,
    // so it must win before Homebrew formula detection.
    if is_node_distribution_path(&normalized) {
        return match source_hint.map(str::trim) {
            Some("pnpm") => InstallMethod::Pnpm,
            Some("npm") => InstallMethod::Npm,
            _ if looks_like_pnpm_path(&normalized) => InstallMethod::Pnpm,
            _ => InstallMethod::Npm,
        };
    }

    if is_homebrew_path(&normalized) {
        return InstallMethod::Homebrew;
    }

    if source_marker == Some("script")
        || normalized.contains("/.bifrost/bin/")
        || normalized.ends_with("/.local/bin/bifrost")
        || normalized.ends_with("/.local/bin/bifrost.exe")
    {
        return InstallMethod::Script;
    }

    InstallMethod::Manual(exe_path.to_path_buf())
}

fn script_install_source_marker_path(executable: &Path) -> PathBuf {
    let marker_name = format!(
        "{}{}",
        executable
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bifrost"),
        SCRIPT_INSTALL_SOURCE_MARKER_SUFFIX
    );
    executable.with_file_name(marker_name)
}

fn read_script_install_source_marker(executable: &Path) -> Option<String> {
    fs::read_to_string(script_install_source_marker_path(executable))
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn is_homebrew_path(path: &str) -> bool {
    path.contains("/cellar/bifrost/")
}

fn is_node_distribution_path(path: &str) -> bool {
    let package_scope = "/node_modules/@bifrost-proxy/";
    path.contains(package_scope)
        && (path.contains("/bifrost-")
            || path.contains("/bifrost/downloaded-bifrost-proxy-bifrost-"))
}

fn looks_like_pnpm_path(path: &str) -> bool {
    path.contains("/.pnpm/")
        || path.contains("/pnpm/global/")
        || path.contains("/library/pnpm/")
        || path.contains("/appdata/local/pnpm/")
}

pub(super) fn restart_executable_for_install_method(
    method: &InstallMethod,
) -> Result<PathBuf, BifrostError> {
    match method {
        InstallMethod::Homebrew => env::current_exe()
            .map(|current| homebrew_launcher_for_executable(&current))
            .map_err(BifrostError::Io),
        InstallMethod::Npm | InstallMethod::Pnpm => resolve_node_managed_binary(method),
        InstallMethod::Script => env::current_exe().map_err(BifrostError::Io),
        InstallMethod::Manual(path) => Ok(path.clone()),
        InstallMethod::Unknown => Err(BifrostError::Config(
            "Cannot determine restart executable for unknown install method".to_string(),
        )),
    }
}

pub(super) fn homebrew_launcher_for_executable(current: &Path) -> PathBuf {
    let current_text = normalized_path(current);
    if current_text.starts_with("/opt/homebrew/") {
        PathBuf::from("/opt/homebrew/bin/bifrost")
    } else if current_text.starts_with("/usr/local/") {
        PathBuf::from("/usr/local/bin/bifrost")
    } else if current_text.starts_with("/home/linuxbrew/") {
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/bifrost")
    } else {
        PathBuf::from("bifrost")
    }
}

fn normalized_node_package_version(target_version: &str) -> Result<&str, BifrostError> {
    let version = target_version.trim().trim_start_matches('v');
    if version.is_empty()
        || version.len() > 128
        || !version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character))
    {
        return Err(BifrostError::Config(format!(
            "refusing unsafe npm/pnpm target version: {target_version}"
        )));
    }
    Ok(version)
}

fn node_package_manager_upgrade_args(
    method: &InstallMethod,
    target_version: &str,
) -> Result<Vec<String>, BifrostError> {
    let package = format!(
        "{NODE_PACKAGE_NAME}@{}",
        normalized_node_package_version(target_version)?
    );
    let args = match method {
        InstallMethod::Npm => vec![
            "install".to_string(),
            "--global".to_string(),
            "--no-audit".to_string(),
            "--progress=false".to_string(),
            package,
        ],
        InstallMethod::Pnpm => vec!["add".to_string(), "--global".to_string(), package],
        _ => Vec::new(),
    };
    Ok(args)
}

pub(super) fn upgrade_via_node_package_manager(
    method: &InstallMethod,
    target_version: &str,
) -> Result<(), BifrostError> {
    let program = method.program().ok_or_else(|| {
        BifrostError::Config("node package-manager upgrade requires npm or pnpm".to_string())
    })?;
    let args = node_package_manager_upgrade_args(method, target_version)?;
    println!("{}", format!("Upgrading via {}...", method).bright_cyan());
    let output = command_output_with_timeout_and_env_streaming(
        Path::new(program),
        &args,
        Duration::from_secs(NODE_PACKAGE_MANAGER_TIMEOUT_SECS),
        Duration::from_secs(UPGRADE_CHILD_PROGRESS_HEARTBEAT_SECS),
        &[],
        None,
        None,
    )
    .map_err(|error| {
        BifrostError::Config(format!(
            "could not run {} upgrade command (`{} {}`): {}",
            method,
            program,
            args.join(" "),
            error
        ))
    })?;
    if output.status != TimedCommandStatus::Success {
        return Err(BifrostError::Config(format!(
            "{} upgrade failed: {}. Retry with: {} {}",
            method,
            summarize_command_output(&output),
            program,
            args.join(" ")
        )));
    }
    println!(
        "{}",
        format!("✓ {} package upgrade completed.", method)
            .bright_green()
            .bold()
    );
    Ok(())
}

fn resolve_node_managed_binary(method: &InstallMethod) -> Result<PathBuf, BifrostError> {
    let program = method.program().ok_or_else(|| {
        BifrostError::Config("node package-manager binary resolution requires npm or pnpm".into())
    })?;
    let root_args = ["root".to_string(), "--global".to_string()];
    let root_output = command_output_with_timeout(
        Path::new(program),
        &root_args,
        Duration::from_secs(NODE_PACKAGE_METADATA_TIMEOUT_SECS),
    )
    .map_err(|error| {
        BifrostError::Config(format!(
            "could not run {} metadata command (`{} {}`): {}",
            method,
            program,
            root_args.join(" "),
            error
        ))
    })?;
    if root_output.status != TimedCommandStatus::Success {
        return Err(BifrostError::Config(format!(
            "could not resolve {} global package root: {}",
            method,
            summarize_command_output(&root_output)
        )));
    }
    let global_root = root_output.stdout.trim();
    if global_root.is_empty() {
        return Err(BifrostError::Config(format!(
            "{} returned an empty global package root",
            method
        )));
    }

    let resolver = concat!(
        "const p=require('path');",
        "const e=p.join(process.argv[1],'@bifrost-proxy','bifrost','lib','index.js');",
        "process.stdout.write(require(e).getBinaryPath());"
    );
    let binary_output = command_output_with_timeout(
        Path::new("node"),
        &[
            "-e".to_string(),
            resolver.to_string(),
            global_root.to_string(),
        ],
        Duration::from_secs(NODE_PACKAGE_METADATA_TIMEOUT_SECS),
    )
    .map_err(|error| {
        BifrostError::Config(format!(
            "could not run Node.js to resolve the {} global Bifrost package: {}",
            method, error
        ))
    })?;
    if binary_output.status != TimedCommandStatus::Success {
        return Err(BifrostError::Config(format!(
            "could not resolve the Bifrost binary from the {} global package: {}",
            method,
            summarize_command_output(&binary_output)
        )));
    }
    let binary = PathBuf::from(binary_output.stdout.trim());
    if !binary.is_file() {
        return Err(BifrostError::Config(format!(
            "{} global package resolved a missing Bifrost binary: {}",
            method,
            binary.display()
        )));
    }
    Ok(binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_all_supported_install_sources_and_preserves_priority() {
        assert_eq!(
            detect_install_method_from(
                Some(Path::new("/opt/homebrew/Cellar/bifrost/1.2.3/bin/bifrost")),
                Some("pnpm"),
                Some("script")
            ),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect_install_method_from(
                Some(Path::new("/opt/homebrew/custom/bin/bifrost")),
                None,
                None
            ),
            InstallMethod::Manual(PathBuf::from("/opt/homebrew/custom/bin/bifrost"))
        );
        assert_eq!(
            detect_install_method_from(
                Some(Path::new(
                    "/usr/local/lib/node_modules/@bifrost-proxy/bifrost-darwin-arm64/bin/bifrost"
                )),
                Some("npm"),
                None
            ),
            InstallMethod::Npm
        );
        assert_eq!(
            detect_install_method_from(
                Some(Path::new(
                    "/opt/homebrew/lib/node_modules/@bifrost-proxy/bifrost-darwin-arm64/bin/bifrost"
                )),
                Some("npm"),
                None
            ),
            InstallMethod::Npm
        );
        assert_eq!(
            detect_install_method_from(
                Some(Path::new(
                    "/Users/test/Library/pnpm/global/5/.pnpm/@bifrost-proxy+bifrost-darwin-arm64@1.2.3/node_modules/@bifrost-proxy/bifrost-darwin-arm64/bin/bifrost"
                )),
                None,
                None
            ),
            InstallMethod::Pnpm
        );
        assert_eq!(
            detect_install_method_from(
                Some(Path::new("/Users/test/.bifrost/bin/bifrost")),
                Some("npm"),
                None
            ),
            InstallMethod::Script
        );
        assert_eq!(
            detect_install_method_from(
                Some(Path::new("/tmp/custom/bifrost")),
                None,
                Some("script")
            ),
            InstallMethod::Script
        );
        assert_eq!(
            detect_install_method_from(Some(Path::new("/tmp/custom/bifrost")), None, None),
            InstallMethod::Manual(PathBuf::from("/tmp/custom/bifrost"))
        );
        assert_eq!(
            detect_install_method_from(None, None, None),
            InstallMethod::Unknown
        );
    }

    #[test]
    fn ignores_spoofed_node_source_hint_outside_node_distribution() {
        assert_eq!(
            detect_install_method_from(Some(Path::new("/tmp/custom/bifrost")), Some("pnpm"), None),
            InstallMethod::Manual(PathBuf::from("/tmp/custom/bifrost"))
        );
    }

    #[test]
    fn reads_only_non_empty_normalized_script_markers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let executable = temp.path().join("bifrost");
        let marker = script_install_source_marker_path(&executable);
        fs::write(&marker, " Script \n").expect("write marker");
        assert_eq!(
            read_script_install_source_marker(&executable).as_deref(),
            Some("script")
        );
        fs::write(marker, "  \n").expect("write empty marker");
        assert_eq!(read_script_install_source_marker(&executable), None);
    }

    #[test]
    fn package_manager_commands_pin_the_requested_version() {
        assert_eq!(
            node_package_manager_upgrade_args(&InstallMethod::Npm, "v1.2.3").unwrap(),
            [
                "install",
                "--global",
                "--no-audit",
                "--progress=false",
                "@bifrost-proxy/bifrost@1.2.3"
            ]
        );
        assert_eq!(
            node_package_manager_upgrade_args(&InstallMethod::Pnpm, "1.2.3").unwrap(),
            ["add", "--global", "@bifrost-proxy/bifrost@1.2.3"]
        );
        assert!(
            node_package_manager_upgrade_args(&InstallMethod::Npm, "1.2.3&whoami")
                .unwrap_err()
                .to_string()
                .contains("refusing unsafe")
        );
        assert_eq!(
            InstallMethod::Npm.program_for_platform(true),
            Some("npm.cmd")
        );
        assert_eq!(
            InstallMethod::Pnpm.program_for_platform(true),
            Some("pnpm.cmd")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolves_node_managed_binary_through_the_selected_global_root() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ENV: &str = "BIFROST_TEST_NODE_MANAGER_RESOLVE_CHILD";
        if env::var(CHILD_ENV).ok().as_deref() != Some("1") {
            let status = Command::new(env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "commands::upgrade::install_method::tests::resolves_node_managed_binary_through_the_selected_global_root",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated resolver test");
            assert!(status.success());
            return;
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("global/node_modules");
        let binary = root.join("@bifrost-proxy/bifrost-platform/bin/bifrost");
        fs::create_dir_all(binary.parent().expect("binary parent")).expect("create binary dir");
        fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("write binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod binary");

        let fake_bin = temp.path().join("bin");
        fs::create_dir_all(&fake_bin).expect("create fake bin");
        let npm = fake_bin.join("npm");
        let node = fake_bin.join("node");
        let command_log = temp.path().join("commands.log");
        fs::write(
            &npm,
            format!(
                "#!/bin/sh\nif [ \"${{1:-}}\" = root ]; then printf '%s\\n' '{}'; exit 0; fi\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                root.display(),
                command_log.display()
            ),
        )
        .expect("write npm");
        fs::write(
            &node,
            format!("#!/bin/sh\nprintf '%s' '{}'\n", binary.display()),
        )
        .expect("write node");
        fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).expect("chmod npm");
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).expect("chmod node");
        env::set_var("PATH", &fake_bin);

        assert_eq!(
            resolve_node_managed_binary(&InstallMethod::Npm).expect("resolve npm binary"),
            binary
        );
        upgrade_via_node_package_manager(&InstallMethod::Npm, "v1.2.3").expect("fake npm upgrade");
        assert_eq!(
            fs::read_to_string(&command_log).expect("read command log"),
            "install --global --no-audit --progress=false @bifrost-proxy/bifrost@1.2.3\n"
        );

        fs::write(&npm, "#!/bin/sh\necho package-manager-failed >&2\nexit 9\n")
            .expect("write failing npm");
        let upgrade_error = upgrade_via_node_package_manager(&InstallMethod::Npm, "1.2.4")
            .expect_err("failed npm upgrade must propagate");
        assert!(upgrade_error.to_string().contains("package-manager-failed"));
        assert!(upgrade_error
            .to_string()
            .contains("npm install --global --no-audit --progress=false"));

        let root_error = resolve_node_managed_binary(&InstallMethod::Npm)
            .expect_err("failed npm root must propagate");
        assert!(root_error
            .to_string()
            .contains("could not resolve npm global package root"));

        fs::write(&npm, "#!/bin/sh\nexit 0\n").expect("write empty-root npm");
        let empty_error =
            resolve_node_managed_binary(&InstallMethod::Npm).expect_err("empty npm root must fail");
        assert!(empty_error
            .to_string()
            .contains("returned an empty global package root"));

        fs::write(
            &npm,
            format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", root.display()),
        )
        .expect("restore npm root");
        fs::write(&node, "#!/bin/sh\necho node-resolver-failed >&2\nexit 8\n")
            .expect("write failing node");
        let node_error = resolve_node_managed_binary(&InstallMethod::Npm)
            .expect_err("failed Node resolver must propagate");
        assert!(node_error.to_string().contains("node-resolver-failed"));

        fs::write(&node, "#!/bin/sh\nprintf '%s' '/missing/bifrost'\n")
            .expect("write missing resolver");
        let missing_error = resolve_node_managed_binary(&InstallMethod::Npm)
            .expect_err("missing resolved binary must fail");
        assert!(missing_error
            .to_string()
            .contains("resolved a missing Bifrost binary"));

        fs::remove_file(&npm).expect("remove fake npm");
        let spawn_upgrade_error = upgrade_via_node_package_manager(&InstallMethod::Npm, "1.2.5")
            .expect_err("missing npm executable must fail");
        assert!(spawn_upgrade_error
            .to_string()
            .contains("could not run npm upgrade command"));
        let spawn_metadata_error = resolve_node_managed_binary(&InstallMethod::Npm)
            .expect_err("missing npm metadata command must fail");
        assert!(spawn_metadata_error
            .to_string()
            .contains("could not run npm metadata command"));

        fs::write(
            &npm,
            format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", root.display()),
        )
        .expect("restore npm for missing node");
        fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).expect("chmod restored npm");
        fs::remove_file(&node).expect("remove fake node");
        let spawn_node_error = resolve_node_managed_binary(&InstallMethod::Npm)
            .expect_err("missing Node executable must fail");
        assert!(spawn_node_error
            .to_string()
            .contains("could not run Node.js"));

        assert!(restart_executable_for_install_method(&InstallMethod::Unknown).is_err());
        assert!(
            node_package_manager_upgrade_args(&InstallMethod::Script, "1.2.3")
                .unwrap()
                .is_empty()
        );
        assert!(upgrade_via_node_package_manager(&InstallMethod::Script, "1.2.3").is_err());
    }
}
