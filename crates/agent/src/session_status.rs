use crate::config::AgentConfig;
use crate::session::turn_timing::TurnTimingSummary;
use crate::session::AgentSession;
use crate::tools::update_plan::PlanStep;
use crate::types::ToolCallLog;
use std::sync::Arc;

pub type ActiveTurnStatusHandle = Arc<std::sync::Mutex<ActiveTurnStatus>>;
pub type AgentTurnProgressSender = tokio::sync::mpsc::UnboundedSender<AgentTurnProgressEvent>;

/// Provider-neutral progress events emitted by the turn loop for IM renderers.
#[derive(Debug, Clone)]
pub enum AgentTurnProgressEvent {
    Status(Box<ActiveTurnStatus>),
    ContextUpdated {
        context: AgentContextSnapshot,
    },
    CompactionStarted {
        progress: AgentCompactionProgress,
    },
    CompactionFinished {
        progress: AgentCompactionProgress,
    },
    CompactionFailed {
        progress: AgentCompactionProgress,
        error: String,
    },
    ToolStarted {
        tool_name: String,
        arguments: String,
    },
    ToolFinished {
        log: ToolCallLog,
        duration_ms: u64,
    },
    LongTaskStatus {
        session_key: String,
        session_id: String,
        profile: String,
        state: String,
        elapsed_ms: u64,
        last_output_preview: Option<String>,
        next_check_at_ms: Option<u64>,
        unchanged_heartbeats: u32,
    },
    PlanUpdated {
        steps: Vec<PlanStep>,
        title: Option<String>,
    },
    TitleUpdated {
        title: String,
    },
    AssistantDelta {
        content: String,
    },
    AssistantFinal {
        content: String,
    },
    TurnFinished {
        content: String,
    },
    TurnFailed {
        error: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextSnapshot {
    pub estimated_context_tokens: u32,
    pub context_window_tokens: Option<u32>,
    pub context_usage_percent: Option<f64>,
    pub compaction_count: u32,
    pub history_version: u64,
    pub message_count: usize,
    pub user_turn_count: usize,
    pub last_response_tokens: Option<u64>,
    pub total_tokens_used: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCompactionProgress {
    pub trigger: String,
    pub reason: String,
    pub phase: String,
    pub pre_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_saved: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages_removed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub compaction_count: u32,
    pub history_version: u64,
    pub context: AgentContextSnapshot,
}

/// Live status for a session while a turn loop is executing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveTurnStatus {
    pub session_key: String,
    pub state: String,
    pub started_at: u64,
    pub updated_at: u64,
    pub current_loop_iteration: u32,
    pub completed_loop_iterations: u32,
    pub max_loop_iterations: u32,
    pub last_response_tokens: Option<u64>,
    pub total_tokens_used: Option<u64>,
    pub estimated_context_tokens: u32,
    pub context_window_tokens: Option<u32>,
    pub context_usage_percent: Option<f64>,
    pub compaction_count: u32,
    pub history_version: u64,
    pub work_dir: Option<String>,
    pub message_count: usize,
    pub local_tool_count: usize,
    pub mcp_tool_count: usize,
    pub pending_guide_messages: Vec<String>,
    pub user_turn_count: usize,
    pub agent_type: Option<String>,
    pub runner_type: Option<String>,
    pub runner_id: Option<String>,
    pub external_conversation_id: Option<String>,
    pub external_thread_id: Option<String>,
    /// Turn timing metrics (TTFT, TTFM, total duration).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_timing: Option<TurnTimingSummary>,
    /// Current turn ID (UUID), when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

impl ActiveTurnStatus {
    pub(crate) fn new(session_key: &str) -> Self {
        let now = current_time_secs();
        Self {
            session_key: session_key.to_string(),
            state: "starting".to_string(),
            started_at: now,
            updated_at: now,
            current_loop_iteration: 0,
            completed_loop_iterations: 0,
            max_loop_iterations: 0,
            last_response_tokens: None,
            total_tokens_used: None,
            estimated_context_tokens: 0,
            context_window_tokens: None,
            context_usage_percent: None,
            compaction_count: 0,
            history_version: 0,
            work_dir: None,
            message_count: 0,
            local_tool_count: 0,
            mcp_tool_count: 0,
            pending_guide_messages: Vec::new(),
            user_turn_count: 0,
            agent_type: None,
            runner_type: None,
            runner_id: None,
            external_conversation_id: None,
            external_thread_id: None,
            turn_timing: None,
            turn_id: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusRuntimeContext {
    pub agent_type: Option<String>,
    pub runner_type: Option<String>,
    pub runner_id: Option<String>,
    pub external_conversation_id: Option<String>,
    pub external_thread_id: Option<String>,
}

pub(crate) fn context_usage_percent(
    estimated_tokens: u32,
    context_window_tokens: Option<u32>,
) -> Option<f64> {
    let window = context_window_tokens?;
    if window == 0 {
        return None;
    }
    Some(((estimated_tokens as f64 / window as f64) * 1000.0).round() / 10.0)
}

pub(crate) fn config_context_window_tokens(config: &AgentConfig) -> Option<u32> {
    config
        .model_context_window
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

pub fn snapshot_agent_context(
    session: &AgentSession,
    config: &AgentConfig,
) -> AgentContextSnapshot {
    let context_window_tokens = config_context_window_tokens(config);
    let estimated_context_tokens = session.effective_token_count();
    AgentContextSnapshot {
        estimated_context_tokens,
        context_window_tokens,
        context_usage_percent: context_usage_percent(
            estimated_context_tokens,
            context_window_tokens,
        ),
        compaction_count: session.compaction_count,
        history_version: session.history_version,
        message_count: session.history.len(),
        user_turn_count: session.user_turn_count(),
        last_response_tokens: session.last_response_tokens,
        total_tokens_used: session.total_tokens_used,
    }
}

pub(crate) fn update_active_turn_status<F>(session: &AgentSession, f: F)
where
    F: FnOnce(&mut ActiveTurnStatus),
{
    let mut snapshot = None;
    if let Some(handle) = &session.active_turn_status {
        if let Ok(mut status) = handle.lock() {
            f(&mut status);
            status.updated_at = current_time_secs();
            snapshot = Some(status.clone());
        }
    }
    if let (Some(sender), Some(status)) = (&session.progress_sender, snapshot) {
        let _ = sender.send(AgentTurnProgressEvent::Status(Box::new(status)));
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveTurnProgress {
    pub state: &'static str,
    pub current_loop_iteration: u32,
    pub completed_loop_iterations: u32,
    pub max_loop_iterations: u32,
    pub local_tool_count: usize,
    pub mcp_tool_count: usize,
}

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn refresh_active_turn_status(
    session: &AgentSession,
    config: &AgentConfig,
    progress: ActiveTurnProgress,
) {
    let context_window_tokens = config_context_window_tokens(config);
    let estimated_context_tokens = session.effective_token_count();
    update_active_turn_status(session, |status| {
        status.state = progress.state.to_string();
        status.current_loop_iteration = progress.current_loop_iteration;
        status.completed_loop_iterations = progress.completed_loop_iterations;
        status.max_loop_iterations = progress.max_loop_iterations;
        status.last_response_tokens = session.last_response_tokens;
        status.total_tokens_used = session.total_tokens_used;
        status.estimated_context_tokens = estimated_context_tokens;
        status.context_window_tokens = context_window_tokens;
        status.context_usage_percent =
            context_usage_percent(estimated_context_tokens, context_window_tokens);
        status.compaction_count = session.compaction_count;
        status.history_version = session.history_version;
        status.work_dir = session.work_dir.clone();
        status.message_count = session.history.len();
        status.local_tool_count = progress.local_tool_count;
        status.mcp_tool_count = progress.mcp_tool_count;
        status.user_turn_count = session.user_turn_count();
        status.agent_type = session.agent_type.clone();
        status.runner_type = session.runner_type.clone();
        status.runner_id = session.runner_id.clone();
        status.external_conversation_id = session.external_conversation_id.clone();
        status.external_thread_id = session.external_thread_id.clone();
        status.pending_guide_messages = session
            .guide_channel
            .as_ref()
            .map(|ch| ch.lock().unwrap().iter().cloned().collect())
            .unwrap_or_default();
    });
    if let Some(sender) = &session.progress_sender {
        let _ = sender.send(AgentTurnProgressEvent::ContextUpdated {
            context: snapshot_agent_context(session, config),
        });
    }
}

pub fn format_active_turn_status_text(status: &ActiveTurnStatus) -> String {
    format_active_turn_status_text_with_context(status, &StatusRuntimeContext::default())
}

pub fn format_active_turn_status_text_with_context(
    status: &ActiveTurnStatus,
    context: &StatusRuntimeContext,
) -> String {
    let token_text = status
        .total_tokens_used
        .map(format_status_metric_count)
        .unwrap_or_else(|| "N/A".to_string());
    let last_token_text = status
        .last_response_tokens
        .map(format_status_metric_count)
        .unwrap_or_else(|| "N/A".to_string());
    let context_text = match (status.context_window_tokens, status.context_usage_percent) {
        (Some(window), Some(percent)) => format!(
            "~{} / {} ({percent:.1}%)",
            format_status_metric_count(status.estimated_context_tokens.into()),
            format_status_metric_count(window.into())
        ),
        _ => format!(
            "~{} / N/A",
            format_status_metric_count(status.estimated_context_tokens.into())
        ),
    };
    let work_dir_text = status.work_dir.as_deref().unwrap_or("N/A");
    let agent_type = status
        .agent_type
        .as_ref()
        .or(context.agent_type.as_ref())
        .map(String::as_str)
        .unwrap_or("Bifrost Agent");
    let runner_type = status
        .runner_type
        .as_ref()
        .or(context.runner_type.as_ref())
        .map(String::as_str)
        .unwrap_or("bifrost_agent");
    let runner_id = status
        .runner_id
        .as_ref()
        .or(context.runner_id.as_ref())
        .map(String::as_str)
        .unwrap_or("N/A");
    let conversation_text = format_conversation_ref(
        status
            .external_thread_id
            .as_ref()
            .or(context.external_thread_id.as_ref())
            .map(String::as_str),
        status
            .external_conversation_id
            .as_ref()
            .or(context.external_conversation_id.as_ref())
            .map(String::as_str),
    );
    let guide_text = if status.pending_guide_messages.is_empty() {
        "- 引导消息: 无".to_string()
    } else {
        let mut text = format!(
            "- 引导消息: {} 条尚未进入 loop",
            status.pending_guide_messages.len()
        );
        for (idx, msg) in status.pending_guide_messages.iter().enumerate() {
            text.push_str(&format!(
                "\n  {}. {}",
                idx + 1,
                truncate_status_text(msg, 80)
            ));
        }
        text
    };
    let context_management_text = format_context_management_status(status.message_count);

    format!(
        "会话状态:\n\
         - 状态: 🔵 正在处理中\n\
         - 工作路径: {}\n\
         - Agent 类型: {}\n\
         - Runner 类型: {}\n\
         - Runner ID: {}\n\
         - 外部会话: {}\n\
         - 历史对话轮次: {}\n\
         - Loop: 第 {} 次 / 最多 {} 次（已完成 {} 次）\n\
         - 实时 token: 累计 {}，最近响应 {}\n\
         - Context 用量: {}\n\
         - 显式压缩次数: {}\n\
         - 上下文管理: {}\n\
         - 消息数: {}\n\
         {}\n\
         - 历史版本: {}\n\
         - MCP 工具数: {}\n\
         - 本地工具数: {}",
        work_dir_text,
        agent_type,
        runner_type,
        runner_id,
        conversation_text,
        status.user_turn_count,
        status.current_loop_iteration,
        status.max_loop_iterations,
        status.completed_loop_iterations,
        token_text,
        last_token_text,
        context_text,
        status.compaction_count,
        context_management_text,
        status.message_count,
        guide_text,
        status.history_version,
        status.mcp_tool_count,
        status.local_tool_count
    )
}

pub fn format_context_management_status(message_count: usize) -> String {
    format!(
        "按 token/context budget 与 compaction 管理（常规请求使用完整 history：{message_count} 条；仅 context-window overflow fallback 会改写 history）"
    )
}

pub fn format_status_metric_count(value: u64) -> String {
    const UNITS: &[(u64, &str)] = &[(1_000, "K"), (1_000_000, "M"), (1_000_000_000, "B")];
    if value < UNITS[0].0 {
        return value.to_string();
    }

    let mut unit_index = UNITS.len() - 1;
    for (index, (unit, _)) in UNITS.iter().enumerate() {
        if value < *unit {
            unit_index = index.saturating_sub(1);
            break;
        }
    }
    while unit_index + 1 < UNITS.len()
        && rounded_metric_tenths(value, UNITS[unit_index].0) >= 10_000
    {
        unit_index += 1;
    }

    let (unit, suffix) = UNITS[unit_index];
    let scaled_tenths = rounded_metric_tenths(value, unit);
    let whole = scaled_tenths / 10;
    let decimal = scaled_tenths % 10;
    if decimal == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{decimal}{suffix}")
    }
}

fn rounded_metric_tenths(value: u64, unit: u64) -> u128 {
    ((value as u128 * 10) + (unit as u128 / 2)) / unit as u128
}

pub fn format_conversation_ref(thread_id: Option<&str>, conversation_id: Option<&str>) -> String {
    if let Some(thread_id) = thread_id.map(str::trim).filter(|value| !value.is_empty()) {
        return format!("Codex threadId={}", truncate_status_text(thread_id, 80));
    }
    if let Some(conversation_id) = conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!(
            "conversationId={}",
            truncate_status_text(conversation_id, 80)
        );
    }
    "N/A".to_string()
}

fn truncate_status_text(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    format!("{}...", s.chars().take(max_chars).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_turn_status_context_usage_percent() {
        assert_eq!(context_usage_percent(50_000, Some(250_000)), Some(20.0));
        assert_eq!(context_usage_percent(1, Some(3)), Some(33.3));
        assert_eq!(context_usage_percent(1, Some(0)), None);
        assert_eq!(context_usage_percent(1, None), None);
    }

    #[test]
    fn test_active_turn_status_text_contains_runtime_metrics() {
        let status = ActiveTurnStatus {
            session_key: "runtime-status".to_string(),
            state: "model_response".to_string(),
            started_at: 10,
            updated_at: 11,
            current_loop_iteration: 3,
            completed_loop_iterations: 2,
            max_loop_iterations: 1000,
            last_response_tokens: Some(17),
            total_tokens_used: Some(51),
            estimated_context_tokens: 4_000,
            context_window_tokens: Some(250_000),
            context_usage_percent: Some(1.6),
            compaction_count: 1,
            history_version: 7,
            work_dir: Some("/tmp/bifrost-work".to_string()),
            message_count: 9,
            local_tool_count: 12,
            mcp_tool_count: 5,
            pending_guide_messages: vec!["第一条引导".to_string(), "第二条引导".to_string()],
            user_turn_count: 4,
            agent_type: Some("Bifrost Agent".to_string()),
            runner_type: Some("bifrost_agent".to_string()),
            runner_id: None,
            external_conversation_id: None,
            external_thread_id: Some("thread-runtime".to_string()),
            turn_timing: None,
            turn_id: None,
        };

        let text = format_active_turn_status_text(&status);
        assert!(text.contains("工作路径: /tmp/bifrost-work"));
        assert!(text.contains("Agent 类型: Bifrost Agent"));
        assert!(text.contains("Runner 类型: bifrost_agent"));
        assert!(text.contains("外部会话: Codex threadId=thread-runtime"));
        assert!(text.contains("历史对话轮次: 4"));
        assert!(text.contains("Loop: 第 3 次 / 最多 1000 次"));
        assert!(text.contains("已完成 2 次"));
        assert!(text.contains("实时 token: 累计 51，最近响应 17"));
        assert!(text.contains("Context 用量: ~4K / 250K (1.6%)"));
        assert!(text.contains("显式压缩次数: 1"));
        assert!(text.contains("上下文管理: 按 token/context budget 与 compaction 管理"));
        assert!(text.contains("常规请求使用完整 history：9 条"));
        assert!(text.contains("引导消息: 2 条尚未进入 loop"));
        assert!(text.contains("1. 第一条引导"));
        assert!(text.contains("2. 第二条引导"));
        assert!(text.contains("MCP 工具数: 5"));
    }
}
