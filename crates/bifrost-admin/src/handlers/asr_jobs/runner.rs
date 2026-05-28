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
                        && task.next_run_at_ms.is_some_and(|next| next <= now)
                        && (!task.paused || task.next_run_at_ms.is_some())
                })
                .collect::<Vec<_>>();
            for task in due {
                let task_id = task.id.clone();
                let task = if task.paused {
                    match resume_temporary_paused_task_for_schedule(&task.id, now) {
                        Ok(Some(task)) => task,
                        Ok(None) => continue,
                        Err(error) => {
                            warn!(
                                task_id = %task_id,
                                error = %error,
                                "failed to auto-resume temporary paused ASR scheduled directory task"
                            );
                            continue;
                        }
                    }
                } else {
                    task
                };
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
    for task in load_tasks()
        .tasks
        .into_iter()
        .filter(task_allows_external_device_event_import)
    {
        sync_external_device_task(task, trigger).await;
    }
}

#[cfg(any(target_os = "macos", test))]
fn task_allows_external_device_event_import(task: &AsrDirectoryTask) -> bool {
    task.import_policy.enabled && !task.external_devices.is_empty()
}

#[cfg(target_os = "macos")]
async fn sync_external_device_task(task: AsrDirectoryTask, trigger: &'static str) {
    if RUNNING_TASKS.lock().unwrap().contains(&task.id) {
        return;
    }
    match start_external_import_background(task.clone(), trigger) {
        Ok(progress) => {
            tracing::info!(
                task_id = %task.id,
                run_id = %progress.run_id,
                trigger,
                "external device sync queued in background"
            );
        }
        Err(error) if error == "ASR external import is already running" => {}
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

struct PendingBatchScan {
    pending: Vec<PathBuf>,
}

fn discover_and_prepare_pending_batch(
    task: &AsrDirectoryTask,
    files: &mut FileStore,
    attempted_keys: &HashSet<String>,
) -> Result<PendingBatchScan, String> {
    let discovered = discover_audio_files(&task.audio_dir, task.recursive)?;
    for path in &discovered {
        let key = source_key(path);
        files
            .files
            .entry(key)
            .or_insert_with(|| pending_record(&task.id, path));
    }
    apply_external_import_hashes_to_records(task, &discovered, files);
    apply_content_hash_dedupe(task, &discovered, files)?;
    save_file_store(&task.id, files)?;

    // Only process files that are truly pending or failed outright.
    // PartialSuccess files already have usable text/timeline output and should
    // be recovered via retry-chunks, not re-processed from scratch.
    let mut pending = discovered
        .iter()
        .filter(|path| {
            let key = source_key(path);
            !attempted_keys.contains(&key)
                && files
                    .files
                    .get(&key)
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
    sort_pending_paths_by_source_time(files, &mut pending);
    Ok(PendingBatchScan { pending })
}

fn sort_pending_paths_by_source_time(files: &FileStore, pending: &mut [PathBuf]) {
    pending.sort_by(|left, right| {
        let left_key = source_key(left);
        let right_key = source_key(right);
        let left_record = files.files.get(&left_key);
        let right_record = files.files.get(&right_key);
        pending_source_time_ms(left_record)
            .cmp(&pending_source_time_ms(right_record))
            .then_with(|| pending_modified_time_ms(left_record).cmp(&pending_modified_time_ms(right_record)))
            .then_with(|| left.cmp(right))
    });
}

fn pending_source_time_ms(record: Option<&FileRecord>) -> u64 {
    record
        .and_then(|record| record.source_created_at_ms)
        .unwrap_or(u64::MAX)
}

fn pending_modified_time_ms(record: Option<&FileRecord>) -> u64 {
    record
        .and_then(|record| record.source_modified_ms)
        .unwrap_or(u64::MAX)
}

async fn run_directory_task(
    task: AsrDirectoryTask,
) -> Result<(AsrDirectoryTask, usize, usize), String> {
    let _guard = ASR_JOB_RUN_LOCK.lock().await;
    let _task_lock = TaskRunFileLock::acquire(&task.id)?;
    start_run_progress(&task.id, "background");
    if task_pause_requested(&task.id) {
        return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
    }
    let mut files = load_file_store(&task.id);
    let reset_count = reset_interrupted_processing_records(&task.id, &mut files);
    if reset_count > 0 {
        tracing::warn!(
            task_id = %task.id,
            reset_count,
            "reset interrupted ASR processing records before starting task run"
        );
    }
    let mut attempted_keys = HashSet::new();
    let mut pending_scan =
        discover_and_prepare_pending_batch(&task, &mut files, &attempted_keys)?;
    update_run_progress(&task.id, |progress| {
        progress.current_file_total = pending_scan.pending.len();
        progress.current_file_index = 0;
        progress.current_chunk_done = 0;
        progress.current_chunk_total = 0;
        progress.message = Some(format!(
            "discovered {} pending file(s)",
            pending_scan.pending.len()
        ));
    });

    if pending_scan.pending.is_empty() {
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

    let resource_decision = crate::handlers::speech::acquire_speech_resource(
        crate::handlers::speech::directory_task_lease(&task.id, &task.model),
    );
    if !resource_decision.granted {
        update_run_progress(&task.id, |progress| {
            progress.stage = "paused".to_string();
            progress.stage_message = resource_decision.reason.clone();
            progress.message = Some(
                "directory task is waiting for realtime voice resources to become idle"
                    .to_string(),
            );
        });
        return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
    }
    let _resource_guard = SpeechResourceLeaseGuard {
        lease: resource_decision.lease,
    };

    let target = target_from_query(Some(&format!(
        "language={}&model={}&owner_module=directory_task&owner_id={}",
        urlencoding::encode(&task.language),
        urlencoding::encode(&task.model),
        urlencoding::encode(&task.id)
    )))?;
    tracing::info!(
        task_id = %task.id,
        model = %task.model,
        language = %task.language,
        runtime_strategy = task.runtime_strategy.as_str(),
        pending_files = pending_scan.pending.len(),
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
    let mut task_server_state = server_url.as_ref().map(|url| ServerRunnerState {
        server_url: url.clone(),
        baseline_rtf: None,
        baseline_samples: Vec::new(),
        server_failures: 0,
        force_fork_for_remaining: startup_fallback_reason.is_some(),
        restart_required: false,
        current_chunk_failure_reason: None,
        fallback_reason: startup_fallback_reason.clone(),
    });

    let pause_check = || {
        task_pause_requested(&task.id)
            || crate::handlers::speech::directory_task_should_yield_for_realtime(&task.id)
    };
    let mut processed_now = 0usize;
    let mut failed_now = 0usize;
    let loop_result: Result<(), String> = loop {
        let pending = std::mem::take(&mut pending_scan.pending);
        for path in &pending {
            attempted_keys.insert(source_key(path));
        }
        let (batch_processed, batch_failed) = match process_pending_files(
            &task,
            &target,
            &asr_bin,
            &model_path,
            server_url.as_deref(),
            startup_fallback_reason.as_deref(),
            &pending,
            processed_now,
            failed_now,
            &mut files,
            &pause_check,
            &mut task_server_state,
            &mut stop_task_server_after_use,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => break Err(error),
        };
        processed_now += batch_processed;
        failed_now += batch_failed;

        files = load_file_store(&task.id);
        pending_scan =
            match discover_and_prepare_pending_batch(&task, &mut files, &attempted_keys) {
                Ok(scan) => scan,
                Err(error) => break Err(error),
            };
        update_run_progress(&task.id, |progress| {
            progress.current_file_index = 0;
            progress.current_file_total = pending_scan.pending.len();
            progress.current_chunk_done = 0;
            progress.current_chunk_total = 0;
            progress.processed_now = processed_now;
            progress.failed_now = failed_now;
            progress.message = if pending_scan.pending.is_empty() {
                Some("no newly appended pending files found".to_string())
            } else {
                Some(format!(
                    "discovered {} appended pending file(s)",
                    pending_scan.pending.len()
                ))
            };
        });
        if pending_scan.pending.is_empty() {
            break Ok(());
        }
        tracing::info!(
            task_id = %task.id,
            pending_files = pending_scan.pending.len(),
            "continuing ASR directory task run with appended files"
        );
    };

    if stop_task_server_after_use {
        tracing::info!(
            task_id = %task.id,
            runtime_strategy = task.runtime_strategy.as_str(),
            "stopping ASR managed server after task-scoped runtime strategy"
        );
        stop_managed_service_for_target(&target).await;
    }

    loop_result?;

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

struct SpeechResourceLeaseGuard {
    lease: Option<bifrost_asr::resources::ResourceLease>,
}

impl Drop for SpeechResourceLeaseGuard {
    fn drop(&mut self) {
        crate::handlers::speech::release_speech_resource(self.lease.as_ref());
    }
}

fn spawn_directory_task_run_background(task: AsrDirectoryTask) -> Result<(), String> {
    FORCE_PAUSED_TASKS.lock().unwrap().remove(&task.id);
    let running_guard = RunningTaskGuard::acquire(&task.id)
        .map_err(|_| "ASR task is already running".to_string())?;

    let task_id = task.id.clone();
    let task_clone = task.clone();
    tokio::spawn(async move {
        let running_guard = running_guard;
        let result = run_directory_task(task_clone.clone()).await;
        match &result {
            Ok((_updated, processed, failed)) => {
                finish_run_progress(
                    &task_clone.id,
                    "completed",
                    *processed,
                    *failed,
                    Some(format!(
                        "ASR directory task completed; processed {processed}, failed {failed}."
                    )),
                );
                tracing::info!(
                    task_id = %task_clone.id, processed = processed, failed = failed,
                    "ASR directory task completed"
                );
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                finish_run_progress(
                    &task_clone.id,
                    "paused",
                    0,
                    0,
                    Some("ASR directory task paused and released compute.".to_string()),
                );
                tracing::info!(
                    task_id = %task_clone.id,
                    "ASR directory task paused and released compute"
                );
            }
            Err(error) => {
                let _ = update_task_after_run(&task_clone.id, Some(error.clone()));
                finish_run_progress(
                    &task_clone.id,
                    "failed",
                    0,
                    0,
                    Some(error.clone()),
                );
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
    processed_now_base: usize,
    failed_now_base: usize,
    files: &mut FileStore,
    pause_check: &(dyn Fn() -> bool + Send + Sync),
    task_server_state: &mut Option<ServerRunnerState>,
    stop_task_server_after_use: &mut bool,
) -> Result<(usize, usize), String> {
    let mut processed_now = processed_now_base;
    let mut failed_now = failed_now_base;

    let total_pending = pending.len();

    for (file_index, path) in pending.iter().enumerate() {
        update_run_progress(&task.id, |progress| {
            progress.current_source_path = Some(path.clone());
            progress.current_file_index = file_index + 1;
            progress.current_file_total = total_pending;
            progress.current_chunk_done = 0;
            progress.current_chunk_total = 0;
            progress.processed_now = processed_now;
            progress.failed_now = failed_now;
            progress.stage = if task.diarization.enabled {
                "normalize".to_string()
            } else {
                "asr".to_string()
            };
            progress.stage_message = task
                .diarization
                .enabled
                .then(|| format!("speaker diarization profile: {}", task.diarization.profile));
            progress.message = Some(format!("processing {}", path.display()));
        });
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

        if task.diarization.enabled {
            update_run_progress(&task.id, |progress| {
                progress.stage = "diarize".to_string();
                progress.stage_message = Some(format!(
                    "identifying speaker turns with {}",
                    task.diarization.profile
                ));
            });
            if !diarization_profile_ready(&task.diarization.profile) {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.runtime_strategy = task.runtime_strategy;
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(format!(
                    "diarization_missing_assets: profile '{}' is not initialized",
                    task.diarization.profile
                ));
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key.clone(), record);
                failed_now += 1;
                let _ = std::fs::remove_dir_all(&_temp_dir_path);
                save_file_store(&task.id, files)?;
                update_run_progress(&task.id, |progress| {
                    progress.processed_now = processed_now;
                    progress.failed_now = failed_now;
                    progress.stage = "failed".to_string();
                    progress.stage_message = Some("speaker diarization profile is not initialized".to_string());
                    progress.message = Some(format!(
                        "processed {processed_now} file(s), failed {failed_now}"
                    ));
                });
                continue;
            }
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
        let mut file_server_state = file_server_url.as_ref().map(|url| ServerRunnerState {
            server_url: url.clone(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: 0,
            force_fork_for_remaining: startup_fallback_reason.is_some(),
            restart_required: false,
            current_chunk_failure_reason: None,
            fallback_reason: startup_fallback_reason.map(str::to_string),
        });
        let chunk_progress_cb = {
            let task_id = task.id.clone();
            let key = key.clone();
            let path = path.clone();
            let runtime_strategy = task.runtime_strategy;
            move |chunk_done: usize, chunk_total: usize| {
                update_run_progress(&task_id, |progress| {
                    progress.current_source_path = Some(path.clone());
                    progress.current_chunk_done = chunk_done;
                    progress.current_chunk_total = chunk_total;
                    progress.message = Some(format!("processing chunk {chunk_done}/{chunk_total}"));
                });
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

        let use_task_lifetime_server = task.runtime_strategy.uses_task_lifetime_server();
        update_run_progress(&task.id, |progress| {
            progress.stage = "asr".to_string();
            progress.stage_message = Some(if task.diarization.enabled {
                "transcribing diarized audio segments".to_string()
            } else {
                "transcribing audio".to_string()
            });
        });
        let transcription_result = if use_task_lifetime_server {
            transcribe_file_for_task_with_wav(
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
                    server_state: Some(task_server_state),
                    managed_server_restart: Some(ManagedServerRestartContext {
                        task,
                        target,
                        scope: "task",
                        stop_after_use: &mut *stop_task_server_after_use,
                    }),
                },
            )
            .await
        } else {
            transcribe_file_for_task_with_wav(
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
                    server_state: Some(&mut file_server_state),
                    managed_server_restart: Some(ManagedServerRestartContext {
                        task,
                        target,
                        scope: "file",
                        stop_after_use: &mut stop_file_server_after_use,
                    }),
                },
            )
            .await
        };
        let fallback_reason_after_transcription = if use_task_lifetime_server {
            task_server_state
                .as_ref()
                .and_then(|state| state.fallback_reason.clone())
        } else {
            file_server_state
                .as_ref()
                .and_then(|state| state.fallback_reason.clone())
        };

        match transcription_result {
            Ok(output) => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = merge_memory_limit_hints(
                    existing_memory_limit_hints.clone(),
                    output.memory_limit_hints,
                );
                record.runtime_strategy = task.runtime_strategy;
                record.chunk_metrics = output.chunk_metrics;
                record.fallback_reason = output
                    .fallback_reason
                    .or_else(|| fallback_reason_after_transcription.clone());
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
                update_run_progress(&task.id, |progress| {
                    progress.processed_now = processed_now;
                    progress.failed_now = failed_now;
                    progress.stage = "finalize".to_string();
                    progress.stage_message = Some("speaker timeline and ASR artifacts written".to_string());
                    progress.message = Some(format!(
                        "processed {processed_now} file(s), failed {failed_now}"
                    ));
                });
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
                    stop_managed_service_for_target(target).await;
                }
                save_file_store(&task.id, files)?;
                return Err(error);
            }
            Err(error) => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.runtime_strategy = task.runtime_strategy;
                record.fallback_reason = fallback_reason_after_transcription
                    .clone()
                    .or_else(|| startup_fallback_reason.map(str::to_string));
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(error);
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                failed_now += 1;
                update_run_progress(&task.id, |progress| {
                    progress.processed_now = processed_now;
                    progress.failed_now = failed_now;
                    progress.message = Some(format!(
                        "processed {processed_now} file(s), failed {failed_now}"
                    ));
                });
            }
        }
        if stop_file_server_after_use {
            tracing::info!(
                task_id = %task.id,
                file = %path.display(),
                runtime_strategy = task.runtime_strategy.as_str(),
                "stopping ASR managed server after file-scoped runtime strategy"
            );
            stop_managed_service_for_target(target).await;
        }
        // Clean up the temp dir BEFORE propagating save errors, so that a
        // save_file_store failure does not leak the normalize temp directory.
        let _ = std::fs::remove_dir_all(&_temp_dir_path);
        save_file_store(&task.id, files)?;
    }

    Ok((
        processed_now.saturating_sub(processed_now_base),
        failed_now.saturating_sub(failed_now_base),
    ))
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
    mut hooks: TaskTranscribeHooks<'_>,
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

    let mut diarized_timeline_segments = None::<Vec<TimelineSegment>>;
    let mut diarized_speakers = None::<Vec<TimelineSpeaker>>;
    let mut diarization_segments_for_manifest = None::<Vec<DiarizationSegment>>;
    let (
        result,
        failed_chunks,
        result_memory_limit_hints,
        result_chunk_metrics,
        result_fallback_reason,
    ) = if task.diarization.enabled {
        let diarized = transcribe_diarized_segments_for_task(
            task,
            asr_bin,
            model_path,
            wav,
            temp.path(),
            hooks,
        )
        .await?;
        let result = WholeFileTranscription {
            text: diarized.text,
            segments: Vec::new(),
        };
        diarized_timeline_segments = Some(diarized.timeline_segments);
        diarized_speakers = Some(diarized.speakers);
        diarization_segments_for_manifest = Some(diarized.diarization_segments);
        (
            result,
            diarized.failed_chunks,
            diarized.memory_limit_hints,
            diarized.chunk_metrics,
            diarized.fallback_reason,
        )
    } else if duration_ms > CHUNK_DURATION_SECS * 1000 {
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
            hooks.server_state,
            hooks.managed_server_restart,
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
        let mut local_server_state;
        let server_state = if let Some(server_state) = hooks.server_state {
            server_state
        } else {
            local_server_state = hooks.server_url.map(|url| ServerRunnerState {
                server_url: url.to_string(),
                baseline_rtf: None,
                baseline_samples: Vec::new(),
                server_failures: 0,
                force_fork_for_remaining: hooks.startup_fallback_reason.is_some(),
                restart_required: false,
                current_chunk_failure_reason: None,
                fallback_reason: hooks.startup_fallback_reason.map(str::to_string),
            });
            &mut local_server_state
        };
        prepare_managed_server_for_chunk(
            task.runtime_strategy,
            server_state,
            hooks.managed_server_restart.as_mut(),
        )
        .await;
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
            server_state,
            hooks.managed_server_restart.as_mut(),
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
                .as_ref()
                .and_then(|state| state.fallback_reason.clone())
                .or_else(|| hooks.startup_fallback_reason.map(str::to_string)),
        )
    };

    let mut segments: Vec<TimelineSegment> =
        if let Some(diarized_segments) = diarized_timeline_segments.take() {
            diarized_segments
                .into_iter()
                .enumerate()
                .map(|(index, mut segment)| {
                    segment.index = index;
                    segment.absolute_start_ms = source_info
                        .source_created_at_ms
                        .map(|start| start.saturating_add(segment.audio_start_ms));
                    segment.absolute_end_ms = source_info
                        .source_created_at_ms
                        .map(|start| start.saturating_add(segment.audio_end_ms));
                    segment
                })
                .collect()
        } else {
            result
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
                        speaker: None,
                        speaker_display_name: None,
                        overlap: false,
                        text,
                    },
                )
                .collect()
        };

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
            speaker: None,
            speaker_display_name: None,
            overlap: false,
            text: text.clone(),
        });
    }
    let mut timeline = TranscriptTimeline {
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
        diarization_profile: None,
        speakers: Vec::new(),
        processed_at_ms: now_ms(),
        segments,
    };
    if let Some(diarization_segments) = diarization_segments_for_manifest.as_deref() {
        timeline.diarization_profile = Some(task.diarization.profile.clone());
        timeline.speakers = diarized_speakers.unwrap_or_default();
        write_diarization_manifest(
            task,
            &timeline,
            timeline.speakers.clone(),
            diarization_segments,
        )?;
    } else {
        apply_diarization_to_timeline(task, &mut timeline, wav)?;
    }
    let metadata = serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "source_path": path,
        "source_size": timeline.source_size,
        "source_modified_ms": timeline.source_modified_ms,
        "source_created_at_ms": timeline.source_created_at_ms,
        "source_created_at_source": timeline.source_created_at_source,
        "media_duration_ms": timeline.media_duration_ms,
        "model": task.model,
        "language": task.language,
        "runtime_strategy": task.runtime_strategy,
        "fallback_reason": result_fallback_reason.clone(),
        "chunk_metrics": result_chunk_metrics.clone(),
        "processed_at_ms": timeline.processed_at_ms,
    });
    let artifact_paths = write_offline_subtitle_artifacts(OfflineSubtitleArtifactRequest {
        data_dir: &bifrost_storage::data_dir(),
        task_id: &task.id,
        source_path: path,
        audio_dir: &task.audio_dir,
        timeline: &timeline,
        fallback_text: &text,
        metadata,
    })?;
    Ok(TranscriptionOutput {
        text,
        text_path: artifact_paths.text_path,
        metadata_path: artifact_paths.metadata_path,
        timeline_path: artifact_paths.timeline_path,
        timeline,
        failed_chunks,
        memory_limit_hints: result_memory_limit_hints,
        chunk_metrics: result_chunk_metrics,
        fallback_reason: result_fallback_reason,
    })
}

#[allow(clippy::too_many_arguments)]
async fn transcribe_diarized_segments_for_task(
    task: &AsrDirectoryTask,
    asr_bin: &Path,
    model_path: &Path,
    wav: &Path,
    temp_dir: &Path,
    mut hooks: TaskTranscribeHooks<'_>,
) -> Result<DiarizedTranscriptionOutput, String> {
    if !diarization_profile_ready(&task.diarization.profile) {
        return Err(format!(
            "diarization_missing_assets: profile '{}' is not initialized",
            task.diarization.profile
        ));
    }
    let diarization_segments =
        run_sherpa_diarization(&task.diarization, &task.diarization.profile, wav)?;
    if diarization_segments.is_empty() {
        return Err("diarization_no_segments: sherpa-onnx returned no speaker segments".to_string());
    }

    let mut local_server_state;
    let server_state = if let Some(server_state) = hooks.server_state {
        server_state
    } else {
        local_server_state = hooks.server_url.map(|url| ServerRunnerState {
            server_url: url.to_string(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: 0,
            force_fork_for_remaining: hooks.startup_fallback_reason.is_some(),
            restart_required: false,
            current_chunk_failure_reason: None,
            fallback_reason: hooks.startup_fallback_reason.map(str::to_string),
        });
        &mut local_server_state
    };
    prepare_managed_server_for_chunk(
        task.runtime_strategy,
        server_state,
        hooks.managed_server_restart.as_mut(),
    )
    .await;

    let speakers = speakers_from_diarization_segments(&diarization_segments);
    let asr_units = plan_asr_units(&diarization_segments, &AsrUnitPlannerConfig::default());
    if asr_units.is_empty() {
        return Err("diarization_no_asr_units: diarization produced no transcribable ASR units"
            .to_string());
    }
    let total_units = asr_units.len();
    let mut all_text = String::new();
    let mut timeline_segments = Vec::new();
    let mut chunk_metrics = Vec::new();
    let failed_chunks = Vec::new();
    let mut fallback_reason = server_state
        .as_ref()
        .and_then(|state| state.fallback_reason.clone())
        .or_else(|| hooks.startup_fallback_reason.map(str::to_string));

    for (index, asr_unit) in asr_units.iter().enumerate() {
        if hooks.pause_check.is_some_and(|check| check()) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        if let Some(callback) = hooks.on_chunk_progress {
            callback(index, total_units);
        }
        let segment_wav = temp_dir.join(format!("{}.wav", asr_unit.unit_id));
        ffmpeg_cut_wav_ms(
            wav,
            &segment_wav,
            asr_unit.source_start_ms,
            asr_unit.source_end_ms,
            hooks.pause_check,
        )
        .await?;
        if compute_wav_rms_energy(&segment_wav).is_some_and(|rms| rms < SILENCE_RMS_THRESHOLD) {
            continue;
        }
        let duration_ms = asr_unit
            .source_end_ms
            .saturating_sub(asr_unit.source_start_ms);
        let duration_secs = duration_ms.div_ceil(1000).max(1);
        let attempt = run_chunk_with_strategy(
            task.runtime_strategy,
            asr_bin,
            model_path,
            &task.language,
            &segment_wav,
            asr_unit.source_start_ms / 1000,
            duration_secs,
            index,
            temp_dir,
            hooks.force_pause_task_id,
            server_state,
            hooks.managed_server_restart.as_mut(),
            None,
        )
        .await?;
        if let Some(callback) = hooks.on_chunk_metric {
            callback(attempt.metric.clone());
            for metric in &attempt.shadow_metrics {
                callback(metric.clone());
            }
        }
        chunk_metrics.push(attempt.metric.clone());
        chunk_metrics.extend(attempt.shadow_metrics.clone());
        fallback_reason = server_state
            .as_ref()
            .and_then(|state| state.fallback_reason.clone())
            .or_else(|| fallback_reason.clone());

        let chunk_result = attempt.result?;
        append_diarized_segment_result(
            &mut all_text,
            &mut timeline_segments,
            asr_unit,
            chunk_result,
        );
    }

    if let Some(callback) = hooks.on_chunk_progress {
        callback(total_units, total_units);
    }

    Ok(DiarizedTranscriptionOutput {
        text: all_text,
        timeline_segments,
        speakers,
        diarization_segments,
        failed_chunks,
        memory_limit_hints: hooks.memory_limit_hints.to_vec(),
        chunk_metrics,
        fallback_reason,
    })
}

fn append_diarized_segment_result(
    all_text: &mut String,
    timeline_segments: &mut Vec<TimelineSegment>,
    asr_unit: &AsrAudioUnit,
    chunk_result: WholeFileTranscription,
) {
    let chunk_text = chunk_result.text.trim();
    if chunk_text.is_empty() && chunk_result.segments.is_empty() {
        return;
    }
    if !chunk_text.is_empty() {
        if !all_text.is_empty() {
            all_text.push('\n');
        }
        all_text.push_str(chunk_text);
    }
    if chunk_result.segments.is_empty() {
        timeline_segments.push(TimelineSegment {
            index: timeline_segments.len(),
            audio_start_ms: asr_unit.source_start_ms,
            audio_end_ms: asr_unit.source_end_ms,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: asr_unit.speaker.clone(),
            speaker_display_name: asr_unit.speaker_display_name.clone(),
            overlap: asr_unit.overlap,
            text: chunk_text.to_string(),
        });
        return;
    }
    for (local_start_ms, local_end_ms, text) in chunk_result.segments {
        if text.trim().is_empty() {
            continue;
        }
        let audio_start_ms = asr_unit.source_start_ms.saturating_add(local_start_ms);
        let audio_end_ms = asr_unit
            .source_start_ms
            .saturating_add(local_end_ms)
            .min(asr_unit.source_end_ms);
        if audio_end_ms <= audio_start_ms {
            continue;
        }
        timeline_segments.push(TimelineSegment {
            index: timeline_segments.len(),
            audio_start_ms,
            audio_end_ms,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: asr_unit.speaker.clone(),
            speaker_display_name: asr_unit.speaker_display_name.clone(),
            overlap: asr_unit.overlap,
            text,
        });
    }
}
