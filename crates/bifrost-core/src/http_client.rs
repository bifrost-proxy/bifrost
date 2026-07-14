use std::collections::HashSet;
use std::error::Error;
use std::path::{Path, PathBuf};

pub const BIFROST_CA_BUNDLE_ENV: &str = "BIFROST_CA_BUNDLE";
pub const BIFROST_CA_DIR_ENV: &str = "BIFROST_CA_DIR";
pub const BIFROST_UNSAFE_SSL_ENV: &str = "BIFROST_UNSAFE_SSL";
pub const GITHUB_CA_BUNDLE_ENV: &str = "BIFROST_GITHUB_CA_BUNDLE";
pub const GITHUB_CA_DIR_ENV: &str = "BIFROST_GITHUB_CA_DIR";
pub const GITHUB_UNSAFE_SSL_ENV: &str = "BIFROST_GITHUB_UNSAFE_SSL";
pub const REMOTE_RELAY_CA_BUNDLE_ENV: &str = "BIFROST_REMOTE_RELAY_CA_BUNDLE";
pub const REMOTE_RELAY_HEADERS_ENV: &str = "BIFROST_REMOTE_RELAY_HEADERS";
pub const REMOTE_UNSAFE_SSL_ENV: &str = "BIFROST_REMOTE_UNSAFE_SSL";
pub const UPGRADE_CA_BUNDLE_ENV: &str = "BIFROST_UPGRADE_CA_BUNDLE";
pub const UPGRADE_CA_DIR_ENV: &str = "BIFROST_UPGRADE_CA_DIR";
pub const UPGRADE_UNSAFE_SSL_ENV: &str = "BIFROST_UPGRADE_UNSAFE_SSL";
#[cfg(test)]
const COMMON_CA_FILE_ENVS: &[&str] = &[
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "PIP_CERT",
    "NPM_CONFIG_CAFILE",
    "npm_config_cafile",
    "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
];
#[cfg(test)]
const COMMON_CA_DIR_ENVS: &[&str] = &["SSL_CERT_DIR"];

#[derive(Debug, Clone, Copy)]
struct TlsTrustProfile {
    name: &'static str,
    ca_file_envs: &'static [&'static str],
    ca_dir_envs: &'static [&'static str],
    unsafe_ssl_envs: &'static [&'static str],
}

const REMOTE_RELAY_CA_FILE_ENVS: &[&str] = &[
    REMOTE_RELAY_CA_BUNDLE_ENV,
    BIFROST_CA_BUNDLE_ENV,
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "PIP_CERT",
    "NPM_CONFIG_CAFILE",
    "npm_config_cafile",
    "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
];
const REMOTE_RELAY_CA_DIR_ENVS: &[&str] = &[BIFROST_CA_DIR_ENV, "SSL_CERT_DIR"];
const REMOTE_RELAY_UNSAFE_SSL_ENVS: &[&str] = &[REMOTE_UNSAFE_SSL_ENV, BIFROST_UNSAFE_SSL_ENV];

const GITHUB_CA_FILE_ENVS: &[&str] = &[
    GITHUB_CA_BUNDLE_ENV,
    UPGRADE_CA_BUNDLE_ENV,
    BIFROST_CA_BUNDLE_ENV,
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "PIP_CERT",
    "NPM_CONFIG_CAFILE",
    "npm_config_cafile",
    "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
];
const GITHUB_CA_DIR_ENVS: &[&str] = &[
    GITHUB_CA_DIR_ENV,
    UPGRADE_CA_DIR_ENV,
    BIFROST_CA_DIR_ENV,
    "SSL_CERT_DIR",
];
const GITHUB_UNSAFE_SSL_ENVS: &[&str] = &[
    GITHUB_UNSAFE_SSL_ENV,
    UPGRADE_UNSAFE_SSL_ENV,
    BIFROST_UNSAFE_SSL_ENV,
];
const OUTBOUND_CA_FILE_ENVS: &[&str] = &[
    BIFROST_CA_BUNDLE_ENV,
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "PIP_CERT",
    "NPM_CONFIG_CAFILE",
    "npm_config_cafile",
    "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
];
const OUTBOUND_CA_DIR_ENVS: &[&str] = &[BIFROST_CA_DIR_ENV, "SSL_CERT_DIR"];
const OUTBOUND_UNSAFE_SSL_ENVS: &[&str] = &[BIFROST_UNSAFE_SSL_ENV];

const REMOTE_RELAY_TRUST_PROFILE: TlsTrustProfile = TlsTrustProfile {
    name: "remote relay",
    ca_file_envs: REMOTE_RELAY_CA_FILE_ENVS,
    ca_dir_envs: REMOTE_RELAY_CA_DIR_ENVS,
    unsafe_ssl_envs: REMOTE_RELAY_UNSAFE_SSL_ENVS,
};

const GITHUB_TRUST_PROFILE: TlsTrustProfile = TlsTrustProfile {
    name: "GitHub",
    ca_file_envs: GITHUB_CA_FILE_ENVS,
    ca_dir_envs: GITHUB_CA_DIR_ENVS,
    unsafe_ssl_envs: GITHUB_UNSAFE_SSL_ENVS,
};

const OUTBOUND_TRUST_PROFILE: TlsTrustProfile = TlsTrustProfile {
    name: "outbound",
    ca_file_envs: OUTBOUND_CA_FILE_ENVS,
    ca_dir_envs: OUTBOUND_CA_DIR_ENVS,
    unsafe_ssl_envs: OUTBOUND_UNSAFE_SSL_ENVS,
};

pub fn direct_reqwest_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_proxy()
}

pub fn direct_sse_reqwest_client_builder() -> reqwest::ClientBuilder {
    direct_reqwest_client_builder()
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
}

pub fn direct_blocking_reqwest_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder().no_proxy()
}

pub fn remote_relay_reqwest_client_builder() -> reqwest::ClientBuilder {
    trusted_reqwest_client_builder(REMOTE_RELAY_TRUST_PROFILE)
}

pub fn remote_relay_sse_reqwest_client_builder() -> reqwest::ClientBuilder {
    remote_relay_reqwest_client_builder()
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
}

pub fn github_reqwest_client_builder() -> reqwest::ClientBuilder {
    trusted_reqwest_client_builder(GITHUB_TRUST_PROFILE)
}

pub fn github_blocking_reqwest_client_builder() -> reqwest::blocking::ClientBuilder {
    trusted_blocking_reqwest_client_builder(GITHUB_TRUST_PROFILE)
}

pub fn outbound_reqwest_client_builder() -> reqwest::ClientBuilder {
    trusted_reqwest_client_builder(OUTBOUND_TRUST_PROFILE)
}

pub fn outbound_blocking_reqwest_client_builder() -> reqwest::blocking::ClientBuilder {
    trusted_blocking_reqwest_client_builder(OUTBOUND_TRUST_PROFILE)
}

pub fn load_reqwest_certificate(path: &Path) -> std::result::Result<reqwest::Certificate, String> {
    let pem = std::fs::read(path).map_err(|error| format!("read CA certificate: {error}"))?;
    reqwest::Certificate::from_pem(&pem).map_err(|error| format!("parse CA certificate: {error}"))
}

pub fn load_reqwest_certificate_bundle(
    path: &Path,
) -> std::result::Result<Vec<reqwest::Certificate>, String> {
    let pem = std::fs::read(path).map_err(|error| format!("read CA certificate: {error}"))?;
    reqwest::Certificate::from_pem_bundle(&pem)
        .map_err(|error| format!("parse CA certificate bundle: {error}"))
}

pub fn format_reqwest_error(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        let detail = error.to_string();
        if !message.contains(&detail) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        source = error.source();
    }
    message
}

pub fn proxied_reqwest_client_builder(
    proxy_url: &str,
    ca_cert_path: Option<&Path>,
) -> std::result::Result<reqwest::ClientBuilder, String> {
    let proxy = reqwest::Proxy::all(proxy_url)
        .map_err(|error| format!("invalid proxy URL '{proxy_url}': {error}"))?;
    let mut builder = direct_reqwest_client_builder().proxy(proxy);
    if let Some(path) = ca_cert_path {
        match load_reqwest_certificate(path) {
            Ok(cert) => {
                builder = builder.add_root_certificate(cert);
            }
            Err(error) => {
                tracing::warn!(
                    proxy_url = %proxy_url,
                    ca_cert_path = %path.display(),
                    error = %error,
                    "proxied HTTP client could not load CA; TLS-intercepted HTTPS requests may fail"
                );
            }
        }
    }
    Ok(builder)
}

pub fn direct_ureq_agent_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().try_proxy_from_env(false)
}

pub fn direct_ureq_agent() -> ureq::Agent {
    direct_ureq_agent_builder().build()
}

fn trusted_reqwest_client_builder(profile: TlsTrustProfile) -> reqwest::ClientBuilder {
    let builder = add_native_root_certificates(
        direct_reqwest_client_builder()
            .tls_built_in_webpki_certs(true)
            .tls_built_in_native_certs(false),
        profile,
    );
    let builder = add_extra_root_certificates(builder, profile);
    if unsafe_ssl_from_env(profile) {
        builder.danger_accept_invalid_certs(true)
    } else {
        builder
    }
}

fn trusted_blocking_reqwest_client_builder(
    profile: TlsTrustProfile,
) -> reqwest::blocking::ClientBuilder {
    let builder = add_blocking_native_root_certificates(
        direct_blocking_reqwest_client_builder()
            .tls_built_in_webpki_certs(true)
            .tls_built_in_native_certs(false),
        profile,
    );
    let builder = add_blocking_extra_root_certificates(builder, profile);
    if unsafe_ssl_from_env(profile) {
        builder.danger_accept_invalid_certs(true)
    } else {
        builder
    }
}

fn add_native_root_certificates(
    mut builder: reqwest::ClientBuilder,
    profile: TlsTrustProfile,
) -> reqwest::ClientBuilder {
    let certificates = crate::native_certificates_der();
    let certificates = parse_native_root_certificates(&certificates);
    let added = certificates.len();
    for certificate in certificates {
        builder = builder.add_root_certificate(certificate);
    }
    tracing::trace!(
        cert_count = added,
        trust_profile = profile.name,
        "added cached native certificates to HTTP client"
    );
    builder
}

fn add_blocking_native_root_certificates(
    mut builder: reqwest::blocking::ClientBuilder,
    profile: TlsTrustProfile,
) -> reqwest::blocking::ClientBuilder {
    let certificates = crate::native_certificates_der();
    let certificates = parse_native_root_certificates(&certificates);
    let added = certificates.len();
    for certificate in certificates {
        builder = builder.add_root_certificate(certificate);
    }
    tracing::trace!(
        cert_count = added,
        trust_profile = profile.name,
        "added cached native certificates to blocking HTTP client"
    );
    builder
}

fn parse_native_root_certificates(certificates_der: &[Vec<u8>]) -> Vec<reqwest::Certificate> {
    certificates_der
        .iter()
        .filter_map(|certificate_der| reqwest::Certificate::from_der(certificate_der).ok())
        .collect()
}

fn add_extra_root_certificates(
    mut builder: reqwest::ClientBuilder,
    profile: TlsTrustProfile,
) -> reqwest::ClientBuilder {
    for (source, path) in ca_file_paths_from_env(profile) {
        match load_reqwest_certificate_bundle(&path) {
            Ok(certs) => {
                let count = certs.len();
                for cert in certs {
                    builder = builder.add_root_certificate(cert);
                }
                tracing::debug!(
                    env = source,
                    ca_cert_path = %path.display(),
                    cert_count = count,
                    trust_profile = profile.name,
                    "loaded extra CA bundle"
                );
            }
            Err(error) => {
                tracing::warn!(
                    env = source,
                    ca_cert_path = %path.display(),
                    error = %error,
                    trust_profile = profile.name,
                    "HTTP client could not load extra CA bundle"
                );
            }
        }
    }

    for (source, dir) in ca_dir_paths_from_env(profile) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(
                    env = source,
                    ca_cert_dir = %dir.display(),
                    error = %error,
                    trust_profile = profile.name,
                    "HTTP client could not read extra CA directory"
                );
                continue;
            }
        };
        let mut paths = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            match load_reqwest_certificate_bundle(&path) {
                Ok(certs) => {
                    let count = certs.len();
                    for cert in certs {
                        builder = builder.add_root_certificate(cert);
                    }
                    tracing::debug!(
                        env = source,
                        ca_cert_path = %path.display(),
                        cert_count = count,
                        trust_profile = profile.name,
                        "loaded extra CA bundle from directory"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        env = source,
                        ca_cert_path = %path.display(),
                        error = %error,
                        trust_profile = profile.name,
                        "skipping non-PEM file in CA directory"
                    );
                }
            }
        }
    }

    builder
}

fn add_blocking_extra_root_certificates(
    mut builder: reqwest::blocking::ClientBuilder,
    profile: TlsTrustProfile,
) -> reqwest::blocking::ClientBuilder {
    for (source, path) in ca_file_paths_from_env(profile) {
        match load_reqwest_certificate_bundle(&path) {
            Ok(certs) => {
                let count = certs.len();
                for cert in certs {
                    builder = builder.add_root_certificate(cert);
                }
                tracing::debug!(
                    env = source,
                    ca_cert_path = %path.display(),
                    cert_count = count,
                    trust_profile = profile.name,
                    "loaded extra CA bundle"
                );
            }
            Err(error) => {
                tracing::warn!(
                    env = source,
                    ca_cert_path = %path.display(),
                    error = %error,
                    trust_profile = profile.name,
                    "HTTP client could not load extra CA bundle"
                );
            }
        }
    }

    for (source, dir) in ca_dir_paths_from_env(profile) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(
                    env = source,
                    ca_cert_dir = %dir.display(),
                    error = %error,
                    trust_profile = profile.name,
                    "HTTP client could not read extra CA directory"
                );
                continue;
            }
        };
        let mut paths = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            match load_reqwest_certificate_bundle(&path) {
                Ok(certs) => {
                    let count = certs.len();
                    for cert in certs {
                        builder = builder.add_root_certificate(cert);
                    }
                    tracing::debug!(
                        env = source,
                        ca_cert_path = %path.display(),
                        cert_count = count,
                        trust_profile = profile.name,
                        "loaded extra CA bundle from directory"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        env = source,
                        ca_cert_path = %path.display(),
                        error = %error,
                        trust_profile = profile.name,
                        "skipping non-PEM file in CA directory"
                    );
                }
            }
        }
    }

    builder
}

#[cfg(test)]
fn remote_relay_ca_file_paths_from_env() -> Vec<(String, PathBuf)> {
    ca_file_paths_from_env(REMOTE_RELAY_TRUST_PROFILE)
}

#[cfg(test)]
fn github_ca_file_paths_from_env() -> Vec<(String, PathBuf)> {
    ca_file_paths_from_env(GITHUB_TRUST_PROFILE)
}

#[cfg(test)]
fn outbound_ca_file_paths_from_env() -> Vec<(String, PathBuf)> {
    ca_file_paths_from_env(OUTBOUND_TRUST_PROFILE)
}

#[cfg(test)]
#[allow(dead_code)]
fn remote_relay_ca_dir_paths_from_env() -> Vec<(String, PathBuf)> {
    ca_dir_paths_from_env(REMOTE_RELAY_TRUST_PROFILE)
}

fn ca_file_paths_from_env(profile: TlsTrustProfile) -> Vec<(String, PathBuf)> {
    let mut paths = Vec::new();
    for env_name in profile.ca_file_envs {
        paths.extend(paths_from_env_var(env_name));
    }
    paths.extend(system_ca_file_paths());
    dedup_existing_paths(paths)
}

fn paths_from_env_var(env_name: &str) -> Vec<(String, PathBuf)> {
    let Some(raw) = std::env::var_os(env_name) else {
        return Vec::new();
    };
    std::env::split_paths(&raw)
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| (env_name.to_string(), path))
        .collect()
}

fn ca_dir_paths_from_env(profile: TlsTrustProfile) -> Vec<(String, PathBuf)> {
    let mut paths = Vec::new();
    for env_name in profile.ca_dir_envs {
        paths.extend(paths_from_env_var(env_name));
    }
    paths.extend(system_ca_dir_paths());
    dedup_existing_paths(paths)
}

fn system_ca_file_paths() -> Vec<(String, PathBuf)> {
    openssl_probe::probe()
        .cert_file
        .into_iter()
        .filter(|path| path.is_file())
        .map(|path| ("system-ca-probe".to_string(), path))
        .collect()
}

fn system_ca_dir_paths() -> Vec<(String, PathBuf)> {
    openssl_probe::probe()
        .cert_dir
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| ("system-ca-probe".to_string(), path))
        .collect()
}

fn dedup_existing_paths(paths: Vec<(String, PathBuf)>) -> Vec<(String, PathBuf)> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|(_, path)| path.exists())
        .filter(|(_, path)| seen.insert(path.clone()))
        .collect()
}

#[cfg(test)]
fn remote_relay_unsafe_ssl_from_env() -> bool {
    unsafe_ssl_from_env(REMOTE_RELAY_TRUST_PROFILE)
}

#[cfg(test)]
fn github_unsafe_ssl_from_env() -> bool {
    unsafe_ssl_from_env(GITHUB_TRUST_PROFILE)
}

#[cfg(test)]
fn outbound_unsafe_ssl_from_env() -> bool {
    unsafe_ssl_from_env(OUTBOUND_TRUST_PROFILE)
}

fn unsafe_ssl_from_env(profile: TlsTrustProfile) -> bool {
    for env_name in profile.unsafe_ssl_envs {
        let Some(raw) = std::env::var_os(env_name) else {
            continue;
        };
        let value = raw.to_string_lossy();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => {
                tracing::warn!(
                    env = *env_name,
                    trust_profile = profile.name,
                    "TLS certificate verification is disabled by environment"
                );
                return true;
            }
            "0" | "false" | "no" | "off" => return false,
            _ => {
                tracing::warn!(
                    env = *env_name,
                    value = trimmed,
                    trust_profile = profile.name,
                    "ignoring unrecognized unsafe SSL environment value"
                );
                continue;
            }
        }
    }
    false
}

pub fn remote_relay_headers_from_env() -> std::result::Result<reqwest::header::HeaderMap, String> {
    let raw = match std::env::var(REMOTE_RELAY_HEADERS_ENV) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(reqwest::header::HeaderMap::new()),
        Err(error) => return Err(format!("read {REMOTE_RELAY_HEADERS_ENV}: {error}")),
    };
    parse_remote_relay_headers(&raw)
}

pub fn apply_remote_relay_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &reqwest::header::HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    builder
}

pub fn parse_remote_relay_headers(
    raw: &str,
) -> std::result::Result<reqwest::header::HeaderMap, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (index, pair) in raw.split(',').enumerate() {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((name, value)) = pair.split_once('=') else {
            return Err(format!(
                "{REMOTE_RELAY_HEADERS_ENV} entry #{} must be name=value",
                index + 1
            ));
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            return Err(format!(
                "{REMOTE_RELAY_HEADERS_ENV} entry #{} has empty header name",
                index + 1
            ));
        }
        let header_name =
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                format!(
                    "{REMOTE_RELAY_HEADERS_ENV} entry #{} has invalid header name '{name}': {error}",
                    index + 1
                )
            })?;
        if is_restricted_remote_relay_header(&header_name) {
            return Err(format!(
                "{REMOTE_RELAY_HEADERS_ENV} entry #{} cannot set restricted header '{}'",
                index + 1,
                header_name
            ));
        }
        let header_value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
            format!(
                "{REMOTE_RELAY_HEADERS_ENV} entry #{} has invalid value for '{}': {error}",
                index + 1,
                header_name
            )
        })?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}

fn is_restricted_remote_relay_header(name: &reqwest::header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization" | "cookie" | "host" | "x-bifrost-token"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        apply_remote_relay_headers, direct_blocking_reqwest_client_builder,
        direct_reqwest_client_builder, direct_ureq_agent, direct_ureq_agent_builder,
        github_blocking_reqwest_client_builder, github_ca_file_paths_from_env,
        github_reqwest_client_builder, github_unsafe_ssl_from_env, load_reqwest_certificate,
        load_reqwest_certificate_bundle, outbound_blocking_reqwest_client_builder,
        outbound_ca_file_paths_from_env, outbound_reqwest_client_builder,
        outbound_unsafe_ssl_from_env, parse_native_root_certificates, parse_remote_relay_headers,
        proxied_reqwest_client_builder, remote_relay_ca_file_paths_from_env,
        remote_relay_reqwest_client_builder, remote_relay_unsafe_ssl_from_env,
        BIFROST_CA_BUNDLE_ENV, BIFROST_CA_DIR_ENV, BIFROST_UNSAFE_SSL_ENV, COMMON_CA_DIR_ENVS,
        COMMON_CA_FILE_ENVS, GITHUB_CA_BUNDLE_ENV, GITHUB_CA_DIR_ENV, GITHUB_UNSAFE_SSL_ENV,
        REMOTE_RELAY_CA_BUNDLE_ENV, REMOTE_RELAY_HEADERS_ENV, REMOTE_UNSAFE_SSL_ENV,
        UPGRADE_CA_BUNDLE_ENV, UPGRADE_CA_DIR_ENV, UPGRADE_UNSAFE_SSL_ENV,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::Duration;

    fn proxy_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn cached_native_certificate_der_is_wrapped_for_reqwest() {
        let certificates = parse_native_root_certificates(&[b"cached-native-root".to_vec()]);

        assert_eq!(certificates.len(), 1);
    }

    fn with_invalid_proxy_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let vars = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"];
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|key| (key.to_string(), std::env::var(key).ok()))
            .collect();

        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            std::env::set_var(key, "http://127.0.0.1:1");
        }
        std::env::remove_var("NO_PROXY");

        let result = f();

        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        result
    }

    fn ca_env_vars() -> Vec<&'static str> {
        let mut vars = vec![
            REMOTE_RELAY_CA_BUNDLE_ENV,
            REMOTE_UNSAFE_SSL_ENV,
            BIFROST_CA_BUNDLE_ENV,
            BIFROST_CA_DIR_ENV,
            BIFROST_UNSAFE_SSL_ENV,
            GITHUB_CA_BUNDLE_ENV,
            GITHUB_CA_DIR_ENV,
            GITHUB_UNSAFE_SSL_ENV,
            UPGRADE_CA_BUNDLE_ENV,
            UPGRADE_CA_DIR_ENV,
            UPGRADE_UNSAFE_SSL_ENV,
        ];
        vars.extend(COMMON_CA_FILE_ENVS.iter().copied());
        vars.extend(COMMON_CA_DIR_ENVS.iter().copied());
        vars
    }

    fn env_name_matches(actual: &str, expected: &str) -> bool {
        if cfg!(windows) {
            actual.eq_ignore_ascii_case(expected)
        } else {
            actual == expected
        }
    }

    fn with_ca_envs_cleared<T>(f: impl FnOnce() -> T) -> T {
        let vars = ca_env_vars();
        let saved: Vec<(&'static str, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        for key in &vars {
            std::env::remove_var(key);
        }

        let result = f();

        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        result
    }

    fn spawn_local_http_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = stream.flush();
        });
        format!("http://{addr}")
    }

    #[test]
    fn blocking_reqwest_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let response = direct_blocking_reqwest_client_builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap()
                .get(url)
                .send()
                .unwrap()
                .text()
                .unwrap();
            assert_eq!(response, "ok");
        });
    }

    #[test]
    fn async_reqwest_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let response = runtime.block_on(async move {
                direct_reqwest_client_builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap()
                    .get(url)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            });
            assert_eq!(response, "ok");
        });
    }

    #[test]
    fn ureq_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let response = direct_ureq_agent_builder()
                .timeout(Duration::from_secs(2))
                .build()
                .get(&url)
                .call()
                .unwrap()
                .into_string()
                .unwrap();
            assert_eq!(response, "ok");
        });
    }

    #[test]
    fn direct_ureq_agent_builds() {
        // Just ensure construction succeeds and returns a usable agent.
        let _agent = direct_ureq_agent();
    }

    #[test]
    fn format_reqwest_error_includes_top_level_message() {
        let error = direct_blocking_reqwest_client_builder()
            .build()
            .unwrap()
            .get("http://")
            .send()
            .unwrap_err();

        let message = super::format_reqwest_error(&error);

        assert!(message.contains("builder error") || message.contains("relative URL"));
    }

    #[test]
    fn load_certificate_errors_on_missing_file() {
        let err =
            load_reqwest_certificate(std::path::Path::new("/nonexistent/ca.pem")).unwrap_err();
        assert!(err.contains("read CA certificate"));
    }

    #[test]
    fn load_certificate_ok_branch_with_empty_bundle() {
        // reqwest accepts a PEM file with no certificate blocks, exercising the
        // Ok arm of load_reqwest_certificate without needing a real cert.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.pem");
        std::fs::write(&path, b"# no certificates here\n").unwrap();
        assert!(load_reqwest_certificate(&path).is_ok());
    }

    #[test]
    fn load_certificate_bundle_accepts_valid_pem_bundle() {
        let pem = include_bytes!("../../bifrost-tls/testdata/test-rsa-ca.crt");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca-bundle.pem");
        std::fs::write(&path, pem).unwrap();

        let certs = load_reqwest_certificate_bundle(&path).unwrap();

        assert!(!certs.is_empty());
    }

    #[test]
    fn remote_relay_ca_bundle_env_accepts_path_lists() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            let dir = tempfile::tempdir().unwrap();
            let ca1 = dir.path().join("ca1.pem");
            let ca2 = dir.path().join("ca2.pem");
            std::fs::write(&ca1, b"# empty bundle\n").unwrap();
            std::fs::write(&ca2, b"# empty bundle\n").unwrap();
            let joined = std::env::join_paths([&ca1, &ca2]).unwrap();
            std::env::set_var(REMOTE_RELAY_CA_BUNDLE_ENV, joined);

            let paths = remote_relay_ca_file_paths_from_env()
                .into_iter()
                .filter(|(source, _)| source == REMOTE_RELAY_CA_BUNDLE_ENV)
                .map(|(_, path)| path)
                .collect::<Vec<_>>();

            assert_eq!(paths, vec![ca1, ca2]);
        });
    }

    #[test]
    fn remote_relay_ca_file_envs_include_common_tooling_overrides() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            let dir = tempfile::tempdir().unwrap();
            for env_name in COMMON_CA_FILE_ENVS {
                let ca = dir.path().join(format!("{env_name}.pem"));
                std::fs::write(&ca, b"# empty bundle\n").unwrap();
                std::env::set_var(env_name, &ca);
                let paths = remote_relay_ca_file_paths_from_env();
                assert!(
                    paths
                        .iter()
                        .any(|(source, path)| env_name_matches(source, env_name) && path == &ca),
                    "{env_name} should be accepted as an extra CA bundle source"
                );
                std::env::remove_var(env_name);
            }
        });
    }

    #[test]
    fn github_ca_file_envs_include_scoped_global_and_common_overrides() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            let dir = tempfile::tempdir().unwrap();
            let envs = [
                GITHUB_CA_BUNDLE_ENV,
                UPGRADE_CA_BUNDLE_ENV,
                BIFROST_CA_BUNDLE_ENV,
                "SSL_CERT_FILE",
                "REQUESTS_CA_BUNDLE",
                "CURL_CA_BUNDLE",
                "NODE_EXTRA_CA_CERTS",
                "GIT_SSL_CAINFO",
                "AWS_CA_BUNDLE",
                "PIP_CERT",
                "NPM_CONFIG_CAFILE",
                "npm_config_cafile",
                "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
            ];
            for env_name in envs {
                let ca = dir.path().join(format!("{env_name}.pem"));
                std::fs::write(&ca, b"# empty bundle\n").unwrap();
                std::env::set_var(env_name, &ca);
                let paths = github_ca_file_paths_from_env();
                assert!(
                    paths
                        .iter()
                        .any(|(source, path)| env_name_matches(source, env_name) && path == &ca),
                    "{env_name} should be accepted as a GitHub CA bundle source"
                );
                std::env::remove_var(env_name);
            }
        });
    }

    #[test]
    fn outbound_ca_file_envs_include_global_and_common_overrides() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            let dir = tempfile::tempdir().unwrap();
            let envs = [
                BIFROST_CA_BUNDLE_ENV,
                "SSL_CERT_FILE",
                "REQUESTS_CA_BUNDLE",
                "CURL_CA_BUNDLE",
                "NODE_EXTRA_CA_CERTS",
                "GIT_SSL_CAINFO",
                "AWS_CA_BUNDLE",
                "PIP_CERT",
                "NPM_CONFIG_CAFILE",
                "npm_config_cafile",
                "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
            ];
            for env_name in envs {
                let ca = dir.path().join(format!("{env_name}.pem"));
                std::fs::write(&ca, b"# empty bundle\n").unwrap();
                std::env::set_var(env_name, &ca);
                let paths = outbound_ca_file_paths_from_env();
                assert!(
                    paths
                        .iter()
                        .any(|(source, path)| env_name_matches(source, env_name) && path == &ca),
                    "{env_name} should be accepted as an outbound CA bundle source"
                );
                std::env::remove_var(env_name);
            }
        });
    }

    #[test]
    fn remote_relay_builder_bypasses_proxy_env() {
        with_invalid_proxy_env(|| {
            let url = spawn_local_http_server();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let response = runtime.block_on(async move {
                remote_relay_reqwest_client_builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap()
                    .get(url)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            });
            assert_eq!(response, "ok");
        });
    }

    #[test]
    fn remote_relay_builder_builds_with_explicit_ca_bundle() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let saved = std::env::var_os(REMOTE_RELAY_CA_BUNDLE_ENV);
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(
            &ca,
            include_bytes!("../../bifrost-tls/testdata/test-rsa-ca.crt"),
        )
        .unwrap();
        std::env::set_var(REMOTE_RELAY_CA_BUNDLE_ENV, &ca);

        let result = remote_relay_reqwest_client_builder().build();

        match saved {
            Some(value) => std::env::set_var(REMOTE_RELAY_CA_BUNDLE_ENV, value),
            None => std::env::remove_var(REMOTE_RELAY_CA_BUNDLE_ENV),
        }
        assert!(result.is_ok());
    }

    #[test]
    fn github_builders_build_with_explicit_ca_bundle() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            let dir = tempfile::tempdir().unwrap();
            let ca = dir.path().join("ca.pem");
            std::fs::write(
                &ca,
                include_bytes!("../../bifrost-tls/testdata/test-rsa-ca.crt"),
            )
            .unwrap();
            std::env::set_var(GITHUB_CA_BUNDLE_ENV, &ca);

            assert!(github_reqwest_client_builder().build().is_ok());
            assert!(github_blocking_reqwest_client_builder().build().is_ok());
        });
    }

    #[test]
    fn outbound_builders_build_with_explicit_ca_bundle() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            let dir = tempfile::tempdir().unwrap();
            let ca = dir.path().join("ca.pem");
            std::fs::write(
                &ca,
                include_bytes!("../../bifrost-tls/testdata/test-rsa-ca.crt"),
            )
            .unwrap();
            std::env::set_var(BIFROST_CA_BUNDLE_ENV, &ca);

            assert!(outbound_reqwest_client_builder().build().is_ok());
            assert!(outbound_blocking_reqwest_client_builder().build().is_ok());
        });
    }

    #[test]
    fn remote_relay_unsafe_ssl_env_parses_true_false_and_invalid_values() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            for value in ["1", "true", "TRUE", "yes", "on"] {
                std::env::set_var(REMOTE_UNSAFE_SSL_ENV, value);
                assert!(
                    remote_relay_unsafe_ssl_from_env(),
                    "{value} should enable remote unsafe SSL"
                );
            }

            for value in ["", "0", "false", "FALSE", "no", "off", "maybe"] {
                std::env::set_var(REMOTE_UNSAFE_SSL_ENV, value);
                assert!(
                    !remote_relay_unsafe_ssl_from_env(),
                    "{value:?} should not enable remote unsafe SSL"
                );
            }
        });
    }

    #[test]
    fn github_unsafe_ssl_env_accepts_github_and_upgrade_aliases() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            for env_name in [
                GITHUB_UNSAFE_SSL_ENV,
                UPGRADE_UNSAFE_SSL_ENV,
                BIFROST_UNSAFE_SSL_ENV,
            ] {
                std::env::set_var(env_name, "true");
                assert!(
                    github_unsafe_ssl_from_env(),
                    "{env_name} should enable GitHub unsafe SSL"
                );
                std::env::set_var(env_name, "false");
                assert!(
                    !github_unsafe_ssl_from_env(),
                    "{env_name}=false should disable GitHub unsafe SSL"
                );
                std::env::remove_var(env_name);
            }
        });
    }

    #[test]
    fn outbound_unsafe_ssl_env_accepts_global_alias() {
        let _guard = proxy_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        with_ca_envs_cleared(|| {
            std::env::set_var(BIFROST_UNSAFE_SSL_ENV, "true");
            assert!(outbound_unsafe_ssl_from_env());

            std::env::set_var(BIFROST_UNSAFE_SSL_ENV, "false");
            assert!(!outbound_unsafe_ssl_from_env());
        });
    }

    #[test]
    fn proxied_builder_rejects_invalid_proxy_url() {
        let err = proxied_reqwest_client_builder("not a url", None).unwrap_err();
        assert!(err.contains("invalid proxy URL"));
    }

    #[test]
    fn proxied_builder_ok_without_ca() {
        let builder = proxied_reqwest_client_builder("http://127.0.0.1:8080", None);
        assert!(builder.is_ok());
        // builds into a client
        assert!(builder.unwrap().build().is_ok());
    }

    #[test]
    fn proxied_builder_with_ca_path_succeeds() {
        // Provide a CA path so the Some(path) branch runs; the loaded (empty)
        // bundle is accepted and the client still builds.
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, b"# empty bundle\n").unwrap();
        let builder = proxied_reqwest_client_builder("http://127.0.0.1:8080", Some(&ca)).unwrap();
        assert!(builder.build().is_ok());
    }

    #[test]
    fn parse_remote_relay_headers_accepts_ppe_headers() {
        let headers =
            parse_remote_relay_headers("x-tt-env=ppe_ticket_system, x-use-ppe=1").unwrap();

        assert_eq!(headers.get("x-tt-env").unwrap(), "ppe_ticket_system");
        assert_eq!(headers.get("x-use-ppe").unwrap(), "1");
    }

    #[test]
    fn parse_remote_relay_headers_rejects_restricted_headers() {
        let err = parse_remote_relay_headers("Authorization=Bearer token").unwrap_err();

        assert!(err.contains(REMOTE_RELAY_HEADERS_ENV));
        assert!(err.contains("restricted"));
    }

    #[test]
    fn apply_remote_relay_headers_adds_headers_to_request() {
        let headers = parse_remote_relay_headers("x-use-ppe=1").unwrap();
        let client = direct_reqwest_client_builder().build().unwrap();
        let request = apply_remote_relay_headers(client.get("http://example.test"), &headers)
            .build()
            .unwrap();

        assert_eq!(request.headers().get("x-use-ppe").unwrap(), "1");
    }
}
