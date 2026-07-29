// ─── Daily Agent durable artifact state ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrDailyAgentProcessedState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub documents: DailyAgentBTreeMap<String, AsrDailyAgentProcessedDocument>,
    #[serde(default)]
    pub date_watermarks: DailyAgentBTreeMap<String, String>,
    #[serde(default)]
    pub artifacts: DailyAgentBTreeMap<String, AsrDailyAgentArtifactState>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AsrDailyAgentArtifactState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_len_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_contract_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "DailyAgentBTreeMap::is_empty")]
    pub upstream_sha256: DailyAgentBTreeMap<String, String>,
}

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

fn daily_agent_config_sha256(task: &AsrDirectoryTask) -> String {
    let value = serde_json::json!({
        "agent_id": task.daily_agent.agent_id,
        "name": task.daily_agent.name,
        "runner": task.daily_agent.runner,
        "timeout_ms": task.daily_agent.timeout_ms,
        "instructions_source": task.daily_agent.instructions_source,
        "instructions": task.daily_agent.instructions,
        "output_dir": task.daily_agent.output_dir,
        "dependencies": task.daily_agent.dependencies,
        "dependency_failure_policy": task.daily_agent.dependency_failure_policy,
        "research_fanout": task.daily_agent.research_fanout,
        "terminology": task.daily_agent.terminology,
    });
    compute_sha256_of_bytes(&serde_json::to_vec(&value).unwrap_or_default())
}

fn daily_agent_upstream_sha256(
    task: &AsrDirectoryTask,
    date: &str,
) -> DailyAgentBTreeMap<String, String> {
    task.daily_agent
        .dependencies
        .iter()
        .filter(|dependency| dependency.include_output)
        .filter_map(|dependency| {
            let path = daily_agent_upstream_input_dir(task, &dependency.agent_id)
                .join(format!("{date}-report.md"));
            compute_sha256(&path)
                .ok()
                .map(|hash| (dependency.agent_id.clone(), hash))
        })
        .collect()
}

fn daily_agent_processed_artifacts_match(
    artifact: Option<&AsrDailyAgentArtifactState>,
    report_target: &str,
    agent_config_sha256: &str,
    upstream_sha256: &DailyAgentBTreeMap<String, String>,
) -> bool {
    let Some(artifact) = artifact else {
        return false;
    };
    if artifact.generator_contract_version != Some(DAILY_AGENT_GENERATOR_CONTRACT_VERSION)
        || artifact.agent_config_sha256.as_deref() != Some(agent_config_sha256)
        || &artifact.upstream_sha256 != upstream_sha256
    {
        return false;
    }
    let report_path = Path::new(report_target);
    let Ok(report_hash) = compute_sha256(report_path) else {
        return false;
    };
    let report_len = std::fs::metadata(report_path).map(|meta| meta.len()).ok();
    artifact.report_sha256.as_deref() == Some(report_hash.as_str())
        && artifact.report_len_bytes == report_len
}
