/// Start the ASR task scheduler if not already running.
/// This is called from the router at server startup so that scheduled tasks
/// execute even if no one visits the ASR Tasks page.
pub(crate) async fn ensure_scheduler_started() {
    let mut started = ASR_SCHEDULER_STARTED.lock().await;
    if *started {
        return;
    }
    *started = true;
    let recovery_tasks = recover_interrupted_task_runs_on_startup();
    start_external_device_event_watcher();
    for task in recovery_tasks {
        let task_id = task.id.clone();
        match spawn_directory_task_run_background(task) {
            Ok(()) => {
                tracing::warn!(
                    task_id = %task_id,
                    "re-enqueued interrupted ASR directory task on scheduler startup"
                );
            }
            Err(error) if error == "ASR task is already running" => {
                tracing::debug!(
                    task_id = %task_id,
                    "skipped interrupted ASR directory task recovery because it is already running"
                );
            }
            Err(error) => {
                tracing::warn!(
                    task_id = %task_id,
                    error = %error,
                    "failed to re-enqueue interrupted ASR directory task on scheduler startup"
                );
            }
        }
    }
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let now = now_ms();
            let due = load_tasks()
                .tasks
                .into_iter()
                .filter(|task| {
                    task.enabled
                        && !task.paused
                        && task.next_run_at_ms.is_some_and(|next| next <= now)
                })
                .collect::<Vec<_>>();
            for task in due {
                let task_id = task.id.clone();
                if let Err(error) = spawn_directory_task_run_background(task) {
                    if error != "ASR task is already running" {
                        warn!(
                            task_id = %task_id,
                            error = %error,
                            "failed to start due ASR scheduled directory task"
                        );
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "macos")]
async fn sync_external_device_tasks(trigger: &'static str) {
    for task in load_tasks().tasks.into_iter().filter(|task| {
        task.import_policy.enabled && !task.paused && !task.external_devices.is_empty()
    }) {
        sync_external_device_task(task, trigger).await;
    }
}

#[cfg(target_os = "macos")]
async fn sync_external_device_task(task: AsrDirectoryTask, trigger: &'static str) {
    if RUNNING_TASKS.lock().unwrap().contains(&task.id) {
        return;
    }
    match sync_external_devices_for_task(&task).await {
        Ok(imported) if imported > 0 && task.import_policy.auto_run_after_import => {
            tracing::info!(
                task_id = %task.id,
                imported,
                trigger,
                "external device sync imported files"
            );
            let _ = spawn_directory_task_run(task);
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                task_id = %task.id,
                error = %error,
                trigger,
                "external device sync failed"
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn start_external_device_event_watcher() {
    tokio::spawn(async {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut child = match Command::new("diskutil")
            .arg("activity")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(%error, "failed to start macOS Disk Arbitration activity watcher");
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            tracing::warn!("macOS Disk Arbitration activity watcher has no stdout");
            let _ = child.kill().await;
            return;
        };
        let mut lines = BufReader::new(stdout).lines();
        let mut last_trigger_ms = 0u64;
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.contains("DiskAppeared")
                && !line.contains("DescriptionChanged")
                && !line.contains("DiskDisappeared")
            {
                continue;
            }
            let now = now_ms();
            if now.saturating_sub(last_trigger_ms) < 2_000 {
                continue;
            }
            last_trigger_ms = now;
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                sync_external_device_tasks("macos_disk_arbitration").await;
            });
        }
        match child.wait().await {
            Ok(status) => {
                tracing::warn!(%status, "macOS Disk Arbitration activity watcher exited");
            }
            Err(error) => {
                tracing::warn!(%error, "macOS Disk Arbitration activity watcher wait failed");
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn start_external_device_event_watcher() {}

async fn run_directory_task(
    task: AsrDirectoryTask,
) -> Result<(AsrDirectoryTask, usize, usize), String> {
    let _guard = ASR_JOB_RUN_LOCK.lock().await;
    let _task_lock = TaskRunFileLock::acquire(&task.id)?;
    if task_pause_requested(&task.id) {
        return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
    }
    let discovered = discover_audio_files(&task.audio_dir, task.recursive)?;
    let mut files = load_file_store(&task.id);
    let reset_count = reset_interrupted_processing_records(&task.id, &mut files);
    if reset_count > 0 {
        tracing::warn!(
            task_id = %task.id,
            reset_count,
            "reset interrupted ASR processing records before starting task run"
        );
    }
    for path in &discovered {
        let key = source_key(path);
        files
            .files
            .entry(key)
            .or_insert_with(|| pending_record(&task.id, path));
    }
    apply_content_hash_dedupe(&task, &discovered, &mut files)?;
    save_file_store(&task.id, &files)?;

    // Only re-process files that are truly pending or failed outright.
    // PartialSuccess files already have usable text/timeline output and
    // should be recovered via the retry-chunks API, NOT re-processed from
    // scratch (which would discard the existing partial results).
    let pending = discovered
        .iter()
        .filter(|path| {
            files
                .files
                .get(&source_key(path))
                .map(|record| {
                    matches!(
                        record.status,
                        FileStatus::Pending | FileStatus::Processing | FileStatus::Failed
                    )
                })
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();

    if pending.is_empty() {
        refresh_task_daily_summaries(&task)?;
        let updated = match update_task_after_run(&task.id, None) {
            Ok(task) => task,
            Err(error) => {
                warn!(
                    task_id = %task.id, %error,
                    "failed to update task metadata after merge-only ASR run"
                );
                task
            }
        };
        return Ok((updated, 0, 0));
    }

    let target = target_from_query(Some(&format!(
        "language={}&model={}",
        urlencoding::encode(&task.language),
        urlencoding::encode(&task.model)
    )))?;
    tracing::info!(
        task_id = %task.id,
        model = %task.model,
        language = %task.language,
        runtime_strategy = task.runtime_strategy.as_str(),
        pending_files = pending.len(),
        "starting ASR directory task run"
    );

    // Plan B (fork-per-chunk): resolve the `asr` binary and model directory
    // instead of starting a long-lived asr-server HTTP daemon. Each chunk
    // will fork a fresh `asr` CLI process, avoiding Metal/MLX state
    // degradation that makes Plan A ~2× slower on batch workloads.
    let asr_bin = target.install_dir().join("asr");
    let model_path = target.model_dir();
    if !asr_bin.is_file() {
        // Attempt asset repair before giving up.
        crate::handlers::asr::run_initializer_silent_pub(target.clone())
            .await
            .map_err(|e| format!("ASR asset preparation failed: {e}"))?;
        if !asr_bin.is_file() {
            return Err(format!(
                "asr CLI binary not found at {} after asset repair",
                asr_bin.display()
            ));
        }
    }
    if !model_path.join("tokenizer.json").is_file() {
        return Err(format!(
            "ASR model not found at {} — run ASR initialization first",
            model_path.display()
        ));
    }

    let mut server_url = None::<String>;
    let mut startup_fallback_reason = None::<String>;
    let mut stop_task_server_after_use = false;
    if task.runtime_strategy.uses_task_lifetime_server() {
        match start_task_managed_server(&task, &target, "task").await {
            Ok(server) => {
                server_url = Some(server.server_url);
                stop_task_server_after_use = server.stop_after_use;
            }
            Err(reason) if task.runtime_strategy == AsrRuntimeStrategy::Auto => {
                tracing::warn!(
                    task_id = %task.id,
                    %reason,
                    "ASR auto strategy falling back to fork_per_chunk during startup"
                );
                startup_fallback_reason = Some(reason);
            }
            Err(reason) => return Err(reason),
        }
    }

    let pause_check = || task_pause_requested(&task.id);
    let loop_result = process_pending_files(
        &task,
        &target,
        &asr_bin,
        &model_path,
        server_url.as_deref(),
        startup_fallback_reason.as_deref(),
        &pending,
        &mut files,
        &pause_check,
    )
    .await;

    if stop_task_server_after_use {
        tracing::info!(
            task_id = %task.id,
            runtime_strategy = task.runtime_strategy.as_str(),
            "stopping ASR managed server after task-scoped runtime strategy"
        );
        stop_any_managed_service().await;
    }

    let (processed_now, failed_now) = loop_result?;

    if let Err(error) = refresh_task_daily_summaries(&task) {
        warn!(task_id = %task.id, error = %error, "failed to generate daily summaries");
    }

    // update_task_after_run persists scheduling metadata (next_run_at_ms,
    // last_error).  If it fails, the per-file results in FileStore are
    // already safely persisted — so log a warning instead of discarding
    // the entire run result.
    let updated = match update_task_after_run(
        &task.id,
        (failed_now > 0).then(|| format!("{failed_now} file(s) failed")),
    ) {
        Ok(t) => t,
        Err(error) => {
            warn!(
                task_id = %task.id, %error,
                "failed to update task metadata after run; file results are safe"
            );
            task
        }
    };
    // Hook: trigger Daily Agent if configured
    maybe_enqueue_daily_agent_after_asr_run(&updated).await;

    Ok((updated, processed_now, failed_now))
}

fn spawn_directory_task_run_background(task: AsrDirectoryTask) -> Result<(), String> {
    FORCE_PAUSED_TASKS.lock().unwrap().remove(&task.id);
    let running_guard =
        RunningTaskGuard::acquire(&task.id).map_err(|_| "ASR task is already running".to_string())?;

    let task_id = task.id.clone();
    let task_clone = task.clone();
    tokio::spawn(async move {
        let running_guard = running_guard;
        let result = run_directory_task(task_clone.clone()).await;
        match &result {
            Ok((_updated, processed, failed)) => {
                tracing::info!(
                    task_id = %task_clone.id, processed = processed, failed = failed,
                    "ASR directory task completed"
                );
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                tracing::info!(
                    task_id = %task_clone.id,
                    "ASR directory task paused and released compute"
                );
            }
            Err(error) => {
                let _ = update_task_after_run(&task_clone.id, Some(error.clone()));
                tracing::warn!(
                    task_id = %task_clone.id, error = %error,
                    "ASR directory task failed"
                );
            }
        }
        drop(running_guard);
        tracing::debug!(
            task_id = %task_id,
            "released ASR directory task running marker"
        );
    });

    Ok(())
}

fn refresh_task_daily_summaries(task: &AsrDirectoryTask) -> Result<Vec<PathBuf>, String> {
    let task_output_dir = text_output_dir(&bifrost_storage::data_dir()).join(&task.id);
    let paths = generate_daily_summaries(&task_output_dir, &task.name)?;
    tracing::info!(
        task_id = %task.id,
        daily_files = paths.len(),
        "refreshed ASR daily summary markdown files"
    );
    Ok(paths)
}

async fn start_task_managed_server(
    task: &AsrDirectoryTask,
    target: &AsrTarget,
    scope: &str,
) -> Result<TaskManagedServer, String> {
    let before = read_service_state(&bifrost_storage::data_dir());
    let response = start_managed_service(target.clone())
        .await
        .map_err(|response| {
            format!(
                "managed ASR server start failed: {}; detail={}",
                response.message,
                response.detail.unwrap_or_default()
            )
        })?;

    if !response.ready {
        return Err(format!(
            "managed ASR server not ready: {}; detail={}",
            response.message,
            response.detail.unwrap_or_default()
        ));
    }

    let after = read_service_state(&bifrost_storage::data_dir());
    let stop_after_use = !service_state_same_process(before.as_ref(), after.as_ref());
    tracing::info!(
        task_id = %task.id,
        runtime_strategy = task.runtime_strategy.as_str(),
        scope,
        server_url = %response.server_url,
        stop_after_use,
        "ASR runtime strategy acquired managed server"
    );

    Ok(TaskManagedServer {
        server_url: response.server_url,
        stop_after_use,
    })
}

fn service_state_same_process(
    before: Option<&AsrServiceState>,
    after: Option<&AsrServiceState>,
) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => before.pid == after.pid && before.port == after.port,
        _ => false,
    }
}

/// Process all pending files using fork-per-chunk (Plan B).
/// Each file (or chunk of a long file) forks a fresh `asr` CLI process for
/// inference, avoiding Metal/MLX state degradation.
#[allow(clippy::too_many_arguments)]
async fn process_pending_files(
    task: &AsrDirectoryTask,
    target: &AsrTarget,
    asr_bin: &Path,
    model_path: &Path,
    server_url: Option<&str>,
    startup_fallback_reason: Option<&str>,
    pending: &[PathBuf],
    files: &mut FileStore,
    pause_check: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(usize, usize), String> {
    let mut processed_now = 0usize;
    let mut failed_now = 0usize;

    let total_pending = pending.len();

    for (file_index, path) in pending.iter().enumerate() {
        if pause_check() {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let key = source_key(path);
        let existing_content_hash = files
            .files
            .get(&key)
            .and_then(|record| record.content_hash.clone());
        let existing_hash_algorithm = files
            .files
            .get(&key)
            .and_then(|record| record.content_hash_algorithm.clone());
        let existing_memory_limit_hints = files
            .files
            .get(&key)
            .map(|record| record.memory_limit_hints.clone())
            .unwrap_or_default();
        // Cache source audio metadata once per file. This calls ffprobe under
        // the hood, so we avoid re-running it for every FileRecord construction.
        let source_info = inspect_source_audio(path);
        let file_started_at_ms = now_ms();
        let mut record = file_record_from_info(&task.id, path, &source_info);
        record.memory_limit_hints = existing_memory_limit_hints.clone();
        record.runtime_strategy = task.runtime_strategy;
        record.fallback_reason = startup_fallback_reason.map(str::to_string);
        record.status = FileStatus::Processing;
        record.started_at_ms = Some(file_started_at_ms);
        record.progress_current = Some(0);
        record.progress_total = Some(total_pending);
        files.files.insert(key.clone(), record);
        save_file_store(&task.id, files)?;

        let normalize_result =
            normalize_to_temp(path, Some(pause_check), source_info.media_duration_ms).await;

        let (wav_path, _temp_dir_path) = match normalize_result {
            Ok(result) => result,
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.status = FileStatus::Pending;
                record.started_at_ms = Some(file_started_at_ms);
                record.progress_current = Some(file_index);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                save_file_store(&task.id, files)?;
                return Err(error);
            }
            Err(error) => {
                // Mark the file as failed and continue with the next one.
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(format!("normalize failed: {error}"));
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                failed_now += 1;
                save_file_store(&task.id, files)?;
                continue;
            }
        };

        if pause_check() {
            let mut record = file_record_from_info(&task.id, path, &source_info);
            record.memory_limit_hints = existing_memory_limit_hints.clone();
            record.status = FileStatus::Pending;
            record.started_at_ms = Some(file_started_at_ms);
            record.progress_current = Some(file_index);
            record.progress_total = Some(total_pending);
            files.files.insert(key, record);
            let _ = std::fs::remove_dir_all(&_temp_dir_path);
            save_file_store(&task.id, files)?;
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }

        let mut file_server_url = server_url.map(str::to_string);
        let mut stop_file_server_after_use = false;
        if task.runtime_strategy == AsrRuntimeStrategy::ReusePerFile {
            match start_task_managed_server(task, target, "file").await {
                Ok(server) => {
                    file_server_url = Some(server.server_url);
                    stop_file_server_after_use = server.stop_after_use;
                }
                Err(error) => {
                    let mut record = file_record_from_info(&task.id, path, &source_info);
                    record.memory_limit_hints = existing_memory_limit_hints.clone();
                    record.runtime_strategy = task.runtime_strategy;
                    record.status = FileStatus::Failed;
                    record.started_at_ms = Some(file_started_at_ms);
                    record.error = Some(error);
                    record.finished_at_ms = Some(now_ms());
                    record.progress_current = Some(file_index + 1);
                    record.progress_total = Some(total_pending);
                    files.files.insert(key, record);
                    failed_now += 1;
                    let _ = std::fs::remove_dir_all(&_temp_dir_path);
                    save_file_store(&task.id, files)?;
                    continue;
                }
            }
        }

        // Construct a progress callback that updates FileStore on each chunk,
        // so the WebUI can show live chunk-level progress.
        let chunk_progress_cb = {
            let task_id = task.id.clone();
            let key = key.clone();
            let path = path.clone();
            let runtime_strategy = task.runtime_strategy;
            move |chunk_done: usize, chunk_total: usize| {
                let mut progress_store = load_file_store(&task_id);
                if let Some(rec) = progress_store.files.get_mut(&key) {
                    rec.progress_current = Some(chunk_done);
                    rec.progress_total = Some(chunk_total);
                } else {
                    // Construct a minimal record without calling pending_record(),
                    // which would run ffprobe on every chunk completion (expensive).
                    let rec = FileRecord {
                        task_id: task_id.clone(),
                        source_path: path.clone(),
                        source_size: None,
                        source_modified_ms: None,
                        source_created_at_ms: None,
                        source_created_at_source: None,
                        content_hash: None,
                        content_hash_algorithm: None,
                        duplicate_of_source_key: None,
                        transcript_alias: None,
                        media_duration_ms: None,
                        status: FileStatus::Processing,
                        output_text_path: None,
                        output_metadata_path: None,
                        output_timeline_path: None,
                        text_chars: 0,
                        error: None,
                        runtime_strategy,
                        chunk_metrics: Vec::new(),
                        fallback_reason: None,
                        started_at_ms: Some(now_ms()),
                        finished_at_ms: None,
                        progress_current: Some(chunk_done),
                        progress_total: Some(chunk_total),
                        failed_chunks: Vec::new(),
                        memory_limit_hints: Vec::new(),
                    };
                    progress_store.files.insert(key.clone(), rec);
                }
                let _ = save_file_store(&task_id, &progress_store);
                tracing::debug!(
                    task_id = %task_id,
                    chunk_done, chunk_total,
                    "chunk progress saved"
                );
            }
        };
        let chunk_metric_cb = {
            let task_id = task.id.clone();
            let key = key.clone();
            let path = path.clone();
            move |metric: AsrChunkMetric| {
                tracing::info!(
                    task_id = %task_id,
                    file = %path.display(),
                    chunk = metric.chunk_index,
                    offset_secs = metric.offset_secs,
                    duration_secs = metric.duration_secs,
                    runner = %metric.runner,
                    status = %metric.status,
                    elapsed_ms = metric.elapsed_ms,
                    rtf = metric.rtf,
                    text_chars = metric.text_chars,
                    fallback_reason = ?metric.fallback_reason,
                    server_url = ?metric.server_url,
                    error = ?metric.error,
                    "ASR chunk metric"
                );
                let mut metric_store = load_file_store(&task_id);
                if let Some(rec) = metric_store.files.get_mut(&key) {
                    rec.chunk_metrics.push(metric);
                    if rec.fallback_reason.is_none() {
                        rec.fallback_reason = rec
                            .chunk_metrics
                            .iter()
                            .find_map(|item| item.fallback_reason.clone());
                    }
                }
                let _ = save_file_store(&task_id, &metric_store);
            }
        };

        match transcribe_file_for_task_with_wav(
            task,
            asr_bin,
            model_path,
            path,
            &wav_path,
            &source_info,
            TaskTranscribeHooks {
                on_chunk_progress: Some(&chunk_progress_cb),
                on_chunk_metric: Some(&chunk_metric_cb),
                pause_check: Some(pause_check),
                force_pause_task_id: Some(&task.id),
                memory_limit_hints: &existing_memory_limit_hints,
                server_url: file_server_url.as_deref(),
                startup_fallback_reason,
            },
        )
        .await
        {
            Ok(output) => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = merge_memory_limit_hints(
                    existing_memory_limit_hints.clone(),
                    output.memory_limit_hints,
                );
                record.runtime_strategy = task.runtime_strategy;
                record.chunk_metrics = output.chunk_metrics;
                record.fallback_reason = output.fallback_reason;
                record.started_at_ms = Some(file_started_at_ms);
                // If some chunks failed, mark as partial_success instead of success.
                if output.failed_chunks.is_empty() {
                    record.status = FileStatus::Success;
                } else {
                    record.status = FileStatus::PartialSuccess;
                    record.failed_chunks = output.failed_chunks;
                    tracing::warn!(
                        file = %path.display(),
                        failed_count = record.failed_chunks.len(),
                        "file completed with partial failures — failed chunks recorded for later retry"
                    );
                }
                record.media_duration_ms = output.timeline.media_duration_ms;
                record.output_text_path = Some(output.text_path);
                record.output_metadata_path = Some(output.metadata_path);
                record.output_timeline_path = Some(output.timeline_path);
                record.content_hash = existing_content_hash.clone();
                record.content_hash_algorithm = existing_hash_algorithm.clone();
                record.text_chars = output.text.chars().count();
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key.clone(), record.clone());
                index_completed_file_hash(task, &key, &record);
                processed_now += 1;
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.status = FileStatus::Pending;
                record.started_at_ms = Some(file_started_at_ms);
                record.progress_current = Some(file_index);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                let _ = std::fs::remove_dir_all(&_temp_dir_path);
                if stop_file_server_after_use {
                    tracing::info!(
                        task_id = %task.id,
                        file = %path.display(),
                        runtime_strategy = task.runtime_strategy.as_str(),
                        "stopping ASR managed server after file-scoped runtime strategy"
                    );
                    stop_any_managed_service().await;
                }
                save_file_store(&task.id, files)?;
                return Err(error);
            }
            Err(error) => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.runtime_strategy = task.runtime_strategy;
                record.fallback_reason = startup_fallback_reason.map(str::to_string);
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(error);
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                failed_now += 1;
            }
        }
        if stop_file_server_after_use {
            tracing::info!(
                task_id = %task.id,
                file = %path.display(),
                runtime_strategy = task.runtime_strategy.as_str(),
                "stopping ASR managed server after file-scoped runtime strategy"
            );
            stop_any_managed_service().await;
        }
        // Clean up the temp dir BEFORE propagating save errors, so that a
        // save_file_store failure does not leak the normalize temp directory.
        let _ = std::fs::remove_dir_all(&_temp_dir_path);
        save_file_store(&task.id, files)?;
    }

    Ok((processed_now, failed_now))
}

/// Transcribe a pre-normalized WAV file for a directory task using
/// fork-per-chunk (Plan B). The `wav_path` must already be a 16kHz mono PCM WAV.
async fn transcribe_file_for_task_with_wav(
    task: &AsrDirectoryTask,
    asr_bin: &Path,
    model_path: &Path,
    path: &Path,
    wav: &Path,
    source_info: &SourceAudioInfo,
    hooks: TaskTranscribeHooks<'_>,
) -> Result<TranscriptionOutput, String> {
    let temp = tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;

    // For long audio (> chunk threshold), split into fixed-duration chunks and
    // process each via a separate `asr` CLI fork.
    const CHUNK_DURATION_SECS: u64 = ASR_TASK_SEGMENT_MAX_MS / 1000;
    // Overlap between consecutive chunks avoids cutting words at boundaries.
    // The `dedupe_increment` function de-duplicates the overlapping text.
    const CHUNK_OVERLAP_SECS: u64 = 2;
    let duration_ms = source_info.media_duration_ms.unwrap_or(0);
    let duration_secs = duration_ms.div_ceil(1000);

    let (
        result,
        failed_chunks,
        result_memory_limit_hints,
        result_chunk_metrics,
        result_fallback_reason,
    ) = if duration_ms > CHUNK_DURATION_SECS * 1000 {
        // Split into chunks and process each via fork-per-chunk.
        let chunked = transcribe_in_chunks(
            asr_bin,
            model_path,
            &task.language,
            wav,
            temp.path(),
            duration_secs,
            CHUNK_DURATION_SECS,
            CHUNK_OVERLAP_SECS,
            duration_ms,
            hooks.on_chunk_progress,
            hooks.pause_check,
            hooks.force_pause_task_id,
            &task.model,
            hooks.memory_limit_hints,
            task.runtime_strategy,
            hooks.server_url,
            hooks.startup_fallback_reason,
            hooks.on_chunk_metric,
        )
        .await?;
        (
            chunked.transcription,
            chunked.failed_chunks,
            chunked.memory_limit_hints,
            chunked.chunk_metrics,
            chunked.fallback_reason,
        )
    } else {
        if hooks.pause_check.is_some_and(|check| check()) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let mut server_state = hooks.server_url.map(|url| ServerRunnerState {
            server_url: url.to_string(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            force_fork_for_remaining: hooks.startup_fallback_reason.is_some(),
            fallback_reason: hooks.startup_fallback_reason.map(str::to_string),
        });
        let attempt = run_chunk_with_strategy(
            task.runtime_strategy,
            asr_bin,
            model_path,
            &task.language,
            wav,
            0,
            duration_secs.max(1),
            0,
            temp.path(),
            hooks.force_pause_task_id,
            &mut server_state,
            None,
        )
        .await?;
        if let Some(cb) = hooks.on_chunk_metric {
            cb(attempt.metric.clone());
            for metric in &attempt.shadow_metrics {
                cb(metric.clone());
            }
        }
        let mut chunk_metrics = vec![attempt.metric.clone()];
        chunk_metrics.extend(attempt.shadow_metrics.clone());
        (
            attempt.result?,
            Vec::new(),
            Vec::new(),
            chunk_metrics,
            server_state
                .and_then(|state| state.fallback_reason)
                .or_else(|| hooks.startup_fallback_reason.map(str::to_string)),
        )
    };

    let mut segments: Vec<TimelineSegment> = result
        .segments
        .into_iter()
        .enumerate()
        .map(
            |(index, (audio_start_ms, audio_end_ms, text))| TimelineSegment {
                index,
                audio_start_ms,
                audio_end_ms,
                absolute_start_ms: source_info
                    .source_created_at_ms
                    .map(|start| start.saturating_add(audio_start_ms)),
                absolute_end_ms: source_info
                    .source_created_at_ms
                    .map(|start| start.saturating_add(audio_end_ms)),
                text,
            },
        )
        .collect();

    let text = result.text;

    // The native `asr` CLI returns plain text only (no per-word timestamps).
    // Synthesize a single segment spanning the entire file duration so that
    // the timeline/daily-summary pipeline has something to work with.
    if segments.is_empty() && !text.is_empty() {
        segments.push(TimelineSegment {
            index: 0,
            audio_start_ms: 0,
            audio_end_ms: duration_ms,
            absolute_start_ms: source_info.source_created_at_ms,
            absolute_end_ms: source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(duration_ms)),
            text: text.clone(),
        });
    }
    let timeline = TranscriptTimeline {
        task_id: task.id.clone(),
        task_name: task.name.clone(),
        source_path: path.to_path_buf(),
        source_size: source_info.source_size,
        source_modified_ms: source_info.source_modified_ms,
        source_created_at_ms: source_info.source_created_at_ms,
        source_created_at_source: source_info.source_created_at_source.clone(),
        media_duration_ms: source_info.media_duration_ms,
        model: task.model.clone(),
        language: task.language.clone(),
        processed_at_ms: now_ms(),
        segments,
    };
    let (text_path, metadata_path, timeline_path) = output_paths(&task.id, path, &task.audio_dir);
    if let Some(parent) = text_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create text dir: {error}"))?;
    }
    let timeline_text = render_timeline_text(&timeline, &text);
    std::fs::write(&text_path, &timeline_text)
        .map_err(|error| format!("write transcript text: {error}"))?;
    std::fs::write(
        &timeline_path,
        serde_json::to_string_pretty(&timeline).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write transcript timeline: {error}"))?;
    let metadata = serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "source_path": path,
        "source_size": timeline.source_size,
        "source_modified_ms": timeline.source_modified_ms,
        "source_created_at_ms": timeline.source_created_at_ms,
        "source_created_at_source": timeline.source_created_at_source,
        "media_duration_ms": timeline.media_duration_ms,
        "text_path": text_path,
        "timeline_path": timeline_path,
        "model": task.model,
        "language": task.language,
        "runtime_strategy": task.runtime_strategy,
        "fallback_reason": result_fallback_reason.clone(),
        "chunk_metrics": result_chunk_metrics.clone(),
        "processed_at_ms": timeline.processed_at_ms,
    });
    std::fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write transcript metadata: {error}"))?;
    Ok(TranscriptionOutput {
        text,
        text_path,
        metadata_path,
        timeline_path,
        timeline,
        failed_chunks,
        memory_limit_hints: result_memory_limit_hints,
        chunk_metrics: result_chunk_metrics,
        fallback_reason: result_fallback_reason,
    })
}
