fn add_task(task: AsrDirectoryTask) -> Result<(), String> {
    let mut store = load_tasks();
    store.tasks.push(task);
    save_tasks(&store)
}

fn load_tasks() -> TaskStore {
    let path = task_store_path();
    let content = std::fs::read_to_string(path).ok();
    let mut store = content
        .and_then(|content| serde_json::from_str::<TaskStore>(&content).ok())
        .filter(|store| store.version == TASK_STORE_VERSION)
        .unwrap_or(TaskStore {
            version: TASK_STORE_VERSION,
            tasks: Vec::new(),
        });
    for task in &mut store.tasks {
        normalize_daily_agent_config_in_place(&mut task.daily_agent);
        if let Ok(audio_dir) = normalize_task_audio_dir_path(&task.audio_dir) {
            task.audio_dir = audio_dir;
        }
    }
    store
}

fn save_tasks(store: &TaskStore) -> Result<(), String> {
    let path = task_store_path();
    atomic_json_write(&path, store)
}

fn update_task_after_run(id: &str, error: Option<String>) -> Result<AsrDirectoryTask, String> {
    let mut store = load_tasks();
    let now = now_ms();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == id) else {
        return Err(format!("ASR task '{id}' not found"));
    };
    task.last_run_at_ms = Some(now);
    task.updated_at_ms = now;
    task.last_error = error;
    task.next_run_at_ms = if !task.enabled {
        None
    } else if task.paused {
        task.next_run_at_ms
    } else {
        task.schedule
            .next_run_at_ms(now.saturating_add(60_000), false)
    };
    let updated = task.clone();
    save_tasks(&store)?;
    Ok(updated)
}

fn update_task_paused(id: &str, paused: bool) -> Result<AsrDirectoryTask, String> {
    update_task_paused_with_mode(id, paused, AsrTaskPauseMode::LongTerm)
}

fn update_task_paused_with_mode(
    id: &str,
    paused: bool,
    mode: AsrTaskPauseMode,
) -> Result<AsrDirectoryTask, String> {
    let mut store = load_tasks();
    let now = now_ms();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == id) else {
        return Err(format!("ASR task '{id}' not found"));
    };
    task.paused = paused;
    task.paused_at_ms = paused.then_some(now);
    task.updated_at_ms = now;
    if paused {
        task.next_run_at_ms = match mode {
            AsrTaskPauseMode::Temporary => {
                task.next_run_at_ms.filter(|next| *next > now).or_else(|| {
                    if task.enabled {
                        task.schedule
                            .next_run_at_ms(now.saturating_add(60_000), false)
                    } else {
                        None
                    }
                })
            }
            AsrTaskPauseMode::LongTerm => None,
        };
        task.last_error = None;
    } else {
        task.next_run_at_ms = task
            .enabled
            .then(|| {
                task.schedule
                    .next_run_at_ms(now.saturating_add(60_000), false)
            })
            .flatten();
    }
    let updated = task.clone();
    save_tasks(&store)?;
    Ok(updated)
}

fn resume_temporary_paused_task_for_schedule(
    id: &str,
    now: u64,
) -> Result<Option<AsrDirectoryTask>, String> {
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == id) else {
        return Err(format!("ASR task '{id}' not found"));
    };
    if !task.paused {
        return Ok(Some(task.clone()));
    }
    if task
        .next_run_at_ms
        .is_none_or(|next_run_at_ms| next_run_at_ms > now)
    {
        return Ok(None);
    }
    task.paused = false;
    task.paused_at_ms = None;
    task.updated_at_ms = now;
    task.last_error = None;
    let updated = task.clone();
    save_tasks(&store)?;
    Ok(Some(updated))
}

fn task_pause_requested(id: &str) -> bool {
    load_tasks()
        .tasks
        .into_iter()
        .find(|task| task.id == id)
        .map(|task| task.paused)
        .unwrap_or(false)
}

fn task_force_pause_requested(id: &str) -> bool {
    FORCE_PAUSED_TASKS.lock().unwrap().contains(id) && task_pause_requested(id)
}

fn task_store_path() -> PathBuf {
    bifrost_storage::data_dir().join("asr/tasks.json")
}

fn file_store_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("files.json")
}

fn run_progress_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("run_progress.json")
}

fn load_run_progress(task_id: &str) -> (Option<AsrRunProgress>, Option<String>) {
    let path = run_progress_path(task_id);
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<AsrRunProgress>(&content) {
            Ok(progress) => (Some(progress), None),
            Err(error) => (
                None,
                Some(format!(
                    "parse run progress {}: {error}",
                    path.display()
                )),
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => (
            None,
            Some(format!("read run progress {}: {error}", path.display())),
        ),
    }
}

fn save_run_progress(task_id: &str, progress: &AsrRunProgress) -> Result<(), String> {
    atomic_json_write(&run_progress_path(task_id), progress)
}

fn start_run_progress(task_id: &str, trigger: &str) -> AsrRunProgress {
    let now = now_ms();
    let progress = AsrRunProgress {
        run_id: format!("{now}-{task_id}"),
        trigger: trigger.to_string(),
        status: "running".to_string(),
        started_at_ms: now,
        updated_at_ms: now,
        finished_at_ms: None,
        current_source_path: None,
        current_file_index: 0,
        current_file_total: 0,
        current_chunk_done: 0,
        current_chunk_total: 0,
        processed_now: 0,
        failed_now: 0,
        max_concurrent_files: default_max_concurrent_files(),
        effective_max_concurrent_files: default_max_concurrent_files(),
        active_file_count: 0,
        stage: "queued".to_string(),
        stage_message: Some("waiting for ASR worker".to_string()),
        message: Some("ASR directory task queued.".to_string()),
    };
    if let Err(error) = save_run_progress(task_id, &progress) {
        tracing::warn!(task_id, %error, "failed to persist ASR run progress");
    }
    progress
}

fn update_run_progress<F>(task_id: &str, update: F)
where
    F: FnOnce(&mut AsrRunProgress),
{
    let _guard = RUN_PROGRESS_UPDATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (progress, _) = load_run_progress(task_id);
    let mut progress = progress.unwrap_or_else(|| start_run_progress(task_id, "background"));
    update(&mut progress);
    progress.updated_at_ms = now_ms();
    if let Err(error) = save_run_progress(task_id, &progress) {
        tracing::warn!(task_id, %error, "failed to persist ASR run progress");
    }
}

fn finish_run_progress(
    task_id: &str,
    status: &str,
    processed_now: usize,
    failed_now: usize,
    message: Option<String>,
) {
    update_run_progress(task_id, |progress| {
        progress.status = status.to_string();
        progress.finished_at_ms = Some(now_ms());
        progress.processed_now = processed_now;
        progress.failed_now = failed_now;
        progress.stage = status.to_string();
        progress.stage_message = None;
        progress.message = message;
    });
}

fn load_file_store(task_id: &str) -> FileStore {
    let path = file_store_path(task_id);
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<FileStore>(&content) {
            Ok(store) if store.version == TASK_STORE_VERSION => store,
            Ok(store) => {
                tracing::warn!(
                    "ASR file store version mismatch: expected {}, got {}",
                    TASK_STORE_VERSION,
                    store.version
                );
                FileStore {
                    version: TASK_STORE_VERSION,
                    files: BTreeMap::new(),
                }
            }
            Err(error) => {
                tracing::warn!(
                    "ASR file store deserialize failed for {}: {error}",
                    path.display()
                );
                FileStore {
                    version: TASK_STORE_VERSION,
                    files: BTreeMap::new(),
                }
            }
        },
        Err(_) => FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        },
    }
}

fn save_file_store(task_id: &str, store: &FileStore) -> Result<(), String> {
    let _guard = FILE_STORE_WRITE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = file_store_path(task_id);
    let mut merged = match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<FileStore>(&content).unwrap_or_else(|_| FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        },
        Err(error) => {
            return Err(format!("read existing file store {}: {error}", path.display()));
        }
    };
    if merged.version != TASK_STORE_VERSION {
        merged.version = TASK_STORE_VERSION;
        merged.files.clear();
    }
    for (key, record) in store.files.iter() {
        merged.files.insert(key.clone(), record.clone());
    }
    normalize_completed_processing_records(task_id, &mut merged);
    atomic_json_write(&path, &merged)
}

/// Write a JSON file atomically: serialize → write to `<path>.tmp` → rename
/// over `<path>`. If the process crashes mid-write only the temp file is
/// corrupted; the original data remains intact.
fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(value).map_err(|e| format!("serialize JSON: {e}"))?;
    atomic_text_write(path, &content)
}

/// Write arbitrary text atomically: write to `<path>.tmp` → rename over
/// `<path>`. Same crash-safety guarantee as `atomic_json_write`.
fn atomic_text_write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create dir for {}: {e}", path.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("atomic");
    let tmp = path.with_file_name(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        now_ms(),
        ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, content.as_bytes())
        .map_err(|e| format!("write temp file {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

fn task_with_summary(task: AsrDirectoryTask) -> TaskWithSummary {
    let summary = summarize_task(&task);
    let bulk_retry = bulk_chunk_retry_state(&task.id);
    TaskWithSummary {
        task,
        summary,
        bulk_retry,
    }
}

fn task_with_control_summary(task: AsrDirectoryTask) -> TaskWithSummary {
    let summary = summarize_task_from_store(&task);
    let bulk_retry = bulk_chunk_retry_state(&task.id);
    TaskWithSummary {
        task,
        summary,
        bulk_retry,
    }
}

fn task_with_list_summary(task: AsrDirectoryTask) -> TaskWithSummary {
    if RUNNING_TASKS.lock().unwrap().contains(&task.id) {
        task_with_control_summary(task)
    } else {
        task_with_summary(task)
    }
}

fn find_task(id: &str) -> Option<AsrDirectoryTask> {
    load_tasks().tasks.into_iter().find(|task| task.id == id)
}

fn task_detail(task: AsrDirectoryTask) -> TaskDetail {
    let discovered = discover_audio_files(&task.audio_dir, task.recursive).unwrap_or_default();
    let mut file_store = load_file_store(&task.id);
    normalize_completed_processing_records(&task.id, &mut file_store);
    let visible = visible_file_entries(&task, &file_store, Some(&discovered));
    let visible_store = file_store_from_entries(visible.iter().cloned());
    let summary = summarize_task_records(&task, &visible_store, Some(&discovered));
    let daily_documents =
        list_daily_documents_for_task(&bifrost_storage::data_dir(), &task.id, &task.name)
            .unwrap_or_default();
    let mut files = visible
        .into_iter()
        .map(|(key, record)| file_record_with_key(&task, key, record))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        file_status_sort_rank(&left.record.status)
            .cmp(&file_status_sort_rank(&right.record.status))
            .then_with(|| left.record.source_path.cmp(&right.record.source_path))
    });
    TaskDetail {
        bulk_retry: bulk_chunk_retry_state(&task.id),
        task,
        summary,
        files,
        daily_documents,
    }
}

fn file_record_with_key(
    task: &AsrDirectoryTask,
    key: String,
    record: FileRecord,
) -> FileRecordWithKey {
    let (diarization_status, speaker_count) = diarization_file_state(task, &record);
    let diarization_manifest_path = task
        .diarization
        .enabled
        .then(|| diarization_manifest_path(&task.id, &record.source_path, &task.audio_dir));
    FileRecordWithKey {
        key,
        record,
        diarization_status,
        diarization_manifest_path,
        speaker_count,
    }
}

fn task_watch_snapshot(task: AsrDirectoryTask, include_recent: bool) -> TaskWatchSnapshot {
    let mut file_store = load_file_store(&task.id);
    normalize_completed_processing_records(&task.id, &mut file_store);
    task_watch_snapshot_from_store(task, &file_store, include_recent)
}

fn task_watch_snapshot_from_store(
    task: AsrDirectoryTask,
    file_store: &FileStore,
    include_recent: bool,
) -> TaskWatchSnapshot {
    let (run_progress, run_progress_warning) = load_run_progress(&task.id);
    let running = RUNNING_TASKS.lock().unwrap().contains(&task.id);
    let visible = visible_file_entries(&task, file_store, None);
    let visible_store = file_store_from_entries(visible.iter().cloned());
    let summary = summarize_task_records(&task, &visible_store, None);
    let service = read_service_state(&bifrost_storage::data_dir()).map(|state| TaskWatchService {
        managed: true,
        server_url: format!("http://{}:{}", state.host, state.port),
        pid: state.pid,
        owner_module: state.lease_owner_module().to_string(),
        owner_id: state.owner_id,
    });

    let current = current_watch_file(&visible_store, run_progress.as_ref());
    let current_key = current.as_ref().map(|(key, _)| key.clone());
    let current_record = current.as_ref().map(|(_, record)| *record);
    let current_path = run_progress
        .as_ref()
        .and_then(|progress| progress.current_source_path.clone())
        .or_else(|| current_record.map(|record| record.source_path.clone()));

    let (current_file_index, current_file_total) = run_progress
        .as_ref()
        .map(|progress| (progress.current_file_index, progress.current_file_total))
        .unwrap_or_else(|| {
            current_record
                .and_then(|record| Some((record.progress_current?, record.progress_total?)))
                .unwrap_or((0, 0))
        });
    let (current_chunk_done, current_chunk_total) = run_progress
        .as_ref()
        .map(|progress| (progress.current_chunk_done, progress.current_chunk_total))
        .unwrap_or_else(|| {
            current_record
                .filter(|record| record.status == FileStatus::Processing)
                .and_then(|record| Some((record.progress_current?, record.progress_total?)))
                .unwrap_or((0, 0))
        });

    let file_percent = if summary.discovered == 0 {
        0.0
    } else {
        ((summary.processed + summary.failed + summary.partial_success) as f64
            / summary.discovered as f64
            * 100.0)
            .min(100.0)
    };

    let consumption = task_watch_consumption(
        &visible_store,
        current_record,
        current_chunk_done,
        current_chunk_total,
    );
    let (eta_ms, eta_confidence) = estimate_watch_eta(
        &consumption,
        running,
        current_record,
        current_chunk_done,
        current_chunk_total,
    );

    let mut warnings = Vec::new();
    let mut snapshot_source = if run_progress.is_some() {
        "live_progress"
    } else {
        "file_store"
    }
    .to_string();
    if let Some(warning) = run_progress_warning {
        warnings.push(warning);
        snapshot_source = "file_store".to_string();
    }
    if let Some(progress) = run_progress.as_ref() {
        if progress.status == "running" && !running {
            snapshot_source = "stale_recovered".to_string();
            warnings.push("ASR run progress was running but no active task marker exists.".to_string());
        }
    }

    let recent_files = if include_recent {
        recent_watch_files(&task, &visible_store, 8)
    } else {
        Vec::new()
    };

    TaskWatchSnapshot {
        task: TaskWatchTask {
            id: task.id.clone(),
            name: task.name.clone(),
            audio_dir: task.audio_dir.clone(),
            enabled: task.enabled,
            paused: task.paused,
            running,
            schedule: task.schedule.clone(),
            language: task.language.clone(),
            model: task.model.clone(),
            runtime_strategy: task.runtime_strategy,
            max_concurrent_files: normalize_max_concurrent_files(task.max_concurrent_files),
            effective_max_concurrent_files: effective_max_concurrent_files(&task),
            diarization: task.diarization.clone(),
            last_run_at_ms: task.last_run_at_ms,
            next_run_at_ms: task.next_run_at_ms,
            last_error: task.last_error.clone(),
        },
        progress: TaskWatchProgress {
            discovered: summary.discovered,
            processed: summary.processed,
            pending: summary.pending,
            failed: summary.failed,
            partial_success: summary.partial_success,
            failed_chunk_count: summary.failed_chunk_count,
            deleted_after_processing: summary.deleted_after_processing,
            file_percent,
            current_file_key: current_key,
            current_source_path: current_path,
            current_file_index,
            current_file_total,
            current_chunk_done,
            current_chunk_total,
            max_concurrent_files: run_progress
                .as_ref()
                .map(|progress| progress.max_concurrent_files)
                .unwrap_or_else(|| normalize_max_concurrent_files(task.max_concurrent_files)),
            effective_max_concurrent_files: run_progress
                .as_ref()
                .map(|progress| progress.effective_max_concurrent_files)
                .unwrap_or_else(|| effective_max_concurrent_files(&task)),
            active_file_count: run_progress
                .as_ref()
                .map(|progress| progress.active_file_count)
                .unwrap_or(0),
            eta_ms,
            eta_confidence,
            stage: run_progress
                .as_ref()
                .map(|progress| progress.stage.clone())
                .unwrap_or_else(|| "idle".to_string()),
            stage_message: run_progress
                .as_ref()
                .and_then(|progress| progress.stage_message.clone()),
        },
        consumption,
        service,
        recent_files,
        daily_agent: task_watch_daily_agent(&task, 8),
        run_progress,
        last_error: task.last_error,
        snapshot_source,
        warnings,
        updated_at_ms: now_ms(),
    }
}

fn current_watch_file<'a>(
    file_store: &'a FileStore,
    run_progress: Option<&AsrRunProgress>,
) -> Option<(String, &'a FileRecord)> {
    if let Some(path) = run_progress.and_then(|progress| progress.current_source_path.as_ref()) {
        if let Some((key, record)) = file_store
            .files
            .iter()
            .find(|(_, record)| record.source_path == *path)
        {
            return Some((key.clone(), record));
        }
    }
    file_store
        .files
        .iter()
        .find(|(_, record)| record.status == FileStatus::Processing)
        .map(|(key, record)| (key.clone(), record))
        .or_else(|| {
            file_store
                .files
                .iter()
                .max_by_key(|(_, record)| record.finished_at_ms.unwrap_or(0))
                .map(|(key, record)| (key.clone(), record))
        })
}

fn task_watch_consumption(
    file_store: &FileStore,
    current_record: Option<&FileRecord>,
    current_chunk_done: usize,
    current_chunk_total: usize,
) -> TaskWatchConsumption {
    let mut source_bytes_total = 0u64;
    let mut source_bytes_processed = 0u64;
    let mut source_bytes_processed_estimated = None::<u64>;
    let mut audio_duration_ms_total = 0u64;
    let mut audio_duration_ms_processed = 0u64;
    let mut audio_duration_ms_processed_estimated = None::<u64>;
    let mut inference_elapsed_ms = 0u64;
    let mut text_chars = 0usize;
    let mut chunks_completed = 0usize;
    let mut chunks_failed = 0usize;
    let mut last_chunk = None::<&AsrChunkMetric>;

    for record in file_store.files.values() {
        if let Some(size) = record.source_size {
            source_bytes_total = source_bytes_total.saturating_add(size);
        }
        if let Some(duration) = record.media_duration_ms {
            audio_duration_ms_total = audio_duration_ms_total.saturating_add(duration);
        }
        if matches!(
            record.status,
            FileStatus::Success | FileStatus::PartialSuccess | FileStatus::Failed
        ) {
            if let Some(size) = record.source_size {
                source_bytes_processed = source_bytes_processed.saturating_add(size);
            }
            if let Some(duration) = record.media_duration_ms {
                audio_duration_ms_processed =
                    audio_duration_ms_processed.saturating_add(duration);
            }
        }
        text_chars = text_chars.saturating_add(record.text_chars);
        chunks_failed = chunks_failed.saturating_add(record.failed_chunks.len());
        for metric in &record.chunk_metrics {
            if metric.status == "ok" {
                inference_elapsed_ms = inference_elapsed_ms.saturating_add(metric.elapsed_ms);
                chunks_completed += 1;
            }
            if last_chunk
                .map(|last| last.recorded_at_ms < metric.recorded_at_ms)
                .unwrap_or(true)
            {
                last_chunk = Some(metric);
            }
        }
    }

    if let Some(record) = current_record.filter(|record| record.status == FileStatus::Processing) {
        let ratio = if current_chunk_total > 0 {
            (current_chunk_done as f64 / current_chunk_total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if let Some(size) = record.source_size {
            source_bytes_processed_estimated =
                Some(source_bytes_processed.saturating_add((size as f64 * ratio) as u64));
        }
        if let Some(duration) = record.media_duration_ms {
            audio_duration_ms_processed_estimated =
                Some(audio_duration_ms_processed.saturating_add((duration as f64 * ratio) as u64));
        }
    }

    let duration_for_rtf = audio_duration_ms_processed_estimated
        .unwrap_or(audio_duration_ms_processed)
        .max(audio_duration_ms_processed);
    let average_rtf = (duration_for_rtf > 0)
        .then(|| inference_elapsed_ms as f64 / duration_for_rtf as f64);

    TaskWatchConsumption {
        source_bytes_total,
        source_bytes_processed,
        source_bytes_processed_estimated,
        audio_duration_ms_total,
        audio_duration_ms_processed,
        audio_duration_ms_processed_estimated,
        inference_elapsed_ms,
        average_rtf,
        last_chunk_rtf: last_chunk.map(|metric| metric.rtf),
        text_chars,
        chunks_completed,
        chunks_failed,
    }
}

fn estimate_watch_eta(
    consumption: &TaskWatchConsumption,
    running: bool,
    current_record: Option<&FileRecord>,
    current_chunk_done: usize,
    current_chunk_total: usize,
) -> (Option<u64>, &'static str) {
    if !running {
        return (None, "none");
    }
    let Some(rtf) = consumption.average_rtf.filter(|rtf| *rtf > 0.0) else {
        return (None, "none");
    };
    let total = consumption.audio_duration_ms_total;
    let processed = consumption
        .audio_duration_ms_processed_estimated
        .unwrap_or(consumption.audio_duration_ms_processed);
    if total > processed {
        return (Some(((total - processed) as f64 * rtf) as u64), "medium");
    }
    if let Some(record) = current_record {
        if let Some(duration) = record.media_duration_ms {
            if current_chunk_total > current_chunk_done && current_chunk_total > 0 {
                let remaining_ratio =
                    (current_chunk_total - current_chunk_done) as f64 / current_chunk_total as f64;
                return (Some((duration as f64 * remaining_ratio * rtf) as u64), "low");
            }
        }
    }
    (None, "none")
}

fn recent_watch_files(
    task: &AsrDirectoryTask,
    file_store: &FileStore,
    limit: usize,
) -> Vec<TaskWatchFile> {
    let mut files = file_store.files.iter().collect::<Vec<_>>();
    files.sort_by(|(_, left), (_, right)| {
        file_status_sort_rank(&left.status)
            .cmp(&file_status_sort_rank(&right.status))
            .then_with(|| {
                right
                    .finished_at_ms
                    .unwrap_or(0)
                    .cmp(&left.finished_at_ms.unwrap_or(0))
            })
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    files
        .into_iter()
        .take(limit)
        .map(|(key, record)| watch_file_from_record(task, key, record))
        .collect()
}

fn watch_file_from_record(task: &AsrDirectoryTask, key: &str, record: &FileRecord) -> TaskWatchFile {
    let (diarization_status, speaker_count) = diarization_file_state(task, record);
    TaskWatchFile {
        key: key.to_string(),
        source_path: record.source_path.clone(),
        status: file_status_as_str(&record.status).to_string(),
        source_size: record.source_size,
        media_duration_ms: record.media_duration_ms,
        progress_current: record.progress_current,
        progress_total: record.progress_total,
        text_chars: record.text_chars,
        last_chunk_rtf: record
            .chunk_metrics
            .iter()
            .max_by_key(|metric| metric.recorded_at_ms)
            .map(|metric| metric.rtf),
        error: record.error.clone(),
        diarization_status,
        speaker_count,
        started_at_ms: record.started_at_ms,
        finished_at_ms: record.finished_at_ms,
    }
}

fn file_status_as_str(status: &FileStatus) -> &'static str {
    match status {
        FileStatus::Pending => "pending",
        FileStatus::Processing => "processing",
        FileStatus::Success => "success",
        FileStatus::PartialSuccess => "partial_success",
        FileStatus::Failed => "failed",
    }
}

fn file_status_sort_rank(status: &FileStatus) -> u8 {
    match status {
        FileStatus::Processing => 0,
        FileStatus::Pending => 1,
        FileStatus::Failed => 2,
        FileStatus::PartialSuccess => 3,
        FileStatus::Success => 4,
    }
}

fn recover_interrupted_task_runs_on_startup() -> Vec<AsrDirectoryTask> {
    let mut tasks_to_resume = Vec::new();
    let mut task_store = load_tasks();
    let mut task_store_changed = false;
    for task in task_store.tasks.iter_mut() {
        if task_is_running(&task.id) {
            continue;
        }
        let lock_path = task_run_lock_path(&task.id);
        let lock_exists = lock_path.exists();
        let stale_lock = lock_exists && is_task_run_lock_stale(&lock_path);
        if lock_exists && !stale_lock {
            continue;
        }
        if stale_lock {
            match std::fs::remove_file(&lock_path) {
                Ok(()) => {
                    tracing::warn!(
                        task_id = %task.id,
                        lock_path = %lock_path.display(),
                        "removed stale ASR task run lock on scheduler startup"
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(
                        task_id = %task.id,
                        lock_path = %lock_path.display(),
                        error = %error,
                        "failed to remove stale ASR task run lock on scheduler startup"
                    );
                    continue;
                }
            }
        }
        let mut files = load_file_store(&task.id);
        let completed_count = normalize_completed_processing_records(&task.id, &mut files);
        let reset_count = reset_interrupted_processing_records(&task.id, &mut files);
        let retryable_failed_count = reset_retryable_failed_records(&task.id, &mut files);
        if completed_count > 0 || reset_count > 0 || retryable_failed_count > 0 {
            match save_file_store(&task.id, &files) {
                Ok(()) => {
                    tracing::warn!(
                        task_id = %task.id,
                        completed_count,
                        reset_count,
                        retryable_failed_count,
                        "reset recoverable ASR records on scheduler startup"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        task_id = %task.id,
                        completed_count,
                        reset_count,
                        retryable_failed_count,
                        error = %error,
                        "failed to persist recoverable ASR record reset"
                    );
                    continue;
                }
            }
        }
        if (completed_count > 0 || reset_count > 0 || retryable_failed_count > 0)
            && task.last_error.is_some()
        {
            task.last_error = None;
            task.updated_at_ms = now_ms();
            task_store_changed = true;
        }
        // 修正 daily_agent.last_status 残留的 "running" 状态：进程已重启，
        // 不可能还有运行中的 daily agent，将其修正为 "interrupted"。
        if task.daily_agent.last_status.as_deref() == Some("running") {
            task.daily_agent.last_status = Some("interrupted".to_string());
            task.updated_at_ms = now_ms();
            task_store_changed = true;
        }
        for agent in &mut task.daily_agent.agents {
            if agent.last_status.as_deref() == Some("running") {
                agent.last_status = Some("interrupted".to_string());
                task.updated_at_ms = now_ms();
                task_store_changed = true;
            }
        }
        if (stale_lock || reset_count > 0 || retryable_failed_count > 0)
            && task.enabled
            && !task.paused
        {
            let summary = summarize_task(task);
            if summary.pending > 0 || summary.failed > 0 {
                tracing::warn!(
                    task_id = %task.id,
                    pending = summary.pending,
                    failed = summary.failed,
                    stale_lock,
                    reset_count,
                    retryable_failed_count,
                    "ASR task is eligible for startup recovery"
                );
                tasks_to_resume.push(task.clone());
            }
        }
    }
    if task_store_changed {
        if let Err(error) = save_tasks(&task_store) {
            tracing::warn!(
                error = %error,
                "failed to persist ASR task recovery metadata"
            );
        }
    }
    tasks_to_resume
}

fn reset_interrupted_processing_records(task_id: &str, files: &mut FileStore) -> usize {
    let mut reset_count = 0usize;
    for record in files.files.values_mut() {
        if record.status != FileStatus::Processing {
            continue;
        }
        if completed_processing_record_is_recoverable(record) {
            continue;
        }
        record.status = FileStatus::Pending;
        record.started_at_ms = None;
        record.finished_at_ms = None;
        record.progress_current = None;
        record.progress_total = None;
        record.error = None;
        reset_count += 1;
    }
    if reset_count > 0 {
        tracing::debug!(
            task_id = %task_id,
            reset_count,
            "reset interrupted ASR processing records to pending"
        );
    }
    reset_count
}

fn normalize_completed_processing_records(_task_id: &str, files: &mut FileStore) -> usize {
    let mut completed_count = 0usize;
    for record in files.files.values_mut() {
        if !completed_processing_record_is_recoverable(record) {
            continue;
        }
        record.status = FileStatus::Success;
        record.error = None;
        if record.progress_total.is_none() {
            record.progress_total = Some(record.chunk_metrics.len());
        }
        if let Some(total) = record.progress_total {
            record.progress_current = Some(total);
        }
        if record.text_chars == 0 {
            if let Some(path) = record.output_text_path.as_ref() {
                if let Ok(text) = std::fs::read_to_string(path) {
                    record.text_chars = text.chars().count();
                }
            }
        }
        record.finished_at_ms = Some(
            record
                .chunk_metrics
                .iter()
                .map(|metric| metric.recorded_at_ms)
                .max()
                .unwrap_or_else(now_ms),
        );
        completed_count += 1;
    }
    completed_count
}

fn completed_processing_record_is_recoverable(record: &FileRecord) -> bool {
    if record.status != FileStatus::Processing
        || record.error.is_some()
        || !record.failed_chunks.is_empty()
        || record.chunk_metrics.is_empty()
    {
        return false;
    }
    if !record.chunk_metrics.iter().all(|metric| metric.status == "ok") {
        return false;
    }
    let Some(total) = record.progress_total else {
        return false;
    };
    let chunk_indexes = record
        .chunk_metrics
        .iter()
        .map(|metric| metric.chunk_index)
        .collect::<HashSet<_>>();
    if total == 0 || chunk_indexes.len() < total {
        return false;
    }
    if !record
        .output_text_path
        .as_ref()
        .is_some_and(|path| path.is_file())
    {
        return false;
    }
    if let Some(path) = record.output_metadata_path.as_ref() {
        if !path.is_file() {
            return false;
        }
    }
    if let Some(path) = record.output_timeline_path.as_ref() {
        if !path.is_file() {
            return false;
        }
    }
    true
}

fn reset_retryable_failed_records(task_id: &str, files: &mut FileStore) -> usize {
    let mut reset_count = 0usize;
    for record in files.files.values_mut() {
        if record.status != FileStatus::Failed {
            continue;
        }
        let Some(error) = record.error.as_deref() else {
            continue;
        };
        if !is_retryable_asr_server_acquire_error(error) {
            continue;
        }
        record.status = FileStatus::Pending;
        record.started_at_ms = None;
        record.finished_at_ms = None;
        record.progress_current = None;
        record.progress_total = None;
        record.error = None;
        reset_count += 1;
    }
    if reset_count > 0 {
        tracing::debug!(
            task_id = %task_id,
            reset_count,
            "reset retryable failed ASR records to pending"
        );
    }
    reset_count
}

fn is_retryable_asr_server_acquire_error(error: &str) -> bool {
    error.trim() == "ASR diarization worker failed:"
        || error.contains("managed ASR server start failed")
        && (error.contains("Qwen3-ASR service is busy")
            || error.contains("local server is reachable, but it is not managed by this Bifrost process")
            || error.contains("Failed to allocate a dynamic ASR service port")
            || error.contains("Timed out waiting for Qwen3-ASR model service to become healthy"))
}

fn read_record_timeline(record: &FileRecord) -> Option<TranscriptTimeline> {
    let path = record.output_timeline_path.as_ref()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<TranscriptTimeline>(&content).ok()
}

fn timeline_speaker_count(timeline: &TranscriptTimeline) -> usize {
    if !timeline.speakers.is_empty() {
        return timeline.speakers.len();
    }
    timeline
        .segments
        .iter()
        .filter_map(|segment| segment.speaker.as_deref())
        .collect::<HashSet<_>>()
        .len()
}

fn diarization_file_state(
    task: &AsrDirectoryTask,
    record: &FileRecord,
) -> (Option<String>, Option<usize>) {
    if !task.diarization.enabled {
        return (None, None);
    }
    if record.status == FileStatus::Processing {
        return (Some("running".to_string()), None);
    }
    if record
        .error
        .as_deref()
        .is_some_and(|error| error.contains("diarization_missing_assets"))
    {
        return (Some("missing_assets".to_string()), None);
    }
    let speaker_count = read_record_timeline(record).and_then(|timeline| {
        (timeline.diarization_profile.is_some()).then(|| timeline_speaker_count(&timeline))
    });
    match speaker_count {
        Some(count) => (Some("success".to_string()), Some(count)),
        None if record.status == FileStatus::Failed => (Some("failed".to_string()), None),
        None => (Some("pending".to_string()), None),
    }
}

fn summarize_diarization(
    task: &AsrDirectoryTask,
    file_store: &FileStore,
) -> (bool, bool, usize, usize) {
    let ready = !task.diarization.enabled || diarization_profile_ready(&task.diarization.profile);
    let running = file_store
        .files
        .values()
        .any(|record| record.status == FileStatus::Processing);
    let mut diarized_files = 0usize;
    let mut speaker_ids = HashSet::new();
    for record in file_store.files.values() {
        if let Some(timeline) = read_record_timeline(record) {
            if timeline.diarization_profile.is_some() {
                diarized_files += 1;
                for speaker in timeline.speakers {
                    speaker_ids.insert(speaker.id);
                }
                for segment in timeline.segments {
                    if let Some(speaker) = segment.speaker {
                        speaker_ids.insert(speaker);
                    }
                }
            }
        }
    }
    (ready, running, diarized_files, speaker_ids.len())
}

fn summarize_task(task: &AsrDirectoryTask) -> TaskSummary {
    let discovered = discover_audio_files(&task.audio_dir, task.recursive).unwrap_or_default();
    let discovered_keys = discovered
        .iter()
        .map(|path| source_key(path))
        .collect::<HashSet<_>>();
    let mut file_store = load_file_store(&task.id);
    normalize_completed_processing_records(&task.id, &mut file_store);
    let visible = visible_file_entries(task, &file_store, Some(&discovered));
    let file_store = file_store_from_entries(visible);
    let audio_source_file_count = discovered.len();
    let audio_source_bytes = discovered
        .iter()
        .filter_map(|path| source_size(path))
        .sum::<u64>();
    let (cleanable_source_file_count, cleanable_source_bytes) =
        cleanable_source_audio_totals(task, &file_store);
    let processed = file_store
        .files
        .values()
        .filter(|record| {
            record.status == FileStatus::Success || record.status == FileStatus::PartialSuccess
        })
        .count();
    let failed = file_store
        .files
        .values()
        .filter(|record| record.status == FileStatus::Failed)
        .count();
    let partial_success = file_store
        .files
        .values()
        .filter(|record| record.status == FileStatus::PartialSuccess)
        .count();
    let failed_chunk_count: usize = file_store
        .files
        .values()
        .map(|record| record.failed_chunks.len())
        .sum();
    let pending = discovered
        .iter()
        .filter(|path| {
            file_store
                .files
                .get(&source_key(path))
                .map(|record| matches!(record.status, FileStatus::Pending | FileStatus::Processing))
                .unwrap_or(true)
        })
        .count();
    let deleted_after_processing = file_store
        .files
        .keys()
        .filter(|key| !discovered_keys.contains(*key))
        .count();
    let (diarization_ready, diarization_running, diarized_files, speaker_count) =
        summarize_diarization(task, &file_store);
    TaskSummary {
        discovered: discovered.len(),
        processed,
        pending,
        failed,
        partial_success,
        failed_chunk_count,
        deleted_after_processing,
        audio_source_bytes,
        audio_source_file_count,
        cleanable_source_bytes,
        cleanable_source_file_count,
        running: RUNNING_TASKS.lock().unwrap().contains(&task.id),
        max_concurrent_files: normalize_max_concurrent_files(task.max_concurrent_files),
        effective_max_concurrent_files: effective_max_concurrent_files(task),
        diarization_enabled: task.diarization.enabled,
        diarization_ready,
        diarization_running,
        diarized_files,
        speaker_count,
    }
}

fn summarize_task_from_store(task: &AsrDirectoryTask) -> TaskSummary {
    let mut file_store = load_file_store(&task.id);
    normalize_completed_processing_records(&task.id, &mut file_store);
    let visible = visible_file_entries(task, &file_store, None);
    let file_store = file_store_from_entries(visible);
    summarize_task_records(task, &file_store, None)
}

fn file_store_from_entries<I>(entries: I) -> FileStore
where
    I: IntoIterator<Item = (String, FileRecord)>,
{
    FileStore {
        version: TASK_STORE_VERSION,
        files: entries.into_iter().collect(),
    }
}

fn visible_file_entries(
    _task: &AsrDirectoryTask,
    file_store: &FileStore,
    discovered: Option<&[PathBuf]>,
) -> Vec<(String, FileRecord)> {
    let discovered_keys = discovered.map(|paths| {
        paths
            .iter()
            .map(|path| source_key(path))
            .collect::<HashSet<_>>()
    });
    let mut best_by_source = BTreeMap::<PathBuf, (String, FileRecord)>::new();
    for (key, record) in &file_store.files {
        match best_by_source.get(&record.source_path) {
            Some((best_key, best_record))
                if visible_record_rank(key, record, discovered_keys.as_ref())
                    <= visible_record_rank(best_key, best_record, discovered_keys.as_ref()) => {}
            _ => {
                best_by_source.insert(record.source_path.clone(), (key.clone(), record.clone()));
            }
        }
    }
    best_by_source.into_values().collect()
}

fn visible_record_rank(
    key: &str,
    record: &FileRecord,
    discovered_keys: Option<&HashSet<String>>,
) -> (u8, u64, usize, u64) {
    let is_current = discovered_keys.is_some_and(|keys| keys.contains(key));
    let status_rank = if is_current {
        5
    } else {
        match record.status {
            FileStatus::Success | FileStatus::PartialSuccess => 4,
            FileStatus::Processing => 3,
            FileStatus::Failed => 2,
            FileStatus::Pending => 1,
        }
    };
    (
        status_rank,
        record
            .finished_at_ms
            .or(record.started_at_ms)
            .or(record.source_modified_ms)
            .unwrap_or(0),
        record.chunk_metrics.len(),
        record.text_chars as u64,
    )
}

fn summarize_task_records(
    task: &AsrDirectoryTask,
    file_store: &FileStore,
    discovered: Option<&[PathBuf]>,
) -> TaskSummary {
    let discovered_keys = discovered.map(|paths| {
        paths
            .iter()
            .map(|path| source_key(path))
            .collect::<HashSet<_>>()
    });
    let processed = file_store
        .files
        .values()
        .filter(|record| {
            record.status == FileStatus::Success || record.status == FileStatus::PartialSuccess
        })
        .count();
    let failed = file_store
        .files
        .values()
        .filter(|record| record.status == FileStatus::Failed)
        .count();
    let partial_success = file_store
        .files
        .values()
        .filter(|record| record.status == FileStatus::PartialSuccess)
        .count();
    let failed_chunk_count: usize = file_store
        .files
        .values()
        .map(|record| record.failed_chunks.len())
        .sum();
    let pending = match discovered {
        Some(paths) => paths
            .iter()
            .filter(|path| {
                file_store
                    .files
                    .get(&source_key(path))
                    .map(|record| {
                        matches!(record.status, FileStatus::Pending | FileStatus::Processing)
                    })
                    .unwrap_or(true)
            })
            .count(),
        None => file_store
            .files
            .values()
            .filter(|record| matches!(record.status, FileStatus::Pending | FileStatus::Processing))
            .count(),
    };
    let deleted_after_processing = discovered_keys
        .as_ref()
        .map(|keys| {
            file_store
                .files
                .keys()
                .filter(|key| !keys.contains(*key))
                .count()
        })
        .unwrap_or(0);

    let (diarization_ready, diarization_running, diarized_files, speaker_count) =
        summarize_diarization(task, file_store);
    let (cleanable_source_file_count, cleanable_source_bytes) = if discovered.is_some() {
        cleanable_source_audio_totals(task, file_store)
    } else {
        (0, 0)
    };
    TaskSummary {
        discovered: discovered.map(|paths| paths.len()).unwrap_or(file_store.files.len()),
        processed,
        pending,
        failed,
        partial_success,
        failed_chunk_count,
        deleted_after_processing,
        audio_source_bytes: discovered
            .map(|paths| paths.iter().filter_map(|path| source_size(path)).sum())
            .unwrap_or(0),
        audio_source_file_count: discovered.map(|paths| paths.len()).unwrap_or(0),
        cleanable_source_bytes,
        cleanable_source_file_count,
        running: RUNNING_TASKS.lock().unwrap().contains(&task.id),
        max_concurrent_files: normalize_max_concurrent_files(task.max_concurrent_files),
        effective_max_concurrent_files: effective_max_concurrent_files(task),
        diarization_enabled: task.diarization.enabled,
        diarization_ready,
        diarization_running,
        diarized_files,
        speaker_count,
    }
}

fn cleanable_source_audio_totals(task: &AsrDirectoryTask, store: &FileStore) -> (usize, u64) {
    store
        .files
        .values()
        .filter(|record| is_cleanable_source_audio_record(task, record))
        .fold((0usize, 0u64), |(count, bytes), record| {
            (
                count + 1,
                bytes + source_size(&record.source_path).or(record.source_size).unwrap_or(0),
            )
        })
}

fn is_cleanable_source_audio_record(task: &AsrDirectoryTask, record: &FileRecord) -> bool {
    record.status == FileStatus::Success
        && record.source_path.is_file()
        && source_path_is_under_audio_dir(&task.audio_dir, &record.source_path)
        && record
            .output_text_path
            .as_ref()
            .is_some_and(|path| path.is_file())
        && record
            .output_timeline_path
            .as_ref()
            .is_some_and(|path| path.is_file())
}

fn source_path_is_under_audio_dir(audio_dir: &Path, source_path: &Path) -> bool {
    match (audio_dir.canonicalize(), source_path.canonicalize()) {
        (Ok(audio_root), Ok(source)) => source.starts_with(audio_root),
        _ => false,
    }
}

fn cleanup_task_source_audio(task: &AsrDirectoryTask) -> CleanupSourceAudioResponse {
    let store = load_file_store(&task.id);
    let mut deleted_files = 0usize;
    let mut deleted_bytes = 0u64;
    let mut skipped_files = 0usize;
    let mut skipped_bytes = 0u64;
    let mut failed_files = Vec::new();

    for record in store.files.values() {
        if !record.source_path.exists() {
            continue;
        }
        let size = source_size(&record.source_path)
            .or(record.source_size)
            .unwrap_or(0);
        if !is_cleanable_source_audio_record(task, record) {
            if record.source_path.is_file() && is_audio_file(&record.source_path) {
                skipped_files += 1;
                skipped_bytes += size;
            }
            continue;
        }
        match std::fs::remove_file(&record.source_path) {
            Ok(()) => {
                deleted_files += 1;
                deleted_bytes += size;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failed_files.push(CleanupSourceAudioFailure {
                source_path: record.source_path.display().to_string(),
                error: error.to_string(),
            }),
        }
    }

    let summary = summarize_task(task);
    let ok = failed_files.is_empty();
    let message = if failed_files.is_empty() {
        format!("Deleted {deleted_files} ASR source audio file(s).")
    } else {
        format!(
            "Deleted {deleted_files} ASR source audio file(s); {} file(s) failed.",
            failed_files.len()
        )
    };
    CleanupSourceAudioResponse {
        ok,
        deleted_files,
        deleted_bytes,
        skipped_files,
        skipped_bytes,
        failed_files,
        summary,
        message,
    }
}

fn discover_audio_files(root: &Path, recursive: bool) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Err(format!(
            "audio directory does not exist: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    discover_audio_files_inner(root, recursive, &mut files)?;
    files.sort();
    Ok(files)
}

fn discover_audio_files_inner(
    root: &Path,
    recursive: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for entry in
        std::fs::read_dir(root).map_err(|error| format!("read dir {}: {error}", root.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(error) => {
                tracing::warn!(dir = %root.display(), %error, "skipping unreadable directory entry");
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "skipping entry with unreadable file type");
                continue;
            }
        };
        if file_type.is_dir() && recursive {
            // Skip hidden .daily directory to avoid conflicts with daily reports.
            if path.file_name().and_then(|n| n.to_str()) == Some(".daily") {
                continue;
            }
            // A single subdirectory failure should not abort the entire scan.
            if let Err(error) = discover_audio_files_inner(&path, recursive, out) {
                tracing::warn!(dir = %path.display(), %error, "skipping unreadable subdirectory");
            }
        } else if file_type.is_file() && is_audio_file(&path) {
            // Skip 0-byte files — they contain no audio data and would
            // only cause downstream ffmpeg/asr errors.
            match entry.metadata() {
                Ok(meta) if meta.len() == 0 => {
                    tracing::debug!(path = %path.display(), "skipping 0-byte audio file");
                }
                _ => out.push(path),
            }
        }
    }
    Ok(())
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn pending_record(task_id: &str, path: &Path) -> FileRecord {
    let source_info = inspect_source_audio(path);
    file_record_from_info(task_id, path, &source_info)
}

/// Build a `FileRecord` from pre-cached `SourceAudioInfo`. Use this instead of
/// `pending_record()` inside loops where you already have the info, to avoid
/// spawning a redundant ffprobe subprocess per call.
fn file_record_from_info(task_id: &str, path: &Path, source_info: &SourceAudioInfo) -> FileRecord {
    FileRecord {
        task_id: task_id.to_string(),
        source_path: path.to_path_buf(),
        source_size: source_info.source_size,
        source_modified_ms: source_info.source_modified_ms,
        source_created_at_ms: source_info.source_created_at_ms,
        source_created_at_source: source_info.source_created_at_source.clone(),
        content_hash: None,
        content_hash_algorithm: None,
        duplicate_of_source_key: None,
        transcript_alias: None,
        media_duration_ms: source_info.media_duration_ms,
        status: FileStatus::Pending,
        output_text_path: None,
        output_metadata_path: None,
        output_timeline_path: None,
        text_chars: 0,
        error: None,
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        chunk_metrics: Vec::new(),
        fallback_reason: None,
        started_at_ms: None,
        finished_at_ms: None,
        progress_current: None,
        progress_total: None,
        failed_chunks: Vec::new(),
        memory_limit_hints: Vec::new(),
    }
}

fn source_key(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha1::new();
    hasher.update(absolute.to_string_lossy().as_bytes());
    if let Some(size) = source_size(path) {
        hasher.update(size.to_le_bytes());
    }
    if let Some(modified_ms) = source_modified_ms(path) {
        hasher.update(modified_ms.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
fn output_paths_in(
    data_dir: &Path,
    task_id: &str,
    source: &Path,
    audio_dir: &Path,
) -> (PathBuf, PathBuf, PathBuf) {
    bifrost_asr::artifacts::output_paths_in(data_dir, task_id, source, audio_dir)
}

struct TaskRunFileLock {
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskRunLockFile {
    pid: u32,
    process_start_time: u64,
    acquired_at_ms: u64,
}

impl TaskRunFileLock {
    fn acquire(task_id: &str) -> Result<Self, String> {
        let path = task_run_lock_path(task_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create task lock dir: {error}"))?;
        }
        match create_task_run_lock(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !is_task_run_lock_stale(&path) {
                    return Err("ASR task is already running".to_string());
                }
                warn!(
                    lock_path = %path.display(),
                    "Removing stale ASR task run lock"
                );
                std::fs::remove_file(&path).map_err(|remove_error| {
                    format!("remove stale ASR task lock: {remove_error}")
                })?;
                create_task_run_lock(&path)
                    .map_err(|create_error| format!("recreate ASR task lock: {create_error}"))?;
            }
            Err(error) => return Err(format!("create ASR task lock: {error}")),
        }
        Ok(Self { path })
    }
}

fn task_run_lock_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("run.lock")
}

impl Drop for TaskRunFileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn create_task_run_lock(path: &Path) -> std::io::Result<()> {
    let lock = TaskRunLockFile {
        pid: std::process::id(),
        process_start_time: current_process_start_time(),
        acquired_at_ms: now_ms(),
    };
    let content = serde_json::to_vec_pretty(&lock)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(&content)
        })
}

fn is_task_run_lock_stale(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(lock) = serde_json::from_str::<TaskRunLockFile>(&content) else {
        return true;
    };
    // Safety net: treat locks older than 12 hours as stale regardless of
    // PID liveness, to prevent permanently stuck tasks after edge-case
    // crashes or PID recycling.
    let age_ms = now_ms().saturating_sub(lock.acquired_at_ms);
    if age_ms > 12 * 60 * 60 * 1000 {
        return true;
    }
    !process_instance_is_alive(lock.pid, lock.process_start_time)
}

fn process_instance_is_alive(pid: u32, expected_start_time: u64) -> bool {
    let pid = Pid::from_u32(pid);
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
    system
        .process(pid)
        .map(|process| process.start_time() == expected_start_time)
        .unwrap_or(false)
}

fn current_process_start_time() -> u64 {
    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
    system
        .process(pid)
        .map(|process| process.start_time())
        .unwrap_or(0)
}
