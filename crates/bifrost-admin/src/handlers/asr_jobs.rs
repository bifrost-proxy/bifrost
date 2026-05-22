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
    start_managed_service, stop_managed_service_for_target, target_from_query, AsrTarget,
};
use crate::handlers::asr_cli_invoke::{
    run_asr_cli_with_footprint_guard_and_abort, ASR_ABORTED_ERROR_MARKER,
    ASR_MEMORY_LIMIT_ERROR_MARKER,
};
use crate::handlers::asr_jobs_daily::{
    list_daily_documents_for_task, read_daily_document_for_task, AsrDailyDocumentSummary,
};
use crate::handlers::asr_jobs_source::source_audio_response;
use crate::handlers::asr_jobs_timeline::{
    generate_daily_summaries, inspect_source_audio, render_timeline_text, source_modified_ms,
    source_size, SourceAudioInfo, TimelineSegment, TranscriptTimeline,
};
use crate::handlers::{
    error_response, json_response, json_response_with_status, method_not_allowed, BoxBody,
};

// The ASR jobs implementation is intentionally split by responsibility.
// `include!` keeps these pieces in the same Rust module, preserving the
// pre-refactor visibility model while making each file reviewable.
include!("asr_jobs/state.rs");
include!("asr_jobs/external_import.rs");
include!("asr_jobs/api.rs");
include!("asr_jobs/retry.rs");
include!("asr_jobs/runner.rs");
include!("asr_jobs/chunk_runtime.rs");
include!("asr_jobs/memory_bisect.rs");
include!("asr_jobs/audio_processing.rs");
include!("asr_jobs/store.rs");
include!("asr_jobs/daily_agent.rs");
include!("asr_jobs/daily_agent_im.rs");
include!("asr_jobs/daily_agent_api.rs");
include!("asr_jobs/tests.rs");
