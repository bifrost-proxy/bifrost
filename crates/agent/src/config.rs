//! Agent configuration with Bifrost agent config.toml support.
//!
//! Config loading order (later overrides earlier):
//! 1. `$BIFROST_DATA_DIR/agent/config.toml` (user-level, default `~/.bifrost/agent/`)
//! 2. `.bifrost/agent/config.toml` (project-level, in cwd)
//! 3. Environment variables for overrides

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Subdirectory under the Bifrost data dir for agent data.
const AGENT_SUBDIR: &str = "agent";
/// Config file name.
pub(crate) const CONFIG_FILENAME: &str = "config.toml";

// ---------------------------------------------------------------------------
// AgentConfig
// ---------------------------------------------------------------------------

/// Shared configuration for external runner sessions and history surfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Whether the agent is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default external runner selected from the runner registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<AgentRunnerMode>,

    // -- Model selection --
    /// Model name (e.g. "gpt-5.4-2026-03-05").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Provider ID (e.g., "openai", "aidp_crawl").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    // -- Prompt instructions --
    /// Base/system instructions forwarded to runners that support overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,

    /// Developer instructions appended as a separate developer section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,

    /// User-provided instructions combined with AGENTS.md project docs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_instructions: Option<String>,

    /// Reasoning effort passed to runners that support it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_reasoning_effort: Option<String>,

    /// Reasoning summary mode passed to runners that support it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_reasoning_summary: Option<String>,

    /// Context window used by shared external-runner status calculations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<i64>,

    // -- Skills --
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillsConfig>,

    // -- Project doc settings --
    /// Max bytes for AGENTS.md (default 32768).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_doc_max_bytes: Option<usize>,

    /// Fallback filenames for project doc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_doc_fallback_filenames: Option<Vec<String>>,

    // -- Runtime settings --
    /// Session TTL in seconds (default 3600).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ttl_secs: Option<u64>,

    // -- Working directory --
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,

    // -- History & Session --
    /// History persistence settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryConfig>,

    /// When true, session is not persisted on disk.
    #[serde(default)]
    pub ephemeral: bool,
}

/// How a message binding resolves its target inside a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTargetMode {
    /// Send back to the inbound conversation/thread (for example a Feishu chat_id).
    SourceThread,
    /// Send back to the inbound sender/user.
    SourceUser,
    /// Send to the provider owner configured on the IM provider.
    Owner,
    /// Resolve `target_id` through the configured IM target store.
    ConfiguredTarget,
}

/// Serializable IM channel binding used by agent config and scheduled tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImMessageChannelBinding {
    pub provider_id: String,
    pub target_id: String,
    #[serde(default = "default_message_target_mode")]
    pub target_mode: MessageTargetMode,
}

/// Runtime selected for IM agent messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunnerMode {
    /// A runner ID from the external runner registry.
    Custom(String),
}

impl AgentRunnerMode {
    pub fn custom_runner_id(&self) -> Option<&str> {
        match self {
            Self::Custom(id) => Some(id.as_str()),
        }
    }

    pub fn is_custom_runner(&self) -> bool {
        matches!(self, Self::Custom(_))
    }
}

impl Serialize for AgentRunnerMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Custom(id) => serializer.serialize_str(id),
        }
    }
}

impl<'de> Deserialize<'de> for AgentRunnerMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let trimmed = value.trim();
        if trimmed.is_empty() || is_removed_builtin_runner_id(trimmed) {
            return Err(serde::de::Error::custom(
                "the built-in Bifrost Agent runner has been removed; select an external runner",
            ));
        }
        Ok(Self::Custom(trimmed.to_string()))
    }
}

fn is_removed_builtin_runner_id(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized == "bifrost_agent"
        || normalized == "bifrost agent"
        || normalized == "builtin"
        || normalized == "bifrost"
}

fn default_message_target_mode() -> MessageTargetMode {
    MessageTargetMode::ConfiguredTarget
}

// ---------------------------------------------------------------------------
// SkillsConfig
// ---------------------------------------------------------------------------

/// Skills configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Whether to include skill instructions in system prompt (default true).
    #[serde(default = "default_true")]
    pub include_instructions: bool,
    /// Per-skill enable/disable configuration.
    #[serde(default)]
    pub config: Vec<SkillConfigEntry>,
}

/// Per-skill configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfigEntry {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// History Configuration
// ---------------------------------------------------------------------------

/// History persistence mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryPersistence {
    /// Save all history entries to disk.
    #[default]
    SaveAll,
    /// Keep only history entries from the last 90 days.
    Last90Days,
    /// Do not write history to disk.
    None,
}

/// Settings that govern if and what will be written to history file.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct HistoryConfig {
    /// Persistence mode for history entries.
    #[serde(default)]
    pub persistence: HistoryPersistence,
    /// Maximum size of the history file in bytes. Oldest entries are dropped
    /// once the file exceeds this limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

/// Default values as associated constants for easy access.
impl AgentConfig {
    pub const DEFAULT_SESSION_TTL: u64 = 3600;
    pub const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 32768;
    pub const DEFAULT_MODEL: &'static str = "gpt-5.4-2026-03-05";
    pub const DEFAULT_MODEL_CONTEXT_WINDOW: i64 = 250_000;
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            runner: Some(AgentRunnerMode::Custom("Codex".to_string())),
            model: Some(AgentConfig::DEFAULT_MODEL.to_string()),
            model_provider: None,
            base_instructions: None,
            developer_instructions: None,
            user_instructions: None,
            model_reasoning_effort: Some("medium".to_string()),
            model_reasoning_summary: Some("auto".to_string()),
            model_context_window: Some(Self::DEFAULT_MODEL_CONTEXT_WINDOW),
            skills: None,
            project_doc_max_bytes: Some(Self::DEFAULT_PROJECT_DOC_MAX_BYTES),
            project_doc_fallback_filenames: None,
            session_ttl_secs: Some(Self::DEFAULT_SESSION_TTL),
            work_dir: None,
            history: None,
            ephemeral: false,
        }
    }
}

impl AgentConfig {
    // -- Accessor methods with defaults --

    pub fn get_model(&self) -> &str {
        self.model.as_deref().unwrap_or(Self::DEFAULT_MODEL)
    }

    pub fn get_session_ttl_secs(&self) -> u64 {
        self.session_ttl_secs.unwrap_or(Self::DEFAULT_SESSION_TTL)
    }

    pub fn get_project_doc_max_bytes(&self) -> usize {
        self.project_doc_max_bytes
            .unwrap_or(Self::DEFAULT_PROJECT_DOC_MAX_BYTES)
    }

    /// Get history configuration (returns default if not set).
    pub fn get_history_config(&self) -> HistoryConfig {
        self.history.clone().unwrap_or_default()
    }

    /// Check if session is ephemeral (not persisted).
    pub fn is_ephemeral(&self) -> bool {
        self.ephemeral
    }

    /// Resolve the working directory path.
    pub fn resolve_work_dir(&self) -> PathBuf {
        self.work_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
    }
}

// ---------------------------------------------------------------------------
// Config loading (TOML files)
// ---------------------------------------------------------------------------

/// Determine the agent home directory.
///
/// Resolution order:
/// 1. `$BIFROST_DATA_DIR/agent/` when `BIFROST_DATA_DIR` is set
/// 2. `$HOME/.bifrost/agent/` (default)
pub fn agent_home_dir() -> PathBuf {
    if let Ok(data_dir) = std::env::var("BIFROST_DATA_DIR") {
        return PathBuf::from(data_dir).join(AGENT_SUBDIR);
    }
    dirs_home()
        .map(|h| h.join(".bifrost").join(AGENT_SUBDIR))
        .unwrap_or_else(|| PathBuf::from(".bifrost").join(AGENT_SUBDIR))
}

/// Load configuration by merging user-level and project-level configs.
pub fn load_config(work_dir: Option<&Path>) -> AgentConfig {
    let mut config = AgentConfig::default();

    // 1. User-level config: $BIFROST_DATA_DIR/agent/config.toml
    let home = agent_home_dir();
    let user_config_path = home.join(CONFIG_FILENAME);
    if let Some(user_cfg) = load_toml_config(&user_config_path) {
        config = merge_config(config, user_cfg);
        debug!(path = %user_config_path.display(), "loaded user-level config");
    }

    // 2. Project-level config: .bifrost/agent/config.toml
    let project_dir = work_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let project_config_path = project_dir
        .join(".bifrost")
        .join(AGENT_SUBDIR)
        .join(CONFIG_FILENAME);
    if let Some(proj_cfg) = load_toml_config(&project_config_path) {
        config = merge_config(config, proj_cfg);
        debug!(path = %project_config_path.display(), "loaded project-level config");
    }

    // 3. Environment variable overrides
    apply_env_overrides(&mut config);

    config
}

/// Load a TOML config file, returning None if it doesn't exist or can't be parsed.
fn load_toml_config(path: &Path) -> Option<AgentConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    match toml::from_str::<AgentConfig>(&content) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to parse config.toml");
            None
        }
    }
}

/// Merge `overlay` into `base`. Non-None fields in overlay override base.
fn merge_config(base: AgentConfig, overlay: AgentConfig) -> AgentConfig {
    AgentConfig {
        enabled: overlay.enabled,
        runner: overlay.runner.or(base.runner),
        model: overlay.model.or(base.model),
        model_provider: overlay.model_provider.or(base.model_provider),
        base_instructions: overlay.base_instructions.or(base.base_instructions),
        developer_instructions: overlay
            .developer_instructions
            .or(base.developer_instructions),
        user_instructions: overlay.user_instructions.or(base.user_instructions),
        model_reasoning_effort: overlay
            .model_reasoning_effort
            .or(base.model_reasoning_effort),
        model_reasoning_summary: overlay
            .model_reasoning_summary
            .or(base.model_reasoning_summary),
        model_context_window: overlay.model_context_window.or(base.model_context_window),
        skills: overlay.skills.or(base.skills),
        project_doc_max_bytes: overlay.project_doc_max_bytes.or(base.project_doc_max_bytes),
        project_doc_fallback_filenames: overlay
            .project_doc_fallback_filenames
            .or(base.project_doc_fallback_filenames),
        session_ttl_secs: overlay.session_ttl_secs.or(base.session_ttl_secs),
        work_dir: overlay.work_dir.or(base.work_dir),
        history: overlay.history.or(base.history),
        ephemeral: overlay.ephemeral || base.ephemeral,
    }
}

/// Apply environment variable overrides.
fn apply_env_overrides(config: &mut AgentConfig) {
    if let Ok(model) = std::env::var("BIFROST_AGENT_MODEL") {
        config.model = Some(model);
    }
    if let Ok(provider) = std::env::var("BIFROST_AGENT_PROVIDER") {
        config.model_provider = Some(provider);
    }
    if let Ok(dir) = std::env::var("BIFROST_AGENT_WORK_DIR") {
        config.work_dir = Some(dir);
    }
}

/// Get the user's home directory.
fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// Returns the user's home directory (e.g. `~`).
/// Used as the root for user-scoped agent paths.
pub fn user_home_dir() -> PathBuf {
    dirs_home().unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// AgentConfigStore (JSON runtime state)
// ---------------------------------------------------------------------------

const AGENT_CONFIG_FILE: &str = "agent_config.json";

/// Persistent config store (JSON file for runtime state).
pub struct AgentConfigStore {
    file_path: PathBuf,
}

impl AgentConfigStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            file_path: data_dir.join(AGENT_CONFIG_FILE),
        }
    }

    pub fn load(&self) -> AgentConfig {
        // Start with TOML config as base (user-level + project-level)
        let base = load_config(None);

        // Overlay with JSON runtime state if it exists
        let config = match std::fs::read_to_string(&self.file_path) {
            Ok(content) => match serde_json::from_str::<AgentConfig>(&content) {
                Ok(json_config) => merge_config(base, json_config),
                Err(e) => {
                    warn!(error = %e, "failed to parse agent config JSON, using TOML config");
                    base
                }
            },
            Err(_) => {
                // First run: persist default config so all values are initialized
                if let Err(e) = self.save(&base) {
                    warn!(error = %e, "failed to initialize default agent config");
                }
                base
            }
        };
        config
    }

    pub fn save(&self, config: &AgentConfig) -> Result<(), String> {
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| format!("failed to serialize config: {e}"))?;
        if let Some(parent) = self.file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&self.file_path, json)
            .map_err(|e| format!("failed to write config file: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_default_config() {
        let config = AgentConfig::default();
        assert!(config.enabled);
        assert_eq!(config.get_model(), "gpt-5.4-2026-03-05");
        assert_eq!(config.get_session_ttl_secs(), 3600);
        assert_eq!(config.get_project_doc_max_bytes(), 32768);
    }

    #[test]
    fn test_merge_config() {
        let base = AgentConfig::default();
        let overlay = AgentConfig {
            model: Some("custom-model".to_string()),
            work_dir: Some("/tmp/external-runner".to_string()),
            ..Default::default()
        };
        let merged = merge_config(base, overlay);
        assert_eq!(merged.get_model(), "custom-model");
        assert_eq!(merged.work_dir.as_deref(), Some("/tmp/external-runner"));
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
enabled = true
model = "gpt-4o"
model_provider = "openai"
work_dir = "/tmp/external-runner"
"#;
        let config: AgentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.get_model(), "gpt-4o");
        assert_eq!(config.model_provider.as_deref(), Some("openai"));
        assert_eq!(config.work_dir.as_deref(), Some("/tmp/external-runner"));
    }

    #[test]
    fn test_config_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentConfigStore::new(dir.path());
        let config = AgentConfig {
            model: Some("test-model".to_string()),
            ..Default::default()
        };
        store.save(&config).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.get_model(), "test-model");
    }

    #[test]
    fn test_config_store_load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentConfigStore::new(dir.path());
        let config = store.load();
        assert_eq!(config.get_model(), "gpt-5.4-2026-03-05");
    }

    #[test]
    fn test_agent_home_dir_default() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // Clear env to test default path
        std::env::remove_var("BIFROST_DATA_DIR");
        let home = agent_home_dir();
        // Default path should be ~/.bifrost/agent (unified with Bifrost data dir)
        assert!(home.to_string_lossy().contains(".bifrost"));
        assert!(home.to_string_lossy().contains("agent"));
    }

    #[test]
    fn default_runner_is_external_codex() {
        assert_eq!(
            AgentConfig::default().runner,
            Some(AgentRunnerMode::Custom("Codex".to_string()))
        );
    }

    #[test]
    fn removed_builtin_runner_aliases_are_rejected() {
        for value in ["", "bifrost_agent", "Bifrost Agent", "builtin", "bifrost"] {
            let encoded = serde_json::to_string(value).expect("encode runner");
            assert!(
                serde_json::from_str::<AgentRunnerMode>(&encoded).is_err(),
                "removed runner alias should be rejected: {value:?}"
            );
        }
    }
}
