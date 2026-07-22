use super::*;

pub(super) fn github_mirror_bases() -> Vec<String> {
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

pub(super) fn mirror_display_name(base_url: &str) -> String {
    base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

pub(super) fn github_path_url(base_url: &str, github_path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        github_path.trim_start_matches('/')
    )
}

pub(super) fn probe_github_url(url: &str, tuning: DownloadTuning) -> bool {
    let client = match bifrost_core::github_blocking_reqwest_client_builder()
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

pub(super) fn select_fastest_github_base_from(
    bases: Vec<String>,
    github_path: &str,
    tuning: DownloadTuning,
) -> Option<String> {
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

pub(crate) fn download_progress_line(
    downloaded: u64,
    total: Option<u64>,
    started: Instant,
) -> String {
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

pub(super) fn human_bytes(bytes: u64) -> String {
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

pub(super) fn download_file_with_progress(
    url: &str,
    output_path: &Path,
    tuning: DownloadTuning,
) -> Result<(), BifrostError> {
    let client = bifrost_core::github_blocking_reqwest_client_builder()
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

pub(super) fn download_file_once_with_progress(
    client: &reqwest::blocking::Client,
    url: &str,
    output_path: &Path,
) -> Result<(), BifrostError> {
    let mut response = client.get(url).send().map_err(|error| {
        BifrostError::Network(format!(
            "Failed to download {url} — {}",
            bifrost_core::format_reqwest_error(&error)
        ))
    })?;

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
            crate::commands::upgrade_background::report_download(downloaded, total, started);
        }
    }

    file.flush().map_err(BifrostError::Io)?;
    println!("\r{}", download_progress_line(downloaded, total, started));
    crate::commands::upgrade_background::report_download(downloaded, total, started);

    if downloaded == 0 {
        return Err(BifrostError::Network(format!(
            "Failed to download {} — empty response",
            url
        )));
    }

    Ok(())
}

pub(super) fn ordered_download_bases(github_path: &str, tuning: DownloadTuning) -> Vec<String> {
    let bases = github_mirror_bases();
    let has_preferred_mirror = env::var("BIFROST_GITHUB_MIRROR")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    if has_preferred_mirror {
        // An explicit mirror is an ordering override, not merely another
        // candidate in the latency race. Keep it first and retain the built-in
        // mirrors as deterministic fallbacks.
        bases
    } else {
        ordered_download_bases_from(bases, github_path, tuning)
    }
}

pub(super) fn ordered_download_bases_from(
    bases: Vec<String>,
    github_path: &str,
    tuning: DownloadTuning,
) -> Vec<String> {
    let selected = select_fastest_github_base_from(bases.clone(), github_path, tuning);
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

pub(super) fn archive_ext_candidates_for_os(
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

pub(super) fn tar_supports_xz() -> bool {
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

pub(super) fn release_archive_ext_candidates() -> Vec<&'static str> {
    let os = if cfg!(windows) { "windows" } else { "unix" };
    archive_ext_candidates_for_os(
        os,
        tar_supports_xz(),
        env::var("BIFROST_DISABLE_XZ_ARCHIVE").ok().as_deref() == Some("1"),
    )
}

pub(super) fn archive_ext_from_path(path: &Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.ends_with(".tar.xz") {
        Some("tar.xz")
    } else if file_name.ends_with(".tar.gz") {
        Some("tar.gz")
    } else if file_name.ends_with(".zip") {
        Some("zip")
    } else {
        None
    }
}

pub(super) fn upgrade_test_overrides_enabled() -> bool {
    cfg!(debug_assertions)
        || env::var("BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES")
            .ok()
            .as_deref()
            == Some("1")
}

pub(super) fn test_upgrade_archive_override(
) -> Result<Option<(PathBuf, &'static str)>, BifrostError> {
    if !upgrade_test_overrides_enabled() {
        return Ok(None);
    }

    let Some(path) = env::var_os("BIFROST_UPGRADE_TEST_ARCHIVE").map(PathBuf::from) else {
        return Ok(None);
    };
    let archive_ext = archive_ext_from_path(&path).ok_or_else(|| {
        BifrostError::Config(format!(
            "BIFROST_UPGRADE_TEST_ARCHIVE must point to .tar.xz, .tar.gz, or .zip: {}",
            path.display()
        ))
    })?;
    Ok(Some((path, archive_ext)))
}

pub(super) fn test_upgrade_latest_version_override() -> Option<VersionCache> {
    if !upgrade_test_overrides_enabled() {
        return None;
    }

    env::var("BIFROST_UPGRADE_TEST_LATEST_VERSION")
        .ok()
        .map(|latest_version| VersionCache {
            latest_version,
            release_highlights: vec!["local upgrade restart e2e".to_string()],
            checked_at: chrono::Utc::now(),
        })
}

pub(super) fn validate_downloaded_archive(
    path: &Path,
    archive_ext: &str,
) -> Result<(), BifrostError> {
    if archive_ext == "zip" {
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

pub(super) fn print_update_info(current: &str, cache: &VersionCache) {
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

pub(super) fn upgrade_via_homebrew(target_version: &str) -> Result<(), BifrostError> {
    println!("{}", "Refreshing Homebrew tap...".bright_cyan());

    let output = command_output_with_timeout(
        Path::new("brew"),
        &[
            "--repository".to_string(),
            "bifrost-proxy/bifrost".to_string(),
        ],
        Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
    );

    if let Ok(output) = output {
        if output.status == TimedCommandStatus::Success {
            let tap_path = output.stdout.trim();
            if !tap_path.is_empty() {
                let _ = command_status_with_timeout(
                    Path::new("git"),
                    &["-C", tap_path, "fetch", "--all", "-q"],
                    Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
                );
                let _ = command_status_with_timeout(
                    Path::new("git"),
                    &["-C", tap_path, "reset", "--hard", "origin/main", "-q"],
                    Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
                );
            }
        }
    }

    println!("{}", "Upgrading via Homebrew...".bright_cyan());

    let status = command_status_with_timeout_streaming(
        Path::new("brew"),
        &["reinstall", HOMEBREW_FORMULA_NAME],
        Duration::from_secs(HOMEBREW_COMMAND_TIMEOUT_SECS),
    );

    let success = match status {
        Ok(TimedCommandStatus::Success) => true,
        _ => {
            println!(
                "{}",
                "Standard install failed, trying --build-from-source...".bright_yellow()
            );
            command_status_with_timeout_streaming(
                Path::new("brew"),
                &["reinstall", "--build-from-source", HOMEBREW_FORMULA_NAME],
                Duration::from_secs(HOMEBREW_COMMAND_TIMEOUT_SECS),
            )? == TimedCommandStatus::Success
        }
    };

    if !success {
        return Err(BifrostError::Network(format!(
            "Homebrew upgrade failed. Try: brew reinstall {}",
            HOMEBREW_FORMULA_NAME
        )));
    }

    let output = command_output_with_timeout(
        Path::new("brew"),
        &[
            "info".to_string(),
            "--json=v2".to_string(),
            HOMEBREW_FORMULA_NAME.to_string(),
        ],
        Duration::from_secs(HOMEBREW_METADATA_TIMEOUT_SECS),
    )?;

    if output.status == TimedCommandStatus::Success {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&output.stdout) {
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
