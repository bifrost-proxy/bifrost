const MOSS_RUNTIME_ASSET_STEM: &str = "moss-joint-runtime";
const MOSS_MODEL_FILE: &str = "model.safetensors";
const MOSS_MODEL_URL: &str =
    "https://huggingface.co/majentik/MOSS-Transcribe-Diarize-MLX-8bit/resolve/90c3a1ab78fa56e47e1493ddea48e3ababaf2f71/model.safetensors";
const MOSS_MODEL_BYTES: u64 = 1_258_427_442;
const MOSS_MODEL_SHA256: &str =
    "469a8969e6b70c8b276411eca54a355a27de9ed6794f738dab53f4ffd3c83190";
const MOSS_MODEL_REQUIRED_FILES: &[&str] = &[
    "config.json",
    "preprocessor_config.json",
    "processor_config.json",
    "tokenizer.json",
    "tokenizer_config.json",
];
const MOSS_MAX_PROMPT_CHARS: usize = 4_000;
const MOSS_CONTEXT_TOKENS: u64 = 131_072;
const MOSS_CONTEXT_MARGIN_TOKENS: u64 = 2_048;
const MOSS_AUDIO_TOKENS_PER_SECOND: u64 = 13;
const MOSS_OUTPUT_TOKENS_PER_SECOND: u64 = 20;
const MOSS_MIN_OUTPUT_TOKENS: u64 = 5_120;
const MOSS_MAX_WHOLE_FILE_SECONDS: u64 = 3_300;
const MOSS_MAX_RUNTIME_RTF: f64 = 0.5;

static MOSS_INIT_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Deserialize)]
struct MossJsonSegment {
    start: f64,
    end: f64,
    speaker: String,
    text: String,
}

#[derive(Debug, Clone)]
struct MossRuntimePaths {
    python_home: PathBuf,
    python: PathBuf,
    site_packages: PathBuf,
    runner: PathBuf,
    model_dir: PathBuf,
    model: PathBuf,
}

#[derive(Debug, Clone)]
struct MossRuntimeSource {
    asset: String,
    url: String,
    sha256: String,
}

#[derive(Debug, Clone)]
struct MossModelSpec {
    url: String,
    bytes: u64,
    sha256: String,
}

fn moss_runtime_dir(asr_home: &Path) -> PathBuf {
    asr_home.join("moss_joint_mlx")
}

fn moss_runtime_paths(asr_home: &Path) -> MossRuntimePaths {
    let root = moss_runtime_dir(asr_home);
    let runtime = root.join("runtime");
    let python_home = runtime.join("python");
    let model_dir = root.join("model");
    MossRuntimePaths {
        python: python_home.join("bin/python3.12"),
        python_home,
        site_packages: runtime.join("site-packages"),
        runner: runtime.join("moss_mlx_runner.py"),
        model: model_dir.join(MOSS_MODEL_FILE),
        model_dir,
    }
}

fn moss_runtime_asset_name_for(os: &str, arch: &str) -> Result<String, String> {
    if os == "macos" && arch == "aarch64" {
        Ok(format!(
            "{MOSS_RUNTIME_ASSET_STEM}-v{}-aarch64-apple-darwin.zip",
            env!("CARGO_PKG_VERSION")
        ))
    } else {
        Err(format!(
            "MOSS joint transcription is currently supported only on Apple Silicon macOS; current platform is {}-{}",
            os, arch
        ))
    }
}

fn moss_runtime_asset_name() -> Result<String, String> {
    moss_runtime_asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn moss_runtime_url(asset: &str) -> String {
    std::env::var("BIFROST_MOSS_RUNTIME_URL").unwrap_or_else(|_| {
        format!(
            "https://github.com/bifrost-proxy/bifrost/releases/download/v{}/{asset}",
            env!("CARGO_PKG_VERSION")
        )
    })
}

fn moss_runtime_checksums_url() -> String {
    format!(
        "https://github.com/bifrost-proxy/bifrost/releases/download/v{}/bifrost-v{}-checksums.txt",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_VERSION")
    )
}

fn normalize_sha256(value: &str, label: &str) -> Result<String, String> {
    let checksum = value.trim().to_ascii_lowercase();
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid SHA-256 for {label}"));
    }
    Ok(checksum)
}

fn parse_runtime_checksum_manifest(manifest: &str, asset: &str) -> Result<String, String> {
    manifest
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let checksum = fields.next()?;
            let path = fields.next()?;
            (Path::new(path).file_name()?.to_str()? == asset).then_some(checksum)
        })
        .ok_or_else(|| format!("release checksums do not contain {asset}"))
        .and_then(|checksum| normalize_sha256(checksum, asset))
}

async fn download_runtime_checksum(url: String, asset: &str) -> Result<String, String> {
    let client = bifrost_core::outbound_reqwest_client_builder()
        .build()
        .map_err(|error| format!("build MOSS checksum client: {error}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("download MOSS release checksums: {error}"))?
        .error_for_status()
        .map_err(|error| format!("download MOSS release checksums: {error}"))?;
    let manifest = response
        .text()
        .await
        .map_err(|error| format!("read MOSS release checksums: {error}"))?;
    parse_runtime_checksum_manifest(&manifest, asset)
}

async fn expected_moss_runtime_checksum(asset: &str) -> Result<String, String> {
    if let Ok(checksum) = std::env::var("BIFROST_MOSS_RUNTIME_SHA256") {
        return normalize_sha256(&checksum, asset);
    }
    if std::env::var_os("BIFROST_MOSS_RUNTIME_URL").is_some() {
        return Err(
            "BIFROST_MOSS_RUNTIME_SHA256 is required with BIFROST_MOSS_RUNTIME_URL".to_string(),
        );
    }
    download_runtime_checksum(moss_runtime_checksums_url(), asset).await
}

async fn moss_runtime_source_for_asset(asset: String) -> Result<MossRuntimeSource, String> {
    Ok(MossRuntimeSource {
        url: moss_runtime_url(&asset),
        sha256: expected_moss_runtime_checksum(&asset).await?,
        asset,
    })
}

fn moss_model_url() -> String {
    std::env::var("BIFROST_MOSS_MODEL_URL").unwrap_or_else(|_| MOSS_MODEL_URL.to_string())
}

fn moss_model_spec() -> MossModelSpec {
    MossModelSpec {
        url: moss_model_url(),
        bytes: MOSS_MODEL_BYTES,
        sha256: MOSS_MODEL_SHA256.to_string(),
    }
}

fn moss_output_token_budget(duration_ms: u64) -> Result<u32, String> {
    let duration_seconds = duration_ms.div_ceil(1_000).max(1);
    if duration_seconds > MOSS_MAX_WHOLE_FILE_SECONDS {
        return Err(format!(
            "moss_audio_too_long: whole-file joint transcription currently supports at most {} minutes; audio is {} minutes",
            MOSS_MAX_WHOLE_FILE_SECONDS / 60,
            duration_seconds.div_ceil(60)
        ));
    }
    let wanted = duration_seconds
        .saturating_mul(MOSS_OUTPUT_TOKENS_PER_SECOND)
        .max(MOSS_MIN_OUTPUT_TOKENS);
    let input = duration_seconds.saturating_mul(MOSS_AUDIO_TOKENS_PER_SECOND);
    let available = MOSS_CONTEXT_TOKENS
        .saturating_sub(MOSS_CONTEXT_MARGIN_TOKENS)
        .saturating_sub(input);
    Ok(wanted.min(available).max(MOSS_MIN_OUTPUT_TOKENS) as u32)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("open {} for hashing: {error}", path.display()))?;
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("read {} for hashing: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_moss_model(path: &Path, spec: &MossModelSpec) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("stat MOSS model {}: {error}", path.display()))?;
    if metadata.len() != spec.bytes {
        return Err(format!(
            "MOSS model size mismatch: expected {}, got {}",
            spec.bytes, metadata.len()
        ));
    }
    let actual = sha256_file(path)?;
    if actual != spec.sha256 {
        return Err(format!(
            "MOSS model checksum mismatch: expected {}, got {actual}",
            spec.sha256
        ));
    }
    Ok(())
}

fn verify_moss_model_snapshot(paths: &MossRuntimePaths, spec: &MossModelSpec) -> Result<(), String> {
    verify_moss_model(&paths.model, spec)?;
    for file in MOSS_MODEL_REQUIRED_FILES {
        let required = paths.model_dir.join(file);
        if !required.is_file() {
            return Err(format!(
                "MOSS model snapshot is missing {}",
                required.display()
            ));
        }
    }
    Ok(())
}

fn install_moss_runtime_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive)
        .map_err(|error| format!("open MOSS runtime archive {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("read MOSS runtime archive {}: {error}", archive.display()))?;
    let archive_root = Path::new("moss-joint-runtime");
    let mut extracted_runner = false;
    let mut extracted_python = false;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("read MOSS runtime archive entry: {error}"))?;
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            format!("unsafe MOSS runtime archive entry {}", entry.name())
        })?;
        let Ok(relative) = enclosed.strip_prefix(archive_root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        // macOS zip tools may encode resource forks as AppleDouble `._*`
        // sidecars. They are not model/runtime inputs and some Python loaders
        // will otherwise discover them as malformed JSON or source files.
        if relative.components().any(|component| {
            let name = component.as_os_str().to_string_lossy();
            name.starts_with("._") || name == ".DS_Store" || name == "__MACOSX"
        }) {
            continue;
        }
        if !relative.starts_with("runtime") && !relative.starts_with("model") {
            continue;
        }
        let target = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|error| {
                format!("create MOSS runtime directory {}: {error}", target.display())
            })?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("create MOSS runtime directory {}: {error}", parent.display())
            })?;
        }
        let mut output = std::fs::File::create(&target)
            .map_err(|error| format!("create MOSS runtime file {}: {error}", target.display()))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("extract MOSS runtime file {}: {error}", target.display()))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).map_err(
                |error| format!("set MOSS runtime permissions {}: {error}", target.display()),
            )?;
        }
        extracted_runner |= relative == Path::new("runtime/moss_mlx_runner.py");
        extracted_python |= relative == Path::new("runtime/python/bin/python3.12");
    }
    if !extracted_runner || !extracted_python {
        return Err(format!(
            "MOSS runtime archive {} does not contain the packaged MLX runner",
            archive.display()
        ));
    }
    Ok(())
}

async fn download_moss_resource(url: String, dest: PathBuf, label: &str) -> Result<(), String> {
    if let Some(path) = url.strip_prefix("file://") {
        tokio::fs::copy(path, &dest)
            .await
            .map_err(|error| format!("copy {label} from {path}: {error}"))?;
        return Ok(());
    }
    let client = bifrost_core::outbound_reqwest_client_builder()
        .build()
        .map_err(|error| format!("build MOSS download client: {error}"))?;
    crate::resource_download::download_with_resume(
        &client,
        crate::resource_download::DownloadRequest {
            url,
            dest,
            label: label.to_string(),
        },
        None,
    )
    .await
    .map(|_| ())
}

fn moss_runtime_help_is_valid(stdout: &[u8], stderr: &[u8]) -> bool {
    [stdout, stderr]
        .concat()
        .windows(b"moss-mlx-runtime ok".len())
        .any(|window| window == b"moss-mlx-runtime ok")
}

fn configure_moss_python_command(command: &mut Command, paths: &MossRuntimePaths) {
    command
        .env("PYTHONHOME", &paths.python_home)
        .env("PYTHONPATH", &paths.site_packages)
        .env("PYTHONNOUSERSITE", "1")
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1");
}

async fn verify_moss_runtime_binary(paths: &MossRuntimePaths) -> Result<(), String> {
    let mut command = Command::new(&paths.python);
    command.arg(&paths.runner).arg("--self-test");
    configure_moss_python_command(&mut command, paths);
    let output = command
        .output()
        .await
        .map_err(|error| {
            format!(
                "run MOSS MLX runtime smoke check {}: {error}",
                paths.python.display()
            )
        })?;
    if moss_runtime_help_is_valid(&output.stdout, &output.stderr) {
        Ok(())
    } else {
        Err(format!(
            "MOSS MLX runtime smoke check failed for {}: {}",
            paths.python.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

async fn moss_runtime_status(
    paths: &MossRuntimePaths,
    model_spec: &MossModelSpec,
) -> (bool, bool) {
    let runtime_valid = paths.python.is_file()
        && paths.runner.is_file()
        && paths.site_packages.is_dir()
        && verify_moss_runtime_binary(paths).await.is_ok();
    let model_valid = paths.model.is_file()
        && tokio::task::spawn_blocking({
            let paths = paths.clone();
            let model_spec = model_spec.clone();
            move || verify_moss_model_snapshot(&paths, &model_spec).is_ok()
        })
        .await
        .unwrap_or(false);
    (runtime_valid, model_valid)
}

async fn initialize_moss_joint_runtime(
    asr_home: &Path,
    task_id: &str,
    paths: &MossRuntimePaths,
    runtime_valid: bool,
    model_valid: bool,
    runtime_source: &MossRuntimeSource,
    model_spec: &MossModelSpec,
) -> Result<(), String> {
    update_run_progress(task_id, |progress| {
        progress.stage = "initializing_moss".to_string();
        progress.stage_message =
            Some("Preparing MOSS joint transcription runtime".to_string());
        progress.message = Some(
            "Downloading the self-contained MLX runtime and verified 8-bit model on first use"
                .to_string(),
        );
    });
    let root = moss_runtime_dir(asr_home);
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| format!("create MOSS runtime dir {}: {error}", root.display()))?;

    if !runtime_valid {
        let runtime_dir = root.join("runtime");
        if runtime_dir.exists() {
            let invalid = root.join(format!("runtime.invalid-{}", now_ms()));
            tokio::fs::rename(&runtime_dir, &invalid)
                .await
                .map_err(|error| {
                    format!(
                        "quarantine invalid MOSS runtime {}: {error}",
                        runtime_dir.display()
                    )
                })?;
        }
        let archive = root.join(&runtime_source.asset);
        download_moss_resource(
            runtime_source.url.clone(),
            archive.clone(),
            "MOSS runtime",
        )
        .await?;
        let archive_for_hash = archive.clone();
        let actual_checksum = tokio::task::spawn_blocking(move || sha256_file(&archive_for_hash))
            .await
            .map_err(|error| format!("join MOSS runtime checksum verification: {error}"))??;
        if actual_checksum != runtime_source.sha256 {
            let invalid = root.join(format!("{}.invalid-{}", runtime_source.asset, now_ms()));
            tokio::fs::rename(&archive, &invalid).await.map_err(|error| {
                format!(
                    "quarantine invalid MOSS runtime archive {}: {error}",
                    archive.display()
                )
            })?;
            return Err(format!(
                "MOSS runtime checksum mismatch: expected {}, got {actual_checksum}",
                runtime_source.sha256
            ));
        }
        let extract_root = root.clone();
        let archive_for_extract = archive.clone();
        tokio::task::spawn_blocking(move || {
            install_moss_runtime_archive(&archive_for_extract, &extract_root)
        })
        .await
        .map_err(|error| format!("join MOSS runtime install: {error}"))??;
        let _ = tokio::fs::remove_file(archive).await;
    }
    verify_moss_runtime_binary(paths).await?;

    if !model_valid {
        tokio::fs::create_dir_all(&paths.model_dir)
            .await
            .map_err(|error| {
                format!(
                    "create MOSS model directory {}: {error}",
                    paths.model_dir.display()
                )
            })?;
        if paths.model.exists() {
            let invalid = paths
                .model_dir
                .join(format!("{MOSS_MODEL_FILE}.invalid-{}", now_ms()));
            tokio::fs::rename(&paths.model, &invalid)
                .await
                .map_err(|error| {
                    format!(
                        "quarantine invalid MOSS model {}: {error}",
                        paths.model.display()
                    )
                })?;
        }
        download_moss_resource(
            model_spec.url.clone(),
            paths.model.clone(),
            "MOSS MLX 8-bit model",
        )
        .await?;
    }
    let paths = paths.clone();
    let model_spec = model_spec.clone();
    tokio::task::spawn_blocking(move || verify_moss_model_snapshot(&paths, &model_spec))
        .await
        .map_err(|error| format!("join MOSS model verification: {error}"))??;
    Ok(())
}

async fn ensure_moss_joint_runtime_with_spec(
    asr_home: &Path,
    task_id: &str,
    model_spec: MossModelSpec,
    runtime_source: Option<MossRuntimeSource>,
) -> Result<MossRuntimePaths, String> {
    let _guard = MOSS_INIT_LOCK.lock().await;
    let paths = moss_runtime_paths(asr_home);
    let (runtime_valid, model_valid) = moss_runtime_status(&paths, &model_spec).await;
    if runtime_valid && model_valid {
        return Ok(paths);
    }
    let runtime_source = match runtime_source {
        Some(source) => source,
        None => moss_runtime_source_for_asset(moss_runtime_asset_name()?).await?,
    };
    initialize_moss_joint_runtime(
        asr_home,
        task_id,
        &paths,
        runtime_valid,
        model_valid,
        &runtime_source,
        &model_spec,
    )
    .await?;
    Ok(paths)
}

async fn ensure_moss_joint_runtime(asr_home: &Path, task_id: &str) -> Result<MossRuntimePaths, String> {
    ensure_moss_joint_runtime_with_spec(asr_home, task_id, moss_model_spec(), None).await
}

fn parse_moss_json(
    stdout: &[u8],
) -> Result<crate::handlers::asr_streaming::WholeFileTranscription, String> {
    let segments: Vec<MossJsonSegment> = serde_json::from_slice(stdout)
        .map_err(|error| format!("parse MOSS runtime JSON: {error}"))?;
    let structured_segments = segments
        .into_iter()
        .filter_map(|segment| {
            let text = segment.text.trim().to_string();
            if text.is_empty() || !segment.start.is_finite() || !segment.end.is_finite() {
                return None;
            }
            let start_ms = (segment.start.max(0.0) * 1_000.0).round() as u64;
            let end_ms = (segment.end.max(segment.start).max(0.0) * 1_000.0).round() as u64;
            Some(bifrost_asr::transcription::TranscriptionSegment {
                start_ms,
                end_ms,
                text,
                speaker: Some(segment.speaker.trim().to_string()).filter(|speaker| !speaker.is_empty()),
                overlap: false,
            })
        })
        .collect::<Vec<_>>();
    let text = structured_segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let plain_segments = structured_segments
        .iter()
        .map(|segment| (segment.start_ms, segment.end_ms, segment.text.clone()))
        .collect();
    Ok(crate::handlers::asr_streaming::WholeFileTranscription {
        text: text.clone(),
        segments: plain_segments,
        structured: bifrost_asr::transcription::StructuredTranscription {
            text,
            segments: structured_segments,
            finish_reason: bifrost_asr::transcription::TranscriptionFinishReason::Completed,
            usage: None,
        },
    })
}

async fn run_moss_joint_transcription(
    runtime: &MossRuntimePaths,
    wav: &Path,
    duration_ms: u64,
    prompt: &str,
    pause_check: Option<&PauseCheckCallback<'_>>,
) -> Result<crate::handlers::asr_streaming::WholeFileTranscription, String> {
    if prompt.chars().count() > MOSS_MAX_PROMPT_CHARS {
        return Err(format!("MOSS prompt exceeds {MOSS_MAX_PROMPT_CHARS} characters"));
    }
    let max_new = moss_output_token_budget(duration_ms)?;
    let prompt_file = if prompt.is_empty() {
        None
    } else {
        use std::io::Write;
        let mut file = tempfile::Builder::new()
            .prefix("bifrost-moss-prompt-")
            .tempfile()
            .map_err(|error| format!("create MOSS prompt file: {error}"))?;
        file.write_all(prompt.as_bytes())
            .map_err(|error| format!("write MOSS prompt file: {error}"))?;
        Some(file)
    };
    let mut command = Command::new(&runtime.python);
    command
        .arg(&runtime.runner)
        .arg("transcribe")
        .arg(&runtime.model_dir)
        .arg(wav)
        .arg("--max-new")
        .arg(max_new.to_string())
        .arg("--format")
        .arg("json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_moss_python_command(&mut command, runtime);
    if let Some(prompt_file) = prompt_file.as_ref() {
        command.arg("--prompt-file").arg(prompt_file.path());
    }
    let child = command.spawn().map_err(|error| {
        format!(
            "start MOSS MLX joint transcription runtime {}: {error}",
            runtime.python.display()
        )
    })?;
    let started = Instant::now();
    let max_runtime = Duration::from_secs_f64(
        (duration_ms.max(1) as f64 / 1_000.0) * MOSS_MAX_RUNTIME_RTF,
    );
    let output = child.wait_with_output();
    tokio::pin!(output);
    let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(started + max_runtime));
    tokio::pin!(deadline);
    let mut pause_poll = tokio::time::interval(Duration::from_millis(500));
    pause_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            result = &mut output => {
                let output = result.map_err(|error| format!("wait for MOSS runtime: {error}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "MOSS runtime failed with {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                return parse_moss_json(&output.stdout);
            }
            _ = &mut deadline => {
                return Err(format!(
                    "moss_rtf_exceeded: inference exceeded {:.1}x audio duration (limit_ms={}, audio_ms={duration_ms})",
                    MOSS_MAX_RUNTIME_RTF,
                    max_runtime.as_millis()
                ));
            }
            _ = pause_poll.tick() => {
                if pause_check.is_some_and(|check| check()) {
                    return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
                }
            }
        }
    }
}
