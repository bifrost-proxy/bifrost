use std::collections::{BTreeMap, BTreeSet};

use bifrost_agent::{PlanStep, ToolCallLog};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Connection Handle (used by provider trait)
// ---------------------------------------------------------------------------

pub struct ConnectionHandle {
    pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl std::fmt::Debug for ConnectionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionHandle").finish()
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImProviderType {
    Feishu,
    Weixin,
    WeChat,
    Webhook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImProviderAgentConfig {
    /// Provider-specific default working directory for agent sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    /// Provider-specific base/system instructions override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    /// Provider-specific developer instructions override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    /// Provider-specific user instructions override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImProviderConfig {
    pub id: String,
    pub provider_type: ImProviderType,
    pub display_name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    /// The open_id of the bot owner. When set, only messages from this user are processed;
    /// messages from other users are rejected. Also used as the default send target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_open_id: Option<String>,
    #[serde(default)]
    pub event_connection_enabled: bool,
    #[serde(default)]
    pub event_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<ImProviderAgentConfig>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// Target
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImTarget {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub receive_id_type: String,
    pub receive_id: String,
    #[serde(default = "default_msg_type")]
    pub default_msg_type: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

fn default_msg_type() -> String {
    "interactive".to_string()
}

// ---------------------------------------------------------------------------
// Provider Policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImProviderPolicyStatus {
    Active,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImPermission {
    ReadStatus,
    SendMessage,
    ManageTarget,
    ManageRoute,
    ManageSchedule,
    ExecuteScript,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImScriptPolicyBinding {
    #[serde(default)]
    pub shell_policy_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_policy_id: Option<String>,
    #[serde(default)]
    pub allow_script_text: bool,
    #[serde(default)]
    pub allow_script_file: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImProviderPolicy {
    pub policy_id: String,
    pub provider_id: String,
    pub provider_type: ImProviderType,
    pub identity_key_type: String,
    pub identity_key_masked: String,
    pub status: ImProviderPolicyStatus,
    #[serde(default)]
    pub permissions: BTreeSet<ImPermission>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_policy_binding: Option<ImScriptPolicyBinding>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// Route
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImEventType {
    MessageReceive,
    AppMention,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImEventMatcher {
    #[serde(default)]
    pub chat_ids: Vec<String>,
    #[serde(default)]
    pub user_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyTarget {
    OriginalChat,
    SpecificTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMode {
    TextSummary,
    CardTemplate(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImRouteAction {
    RunScriptAndReply {
        #[serde(skip_serializing_if = "Option::is_none")]
        script_text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        script_file: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default = "default_reply_target")]
        reply_target: ReplyTarget,
        #[serde(default = "default_reply_mode")]
        reply_mode: ReplyMode,
    },
    /// Forward the message to an LLM agent and reply with the model's response.
    /// The global agent config (base_url, api_key, model) is loaded from ImAgentConfigStore.
    /// Per-route overrides (system_prompt, model) are specified here.
    AgentChat {
        /// Override the default system prompt for this route
        #[serde(skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        /// Override the default model for this route
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Where to send the reply
        #[serde(default = "default_reply_target")]
        reply_target: ReplyTarget,
    },
}

fn default_reply_target() -> ReplyTarget {
    ReplyTarget::OriginalChat
}

fn default_reply_mode() -> ReplyMode {
    ReplyMode::TextSummary
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImRoute {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub event_type: ImEventType,
    pub matcher: ImEventMatcher,
    pub action: ImRouteAction,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

fn default_timeout_ms() -> u64 {
    30_000
}

fn default_max_output_bytes() -> u64 {
    1_048_576 // 1 MB
}

// ---------------------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleTrigger {
    Cron {
        expr: String,
        #[serde(default = "default_schedule_timezone")]
        timezone: String,
    },
    Interval {
        every_ms: u64,
    },
}

fn default_schedule_timezone() -> String {
    "UTC".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskScript {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl TaskScript {
    pub fn is_empty(&self) -> bool {
        self.script_text
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
            && self
                .script_file
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleTaskType {
    #[default]
    Script,
    Agent,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleAgentTask {
    /// Preset prompt sent to the agent when the schedule fires.
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyPolicy {
    Forbid,
    #[default]
    SkipIfRunning,
    QueueOne,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImSchedule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub target_id: String,
    pub trigger: ScheduleTrigger,
    #[serde(default)]
    pub task_type: ScheduleTaskType,
    #[serde(default)]
    pub script: TaskScript,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ScheduleAgentTask>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
    #[serde(default)]
    pub concurrency_policy: ConcurrencyPolicy,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<u64>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl ImSchedule {
    pub fn infer_task_type(&mut self) {
        if self.task_type == ScheduleTaskType::Script
            && self
                .agent
                .as_ref()
                .is_some_and(|agent| !agent.prompt.trim().is_empty())
            && self.script.is_empty()
        {
            self.task_type = ScheduleTaskType::Agent;
        }
    }

    pub fn validate_for_save(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("schedule name cannot be empty".to_string());
        }
        match &self.trigger {
            ScheduleTrigger::Cron { expr, .. } if expr.trim().is_empty() => {
                return Err("cron expression cannot be empty".to_string());
            }
            ScheduleTrigger::Interval { every_ms } if *every_ms == 0 => {
                return Err("interval every_ms must be greater than 0".to_string());
            }
            _ => {}
        }

        match self.task_type {
            ScheduleTaskType::Script => {
                if self.target_id.trim().is_empty() {
                    return Err("script schedules require target_id".to_string());
                }
                if self.script.is_empty() {
                    return Err(
                        "script schedules require script.script_text or script.script_file"
                            .to_string(),
                    );
                }
            }
            ScheduleTaskType::Agent => {
                let prompt = self
                    .agent
                    .as_ref()
                    .map(|agent| agent.prompt.trim())
                    .unwrap_or_default();
                if prompt.is_empty() {
                    return Err("agent schedules require agent.prompt".to_string());
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Task Run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    InboundMessage,
    Schedule,
    ManualRun,
    RemoteManualRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    Running,
    Success,
    Failed,
    Timeout,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImTaskRun {
    pub run_id: String,
    pub trigger_source: TriggerSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    pub status: TaskRunStatus,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_type: Option<ScheduleTaskType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_final_response: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_tool_calls: Vec<ToolCallLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_plan_steps: Option<Vec<PlanStep>>,
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImEventSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImEventMessage {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImImageAttachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImImageAttachment {
    pub file_key: String,
    #[serde(default)]
    pub source: ImImageSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_query_param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImImageSource {
    #[default]
    MessageResource,
    UploadedImage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImEvent {
    pub event_id: String,
    pub provider_id: String,
    pub provider_type: ImProviderType,
    pub event_type: String,
    pub source: ImEventSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<ImEventMessage>,
    pub received_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_digest: Option<String>,
}

// ---------------------------------------------------------------------------
// Message Log (unified inbound/outbound message record)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Success,
    Failed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImMessageLog {
    pub id: String,
    pub provider_id: String,
    pub direction: MessageDirection,
    pub status: MessageStatus,
    pub timestamp: u64,
    /// For outbound: target_id; for inbound: sender open_id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    /// Feishu message_id returned by API
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// "text" / "interactive" / etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_type: Option<String>,
    /// Text preview (truncated for large messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
    /// How the message was triggered
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Sender open_id (for inbound messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_open_id: Option<String>,
    /// Event id (for inbound messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Whether reaction was added successfully (for inbound)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_added: Option<bool>,
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default = "default_send_msg_type")]
    pub msg_type: String,
}

fn default_send_msg_type() -> String {
    "interactive".to_string()
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            uuid: None,
            msg_type: default_send_msg_type(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadedImage {
    pub image_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderValidation {
    pub valid: bool,
    #[serde(default)]
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<u64>,
    #[serde(default)]
    pub reconnect_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            last_connected_at: None,
            last_event_at: None,
            reconnect_count: 0,
            last_error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Remote IM Gateway Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteImGatewayAction {
    ListProviders,
    GetProvider,
    ValidateProvider,
    GetConnectionStatus,
    ConnectProvider,
    DisconnectProvider,
    ListTargets,
    GetTarget,
    AddTarget,
    UpdateTarget,
    DeleteTarget,
    SendMessage,
    ListRoutes,
    AddRoute,
    UpdateRoute,
    DeleteRoute,
    ListSchedules,
    AddSchedule,
    UpdateSchedule,
    DeleteSchedule,
    ListRuns,
    ExecuteScript,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteImGatewayCommand {
    pub action: RemoteImGatewayAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Safe Summaries (for remote responses)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSafeSummary {
    pub provider_id: String,
    pub provider_type: ImProviderType,
    pub enabled: bool,
    pub connection_state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id_masked: Option<String>,
    pub secret_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSafeSummary {
    pub target_id: String,
    pub provider_id: String,
    pub display_name: String,
    pub receive_id_type: String,
    pub receive_id_masked: String,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Task Execution Request
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImTaskExecutionRequest {
    pub provider_id: String,
    pub trigger_source: TriggerSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_policy_binding: Option<ImScriptPolicyBinding>,
    pub script: TaskScript,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
}
