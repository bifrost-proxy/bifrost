use bifrost_core::BifrostError;
use colored::Colorize;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::update_check::{get_latest_version, get_latest_version_fresh_with_diagnostics};
use crate::process::{
    capture_runtime_system_proxy_snapshot, is_process_running, read_pid, read_runtime_info,
    RuntimeSystemProxySnapshot,
};
use bifrost_core::version_check::{
    is_newer_version, make_release_tag, VersionCache, GITHUB_RELEASE_URL,
};
const GITHUB_BASE_URL: &str = "https://github.com";
const DEFAULT_GITHUB_MIRROR_URLS: &[&str] = &[
    "https://github.com",
    "https://ghfast.top/https://github.com",
    "https://github.moeyy.xyz/https://github.com",
];
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 10;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
const MIRROR_PROBE_TIMEOUT_SECS: u64 = 5;
const DOWNLOAD_TRIES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DownloadTuning {
    connect_timeout_secs: u64,
    download_timeout_secs: u64,
    mirror_probe_timeout_secs: u64,
    download_tries: usize,
}

impl Default for DownloadTuning {
    fn default() -> Self {
        Self {
            connect_timeout_secs: DOWNLOAD_CONNECT_TIMEOUT_SECS,
            download_timeout_secs: DOWNLOAD_TIMEOUT_SECS,
            mirror_probe_timeout_secs: MIRROR_PROBE_TIMEOUT_SECS,
            download_tries: DOWNLOAD_TRIES,
        }
    }
}

impl DownloadTuning {
    fn from_env() -> Self {
        Self {
            connect_timeout_secs: positive_env_u64(
                "BIFROST_DOWNLOAD_CONNECT_TIMEOUT",
                DOWNLOAD_CONNECT_TIMEOUT_SECS,
            ),
            download_timeout_secs: positive_env_u64(
                "BIFROST_DOWNLOAD_TIMEOUT",
                DOWNLOAD_TIMEOUT_SECS,
            ),
            mirror_probe_timeout_secs: positive_env_u64(
                "BIFROST_MIRROR_PROBE_TIMEOUT",
                MIRROR_PROBE_TIMEOUT_SECS,
            ),
            download_tries: positive_env_usize("BIFROST_DOWNLOAD_TRIES", DOWNLOAD_TRIES),
        }
    }
}

fn parse_positive_u64(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_positive_usize(value: Option<&str>, default: usize) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_u64(name: &str, default: u64) -> u64 {
    parse_positive_u64(env::var(name).ok().as_deref(), default)
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    parse_positive_usize(env::var(name).ok().as_deref(), default)
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstallMethod {
    Homebrew,
    Script,
    Manual(PathBuf),
    Unknown,
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallMethod::Homebrew => write!(f, "Homebrew"),
            InstallMethod::Script => write!(f, "Install script"),
            InstallMethod::Manual(path) => write!(f, "Manual ({})", path.display()),
            InstallMethod::Unknown => write!(f, "Unknown"),
        }
    }
}

fn detect_install_method() -> InstallMethod {
    let exe_path = match env::current_exe() {
        Ok(path) => path,
        Err(_) => return InstallMethod::Unknown,
    };

    let exe_path_str = exe_path.to_string_lossy();

    if exe_path_str.contains("/opt/homebrew/")
        || exe_path_str.contains("/usr/local/Cellar/")
        || exe_path_str.contains("/home/linuxbrew/")
    {
        return InstallMethod::Homebrew;
    }

    if exe_path_str.contains("/.bifrost/bin/") {
        return InstallMethod::Script;
    }

    InstallMethod::Manual(exe_path)
}

fn get_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("aarch64-apple-darwin")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("x86_64-apple-darwin")
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
    {
        Some("x86_64-unknown-linux-musl")
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(target_env = "musl")))]
    {
        if should_use_musl_fallback() {
            Some("x86_64-unknown-linux-musl")
        } else {
            Some("x86_64-unknown-linux-gnu")
        }
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
    {
        Some("aarch64-unknown-linux-musl")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", not(target_env = "musl")))]
    {
        if should_use_musl_fallback() {
            Some("aarch64-unknown-linux-musl")
        } else {
            Some("aarch64-unknown-linux-gnu")
        }
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("x86_64-pc-windows-msvc")
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        Some("aarch64-pc-windows-msvc")
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
    )))]
    {
        None
    }
}

#[cfg(any(
    test,
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        not(target_env = "musl")
    )
))]
const MIN_GLIBC_VERSION: (u32, u32) = (2, 39);

#[cfg(any(
    test,
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        not(target_env = "musl")
    )
))]
fn glibc_requires_musl_fallback(version: Option<(u32, u32)>) -> bool {
    match version {
        Some(version) => version < MIN_GLIBC_VERSION,
        None => true,
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    not(target_env = "musl")
))]
fn should_use_musl_fallback() -> bool {
    glibc_requires_musl_fallback(detect_glibc_version())
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    not(target_env = "musl")
))]
fn detect_glibc_version() -> Option<(u32, u32)> {
    let output = Command::new("ldd").arg("--version").output().ok()?;

    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);

    if !text.to_lowercase().contains("glibc") && !text.to_lowercase().contains("gnu libc") {
        return None;
    }

    let first_line = text.lines().next()?;
    let version_str = first_line.split_whitespace().rfind(|word| {
        let parts: Vec<&str> = word.split('.').collect();
        parts.len() == 2 && parts[0].parse::<u32>().is_ok() && parts[1].parse::<u32>().is_ok()
    })?;

    let parts: Vec<&str> = version_str.split('.').collect();
    let major = parts[0].parse::<u32>().ok()?;
    let minor = parts[1].parse::<u32>().ok()?;
    Some((major, minor))
}

#[cfg(target_os = "macos")]
fn clear_quarantine_attr(path: &Path) {
    use tracing::debug;
    for flag in ["-c", "-d com.apple.quarantine", "-d com.apple.provenance"] {
        let args: Vec<&str> = flag.split_whitespace().collect();
        let result = Command::new("xattr").args(&args).arg(path).output();
        match result {
            Ok(output) if !output.status.success() => {
                debug!(
                    flag,
                    path = %path.display(),
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "xattr removal returned non-zero (may be absent, safe to ignore)"
                );
            }
            Err(e) => {
                debug!(
                    flag,
                    path = %path.display(),
                    error = %e,
                    "failed to run xattr command"
                );
            }
            _ => {}
        }
    }
}

fn get_musl_fallback_triple(target: &str) -> Option<String> {
    match target {
        "x86_64-unknown-linux-gnu" => Some("x86_64-unknown-linux-musl".to_string()),
        "aarch64-unknown-linux-gnu" => Some("aarch64-unknown-linux-musl".to_string()),
        _ => None,
    }
}

fn verify_binary(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn prompt_confirm(message: &str) -> bool {
    print!("{} [y/N]: ", message);
    io::stdout().flush().ok();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }

    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn github_mirror_bases() -> Vec<String> {
    let preferred = env::var("BIFROST_GITHUB_MIRROR")
        .ok()
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty());
    let mut bases = Vec::new();

    if let Some(preferred) = preferred.as_ref() {
        bases.push(preferred.clone());
    }

    for base in DEFAULT_GITHUB_MIRROR_URLS {
        let normalized = base.trim_end_matches('/').to_string();
        if preferred.as_deref() != Some(normalized.as_str()) {
            bases.push(normalized);
        }
    }

    bases
}

fn mirror_display_name(base_url: &str) -> String {
    base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

fn github_path_url(base_url: &str, github_path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        github_path.trim_start_matches('/')
    )
}

fn probe_github_url(url: &str, tuning: DownloadTuning) -> bool {
    let client = match bifrost_core::direct_blocking_reqwest_client_builder()
        .connect_timeout(Duration::from_secs(tuning.connect_timeout_secs))
        .timeout(Duration::from_secs(tuning.mirror_probe_timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };

    let head_ok = client
        .head(url)
        .send()
        .map(|response| response.status().is_success())
        .unwrap_or(false);
    if head_ok {
        return true;
    }

    client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .map(|response| {
            response.status().is_success()
                || response.status() == reqwest::StatusCode::PARTIAL_CONTENT
        })
        .unwrap_or(false)
}

fn select_fastest_github_base(github_path: &str, tuning: DownloadTuning) -> Option<String> {
    let bases = github_mirror_bases();
    if bases.is_empty() {
        return None;
    }
    if bases.len() == 1 {
        return bases.into_iter().next();
    }

    let (tx, rx) = mpsc::channel();
    for (index, base) in bases.iter().cloned().enumerate() {
        let tx = tx.clone();
        let url = github_path_url(&base, github_path);
        thread::spawn(move || {
            let started = Instant::now();
            if probe_github_url(&url, tuning) {
                let _ = tx.send((index, base, started.elapsed()));
            }
        });
    }
    drop(tx);

    rx.recv_timeout(Duration::from_secs(tuning.mirror_probe_timeout_secs + 1))
        .ok()
        .map(|(_, base, _)| base)
}

fn download_progress_line(downloaded: u64, total: Option<u64>, started: Instant) -> String {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let speed = downloaded as f64 / elapsed;
    match total {
        Some(total) if total > 0 => {
            let percent = ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
            format!(
                "Downloading… {:>5.1}% ({}/{}, {}/s)",
                percent,
                human_bytes(downloaded),
                human_bytes(total),
                human_bytes(speed as u64)
            )
        }
        _ => format!(
            "Downloading… {} ({}/s)",
            human_bytes(downloaded),
            human_bytes(speed as u64)
        ),
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn download_file_with_progress(
    url: &str,
    output_path: &Path,
    tuning: DownloadTuning,
) -> Result<(), BifrostError> {
    let client = bifrost_core::direct_blocking_reqwest_client_builder()
        .connect_timeout(Duration::from_secs(tuning.connect_timeout_secs))
        .timeout(Duration::from_secs(tuning.download_timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| BifrostError::Network(format!("Failed to build HTTP client: {error}")))?;

    let mut last_error = None;
    for attempt in 1..=tuning.download_tries {
        if attempt > 1 {
            println!(
                "{}",
                format!(
                    "Retrying download ({}/{})...",
                    attempt, tuning.download_tries
                )
                .bright_yellow()
            );
        }

        match download_file_once_with_progress(&client, url, output_path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let _ = fs::remove_file(output_path);
                last_error = Some(error);
                if attempt < tuning.download_tries {
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| BifrostError::Network(format!("Failed to download {}", url))))
}

fn download_file_once_with_progress(
    client: &reqwest::blocking::Client,
    url: &str,
    output_path: &Path,
) -> Result<(), BifrostError> {
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| BifrostError::Network(format!("Failed to download {url} — {error}")))?;

    if !response.status().is_success() {
        return Err(BifrostError::Network(format!(
            "Failed to download {} — HTTP {}",
            url,
            response.status()
        )));
    }

    let total = response.content_length();
    let mut file = fs::File::create(output_path).map_err(BifrostError::Io)?;
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    let mut last_render = Instant::now() - Duration::from_secs(1);
    let started = Instant::now();

    loop {
        let read = response.read(&mut buffer).map_err(BifrostError::Io)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(BifrostError::Io)?;
        downloaded += read as u64;

        if last_render.elapsed() >= Duration::from_millis(250) {
            print!("\r{}", download_progress_line(downloaded, total, started));
            io::stdout().flush().ok();
            last_render = Instant::now();
        }
    }

    file.flush().map_err(BifrostError::Io)?;
    println!("\r{}", download_progress_line(downloaded, total, started));

    if downloaded == 0 {
        return Err(BifrostError::Network(format!(
            "Failed to download {} — empty response",
            url
        )));
    }

    Ok(())
}

fn ordered_download_bases(github_path: &str, tuning: DownloadTuning) -> Vec<String> {
    let bases = github_mirror_bases();
    let selected = select_fastest_github_base(github_path, tuning);
    let mut ordered = Vec::new();

    if let Some(selected) = selected {
        ordered.push(selected);
    }

    for base in bases {
        if !ordered.iter().any(|existing| existing == &base) {
            ordered.push(base);
        }
    }

    if ordered.is_empty() {
        ordered.push(GITHUB_BASE_URL.to_string());
    }

    ordered
}

fn archive_ext_candidates_for_os(
    os: &str,
    tar_xz_supported: bool,
    xz_disabled: bool,
) -> Vec<&'static str> {
    match os {
        "windows" => vec!["zip"],
        _ => {
            let mut candidates = Vec::new();
            if tar_xz_supported && !xz_disabled {
                candidates.push("tar.xz");
            }
            candidates.push("tar.gz");
            candidates
        }
    }
}

fn tar_supports_xz() -> bool {
    if env::var("BIFROST_DISABLE_XZ_ARCHIVE").ok().as_deref() == Some("1") {
        return false;
    }

    let output = Command::new("tar").arg("--help").output();
    let Ok(output) = output else {
        return false;
    };
    let help = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    help.contains("-J") || help.to_ascii_lowercase().contains("xz")
}

fn release_archive_ext_candidates() -> Vec<&'static str> {
    let os = if cfg!(windows) { "windows" } else { "unix" };
    archive_ext_candidates_for_os(
        os,
        tar_supports_xz(),
        env::var("BIFROST_DISABLE_XZ_ARCHIVE").ok().as_deref() == Some("1"),
    )
}

fn validate_downloaded_archive(path: &Path, archive_ext: &str) -> Result<(), BifrostError> {
    if cfg!(windows) || archive_ext == "zip" {
        return Ok(());
    }

    let tar_flag = if archive_ext == "tar.xz" {
        "-tJf"
    } else {
        "-tzf"
    };
    let output = Command::new("tar")
        .arg(tar_flag)
        .arg(path)
        .output()
        .map_err(BifrostError::Io)?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(BifrostError::Parse(format!(
            "Downloaded archive is invalid — {}",
            stderr.trim()
        )))
    }
}

fn print_update_info(current: &str, cache: &VersionCache) {
    let separator = "─".repeat(64);
    let release_url = format!("{}/v{}", GITHUB_RELEASE_URL, cache.latest_version);

    println!();
    println!("{}", separator.bright_cyan());
    println!("{}", "  📦 New version available!".bright_cyan().bold());
    println!();
    println!("     Current version: {}", current.bright_yellow().bold());
    println!(
        "     Latest version:  {}",
        cache.latest_version.bright_green().bold()
    );

    if !cache.release_highlights.is_empty() {
        println!();
        println!("     {}", "What's new:".bright_white().bold());
        for highlight in &cache.release_highlights {
            println!("       {} {}", "•".bright_cyan(), highlight.bright_white());
        }
    }

    println!();
    println!(
        "     {} {}",
        "Release notes:".dimmed(),
        release_url.dimmed()
    );
    println!("{}", separator.bright_cyan());
    println!();
}

const HOMEBREW_FORMULA_NAME: &str = "bifrost-proxy/bifrost/bifrost";

fn upgrade_via_homebrew(target_version: &str) -> Result<(), BifrostError> {
    println!("{}", "Refreshing Homebrew tap...".bright_cyan());

    let output = Command::new("brew")
        .args(["--repository", "bifrost-proxy/bifrost"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(tap_path) = String::from_utf8(output.stdout) {
                let tap_path = tap_path.trim();
                if !tap_path.is_empty() {
                    let _ = Command::new("git")
                        .args(["-C", tap_path, "fetch", "--all", "-q"])
                        .status();
                    let _ = Command::new("git")
                        .args(["-C", tap_path, "reset", "--hard", "origin/main", "-q"])
                        .status();
                }
            }
        }
    }

    println!("{}", "Upgrading via Homebrew...".bright_cyan());

    let status = Command::new("brew")
        .args(["reinstall", HOMEBREW_FORMULA_NAME])
        .status();

    let success = match status {
        Ok(s) if s.success() => true,
        _ => {
            println!(
                "{}",
                "Standard install failed, trying --build-from-source...".bright_yellow()
            );
            let fallback_status = Command::new("brew")
                .args(["reinstall", "--build-from-source", HOMEBREW_FORMULA_NAME])
                .status()
                .map_err(BifrostError::Io)?;
            fallback_status.success()
        }
    };

    if !success {
        return Err(BifrostError::Network(format!(
            "Homebrew upgrade failed. Try: brew reinstall {}",
            HOMEBREW_FORMULA_NAME
        )));
    }

    let output = Command::new("brew")
        .args(["info", "--json=v2", HOMEBREW_FORMULA_NAME])
        .output()
        .map_err(BifrostError::Io)?;

    if output.status.success() {
        if let Ok(json_str) = String::from_utf8(output.stdout) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let Some(installed) = json["formulae"]
                    .get(0)
                    .and_then(|f| f["installed"].as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|i| i["version"].as_str())
                {
                    if installed == target_version {
                        println!(
                            "{}",
                            "✓ Upgrade completed successfully!".bright_green().bold()
                        );
                        return Ok(());
                    } else {
                        println!(
                            "{}",
                            format!(
                                "⚠ Installed version ({}) doesn't match target version ({}).",
                                installed, target_version
                            )
                            .bright_yellow()
                        );
                        println!(
                            "{}",
                            "  The Homebrew tap may not be updated yet. Please try again later or install manually:"
                                .bright_yellow()
                        );
                        println!(
                            "  {}",
                            "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash"
                                .bright_cyan()
                        );
                        return Ok(());
                    }
                }
            }
        }
    }

    println!(
        "{}",
        "⚠ Upgrade completed but could not verify installed version.".bright_yellow()
    );
    println!(
        "{}",
        "  Run `bifrost --version` to confirm the upgrade succeeded.".dimmed()
    );
    Ok(())
}

fn upgrade_via_script() -> Result<(), BifrostError> {
    println!("{}", "Upgrading via install script...".bright_cyan());

    let status = Command::new("sh")
        .args([
            "-c",
            "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash",
        ])
        .status()
        .map_err(BifrostError::Io)?;

    if status.success() {
        println!(
            "{}",
            "✓ Upgrade completed successfully!".bright_green().bold()
        );
        Ok(())
    } else {
        Err(BifrostError::Network(
            "Install script failed — check network connection and try again".to_string(),
        ))
    }
}

fn download_and_install(
    target: &str,
    version: &str,
    target_path: &PathBuf,
    temp_dir: &tempfile::TempDir,
) -> Result<(), BifrostError> {
    let tuning = DownloadTuning::from_env();
    let release_tag = make_release_tag(version);
    let mut last_error = None;
    let mut selected_archive_path = None;
    let mut selected_archive_ext = None;

    for archive_ext in release_archive_ext_candidates() {
        let archive_name = format!("bifrost-v{}-{}.{}", version, target, archive_ext);
        let archive_github_path = format!(
            "bifrost-proxy/bifrost/releases/download/{}/{}",
            release_tag, archive_name
        );
        let archive_path = temp_dir.path().join(&archive_name);

        for (attempt, base) in ordered_download_bases(&archive_github_path, tuning)
            .into_iter()
            .enumerate()
        {
            let download_url = github_path_url(&base, &archive_github_path);
            if attempt == 0 {
                println!(
                    "{} {}",
                    "Selected fastest available source:".bright_cyan(),
                    mirror_display_name(&base).bright_white()
                );
            } else {
                println!(
                    "{} {}",
                    "Retrying with source:".bright_yellow(),
                    mirror_display_name(&base).bright_white()
                );
            }
            println!("{} {}", "Downloading:".bright_cyan(), download_url.dimmed());

            match download_file_with_progress(&download_url, &archive_path, tuning) {
                Ok(()) => {
                    if let Err(error) = validate_downloaded_archive(&archive_path, archive_ext) {
                        let _ = fs::remove_file(&archive_path);
                        println!(
                            "{} {}",
                            "Downloaded archive failed validation:".bright_yellow(),
                            error.to_string().dimmed()
                        );
                        last_error = Some(error);
                        continue;
                    }
                    if attempt > 0 {
                        println!(
                            "{} {}",
                            "Downloaded via fallback source:".bright_green(),
                            mirror_display_name(&base).bright_white()
                        );
                    }
                    selected_archive_path = Some(archive_path);
                    selected_archive_ext = Some(archive_ext);
                    last_error = None;
                    break;
                }
                Err(error) => {
                    let _ = fs::remove_file(&archive_path);
                    println!(
                        "{} {}",
                        "Download source failed:".bright_yellow(),
                        error.to_string().dimmed()
                    );
                    last_error = Some(error);
                }
            }
        }

        if selected_archive_path.is_some() {
            break;
        }
        if archive_ext != "tar.gz" && archive_ext != "zip" {
            println!(
                "{} {}",
                "Archive download failed, falling back to:".bright_yellow(),
                "tar.gz".bright_white()
            );
        }
    }

    let archive_path = selected_archive_path.ok_or_else(|| {
        last_error.unwrap_or_else(|| {
            BifrostError::Network("Failed to download release archive".to_string())
        })
    })?;
    let archive_ext = selected_archive_ext
        .ok_or_else(|| BifrostError::Network("Failed to download release archive".to_string()))?;

    println!("{}", "Extracting archive...".bright_cyan());

    let extract_dir = temp_dir.path().join(format!("extract_{}", target));
    fs::create_dir_all(&extract_dir)?;

    if cfg!(windows) {
        let output = Command::new("powershell")
            .args([
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}'",
                    archive_path.display(),
                    extract_dir.display()
                ),
            ])
            .output()
            .map_err(BifrostError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BifrostError::Parse(format!(
                "Failed to extract archive — {}",
                stderr.trim()
            )));
        }
    } else {
        let tar_flag = if archive_ext == "tar.xz" {
            "-xJf"
        } else {
            "-xzf"
        };
        let output = Command::new("tar")
            .args([
                tar_flag,
                archive_path.to_str().unwrap(),
                "-C",
                extract_dir.to_str().unwrap(),
            ])
            .output()
            .map_err(BifrostError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BifrostError::Parse(format!(
                "Failed to extract archive — {}",
                stderr.trim()
            )));
        }
    }

    let binary_name = if cfg!(windows) {
        "bifrost.exe"
    } else {
        "bifrost"
    };
    let extracted_dir = extract_dir.join(format!("bifrost-v{}-{}", version, target));
    let new_binary = extracted_dir.join(binary_name);

    if !new_binary.exists() {
        return Err(BifrostError::NotFound(format!(
            "Binary not found in archive: {}",
            new_binary.display()
        )));
    }

    println!(
        "{} {}",
        "Replacing binary at:".bright_cyan(),
        target_path.display()
    );

    let backup_path = target_path.with_extension("backup");
    if target_path.exists() {
        fs::rename(target_path, &backup_path).map_err(|e| {
            BifrostError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to backup current binary {}: {}",
                    target_path.display(),
                    e
                ),
            ))
        })?;
    }

    match fs::copy(&new_binary, target_path) {
        Ok(_) => {
            if backup_path.exists() {
                let _ = fs::remove_file(&backup_path);
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(target_path)
                    .map_err(|e| {
                        BifrostError::Io(std::io::Error::new(
                            e.kind(),
                            format!(
                                "failed to read metadata of {}: {}",
                                target_path.display(),
                                e
                            ),
                        ))
                    })?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(target_path, perms).map_err(|e| {
                    BifrostError::Io(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to set executable permissions on {}: {}",
                            target_path.display(),
                            e
                        ),
                    ))
                })?;
            }

            #[cfg(target_os = "macos")]
            {
                clear_quarantine_attr(target_path);
            }

            Ok(())
        }
        Err(e) => {
            if backup_path.exists() {
                let _ = fs::rename(&backup_path, target_path);
            }
            Err(BifrostError::Io(e))
        }
    }
}

fn upgrade_manual(target_path: &PathBuf, version: &str) -> Result<(), BifrostError> {
    let target = get_target_triple().ok_or_else(|| {
        BifrostError::Config("Unsupported platform for automatic upgrade".to_string())
    })?;

    let temp_dir = tempfile::tempdir().map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;

    let mut effective_target = target.to_string();

    let install_result = download_and_install(target, version, target_path, &temp_dir);

    let needs_musl_fallback = match &install_result {
        Ok(()) => !verify_binary(target_path),
        Err(_) => true,
    };

    if needs_musl_fallback {
        if let Some(musl_target) = get_musl_fallback_triple(target) {
            let reason = if install_result.is_err() {
                "download/install failed"
            } else {
                "binary failed to run — likely a glibc version mismatch"
            };
            println!(
                "{}",
                format!("⚠ {} binary {}", target, reason).bright_yellow()
            );
            println!(
                "{}",
                format!("  Retrying with musl build: {}", musl_target).bright_cyan()
            );

            download_and_install(&musl_target, version, target_path, &temp_dir)?;

            if !verify_binary(target_path) {
                return Err(BifrostError::Config(
                    "Fallback musl binary also failed to run. Try installing manually with:\n  curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash".to_string(),
                ));
            }

            effective_target = musl_target;
            println!("{}", "✓ musl fallback succeeded".bright_green());
        } else if let Err(e) = install_result {
            return Err(e);
        } else {
            return Err(BifrostError::Config(
                "Installed binary failed verification (`bifrost --version` returned non-zero). Try installing manually with:\n  curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash".to_string(),
            ));
        }
    }

    println!(
        "{}",
        format!("✓ Upgrade completed successfully! ({})", effective_target)
            .bright_green()
            .bold()
    );
    Ok(())
}

pub fn handle_upgrade(force: bool, restart: bool) -> Result<(), BifrostError> {
    let current_version = env!("CARGO_PKG_VERSION");

    println!(
        "{} {}",
        "Checking for updates...".bright_cyan(),
        format!("(current: v{})", current_version).dimmed()
    );

    let cache = match get_latest_version_fresh_with_diagnostics() {
        Ok(c) => c,
        Err(diagnostic) => {
            if let Some(cached) = get_latest_version() {
                println!(
                    "{}",
                    format!(
                        "⚠ Could not fetch latest version ({}), using cached data.",
                        diagnostic
                    )
                    .bright_yellow()
                );
                cached
            } else {
                println!(
                    "{}",
                    format!("⚠ Could not check for updates: {}", diagnostic).bright_yellow()
                );
                println!();
                println!("{}", "  You can upgrade manually by running:".dimmed());
                println!(
                    "  {}",
                    "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash"
                        .bright_cyan()
                );
                println!();
                println!("{}", "  Troubleshooting tips:".dimmed());
                println!("{}", "    • Check your internet connection".dimmed());
                println!(
                    "{}",
                    "    • If behind a proxy/firewall, ensure github.com is accessible".dimmed()
                );
                println!(
                    "{}",
                    "    • Try: curl -sI -o /dev/null -w '%{url_effective}' -L https://github.com/bifrost-proxy/bifrost/releases/latest"
                        .dimmed()
                );
                println!(
                    "{}",
                    "    • Set RUST_LOG=debug for detailed diagnostics".dimmed()
                );
                return Ok(());
            }
        }
    };

    if !is_newer_version(current_version, &cache.latest_version) {
        println!(
            "{}",
            format!(
                "✓ You're already on the latest version (v{})",
                current_version
            )
            .bright_green()
            .bold()
        );
        return Ok(());
    }

    print_update_info(current_version, &cache);

    let install_method = detect_install_method();
    println!(
        "     {} {}",
        "Install method:".dimmed(),
        format!("{}", install_method).bright_white()
    );
    println!();

    if !force && !prompt_confirm("Do you want to upgrade now?") {
        println!("{}", "Upgrade cancelled.".dimmed());
        return Ok(());
    }

    println!();

    let upgrade_result = match install_method {
        InstallMethod::Homebrew => upgrade_via_homebrew(&cache.latest_version),
        InstallMethod::Script => upgrade_via_script(),
        InstallMethod::Manual(path) => upgrade_manual(&path, &cache.latest_version),
        InstallMethod::Unknown => {
            println!(
                "{}",
                "⚠ Could not detect installation method.".bright_yellow()
            );
            println!("Please upgrade manually:");
            println!(
                "  {}",
                "curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash"
                    .bright_cyan()
            );
            println!(
                "  Or download from: {}",
                format!("{}/v{}", GITHUB_RELEASE_URL, cache.latest_version).bright_cyan()
            );
            return Ok(());
        }
    };

    upgrade_result?;

    maybe_restart_running_proxy(restart)?;

    Ok(())
}

fn maybe_restart_running_proxy(auto_restart: bool) -> Result<(), BifrostError> {
    let pid = match read_pid() {
        Some(pid) if is_process_running(pid) => pid,
        _ => return Ok(()),
    };

    println!();
    println!(
        "{}",
        format!("  Detected running Bifrost proxy (PID: {})", pid)
            .bright_yellow()
            .bold()
    );

    let should_restart = if auto_restart {
        println!(
            "{}",
            "  Auto-restarting due to --restart flag...".bright_cyan()
        );
        true
    } else {
        println!(
            "{}",
            "  The proxy needs to be restarted to use the new version.".bright_yellow()
        );
        println!(
            "{}",
            "  Tip: use `bifrost upgrade --restart` to restart automatically next time.".dimmed()
        );
        println!();
        prompt_confirm("  Restart the proxy now?")
    };

    if !should_restart {
        println!(
            "{}",
            "  You can restart manually with: bifrost stop && bifrost start -d".dimmed()
        );
        return Ok(());
    }

    let runtime_info = read_runtime_info();
    let system_proxy_snapshot = capture_runtime_system_proxy_snapshot(runtime_info.as_ref());

    println!("{}", "  Stopping current proxy...".bright_cyan());
    super::stop::run_stop_for_restart()
        .map_err(|e| BifrostError::Config(format!("Failed to stop running proxy: {}", e)))?;

    let exe_path = env::current_exe().map_err(BifrostError::Io)?;
    let args = build_restart_args(runtime_info.as_ref(), system_proxy_snapshot.as_ref());

    println!(
        "{} {} {}",
        "  Starting proxy with:".bright_cyan(),
        exe_path.display(),
        args.join(" ")
    );

    let status = Command::new(&exe_path)
        .args(&args)
        .status()
        .map_err(BifrostError::Io)?;

    if status.success() {
        println!(
            "{}",
            "✓ Proxy restarted successfully with the new version!"
                .bright_green()
                .bold()
        );
    } else {
        return Err(BifrostError::Config(
            "Failed to restart proxy after upgrade. Please start manually with: bifrost start -d"
                .to_string(),
        ));
    }

    Ok(())
}

fn build_restart_args(
    runtime_info: Option<&crate::process::RuntimeInfo>,
    system_proxy_snapshot: Option<&RuntimeSystemProxySnapshot>,
) -> Vec<String> {
    let mut args = vec![
        "start".to_string(),
        "-d".to_string(),
        "-y".to_string(),
        "--skip-cert-check".to_string(),
    ];

    if let Some(info) = runtime_info {
        args.push("-p".to_string());
        args.push(info.port.to_string());

        if let Some(ref host) = info.host {
            if host != "127.0.0.1" {
                args.push("--host".to_string());
                args.push(host.clone());
            }
        }

        if let Some(socks5_port) = info.socks5_port {
            args.push("--socks5-port".to_string());
            args.push(socks5_port.to_string());
        }
    }

    if let Some(snapshot) = system_proxy_snapshot {
        args.push("--system-proxy".to_string());
        args.push("--proxy-bypass".to_string());
        args.push(snapshot.bypass.clone());
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_install_method_returns_valid_variant() {
        let method = detect_install_method();
        match method {
            InstallMethod::Homebrew
            | InstallMethod::Script
            | InstallMethod::Manual(_)
            | InstallMethod::Unknown => {}
        }
    }

    #[test]
    fn test_install_method_display() {
        assert_eq!(InstallMethod::Homebrew.to_string(), "Homebrew");
        assert_eq!(InstallMethod::Script.to_string(), "Install script");
        assert_eq!(
            InstallMethod::Manual(PathBuf::from("/usr/local/bin/bifrost")).to_string(),
            "Manual (/usr/local/bin/bifrost)"
        );
        assert_eq!(InstallMethod::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn test_cli_upgrade_restart_flag_parsed() {
        use crate::cli::{Cli, Commands};
        use clap::Parser;

        let cli = Cli::parse_from(["bifrost", "upgrade", "--restart"]);
        match cli.command {
            Some(Commands::Upgrade { yes, restart }) => {
                assert!(!yes);
                assert!(restart);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_yes_and_restart_flags() {
        use crate::cli::{Cli, Commands};
        use clap::Parser;

        let cli = Cli::parse_from(["bifrost", "upgrade", "-y", "--restart"]);
        match cli.command {
            Some(Commands::Upgrade { yes, restart }) => {
                assert!(yes);
                assert!(restart);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_no_flags() {
        use crate::cli::{Cli, Commands};
        use clap::Parser;

        let cli = Cli::parse_from(["bifrost", "upgrade"]);
        match cli.command {
            Some(Commands::Upgrade { yes, restart }) => {
                assert!(!yes);
                assert!(!restart);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn upgrade_via_script_source_keeps_terminal_output_visible() {
        let source = include_str!("upgrade.rs");

        assert!(source.contains("Command::new(\"sh\")"));
        assert!(source.contains(".status()"));
        assert!(!source.contains("let output = Command::new(\"sh\")"));
    }

    #[test]
    fn test_glibc_2_38_requires_musl_for_upgrade() {
        assert!(glibc_requires_musl_fallback(Some((2, 38))));
    }

    #[test]
    fn test_glibc_2_39_keeps_gnu_for_upgrade() {
        assert!(!glibc_requires_musl_fallback(Some((2, 39))));
    }

    #[test]
    fn test_unknown_glibc_requires_musl_for_upgrade() {
        assert!(glibc_requires_musl_fallback(None));
    }

    #[test]
    fn test_build_restart_args_with_runtime_info() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 8080,
            socks5_port: Some(1080),
            host: Some("0.0.0.0".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
        };

        let args = build_restart_args(Some(&info), None);
        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "8080",
                "--host",
                "0.0.0.0",
                "--socks5-port",
                "1080"
            ]
        );
    }

    #[test]
    fn test_build_restart_args_default_host_skipped() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
        };

        let args = build_restart_args(Some(&info), None);
        assert_eq!(
            args,
            vec!["start", "-d", "-y", "--skip-cert-check", "-p", "9900"]
        );
    }

    #[test]
    fn test_build_restart_args_no_runtime_info() {
        let args = build_restart_args(None, None);
        assert_eq!(args, vec!["start", "-d", "-y", "--skip-cert-check"]);
    }

    #[test]
    fn test_build_restart_args_no_host() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 8800,
            socks5_port: None,
            host: None,
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
        };

        let args = build_restart_args(Some(&info), None);
        assert_eq!(
            args,
            vec!["start", "-d", "-y", "--skip-cert-check", "-p", "8800"]
        );
    }

    #[test]
    fn test_build_restart_args_preserves_system_proxy_snapshot() {
        let info = crate::process::RuntimeInfo {
            pid: 12345,
            port: 9900,
            socks5_port: None,
            host: Some("127.0.0.1".to_string()),
            started_at_ms: None,
            start_mode: Default::default(),
            restartable_runtime: false,
            binary_path: None,
        };
        let snapshot = RuntimeSystemProxySnapshot {
            bypass: "localhost,127.0.0.1,*.local".to_string(),
        };

        let args = build_restart_args(Some(&info), Some(&snapshot));

        assert_eq!(
            args,
            vec![
                "start",
                "-d",
                "-y",
                "--skip-cert-check",
                "-p",
                "9900",
                "--system-proxy",
                "--proxy-bypass",
                "localhost,127.0.0.1,*.local"
            ]
        );
    }

    #[test]
    fn upgrade_download_progress_formats_percent_and_size() {
        let started = Instant::now() - Duration::from_secs(2);
        let line = download_progress_line(512, Some(1024), started);

        assert!(line.contains("50.0%"));
        assert!(line.contains("512 B/1.0 KiB"));
        assert!(line.contains("/s"));
    }

    #[test]
    fn upgrade_github_path_url_joins_mirror_and_release_path() {
        assert_eq!(
            github_path_url(
                "https://ghfast.top/https://github.com/",
                "bifrost-proxy/bifrost/releases/download/v0.0.88/a.tar.gz"
            ),
            "https://ghfast.top/https://github.com/bifrost-proxy/bifrost/releases/download/v0.0.88/a.tar.gz"
        );
    }

    #[test]
    fn upgrade_mirror_display_name_hides_full_path() {
        assert_eq!(
            mirror_display_name("https://ghfast.top/https://github.com"),
            "ghfast.top"
        );
    }

    #[test]
    fn upgrade_archive_candidates_prefer_xz_then_keep_gz_compatibility() {
        assert_eq!(
            archive_ext_candidates_for_os("macos", true, false),
            vec!["tar.xz", "tar.gz"]
        );
        assert_eq!(
            archive_ext_candidates_for_os("linux", true, true),
            vec!["tar.gz"]
        );
        assert_eq!(
            archive_ext_candidates_for_os("windows", true, false),
            vec!["zip"]
        );
    }

    #[test]
    fn upgrade_archive_validation_rejects_invalid_tar_xz_before_extract() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("broken.tar.xz");
        std::fs::write(&archive, b"not an xz archive").expect("write archive");

        assert!(validate_downloaded_archive(&archive, "tar.xz").is_err());
    }

    #[test]
    fn upgrade_download_tuning_parses_positive_values() {
        let tuning = DownloadTuning {
            connect_timeout_secs: parse_positive_u64(Some("7"), DOWNLOAD_CONNECT_TIMEOUT_SECS),
            download_timeout_secs: parse_positive_u64(Some("90"), DOWNLOAD_TIMEOUT_SECS),
            mirror_probe_timeout_secs: parse_positive_u64(Some("3"), MIRROR_PROBE_TIMEOUT_SECS),
            download_tries: parse_positive_usize(Some("4"), DOWNLOAD_TRIES),
        };

        assert_eq!(
            tuning,
            DownloadTuning {
                connect_timeout_secs: 7,
                download_timeout_secs: 90,
                mirror_probe_timeout_secs: 3,
                download_tries: 4,
            }
        );
    }

    #[test]
    fn upgrade_download_tuning_rejects_invalid_values() {
        assert_eq!(parse_positive_u64(Some("0"), 5), 5);
        assert_eq!(parse_positive_u64(Some("abc"), 5), 5);
        assert_eq!(parse_positive_usize(Some("0"), 2), 2);
        assert_eq!(parse_positive_usize(Some("abc"), 2), 2);
    }
}
