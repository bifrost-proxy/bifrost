use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, Local, LocalResult, NaiveDate, TimeZone,
    Timelike,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::sync::Mutex as StdMutex;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::warn;

#[cfg(unix)]
use nix::sys::signal::{killpg, Signal};
#[cfg(unix)]
use nix::unistd::Pid as NixPid;

use crate::asr_runtime::{now_ms, read_service_state, text_output_dir, AsrServiceState};
use crate::handlers::asr::{
    resolve_managed_target, start_managed_service, stop_managed_service_for_target,
    target_from_query, AsrTarget,
};
use crate::handlers::asr_cli_invoke::{
    asr_chunk_timeout, run_asr_cli_with_footprint_guard_timeout_and_abort,
    ASR_ABORTED_ERROR_MARKER, ASR_MEMORY_LIMIT_ERROR_MARKER, ASR_TIMEOUT_ERROR_MARKER,
};
use crate::handlers::asr_jobs_daily::{
    list_daily_documents_for_task, read_daily_document_for_task, AsrDailyDocumentSummary,
};
use crate::handlers::asr_jobs_source::source_audio_response;
use crate::handlers::asr_jobs_timeline::{
    generate_daily_summaries, inspect_source_audio, normalize_timeline_segments,
    render_timeline_text, source_modified_ms, source_size, SourceAudioInfo, TimelineSegment,
    TimelineSpeaker, TranscriptTimeline, ASR_TASK_SEGMENT_MAX_MS,
};
use crate::handlers::asr_streaming::call_asr_text_endpoint;
#[cfg(test)]
use crate::handlers::asr_streaming::{asr_server_request_timeout, asr_text_request_timeout};
use crate::handlers::{
    error_response, full_body, json_response, json_response_with_status, method_not_allowed,
    BoxBody,
};
use bifrost_asr::offline::{write_offline_subtitle_artifacts, OfflineSubtitleArtifactRequest};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use bifrost_asr::planner::seconds_to_ms;
use bifrost_asr::planner::{
    interval_overlap_ms, plan_asr_units, speaker_display_name, speakers_from_diarization_segments,
    AsrAudioUnit, AsrUnitPlannerConfig, DiarizationSegment,
};
use bifrost_asr::profiles::{
    default_diarization_profile, AsrDiarizationConfig, DEFAULT_DIARIZATION_PROFILE,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use bifrost_asr::speaker::{
    stabilize_diarization_speakers, SpeakerMergeDecision, SpeakerStabilizationConfig,
};

// The ASR jobs implementation is intentionally split by responsibility.
// `include!` keeps these pieces in the same Rust module, preserving the
// pre-refactor visibility model while making each file reviewable.
include!("asr_jobs/state.rs");
include!("asr_jobs/diarization.rs");
include!("asr_jobs/voiceprint.rs");
include!("asr_jobs/external_import.rs");
include!("asr_jobs/api.rs");
include!("asr_jobs/retry.rs");
include!("asr_jobs/runner.rs");
include!("asr_jobs/chunk_runtime.rs");
include!("asr_jobs/memory_bisect.rs");
include!("asr_jobs/audio_processing.rs");
include!("asr_jobs/store.rs");
include!("asr_jobs/daily_agent.rs");
include!("asr_jobs/daily_agent_workspace.rs");
include!("asr_jobs/daily_agent_records.rs");
include!("asr_jobs/daily_agent_im.rs");
include!("asr_jobs/daily_agent_sync.rs");
include!("asr_jobs/daily_agent_api.rs");
include!("asr_jobs/tests.rs");
