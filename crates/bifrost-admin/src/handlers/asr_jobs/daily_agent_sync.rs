// ─── Daily Agent Report Sync ─────────────────────────────────────────────────

const DAILY_AGENT_REPORT_SYNC_TIMEOUT: Duration = Duration::from_secs(8);
const DAILY_AGENT_ORIGINAL_SYNC_DIR_NAME: &str = "原始文件";

static DAILY_AGENT_REPORT_SYNC_SEMAPHORE: Lazy<std::sync::Arc<tokio::sync::Semaphore>> =
    Lazy::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)));

type DailyAgentAggregateSyncResult = (
    AsrDailyAgentReportSyncResult,
    Vec<(AsrDirectoryTask, AsrDailyAgentReportSyncResult)>,
    AsrDailyAgentReportSyncResult,
);

#[derive(Debug)]
enum DailyAgentReportSyncExecutionError {
    Busy,
    TimedOut,
    Join(String),
    Sync(String),
}

impl DailyAgentReportSyncExecutionError {
    fn message(&self) -> String {
        match self {
            Self::Busy => "Daily Agent report sync is already running".to_string(),
            Self::TimedOut => format!(
                "Daily Agent report sync exceeded {} seconds and will continue in the blocking worker",
                DAILY_AGENT_REPORT_SYNC_TIMEOUT.as_secs()
            ),
            Self::Join(error) => format!("Daily Agent report sync worker failed: {error}"),
            Self::Sync(error) => error.clone(),
        }
    }
}

fn expand_daily_agent_sync_dir(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

fn configured_daily_agent_report_sync_dir(task: &AsrDirectoryTask) -> Option<PathBuf> {
    task.daily_agent
        .report_sync_dir
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(expand_daily_agent_sync_dir)
}

fn daily_agent_report_sync_agent_dir(task: &AsrDirectoryTask) -> String {
    let agent_dir = normalize_daily_agent_token(&task.daily_agent.agent_id);
    if agent_dir.is_empty() {
        normalize_daily_agent_token(&task.daily_agent.output_dir)
    } else {
        agent_dir
    }
}

fn daily_agent_report_sync_target_path_unchecked(task: &AsrDirectoryTask) -> Option<PathBuf> {
    configured_daily_agent_report_sync_dir(task)
        .map(|root_dir| root_dir.join(daily_agent_report_sync_agent_dir(task)))
}

fn daily_agent_report_sync_target_dir(task: &AsrDirectoryTask) -> Result<PathBuf, String> {
    let root_dir = configured_daily_agent_report_sync_dir(task)
        .ok_or_else(|| "Daily Agent report sync directory is not configured".to_string())?;

    if root_dir.exists() && !root_dir.is_dir() {
        return Err(format!(
            "Daily Agent report sync target is not a directory: {}",
            root_dir.display()
        ));
    }

    Ok(root_dir.join(daily_agent_report_sync_agent_dir(task)))
}

fn daily_agent_original_sync_target_path_unchecked(task: &AsrDirectoryTask) -> Option<PathBuf> {
    configured_daily_agent_report_sync_dir(task)
        .map(|root_dir| root_dir.join(DAILY_AGENT_ORIGINAL_SYNC_DIR_NAME))
}

fn daily_agent_original_sync_target_dir(task: &AsrDirectoryTask) -> Result<PathBuf, String> {
    let root_dir = configured_daily_agent_report_sync_dir(task)
        .ok_or_else(|| "Daily Agent report sync directory is not configured".to_string())?;

    if root_dir.exists() && !root_dir.is_dir() {
        return Err(format!(
            "Daily Agent report sync target is not a directory: {}",
            root_dir.display()
        ));
    }

    Ok(root_dir.join(DAILY_AGENT_ORIGINAL_SYNC_DIR_NAME))
}

fn list_daily_agent_original_files(task: &AsrDirectoryTask) -> Vec<String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let Ok(entries) = std::fs::read_dir(daily_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_daily_source_markdown_path(path))
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    paths.sort();
    paths
}

fn sync_daily_agent_original_files(
    task: &AsrDirectoryTask,
    original_paths: &[String],
) -> Result<AsrDailyAgentReportSyncResult, String> {
    let target_dir = daily_agent_original_sync_target_dir(task)?;
    let mut valid_paths = Vec::new();
    let mut validation_errors = Vec::new();
    let mut skipped_files = 0;

    for original_path in original_paths {
        let source = PathBuf::from(original_path);
        if !is_daily_source_markdown_path(&source) {
            validation_errors.push(format!(
                "original transcript does not exist or is not a daily markdown file: {}",
                source.display()
            ));
            continue;
        }
        let target = target_dir.join(source.file_name().expect("validated daily source filename"));
        if target.is_file()
            && matches!(daily_agent_source_copy_is_current(&source, &target), Ok(true))
        {
            skipped_files += 1;
            continue;
        }
        valid_paths.push(original_path.clone());
    }

    // Reuse the report sync copier so both report and original transcript
    // files share the same atomic replace and unchanged-file skip behavior.
    let mut original_task = task.clone();
    original_task.daily_agent.report_sync_dir = Some(target_dir.to_string_lossy().to_string());
    original_task.daily_agent.agent_id.clear();
    original_task.daily_agent.output_dir.clear();
    let mut result = sync_daily_agent_report_files(&original_task, &valid_paths)?;
    result.target_dir = target_dir.to_string_lossy().to_string();
    result.total_files = original_paths.len();
    result.skipped_files += skipped_files;
    result.failed_files += validation_errors.len();
    result.errors.extend(validation_errors);

    tracing::info!(
        task_id = %task.id,
        target_dir = %result.target_dir,
        total_files = result.total_files,
        copied_files = result.copied_files,
        skipped_files = result.skipped_files,
        failed_files = result.failed_files,
        "synced ASR daily agent original transcripts"
    );

    Ok(result)
}

fn sync_daily_agent_report_files(
    task: &AsrDirectoryTask,
    report_paths: &[String],
) -> Result<AsrDailyAgentReportSyncResult, String> {
    let target_dir = daily_agent_report_sync_target_dir(task)?;

    std::fs::create_dir_all(&target_dir)
        .map_err(|error| format!("create report sync directory {}: {error}", target_dir.display()))?;

    let mut result = AsrDailyAgentReportSyncResult {
        target_dir: target_dir.to_string_lossy().to_string(),
        total_files: report_paths.len(),
        synced_at_ms: now_ms(),
        ..Default::default()
    };

    for report_path in report_paths {
        let source = PathBuf::from(report_path);
        let Some(file_name) = source.file_name() else {
            result.failed_files += 1;
            result.errors.push(format!("invalid report path: {report_path}"));
            continue;
        };
        if !source.is_file() {
            result.failed_files += 1;
            result
                .errors
                .push(format!("report does not exist: {}", source.display()));
            continue;
        }

        let target = target_dir.join(file_name);
        if source == target {
            result.skipped_files += 1;
            continue;
        }

        let temp_target = target_dir.join(format!(
            ".{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            now_ms()
        ));
        match std::fs::copy(&source, &temp_target).and_then(|_| {
            match std::fs::rename(&temp_target, &target) {
                Ok(()) => Ok(()),
                Err(error) if target.exists() => {
                    std::fs::remove_file(&target)?;
                    std::fs::rename(&temp_target, &target).map_err(|_| error)
                }
                Err(error) => Err(error),
            }
        }) {
            Ok(()) => {
                result.copied_files += 1;
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp_target);
                result.failed_files += 1;
                result.errors.push(format!(
                    "copy {} to {}: {error}",
                    source.display(),
                    target.display()
                ));
            }
        }
    }

    tracing::info!(
        task_id = %task.id,
        target_dir = %result.target_dir,
        total_files = result.total_files,
        copied_files = result.copied_files,
        skipped_files = result.skipped_files,
        failed_files = result.failed_files,
        "synced ASR daily agent reports"
    );

    Ok(result)
}

fn sync_all_daily_agent_reports_by_agent(
    task: &AsrDirectoryTask,
) -> Result<DailyAgentAggregateSyncResult, String> {
    let root_dir = configured_daily_agent_report_sync_dir(task)
        .ok_or_else(|| "Daily Agent report sync directory is not configured".to_string())?;

    if root_dir.exists() && !root_dir.is_dir() {
        return Err(format!(
            "Daily Agent report sync target is not a directory: {}",
            root_dir.display()
        ));
    }
    std::fs::create_dir_all(&root_dir)
        .map_err(|error| format!("create report sync directory {}: {error}", root_dir.display()))?;

    let reports = list_daily_agent_report_files(&task.id);
    let mut grouped_reports: BTreeMap<String, (AsrDirectoryTask, Vec<String>)> = BTreeMap::new();
    for report_path in reports {
        let output_dir = agent_output_dir_from_report_path(task, &report_path);
        let agent = agent_for_output_dir(task, &output_dir);
        let agent_task = task_for_daily_agent(task, &agent);
        grouped_reports
            .entry(agent.id.clone())
            .or_insert_with(|| (agent_task, Vec::new()))
            .1
            .push(report_path.to_string_lossy().to_string());
    }

    let mut aggregate = AsrDailyAgentReportSyncResult {
        target_dir: root_dir.to_string_lossy().to_string(),
        total_files: 0,
        synced_at_ms: now_ms(),
        ..Default::default()
    };
    let mut per_agent = Vec::new();

    let original_paths = list_daily_agent_original_files(task);
    let original_result = sync_daily_agent_original_files(task, &original_paths).unwrap_or_else(
        |error| failed_daily_agent_original_sync_result(task, original_paths.len(), error),
    );
    aggregate.total_files += original_result.total_files;
    aggregate.copied_files += original_result.copied_files;
    aggregate.skipped_files += original_result.skipped_files;
    aggregate.failed_files += original_result.failed_files;
    aggregate.errors.extend(original_result.errors.clone());

    for (_, (agent_task, agent_reports)) in grouped_reports {
        let result = sync_daily_agent_report_files(&agent_task, &agent_reports).unwrap_or_else(
            |error| failed_daily_agent_report_sync_result(&agent_task, agent_reports.len(), error),
        );
        aggregate.total_files += result.total_files;
        aggregate.copied_files += result.copied_files;
        aggregate.skipped_files += result.skipped_files;
        aggregate.failed_files += result.failed_files;
        aggregate.errors.extend(result.errors.clone());
        per_agent.push((agent_task, result));
    }

    Ok((aggregate, per_agent, original_result))
}

async fn sync_daily_agent_report_files_isolated(
    task: AsrDirectoryTask,
    report_paths: Vec<String>,
) -> Result<AsrDailyAgentReportSyncResult, DailyAgentReportSyncExecutionError> {
    let permit = DAILY_AGENT_REPORT_SYNC_SEMAPHORE
        .clone()
        .try_acquire_owned()
        .map_err(|_| DailyAgentReportSyncExecutionError::Busy)?;
    let join = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        sync_daily_agent_report_files(&task, &report_paths)
    });

    match tokio::time::timeout(DAILY_AGENT_REPORT_SYNC_TIMEOUT, join).await {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(DailyAgentReportSyncExecutionError::Sync(error)),
        Ok(Err(error)) => Err(DailyAgentReportSyncExecutionError::Join(error.to_string())),
        Err(_) => Err(DailyAgentReportSyncExecutionError::TimedOut),
    }
}

async fn sync_all_daily_agent_reports_by_agent_isolated(
    task: AsrDirectoryTask,
) -> Result<DailyAgentAggregateSyncResult, DailyAgentReportSyncExecutionError> {
    let permit = DAILY_AGENT_REPORT_SYNC_SEMAPHORE
        .clone()
        .try_acquire_owned()
        .map_err(|_| DailyAgentReportSyncExecutionError::Busy)?;
    let join = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        sync_all_daily_agent_reports_by_agent(&task)
    });

    match tokio::time::timeout(DAILY_AGENT_REPORT_SYNC_TIMEOUT, join).await {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(DailyAgentReportSyncExecutionError::Sync(error)),
        Ok(Err(error)) => Err(DailyAgentReportSyncExecutionError::Join(error.to_string())),
        Err(_) => Err(DailyAgentReportSyncExecutionError::TimedOut),
    }
}

async fn sync_daily_agent_original_files_isolated(
    task: AsrDirectoryTask,
    original_paths: Vec<String>,
) -> Result<AsrDailyAgentReportSyncResult, DailyAgentReportSyncExecutionError> {
    let permit = DAILY_AGENT_REPORT_SYNC_SEMAPHORE
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| DailyAgentReportSyncExecutionError::Join(error.to_string()))?;
    let join = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        sync_daily_agent_original_files(&task, &original_paths)
    });

    match tokio::time::timeout(DAILY_AGENT_REPORT_SYNC_TIMEOUT, join).await {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(DailyAgentReportSyncExecutionError::Sync(error)),
        Ok(Err(error)) => Err(DailyAgentReportSyncExecutionError::Join(error.to_string())),
        Err(_) => Err(DailyAgentReportSyncExecutionError::TimedOut),
    }
}

fn failed_daily_agent_report_sync_result(
    task: &AsrDirectoryTask,
    total_files: usize,
    error: String,
) -> AsrDailyAgentReportSyncResult {
    AsrDailyAgentReportSyncResult {
        target_dir: daily_agent_report_sync_target_path_unchecked(task)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default(),
        total_files,
        failed_files: total_files.max(1),
        synced_at_ms: now_ms(),
        errors: vec![error],
        ..Default::default()
    }
}

fn failed_daily_agent_original_sync_result(
    task: &AsrDirectoryTask,
    total_files: usize,
    error: String,
) -> AsrDailyAgentReportSyncResult {
    AsrDailyAgentReportSyncResult {
        target_dir: daily_agent_original_sync_target_path_unchecked(task)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default(),
        total_files,
        failed_files: total_files.max(1),
        synced_at_ms: now_ms(),
        errors: vec![error],
        ..Default::default()
    }
}

fn update_daily_agent_report_sync_status(
    source_task: &AsrDirectoryTask,
    result: AsrDailyAgentReportSyncResult,
) -> Result<(), String> {
    let task_id = source_task.id.as_str();
    let agent_id = source_task.daily_agent.agent_id.clone();
    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) else {
        return Err(format!("ASR task '{task_id}' not found"));
    };
    sync_daily_agent_item_status(&mut task.daily_agent, &agent_id, |agent| {
        agent.last_report_sync = Some(result.clone());
    });
    mirror_daily_agent_legacy_status(&mut task.daily_agent, &agent_id);
    task.updated_at_ms = now_ms();
    save_tasks(&store)
}

fn update_daily_agent_original_sync_status(
    source_task: &AsrDirectoryTask,
    result: AsrDailyAgentReportSyncResult,
) -> Result<(), String> {
    let task_id = source_task.id.as_str();
    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == task_id) else {
        return Err(format!("ASR task '{task_id}' not found"));
    };
    task.daily_agent.last_original_sync = Some(result);
    task.updated_at_ms = now_ms();
    save_tasks(&store)
}

async fn sync_daily_agent_original_files_after_refresh(task: &AsrDirectoryTask) {
    if configured_daily_agent_report_sync_dir(task).is_none() {
        return;
    }

    let original_paths = list_daily_agent_original_files(task);
    let result = match sync_daily_agent_original_files_isolated(task.clone(), original_paths.clone())
        .await
    {
        Ok(result) => result,
        Err(error) => failed_daily_agent_original_sync_result(task, original_paths.len(), error.message()),
    };
    if let Err(error) = update_daily_agent_original_sync_status(task, result.clone()) {
        tracing::warn!(
            task_id = %task.id,
            error = %error,
            "failed to persist ASR daily agent original transcript sync status"
        );
    }
    if result.failed_files > 0 {
        tracing::warn!(
            task_id = %task.id,
            failed_files = result.failed_files,
            errors = ?result.errors,
            "ASR daily agent original transcript sync failed after daily refresh"
        );
    }
}

fn spawn_daily_agent_original_files_after_refresh(task: &AsrDirectoryTask) {
    if configured_daily_agent_report_sync_dir(task).is_none() {
        return;
    }
    let task = task.clone();
    tokio::spawn(async move {
        sync_daily_agent_original_files_after_refresh(&task).await;
    });
}
