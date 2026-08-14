struct RunningSourceCompressionGuard {
    task_id: String,
}

struct SourceCompressionFileLock {
    path: PathBuf,
}

impl SourceCompressionFileLock {
    fn acquire(task_id: &str) -> Result<Self, String> {
        let path = source_compression_run_lock_path(task_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create source compression lock dir: {error}"))?;
        }
        match create_task_run_lock(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                replace_stale_task_run_lock(
                    &path,
                    "ASR source-audio compression is already running",
                    "source compression lock",
                )?;
            }
            Err(error) => return Err(format!("create source compression lock: {error}")),
        }
        Ok(Self { path })
    }
}

impl Drop for SourceCompressionFileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct RemoveFileOnDrop(PathBuf);

#[cfg(test)]
static TEST_COMPRESSION_FFMPEG_PATH: Lazy<StdMutex<Option<PathBuf>>> =
    Lazy::new(|| StdMutex::new(None));

#[cfg(test)]
struct TestCompressionFfmpegGuard(Option<PathBuf>);

#[cfg(test)]
impl TestCompressionFfmpegGuard {
    fn set(path: PathBuf) -> Self {
        let previous = TEST_COMPRESSION_FFMPEG_PATH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(path);
        Self(previous)
    }
}

#[cfg(test)]
impl Drop for TestCompressionFfmpegGuard {
    fn drop(&mut self) {
        *TEST_COMPRESSION_FFMPEG_PATH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.0.take();
    }
}

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl RunningSourceCompressionGuard {
    fn acquire(task_id: &str) -> Result<Self, String> {
        let _lifecycle = SOURCE_AUDIO_LIFECYCLE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if task_is_running(task_id) {
            return Err("ASR task is running; wait for transcription to finish".to_string());
        }
        if external_import_is_running(task_id) {
            return Err("ASR external import is running; wait for it to finish".to_string());
        }
        if RUNNING_CHUNK_RETRY_TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(task_id)
            || bulk_chunk_retry_state(task_id).is_some_and(|retry| {
                matches!(
                    retry.status,
                    BulkChunkRetryStatus::Queued | BulkChunkRetryStatus::Running
                )
            })
        {
            return Err("ASR failed-chunk retry is running; wait for it to finish".to_string());
        }
        let mut running = RUNNING_SOURCE_COMPRESSION_TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !running.is_empty() {
            return Err("another ASR source-audio compression job is already running".to_string());
        }
        running.insert(task_id.to_string());
        Ok(Self {
            task_id: task_id.to_string(),
        })
    }
}

impl Drop for RunningSourceCompressionGuard {
    fn drop(&mut self) {
        RUNNING_SOURCE_COMPRESSION_TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.task_id);
        SOURCE_COMPRESSION_CANCEL_REQUESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.task_id);
        set_worker_source_compression_cancel(&self.task_id, false);
    }
}

fn source_compression_is_running(task_id: &str) -> bool {
    if RUNNING_SOURCE_COMPRESSION_TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(task_id)
    {
        return true;
    }
    let lock_path = source_compression_run_lock_path(task_id);
    if lock_path.is_file() && !is_task_run_lock_stale(&lock_path) {
        return true;
    }
    load_source_compression_state(task_id).is_some_and(|state| {
        state.status == SourceAudioCompressionStatus::Queued
            && now_ms().saturating_sub(state.updated_at_ms) < 2 * 60 * 1000
    })
}

fn source_compression_run_lock_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("source_compression.lock")
}

fn source_compression_state_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("source_compression.json")
}

fn load_source_compression_state(task_id: &str) -> Option<SourceAudioCompressionState> {
    std::fs::read_to_string(source_compression_state_path(task_id))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn save_source_compression_state(state: &SourceAudioCompressionState) -> Result<(), String> {
    atomic_json_write(&source_compression_state_path(&state.task_id), state)
}

fn normalized_source_compression_state(task_id: &str) -> Option<SourceAudioCompressionState> {
    let mut state = load_source_compression_state(task_id)?;
    if matches!(
        state.status,
        SourceAudioCompressionStatus::Queued
            | SourceAudioCompressionStatus::Running
            | SourceAudioCompressionStatus::Cancelling
    ) && !source_compression_is_running(task_id)
    {
        state.status = SourceAudioCompressionStatus::Interrupted;
        state.updated_at_ms = now_ms();
        state.finished_at_ms = Some(state.updated_at_ms);
        state.current_source_path = None;
        state.message =
            "Compression was interrupted; start it again to recover and continue safely."
                .to_string();
        let _ = save_source_compression_state(&state);
    }
    Some(state)
}

fn update_source_compression_state<F>(task_id: &str, update: F)
where
    F: FnOnce(&mut SourceAudioCompressionState),
{
    let Some(mut state) = load_source_compression_state(task_id) else {
        return;
    };
    update(&mut state);
    state.updated_at_ms = now_ms();
    if let Err(error) = save_source_compression_state(&state) {
        tracing::warn!(task_id, %error, "failed to persist source compression state");
    }
}

fn source_compression_cancel_requested(task_id: &str) -> bool {
    SOURCE_COMPRESSION_CANCEL_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(task_id)
        || worker_source_compression_cancel_path(task_id).is_file()
}

fn worker_source_compression_cancel_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("runtime/asr-worker/cancel")
        .join(format!("compression-{task_id}.cancel"))
}

pub(crate) fn set_worker_source_compression_cancel(task_id: &str, requested: bool) {
    let path = worker_source_compression_cancel_path(task_id);
    if requested {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, now_ms().to_string()) {
            warn!(task_id, error = %error, "failed to persist ASR compression cancel marker");
        }
    } else if let Err(error) = std::fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!(task_id, error = %error, "failed to clear ASR compression cancel marker");
        }
    }
}

pub(crate) fn cancel_all_worker_source_compressions() {
    let task_ids = RUNNING_SOURCE_COMPRESSION_TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    for task_id in task_ids {
        set_worker_source_compression_cancel(&task_id, true);
    }
}

fn start_source_compression_background(
    task: AsrDirectoryTask,
) -> Result<SourceAudioCompressionState, String> {
    if task_is_running(&task.id) {
        return Err("ASR task is running; wait for transcription to finish".to_string());
    }
    if external_import_is_running(&task.id) {
        return Err("ASR external import is running; wait for it to finish".to_string());
    }
    if bulk_chunk_retry_state(&task.id).is_some_and(|retry| {
        matches!(
            retry.status,
            BulkChunkRetryStatus::Queued | BulkChunkRetryStatus::Running
        )
    }) {
        return Err("ASR failed-chunk retry is running; wait for it to finish".to_string());
    }

    set_worker_source_compression_cancel(&task.id, false);
    let guard = RunningSourceCompressionGuard::acquire(&task.id)?;
    recover_source_compression_backups(&task)?;
    let store = load_file_store(&task.id);
    let targets = store
        .files
        .iter()
        .filter(|(_, record)| is_compressible_source_audio_record(&task, record))
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect::<Vec<_>>();
    let now = now_ms();
    let state = SourceAudioCompressionState {
        task_id: task.id.clone(),
        status: SourceAudioCompressionStatus::Queued,
        queued_files: targets.len(),
        processed_files: 0,
        compressed_files: 0,
        skipped_files: 0,
        failed_files: 0,
        original_bytes: targets
            .iter()
            .filter_map(|(_, record)| source_size(&record.source_path))
            .sum(),
        compressed_bytes: 0,
        saved_bytes: 0,
        started_at_ms: None,
        updated_at_ms: now,
        finished_at_ms: None,
        current_source_path: None,
        message: if targets.is_empty() {
            "No completed WAV source audio is eligible for compression.".to_string()
        } else {
            format!("Queued {} completed WAV file(s) for compression.", targets.len())
        },
        results: Vec::new(),
    };
    save_source_compression_state(&state)?;

    if targets.is_empty() {
        let mut completed = state;
        completed.status = SourceAudioCompressionStatus::Completed;
        completed.started_at_ms = Some(now);
        completed.finished_at_ms = Some(now);
        save_source_compression_state(&completed)?;
        return Ok(completed);
    }

    let response = state.clone();
    if !cfg!(test)
        && crate::worker_runtime::worker_execution_enabled(crate::worker_runtime::WorkerKind::Asr)
        && !crate::worker_runtime::asr::is_asr_worker_process()
    {
        let task_id = task.id.clone();
        tokio::spawn(async move {
            if let Err(error) = crate::worker_runtime::asr::run_source_compression(&task_id).await {
                update_source_compression_state(&task_id, |state| {
                    state.status = SourceAudioCompressionStatus::Interrupted;
                    state.finished_at_ms = Some(now_ms());
                    state.current_source_path = None;
                    state.message = format!("ASR compression worker failed: {error}");
                });
                tracing::warn!(task_id = %task_id, error = %error, "ASR compression worker request failed");
            }
            drop(guard);
        });
        return Ok(response);
    }

    tokio::task::spawn_blocking(move || run_source_compression_job(task, targets, guard, None));
    Ok(response)
}

pub(crate) async fn run_source_compression_in_worker(
    task_id: &str,
) -> Result<serde_json::Value, String> {
    let task = find_task(task_id).ok_or_else(|| format!("ASR task '{task_id}' not found"))?;
    set_worker_source_compression_cancel(task_id, false);
    let guard = RunningSourceCompressionGuard::acquire(task_id)?;
    let process_lock = SourceCompressionFileLock::acquire(task_id)?;
    recover_source_compression_backups(&task)?;
    let store = load_file_store(task_id);
    let targets = store
        .files
        .iter()
        .filter(|(_, record)| is_compressible_source_audio_record(&task, record))
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        run_source_compression_job(task, targets, guard, Some(process_lock))
    })
        .await
        .map_err(|error| format!("ASR compression worker task failed: {error}"))?;
    let state = load_source_compression_state(task_id)
        .ok_or_else(|| "ASR compression worker did not persist a final state".to_string())?;
    serde_json::to_value(state)
        .map_err(|error| format!("serialize ASR compression worker state: {error}"))
}

fn run_source_compression_job(
    task: AsrDirectoryTask,
    targets: Vec<(String, FileRecord)>,
    _guard: RunningSourceCompressionGuard,
    _process_lock: Option<SourceCompressionFileLock>,
) {
    update_source_compression_state(&task.id, |state| {
        state.status = SourceAudioCompressionStatus::Running;
        state.started_at_ms = Some(now_ms());
        state.message = "Compressing completed WAV source audio with lossless FLAC.".to_string();
    });

    if let Err(error) = recover_source_compression_backups(&task) {
        tracing::warn!(task_id = %task.id, %error, "source compression recovery warning");
    }

    for (old_key, record) in targets {
        if source_compression_cancel_requested(&task.id) {
            break;
        }
        update_source_compression_state(&task.id, |state| {
            state.current_source_path = Some(record.source_path.display().to_string());
        });
        let result = compress_source_record(&task, &old_key, &record);
        update_source_compression_state(&task.id, |state| {
            state.processed_files += 1;
            match result.status.as_str() {
                "compressed" => {
                    state.compressed_files += 1;
                    state.compressed_bytes += result.compressed_bytes;
                    state.saved_bytes += result.saved_bytes;
                }
                "skipped" => state.skipped_files += 1,
                _ => state.failed_files += 1,
            }
            state.results.push(result);
        });
    }

    let cancelled = source_compression_cancel_requested(&task.id);
    update_source_compression_state(&task.id, |state| {
        state.current_source_path = None;
        state.finished_at_ms = Some(now_ms());
        state.status = if cancelled {
            SourceAudioCompressionStatus::Cancelled
        } else if state.failed_files > 0 {
            SourceAudioCompressionStatus::CompletedWithErrors
        } else {
            SourceAudioCompressionStatus::Completed
        };
        state.message = if cancelled {
            format!(
                "Compression cancelled after {} of {} file(s).",
                state.processed_files, state.queued_files
            )
        } else {
            format!(
                "Compressed {} file(s), skipped {}, failed {}; saved {} bytes.",
                state.compressed_files, state.skipped_files, state.failed_files, state.saved_bytes
            )
        };
    });
}

fn compress_source_record(
    task: &AsrDirectoryTask,
    old_key: &str,
    record: &FileRecord,
) -> SourceAudioCompressionFileResult {
    let source = &record.source_path;
    let original_bytes = source_size(source).unwrap_or(0);
    let result = (|| -> Result<(PathBuf, u64, u64), String> {
        if !is_compressible_source_audio_record(task, record) {
            return Err("source is no longer eligible for compression".to_string());
        }
        let final_path = source.with_extension("flac");
        if final_path.exists() {
            return Err(format!("destination already exists: {}", final_path.display()));
        }
        let file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "source file name is not valid UTF-8".to_string())?;
        let part_path = source.with_file_name(format!(".{file_name}.bifrost-compress.part"));
        let backup_path = source.with_file_name(format!(".{file_name}.bifrost-compress-backup"));
        if backup_path.exists() {
            return Err(format!(
                "recovery backup already exists: {}",
                backup_path.display()
            ));
        }
        let _ = std::fs::remove_file(&part_path);
        let _part_cleanup = RemoveFileOnDrop(part_path.clone());

        encode_source_to_flac(source, &part_path, record.media_duration_ms)?;
        let compressed_bytes = source_size(&part_path)
            .ok_or_else(|| "compressed output is missing".to_string())?;
        if compressed_bytes >= original_bytes {
            let _ = std::fs::remove_file(&part_path);
            return Ok((source.clone(), original_bytes, 0));
        }
        let original_hash = decoded_pcm_sha256(source, record.media_duration_ms)?;
        let compressed_hash = decoded_pcm_sha256(&part_path, record.media_duration_ms)?;
        if original_hash != compressed_hash {
            let _ = std::fs::remove_file(&part_path);
            return Err("decoded PCM verification failed; original WAV was preserved".to_string());
        }
        if !source_matches_recorded_identity(record) || source_size(source) != Some(original_bytes) {
            return Err(
                "source WAV changed after transcription or during compression; original was preserved"
                    .to_string(),
            );
        }

        let original_permissions = std::fs::metadata(source)
            .map_err(|error| format!("read original permissions: {error}"))?
            .permissions();
        std::fs::set_permissions(&part_path, original_permissions)
            .map_err(|error| format!("preserve source permissions: {error}"))?;
        std::fs::rename(source, &backup_path)
            .map_err(|error| format!("create rollback backup: {error}"))?;
        if let Err(error) = std::fs::rename(&part_path, &final_path) {
            let _ = std::fs::rename(&backup_path, source);
            return Err(format!("install compressed audio: {error}"));
        }

        let mut store = load_file_store(&task.id);
        let Some(mut migrated) = store.files.remove(old_key) else {
            let _ = std::fs::remove_file(&final_path);
            let _ = std::fs::rename(&backup_path, source);
            return Err("ASR file record changed while compression was running".to_string());
        };
        migrated.source_path = final_path.clone();
        migrated.source_size = Some(compressed_bytes);
        // Keep the recording chronology stable even though the FLAC file itself is new.
        migrated.source_modified_ms = record.source_modified_ms;
        migrated.source_compression = Some(SourceAudioCompressionRecord {
            codec: "flac".to_string(),
            original_source_path: source.clone(),
            original_size_bytes: original_bytes,
            original_modified_ms: record.source_modified_ms,
            compressed_size_bytes: compressed_bytes,
            saved_bytes: original_bytes - compressed_bytes,
            pcm_sha256: original_hash,
            compressed_at_ms: now_ms(),
        });
        let new_key = source_key(&final_path);
        for other in store.files.values_mut() {
            if other.duplicate_of_source_key.as_deref() == Some(old_key) {
                other.duplicate_of_source_key = Some(new_key.clone());
                if other.transcript_alias.as_deref() == Some(source.to_string_lossy().as_ref()) {
                    other.transcript_alias = Some(final_path.display().to_string());
                }
            }
        }
        store.files.insert(new_key.clone(), migrated);
        if let Err(error) = save_file_store_with_removals(&task.id, &store, &[old_key.to_string()]) {
            let _ = std::fs::remove_file(&final_path);
            let _ = std::fs::rename(&backup_path, source);
            return Err(format!("persist migrated ASR record: {error}"));
        }
        sync_compression_auxiliary_records(
            &task.id,
            old_key,
            &new_key,
            source,
            &final_path,
            compressed_bytes,
        )?;
        std::fs::remove_file(&backup_path)
            .map_err(|error| format!("remove rollback backup: {error}"))?;
        Ok((final_path, compressed_bytes, original_bytes - compressed_bytes))
    })();

    match result {
        Ok((path, compressed_bytes, 0)) if path == *source => SourceAudioCompressionFileResult {
            source_path: source.display().to_string(),
            compressed_path: None,
            status: "skipped".to_string(),
            original_bytes,
            compressed_bytes,
            saved_bytes: 0,
            message: "FLAC would not reduce storage; original WAV was preserved.".to_string(),
        },
        Ok((path, compressed_bytes, saved_bytes)) => SourceAudioCompressionFileResult {
            source_path: source.display().to_string(),
            compressed_path: Some(path.display().to_string()),
            status: "compressed".to_string(),
            original_bytes,
            compressed_bytes,
            saved_bytes,
            message: "Lossless PCM verification passed.".to_string(),
        },
        Err(message) => SourceAudioCompressionFileResult {
            source_path: source.display().to_string(),
            compressed_path: None,
            status: "failed".to_string(),
            original_bytes,
            compressed_bytes: 0,
            saved_bytes: 0,
            message,
        },
    }
}

fn encode_source_to_flac(
    source: &Path,
    part_path: &Path,
    media_duration_ms: Option<u64>,
) -> Result<(), String> {
    let mut encode = source_compression_ffmpeg_command();
    encode
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:a:0",
            "-map_metadata",
            "0",
            "-c:a",
            "flac",
            "-compression_level",
            "8",
            "-f",
            "flac",
        ])
        .arg(part_path);
    let output = run_process_with_timeout(&mut encode, ffmpeg_normalize_timeout(media_duration_ms))?;
    if output.status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(part_path);
        Err(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn decoded_pcm_sha256(path: &Path, media_duration_ms: Option<u64>) -> Result<String, String> {
    let mut command = source_compression_ffmpeg_command();
    command
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-c:a",
            "pcm_s32le",
            "-f",
            "hash",
            "-hash",
            "sha256",
            "-",
        ]);
    let output = run_process_with_timeout(&mut command, ffmpeg_normalize_timeout(media_duration_ms))?;
    if !output.status.success() {
        return Err(format!(
            "decode PCM for verification: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout);
    value
        .trim()
        .strip_prefix("SHA256=")
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "ffmpeg did not return a SHA-256 PCM hash".to_string())
}

fn source_compression_ffmpeg_command() -> std::process::Command {
    #[cfg(test)]
    if let Some(path) = TEST_COMPRESSION_FFMPEG_PATH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return std::process::Command::new(path);
    }
    std::process::Command::new("ffmpeg")
}

fn run_process_with_timeout(
    command: &mut std::process::Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start process: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|error| format!("collect process output: {error}"));
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("process timed out after {} seconds", timeout.as_secs()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait for process: {error}"));
            }
        }
    }
}

fn sync_compression_auxiliary_records(
    task_id: &str,
    old_key: &str,
    new_key: &str,
    old_path: &Path,
    new_path: &Path,
    compressed_bytes: u64,
) -> Result<(), String> {
    let mut index = load_content_hash_index(task_id);
    let mut index_changed = false;
    for record in index.hashes.values_mut() {
        if record.canonical_source_key == old_key || record.canonical_source_path == old_path {
            record.canonical_source_key = new_key.to_string();
            record.canonical_source_path = new_path.to_path_buf();
            index_changed = true;
        }
    }
    if index_changed {
        save_content_hash_index(task_id, &index)?;
    }

    let mut imports = load_external_import_store(task_id);
    let mut imports_changed = false;
    for device in imports.devices.values_mut() {
        for record in device.files.values_mut() {
            if record.target_path == old_path {
                record.target_path = new_path.to_path_buf();
                record.target_size = compressed_bytes;
                imports_changed = true;
            }
        }
    }
    if imports_changed {
        save_external_import_store(task_id, &imports)?;
    }
    Ok(())
}

fn recover_source_compression_backups(task: &AsrDirectoryTask) -> Result<(), String> {
    let store = load_file_store(&task.id);
    for (key, record) in store.files {
        let source = &record.source_path;
        if !source_path_is_within_task_audio_dir_for_recovery(&task.audio_dir, source) {
            continue;
        }

        if record.source_compression.is_none() {
            let Some(file_name) = source.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let part = source.with_file_name(format!(".{file_name}.bifrost-compress.part"));
            let backup =
                source.with_file_name(format!(".{file_name}.bifrost-compress-backup"));
            let final_path = source.with_extension("flac");

            if backup.is_file() {
                if source.is_file() {
                    if !source_matches_recorded_identity(&record) {
                        return Err(format!(
                            "source and recovery backup both exist with conflicting identity: {}",
                            source.display()
                        ));
                    }
                    remove_file_if_exists(&part)?;
                    remove_file_if_exists(&final_path)?;
                    remove_file_if_exists(&backup)?;
                } else {
                    if !path_matches_recorded_identity(
                        &backup,
                        record.source_size,
                        record.source_modified_ms,
                    ) {
                        return Err(format!(
                            "recovery backup no longer matches the transcribed WAV: {}",
                            backup.display()
                        ));
                    }
                    remove_file_if_exists(&part)?;
                    remove_file_if_exists(&final_path)?;
                    std::fs::rename(&backup, source).map_err(|error| {
                        format!(
                            "restore interrupted source audio {}: {error}",
                            source.display()
                        )
                    })?;
                }
            } else if source.is_file() {
                remove_file_if_exists(&part)?;
            }
            continue;
        }

        let compression = record.source_compression.as_ref().unwrap();
        let old_path = &compression.original_source_path;
        if !source_path_is_within_task_audio_dir_for_recovery(&task.audio_dir, old_path) {
            continue;
        }
        let Some(file_name) = old_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let backup = old_path.with_file_name(format!(".{file_name}.bifrost-compress-backup"));
        let part = old_path.with_file_name(format!(".{file_name}.bifrost-compress.part"));
        if !backup.is_file() && !part.exists() {
            continue;
        }

        if source.is_file() {
            if source_size(source) != Some(compression.compressed_size_bytes)
                || decoded_pcm_sha256(source, record.media_duration_ms)? != compression.pcm_sha256
            {
                return Err(format!(
                    "compressed audio failed recovery verification; backup was preserved: {}",
                    source.display()
                ));
            }
            sync_compression_auxiliary_records(
                &task.id,
                "",
                &key,
                old_path,
                source,
                compression.compressed_size_bytes,
            )?;
            remove_file_if_exists(&part)?;
            remove_file_if_exists(&backup)?;
            continue;
        }

        if !backup.is_file() && !old_path.is_file() {
            continue;
        }

        remove_file_if_exists(&part)?;
        if backup.is_file() {
            if old_path.is_file() {
                if !path_matches_recorded_identity(
                    old_path,
                    Some(compression.original_size_bytes),
                    compression.original_modified_ms,
                ) {
                    return Err(format!(
                        "restored WAV no longer matches the compression record: {}",
                        old_path.display()
                    ));
                }
                remove_file_if_exists(&backup)?;
            } else {
                if !path_matches_recorded_identity(
                    &backup,
                    Some(compression.original_size_bytes),
                    compression.original_modified_ms,
                ) {
                    return Err(format!(
                        "rollback backup no longer matches the original WAV: {}",
                        backup.display()
                    ));
                }
                std::fs::rename(&backup, old_path).map_err(|error| {
                    format!(
                        "restore rollback backup {}: {error}",
                        old_path.display()
                    )
                })?;
            }
        } else if !path_matches_recorded_identity(
            old_path,
            Some(compression.original_size_bytes),
            compression.original_modified_ms,
        ) {
            return Err(format!(
                "restored WAV no longer matches the compression record: {}",
                old_path.display()
            ));
        }

        let old_key = source_key(old_path);
        let mut latest = load_file_store(&task.id);
        let Some(mut restored) = latest.files.remove(&key) else {
            continue;
        };
        restored.source_path = old_path.clone();
        restored.source_size = Some(compression.original_size_bytes);
        restored.source_modified_ms = compression.original_modified_ms;
        restored.source_compression = None;
        for other in latest.files.values_mut() {
            if other.duplicate_of_source_key.as_deref() == Some(key.as_str()) {
                other.duplicate_of_source_key = Some(old_key.clone());
                if other.transcript_alias.as_deref() == Some(source.to_string_lossy().as_ref()) {
                    other.transcript_alias = Some(old_path.display().to_string());
                }
            }
        }
        latest.files.insert(old_key.clone(), restored);
        save_file_store_with_removals(&task.id, &latest, std::slice::from_ref(&key))?;
        sync_compression_auxiliary_records(
            &task.id,
            &key,
            &old_key,
            source,
            old_path,
            compression.original_size_bytes,
        )?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

fn source_path_is_within_task_audio_dir_for_recovery(
    audio_dir: &Path,
    source_path: &Path,
) -> bool {
    let (Ok(audio_root), Some(parent)) = (audio_dir.canonicalize(), source_path.parent()) else {
        return false;
    };
    parent
        .canonicalize()
        .is_ok_and(|canonical_parent| canonical_parent.starts_with(audio_root))
}

fn cancel_source_compression(task_id: &str) -> Option<SourceAudioCompressionState> {
    if !source_compression_is_running(task_id) {
        return normalized_source_compression_state(task_id);
    }
    SOURCE_COMPRESSION_CANCEL_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(task_id.to_string());
    set_worker_source_compression_cancel(task_id, true);
    if crate::worker_runtime::worker_execution_enabled(crate::worker_runtime::WorkerKind::Asr)
        && !crate::worker_runtime::asr::is_asr_worker_process()
    {
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            crate::worker_runtime::asr::stop_source_compression(&task_id).await;
        });
    }
    update_source_compression_state(task_id, |state| {
        state.status = SourceAudioCompressionStatus::Cancelling;
        state.message = "Cancellation requested; the current file will finish safely.".to_string();
    });
    load_source_compression_state(task_id)
}
