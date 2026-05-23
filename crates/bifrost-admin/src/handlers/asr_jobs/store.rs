fn add_task(task: AsrDirectoryTask) -> Result<(), String> {
    let mut store = load_tasks();
    store.tasks.push(task);
    save_tasks(&store)
}

fn load_tasks() -> TaskStore {
    let path = task_store_path();
    let content = std::fs::read_to_string(path).ok();
    content
        .and_then(|content| serde_json::from_str::<TaskStore>(&content).ok())
        .filter(|store| store.version == TASK_STORE_VERSION)
        .unwrap_or(TaskStore {
            version: TASK_STORE_VERSION,
            tasks: Vec::new(),
        })
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
    let path = file_store_path(task_id);
    atomic_json_write(&path, store)
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
    let tmp = path.with_extension("tmp");
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
    let summary = if RUNNING_TASKS.lock().unwrap().contains(&task.id) {
        summarize_task_from_store(&task)
    } else {
        summarize_task(&task)
    };
    let daily_documents =
        list_daily_documents_for_task(&bifrost_storage::data_dir(), &task.id, &task.name)
            .unwrap_or_default();
    let mut files = load_file_store(&task.id)
        .files
        .into_iter()
        .map(|(key, record)| FileRecordWithKey { key, record })
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
        let reset_count = reset_interrupted_processing_records(&task.id, &mut files);
        let retryable_failed_count = reset_retryable_failed_records(&task.id, &mut files);
        if reset_count > 0 || retryable_failed_count > 0 {
            match save_file_store(&task.id, &files) {
                Ok(()) => {
                    tracing::warn!(
                        task_id = %task.id,
                        reset_count,
                        retryable_failed_count,
                        "reset recoverable ASR records on scheduler startup"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        task_id = %task.id,
                        reset_count,
                        retryable_failed_count,
                        error = %error,
                        "failed to persist recoverable ASR record reset"
                    );
                    continue;
                }
            }
        }
        if (reset_count > 0 || retryable_failed_count > 0) && task.last_error.is_some() {
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
    error.contains("managed ASR server start failed")
        && (error.contains("Qwen3-ASR service is busy")
            || error.contains("local server is reachable, but it is not managed by this Bifrost process")
            || error.contains("Failed to allocate a dynamic ASR service port")
            || error.contains("Timed out waiting for Qwen3-ASR model service to become healthy"))
}

fn summarize_task(task: &AsrDirectoryTask) -> TaskSummary {
    let discovered = discover_audio_files(&task.audio_dir, task.recursive).unwrap_or_default();
    let discovered_keys = discovered
        .iter()
        .map(|path| source_key(path))
        .collect::<HashSet<_>>();
    let file_store = load_file_store(&task.id);
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
    }
}

fn summarize_task_from_store(task: &AsrDirectoryTask) -> TaskSummary {
    let file_store = load_file_store(&task.id);
    summarize_task_records(task, &file_store, None)
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
        cleanable_source_bytes: 0,
        cleanable_source_file_count: 0,
        running: RUNNING_TASKS.lock().unwrap().contains(&task.id),
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

fn output_paths(task_id: &str, source: &Path, audio_dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    output_paths_in(&bifrost_storage::data_dir(), task_id, source, audio_dir)
}

fn output_paths_in(
    data_dir: &Path,
    task_id: &str,
    source: &Path,
    audio_dir: &Path,
) -> (PathBuf, PathBuf, PathBuf) {
    // Preserve the relative directory structure from audio_dir so that output
    // mirrors the original folder hierarchy (important for recursive tasks).
    let relative = source
        .strip_prefix(audio_dir)
        .unwrap_or(source.file_name().map(Path::new).unwrap_or(source));
    let stem = relative.with_extension("");
    let dir = text_output_dir(data_dir).join(task_id);
    (
        dir.join(format!("{}.txt", stem.display())),
        dir.join(format!("{}.json", stem.display())),
        dir.join(format!("{}.timeline.json", stem.display())),
    )
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
