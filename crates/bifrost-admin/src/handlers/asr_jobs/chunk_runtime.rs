#[cfg(not(test))]
use crate::handlers::asr_streaming::call_asr_whole_file_endpoint;
use crate::handlers::asr_streaming::{
    append_transcript_delta, dedupe_increment, WholeFileTranscription,
};

/// Result of chunked transcription: the merged text + any chunks that failed.
struct ChunkedTranscriptionResult {
    transcription: WholeFileTranscription,
    /// Chunks that exhausted all retries and bisect attempts.
    failed_chunks: Vec<FailedChunkRecord>,
    memory_limit_hints: Vec<AsrChunkMemoryHint>,
    chunk_metrics: Vec<AsrChunkMetric>,
    fallback_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct ServerRunnerState {
    server_url: String,
    baseline_rtf: Option<f64>,
    baseline_samples: Vec<f64>,
    server_failures: u32,
    force_fork_for_remaining: bool,
    restart_required: bool,
    current_chunk_failure_reason: Option<String>,
    fallback_reason: Option<String>,
}

struct TaskManagedServer {
    server_url: String,
    stop_after_use: bool,
}

struct ManagedServerRestartContext<'a> {
    task: &'a AsrDirectoryTask,
    target: &'a AsrTarget,
    scope: &'static str,
    stop_after_use: &'a mut bool,
}

#[derive(Debug)]
struct ChunkAttempt {
    result: Result<WholeFileTranscription, String>,
    metric: AsrChunkMetric,
    shadow_metrics: Vec<AsrChunkMetric>,
}

const ASR_MAX_SERVER_FAILURES_PER_FILE_ENV: &str = "BIFROST_ASR_MAX_SERVER_FAILURES_PER_FILE";
const ASR_MAX_SERVER_FAILURES_PER_TASK_ENV: &str = "BIFROST_ASR_MAX_SERVER_FAILURES_PER_TASK";
const ASR_DEFAULT_MAX_SERVER_FAILURES_PER_FILE: u32 = 2;
const ASR_DEFAULT_MAX_SERVER_FAILURES_PER_TASK: u32 = 3;

/// Split a long WAV file into fixed-duration chunks, transcribe each chunk
/// by forking the native `asr` CLI (Plan B), and merge the results into one
/// `WholeFileTranscription`.
///
/// Chunks overlap by `overlap_secs` seconds so that words sitting at chunk
/// boundaries are not cut mid-utterance. The overlap is de-duplicated using
/// `dedupe_increment` before merging into the final text.
#[allow(clippy::too_many_arguments)]
async fn run_chunk_with_strategy(
    strategy: AsrRuntimeStrategy,
    asr_bin: &Path,
    model_path: &Path,
    language: &str,
    chunk_path: &Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_index: usize,
    tmp_dir: &Path,
    force_pause_task_id: Option<&str>,
    server_state: &mut Option<ServerRunnerState>,
    server_restart: Option<&mut ManagedServerRestartContext<'_>>,
    memory_events: Option<std::sync::Arc<StdMutex<Vec<AsrMemoryLimitEvent>>>>,
) -> Result<ChunkAttempt, String> {
    match strategy {
        AsrRuntimeStrategy::ForkPerChunk => {
            run_fork_chunk(
                asr_bin,
                model_path,
                language,
                chunk_path,
                chunk_offset_secs,
                chunk_duration_secs,
                chunk_index,
                tmp_dir,
                force_pause_task_id,
                None,
                memory_events,
            )
            .await
        }
        AsrRuntimeStrategy::ReuseServer | AsrRuntimeStrategy::ReusePerFile => {
            if let Some(reason) = server_state
                .as_mut()
                .and_then(|state| state.current_chunk_failure_reason.take())
            {
                return Ok(failed_server_chunk_attempt(
                    chunk_offset_secs,
                    chunk_duration_secs,
                    chunk_index,
                    reason,
                ));
            }

            if server_state
                .as_ref()
                .map(|state| state.force_fork_for_remaining)
                .unwrap_or(false)
            {
                let reason = server_state
                    .as_ref()
                    .and_then(|state| state.fallback_reason.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "{} strategy previously lost managed server",
                            strategy.as_str()
                        )
                    });
                return run_fork_chunk(
                    asr_bin,
                    model_path,
                    language,
                    chunk_path,
                    chunk_offset_secs,
                    chunk_duration_secs,
                    chunk_index,
                    tmp_dir,
                    force_pause_task_id,
                    Some(reason),
                    memory_events,
                )
                .await;
            }

            let server_attempt = run_server_chunk(
                server_state.as_ref().ok_or_else(|| {
                    format!("{} strategy has no managed ASR server", strategy.as_str())
                })?,
                language,
                chunk_path,
                chunk_offset_secs,
                chunk_duration_secs,
                chunk_index,
                None,
            )
            .await?;
            if server_attempt.result.is_ok() {
                if let Some(state) = server_state.as_mut() {
                    state.server_failures = 0;
                }
                Ok(server_attempt)
            } else {
                let error = server_attempt
                    .result
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "unknown ASR server error".to_string());
                retry_server_failure_or_fork(
                    strategy,
                    asr_bin,
                    model_path,
                    language,
                    chunk_path,
                    chunk_offset_secs,
                    chunk_duration_secs,
                    chunk_index,
                    server_state,
                    server_restart,
                    server_attempt,
                    error,
                    tmp_dir,
                    force_pause_task_id,
                    memory_events,
                )
                .await
            }
        }
        AsrRuntimeStrategy::Auto => {
            if let Some(reason) = server_state
                .as_mut()
                .and_then(|state| state.current_chunk_failure_reason.take())
            {
                return Ok(failed_server_chunk_attempt(
                    chunk_offset_secs,
                    chunk_duration_secs,
                    chunk_index,
                    reason,
                ));
            }

            if server_state
                .as_ref()
                .map(|state| state.force_fork_for_remaining)
                .unwrap_or(true)
            {
                let reason = server_state
                    .as_ref()
                    .and_then(|state| state.fallback_reason.clone())
                    .unwrap_or_else(|| "auto strategy server unavailable".to_string());
                return run_fork_chunk(
                    asr_bin,
                    model_path,
                    language,
                    chunk_path,
                    chunk_offset_secs,
                    chunk_duration_secs,
                    chunk_index,
                    tmp_dir,
                    force_pause_task_id,
                    Some(reason),
                    memory_events,
                )
                .await;
            }

            let server_attempt = run_server_chunk(
                server_state
                    .as_ref()
                    .ok_or_else(|| "auto strategy has no managed ASR server".to_string())?,
                language,
                chunk_path,
                chunk_offset_secs,
                chunk_duration_secs,
                chunk_index,
                None,
            )
            .await?;
            if server_attempt.result.is_ok()
                && should_keep_using_server(server_state, &server_attempt.metric)
            {
                if let Some(state) = server_state.as_mut() {
                    state.server_failures = 0;
                }
                Ok(server_attempt)
            } else if server_attempt.result.is_ok() {
                let reason = "auto strategy RTF regression exceeded threshold".to_string();
                if let Some(state) = server_state.as_mut() {
                    state.force_fork_for_remaining = true;
                    state.fallback_reason = Some(reason.clone());
                }
                tracing::warn!(
                    chunk = chunk_index,
                    offset_secs = chunk_offset_secs,
                    duration_secs = chunk_duration_secs,
                    rtf = server_attempt.metric.rtf,
                    %reason,
                    "ASR auto strategy switching remaining chunks to fork_per_chunk"
                );
                Ok(server_attempt)
            } else {
                let error = server_attempt
                    .result
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "unknown ASR server error".to_string());
                retry_server_failure_or_fork(
                    strategy,
                    asr_bin,
                    model_path,
                    language,
                    chunk_path,
                    chunk_offset_secs,
                    chunk_duration_secs,
                    chunk_index,
                    server_state,
                    server_restart,
                    server_attempt,
                    error,
                    tmp_dir,
                    force_pause_task_id,
                    memory_events,
                )
                .await
            }
        }
        AsrRuntimeStrategy::Compare => {
            let fork_attempt = run_fork_chunk(
                asr_bin,
                model_path,
                language,
                chunk_path,
                chunk_offset_secs,
                chunk_duration_secs,
                chunk_index,
                tmp_dir,
                force_pause_task_id,
                None,
                memory_events,
            )
            .await?;
            let server_attempt = run_server_chunk(
                server_state
                    .as_ref()
                    .ok_or_else(|| "compare strategy has no managed ASR server".to_string())?,
                language,
                chunk_path,
                chunk_offset_secs,
                chunk_duration_secs,
                chunk_index,
                Some("compare_shadow".to_string()),
            )
            .await?;
            if fork_attempt.metric.text_sha1 != server_attempt.metric.text_sha1 {
                tracing::warn!(
                    chunk = chunk_index,
                    offset_secs = chunk_offset_secs,
                    duration_secs = chunk_duration_secs,
                    fork_sha1 = %fork_attempt.metric.text_sha1,
                    server_sha1 = %server_attempt.metric.text_sha1,
                    "ASR compare strategy produced different text hashes"
                );
            }
            tracing::info!(
                chunk = chunk_index,
                fork_rtf = fork_attempt.metric.rtf,
                server_rtf = server_attempt.metric.rtf,
                fork_status = %fork_attempt.metric.status,
                server_status = %server_attempt.metric.status,
                "ASR compare strategy completed paired chunk"
            );
            let mut paired = fork_attempt;
            paired.shadow_metrics.push(server_attempt.metric);
            Ok(paired)
        }
    }
}

fn should_keep_using_server(
    server_state: &mut Option<ServerRunnerState>,
    metric: &AsrChunkMetric,
) -> bool {
    let Some(state) = server_state.as_mut() else {
        return false;
    };
    if metric.status != "ok" {
        return false;
    }
    if state.baseline_rtf.is_none() {
        state.baseline_samples.push(metric.rtf);
        if state.baseline_samples.len() >= 3 {
            let sum: f64 = state.baseline_samples.iter().sum();
            state.baseline_rtf = Some(sum / state.baseline_samples.len() as f64);
        }
        return true;
    }
    let baseline = state.baseline_rtf.unwrap_or(metric.rtf.max(0.001));
    metric.rtf <= baseline * ASR_AUTO_FALLBACK_RTF_MULTIPLIER
}

#[allow(clippy::too_many_arguments)]
async fn retry_server_failure_or_fork(
    strategy: AsrRuntimeStrategy,
    asr_bin: &Path,
    model_path: &Path,
    language: &str,
    chunk_path: &Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_index: usize,
    server_state: &mut Option<ServerRunnerState>,
    mut server_restart: Option<&mut ManagedServerRestartContext<'_>>,
    first_server_attempt: ChunkAttempt,
    first_error: String,
    tmp_dir: &Path,
    force_pause_task_id: Option<&str>,
    memory_events: Option<std::sync::Arc<StdMutex<Vec<AsrMemoryLimitEvent>>>>,
) -> Result<ChunkAttempt, String> {
    let mut shadow_metrics = vec![first_server_attempt.metric.clone()];
    let reason = server_failure_fallback_reason(strategy, &first_error);
    if let Some(state) = server_state.as_mut() {
        state.server_failures = state.server_failures.saturating_add(1);
        state.restart_required = true;
        state.fallback_reason = Some(reason.clone());
        apply_server_failure_breaker_if_needed(
            strategy,
            state,
            chunk_index,
            chunk_offset_secs,
            chunk_duration_secs,
        );
    }

    tracing::warn!(
        chunk = chunk_index,
        offset_secs = chunk_offset_secs,
        duration_secs = chunk_duration_secs,
        server_url = ?first_server_attempt.metric.server_url,
        fallback_reason = %reason,
        server_error = %first_error,
        retryable = is_server_restart_retriable(&first_error),
        "ASR server chunk failed"
    );

    stop_failed_managed_server_after_failure(
        strategy,
        chunk_index,
        chunk_offset_secs,
        chunk_duration_secs,
        &first_server_attempt.metric,
        server_restart.as_deref_mut(),
    )
    .await;

    if server_restart.is_none() && matches!(strategy, AsrRuntimeStrategy::Auto) {
        let mut failed_attempt = first_server_attempt;
        failed_attempt.metric.fallback_reason = Some(reason);
        failed_attempt.shadow_metrics = shadow_metrics.split_off(1);
        return Ok(failed_attempt);
    }

    let fork_reason = server_state
        .as_ref()
        .filter(|state| state.force_fork_for_remaining)
        .and_then(|state| state.fallback_reason.clone())
        .unwrap_or_else(|| reason.clone());
    let mut fallback_attempt = run_fork_chunk(
        asr_bin,
        model_path,
        language,
        chunk_path,
        chunk_offset_secs,
        chunk_duration_secs,
        chunk_index,
        tmp_dir,
        force_pause_task_id,
        Some(fork_reason.clone()),
        memory_events,
    )
    .await?;
    fallback_attempt.shadow_metrics = shadow_metrics;
    match &fallback_attempt.result {
        Ok(_) => {
            tracing::info!(
                chunk = chunk_index,
                offset_secs = chunk_offset_secs,
                duration_secs = chunk_duration_secs,
                fallback_reason = %fork_reason,
                "ASR server chunk recovered via fork_per_chunk fallback"
            );
        }
        Err(error) => {
            tracing::warn!(
                chunk = chunk_index,
                offset_secs = chunk_offset_secs,
                duration_secs = chunk_duration_secs,
                fallback_reason = %fork_reason,
                fallback_error = %error,
                "ASR fork_per_chunk fallback failed after server chunk failure"
            );
        }
    }
    Ok(fallback_attempt)
}

async fn stop_failed_managed_server_after_failure(
    strategy: AsrRuntimeStrategy,
    chunk_index: usize,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    metric: &AsrChunkMetric,
    restart: Option<&mut ManagedServerRestartContext<'_>>,
) {
    let Some(restart) = restart else {
        return;
    };
    tracing::warn!(
        task_id = %restart.task.id,
        runtime_strategy = strategy.as_str(),
        scope = restart.scope,
        chunk = chunk_index,
        offset_secs = chunk_offset_secs,
        duration_secs = chunk_duration_secs,
        server_url = ?metric.server_url,
        "stopping failed managed ASR server after chunk failure"
    );
    stop_managed_service_for_target(restart.target).await;
}

fn server_failure_fallback_reason(strategy: AsrRuntimeStrategy, error: &str) -> String {
    let kind = if is_server_transport_failure(error) {
        "transport"
    } else if is_mlx_empty_tensor_failure(error) {
        "mlx_empty_tensor"
    } else {
        "server"
    };
    format!(
        "{} strategy {kind} failure; retrying current chunk via fork_per_chunk and scheduling managed ASR server restart for later chunks: {error}",
        strategy.as_str()
    )
}

fn max_server_failures_for_strategy(strategy: AsrRuntimeStrategy) -> u32 {
    let (env_key, default) = match strategy {
        AsrRuntimeStrategy::ReusePerFile => (
            ASR_MAX_SERVER_FAILURES_PER_FILE_ENV,
            ASR_DEFAULT_MAX_SERVER_FAILURES_PER_FILE,
        ),
        _ => (
            ASR_MAX_SERVER_FAILURES_PER_TASK_ENV,
            ASR_DEFAULT_MAX_SERVER_FAILURES_PER_TASK,
        ),
    };
    std::env::var(env_key)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn apply_server_failure_breaker_if_needed(
    strategy: AsrRuntimeStrategy,
    state: &mut ServerRunnerState,
    chunk_index: usize,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
) {
    let max_failures = max_server_failures_for_strategy(strategy);
    if state.server_failures < max_failures {
        return;
    }
    let disabled_reason = format!(
        "{} strategy observed {} managed ASR server failures; switching remaining chunks to fork_per_chunk isolation",
        strategy.as_str(),
        state.server_failures
    );
    state.force_fork_for_remaining = true;
    state.restart_required = false;
    state.fallback_reason = Some(disabled_reason.clone());
    tracing::warn!(
        chunk = chunk_index,
        offset_secs = chunk_offset_secs,
        duration_secs = chunk_duration_secs,
        failures = state.server_failures,
        max_failures,
        %disabled_reason,
        "ASR managed server failure threshold reached"
    );
}

fn is_server_restart_retriable(error: &str) -> bool {
    is_server_transport_failure(error) || is_mlx_empty_tensor_failure(error)
}

fn is_server_transport_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("connect")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("timed out")
        || lower.contains("connection closed")
}

fn is_mlx_empty_tensor_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("mlx error")
        && lower.contains("reshape")
        && (lower.contains("array of size 0") || lower.contains("size 0"))
}

#[allow(clippy::too_many_arguments)]
async fn run_fork_chunk(
    asr_bin: &Path,
    model_path: &Path,
    language: &str,
    chunk_path: &Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_index: usize,
    tmp_dir: &Path,
    force_pause_task_id: Option<&str>,
    fallback_reason: Option<String>,
    memory_events: Option<std::sync::Arc<StdMutex<Vec<AsrMemoryLimitEvent>>>>,
) -> Result<ChunkAttempt, String> {
    let started = Instant::now();
    let result = transcribe_single_chunk_with_bisect(
        asr_bin,
        model_path,
        language,
        chunk_path,
        chunk_offset_secs,
        chunk_duration_secs,
        chunk_index,
        tmp_dir,
        force_pause_task_id,
        memory_events,
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let metric = chunk_metric(
        chunk_index,
        chunk_offset_secs,
        chunk_duration_secs,
        "fork_per_chunk",
        &result,
        elapsed_ms,
        None,
        fallback_reason,
    );
    Ok(ChunkAttempt {
        result,
        metric,
        shadow_metrics: Vec::new(),
    })
}

async fn run_server_chunk(
    state: &ServerRunnerState,
    language: &str,
    chunk_path: &Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_index: usize,
    fallback_reason: Option<String>,
) -> Result<ChunkAttempt, String> {
    let started = Instant::now();
    let result = run_server_chunk_request(state, language, chunk_path, chunk_duration_secs).await;
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let metric = chunk_metric(
        chunk_index,
        chunk_offset_secs,
        chunk_duration_secs,
        "reuse_server",
        &result,
        elapsed_ms,
        Some(state.server_url.clone()),
        fallback_reason,
    );
    Ok(ChunkAttempt {
        result,
        metric,
        shadow_metrics: Vec::new(),
    })
}

fn failed_server_chunk_attempt(
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_index: usize,
    reason: String,
) -> ChunkAttempt {
    let result = Err(reason.clone());
    let metric = chunk_metric(
        chunk_index,
        chunk_offset_secs,
        chunk_duration_secs,
        "reuse_server",
        &result,
        0,
        None,
        Some(reason),
    );
    ChunkAttempt {
        result,
        metric,
        shadow_metrics: Vec::new(),
    }
}

#[cfg(not(test))]
async fn run_server_chunk_request(
    state: &ServerRunnerState,
    language: &str,
    chunk_path: &Path,
    chunk_duration_secs: u64,
) -> Result<WholeFileTranscription, String> {
    call_asr_whole_file_endpoint(
        &state.server_url,
        language,
        chunk_path,
        Some(chunk_duration_secs * 1000),
    )
    .await
}

#[cfg(test)]
async fn run_server_chunk_request(
    state: &ServerRunnerState,
    _language: &str,
    _chunk_path: &Path,
    chunk_duration_secs: u64,
) -> Result<WholeFileTranscription, String> {
    if let Some(error) = state.server_url.strip_prefix("test-error:") {
        return Err(error.to_string());
    }
    if state.server_url.starts_with("test-ok:") {
        return Ok(WholeFileTranscription {
            text: format!("server:{}:{chunk_duration_secs}", state.server_url),
            segments: Vec::new(),
        });
    }
    Ok(WholeFileTranscription {
        text: String::new(),
        segments: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn chunk_metric(
    chunk_index: usize,
    offset_secs: u64,
    duration_secs: u64,
    runner: &str,
    result: &Result<WholeFileTranscription, String>,
    elapsed_ms: u64,
    server_url: Option<String>,
    fallback_reason: Option<String>,
) -> AsrChunkMetric {
    let (status, text, error) = match result {
        Ok(transcription) => ("ok".to_string(), transcription.text.as_str(), None),
        Err(error) => ("error".to_string(), "", Some(error.clone())),
    };
    let rtf = if duration_secs == 0 {
        0.0
    } else {
        elapsed_ms as f64 / 1000.0 / duration_secs as f64
    };
    AsrChunkMetric {
        chunk_index,
        offset_secs,
        duration_secs,
        runner: runner.to_string(),
        status,
        elapsed_ms,
        rtf,
        text_chars: text.chars().count(),
        text_sha1: sha1_hex(text.as_bytes()),
        server_url,
        fallback_reason,
        error,
        recorded_at_ms: now_ms(),
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[allow(clippy::too_many_arguments)]
async fn transcribe_in_chunks(
    asr_bin: &Path,
    model_path: &Path,
    language: &str,
    wav_path: &Path,
    tmp_dir: &Path,
    total_secs: u64,
    chunk_secs: u64,
    overlap_secs: u64,
    total_duration_ms: u64,
    on_chunk_progress: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
    pause_check: Option<&(dyn Fn() -> bool + Send + Sync)>,
    force_pause_task_id: Option<&str>,
    model: &str,
    existing_memory_limit_hints: &[AsrChunkMemoryHint],
    runtime_strategy: AsrRuntimeStrategy,
    server_url: Option<&str>,
    startup_fallback_reason: Option<&str>,
    server_state: Option<&mut Option<ServerRunnerState>>,
    mut server_restart: Option<ManagedServerRestartContext<'_>>,
    on_chunk_metric: Option<&(dyn Fn(AsrChunkMetric) + Send + Sync)>,
) -> Result<ChunkedTranscriptionResult, String> {
    // The effective step between chunk starts accounts for overlap so that
    // consecutive chunks share `overlap_secs` seconds of audio.
    let step_secs = chunk_secs.saturating_sub(overlap_secs).max(1);

    tracing::info!(
        total_secs = total_secs,
        chunk_secs = chunk_secs,
        overlap_secs = overlap_secs,
        step_secs = step_secs,
        "planning ASR audio chunks"
    );

    let chunk_boundaries = plan_asr_chunk_boundaries(total_secs, chunk_secs, overlap_secs);

    let total_chunks = chunk_boundaries.len();
    tracing::info!(total_chunks = total_chunks, "planned ASR chunk boundaries");

    // Transcribe each chunk sequentially via fork-per-chunk. Each chunk is
    // extracted immediately before inference and removed immediately after,
    // avoiding unbounded ffmpeg fan-out and large temporary disk usage for
    // multi-hour recordings.
    // Each chunk forks a fresh `asr` CLI process. Sequential execution is
    // optimal because the native CLI takes exclusive control of the GPU.
    let mut all_segments: Vec<(u64, u64, String)> = Vec::new();
    let mut all_text = String::new();
    let mut failed_chunks: Vec<FailedChunkRecord> = Vec::new();
    let mut memory_limit_hints = existing_memory_limit_hints.to_vec();
    let mut chunk_metrics = Vec::new();
    let mut local_server_state;
    let shared_server_state = if let Some(state) = server_state {
        state
    } else {
        local_server_state = server_url.map(|url| ServerRunnerState {
            server_url: url.to_string(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: 0,
            force_fork_for_remaining: startup_fallback_reason.is_some(),
            restart_required: false,
            current_chunk_failure_reason: None,
            fallback_reason: startup_fallback_reason.map(str::to_string),
        });
        &mut local_server_state
    };

    for (chunk_index, &(chunk_offset_secs, this_chunk_secs)) in chunk_boundaries.iter().enumerate()
    {
        if pause_check.is_some_and(|check| check()) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let chunk_path = tmp_dir.join(format!("chunk-{chunk_index:04}.wav"));
        ffmpeg_split_chunk(
            wav_path,
            &chunk_path,
            chunk_offset_secs,
            this_chunk_secs,
            pause_check,
        )
        .await
        .map_err(|error| {
            format!(
                "chunk {chunk_index} split failed at {chunk_offset_secs}s+{this_chunk_secs}s: {error}"
            )
        })?;

        // --- Pre-filter: skip chunks that are too short or silent ---
        // These are the primary triggers for MLX reshape-to-empty-tensor crashes.
        if this_chunk_secs < MIN_CHUNK_SECS {
            tracing::warn!(
                chunk = chunk_index,
                duration_secs = this_chunk_secs,
                "skipping chunk: duration below minimum ({MIN_CHUNK_SECS}s)"
            );
            // Notify progress even for skipped chunks.
            if let Some(cb) = on_chunk_progress {
                cb(chunk_index + 1, total_chunks);
            }
            let _ = std::fs::remove_file(&chunk_path);
            continue;
        }

        let chunk_rms = compute_wav_rms_energy(&chunk_path);
        let chunk_is_silent = match chunk_rms {
            Some(rms) => rms < SILENCE_RMS_THRESHOLD,
            None => true, // unreadable / invalid WAV → treat as silent to avoid crash
        };

        if chunk_is_silent {
            tracing::warn!(
                chunk = chunk_index,
                rms = ?chunk_rms,
                threshold = SILENCE_RMS_THRESHOLD,
                "skipping chunk: silence detected (RMS below threshold)"
            );
            if let Some(cb) = on_chunk_progress {
                cb(chunk_index + 1, total_chunks);
            }
            let _ = std::fs::remove_file(&chunk_path);
            continue;
        }

        tracing::info!(
            chunk = chunk_index,
            total_chunks = total_chunks,
            offset_secs = chunk_offset_secs,
            chunk_secs = this_chunk_secs,
            rms = ?chunk_rms,
            runtime_strategy = runtime_strategy.as_str(),
            "transcribing ASR chunk"
        );

        // Try the chunk with retries, then bisect on persistent failure.
        // On memory-limit repeats, remember the final bisect window and use it
        // on later runs instead of re-hitting the full 30s chunk.
        let memory_events = std::sync::Arc::new(StdMutex::new(Vec::new()));
        let matching_hint = find_memory_limit_hint(
            &memory_limit_hints,
            model,
            chunk_offset_secs,
            this_chunk_secs,
        );
        let transcription_attempt = if let Some(hint) = matching_hint {
            tracing::warn!(
                chunk = chunk_index,
                offset_secs = chunk_offset_secs,
                duration_secs = this_chunk_secs,
                preferred_chunk_secs = hint.preferred_chunk_secs,
                triggers = hint.trigger_count,
                "using remembered ASR memory-limit bisect hint"
            );
            let started = Instant::now();
            let result = transcribe_chunk_with_memory_hint(
                asr_bin,
                model_path,
                language,
                &chunk_path,
                chunk_offset_secs,
                this_chunk_secs,
                chunk_index,
                tmp_dir,
                hint.preferred_chunk_secs,
                force_pause_task_id,
                Some(memory_events.clone()),
            )
            .await;
            let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            let metric = chunk_metric(
                chunk_index,
                chunk_offset_secs,
                this_chunk_secs,
                "fork_per_chunk",
                &result,
                elapsed_ms,
                None,
                Some("memory_limit_hint".to_string()),
            );
            ChunkAttempt {
                result,
                metric,
                shadow_metrics: Vec::new(),
            }
        } else {
            prepare_managed_server_for_chunk(
                runtime_strategy,
                shared_server_state,
                server_restart.as_mut(),
            )
            .await;
            run_chunk_with_strategy(
                runtime_strategy,
                asr_bin,
                model_path,
                language,
                &chunk_path,
                chunk_offset_secs,
                this_chunk_secs,
                chunk_index,
                tmp_dir,
                force_pause_task_id,
                shared_server_state,
                server_restart.as_mut(),
                Some(memory_events.clone()),
            )
            .await?
        };

        let observed_memory_events = memory_events.lock().unwrap().clone();
        if !observed_memory_events.is_empty() {
            merge_memory_limit_events_into_hints(
                &mut memory_limit_hints,
                model,
                chunk_offset_secs,
                this_chunk_secs,
                &observed_memory_events,
            );
        }

        if let Some(cb) = on_chunk_metric {
            cb(transcription_attempt.metric.clone());
            for metric in &transcription_attempt.shadow_metrics {
                cb(metric.clone());
            }
        }
        chunk_metrics.push(transcription_attempt.metric.clone());
        chunk_metrics.extend(transcription_attempt.shadow_metrics.clone());

        match transcription_attempt.result {
            Ok(chunk_result) => {
                append_chunk_transcription(
                    &mut all_segments,
                    &mut all_text,
                    chunk_result,
                    chunk_offset_secs,
                    this_chunk_secs,
                    overlap_secs,
                    total_duration_ms,
                );
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                let _ = std::fs::remove_file(&chunk_path);
                return Err(error);
            }
            Err(error) => {
                tracing::error!(
                    chunk = chunk_index,
                    offset_secs = chunk_offset_secs,
                    duration_secs = this_chunk_secs,
                    rms = ?chunk_rms,
                    %error,
                    "chunk failed all retries + bisect, recording for later retry"
                );
                failed_chunks.push(FailedChunkRecord {
                    chunk_index,
                    offset_secs: chunk_offset_secs,
                    duration_secs: this_chunk_secs,
                    error: error.clone(),
                    attempts: 3, // 3 retries + bisect sub-retries
                    energy_rms: chunk_rms,
                    is_silent: chunk_is_silent,
                });
                // Insert a placeholder marker in the text so users know where
                // the gap is.
                if !all_text.is_empty() {
                    all_text.push('\n');
                }
                all_text.push_str(&format!(
                    "[chunk {chunk_index} failed: {chunk_offset_secs}s-{}s]",
                    chunk_offset_secs + this_chunk_secs
                ));
            }
        }

        // Notify caller about chunk progress (for WebUI live updates).
        if let Some(cb) = on_chunk_progress {
            cb(chunk_index + 1, total_chunks);
        }

        if pause_check.is_some_and(|check| check()) {
            let _ = std::fs::remove_file(&chunk_path);
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }

        // Brief cooldown between chunks to let the Metal/GPU driver fully
        // release resources. Without this, rapid fork-exit cycles can
        // accumulate driver state and cause sporadic exit-255 crashes.
        if chunk_index + 1 < total_chunks {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        let _ = std::fs::remove_file(&chunk_path);
    }

    // If no segments came back (server returned plain text only), create a
    // single segment spanning the entire file.
    if all_segments.is_empty() && !all_text.is_empty() {
        all_segments.push((0, total_duration_ms, all_text.clone()));
    }

    Ok(ChunkedTranscriptionResult {
        transcription: WholeFileTranscription {
            text: all_text,
            segments: all_segments,
        },
        failed_chunks,
        memory_limit_hints,
        chunk_metrics,
        fallback_reason: shared_server_state
            .as_ref()
            .and_then(|state| state.fallback_reason.clone())
            .or_else(|| startup_fallback_reason.map(str::to_string)),
    })
}

async fn prepare_managed_server_for_chunk(
    runtime_strategy: AsrRuntimeStrategy,
    server_state: &mut Option<ServerRunnerState>,
    restart: Option<&mut ManagedServerRestartContext<'_>>,
) {
    if !matches!(
        runtime_strategy,
        AsrRuntimeStrategy::ReuseServer | AsrRuntimeStrategy::ReusePerFile | AsrRuntimeStrategy::Auto
    ) {
        return;
    }
    let Some(state) = server_state.as_mut() else {
        return;
    };
    if state.force_fork_for_remaining || !state.restart_required {
        return;
    }
    let Some(restart) = restart else {
        return;
    };

    let previous_url = state.server_url.clone();
    tracing::warn!(
        task_id = %restart.task.id,
        runtime_strategy = runtime_strategy.as_str(),
        scope = restart.scope,
        previous_server_url = %previous_url,
        fallback_reason = ?state.fallback_reason,
        "restarting managed ASR server before next chunk after prior server failure"
    );
    stop_managed_service_for_target(restart.target).await;
    match start_task_managed_server(restart.task, restart.target, restart.scope).await {
        Ok(server) => {
            *restart.stop_after_use |= server.stop_after_use;
            state.server_url = server.server_url;
            state.baseline_rtf = None;
            state.baseline_samples.clear();
            state.server_failures = 0;
            state.restart_required = false;
            state.current_chunk_failure_reason = None;
            tracing::info!(
                task_id = %restart.task.id,
                runtime_strategy = runtime_strategy.as_str(),
                scope = restart.scope,
                previous_server_url = %previous_url,
                server_url = %state.server_url,
                "managed ASR server restarted for later chunk"
            );
        }
        Err(error) => {
            let reason = format!(
                "{} strategy managed ASR server restart failed; recording current chunk as failed and will retry server restart before the next server chunk: {error}",
                runtime_strategy.as_str()
            );
            state.restart_required = true;
            state.current_chunk_failure_reason = Some(reason.clone());
            state.fallback_reason = Some(reason.clone());
            tracing::warn!(
                task_id = %restart.task.id,
                runtime_strategy = runtime_strategy.as_str(),
                scope = restart.scope,
                previous_server_url = %previous_url,
                %reason,
                "managed ASR server restart failed before chunk"
            );
        }
    }
}

fn plan_asr_chunk_boundaries(
    total_secs: u64,
    chunk_secs: u64,
    overlap_secs: u64,
) -> Vec<(u64, u64)> {
    let step_secs = chunk_secs.saturating_sub(overlap_secs).max(1);
    let mut chunk_boundaries: Vec<(u64, u64)> = Vec::new();
    let mut offset = 0u64;
    while offset < total_secs {
        let remaining = total_secs - offset;
        let this_chunk = remaining.min(chunk_secs);
        chunk_boundaries.push((offset, this_chunk));
        offset += step_secs;
        // Avoid creating a tiny trailing chunk (< overlap) that would be
        // entirely overlapping with the previous chunk.
        if offset < total_secs && (total_secs - offset) <= overlap_secs {
            let last = chunk_boundaries.last_mut().unwrap();
            last.1 = total_secs - last.0;
            break;
        }
    }
    chunk_boundaries
}

fn append_chunk_transcription(
    all_segments: &mut Vec<(u64, u64, String)>,
    all_text: &mut String,
    chunk_result: WholeFileTranscription,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    overlap_secs: u64,
    total_duration_ms: u64,
) {
    let offset_ms = chunk_offset_secs * 1000;
    let chunk_end_ms = offset_ms
        .saturating_add(chunk_duration_secs * 1000)
        .min(total_duration_ms);
    let mut appended_text = String::new();

    if !chunk_result.text.is_empty() {
        if overlap_secs > 0 && !all_text.is_empty() {
            appended_text = dedupe_increment(all_text, &chunk_result.text);
            if !appended_text.is_empty() {
                append_transcript_delta(all_text, &appended_text);
            }
        } else if !all_text.is_empty() {
            appended_text = chunk_result.text.clone();
            all_text.push('\n');
            all_text.push_str(&chunk_result.text);
        } else {
            appended_text = chunk_result.text.clone();
            all_text.push_str(&chunk_result.text);
        }
    }

    if chunk_result.segments.is_empty() {
        if !appended_text.is_empty() && chunk_end_ms > offset_ms {
            all_segments.push((offset_ms, chunk_end_ms, appended_text));
        }
        return;
    }

    for (start_ms, end_ms, text) in chunk_result.segments {
        all_segments.push((start_ms + offset_ms, end_ms + offset_ms, text));
    }
}

fn normalize_timeline_segments(timeline: &mut TranscriptTimeline) {
    if timeline.segments.iter().all(|segment| {
        segment.audio_end_ms.saturating_sub(segment.audio_start_ms) <= ASR_TASK_SEGMENT_MAX_MS
    }) {
        return;
    }

    let mut normalized = Vec::new();
    for segment in timeline.segments.drain(..) {
        normalized.extend(split_timeline_segment(
            segment,
            timeline.source_created_at_ms,
            ASR_TASK_SEGMENT_MAX_MS,
        ));
    }
    for (index, segment) in normalized.iter_mut().enumerate() {
        segment.index = index;
    }
    timeline.segments = normalized;
}

fn split_timeline_segment(
    segment: TimelineSegment,
    source_created_at_ms: Option<u64>,
    max_segment_ms: u64,
) -> Vec<TimelineSegment> {
    let duration_ms = segment.audio_end_ms.saturating_sub(segment.audio_start_ms);
    if duration_ms <= max_segment_ms || max_segment_ms == 0 {
        return vec![segment];
    }

    let window_count = duration_ms.div_ceil(max_segment_ms) as usize;
    let text_chars = segment.text.chars().collect::<Vec<_>>();
    let text_len = text_chars.len();
    let absolute_base = segment.absolute_start_ms.or_else(|| {
        source_created_at_ms.map(|source_start| source_start.saturating_add(segment.audio_start_ms))
    });

    (0..window_count)
        .map(|window_index| {
            let audio_start_ms = segment
                .audio_start_ms
                .saturating_add((window_index as u64).saturating_mul(max_segment_ms));
            let audio_end_ms = segment.audio_end_ms.min(
                segment
                    .audio_start_ms
                    .saturating_add(((window_index as u64) + 1).saturating_mul(max_segment_ms)),
            );
            let char_start = text_len.saturating_mul(window_index) / window_count;
            let char_end = text_len.saturating_mul(window_index + 1) / window_count;
            let text = text_chars[char_start..char_end].iter().collect::<String>();
            let absolute_start_ms = absolute_base.map(|base| {
                base.saturating_add(audio_start_ms.saturating_sub(segment.audio_start_ms))
            });
            let absolute_end_ms = absolute_base.map(|base| {
                base.saturating_add(audio_end_ms.saturating_sub(segment.audio_start_ms))
            });

            TimelineSegment {
                index: window_index,
                audio_start_ms,
                audio_end_ms,
                absolute_start_ms,
                absolute_end_ms,
                text,
            }
        })
        .collect()
}
