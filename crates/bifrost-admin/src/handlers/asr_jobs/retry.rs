/// POST /api/asr/tasks/{task_id}/files/{file_key}/retry-chunks
///
/// Retry all failed chunks for a file that is in `partial_success` status.
/// Each chunk is re-extracted from the original source audio and transcribed
/// using the bisect strategy. Successfully recovered chunks replace their
/// placeholder markers in the text file. When all chunks succeed, the file
/// status is promoted to `success`.
async fn retry_failed_chunks_response(task_id: &str, file_key: &str) -> Response<BoxBody> {
    // Acquire the global ASR job lock so we don't compete with a scheduled
    // task for GPU resources. This serialises manual retry vs scheduled runs.
    let _gpu_guard = ASR_JOB_RUN_LOCK.lock().await;

    match retry_failed_chunks_for_file_locked(task_id, file_key).await {
        Ok(outcome) => {
            if !outcome.daily_documents_refreshed.is_empty() {
                if let Some(task) = find_task(task_id) {
                    spawn_daily_agent_original_files_after_refresh(&task);
                    maybe_enqueue_daily_agent_after_asr_run(&task).await;
                }
            }
            retry_file_chunks_success_response(outcome)
        }
        Err(error) => retry_file_chunks_error_response(error),
    }
}

fn retry_file_chunks_success_response(outcome: RetryFileChunksOutcome) -> Response<BoxBody> {
    let mut response = serde_json::json!({
        "message": format!(
            "Retried {} failed chunks: {} recovered, {} still failed",
            outcome.total,
            outcome.recovered,
            outcome.still_failed_chunks.len()
        ),
        "recovered": outcome.recovered,
        "still_failed": outcome.still_failed_chunks.len(),
        "still_failed_chunks": outcome.still_failed_chunks,
        "daily_documents_refreshed": outcome.daily_documents_refreshed,
        "status": outcome.status,
    });
    if !outcome.persist_warnings.is_empty() {
        response["persist_warnings"] = serde_json::json!(outcome.persist_warnings);
    }
    json_response(&response)
}

fn retry_file_chunks_error_response(error: RetryFileChunksError) -> Response<BoxBody> {
    match error {
        RetryFileChunksError::TaskNotFound => {
            error_response(StatusCode::NOT_FOUND, "ASR task not found")
        }
        RetryFileChunksError::FileNotFound => {
            error_response(StatusCode::NOT_FOUND, "File not found in task")
        }
        RetryFileChunksError::NoFailedChunks(status) => json_response(&serde_json::json!({
            "message": "No failed chunks to retry",
            "status": status,
        })),
        RetryFileChunksError::Internal(error) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
        }
    }
}

async fn retry_failed_chunks_for_file_locked(
    task_id: &str,
    file_key: &str,
) -> Result<RetryFileChunksOutcome, RetryFileChunksError> {
    // 1. Validate task and file.
    let task = load_tasks()
        .tasks
        .into_iter()
        .find(|t| t.id == task_id)
        .ok_or(RetryFileChunksError::TaskNotFound)?;
    let mut files = load_file_store(task_id);
    let record = files
        .files
        .get(file_key)
        .cloned()
        .ok_or(RetryFileChunksError::FileNotFound)?;
    if record.failed_chunks.is_empty() {
        return Err(RetryFileChunksError::NoFailedChunks(record.status));
    }

    // 2. Resolve ASR binary and model.
    let target = match target_from_query(Some(&format!(
        "language={}&model={}&owner_module=directory_task&owner_id={}",
        urlencoding::encode(&task.language),
        urlencoding::encode(&task.model),
        urlencoding::encode(&task.id)
    ))) {
        Ok(t) => t,
        Err(e) => return Err(RetryFileChunksError::Internal(e)),
    };
    let asr_bin = target.install_dir().join("asr");
    let model_path = target.model_dir();
    if !asr_bin.is_file() {
        return Err(RetryFileChunksError::Internal(
            "ASR binary not found — run initializer first".to_string(),
        ));
    }

    // 3. Normalize source audio to temp WAV.
    let source_path = &record.source_path;
    if !source_path.is_file() {
        return Err(RetryFileChunksError::Internal(format!(
            "Source audio file not found: {}",
            source_path.display()
        )));
    }
    let (wav_path, temp_dir) = match normalize_to_temp(source_path, None, None).await {
        Ok(r) => r,
        Err(e) => {
            return Err(RetryFileChunksError::Internal(format!(
                "normalize failed: {e}"
            )));
        }
    };

    // 4. Retry each failed chunk.
    let chunks_to_retry = record.failed_chunks.clone();
    let total = chunks_to_retry.len();
    let mut still_failed: Vec<FailedChunkRecord> = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut recovered_texts: Vec<(usize, u64, u64, String, Vec<(u64, u64, String)>)> = Vec::new();

    let retry_tmp =
        tempfile::tempdir().unwrap_or_else(|_| tempfile::tempdir().expect("create retry temp dir"));

    for (retry_index, fc) in chunks_to_retry.iter().enumerate() {
        tracing::info!(
            task_id = %task_id,
            file_key = %file_key,
            chunk_index = fc.chunk_index,
            offset_secs = fc.offset_secs,
            duration_secs = fc.duration_secs,
            retry = retry_index + 1,
            total = total,
            "retrying failed chunk"
        );

        // Extract the chunk from the normalized WAV.
        let chunk_wav = retry_tmp
            .path()
            .join(format!("retry-chunk-{}.wav", fc.chunk_index));
        if let Err(e) = ffmpeg_split_chunk(
            &wav_path,
            &chunk_wav,
            fc.offset_secs,
            fc.duration_secs,
            None,
        )
        .await
        {
            tracing::error!(chunk = fc.chunk_index, %e, "failed to extract chunk for retry");
            still_failed.push(FailedChunkRecord {
                attempts: fc.attempts + 1,
                error: format!("extract failed: {e}"),
                ..fc.clone()
            });
            continue;
        }

        // Transcribe with bisect.
        match transcribe_single_chunk_with_bisect(
            &asr_bin,
            &model_path,
            &task.language,
            &chunk_wav,
            fc.offset_secs,
            fc.duration_secs,
            fc.chunk_index,
            retry_tmp.path(),
            None,
            None,
        )
        .await
        {
            Ok(result) => {
                let mut segments = Vec::new();
                let mut recovered_text = String::new();
                append_chunk_transcription(
                    &mut segments,
                    &mut recovered_text,
                    result,
                    fc.offset_secs,
                    fc.duration_secs,
                    0,
                    record
                        .media_duration_ms
                        .unwrap_or_else(|| (fc.offset_secs + fc.duration_secs) * 1000),
                );
                recovered_texts.push((
                    fc.chunk_index,
                    fc.offset_secs,
                    fc.duration_secs,
                    recovered_text,
                    segments,
                ));
                tracing::info!(chunk = fc.chunk_index, "chunk retry succeeded");
            }
            Err(e) => {
                tracing::warn!(
                    chunk = fc.chunk_index, %e,
                    "chunk retry still failed"
                );
                still_failed.push(FailedChunkRecord {
                    attempts: fc.attempts + 3, // 3 more attempts this round
                    error: e,
                    ..fc.clone()
                });
            }
        }

        // GPU cooldown.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // 5. Update the text file: replace placeholder markers with recovered text.
    let recovered_count = recovered_texts.len();
    let mut persist_warnings: Vec<String> = Vec::new();
    if recovered_count > 0 {
        if let Some(ref text_path) = record.output_text_path {
            match std::fs::read_to_string(text_path) {
                Ok(mut full_text) => {
                    for (chunk_idx, offset, duration, new_text, _) in &recovered_texts {
                        let placeholder = format!(
                            "[chunk {} failed: {}s-{}s]",
                            chunk_idx,
                            offset,
                            offset + duration
                        );
                        full_text = full_text.replace(&placeholder, new_text);
                    }
                    if let Err(e) = atomic_text_write(text_path, &full_text) {
                        let msg = format!("failed to persist recovered text: {e}");
                        tracing::error!(task_id, file_key, "{msg}");
                        persist_warnings.push(msg);
                    }
                }
                Err(e) => {
                    let msg = format!("failed to read text file for patching: {e}");
                    tracing::error!(task_id, file_key, "{msg}");
                    persist_warnings.push(msg);
                }
            }
        }

        // Also update the timeline JSON with recovered segments.
        if let Some(ref timeline_path) = record.output_timeline_path {
            match std::fs::read_to_string(timeline_path) {
                Ok(content) => {
                    match serde_json::from_str::<TranscriptTimeline>(&content) {
                        Ok(mut timeline) => {
                            for (_, _, _, _, new_segments) in &recovered_texts {
                                for (s, e, t) in new_segments {
                                    let source_created = timeline.source_created_at_ms;
                                    timeline.segments.push(TimelineSegment {
                                        index: timeline.segments.len(),
                                        audio_start_ms: *s,
                                        audio_end_ms: *e,
                                        absolute_start_ms: source_created
                                            .map(|c| c.saturating_add(*s)),
                                        absolute_end_ms: source_created
                                            .map(|c| c.saturating_add(*e)),
                                        speaker: None,
                                        speaker_display_name: None,
                                        overlap: false,
                                        text: t.clone(),
                                    });
                                }
                            }
                            // Sort segments by audio_start_ms.
                            timeline.segments.sort_by_key(|seg| seg.audio_start_ms);
                            // Re-index.
                            for (i, seg) in timeline.segments.iter_mut().enumerate() {
                                seg.index = i;
                            }
                            if let Err(e) = atomic_json_write(timeline_path, &timeline) {
                                let msg = format!("failed to persist recovered timeline: {e}");
                                tracing::error!(task_id, file_key, "{msg}");
                                persist_warnings.push(msg);
                            }
                        }
                        Err(e) => {
                            let msg = format!("failed to parse timeline JSON: {e}");
                            tracing::error!(task_id, file_key, "{msg}");
                            persist_warnings.push(msg);
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("failed to read timeline file: {e}");
                    tracing::error!(task_id, file_key, "{msg}");
                    persist_warnings.push(msg);
                }
            }
        }

        if let Some(ref metadata_path) = record.output_metadata_path {
            match std::fs::read_to_string(metadata_path) {
                Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(mut metadata) => {
                        metadata["retry_updated_at_ms"] = serde_json::json!(now_ms());
                        metadata["retry_recovered_chunks"] = serde_json::json!(recovered_texts
                            .iter()
                            .map(|(chunk_index, offset_secs, duration_secs, text, _)| {
                                serde_json::json!({
                                    "chunk_index": chunk_index,
                                    "offset_secs": offset_secs,
                                    "duration_secs": duration_secs,
                                    "text_chars": text.chars().count(),
                                })
                            })
                            .collect::<Vec<_>>());
                        metadata["retry_still_failed_chunks"] = serde_json::json!(still_failed);
                        if let Err(e) = atomic_json_write(metadata_path, &metadata) {
                            let msg = format!("failed to persist recovered metadata: {e}");
                            tracing::error!(task_id, file_key, "{msg}");
                            persist_warnings.push(msg);
                        }
                    }
                    Err(e) => {
                        let msg = format!("failed to parse metadata JSON: {e}");
                        tracing::error!(task_id, file_key, "{msg}");
                        persist_warnings.push(msg);
                    }
                },
                Err(e) => {
                    let msg = format!("failed to read metadata file for patching: {e}");
                    tracing::error!(task_id, file_key, "{msg}");
                    persist_warnings.push(msg);
                }
            }
        }
    }

    // 6. Update file record.
    let mut updated_record = record.clone();
    updated_record.failed_chunks = still_failed.clone();
    if still_failed.is_empty() && persist_warnings.is_empty() {
        updated_record.status = FileStatus::Success;
    } else if still_failed.is_empty() && !persist_warnings.is_empty() {
        // Chunks recovered but persistence failed — keep PartialSuccess so
        // the user knows to retry the save.
        updated_record.status = FileStatus::PartialSuccess;
    }
    // Recount chars from the text file.
    if let Some(ref text_path) = updated_record.output_text_path {
        if let Ok(text) = std::fs::read_to_string(text_path) {
            updated_record.text_chars = text.chars().count();
        }
    }
    let updated_status = updated_record.status.clone();
    files.files.insert(file_key.to_string(), updated_record);
    if let Err(e) = save_file_store(task_id, &files) {
        let msg = format!("failed to persist file store: {e}");
        tracing::error!(task_id, file_key, "{msg}");
        persist_warnings.push(msg);
    }

    let mut daily_documents_refreshed: Vec<String> = Vec::new();
    if recovered_count > 0 && persist_warnings.is_empty() {
        match refresh_task_daily_summaries(&task) {
            Ok(paths) => {
                daily_documents_refreshed = paths
                    .into_iter()
                    .map(|path| path.display().to_string())
                    .collect();
            }
            Err(e) => {
                let msg = format!("failed to refresh daily summaries after chunk retry: {e}");
                tracing::error!(task_id, file_key, "{msg}");
                persist_warnings.push(msg);
            }
        }
    }

    // Clean up.
    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(RetryFileChunksOutcome {
        status: updated_status,
        total,
        recovered: recovered_count,
        still_failed_chunks: still_failed,
        daily_documents_refreshed,
        persist_warnings,
    })
}

async fn retry_all_failed_chunks_response(task_id: &str) -> Response<BoxBody> {
    let task = match find_task(task_id) {
        Some(task) => task,
        None => return error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    };
    let targets = retryable_failed_chunk_files(&load_file_store(task_id));
    if targets.is_empty() {
        let now = now_ms();
        let state = BulkChunkRetryState {
            task_id: task_id.to_string(),
            status: BulkChunkRetryStatus::Completed,
            queued_files: 0,
            processed_files: 0,
            total_failed_chunks: 0,
            recovered_chunks: 0,
            still_failed_chunks: 0,
            started_at_ms: Some(now),
            updated_at_ms: now,
            finished_at_ms: Some(now),
            current_file_key: None,
            current_source_path: None,
            message: "No failed chunks to retry".to_string(),
            results: Vec::new(),
        };
        set_bulk_chunk_retry_state(state.clone());
        return json_response(&serde_json::json!({
            "message": state.message,
            "retry": state,
        }));
    }

    if let Some(existing) = bulk_chunk_retry_state(task_id) {
        if matches!(
            existing.status,
            BulkChunkRetryStatus::Queued | BulkChunkRetryStatus::Running
        ) {
            return json_response_with_status(
                StatusCode::ACCEPTED,
                &serde_json::json!({
                    "message": existing.message,
                    "retry": existing,
                }),
            );
        }
    }

    let now = now_ms();
    let state = BulkChunkRetryState {
        task_id: task_id.to_string(),
        status: BulkChunkRetryStatus::Queued,
        queued_files: targets.len(),
        processed_files: 0,
        total_failed_chunks: targets.iter().map(|target| target.failed_chunks).sum(),
        recovered_chunks: 0,
        still_failed_chunks: 0,
        started_at_ms: None,
        updated_at_ms: now,
        finished_at_ms: None,
        current_file_key: None,
        current_source_path: None,
        message: format!(
            "Queued {} files with {} failed chunks for retry",
            targets.len(),
            targets
                .iter()
                .map(|target| target.failed_chunks)
                .sum::<usize>()
        ),
        results: Vec::new(),
    };
    set_bulk_chunk_retry_state(state.clone());

    tracing::info!(
        task_id = %task_id,
        queued_files = state.queued_files,
        total_failed_chunks = state.total_failed_chunks,
        "queued bulk ASR failed chunk retry"
    );
    tokio::spawn(run_bulk_failed_chunk_retry(task));

    json_response_with_status(
        StatusCode::ACCEPTED,
        &serde_json::json!({
            "message": state.message,
            "retry": state,
        }),
    )
}

async fn run_bulk_failed_chunk_retry(task: AsrDirectoryTask) {
    update_bulk_chunk_retry_state(&task.id, |state| {
        state.status = BulkChunkRetryStatus::Queued;
        state.updated_at_ms = now_ms();
        state.message =
            "Bulk failed chunk retry is waiting for the ASR processing lock".to_string();
    });

    let _gpu_guard = ASR_JOB_RUN_LOCK.lock().await;
    let start_ms = now_ms();
    update_bulk_chunk_retry_state(&task.id, |state| {
        state.status = BulkChunkRetryStatus::Running;
        state.started_at_ms = Some(start_ms);
        state.updated_at_ms = start_ms;
        state.message = "Bulk failed chunk retry is running".to_string();
    });

    let targets = retryable_failed_chunk_files(&load_file_store(&task.id));
    update_bulk_chunk_retry_state(&task.id, |state| {
        state.queued_files = targets.len();
        state.total_failed_chunks = targets.iter().map(|target| target.failed_chunks).sum();
        state.updated_at_ms = now_ms();
        state.message = format!(
            "Retrying {} files with {} failed chunks",
            state.queued_files, state.total_failed_chunks
        );
    });

    tracing::info!(
        task_id = %task.id,
        queued_files = targets.len(),
        total_failed_chunks = targets.iter().map(|target| target.failed_chunks).sum::<usize>(),
        "starting bulk ASR failed chunk retry"
    );

    let mut refreshed_daily_documents = false;
    for target in targets {
        let file_start = Instant::now();
        update_bulk_chunk_retry_state(&task.id, |state| {
            state.current_file_key = Some(target.file_key.clone());
            state.current_source_path = Some(target.source_path.clone());
            state.updated_at_ms = now_ms();
            state.message = format!(
                "Retrying {} failed chunks in {}",
                target.failed_chunks, target.source_path
            );
        });
        tracing::info!(
            task_id = %task.id,
            file_key = %target.file_key,
            source_path = %target.source_path,
            failed_chunks = target.failed_chunks,
            "bulk ASR failed chunk retry started file"
        );

        let result = retry_failed_chunks_for_file_locked(&task.id, &target.file_key).await;
        let elapsed_ms = file_start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        match result {
            Ok(outcome) => {
                let still_failed = outcome.still_failed_chunks.len();
                if !outcome.daily_documents_refreshed.is_empty() {
                    refreshed_daily_documents = true;
                }
                let file_result = BulkChunkRetryFileResult {
                    file_key: target.file_key.clone(),
                    source_path: target.source_path.clone(),
                    failed_before: outcome.total,
                    recovered: outcome.recovered,
                    still_failed,
                    status: if still_failed == 0 && outcome.persist_warnings.is_empty() {
                        "success".to_string()
                    } else {
                        "partial_success".to_string()
                    },
                    elapsed_ms,
                    message: format!(
                        "Retried {} failed chunks: {} recovered, {} still failed",
                        outcome.total, outcome.recovered, still_failed
                    ),
                    daily_documents_refreshed: outcome.daily_documents_refreshed,
                    persist_warnings: outcome.persist_warnings,
                };
                tracing::info!(
                    task_id = %task.id,
                    file_key = %target.file_key,
                    recovered = file_result.recovered,
                    still_failed = file_result.still_failed,
                    elapsed_ms = elapsed_ms,
                    "bulk ASR failed chunk retry finished file"
                );
                update_bulk_chunk_retry_state(&task.id, |state| {
                    state.processed_files += 1;
                    state.recovered_chunks += file_result.recovered;
                    state.still_failed_chunks += file_result.still_failed;
                    state.updated_at_ms = now_ms();
                    state.results.push(file_result);
                });
            }
            Err(error) => {
                let message = retry_file_chunks_error_message(&error);
                let file_result = BulkChunkRetryFileResult {
                    file_key: target.file_key.clone(),
                    source_path: target.source_path.clone(),
                    failed_before: target.failed_chunks,
                    recovered: 0,
                    still_failed: target.failed_chunks,
                    status: "failed".to_string(),
                    elapsed_ms,
                    message: message.clone(),
                    daily_documents_refreshed: Vec::new(),
                    persist_warnings: Vec::new(),
                };
                tracing::warn!(
                    task_id = %task.id,
                    file_key = %target.file_key,
                    error = %message,
                    elapsed_ms = elapsed_ms,
                    "bulk ASR failed chunk retry failed file"
                );
                update_bulk_chunk_retry_state(&task.id, |state| {
                    state.processed_files += 1;
                    state.still_failed_chunks += file_result.still_failed;
                    state.updated_at_ms = now_ms();
                    state.results.push(file_result);
                });
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    update_bulk_chunk_retry_state(&task.id, |state| {
        let finished_at_ms = now_ms();
        state.current_file_key = None;
        state.current_source_path = None;
        state.finished_at_ms = Some(finished_at_ms);
        state.updated_at_ms = finished_at_ms;
        state.status = if state.still_failed_chunks == 0 {
            BulkChunkRetryStatus::Completed
        } else {
            BulkChunkRetryStatus::Failed
        };
        state.message = format!(
            "Bulk retry finished: {} recovered, {} still failed across {} files",
            state.recovered_chunks, state.still_failed_chunks, state.processed_files
        );
    });

    if let Some(state) = bulk_chunk_retry_state(&task.id) {
        tracing::info!(
            task_id = %task.id,
            processed_files = state.processed_files,
            recovered_chunks = state.recovered_chunks,
            still_failed_chunks = state.still_failed_chunks,
            "bulk ASR failed chunk retry completed"
        );
    }

    if refreshed_daily_documents {
        spawn_daily_agent_original_files_after_refresh(&task);
        maybe_enqueue_daily_agent_after_asr_run(&task).await;
    }
}

fn retryable_failed_chunk_files(store: &FileStore) -> Vec<BulkChunkRetryFileTarget> {
    let mut targets = store
        .files
        .iter()
        .filter(|(_, record)| !record.failed_chunks.is_empty())
        .map(|(file_key, record)| BulkChunkRetryFileTarget {
            file_key: file_key.clone(),
            source_path: record.source_path.display().to_string(),
            failed_chunks: record.failed_chunks.len(),
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| {
        left.source_path
            .cmp(&right.source_path)
            .then_with(|| left.file_key.cmp(&right.file_key))
    });
    targets
}

fn bulk_chunk_retry_state(task_id: &str) -> Option<BulkChunkRetryState> {
    BULK_CHUNK_RETRY_JOBS.lock().unwrap().get(task_id).cloned()
}

fn set_bulk_chunk_retry_state(state: BulkChunkRetryState) {
    BULK_CHUNK_RETRY_JOBS
        .lock()
        .unwrap()
        .insert(state.task_id.clone(), state);
}

fn update_bulk_chunk_retry_state<F>(task_id: &str, update: F)
where
    F: FnOnce(&mut BulkChunkRetryState),
{
    if let Some(state) = BULK_CHUNK_RETRY_JOBS.lock().unwrap().get_mut(task_id) {
        update(state);
    }
}

fn retry_file_chunks_error_message(error: &RetryFileChunksError) -> String {
    match error {
        RetryFileChunksError::TaskNotFound => "ASR task not found".to_string(),
        RetryFileChunksError::FileNotFound => "File not found in task".to_string(),
        RetryFileChunksError::NoFailedChunks(_) => "No failed chunks to retry".to_string(),
        RetryFileChunksError::Internal(error) => error.clone(),
    }
}
