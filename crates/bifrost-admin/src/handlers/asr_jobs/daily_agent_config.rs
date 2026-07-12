// ─── Daily Agent Runner ───────────────────────────────────────────────────────
// ASR 任务完成后的后处理：触发 Agent 对每日转写做二次整理

use std::collections::BTreeMap as DailyAgentBTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

const DEFAULT_DAILY_AGENT_TIMEOUT_MS: u64 = 7_200_000; // 2 hours
const DEFAULT_ASR_DAILY_AGENTS_MD: &str = include_str!("daily_agent_template.md");
const DEFAULT_ASR_TOMORROW_TODO_AGENT_MD: &str = include_str!("daily_agent_tomorrow_todo_template.md");
const DEFAULT_ASR_RESEARCH_SEED_AGENT_MD: &str =
    include_str!("daily_agent_research_seed_template.md");
const DEFAULT_ASR_RESEARCH_DISPATCHER_AGENT_MD: &str =
    include_str!("daily_agent_research_dispatcher_template.md");
const PROCESSED_STATE_VERSION: u32 = 1;
const CONVERSATION_STATE_VERSION: u32 = 1;
const DEFAULT_DAILY_AGENT_ID: &str = "daily_report";
const DEFAULT_DAILY_AGENT_NAME: &str = "daily_report";
const DEFAULT_DAILY_AGENT_OUTPUT_DIR: &str = "report";
const DEFAULT_TOMORROW_TODO_AGENT_ID: &str = "tomorrow_todo";
const DEFAULT_TOMORROW_TODO_AGENT_NAME: &str = "tomorrow_todo";
const DEFAULT_TOMORROW_TODO_OUTPUT_DIR: &str = "tomorrow_todo";
const DEFAULT_RESEARCH_SEED_AGENT_ID: &str = "research_seed";
const DEFAULT_RESEARCH_DISPATCHER_AGENT_ID: &str = "research_dispatcher";
const DEFAULT_DAILY_AGENT_IM_CHANNEL: &str = "owner:feishu-main";
const DAILY_AGENT_TERMS_FILENAME: &str = "TERMS.md";

fn default_daily_agent_timeout_ms() -> u64 {
    DEFAULT_DAILY_AGENT_TIMEOUT_MS
}

fn default_daily_agent_runner() -> String {
    "Codex".to_string()
}

fn default_daily_agent_id() -> String {
    DEFAULT_DAILY_AGENT_ID.to_string()
}

fn default_daily_agent_name() -> String {
    DEFAULT_DAILY_AGENT_NAME.to_string()
}

fn default_daily_agent_output_dir() -> String {
    DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string()
}

fn daily_agent_enabled_by_default() -> bool {
    true
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
    #[serde(default = "default_daily_agent_id")]
    pub agent_id: String,
    #[serde(default = "default_daily_agent_name")]
    pub name: String,
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
    #[serde(default = "default_daily_agent_output_dir")]
    pub output_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<AsrDailyAgentDependency>,
    #[serde(default)]
    pub dependency_failure_policy: AsrDailyAgentDependencyFailurePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_fanout: Option<AsrDailyAgentResearchFanoutConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AsrDailyAgentItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminology: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_sync_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report_sync: Option<AsrDailyAgentReportSyncResult>,
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
            enabled: true,
            agent_id: default_daily_agent_id(),
            name: default_daily_agent_name(),
            runner: default_daily_agent_runner(),
            timeout_ms: DEFAULT_DAILY_AGENT_TIMEOUT_MS,
            trigger_policy: AsrDailyAgentTriggerPolicy::default(),
            session_key: None,
            instructions_source: AsrDailyAgentInstructionsSource::default(),
            instructions: None,
            im_delivery: AsrDailyAgentImDeliveryConfig::default(),
            output_dir: default_daily_agent_output_dir(),
            dependencies: Vec::new(),
            dependency_failure_policy: AsrDailyAgentDependencyFailurePolicy::default(),
            research_fanout: None,
            agents: default_daily_agent_items(),
            terminology: None,
            report_sync_dir: None,
            last_report_sync: None,
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            last_run_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDailyAgentItem {
    #[serde(default = "default_daily_agent_id")]
    pub id: String,
    #[serde(default = "default_daily_agent_name")]
    pub name: String,
    #[serde(default = "daily_agent_enabled_by_default")]
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
    #[serde(default = "default_daily_agent_output_dir")]
    pub output_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<AsrDailyAgentDependency>,
    #[serde(default)]
    pub dependency_failure_policy: AsrDailyAgentDependencyFailurePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_fanout: Option<AsrDailyAgentResearchFanoutConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_sync_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report_sync: Option<AsrDailyAgentReportSyncResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}

impl AsrDailyAgentItem {
    fn daily_report() -> Self {
        Self {
            id: DEFAULT_DAILY_AGENT_ID.to_string(),
            name: DEFAULT_DAILY_AGENT_NAME.to_string(),
            enabled: true,
            runner: default_daily_agent_runner(),
            timeout_ms: DEFAULT_DAILY_AGENT_TIMEOUT_MS,
            trigger_policy: AsrDailyAgentTriggerPolicy::AfterAsrRun,
            session_key: None,
            instructions_source: AsrDailyAgentInstructionsSource::Default,
            instructions: None,
            im_delivery: AsrDailyAgentImDeliveryConfig::default(),
            output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
            dependencies: Vec::new(),
            dependency_failure_policy: AsrDailyAgentDependencyFailurePolicy::default(),
            research_fanout: None,
            report_sync_dir: None,
            last_report_sync: None,
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            last_run_id: None,
        }
    }

    fn tomorrow_todo() -> Self {
        Self {
            id: DEFAULT_TOMORROW_TODO_AGENT_ID.to_string(),
            name: DEFAULT_TOMORROW_TODO_AGENT_NAME.to_string(),
            enabled: true,
            runner: default_daily_agent_runner(),
            timeout_ms: DEFAULT_DAILY_AGENT_TIMEOUT_MS,
            trigger_policy: AsrDailyAgentTriggerPolicy::AfterAsrRun,
            session_key: None,
            instructions_source: AsrDailyAgentInstructionsSource::Default,
            instructions: None,
            im_delivery: AsrDailyAgentImDeliveryConfig {
                enabled: true,
                channel: Some(DEFAULT_DAILY_AGENT_IM_CHANNEL.to_string()),
                mode: AsrDailyAgentImDeliveryMode::FullReport,
                send_policy: AsrDailyAgentImSendPolicy::OnSuccessWithReport,
                last_sent_at_ms: None,
                last_send_error: None,
            },
            output_dir: DEFAULT_TOMORROW_TODO_OUTPUT_DIR.to_string(),
            dependencies: Vec::new(),
            dependency_failure_policy: AsrDailyAgentDependencyFailurePolicy::default(),
            research_fanout: None,
            report_sync_dir: None,
            last_report_sync: None,
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            last_run_id: None,
        }
    }
}

fn default_daily_agent_items() -> Vec<AsrDailyAgentItem> {
    vec![
        AsrDailyAgentItem::daily_report(),
        AsrDailyAgentItem::tomorrow_todo(),
    ]
}

fn daily_agent_item_from_legacy(config: &AsrDailyAgentConfig) -> AsrDailyAgentItem {
    let primary_defaults = !config.agents.is_empty()
        && config.agent_id == DEFAULT_DAILY_AGENT_ID
        && config.name == DEFAULT_DAILY_AGENT_NAME
        && config.runner == default_daily_agent_runner()
        && config.output_dir == DEFAULT_DAILY_AGENT_OUTPUT_DIR
        && config.dependencies.is_empty()
        && config.dependency_failure_policy == AsrDailyAgentDependencyFailurePolicy::Skip;
    if primary_defaults {
        return config.agents[0].clone();
    }
    AsrDailyAgentItem {
        id: if config.agent_id.trim().is_empty() {
            default_daily_agent_id()
        } else {
            config.agent_id.trim().to_string()
        },
        name: if config.name.trim().is_empty() {
            default_daily_agent_name()
        } else {
            config.name.trim().to_string()
        },
        enabled: true,
        runner: config.runner.clone(),
        timeout_ms: config.timeout_ms,
        trigger_policy: config.trigger_policy.clone(),
        session_key: config.session_key.clone(),
        instructions_source: config.instructions_source.clone(),
        instructions: config.instructions.clone(),
        im_delivery: config.im_delivery.clone(),
        output_dir: if config.output_dir.trim().is_empty() {
            default_daily_agent_output_dir()
        } else {
            config.output_dir.trim().to_string()
        },
        dependencies: config.dependencies.clone(),
        dependency_failure_policy: config.dependency_failure_policy.clone(),
        research_fanout: config.research_fanout.clone(),
        report_sync_dir: config.report_sync_dir.clone(),
        last_report_sync: config.last_report_sync.clone(),
        last_run_at_ms: config.last_run_at_ms,
        last_status: config.last_status.clone(),
        last_error: config.last_error.clone(),
        last_run_id: config.last_run_id.clone(),
    }
}

fn daily_agent_config_from_item(item: &AsrDailyAgentItem) -> AsrDailyAgentConfig {
    AsrDailyAgentConfig {
        enabled: item.enabled,
        agent_id: item.id.clone(),
        name: item.name.clone(),
        runner: item.runner.clone(),
        timeout_ms: item.timeout_ms,
        trigger_policy: item.trigger_policy.clone(),
        session_key: item.session_key.clone(),
        instructions_source: item.instructions_source.clone(),
        instructions: item.instructions.clone(),
        im_delivery: item.im_delivery.clone(),
        output_dir: item.output_dir.clone(),
        dependencies: item.dependencies.clone(),
        dependency_failure_policy: item.dependency_failure_policy.clone(),
        research_fanout: item.research_fanout.clone(),
        agents: Vec::new(),
        terminology: None,
        report_sync_dir: item.report_sync_dir.clone(),
        last_report_sync: item.last_report_sync.clone(),
        last_run_at_ms: item.last_run_at_ms,
        last_status: item.last_status.clone(),
        last_error: item.last_error.clone(),
        last_run_id: item.last_run_id.clone(),
    }
}

fn normalize_daily_agent_token(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches(['_', '-'])
        .to_string()
}

fn is_valid_daily_agent_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn normalize_daily_agent_item(mut item: AsrDailyAgentItem) -> AsrDailyAgentItem {
    item.id = item.id.trim().to_string();
    if item.id.is_empty() {
        item.id = normalize_daily_agent_token(&item.name);
    }
    if item.id.is_empty() {
        item.id = default_daily_agent_id();
    }
    item.name = item.name.trim().to_string();
    if item.name.is_empty() {
        item.name = item.id.clone();
    }
    item.output_dir = item.output_dir.trim().to_string();
    if item.output_dir.is_empty() {
        item.output_dir = item.name.clone();
    }
    item.runner = item.runner.trim().to_string();
    item.session_key = item
        .session_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    item.report_sync_dir = item
        .report_sync_dir
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    item.dependencies = item
        .dependencies
        .into_iter()
        .map(|mut dependency| {
            dependency.agent_id = dependency.agent_id.trim().to_string();
            dependency
        })
        .collect();
    if let Some(mut fanout) = item.research_fanout.take() {
        fanout.chatgpt_interface_mode = fanout
            .chatgpt_interface_mode
            .trim()
            .to_ascii_lowercase();
        fanout.chatgpt_model = fanout.chatgpt_model.trim().to_ascii_lowercase();
        fanout.allowed_runners = fanout
            .allowed_runners
            .into_iter()
            .map(|runner| runner.trim().to_string())
            .filter(|runner| !runner.is_empty())
            .collect();
        fanout.allowed_runners.sort();
        fanout.allowed_runners.dedup();
        fanout.context_profiles = fanout
            .context_profiles
            .into_iter()
            .map(|(key, mut profile)| {
                profile.runner = profile.runner.trim().to_string();
                profile.work_dir = profile.work_dir.trim().to_string();
                profile.instructions = profile
                    .instructions
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty());
                (key.trim().to_string(), profile)
            })
            .collect();
        item.research_fanout = Some(fanout);
    }
    item
}

fn normalize_daily_agent_terminology(terminology: Option<String>) -> Option<String> {
    terminology
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalized_daily_agents(config: &AsrDailyAgentConfig) -> Vec<AsrDailyAgentItem> {
    let mut agents = if config.agents.is_empty() {
        let mut agents = vec![daily_agent_item_from_legacy(config)];
        let tomorrow_todo = AsrDailyAgentItem::tomorrow_todo();
        if agents.iter().all(|agent| agent.id != tomorrow_todo.id)
            && agents
                .iter()
                .all(|agent| agent.output_dir != tomorrow_todo.output_dir)
        {
            agents.push(tomorrow_todo);
        }
        agents
    } else {
        config.agents.clone()
    };
    agents = agents.into_iter().map(normalize_daily_agent_item).collect();
    agents
}

fn normalize_daily_agent_config(config: &AsrDailyAgentConfig) -> AsrDailyAgentConfig {
    let enabled = config.enabled;
    let agents = normalized_daily_agents(config);
    let Some(first_agent) = agents.first().cloned() else {
        return AsrDailyAgentConfig {
            enabled,
            ..Default::default()
        };
    };
    let mut normalized = daily_agent_config_from_item(&first_agent);
    normalized.enabled = enabled;
    normalized.agents = agents;
    normalized.terminology = normalize_daily_agent_terminology(config.terminology.clone());
    normalized
}

fn normalize_daily_agent_config_in_place(config: &mut AsrDailyAgentConfig) {
    *config = normalize_daily_agent_config(config);
}

fn validate_daily_agent_item(item: &AsrDailyAgentItem) -> Result<(), String> {
    if !is_valid_daily_agent_token(&item.id) {
        return Err("Daily Agent id must use English letters, numbers, '_' or '-'".to_string());
    }
    if !is_valid_daily_agent_token(&item.name) {
        return Err("Daily Agent name must use English letters, numbers, '_' or '-'".to_string());
    }
    if !is_valid_daily_agent_token(&item.output_dir) {
        return Err("Daily Agent output_dir must use English letters, numbers, '_' or '-'".to_string());
    }
    if item.enabled && item.runner.trim().is_empty() {
        return Err(format!("Daily Agent '{}' must select a runner", item.name));
    }
    if item.im_delivery.enabled && daily_agent_im_channel_for_config(&item.im_delivery).is_none() {
        return Err(format!("Daily Agent '{}' must select an IM channel", item.name));
    }
    if let Some(fanout) = &item.research_fanout {
        if fanout.max_questions == 0 || fanout.max_questions > 50 {
            return Err(format!(
                "Daily Agent '{}' research_fanout.max_questions must be between 1 and 50",
                item.name
            ));
        }
        if fanout.chatgpt_interface_mode != "chat" || fanout.chatgpt_model != "pro" {
            return Err(format!(
                "Daily Agent '{}' research fan-out requires ChatGPT interface_mode='chat' and model='pro'",
                item.name
            ));
        }
        for runner in &fanout.allowed_runners {
            if runner.trim().is_empty() {
                return Err(format!(
                    "Daily Agent '{}' research_fanout.allowed_runners cannot contain an empty runner",
                    item.name
                ));
            }
        }
        for (key, profile) in &fanout.context_profiles {
            if !is_valid_daily_agent_token(key) {
                return Err(format!(
                    "Daily Agent '{}' context profile '{}' must use English letters, numbers, '_' or '-'",
                    item.name, key
                ));
            }
            if profile.runner.trim().is_empty() || profile.work_dir.trim().is_empty() {
                return Err(format!(
                    "Daily Agent '{}' context profile '{}' requires runner and work_dir",
                    item.name, key
                ));
            }
        }
    }
    Ok(())
}

fn validate_daily_agent_config(config: &AsrDailyAgentConfig) -> Result<(), String> {
    let agents = normalized_daily_agents(config);
    let mut seen_ids = HashSet::new();
    let mut seen_dirs = HashSet::new();
    for agent in &agents {
        validate_daily_agent_item(agent)?;
        if !seen_ids.insert(agent.id.clone()) {
            return Err(format!("Duplicate Daily Agent id '{}'", agent.id));
        }
        if !seen_dirs.insert(agent.output_dir.clone()) {
            return Err(format!(
                "Duplicate Daily Agent output_dir '{}'",
                agent.output_dir
            ));
        }
    }
    let known_ids = agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<HashSet<_>>();
    for agent in &agents {
        let mut seen_dependencies = HashSet::new();
        for dependency in &agent.dependencies {
            if dependency.agent_id.is_empty() {
                return Err(format!(
                    "Daily Agent '{}' dependency agent_id cannot be empty",
                    agent.id
                ));
            }
            if dependency.agent_id == agent.id {
                return Err(format!(
                    "Daily Agent '{}' cannot depend on itself",
                    agent.id
                ));
            }
            if !known_ids.contains(dependency.agent_id.as_str()) {
                return Err(format!(
                    "Daily Agent '{}' depends on unknown agent '{}'",
                    agent.id, dependency.agent_id
                ));
            }
            if !seen_dependencies.insert(dependency.agent_id.as_str()) {
                return Err(format!(
                    "Daily Agent '{}' has duplicate dependency '{}'",
                    agent.id, dependency.agent_id
                ));
            }
        }
    }
    stable_topological_daily_agents(&agents)?;
    Ok(())
}

fn stable_topological_daily_agents(
    agents: &[AsrDailyAgentItem],
) -> Result<Vec<AsrDailyAgentItem>, String> {
    let mut ordered = Vec::with_capacity(agents.len());
    let mut emitted = HashSet::new();

    while ordered.len() < agents.len() {
        let mut progressed = false;
        for agent in agents {
            if emitted.contains(agent.id.as_str()) {
                continue;
            }
            if agent
                .dependencies
                .iter()
                .all(|dependency| emitted.contains(dependency.agent_id.as_str()))
            {
                emitted.insert(agent.id.clone());
                ordered.push(agent.clone());
                progressed = true;
            }
        }
        if !progressed {
            let blocked = agents
                .iter()
                .filter(|agent| !emitted.contains(agent.id.as_str()))
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Daily Agent dependency cycle detected among: {blocked}"
            ));
        }
    }

    Ok(ordered)
}

fn ordered_daily_agents(
    config: &AsrDailyAgentConfig,
) -> Result<Vec<AsrDailyAgentItem>, String> {
    let agents = normalized_daily_agents(config);
    validate_daily_agent_config(config)?;
    stable_topological_daily_agents(&agents)
}

fn task_for_daily_agent(task: &AsrDirectoryTask, agent: &AsrDailyAgentItem) -> AsrDirectoryTask {
    let inherited_report_sync_dir = task.daily_agent.report_sync_dir.clone();
    let inherited_terminology = task.daily_agent.terminology.clone();
    let mut task = task.clone();
    task.daily_agent = daily_agent_config_from_item(agent);
    task.daily_agent.terminology = normalize_daily_agent_terminology(inherited_terminology);
    if task.daily_agent.report_sync_dir.is_none() {
        task.daily_agent.report_sync_dir = inherited_report_sync_dir;
    }
    task
}

fn daily_agent_dependency_includes_output() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AsrDailyAgentDependency {
    pub agent_id: String,
    #[serde(default = "daily_agent_dependency_includes_output")]
    pub include_output: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDailyAgentDependencyFailurePolicy {
    #[default]
    Skip,
    Continue,
}

fn default_research_fanout_max_questions() -> usize {
    8
}

fn default_research_chatgpt_interface_mode() -> String {
    "chat".to_string()
}

fn default_research_chatgpt_model() -> String {
    "pro".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AsrDailyAgentResearchFanoutConfig {
    #[serde(default = "default_research_fanout_max_questions")]
    pub max_questions: usize,
    #[serde(default = "default_research_chatgpt_interface_mode")]
    pub chatgpt_interface_mode: String,
    #[serde(default = "default_research_chatgpt_model")]
    pub chatgpt_model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_runners: Vec<String>,
    #[serde(default, skip_serializing_if = "DailyAgentBTreeMap::is_empty")]
    pub context_profiles:
        DailyAgentBTreeMap<String, AsrDailyAgentResearchContextProfile>,
}

impl Default for AsrDailyAgentResearchFanoutConfig {
    fn default() -> Self {
        Self {
            max_questions: default_research_fanout_max_questions(),
            chatgpt_interface_mode: default_research_chatgpt_interface_mode(),
            chatgpt_model: default_research_chatgpt_model(),
            allowed_runners: Vec::new(),
            context_profiles: DailyAgentBTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AsrDailyAgentResearchContextProfile {
    pub runner: String,
    pub work_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

fn daily_agent_instruction_template(agent_id: &str) -> &'static str {
    match agent_id {
        DEFAULT_TOMORROW_TODO_AGENT_ID => DEFAULT_ASR_TOMORROW_TODO_AGENT_MD,
        DEFAULT_RESEARCH_SEED_AGENT_ID => DEFAULT_ASR_RESEARCH_SEED_AGENT_MD,
        DEFAULT_RESEARCH_DISPATCHER_AGENT_ID => DEFAULT_ASR_RESEARCH_DISPATCHER_AGENT_MD,
        _ => DEFAULT_ASR_DAILY_AGENTS_MD,
    }
}

fn daily_agent_instruction_content(task: &AsrDirectoryTask) -> String {
    daily_agent_instruction_template(&task.daily_agent.agent_id)
        .replace("{{task_name}}", &task.name)
        .replace("{{daily_dir}}", "./input")
        .replace(
            "{{report_dir}}",
            &format!("./output/{}/", task.daily_agent.output_dir),
        )
}

fn daily_agent_instructions_dir(task_id: &str) -> PathBuf {
    daily_dir_for_task(task_id).join("agents")
}

fn daily_agent_work_dir(task: &AsrDirectoryTask) -> PathBuf {
    daily_agent_instructions_dir(&task.id).join(&task.daily_agent.agent_id)
}

fn daily_agent_instructions_path(task: &AsrDirectoryTask) -> PathBuf {
    daily_agent_work_dir(task).join("AGENTS.md")
}

fn daily_agent_terms_path(task: &AsrDirectoryTask) -> PathBuf {
    daily_agent_work_dir(task).join(DAILY_AGENT_TERMS_FILENAME)
}

fn daily_agent_input_dir(task: &AsrDirectoryTask) -> PathBuf {
    daily_agent_work_dir(task).join("input")
}

fn daily_agent_upstream_input_dir(task: &AsrDirectoryTask, agent_id: &str) -> PathBuf {
    daily_agent_input_dir(task).join("upstream").join(agent_id)
}

fn daily_agent_output_root_dir(task: &AsrDirectoryTask) -> PathBuf {
    daily_agent_work_dir(task).join("output")
}

fn daily_agent_output_dir(task: &AsrDirectoryTask) -> PathBuf {
    daily_agent_output_root_dir(task).join(&task.daily_agent.output_dir)
}

fn daily_agent_source_copy_path(task: &AsrDirectoryTask, date: &str) -> PathBuf {
    daily_agent_input_dir(task).join(format!("{date}.md"))
}

fn daily_agent_processed_key(task: &AsrDirectoryTask, date: &str) -> String {
    format!("{}:{date}", task.daily_agent.agent_id)
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AsrDailyAgentReportSyncResult {
    pub target_dir: String,
    pub total_files: usize,
    pub copied_files: usize,
    pub skipped_files: usize,
    pub failed_files: usize,
    pub synced_at_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}
