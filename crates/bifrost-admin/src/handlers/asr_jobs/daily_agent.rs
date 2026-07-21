// ─── Processed State ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrDailyAgentProcessedState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub documents: DailyAgentBTreeMap<String, AsrDailyAgentProcessedDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsrDailyAgentProcessedDocument {
    #[serde(default = "default_daily_agent_id")]
    pub agent_id: String,
    #[serde(default = "default_daily_agent_name")]
    pub agent_name: String,
    #[serde(default = "default_daily_agent_output_dir")]
    pub output_dir: String,
    pub date: String,
    pub source_sha256: String,
    pub source_len_bytes: u64,
    pub processed_at_ms: u64,
    pub runner: String,
    pub report_path: Option<String>,
    pub last_run_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct AsrDailyAgentReportIndexStatus {
    report_files: usize,
    processed_documents: usize,
    indexed_reports: usize,
    unindexed_reports: usize,
    processed_missing_report: usize,
    unindexed_dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrDailyAgentConversationState {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    updated_at_ms: Option<u64>,
}

// ─── Change Plan ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DailyAgentChangeKind {
    NewFile,
    Appended,
    Rewritten,
    Unchanged,
    Force,
}

#[derive(Debug, Clone, Serialize)]
struct DailyAgentChangePlanEntry {
    date: String,
    source_path: String,
    change_kind: DailyAgentChangeKind,
    source_sha256: String,
    source_len_bytes: u64,
    report_target: String,
    /// For appended: the byte offset where new content starts
    #[serde(skip_serializing_if = "Option::is_none")]
    append_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct AsrDailyAgentChangePlan {
    task_id: String,
    entries: Vec<DailyAgentChangePlanEntry>,
    skipped: bool,
    skip_reason: Option<String>,
}

// ─── Workspace Status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct AsrDailyWorkspaceStatus {
    daily_dir: String,
    report_dir: String,
    agents_path: String,
    agents_exists: bool,
    git_available: bool,
    git_initialized: bool,
    git_error: Option<String>,
    report_count: usize,
    agents: Vec<AsrDailyWorkspaceAgentStatus>,
}

#[derive(Debug, Clone, Serialize)]
struct AsrDailyWorkspaceAgentStatus {
    agent_id: String,
    name: String,
    output_dir: String,
    report_dir: String,
    instructions_path: String,
    instructions_exists: bool,
    report_count: usize,
}

// ─── Run Result ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct DailyAgentRunResult {
    agent_id: String,
    agent_name: String,
    run_id: String,
    status: String,
    trigger_source: String,
    started_at_ms: u64,
    finished_at_ms: u64,
    error: Option<String>,
    reports_generated: Vec<String>,
}

#[derive(Debug, Clone)]
struct DailyAgentEntryFailure {
    date: String,
    report_target: String,
    error: String,
}

#[derive(Debug, Clone, Default)]
struct DailyAgentInnerResult {
    reports_generated: Vec<String>,
    failed_entries: Vec<DailyAgentEntryFailure>,
}

// ─── Core Functions ───────────────────────────────────────────────────────────

fn daily_agent_processed_state_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("daily_agent_processed.json")
}

fn load_daily_agent_processed_state(task_id: &str) -> AsrDailyAgentProcessedState {
    let path = daily_agent_processed_state_path(task_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_daily_agent_processed_state(
    task_id: &str,
    state: &AsrDailyAgentProcessedState,
) -> Result<(), String> {
    let path = daily_agent_processed_state_path(task_id);
    atomic_json_write(&path, state)
}

fn daily_agent_conversation_state_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("daily_agent_conversation.json")
}

fn daily_agent_conversation_state_path_for_task(task: &AsrDirectoryTask) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(&task.id)
        .join(format!(
            "daily_agent_conversation_{}.json",
            task.daily_agent.agent_id
        ))
}

fn load_daily_agent_conversation_state(task_id: &str) -> AsrDailyAgentConversationState {
    let path = daily_agent_conversation_state_path(task_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn load_daily_agent_conversation_state_for_task(
    task: &AsrDirectoryTask,
) -> AsrDailyAgentConversationState {
    let path = daily_agent_conversation_state_path_for_task(task);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .or_else(|| {
            if task.daily_agent.agent_id == DEFAULT_DAILY_AGENT_ID {
                Some(load_daily_agent_conversation_state(&task.id))
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn save_daily_agent_conversation_state_for_task(
    task: &AsrDirectoryTask,
    state: &AsrDailyAgentConversationState,
) -> Result<(), String> {
    let path = daily_agent_conversation_state_path_for_task(task);
    atomic_json_write(&path, state)
}

fn daily_dir_for_task(task_id: &str) -> PathBuf {
    text_output_dir(&bifrost_storage::data_dir())
        .join(task_id)
        .join(".daily")
}

/// Validate that a date string matches YYYY-MM-DD format.
fn is_valid_date_format(date: &str) -> bool {
    if date.len() != 10 {
        return false;
    }
    let bytes = date.as_bytes();
    // Check format: DDDD-DD-DD where D is digit
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[0..4].iter().all(|b| b.is_ascii_digit())
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
}

fn compute_sha256(path: &Path) -> Result<String, String> {
    use sha2::{Digest as Sha2Digest, Sha256};
    let content = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let hash = Sha256::digest(&content);
    Ok(format!("{:x}", hash))
}

fn compute_sha256_of_bytes(data: &[u8]) -> String {
    use sha2::{Digest as Sha2Digest, Sha256};
    let hash = Sha256::digest(data);
    format!("{:x}", hash)
}

fn build_daily_agent_change_plan(
    task: &AsrDirectoryTask,
    _trigger_source: &str,
    requested_date: Option<&str>,
    force: bool,
) -> Result<AsrDailyAgentChangePlan, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let report_dir = daily_agent_output_dir(task);
    let processed = load_daily_agent_processed_state(&task.id);

    // Scan daily/*.md files (exclude AGENTS.md, hidden files, report/)
    let mut entries = Vec::new();
    let dir_entries =
        std::fs::read_dir(&daily_dir).map_err(|e| format!("read daily dir: {e}"))?;

    for entry in dir_entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Skip non-markdown, hidden files, AGENTS.md, directories
        if !filename.ends_with(".md")
            || filename.starts_with('.')
            || filename == "AGENTS.md"
            || path.is_dir()
        {
            continue;
        }

        // Extract date from filename (YYYY-MM-DD.md)
        let date = filename.trim_end_matches(".md");

        // Validate date format
        if !is_valid_date_format(date) {
            tracing::debug!(
                task_id = %task.id,
                filename,
                "skipped non-date markdown file"
            );
            continue;
        }

        // Filter by requested_date if specified
        if let Some(req_date) = requested_date {
            if date != req_date {
                continue;
            }
        }

        // Compute current file hash and size
        let source_sha256 = compute_sha256(&path)?;
        let source_len_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        let report_target = report_dir
            .join(format!("{}-report.md", date))
            .to_string_lossy()
            .to_string();

        // Determine change kind
        let (change_kind, append_offset) = if force {
            (DailyAgentChangeKind::Force, None)
        } else if let Some(prev) = processed
            .documents
            .get(&daily_agent_processed_key(task, date))
            .or_else(|| processed.documents.get(date))
        {
            if prev.source_sha256 == source_sha256 {
                (DailyAgentChangeKind::Unchanged, None)
            } else if source_len_bytes > prev.source_len_bytes {
                // Verify it's truly appended: read the first prev.source_len_bytes,
                // hash them, and compare with the previous hash.
                let is_appended = std::fs::read(&path)
                    .ok()
                    .map(|content| {
                        if content.len() as u64 >= prev.source_len_bytes {
                            let prefix = &content[..prev.source_len_bytes as usize];
                            let prefix_hash = compute_sha256_of_bytes(prefix);
                            prefix_hash == prev.source_sha256
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);

                if is_appended {
                    (DailyAgentChangeKind::Appended, Some(prev.source_len_bytes))
                } else {
                    (DailyAgentChangeKind::Rewritten, None)
                }
            } else {
                (DailyAgentChangeKind::Rewritten, None)
            }
        } else {
            (DailyAgentChangeKind::NewFile, None)
        };

        entries.push(DailyAgentChangePlanEntry {
            date: date.to_string(),
            source_path: path.to_string_lossy().to_string(),
            change_kind,
            source_sha256,
            source_len_bytes,
            report_target,
            append_offset,
        });
    }

    // Sort by date
    entries.sort_by(|a, b| a.date.cmp(&b.date));

    // Short-circuit: all unchanged and not force
    let all_unchanged = entries
        .iter()
        .all(|e| e.change_kind == DailyAgentChangeKind::Unchanged);

    if all_unchanged && !force && !entries.is_empty() {
        tracing::info!(
            task_id = %task.id,
            "skipped ASR daily agent run: no daily markdown changes"
        );
        return Ok(AsrDailyAgentChangePlan {
            task_id: task.id.clone(),
            entries,
            skipped: true,
            skip_reason: Some("no daily markdown changes".to_string()),
        });
    }

    if entries.is_empty() {
        return Ok(AsrDailyAgentChangePlan {
            task_id: task.id.clone(),
            entries,
            skipped: true,
            skip_reason: Some("no daily markdown files found".to_string()),
        });
    }

    let changed_count = entries
        .iter()
        .filter(|e| e.change_kind != DailyAgentChangeKind::Unchanged)
        .count();
    let unchanged_count = entries.len() - changed_count;

    tracing::info!(
        task_id = %task.id,
        changed = changed_count,
        unchanged = unchanged_count,
        "planned ASR daily agent changes"
    );

    Ok(AsrDailyAgentChangePlan {
        task_id: task.id.clone(),
        entries,
        skipped: false,
        skip_reason: None,
    })
}

async fn maybe_enqueue_daily_agent_after_asr_run(task: &AsrDirectoryTask) {
    if !task.daily_agent.enabled {
        return;
    }
    let agents = normalized_daily_agents(&task.daily_agent);
    let runnable_agents: Vec<_> = agents
        .into_iter()
        .filter(|agent| agent.enabled && agent.trigger_policy == AsrDailyAgentTriggerPolicy::AfterAsrRun)
        .collect();
    if runnable_agents.is_empty() {
        return;
    }
    let runnable_agents: Vec<_> = runnable_agents
        .into_iter()
        .filter(daily_agent_runner_ready_for_agent)
        .collect();
    if runnable_agents.is_empty() {
        return;
    }

    match daily_agent_has_changed_daily_markdown(task, &runnable_agents) {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!(
                task_id = %task.id,
                trigger_source = "asr_completion",
                "skipped daily agent: no daily markdown changes after ASR run"
            );
            return;
        }
        Err(error) => {
            tracing::warn!(
                task_id = %task.id,
                trigger_source = "asr_completion",
                error = %error,
                "skipped daily agent: failed to inspect daily markdown changes"
            );
            return;
        }
    }

    // Check if already running
    {
        let running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if running.contains(&task.id) {
            tracing::debug!(
                task_id = %task.id,
                "skipped daily agent: already running"
            );
            return;
        }
    }

    let task_id = task.id.clone();
    let task_clone = task.clone();

    // Spawn the daily agent run in background
    tokio::spawn(async move {
        run_daily_agents(&task_clone, "asr_completion", None, false).await;
    });

    tracing::info!(
        task_id = %task_id,
        trigger_source = "asr_completion",
        agents = runnable_agents.len(),
        "queued ASR daily agent run"
    );
}

fn daily_agent_has_changed_daily_markdown(
    task: &AsrDirectoryTask,
    agents: &[AsrDailyAgentItem],
) -> Result<bool, String> {
    for agent in agents {
        let agent_task = task_for_daily_agent(task, agent);
        let plan = build_daily_agent_change_plan(&agent_task, "asr_completion", None, false)?;
        if plan
            .entries
            .iter()
            .any(|entry| entry.change_kind != DailyAgentChangeKind::Unchanged)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn run_daily_agents(
    task: &AsrDirectoryTask,
    trigger_source: &str,
    requested_date: Option<&str>,
    force: bool,
) -> Vec<DailyAgentRunResult> {
    let agents = normalized_daily_agents(&task.daily_agent);
    let runnable_agents: Vec<_> = agents
        .into_iter()
        .filter(|agent| agent.enabled && daily_agent_runner_ready_for_agent(agent))
        .filter(|agent| {
            trigger_source == "manual"
                || agent.trigger_policy == AsrDailyAgentTriggerPolicy::AfterAsrRun
        })
        .collect();

    let task_id = task.id.clone();
    {
        let mut running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.insert(task_id.clone());
    }

    let task_lock = get_daily_agent_task_lock(&task_id);
    let _lock = task_lock.lock().await;

    let mut results = Vec::new();
    for agent in runnable_agents {
        let agent_task = task_for_daily_agent(task, &agent);
        results.push(
            run_daily_agent_locked(&agent_task, trigger_source, requested_date, force).await,
        );
    }

    {
        let mut running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.remove(&task_id);
    }

    results
}

fn daily_agent_effective_last_status(task: &AsrDirectoryTask) -> Option<String> {
    let agents = normalized_daily_agents(&task.daily_agent);
    let latest_agent = agents
        .iter()
        .filter(|agent| agent.last_status.is_some())
        .max_by_key(|agent| agent.last_run_at_ms.unwrap_or_default());
    let latest_status = latest_agent
        .and_then(|agent| agent.last_status.clone())
        .or_else(|| task.daily_agent.last_status.clone());

    if latest_status.as_deref() == Some("running") {
        let running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !running.contains(&task.id) {
            return Some("interrupted".to_string());
        }
    }
    latest_status
}

async fn run_daily_agent(
    task: &AsrDirectoryTask,
    trigger_source: &str,
    requested_date: Option<&str>,
    force: bool,
) -> DailyAgentRunResult {
    let task_id = task.id.clone();
    {
        let mut running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.insert(task_id.clone());
    }

    let task_lock = get_daily_agent_task_lock(&task_id);
    let _lock = task_lock.lock().await;
    let result = run_daily_agent_locked(task, trigger_source, requested_date, force).await;

    {
        let mut running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.remove(&task_id);
    }

    result
}

async fn run_daily_agent_locked(
    task: &AsrDirectoryTask,
    trigger_source: &str,
    requested_date: Option<&str>,
    force: bool,
) -> DailyAgentRunResult {
    let started_at_ms = now_ms();
    let run_id = format!("{}-{}", started_at_ms, uuid::Uuid::new_v4());
    let task_id = task.id.clone();
    let daily_dir = daily_dir_for_task(&task_id);

    tracing::info!(
        task_id = %task_id,
        agent_id = %task.daily_agent.agent_id,
        agent_name = %task.daily_agent.name,
        run_id = %run_id,
        trigger_source,
        runner = %task.daily_agent.runner,
        force,
        requested_date = ?requested_date,
        "starting ASR daily agent run"
    );

    let _ = update_daily_agent_status(task, "running", None, &run_id);

    let result = run_daily_agent_inner(
        task,
        trigger_source,
        requested_date,
        force,
        &run_id,
    )
    .await;

    // Determine success/failure
    let (status_str, error, reports_generated) = match &result {
        Ok(inner) => {
            let reports = inner.reports_generated.clone();
            let entry_error = daily_agent_entry_failure_summary(&inner.failed_entries);
            let status = if inner.failed_entries.is_empty() {
                "success"
            } else if reports.is_empty() {
                "failed"
            } else {
                "partial_success"
            };
            // Git commit after successful run
            let commit_msg = format!(
                "daily agent run {} ({}): {} report(s), {} failure(s)",
                &run_id[..8.min(run_id.len())],
                trigger_source,
                reports.len(),
                inner.failed_entries.len()
            );
            let _commit_hash = try_git_commit(&daily_dir, &commit_msg);
            (status.to_string(), entry_error, reports)
        }
        Err(e) => ("failed".to_string(), Some(e.clone()), Vec::new()),
    };

    // Persist daily agent state in task config
    let _ = update_daily_agent_status(task, &status_str, error.as_deref(), &run_id);

    // IM delivery based on send_policy
    let success = status_str == "success";
    let has_sendable_reports =
        daily_agent_run_has_sendable_reports(&status_str, &reports_generated);
    let should_send_im = task.daily_agent.im_delivery.enabled
        && daily_agent_im_channel(task).is_some()
        && match task.daily_agent.im_delivery.send_policy {
            AsrDailyAgentImSendPolicy::Always => true,
            AsrDailyAgentImSendPolicy::OnSuccess => success,
            AsrDailyAgentImSendPolicy::OnSuccessWithReport => has_sendable_reports,
        };

    if should_send_im {
        let im_content = if has_sendable_reports {
            build_im_content_for_reports(task, &reports_generated)
        } else {
            // For Always policy on failure, send error notification
            format!(
                "⚠️ ASR Daily Agent 运行失败\n\n任务: {}\n错误: {}",
                task.name,
                error.as_deref().unwrap_or("unknown error")
            )
        };

        if let Err(e) =
            send_daily_agent_im_message(task, &im_content, &run_id, reports_generated.len()).await
        {
            tracing::warn!(
                task_id = %task.id,
                error = %e,
                "ASR daily agent IM delivery failed"
            );
            let _ = update_daily_agent_im_error(task, &e);
        }
    }

    let finished_at_ms = now_ms();

    tracing::info!(
        task_id = %task_id,
        agent_id = %task.daily_agent.agent_id,
        run_id = %run_id,
        status = %status_str,
        reports = reports_generated.len(),
        error = error.as_deref(),
        duration_ms = finished_at_ms - started_at_ms,
        "ASR daily agent run completed"
    );

    DailyAgentRunResult {
        agent_id: task.daily_agent.agent_id.clone(),
        agent_name: task.daily_agent.name.clone(),
        run_id,
        status: status_str,
        trigger_source: trigger_source.to_string(),
        started_at_ms,
        finished_at_ms,
        error,
        reports_generated,
    }
}

async fn sync_daily_agent_reports_after_generation(
    task: &AsrDirectoryTask,
    reports_generated: &[String],
) -> Result<(), String> {
    if reports_generated.is_empty() || task.daily_agent.report_sync_dir.is_none() {
        return Ok(());
    }

    let sync_result =
        match sync_daily_agent_report_files_isolated(task.clone(), reports_generated.to_vec()).await
        {
            Ok(result) => result,
            Err(error) => failed_daily_agent_report_sync_result(
                task,
                reports_generated.len(),
                error.message(),
            ),
        };
    update_daily_agent_report_sync_status(task, sync_result.clone())?;
    if sync_result.failed_files > 0 {
        tracing::warn!(
            task_id = %task.id,
            failed_files = sync_result.failed_files,
            errors = ?sync_result.errors,
            "ASR daily agent report sync failed after report generation"
        );
    }

    Ok(())
}

/// Build IM content for reports. FullReport returns the report text; send-time
/// delivery handles chunking and preserves errors without summary fallback.
fn build_im_content_for_reports(task: &AsrDirectoryTask, reports: &[String]) -> String {
    match task.daily_agent.im_delivery.mode {
        AsrDailyAgentImDeliveryMode::FullReport => {
            // Read full report content (no length check — send as-is)
            let mut full_content = String::new();
            for report_path in reports {
                if let Ok(text) = std::fs::read_to_string(report_path) {
                    if !full_content.is_empty() {
                        full_content.push_str("\n---\n\n");
                    }
                    full_content.push_str(&text);
                }
            }
            if full_content.is_empty() {
                build_im_summary(task, reports)
            } else {
                full_content
            }
        }
        AsrDailyAgentImDeliveryMode::Summary => build_im_summary(task, reports),
    }
}

fn build_im_summary(task: &AsrDirectoryTask, reports: &[String]) -> String {
    build_im_summary_with_count(task, reports.len())
}

fn build_im_summary_with_count(task: &AsrDirectoryTask, report_count: usize) -> String {
    format!(
        "📋 ASR Daily Agent 完成报告整理\n\n任务: {}\n报告数: {}",
        task.name,
        report_count
    )
}

struct DailyAgentImChannel<'a> {
    provider_id: Option<&'a str>,
    target_id: &'a str,
}

fn daily_agent_im_channel(task: &AsrDirectoryTask) -> Option<DailyAgentImChannel<'_>> {
    daily_agent_im_channel_for_config(&task.daily_agent.im_delivery)
}

fn daily_agent_im_channel_for_config(
    config: &AsrDailyAgentImDeliveryConfig,
) -> Option<DailyAgentImChannel<'_>> {
    let channel = config.channel.as_deref()?.trim();
    if let Some(provider_id) = channel.strip_prefix("owner:") {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return None;
        }
        return Some(DailyAgentImChannel {
            provider_id: Some(provider_id),
            target_id: "owner",
        });
    }
    if let Some(target_id) = channel.strip_prefix("target:") {
        let target_id = target_id.trim();
        if target_id.is_empty() {
            return None;
        }
        return Some(DailyAgentImChannel {
            provider_id: None,
            target_id,
        });
    }
    None
}

#[cfg(test)]
fn daily_agent_runner_ready(task: &AsrDirectoryTask) -> bool {
    !task.daily_agent.runner.trim().is_empty()
}

fn daily_agent_runner_ready_for_agent(agent: &AsrDailyAgentItem) -> bool {
    !agent.runner.trim().is_empty()
}

fn daily_agent_external_runner_id(task: &AsrDirectoryTask) -> Option<&str> {
    let runner = task.daily_agent.runner.trim();
    (!runner.is_empty()).then_some(runner)
}

fn collect_report_outputs_for_plan_excluding_targets(
    plan: &AsrDailyAgentChangePlan,
    excluded_targets: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let mut reports_generated = Vec::new();
    let mut missing_reports = Vec::new();
    for entry in plan
        .entries
        .iter()
        .filter(|entry| entry.change_kind != DailyAgentChangeKind::Unchanged)
    {
        if excluded_targets.contains(&entry.report_target) {
            continue;
        }
        let report_path = PathBuf::from(&entry.report_target);
        if report_path.exists() {
            reports_generated.push(entry.report_target.clone());
        } else {
            missing_reports.push(entry.report_target.clone());
        }
    }
    (reports_generated, missing_reports)
}

fn daily_agent_entry_failure_summary(failures: &[DailyAgentEntryFailure]) -> Option<String> {
    if failures.is_empty() {
        return None;
    }
    let mut parts = failures
        .iter()
        .take(5)
        .map(|failure| {
            format!(
                "{} ({}): {}",
                failure.date, failure.report_target, failure.error
            )
        })
        .collect::<Vec<_>>();
    if failures.len() > parts.len() {
        parts.push(format!("... and {} more", failures.len() - parts.len()));
    }
    Some(format!(
        "{} daily agent entr{} failed: {}",
        failures.len(),
        if failures.len() == 1 { "y" } else { "ies" },
        parts.join("; ")
    ))
}

fn daily_agent_run_has_sendable_reports(status: &str, reports_generated: &[String]) -> bool {
    matches!(status, "success" | "partial_success") && !reports_generated.is_empty()
}

fn record_daily_agent_entry_failure(
    failed_entries: &mut Vec<DailyAgentEntryFailure>,
    task: &AsrDirectoryTask,
    entry: DailyAgentChangePlanEntry,
    error: impl Into<String>,
) {
    let error = error.into();
    tracing::warn!(
        task_id = %task.id,
        agent_id = %task.daily_agent.agent_id,
        date = %entry.date,
        report_target = %entry.report_target,
        error = %error,
        "ASR daily agent entry failed; continuing with remaining entries"
    );
    failed_entries.push(DailyAgentEntryFailure {
        date: entry.date,
        report_target: entry.report_target,
        error,
    });
}

fn single_entry_change_plan(
    plan: &AsrDailyAgentChangePlan,
    entry: DailyAgentChangePlanEntry,
) -> AsrDailyAgentChangePlan {
    AsrDailyAgentChangePlan {
        task_id: plan.task_id.clone(),
        entries: vec![entry],
        skipped: false,
        skip_reason: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatGptWebDailyAgentContract {
    DailyReport,
    TomorrowTodo,
    GenericMarkdown,
}

fn chatgpt_web_daily_agent_contract(
    agent_id: &str,
    output_dir: &str,
) -> ChatGptWebDailyAgentContract {
    if agent_id == DEFAULT_TOMORROW_TODO_AGENT_ID
        || output_dir == DEFAULT_TOMORROW_TODO_OUTPUT_DIR
    {
        ChatGptWebDailyAgentContract::TomorrowTodo
    } else if agent_id == DEFAULT_DAILY_AGENT_ID || output_dir == DEFAULT_DAILY_AGENT_OUTPUT_DIR {
        ChatGptWebDailyAgentContract::DailyReport
    } else {
        ChatGptWebDailyAgentContract::GenericMarkdown
    }
}

fn chatgpt_web_normalized_response(response: &str) -> &str {
    response
        .trim()
        .trim_start_matches("ChatGPT 说")
        .trim_start_matches([':', '：'])
        .trim()
}

fn chatgpt_web_response_is_placeholder(normalized: &str) -> bool {
    normalized.starts_with("用户的消息为空")
        || normalized.contains("上传的文件包含")
        || normalized == "正在思考"
        || normalized == "正在打草稿"
}

fn validate_chatgpt_web_daily_report_response(response: &str, date: &str) -> Result<(), String> {
    let trimmed = response.trim();
    if trimmed.len() < 512 {
        return Err(format!(
            "chatgpt_web daily report response too short for {date}: {} bytes",
            trimmed.len()
        ));
    }
    if !trimmed.contains(date) {
        return Err(format!(
            "chatgpt_web daily report response missing target date {date}"
        ));
    }
    let normalized = chatgpt_web_normalized_response(trimmed);
    let expected_heading = format!("# {date} 日报");
    if !normalized.starts_with(&expected_heading) {
        return Err(format!(
            "chatgpt_web daily report response missing leading report heading {expected_heading}"
        ));
    }
    if !normalized.contains("今日概览") || !normalized.contains("证据与不确定性") {
        return Err(format!(
            "chatgpt_web daily report response missing required report sections for {date}"
        ));
    }
    if chatgpt_web_response_is_placeholder(normalized) {
        return Err(format!(
            "chatgpt_web daily report response is a status/error placeholder for {date}"
        ));
    }
    Ok(())
}

fn tomorrow_todo_target_date(source_date: &str) -> String {
    NaiveDate::parse_from_str(source_date, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.checked_add_signed(ChronoDuration::days(1)))
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| source_date.to_string())
}

fn validate_chatgpt_web_tomorrow_todo_response(
    response: &str,
    source_date: &str,
) -> Result<(), String> {
    let target_date = tomorrow_todo_target_date(source_date);
    let trimmed = response.trim();
    if trimmed.len() < 128 {
        return Err(format!(
            "chatgpt_web tomorrow_todo response too short for {source_date}: {} bytes",
            trimmed.len()
        ));
    }
    if !trimmed.contains(&target_date) {
        return Err(format!(
            "chatgpt_web tomorrow_todo response missing target date {target_date} for source {source_date}"
        ));
    }
    let normalized = chatgpt_web_normalized_response(trimmed);
    let expected_heading = format!("# 明日 To Do List - {target_date}");
    if !normalized.starts_with(&expected_heading) {
        return Err(format!(
            "chatgpt_web tomorrow_todo response missing leading todo heading {expected_heading}"
        ));
    }
    if normalized.starts_with(&format!("# {source_date} 日报"))
        || normalized.starts_with(&format!("# {target_date} 日报"))
    {
        return Err(format!(
            "chatgpt_web tomorrow_todo response was a daily report for source {source_date}"
        ));
    }
    for section in ["明天必须完成", "可选推进", "需要确认"] {
        if !normalized.contains(section) {
            return Err(format!(
                "chatgpt_web tomorrow_todo response missing required section {section} for source {source_date}"
            ));
        }
    }
    if chatgpt_web_response_is_placeholder(normalized) {
        return Err(format!(
            "chatgpt_web tomorrow_todo response is a status/error placeholder for source {source_date}"
        ));
    }
    Ok(())
}

fn validate_chatgpt_web_generic_markdown_response(
    response: &str,
    date: &str,
    agent_id: &str,
) -> Result<(), String> {
    let trimmed = response.trim();
    if trimmed.len() < 64 {
        return Err(format!(
            "chatgpt_web {agent_id} response too short for {date}: {} bytes",
            trimmed.len()
        ));
    }
    let normalized = chatgpt_web_normalized_response(trimmed);
    if !trimmed.contains(date) && !normalized.starts_with("# ") {
        return Err(format!(
            "chatgpt_web {agent_id} response missing target date {date} or markdown heading"
        ));
    }
    if chatgpt_web_response_is_placeholder(normalized) {
        return Err(format!(
            "chatgpt_web {agent_id} response is a status/error placeholder for {date}"
        ));
    }
    Ok(())
}

fn validate_chatgpt_web_daily_agent_response(
    response: &str,
    date: &str,
    agent_id: &str,
    output_dir: &str,
) -> Result<(), String> {
    match chatgpt_web_daily_agent_contract(agent_id, output_dir) {
        ChatGptWebDailyAgentContract::DailyReport => {
            validate_chatgpt_web_daily_report_response(response, date)
        }
        ChatGptWebDailyAgentContract::TomorrowTodo => {
            validate_chatgpt_web_tomorrow_todo_response(response, date)
        }
        ChatGptWebDailyAgentContract::GenericMarkdown => {
            validate_chatgpt_web_generic_markdown_response(response, date, agent_id)
        }
    }
}

fn chatgpt_web_daily_agent_retry_prompt(
    date: &str,
    contract: ChatGptWebDailyAgentContract,
) -> String {
    match contract {
        ChatGptWebDailyAgentContract::DailyReport => format!(
            "上一条回复不是最终日报。请不要说明计划，不要总结你将要做什么，立即根据刚刚上传或粘贴的完整 Markdown 内容，直接输出完整的 {date} 日报正文。必须包含 `# {date} 日报`、`## 今日概览` 和 `## 证据与不确定性`，不要使用代码块包装."
        ),
        ChatGptWebDailyAgentContract::TomorrowTodo => format!(
            "上一条回复不是最终明日待办。请不要说明计划，不要总结你将要做什么，立即根据刚刚上传或粘贴的完整 Markdown 内容，直接输出完整的 {} 明日 To Do List。必须包含 `# 明日 To Do List - {}`、`## 明天必须完成`、`## 可选推进` 和 `## 需要确认`，不要使用代码块包装.",
            tomorrow_todo_target_date(date),
            tomorrow_todo_target_date(date),
        ),
        ChatGptWebDailyAgentContract::GenericMarkdown => format!(
            "上一条回复不是最终 Markdown 输出。请不要说明计划，不要总结你将要做什么，立即根据刚刚上传或粘贴的完整 Markdown 内容，直接输出 {date} 对应的最终正文。输出必须是 Markdown 正文，不要使用代码块包装."
        ),
    }
}

fn chatgpt_web_daily_agent_continuation_prompt(
    date: &str,
    contract: ChatGptWebDailyAgentContract,
    tail: &str,
) -> String {
    match contract {
        ChatGptWebDailyAgentContract::DailyReport => format!(
            "上一条 {date} 日报正文在中途截断了。请从下面这段末尾之后继续写，不要重复前文，不要使用代码块；继续补齐剩余章节，最后必须包含 `## 证据与不确定性`。\n\n上一条末尾：\n{tail}"
        ),
        ChatGptWebDailyAgentContract::TomorrowTodo => format!(
            "上一条 {} 明日 To Do List 在中途截断了。请从下面这段末尾之后继续写，不要重复前文，不要使用代码块；继续补齐剩余章节，最后必须包含 `## 需要确认`。\n\n上一条末尾：\n{tail}",
            tomorrow_todo_target_date(date),
        ),
        ChatGptWebDailyAgentContract::GenericMarkdown => format!(
            "上一条 {date} Markdown 输出在中途截断了。请从下面这段末尾之后继续写，不要重复前文，不要使用代码块；继续补齐剩余正文。\n\n上一条末尾：\n{tail}"
        ),
    }
}

#[cfg(test)]
fn merge_chatgpt_web_daily_report_continuation(
    base: &str,
    continuation: &str,
    date: &str,
) -> String {
    let continuation = continuation.trim();
    if validate_chatgpt_web_daily_report_response(continuation, date).is_ok() {
        continuation.to_string()
    } else {
        format!("{}\n{}", base.trim_end(), continuation.trim_start())
    }
}

fn merge_chatgpt_web_daily_agent_continuation(
    base: &str,
    continuation: &str,
    date: &str,
    agent_id: &str,
    output_dir: &str,
) -> String {
    let continuation = continuation.trim();
    if validate_chatgpt_web_daily_agent_response(continuation, date, agent_id, output_dir).is_ok()
    {
        continuation.to_string()
    } else {
        format!("{}\n{}", base.trim_end(), continuation.trim_start())
    }
}

fn chatgpt_web_daily_report_tail(response: &str, max_chars: usize) -> String {
    let chars: Vec<char> = response.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

fn metadata_value(
    metadata: &DailyAgentBTreeMap<String, String>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| metadata.get(*key))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn persist_daily_agent_conversation_success(
    task: &AsrDirectoryTask,
    adapter: &str,
    session_key: &str,
    metadata: Option<&DailyAgentBTreeMap<String, String>>,
) -> Result<(), String> {
    let mut state = load_daily_agent_conversation_state_for_task(task);
    state.version = CONVERSATION_STATE_VERSION;
    state.adapter = Some(adapter.to_string());
    state.session_key = Some(session_key.to_string());
    state.initialized = true;
    state.updated_at_ms = Some(now_ms());
    if adapter == "chatgpt_web" {
        state.conversation_id = None;
    } else if let Some(metadata) = metadata {
        if let Some(conversation_id) =
            metadata_value(metadata, &["conversationId", "conversation_id"])
        {
            state.conversation_id = Some(conversation_id);
        }
        if let Some(thread_id) = metadata_value(metadata, &["threadId", "thread_id"]) {
            state.thread_id = Some(thread_id);
        }
    }
    save_daily_agent_conversation_state_for_task(task, &state)
}


async fn run_external_daily_agent_prompt(
    task: &AsrDirectoryTask,
    runner_id: &str,
    prompt: String,
    daily_dir: &Path,
    session_key: &str,
    conversation_state: &AsrDailyAgentConversationState,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
) -> Result<crate::im_gateway::external_cli::ExternalCliRunResult, String> {
    let params = daily_agent_external_runner_params(
        effective.settings.adapter.as_str(),
        conversation_state,
    );
    let request_session_key = if effective.settings.adapter == "chatgpt_web" {
        crate::im_gateway::chatgpt_web::clear_session_conversation(session_key).await;
        None
    } else {
        Some(session_key.to_string())
    };
    run_external_daily_agent_prompt_with_params(
        task,
        runner_id,
        prompt,
        daily_dir,
        request_session_key,
        params,
        effective,
    )
    .await
}

async fn run_external_daily_agent_prompt_with_params(
    task: &AsrDirectoryTask,
    runner_id: &str,
    prompt: String,
    daily_dir: &Path,
    request_session_key: Option<String>,
    params: serde_json::Value,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
) -> Result<crate::im_gateway::external_cli::ExternalCliRunResult, String> {
    let operation = match effective.settings.adapter.as_str() {
        "codex" => "run".to_string(),
        _ => "send".to_string(),
    };
    let adapter_config = daily_agent_external_runner_adapter_config(
        effective.settings.adapter.as_str(),
        &effective.settings.adapter_config,
        task.daily_agent.timeout_ms,
    );

    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        message: prompt,
        operation,
        params,
        provider_id: None,
        runner_id: Some(runner_id.to_string()),
        session_key: request_session_key,
        runtime: "external_cli".to_string(),
        adapter: effective.settings.adapter.clone(),
        work_dir: Some(daily_dir.to_path_buf()),
        instructions: None,
        adapter_config,
        allow_work_dirs: vec![
            daily_dir.to_string_lossy().to_string(),
            daily_dir_for_task(&task.id).to_string_lossy().to_string(),
        ],
        inject_bifrost_tools: effective.settings.inject_bifrost_tools,
        skill_paths: effective.settings.skill_paths.clone(),
    };

    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
        bifrost_storage::data_dir().join("im_gateway/runs"),
    );

    let timeout_duration = Duration::from_millis(task.daily_agent.timeout_ms);
    let run_result = tokio::time::timeout(timeout_duration, runtime.run(request))
        .await
        .map_err(|_| {
            format!(
                "daily agent run timed out after {}ms",
                task.daily_agent.timeout_ms
            )
        })?
        .map_err(|e| format!("external CLI run failed: {e}"))?;

    if run_result.status != crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded {
        return Err(format!(
            "external CLI run failed with status {:?}: {}",
            run_result.status, run_result.response
        ));
    }

    Ok(run_result)
}

async fn wait_chatgpt_web_daily_agent_conversation(
    task: &AsrDirectoryTask,
    runner_id: &str,
    daily_dir: &Path,
    conversation_id: &str,
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
) -> Result<crate::im_gateway::external_cli::ExternalCliRunResult, String> {
    let adapter_config = daily_agent_external_runner_adapter_config(
        effective.settings.adapter.as_str(),
        &effective.settings.adapter_config,
        daily_agent_chatgpt_web_same_conversation_wait_timeout_ms(task.daily_agent.timeout_ms),
    );
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        message: String::new(),
        operation: "wait".to_string(),
        params: serde_json::json!({ "conversationId": conversation_id }),
        provider_id: None,
        runner_id: Some(runner_id.to_string()),
        session_key: None,
        runtime: "external_cli".to_string(),
        adapter: effective.settings.adapter.clone(),
        work_dir: Some(daily_dir.to_path_buf()),
        instructions: None,
        adapter_config,
        allow_work_dirs: vec![
            daily_dir.to_string_lossy().to_string(),
            daily_dir_for_task(&task.id).to_string_lossy().to_string(),
        ],
        inject_bifrost_tools: effective.settings.inject_bifrost_tools,
        skill_paths: effective.settings.skill_paths.clone(),
    };

    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
        bifrost_storage::data_dir().join("im_gateway/runs"),
    );
    let wait_timeout_ms =
        daily_agent_chatgpt_web_same_conversation_wait_timeout_ms(task.daily_agent.timeout_ms);
    let timeout_duration = Duration::from_millis(wait_timeout_ms);
    let run_result = tokio::time::timeout(timeout_duration, runtime.run(request))
        .await
        .map_err(|_| {
            format!(
                "daily agent wait timed out after {}ms",
                wait_timeout_ms
            )
        })?
        .map_err(|e| format!("external CLI wait failed: {e}"))?;

    if run_result.status != crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded {
        return Err(format!(
            "external CLI wait failed with status {:?}: {}",
            run_result.status, run_result.response
        ));
    }

    Ok(run_result)
}

fn daily_agent_external_runner_adapter_config(
    adapter: &str,
    config: &crate::im_gateway::external_cli::ExternalCliAdapterConfig,
    daily_timeout_ms: u64,
) -> crate::im_gateway::external_cli::ExternalCliAdapterConfig {
    let mut config = config.clone();
    if adapter != "chatgpt_web" {
        return config;
    }
    let inner_timeout_secs = daily_agent_chatgpt_web_inner_timeout_secs(daily_timeout_ms);
    config.timeout_secs = Some(inner_timeout_secs);
    config
}

fn daily_agent_chatgpt_web_inner_timeout_secs(daily_timeout_ms: u64) -> u64 {
    const MIN_TIMEOUT_SECS: u64 = 1;
    const OUTER_TIMEOUT_HEADROOM_SECS: u64 = 30;
    let daily_timeout_secs = daily_timeout_ms / 1000;
    daily_timeout_secs
        .saturating_sub(OUTER_TIMEOUT_HEADROOM_SECS)
        .max(MIN_TIMEOUT_SECS)
}

fn daily_agent_chatgpt_web_same_conversation_wait_timeout_ms(daily_timeout_ms: u64) -> u64 {
    const SAME_CONVERSATION_WAIT_MIN_MS: u64 = 5_000;
    const OUTER_TIMEOUT_HEADROOM_MS: u64 = 30_000;
    if daily_timeout_ms <= SAME_CONVERSATION_WAIT_MIN_MS {
        return SAME_CONVERSATION_WAIT_MIN_MS;
    }
    if daily_timeout_ms <= OUTER_TIMEOUT_HEADROOM_MS * 2 {
        return daily_timeout_ms;
    }
    daily_timeout_ms
        .saturating_sub(OUTER_TIMEOUT_HEADROOM_MS)
        .clamp(SAME_CONVERSATION_WAIT_MIN_MS, daily_timeout_ms)
}

fn daily_agent_external_runner_params(
    adapter: &str,
    conversation_state: &AsrDailyAgentConversationState,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if adapter == "codex" {
        if let Some(thread_id) = conversation_state.thread_id.as_deref() {
            params.insert(
                "threadId".to_string(),
                serde_json::Value::String(thread_id.to_string()),
            );
        }
    }
    if params.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(params)
    }
}

fn apply_external_daily_agent_metadata(
    state: &mut AsrDailyAgentConversationState,
    metadata: &DailyAgentBTreeMap<String, String>,
) {
    if let Some(conversation_id) = metadata_value(metadata, &["conversationId", "conversation_id"])
    {
        state.conversation_id = Some(conversation_id);
    }
    if let Some(thread_id) = metadata_value(metadata, &["threadId", "thread_id"]) {
        state.thread_id = Some(thread_id);
    }
    state.initialized = true;
}

async fn run_daily_agent_inner(
    task: &AsrDirectoryTask,
    trigger_source: &str,
    requested_date: Option<&str>,
    force: bool,
    run_id: &str,
) -> Result<DailyAgentInnerResult, String> {
    // 1. Ensure workspace
    ensure_asr_daily_workspace(task)?;

    // 2. Build change plan
    let plan = build_daily_agent_change_plan(task, trigger_source, requested_date, force)?;
    sync_daily_agent_plan_sources_to_work_dir(task, &plan)?;

    if plan.skipped {
        tracing::info!(
            task_id = %task.id,
            trigger_source,
            reason = plan.skip_reason.as_deref().unwrap_or("unknown"),
            "skipped ASR daily agent run"
        );
        return Ok(DailyAgentInnerResult::default());
    }

    // 3. Determine adapter for prompt construction
    let adapter = if let Some(runner_id) = daily_agent_external_runner_id(task) {
        let config_store =
            crate::im_gateway::external_cli::ExternalCliConfigStore::new(&bifrost_storage::data_dir());
        let config = config_store.load();
        let effective =
            crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
                &config,
                None,
                Some(runner_id),
            );
        effective.settings.adapter.clone()
    } else {
        String::new()
    };

    // 4. Prepare conversation context
    let agent_work_dir = daily_agent_work_dir(task);
    let session_key = task
        .daily_agent
        .session_key
        .clone()
        .unwrap_or_else(|| format!("asr-daily:{}:{}", task.id, task.daily_agent.agent_id));
    let mut conversation_state = load_daily_agent_conversation_state_for_task(task);

    // 5. Dispatch to runner
    let conversation_success: Option<(String, Option<DailyAgentBTreeMap<String, String>>)>;
    let mut failed_entries = Vec::new();

    if let Some(runner_id) = daily_agent_external_runner_id(task) {
            let runner_id = runner_id.to_string();
            // Read runner config to get adapter and other settings
            let config_store = crate::im_gateway::external_cli::ExternalCliConfigStore::new(
                &bifrost_storage::data_dir(),
            );
            let config = config_store.load();
            let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
                &config,
                None,
                Some(&runner_id),
            );

            let mut last_metadata: Option<DailyAgentBTreeMap<String, String>> = None;

            if adapter == "chatgpt_web" {
                let agent_id = task.daily_agent.agent_id.as_str();
                let output_dir = task.daily_agent.output_dir.as_str();
                let response_contract =
                    chatgpt_web_daily_agent_contract(agent_id, output_dir);
                let changed_entries: Vec<_> = plan
                    .entries
                    .iter()
                    .filter(|e| e.change_kind != DailyAgentChangeKind::Unchanged)
                    .cloned()
                    .collect();
                'entry_loop: for entry in changed_entries {
                    macro_rules! fail_entry {
                        ($error:expr) => {{
                            record_daily_agent_entry_failure(
                                &mut failed_entries,
                                task,
                                entry,
                                $error,
                            );
                            continue 'entry_loop;
                        }};
                    }
                    let entry_plan = single_entry_change_plan(&plan, entry.clone());
                    let prompt = match build_daily_agent_prompt(task, &entry_plan, &adapter, true)
                    {
                        Ok(prompt) => prompt,
                        Err(error) => fail_entry!(error),
                    };
                    let run_result = match run_external_daily_agent_prompt(
                        task,
                        &runner_id,
                        prompt,
                        &agent_work_dir,
                        &session_key,
                        &conversation_state,
                        &effective,
                    )
                    .await
                    {
                        Ok(run_result) => run_result,
                        Err(error) => fail_entry!(error),
                    };
                    let mut entry_last_metadata = Some(run_result.metadata.clone());
                    let response = run_result.response.trim();
                    if response.is_empty() {
                        fail_entry!("chatgpt_web daily agent returned an empty response");
                    }
                    let response = match validate_chatgpt_web_daily_agent_response(
                        response,
                        &entry.date,
                        agent_id,
                        output_dir,
                    ) {
                        Ok(()) => response.to_string(),
                        Err(first_error) => {
                            let Some(conversation_id) =
                                metadata_value(&run_result.metadata, &["conversationId", "conversation_id"])
                            else {
                                fail_entry!(first_error);
                            };
                            tracing::warn!(
                                date = %entry.date,
                                agent_id,
                                conversation_id = %conversation_id,
                                error = %first_error,
                                "chatgpt_web daily agent response failed validation; waiting for same conversation before retry"
                            );
                            let mut same_conversation_response = None;
                            let waited_result = wait_chatgpt_web_daily_agent_conversation(
                                task,
                                &runner_id,
                                &agent_work_dir,
                                &conversation_id,
                                &effective,
                            )
                            .await;
                            if let Ok(waited_result) = waited_result {
                                entry_last_metadata = Some(waited_result.metadata.clone());
                                let waited_response = waited_result.response.trim().to_string();
                                if validate_chatgpt_web_daily_agent_response(
                                    &waited_response,
                                    &entry.date,
                                    agent_id,
                                    output_dir,
                                )
                                .is_ok()
                                {
                                    tracing::info!(
                                        date = %entry.date,
                                        agent_id,
                                        conversation_id = %conversation_id,
                                        "chatgpt_web daily agent same-conversation wait produced valid final response"
                                    );
                                    same_conversation_response = Some(waited_response);
                                } else {
                                    tracing::warn!(
                                        date = %entry.date,
                                        agent_id,
                                        conversation_id = %conversation_id,
                                        waited_len = waited_response.len(),
                                        "chatgpt_web daily agent same-conversation wait still failed validation; retrying with explicit final-output instruction"
                                    );
                                }
                            } else if let Err(wait_error) = waited_result {
                                tracing::warn!(
                                    date = %entry.date,
                                    agent_id,
                                    conversation_id = %conversation_id,
                                    error = %wait_error,
                                    "chatgpt_web daily agent same-conversation wait failed; retrying with explicit final-output instruction"
                                );
                            }
                            if let Some(valid_response) = same_conversation_response {
                                valid_response
                            } else {
                            tracing::warn!(
                                date = %entry.date,
                                agent_id,
                                conversation_id = %conversation_id,
                                error = %first_error,
                                "chatgpt_web daily agent response failed validation; retrying with explicit final-output instruction"
                            );
                            let retry_prompt = chatgpt_web_daily_agent_retry_prompt(
                                &entry.date,
                                response_contract,
                            );
                            let retry_result = match run_external_daily_agent_prompt_with_params(
                                task,
                                &runner_id,
                                retry_prompt,
                                &agent_work_dir,
                                None,
                                serde_json::json!({ "conversationId": conversation_id }),
                                &effective,
                            )
                            .await
                            {
                                Ok(retry_result) => retry_result,
                                Err(error) => fail_entry!(error),
                            };
                            entry_last_metadata = Some(retry_result.metadata.clone());
                            let mut retry_response = retry_result.response.trim().to_string();
                            if validate_chatgpt_web_daily_agent_response(
                                &retry_response,
                                &entry.date,
                                agent_id,
                                output_dir,
                            )
                            .is_err()
                            {
                                let retry_conversation_id = metadata_value(
                                    &retry_result.metadata,
                                    &["conversationId", "conversation_id"],
                                )
                                .unwrap_or_else(|| conversation_id.clone());
                                for attempt in 1..=3 {
                                    let tail = chatgpt_web_daily_report_tail(&retry_response, 1200);
                                    tracing::warn!(
                                        date = %entry.date,
                                        agent_id,
                                        conversation_id = %retry_conversation_id,
                                        attempt,
                                        "chatgpt_web daily agent response appears truncated; requesting continuation"
                                    );
                                    let continuation_prompt =
                                        chatgpt_web_daily_agent_continuation_prompt(
                                            &entry.date,
                                            response_contract,
                                            &tail,
                                        );
                                    let continuation_result =
                                        match run_external_daily_agent_prompt_with_params(
                                            task,
                                            &runner_id,
                                            continuation_prompt,
                                            &agent_work_dir,
                                            None,
                                            serde_json::json!({
                                                "conversationId": retry_conversation_id
                                            }),
                                            &effective,
                                        )
                                        .await
                                        {
                                            Ok(continuation_result) => continuation_result,
                                            Err(error) => fail_entry!(error),
                                        };
                                    entry_last_metadata =
                                        Some(continuation_result.metadata.clone());
                                    retry_response = merge_chatgpt_web_daily_agent_continuation(
                                        &retry_response,
                                        &continuation_result.response,
                                        &entry.date,
                                        agent_id,
                                        output_dir,
                                    );
                                    if validate_chatgpt_web_daily_agent_response(
                                        &retry_response,
                                        &entry.date,
                                        agent_id,
                                        output_dir,
                                    )
                                    .is_ok()
                                    {
                                        break;
                                    }
                                }
                            }
                            if let Err(error) = validate_chatgpt_web_daily_agent_response(
                                &retry_response,
                                &entry.date,
                                agent_id,
                                output_dir,
                            ) {
                                fail_entry!(error);
                            }
                            retry_response
                            }
                        }
                    };
                    let report_path = PathBuf::from(&entry.report_target);
                    if let Some(parent) = report_path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    if let Err(e) = std::fs::write(&report_path, &response) {
                        fail_entry!(format!(
                            "failed to save chatgpt_web response as report {}: {e}",
                            report_path.display()
                        ));
                    } else {
                        tracing::info!(
                            report_path = %report_path.display(),
                            len = response.len(),
                            "saved chatgpt_web response as report"
                        );
                    }
                    last_metadata = entry_last_metadata;
                }
            } else {
                let prompt = build_daily_agent_prompt(task, &plan, &adapter, false)?;
                let run_result = run_external_daily_agent_prompt(
                    task,
                    &runner_id,
                    prompt,
                    &agent_work_dir,
                    &session_key,
                    &conversation_state,
                    &effective,
                )
                .await?;
                apply_external_daily_agent_metadata(&mut conversation_state, &run_result.metadata);
                last_metadata = Some(run_result.metadata.clone());
            }
            conversation_success = Some((effective.settings.adapter.clone(), last_metadata));
        } else {
            return Err("daily agent requires an external runner".to_string());
        }

    // 6. Validate reports before marking source documents processed.
    let failed_report_targets = failed_entries
        .iter()
        .map(|failure| failure.report_target.clone())
        .collect::<HashSet<_>>();
    let (reports_generated, missing_reports) =
        collect_report_outputs_for_plan_excluding_targets(&plan, &failed_report_targets);
    if !missing_reports.is_empty() {
        tracing::warn!(
            task_id = %task.id,
            missing_reports = ?missing_reports,
            "ASR daily agent runner returned without required report files"
        );
        return Err(format!(
            "daily agent runner completed but did not generate {} required report(s): {}",
            missing_reports.len(),
            missing_reports.join(", ")
        ));
    }

    if let Some((adapter, metadata)) = conversation_success.as_ref() {
        persist_daily_agent_conversation_success(
            task,
            adapter,
            &session_key,
            metadata.as_ref(),
        )?;
    }

    // 7. Update processed state
    let mut processed = load_daily_agent_processed_state(&task.id);
    let generated_report_targets = reports_generated.iter().cloned().collect::<HashSet<_>>();

    for entry in plan
        .entries
        .iter()
        .filter(|entry| {
            entry.change_kind != DailyAgentChangeKind::Unchanged
                && generated_report_targets.contains(&entry.report_target)
        })
    {
        processed.documents.insert(
            daily_agent_processed_key(task, &entry.date),
            AsrDailyAgentProcessedDocument {
                agent_id: task.daily_agent.agent_id.clone(),
                agent_name: task.daily_agent.name.clone(),
                output_dir: task.daily_agent.output_dir.clone(),
                date: entry.date.clone(),
                source_sha256: entry.source_sha256.clone(),
                source_len_bytes: entry.source_len_bytes,
                processed_at_ms: now_ms(),
                runner: task.daily_agent.runner.clone(),
                report_path: Some(entry.report_target.clone()),
                last_run_id: run_id.to_string(),
            },
        );
    }

    processed.version = PROCESSED_STATE_VERSION;
    save_daily_agent_processed_state(&task.id, &processed)?;

    sync_daily_agent_reports_after_generation(task, &reports_generated).await?;

    tracing::info!(
        task_id = %task.id,
        reports = reports_generated.len(),
        "updated ASR daily agent processed state"
    );

    Ok(DailyAgentInnerResult {
        reports_generated,
        failed_entries,
    })
}

fn sync_daily_agent_item_status(
    task_config: &mut AsrDailyAgentConfig,
    agent_id: &str,
    update: impl FnOnce(&mut AsrDailyAgentItem),
) {
    if task_config.agents.is_empty() {
        task_config.agents = normalized_daily_agents(task_config);
    }
    if let Some(agent) = task_config
        .agents
        .iter_mut()
        .find(|agent| normalize_daily_agent_token(&agent.id) == agent_id)
    {
        update(agent);
    } else if agent_id == task_config.agent_id {
        let mut item = daily_agent_item_from_legacy(task_config);
        update(&mut item);
        task_config.agents.push(item);
    }
}

fn mirror_daily_agent_legacy_status(task_config: &mut AsrDailyAgentConfig, agent_id: &str) {
    let primary_id = normalized_daily_agents(task_config)
        .first()
        .map(|agent| agent.id.clone())
        .unwrap_or_else(default_daily_agent_id);
    if agent_id == primary_id {
        if let Some(agent) = task_config
            .agents
            .iter()
            .find(|agent| normalize_daily_agent_token(&agent.id) == agent_id)
            .cloned()
        {
            task_config.enabled = agent.enabled;
            task_config.agent_id = agent.id;
            task_config.name = agent.name;
            task_config.runner = agent.runner;
            task_config.timeout_ms = agent.timeout_ms;
            task_config.trigger_policy = agent.trigger_policy;
            task_config.session_key = agent.session_key;
            task_config.instructions_source = agent.instructions_source;
            task_config.instructions = agent.instructions;
            task_config.im_delivery = agent.im_delivery;
            task_config.output_dir = agent.output_dir;
            task_config.report_sync_dir = agent.report_sync_dir;
            task_config.last_report_sync = agent.last_report_sync;
            task_config.last_run_at_ms = agent.last_run_at_ms;
            task_config.last_status = agent.last_status;
            task_config.last_error = agent.last_error;
            task_config.last_run_id = agent.last_run_id;
        }
    }
}

fn set_primary_daily_agent_report_sync_dir(
    task_config: &mut AsrDailyAgentConfig,
    report_sync_dir: Option<String>,
) {
    let report_sync_dir = report_sync_dir
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let primary_id = normalized_daily_agents(task_config)
        .first()
        .map(|agent| normalize_daily_agent_token(&agent.id))
        .unwrap_or_else(default_daily_agent_id);
    task_config.report_sync_dir = report_sync_dir.clone();
    if task_config.agents.is_empty() {
        task_config.agents = normalized_daily_agents(task_config);
    }
    for agent in &mut task_config.agents {
        agent.report_sync_dir = report_sync_dir.clone();
    }
    mirror_daily_agent_legacy_status(task_config, &primary_id);
}

fn update_daily_agent_status(
    source_task: &AsrDirectoryTask,
    status: &str,
    error: Option<&str>,
    run_id: &str,
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
        agent.last_run_at_ms = Some(now_ms());
        agent.last_status = Some(status.to_string());
        agent.last_error = error.map(|e| e.to_string());
        agent.last_run_id = Some(run_id.to_string());
    });
    mirror_daily_agent_legacy_status(&mut task.daily_agent, &agent_id);
    save_tasks(&store)
}

fn update_daily_agent_im_error(source_task: &AsrDirectoryTask, error: &str) -> Result<(), String> {
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
        agent.im_delivery.last_send_error = Some(error.to_string());
    });
    mirror_daily_agent_legacy_status(&mut task.daily_agent, &agent_id);
    save_tasks(&store)
}

fn update_daily_agent_im_sent(source_task: &AsrDirectoryTask) -> Result<(), String> {
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
        agent.im_delivery.last_sent_at_ms = Some(now_ms());
        agent.im_delivery.last_send_error = None;
    });
    mirror_daily_agent_legacy_status(&mut task.daily_agent, &agent_id);
    save_tasks(&store)
}
