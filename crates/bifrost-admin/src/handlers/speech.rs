use std::sync::Mutex as StdMutex;
use std::time::Duration;

use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::Deserialize;

use bifrost_asr::decision::{resolve_engine_decision, SpeechRequest, SpeechRuntimeEnv};
use bifrost_asr::profiles::{builtin_speech_pipeline_profiles, SpeechMode};
use bifrost_asr::resources::{
    ResourceLease, ResourceLeaseDecision, ResourceLeaseManager, ResourceLeaseRequest,
    ResourceLeaseSnapshot, SpeechResourceKind,
};

use crate::asr_runtime::{now_ms, probe_health_blocking, read_service_state};
use crate::handlers::asr_jobs::diarization_profile_ready;
use crate::handlers::{error_response, json_response, method_not_allowed, BoxBody};

static SPEECH_RESOURCE_MANAGER: Lazy<StdMutex<ResourceLeaseManager>> =
    Lazy::new(|| StdMutex::new(ResourceLeaseManager::default()));

#[derive(Debug, Deserialize)]
struct SpeechDecisionQuery {
    mode: String,
    pipeline_profile: Option<String>,
    model: Option<String>,
    language: Option<String>,
    #[serde(default)]
    speaker_aware: bool,
    #[serde(default)]
    scheduled: bool,
}

pub async fn handle_speech(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::GET, "/api/speech/pipelines") => {
            json_response(&serde_json::json!({ "profiles": builtin_speech_pipeline_profiles() }))
        }
        (&Method::GET, "/api/speech/pipelines/status") => json_response(&serde_json::json!({
            "profiles": builtin_speech_pipeline_profiles(),
            "runtime": speech_runtime_env(),
            "resources": speech_resource_snapshot(),
        })),
        (&Method::GET, "/api/speech/resources") => {
            json_response(&serde_json::json!({ "resource": speech_resource_snapshot() }))
        }
        (&Method::GET, "/api/speech/decision") => get_decision_response(req).await,
        (&Method::GET, _) | (&Method::POST, _) => {
            error_response(StatusCode::NOT_FOUND, "Speech pipeline endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

async fn get_decision_response(req: Request<Incoming>) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or_default();
    let params = match serde_urlencoded::from_str::<SpeechDecisionQuery>(query) {
        Ok(params) => params,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid speech decision query: {error}"),
            )
        }
    };
    let Some(mode) = SpeechMode::parse(&params.mode) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid speech mode");
    };
    let decision = resolve_engine_decision(
        mode,
        SpeechRequest {
            pipeline_profile: params.pipeline_profile,
            model: params.model,
            language: params.language,
            speaker_aware: params.speaker_aware,
            scheduled: params.scheduled,
        },
        speech_runtime_env(),
    );
    json_response(&decision)
}

pub(crate) fn acquire_speech_resource(request: ResourceLeaseRequest) -> ResourceLeaseDecision {
    SPEECH_RESOURCE_MANAGER
        .lock()
        .expect("speech resource manager poisoned")
        .acquire(request, now_ms())
}

pub(crate) fn release_speech_resource(lease: Option<&ResourceLease>) {
    let Some(lease) = lease else {
        return;
    };
    SPEECH_RESOURCE_MANAGER
        .lock()
        .expect("speech resource manager poisoned")
        .release(&lease.lease_id);
}

pub(crate) fn directory_task_should_yield_for_realtime(task_id: &str) -> bool {
    SPEECH_RESOURCE_MANAGER
        .lock()
        .expect("speech resource manager poisoned")
        .should_yield_for_realtime("directory_task", task_id)
}

pub(crate) fn speech_resource_snapshot() -> ResourceLeaseSnapshot {
    SPEECH_RESOURCE_MANAGER
        .lock()
        .expect("speech resource manager poisoned")
        .snapshot()
}

pub(crate) fn realtime_voice_lease(session_id: &str, model: &str) -> ResourceLeaseRequest {
    ResourceLeaseRequest {
        owner_module: "realtime_voice".to_string(),
        owner_id: session_id.to_string(),
        kind: SpeechResourceKind::StatefulVoiceWorker,
        model_id: Some(model.to_string()),
        priority: 100,
        preemptible: false,
    }
}

pub(crate) fn directory_task_lease(task_id: &str, model: &str) -> ResourceLeaseRequest {
    ResourceLeaseRequest {
        owner_module: "directory_task".to_string(),
        owner_id: task_id.to_string(),
        kind: SpeechResourceKind::OfflineAsrServer,
        model_id: Some(model.to_string()),
        priority: 20,
        preemptible: true,
    }
}

pub(crate) fn offline_job_lease(job_id: &str, model: &str) -> ResourceLeaseRequest {
    ResourceLeaseRequest {
        owner_module: "offline_job".to_string(),
        owner_id: job_id.to_string(),
        kind: SpeechResourceKind::OfflineAsrServer,
        model_id: Some(model.to_string()),
        priority: 70,
        preemptible: true,
    }
}

fn speech_runtime_env() -> SpeechRuntimeEnv {
    let platform = bifrost_asr::current_platform();
    let asr_ready = read_service_state(&bifrost_storage::data_dir())
        .and_then(|state| {
            probe_health_blocking(&state.host, state.port, Duration::from_millis(250))
                .ok()
                .map(|_| true)
        })
        .unwrap_or(false);
    let snapshot = speech_resource_snapshot();
    SpeechRuntimeEnv {
        platform_supported: platform.supported,
        asr_ready,
        diarization_ready: diarization_profile_ready(
            bifrost_asr::profiles::DEFAULT_DIARIZATION_PROFILE,
        ),
        realtime_voice_active: snapshot.realtime_voice_active,
        offline_asr_active: snapshot.offline_asr_active,
    }
}
