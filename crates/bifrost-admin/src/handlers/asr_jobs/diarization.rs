#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(unused_imports)]
use bifrost_asr::native::sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, Wave,
};
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use bifrost_asr::profiles::DEFAULT_AUTO_MAX_SPEAKERS;

const SHERPA_SEGMENTATION_MODEL_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.int8.onnx";
const SHERPA_EMBEDDING_MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";
const SHERPA_SEGMENTATION_MIN_BYTES: u64 = 1_000_000;
const SHERPA_EMBEDDING_MIN_BYTES: u64 = 30_000_000;
const VOICEPRINT_SAMPLE_RATE: u32 = 16_000;
const VOICEPRINT_MIN_TOTAL_MS: u64 = 2_000;
const VOICEPRINT_MIN_PROMPT_VERIFY_MS: u64 = 1_200;
const VOICEPRINT_MIN_PROMPT_RMS: f32 = 0.008;
const VOICEPRINT_MIN_IDENTIFY_SPEECH_MS: u64 = 800;
const VOICEPRINT_IDENTIFY_SPEECH_RMS: f32 = 0.006;
const VOICEPRINT_PROMPT_MATCH_THRESHOLD: f32 = 0.72;
pub(crate) const VOICEPRINT_SPEAKER_MATCH_THRESHOLD: f32 = 0.60;
const UPLOAD_SPEAKER_ASR_CHUNK_DURATION_SECS: u64 = 30;
const UPLOAD_SPEAKER_ASR_CHUNK_OVERLAP_SECS: u64 = 2;

const VOICEPRINT_PROMPTS: &[&str] = &[
    "今天我会用 Bifrost 录入自己的声纹，用于本地离线音频处理。",
    "请记录我的声音特征，后续把说话人时间线映射成我的名字。",
    "这是一段用于声纹注册的自然朗读，包含稳定的音量和清晰发音。",
];

#[derive(Debug, Clone)]
struct SherpaDiarizationModelPack {
    segmentation_model: PathBuf,
    embedding_model: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct UploadedDiarizedTranscriptionSegment {
    pub(crate) index: usize,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) speaker: String,
    pub(crate) display_name: String,
    pub(crate) mapped_profile_id: Option<String>,
    pub(crate) confidence: Option<f32>,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct UploadedDiarizedTranscription {
    pub(crate) text: String,
    pub(crate) segments: Vec<UploadedDiarizedTranscriptionSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrDiarizationProfileInfo {
    id: String,
    label: String,
    engine: String,
    quality_tier: String,
    requires_init: bool,
    ready: bool,
    install_dir: PathBuf,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrDiarizationStatusResponse {
    profile: AsrDiarizationProfileInfo,
    voiceprint_dir: PathBuf,
    speaker_profile_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiarizationManifest {
    task_id: String,
    source_path: PathBuf,
    profile: String,
    engine: String,
    created_at_ms: u64,
    speakers: Vec<TimelineSpeaker>,
    segments: Vec<DiarizationManifestSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiarizationManifestSegment {
    index: usize,
    speaker: String,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mapped_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confidence: Option<f32>,
    start_ms: u64,
    end_ms: u64,
    overlap: bool,
}

#[derive(Debug, Deserialize)]
struct SpeakerRenameRequest {
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct SpeakerProfileCreateRequest {
    name: String,
    #[serde(default)]
    source: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct SpeakerProfileCreateResponse {
    id: String,
    name: String,
    profile_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct SpeakerEnrollmentCreateRequest {
    name: String,
    #[serde(default = "default_diarization_profile")]
    diarization_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpeakerEnrollmentPrompt {
    id: String,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpeakerEnrollmentSession {
    id: String,
    speaker_name: String,
    diarization_profile: String,
    sample_rate: u32,
    audio_format: String,
    prompts: Vec<SpeakerEnrollmentPrompt>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct SpeakerEnrollmentSessionResponse {
    session: SpeakerEnrollmentSession,
}

#[derive(Debug, Deserialize)]
struct SpeakerEnrollmentAudioRequest {
    prompt_id: String,
    pcm16le_base64: String,
    sample_rate: u32,
    #[serde(default = "default_mono_channels")]
    channels: u8,
    #[serde(default)]
    final_chunk: bool,
}

#[derive(Debug, Serialize)]
struct SpeakerEnrollmentAudioResponse {
    session_id: String,
    prompt_id: String,
    received_bytes: u64,
    duration_ms: u64,
    total_duration_ms: u64,
    final_chunk: bool,
}

#[derive(Debug, Deserialize)]
struct SpeakerEnrollmentVerifyRequest {
    prompt_id: String,
    pcm16le_base64: String,
    sample_rate: u32,
    #[serde(default = "default_mono_channels")]
    channels: u8,
}

#[derive(Debug, Serialize)]
struct SpeakerEnrollmentVerifyResponse {
    prompt_id: String,
    transcript: String,
    match_score: f32,
    matched: bool,
}

#[derive(Debug, Deserialize)]
struct SpeakerVoiceIdentifyRequest {
    pcm16le_base64: String,
    sample_rate: u32,
    #[serde(default = "default_mono_channels")]
    channels: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SpeakerVoiceIdentifyResponse {
    pub(crate) matched: bool,
    pub(crate) profile_id: Option<String>,
    pub(crate) display_name: String,
    pub(crate) speaker: String,
    pub(crate) confidence: f32,
    pub(crate) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    pub(crate) audio_duration_ms: u64,
    pub(crate) speech_duration_ms: u64,
}
#[derive(Debug, Serialize, Deserialize)]
struct SpeakerVoiceprintSample {
    prompt_id: String,
    text: String,
    duration_ms: u64,
    rms: f32,
    clipped_ratio: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct SpeakerVoiceprintProfile {
    id: String,
    display_name: String,
    source: String,
    diarization_profile: String,
    embedding_model: String,
    embedding_dim: usize,
    embedding: Vec<f32>,
    sample_rate: u32,
    total_duration_ms: u64,
    samples: Vec<SpeakerVoiceprintSample>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone)]
struct RegisteredSpeakerProfile {
    id: String,
    display_name: String,
    embedding: Vec<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SpeakerEnrollmentFinishResponse {
    profile: SpeakerVoiceprintProfile,
    profile_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum AsrDiarizationWorkerRequest {
    Diarize {
        profile: String,
        config: AsrDiarizationConfig,
        normalized_wav: PathBuf,
    },
    IdentifyPcm16 {
        pcm16le_path: PathBuf,
        sample_rate: u32,
    },
    FinishEnrollment {
        session_id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum AsrDiarizationWorkerResponse {
    Diarize { segments: Vec<DiarizationSegment> },
    Identify { response: SpeakerVoiceIdentifyResponse },
    FinishEnrollment { response: SpeakerEnrollmentFinishResponse },
}

async fn handle_diarization_api(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::GET, "/api/asr/diarization/profiles") => list_diarization_profiles_response(),
        (&Method::GET, "/api/asr/diarization/status") => {
            get_diarization_status_response(req.uri().query())
        }
        (&Method::GET, "/api/asr/diarization/init-stream") => {
            get_diarization_init_stream_response(req.uri().query())
        }
        (&Method::GET, _)
            if path.starts_with("/api/asr/tasks/") && path.ends_with("/diarization") =>
        {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/diarization")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 3 || parts[1] != "files" {
                return error_response(StatusCode::NOT_FOUND, "ASR diarization endpoint not found");
            }
            get_task_file_diarization_response(parts[0], parts[2])
        }
        (&Method::PATCH, _)
            if path.starts_with("/api/asr/tasks/") && path.contains("/speakers/") =>
        {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 5 || parts[1] != "files" || parts[3] != "speakers" {
                return error_response(StatusCode::NOT_FOUND, "ASR speaker endpoint not found");
            }
            patch_task_file_speaker_response(parts[0], parts[2], parts[4], req).await
        }
        (&Method::POST, "/api/asr/speaker-profiles") => post_speaker_profile_response(req).await,
        (&Method::GET, "/api/asr/speaker-profiles") => list_speaker_profiles_response(),
        (&Method::POST, "/api/asr/speaker-profiles/identify") => {
            post_speaker_voice_identify_response(req).await
        }
        (&Method::DELETE, _)
            if path.starts_with("/api/asr/speaker-profiles/")
                && !path.contains("/enrollment-sessions/") =>
        {
            let profile_id = path.trim_start_matches("/api/asr/speaker-profiles/");
            delete_speaker_profile_response(profile_id)
        }
        (&Method::GET, _)
            if path.starts_with("/api/asr/speaker-profiles/")
                && !path.contains("/enrollment-sessions/") =>
        {
            let profile_id = path.trim_start_matches("/api/asr/speaker-profiles/");
            get_speaker_profile_response(profile_id)
        }
        (&Method::POST, "/api/asr/speaker-profiles/enrollment-sessions") => {
            post_speaker_enrollment_session_response(req).await
        }
        (&Method::POST, _)
            if path.starts_with("/api/asr/speaker-profiles/enrollment-sessions/")
                && path.ends_with("/audio") =>
        {
            let session_id = path
                .trim_start_matches("/api/asr/speaker-profiles/enrollment-sessions/")
                .trim_end_matches("/audio")
                .trim_matches('/');
            post_speaker_enrollment_audio_response(session_id, req).await
        }
        (&Method::POST, _)
            if path.starts_with("/api/asr/speaker-profiles/enrollment-sessions/")
                && path.ends_with("/verify") =>
        {
            let session_id = path
                .trim_start_matches("/api/asr/speaker-profiles/enrollment-sessions/")
                .trim_end_matches("/verify")
                .trim_matches('/');
            post_speaker_enrollment_verify_response(session_id, req).await
        }
        (&Method::POST, _)
            if path.starts_with("/api/asr/speaker-profiles/enrollment-sessions/")
                && path.ends_with("/finish") =>
        {
            let session_id = path
                .trim_start_matches("/api/asr/speaker-profiles/enrollment-sessions/")
                .trim_end_matches("/finish")
                .trim_matches('/');
            post_speaker_enrollment_finish_response(session_id).await
        }
        (&Method::GET, _) | (&Method::POST, _) | (&Method::PATCH, _) => {
            error_response(StatusCode::NOT_FOUND, "ASR diarization endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

fn list_diarization_profiles_response() -> Response<BoxBody> {
    json_response(&serde_json::json!({
        "profiles": [
            diarization_profile_info(DEFAULT_DIARIZATION_PROFILE),
            AsrDiarizationProfileInfo {
                id: "pyannote-community-quality".to_string(),
                label: "pyannote community quality".to_string(),
                engine: "pyannote-sidecar".to_string(),
                quality_tier: "quality".to_string(),
                requires_init: true,
                ready: false,
                install_dir: diarization_profile_dir("pyannote-community-quality"),
                message: "Python sidecar profile is reserved for a later install flow with Hugging Face token acceptance.".to_string(),
            },
        ]
    }))
}

fn get_diarization_status_response(query: Option<&str>) -> Response<BoxBody> {
    let profile = profile_from_query(query);
    json_response(&diarization_status_response(&profile))
}

fn get_diarization_init_stream_response(query: Option<&str>) -> Response<BoxBody> {
    let profile = profile_from_query(query);
    let result = prepare_diarization_profile(&profile);
    let status = diarization_status_response(&profile);
    let (event, payload) = match result {
        Ok(()) => (
            "done",
            serde_json::json!({
                "profile": profile,
                "status": status,
                "message": "diarization profile initialized"
            }),
        ),
        Err(error) => (
            "error",
            serde_json::json!({
                "profile": profile,
                "status": status,
                "message": error
            }),
        ),
    };
    let body = format!(
        "event: progress\ndata: {}\n\nevent: {event}\ndata: {}\n\n",
        serde_json::json!({"profile": profile, "step": "prepare_model_pack"}),
        payload
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(hyper::header::CACHE_CONTROL, "no-cache")
        .body(full_body(body))
        .unwrap_or_else(|_| Response::new(full_body("stream failed")))
}

fn diarization_status_response(profile: &str) -> AsrDiarizationStatusResponse {
    AsrDiarizationStatusResponse {
        profile: diarization_profile_info(profile),
        voiceprint_dir: voiceprint_dir(),
        speaker_profile_count: count_speaker_profiles(),
    }
}

fn diarization_profile_info(profile: &str) -> AsrDiarizationProfileInfo {
    let dir = diarization_profile_dir(profile);
    let ready = diarization_profile_ready(profile);
    AsrDiarizationProfileInfo {
        id: profile.to_string(),
        label: match profile {
            DEFAULT_DIARIZATION_PROFILE => "sherpa-onnx balanced".to_string(),
            other => other.replace('-', " "),
        },
        engine: match profile {
            DEFAULT_DIARIZATION_PROFILE => "sherpa-onnx".to_string(),
            other if other.contains("pyannote") => "pyannote-sidecar".to_string(),
            _ => "external".to_string(),
        },
        quality_tier: if profile == DEFAULT_DIARIZATION_PROFILE {
            "balanced".to_string()
        } else {
            "external".to_string()
        },
        requires_init: true,
        ready,
        install_dir: dir,
        message: if ready {
            "ready".to_string()
        } else {
            "not initialized; run bifrost ai asr diarization init".to_string()
        },
    }
}

fn prepare_diarization_profile(profile: &str) -> Result<(), String> {
    validate_profile_id(profile)?;
    if profile != DEFAULT_DIARIZATION_PROFILE {
        return Err(format!(
            "diarization profile '{profile}' is not installable by the built-in initializer"
        ));
    }
    let dir = diarization_profile_dir(profile);
    std::fs::create_dir_all(&dir).map_err(|error| format!("create diarization profile: {error}"))?;
    std::fs::create_dir_all(voiceprint_dir())
        .map_err(|error| format!("create speaker profile dir: {error}"))?;
    let pack = sherpa_model_pack_paths(profile);
    if let Some(parent) = pack.segmentation_model.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create segmentation model dir: {error}"))?;
    }
    if let Some(parent) = pack.embedding_model.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create embedding model dir: {error}"))?;
    }
    download_model_asset_if_needed(
        SHERPA_SEGMENTATION_MODEL_URL,
        &pack.segmentation_model,
        SHERPA_SEGMENTATION_MIN_BYTES,
    )?;
    download_model_asset_if_needed(
        SHERPA_EMBEDDING_MODEL_URL,
        &pack.embedding_model,
        SHERPA_EMBEDDING_MIN_BYTES,
    )?;
    let manifest = serde_json::json!({
        "profile": profile,
        "engine": "sherpa-onnx",
        "segmentation": {
            "model": pack.segmentation_model,
            "source_url": SHERPA_SEGMENTATION_MODEL_URL,
            "min_bytes": SHERPA_SEGMENTATION_MIN_BYTES
        },
        "embedding": {
            "model": pack.embedding_model,
            "source_url": SHERPA_EMBEDDING_MODEL_URL,
            "min_bytes": SHERPA_EMBEDDING_MIN_BYTES
        },
        "created_at_ms": now_ms(),
        "runtime_note": "This profile is ready only when real sherpa-onnx ONNX model files exist locally. Speaker labels are produced from OfflineSpeakerDiarization segments, not from heuristic assignment."
    });
    atomic_json_write(&dir.join("profile.json"), &manifest)?;
    Ok(())
}

pub(crate) fn diarization_profile_ready(profile: &str) -> bool {
    profile == DEFAULT_DIARIZATION_PROFILE
        && sherpa_model_pack_paths(profile).is_ready()
        && diarization_profile_dir(profile).join("profile.json").is_file()
}

fn profile_from_query(query: Option<&str>) -> String {
    query
        .and_then(|query| query_param_value(query, "profile"))
        .filter(|profile| validate_profile_id(profile).is_ok())
        .unwrap_or_else(default_diarization_profile)
}

fn validate_profile_id(profile: &str) -> Result<(), String> {
    if profile.is_empty()
        || profile
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    {
        return Err("invalid diarization profile id".to_string());
    }
    Ok(())
}

fn diarization_profile_dir(profile: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr")
        .join("diarization")
        .join("profiles")
        .join(profile)
}

fn voiceprint_dir() -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr")
        .join("diarization")
        .join("speaker-profiles")
}

fn sherpa_model_pack_paths(profile: &str) -> SherpaDiarizationModelPack {
    let dir = diarization_profile_dir(profile);
    SherpaDiarizationModelPack {
        segmentation_model: dir.join("segmentation").join("model.int8.onnx"),
        embedding_model: dir
            .join("embedding")
            .join("3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"),
    }
}

impl SherpaDiarizationModelPack {
    fn is_ready(&self) -> bool {
        model_file_has_min_size(&self.segmentation_model, SHERPA_SEGMENTATION_MIN_BYTES)
            && model_file_has_min_size(&self.embedding_model, SHERPA_EMBEDDING_MIN_BYTES)
    }
}

fn model_file_has_min_size(path: &Path, min_bytes: u64) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() >= min_bytes)
        .unwrap_or(false)
}

fn download_model_asset_if_needed(
    url: &str,
    destination: &Path,
    min_bytes: u64,
) -> Result<(), String> {
    if model_file_has_min_size(destination, min_bytes) {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create model asset dir: {error}"))?;
    }
    let temp_path = destination.with_extension("download");
    let client = bifrost_core::outbound_blocking_reqwest_client_builder()
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|error| format!("create HTTP client: {error}"))?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| {
            format!(
                "download diarization model {url}: {}",
                bifrost_core::format_reqwest_error(&error)
            )
        })?
        .error_for_status()
        .map_err(|error| {
            format!(
                "download diarization model {url}: {}",
                bifrost_core::format_reqwest_error(&error)
            )
        })?;
    let mut output = std::fs::File::create(&temp_path)
        .map_err(|error| format!("create model asset {}: {error}", temp_path.display()))?;
    std::io::copy(&mut response, &mut output)
        .map_err(|error| format!("write model asset {}: {error}", temp_path.display()))?;
    drop(output);
    if !model_file_has_min_size(&temp_path, min_bytes) {
        let actual_len = std::fs::metadata(&temp_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "downloaded diarization model is incomplete: {} has {actual_len} bytes, expected at least {min_bytes}",
            temp_path.display()
        ));
    }
    std::fs::rename(&temp_path, destination).map_err(|error| {
        format!(
            "install diarization model {} -> {}: {error}",
            temp_path.display(),
            destination.display()
        )
    })
}

fn count_speaker_profiles() -> usize {
    std::fs::read_dir(voiceprint_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry.path().is_file()
                        && entry.path().extension().is_some_and(|ext| ext == "json")
                })
                .count()
        })
        .unwrap_or(0)
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn resolved_diarization_cluster_count(config: &AsrDiarizationConfig) -> i32 {
    let count = config
        .known_speaker_count
        .or(config.max_speakers)
        .unwrap_or(DEFAULT_AUTO_MAX_SPEAKERS);
    i32::from(count.max(1))
}

fn apply_diarization_to_timeline(
    task: &AsrDirectoryTask,
    timeline: &mut TranscriptTimeline,
    normalized_wav: &Path,
) -> Result<(), String> {
    if !task.diarization.enabled {
        return Ok(());
    }
    if !diarization_profile_ready(&task.diarization.profile) {
        return Err(format!(
            "diarization_missing_assets: profile '{}' is not initialized",
            task.diarization.profile
        ));
    }
    let diarization_segments =
        run_sherpa_diarization(&task.diarization, &task.diarization.profile, normalized_wav)?;
    if diarization_segments.is_empty() {
        return Err("diarization_no_segments: sherpa-onnx returned no speaker segments".to_string());
    }
    let speakers = speakers_from_diarization_segments(&diarization_segments);
    apply_speaker_segments_to_asr_timeline(timeline, &diarization_segments)?;
    timeline.diarization_profile = Some(task.diarization.profile.clone());
    timeline.speakers = speakers.clone();
    write_diarization_manifest(task, timeline, speakers, &diarization_segments)
}

#[cfg(not(test))]
fn asr_diarization_worker_request_dir() -> Result<PathBuf, String> {
    let dir = bifrost_storage::data_dir()
        .join("runtime")
        .join("asr-diarization-worker");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("create ASR diarization worker request dir: {error}"))?;
    Ok(dir)
}

#[cfg(not(test))]
fn run_asr_diarization_worker_request(
    request: AsrDiarizationWorkerRequest,
) -> Result<AsrDiarizationWorkerResponse, String> {
    let request_dir = asr_diarization_worker_request_dir()?;
    let request_path = request_dir.join(format!("request-{}.json", uuid::Uuid::new_v4()));
    let request_json = serde_json::to_vec(&request)
        .map_err(|error| format!("serialize ASR diarization worker request: {error}"))?;
    std::fs::write(&request_path, request_json)
        .map_err(|error| format!("write ASR diarization worker request: {error}"))?;

    let current_exe =
        std::env::current_exe().map_err(|error| format!("resolve current executable: {error}"))?;
    let alias_dir = bifrost_storage::data_dir()
        .join("runtime")
        .join("process-aliases");
    let worker_exe = bifrost_core::process_alias_executable(
        &current_exe,
        &alias_dir,
        "bifrost-asr-diarization",
    )
    .unwrap_or_else(|error| {
        warn!(
            error = %error,
            executable = %current_exe.display(),
            "failed to create ASR diarization process alias; using original executable"
        );
        current_exe
    });
    let output = std::process::Command::new(worker_exe)
        .arg("asr-diarization-worker")
        .arg("--request")
        .arg(&request_path)
        .output()
        .map_err(|error| format!("spawn ASR diarization worker: {error}"))?;
    let _ = std::fs::remove_file(&request_path);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "ASR diarization worker failed: {}{}",
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" stdout={}", stdout.trim())
            }
        ));
    }
    serde_json::from_slice::<AsrDiarizationWorkerResponse>(&output.stdout)
        .map_err(|error| format!("parse ASR diarization worker response: {error}"))
}

#[cfg(test)]
#[allow(dead_code)]
fn run_asr_diarization_worker_request(
    request: AsrDiarizationWorkerRequest,
) -> Result<AsrDiarizationWorkerResponse, String> {
    run_asr_diarization_worker_request_in_process(request)
}

fn run_asr_diarization_worker_request_in_process(
    request: AsrDiarizationWorkerRequest,
) -> Result<AsrDiarizationWorkerResponse, String> {
    match request {
        AsrDiarizationWorkerRequest::Diarize {
            profile,
            config,
            normalized_wav,
        } => run_sherpa_diarization_in_process(&config, &profile, &normalized_wav)
            .map(|segments| AsrDiarizationWorkerResponse::Diarize { segments }),
        AsrDiarizationWorkerRequest::IdentifyPcm16 {
            pcm16le_path,
            sample_rate,
        } => {
            let bytes = std::fs::read(&pcm16le_path)
                .map_err(|error| format!("read voiceprint identify audio: {error}"))?;
            identify_speaker_voice_pcm16_in_process(&bytes, sample_rate)
                .map(|response| AsrDiarizationWorkerResponse::Identify { response })
        }
        AsrDiarizationWorkerRequest::FinishEnrollment { session_id } => {
            let session = read_speaker_enrollment_session(&session_id)?;
            finish_speaker_enrollment_in_process(&session)
                .map(|response| AsrDiarizationWorkerResponse::FinishEnrollment { response })
        }
    }
}

pub fn run_asr_diarization_worker_stdio(request_path: &Path) -> Result<(), String> {
    let raw = std::fs::read(request_path)
        .map_err(|error| format!("read ASR diarization worker request: {error}"))?;
    let request = serde_json::from_slice::<AsrDiarizationWorkerRequest>(&raw)
        .map_err(|error| format!("parse ASR diarization worker request: {error}"))?;
    let response = run_asr_diarization_worker_request_in_process(request)?;
    let json = serde_json::to_string(&response)
        .map_err(|error| format!("serialize ASR diarization worker response: {error}"))?;
    println!("{json}");
    Ok(())
}

fn run_sherpa_diarization(
    config: &AsrDiarizationConfig,
    profile: &str,
    normalized_wav: &Path,
) -> Result<Vec<DiarizationSegment>, String> {
    #[cfg(test)]
    {
        run_sherpa_diarization_in_process(config, profile, normalized_wav)
    }
    #[cfg(not(test))]
    {
        run_asr_diarization_worker_request(AsrDiarizationWorkerRequest::Diarize {
            profile: profile.to_string(),
            config: config.clone(),
            normalized_wav: normalized_wav.to_path_buf(),
        })
        .and_then(|response| match response {
            AsrDiarizationWorkerResponse::Diarize { segments } => Ok(segments),
            other => Err(format!("unexpected ASR diarization worker response: {other:?}")),
        })
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_sherpa_diarization_in_process(
    config: &AsrDiarizationConfig,
    profile: &str,
    normalized_wav: &Path,
) -> Result<Vec<DiarizationSegment>, String> {
    let pack = sherpa_model_pack_paths(profile);
    let segmentation_model = pack
        .segmentation_model
        .to_str()
        .ok_or_else(|| "diarization model path contains non-utf8 characters".to_string())?
        .to_string();
    let embedding_model_path = pack.embedding_model.clone();
    let embedding_model = embedding_model_path
        .to_str()
        .ok_or_else(|| "diarization embedding path contains non-utf8 characters".to_string())?
        .to_string();
    let wav_path = normalized_wav
        .to_str()
        .ok_or_else(|| "normalized audio path contains non-utf8 characters".to_string())?;
    let clustering = FastClusteringConfig {
        num_clusters: resolved_diarization_cluster_count(config),
        threshold: 0.5,
    };
    let diarization_config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                model: Some(segmentation_model),
            },
            num_threads: 2,
            debug: false,
            provider: Some("cpu".to_string()),
        },
        embedding: SpeakerEmbeddingExtractorConfig {
            model: Some(embedding_model),
            num_threads: 2,
            debug: false,
            provider: Some("cpu".to_string()),
        },
        clustering,
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };
    let diarizer = OfflineSpeakerDiarization::create(&diarization_config)
        .ok_or_else(|| "create sherpa-onnx offline speaker diarization failed".to_string())?;
    let wave = Wave::read(wav_path).ok_or_else(|| {
        format!(
            "read normalized audio for diarization failed: {}",
            normalized_wav.display()
        )
    })?;
    if wave.sample_rate() != diarizer.sample_rate() {
        return Err(format!(
            "diarization_sample_rate_mismatch: audio sample rate {} != model sample rate {}",
            wave.sample_rate(),
            diarizer.sample_rate()
        ));
    }
    let result = diarizer
        .process(wave.samples())
        .ok_or_else(|| "sherpa-onnx offline speaker diarization failed".to_string())?;
    let raw_segments = result
        .sort_by_start_time()
        .into_iter()
        .filter(|segment| segment.end > segment.start)
        .collect::<Vec<_>>();
    let mut segments = raw_segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let speaker = format!("speaker_{:02}", segment.speaker.max(0));
            DiarizationSegment {
                display_name: speaker_display_name(segment.speaker.max(0) as usize),
                speaker,
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: seconds_to_ms(segment.start),
                end_ms: seconds_to_ms(segment.end),
                overlap: raw_segments.iter().enumerate().any(|(other_index, other)| {
                    other_index != index
                        && other.speaker != segment.speaker
                        && interval_overlap_ms(
                            seconds_to_ms(segment.start),
                            seconds_to_ms(segment.end),
                            seconds_to_ms(other.start),
                            seconds_to_ms(other.end),
                        ) > 0
                }),
            }
        })
        .collect::<Vec<_>>();
    let mut speaker_embeddings = compute_diarization_speaker_embeddings(
        &embedding_model_path,
        wave.samples(),
        wave.sample_rate(),
        &segments,
    )?;
    let merge_decisions = stabilize_diarization_speakers(
        &mut segments,
        &speaker_embeddings,
        &SpeakerStabilizationConfig::default(),
    );
    log_speaker_stabilization_decisions(&merge_decisions);
    if !merge_decisions.is_empty() {
        speaker_embeddings = compute_diarization_speaker_embeddings(
            &embedding_model_path,
            wave.samples(),
            wave.sample_rate(),
            &segments,
        )?;
    }
    if config.voiceprint_matching {
        map_speakers_with_registered_voiceprints(&mut segments, &speaker_embeddings);
    }
    Ok(segments)
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn run_sherpa_diarization_in_process(
    _config: &AsrDiarizationConfig,
    _profile: &str,
    _normalized_wav: &Path,
) -> Result<Vec<DiarizationSegment>, String> {
    Err(format!(
        "diarization_unsupported_platform: sherpa-onnx is not available for {}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn log_speaker_stabilization_decisions(decisions: &[SpeakerMergeDecision]) {
    for decision in decisions {
        tracing::info!(
            target: "bifrost_admin::asr_jobs",
            from_speaker = %decision.from_speaker,
            to_speaker = %decision.to_speaker,
            similarity = decision.similarity,
            reason = %decision.reason,
            "merged unstable diarization speaker cluster"
        );
    }
}

pub(crate) async fn transcribe_uploaded_wav_with_voiceprint_speakers(
    server_url: &str,
    language: &str,
    normalized_wav: &Path,
    stream_tx: Option<&tokio::sync::mpsc::Sender<hyper::body::Bytes>>,
) -> Result<Option<UploadedDiarizedTranscription>, String> {
    if !diarization_profile_ready(DEFAULT_DIARIZATION_PROFILE) || count_speaker_profiles() == 0 {
        return Ok(None);
    }
    let config = AsrDiarizationConfig {
        enabled: true,
        profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
        min_speakers: None,
        max_speakers: None,
        known_speaker_count: None,
        voiceprint_matching: true,
    };
    let diarization_segments =
        run_sherpa_diarization(&config, DEFAULT_DIARIZATION_PROFILE, normalized_wav)?;
    if diarization_segments.is_empty() {
        return Ok(None);
    }

    let temp =
        tempfile::tempdir().map_err(|error| format!("create upload diarization temp dir: {error}"))?;
    let mut output = UploadedDiarizedTranscription {
        text: String::new(),
        segments: Vec::new(),
    };
    let asr_units = plan_asr_units(&diarization_segments, &AsrUnitPlannerConfig::default());

    for (index, asr_unit) in asr_units.iter().enumerate() {
        let duration_ms = asr_unit
            .source_end_ms
            .saturating_sub(asr_unit.source_start_ms);
        let chunk_boundaries = plan_uploaded_speaker_asr_chunks(duration_ms);
        let mut segment_committed = String::new();
        for (chunk_index, (chunk_offset_ms, chunk_duration_ms)) in
            chunk_boundaries.iter().copied().enumerate()
        {
            let chunk_start_ms = asr_unit.source_start_ms.saturating_add(chunk_offset_ms);
            let chunk_end_ms = chunk_start_ms
                .saturating_add(chunk_duration_ms)
                .min(asr_unit.source_end_ms);
            let segment_wav =
                temp.path()
                    .join(format!("upload-{}-chunk-{chunk_index:04}.wav", asr_unit.unit_id));
            ffmpeg_cut_wav_ms(normalized_wav, &segment_wav, chunk_start_ms, chunk_end_ms, None)
                .await?;
            if compute_wav_rms_energy(&segment_wav).is_some_and(|rms| rms < SILENCE_RMS_THRESHOLD)
            {
                let _ = std::fs::remove_file(&segment_wav);
                continue;
            }
            let actual_chunk_duration_ms = chunk_end_ms.saturating_sub(chunk_start_ms);
            let result = crate::handlers::asr_streaming::call_asr_whole_file_endpoint(
                server_url,
                language,
                &segment_wav,
                Some(actual_chunk_duration_ms),
            )
            .await
            .map_err(|error| {
                format!(
                    "speaker segment {} chunk {}: {error}",
                    index + 1,
                    chunk_index + 1
                )
            })?;
            let texts = if result.segments.is_empty() {
                vec![(0, actual_chunk_duration_ms, result.text)]
            } else {
                result.segments
            };
            for (local_start_ms, local_end_ms, text) in texts {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let text = if chunk_boundaries.len() > 1 {
                    let delta =
                        crate::handlers::asr_streaming::dedupe_increment(&segment_committed, text);
                    if delta.is_empty() {
                        continue;
                    }
                    crate::handlers::asr_streaming::append_transcript_delta(
                        &mut segment_committed,
                        &delta,
                    );
                    delta
                } else {
                    text.to_string()
                };
                if !output.text.is_empty() {
                    output.text.push('\n');
                }
                let speaker_label = speaker_transcript_label_for_unit(asr_unit);
                let line = format!("{speaker_label}: {text}");
                output.text.push_str(&line);
                let segment = UploadedDiarizedTranscriptionSegment {
                    index: output.segments.len(),
                    start_ms: chunk_start_ms.saturating_add(local_start_ms),
                    end_ms: chunk_start_ms
                        .saturating_add(local_end_ms)
                        .min(asr_unit.source_end_ms),
                    speaker: asr_unit.speaker.clone().unwrap_or_default(),
                    display_name: asr_unit.speaker_display_name.clone().unwrap_or_default(),
                    mapped_profile_id: asr_unit.mapped_profile_id.clone(),
                    confidence: asr_unit.confidence,
                    text,
                };
                if let Some(tx) = stream_tx {
                    crate::handlers::asr::send_asr_segment(
                        tx,
                        "final",
                        crate::handlers::asr::AsrSegmentPayload {
                            index: segment.index,
                            start_ms: segment.start_ms,
                            end_ms: segment.end_ms,
                            stable_start_ms: segment.start_ms,
                            stable_end_ms: segment.end_ms,
                            text: &segment.text,
                            delta: &line,
                            committed: &output.text,
                            speaker: Some(&segment.speaker),
                            speaker_display_name: Some(&segment.display_name),
                            speaker_profile_id: segment.mapped_profile_id.as_deref(),
                            speaker_confidence: segment.confidence,
                        },
                    )
                    .await;
                }
                output.segments.push(segment);
            }
            let _ = std::fs::remove_file(&segment_wav);
        }
    }

    if output.segments.is_empty() {
        Ok(None)
    } else {
        Ok(Some(output))
    }
}

fn speaker_transcript_label_for_unit(unit: &AsrAudioUnit) -> String {
    let display_name = unit
        .speaker_display_name
        .as_deref()
        .or(unit.speaker.as_deref())
        .unwrap_or("unknown_speaker");
    speaker_transcript_label_from_parts(display_name, unit.mapped_profile_id.as_deref(), unit.confidence)
}

#[cfg(test)]
fn speaker_transcript_label(segment: &DiarizationSegment) -> String {
    speaker_transcript_label_from_parts(
        &segment.display_name,
        segment.mapped_profile_id.as_deref(),
        segment.confidence,
    )
}

fn speaker_transcript_label_from_parts(
    display_name: &str,
    mapped_profile_id: Option<&str>,
    confidence: Option<f32>,
) -> String {
    match (mapped_profile_id, confidence) {
        (Some(_), Some(confidence)) => format!(
            "{} ({}% match)",
            display_name,
            (confidence.clamp(0.0, 1.0) * 100.0).round() as u32
        ),
        _ => display_name.to_string(),
    }
}

fn plan_uploaded_speaker_asr_chunks(segment_duration_ms: u64) -> Vec<(u64, u64)> {
    let duration_secs = segment_duration_ms.div_ceil(1000).max(1);
    if duration_secs <= UPLOAD_SPEAKER_ASR_CHUNK_DURATION_SECS {
        return vec![(0, duration_secs * 1000)];
    }
    let step_secs = UPLOAD_SPEAKER_ASR_CHUNK_DURATION_SECS
        .saturating_sub(UPLOAD_SPEAKER_ASR_CHUNK_OVERLAP_SECS)
        .max(1);
    let mut chunks = Vec::new();
    let mut offset_secs = 0u64;
    while offset_secs < duration_secs {
        let remaining = duration_secs - offset_secs;
        let chunk_secs = remaining.min(UPLOAD_SPEAKER_ASR_CHUNK_DURATION_SECS);
        chunks.push((offset_secs * 1000, chunk_secs * 1000));
        offset_secs += step_secs;
        if offset_secs < duration_secs
            && duration_secs - offset_secs <= UPLOAD_SPEAKER_ASR_CHUNK_OVERLAP_SECS
        {
            if let Some(last) = chunks.last_mut() {
                last.1 = duration_secs.saturating_sub(last.0 / 1000) * 1000;
            }
            break;
        }
    }
    chunks
}

fn apply_speaker_segments_to_asr_timeline(
    timeline: &mut TranscriptTimeline,
    diarization_segments: &[DiarizationSegment],
) -> Result<(), String> {
    for segment in &mut timeline.segments {
        let mut overlap_by_speaker = BTreeMap::<String, (u64, String, bool)>::new();
        for diarization in diarization_segments {
            let overlap_ms = interval_overlap_ms(
                segment.audio_start_ms,
                segment.audio_end_ms,
                diarization.start_ms,
                diarization.end_ms,
            );
            if overlap_ms == 0 {
                continue;
            }
            let entry = overlap_by_speaker.entry(diarization.speaker.clone()).or_insert((
                0,
                diarization.display_name.clone(),
                false,
            ));
            entry.0 = entry.0.saturating_add(overlap_ms);
            entry.2 |= diarization.overlap;
        }
        let Some((speaker, (_, display_name, segment_overlap))) = overlap_by_speaker
            .into_iter()
            .max_by_key(|(_, (overlap_ms, _, _))| *overlap_ms)
        else {
            return Err(format!(
                "diarization_alignment_failed: ASR segment {} ({}..{} ms) has no speaker overlap",
                segment.index, segment.audio_start_ms, segment.audio_end_ms
            ));
        };
        segment.speaker = Some(speaker);
        segment.speaker_display_name = Some(display_name);
        segment.overlap = segment_overlap;
    }
    Ok(())
}

fn write_diarization_manifest(
    task: &AsrDirectoryTask,
    timeline: &TranscriptTimeline,
    speakers: Vec<TimelineSpeaker>,
    diarization_segments: &[DiarizationSegment],
) -> Result<(), String> {
    let segments = diarization_segments
        .iter()
        .enumerate()
        .map(|(index, segment)| DiarizationManifestSegment {
                index,
                speaker: segment.speaker.clone(),
                display_name: segment.display_name.clone(),
                mapped_profile_id: segment.mapped_profile_id.clone(),
                confidence: segment.confidence,
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                overlap: segment.overlap,
            })
        .collect::<Vec<_>>();
    let manifest = DiarizationManifest {
        task_id: task.id.clone(),
        source_path: timeline.source_path.clone(),
        profile: task.diarization.profile.clone(),
        engine: "sherpa-onnx".to_string(),
        created_at_ms: now_ms(),
        speakers,
        segments,
    };
    let path = diarization_manifest_path(&task.id, &timeline.source_path, &task.audio_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create diarization manifest dir: {error}"))?;
    }
    atomic_json_write(&path, &manifest)
}

fn rewrite_diarization_manifest_display_names(
    task: &AsrDirectoryTask,
    timeline: &TranscriptTimeline,
) -> Result<(), String> {
    let path = diarization_manifest_path(&task.id, &timeline.source_path, &task.audio_dir);
    let data = std::fs::read_to_string(&path)
        .map_err(|error| format!("read diarization manifest {}: {error}", path.display()))?;
    let mut manifest: DiarizationManifest =
        serde_json::from_str(&data).map_err(|error| format!("parse diarization manifest: {error}"))?;
    let display_names = timeline
        .speakers
        .iter()
        .map(|speaker| (speaker.id.clone(), speaker.display_name.clone()))
        .collect::<BTreeMap<_, _>>();
    for speaker in &mut manifest.speakers {
        if let Some(display_name) = display_names.get(&speaker.id) {
            speaker.display_name = display_name.clone();
        }
    }
    for segment in &mut manifest.segments {
        if let Some(display_name) = display_names.get(&segment.speaker) {
            segment.display_name = display_name.clone();
        }
    }
    atomic_json_write(&path, &manifest)
}

fn diarization_manifest_path(task_id: &str, source_path: &Path, audio_dir: &Path) -> PathBuf {
    let relative = source_path.strip_prefix(audio_dir).unwrap_or(source_path);
    bifrost_storage::data_dir()
        .join("asr")
        .join("data")
        .join("diarization")
        .join(task_id)
        .join(relative)
        .with_extension("diarization.json")
}

fn load_task_file_timeline(
    task_id: &str,
    file_key: &str,
) -> Result<(FileRecord, TranscriptTimeline), (StatusCode, String)> {
    let files = load_file_store(task_id);
    let Some(record) = files.files.get(file_key).cloned() else {
        return Err((StatusCode::NOT_FOUND, "ASR task file not found".to_string()));
    };
    let Some(path) = &record.output_timeline_path else {
        return Err((
            StatusCode::NOT_FOUND,
            "ASR task file timeline not found".to_string(),
        ));
    };
    let timeline = std::fs::read_to_string(path)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("read timeline {}: {error}", path.display()),
            )
        })
        .and_then(|content| {
            serde_json::from_str::<TranscriptTimeline>(&content).map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("parse timeline {}: {error}", path.display()),
                )
            })
        })?;
    Ok((record, timeline))
}

fn get_task_file_diarization_response(task_id: &str, file_key: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let (_record, timeline) = match load_task_file_timeline(task_id, file_key) {
        Ok(value) => value,
        Err((status, error)) => return error_response(status, &error),
    };
    let manifest_path = diarization_manifest_path(&task.id, &timeline.source_path, &task.audio_dir);
    json_response(&serde_json::json!({
        "enabled": task.diarization.enabled,
        "profile": timeline.diarization_profile,
        "manifest_path": manifest_path,
        "speakers": timeline.speakers,
        "segments": timeline.segments.iter().map(|segment| serde_json::json!({
            "index": segment.index,
            "speaker": segment.speaker,
            "speaker_display_name": segment.speaker_display_name,
            "start_ms": segment.audio_start_ms,
            "end_ms": segment.audio_end_ms,
            "overlap": segment.overlap,
        })).collect::<Vec<_>>(),
    }))
}

async fn patch_task_file_speaker_response(
    task_id: &str,
    file_key: &str,
    speaker_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read speaker patch body: {error}"),
            );
        }
    };
    let patch: SpeakerRenameRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid speaker patch JSON: {error}"),
            );
        }
    };
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let (record, mut timeline) = match load_task_file_timeline(task_id, file_key) {
        Ok(value) => value,
        Err((status, error)) => return error_response(status, &error),
    };
    let display_name = patch.display_name.trim();
    if display_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "speaker display_name cannot be empty");
    }
    for speaker in &mut timeline.speakers {
        if speaker.id == speaker_id {
            speaker.display_name = display_name.to_string();
        }
    }
    for segment in &mut timeline.segments {
        if segment.speaker.as_deref() == Some(speaker_id) {
            segment.speaker_display_name = Some(display_name.to_string());
        }
    }
    let Some(timeline_path) = record.output_timeline_path.as_ref() else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file timeline not found");
    };
    if let Err(error) = atomic_json_write(timeline_path, &timeline) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    if let Some(text_path) = record.output_text_path.as_ref() {
        let text = render_timeline_text(&timeline, "");
        if let Err(error) = std::fs::write(text_path, text) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("rewrite speaker timeline text: {error}"),
            );
        }
    }
    if let Err(error) = rewrite_diarization_manifest_display_names(&task, &timeline) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response(&timeline)
}

async fn post_speaker_profile_response(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read speaker profile body: {error}"),
            );
        }
    };
    let create: SpeakerProfileCreateRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid speaker profile JSON: {error}"),
            );
        }
    };
    let name = create.name.trim();
    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "speaker profile name cannot be empty");
    }
    let id = uuid::Uuid::new_v4().as_simple().to_string();
    let dir = voiceprint_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("create speaker profile dir: {error}"),
        );
    }
    let profile_path = dir.join(format!("{id}.json"));
    let payload = serde_json::json!({
        "id": id,
        "name": name,
        "created_at_ms": now_ms(),
        "source": create.source,
        "embedding": null,
        "status": "reserved_for_voiceprint_enrollment"
    });
    if let Err(error) = atomic_json_write(&profile_path, &payload) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response_with_status(
        StatusCode::CREATED,
        &SpeakerProfileCreateResponse {
            id,
            name: name.to_string(),
            profile_path,
        },
    )
}
