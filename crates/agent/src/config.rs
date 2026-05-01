//! Agent configuration with Codex-compatible config.toml support.
//!
//! Config loading order (later overrides earlier):
//! 1. `$BIFROST_DATA_DIR/agent/config.toml` (user-level, default `~/.bifrost/agent/`)
//! 2. `.bifrost/agent/config.toml` (project-level, in cwd)
//! 3. Environment variables for overrides

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Subdirectory under the Bifrost data dir for agent data.
const AGENT_SUBDIR: &str = "agent";
/// Config file name.
const CONFIG_FILENAME: &str = "config.toml";
/// Environment variable for overriding agent home (legacy compat).
const AGENT_HOME_ENV: &str = "BIFROST_AGENT_HOME";
/// Legacy home directory name (for migration detection).
const LEGACY_AGENT_HOME_DIR: &str = ".bifrost-agent";

// ---------------------------------------------------------------------------
// AgentConfig
// ---------------------------------------------------------------------------

/// Agent configuration for the model API and runtime behavior.
/// Compatible subset of Codex's ConfigToml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Whether the agent is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    // -- Model selection --
    /// Model name (e.g. "gpt-5.4-2026-03-05").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Provider ID (e.g., "openai", "aidp_crawl").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// Custom model providers (Codex-compatible format).
    #[serde(default)]
    pub model_providers: HashMap<String, ModelProviderConfig>,

    // -- System instructions --
    /// Instructions text (Codex's "instructions" field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,

    // -- Model parameters --
    /// Reasoning effort ("low", "medium", "high").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_reasoning_effort: Option<String>,

    /// Reasoning summary mode ("auto", "concise", "detailed").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_reasoning_summary: Option<String>,

    /// Context window size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<i64>,

    /// Auto compact token threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_auto_compact_token_limit: Option<i64>,

    /// Max tokens for completion response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,

    // -- MCP servers --
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,

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
    /// Shell command timeout in seconds (default 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_timeout_secs: Option<u64>,

    /// Max turn iterations (default 20).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turn_iterations: Option<u32>,

    /// Max history messages per session (default 50).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_history_messages: Option<u32>,

    /// Session TTL in seconds (default 3600).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ttl_secs: Option<u64>,

    /// Max tool output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output_token_limit: Option<usize>,

    /// Request timeout in seconds (default 120).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout_secs: Option<u64>,

    // -- Working directory --
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,

    // -- History & Session (Codex-compatible) --
    /// History persistence settings (Codex's `history` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryConfig>,

    /// When true, session is not persisted on disk (Codex's `ephemeral` field).
    #[serde(default)]
    pub ephemeral: bool,

    // -- Memories subsystem (Codex-compatible) --
    /// Memories subsystem settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memories: Option<MemoriesConfig>,

    // -- Background terminal --
    /// Maximum poll window for background terminal output (write_stdin), in ms.
    /// Default: 300000 (5 minutes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_terminal_max_timeout: Option<u64>,
}

// ---------------------------------------------------------------------------
// ModelProviderConfig
// ---------------------------------------------------------------------------

/// Configuration for a model provider (based on Codex's ModelProviderInfo).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    pub name: Option<String>,
    pub base_url: Option<String>,
    /// Environment variable name for the API key.
    pub env_key: Option<String>,
    /// Static HTTP headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_headers: Option<HashMap<String, String>>,
    /// Header name → environment variable name (value resolved at runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_http_headers: Option<HashMap<String, String>>,
    /// Max retries for requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_max_retries: Option<u64>,
    /// Stream idle timeout in ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_ms: Option<u64>,
    /// Max retries for stream reconnection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_max_retries: Option<u64>,
}

// ---------------------------------------------------------------------------
// McpServerConfig
// ---------------------------------------------------------------------------

/// Configuration for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    // Stdio transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    // HTTP transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,

    // Common
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Startup timeout in seconds (default 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_timeout_sec: Option<u64>,
    /// Tool call timeout in seconds (default 60).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_timeout_sec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
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
// History Configuration (Codex-compatible)
// ---------------------------------------------------------------------------

/// History persistence mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryPersistence {
    /// Save all history entries to disk.
    #[default]
    SaveAll,
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
// Memories Configuration (Codex-compatible)
// ---------------------------------------------------------------------------

/// Memories subsystem settings (compatible with Codex's MemoriesToml).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MemoriesConfig {
    /// When `true`, external context sources mark the thread `memory_mode` as polluted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_on_external_context: Option<bool>,
    /// When `false`, newly created threads are stored with `memory_mode = "disabled"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_memories: Option<bool>,
    /// When `false`, skip injecting memory usage instructions into developer prompts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_memories: Option<bool>,
    /// Maximum number of recent raw memories retained for global consolidation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_raw_memories_for_consolidation: Option<usize>,
    /// Maximum number of days since a memory was last used before it becomes ineligible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_unused_days: Option<i64>,
    /// Maximum age of the threads used for memories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rollout_age_days: Option<i64>,
    /// Maximum number of rollout candidates processed per pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rollouts_per_startup: Option<usize>,
    /// Minimum idle time between last thread activity and memory creation (hours).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_rollout_idle_hours: Option<i64>,
    /// Model used for thread summarisation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extract_model: Option<String>,
    /// Model used for memory consolidation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation_model: Option<String>,
}

// ---------------------------------------------------------------------------
// EffectiveModelConfig (resolved from provider)
// ---------------------------------------------------------------------------

/// Resolved model configuration ready for use by the client.
#[derive(Debug, Clone)]
pub struct EffectiveModelConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub max_completion_tokens: u32,
    pub reasoning_effort: Option<String>,
    pub reasoning_summary: Option<String>,
    pub request_timeout_secs: u64,
    pub extra_headers: HashMap<String, String>,
    pub use_azure_auth: bool,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

/// Default values as associated constants for easy access.
impl AgentConfig {
    pub const DEFAULT_MAX_COMPLETION_TOKENS: u32 = 16384;
    pub const DEFAULT_MAX_HISTORY: u32 = 50;
    pub const DEFAULT_SESSION_TTL: u64 = 3600;
    pub const DEFAULT_REQUEST_TIMEOUT: u64 = 120;
    pub const DEFAULT_SHELL_TIMEOUT: u64 = 30;
    pub const DEFAULT_MAX_TURN_ITERATIONS: u32 = 20;
    pub const DEFAULT_COMPACT_THRESHOLD: u32 = 80000;
    pub const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 32768;
    pub const DEFAULT_MODEL: &'static str = "gpt-5.4-2026-03-05";
    pub const DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
    /// Default tool output token limit (matching Codex DEFAULT_MAX_OUTPUT_TOKENS).
    pub const DEFAULT_TOOL_OUTPUT_TOKEN_LIMIT: usize = 10_000;
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: Some(AgentConfig::DEFAULT_MODEL.to_string()),
            model_provider: Some("aidp_crawl".to_string()),
            model_providers: HashMap::new(),
            instructions: None,
            model_reasoning_effort: Some("medium".to_string()),
            model_reasoning_summary: Some("auto".to_string()),
            model_context_window: Some(200_000),
            model_auto_compact_token_limit: Some(Self::DEFAULT_COMPACT_THRESHOLD as i64),
            max_completion_tokens: Some(Self::DEFAULT_MAX_COMPLETION_TOKENS),
            mcp_servers: HashMap::new(),
            skills: None,
            project_doc_max_bytes: Some(Self::DEFAULT_PROJECT_DOC_MAX_BYTES),
            project_doc_fallback_filenames: None,
            shell_timeout_secs: Some(Self::DEFAULT_SHELL_TIMEOUT),
            max_turn_iterations: Some(Self::DEFAULT_MAX_TURN_ITERATIONS),
            max_history_messages: Some(Self::DEFAULT_MAX_HISTORY),
            session_ttl_secs: Some(Self::DEFAULT_SESSION_TTL),
            tool_output_token_limit: Some(Self::DEFAULT_TOOL_OUTPUT_TOKEN_LIMIT),
            request_timeout_secs: Some(Self::DEFAULT_REQUEST_TIMEOUT),
            work_dir: None,
            history: None,
            ephemeral: false,
            memories: None,
            background_terminal_max_timeout: Some(Self::DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS),
        }
    }
}

impl AgentConfig {
    // -- Accessor methods with defaults --

    pub fn get_model(&self) -> &str {
        self.model.as_deref().unwrap_or(Self::DEFAULT_MODEL)
    }

    pub fn get_max_completion_tokens(&self) -> u32 {
        self.max_completion_tokens
            .unwrap_or(Self::DEFAULT_MAX_COMPLETION_TOKENS)
    }

    pub fn get_max_history_messages(&self) -> u32 {
        self.max_history_messages
            .unwrap_or(Self::DEFAULT_MAX_HISTORY)
    }

    pub fn get_session_ttl_secs(&self) -> u64 {
        self.session_ttl_secs.unwrap_or(Self::DEFAULT_SESSION_TTL)
    }

    pub fn get_request_timeout_secs(&self) -> u64 {
        self.request_timeout_secs
            .unwrap_or(Self::DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn get_shell_timeout_secs(&self) -> u64 {
        self.shell_timeout_secs
            .unwrap_or(Self::DEFAULT_SHELL_TIMEOUT)
    }

    pub fn get_max_turn_iterations(&self) -> u32 {
        self.max_turn_iterations
            .unwrap_or(Self::DEFAULT_MAX_TURN_ITERATIONS)
    }

    pub fn get_compact_threshold_tokens(&self) -> u32 {
        self.model_auto_compact_token_limit
            .map(|v| v as u32)
            .unwrap_or(Self::DEFAULT_COMPACT_THRESHOLD)
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

    /// Get memories configuration (returns default if not set).
    pub fn get_memories_config(&self) -> MemoriesConfig {
        self.memories.clone().unwrap_or_default()
    }

    /// Get background terminal max timeout in milliseconds.
    pub fn get_background_terminal_max_timeout(&self) -> u64 {
        self.background_terminal_max_timeout
            .unwrap_or(Self::DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS)
    }

    /// Resolve the working directory path.
    pub fn resolve_work_dir(&self) -> PathBuf {
        self.work_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
    }

    /// Resolve the effective model configuration by looking up the provider.
    ///
    /// User-defined providers are merged field-by-field with built-in defaults:
    /// if a user-defined field is `None`, the built-in value is used as fallback.
    pub fn resolve_effective_config(&self) -> Result<EffectiveModelConfig, String> {
        let provider_id = self.model_provider.as_deref().unwrap_or("aidp_crawl");

        // Get built-in provider as the base
        let builtin = get_builtin_provider(provider_id);

        // Merge user-defined provider on top of built-in (field-by-field)
        let provider = if let Some(user_provider) = self.model_providers.get(provider_id) {
            ModelProviderConfig {
                name: user_provider.name.clone().or(builtin.name),
                base_url: user_provider.base_url.clone().or(builtin.base_url),
                env_key: user_provider.env_key.clone().or(builtin.env_key),
                http_headers: user_provider.http_headers.clone().or(builtin.http_headers),
                env_http_headers: user_provider
                    .env_http_headers
                    .clone()
                    .or(builtin.env_http_headers),
                request_max_retries: user_provider
                    .request_max_retries
                    .or(builtin.request_max_retries),
                stream_idle_timeout_ms: user_provider
                    .stream_idle_timeout_ms
                    .or(builtin.stream_idle_timeout_ms),
                stream_max_retries: user_provider
                    .stream_max_retries
                    .or(builtin.stream_max_retries),
            }
        } else {
            builtin
        };

        let base_url = provider
            .base_url
            .ok_or_else(|| format!("provider '{}' has no base_url", provider_id))?;

        // Resolve API key from env
        let env_key = provider.env_key.as_deref().unwrap_or("OPENAI_API_KEY");
        let api_key = std::env::var(env_key).unwrap_or_default();

        // Resolve headers
        let mut extra_headers = HashMap::new();
        if let Some(ref static_headers) = provider.http_headers {
            extra_headers.extend(static_headers.clone());
        }
        if let Some(ref env_headers) = provider.env_http_headers {
            for (header_name, env_var) in env_headers {
                if let Ok(val) = std::env::var(env_var) {
                    extra_headers.insert(header_name.clone(), val);
                }
            }
        }

        // Determine auth style
        let use_azure_auth = extra_headers.contains_key("api-key");

        Ok(EffectiveModelConfig {
            model: self.get_model().to_string(),
            base_url,
            api_key,
            max_completion_tokens: self.get_max_completion_tokens(),
            reasoning_effort: self.model_reasoning_effort.clone(),
            reasoning_summary: self.model_reasoning_summary.clone(),
            request_timeout_secs: self.get_request_timeout_secs(),
            extra_headers,
            use_azure_auth,
        })
    }

    /// Resolve the API key (backward-compatible convenience method).
    pub fn resolve_api_key(&self) -> Result<String, String> {
        let effective = self.resolve_effective_config()?;
        if effective.api_key.is_empty() {
            let provider_id = self.model_provider.as_deref().unwrap_or("aidp_crawl");
            let builtin = get_builtin_provider(provider_id);
            let env_key = if let Some(user_p) = self.model_providers.get(provider_id) {
                user_p
                    .env_key
                    .as_deref()
                    .or(builtin.env_key.as_deref())
                    .unwrap_or("OPENAI_API_KEY")
            } else {
                builtin.env_key.as_deref().unwrap_or("OPENAI_API_KEY")
            };
            Err(format!("environment variable '{}' not set", env_key))
        } else {
            Ok(effective.api_key)
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in providers
// ---------------------------------------------------------------------------

/// Get a built-in provider config by ID.
fn get_builtin_provider(id: &str) -> ModelProviderConfig {
    match id {
        // OpenAI - official API
        "openai" => ModelProviderConfig {
            name: Some("OpenAI".to_string()),
            base_url: Some("https://api.openai.com/v1/chat/completions".to_string()),
            env_key: Some("OPENAI_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: Some({
                let mut m = HashMap::new();
                m.insert(
                    "OpenAI-Organization".to_string(),
                    "OPENAI_ORGANIZATION".to_string(),
                );
                m.insert("OpenAI-Project".to_string(), "OPENAI_PROJECT".to_string());
                m
            }),
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // AIDP Crawl - ByteDance internal
        "aidp_crawl" => ModelProviderConfig {
            name: Some("AIDP Crawl".to_string()),
            base_url: Some(
                "https://search.bytedance.net/gpt/openapi/online/multimodal/crawl".to_string(),
            ),
            env_key: Some("MODELHUB_AK".to_string()),
            http_headers: None,
            env_http_headers: Some({
                let mut m = HashMap::new();
                m.insert("api-key".to_string(), "MODELHUB_AK".to_string());
                m.insert("X-TT-LOGID".to_string(), "MODELHUB_LOGID".to_string());
                m
            }),
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Azure OpenAI - Microsoft Azure
        "azure" => ModelProviderConfig {
            name: Some("Azure OpenAI".to_string()),
            base_url: None, // User must provide: https://<resource>.openai.azure.com/openai/deployments/<deployment>/chat/completions?api-version=...
            env_key: Some("AZURE_OPENAI_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: Some({
                let mut m = HashMap::new();
                m.insert("api-key".to_string(), "AZURE_OPENAI_API_KEY".to_string());
                m
            }),
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Anthropic - Claude API (OpenAI-compatible endpoint)
        "anthropic" => ModelProviderConfig {
            name: Some("Anthropic".to_string()),
            base_url: Some("https://api.anthropic.com/v1/chat/completions".to_string()),
            env_key: Some("ANTHROPIC_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: Some({
                let mut m = HashMap::new();
                m.insert("x-api-key".to_string(), "ANTHROPIC_API_KEY".to_string());
                m
            }),
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Google Gemini - via OpenAI-compatible endpoint
        "gemini" => ModelProviderConfig {
            name: Some("Google Gemini".to_string()),
            base_url: Some(
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
                    .to_string(),
            ),
            env_key: Some("GOOGLE_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: Some({
                let mut m = HashMap::new();
                m.insert("Authorization".to_string(), "GOOGLE_API_KEY".to_string());
                m
            }),
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Groq - fast inference
        "groq" => ModelProviderConfig {
            name: Some("Groq".to_string()),
            base_url: Some("https://api.groq.com/openai/v1/chat/completions".to_string()),
            env_key: Some("GROQ_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // DeepSeek - Chinese LLM provider
        "deepseek" => ModelProviderConfig {
            name: Some("DeepSeek".to_string()),
            base_url: Some("https://api.deepseek.com/v1/chat/completions".to_string()),
            env_key: Some("DEEPSEEK_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Ollama - local inference (default port 11434)
        "ollama" => ModelProviderConfig {
            name: Some("Ollama".to_string()),
            base_url: Some("http://localhost:11434/v1/chat/completions".to_string()),
            env_key: None, // No API key needed for local
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
        // LM Studio - local inference (default port 1234)
        "lmstudio" => ModelProviderConfig {
            name: Some("LM Studio".to_string()),
            base_url: Some("http://localhost:1234/v1/chat/completions".to_string()),
            env_key: None, // No API key needed for local
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
        // Amazon Bedrock - AWS managed
        "amazon-bedrock" => ModelProviderConfig {
            name: Some("Amazon Bedrock".to_string()),
            base_url: Some(
                "https://bedrock-mantle.us-east-1.api.aws/openai/v1/chat/completions".to_string(),
            ),
            env_key: None, // Uses AWS credentials chain
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // OpenRouter - unified API for many models
        "openrouter" => ModelProviderConfig {
            name: Some("OpenRouter".to_string()),
            base_url: Some("https://openrouter.ai/api/v1/chat/completions".to_string()),
            env_key: Some("OPENROUTER_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: Some({
                let mut m = HashMap::new();
                m.insert(
                    "HTTP-Referer".to_string(),
                    "OPENROUTER_SITE_URL".to_string(),
                );
                m.insert("X-Title".to_string(), "OPENROUTER_SITE_NAME".to_string());
                m
            }),
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // xAI - Grok API
        "xai" => ModelProviderConfig {
            name: Some("xAI (Grok)".to_string()),
            base_url: Some("https://api.x.ai/v1/chat/completions".to_string()),
            env_key: Some("XAI_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Mistral AI
        "mistral" => ModelProviderConfig {
            name: Some("Mistral AI".to_string()),
            base_url: Some("https://api.mistral.ai/v1/chat/completions".to_string()),
            env_key: Some("MISTRAL_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Cerebras - ultra-fast inference
        "cerebras" => ModelProviderConfig {
            name: Some("Cerebras".to_string()),
            base_url: Some("https://api.cerebras.ai/v1/chat/completions".to_string()),
            env_key: Some("CEREBRAS_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(4),
            stream_idle_timeout_ms: Some(300_000),
            stream_max_retries: Some(5),
        },
        // Unknown provider - fallback
        _ => ModelProviderConfig {
            name: None,
            base_url: None,
            env_key: Some("OPENAI_API_KEY".to_string()),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    }
}

/// Get the list of all built-in provider IDs.
pub fn builtin_provider_ids() -> &'static [&'static str] {
    &[
        "openai",
        "aidp_crawl",
        "azure",
        "anthropic",
        "gemini",
        "groq",
        "deepseek",
        "ollama",
        "lmstudio",
        "amazon-bedrock",
        "openrouter",
        "xai",
        "mistral",
        "cerebras",
    ]
}

/// Provider info for API responses (lightweight version without sensitive data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: Option<String>,
    pub env_key: Option<String>,
}

/// Get all built-in providers as a list of ProviderInfo (for API/WebUI).
pub fn list_builtin_providers() -> Vec<ProviderInfo> {
    builtin_provider_ids()
        .iter()
        .map(|&id| {
            let cfg = get_builtin_provider(id);
            ProviderInfo {
                id: id.to_string(),
                name: cfg.name.clone().unwrap_or_else(|| id.to_string()),
                base_url: cfg.base_url.clone(),
                env_key: cfg.env_key.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Config loading (TOML files)
// ---------------------------------------------------------------------------

/// Determine the agent home directory.
///
/// Resolution order:
/// 1. `BIFROST_AGENT_HOME` env var (legacy compat)
/// 2. `$BIFROST_DATA_DIR/agent/` (preferred, unified with Bifrost data dir)
/// 3. `$HOME/.bifrost/agent/` (default)
pub fn agent_home_dir() -> PathBuf {
    // Legacy override via env var
    if let Ok(path) = std::env::var(AGENT_HOME_ENV) {
        return PathBuf::from(path);
    }
    // Prefer BIFROST_DATA_DIR if set
    if let Ok(data_dir) = std::env::var("BIFROST_DATA_DIR") {
        return PathBuf::from(data_dir).join(AGENT_SUBDIR);
    }
    // Default: ~/.bifrost/agent/
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
    } else {
        // Fallback: try legacy ~/.bifrost-agent/config.toml
        if let Some(legacy_home) = dirs_home() {
            let legacy_path = legacy_home
                .join(LEGACY_AGENT_HOME_DIR)
                .join(CONFIG_FILENAME);
            if let Some(legacy_cfg) = load_toml_config(&legacy_path) {
                config = merge_config(config, legacy_cfg);
                debug!(path = %legacy_path.display(), "loaded legacy user-level config");
            }
        }
    }

    // 2. Project-level config: .bifrost/agent/config.toml (or legacy .bifrost-agent/config.toml)
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
    } else {
        // Fallback: try legacy project .bifrost-agent/config.toml
        let legacy_project_path = project_dir
            .join(LEGACY_AGENT_HOME_DIR)
            .join(CONFIG_FILENAME);
        if let Some(proj_cfg) = load_toml_config(&legacy_project_path) {
            config = merge_config(config, proj_cfg);
            debug!(path = %legacy_project_path.display(), "loaded legacy project-level config");
        }
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
        model: overlay.model.or(base.model),
        model_provider: overlay.model_provider.or(base.model_provider),
        model_providers: {
            let mut merged = base.model_providers;
            merged.extend(overlay.model_providers);
            merged
        },
        instructions: overlay.instructions.or(base.instructions),
        model_reasoning_effort: overlay
            .model_reasoning_effort
            .or(base.model_reasoning_effort),
        model_reasoning_summary: overlay
            .model_reasoning_summary
            .or(base.model_reasoning_summary),
        model_context_window: overlay.model_context_window.or(base.model_context_window),
        model_auto_compact_token_limit: overlay
            .model_auto_compact_token_limit
            .or(base.model_auto_compact_token_limit),
        max_completion_tokens: overlay.max_completion_tokens.or(base.max_completion_tokens),
        mcp_servers: {
            let mut merged = base.mcp_servers;
            merged.extend(overlay.mcp_servers);
            merged
        },
        skills: overlay.skills.or(base.skills),
        project_doc_max_bytes: overlay.project_doc_max_bytes.or(base.project_doc_max_bytes),
        project_doc_fallback_filenames: overlay
            .project_doc_fallback_filenames
            .or(base.project_doc_fallback_filenames),
        shell_timeout_secs: overlay.shell_timeout_secs.or(base.shell_timeout_secs),
        max_turn_iterations: overlay.max_turn_iterations.or(base.max_turn_iterations),
        max_history_messages: overlay.max_history_messages.or(base.max_history_messages),
        session_ttl_secs: overlay.session_ttl_secs.or(base.session_ttl_secs),
        tool_output_token_limit: overlay
            .tool_output_token_limit
            .or(base.tool_output_token_limit),
        request_timeout_secs: overlay.request_timeout_secs.or(base.request_timeout_secs),
        work_dir: overlay.work_dir.or(base.work_dir),
        history: overlay.history.or(base.history),
        ephemeral: overlay.ephemeral || base.ephemeral,
        memories: overlay.memories.or(base.memories),
        background_terminal_max_timeout: overlay
            .background_terminal_max_timeout
            .or(base.background_terminal_max_timeout),
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
    if let Ok(timeout) = std::env::var("BIFROST_AGENT_SHELL_TIMEOUT") {
        if let Ok(v) = timeout.parse() {
            config.shell_timeout_secs = Some(v);
        }
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

// ---------------------------------------------------------------------------
// AgentConfigStore (JSON persistence - backward compat)
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

    #[test]
    fn test_default_config() {
        let config = AgentConfig::default();
        assert!(config.enabled);
        assert_eq!(config.get_model(), "gpt-5.4-2026-03-05");
        assert_eq!(config.get_max_completion_tokens(), 16384);
        assert_eq!(config.get_shell_timeout_secs(), 30);
    }

    #[test]
    fn test_resolve_effective_config_builtin_aidp() {
        // This test verifies structure; the API key won't be set in test env
        let config = AgentConfig::default();
        let effective = config.resolve_effective_config().unwrap();
        assert!(effective.base_url.contains("bytedance.net"));
        assert_eq!(effective.model, "gpt-5.4-2026-03-05");
    }

    #[test]
    fn test_resolve_effective_config_openai() {
        let config = AgentConfig {
            model_provider: Some("openai".to_string()),
            ..Default::default()
        };
        let effective = config.resolve_effective_config().unwrap();
        assert!(effective.base_url.contains("api.openai.com"));
    }

    #[test]
    fn test_resolve_effective_config_custom_provider() {
        let mut providers = HashMap::new();
        providers.insert(
            "custom".to_string(),
            ModelProviderConfig {
                name: Some("Custom".to_string()),
                base_url: Some("https://custom.example.com/v1/chat".to_string()),
                env_key: Some("CUSTOM_KEY".to_string()),
                http_headers: None,
                env_http_headers: None,
                request_max_retries: None,
                stream_idle_timeout_ms: None,
                stream_max_retries: None,
            },
        );
        let config = AgentConfig {
            model_provider: Some("custom".to_string()),
            model_providers: providers,
            ..Default::default()
        };
        let effective = config.resolve_effective_config().unwrap();
        assert_eq!(effective.base_url, "https://custom.example.com/v1/chat");
    }

    #[test]
    fn test_merge_config() {
        let base = AgentConfig::default();
        let overlay = AgentConfig {
            model: Some("custom-model".to_string()),
            shell_timeout_secs: Some(60),
            ..Default::default()
        };
        let merged = merge_config(base, overlay);
        assert_eq!(merged.get_model(), "custom-model");
        assert_eq!(merged.get_shell_timeout_secs(), 60);
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
enabled = true
model = "gpt-4o"
model_provider = "openai"
shell_timeout_secs = 45
max_turn_iterations = 30

[model_providers.custom]
name = "My Provider"
base_url = "https://example.com/api"
env_key = "MY_API_KEY"

[mcp_servers.test_server]
command = "node"
args = ["server.js"]
enabled = true
"#;
        let config: AgentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.get_model(), "gpt-4o");
        assert_eq!(config.model_provider.as_deref(), Some("openai"));
        assert_eq!(config.get_shell_timeout_secs(), 45);
        assert!(config.model_providers.contains_key("custom"));
        assert!(config.mcp_servers.contains_key("test_server"));
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
    fn test_builtin_provider_ids() {
        let ids = builtin_provider_ids();
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"aidp_crawl"));
    }

    #[test]
    fn test_agent_home_dir_default() {
        // Clear env to test default path
        std::env::remove_var(AGENT_HOME_ENV);
        std::env::remove_var("BIFROST_DATA_DIR");
        let home = agent_home_dir();
        // Default path should be ~/.bifrost/agent (unified with Bifrost data dir)
        assert!(home.to_string_lossy().contains(".bifrost"));
        assert!(home.to_string_lossy().contains("agent"));
    }

    #[test]
    fn test_provider_merge_null_fields_fallback_to_builtin() {
        // Simulates the case where agent_config.json has a user-defined provider
        // with null fields (base_url: null, env_key: null) that should NOT shadow
        // the built-in provider's values.
        //
        // Set MODELHUB_AK so that env_http_headers resolves "api-key" header,
        // which makes use_azure_auth == true.
        std::env::set_var("MODELHUB_AK", "test-key");
        let mut providers = HashMap::new();
        providers.insert(
            "aidp_crawl".to_string(),
            ModelProviderConfig {
                name: Some("aidp_crawl".to_string()),
                base_url: None, // null — should fall back to built-in
                env_key: None,  // null — should fall back to built-in
                http_headers: None,
                env_http_headers: None,
                request_max_retries: None,
                stream_idle_timeout_ms: None,
                stream_max_retries: None,
            },
        );
        let config = AgentConfig {
            model_provider: Some("aidp_crawl".to_string()),
            model_providers: providers,
            ..Default::default()
        };
        let effective = config.resolve_effective_config().unwrap();
        // Should use built-in base_url, not fail with "no base_url"
        assert!(effective.base_url.contains("bytedance.net"));
        assert!(effective.use_azure_auth);
    }

    #[test]
    fn test_provider_merge_user_override_takes_precedence() {
        // User-defined fields should override built-in when not null
        //
        // Set MODELHUB_AK so that env_http_headers resolves "api-key" header,
        // which makes use_azure_auth == true.
        std::env::set_var("MODELHUB_AK", "test-key");
        let mut providers = HashMap::new();
        providers.insert(
            "aidp_crawl".to_string(),
            ModelProviderConfig {
                name: None,
                base_url: Some("https://custom.example.com/api".to_string()),
                env_key: None, // falls back to built-in MODELHUB_AK
                http_headers: None,
                env_http_headers: None,
                request_max_retries: None,
                stream_idle_timeout_ms: None,
                stream_max_retries: None,
            },
        );
        let config = AgentConfig {
            model_provider: Some("aidp_crawl".to_string()),
            model_providers: providers,
            ..Default::default()
        };
        let effective = config.resolve_effective_config().unwrap();
        // User's base_url override should win
        assert_eq!(effective.base_url, "https://custom.example.com/api");
        // Built-in env_http_headers should still be present (api-key, X-TT-LOGID)
        assert!(effective.use_azure_auth);
    }
}
