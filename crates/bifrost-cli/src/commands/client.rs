use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use bifrost_core::{BifrostError, Result};
use clap::{error::ErrorKind, Parser};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::cli::{Cli, ClientInvocation, ClientTargetCli, ClientTargetCommands, Commands};
use crate::commands::config::client::ConfigApiClient;

pub const CLIENT_BASE_URL_ENV: &str = "BIFROST_INTERNAL_CLIENT_BASE_URL";
pub const CLIENT_TOKEN_ENV: &str = "BIFROST_INTERNAL_CLIENT_TOKEN";
pub const CLIENT_TARGET_NAME_ENV: &str = "BIFROST_INTERNAL_CLIENT_TARGET_NAME";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientTarget {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_admin_username")]
    pub username: String,
    #[serde(default)]
    pub allow_insecure_http: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct TargetStore {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    targets: Vec<ClientTarget>,
}

impl Default for TargetStore {
    fn default() -> Self {
        Self {
            version: store_version(),
            targets: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CredentialStore {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    credentials: BTreeMap<String, SavedCredential>,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self {
            version: store_version(),
            credentials: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedCredential {
    token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    token: String,
    expires_at: String,
    username: String,
}

fn store_version() -> u32 {
    1
}

fn default_admin_username() -> String {
    "admin".to_string()
}

fn client_dir() -> PathBuf {
    bifrost_storage::data_dir().join("cli")
}

fn targets_path() -> PathBuf {
    client_dir().join("admin-targets.toml")
}

fn credentials_path() -> PathBuf {
    client_dir().join("admin-credentials.toml")
}

fn read_toml<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = fs::read_to_string(path)?;
    toml::from_str(&content).map_err(|error| {
        BifrostError::Config(format!("failed to parse {}: {error}", path.display()))
    })
}

fn atomic_write_private(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        BifrostError::Config(format!("invalid client state path: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn save_targets(store: &TargetStore) -> Result<()> {
    let content = toml::to_string_pretty(store)
        .map_err(|error| BifrostError::Config(format!("failed to encode targets: {error}")))?;
    atomic_write_private(&targets_path(), &content)
}

fn save_credentials(store: &CredentialStore) -> Result<()> {
    let content = toml::to_string_pretty(store)
        .map_err(|error| BifrostError::Config(format!("failed to encode credentials: {error}")))?;
    atomic_write_private(&credentials_path(), &content)
}

pub fn normalize_base_url(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BifrostError::Config(
            "target URL cannot be empty".to_string(),
        ));
    }
    let had_explicit_scheme = trimmed.contains("://");
    let candidate = if had_explicit_scheme {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let mut url = Url::parse(&candidate)
        .map_err(|error| BifrostError::Config(format!("invalid target URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(BifrostError::Config(
            "target URL must use http or https".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(BifrostError::Config(
            "target URL must not contain credentials".to_string(),
        ));
    }
    if url.fragment().is_some() || url.query().is_some() {
        return Err(BifrostError::Config(
            "target URL must not contain a query or fragment".to_string(),
        ));
    }
    if url.host_str().is_none() {
        return Err(BifrostError::Config("target URL has no host".to_string()));
    }
    if !had_explicit_scheme && url.port().is_none() {
        url.set_port(Some(9900)).map_err(|_| {
            BifrostError::Config("target URL does not support an explicit port".to_string())
        })?;
    }
    let path = url.path().trim_end_matches('/');
    if !path.is_empty() && path != "/_bifrost" && path != "/_bifrost/api" {
        return Err(BifrostError::Config(
            "target URL path must be empty, /_bifrost, or /_bifrost/api".to_string(),
        ));
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    let mut normalized = url.to_string();
    normalized.truncate(normalized.trim_end_matches('/').len());
    Ok(normalized)
}

fn find_target<'a>(store: &'a TargetStore, selector: &str) -> Result<&'a ClientTarget> {
    let by_name = store
        .targets
        .iter()
        .find(|target| target.name.eq_ignore_ascii_case(selector));
    if let Some(target) = by_name {
        return Ok(target);
    }
    let normalized = normalize_base_url(selector)?;
    store
        .targets
        .iter()
        .find(|target| target.base_url == normalized)
        .ok_or_else(|| BifrostError::NotFound(format!("client target '{selector}'")))
}

fn select_target(store: &TargetStore, explicit: Option<&str>) -> Result<ClientTarget> {
    let selector = explicit
        .map(str::to_string)
        .or_else(|| std::env::var("BIFROST_CLIENT_TARGET").ok());
    if let Some(selector) = selector {
        if let Ok(target) = find_target(store, &selector) {
            return Ok(target.clone());
        }
        return Ok(ClientTarget {
            id: format!("temporary:{}", normalize_base_url(&selector)?),
            name: selector.clone(),
            base_url: normalize_base_url(&selector)?,
            username: default_admin_username(),
            allow_insecure_http: true,
        });
    }
    match store.targets.as_slice() {
        [] => Err(BifrostError::Config(
            "no client targets configured; run `bifrost client target add <name> --url <url>`"
                .to_string(),
        )),
        [target] => Ok(target.clone()),
        targets if io::stdin().is_terminal() && io::stderr().is_terminal() => {
            let labels = targets
                .iter()
                .map(|target| format!("{} ({})", target.name, target.base_url))
                .collect::<Vec<_>>();
            let index = dialoguer::Select::new()
                .with_prompt("Select Bifrost target")
                .items(&labels)
                .default(0)
                .interact()
                .map_err(|error| {
                    BifrostError::Config(format!("target selection failed: {error}"))
                })?;
            Ok(targets[index].clone())
        }
        targets => Err(BifrostError::Config(format!(
            "multiple client targets configured; pass --target. Available targets: {}",
            targets
                .iter()
                .map(|target| target.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn credential_for(target: &ClientTarget, explicit_target: bool) -> Result<String> {
    if explicit_target {
        if let Ok(token) = std::env::var("BIFROST_ADMIN_TOKEN") {
            if !token.trim().is_empty() {
                return Ok(token);
            }
        }
    }
    if target.id.starts_with("temporary:") {
        return Err(BifrostError::Config(
            "a temporary target requires an explicit --target and BIFROST_ADMIN_TOKEN".to_string(),
        ));
    }
    let store: CredentialStore = read_toml(&credentials_path())?;
    let credential = store.credentials.get(&target.id).ok_or_else(|| {
        BifrostError::Config(format!(
            "target '{}' is not logged in; run `bifrost client target login {}`",
            target.name, target.name
        ))
    })?;
    if chrono::DateTime::parse_from_rfc3339(&credential.expires_at)
        .map(|expires| expires <= chrono::Utc::now())
        .unwrap_or(true)
    {
        return Err(BifrostError::Config(format!(
            "saved session for target '{}' has expired; log in again",
            target.name
        )));
    }
    Ok(credential.token.clone())
}

fn target_command(args: Vec<OsString>) -> Result<()> {
    let cli = match ClientTargetCli::try_parse_from(
        std::iter::once(OsString::from("bifrost client target")).chain(args),
    ) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            error
                .print()
                .map_err(|print_error| BifrostError::Config(print_error.to_string()))?;
            return Ok(());
        }
        Err(error) => return Err(BifrostError::Config(error.to_string())),
    };
    let mut targets: TargetStore = read_toml(&targets_path())?;
    match cli.action {
        ClientTargetCommands::Add {
            name,
            url,
            allow_insecure_http,
        } => {
            if name.trim().is_empty() || name.contains(char::is_whitespace) {
                return Err(BifrostError::Config(
                    "target name must be non-empty and contain no whitespace".to_string(),
                ));
            }
            if targets
                .targets
                .iter()
                .any(|target| target.name.eq_ignore_ascii_case(&name))
            {
                return Err(BifrostError::AlreadyExists(format!(
                    "client target '{name}'"
                )));
            }
            let base_url = normalize_base_url(&url)?;
            if base_url.starts_with("http://") && !allow_insecure_http {
                return Err(BifrostError::Config(
                    "plain HTTP exposes the Admin password and token; pass --allow-insecure-http for a trusted LAN or use HTTPS".to_string(),
                ));
            }
            targets.targets.push(ClientTarget {
                id: Uuid::new_v4().to_string(),
                name: name.clone(),
                base_url: base_url.clone(),
                username: default_admin_username(),
                allow_insecure_http,
            });
            targets
                .targets
                .sort_by_key(|target| target.name.to_lowercase());
            save_targets(&targets)?;
            println!("Added client target '{name}' ({base_url}).");
        }
        ClientTargetCommands::List => {
            let credentials: CredentialStore = read_toml(&credentials_path())?;
            for target in &targets.targets {
                let state = credentials
                    .credentials
                    .get(&target.id)
                    .map(|credential| format!("logged in until {}", credential.expires_at))
                    .unwrap_or_else(|| "not logged in".to_string());
                println!(
                    "{}\t{}\t{}\t{}",
                    target.name, target.base_url, target.username, state
                );
            }
        }
        ClientTargetCommands::Show { name } => {
            let target = find_target(&targets, &name)?;
            let credentials: CredentialStore = read_toml(&credentials_path())?;
            let expires_at = credentials
                .credentials
                .get(&target.id)
                .map(|credential| credential.expires_at.as_str());
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "name": target.name,
                    "url": target.base_url,
                    "username": target.username,
                    "allow_insecure_http": target.allow_insecure_http,
                    "logged_in": expires_at.is_some(),
                    "expires_at": expires_at,
                }))?
            );
        }
        ClientTargetCommands::Login {
            name,
            username,
            password_stdin,
        } => {
            let index = targets
                .targets
                .iter()
                .position(|target| target.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| BifrostError::NotFound(format!("client target '{name}'")))?;
            let password = if password_stdin {
                let mut password = String::new();
                io::stdin().read_to_string(&mut password)?;
                password.trim_end_matches(['\r', '\n']).to_string()
            } else if io::stdin().is_terminal() {
                dialoguer::Password::new()
                    .with_prompt(format!("Admin password for {name}"))
                    .interact()
                    .map_err(|error| {
                        BifrostError::Config(format!("password input failed: {error}"))
                    })?
            } else {
                return Err(BifrostError::Config(
                    "non-interactive login requires --password-stdin".to_string(),
                ));
            };
            let target = &targets.targets[index];
            let client = ConfigApiClient::from_base_url(&target.base_url, None)
                .map_err(BifrostError::Config)?;
            let status: serde_json::Value = client.get_public("/auth/status").map_err(|error| {
                if error.contains("HTTP 403") {
                    BifrostError::Config(
                        "remote Admin access is disabled or denied; enable it locally on the target with `bifrost admin remote enable`".to_string(),
                    )
                } else {
                    BifrostError::Network(error)
                }
            })?;
            if status
                .get("remote_access_enabled")
                .and_then(|v| v.as_bool())
                != Some(true)
            {
                return Err(BifrostError::Config(
                    "remote Admin access is disabled; enable it locally on the target with `bifrost admin remote enable`".to_string(),
                ));
            }
            let login: LoginResponse = client
                .post_public(
                    "/auth/login",
                    &serde_json::json!({"username": username, "password": password}),
                )
                .map_err(BifrostError::Network)?;
            let mut credentials: CredentialStore = read_toml(&credentials_path())?;
            credentials.credentials.insert(
                target.id.clone(),
                SavedCredential {
                    token: login.token,
                    expires_at: login.expires_at.clone(),
                },
            );
            save_credentials(&credentials)?;
            targets.targets[index].username = login.username;
            save_targets(&targets)?;
            println!("Logged in to '{name}' until {}.", login.expires_at);
        }
        ClientTargetCommands::Logout { name } => {
            let target = find_target(&targets, &name)?;
            let mut credentials: CredentialStore = read_toml(&credentials_path())?;
            credentials.credentials.remove(&target.id);
            save_credentials(&credentials)?;
            println!("Logged out from client target '{}'.", target.name);
        }
        ClientTargetCommands::Rename { name, new_name } => {
            if new_name.trim().is_empty() || new_name.contains(char::is_whitespace) {
                return Err(BifrostError::Config(
                    "target name must be non-empty and contain no whitespace".to_string(),
                ));
            }
            if targets
                .targets
                .iter()
                .any(|target| target.name.eq_ignore_ascii_case(&new_name))
            {
                return Err(BifrostError::AlreadyExists(format!(
                    "client target '{new_name}'"
                )));
            }
            let target = targets
                .targets
                .iter_mut()
                .find(|target| target.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| BifrostError::NotFound(format!("client target '{name}'")))?;
            target.name = new_name.clone();
            save_targets(&targets)?;
            println!("Renamed client target '{name}' to '{new_name}'.");
        }
        ClientTargetCommands::Remove { name } => {
            let target = find_target(&targets, &name)?.clone();
            targets
                .targets
                .retain(|candidate| candidate.id != target.id);
            save_targets(&targets)?;
            let mut credentials: CredentialStore = read_toml(&credentials_path())?;
            credentials.credentials.remove(&target.id);
            save_credentials(&credentials)?;
            println!("Removed client target '{}'.", target.name);
        }
    }
    Ok(())
}

fn validate_client_command(command: &Commands) -> Result<()> {
    let unsupported = match command {
        Commands::Status { tui: false, .. }
        | Commands::Rule { .. }
        | Commands::Group { .. }
        | Commands::Port { .. }
        | Commands::Whitelist { .. }
        | Commands::Account { .. }
        | Commands::Value { .. }
        | Commands::Script { .. }
        | Commands::Config { .. }
        | Commands::Admin { .. }
        | Commands::Traffic { .. }
        | Commands::Capture { .. }
        | Commands::Search { .. }
        | Commands::Metrics { .. }
        | Commands::Login { .. }
        | Commands::Sync { .. }
        | Commands::Import { .. }
        | Commands::Export { .. }
        | Commands::VersionCheck => None,
        Commands::Status { tui: true, .. } => {
            Some("the interactive status TUI is not available over Client transport yet")
        }
        Commands::Client(_) => Some("nested client mode is not allowed"),
        Commands::Remote { .. } => {
            Some("Remote Invoke is a separate transport; use `bifrost remote` directly")
        }
        Commands::Start { .. } | Commands::Stop | Commands::Restart { .. } => Some(
            "service lifecycle commands require local access or an explicit Remote Invoke workflow",
        ),
        Commands::CliProxy { .. }
        | Commands::SystemProxy { .. }
        | Commands::KeepAwake { .. }
        | Commands::Upgrade { .. }
        | Commands::Completions { .. }
        | Commands::InstallSkill { .. }
        | Commands::App { .. }
        | Commands::Ca { .. } => Some("this command changes the client machine and is local-only"),
        Commands::SelfUpdate { .. }
        | Commands::WindowsUpgradeHandoff { .. }
        | Commands::AsrDiarizationWorker { .. }
        | Commands::AppIconWorker { .. }
        | Commands::AuxiliaryWorker { .. } => Some("internal worker commands are local-only"),
        Commands::Setting { .. } => Some("Remote Invoke settings are not part of Client mode"),
        Commands::Ai { .. } | Commands::Im { .. } | Commands::Agent { .. } => {
            Some("this management surface has not been migrated to Client transport yet")
        }
    };
    if let Some(reason) = unsupported {
        return Err(BifrostError::Config(format!(
            "command is not supported in Client mode: {reason}"
        )));
    }
    Ok(())
}

pub fn handle_client(invocation: ClientInvocation) -> Result<()> {
    let mut args = invocation.args.into_iter();
    let first = args
        .next()
        .ok_or_else(|| BifrostError::Config("client requires a command".to_string()))?;
    if first == "target" {
        if invocation.target.is_some() {
            return Err(BifrostError::Config(
                "client target management does not accept --target".to_string(),
            ));
        }
        return target_command(args.collect());
    }

    let business_args = std::iter::once(first).chain(args).collect::<Vec<_>>();
    let parsed = match Cli::try_parse_from(
        std::iter::once(OsString::from("bifrost")).chain(business_args.iter().cloned()),
    ) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            error
                .print()
                .map_err(|print_error| BifrostError::Config(print_error.to_string()))?;
            return Ok(());
        }
        Err(error) => return Err(BifrostError::Config(error.to_string())),
    };
    let command = parsed.command.as_ref().ok_or_else(|| {
        BifrostError::Config("client requires an existing Bifrost command".to_string())
    })?;
    validate_client_command(command)?;

    let targets: TargetStore = read_toml(&targets_path())?;
    let explicit_target = invocation.target.is_some();
    let target = select_target(&targets, invocation.target.as_deref())?;
    let token = credential_for(&target, explicit_target)?;
    if io::stderr().is_terminal() {
        eprintln!("Target: {} ({})", target.name, target.base_url);
    }

    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args(business_args)
        .env(CLIENT_BASE_URL_ENV, &target.base_url)
        .env(CLIENT_TOKEN_ENV, token)
        .env(CLIENT_TARGET_NAME_ENV, &target.name);
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

pub fn active_base_url() -> Option<String> {
    std::env::var(CLIENT_BASE_URL_ENV).ok()
}

pub fn active_token() -> Option<String> {
    std::env::var(CLIENT_TOKEN_ENV).ok()
}

pub fn is_active() -> bool {
    active_base_url().is_some()
}

pub fn api_url(local_port: u16, path: &str) -> String {
    if let Some(base_url) = active_base_url() {
        format!("{}/_bifrost/api{}", base_url.trim_end_matches('/'), path)
    } else {
        format!("http://127.0.0.1:{local_port}/_bifrost/api{path}")
    }
}

pub fn authenticated_request(agent: &ureq::Agent, method: &str, url: &str) -> ureq::Request {
    let request = agent.request(method, url);
    match active_token() {
        Some(token) => request.set("Authorization", &format!("Bearer {token}")),
        None => request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str, base_url: &str) -> ClientTarget {
        ClientTarget {
            id: format!("id-{name}"),
            name: name.to_string(),
            base_url: base_url.to_string(),
            username: default_admin_username(),
            allow_insecure_http: base_url.starts_with("http://"),
        }
    }

    #[test]
    fn client_admin_cli_unit_boundaries_fail_closed() {
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("://").is_err());
        assert!(normalize_base_url("ftp://example.com").is_err());
        assert!(normalize_base_url("http://user@example.com").is_err());
        assert!(normalize_base_url("http://example.com?secret=1").is_err());
        assert!(normalize_base_url("http://example.com/other").is_err());

        let only = target("Lab", "http://192.0.2.10:8800");
        let single = TargetStore {
            version: 1,
            targets: vec![only.clone()],
        };
        assert_eq!(find_target(&single, "lab").unwrap(), &only);
        assert_eq!(
            find_target(&single, "http://192.0.2.10:8800").unwrap(),
            &only
        );
        assert!(find_target(&single, "missing.example").is_err());
        assert_eq!(select_target(&single, None).unwrap(), only);

        let empty = TargetStore::default();
        assert!(select_target(&empty, None).is_err());
        let temporary = select_target(&empty, Some("192.0.2.20:8811")).unwrap();
        assert!(temporary.id.starts_with("temporary:"));
        assert!(credential_for(&temporary, false).is_err());

        let multiple = TargetStore {
            version: 1,
            targets: vec![
                target("one", "http://192.0.2.1:8800"),
                target("two", "http://192.0.2.2:8800"),
            ],
        };
        assert!(select_target(&multiple, None).is_err());

        assert!(handle_client(ClientInvocation {
            target: None,
            args: Vec::new(),
        })
        .is_err());
        assert!(handle_client(ClientInvocation {
            target: Some("one".to_string()),
            args: vec![OsString::from("target"), OsString::from("list")],
        })
        .is_err());
        assert!(handle_client(ClientInvocation {
            target: None,
            args: vec![OsString::from("definitely-not-a-command")],
        })
        .is_err());
        assert!(target_command(vec![OsString::from("--definitely-invalid")]).is_err());

        assert_eq!(
            api_url(8812, "/traffic"),
            "http://127.0.0.1:8812/_bifrost/api/traffic"
        );
        let agent = bifrost_core::direct_ureq_agent_builder().build();
        let request = authenticated_request(&agent, "GET", "http://127.0.0.1:9/");
        assert_eq!(request.header("Authorization"), None);
    }

    #[test]
    fn normalizes_supported_target_forms() {
        assert_eq!(
            normalize_base_url("10.0.0.8").unwrap(),
            "http://10.0.0.8:9900"
        );
        assert_eq!(
            normalize_base_url("10.0.0.8:8800").unwrap(),
            "http://10.0.0.8:8800"
        );
        assert_eq!(
            normalize_base_url("https://example.com/_bifrost/api/").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_base_url("http://[::1]:8800").unwrap(),
            "http://[::1]:8800"
        );
        assert_eq!(normalize_base_url("[::1]").unwrap(), "http://[::1]:9900");
    }

    #[test]
    fn rejects_unsafe_url_components() {
        assert!(normalize_base_url("ftp://example.com").is_err());
        assert!(normalize_base_url("http://user:pass@example.com").is_err());
        assert!(normalize_base_url("http://example.com/path").is_err());
        assert!(normalize_base_url("http://example.com#fragment").is_err());
    }

    #[test]
    fn new_stores_use_the_current_schema_version() {
        assert_eq!(TargetStore::default().version, 1);
        assert_eq!(CredentialStore::default().version, 1);
    }

    #[cfg(unix)]
    #[test]
    fn private_store_write_is_atomic_and_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cli").join("targets.toml");
        atomic_write_private(&path, "version = 1\n").unwrap();
        atomic_write_private(&path, "version = 2\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "version = 2\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn rejects_client_and_remote_nesting() {
        let client = Cli::try_parse_from(["bifrost", "client", "status"]).unwrap();
        assert!(matches!(client.command, Some(Commands::Client(_))));
        let remote = Cli::try_parse_from(["bifrost", "remote", "conn", "status"]).unwrap();
        assert!(validate_client_command(remote.command.as_ref().unwrap()).is_err());
    }

    #[test]
    fn capability_registry_accepts_admin_commands_and_rejects_local_commands() {
        let supported = [
            vec!["bifrost", "status"],
            vec!["bifrost", "traffic", "list"],
            vec!["bifrost", "rule", "list"],
            vec!["bifrost", "value", "list"],
            vec!["bifrost", "script", "list"],
            vec!["bifrost", "config", "show"],
            vec!["bifrost", "whitelist", "status"],
            vec!["bifrost", "account", "list"],
            vec!["bifrost", "metrics", "summary"],
        ];
        for argv in supported {
            let parsed = Cli::try_parse_from(argv.clone()).unwrap();
            assert!(
                validate_client_command(parsed.command.as_ref().unwrap()).is_ok(),
                "expected Client support for {argv:?}"
            );
        }

        let unsupported = [
            vec!["bifrost", "start"],
            vec!["bifrost", "stop"],
            vec!["bifrost", "ca", "info"],
            vec!["bifrost", "system-proxy", "status"],
            vec!["bifrost", "remote", "conn", "status"],
            vec!["bifrost", "setting", "grant", "list"],
        ];
        for argv in unsupported {
            let parsed = Cli::try_parse_from(argv.clone()).unwrap();
            assert!(
                validate_client_command(parsed.command.as_ref().unwrap()).is_err(),
                "expected Client rejection for {argv:?}"
            );
        }
    }

    #[test]
    fn unsupported_top_level_commands_are_rejected_before_dispatch() {
        let accepted = Cli::try_parse_from(["bifrost", "client", "status"]).unwrap();
        assert!(matches!(accepted.command, Some(Commands::Client(_))));

        for argv in [
            vec!["bifrost", "status", "--tui"],
            vec!["bifrost", "ai", "--help"],
            vec!["bifrost", "im", "--help"],
            vec!["bifrost", "agent", "--help"],
        ] {
            match Cli::try_parse_from(argv.clone()) {
                Ok(parsed) => assert!(
                    validate_client_command(parsed.command.as_ref().unwrap()).is_err(),
                    "expected an explicit Client rejection for {argv:?}"
                ),
                Err(error) if error.kind() == ErrorKind::DisplayHelp => {}
                Err(error) => panic!("unexpected parse error for {argv:?}: {error}"),
            }
        }
    }
}
