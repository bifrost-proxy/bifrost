use crate::config::AgentConfig;
use crate::session::AgentSession;
use std::sync::Arc;

pub type ActiveTurnStatusHandle = Arc<std::sync::Mutex<ActiveTurnStatus>>;

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
        }
    }
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

pub(crate) fn update_active_turn_status<F>(session: &AgentSession, f: F)
where
    F: FnOnce(&mut ActiveTurnStatus),
{
    if let Some(handle) = &session.active_turn_status {
        if let Ok(mut status) = handle.lock() {
            f(&mut status);
            status.updated_at = current_time_secs();
        }
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
    });
}

pub fn format_active_turn_status_text(status: &ActiveTurnStatus) -> String {
    let token_text = status
        .total_tokens_used
        .map(|value| value.to_string())
        .unwrap_or_else(|| "N/A".to_string());
    let last_token_text = status
        .last_response_tokens
        .map(|value| value.to_string())
        .unwrap_or_else(|| "N/A".to_string());
    let context_text = match (status.context_window_tokens, status.context_usage_percent) {
        (Some(window), Some(percent)) => format!(
            "~{} / {} ({percent:.1}%)",
            status.estimated_context_tokens, window
        ),
        _ => format!("~{} / N/A", status.estimated_context_tokens),
    };
    let work_dir_text = status.work_dir.as_deref().unwrap_or("N/A");

    format!(
        "会话状态:\n\
         - 状态: 🔵 正在处理中\n\
         - 工作路径: {}\n\
         - Loop: 第 {} 次 / 最多 {} 次（已完成 {} 次）\n\
         - 实时 token: 累计 {}，最近响应 {}\n\
         - Context 用量: {}\n\
         - 压缩次数: {}\n\
         - 消息数: {}\n\
         - 历史版本: {}\n\
         - MCP 工具数: {}\n\
         - 本地工具数: {}",
        work_dir_text,
        status.current_loop_iteration,
        status.max_loop_iterations,
        status.completed_loop_iterations,
        token_text,
        last_token_text,
        context_text,
        status.compaction_count,
        status.message_count,
        status.history_version,
        status.mcp_tool_count,
        status.local_tool_count
    )
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
        };

        let text = format_active_turn_status_text(&status);
        assert!(text.contains("工作路径: /tmp/bifrost-work"));
        assert!(text.contains("Loop: 第 3 次 / 最多 1000 次"));
        assert!(text.contains("已完成 2 次"));
        assert!(text.contains("实时 token: 累计 51，最近响应 17"));
        assert!(text.contains("Context 用量: ~4000 / 250000 (1.6%)"));
        assert!(text.contains("压缩次数: 1"));
        assert!(text.contains("MCP 工具数: 5"));
    }
}
