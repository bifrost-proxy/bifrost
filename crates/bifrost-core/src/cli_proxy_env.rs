use std::path::{Path, PathBuf};

use base64::Engine;

use crate::{BifrostError, Result};

const START_MARKER: &str = "# >>> Bifrost CLI proxy environment start >>>";
const END_MARKER: &str = "# <<< Bifrost CLI proxy environment end <<<";

const PROXY_ENV_VARS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];
const NO_PROXY_ENV_VARS: &[&str] = &["NO_PROXY", "no_proxy"];
const DIRECT_CA_FILE_ENV_VARS: &[&str] = &["BIFROST_CA_BUNDLE", "NODE_EXTRA_CA_CERTS"];
const BUNDLE_CA_FILE_ENV_VARS: &[&str] = &[
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "PIP_CERT",
    "NPM_CONFIG_CAFILE",
    "npm_config_cafile",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
    "CARGO_HTTP_CAINFO",
    "CARGO_HTTP_PROXY_CAINFO",
    "COMPOSER_CAFILE",
    "DENO_CERT",
];
const CA_DIR_ENV_VARS: &[&str] = &["BIFROST_CA_DIR", "SSL_CERT_DIR"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliProxyShell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

impl CliProxyShell {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::PowerShell => "powershell",
        }
    }

    pub fn detect() -> Result<Self> {
        let shell = std::env::var("SHELL").ok();
        detect_shell_environment(
            shell.as_deref(),
            std::env::var_os("POWERSHELL_DISTRIBUTION_CHANNEL").is_some(),
        )
    }
}

fn detect_shell_environment(
    shell: Option<&str>,
    powershell_channel: bool,
) -> Result<CliProxyShell> {
    if let Some(shell) = shell {
        let shell = shell.to_ascii_lowercase();
        if shell.contains("zsh") {
            return Ok(CliProxyShell::Zsh);
        }
        if shell.contains("bash") {
            return Ok(CliProxyShell::Bash);
        }
        if shell.contains("fish") {
            return Ok(CliProxyShell::Fish);
        }
    }

    if powershell_channel {
        return Ok(CliProxyShell::PowerShell);
    }

    Err(BifrostError::Config(
        "Could not detect a supported current shell. Use --shell bash|zsh|fish|powershell"
            .to_string(),
    ))
}

impl CliProxyShell {
    pub fn config_paths(self) -> Result<Vec<PathBuf>> {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .ok_or_else(|| BifrostError::Config("Could not determine home directory".into()))?;

        Ok(self.config_paths_for_home(&home))
    }

    pub fn config_paths_for_home(self, home: &Path) -> Vec<PathBuf> {
        match self {
            Self::Bash => vec![home.join(".bashrc"), home.join(".bash_profile")],
            Self::Zsh => vec![home.join(".zshrc"), home.join(".zprofile")],
            Self::Fish => vec![home.join(".config/fish/config.fish")],
            Self::PowerShell => powershell_profile_paths(home),
        }
    }
}

#[cfg(windows)]
fn powershell_profile_paths(home: &Path) -> Vec<PathBuf> {
    // Derive these paths from the already-resolved HOME. Besides matching PowerShell's
    // conventional user profiles, this keeps explicit/test HOME overrides isolated instead of
    // unexpectedly falling back to another account's Documents directory.
    let documents = home.join("Documents");
    vec![
        documents.join("PowerShell/Microsoft.PowerShell_profile.ps1"),
        documents.join("WindowsPowerShell/Microsoft.PowerShell_profile.ps1"),
    ]
}

#[cfg(not(windows))]
fn powershell_profile_paths(home: &Path) -> Vec<PathBuf> {
    vec![home.join(".config/powershell/Microsoft.PowerShell_profile.ps1")]
}

#[derive(Debug, Clone)]
pub struct CliProxyEnvironmentConfig {
    pub proxy_url: String,
    pub no_proxy: String,
    pub ca_file: PathBuf,
    pub ca_bundle: PathBuf,
    pub ca_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliProxyEnvironmentResult {
    pub shell: CliProxyShell,
    pub config_paths: Vec<PathBuf>,
    pub changed_paths: Vec<PathBuf>,
}

pub struct CliProxyEnvironmentManager {
    shell: CliProxyShell,
    config_paths: Vec<PathBuf>,
}

impl CliProxyEnvironmentManager {
    pub fn new(shell: CliProxyShell) -> Result<Self> {
        Ok(Self {
            shell,
            config_paths: shell.config_paths()?,
        })
    }

    fn with_paths(shell: CliProxyShell, config_paths: Vec<PathBuf>) -> Self {
        Self {
            shell,
            config_paths,
        }
    }

    pub fn enable(&self, config: &CliProxyEnvironmentConfig) -> Result<CliProxyEnvironmentResult> {
        validate_environment_config(config)?;
        let block = generate_config_block(self.shell, config)?;
        let prepared = self.prepare_updates(|_path, content| {
            let updated = replace_or_add_marked_block(&content, &block)?;
            if updated == content {
                Ok(None)
            } else {
                Ok(Some(updated))
            }
        })?;
        let changed_paths = write_prepared_updates(prepared)?;

        Ok(CliProxyEnvironmentResult {
            shell: self.shell,
            config_paths: self.config_paths.clone(),
            changed_paths,
        })
    }

    pub fn manual_enable_block(&self, config: &CliProxyEnvironmentConfig) -> Result<String> {
        generate_config_block(self.shell, config)
    }

    pub fn environment_markers() -> (&'static str, &'static str) {
        (START_MARKER, END_MARKER)
    }

    /// Remove every standalone `bifrost cli-proxy enable` block for the current user.
    ///
    /// The lifecycle cleanup daemon cannot assume it was launched from the same shell that ran
    /// `enable`, so cleanup must cover every supported profile instead of only the daemon's
    /// current shell. All paths are prepared before any write, preserving the same transactional
    /// rollback behavior as a single-shell disable.
    pub fn disable_all_managed() -> Result<Vec<PathBuf>> {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .ok_or_else(|| BifrostError::Config("Could not determine home directory".into()))?;
        Self::disable_all_managed_for_home(&home)
    }

    fn all_supported_paths_for_home(home: &Path) -> Vec<PathBuf> {
        let mut paths = [
            CliProxyShell::Bash,
            CliProxyShell::Zsh,
            CliProxyShell::Fish,
            CliProxyShell::PowerShell,
        ]
        .into_iter()
        .flat_map(|shell| shell.config_paths_for_home(home))
        .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }

    fn disable_all_managed_for_home(home: &Path) -> Result<Vec<PathBuf>> {
        let manager = Self::with_paths(
            CliProxyShell::Bash,
            Self::all_supported_paths_for_home(home),
        );
        Ok(manager.disable()?.changed_paths)
    }

    pub fn disable(&self) -> Result<CliProxyEnvironmentResult> {
        let prepared = self.prepare_updates(|_path, content| {
            let updated = remove_marked_block(&content)?;
            if updated == content {
                Ok(None)
            } else {
                Ok(Some(updated))
            }
        })?;
        let changed_paths = write_prepared_updates(prepared)?;

        Ok(CliProxyEnvironmentResult {
            shell: self.shell,
            config_paths: self.config_paths.clone(),
            changed_paths,
        })
    }

    fn prepare_updates(
        &self,
        mut update: impl FnMut(&Path, String) -> Result<Option<String>>,
    ) -> Result<Vec<PreparedProfileUpdate>> {
        let mut prepared = Vec::new();
        for path in &self.config_paths {
            if path.exists() && !path.is_file() {
                return Err(BifrostError::Config(format!(
                    "Shell profile exists but is not a regular file: {}",
                    path.display()
                )));
            }
            let original = if path.exists() {
                Some(std::fs::read_to_string(path)?)
            } else {
                None
            };
            let content = original.clone().unwrap_or_default();
            if let Some(updated) = update(path, content)? {
                prepared.push(PreparedProfileUpdate {
                    path: path.clone(),
                    original,
                    updated,
                });
            }
        }
        Ok(prepared)
    }
}

struct PreparedProfileUpdate {
    path: PathBuf,
    original: Option<String>,
    updated: String,
}

fn write_prepared_updates(prepared: Vec<PreparedProfileUpdate>) -> Result<Vec<PathBuf>> {
    let mut changed_paths = Vec::new();
    for (index, item) in prepared.iter().enumerate() {
        // Keep profile files in place even when disabling leaves them empty. An empty profile may
        // have existed before enable, and removing it would be an unrelated user-visible change.
        let write_result = item
            .path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&item.path, &item.updated));
        if let Err(write_error) = write_result {
            let rollback_errors = prepared[..=index]
                .iter()
                .rev()
                .filter_map(|applied| rollback_profile_update(applied).err())
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
            if rollback_errors.is_empty() {
                return Err(BifrostError::Config(format!(
                    "Failed to update shell profile {}: {write_error}",
                    item.path.display()
                )));
            }
            return Err(BifrostError::Config(format!(
                "Failed to update {}: {write_error}; rollback also failed: {}",
                item.path.display(),
                rollback_errors.join("; ")
            )));
        }
        changed_paths.push(item.path.clone());
    }
    Ok(changed_paths)
}

fn rollback_profile_update(item: &PreparedProfileUpdate) -> std::io::Result<()> {
    match &item.original {
        Some(content) => std::fs::write(&item.path, content),
        None if item.path.exists() => std::fs::remove_file(&item.path),
        None => Ok(()),
    }
}

pub fn create_combined_ca_bundle(ca_file: &Path, output: &Path) -> Result<usize> {
    if !ca_file.is_file() {
        return Err(BifrostError::Config(format!(
            "CA file does not exist or is not a file: {}",
            ca_file.display()
        )));
    }

    let native = rustls_native_certs::load_native_certs();
    if native.certs.is_empty() {
        return Err(BifrostError::Config(format!(
            "Could not load system root certificates ({} loader errors); refusing to create a CA bundle that would replace system trust",
            native.errors.len()
        )));
    }

    let roots = native
        .certs
        .iter()
        .map(|cert| cert.as_ref())
        .collect::<Vec<_>>();
    write_combined_ca_bundle(ca_file, output, &roots)?;
    Ok(roots.len())
}

fn write_combined_ca_bundle(ca_file: &Path, output: &Path, roots: &[&[u8]]) -> Result<()> {
    let bifrost_ca = std::fs::read_to_string(ca_file)?;
    if !bifrost_ca.contains("-----BEGIN CERTIFICATE-----") {
        return Err(BifrostError::Config(format!(
            "CA file is not a PEM certificate: {}",
            ca_file.display()
        )));
    }

    let mut bundle = String::new();
    for root in roots {
        append_der_certificate_pem(&mut bundle, root);
    }
    if !bundle.ends_with('\n') {
        bundle.push('\n');
    }
    bundle.push_str(bifrost_ca.trim());
    bundle.push('\n');

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, bundle)?;
    Ok(())
}

fn append_der_certificate_pem(output: &mut String, der: &[u8]) {
    output.push_str("-----BEGIN CERTIFICATE-----\n");
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    for chunk in encoded.as_bytes().chunks(64) {
        output.push_str(std::str::from_utf8(chunk).expect("base64 is valid UTF-8"));
        output.push('\n');
    }
    output.push_str("-----END CERTIFICATE-----\n");
}

fn validate_environment_config(config: &CliProxyEnvironmentConfig) -> Result<()> {
    if !config.ca_file.is_file() {
        return Err(BifrostError::Config(format!(
            "CA file does not exist or is not a file: {}",
            config.ca_file.display()
        )));
    }
    if !config.ca_bundle.is_file() {
        return Err(BifrostError::Config(format!(
            "Combined CA bundle does not exist or is not a file: {}",
            config.ca_bundle.display()
        )));
    }
    if !config.ca_dir.is_dir() {
        return Err(BifrostError::Config(format!(
            "CA directory does not exist or is not a directory: {}",
            config.ca_dir.display()
        )));
    }
    validate_environment_value("proxy URL", &config.proxy_url)?;
    validate_environment_value("no-proxy list", &config.no_proxy)?;
    path_environment_value("CA file", &config.ca_file)?;
    path_environment_value("combined CA bundle", &config.ca_bundle)?;
    path_environment_value("CA directory", &config.ca_dir)?;
    Ok(())
}

fn validate_environment_value(label: &str, value: &str) -> Result<()> {
    if value.contains(['\r', '\n']) || value.contains(START_MARKER) || value.contains(END_MARKER) {
        return Err(BifrostError::Config(format!(
            "{label} cannot contain newlines or Bifrost CLI proxy marker text"
        )));
    }
    Ok(())
}

fn path_environment_value<'a>(label: &str, path: &'a Path) -> Result<&'a str> {
    let value = path.to_str().ok_or_else(|| {
        BifrostError::Config(format!(
            "{label} is not valid UTF-8 and cannot be written safely to a shell profile: {}",
            path.display()
        ))
    })?;
    validate_environment_value(label, value)?;
    Ok(value)
}

fn generate_config_block(
    shell: CliProxyShell,
    config: &CliProxyEnvironmentConfig,
) -> Result<String> {
    validate_environment_value("proxy URL", &config.proxy_url)?;
    validate_environment_value("no-proxy list", &config.no_proxy)?;
    let mut variables = Vec::new();
    for name in PROXY_ENV_VARS {
        variables.push((*name, config.proxy_url.as_str()));
    }
    for name in NO_PROXY_ENV_VARS {
        variables.push((*name, config.no_proxy.as_str()));
    }
    let ca_file = path_environment_value("CA file", &config.ca_file)?;
    for name in DIRECT_CA_FILE_ENV_VARS {
        variables.push((*name, ca_file));
    }
    let ca_bundle = path_environment_value("combined CA bundle", &config.ca_bundle)?;
    for name in BUNDLE_CA_FILE_ENV_VARS {
        variables.push((*name, ca_bundle));
    }
    let ca_dir = path_environment_value("CA directory", &config.ca_dir)?;
    for name in CA_DIR_ENV_VARS {
        variables.push((*name, ca_dir));
    }

    let mut lines = vec![START_MARKER.to_string()];
    lines.extend(
        variables
            .into_iter()
            .map(|(name, value)| format_assignment(shell, name, value)),
    );
    lines.push(END_MARKER.to_string());
    Ok(lines.join("\n"))
}

fn format_assignment(shell: CliProxyShell, name: &str, value: &str) -> String {
    match shell {
        CliProxyShell::Bash | CliProxyShell::Zsh => {
            format!("export {name}={}", quote_posix(value))
        }
        CliProxyShell::Fish => format!("set -gx {name} {}", quote_fish(value)),
        CliProxyShell::PowerShell => format!("$env:{name} = {}", quote_powershell(value)),
    }
}

fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn quote_fish(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn quote_powershell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn replace_or_add_marked_block(content: &str, block: &str) -> Result<String> {
    match marker_bounds(content)? {
        (Some(start), Some(end)) if start < end => {
            let before = &content[..start];
            let after = &content[end + END_MARKER.len()..];
            Ok(format!("{before}{block}{after}"))
        }
        (None, None) if content.is_empty() => Ok(block.to_string()),
        (None, None) => Ok(format!("{}\n\n{block}", content.trim_end())),
        _ => unreachable!("marker_bounds validates paired marker order"),
    }
}

fn remove_marked_block(content: &str) -> Result<String> {
    match marker_bounds(content)? {
        (Some(start), Some(end)) if start < end => {
            let before = content[..start].trim_end();
            let after = content[end + END_MARKER.len()..].trim_start_matches(['\r', '\n']);
            Ok(match (before.is_empty(), after.is_empty()) {
                (true, true) => String::new(),
                (true, false) => after.to_string(),
                (false, true) => before.to_string(),
                (false, false) => format!("{before}\n{after}"),
            })
        }
        (None, None) => Ok(content.to_string()),
        _ => unreachable!("marker_bounds validates paired marker order"),
    }
}

fn marker_bounds(content: &str) -> Result<(Option<usize>, Option<usize>)> {
    let starts = content.match_indices(START_MARKER).collect::<Vec<_>>();
    let ends = content.match_indices(END_MARKER).collect::<Vec<_>>();
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok((None, None)),
        ([(start, _)], [(end, _)]) if start < end => Ok((Some(*start), Some(*end))),
        _ => Err(BifrostError::Config(format!(
            "Shell profile contains an incomplete, reversed, or duplicate Bifrost CLI proxy marker block; remove every range from {START_MARKER:?} through {END_MARKER:?} manually"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ScopedEnv {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl ScopedEnv {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn test_config(temp: &tempfile::TempDir) -> CliProxyEnvironmentConfig {
        let ca_file = temp.path().join("certs/ca.crt");
        let ca_bundle = temp.path().join("certs/cli-proxy-ca-bundle.pem");
        std::fs::create_dir_all(ca_file.parent().unwrap()).unwrap();
        std::fs::write(
            &ca_file,
            "-----BEGIN CERTIFICATE-----\nY2E=\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        std::fs::write(
            &ca_bundle,
            "-----BEGIN CERTIFICATE-----\nYnVuZGxl\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        CliProxyEnvironmentConfig {
            proxy_url: "http://127.0.0.1:9900".into(),
            no_proxy: "localhost,127.0.0.1,::1".into(),
            ca_file,
            ca_bundle,
            ca_dir: temp.path().join("certs"),
        }
    }

    #[test]
    fn shell_config_paths_cover_supported_shells() {
        let home = Path::new("/tmp/example home");
        assert_eq!(
            CliProxyShell::Bash.config_paths_for_home(home),
            vec![home.join(".bashrc"), home.join(".bash_profile")]
        );
        assert_eq!(
            CliProxyShell::Zsh.config_paths_for_home(home),
            vec![home.join(".zshrc"), home.join(".zprofile")]
        );
        assert_eq!(
            CliProxyShell::Fish.config_paths_for_home(home),
            vec![home.join(".config/fish/config.fish")]
        );
        assert!(!CliProxyShell::PowerShell
            .config_paths_for_home(home)
            .is_empty());
    }

    #[test]
    fn shell_environment_detection_covers_every_supported_shell_and_failure() {
        assert_eq!(
            detect_shell_environment(Some("/BIN/ZSH"), false).unwrap(),
            CliProxyShell::Zsh
        );
        assert_eq!(
            detect_shell_environment(Some("/bin/bash"), false).unwrap(),
            CliProxyShell::Bash
        );
        assert_eq!(
            detect_shell_environment(Some("/usr/bin/fish"), false).unwrap(),
            CliProxyShell::Fish
        );
        assert_eq!(
            detect_shell_environment(Some("unknown"), true).unwrap(),
            CliProxyShell::PowerShell
        );
        assert!(detect_shell_environment(None, false).is_err());
    }

    #[test]
    fn shell_detect_reads_the_process_environment() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _shell = ScopedEnv::set("SHELL", "/bin/zsh");
        let _powershell = ScopedEnv::set("POWERSHELL_DISTRIBUTION_CHANNEL", "test");

        assert_eq!(CliProxyShell::detect().unwrap(), CliProxyShell::Zsh);
    }

    #[test]
    fn enable_writes_complete_bash_environment_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".bashrc");
        std::fs::write(&path, "export USER_SETTING='kept'\n").unwrap();
        let manager =
            CliProxyEnvironmentManager::with_paths(CliProxyShell::Bash, vec![path.clone()]);
        let config = test_config(&temp);

        manager.enable(&config).unwrap();
        manager.enable(&config).unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content.matches(START_MARKER).count(), 1);
        assert!(content.contains("export USER_SETTING='kept'"));
        assert!(content.contains("export HTTPS_PROXY='http://127.0.0.1:9900'"));
        assert!(content.contains("export NODE_EXTRA_CA_CERTS='"));
        assert!(content.contains("export REQUESTS_CA_BUNDLE='"));
        assert!(content.contains("export CARGO_HTTP_CAINFO='"));
        assert!(content.contains("export DENO_CERT='"));
        assert!(content.contains("export BIFROST_CA_DIR='"));
    }

    #[test]
    fn disable_only_removes_dedicated_environment_block() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".zshrc");
        let manager =
            CliProxyEnvironmentManager::with_paths(CliProxyShell::Zsh, vec![path.clone()]);
        let config = test_config(&temp);
        std::fs::write(
            &path,
            "before\n# >>> Bifrost proxy start >>>\nexport HTTP_PROXY=old\n# <<< Bifrost proxy end <<<\nafter\n",
        )
        .unwrap();

        manager.enable(&config).unwrap();
        manager.disable().unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("before"));
        assert!(content.contains("after"));
        assert!(content.contains("# >>> Bifrost proxy start >>>"));
        assert!(!content.contains(START_MARKER));
    }

    #[test]
    fn shell_assignments_quote_metacharacters() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = test_config(&temp);
        config.no_proxy = "one'; touch /tmp/not-run; echo 'two".into();

        let bash = generate_config_block(CliProxyShell::Bash, &config).unwrap();
        assert!(bash.contains("'one'\\''; touch /tmp/not-run; echo '\\''two'"));
        let fish = generate_config_block(CliProxyShell::Fish, &config).unwrap();
        assert!(fish.contains("'one\\'; touch /tmp/not-run; echo \\'two'"));
        let powershell = generate_config_block(CliProxyShell::PowerShell, &config).unwrap();
        assert!(powershell.contains("'one''; touch /tmp/not-run; echo ''two'"));
    }

    #[test]
    fn combined_bundle_contains_native_roots_and_bifrost_ca() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(&temp);
        let output = temp.path().join("combined.pem");
        write_combined_ca_bundle(&config.ca_file, &output, &[b"system-root"]).unwrap();
        let content = std::fs::read_to_string(output).unwrap();
        assert!(content.contains("c3lzdGVtLXJvb3Q="));
        assert!(content.contains("Y2E="));
        assert_eq!(content.matches("-----BEGIN CERTIFICATE-----").count(), 2);
    }

    #[test]
    fn enable_rejects_missing_ca_directory() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CliProxyEnvironmentManager::with_paths(
            CliProxyShell::Bash,
            vec![temp.path().join(".bashrc")],
        );
        let mut config = test_config(&temp);
        config.ca_dir = temp.path().join("missing");
        let error = manager.enable(&config).unwrap_err().to_string();
        assert!(error.contains("CA directory"));
    }

    #[test]
    fn enable_and_disable_reject_incomplete_marker_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".bashrc");
        let manager =
            CliProxyEnvironmentManager::with_paths(CliProxyShell::Bash, vec![path.clone()]);
        let config = test_config(&temp);
        std::fs::write(&path, format!("user setting\n{START_MARKER}\n")).unwrap();

        let enable_error = manager.enable(&config).unwrap_err().to_string();
        assert!(enable_error.contains("incomplete, reversed, or duplicate"));
        let disable_error = manager.disable().unwrap_err().to_string();
        assert!(disable_error.contains("incomplete, reversed, or duplicate"));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            format!("user setting\n{START_MARKER}\n")
        );
    }

    #[test]
    fn enable_preflights_every_profile_before_writing_any_profile() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join(".bashrc");
        let second = temp.path().join(".bash_profile");
        std::fs::write(&first, "original\n").unwrap();
        std::fs::create_dir(&second).unwrap();
        let manager = CliProxyEnvironmentManager::with_paths(
            CliProxyShell::Bash,
            vec![first.clone(), second],
        );

        let error = manager.enable(&test_config(&temp)).unwrap_err().to_string();
        assert!(error.contains("not a regular file"));
        assert_eq!(std::fs::read_to_string(first).unwrap(), "original\n");
    }

    #[test]
    fn disable_preserves_an_empty_profile_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".bashrc");
        let manager =
            CliProxyEnvironmentManager::with_paths(CliProxyShell::Bash, vec![path.clone()]);
        manager.enable(&test_config(&temp)).unwrap();
        manager.disable().unwrap();
        assert!(path.is_file());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "");
    }

    #[test]
    fn disable_noop_and_block_position_matrix_preserve_unmanaged_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".bashrc");
        let manager =
            CliProxyEnvironmentManager::with_paths(CliProxyShell::Bash, vec![path.clone()]);
        assert!(manager.disable().unwrap().changed_paths.is_empty());

        let block = format!("{START_MARKER}\nmanaged\n{END_MARKER}");
        assert_eq!(remove_marked_block(&block).unwrap(), "");
        assert_eq!(
            remove_marked_block(&format!("{block}\nafter")).unwrap(),
            "after"
        );
        assert_eq!(
            remove_marked_block(&format!("before\n{block}")).unwrap(),
            "before"
        );
        assert_eq!(
            remove_marked_block(&format!("before\n{block}\nafter")).unwrap(),
            "before\nafter"
        );
        assert_eq!(remove_marked_block("unmanaged").unwrap(), "unmanaged");
    }

    #[test]
    fn enable_rolls_back_earlier_profile_when_a_later_write_fails() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join(".bashrc");
        let blocker = temp.path().join("not-a-directory");
        let second = blocker.join(".bash_profile");
        std::fs::write(&first, "original\n").unwrap();
        std::fs::write(&blocker, "blocker\n").unwrap();
        let manager = CliProxyEnvironmentManager::with_paths(
            CliProxyShell::Bash,
            vec![first.clone(), second],
        );

        assert!(manager.enable(&test_config(&temp)).is_err());
        assert_eq!(std::fs::read_to_string(first).unwrap(), "original\n");
    }

    #[test]
    fn write_failure_reports_when_rollback_also_fails() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("not-a-directory");
        std::fs::write(&blocker, "blocker\n").unwrap();
        let error = write_prepared_updates(vec![PreparedProfileUpdate {
            path: blocker.join("profile"),
            original: Some("original\n".into()),
            updated: "updated\n".into(),
        }])
        .unwrap_err()
        .to_string();

        assert!(error.contains("rollback also failed"));
    }

    #[test]
    fn config_render_rejects_newlines_marker_injection_and_duplicate_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = test_config(&temp);
        config.no_proxy = "localhost\nexport ATTACK=1".into();
        let error = generate_config_block(CliProxyShell::Bash, &config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot contain newlines"));

        config.no_proxy = format!("localhost,{START_MARKER}");
        assert!(generate_config_block(CliProxyShell::Bash, &config).is_err());

        let duplicate = format!("{START_MARKER}\na\n{END_MARKER}\n{START_MARKER}\nb\n{END_MARKER}");
        let error = remove_marked_block(&duplicate).unwrap_err().to_string();
        assert!(error.contains("duplicate"));
    }

    #[test]
    fn certificate_bundle_and_config_validation_cover_error_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.pem");
        assert!(create_combined_ca_bundle(&missing, &temp.path().join("out.pem")).is_err());

        let invalid_ca = temp.path().join("invalid.pem");
        std::fs::write(&invalid_ca, "not a certificate").unwrap();
        assert!(write_combined_ca_bundle(
            &invalid_ca,
            &temp.path().join("invalid-out.pem"),
            &[b"root"]
        )
        .is_err());

        let valid_ca = temp.path().join("valid.pem");
        std::fs::write(
            &valid_ca,
            "-----BEGIN CERTIFICATE-----\nY2E=\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let no_roots = temp.path().join("nested/no-roots.pem");
        write_combined_ca_bundle(&valid_ca, &no_roots, &[]).unwrap();
        assert!(std::fs::read_to_string(no_roots).unwrap().starts_with('\n'));

        let manager = CliProxyEnvironmentManager::with_paths(
            CliProxyShell::Bash,
            vec![temp.path().join(".bashrc")],
        );
        let mut config = test_config(&temp);
        std::fs::remove_file(&config.ca_file).unwrap();
        assert!(manager
            .enable(&config)
            .unwrap_err()
            .to_string()
            .contains("CA file"));

        config = test_config(&temp);
        std::fs::remove_file(&config.ca_bundle).unwrap();
        assert!(manager
            .enable(&config)
            .unwrap_err()
            .to_string()
            .contains("Combined CA bundle"));
    }

    #[test]
    fn disable_all_managed_covers_every_shell_and_preserves_lifecycle_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let standalone = format!("{START_MARKER}\nmanaged\n{END_MARKER}");
        let lifecycle =
            "# >>> Bifrost proxy start >>>\nexport HTTP_PROXY=old\n# <<< Bifrost proxy end <<<";
        let bash = home.join(".bashrc");
        let fish = home.join(".config/fish/config.fish");
        let powershell = CliProxyShell::PowerShell.config_paths_for_home(home);
        std::fs::create_dir_all(fish.parent().unwrap()).unwrap();
        std::fs::write(&bash, format!("bash-before\n{standalone}\n{lifecycle}\n")).unwrap();
        std::fs::write(&fish, format!("fish-before\n{standalone}\n")).unwrap();
        for path in &powershell {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("pwsh-before\n{standalone}\n")).unwrap();
        }

        let changed = CliProxyEnvironmentManager::disable_all_managed_for_home(home).unwrap();

        assert_eq!(changed.len(), 2 + powershell.len());
        assert_eq!(
            std::fs::read_to_string(&bash).unwrap(),
            format!("bash-before\n{lifecycle}\n")
        );
        assert_eq!(std::fs::read_to_string(&fish).unwrap(), "fish-before");
        for path in powershell {
            assert_eq!(std::fs::read_to_string(path).unwrap(), "pwsh-before");
        }
    }

    #[test]
    fn disable_all_managed_preflights_every_profile_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let bash = home.join(".bashrc");
        let original = format!("before\n{START_MARKER}\nmanaged\n{END_MARKER}\nafter\n");
        std::fs::write(&bash, &original).unwrap();
        let invalid_profile = CliProxyShell::PowerShell
            .config_paths_for_home(home)
            .into_iter()
            .next()
            .unwrap();
        std::fs::create_dir_all(&invalid_profile).unwrap();

        let error = CliProxyEnvironmentManager::disable_all_managed_for_home(home)
            .unwrap_err()
            .to_string();

        assert!(error.contains("not a regular file"));
        assert_eq!(std::fs::read_to_string(bash).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_ca_path_is_rejected_before_shell_rendering() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![b'c', b'a', 0xff]));
        let error = path_environment_value("CA file", &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not valid UTF-8"));
    }
}
