// ─── Daily Agent Runner ───────────────────────────────────────────────────────
// ASR 任务完成后的后处理：触发 Agent 对每日转写做二次整理

use std::collections::BTreeMap as DailyAgentBTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

const DEFAULT_DAILY_AGENT_TIMEOUT_MS: u64 = 7_200_000; // 2 hours
const DEFAULT_ASR_DAILY_AGENTS_MD: &str = include_str!("daily_agent_template.md");
const PROCESSED_STATE_VERSION: u32 = 1;
const CONVERSATION_STATE_VERSION: u32 = 1;

fn default_daily_agent_timeout_ms() -> u64 {
    DEFAULT_DAILY_AGENT_TIMEOUT_MS
}

fn default_daily_agent_runner() -> String {
    "bifrost_agent".to_string()
}

/// Per-task locks for Daily Agent runs (independent from ASR_JOB_RUN_LOCK).
/// Each task gets its own async Mutex so different tasks can run concurrently.
static DAILY_AGENT_TASK_LOCKS: Lazy<StdMutex<HashMap<String, Arc<Mutex<()>>>>> =
    Lazy::new(|| StdMutex::new(HashMap::new()));

/// Tracks which task IDs currently have a Daily Agent run in progress
static DAILY_AGENT_RUNNING_TASKS: Lazy<StdMutex<HashSet<String>>> =
    Lazy::new(|| StdMutex::new(HashSet::new()));

/// Local mutex for task config writes from daily agent operations to prevent
/// read-modify-write races on the task store.
static DAILY_AGENT_TASK_CONFIG_LOCK: Lazy<StdMutex<()>> = Lazy::new(|| StdMutex::new(()));

fn get_daily_agent_task_lock(task_id: &str) -> Arc<Mutex<()>> {
    let mut locks = DAILY_AGENT_TASK_LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    locks
        .entry(task_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

// ─── Data Models ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDailyAgentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_daily_agent_runner")]
    pub runner: String,
    #[serde(default = "default_daily_agent_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub trigger_policy: AsrDailyAgentTriggerPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default)]
    pub instructions_source: AsrDailyAgentInstructionsSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default)]
    pub im_delivery: AsrDailyAgentImDeliveryConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}

impl Default for AsrDailyAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            runner: default_daily_agent_runner(),
            timeout_ms: DEFAULT_DAILY_AGENT_TIMEOUT_MS,
            trigger_policy: AsrDailyAgentTriggerPolicy::default(),
            session_key: None,
            instructions_source: AsrDailyAgentInstructionsSource::default(),
            instructions: None,
            im_delivery: AsrDailyAgentImDeliveryConfig::default(),
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            last_run_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentTriggerPolicy {
    #[default]
    AfterAsrRun,
    ManualOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentInstructionsSource {
    #[default]
    Default,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AsrDailyAgentImDeliveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default)]
    pub mode: AsrDailyAgentImDeliveryMode,
    #[serde(default)]
    pub send_policy: AsrDailyAgentImSendPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sent_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_send_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentImDeliveryMode {
    #[default]
    FullReport,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentImSendPolicy {
    #[default]
    OnSuccessWithReport,
    OnSuccess,
    Always,
}

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
}

// ─── Run Result ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct DailyAgentRunResult {
    run_id: String,
    status: String,
    trigger_source: String,
    started_at_ms: u64,
    finished_at_ms: u64,
    error: Option<String>,
    reports_generated: Vec<String>,
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

fn load_daily_agent_conversation_state(task_id: &str) -> AsrDailyAgentConversationState {
    let path = daily_agent_conversation_state_path(task_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_daily_agent_conversation_state(
    task_id: &str,
    state: &AsrDailyAgentConversationState,
) -> Result<(), String> {
    let path = daily_agent_conversation_state_path(task_id);
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

/// Read workspace status without creating directories or files (for GET endpoint).
fn read_workspace_status(task: &AsrDirectoryTask) -> AsrDailyWorkspaceStatus {
    let daily_dir = daily_dir_for_task(&task.id);
    let report_dir = daily_dir.join("report");
    let agents_path = daily_dir.join("AGENTS.md");

    let agents_exists = agents_path.exists();
    let git_initialized = daily_dir.join(".git").exists();

    let report_count = std::fs::read_dir(&report_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "md")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);

    AsrDailyWorkspaceStatus {
        daily_dir: daily_dir.to_string_lossy().to_string(),
        report_dir: report_dir.to_string_lossy().to_string(),
        agents_path: agents_path.to_string_lossy().to_string(),
        agents_exists,
        git_available: true, // Assume available; actual check deferred to ensure_*
        git_initialized,
        git_error: None,
        report_count,
    }
}

fn ensure_asr_daily_workspace(
    task: &AsrDirectoryTask,
) -> Result<AsrDailyWorkspaceStatus, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let report_dir = daily_dir.join("report");
    let agents_path = daily_dir.join("AGENTS.md");
    let gitignore_path = daily_dir.join(".gitignore");

    // Create directories
    std::fs::create_dir_all(&daily_dir).map_err(|e| format!("create daily dir: {e}"))?;
    std::fs::create_dir_all(&report_dir).map_err(|e| format!("create report dir: {e}"))?;

    // Write AGENTS.md if not exists
    let agents_exists = if agents_path.exists() {
        true
    } else {
        let content =
            if task.daily_agent.instructions_source == AsrDailyAgentInstructionsSource::Custom {
                task.daily_agent.instructions.clone().unwrap_or_default()
            } else {
                DEFAULT_ASR_DAILY_AGENTS_MD
                    .replace("{{task_name}}", &task.name)
                    .replace("{{daily_dir}}", ".")
                    .replace("{{report_dir}}", "./report/")
            };
        std::fs::write(&agents_path, content.as_bytes())
            .map_err(|e| format!("write AGENTS.md: {e}"))?;
        true
    };

    // Write .gitignore if not exists
    if !gitignore_path.exists() {
        let _ = std::fs::write(&gitignore_path, ".DS_Store\n");
    }

    // Git init (best-effort)
    let (git_available, git_initialized, git_error) = try_git_init(&daily_dir);

    // Count reports
    let report_count = std::fs::read_dir(&report_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "md")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);

    let status = AsrDailyWorkspaceStatus {
        daily_dir: daily_dir.to_string_lossy().to_string(),
        report_dir: report_dir.to_string_lossy().to_string(),
        agents_path: agents_path.to_string_lossy().to_string(),
        agents_exists,
        git_available,
        git_initialized,
        git_error,
        report_count,
    };

    tracing::info!(
        task_id = %task.id,
        daily_dir = %status.daily_dir,
        git_initialized = status.git_initialized,
        "initialized ASR daily agent workspace"
    );

    Ok(status)
}

fn try_git_init(daily_dir: &Path) -> (bool, bool, Option<String>) {
    // Check if already initialized (no need to run git --version)
    if daily_dir.join(".git").exists() {
        return (true, true, None);
    }

    // Try git init (implicitly checks if git is available)
    let result = std::process::Command::new("git")
        .arg("init")
        .current_dir(daily_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => (true, true, None),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            tracing::warn!(daily_dir = %daily_dir.display(), error = %stderr, "git init failed");
            (true, false, Some(stderr))
        }
        Err(e) => {
            let is_not_found = e.kind() == std::io::ErrorKind::NotFound;
            if is_not_found {
                (false, false, Some("git executable not found".to_string()))
            } else {
                tracing::warn!(daily_dir = %daily_dir.display(), error = %e, "git init failed");
                (true, false, Some(e.to_string()))
            }
        }
    }
}

fn try_git_commit(daily_dir: &Path, message: &str) -> Option<String> {
    if !daily_dir.join(".git").exists() {
        return None;
    }

    // git add *.md report/ .gitignore (track daily source files too)
    let _ = std::process::Command::new("git")
        .args(["add", "*.md", "report/", ".gitignore"])
        .current_dir(daily_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // git commit
    let result = std::process::Command::new("git")
        .args(["commit", "-m", message, "--allow-empty-message"])
        .current_dir(daily_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => {
            tracing::debug!(daily_dir = %daily_dir.display(), "git commit succeeded");
            // Capture the commit hash
            let hash_output = std::process::Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .current_dir(daily_dir)
                .output();
            hash_output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // "nothing to commit" is not a real error
            if !stderr.contains("nothing to commit") {
                tracing::warn!(daily_dir = %daily_dir.display(), error = %stderr, "git commit failed");
            }
            None
        }
        Err(e) => {
            tracing::warn!(daily_dir = %daily_dir.display(), error = %e, "git commit failed");
            None
        }
    }
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

fn daily_agent_report_dirs_for_task(task_id: &str) -> Vec<PathBuf> {
    let daily_dir = daily_dir_for_task(task_id);
    let mut exact_lower = Vec::new();
    let mut case_compat = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&daily_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "report" {
                exact_lower.push(path);
            } else if name.eq_ignore_ascii_case("report") {
                case_compat.push(path);
            }
        }
    }

    exact_lower.sort();
    case_compat.sort();
    let mut dirs = exact_lower;
    dirs.extend(case_compat);
    if dirs.is_empty() {
        dirs.push(daily_dir.join("report"));
    }
    dirs
}

fn daily_agent_report_date_from_path(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    let date = filename.strip_suffix("-report.md")?;
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
        Some(date.to_string())
    } else {
        None
    }
}

fn list_daily_agent_report_files(task_id: &str) -> Vec<PathBuf> {
    let mut reports = Vec::new();
    for report_dir in daily_agent_report_dirs_for_task(task_id) {
        let Ok(entries) = std::fs::read_dir(&report_dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() && daily_agent_report_date_from_path(&path).is_some() {
                reports.push(path);
            }
        }
    }
    reports.sort();
    reports
}

fn build_daily_agent_report_index_status(
    task_id: &str,
    processed: &AsrDailyAgentProcessedState,
) -> AsrDailyAgentReportIndexStatus {
    let report_files = list_daily_agent_report_files(task_id);
    let report_dates: HashSet<String> = report_files
        .iter()
        .filter_map(|path| daily_agent_report_date_from_path(path))
        .collect();

    let mut unindexed_dates: Vec<String> = report_dates
        .iter()
        .filter(|date| !processed.documents.contains_key(*date))
        .cloned()
        .collect();
    unindexed_dates.sort();

    let processed_missing_report = processed
        .documents
        .keys()
        .filter(|date| !report_dates.contains(*date))
        .count();

    AsrDailyAgentReportIndexStatus {
        report_files: report_dates.len(),
        processed_documents: processed.documents.len(),
        indexed_reports: report_dates.len().saturating_sub(unindexed_dates.len()),
        unindexed_reports: unindexed_dates.len(),
        processed_missing_report,
        unindexed_dates,
    }
}

fn build_daily_agent_change_plan(
    task: &AsrDirectoryTask,
    _trigger_source: &str,
    requested_date: Option<&str>,
    force: bool,
) -> Result<AsrDailyAgentChangePlan, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let report_dir = daily_dir.join("report");
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
        } else if let Some(prev) = processed.documents.get(date) {
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

fn build_daily_agent_prompt(
    task: &AsrDirectoryTask,
    plan: &AsrDailyAgentChangePlan,
    adapter: &str,
    chatgpt_first_turn: bool,
) -> Result<String, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let changed_entries: Vec<_> = plan
        .entries
        .iter()
        .filter(|e| e.change_kind != DailyAgentChangeKind::Unchanged)
        .collect();

    let is_file_capable = adapter != "chatgpt_web";
    let is_chatgpt_web = adapter == "chatgpt_web";

    let mut prompt = if is_file_capable {
        String::from("请根据当前目录 AGENTS.md，检查并处理以下变更文件：\n\n")
    } else {
        String::from("请根据以下 AGENTS.md 指令，对变更文件进行分析整理，直接以 Markdown 格式输出报告内容：\n\n")
    };
            for entry in &changed_entries {
                if is_file_capable {
                    prompt.push_str(&format!(
                        "- {}.md: change_kind={:?}, source_sha256={}, report={}\n",
                        entry.date,
                        entry.change_kind,
                        &entry.source_sha256[..8],
                        PathBuf::from(&entry.report_target)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    ));
                } else {
                    prompt.push_str(&format!(
                        "- {}.md: change_kind={:?}\n",
                        entry.date, entry.change_kind,
                    ));
                }
            }
            if is_file_capable {
                prompt.push_str(
                    "\n只刷新这些日期对应的 report。不要修改原始 YYYY-MM-DD.md。\n",
                );
            } else {
                prompt.push_str(
                    "\n请直接输出 Markdown 格式的报告。不需要创建文件，不需要代码块包裹，直接输出内容即可。\n",
                );
            }

            if is_chatgpt_web {
                if chatgpt_first_turn {
                    prompt.push_str(
                        "\n这是该 ASR 任务固定 ChatGPT Web 对话的第一轮。请先记住 AGENTS.md 指令，后续消息只会发送新增或变更内容。\n",
                    );
                } else {
                    prompt.push_str(
                        "\n这是该 ASR 任务固定 ChatGPT Web 对话的后续轮次。沿用之前的 AGENTS.md 指令，只处理本轮新增或变更内容。\n",
                    );
                }

                let agents_path = daily_dir.join("AGENTS.md");
                if chatgpt_first_turn {
                    if let Ok(agents_content) = std::fs::read_to_string(&agents_path) {
                    prompt.push_str("\n---\n## AGENTS.md 内容：\n\n```markdown\n");
                    prompt.push_str(&agents_content);
                    prompt.push_str("\n```\n");
                    }
                }

                prompt.push_str("\n---\n## 已有 report 内容（如存在，用于增量合并）：\n");
                for entry in &changed_entries {
                    if let Ok(report_content) = std::fs::read_to_string(&entry.report_target) {
                        prompt.push_str(&format!(
                            "\n### {}-report.md:\n\n```markdown\n{}\n```\n",
                            entry.date, report_content
                        ));
                    }
                }

                prompt.push_str("\n---\n## 变更文件内容：\n");
                for entry in &changed_entries {
                    if let Ok(file_content) = std::fs::read_to_string(&entry.source_path) {
                        let content_to_include = if entry.change_kind
                            == DailyAgentChangeKind::Appended
                        {
                            // For appended files, only include the new content
                            if let Some(offset) = entry.append_offset {
                                if (offset as usize) < file_content.len() {
                                    format!(
                                        "[新增内容，从字节 {} 开始]\n{}",
                                        offset,
                                        &file_content[offset as usize..]
                                    )
                                } else {
                                    file_content
                                }
                            } else {
                                file_content
                            }
                        } else {
                            file_content
                        };
                        prompt.push_str(&format!(
                            "\n### {}.md ({:?}):\n\n```markdown\n{}\n```\n",
                            entry.date, entry.change_kind, content_to_include
                        ));
                    }
                }
            }

    Ok(prompt)
}

async fn maybe_enqueue_daily_agent_after_asr_run(task: &AsrDirectoryTask) {
    if !task.daily_agent.enabled {
        return;
    }
    if task.daily_agent.trigger_policy != AsrDailyAgentTriggerPolicy::AfterAsrRun {
        return;
    }
    let summary = summarize_task_from_store(task);
    if !daily_agent_asr_completion_ready(&summary) {
        tracing::info!(
            task_id = %task.id,
            pending = summary.pending,
            "skipped daily agent: ASR task still has pending/processing files"
        );
        return;
    }
    if !daily_agent_runner_ready(task) {
        tracing::debug!(
            task_id = %task.id,
            runner = %task.daily_agent.runner,
            "skipped daily agent: runner not configured"
        );
        return;
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
        run_daily_agent(&task_clone, "asr_completion", None, false).await;
    });

    tracing::info!(
        task_id = %task_id,
        trigger_source = "asr_completion",
        runner = %task.daily_agent.runner,
        "queued ASR daily agent run"
    );
}

/// 判断 ASR 任务是否已完成所有待处理工作，可以触发 daily agent。
/// 只检查是否还有尚未处理或正在处理中的文件（pending 包含 Pending + Processing 状态）。
/// 失败或部分成功的文件不阻塞 daily agent 触发——因为这些文件可能永远无法成功。
fn daily_agent_asr_completion_ready(summary: &TaskSummary) -> bool {
    summary.pending == 0
}

fn daily_agent_effective_last_status(task: &AsrDirectoryTask) -> Option<String> {
    if task.daily_agent.last_status.as_deref() == Some("running") {
        let running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !running.contains(&task.id) {
            return Some("interrupted".to_string());
        }
    }
    task.daily_agent.last_status.clone()
}

async fn run_daily_agent(
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
        run_id = %run_id,
        trigger_source,
        runner = %task.daily_agent.runner,
        force,
        requested_date = ?requested_date,
        "starting ASR daily agent run"
    );

    // Mark as running
    {
        let mut running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.insert(task_id.clone());
    }
    let _ = update_daily_agent_status(&task_id, "running", None, &run_id);

    // Acquire per-task lock
    let task_lock = get_daily_agent_task_lock(&task_id);
    let _lock = task_lock.lock().await;

    let result = run_daily_agent_inner(
        task,
        trigger_source,
        requested_date,
        force,
        &run_id,
    )
    .await;

    // Remove from running set
    {
        let mut running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.remove(&task_id);
    }

    // Determine success/failure
    let (status_str, error, reports_generated) = match &result {
        Ok(reports) => {
            // Git commit after successful run
            let commit_msg = format!(
                "daily agent run {} ({}): {} report(s)",
                &run_id[..8.min(run_id.len())],
                trigger_source,
                reports.len()
            );
            let _commit_hash = try_git_commit(&daily_dir, &commit_msg);
            ("success".to_string(), None, reports.clone())
        }
        Err(e) => ("failed".to_string(), Some(e.clone()), Vec::new()),
    };

    // Persist daily agent state in task config
    let _ = update_daily_agent_status(&task_id, &status_str, error.as_deref(), &run_id);

    // IM delivery based on send_policy
    let success = error.is_none();
    let should_send_im = task.daily_agent.im_delivery.enabled
        && daily_agent_im_channel(task).is_some()
        && match task.daily_agent.im_delivery.send_policy {
            AsrDailyAgentImSendPolicy::Always => true,
            AsrDailyAgentImSendPolicy::OnSuccess => success,
            AsrDailyAgentImSendPolicy::OnSuccessWithReport => {
                success && !reports_generated.is_empty()
            }
        };

    if should_send_im {
        let im_content = if success {
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
            let _ = update_daily_agent_im_error(&task_id, &e);
        }
    }

    let finished_at_ms = now_ms();

    tracing::info!(
        task_id = %task_id,
        run_id = %run_id,
        status = %status_str,
        reports = reports_generated.len(),
        duration_ms = finished_at_ms - started_at_ms,
        "ASR daily agent run completed"
    );

    DailyAgentRunResult {
        run_id,
        status: status_str,
        trigger_source: trigger_source.to_string(),
        started_at_ms,
        finished_at_ms,
        error,
        reports_generated,
    }
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
    let channel = task.daily_agent.im_delivery.channel.as_deref()?.trim();
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

fn daily_agent_runner_ready(task: &AsrDirectoryTask) -> bool {
    !task.daily_agent.runner.trim().is_empty()
}

fn daily_agent_runner_is_bifrost(task: &AsrDirectoryTask) -> bool {
    task.daily_agent.runner.trim() == "bifrost_agent"
}

fn daily_agent_external_runner_id(task: &AsrDirectoryTask) -> Option<&str> {
    let runner = task.daily_agent.runner.trim();
    (!runner.is_empty() && runner != "bifrost_agent").then_some(runner)
}

fn collect_report_outputs_for_plan(
    plan: &AsrDailyAgentChangePlan,
) -> (Vec<String>, Vec<String>) {
    let mut reports_generated = Vec::new();
    let mut missing_reports = Vec::new();
    for entry in plan
        .entries
        .iter()
        .filter(|entry| entry.change_kind != DailyAgentChangeKind::Unchanged)
    {
        let report_path = PathBuf::from(&entry.report_target);
        if report_path.exists() {
            reports_generated.push(entry.report_target.clone());
        } else {
            missing_reports.push(entry.report_target.clone());
        }
    }
    (reports_generated, missing_reports)
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
    task_id: &str,
    adapter: &str,
    session_key: &str,
    metadata: Option<&DailyAgentBTreeMap<String, String>>,
) -> Result<(), String> {
    let mut state = load_daily_agent_conversation_state(task_id);
    state.version = CONVERSATION_STATE_VERSION;
    state.adapter = Some(adapter.to_string());
    state.session_key = Some(session_key.to_string());
    state.initialized = true;
    state.updated_at_ms = Some(now_ms());
    if let Some(metadata) = metadata {
        if let Some(conversation_id) =
            metadata_value(metadata, &["conversationId", "conversation_id"])
        {
            state.conversation_id = Some(conversation_id);
        }
        if let Some(thread_id) = metadata_value(metadata, &["threadId", "thread_id"]) {
            state.thread_id = Some(thread_id);
        }
    }
    save_daily_agent_conversation_state(task_id, &state)
}

async fn run_bifrost_agent_daily_runner(
    task: &AsrDirectoryTask,
    prompt: &str,
    daily_dir: &Path,
    session_key: &str,
) -> Result<String, String> {
    bifrost_agent::install_system_skills();
    let agent_data_dir = bifrost_storage::data_dir().join("agent");
    let _ = std::fs::create_dir_all(&agent_data_dir);
    let mut config = bifrost_agent::AgentConfigStore::new(&agent_data_dir).load();
    config.enabled = true;
    config.runner = None;
    config.work_dir = Some(daily_dir.to_string_lossy().to_string());

    let client = bifrost_agent::AgentClient::default();
    let tools = bifrost_agent::ToolRegistry::with_defaults();
    let session_manager = bifrost_agent::AgentSessionManager::new(config.get_session_ttl_secs());
    let Some(mut session) = session_manager.try_take_session_with_work_dir(
        session_key,
        Some(daily_dir.to_string_lossy().to_string()),
    ) else {
        return Err(format!(
            "bifrost_agent session '{session_key}' is already running"
        ));
    };

    // 设置 session 来源标记，使其在 sessions 列表中可见
    session.source = "daily_agent".to_string();
    session.mark_bifrost_agent_runtime();

    // 创建 ConversationRecorder 以持久化会话记录
    let persist_data_dir = bifrost_agent::config::agent_home_dir();
    let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(
        &persist_data_dir,
        session_key,
    );
    let _ = recorder.record_session_start(
        session_key,
        serde_json::json!({
            "source": "daily_agent",
            "work_dir": daily_dir.to_string_lossy(),
            "task_id": task.id,
            "task_name": task.name,
            "model": config.model,
            "provider": config.model_provider,
        }),
    );

    let timeout_result = tokio::time::timeout(
        Duration::from_millis(task.daily_agent.timeout_ms),
        bifrost_agent::session::run_turn_with_mcp(
            &client,
            &config,
            &mut session,
            &tools,
            None,
            prompt,
            None,
            Some(&mut recorder),
        ),
    )
    .await;

    // 无论成功、失败还是超时，都记录 session 结束
    let (status, final_result) = match timeout_result {
        Ok(Ok(turn)) => ("success", Ok(turn.response)),
        Ok(Err(e)) => ("failed", Err(format!("bifrost_agent run failed: {e}"))),
        Err(_) => (
            "timeout",
            Err(format!(
                "daily agent run timed out after {}ms",
                task.daily_agent.timeout_ms
            )),
        ),
    };

    let _ = recorder.record_session_end(
        session_key,
        serde_json::json!({
            "total_tokens": session.total_tokens_used.unwrap_or(0),
            "status": status,
        }),
    );

    session_manager.return_session(session);
    final_result
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
    let operation = match effective.settings.adapter.as_str() {
        "codex" => "run".to_string(),
        _ => "send".to_string(),
    };

    let mut params = serde_json::Map::new();
    if effective.settings.adapter == "codex" {
        if let Some(thread_id) = conversation_state.thread_id.as_deref() {
            params.insert(
                "threadId".to_string(),
                serde_json::Value::String(thread_id.to_string()),
            );
        }
    }
    if effective.settings.adapter == "chatgpt_web" {
        if let Some(conversation_id) = conversation_state.conversation_id.as_deref() {
            params.insert(
                "conversationId".to_string(),
                serde_json::Value::String(conversation_id.to_string()),
            );
        }
    }
    let params = if params.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(params)
    };

    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        message: prompt,
        operation,
        params,
        provider_id: None,
        runner_id: Some(runner_id.to_string()),
        session_key: Some(session_key.to_string()),
        runtime: "external_cli".to_string(),
        adapter: effective.settings.adapter.clone(),
        work_dir: Some(daily_dir.to_path_buf()),
        instructions: None,
        adapter_config: effective.settings.adapter_config.clone(),
        allow_work_dirs: vec![daily_dir.to_string_lossy().to_string()],
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
) -> Result<Vec<String>, String> {
    // 1. Ensure workspace
    ensure_asr_daily_workspace(task)?;

    // 2. Build change plan
    let plan = build_daily_agent_change_plan(task, trigger_source, requested_date, force)?;

    if plan.skipped {
        tracing::info!(
            task_id = %task.id,
            trigger_source,
            reason = plan.skip_reason.as_deref().unwrap_or("unknown"),
            "skipped ASR daily agent run"
        );
        return Ok(Vec::new());
    }

    // 3. Determine adapter for prompt construction
    let adapter = if daily_agent_runner_is_bifrost(task) {
        "bifrost_agent".to_string()
    } else if let Some(runner_id) = daily_agent_external_runner_id(task) {
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
    let daily_dir = daily_dir_for_task(&task.id);
    let session_key = task
        .daily_agent
        .session_key
        .clone()
        .unwrap_or_else(|| format!("asr-daily:{}", task.id));
    let mut conversation_state = load_daily_agent_conversation_state(&task.id);

    // 5. Dispatch to runner
    let conversation_success: Option<(String, Option<DailyAgentBTreeMap<String, String>>)>;

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
                let changed_entries: Vec<_> = plan
                    .entries
                    .iter()
                    .filter(|e| e.change_kind != DailyAgentChangeKind::Unchanged)
                    .cloned()
                    .collect();
                for entry in changed_entries {
                    let entry_plan = single_entry_change_plan(&plan, entry.clone());
                    let chatgpt_first_turn = !conversation_state.initialized;
                    let prompt =
                        build_daily_agent_prompt(task, &entry_plan, &adapter, chatgpt_first_turn)?;
                    let run_result = run_external_daily_agent_prompt(
                        task,
                        &runner_id,
                        prompt,
                        &daily_dir,
                        &session_key,
                        &conversation_state,
                        &effective,
                    )
                    .await?;
                    apply_external_daily_agent_metadata(
                        &mut conversation_state,
                        &run_result.metadata,
                    );
                    last_metadata = Some(run_result.metadata.clone());
                    if run_result.response.trim().is_empty() {
                        continue;
                    }
                    let report_path = PathBuf::from(&entry.report_target);
                    if let Some(parent) = report_path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    if let Err(e) = std::fs::write(&report_path, run_result.response.trim()) {
                        tracing::warn!(
                            report_path = %report_path.display(),
                            error = %e,
                            "failed to save chatgpt_web response as report"
                        );
                    } else {
                        tracing::info!(
                            report_path = %report_path.display(),
                            len = run_result.response.trim().len(),
                            "saved chatgpt_web response as report"
                        );
                    }
                }
            } else {
                let prompt = build_daily_agent_prompt(task, &plan, &adapter, false)?;
                let run_result = run_external_daily_agent_prompt(
                    task,
                    &runner_id,
                    prompt,
                    &daily_dir,
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
            let prompt = build_daily_agent_prompt(task, &plan, &adapter, false)?;
            let response =
                run_bifrost_agent_daily_runner(task, &prompt, &daily_dir, &session_key).await?;
            conversation_success = Some(("bifrost_agent".to_string(), None));
            tracing::info!(
                task_id = %task.id,
                response_len = response.len(),
                "bifrost_agent daily runner completed"
            );
    }

    // 6. Validate reports before marking source documents processed.
    let (reports_generated, missing_reports) = collect_report_outputs_for_plan(&plan);
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
            &task.id,
            adapter,
            &session_key,
            metadata.as_ref(),
        )?;
    }

    // 7. Update processed state
    let mut processed = load_daily_agent_processed_state(&task.id);

    for entry in plan
        .entries
        .iter()
        .filter(|entry| entry.change_kind != DailyAgentChangeKind::Unchanged)
    {
        processed.documents.insert(
            entry.date.clone(),
            AsrDailyAgentProcessedDocument {
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

    tracing::info!(
        task_id = %task.id,
        reports = reports_generated.len(),
        "updated ASR daily agent processed state"
    );

    Ok(reports_generated)
}

fn update_daily_agent_status(
    task_id: &str,
    status: &str,
    error: Option<&str>,
    run_id: &str,
) -> Result<(), String> {
    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) else {
        return Err(format!("ASR task '{task_id}' not found"));
    };
    task.daily_agent.last_run_at_ms = Some(now_ms());
    task.daily_agent.last_status = Some(status.to_string());
    task.daily_agent.last_error = error.map(|e| e.to_string());
    task.daily_agent.last_run_id = Some(run_id.to_string());
    save_tasks(&store)
}

fn update_daily_agent_im_error(task_id: &str, error: &str) -> Result<(), String> {
    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) else {
        return Err(format!("ASR task '{task_id}' not found"));
    };
    task.daily_agent.im_delivery.last_send_error = Some(error.to_string());
    save_tasks(&store)
}

fn update_daily_agent_im_sent(task_id: &str) -> Result<(), String> {
    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) else {
        return Err(format!("ASR task '{task_id}' not found"));
    };
    task.daily_agent.im_delivery.last_sent_at_ms = Some(now_ms());
    task.daily_agent.im_delivery.last_send_error = None;
    save_tasks(&store)
}
