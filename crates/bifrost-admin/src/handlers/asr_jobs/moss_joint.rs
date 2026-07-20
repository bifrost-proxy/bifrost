const MOSS_RUNTIME_ASSET_STEM: &str = "moss-joint-runtime";
const MOSS_MODEL_ID: &str = "MOSS-Transcribe-Diarize-MLX-8bit";
const MOSS_VERIFICATION_FILE: &str = "verification.json";
const MOSS_VERIFICATION_SCHEMA_VERSION: u8 = 4;
const MOSS_MODEL_FILE: &str = "model.safetensors";
const MOSS_MODEL_URL: &str =
    "https://huggingface.co/majentik/MOSS-Transcribe-Diarize-MLX-8bit/resolve/90c3a1ab78fa56e47e1493ddea48e3ababaf2f71/model.safetensors";
const MOSS_MODEL_BYTES: u64 = 1_258_427_442;
const MOSS_MODEL_SHA256: &str =
    "469a8969e6b70c8b276411eca54a355a27de9ed6794f738dab53f4ffd3c83190";
const MOSS_MODEL_REQUIRED_FILES: &[&str] = &[
    "added_tokens.json",
    "chat_template.jinja",
    "config.json",
    "generation_config.json",
    "merges.txt",
    "model.safetensors.index.json",
    "preprocessor_config.json",
    "processor_config.json",
    "special_tokens_map.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "vocab.json",
];
const MOSS_MAX_PROMPT_CHARS: usize = 4_000;
const MOSS_CONTEXT_TOKENS: u64 = 131_072;
const MOSS_CONTEXT_MARGIN_TOKENS: u64 = 2_048;
const MOSS_AUDIO_TOKENS_PER_SECOND: u64 = 13;
const MOSS_OUTPUT_TOKENS_PER_SECOND: u64 = 20;
const MOSS_MIN_OUTPUT_TOKENS: u64 = 5_120;
const MOSS_MAX_WHOLE_FILE_SECONDS: u64 = 3_300;
const MOSS_MAX_RUNTIME_RTF: f64 = 0.5;
const MOSS_MIN_AUDIO_DURATION_MS: u64 = 10_000;

static MOSS_INIT_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Deserialize)]
struct MossJsonSegment {
    start: f64,
    end: f64,
    speaker: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct MossJsonEnvelope {
    segments: Vec<MossJsonSegment>,
    finish_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MossJsonPayload {
    Envelope(MossJsonEnvelope),
    Legacy(Vec<MossJsonSegment>),
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

#[derive(Debug, Deserialize, Serialize)]
struct MossVerificationMarker {
    schema_version: u8,
    model_bytes: u64,
    model_sha256: String,
    model_modified_ms: u64,
    python_sha256: String,
    runner_sha256: String,
    runtime_site_packages_sha256: BTreeMap<String, String>,
    #[serde(default)]
    model_metadata_sha256: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy)]
struct MossComponentStatus {
    runtime_valid: bool,
    model_weight_valid: bool,
    model_metadata_valid: bool,
}

impl MossComponentStatus {
    fn model_valid(self) -> bool {
        self.model_weight_valid && self.model_metadata_valid
    }

    fn all_valid(self) -> bool {
        self.runtime_valid && self.model_valid()
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MossModelManagementStatus {
    status: &'static str,
    ready: bool,
    installed: bool,
    platform_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unsupported_reason: Option<String>,
    runtime_ready: bool,
    model_ready: bool,
    model: &'static str,
    runtime_asset: Option<String>,
    install_dir: String,
    runtime_dir: String,
    model_dir: String,
    expected_model_bytes: u64,
    installed_model_bytes: u64,
    message: String,
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

fn moss_verification_path(asr_home: &Path) -> PathBuf {
    moss_runtime_dir(asr_home).join(MOSS_VERIFICATION_FILE)
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

fn validate_moss_transcription_mode_for_platform(
    mode: AsrTranscriptionMode,
    os: &str,
    arch: &str,
) -> Result<(), String> {
    if mode != AsrTranscriptionMode::MossJoint {
        return Ok(());
    }
    moss_runtime_asset_name_for(os, arch).map(|_| ())
}

fn validate_moss_transcription_mode(mode: AsrTranscriptionMode) -> Result<(), String> {
    validate_moss_transcription_mode_for_platform(
        mode,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
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

fn validate_moss_audio_input(wav: &Path, duration_ms: u64) -> Result<(), String> {
    if duration_ms == 0 {
        return Err(moss_non_retryable_runtime_error(
            "moss_duration_unavailable: audio duration is required for the 0.5x runtime guard"
        ));
    }
    if duration_ms < MOSS_MIN_AUDIO_DURATION_MS {
        return Err(moss_non_retryable_runtime_error(&format!(
            "moss_audio_too_short: joint speaker-aware transcription requires at least {:.1} seconds under the 0.5x runtime SLA; audio_ms={duration_ms}",
            MOSS_MIN_AUDIO_DURATION_MS as f64 / 1_000.0
        )));
    }
    match compute_wav_rms_energy(wav) {
        Some(rms) if rms < SILENCE_RMS_THRESHOLD => Err(moss_non_retryable_runtime_error(&format!(
            "moss_audio_silent: normalized audio RMS {rms:.2} is below the safe speech threshold {SILENCE_RMS_THRESHOLD:.2}"
        ))),
        Some(_) => Ok(()),
        None => Err(moss_non_retryable_runtime_error(
            "moss_audio_invalid: normalized WAV energy could not be measured",
        )),
    }
}

fn moss_remaining_runtime_budget(
    duration_ms: u64,
    file_started_at_ms: Option<u64>,
) -> Result<Duration, String> {
    let limit_ms = ((duration_ms as f64) * MOSS_MAX_RUNTIME_RTF).floor() as u64;
    let elapsed_ms = file_started_at_ms
        .map(|started_at_ms| now_ms().saturating_sub(started_at_ms))
        .unwrap_or(0);
    let remaining_ms = limit_ms.saturating_sub(elapsed_ms);
    if remaining_ms == 0 {
        return Err(format!(
            "moss_rtf_exceeded: end-to-end processing exhausted {:.1}x audio duration before inference (limit_ms={limit_ms}, elapsed_ms={elapsed_ms}, audio_ms={duration_ms})",
            MOSS_MAX_RUNTIME_RTF
        ));
    }
    Ok(Duration::from_millis(remaining_ms))
}

fn moss_non_retryable_runtime_error(error: &str) -> String {
    format!(
        "moss_non_retryable_v{}: {error}",
        env!("CARGO_PKG_VERSION")
    )
}

fn moss_runtime_error_is_deterministic(error: &str) -> bool {
    error.contains("no complete speaker-aware segment before")
        || error.contains("no valid speaker-aware segments")
        || error.contains("degenerate repetitive transcription")
        || error.contains("no positive-duration speaker-aware segments")
        || error.contains("max-new token limit before completion")
}

fn moss_failure_is_non_retryable_for_unchanged_source(
    task: &AsrDirectoryTask,
    path: &Path,
    record: &FileRecord,
) -> bool {
    if task.transcription_mode != AsrTranscriptionMode::MossJoint
        || record.status != FileStatus::Failed
    {
        return false;
    }
    let Some(error) = record.error.as_deref() else {
        return false;
    };
    let versioned_prefix = format!("moss_non_retryable_v{}:", env!("CARGO_PKG_VERSION"));
    // The unversioned forms only existed in the local 0.0.156 pre-release
    // validation. Do not carry this compatibility forward: a later runtime
    // version must be allowed to retry the source with improved decoding.
    let legacy_unversioned = env!("CARGO_PKG_VERSION") == "0.0.156"
        && (error.contains("moss_duration_unavailable:")
            || error.contains("moss_audio_too_short:")
            || error.contains("moss_audio_silent:")
            || error.contains("moss_audio_invalid:")
            || moss_runtime_error_is_deterministic(error));
    let deterministic = error.starts_with(&versioned_prefix) || legacy_unversioned;
    deterministic
        && record.source_size.is_some()
        && record.source_modified_ms.is_some()
        && record.source_size == source_size(path)
        && record.source_modified_ms == source_modified_ms(path)
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

fn moss_model_metadata_sha256(
    paths: &MossRuntimePaths,
) -> Result<BTreeMap<String, String>, String> {
    MOSS_MODEL_REQUIRED_FILES
        .iter()
        .map(|file| {
            let path = paths.model_dir.join(file);
            sha256_file(&path).map(|sha256| ((*file).to_string(), sha256))
        })
        .collect()
}

fn moss_model_layout_is_complete(paths: &MossRuntimePaths, spec: &MossModelSpec) -> bool {
    std::fs::metadata(&paths.model)
        .is_ok_and(|metadata| metadata.len() == spec.bytes)
        && MOSS_MODEL_REQUIRED_FILES
            .iter()
            .all(|file| paths.model_dir.join(file).is_file())
}

fn moss_site_packages_sha256(paths: &MossRuntimePaths) -> Result<BTreeMap<String, String>, String> {
    fn collect(
        root: &Path,
        directory: &Path,
        checksums: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        let entries = std::fs::read_dir(directory).map_err(|error| {
            format!(
                "read MOSS site-packages directory {}: {error}",
                directory.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "read MOSS site-packages entry in {}: {error}",
                    directory.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                format!("inspect MOSS site-packages entry {}: {error}", path.display())
            })?;
            if file_type.is_dir() {
                if entry.file_name() != "__pycache__" {
                    collect(root, &path, checksums)?;
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str());
            if matches!(extension, Some("pyc" | "pyo")) {
                continue;
            }
            let relative = path.strip_prefix(root).map_err(|error| {
                format!("resolve MOSS site-packages path {}: {error}", path.display())
            })?;
            checksums.insert(relative.to_string_lossy().into_owned(), sha256_file(&path)?);
        }
        Ok(())
    }

    let mut checksums = BTreeMap::new();
    collect(&paths.site_packages, &paths.site_packages, &mut checksums)?;
    if checksums.is_empty() {
        return Err(format!(
            "MOSS site-packages directory {} contains no packaged dependencies",
            paths.site_packages.display()
        ));
    }
    Ok(checksums)
}

fn write_moss_verification_marker(
    asr_home: &Path,
    paths: &MossRuntimePaths,
    spec: &MossModelSpec,
) -> Result<(), String> {
    let model_modified_ms = std::fs::metadata(&paths.model)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .ok_or_else(|| format!("read MOSS model mtime {}", paths.model.display()))?;
    let marker = MossVerificationMarker {
        schema_version: MOSS_VERIFICATION_SCHEMA_VERSION,
        model_bytes: spec.bytes,
        model_sha256: spec.sha256.clone(),
        model_modified_ms,
        python_sha256: sha256_file(&paths.python)?,
        runner_sha256: sha256_file(&paths.runner)?,
        runtime_site_packages_sha256: moss_site_packages_sha256(paths)?,
        model_metadata_sha256: moss_model_metadata_sha256(paths)?,
    };
    let encoded = serde_json::to_vec_pretty(&marker)
        .map_err(|error| format!("serialize MOSS verification marker: {error}"))?;
    let path = moss_verification_path(asr_home);
    std::fs::write(&path, encoded)
        .map_err(|error| format!("write MOSS verification marker {}: {error}", path.display()))
}

fn moss_verified_component_status(
    asr_home: &Path,
    paths: &MossRuntimePaths,
    spec: &MossModelSpec,
) -> (bool, bool) {
    let marker = std::fs::read(moss_verification_path(asr_home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<MossVerificationMarker>(&bytes).ok());
    let Some(marker) = marker.filter(|marker| {
        marker.schema_version == MOSS_VERIFICATION_SCHEMA_VERSION
            && marker.model_bytes == spec.bytes
            && marker.model_sha256 == spec.sha256
    }) else {
        return (false, false);
    };
    let runtime_ready = paths.python.is_file()
        && paths.runner.is_file()
        && paths.site_packages.is_dir()
        && sha256_file(&paths.python).is_ok_and(|sha| sha == marker.python_sha256)
        && sha256_file(&paths.runner).is_ok_and(|sha| sha == marker.runner_sha256)
        && moss_site_packages_sha256(paths)
            .is_ok_and(|checksums| checksums == marker.runtime_site_packages_sha256);
    let model_modified_ms = std::fs::metadata(&paths.model)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64);
    let model_ready = moss_model_layout_is_complete(paths, spec)
        && model_modified_ms == Some(marker.model_modified_ms)
        && moss_model_metadata_sha256(paths)
            .is_ok_and(|metadata| metadata == marker.model_metadata_sha256);
    (runtime_ready, model_ready)
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

async fn download_moss_resource(
    url: String,
    dest: PathBuf,
    label: &str,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::resource_download::DownloadProgress>>,
) -> Result<(), String> {
    if let Some(path) = url.strip_prefix("file://") {
        let bytes = tokio::fs::copy(path, &dest)
            .await
            .map_err(|error| format!("copy {label} from {path}: {error}"))?;
        if let Some(tx) = progress_tx {
            let _ = tx.send(crate::resource_download::DownloadProgress {
                label: label.to_string(),
                url,
                dest: dest.display().to_string(),
                downloaded_bytes: bytes,
                total_bytes: Some(bytes),
                percent: Some(100),
                bytes_per_second: None,
                eta_seconds: Some(0),
                elapsed_ms: 0,
                resumed: false,
                complete: true,
            });
        }
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
        progress_tx,
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
    asr_home: &Path,
    paths: &MossRuntimePaths,
    model_spec: &MossModelSpec,
) -> MossComponentStatus {
    let marker = std::fs::read(moss_verification_path(asr_home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<MossVerificationMarker>(&bytes).ok())
        .filter(|marker| {
            marker.schema_version == MOSS_VERIFICATION_SCHEMA_VERSION
                && marker.model_bytes == model_spec.bytes
                && marker.model_sha256 == model_spec.sha256
        });
    let model_metadata_valid = marker.as_ref().is_some_and(|marker| {
        moss_model_metadata_sha256(paths)
            .is_ok_and(|metadata| metadata == marker.model_metadata_sha256)
    });
    let runtime_valid = paths.python.is_file()
        && paths.runner.is_file()
        && paths.site_packages.is_dir()
        && marker.as_ref().is_some_and(|marker| {
            moss_site_packages_sha256(paths)
                .is_ok_and(|checksums| checksums == marker.runtime_site_packages_sha256)
        })
        && verify_moss_runtime_binary(paths).await.is_ok();
    let model_weight_valid = paths.model.is_file()
        && tokio::task::spawn_blocking({
            let paths = paths.clone();
            let model_spec = model_spec.clone();
            move || verify_moss_model_snapshot(&paths, &model_spec).is_ok()
        })
        .await
        .unwrap_or(false);
    MossComponentStatus {
        runtime_valid,
        model_weight_valid,
        model_metadata_valid,
    }
}

async fn initialize_moss_joint_runtime(
    asr_home: &Path,
    task_id: &str,
    paths: &MossRuntimePaths,
    component_status: MossComponentStatus,
    runtime_source: &MossRuntimeSource,
    model_spec: &MossModelSpec,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::resource_download::DownloadProgress>>,
) -> Result<(), String> {
    if !task_id.is_empty() {
        update_run_progress(task_id, |progress| {
            progress.stage = "initializing_moss".to_string();
            progress.stage_message =
                Some("Preparing MOSS joint transcription runtime".to_string());
            progress.message = Some(
                "Downloading the self-contained MLX runtime and verified 8-bit model on first use"
                    .to_string(),
            );
        });
    }
    let root = moss_runtime_dir(asr_home);
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| format!("create MOSS runtime dir {}: {error}", root.display()))?;

    if !component_status.runtime_valid || !component_status.model_metadata_valid {
        let runtime_dir = root.join("runtime");
        if !component_status.runtime_valid && runtime_dir.exists() {
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
            progress_tx.clone(),
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

    if !component_status.model_weight_valid {
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
            progress_tx,
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
    ensure_moss_joint_runtime_with_spec_and_progress(
        asr_home,
        task_id,
        model_spec,
        runtime_source,
        None,
    )
    .await
}

async fn ensure_moss_joint_runtime_with_spec_and_progress(
    asr_home: &Path,
    task_id: &str,
    model_spec: MossModelSpec,
    runtime_source: Option<MossRuntimeSource>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::resource_download::DownloadProgress>>,
) -> Result<MossRuntimePaths, String> {
    let _guard = MOSS_INIT_LOCK.lock().await;
    let paths = moss_runtime_paths(asr_home);
    let component_status = moss_runtime_status(asr_home, &paths, &model_spec).await;
    if component_status.all_valid() {
        write_moss_verification_marker(asr_home, &paths, &model_spec)?;
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
        component_status,
        &runtime_source,
        &model_spec,
        progress_tx,
    )
    .await?;
    write_moss_verification_marker(asr_home, &paths, &model_spec)?;
    Ok(paths)
}

async fn ensure_moss_joint_runtime(asr_home: &Path, task_id: &str) -> Result<MossRuntimePaths, String> {
    ensure_moss_joint_runtime_with_spec(asr_home, task_id, moss_model_spec(), None).await
}

async fn moss_management_status_with_spec(
    asr_home: &Path,
    os: &str,
    arch: &str,
    model_spec: &MossModelSpec,
) -> MossModelManagementStatus {
    let paths = moss_runtime_paths(asr_home);
    let install_dir = moss_runtime_dir(asr_home);
    let runtime_dir = install_dir.join("runtime");
    let installed_model_bytes = tokio::fs::metadata(&paths.model)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let runtime_asset = match moss_runtime_asset_name_for(os, arch) {
        Ok(asset) => asset,
        Err(reason) => {
            return MossModelManagementStatus {
                status: "unsupported",
                ready: false,
                installed: false,
                platform_supported: false,
                unsupported_reason: Some(reason.clone()),
                runtime_ready: false,
                model_ready: false,
                model: MOSS_MODEL_ID,
                runtime_asset: None,
                install_dir: install_dir.display().to_string(),
                runtime_dir: runtime_dir.display().to_string(),
                model_dir: paths.model_dir.display().to_string(),
                expected_model_bytes: model_spec.bytes,
                installed_model_bytes,
                message: reason,
            };
        }
    };
    let (runtime_ready, model_ready) = moss_verified_component_status(asr_home, &paths, model_spec);
    let ready = runtime_ready && model_ready;
    let status = if ready {
        "ready"
    } else if runtime_ready || model_ready {
        "partial"
    } else {
        "missing"
    };
    MossModelManagementStatus {
        status,
        ready,
        installed: ready,
        platform_supported: true,
        unsupported_reason: None,
        runtime_ready,
        model_ready,
        model: MOSS_MODEL_ID,
        runtime_asset: Some(runtime_asset),
        install_dir: install_dir.display().to_string(),
        runtime_dir: runtime_dir.display().to_string(),
        model_dir: paths.model_dir.display().to_string(),
        expected_model_bytes: model_spec.bytes,
        installed_model_bytes,
        message: if ready {
            "MOSS joint transcription runtime and verified 8-bit model are ready.".to_string()
        } else if runtime_ready {
            "MOSS runtime is ready, but the verified 8-bit model is missing or invalid."
                .to_string()
        } else if model_ready {
            "MOSS model is verified, but the packaged MLX runtime is missing or invalid."
                .to_string()
        } else {
            "MOSS runtime and verified 8-bit model are not initialized yet.".to_string()
        },
    }
}

pub(crate) async fn handle_moss_model_status() -> Response<BoxBody> {
    json_response(
        &moss_management_status_with_spec(
            &fixed_asr_home(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            &moss_model_spec(),
        )
        .await,
    )
}

pub(crate) async fn stream_moss_model_initialization(
    tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
) {
    stream_moss_model_initialization_with_spec(
        tx,
        fixed_asr_home(),
        moss_runtime_asset_name(),
        moss_model_spec(),
        None,
    )
    .await;
}

async fn stream_moss_model_initialization_with_spec(
    tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
    asr_home: PathBuf,
    asset: Result<String, String>,
    model_spec: MossModelSpec,
    runtime_source: Option<MossRuntimeSource>,
) {
    use crate::handlers::asr::{send_done, send_error, send_progress, AsrStreamPayload};

    let asset = match asset {
        Ok(asset) => asset,
        Err(error) => {
            send_error(&tx, "MOSS initialization is not supported on this computer.", Some(&error))
                .await;
            return;
        }
    };
    send_progress(
        &tx,
        AsrStreamPayload {
            phase: "preflight",
            status: "running",
            progress: 5,
            message: "Checking MOSS runtime and model assets.",
            detail: Some(&asset),
            file: None,
            server_url: None,
        },
    )
    .await;

    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::resource_download::DownloadProgress>();
    let event_tx = tx.clone();
    let forward = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            let download_percent = progress.percent.unwrap_or(0);
            let overall = if progress.label == "MOSS runtime" {
                10_u8.saturating_add(download_percent.saturating_mul(25) / 100)
            } else {
                40_u8.saturating_add(download_percent.saturating_mul(55) / 100)
            };
            send_progress(
                &event_tx,
                AsrStreamPayload {
                    phase: "download",
                    status: "running",
                    progress: overall,
                    message: "Downloading verified MOSS assets.",
                    detail: Some(&progress.label),
                    file: Some(&progress.label),
                    server_url: None,
                },
            )
            .await;
        }
    });

    let result = ensure_moss_joint_runtime_with_spec_and_progress(
        &asr_home,
        "",
        model_spec,
        runtime_source,
        Some(progress_tx),
    )
    .await;
    let _ = forward.await;
    match result {
        Ok(_) => {
            send_progress(
                &tx,
                AsrStreamPayload {
                    phase: "installed",
                    status: "ready",
                    progress: 100,
                    message: "MOSS runtime and verified 8-bit model are ready.",
                    detail: None,
                    file: None,
                    server_url: None,
                },
            )
            .await;
            send_done(&tx).await;
        }
        Err(error) => {
            send_error(&tx, "MOSS initialization failed.", Some(&error)).await;
        }
    }
}

fn parse_moss_json(
    stdout: &[u8],
    duration_ms: u64,
) -> Result<crate::handlers::asr_streaming::WholeFileTranscription, String> {
    let payload: MossJsonPayload = serde_json::from_slice(stdout)
        .map_err(|error| format!("parse MOSS runtime JSON: {error}"))?;
    let (segments, finish_reason) = match payload {
        MossJsonPayload::Envelope(envelope) => (envelope.segments, envelope.finish_reason),
        MossJsonPayload::Legacy(segments) => (segments, "completed".to_string()),
    };
    if finish_reason == "length" {
        return Err("MOSS generation reached the max-new token limit before completion".to_string());
    }
    if finish_reason != "completed" {
        return Err(format!(
            "MOSS runtime returned unsupported finish reason {finish_reason}"
        ));
    }
    let duration_seconds = duration_ms as f64 / 1_000.0;
    let structured_segments = segments
        .into_iter()
        .filter_map(|segment| {
            let text = segment.text.trim().to_string();
            let speaker = segment.speaker.trim().to_string();
            if text.is_empty()
                || speaker.is_empty()
                || !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.end <= segment.start
            {
                return None;
            }
            if segment.start >= duration_seconds {
                return None;
            }
            let start_ms = (segment.start.max(0.0) * 1_000.0).round() as u64;
            let end_ms = (segment.end.min(duration_seconds).max(0.0) * 1_000.0).round() as u64;
            if end_ms <= start_ms {
                return None;
            }
            Some(bifrost_asr::transcription::TranscriptionSegment {
                start_ms,
                end_ms,
                text,
                speaker: Some(speaker),
                overlap: false,
            })
        })
        .collect::<Vec<_>>();
    if structured_segments.is_empty() {
        return Err(
            "MOSS runtime returned no positive-duration speaker-aware segments".to_string(),
        );
    }
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
    file_started_at_ms: Option<u64>,
) -> Result<crate::handlers::asr_streaming::WholeFileTranscription, String> {
    if prompt.chars().count() > MOSS_MAX_PROMPT_CHARS {
        return Err(format!("MOSS prompt exceeds {MOSS_MAX_PROMPT_CHARS} characters"));
    }
    let max_new = moss_output_token_budget(duration_ms)?;
    let budget_started = Instant::now();
    let max_runtime = moss_remaining_runtime_budget(duration_ms, file_started_at_ms)?;
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
    let output = child.wait_with_output();
    tokio::pin!(output);
    let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(
        budget_started + max_runtime,
    ));
    tokio::pin!(deadline);
    let mut pause_poll = tokio::time::interval(Duration::from_millis(500));
    pause_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            result = &mut output => {
                let output = result.map_err(|error| format!("wait for MOSS runtime: {error}"))?;
                if !output.status.success() {
                    let error = format!(
                        "MOSS runtime failed with {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                    return Err(if moss_runtime_error_is_deterministic(&error) {
                        moss_non_retryable_runtime_error(&error)
                    } else {
                        error
                    });
                }
                return parse_moss_json(&output.stdout, duration_ms).map_err(|error| {
                    if moss_runtime_error_is_deterministic(&error) {
                        moss_non_retryable_runtime_error(&error)
                    } else {
                        error
                    }
                });
            }
            _ = &mut deadline => {
                return Err(format!(
                    "moss_rtf_exceeded: end-to-end processing exceeded {:.1}x audio duration (remaining_limit_ms={}, audio_ms={duration_ms})",
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
