//! Turn-loop runtime for Bifrost Agent sessions.
//!
//! Transitional split note: this file owns the central model/tool loop and is
//! intentionally isolated from session storage, progress helpers, slash command
//! routing helpers, compaction helpers, pending input helpers, and tool
//! dispatch helpers. Further reductions should extract slash command handling
//! and retry/compaction branches without changing the public `run_turn*`
//! contract.

use super::*;
use crate::types::ToolRuntimeEvent;

pub(super) fn display_iteration(iteration: usize) -> u32 {
    u32::try_from(iteration + 1).unwrap_or(u32::MAX)
}

// Turn Loop
// ---------------------------------------------------------------------------

pub(super) const STOPPED_RESPONSE: &str = "已收到 /stop，正在执行的 Agent loop 已停止。";

fn stop_requested(session: &AgentSession) -> bool {
    session
        .stop_signal
        .as_ref()
        .is_some_and(|signal| signal.is_requested())
}

pub(super) fn clear_completed_plan_for_new_turn(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
) {
    if !session
        .current_plan
        .as_deref()
        .is_some_and(plan_is_complete)
    {
        return;
    }

    session.current_plan = None;
    session.plan_repair_attempts = 0;

    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(error) =
            rec.record_plan_cleared(&session.session_key, "new_turn_after_completion")
        {
            warn!(error = %error, "failed to record plan clear");
        }
    }

    if let Some(sender) = &session.progress_sender {
        let _ = sender.send(AgentTurnProgressEvent::PlanUpdated {
            steps: Vec::new(),
            title: session.title.clone(),
        });
    }
    if let Some(sender) = &session.plan_sender {
        if let Err(error) = sender.send((Vec::new(), session.title.clone())) {
            debug!(error = %error, "plan_sender channel closed");
        }
    }
}

fn direct_workdir_switch_path(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    let path = std::path::Path::new(trimmed);
    if path.is_absolute() && path.is_dir() {
        return Some(trimmed.to_string());
    }
    None
}

fn drain_guide_messages(session: &AgentSession) -> Vec<String> {
    session
        .guide_channel
        .as_ref()
        .map(|ch| ch.drain())
        .unwrap_or_default()
}

pub fn combine_guide_messages(messages: Vec<String>) -> Option<String> {
    let messages: Vec<String> = messages
        .into_iter()
        .map(|msg| msg.trim().to_string())
        .filter(|msg| !msg.is_empty())
        .collect();
    match messages.len() {
        0 => None,
        1 => messages.into_iter().next(),
        _ => Some(
            messages
                .iter()
                .enumerate()
                .map(|(idx, msg)| format!("引导消息 {}:\n{}", idx + 1, msg))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
    }
}

async fn chat_completion_or_stop(
    client: &AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    messages: &[ChatMessage],
    tools: &[crate::types::ToolDefinition],
) -> Result<Option<crate::types::ModelResponse>, String> {
    let request = client.chat_completion(config, messages, tools);
    let Some(signal) = session.stop_signal.clone() else {
        return request.await.map(Some);
    };
    tokio::select! {
        result = request => result.map(Some),
        _ = signal.cancelled() => Ok(None),
    }
}

fn stopped_turn_result(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tool_calls_log: Vec<ToolCallLog>,
) -> TurnResult {
    session.record_turn_event(CodexTurnEventKind::TurnStopped);
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::TaskAborted {
            reason: crate::tools::goal::GoalAbortReason::Interrupted,
        },
    );
    session.add_assistant_message(STOPPED_RESPONSE);
    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(e) = rec.record_assistant_message(&session.session_key, STOPPED_RESPONSE) {
            warn!(error = %e, "failed to record stop assistant message");
        }
    }
    TurnResult {
        response: STOPPED_RESPONSE.to_string(),
        tool_calls_log,
        work_dir_switched: None,
        title_updated: session.title.clone(),
        plan_steps: session.current_plan.clone(),
        goal_needs_continuation: false,
        goal_objective: None,
    }
}

fn append_cancelled_tool_results(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tool_calls_log: &mut Vec<ToolCallLog>,
    tool_calls: &[ToolCallMessage],
    start_index: usize,
) {
    const CANCELLED_TOOL_OUTPUT: &str = "Tool call cancelled because /stop was requested.";
    for tc in tool_calls.iter().skip(start_index) {
        session.add_tool_result(&tc.id, CANCELLED_TOOL_OUTPUT);
        session.record_turn_event_entry(
            CodexTurnEvent::new(0, CodexTurnEventKind::ToolCallCompleted)
                .with_tool(tc.name(), tc.id.clone())
                .with_success(false)
                .with_detail("cancelled"),
        );
        if let Some(rec) = recorder.as_deref_mut() {
            if let Err(e) = rec.record_tool_result_with_call_id(
                &session.session_key,
                tc.name(),
                CANCELLED_TOOL_OUTPUT,
                false,
                Some(&tc.id),
            ) {
                warn!(error = %e, "failed to record cancelled tool result");
            }
        }
        tool_calls_log.push(ToolCallLog {
            tool_name: tc.name().to_string(),
            arguments: tc.arguments().to_string(),
            result: CANCELLED_TOOL_OUTPUT.to_string(),
            success: false,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn record_tool_call_started(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tc: &ToolCallMessage,
    iteration: usize,
) -> std::time::Instant {
    info!(
        tool = %tc.name(),
        call_id = %tc.id,
        "executing tool call"
    );
    if let Some(sender) = &session.progress_sender {
        let _ = sender.send(AgentTurnProgressEvent::ToolStarted {
            tool_name: tc.name().to_string(),
            arguments: tc.arguments().to_string(),
        });
    }
    session.record_turn_event_entry(
        CodexTurnEvent::new(0, CodexTurnEventKind::ToolCallStarted)
            .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
            .with_tool(tc.name(), tc.id.clone()),
    );
    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(e) = rec.record_tool_call_with_id(
            &session.session_key,
            tc.name(),
            tc.arguments(),
            Some(&tc.id),
        ) {
            warn!(error = %e, "failed to record tool call");
        }
    }
    std::time::Instant::now()
}

#[allow(clippy::too_many_arguments)]
fn apply_tool_call_completion(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tool_calls_log: &mut Vec<ToolCallLog>,
    tool_defs: &mut Vec<crate::types::ToolDefinition>,
    visible_tool_names: &mut HashSet<String>,
    config: &AgentConfig,
    tc: &ToolCallMessage,
    result: crate::types::ToolResult,
    tool_started_at: std::time::Instant,
    tool_output_limit: usize,
    iteration: usize,
    local_tool_count: usize,
    mcp_tool_count: usize,
) {
    debug!(
        tool = %tc.name(),
        success = result.success,
        output_len = result.output.len(),
        output_preview = %telemetry_preview(&result.output),
        "tool call completed"
    );

    if tc.name() == TOOL_SEARCH_TOOL_NAME && result.success {
        for definition in parse_loadable_tool_definitions(&result.output) {
            if visible_tool_names.insert(definition.name().to_string()) {
                debug!(
                    tool = %definition.name(),
                    "loaded deferred tool definition from tool_search"
                );
                session.record_turn_event_entry(
                    CodexTurnEvent::new(0, CodexTurnEventKind::DeferredToolLoaded)
                        .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                        .with_tool(definition.name(), tc.id.clone()),
                );
                tool_defs.push(definition);
            }
        }
    }

    // Apply tool output token limit truncation with 1.2x serialization
    // budget (matching Codex's policy * 1.2) to account for JSON escaping
    // overhead when the text is serialized into the API request payload.
    let output = if tool_output_limit > 0 {
        let budget = (tool_output_limit as u64)
            .saturating_mul(12)
            .saturating_div(10) as usize;
        truncate_tool_output(&result.output, budget)
    } else {
        result.output.clone()
    };

    let log = ToolCallLog {
        tool_name: tc.name().to_string(),
        arguments: tc.arguments().to_string(),
        result: result.output.clone(),
        success: result.success,
    };
    if let Some(sender) = &session.progress_sender {
        let duration_ms = u64::try_from(tool_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = sender.send(AgentTurnProgressEvent::ToolFinished {
            log: log.clone(),
            duration_ms,
        });
    }
    tool_calls_log.push(log);

    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(e) = rec.record_tool_result_with_call_id(
            &session.session_key,
            tc.name(),
            &result.output,
            result.success,
            Some(&tc.id),
        ) {
            warn!(error = %e, "failed to record tool result");
        }
    }

    session.add_tool_result(&tc.id, &output);
    apply_tool_runtime_events(
        session,
        recorder,
        tc.name(),
        result.success,
        result.runtime_events,
    );
    session.record_turn_event_entry(
        CodexTurnEvent::new(0, CodexTurnEventKind::ToolCallCompleted)
            .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
            .with_tool(tc.name(), tc.id.clone())
            .with_success(result.success),
    );
    refresh_active_turn_status(
        session,
        config,
        ActiveTurnProgress {
            state: "tool_calls",
            current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
            completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
            max_loop_iterations: config.get_max_turn_iterations(),
            local_tool_count,
            mcp_tool_count,
        },
    );

    // Skip ToolCompleted accounting for update_goal; the goal handler itself
    // fires ToolCompletedGoal with suppressed steering.
    if tc.name() != crate::tools::goal::UPDATE_GOAL_TOOL_NAME {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::ToolCompleted,
        );
    }
}

pub(super) fn apply_tool_runtime_events(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    tool_name: &str,
    success: bool,
    runtime_events: Vec<ToolRuntimeEvent>,
) {
    if success {
        for event in runtime_events {
            match event {
                ToolRuntimeEvent::PlanUpdate(args) => {
                    apply_plan_update_snapshot(session, recorder, args);
                }
            }
        }
    } else if !runtime_events.is_empty() {
        warn!(
            tool = %tool_name,
            runtime_event_count = runtime_events.len(),
            "ignoring runtime events from failed tool result"
        );
    }
}

enum LongTaskWatchOutcome {
    Completed(crate::types::ToolResult),
    Stopped,
}

const ADAPTIVE_LONG_TASK_PROFILE: &str = "adaptive";
const ADAPTIVE_LONG_TASK_MAX_WAIT_MS: u64 = 3_900_000;
const ADAPTIVE_LONG_TASK_MAX_INTERVAL_MS: u64 = 300_000;

struct LongTaskCandidate {
    session_id: String,
    profile: String,
    initial_output: String,
}

fn parse_long_task_candidate(
    tool_name: &str,
    arguments: &str,
    output: &str,
) -> Option<LongTaskCandidate> {
    if tool_name != "exec_command" {
        return None;
    }
    let args: serde_json::Value = serde_json::from_str(arguments).ok()?;
    if args
        .get("tty")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    if !value
        .get("exit_code")
        .is_some_and(serde_json::Value::is_null)
    {
        return None;
    }
    let running = value
        .get("running")
        .and_then(|value| value.as_bool())
        .unwrap_or_else(|| {
            !value
                .get("session_id")
                .is_some_and(serde_json::Value::is_null)
        });
    if !running {
        return None;
    }
    let session_id = match value.get("session_id")? {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) if !text.trim().is_empty() => text.clone(),
        _ => return None,
    };
    let long_task_candidate = value
        .get("long_task_candidate")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !long_task_candidate {
        return None;
    }
    let profile = value
        .get("suggested_wait_profile")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ADAPTIVE_LONG_TASK_PROFILE)
        .to_string();
    let initial_output = value
        .get("output")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    Some(LongTaskCandidate {
        session_id,
        profile,
        initial_output,
    })
}

#[allow(clippy::too_many_arguments)]
async fn maybe_watch_exec_long_task(
    session: &mut AgentSession,
    config: &AgentConfig,
    tools: &ToolRegistry,
    tc: &ToolCallMessage,
    result: crate::types::ToolResult,
    iteration: usize,
    tool_output_limit: usize,
    local_tool_count: usize,
    mcp_tool_count: usize,
) -> LongTaskWatchOutcome {
    let Some(candidate) = parse_long_task_candidate(tc.name(), tc.arguments(), &result.output)
    else {
        return LongTaskWatchOutcome::Completed(result);
    };

    session.record_turn_event_entry(
        CodexTurnEvent::new(0, CodexTurnEventKind::TurnSuspended)
            .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
            .with_tool(tc.name(), tc.id.clone())
            .with_detail(format!(
                "waiting_on_session={},profile={}",
                candidate.session_id, candidate.profile
            )),
    );
    session.record_turn_event_entry(
        CodexTurnEvent::new(0, CodexTurnEventKind::LongTaskWatchStarted)
            .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
            .with_tool(tc.name(), tc.id.clone()),
    );

    let started = std::time::Instant::now();
    let max_wait = long_task_profile_max_wait(
        &candidate.profile,
        config.get_background_terminal_max_timeout(),
    );
    let mut interval_ms = 250_u64;
    let mut unchanged_heartbeats = 0_u32;
    let mut total_omitted_heartbeats = 0_u32;
    let mut output_parts = Vec::new();
    if !candidate.initial_output.trim().is_empty() {
        output_parts.push(candidate.initial_output.clone());
    }

    loop {
        if stop_requested(session) {
            let _ = tools.terminate_exec_session(&candidate.session_id).await;
            return LongTaskWatchOutcome::Stopped;
        }

        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if elapsed_ms >= u64::try_from(max_wait.as_millis()).unwrap_or(u64::MAX) {
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::LongTaskResumeFailed)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_tool(tc.name(), tc.id.clone())
                    .with_detail("timeout"),
            );
            let output = output_parts.join("\n");
            return LongTaskWatchOutcome::Completed(crate::types::ToolResult {
                success: true,
                output: serde_json::json!({
                    "session_id": candidate.session_id,
                    "exit_code": null,
                    "running": true,
                    "resume_reason": "timeout",
                    "profile": candidate.profile,
                    "elapsed_ms": elapsed_ms,
                    "omitted_heartbeats": total_omitted_heartbeats,
                    "model_request_count_while_waiting": 0,
                    "output": truncate_tool_output(&output, tool_output_limit.saturating_mul(4)),
                })
                .to_string(),
                runtime_events: Vec::new(),
            });
        }

        let wait_interval_ms =
            jittered_watch_interval_ms(interval_ms, &candidate.session_id, unchanged_heartbeats);
        let last_output_preview = output_parts
            .last()
            .map(|output| truncate_tool_output(output, 512));
        let next_check_at_ms = current_time_millis().saturating_add(wait_interval_ms);
        if let Some(sender) = &session.progress_sender {
            let _ = sender.send(AgentTurnProgressEvent::LongTaskStatus {
                session_key: session.session_key.clone(),
                session_id: candidate.session_id.clone(),
                profile: candidate.profile.clone(),
                state: "waiting_on_session".to_string(),
                elapsed_ms,
                last_output_preview,
                next_check_at_ms: Some(next_check_at_ms),
                unchanged_heartbeats,
            });
        }
        refresh_active_turn_status(
            session,
            config,
            ActiveTurnProgress {
                state: "waiting_on_session",
                current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                completed_loop_iterations: u32::try_from(iteration).unwrap_or(u32::MAX),
                max_loop_iterations: config.get_max_turn_iterations(),
                local_tool_count,
                mcp_tool_count,
            },
        );

        let Some(poll_result) = tools
            .poll_exec_session(
                &candidate.session_id,
                wait_interval_ms,
                Some(tool_output_limit),
            )
            .await
        else {
            return LongTaskWatchOutcome::Completed(result);
        };
        if !poll_result.success {
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::LongTaskResumeFailed)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_tool(tc.name(), tc.id.clone())
                    .with_detail("poll_failed"),
            );
            return LongTaskWatchOutcome::Completed(poll_result);
        }
        let poll_json: serde_json::Value =
            serde_json::from_str(&poll_result.output).unwrap_or(serde_json::Value::Null);
        let new_output = poll_json
            .get("output")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if new_output.trim().is_empty() {
            unchanged_heartbeats = unchanged_heartbeats.saturating_add(1);
            total_omitted_heartbeats = total_omitted_heartbeats.saturating_add(1);
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::LongTaskHeartbeat)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_tool(tc.name(), tc.id.clone()),
            );
        } else {
            output_parts.push(new_output);
            unchanged_heartbeats = 0;
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::LongTaskOutputAvailable)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_tool(tc.name(), tc.id.clone()),
            );
            interval_ms = 250;
        }

        let exit_code = poll_json.get("exit_code").and_then(|value| value.as_i64());
        if exit_code.is_some() {
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::LongTaskExited)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_tool(tc.name(), tc.id.clone())
                    .with_detail(format!("exit_code={}", exit_code.unwrap_or_default())),
            );
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::TurnResumed)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_tool(tc.name(), tc.id.clone()),
            );
            let output = output_parts.join("\n");
            return LongTaskWatchOutcome::Completed(crate::types::ToolResult {
                success: true,
                output: serde_json::json!({
                    "session_id": null,
                    "exit_code": exit_code,
                    "running": false,
                    "resume_reason": "process_exited",
                    "profile": candidate.profile,
                    "elapsed_ms": elapsed_ms,
                    "omitted_heartbeats": total_omitted_heartbeats,
                    "model_request_count_while_waiting": 0,
                    "output": truncate_tool_output(&output, tool_output_limit.saturating_mul(4)),
                })
                .to_string(),
                runtime_events: Vec::new(),
            });
        }

        interval_ms = next_long_task_interval_ms(&candidate.profile, interval_ms);
    }
}

fn current_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(super) fn jittered_watch_interval_ms(base_ms: u64, session_id: &str, heartbeat: u32) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut hasher);
    heartbeat.hash(&mut hasher);
    let jitter_percent = 90 + (hasher.finish() % 21);
    base_ms.saturating_mul(jitter_percent).saturating_div(100)
}

pub(super) fn long_task_profile_max_wait(
    _profile: &str,
    configured_timeout_ms: u64,
) -> std::time::Duration {
    std::time::Duration::from_millis(configured_timeout_ms.max(ADAPTIVE_LONG_TASK_MAX_WAIT_MS))
}

pub(super) fn long_task_profile_max_interval_ms(_profile: &str) -> u64 {
    ADAPTIVE_LONG_TASK_MAX_INTERVAL_MS
}

pub(super) fn next_long_task_interval_ms(profile: &str, current_ms: u64) -> u64 {
    current_ms
        .saturating_mul(2)
        .clamp(250, long_task_profile_max_interval_ms(profile))
}

fn is_parallel_local_tool_call(
    tools: &ToolRegistry,
    mcp: Option<&McpManager>,
    tool_name: &str,
) -> bool {
    tools.contains_tool(tool_name)
        && local_tool_supports_parallel(tool_name)
        && tool_name != TOOL_SEARCH_TOOL_NAME
        && crate::tools::goal::goal_tool_names()
            .iter()
            .all(|goal_tool| goal_tool != &tool_name)
        && !mcp.is_some_and(|manager| manager.is_mcp_tool(tool_name))
}

/// Run a single turn of the agent conversation.
///
/// This implements the core turn loop:
/// 1. Handle built-in commands (/clear, /reset)
/// 2. Build prompt prefix and run pre-sampling compaction when needed
/// 3. Add the user message and send to model with available tools
/// 4. If model returns tool_calls → execute → mid-turn compaction check → loop
/// 5. If model returns text → return as final response
pub async fn run_turn(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    tools: &ToolRegistry,
    user_message: &str,
    system_prompt_override: Option<&str>,
) -> Result<TurnResult, String> {
    run_turn_with_mcp(
        client,
        config,
        session,
        tools,
        None,
        user_message,
        system_prompt_override,
        None,
    )
    .await
}

/// Run a single turn with optional MCP tool support.
///
/// Same as `run_turn` but accepts an optional `McpManager` for routing tool calls
/// to MCP servers when the model invokes MCP-provided tools.
#[allow(clippy::too_many_arguments)]
pub async fn run_turn_with_mcp(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    tools: &ToolRegistry,
    mcp: Option<&mut McpManager>,
    user_message: &str,
    system_prompt_override: Option<&str>,
    recorder: Option<&mut ConversationRecorder>,
) -> Result<TurnResult, String> {
    run_turn_with_mcp_multimodal(
        client,
        config,
        session,
        tools,
        mcp,
        user_message,
        &[],
        system_prompt_override,
        recorder,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_turn_with_mcp_multimodal(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    tools: &ToolRegistry,
    mut mcp: Option<&mut McpManager>,
    user_message: &str,
    images: &[ChatImageInput],
    system_prompt_override: Option<&str>,
    mut recorder: Option<&mut ConversationRecorder>,
) -> Result<TurnResult, String> {
    if !config.enabled {
        return Err("agent is disabled".to_string());
    }
    session.clear_turn_events();
    session.record_turn_event(CodexTurnEventKind::TurnStarted);

    let trimmed = user_message.trim();
    if let Some(new_dir) = direct_workdir_switch_path(trimmed) {
        info!(
            session_key = %session.session_key,
            new_work_dir = %new_dir,
            "switching session work directory from direct path input"
        );
        session.reinitialize_work_dir(new_dir.clone());
        session.record_turn_event(CodexTurnEventKind::TurnCompleted);
        return Ok(TurnResult {
            response: format!(
                "已切换工作目录到: {}\n\n会话历史已清空，已重新加载项目配置。",
                new_dir
            ),
            tool_calls_log: Vec::new(),
            work_dir_switched: Some(new_dir),
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // Handle built-in commands
    let slash_dispatch = slash_commands::dispatch(&session.slash_router, trimmed);
    match &slash_dispatch {
        Dispatch::Unknown(command) => {
            debug!(
                session_key = %session.session_key,
                command = %command,
                "unknown slash input falls back to normal chat message"
            );
        }
        Dispatch::RunSkill { record, invocation } => {
            let report = bifrost_skills::SkillExecutor::default()
                .execute(record.as_ref(), invocation.clone())
                .await?;
            return Ok(TurnResult {
                response: if report.stdout.trim().is_empty() {
                    "Skill 执行完成。".to_string()
                } else {
                    report.stdout.trim().to_string()
                },
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        Dispatch::Builtin { .. } | Dispatch::NotACommand => {}
    }

    if matches!(slash_dispatch, Dispatch::NotACommand | Dispatch::Unknown(_)) && !trimmed.is_empty()
    {
        clear_completed_plan_for_new_turn(session, &mut recorder);
    }

    // /help — show available commands
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Help,
            ..
        }
    ) {
        return Ok(TurnResult {
            response: session.slash_router.help_text(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /stop reaches this path only when no other loop is active for the session.
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Stop,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        return Ok(TurnResult {
            response: "当前没有正在执行的 Agent loop。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Clear | BuiltinCommand::Reset,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        session.clear();
        return Ok(TurnResult {
            response: "会话已重置，可以开始新的对话。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /undo [N] — rollback last N user turns (default 1)
    if let Dispatch::Builtin {
        command: BuiltinCommand::Undo,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let n: u32 = args.parse().unwrap_or(1);
        let removed = session.rollback(n);
        return Ok(TurnResult {
            response: format!(
                "已回退 {n} 轮对话（移除了 {removed} 条消息）。当前历史: {} 条消息。",
                session.history.len()
            ),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /compact — manual compaction (same as Codex's CompactionTrigger::Manual)
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Compact,
            ..
        }
    ) {
        return super::run_manual_compaction_command(client, config, session, recorder).await;
    }

    // /remember <text> — explicit long-term memory write.
    if let Dispatch::Builtin {
        command: BuiltinCommand::Remember,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        if args.trim().is_empty() {
            return Ok(TurnResult {
                response: "用法: /remember <text>".to_string(),
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        let record = memory::remember_explicit(config, session, args.trim())?;
        return Ok(TurnResult {
            response: format!("已记住长期记忆: {}", record.id),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /memories — list visible long-term memories.
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Memories,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let records = memory::list_visible_memories(config, session, 20)?;
        let response = if records.is_empty() {
            "当前 scope 没有长期记忆。".to_string()
        } else {
            let mut lines = vec!["当前可见长期记忆文件条目:".to_string()];
            for record in records {
                lines.push(format!(
                    "- {} [{}] {}",
                    record.id, record.path, record.content
                ));
            }
            lines.join("\n")
        };
        return Ok(TurnResult {
            response,
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /forget <id|last> — soft delete one visible memory.
    if let Dispatch::Builtin {
        command: BuiltinCommand::Forget,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        if args.trim().is_empty() {
            return Ok(TurnResult {
                response: "用法: /forget <id|last>".to_string(),
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        let response = match memory::forget_memory(config, session, args.trim())? {
            Some(id) => format!("已忘记长期记忆: {id}"),
            None => "没有找到可忘记的长期记忆。".to_string(),
        };
        return Ok(TurnResult {
            response,
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    if let Dispatch::Builtin {
        command: BuiltinCommand::Goal,
        ref args,
    } = slash_dispatch
    {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let response = handle_goal_command(session, args).await;
        return Ok(TurnResult {
            response,
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /status — show session state (token usage, compaction count, etc.)
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Status,
            ..
        }
    ) {
        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                warn!(error = %e, "failed to record user message");
            }
        }
        let est = session.estimate_tokens();
        let context_tokens = session.effective_token_count();
        let real = session
            .total_tokens_used
            .map(crate::session_status::format_status_metric_count)
            .unwrap_or_else(|| "N/A".to_string());
        let mcp_tool_count = mcp.as_ref().map(|m| m.list_tools().len()).unwrap_or(0);
        let context_window = config_context_window_tokens(config);
        let context_usage = context_usage_percent(context_tokens, context_window)
            .map(|percent| format!("{percent:.1}%"))
            .unwrap_or_else(|| "N/A".to_string());
        let context_window_text = context_window
            .map(|window| crate::session_status::format_status_metric_count(window.into()))
            .unwrap_or_else(|| "N/A".to_string());
        let context_management_text =
            crate::session_status::format_context_management_status(session.history.len());
        let work_dir_text = session.work_dir.as_deref().unwrap_or("N/A");
        let agent_type = session.agent_type.as_deref().unwrap_or("Bifrost Agent");
        let runner_type = session.runner_type.as_deref().unwrap_or("bifrost_agent");
        let runner_id = session.runner_id.as_deref().unwrap_or("N/A");
        let conversation_ref = crate::session_status::format_conversation_ref(
            session.external_thread_id.as_deref(),
            session.external_conversation_id.as_deref(),
        );
        return Ok(TurnResult {
            response: format!(
                "会话状态:\n- 工作路径: {}\n- Agent 类型: {}\n- Runner 类型: {}\n- Runner ID: {}\n- 外部会话: {}\n- 历史对话轮次: {}\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- Context 用量: ~{} / {} ({})\n- 显式压缩次数: {}\n- 上下文管理: {}\n- 历史版本: {}\n- MCP 工具数: {}",
                work_dir_text,
                agent_type,
                runner_type,
                runner_id,
                conversation_ref,
                session.user_turn_count(),
                session.history.len(),
                crate::session_status::format_status_metric_count(est.into()),
                real,
                crate::session_status::format_status_metric_count(context_tokens.into()),
                context_window_text,
                context_usage,
                session.compaction_count,
                context_management_text,
                session.history_version,
                mcp_tool_count
            ),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // /resume — load the most recent JSONL conversation and restore session history
    if matches!(
        slash_dispatch,
        Dispatch::Builtin {
            command: BuiltinCommand::Resume,
            ..
        }
    ) {
        let agent_home = crate::config::agent_home_dir();
        let mut files = persistence::list_conversations(&agent_home, Some(&session.session_key));
        // Exclude the current recorder's file (it was just created for this request
        // and contains no conversation data yet).
        if let Some(ref rec) = recorder {
            let current_file = rec.file_path().to_path_buf();
            files.retain(|f| f != &current_file);
        }
        // Try files from newest to oldest, skipping any that load 0 messages.
        while let Some(candidate) = files.pop() {
            match persistence::load_conversation_lossy(&candidate) {
                Ok(report)
                    if !report.messages.is_empty()
                        && report.session_key.as_deref() == Some(session.session_key.as_str()) =>
                {
                    let count = report.messages.len();
                    session.history = report.messages;
                    if let Ok(runtime_state) = persistence::load_session_runtime_state(&candidate) {
                        session.current_goal = runtime_state.current_goal;
                        session.current_plan = runtime_state.current_plan;
                        session.total_tokens_used = runtime_state.total_tokens_used;
                        session.restore_token_snapshot(runtime_state.last_response_tokens);
                        session.compaction_count = runtime_state.compaction_count;
                        session.resolved_base_instructions = runtime_state.base_instructions;
                        crate::tools::goal::goal_runtime_apply(
                            session,
                            crate::tools::goal::GoalRuntimeEvent::ThreadResumed,
                        );
                    }
                    session.history_version = session.history_version.saturating_add(1);
                    if report.skipped_lines > 0 {
                        warn!(
                            file = %candidate.display(),
                            skipped_lines = report.skipped_lines,
                            "resumed session from lossy conversation history"
                        );
                    }
                    return Ok(TurnResult {
                        response: format!(
                            "已恢复会话历史，加载了 {} 条消息（来源: {}）。",
                            count,
                            candidate.display()
                        ),
                        tool_calls_log: Vec::new(),
                        work_dir_switched: None,
                        title_updated: None,
                        plan_steps: session.current_plan.clone(),
                        goal_needs_continuation: false,
                        goal_objective: None,
                    });
                }
                Ok(_) => {
                    // 0 messages — skip this file (e.g. session_start only)
                    debug!(
                        file = %candidate.display(),
                        "skipping session file with 0 messages during resume"
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        file = %candidate.display(),
                        error = %e,
                        "failed to load session file during resume, trying next"
                    );
                    continue;
                }
            }
        }
        return Ok(TurnResult {
            response: "没有找到可恢复的会话记录。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    if let Dispatch::Builtin {
        command: BuiltinCommand::Skill,
        ref args,
    } = slash_dispatch
    {
        // Route through the authoring hub so `/skill start|draft|commit|cancel`
        // drives `SkillAuthoringSession` → `SkillStore::commit`, getting
        // validation, manifest.json, checksum, and `.history` archival for free.
        let store: Option<Arc<SkillStore>> = session
            .skill_registry
            .as_ref()
            .map(|registry| registry.store());
        if let Some(store) = store {
            let outcome = session.skill_authoring.dispatch(
                &session.session_key,
                store,
                session.skill_registry.as_deref(),
                args,
            );
            // Refresh the registry so a freshly committed skill becomes
            // discoverable via slash commands in the same session.
            if outcome.committed.is_some() {
                if let Some(registry) = session.skill_registry.as_ref() {
                    if let Err(e) = registry.reload_all() {
                        warn!(error = %e, "failed to reload skill registry after commit");
                    }
                }
            }
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_user_message(&session.session_key, trimmed) {
                    warn!(error = %e, "failed to record user message");
                }
            }
            return Ok(TurnResult {
                response: outcome.response,
                tool_calls_log: Vec::new(),
                work_dir_switched: None,
                title_updated: None,
                plan_steps: None,
                goal_needs_continuation: false,
                goal_objective: None,
            });
        }
        return Ok(TurnResult {
            response: "Skill registry 尚未初始化，无法创建 skill。".to_string(),
            tool_calls_log: Vec::new(),
            work_dir_switched: None,
            title_updated: None,
            plan_steps: None,
            goal_needs_continuation: false,
            goal_objective: None,
        });
    }

    // A normal new user turn resumes an interrupted goal, matching Codex's
    // thread-resume behavior more closely than keeping the goal paused until
    // an explicit /resume slash command.
    if !trimmed.starts_with('/') {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::MaybeContinueIfIdle,
        );
    }

    // Signal turn started for goal accounting baseline snapshot.
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::TurnStarted,
    );

    // Lazily load user instructions on first turn (matching Codex's once-per-session
    // loading). Subsequent turns reuse the cached value.
    if session.user_instructions.is_none() {
        session.load_user_instructions(config);
    }

    let base_instructions = if let Some(route_base_prompt) = system_prompt_override {
        route_base_prompt.to_string()
    } else if let Some(existing) = session.resolved_base_instructions.clone() {
        existing
    } else {
        let resolved = prompt::resolve_base_instructions_text(config, None);
        session.resolved_base_instructions = Some(resolved.clone());
        resolved
    };

    // Build prompt prefix:
    // system(base instructions) + developer sections + contextual user sections.
    // Route-level `system_prompt_override` is treated as a base prompt override,
    // not as a full prompt replacement, so tools/skills/AGENTS/env still apply.
    let prompt_messages = prompt::build_prompt_messages_with_skill_registry(
        config,
        &base_instructions,
        session.work_dir.as_deref(),
        session.skill_registry.as_deref(),
        session.user_instructions.as_deref(),
    );
    let prompt_prefix = prompt_messages.prefix;

    compact_pre_sampling_if_needed(client, config, session, &mut recorder, &base_instructions)
        .await?;

    // Add user message to history
    session.add_user_message_with_images(user_message, images);

    // Record user message
    if let Some(ref mut rec) = recorder {
        if let Err(e) =
            rec.record_user_message_with_images(&session.session_key, user_message, images)
        {
            warn!(error = %e, "failed to record user message");
        }
    }

    tools.configure_exec_sessions(config.get_background_terminal_max_timeout());

    // Merge tool definitions: local tools + direct MCP tools. Deferred MCP
    // tools are available through turn-scoped `tool_search`.
    let mut tool_defs = tools.definitions();
    let local_tool_count = tool_defs.len();
    let mut visible_tool_names = tool_defs
        .iter()
        .map(|tool| tool.name().to_string())
        .collect::<HashSet<_>>();
    let mut tool_search_entries = Vec::new();
    let mcp_tool_count = mcp.as_ref().map(|m| m.list_tools().len()).unwrap_or(0);
    if let Some(ref mcp_mgr) = mcp {
        let exposure = mcp_mgr.tool_exposure();
        for definition in exposure.direct_tools {
            visible_tool_names.insert(definition.name().to_string());
            tool_defs.push(definition);
        }
        tool_search_entries.extend(exposure.deferred_tools.into_iter().map(|definition| {
            let source = definition
                .name()
                .split_once("__")
                .map(|(prefix, _)| format!("MCP tools: {prefix}"))
                .unwrap_or_else(|| "MCP tools".to_string());
            ToolSearchEntry::new(definition, source)
        }));
    }
    let tool_search_tool = if tool_search_entries.is_empty() {
        None
    } else {
        let definition = tool_search_definition(&tool_search_entries);
        visible_tool_names.insert(definition.name().to_string());
        tool_defs.push(definition);
        Some(ToolSearchTool::new(tool_search_entries))
    };
    info!(
        local_tools = local_tool_count,
        mcp_tools = mcp_tool_count,
        visible_tools = tool_defs.len(),
        tool_search_visible = tool_search_tool.is_some(),
        "assembled agent tool definitions"
    );

    let work_dir = session
        .work_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.resolve_work_dir());
    // Ensure work_dir exists
    let _ = std::fs::create_dir_all(&work_dir);
    let mut tool_calls_log: Vec<crate::types::ToolCallLog> = Vec::new();
    let max_iterations = config.get_max_turn_iterations() as usize;
    let tool_output_limit = config
        .tool_output_token_limit
        .unwrap_or(AgentConfig::DEFAULT_TOOL_OUTPUT_TOKEN_LIMIT);

    // --- Codex parity: create per-turn context snapshot ---
    let turn_timing = std::sync::Arc::new(super::turn_timing::TurnTimingState::default());
    let cancellation_token = crate::turn_runtime::CancellationToken::default_grace();
    let turn_context = super::turn_context::BifrostTurnContext {
        turn_id: uuid::Uuid::new_v4().to_string(),
        model: config.model.clone().unwrap_or_default(),
        reasoning_effort: config.model_reasoning_effort.clone(),
        developer_instructions: None,
        max_iterations: config.get_max_turn_iterations(),
        max_output_tokens: config.max_completion_tokens,
        temperature: None,
        verbose_logging: false,
        cancellation_token: cancellation_token.clone(),
        turn_timing: turn_timing.clone(),
        started_at: std::time::Instant::now(),
        features: super::turn_context::TurnFeatures {
            mcp_enabled: mcp.is_some(),
            streaming_enabled: false,
            mid_turn_compaction: true,
            tool_search_enabled: tool_search_tool.is_some(),
            multimodal_enabled: !images.is_empty(),
        },
    };
    // Record turn start time for TTFT/TTFM tracking.
    turn_timing.mark_turn_started(turn_context.started_at);
    debug!(
        turn_id = %turn_context.turn_id,
        model = %turn_context.model,
        "created turn context"
    );

    // Dispatch turn-start hook to all registered lifecycle hooks.
    session
        .hook_registry
        .dispatch_turn_start(&turn_context, super::task::TaskKind::Regular)
        .await;
    // Keep cancellation_token alive for the turn duration.
    if let Some(signal) = session.stop_signal.clone() {
        let token = cancellation_token.clone();
        tokio::spawn(async move {
            signal.cancelled().await;
            token.cancel();
        });
    }
    let _cancellation_guard = cancellation_token;
    refresh_active_turn_status(
        session,
        config,
        ActiveTurnProgress {
            state: "starting",
            current_loop_iteration: 0,
            completed_loop_iterations: 0,
            max_loop_iterations: config.get_max_turn_iterations(),
            local_tool_count,
            mcp_tool_count,
        },
    );

    info!(
        session_key = %session.session_key,
        message_count = session.history.len(),
        tool_count = tool_defs.len(),
        compaction_count = session.compaction_count,
        "starting agent turn"
    );

    // Skip memory injection if session was just cleared (prevents model from
    // "remembering" prior conversation context immediately after /clear).
    let memory_message = if session.memory_cleared {
        info!(
            session_key = %session.session_key,
            "skipping memory injection after /clear"
        );
        session.memory_cleared = false; // reset for future turns
        None
    } else {
        memory::recall_system_message(config, session, user_message)
    };

    for iteration in 0..max_iterations {
        // Drain steer queue before building the next model request (Codex parity).
        // Steer inputs injected mid-turn are appended as user messages so the
        // model sees them in the next prompt build.
        {
            let mut steer_messages = session
                .steer_queue
                .drain_by_mode(super::steer::SteerInjectMode::Immediate);
            steer_messages.extend(
                session
                    .steer_queue
                    .drain_by_mode(super::steer::SteerInjectMode::NextPromptBuild),
            );
            for steer in steer_messages {
                debug!(
                    label = ?steer.label,
                    content_len = steer.content.len(),
                    mode = ?steer.mode,
                    "injecting steer queue message"
                );
                if let Some(ref mut rec) = recorder {
                    if let Err(error) =
                        rec.record_user_message(&session.session_key, &steer.content)
                    {
                        warn!(error = %error, "failed to record steer queue message");
                    }
                }
                session.history.push(ChatMessage::user(&steer.content));
            }
        }

        // Build messages from full sanitized history. Compaction is the normal
        // history-rewrite path; provider overflow is surfaced without silently
        // trimming live history.
        let messages = session.build_messages(&prompt_prefix, memory_message.as_ref());
        let model_request_progress = ActiveTurnProgress {
            state: "model_request",
            current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
            completed_loop_iterations: u32::try_from(iteration).unwrap_or(u32::MAX),
            max_loop_iterations: config.get_max_turn_iterations(),
            local_tool_count,
            mcp_tool_count,
        };
        refresh_active_turn_status(session, config, model_request_progress);

        debug!(
            iteration,
            message_count = messages.len(),
            history_len = session.history.len(),
            "sending model request"
        );
        session.record_turn_event_entry(
            CodexTurnEvent::new(0, CodexTurnEventKind::ModelRequestStarted)
                .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX)),
        );

        let response =
            chat_completion_or_stop(client, config, session, &messages, &tool_defs).await;

        // Retry with exponential backoff on transient errors;
        // Degradation on context window overflow:
        //   1. Emergency compact (best effort — preserves most context)
        //   2. Loop trim oldest messages until within budget
        //   3. Give up only when history is exhausted
        let response = match response {
            Ok(Some(r)) => r,
            Ok(None) => {
                info!(
                    session_key = %session.session_key,
                    iteration = iteration + 1,
                    "agent turn stopped during model request"
                );
                // Dispatch turn-abort hook for user-initiated stop.
                {
                    let timing_summary =
                        super::turn_timing::TurnTimingSummary::from_state(&turn_timing);
                    let outcome = super::hooks::TurnOutcome::Cancelled {
                        reason: "user_cancelled".to_string(),
                    };
                    session
                        .hook_registry
                        .dispatch_turn_abort(&turn_context, &outcome, &timing_summary)
                        .await;
                }
                return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
            }
            Err(e) => {
                if is_context_window_error(&e) {
                    warn!(
                        error = %e,
                        history_len = session.history.len(),
                        estimated_tokens = session.estimate_tokens(),
                        "context window exceeded, starting degradation chain"
                    );

                    // Step 1: Try emergency compact first (best quality reduction)
                    let compacted = match compact::compact_session_with_hooks(
                        client,
                        config,
                        session,
                        compact::CompactionTrigger::Auto,
                        compact::CompactionReason::ContextLimit,
                        compact::CompactionPhase::MidTurn,
                        Some(&base_instructions),
                        compact::InitialContextInjection::BeforeLastUserMessage,
                        build_mid_turn_initial_context(
                            session,
                            &prompt_prefix,
                            memory_message.as_ref(),
                        ),
                        &turn_context.cancellation_token,
                    )
                    .await
                    {
                        Ok(result) if result.performed => {
                            record_compaction_event(
                                &mut recorder,
                                session,
                                &result,
                                "auto",
                                "context_limit",
                                "mid_turn",
                                true,
                            );
                            info!(
                                tokens_saved = result.tokens_saved,
                                duration_ms = ?result.duration_ms,
                                "emergency compact succeeded"
                            );
                            refresh_active_turn_status(session, config, model_request_progress);
                            true
                        }
                        Ok(_) => {
                            info!("emergency compact skipped");
                            false
                        }
                        Err(compact_err) => {
                            warn!(error = %compact_err, "emergency compact failed");
                            false
                        }
                    };

                    // Step 2: If compact worked, retry once. If the provider
                    // still reports context overflow, do not silently delete
                    // live history; mirror Codex by marking the context full
                    // and surfacing the overflow.
                    if compacted {
                        let retry_messages =
                            session.build_messages(&prompt_prefix, memory_message.as_ref());
                        match chat_completion_or_stop(
                            client,
                            config,
                            session,
                            &retry_messages,
                            &tool_defs,
                        )
                        .await
                        {
                            Ok(Some(r)) => r,
                            Ok(None) => {
                                info!(
                                    session_key = %session.session_key,
                                    "agent turn stopped during post-compact retry"
                                );
                                return Ok(stopped_turn_result(
                                    session,
                                    &mut recorder,
                                    tool_calls_log,
                                ));
                            }
                            Err(e2) if is_context_window_error(&e2) => {
                                warn!(
                                    error = %e2,
                                    "still over context limit after compact; preserving live history"
                                );
                                mark_context_window_full(session, config, model_request_progress);
                                crate::tools::goal::goal_runtime_apply(
                                    session,
                                    crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                        reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                    },
                                );
                                return Err(e2);
                            }
                            Err(e2) => {
                                crate::tools::goal::goal_runtime_apply(
                                    session,
                                    crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                        reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                    },
                                );
                                return Err(e2);
                            }
                        }
                    } else {
                        warn!(
                            error = %e,
                            "context overflow could not be compacted; preserving live history"
                        );
                        mark_context_window_full(session, config, model_request_progress);
                        crate::tools::goal::goal_runtime_apply(
                            session,
                            crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                reason: crate::tools::goal::GoalAbortReason::Interrupted,
                            },
                        );
                        return Err(e);
                    }
                } else if is_retryable_error(&e) {
                    // Transient error — exponential backoff retry (up to 5 attempts)
                    const MAX_RETRIES: usize = 5;
                    let mut last_err = e;
                    let mut succeeded = None;
                    for retry in 0..MAX_RETRIES {
                        let delay_ms = 1000 * (1u64 << retry.min(3));
                        warn!(
                            error = %last_err,
                            retry_attempt = retry + 1,
                            max_retries = MAX_RETRIES,
                            retry_in_ms = delay_ms,
                            "transient API error, retrying"
                        );
                        if let Some(signal) = session.stop_signal.clone() {
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                                _ = signal.cancelled() => {
                                    info!(
                                        session_key = %session.session_key,
                                        "agent turn stopped during retry backoff"
                                    );
                                    return Ok(stopped_turn_result(
                                        session,
                                        &mut recorder,
                                        tool_calls_log,
                                    ));
                                }
                            }
                        } else {
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        }
                        let retry_messages =
                            session.build_messages(&prompt_prefix, memory_message.as_ref());
                        match chat_completion_or_stop(
                            client,
                            config,
                            session,
                            &retry_messages,
                            &tool_defs,
                        )
                        .await
                        {
                            Ok(Some(r)) => {
                                info!(retry_attempt = retry + 1, "retry succeeded");
                                succeeded = Some(r);
                                break;
                            }
                            Ok(None) => {
                                info!(
                                    session_key = %session.session_key,
                                    "agent turn stopped during retry model request"
                                );
                                return Ok(stopped_turn_result(
                                    session,
                                    &mut recorder,
                                    tool_calls_log,
                                ));
                            }
                            Err(e2) => {
                                last_err = e2;
                            }
                        }
                    }
                    match succeeded {
                        Some(r) => r,
                        None => {
                            // All retries exhausted — graceful degradation
                            if !tool_calls_log.is_empty() {
                                // We have partial results from tool calls; return them
                                warn!(
                                    error = %last_err,
                                    tool_calls_done = tool_calls_log.len(),
                                    "all retries failed, returning partial results"
                                );
                                let mut partial_response = String::from(
                                    "⚠️ **模型调用失败，以下是已执行的工具结果：**\n\n",
                                );
                                for log in &tool_calls_log {
                                    let icon = if log.success { "✅" } else { "❌" };
                                    partial_response
                                        .push_str(&format!("{} `{}`\n", icon, log.tool_name));
                                }
                                partial_response.push_str(&format!(
                                    "\n---\n**错误原因**: {}\n\n请重新发送消息或稍后重试。",
                                    last_err
                                ));
                                session.add_assistant_message(&partial_response);
                                crate::tools::goal::goal_runtime_apply(
                                    session,
                                    crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                        reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                    },
                                );
                                return Ok(TurnResult {
                                    response: partial_response,
                                    tool_calls_log,
                                    work_dir_switched: None,
                                    title_updated: None,
                                    plan_steps: None,
                                    goal_needs_continuation: false,
                                    goal_objective: None,
                                });
                            }
                            crate::tools::goal::goal_runtime_apply(
                                session,
                                crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                    reason: crate::tools::goal::GoalAbortReason::Interrupted,
                                },
                            );
                            return Err(last_err);
                        }
                    }
                } else {
                    // Non-retryable, non-context-window error
                    // If we have partial tool results, return them gracefully
                    if !tool_calls_log.is_empty() {
                        warn!(
                            error = %e,
                            tool_calls_done = tool_calls_log.len(),
                            "non-retryable error, returning partial results"
                        );
                        let mut partial_response =
                            String::from("⚠️ **模型调用失败，以下是已执行的工具结果：**\n\n");
                        for log in &tool_calls_log {
                            let icon = if log.success { "✅" } else { "❌" };
                            partial_response.push_str(&format!("{} `{}`\n", icon, log.tool_name));
                        }
                        partial_response.push_str(&format!(
                            "\n---\n**错误原因**: {}\n\n请重新发送消息或稍后重试。",
                            e
                        ));
                        session.add_assistant_message(&partial_response);
                        crate::tools::goal::goal_runtime_apply(
                            session,
                            crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                                reason: crate::tools::goal::GoalAbortReason::Interrupted,
                            },
                        );
                        return Ok(TurnResult {
                            response: partial_response,
                            tool_calls_log,
                            work_dir_switched: None,
                            title_updated: None,
                            plan_steps: None,
                            goal_needs_continuation: false,
                            goal_objective: None,
                        });
                    }
                    crate::tools::goal::goal_runtime_apply(
                        session,
                        crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                            reason: crate::tools::goal::GoalAbortReason::Interrupted,
                        },
                    );
                    return Err(e);
                }
            }
        };

        if stop_requested(session) {
            info!(
                session_key = %session.session_key,
                iteration = iteration + 1,
                "agent turn stopped after model response"
            );
            return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
        }

        // Track real token usage from API response. Cumulative cost uses
        // total_tokens while the active context snapshot uses input tokens.
        if let Some(ref usage) = response.usage {
            session.track_token_usage(usage.context_tokens(), usage.total_tokens);
            // Keep server-observed prefill baseline for status/debug telemetry.
            session
                .auto_compact_window
                .ensure_server_observed_prefill_from_usage(usage.prompt_tokens);
        }
        // Record TTFT on first model response (Codex parity: turn_timing).
        if iteration == 0 {
            if let Some(ttft) = turn_timing.record_first_token() {
                debug!(ttft_ms = ttft.as_millis(), "recorded TTFT");
            }
        }
        session.record_turn_event_entry(
            CodexTurnEvent::new(0, CodexTurnEventKind::ModelResponseCompleted)
                .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                .with_detail(format!("finish_reason={}", response.finish_reason)),
        );
        refresh_active_turn_status(
            session,
            config,
            session_compaction::model_response_progress(
                config,
                display_iteration(iteration),
                local_tool_count,
                mcp_tool_count,
            ),
        );

        // Check if model wants to call tools
        if response.tool_calls.is_empty() {
            if session
                .current_plan
                .as_ref()
                .is_some_and(|plan| plan_has_unfinished_steps(plan))
                && session.plan_repair_attempts < 2
            {
                session.plan_repair_attempts = session.plan_repair_attempts.saturating_add(1);
                session.history.push(ChatMessage::system(
                    "You already have an active task plan. Before concluding, call update_plan with the complete current task snapshot. If the work is complete, mark all still-relevant steps as completed. Do not re-add steps that are no longer relevant to the current task.",
                ));
                session.touch();
                continue;
            }

            // Guide-mode final check: if a guide message arrived while the model
            // was generating its final response (no tool calls), the mid-turn
            // checkpoint (after tool execution) never consumed it. Check here
            // before ending the turn so the message is not silently lost.
            let guide_messages = drain_guide_messages(session);
            if let Some(guide_msg) = combine_guide_messages(guide_messages) {
                info!(
                    session_key = %session.session_key,
                    guide_msg_len = guide_msg.len(),
                    "guide message found at turn end, continuing iteration"
                );
                // Keep the model's final text as assistant message before injecting guide
                let content = response
                    .content
                    .or(response.reasoning_content)
                    .unwrap_or_default();
                if !content.is_empty() {
                    if let Some(sender) = &session.progress_sender {
                        let _ = sender.send(AgentTurnProgressEvent::AssistantDelta {
                            content: content.clone(),
                        });
                    }
                    session.add_assistant_message(&content);
                    if let Some(ref mut rec) = recorder {
                        let response_tokens =
                            response.usage.as_ref().map(|usage| usage.total_tokens);
                        let context_tokens =
                            response.usage.as_ref().map(|usage| usage.context_tokens());
                        if let Err(e) = rec.record_assistant_message_with_token_usage(
                            &session.session_key,
                            &content,
                            response_tokens,
                            context_tokens,
                        ) {
                            warn!(error = %e, "failed to record assistant message (guide inject)");
                        }
                    }
                }
                session.add_user_message(&guide_msg);
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_user_message(&session.session_key, &guide_msg) {
                        warn!(error = %e, "failed to record guide message at turn end");
                    }
                }
                compact_mid_turn_if_needed(
                    client,
                    config,
                    session,
                    &mut recorder,
                    &prompt_prefix,
                    memory_message.as_ref(),
                    ActiveTurnProgress {
                        state: "model_response",
                        current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                        completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                        max_loop_iterations: config.get_max_turn_iterations(),
                        local_tool_count,
                        mcp_tool_count,
                    },
                    &turn_context.cancellation_token,
                )
                .await?;
                continue;
            }

            // Pending queue check: if messages are queued (e.g. from IM interleave),
            // pop the next one and continue the turn loop instead of returning.
            if let Some(queued_msg) = pending_input::pop_next_non_empty(session) {
                info!(
                    session_key = %session.session_key,
                    queued_msg_len = queued_msg.len(),
                    remaining_queue = session.pending_messages.len(),
                    "processing queued message from pending_messages"
                );
                // Keep the model's final text as assistant message
                let content = response
                    .content
                    .or(response.reasoning_content)
                    .unwrap_or_default();
                if !content.is_empty() {
                    progress::assistant_delta(session.progress_sender.as_ref(), content.clone());
                    session.add_assistant_message(&content);
                    if let Some(ref mut rec) = recorder {
                        let response_tokens =
                            response.usage.as_ref().map(|usage| usage.total_tokens);
                        let context_tokens =
                            response.usage.as_ref().map(|usage| usage.context_tokens());
                        if let Err(e) = rec.record_assistant_message_with_token_usage(
                            &session.session_key,
                            &content,
                            response_tokens,
                            context_tokens,
                        ) {
                            warn!(error = %e, "failed to record assistant message (queue)");
                        }
                    }
                }
                session.add_user_message(&queued_msg);
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_user_message(&session.session_key, &queued_msg) {
                        warn!(error = %e, "failed to record queued message");
                    }
                }
                compact_mid_turn_if_needed(
                    client,
                    config,
                    session,
                    &mut recorder,
                    &prompt_prefix,
                    memory_message.as_ref(),
                    ActiveTurnProgress {
                        state: "model_response",
                        current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                        completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                        max_loop_iterations: config.get_max_turn_iterations(),
                        local_tool_count,
                        mcp_tool_count,
                    },
                    &turn_context.cancellation_token,
                )
                .await?;
                continue;
            }

            // Model finished — extract text response
            let content = response
                .content
                .or(response.reasoning_content)
                .unwrap_or_default();
            crate::tools::goal::goal_runtime_apply(
                session,
                crate::tools::goal::GoalRuntimeEvent::TurnFinished {
                    turn_completed: true,
                },
            );

            session.add_assistant_message(&content);

            // Record assistant message
            if let Some(ref mut rec) = recorder {
                let response_tokens = response.usage.as_ref().map(|usage| usage.total_tokens);
                let context_tokens = response.usage.as_ref().map(|usage| usage.context_tokens());
                if let Err(e) = rec.record_assistant_message_with_token_usage(
                    &session.session_key,
                    &content,
                    response_tokens,
                    context_tokens,
                ) {
                    warn!(error = %e, "failed to record assistant message");
                }
            }

            info!(
                session_key = %session.session_key,
                iterations = iteration + 1,
                tool_calls = tool_calls_log.len(),
                total_tokens_used = ?session.total_tokens_used,
                "agent turn completed"
            );

            // Pollution-aware memory extraction (skips if session is polluted).
            memory::auto_extract_after_turn_with_pollution_check(
                std::sync::Arc::new(client.clone()),
                config.clone(),
                session.session_key.clone(),
                user_message.to_string(),
                content.clone(),
                session.pollution_detector.clone(),
            );

            // Citation tracking via CitationConsumer (Codex parity).
            let citation_result = session.citation_consumer.process_turn_citations(&content);
            if citation_result.entry_count > 0 {
                tracing::debug!(
                    entries = citation_result.entry_count,
                    rollout_ids = citation_result.rollout_id_count,
                    "Citation consumer processed turn citations"
                );
                let usage_tracker =
                    crate::memory::usage_tracking::UsageTracker::new(&crate::memory::memory_root());
                if let Err(error) =
                    usage_tracker.record_citation_usage(&citation_result.cited_rollout_ids)
                {
                    warn!(error = %error, "failed to record memory citation usage");
                }
            }

            session.record_turn_event(CodexTurnEventKind::TurnCompleted);
            // Record TTFM — first complete message to user (Codex parity).
            if let Some(ttfm) = turn_timing.record_first_message() {
                debug!(ttfm_ms = ttfm.as_millis(), "recorded TTFM");
            }
            // Log turn timing summary for observability.
            let timing_summary = super::turn_timing::TurnTimingSummary::from_state(&turn_timing);
            tracing::info!(
                ttft_ms = ?timing_summary.ttft_ms,
                ttfm_ms = ?timing_summary.ttfm_ms,
                total_ms = ?timing_summary.total_ms,
                "Turn timing recorded"
            );

            // Dispatch turn-complete hook to all registered lifecycle hooks.
            {
                let outcome = super::hooks::TurnOutcome::Completed {
                    last_message: Some(content.clone()),
                    needs_follow_up: session
                        .current_goal
                        .as_ref()
                        .is_some_and(|g| g.status == crate::tools::goal::GoalStatus::Active),
                };
                session
                    .hook_registry
                    .dispatch_turn_complete(&turn_context, &outcome, &timing_summary)
                    .await;
            }

            progress::assistant_final(session.progress_sender.as_ref(), content.clone());
            return Ok(TurnResult {
                response: content,
                tool_calls_log,
                work_dir_switched: None,
                title_updated: session.title.clone(),
                plan_steps: session.current_plan.clone(),
                goal_needs_continuation: session
                    .current_goal
                    .as_ref()
                    .is_some_and(|g| g.status == crate::tools::goal::GoalStatus::Active),
                goal_objective: session
                    .current_goal
                    .as_ref()
                    .filter(|g| g.status == crate::tools::goal::GoalStatus::Active)
                    .map(|g| g.objective.clone()),
            });
        }

        // Model wants to call tools
        info!(
            iteration,
            tool_count = response.tool_calls.len(),
            "model requested tool calls"
        );
        let process_content = response
            .content
            .clone()
            .or(response.reasoning_content.clone())
            .unwrap_or_default();
        if !process_content.trim().is_empty() {
            if let Some(sender) = &session.progress_sender {
                let _ = sender.send(AgentTurnProgressEvent::AssistantDelta {
                    content: process_content.clone(),
                });
            }
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_assistant_delta(&session.session_key, &process_content) {
                    warn!(error = %e, "failed to record assistant delta");
                }
            }
        }
        refresh_active_turn_status(
            session,
            config,
            ActiveTurnProgress {
                state: "tool_calls",
                current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                max_loop_iterations: config.get_max_turn_iterations(),
                local_tool_count,
                mcp_tool_count,
            },
        );

        // Record the assistant message with tool calls
        session.add_assistant_tool_calls(&response.tool_calls);
        session.record_turn_event_entry(
            CodexTurnEvent::new(0, CodexTurnEventKind::AssistantToolCallsRecorded)
                .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                .with_detail(format!("tool_calls={}", response.tool_calls.len())),
        );

        // Execute tool calls as ordered batches. Local tools that do not
        // mutate turn/session routing state run through FuturesOrdered, which
        // lets work overlap while still appending tool results to history in the
        // model-provided order. Stateful tools, tool_search, and MCP tools stay
        // ordered so their side effects are visible at the correct point.
        let tool_orchestrator = ToolOrchestrator::open();
        let mut tool_index = 0;
        while tool_index < response.tool_calls.len() {
            let tc = &response.tool_calls[tool_index];
            if stop_requested(session) {
                info!(
                    session_key = %session.session_key,
                    tool = %tc.name(),
                    "agent turn stopped before tool call"
                );
                append_cancelled_tool_results(
                    session,
                    &mut recorder,
                    &mut tool_calls_log,
                    &response.tool_calls,
                    tool_index,
                );
                return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
            }

            if is_parallel_local_tool_call(tools, mcp.as_deref(), tc.name()) {
                let batch_start = tool_index;
                let mut batch_end = batch_start;
                while batch_end < response.tool_calls.len()
                    && is_parallel_local_tool_call(
                        tools,
                        mcp.as_deref(),
                        response.tool_calls[batch_end].name(),
                    )
                {
                    batch_end += 1;
                }

                session.record_turn_event_entry(
                    CodexTurnEvent::new(0, CodexTurnEventKind::ToolBatchStarted)
                        .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                        .with_detail(format!(
                            "mode=parallel,count={}",
                            batch_end.saturating_sub(batch_start)
                        )),
                );

                let mut futures = FuturesOrdered::new();
                for batch_index in batch_start..batch_end {
                    let batch_tc = &response.tool_calls[batch_index];
                    let tool_started_at =
                        record_tool_call_started(session, &mut recorder, batch_tc, iteration);
                    let handler = tools
                        .handler(batch_tc.name())
                        .expect("parallel local tool was resolved before scheduling");
                    let arguments = batch_tc.arguments().to_string();
                    let batch_work_dir = work_dir.clone();
                    let orchestrator = tool_orchestrator.clone();
                    let tool_name = batch_tc.name().to_string();
                    futures.push_back(async move {
                        let result = tool_dispatch::run_local_handler(
                            &orchestrator,
                            &tool_name,
                            &arguments,
                            &batch_work_dir,
                            handler,
                        )
                        .await;
                        (batch_index, result, tool_started_at)
                    });
                }

                let mut next_cancel_index = batch_start;
                let stop_signal = session.stop_signal.clone();
                while !futures.is_empty() {
                    let next = if let Some(signal) = stop_signal.as_ref() {
                        tokio::select! {
                            result = futures.next() => Some(result),
                            _ = signal.cancelled() => None,
                        }
                    } else {
                        Some(futures.next().await)
                    };

                    let Some(Some((completed_index, result, tool_started_at))) = next else {
                        info!(
                            session_key = %session.session_key,
                            "agent turn stopped during parallel tool batch"
                        );
                        append_cancelled_tool_results(
                            session,
                            &mut recorder,
                            &mut tool_calls_log,
                            &response.tool_calls,
                            next_cancel_index,
                        );
                        return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
                    };
                    next_cancel_index = completed_index.saturating_add(1);
                    let completed_tc = &response.tool_calls[completed_index];
                    apply_tool_call_completion(
                        session,
                        &mut recorder,
                        &mut tool_calls_log,
                        &mut tool_defs,
                        &mut visible_tool_names,
                        config,
                        completed_tc,
                        result,
                        tool_started_at,
                        tool_output_limit,
                        iteration,
                        local_tool_count,
                        mcp_tool_count,
                    );
                }

                session.record_turn_event_entry(
                    CodexTurnEvent::new(0, CodexTurnEventKind::ToolBatchCompleted)
                        .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                        .with_detail(format!(
                            "mode=parallel,count={}",
                            batch_end.saturating_sub(batch_start)
                        )),
                );
                tool_index = batch_end;
                continue;
            }

            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::ToolBatchStarted)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_detail("mode=ordered,count=1"),
            );
            let tool_started_at = record_tool_call_started(session, &mut recorder, tc, iteration);
            let stop_signal = session.stop_signal.clone();
            let tool_execution = async {
                if let Some(result) =
                    crate::tools::goal::execute_goal_tool(session, tc.name(), tc.arguments())
                {
                    result
                } else if tc.name() == TOOL_SEARCH_TOOL_NAME {
                    match tool_search_tool.as_ref() {
                        Some(tool) => {
                            tool_orchestrator
                                .run_local(tc.name(), &work_dir, || async {
                                    tool.execute(tc.arguments(), &work_dir).await
                                })
                                .await
                        }
                        None => crate::types::ToolResult {
                            success: false,
                            output:
                                "tool_search is unavailable because no deferred tools are active"
                                    .to_string(),
                            runtime_events: Vec::new(),
                        },
                    }
                } else if mcp.as_ref().is_some_and(|m| m.is_mcp_tool(tc.name())) {
                    // Route to MCP server
                    match mcp.as_mut() {
                        Some(m) => {
                            tool_dispatch::run_mcp(&tool_orchestrator, m, tc.name(), tc.arguments())
                                .await
                        }
                        None => crate::types::ToolResult {
                            success: false,
                            output: "MCP manager unavailable".to_string(),
                            runtime_events: Vec::new(),
                        },
                    }
                } else {
                    // Route to local tool registry
                    tool_dispatch::run_local_registry(
                        &tool_orchestrator,
                        tools,
                        tc.name(),
                        tc.arguments(),
                        &work_dir,
                    )
                    .await
                }
            };
            let result = if let Some(signal) = stop_signal {
                tokio::select! {
                    result = tool_execution => Some(result),
                    _ = signal.cancelled() => None,
                }
            } else {
                Some(tool_execution.await)
            };
            let Some(result) = result else {
                info!(
                    session_key = %session.session_key,
                    tool = %tc.name(),
                    "agent turn stopped during tool call"
                );
                append_cancelled_tool_results(
                    session,
                    &mut recorder,
                    &mut tool_calls_log,
                    &response.tool_calls,
                    tool_index,
                );
                return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
            };
            let result = match maybe_watch_exec_long_task(
                session,
                config,
                tools,
                tc,
                result,
                iteration,
                tool_output_limit,
                local_tool_count,
                mcp_tool_count,
            )
            .await
            {
                LongTaskWatchOutcome::Completed(result) => result,
                LongTaskWatchOutcome::Stopped => {
                    append_cancelled_tool_results(
                        session,
                        &mut recorder,
                        &mut tool_calls_log,
                        &response.tool_calls,
                        tool_index,
                    );
                    return Ok(stopped_turn_result(session, &mut recorder, tool_calls_log));
                }
            };
            apply_tool_call_completion(
                session,
                &mut recorder,
                &mut tool_calls_log,
                &mut tool_defs,
                &mut visible_tool_names,
                config,
                tc,
                result,
                tool_started_at,
                tool_output_limit,
                iteration,
                local_tool_count,
                mcp_tool_count,
            );
            // Mark session as polluted if this was an MCP tool call (external context).
            if mcp.as_ref().is_some_and(|m| m.is_mcp_tool(tc.name())) {
                session
                    .pollution_detector
                    .mark_polluted_if_external_context(tc.name());
            }
            session.record_turn_event_entry(
                CodexTurnEvent::new(0, CodexTurnEventKind::ToolBatchCompleted)
                    .with_iteration(u32::try_from(iteration + 1).unwrap_or(u32::MAX))
                    .with_detail("mode=ordered,count=1"),
            );
            tool_index += 1;
        }

        // Check if switch_workdir was called — if so, apply the switch and exit the turn
        if let Some(switch_log) = tool_calls_log
            .iter()
            .find(|l| l.tool_name == "switch_workdir" && l.success)
        {
            if let Some(new_dir) = switch_log.result.strip_prefix("SWITCH_WORKDIR:") {
                let new_dir = new_dir.to_string();
                info!(
                    session_key = %session.session_key,
                    new_work_dir = %new_dir,
                    "switching session work directory"
                );
                session.reinitialize_work_dir(new_dir.clone());
                session.record_turn_event(CodexTurnEventKind::TurnCompleted);
                return Ok(TurnResult {
                    response: format!(
                        "已切换工作目录到: {}\n\n会话历史已清空，已重新加载项目配置。",
                        new_dir
                    ),
                    tool_calls_log,
                    work_dir_switched: Some(new_dir),
                    title_updated: None,
                    plan_steps: None,
                    goal_needs_continuation: false,
                    goal_objective: None,
                });
            }
        }

        // Check if set_title was called — if so, update session.title
        if let Some(title_log) = tool_calls_log
            .iter()
            .rfind(|l| l.tool_name == "set_title" && l.success)
        {
            if let Some(new_title) = title_log.result.strip_prefix("SET_TITLE:") {
                session.title = Some(new_title.to_string());
                if let Some(sender) = &session.progress_sender {
                    let _ = sender.send(AgentTurnProgressEvent::TitleUpdated {
                        title: new_title.to_string(),
                    });
                }
                // Persist title update event
                if let Some(ref mut rec) = recorder {
                    if let Err(e) = rec.record_title_updated(&session.session_key, new_title) {
                        warn!(error = %e, "failed to record title update");
                    }
                }
            }
        }

        // Guide-mode injection: check if an IM user sent a guide message while
        // this tool call batch was executing. If so, append it to history so the
        // model sees it in the next iteration (before compaction to preserve it).
        let guide_messages = drain_guide_messages(session);
        if let Some(guide_msg) = combine_guide_messages(guide_messages) {
            info!(
                session_key = %session.session_key,
                guide_msg_len = guide_msg.len(),
                "guide message injected mid-turn"
            );
            session.add_user_message(&guide_msg);
            if let Some(ref mut rec) = recorder {
                if let Err(e) = rec.record_user_message(&session.session_key, &guide_msg) {
                    warn!(error = %e, "failed to record guide message");
                }
            }
        }

        // Mid-turn compaction check (same pattern as Codex's auto-compact in turn.rs)
        compact_mid_turn_if_needed(
            client,
            config,
            session,
            &mut recorder,
            &prompt_prefix,
            memory_message.as_ref(),
            ActiveTurnProgress {
                state: "tool_calls",
                current_loop_iteration: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                completed_loop_iterations: u32::try_from(iteration + 1).unwrap_or(u32::MAX),
                max_loop_iterations: config.get_max_turn_iterations(),
                local_tool_count,
                mcp_tool_count,
            },
            &turn_context.cancellation_token,
        )
        .await?;
    }

    error!(
        session_key = %session.session_key,
        max_iterations,
        "agent turn exceeded max iterations"
    );
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::TaskAborted {
            reason: crate::tools::goal::GoalAbortReason::Interrupted,
        },
    );
    // Dispatch turn-abort hook for max-iterations exceeded.
    {
        let timing_summary = super::turn_timing::TurnTimingSummary::from_state(&turn_timing);
        let outcome = super::hooks::TurnOutcome::Failed {
            error: format!("exceeded maximum iterations ({max_iterations})"),
        };
        session
            .hook_registry
            .dispatch_turn_abort(&turn_context, &outcome, &timing_summary)
            .await;
    }
    Err(format!("exceeded maximum iterations ({max_iterations})"))
}

async fn handle_goal_command(session: &mut AgentSession, args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" || trimmed == "status" {
        return crate::tools::goal::execute_goal_tool(session, "get_goal", "{}")
            .expect("get_goal handler")
            .output;
    }

    // External mutation starting: flush accounting before we modify goal state.
    crate::tools::goal::goal_runtime_apply(
        session,
        crate::tools::goal::GoalRuntimeEvent::ExternalMutationStarting,
    );

    if trimmed == "complete" {
        return crate::tools::goal::execute_goal_tool(
            session,
            "update_goal",
            r#"{"status":"complete"}"#,
        )
        .expect("update_goal handler")
        .output;
    }

    if trimmed == "pause" {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::ExternalSet {
                status: crate::tools::goal::GoalStatus::Paused,
            },
        );
        return crate::tools::goal::execute_goal_tool(session, "get_goal", "{}")
            .expect("get_goal handler")
            .output;
    }

    if trimmed == "resume" {
        crate::tools::goal::goal_runtime_apply(
            session,
            crate::tools::goal::GoalRuntimeEvent::ExternalSet {
                status: crate::tools::goal::GoalStatus::Active,
            },
        );
        return crate::tools::goal::execute_goal_tool(session, "get_goal", "{}")
            .expect("get_goal handler")
            .output;
    }

    if let Some(rest) = trimmed.strip_prefix("set ") {
        let rest = rest.trim();
        if rest.is_empty() {
            return "用法: /goal [show|set <objective>|set --budget N <objective>|pause|resume|complete]"
                .to_string();
        }

        let (token_budget, objective) = if let Some(without_flag) = rest.strip_prefix("--budget ") {
            let without_flag = without_flag.trim();
            let Some((budget, objective)) = without_flag.split_once(char::is_whitespace) else {
                return "用法: /goal set --budget <N> <objective>".to_string();
            };
            let Ok(token_budget) = budget.parse::<u64>() else {
                return "goal budget 必须是正整数。".to_string();
            };
            (Some(token_budget), objective.trim())
        } else {
            (None, rest)
        };

        if objective.is_empty() {
            return "goal objective 不能为空。".to_string();
        }

        return crate::tools::goal::execute_goal_tool(
            session,
            "create_goal",
            &serde_json::json!({
                "objective": objective,
                "token_budget": token_budget,
            })
            .to_string(),
        )
        .expect("create_goal handler")
        .output;
    }

    "用法: /goal [show|set <objective>|set --budget N <objective>|pause|resume|complete]"
        .to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Maximum bytes for telemetry previews (matching Codex's TELEMETRY_PREVIEW_MAX_BYTES).
const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024; // 2 KiB

/// Maximum lines for telemetry previews (matching Codex's TELEMETRY_PREVIEW_MAX_LINES).
const TELEMETRY_PREVIEW_MAX_LINES: usize = 64;

/// Create a truncated preview of tool output for telemetry/logging.
/// Truncates both by bytes and lines, whichever limit is hit first.
fn telemetry_preview(output: &str) -> String {
    // First truncate by bytes (using safe UTF-8 boundary truncation)
    let truncated = if output.len() > TELEMETRY_PREVIEW_MAX_BYTES {
        let end = bifrost_core::text::floor_char_boundary(output, TELEMETRY_PREVIEW_MAX_BYTES);
        format!("{}... ({} bytes total)", &output[..end], output.len())
    } else {
        output.to_string()
    };

    // Then truncate by lines
    let lines: Vec<&str> = truncated.lines().collect();
    if lines.len() > TELEMETRY_PREVIEW_MAX_LINES {
        let kept: Vec<&str> = lines
            .into_iter()
            .take(TELEMETRY_PREVIEW_MAX_LINES)
            .collect();
        format!("{}\n... (more lines omitted)", kept.join("\n"))
    } else {
        truncated
    }
}

pub(super) fn apply_plan_update_snapshot(
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    args: crate::tools::update_plan::UpdatePlanArgs,
) {
    let steps = args.plan;
    if steps.is_empty()
        && session
            .current_plan
            .as_deref()
            .is_some_and(plan_has_unfinished_steps)
    {
        warn!(
            session_key = %session.session_key,
            explanation = ?args.explanation,
            "ignored empty update_plan snapshot while current plan has unfinished steps"
        );
        return;
    }
    session.current_plan = if steps.is_empty() {
        None
    } else {
        Some(steps.clone())
    };
    session.plan_repair_attempts = 0;
    if let Some(ref mut rec) = recorder {
        if steps.is_empty() {
            let reason = args
                .explanation
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("update_plan_empty_snapshot");
            if let Err(e) = rec.record_plan_cleared(&session.session_key, reason) {
                warn!(error = %e, "failed to record plan clear");
            }
        } else if let Err(e) =
            rec.record_plan_updated(&session.session_key, &steps, args.explanation.as_deref())
        {
            warn!(error = %e, "failed to record plan update");
        }
    }
    if let Some(sender) = &session.progress_sender {
        let _ = sender.send(AgentTurnProgressEvent::PlanUpdated {
            steps: steps.clone(),
            title: session.title.clone(),
        });
    }
    if let Some(ref sender) = session.plan_sender {
        if let Err(e) = sender.send((steps, session.title.clone())) {
            debug!(error = %e, "plan_sender channel closed");
        }
    }
}

pub(super) fn plan_is_complete(plan: &[crate::tools::update_plan::PlanStep]) -> bool {
    !plan.is_empty() && !plan_has_unfinished_steps(plan)
}

pub(super) fn plan_has_unfinished_steps(plan: &[crate::tools::update_plan::PlanStep]) -> bool {
    plan.iter().any(|step| {
        matches!(
            step.status,
            crate::tools::update_plan::PlanStepStatus::Pending
                | crate::tools::update_plan::PlanStepStatus::InProgress
        )
    })
}

/// Truncate tool output to stay within a token budget.
///
/// Uses the approximation of 1 token ≈ 4 characters. If the output exceeds
/// `max_tokens * 4` characters, removes the middle section and inserts a marker.
pub(super) fn truncate_tool_output(output: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if output.len() <= max_chars {
        return output.to_string();
    }

    // Include total line count so the model knows the full output size
    // (matching Codex's formatted_truncate_text behavior)
    let total_lines = output.lines().count();

    // Keep first half and last half, remove middle
    let half = max_chars / 2;
    let head_end = output.floor_char_boundary(half);
    let tail_start = output.ceil_char_boundary(output.len() - half);
    let head = &output[..head_end];
    let tail = &output[tail_start..];
    let removed = output.len() - max_chars;
    format!(
        "Total output lines: {total_lines}\n\n{}\n\n... [{} characters truncated] ...\n\n{}",
        head, removed, tail
    )
}

/// Build initial context items for mid-turn compaction injection.
///
/// Inserted before the last user message in the compacted history via
/// [`compact::InitialContextInjection::BeforeLastUserMessage`] to preserve
/// continuity within the turn loop. Base/system instructions stay outside
/// replacement history, matching Codex's `Prompt.base_instructions` boundary;
/// only developer/contextual user prompt items plus optional memory are
/// reinjected.
pub(super) fn build_mid_turn_initial_context(
    _session: &AgentSession,
    prompt_prefix: &[ChatMessage],
    memory_message: Option<&ChatMessage>,
) -> Vec<ChatMessage> {
    let mut context =
        Vec::with_capacity(prompt_prefix.len() + usize::from(memory_message.is_some()));
    context.extend(
        prompt_prefix
            .iter()
            .filter(|message| message.role != "system")
            .cloned(),
    );
    if let Some(memory_message) = memory_message {
        context.push(memory_message.clone());
    }
    context
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn compact_mid_turn_if_needed(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    prompt_prefix: &[ChatMessage],
    memory_message: Option<&ChatMessage>,
    progress: ActiveTurnProgress,
    cancellation_token: &crate::turn_runtime::CancellationToken,
) -> Result<(), String> {
    if !compact::should_compact(session, config) {
        return Ok(());
    }

    info!(
        estimated_tokens = session.estimate_tokens(),
        compaction_count = session.compaction_count,
        "triggering mid-turn compaction"
    );
    let initial_context = build_mid_turn_initial_context(session, prompt_prefix, memory_message);
    match compact::compact_session_with_hooks(
        client,
        config,
        session,
        compact::CompactionTrigger::Auto,
        compact::CompactionReason::ContextLimit,
        compact::CompactionPhase::MidTurn,
        prompt_prefix
            .iter()
            .find(|message| message.role == "system")
            .and_then(|message| message.content.as_deref()),
        compact::InitialContextInjection::BeforeLastUserMessage,
        initial_context,
        cancellation_token,
    )
    .await
    {
        Ok(result) if result.performed => {
            record_compaction_event(
                recorder,
                session,
                &result,
                "auto",
                "context_limit",
                "mid_turn",
                false,
            );
            info!(
                tokens_saved = result.tokens_saved,
                duration_ms = ?result.duration_ms,
                "mid-turn compaction succeeded"
            );
            refresh_active_turn_status(session, config, progress);
        }
        Ok(_) => {}
        Err(e) => {
            warn!(error = %e, "mid-turn compaction failed");
            crate::tools::goal::goal_runtime_apply(
                session,
                crate::tools::goal::GoalRuntimeEvent::TaskAborted {
                    reason: crate::tools::goal::GoalAbortReason::Interrupted,
                },
            );
            return Err(e);
        }
    }
    Ok(())
}

async fn compact_pre_sampling_if_needed(
    client: &AgentClient,
    config: &AgentConfig,
    session: &mut AgentSession,
    recorder: &mut Option<&mut ConversationRecorder>,
    base_instructions: &str,
) -> Result<(), String> {
    if !compact::should_compact(session, config) {
        return Ok(());
    }

    info!(
        session_key = %session.session_key,
        estimated_tokens = session.effective_token_count(),
        threshold = config.get_compact_threshold_tokens(),
        "pre-sampling token limit reached; running compaction before adding new input"
    );

    let result = compact::compact_session_with_hooks(
        client,
        config,
        session,
        compact::CompactionTrigger::Auto,
        compact::CompactionReason::ContextLimit,
        compact::CompactionPhase::PreTurn,
        Some(base_instructions),
        compact::InitialContextInjection::DoNotInject,
        vec![],
        &crate::turn_runtime::CancellationToken::default_grace(),
    )
    .await?;

    if result.performed {
        record_compaction_event(
            recorder,
            session,
            &result,
            "auto",
            "context_limit",
            "pre_turn",
            false,
        );
        info!(
            tokens_saved = result.tokens_saved,
            duration_ms = ?result.duration_ms,
            "pre-sampling compact completed"
        );
    }

    Ok(())
}

pub(super) fn record_compaction_event(
    recorder: &mut Option<&mut ConversationRecorder>,
    session: &AgentSession,
    result: &compact::CompactionResult,
    trigger: &str,
    reason: &str,
    phase: &str,
    emergency: bool,
) {
    let mut metadata = serde_json::json!({
        "trigger": trigger,
        "reason": reason,
        "phase": phase,
        "pre_tokens": result.pre_tokens,
        "post_tokens": result.post_tokens,
        "tokens_saved": result.tokens_saved,
        "messages_removed": result.messages_removed,
        "compaction_count": session.compaction_count,
        "total_tokens": session.total_tokens_used.unwrap_or(0),
        "replacement_history": &session.history,
        "current_plan": &session.current_plan,
    });
    if emergency {
        metadata["emergency"] = serde_json::Value::Bool(true);
    }

    if let Some(rec) = recorder.as_deref_mut() {
        if let Err(e) = rec.record_compaction(&session.session_key, metadata) {
            warn!(error = %e, "failed to record compaction event");
        }
    }
}

/// Check if an API error indicates context window overflow.
/// Matches common patterns from OpenAI/Azure/ModelHub error messages.
pub(super) fn is_context_window_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("maximum context length")
        || lower.contains("token limit")
        || (lower.contains("too many tokens") && lower.contains("max"))
}

/// Check if an API error is transient and worth retrying.
pub(super) fn is_retryable_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("429")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("connection")
        || lower.contains("temporary")
        || lower.contains("server error")
        || lower.contains("internal error")
        || lower.contains("service unavailable")
        || lower.contains("gateway")
        || lower.contains("overloaded")
        || lower.contains("try again")
        || lower.contains("http request failed")
}

fn mark_context_window_full(
    session: &mut AgentSession,
    config: &AgentConfig,
    progress: ActiveTurnProgress,
) {
    if let Some(context_window) = config_context_window_tokens(config) {
        session.restore_token_snapshot(Some(u64::from(context_window)));
    }
    refresh_active_turn_status(session, config, progress);
}
