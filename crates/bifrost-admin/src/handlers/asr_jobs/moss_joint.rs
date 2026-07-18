const MOSS_RUNTIME_ASSET_STEM: &str = "moss-joint-runtime";
const MOSS_MODEL_FILE: &str = "moss-transcribe-q5_0.gguf";
const MOSS_MODEL_URL: &str =
    "https://huggingface.co/mudler/moss-transcribe.cpp-gguf/resolve/main/moss-transcribe-q5_0.gguf";
const MOSS_MODEL_BYTES: u64 = 648_174_592;
const MOSS_MODEL_SHA256: &str =
    "7e9ce1de5648ed49fc5c4f5e003d61a7421a63c14074f7275dc8a8cc664ff865";
const MOSS_MAX_PROMPT_CHARS: usize = 4_000;
const MOSS_CONTEXT_TOKENS: u64 = 131_072;
const MOSS_CONTEXT_MARGIN_TOKENS: u64 = 2_048;
const MOSS_AUDIO_TOKENS_PER_SECOND: u64 = 18;
const MOSS_OUTPUT_TOKENS_PER_SECOND: u64 = 20;
const MOSS_MIN_OUTPUT_TOKENS: u64 = 5_120;
const MOSS_MAX_WHOLE_FILE_SECONDS: u64 = 3_300;

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
    binary: PathBuf,
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
    asr_home.join("moss_joint")
}

fn moss_runtime_paths(asr_home: &Path) -> MossRuntimePaths {
    let root = moss_runtime_dir(asr_home);
    MossRuntimePaths {
        binary: root.join("moss-transcribe"),
        model: root.join(MOSS_MODEL_FILE),
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

fn install_moss_runtime_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive)
        .map_err(|error| format!("open MOSS runtime archive {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("read MOSS runtime archive {}: {error}", archive.display()))?;
    let mut binary_entry = None;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|error| format!("read MOSS runtime archive entry: {error}"))?;
        if entry.name().ends_with("/moss-transcribe") || entry.name() == "moss-transcribe" {
            binary_entry = Some(index);
            break;
        }
    }
    let index = binary_entry.ok_or_else(|| {
        format!(
            "MOSS runtime archive {} does not contain moss-transcribe",
            archive.display()
        )
    })?;
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("create MOSS runtime dir {}: {error}", destination.display()))?;
    let mut entry = zip
        .by_index(index)
        .map_err(|error| format!("read MOSS runtime binary: {error}"))?;
    let binary = destination.join("moss-transcribe");
    let mut output = std::fs::File::create(&binary)
        .map_err(|error| format!("create MOSS runtime binary {}: {error}", binary.display()))?;
    std::io::copy(&mut entry, &mut output)
        .map_err(|error| format!("extract MOSS runtime binary {}: {error}", binary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = output
            .metadata()
            .map_err(|error| format!("stat MOSS runtime binary: {error}"))?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions)
            .map_err(|error| format!("mark MOSS runtime executable: {error}"))?;
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
        .windows(b"usage: moss-transcribe".len())
        .any(|window| window == b"usage: moss-transcribe")
}

async fn verify_moss_runtime_binary(path: &Path) -> Result<(), String> {
    let output = Command::new(path)
        .output()
        .await
        .map_err(|error| format!("run MOSS runtime smoke check {}: {error}", path.display()))?;
    if moss_runtime_help_is_valid(&output.stdout, &output.stderr) {
        Ok(())
    } else {
        Err(format!(
            "MOSS runtime smoke check failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

async fn moss_runtime_status(
    paths: &MossRuntimePaths,
    model_spec: &MossModelSpec,
) -> (bool, bool) {
    let runtime_valid = paths.binary.is_file()
        && verify_moss_runtime_binary(&paths.binary).await.is_ok();
    let model_valid = paths.model.is_file()
        && tokio::task::spawn_blocking({
            let model = paths.model.clone();
            let model_spec = model_spec.clone();
            move || verify_moss_model(&model, &model_spec).is_ok()
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
            "Downloading the native runtime and verified Q5 model on first use".to_string(),
        );
    });
    let root = moss_runtime_dir(asr_home);
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| format!("create MOSS runtime dir {}: {error}", root.display()))?;

    if !runtime_valid {
        if paths.binary.exists() {
            let invalid = root.join(format!("moss-transcribe.invalid-{}", now_ms()));
            tokio::fs::rename(&paths.binary, &invalid)
                .await
                .map_err(|error| {
                    format!(
                        "quarantine invalid MOSS runtime {}: {error}",
                        paths.binary.display()
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
    verify_moss_runtime_binary(&paths.binary).await?;

    if !model_valid {
        if paths.model.exists() {
            let invalid = root.join(format!("{MOSS_MODEL_FILE}.invalid-{}", now_ms()));
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
            "MOSS Q5 model",
        )
        .await?;
    }
    let model = paths.model.clone();
    let model_spec = model_spec.clone();
    tokio::task::spawn_blocking(move || verify_moss_model(&model, &model_spec))
        .await
        .map_err(|error| format!("join MOSS model verification: {error}"))??;
    Ok(())
}

async fn ensure_moss_joint_runtime(asr_home: &Path, task_id: &str) -> Result<MossRuntimePaths, String> {
    let _guard = MOSS_INIT_LOCK.lock().await;
    let paths = moss_runtime_paths(asr_home);
    let model_spec = moss_model_spec();
    let (runtime_valid, model_valid) = moss_runtime_status(&paths, &model_spec).await;
    if runtime_valid && model_valid {
        return Ok(paths);
    }
    let asset = moss_runtime_asset_name()?;
    let runtime_source = MossRuntimeSource {
        url: moss_runtime_url(&asset),
        sha256: expected_moss_runtime_checksum(&asset).await?,
        asset,
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
    let mut command = Command::new(&runtime.binary);
    command
        .arg("transcribe")
        .arg(&runtime.model)
        .arg(wav)
        .arg("--max-new")
        .arg(max_new.to_string())
        .arg("--format")
        .arg("json")
        .env("MTD_THREADS", "8")
        // The pinned GGML revision can leave a Metal residency set registered
        // during process teardown and abort after otherwise successful
        // inference. Disabling this optional cache keeps the native runtime
        // stable across Apple Silicon generations without changing results.
        .env("GGML_METAL_NO_RESIDENCY", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(prompt_file) = prompt_file.as_ref() {
        command.arg("--prompt-file").arg(prompt_file.path());
    }
    let child = command.spawn().map_err(|error| {
        format!("start MOSS joint transcription runtime {}: {error}", runtime.binary.display())
    })?;
    let output = child.wait_with_output();
    tokio::pin!(output);
    loop {
        tokio::select! {
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
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if pause_check.is_some_and(|check| check()) {
                    return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
                }
            }
        }
    }
}
