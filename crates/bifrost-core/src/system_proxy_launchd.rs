use std::path::{Path, PathBuf};

use crate::{BifrostError, Result, SystemProxyManager};

pub const DEFAULT_LABEL: &str = "com.bifrost.system-proxy-cleanup";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const STOP_SUPPRESSION_FILE_NAME: &str = ".system_proxy_launchd_stop_suppressed";
const STOP_SUPPRESSION_TTL_SECS: u64 = 300;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemProxyLaunchdConfig {
    pub label: String,
    pub program: PathBuf,
    pub data_dir: PathBuf,
    pub plist_path: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub installed_version: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemProxyLaunchdStatus {
    pub supported: bool,
    pub installed: bool,
    pub loaded: bool,
    pub label: String,
    pub plist_path: PathBuf,
    pub program: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub installed_version: Option<String>,
    pub current_version: String,
    pub needs_upgrade: bool,
    pub message: Option<String>,
}

impl SystemProxyLaunchdConfig {
    pub fn new(
        label: Option<String>,
        program: Option<PathBuf>,
        data_dir: PathBuf,
        plist_path: Option<PathBuf>,
    ) -> Result<Self> {
        let label = label.unwrap_or_else(|| DEFAULT_LABEL.to_string());
        validate_label(&label)?;
        let program = match program {
            Some(program) => absolutize(program)?,
            None => std::env::current_exe().map_err(|error| {
                BifrostError::Config(format!("Failed to resolve current executable: {error}"))
            })?,
        };
        let data_dir = absolutize(data_dir)?;
        let plist_path = match plist_path {
            Some(path) => absolutize(path)?,
            None => PathBuf::from(format!("/Library/LaunchDaemons/{label}.plist")),
        };

        Ok(Self {
            label,
            program,
            data_dir,
            plist_path,
            stdout_path: PathBuf::from("/var/log/bifrost-system-proxy-cleanup.log"),
            stderr_path: PathBuf::from("/var/log/bifrost-system-proxy-cleanup.err"),
            installed_version: CURRENT_VERSION.to_string(),
        })
    }
}

pub fn render_launchd_plist(config: &SystemProxyLaunchdConfig) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>system-proxy</string>
    <string>cleanup-daemon</string>
    <string>--data-dir</string>
    <string>{data_dir}</string>
    <string>--installed-version</string>
    <string>{version}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout_path}</string>
  <key>StandardErrorPath</key>
  <string>{stderr_path}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>BIFROST_LAUNCHD_INSTALLED_VERSION</key>
    <string>{version}</string>
  </dict>
</dict>
</plist>
"#,
        label = escape_xml(&config.label),
        program = escape_xml(config.program.to_string_lossy().as_ref()),
        data_dir = escape_xml(config.data_dir.to_string_lossy().as_ref()),
        version = escape_xml(&config.installed_version),
        stdout_path = escape_xml(config.stdout_path.to_string_lossy().as_ref()),
        stderr_path = escape_xml(config.stderr_path.to_string_lossy().as_ref()),
    )
}

pub fn install_launchd_cleanup(
    config: &SystemProxyLaunchdConfig,
) -> Result<SystemProxyLaunchdStatus> {
    ensure_macos_supported()?;
    ensure_root("install macOS system proxy cleanup LaunchDaemon")?;
    std::fs::create_dir_all(&config.data_dir)?;
    if let Some(parent) = config.plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config.plist_path, render_launchd_plist(config))?;
    set_secure_plist_permissions(&config.plist_path)?;

    let stop_suppression_marker = create_stop_suppression_marker_for_plist(&config.plist_path).ok();
    run_launchctl(&[
        "bootout",
        "system",
        config.plist_path.to_string_lossy().as_ref(),
    ])
    .ok();
    remove_stop_suppression_marker(stop_suppression_marker.as_deref());
    run_launchctl(&["enable", &format!("system/{}", config.label)]).ok();
    run_launchctl(&[
        "bootstrap",
        "system",
        config.plist_path.to_string_lossy().as_ref(),
    ])?;
    run_launchctl(&["enable", &format!("system/{}", config.label)])?;
    run_launchctl(&["kickstart", "-k", &format!("system/{}", config.label)]).ok();
    launchd_status_for_config(config)
}

pub fn install_launchd_cleanup_with_gui_auth(
    config: &SystemProxyLaunchdConfig,
) -> Result<SystemProxyLaunchdStatus> {
    ensure_macos_supported()?;
    let args = vec![
        "system-proxy".to_string(),
        "launchd".to_string(),
        "install".to_string(),
        "--data-dir".to_string(),
        config.data_dir.display().to_string(),
        "--program".to_string(),
        config.program.display().to_string(),
        "--label".to_string(),
        config.label.clone(),
        "--plist-path".to_string(),
        config.plist_path.display().to_string(),
    ];
    run_with_gui_auth(&config.program, &args, &install_auth_prompt(config))?;
    launchd_status_for_config(config)
}

pub fn uninstall_launchd_cleanup(
    label: Option<&str>,
    plist_path: Option<PathBuf>,
) -> Result<SystemProxyLaunchdStatus> {
    ensure_macos_supported()?;
    ensure_root("uninstall macOS system proxy cleanup LaunchDaemon")?;
    let label = label.unwrap_or(DEFAULT_LABEL);
    validate_label(label)?;
    let plist_path = plist_path.unwrap_or_else(|| default_plist_path(label));
    let stop_suppression_marker = create_stop_suppression_marker_for_plist(&plist_path).ok();
    run_launchctl(&["bootout", "system", plist_path.to_string_lossy().as_ref()]).ok();
    remove_stop_suppression_marker(stop_suppression_marker.as_deref());
    run_launchctl(&["disable", &format!("system/{label}")]).ok();
    if plist_path.exists() {
        std::fs::remove_file(&plist_path)?;
    }
    launchd_status(label, Some(plist_path))
}

pub fn uninstall_launchd_cleanup_with_gui_auth(
    label: Option<&str>,
    plist_path: Option<PathBuf>,
    program: Option<PathBuf>,
) -> Result<SystemProxyLaunchdStatus> {
    ensure_macos_supported()?;
    let label = label.unwrap_or(DEFAULT_LABEL);
    validate_label(label)?;
    let program = match program {
        Some(program) => absolutize(program)?,
        None => std::env::current_exe().map_err(|error| {
            BifrostError::Config(format!("Failed to resolve current executable: {error}"))
        })?,
    };
    let mut args = vec![
        "system-proxy".to_string(),
        "launchd".to_string(),
        "uninstall".to_string(),
        "--label".to_string(),
        label.to_string(),
    ];
    if let Some(plist_path) = &plist_path {
        args.push("--plist-path".to_string());
        args.push(plist_path.display().to_string());
    }
    run_with_gui_auth(&program, &args, &uninstall_auth_prompt(label))?;
    launchd_status(label, plist_path)
}

pub fn launchd_status(
    label: &str,
    plist_path: Option<PathBuf>,
) -> Result<SystemProxyLaunchdStatus> {
    launchd_status_with_expected(label, plist_path, std::env::current_exe().ok(), None)
}

pub fn launchd_status_for_config(
    config: &SystemProxyLaunchdConfig,
) -> Result<SystemProxyLaunchdStatus> {
    launchd_status_with_expected(
        &config.label,
        Some(config.plist_path.clone()),
        Some(config.program.clone()),
        Some(config.data_dir.clone()),
    )
}

fn launchd_status_with_expected(
    label: &str,
    plist_path: Option<PathBuf>,
    expected_program: Option<PathBuf>,
    expected_data_dir: Option<PathBuf>,
) -> Result<SystemProxyLaunchdStatus> {
    validate_label(label)?;
    let supported = cfg!(target_os = "macos");
    let plist_path = plist_path.unwrap_or_else(|| default_plist_path(label));
    if !supported {
        return Ok(SystemProxyLaunchdStatus {
            supported: false,
            installed: false,
            loaded: false,
            label: label.to_string(),
            plist_path,
            program: None,
            data_dir: None,
            installed_version: None,
            current_version: CURRENT_VERSION.to_string(),
            needs_upgrade: false,
            message: Some("LaunchDaemon cleanup is only supported on macOS".to_string()),
        });
    }

    let plist_content = std::fs::read_to_string(&plist_path).ok();
    let installed = plist_content.is_some();
    let loaded = run_launchctl(&["print", &format!("system/{label}")]).is_ok();
    let parsed = plist_content.as_deref().map(parse_installed_plist);
    let program = parsed.as_ref().and_then(|parsed| parsed.program.clone());
    let data_dir = parsed.as_ref().and_then(|parsed| parsed.data_dir.clone());
    let installed_version = parsed
        .as_ref()
        .and_then(|parsed| parsed.installed_version.clone());
    let needs_upgrade = installed
        && (installed_version.as_deref() != Some(CURRENT_VERSION)
            || program.as_ref().zip(expected_program.as_ref()).is_some_and(
                |(installed_program, current_program)| installed_program != current_program,
            )
            || data_dir
                .as_ref()
                .zip(expected_data_dir.as_ref())
                .is_some_and(|(installed_data_dir, expected_data_dir)| {
                    installed_data_dir != expected_data_dir
                }));
    let message = if needs_upgrade {
        Some(
            "Installed cleanup LaunchDaemon does not match the current Bifrost binary, data directory, or version"
                .to_string(),
        )
    } else if installed && !loaded {
        Some("Cleanup LaunchDaemon plist exists but is not loaded".to_string())
    } else {
        None
    };

    Ok(SystemProxyLaunchdStatus {
        supported,
        installed,
        loaded,
        label: label.to_string(),
        plist_path,
        program,
        data_dir,
        installed_version,
        current_version: CURRENT_VERSION.to_string(),
        needs_upgrade,
        message,
    })
}

pub fn recover_if_no_live_runtime(data_dir: &Path) -> Result<bool> {
    if runtime_pid_is_alive(data_dir) {
        tracing::info!(
            target: "bifrost_core::system_proxy_launchd",
            data_dir = %data_dir.display(),
            "system proxy cleanup daemon skipped startup cleanup because Bifrost runtime is alive"
        );
        return Ok(false);
    }
    if SystemProxyManager::managed_target_has_live_listener(data_dir) {
        tracing::info!(
            target: "bifrost_core::system_proxy_launchd",
            data_dir = %data_dir.display(),
            "system proxy cleanup daemon skipped startup cleanup because managed proxy target is still serving"
        );
        return Ok(false);
    }
    SystemProxyManager::recover_from_crash(data_dir)?;
    Ok(true)
}

pub fn consume_stop_restore_suppression(data_dir: &Path) -> bool {
    let marker = data_dir.join(STOP_SUPPRESSION_FILE_NAME);
    let Ok(metadata) = std::fs::metadata(&marker) else {
        return false;
    };
    let is_recent = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed.as_secs() <= STOP_SUPPRESSION_TTL_SECS);
    let _ = std::fs::remove_file(&marker);
    is_recent
}

fn runtime_pid_is_alive(data_dir: &Path) -> bool {
    let runtime_path = data_dir.join("runtime.json");
    let pid = std::fs::read_to_string(&runtime_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| value.get("pid").and_then(|pid| pid.as_u64()))
        .or_else(|| {
            std::fs::read_to_string(data_dir.join("bifrost.pid"))
                .ok()
                .and_then(|content| content.trim().parse::<u64>().ok())
        })
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .map(|pid| pid as u32);

    pid.is_some_and(process_is_running)
}

fn process_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("/bin/kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn ensure_macos_supported() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(BifrostError::Config(
            "LaunchDaemon cleanup is only supported on macOS".to_string(),
        ))
    }
}

fn ensure_root(action: &str) -> Result<()> {
    if is_root() {
        return Ok(());
    }
    Err(BifrostError::Config(format!(
        "RequiresAdmin: {action} requires root privileges; run with sudo"
    )))
}

fn is_root() -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("/usr/bin/id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|uid| uid.trim() == "0")
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn run_launchctl(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|error| BifrostError::Config(format!("Failed to execute launchctl: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(BifrostError::Config(format!(
        "launchctl {} failed (code {:?}): {}{}",
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn create_stop_suppression_marker_for_plist(plist_path: &Path) -> Result<PathBuf> {
    let content = std::fs::read_to_string(plist_path)?;
    let parsed = parse_installed_plist(&content);
    let data_dir = parsed.data_dir.ok_or_else(|| {
        BifrostError::Config(format!(
            "Failed to read cleanup LaunchDaemon data directory from {}",
            plist_path.display()
        ))
    })?;
    std::fs::create_dir_all(&data_dir)?;
    let marker = data_dir.join(STOP_SUPPRESSION_FILE_NAME);
    std::fs::write(&marker, CURRENT_VERSION)?;
    Ok(marker)
}

fn remove_stop_suppression_marker(marker: Option<&Path>) {
    if let Some(marker) = marker {
        let _ = std::fs::remove_file(marker);
    }
}

fn run_with_gui_auth(program: &Path, args: &[String], prompt: &str) -> Result<()> {
    let mut command_parts = Vec::with_capacity(args.len() + 1);
    command_parts.push(shell_quote(program.to_string_lossy().as_ref()));
    command_parts.extend(args.iter().map(|arg| shell_quote(arg)));
    let shell_command = command_parts.join(" ");
    let script = build_gui_auth_script(&shell_command, prompt);

    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .map_err(|error| BifrostError::Config(format!("Failed to execute osascript: {error}")))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || stderr.contains("-128") {
        return Err(BifrostError::Config(
            "UserCancelled: User cancelled authorization".to_string(),
        ));
    }

    Err(BifrostError::Config(format!(
        "osascript failed (code {:?}): {} {}",
        output.status.code(),
        stdout.trim(),
        stderr.trim()
    )))
}

fn build_gui_auth_script(shell_command: &str, prompt: &str) -> String {
    format!(
        r#"do shell script "{}" with administrator privileges with prompt "{}""#,
        escape_applescript_string(shell_command),
        escape_applescript_string(prompt)
    )
}

fn install_auth_prompt(_config: &SystemProxyLaunchdConfig) -> String {
    "Bifrost needs administrator permission to install its network protection helper.\n\nIf Bifrost exits unexpectedly, this helper restores your system proxy settings automatically."
        .to_string()
}

fn uninstall_auth_prompt(_label: &str) -> String {
    "Bifrost needs administrator permission to remove its network protection helper.\n\nAfter removal, Bifrost cannot automatically restore system proxy settings after an unexpected exit."
        .to_string()
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\\''"))
}

fn escape_applescript_string(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn set_secure_plist_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn default_plist_path(label: &str) -> PathBuf {
    PathBuf::from(format!("/Library/LaunchDaemons/{label}.plist"))
}

fn validate_label(label: &str) -> Result<()> {
    let valid = !label.is_empty()
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "Invalid LaunchDaemon label: {label}"
        )))
    }
}

fn absolutize(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()?.join(path))
}

#[derive(Debug, Default)]
struct ParsedLaunchdPlist {
    program: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    installed_version: Option<String>,
}

fn parse_installed_plist(content: &str) -> ParsedLaunchdPlist {
    let program_arguments = content
        .split_once("<key>ProgramArguments</key>")
        .and_then(|(_, after_key)| after_key.split_once("</array>").map(|(array, _)| array))
        .unwrap_or(content);
    let strings = extract_xml_strings(program_arguments);
    let program = strings.first().map(PathBuf::from);
    let data_dir = strings
        .windows(2)
        .find(|pair| pair[0] == "--data-dir")
        .map(|pair| PathBuf::from(&pair[1]));
    let installed_version = strings
        .windows(2)
        .find(|pair| pair[0] == "--installed-version")
        .map(|pair| pair[1].clone())
        .or_else(|| extract_env_value(content, "BIFROST_LAUNCHD_INSTALLED_VERSION"));

    ParsedLaunchdPlist {
        program,
        data_dir,
        installed_version,
    }
}

fn extract_xml_strings(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("<string>") {
        rest = &rest[start + "<string>".len()..];
        let Some(end) = rest.find("</string>") else {
            break;
        };
        result.push(unescape_xml(&rest[..end]));
        rest = &rest[end + "</string>".len()..];
    }
    result
}

fn extract_env_value(content: &str, key: &str) -> Option<String> {
    let key_marker = format!("<key>{}</key>", escape_xml(key));
    let after_key = content.split_once(&key_marker)?.1;
    extract_xml_strings(after_key).into_iter().next()
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unescape_xml(input: &str) -> String {
    input
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_plist_contains_cleanup_daemon_version_and_paths() {
        let config = SystemProxyLaunchdConfig::new(
            Some("com.bifrost.test".to_string()),
            Some(PathBuf::from("/tmp/bifrost")),
            PathBuf::from("/tmp/bifrost-data"),
            Some(PathBuf::from("/tmp/com.bifrost.test.plist")),
        )
        .expect("config");
        let plist = render_launchd_plist(&config);

        assert!(plist.contains("cleanup-daemon"));
        assert!(plist.contains("--installed-version"));
        assert!(plist.contains(CURRENT_VERSION));
        assert!(plist.contains("/tmp/bifrost-data"));
    }

    #[test]
    fn parse_installed_plist_detects_program_data_dir_and_version() {
        let config = SystemProxyLaunchdConfig::new(
            Some("com.bifrost.test".to_string()),
            Some(PathBuf::from("/tmp/bifrost")),
            PathBuf::from("/tmp/bifrost-data"),
            Some(PathBuf::from("/tmp/com.bifrost.test.plist")),
        )
        .expect("config");
        let parsed = parse_installed_plist(&render_launchd_plist(&config));

        assert_eq!(parsed.program, Some(PathBuf::from("/tmp/bifrost")));
        assert_eq!(parsed.data_dir, Some(PathBuf::from("/tmp/bifrost-data")));
        assert_eq!(parsed.installed_version.as_deref(), Some(CURRENT_VERSION));
    }

    #[test]
    fn gui_auth_script_includes_bifrost_prompt() {
        let config = SystemProxyLaunchdConfig::new(
            Some("com.bifrost.test".to_string()),
            Some(PathBuf::from("/tmp/bifrost")),
            PathBuf::from("/tmp/bifrost-data"),
            Some(PathBuf::from("/tmp/com.bifrost.test.plist")),
        )
        .expect("config");
        let prompt = install_auth_prompt(&config);
        let script = build_gui_auth_script("true", &prompt);

        assert!(script.contains("with administrator privileges with prompt"));
        assert!(script.contains("Bifrost needs administrator permission"));
        assert!(script.contains("network protection helper"));
        assert!(script.contains("restores your system proxy settings automatically"));
        assert!(script.contains("exits unexpectedly"));
    }

    #[test]
    fn stop_restore_suppression_marker_is_consumed_once() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!("bifrost-launchd-test-{unique}"));
        let plist_path = std::env::temp_dir().join(format!("com.bifrost.test.{unique}.plist"));
        let config = SystemProxyLaunchdConfig::new(
            Some(format!("com.bifrost.test.{unique}")),
            Some(PathBuf::from("/tmp/bifrost")),
            data_dir.clone(),
            Some(plist_path.clone()),
        )
        .expect("config");
        std::fs::write(&plist_path, render_launchd_plist(&config)).expect("write plist");

        let marker = create_stop_suppression_marker_for_plist(&plist_path).expect("marker");
        assert!(marker.exists());
        assert!(consume_stop_restore_suppression(&data_dir));
        assert!(!consume_stop_restore_suppression(&data_dir));

        let _ = std::fs::remove_file(plist_path);
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn status_for_config_requires_matching_data_dir() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let plist_path = std::env::temp_dir().join(format!("com.bifrost.test.{unique}.plist"));
        let installed_config = SystemProxyLaunchdConfig::new(
            Some(format!("com.bifrost.test.{unique}")),
            Some(std::env::current_exe().expect("current exe")),
            PathBuf::from("/tmp/bifrost-installed-data"),
            Some(plist_path.clone()),
        )
        .expect("installed config");
        std::fs::write(&plist_path, render_launchd_plist(&installed_config)).expect("write plist");

        let current_config = SystemProxyLaunchdConfig::new(
            Some(format!("com.bifrost.test.{unique}")),
            Some(std::env::current_exe().expect("current exe")),
            PathBuf::from("/tmp/bifrost-current-data"),
            Some(plist_path.clone()),
        )
        .expect("current config");
        let status = launchd_status_for_config(&current_config).expect("status");
        let _ = std::fs::remove_file(plist_path);

        assert!(status.installed);
        assert!(status.needs_upgrade);
        assert!(status
            .message
            .as_deref()
            .expect("message")
            .contains("data directory"));
    }

    #[test]
    fn invalid_launchd_label_is_rejected() {
        let error = SystemProxyLaunchdConfig::new(
            Some("../bad".to_string()),
            Some(PathBuf::from("/tmp/bifrost")),
            PathBuf::from("/tmp/bifrost-data"),
            None,
        )
        .expect_err("invalid label");
        assert!(error.to_string().contains("Invalid LaunchDaemon label"));
    }
}
