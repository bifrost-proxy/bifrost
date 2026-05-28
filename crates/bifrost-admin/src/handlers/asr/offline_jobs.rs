use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use bifrost_asr::offline::{write_offline_subtitle_artifacts, OfflineSubtitleArtifactRequest};
use bifrost_asr::timeline::{
    inspect_source_audio, TimelineSegment, TimelineSpeaker, TranscriptTimeline,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::Serialize;
use uuid::Uuid;

use crate::asr_runtime::now_ms;
use crate::handlers::asr_jobs::transcribe_uploaded_wav_with_voiceprint_speakers;
use crate::handlers::asr_streaming::call_asr_whole_file_endpoint;
use crate::handlers::{
    error_response, full_body, json_response, json_response_with_status, BoxBody,
};

use super::{
    prepare_audio_for_asr, query_flag_enabled, query_param_value, start_managed_service,
    stop_managed_service_for_target, target_from_query, AsrTarget, MAX_ASR_UPLOAD_BYTES,
};

static OFFLINE_JOBS: Lazy<StdMutex<std::collections::BTreeMap<String, OfflineAsrJob>>> =
    Lazy::new(|| StdMutex::new(std::collections::BTreeMap::new()));

#[derive(Debug, Clone, Serialize)]
struct OfflineAsrJob {
    job_id: String,
    status: String,
    pipeline_profile: String,
    source_path: Option<PathBuf>,
    model: String,
    language: String,
    speaker_aware: bool,
    created_at_ms: u64,
    updated_at_ms: u64,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
    error: Option<String>,
    events: Vec<OfflineAsrJobEvent>,
    artifacts: Option<OfflineAsrJobArtifacts>,
}

#[derive(Debug, Clone, Serialize)]
struct OfflineAsrJobEvent {
    at_ms: u64,
    stage: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct OfflineAsrJobArtifacts {
    text_path: PathBuf,
    metadata_path: PathBuf,
    timeline_path: PathBuf,
    srt_path: PathBuf,
    vtt_path: PathBuf,
}

pub(super) async fn handle_create(req: Request<Incoming>) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or_default();
    let mut requested_target = match target_from_query(Some(query)) {
        Ok(target) => target,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let speaker_aware =
        query_flag_enabled(query, "speaker_aware") || query_flag_enabled(query, "speaker-aware");
    let pipeline_profile = query_param_value(query, "pipeline_profile").unwrap_or_else(|| {
        if speaker_aware {
            bifrost_asr::profiles::OFFLINE_SPEAKER_SUBTITLE_PROFILE.to_string()
        } else {
            bifrost_asr::profiles::OFFLINE_PLAIN_ASR_PROFILE.to_string()
        }
    });
    let content_type = match req
        .headers()
        .get("Content-Type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
    {
        Some(value) if value.starts_with("multipart/form-data") => value,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "ASR offline job requires multipart/form-data with a file field",
            );
        }
    };
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read ASR offline job upload: {error}"),
            );
        }
    };
    if body.len() > MAX_ASR_UPLOAD_BYTES {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ASR offline job upload is too large",
        );
    }

    let job_id = format!("offline_{}", Uuid::new_v4().as_simple());
    requested_target.owner_module = "offline_job".to_string();
    requested_target.owner_id = Some(job_id.clone());
    let now = now_ms();
    let job = OfflineAsrJob {
        job_id: job_id.clone(),
        status: "queued".to_string(),
        pipeline_profile,
        source_path: None,
        model: requested_target.model.clone(),
        language: requested_target.language.clone(),
        speaker_aware,
        created_at_ms: now,
        updated_at_ms: now,
        started_at_ms: None,
        finished_at_ms: None,
        error: None,
        events: vec![OfflineAsrJobEvent {
            at_ms: now,
            stage: "queued".to_string(),
            message: "offline ASR job queued".to_string(),
        }],
        artifacts: None,
    };
    insert_offline_job(job);

    tokio::spawn(run_offline_job(
        job_id.clone(),
        requested_target,
        content_type,
        body,
    ));

    let job = get_offline_job(&job_id).expect("offline job inserted before spawn");
    json_response_with_status(StatusCode::CREATED, &job)
}

pub(super) fn handle_get(path: &str) -> Response<BoxBody> {
    let parts = path
        .trim_start_matches("/api/asr/offline-jobs/")
        .trim_end_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [job_id] => match get_offline_job(job_id) {
            Some(job) => json_response(&job),
            None => error_response(StatusCode::NOT_FOUND, "ASR offline job not found"),
        },
        [job_id, "events"] => match get_offline_job(job_id) {
            Some(job) => json_response(&serde_json::json!({ "events": job.events })),
            None => error_response(StatusCode::NOT_FOUND, "ASR offline job not found"),
        },
        [job_id, "artifacts"] => match get_offline_job(job_id) {
            Some(job) => json_response(&serde_json::json!({
                "artifacts": offline_job_artifact_values(job.artifacts.as_ref())
            })),
            None => error_response(StatusCode::NOT_FOUND, "ASR offline job not found"),
        },
        [job_id, "artifacts", format] => {
            let Some(job) = get_offline_job(job_id) else {
                return error_response(StatusCode::NOT_FOUND, "ASR offline job not found");
            };
            let Some(path) = offline_job_artifact_path(job.artifacts.as_ref(), format) else {
                return error_response(StatusCode::NOT_FOUND, "ASR offline job artifact not found");
            };
            match std::fs::read(&path) {
                Ok(bytes) => Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", artifact_content_type(format))
                    .body(full_body(bytes))
                    .unwrap(),
                Err(error) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("read artifact {}: {error}", path.display()),
                ),
            }
        }
        _ => error_response(StatusCode::NOT_FOUND, "ASR offline job endpoint not found"),
    }
}

async fn run_offline_job(job_id: String, target: AsrTarget, content_type: String, body: Bytes) {
    update_offline_job_status(
        &job_id,
        "running",
        "preflight",
        "offline ASR job started",
        None,
    );
    let resource_decision = crate::handlers::speech::acquire_speech_resource(
        crate::handlers::speech::offline_job_lease(&job_id, &target.model),
    );
    if !resource_decision.granted {
        update_offline_job_status(
            &job_id,
            "failed",
            "resource",
            "offline ASR resource is not available",
            resource_decision.reason,
        );
        return;
    }
    let resource_lease = resource_decision.lease;
    let result = run_offline_job_inner(&job_id, target.clone(), content_type, body).await;
    stop_managed_service_for_target(&target).await;
    crate::handlers::speech::release_speech_resource(resource_lease.as_ref());
    if let Err(error) = result {
        update_offline_job_status(
            &job_id,
            "failed",
            "failed",
            "offline ASR job failed",
            Some(error),
        );
    }
}

async fn run_offline_job_inner(
    job_id: &str,
    target: AsrTarget,
    content_type: String,
    body: Bytes,
) -> Result<(), String> {
    update_offline_job_status(
        job_id,
        "running",
        "preprocess",
        "normalizing uploaded audio",
        None,
    );
    let wav_bytes = prepare_audio_for_asr(&content_type, &body).await?;
    let job_dir = bifrost_storage::data_dir()
        .join("asr")
        .join("offline-jobs")
        .join(job_id);
    tokio::fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("create offline job dir: {error}"))?;
    let wav_path = job_dir.join("source.wav");
    tokio::fs::write(&wav_path, wav_bytes)
        .await
        .map_err(|error| format!("write offline job normalized wav: {error}"))?;
    update_offline_job(job_id.to_string(), |mut job| {
        job.source_path = Some(wav_path.clone());
        job
    });

    update_offline_job_status(job_id, "running", "asr", "starting local ASR runtime", None);
    let service = start_managed_service(target.clone())
        .await
        .map_err(|response| response.detail.unwrap_or(response.message))?;
    let server_url = service.server_url;

    let source_info = inspect_source_audio(&wav_path);
    let speaker_aware = get_offline_job(job_id)
        .map(|job| job.speaker_aware)
        .unwrap_or(false);
    let (text, speakers, segments) = if speaker_aware {
        if let Some(diarized) = transcribe_uploaded_wav_with_voiceprint_speakers(
            &server_url,
            &target.language,
            &wav_path,
        )
        .await?
        {
            let mut speaker_map = std::collections::BTreeMap::<String, TimelineSpeaker>::new();
            let segments = diarized
                .segments
                .into_iter()
                .map(|segment| {
                    speaker_map
                        .entry(segment.speaker.clone())
                        .or_insert(TimelineSpeaker {
                            id: segment.speaker.clone(),
                            display_name: segment.display_name.clone(),
                            mapped_profile_id: segment.mapped_profile_id.clone(),
                            confidence: segment.confidence,
                            candidate_profile_id: None,
                            candidate_display_name: None,
                            candidate_confidence: None,
                        });
                    TimelineSegment {
                        index: segment.index,
                        audio_start_ms: segment.start_ms,
                        audio_end_ms: segment.end_ms,
                        absolute_start_ms: source_info
                            .source_created_at_ms
                            .map(|start| start.saturating_add(segment.start_ms)),
                        absolute_end_ms: source_info
                            .source_created_at_ms
                            .map(|start| start.saturating_add(segment.end_ms)),
                        speaker: Some(segment.speaker),
                        speaker_display_name: Some(segment.display_name),
                        overlap: false,
                        text: segment.text,
                    }
                })
                .collect::<Vec<_>>();
            (diarized.text, speaker_map.into_values().collect(), segments)
        } else {
            transcribe_offline_plain(&server_url, &target.language, &wav_path, &source_info).await?
        }
    } else {
        transcribe_offline_plain(&server_url, &target.language, &wav_path, &source_info).await?
    };
    let diarization_profile = if speaker_aware && !speakers.is_empty() {
        Some(bifrost_asr::profiles::DEFAULT_DIARIZATION_PROFILE.to_string())
    } else {
        None
    };
    let timeline = TranscriptTimeline {
        task_id: job_id.to_string(),
        task_name: "Offline ASR job".to_string(),
        source_path: wav_path.clone(),
        source_size: source_info.source_size,
        source_modified_ms: source_info.source_modified_ms,
        source_created_at_ms: source_info.source_created_at_ms,
        source_created_at_source: source_info.source_created_at_source,
        media_duration_ms: source_info.media_duration_ms,
        model: target.model.clone(),
        language: target.language.clone(),
        diarization_profile,
        speakers,
        processed_at_ms: now_ms(),
        segments,
    };
    update_offline_job_status(
        job_id,
        "running",
        "artifacts",
        "writing subtitle artifacts",
        None,
    );
    let metadata = serde_json::json!({
        "job_id": job_id,
        "pipeline_profile": get_offline_job(job_id).map(|job| job.pipeline_profile),
        "source_path": wav_path,
        "model": target.model,
        "language": target.language,
        "speaker_aware": speaker_aware,
        "processed_at_ms": timeline.processed_at_ms,
    });
    let paths = write_offline_subtitle_artifacts(OfflineSubtitleArtifactRequest {
        data_dir: &bifrost_storage::data_dir(),
        task_id: job_id,
        source_path: &timeline.source_path,
        audio_dir: &job_dir,
        timeline: &timeline,
        fallback_text: &text,
        metadata,
    })?;
    update_offline_job(job_id.to_string(), |mut job| {
        let now = now_ms();
        job.status = "succeeded".to_string();
        job.updated_at_ms = now;
        job.finished_at_ms = Some(now);
        job.events.push(OfflineAsrJobEvent {
            at_ms: now,
            stage: "succeeded".to_string(),
            message: "offline ASR artifacts are ready".to_string(),
        });
        job.artifacts = Some(OfflineAsrJobArtifacts {
            text_path: paths.text_path,
            metadata_path: paths.metadata_path,
            timeline_path: paths.timeline_path,
            srt_path: paths.srt_path,
            vtt_path: paths.vtt_path,
        });
        job
    });
    Ok(())
}

async fn transcribe_offline_plain(
    server_url: &str,
    language: &str,
    wav_path: &Path,
    source_info: &bifrost_asr::timeline::SourceAudioInfo,
) -> Result<(String, Vec<TimelineSpeaker>, Vec<TimelineSegment>), String> {
    let result = call_asr_whole_file_endpoint(
        server_url,
        language,
        wav_path,
        source_info.media_duration_ms,
    )
    .await?;
    let mut segments = result
        .segments
        .iter()
        .enumerate()
        .map(|(index, (start_ms, end_ms, text))| TimelineSegment {
            index,
            audio_start_ms: *start_ms,
            audio_end_ms: *end_ms,
            absolute_start_ms: source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(*start_ms)),
            absolute_end_ms: source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(*end_ms)),
            speaker: None,
            speaker_display_name: None,
            overlap: false,
            text: text.clone(),
        })
        .collect::<Vec<_>>();
    if segments.is_empty() && !result.text.trim().is_empty() {
        let duration_ms = source_info.media_duration_ms.unwrap_or(0);
        segments.push(TimelineSegment {
            index: 0,
            audio_start_ms: 0,
            audio_end_ms: duration_ms,
            absolute_start_ms: source_info.source_created_at_ms,
            absolute_end_ms: source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(duration_ms)),
            speaker: None,
            speaker_display_name: None,
            overlap: false,
            text: result.text.clone(),
        });
    }
    Ok((result.text, Vec::new(), segments))
}

fn get_offline_job(job_id: &str) -> Option<OfflineAsrJob> {
    OFFLINE_JOBS
        .lock()
        .expect("offline ASR jobs store poisoned")
        .get(job_id)
        .cloned()
}

fn insert_offline_job(job: OfflineAsrJob) {
    OFFLINE_JOBS
        .lock()
        .expect("offline ASR jobs store poisoned")
        .insert(job.job_id.clone(), job);
}

fn update_offline_job(job_id: String, update: impl FnOnce(OfflineAsrJob) -> OfflineAsrJob) {
    let mut jobs = OFFLINE_JOBS
        .lock()
        .expect("offline ASR jobs store poisoned");
    if let Some(job) = jobs.remove(&job_id) {
        jobs.insert(job_id, update(job));
    }
}

fn update_offline_job_status(
    job_id: &str,
    status: &str,
    stage: &str,
    message: &str,
    error: Option<String>,
) {
    update_offline_job(job_id.to_string(), |mut job| {
        let now = now_ms();
        job.status = status.to_string();
        job.updated_at_ms = now;
        if status == "running" {
            job.started_at_ms.get_or_insert(now);
        }
        if status == "failed" {
            job.finished_at_ms = Some(now);
            job.error = error.clone();
        }
        job.events.push(OfflineAsrJobEvent {
            at_ms: now,
            stage: stage.to_string(),
            message: error.unwrap_or_else(|| message.to_string()),
        });
        job
    });
}

fn offline_job_artifact_values(
    artifacts: Option<&OfflineAsrJobArtifacts>,
) -> Vec<serde_json::Value> {
    let Some(artifacts) = artifacts else {
        return Vec::new();
    };
    [
        ("txt", &artifacts.text_path),
        ("metadata_json", &artifacts.metadata_path),
        ("timeline_json", &artifacts.timeline_path),
        ("srt", &artifacts.srt_path),
        ("vtt", &artifacts.vtt_path),
    ]
    .into_iter()
    .map(|(format, path)| {
        serde_json::json!({
            "format": format,
            "path": path.to_string_lossy().to_string(),
            "exists": path.is_file(),
        })
    })
    .collect()
}

fn offline_job_artifact_path(
    artifacts: Option<&OfflineAsrJobArtifacts>,
    format: &str,
) -> Option<PathBuf> {
    let artifacts = artifacts?;
    match format {
        "txt" | "text" => Some(artifacts.text_path.clone()),
        "metadata" | "metadata_json" | "json" => Some(artifacts.metadata_path.clone()),
        "timeline" | "timeline_json" => Some(artifacts.timeline_path.clone()),
        "srt" => Some(artifacts.srt_path.clone()),
        "vtt" => Some(artifacts.vtt_path.clone()),
        _ => None,
    }
}

fn artifact_content_type(format: &str) -> &'static str {
    match format {
        "vtt" => "text/vtt; charset=utf-8",
        "srt" | "txt" => "text/plain; charset=utf-8",
        "timeline" | "timeline_json" | "metadata" | "metadata_json" | "json" => {
            "application/json; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}
