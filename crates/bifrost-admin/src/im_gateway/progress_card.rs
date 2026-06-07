use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{info, warn};

use bifrost_agent::{
    ActiveTurnStatus, AgentContextSnapshot, AgentTurnProgressEvent, PlanStep, PlanStepStatus,
    ToolCallLog,
};
use bifrost_core::{BifrostError, Result};

use super::feishu::FeishuProvider;
use super::queue_manager::QueueItem;
use super::types::{ImProviderConfig, ImTarget, SendResult};

const OUTPUT_ELEMENT_ID: &str = "agent_output";
const PLAN_PANEL_ELEMENT_ID: &str = "agent_plan_panel";
const PLAN_ELEMENT_ID: &str = "agent_plan";
const TOOL_PANEL_ELEMENT_ID: &str = "agent_tool_panel";
const TOOL_LOG_ELEMENT_ID: &str = "agent_tool_log";
const STATUS_PANEL_ELEMENT_ID: &str = "agent_status_panel";
const FOOTER_ELEMENT_ID: &str = "agent_footer";
const THINKING_PANEL_ELEMENT_ID: &str = "agent_thinking_panel";
const PROCESS_PANEL_ELEMENT_ID: &str = "agent_process_panel";
const PROCESS_LOG_ELEMENT_ID: &str = "agent_process_log";
const PROCESS_DYNAMIC_LOG_ELEMENT_PREFIX: &str = "ap_log";
const PROCESS_TOOL_ELEMENT_PREFIX: &str = "ap_t";
const PROCESS_TOOL_DETAIL_ELEMENT_PREFIX: &str = "ap_td";
const PROCESS_TOOL_GROUP_ELEMENT_PREFIX: &str = "ap_tg";
const PROCESS_TOOL_INPUT_PREVIEW_CHARS: usize = 300;
const PROCESS_TOOL_OUTPUT_PREVIEW_CHARS: usize = 700;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImProgressCardCapability {
    StreamingCard,
    PatchMessage,
    SendOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImProgressPhase {
    Running,
    Finished,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ImAgentProgressSnapshot {
    pub session_key: String,
    pub title: Option<String>,
    pub output: String,
    pub last_thought: Option<String>,
    pub runner: Option<ProgressRunnerSummary>,
    pub plan_steps: Vec<PlanStep>,
    pub proposed_plan: Option<String>,
    pub tool_calls: Vec<ToolCallLog>,
    pub latest_tool: Option<ProgressToolSummary>,
    pub timeline: Vec<ProgressTimelineItem>,
    pub status: Option<ActiveTurnStatus>,
    pub context: Option<AgentContextSnapshot>,
    pub queue_items: Vec<QueueItem>,
    pub guide_pending: bool,
    pub activity_notice: Option<String>,
    pub phase: ImProgressPhase,
}

impl ImAgentProgressSnapshot {
    pub fn new(session_key: impl Into<String>, initial_message: &str) -> Self {
        Self {
            session_key: session_key.into(),
            title: Some(default_card_title(initial_message)),
            output: String::new(),
            last_thought: None,
            runner: None,
            plan_steps: Vec::new(),
            proposed_plan: None,
            tool_calls: Vec::new(),
            latest_tool: None,
            timeline: Vec::new(),
            status: None,
            context: None,
            queue_items: Vec::new(),
            guide_pending: false,
            activity_notice: None,
            phase: ImProgressPhase::Running,
        }
    }

    pub fn apply_event(&mut self, event: AgentTurnProgressEvent) {
        match event {
            AgentTurnProgressEvent::Status(status) => {
                let state = status.state.trim();
                if is_human_readable_progress_status(state) {
                    self.push_timeline(ProgressTimelineItem::status(state.to_string()));
                }
                self.context = Some(context_snapshot_from_status(&status));
                self.status = Some(*status);
            }
            AgentTurnProgressEvent::ContextUpdated { context } => {
                self.apply_context_snapshot(context);
            }
            AgentTurnProgressEvent::CompactionStarted { progress } => {
                self.apply_context_snapshot(progress.context);
            }
            AgentTurnProgressEvent::CompactionFinished { progress } => {
                self.apply_context_snapshot(progress.context);
            }
            AgentTurnProgressEvent::CompactionFailed { progress, .. } => {
                self.apply_context_snapshot(progress.context);
            }
            AgentTurnProgressEvent::ToolStarted {
                tool_name,
                arguments,
            } => {
                self.latest_tool = Some(ProgressToolSummary {
                    tool_name: tool_name.clone(),
                    arguments: Some(arguments.clone()),
                    success: None,
                    result_preview: None,
                    duration_ms: None,
                });
                let summary = format!("正在运行 `{}`", truncate_one_line(&tool_name, 32));
                self.upsert_running_timeline_tool(tool_name, arguments, summary);
            }
            AgentTurnProgressEvent::ToolFinished { log, duration_ms } => {
                self.latest_tool = Some(ProgressToolSummary {
                    tool_name: log.tool_name.clone(),
                    arguments: Some(log.arguments.clone()),
                    success: Some(log.success),
                    result_preview: Some(truncate_str(&log.result, 160)),
                    duration_ms: Some(duration_ms),
                });
                self.finish_timeline_tool(&log, duration_ms);
                self.tool_calls.push(log);
            }
            AgentTurnProgressEvent::LongTaskStatus {
                session_id,
                profile,
                state,
                elapsed_ms,
                last_output_preview,
                next_check_at_ms,
                unchanged_heartbeats,
                ..
            } => {
                let mut preview =
                    format!("state={state}, profile={profile}, elapsed={}ms", elapsed_ms);
                if let Some(output) = last_output_preview.filter(|value| !value.trim().is_empty()) {
                    preview.push_str(&format!(", last={}", truncate_str(&output, 120)));
                }
                if let Some(next_check_at_ms) = next_check_at_ms {
                    preview.push_str(&format!(", next_check_at={next_check_at_ms}"));
                }
                if unchanged_heartbeats > 0 {
                    preview.push_str(&format!(", unchanged_heartbeats={unchanged_heartbeats}"));
                }
                self.latest_tool = Some(ProgressToolSummary {
                    tool_name: "exec_command".to_string(),
                    arguments: Some(format!("session_id={session_id}")),
                    success: None,
                    result_preview: Some(preview),
                    duration_ms: Some(elapsed_ms),
                });
                self.upsert_running_timeline_tool(
                    "exec_command".to_string(),
                    format!("session_id={session_id}"),
                    format!("长任务状态：{state}，已运行 {elapsed_ms}ms"),
                );
            }
            AgentTurnProgressEvent::PlanUpdated { steps, title } => {
                self.plan_steps = steps;
                if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
                    self.title = Some(title);
                }
            }
            AgentTurnProgressEvent::ProposedPlan { content } => {
                if !content.trim().is_empty() {
                    self.proposed_plan = Some(content);
                    self.title = Some("实施方案".to_string());
                }
            }
            AgentTurnProgressEvent::TitleUpdated { title } => {
                if !title.trim().is_empty() {
                    self.title = Some(title);
                }
            }
            AgentTurnProgressEvent::AssistantDelta { content } => {
                if !content.trim().is_empty() {
                    self.push_timeline(ProgressTimelineItem::thinking(content.clone()));
                    self.last_thought = Some(content);
                }
            }
            AgentTurnProgressEvent::AssistantFinal { content } => {
                if !content.trim().is_empty() {
                    if matches!(self.phase, ImProgressPhase::Running) {
                        self.push_timeline(ProgressTimelineItem::thinking(content.clone()));
                        self.last_thought = Some(content);
                    } else {
                        self.output = content;
                    }
                }
            }
            AgentTurnProgressEvent::TurnFinished { content } => {
                self.phase = ImProgressPhase::Finished;
                if !content.trim().is_empty() {
                    self.output = content;
                }
            }
            AgentTurnProgressEvent::TurnFailed { error } => {
                self.phase = ImProgressPhase::Failed;
                self.push_timeline(ProgressTimelineItem::status(format!(
                    "执行失败：{}",
                    truncate_one_line(&error, 120)
                )));
                self.output = format!("Agent 执行失败：{}", truncate_str(&error, 300));
            }
        }
    }

    fn push_timeline(&mut self, item: ProgressTimelineItem) {
        self.timeline.push(item);
    }

    fn finish_timeline_tool(&mut self, log: &ToolCallLog, duration_ms: u64) {
        if let Some(item) = self.timeline.iter_mut().rev().find(|item| {
            item.kind == ProgressTimelineKind::Tool
                && !item.completed
                && timeline_tool_matches(item, log)
        }) {
            item.title = log.tool_name.clone();
            item.summary = format_tool_timeline_summary(log, duration_ms);
            item.detail = format_tool_timeline_detail(&log.arguments, &log.result, duration_ms);
            item.completed = true;
            item.success = Some(log.success);
            return;
        }
        self.push_timeline(ProgressTimelineItem::tool_finished(log, duration_ms));
    }

    fn upsert_running_timeline_tool(&mut self, title: String, arguments: String, summary: String) {
        if let Some(item) = self.timeline.iter_mut().rev().find(|item| {
            item.kind == ProgressTimelineKind::Tool
                && !item.completed
                && item.title == title
                && item.detail == format_tool_input_preview(&arguments)
        }) {
            item.title = title;
            item.summary = summary;
            if item.detail.trim().is_empty() {
                item.detail = format_tool_input_preview(&arguments);
            }
            return;
        }
        let mut item = ProgressTimelineItem::tool_started(title, arguments);
        item.summary = summary;
        self.push_timeline(item);
    }

    fn apply_context_snapshot(&mut self, context: AgentContextSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.estimated_context_tokens = context.estimated_context_tokens;
            status.context_window_tokens = context.context_window_tokens;
            status.context_usage_percent = context.context_usage_percent;
            status.compaction_count = context.compaction_count;
            status.history_version = context.history_version;
            status.message_count = context.message_count;
            status.user_turn_count = context.user_turn_count;
            status.last_response_tokens = context.last_response_tokens;
            status.total_tokens_used = context.total_tokens_used;
        }
        self.context = Some(context);
    }

    pub fn update_queue_state(
        &mut self,
        queue_items: Vec<QueueItem>,
        guide_pending: bool,
        notice: Option<String>,
    ) {
        self.queue_items = queue_items;
        self.guide_pending = guide_pending;
        if let Some(notice) = notice.filter(|value| !value.trim().is_empty()) {
            self.activity_notice = Some(truncate_one_line(&notice, 80));
        }
    }
}

fn context_snapshot_from_status(status: &ActiveTurnStatus) -> AgentContextSnapshot {
    AgentContextSnapshot {
        estimated_context_tokens: status.estimated_context_tokens,
        context_window_tokens: status.context_window_tokens,
        context_usage_percent: status.context_usage_percent,
        compaction_count: status.compaction_count,
        history_version: status.history_version,
        message_count: status.message_count,
        user_turn_count: status.user_turn_count,
        last_response_tokens: status.last_response_tokens,
        total_tokens_used: status.total_tokens_used,
    }
}

fn is_human_readable_progress_status(status: &str) -> bool {
    let status = status.trim();
    if status.is_empty() || status.eq_ignore_ascii_case("running") {
        return false;
    }
    let lower = status.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "turn started" | "turn completed" | "run started" | "run completed"
    ) || lower.starts_with("model rerouted:")
    {
        return false;
    }
    if looks_like_uuid(status) {
        return false;
    }
    true
}

fn looks_like_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressRunnerSummary {
    pub runner_id: String,
    pub adapter: String,
    pub model: Option<String>,
    pub model_source: Option<String>,
    pub token_usage: Option<ProgressRunnerTokenUsage>,
    pub work_dir: Option<String>,
    pub external_thread_id: Option<String>,
    pub external_conversation_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressRunnerTokenUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressToolSummary {
    pub tool_name: String,
    pub arguments: Option<String>,
    pub success: Option<bool>,
    pub result_preview: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressTimelineKind {
    Thinking,
    Tool,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressTimelineItem {
    pub kind: ProgressTimelineKind,
    pub title: String,
    pub summary: String,
    pub detail: String,
    pub completed: bool,
    pub success: Option<bool>,
}

impl ProgressTimelineItem {
    fn thinking(content: String) -> Self {
        Self {
            kind: ProgressTimelineKind::Thinking,
            title: "思考".to_string(),
            summary: truncate_one_line(&content, 120),
            detail: content,
            completed: true,
            success: None,
        }
    }

    fn status(content: String) -> Self {
        Self {
            kind: ProgressTimelineKind::Status,
            title: "状态".to_string(),
            summary: truncate_one_line(&content, 120),
            detail: content,
            completed: true,
            success: None,
        }
    }

    fn tool_started(tool_name: String, arguments: String) -> Self {
        let summary = format!("正在运行 `{}`", truncate_one_line(&tool_name, 32));
        Self {
            kind: ProgressTimelineKind::Tool,
            title: tool_name,
            summary,
            detail: format_tool_input_preview(&arguments),
            completed: false,
            success: None,
        }
    }

    fn tool_finished(log: &ToolCallLog, duration_ms: u64) -> Self {
        Self {
            kind: ProgressTimelineKind::Tool,
            title: log.tool_name.clone(),
            summary: format_tool_timeline_summary(log, duration_ms),
            detail: format_tool_timeline_detail(&log.arguments, &log.result, duration_ms),
            completed: true,
            success: Some(log.success),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeishuProgressCardHandle {
    pub card_id: String,
    pub message_id: Option<String>,
    pub sequence: u64,
    pub generation: u64,
    pub rendered_title: String,
    pub rendered_has_plan: bool,
    pub rendered_has_tool: bool,
    pub rendered_has_thinking: bool,
    pub rendered_has_process: bool,
    pub rendered_phase: ImProgressPhase,
    rendered_output_hash: u64,
    rendered_plan_hash: Option<u64>,
    rendered_tool_hash: Option<u64>,
    rendered_status_hash: u64,
    rendered_thinking_hash: Option<u64>,
    rendered_process_hash: Option<u64>,
}

impl FeishuProgressCardHandle {
    fn next_sequence(&mut self) -> (u64, String) {
        self.sequence = self.sequence.saturating_add(1);
        (
            self.sequence,
            format!(
                "progress_{}_{}",
                self.generation,
                uuid::Uuid::new_v4().simple()
            ),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ProgressCardMessageInfo {
    pub card_id: String,
    pub message_id: Option<String>,
}

pub struct FeishuProgressCardSession {
    feishu: Arc<FeishuProvider>,
    provider: ImProviderConfig,
    target: ImTarget,
    snapshot: ImAgentProgressSnapshot,
    handle: Option<FeishuProgressCardHandle>,
    generation: u64,
}

impl FeishuProgressCardSession {
    pub fn new(
        feishu: Arc<FeishuProvider>,
        provider: ImProviderConfig,
        target: ImTarget,
        snapshot: ImAgentProgressSnapshot,
    ) -> Self {
        Self {
            feishu,
            provider,
            target,
            snapshot,
            handle: None,
            generation: 0,
        }
    }

    pub fn snapshot(&self) -> &ImAgentProgressSnapshot {
        &self.snapshot
    }

    pub async fn start(&mut self) -> Result<SendResult> {
        self.send_initial_card().await
    }

    pub async fn apply_event(&mut self, event: AgentTurnProgressEvent) -> Result<()> {
        self.snapshot.apply_event(event);
        self.flush_snapshot().await
    }

    pub async fn update_queue_state_and_flush(
        &mut self,
        queue_items: Vec<QueueItem>,
        guide_pending: bool,
        notice: Option<String>,
    ) -> Result<()> {
        self.snapshot
            .update_queue_state(queue_items, guide_pending, notice);
        self.flush_snapshot().await
    }

    pub async fn update_runner_summary_and_flush(
        &mut self,
        runner: ProgressRunnerSummary,
    ) -> Result<()> {
        self.snapshot.runner = Some(runner);
        self.flush_snapshot().await
    }

    pub async fn update_queue_state_and_rollover(
        &mut self,
        queue_items: Vec<QueueItem>,
        guide_pending: bool,
        notice: Option<String>,
    ) -> Result<bool> {
        if !self.can_rollover_current_card() {
            return Ok(false);
        }
        self.snapshot
            .update_queue_state(queue_items, guide_pending, notice);
        self.rollover_snapshot("已冻结旧进度快照，后续进展见下方新卡片")
            .await
            .map(|_| true)
    }

    pub async fn restart_turn(&mut self, initial_message: &str) -> Result<()> {
        let session_key = self.snapshot.session_key.clone();
        self.snapshot = ImAgentProgressSnapshot::new(session_key, initial_message);
        self.flush_snapshot().await
    }

    pub async fn rollover_turn(&mut self, initial_message: &str) -> Result<bool> {
        if !self.can_rollover_current_card() {
            return Ok(false);
        }
        let previous_handle = self.handle.clone();
        let previous_snapshot = self.snapshot.clone();
        let previous_generation = self.generation;
        let session_key = self.snapshot.session_key.clone();
        self.snapshot = ImAgentProgressSnapshot::new(session_key, initial_message);
        if let Err(error) = self.send_initial_card().await {
            self.snapshot = previous_snapshot;
            self.handle = previous_handle;
            self.generation = previous_generation;
            return Err(error);
        }
        self.freeze_previous_card_after_rollover(
            previous_handle,
            &previous_snapshot,
            "已冻结上一轮进度快照，后续进展见下方新卡片",
        )
        .await;
        Ok(true)
    }

    fn can_rollover_current_card(&self) -> bool {
        matches!(self.snapshot.phase, ImProgressPhase::Running)
    }

    async fn rollover_snapshot(&mut self, freeze_notice: &str) -> Result<()> {
        let previous_handle = self.handle.clone();
        let previous_snapshot = self.snapshot.clone();
        let previous_generation = self.generation;
        if let Err(error) = self.send_initial_card().await {
            self.handle = previous_handle;
            self.generation = previous_generation;
            return Err(error);
        }
        self.freeze_previous_card_after_rollover(
            previous_handle,
            &previous_snapshot,
            freeze_notice,
        )
        .await;
        Ok(())
    }

    async fn freeze_previous_card_after_rollover(
        &self,
        previous_handle: Option<FeishuProgressCardHandle>,
        previous_snapshot: &ImAgentProgressSnapshot,
        freeze_notice: &str,
    ) {
        let Some(mut handle) = previous_handle else {
            return;
        };
        let mut frozen_snapshot = previous_snapshot.clone();
        frozen_snapshot.phase = ImProgressPhase::Finished;
        frozen_snapshot.activity_notice = Some(truncate_one_line(freeze_notice, 80));
        let card = build_feishu_progress_card(&frozen_snapshot, false);
        let (sequence, uuid) = handle.next_sequence();
        if let Err(error) = self
            .feishu
            .update_card_entity(&self.provider, &handle.card_id, card, sequence, &uuid)
            .await
        {
            warn!(
                card_id = %handle.card_id,
                error = %error,
                "failed to freeze previous Feishu progress card entity after rollover"
            );
        }
        let summary = rollover_frozen_summary(&frozen_snapshot);
        let settings = serde_json::json!({
            "config": {
                "streaming_mode": false,
                "summary": {
                    "content": summary
                }
            }
        });
        let (sequence, uuid) = handle.next_sequence();
        if let Err(error) = self
            .feishu
            .update_card_settings(&self.provider, &handle.card_id, settings, sequence, &uuid)
            .await
        {
            warn!(
                card_id = %handle.card_id,
                error = %error,
                "failed to close previous Feishu progress card streaming after rollover"
            );
        } else {
            info!(
                card_id = %handle.card_id,
                message_id = handle.message_id.as_deref().unwrap_or(""),
                "froze previous Feishu progress card after rollover"
            );
        }
    }

    pub async fn finish(&mut self, output: Option<String>, failed: bool) -> Result<()> {
        if let Some(output) = output.filter(|value| !value.trim().is_empty()) {
            self.snapshot.output = output;
        }
        self.snapshot.phase = if failed {
            ImProgressPhase::Failed
        } else {
            ImProgressPhase::Finished
        };
        let flush_result = self.flush_snapshot().await;
        let close_result = self.close_streaming().await;
        match (flush_result, close_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(flush_error), Ok(())) => Err(flush_error),
            (Ok(()), Err(close_error)) => Err(close_error),
            (Err(flush_error), Err(close_error)) => Err(BifrostError::Network(format!(
                "final progress card flush failed: {flush_error}; close streaming failed: {close_error}"
            ))),
        }
    }

    pub fn message_info(&self) -> Option<ProgressCardMessageInfo> {
        self.handle.as_ref().map(|handle| ProgressCardMessageInfo {
            card_id: handle.card_id.clone(),
            message_id: handle.message_id.clone(),
        })
    }

    async fn send_initial_card(&mut self) -> Result<SendResult> {
        self.generation = self.generation.saturating_add(1);
        let card = build_feishu_progress_card(&self.snapshot, true);
        let card_id = self.feishu.create_card_entity(&self.provider, card).await?;
        let send_uuid = format!("progress_send_{}", uuid::Uuid::new_v4().simple());
        let send_result = self
            .feishu
            .send_card_entity(&self.provider, &self.target, &card_id, Some(&send_uuid))
            .await?;
        self.handle = Some(FeishuProgressCardHandle {
            card_id,
            message_id: send_result.message_id.clone(),
            sequence: 1,
            generation: self.generation,
            rendered_title: header_title(&self.snapshot).to_string(),
            rendered_has_plan: has_plan_state(&self.snapshot),
            rendered_has_tool: has_tool_state(&self.snapshot),
            rendered_has_thinking: has_thinking_state(&self.snapshot),
            rendered_has_process: has_process_state(&self.snapshot),
            rendered_phase: self.snapshot.phase,
            rendered_output_hash: output_hash(&self.snapshot),
            rendered_plan_hash: current_has_plan_hash(&self.snapshot),
            rendered_tool_hash: current_has_tool_hash(&self.snapshot),
            rendered_status_hash: status_hash(&self.snapshot),
            rendered_thinking_hash: current_has_thinking_hash(&self.snapshot),
            rendered_process_hash: current_has_process_hash(&self.snapshot),
        });
        if let Some(handle) = self.handle.as_ref() {
            info!(
                card_id = %handle.card_id,
                message_id = handle.message_id.as_deref().unwrap_or(""),
                generation = handle.generation,
                "sent Feishu progress card"
            );
        }
        Ok(send_result)
    }

    async fn flush_snapshot(&mut self) -> Result<()> {
        let Some(handle) = self.handle.as_mut() else {
            return Ok(());
        };
        let current_title = header_title(&self.snapshot).to_string();
        let current_has_plan = has_plan_state(&self.snapshot);
        let current_has_process = has_process_state(&self.snapshot);
        let current_has_tool = has_tool_state(&self.snapshot) && !current_has_process;
        let current_has_thinking = has_thinking_state(&self.snapshot) && !current_has_process;
        let current_process_hash = current_has_process_hash(&self.snapshot);
        if handle.rendered_title != current_title
            || handle.rendered_has_plan != current_has_plan
            || handle.rendered_has_tool != current_has_tool
            || handle.rendered_has_thinking != current_has_thinking
            || handle.rendered_has_process != current_has_process
            || handle.rendered_process_hash != current_process_hash
            || handle.rendered_phase != self.snapshot.phase
        {
            let card = build_feishu_progress_card(&self.snapshot, true);
            let (sequence, uuid) = handle.next_sequence();
            self.feishu
                .update_card_entity(&self.provider, &handle.card_id, card, sequence, &uuid)
                .await?;
            handle.rendered_title = current_title;
            handle.rendered_has_plan = current_has_plan;
            handle.rendered_has_tool = current_has_tool;
            handle.rendered_has_thinking = current_has_thinking;
            handle.rendered_has_process = current_has_process;
            handle.rendered_phase = self.snapshot.phase;
            handle.rendered_output_hash = output_hash(&self.snapshot);
            handle.rendered_plan_hash = current_has_plan_hash(&self.snapshot);
            handle.rendered_tool_hash = current_has_tool_hash(&self.snapshot);
            handle.rendered_status_hash = status_hash(&self.snapshot);
            handle.rendered_thinking_hash = current_has_thinking_hash(&self.snapshot);
            handle.rendered_process_hash = current_process_hash;
            return Ok(());
        }

        let output_content = format_output_markdown(&self.snapshot);
        let output_hash = stable_hash(&output_content);
        if handle.rendered_output_hash != output_hash {
            let (sequence, uuid) = handle.next_sequence();
            self.feishu
                .update_card_element_content(
                    &self.provider,
                    &handle.card_id,
                    OUTPUT_ELEMENT_ID,
                    &output_content,
                    sequence,
                    &uuid,
                )
                .await?;
            handle.rendered_output_hash = output_hash;
        }

        if current_has_plan {
            let element = build_plan_panel_element(&self.snapshot);
            let hash = element_hash(&element);
            if handle.rendered_plan_hash != Some(hash) {
                let (sequence, uuid) = handle.next_sequence();
                self.feishu
                    .update_card_element(
                        &self.provider,
                        &handle.card_id,
                        PLAN_PANEL_ELEMENT_ID,
                        element,
                        sequence,
                        &uuid,
                    )
                    .await?;
                handle.rendered_plan_hash = Some(hash);
            }
        }
        if current_has_tool {
            let element = build_tool_panel_element(&self.snapshot);
            let hash = element_hash(&element);
            if handle.rendered_tool_hash != Some(hash) {
                let (sequence, uuid) = handle.next_sequence();
                self.feishu
                    .update_card_element(
                        &self.provider,
                        &handle.card_id,
                        TOOL_PANEL_ELEMENT_ID,
                        element,
                        sequence,
                        &uuid,
                    )
                    .await?;
                handle.rendered_tool_hash = Some(hash);
            }
        }
        let status_element = build_status_panel_element(&self.snapshot);
        let status_hash = element_hash(&status_element);
        if handle.rendered_status_hash != status_hash {
            let (sequence, uuid) = handle.next_sequence();
            self.feishu
                .update_card_element(
                    &self.provider,
                    &handle.card_id,
                    STATUS_PANEL_ELEMENT_ID,
                    status_element,
                    sequence,
                    &uuid,
                )
                .await?;
            handle.rendered_status_hash = status_hash;
        }
        if current_has_thinking {
            let element = build_thinking_panel_element(&self.snapshot);
            let hash = element_hash(&element);
            if handle.rendered_thinking_hash != Some(hash) {
                let (sequence, uuid) = handle.next_sequence();
                self.feishu
                    .update_card_element(
                        &self.provider,
                        &handle.card_id,
                        THINKING_PANEL_ELEMENT_ID,
                        element,
                        sequence,
                        &uuid,
                    )
                    .await?;
                handle.rendered_thinking_hash = Some(hash);
            }
        }
        Ok(())
    }

    async fn close_streaming(&mut self) -> Result<()> {
        let Some(handle) = self.handle.as_mut() else {
            return Ok(());
        };
        let summary = truncate_str(&self.snapshot.output, 80);
        let settings = serde_json::json!({
            "config": {
                "streaming_mode": false,
                "summary": {
                    "content": summary
                }
            }
        });
        let (sequence, uuid) = handle.next_sequence();
        self.feishu
            .update_card_settings(&self.provider, &handle.card_id, settings, sequence, &uuid)
            .await
    }
}

#[derive(Clone, Default)]
pub struct ImAgentProgressRegistry {
    sessions: Arc<DashMap<String, Arc<Mutex<FeishuProgressCardSession>>>>,
}

impl ImAgentProgressRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_feishu(
        &self,
        session_key: &str,
        feishu: Arc<FeishuProvider>,
        provider: ImProviderConfig,
        target: ImTarget,
        initial_message: &str,
    ) -> Result<Arc<Mutex<FeishuProgressCardSession>>> {
        let snapshot = ImAgentProgressSnapshot::new(session_key, initial_message);
        let mut session = FeishuProgressCardSession::new(feishu, provider, target, snapshot);
        session.start().await?;
        let session = Arc::new(Mutex::new(session));
        self.sessions
            .insert(session_key.to_string(), Arc::clone(&session));
        Ok(session)
    }

    pub async fn apply_event(&self, session_key: &str, event: AgentTurnProgressEvent) {
        self.apply_events(session_key, vec![event]).await;
    }

    pub async fn apply_events(&self, session_key: &str, events: Vec<AgentTurnProgressEvent>) {
        if let Some(session) = self.sessions.get(session_key) {
            let mut session = session.value().lock().await;
            for event in events {
                session.snapshot.apply_event(event);
            }
            if let Err(error) = session.flush_snapshot().await {
                warn!(
                    session_key = session_key,
                    error = %error,
                    "failed to apply IM progress card event"
                );
            }
        }
    }

    pub async fn update_queue_state(
        &self,
        session_key: &str,
        queue_items: Vec<QueueItem>,
        guide_pending: bool,
        notice: Option<String>,
    ) -> bool {
        let Some(session) = self.sessions.get(session_key) else {
            return false;
        };
        let session = Arc::clone(session.value());
        let result = session
            .lock()
            .await
            .update_queue_state_and_rollover(queue_items, guide_pending, notice)
            .await;
        match result {
            Ok(updated) => updated,
            Err(error) => {
                warn!(
                    session_key = session_key,
                    error = %error,
                    "failed to update IM progress card queue state"
                );
                false
            }
        }
    }

    pub async fn update_runner_summary(
        &self,
        session_key: &str,
        runner: ProgressRunnerSummary,
    ) -> bool {
        let Some(session) = self.sessions.get(session_key) else {
            return false;
        };
        let session = Arc::clone(session.value());
        let result = session
            .lock()
            .await
            .update_runner_summary_and_flush(runner)
            .await;
        match result {
            Ok(()) => true,
            Err(error) => {
                warn!(
                    session_key = session_key,
                    error = %error,
                    "failed to update IM progress card runner summary"
                );
                false
            }
        }
    }

    pub async fn finish(
        &self,
        session_key: &str,
        output: Option<String>,
        failed: bool,
    ) -> Option<ProgressCardMessageInfo> {
        let session = self.sessions.get(session_key)?;
        let session = Arc::clone(session.value());
        let mut session = session.lock().await;
        let message_info = session.message_info();
        let result = session.finish(output, failed).await;
        if let Err(error) = result {
            warn!(
                session_key = session_key,
                error = %error,
                "failed to finish IM progress card"
            );
        }
        message_info
    }

    pub async fn restart_existing(&self, session_key: &str, initial_message: &str) -> bool {
        let Some(session) = self.sessions.get(session_key) else {
            return false;
        };
        let session = Arc::clone(session.value());
        let result = session.lock().await.restart_turn(initial_message).await;
        match result {
            Ok(()) => true,
            Err(error) => {
                warn!(
                    session_key = session_key,
                    error = %error,
                    "failed to restart existing IM progress card"
                );
                false
            }
        }
    }

    pub async fn rollover_existing(&self, session_key: &str, initial_message: &str) -> bool {
        let Some(session) = self.sessions.get(session_key) else {
            return false;
        };
        let session = Arc::clone(session.value());
        let result = session.lock().await.rollover_turn(initial_message).await;
        match result {
            Ok(rolled_over) => rolled_over,
            Err(error) => {
                warn!(
                    session_key = session_key,
                    error = %error,
                    "failed to roll over existing IM progress card"
                );
                false
            }
        }
    }
}

pub fn build_feishu_progress_card(
    snapshot: &ImAgentProgressSnapshot,
    streaming_mode: bool,
) -> serde_json::Value {
    let mut elements = Vec::new();
    let output_element = serde_json::json!({
        "tag": "markdown",
        "content": format_output_markdown(snapshot),
        "element_id": OUTPUT_ELEMENT_ID
    });

    elements.push(build_status_panel_element(snapshot));
    if !snapshot.plan_steps.is_empty() || snapshot.proposed_plan.is_some() {
        elements.push(build_plan_panel_element(snapshot));
    }
    if has_process_state(snapshot) {
        append_process_elements(snapshot, &mut elements);
    } else {
        if has_tool_state(snapshot) {
            elements.push(build_tool_panel_element(snapshot));
        }
        if has_thinking_state(snapshot) {
            elements.push(build_thinking_panel_element(snapshot));
        }
    }
    elements.push(output_element);

    serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true,
            "streaming_mode": streaming_mode,
            "summary": {
                "content": if streaming_mode { "[生成中...]".to_string() } else { truncate_str(&snapshot.output, 80) }
            },
            "streaming_config": {
                "print_frequency_ms": { "default": 70 },
                "print_step": { "default": 1 },
                "print_strategy": "fast"
            }
        },
        "header": {
            "template": match snapshot.phase {
                ImProgressPhase::Running => "blue",
                ImProgressPhase::Finished => "green",
                ImProgressPhase::Failed => "red",
            },
            "title": {
                "tag": "plain_text",
                "content": header_title(snapshot)
            }
        },
        "body": {
            "elements": elements
        }
    })
}

fn header_title(snapshot: &ImAgentProgressSnapshot) -> &str {
    snapshot.title.as_deref().unwrap_or("Bifrost AI")
}

fn default_card_title(initial_message: &str) -> String {
    let title = initial_message.trim();
    if title.is_empty() {
        "Bifrost AI".to_string()
    } else {
        truncate_str(title, 80)
    }
}

fn has_tool_state(snapshot: &ImAgentProgressSnapshot) -> bool {
    snapshot.latest_tool.is_some() || !snapshot.tool_calls.is_empty()
}

fn has_process_state(snapshot: &ImAgentProgressSnapshot) -> bool {
    !snapshot.timeline.is_empty()
}

fn has_thinking_state(snapshot: &ImAgentProgressSnapshot) -> bool {
    snapshot
        .last_thought
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn current_has_plan_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if !has_plan_state(snapshot) {
        return None;
    }
    Some(element_hash(&build_plan_panel_element(snapshot)))
}

fn has_plan_state(snapshot: &ImAgentProgressSnapshot) -> bool {
    !snapshot.plan_steps.is_empty() || snapshot.proposed_plan.is_some()
}

fn current_has_tool_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if !has_tool_state(snapshot) || has_process_state(snapshot) {
        return None;
    }
    Some(element_hash(&build_tool_panel_element(snapshot)))
}

fn current_has_thinking_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if !has_thinking_state(snapshot) || has_process_state(snapshot) {
        return None;
    }
    Some(element_hash(&build_thinking_panel_element(snapshot)))
}

fn current_has_process_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if !has_process_state(snapshot) {
        return None;
    }
    Some(process_elements_hash(snapshot))
}

fn output_hash(snapshot: &ImAgentProgressSnapshot) -> u64 {
    stable_hash(&format_output_markdown(snapshot))
}

fn status_hash(snapshot: &ImAgentProgressSnapshot) -> u64 {
    element_hash(&build_status_panel_element(snapshot))
}

fn process_elements_hash(snapshot: &ImAgentProgressSnapshot) -> u64 {
    let mut elements = Vec::new();
    append_process_elements(snapshot, &mut elements);
    stable_hash(&serde_json::to_string(&elements).unwrap_or_default())
}

fn element_hash(element: &serde_json::Value) -> u64 {
    stable_hash(&serde_json::to_string(element).unwrap_or_default())
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn format_output_markdown(snapshot: &ImAgentProgressSnapshot) -> String {
    if snapshot.output.trim().is_empty() {
        return "处理中...".to_string();
    }
    crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&snapshot.output)
}

fn rollover_frozen_summary(snapshot: &ImAgentProgressSnapshot) -> String {
    snapshot
        .activity_notice
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| truncate_str(value, 80))
        .unwrap_or_else(|| "已冻结进度快照".to_string())
}

fn format_plan_markdown(steps: &[PlanStep]) -> String {
    if steps.is_empty() {
        return "暂无任务计划".to_string();
    }
    steps
        .iter()
        .map(|step| format!("{} {}", step.status.emoji(), step.step))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_plan_panel_element(snapshot: &ImAgentProgressSnapshot) -> serde_json::Value {
    serde_json::json!({
        "tag": "collapsible_panel",
        "element_id": PLAN_PANEL_ELEMENT_ID,
        "expanded": true,
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_plan_panel_title(snapshot)
            }
        },
        "elements": [{
            "tag": "markdown",
            "content": format_plan_panel_markdown(snapshot),
            "element_id": PLAN_ELEMENT_ID
        }]
    })
}

fn format_plan_panel_markdown(snapshot: &ImAgentProgressSnapshot) -> String {
    if let Some(plan) = snapshot
        .proposed_plan
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return crate::im_gateway::markdown_converter::convert_to_feishu_markdown(plan);
    }
    format_plan_markdown(&snapshot.plan_steps)
}

fn format_plan_panel_title(snapshot: &ImAgentProgressSnapshot) -> String {
    if snapshot.proposed_plan.is_some() {
        return "实施方案".to_string();
    }
    let steps = &snapshot.plan_steps;
    if steps.is_empty() {
        return "任务计划".to_string();
    }
    if let Some(step) = steps
        .iter()
        .find(|step| step.status == PlanStepStatus::InProgress)
    {
        return format!("任务计划：{}", truncate_one_line(&step.step, 64));
    }
    if let Some(step) = steps
        .iter()
        .find(|step| step.status == PlanStepStatus::Pending)
    {
        return format!("任务计划：待处理 {}", truncate_one_line(&step.step, 56));
    }
    format!("任务计划：已完成 {}/{}", steps.len(), steps.len())
}

fn format_tool_summary_markdown(tool: Option<&ProgressToolSummary>) -> String {
    let Some(tool) = tool else {
        return "**工具执行状态**\n\n暂无工具调用".to_string();
    };
    let status = match tool.success {
        Some(true) => "完成",
        Some(false) => "失败",
        None => "执行中",
    };
    let mut text = format!(
        "**工具执行状态**\n\n最新工具：`{}` · {}",
        tool.tool_name, status
    );
    if let Some(arguments) = tool.arguments.as_deref().filter(|value| !value.is_empty()) {
        text.push_str(&format!("\n输入：`{}`", truncate_str(arguments, 120)));
    }
    if let Some(result) = tool
        .result_preview
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        text.push_str(&format!("\n结果：{}", result));
    }
    if let Some(duration_ms) = tool.duration_ms {
        text.push_str(&format!("\n耗时：{}ms", duration_ms));
    }
    text
}

fn append_process_elements(
    snapshot: &ImAgentProgressSnapshot,
    elements: &mut Vec<serde_json::Value>,
) {
    if !has_process_state(snapshot) {
        return;
    }
    elements.push(build_process_panel_element(snapshot));
}

fn build_process_panel_element(snapshot: &ImAgentProgressSnapshot) -> serde_json::Value {
    serde_json::json!({
        "tag": "collapsible_panel",
        "element_id": PROCESS_PANEL_ELEMENT_ID,
        "expanded": matches!(snapshot.phase, ImProgressPhase::Running),
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_process_panel_title(snapshot)
            }
        },
        "elements": build_process_loop_elements(snapshot)
    })
}

fn build_process_loop_elements(snapshot: &ImAgentProgressSnapshot) -> Vec<serde_json::Value> {
    if snapshot.timeline.is_empty() {
        return vec![build_process_markdown_element(
            PROCESS_LOG_ELEMENT_ID.to_string(),
            "暂无过程信息".to_string(),
        )];
    }

    let mut elements = Vec::new();
    let mut markdown_lines = Vec::<String>::new();
    let mut item_index = 0;
    let mut markdown_element_count = 0;

    let mut timeline_index = 0;
    while timeline_index < snapshot.timeline.len() {
        let item = &snapshot.timeline[timeline_index];
        match item.kind {
            ProgressTimelineKind::Thinking | ProgressTimelineKind::Status => {
                item_index += 1;
                markdown_lines.push(format_process_timeline_line(item_index, item));
                timeline_index += 1;
            }
            ProgressTimelineKind::Tool => {
                flush_process_markdown(
                    &mut elements,
                    &mut markdown_lines,
                    &mut markdown_element_count,
                );
                let batch_start = timeline_index;
                let mut batch = Vec::new();
                while timeline_index < snapshot.timeline.len()
                    && snapshot.timeline[timeline_index].kind == ProgressTimelineKind::Tool
                {
                    batch.push((timeline_index, &snapshot.timeline[timeline_index]));
                    timeline_index += 1;
                }
                if batch.len() == 1 {
                    elements.push(build_process_tool_detail_element(batch_start, item));
                } else {
                    elements.push(build_process_tool_group_element(batch_start, &batch));
                }
            }
        }
    }

    flush_process_markdown(
        &mut elements,
        &mut markdown_lines,
        &mut markdown_element_count,
    );
    elements
}

fn flush_process_markdown(
    elements: &mut Vec<serde_json::Value>,
    markdown_lines: &mut Vec<String>,
    markdown_element_count: &mut usize,
) {
    if markdown_lines.is_empty() {
        return;
    }
    let element_id = if *markdown_element_count == 0 {
        PROCESS_LOG_ELEMENT_ID.to_string()
    } else {
        process_dynamic_element_id(
            PROCESS_DYNAMIC_LOG_ELEMENT_PREFIX,
            *markdown_element_count + 1,
        )
    };
    elements.push(build_process_markdown_element(
        element_id,
        markdown_lines.join("\n"),
    ));
    markdown_lines.clear();
    *markdown_element_count += 1;
}

fn build_process_markdown_element(element_id: String, content: String) -> serde_json::Value {
    serde_json::json!({
        "tag": "markdown",
        "content": content,
        "element_id": element_id
    })
}

fn build_process_tool_detail_element(
    index: usize,
    item: &ProgressTimelineItem,
) -> serde_json::Value {
    serde_json::json!({
        "tag": "collapsible_panel",
        "element_id": process_dynamic_element_id(PROCESS_TOOL_ELEMENT_PREFIX, index),
        "expanded": false,
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_process_tool_panel_title(item)
            }
        },
        "elements": [{
            "tag": "markdown",
            "content": crate::im_gateway::markdown_converter::convert_to_feishu_markdown(
                &item.detail
            ),
            "element_id": process_dynamic_element_id(PROCESS_TOOL_DETAIL_ELEMENT_PREFIX, index)
        }]
    })
}

fn build_process_tool_group_element(
    index: usize,
    items: &[(usize, &ProgressTimelineItem)],
) -> serde_json::Value {
    let children: Vec<serde_json::Value> = items
        .iter()
        .map(|(item_index, item)| build_process_tool_detail_element(*item_index, item))
        .collect();
    serde_json::json!({
        "tag": "collapsible_panel",
        "element_id": process_dynamic_element_id(PROCESS_TOOL_GROUP_ELEMENT_PREFIX, index),
        "expanded": false,
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_process_tool_group_title(items)
            }
        },
        "elements": children
    })
}

fn process_dynamic_element_id(prefix: &str, index: usize) -> String {
    format!("{prefix}_{index}")
}

fn format_process_panel_title(snapshot: &ImAgentProgressSnapshot) -> String {
    let tool_count = process_tool_count(snapshot);
    let base = match snapshot.phase {
        ImProgressPhase::Running => "执行过程",
        ImProgressPhase::Finished => "执行过程",
        ImProgressPhase::Failed => "失败前过程",
    };
    if tool_count == 0 {
        base.to_string()
    } else {
        format!("{base}：已运行 {tool_count} 条命令")
    }
}

fn format_process_tool_panel_title(item: &ProgressTimelineItem) -> String {
    let verb = match item.success {
        Some(true) => "已完成",
        Some(false) => "运行失败",
        None => "正在运行",
    };
    let duration = process_tool_duration_label(item);
    match duration {
        Some(duration) => format!(
            "{}：{} · {}",
            verb,
            truncate_one_line(&item.title, 28),
            duration
        ),
        None => format!("{}：{}", verb, truncate_one_line(&item.title, 28)),
    }
}

fn format_process_tool_group_title(items: &[(usize, &ProgressTimelineItem)]) -> String {
    let failed_count = items
        .iter()
        .filter(|(_, item)| item.success == Some(false))
        .count();
    let running_count = items
        .iter()
        .filter(|(_, item)| item.success.is_none())
        .count();
    if failed_count > 0 {
        format!("已运行 {} 条命令，{} 条失败", items.len(), failed_count)
    } else if running_count > 0 {
        format!("正在运行 {} 条命令", items.len())
    } else {
        format!("已运行 {} 条命令", items.len())
    }
}

fn process_tool_duration_label(item: &ProgressTimelineItem) -> Option<String> {
    item.summary
        .split(" · ")
        .find(|part| {
            part.strip_suffix("ms")
                .is_some_and(|prefix| prefix.chars().all(|ch| ch.is_ascii_digit()))
        })
        .map(ToString::to_string)
}

fn process_tool_count(snapshot: &ImAgentProgressSnapshot) -> usize {
    snapshot
        .timeline
        .iter()
        .filter(|item| item.kind == ProgressTimelineKind::Tool)
        .count()
}

fn format_process_timeline_line(index: usize, item: &ProgressTimelineItem) -> String {
    match item.kind {
        ProgressTimelineKind::Thinking => format!(
            "{}. {}",
            index,
            crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&truncate_str(
                &item.detail,
                600
            ))
        ),
        ProgressTimelineKind::Status => format!(
            "{}. 状态：{}",
            index,
            crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&item.summary)
        ),
        ProgressTimelineKind::Tool => {
            let status = match item.success {
                Some(true) => "完成",
                Some(false) => "失败",
                None => "执行中",
            };
            format!(
                "{}. `{}` · {} · {}",
                index,
                truncate_one_line(&item.title, 32),
                status,
                crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&item.summary)
            )
        }
    }
}

fn build_tool_panel_element(snapshot: &ImAgentProgressSnapshot) -> serde_json::Value {
    serde_json::json!({
        "tag": "collapsible_panel",
        "element_id": TOOL_PANEL_ELEMENT_ID,
        "expanded": false,
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_tool_panel_title(snapshot)
            }
        },
        "elements": [{
            "tag": "markdown",
            "content": format_tool_details_markdown(&snapshot.tool_calls, snapshot.latest_tool.as_ref()),
            "element_id": TOOL_LOG_ELEMENT_ID
        }]
    })
}

fn format_tool_timeline_summary(log: &ToolCallLog, duration_ms: u64) -> String {
    let status = if log.success { "已完成" } else { "失败" };
    let mut summary = format!("{} `{}`", status, truncate_one_line(&log.tool_name, 32));
    if duration_ms > 0 {
        summary.push_str(&format!(" · {duration_ms}ms"));
    }
    if !log.result.trim().is_empty() {
        summary.push_str(&format!(" · {}", truncate_one_line(&log.result, 80)));
    }
    summary
}

fn timeline_tool_matches(item: &ProgressTimelineItem, log: &ToolCallLog) -> bool {
    item.title == log.tool_name
        && (log.arguments.trim().is_empty()
            || item.detail == format_tool_input_preview(&log.arguments))
}

fn format_tool_input_preview(arguments: &str) -> String {
    format!(
        "输入：`{}`",
        truncate_str(arguments, PROCESS_TOOL_INPUT_PREVIEW_CHARS)
    )
}

fn format_tool_timeline_detail(arguments: &str, result: &str, duration_ms: u64) -> String {
    let mut detail = String::new();
    if !arguments.trim().is_empty() {
        detail.push_str(&format_tool_input_preview(arguments));
    }
    if duration_ms > 0 {
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&format!("耗时：{duration_ms}ms"));
    }
    if !result.trim().is_empty() {
        if !detail.is_empty() {
            detail.push_str("\n\n");
        }
        detail.push_str("输出：\n```text\n");
        detail.push_str(&truncate_str(result, PROCESS_TOOL_OUTPUT_PREVIEW_CHARS));
        detail.push_str("\n```");
    }
    if detail.is_empty() {
        "暂无工具详情".to_string()
    } else {
        detail
    }
}

fn build_status_panel_element(snapshot: &ImAgentProgressSnapshot) -> serde_json::Value {
    serde_json::json!({
        "tag": "collapsible_panel",
        "element_id": STATUS_PANEL_ELEMENT_ID,
        "expanded": false,
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_status_panel_title(snapshot)
            }
        },
        "elements": [{
            "tag": "markdown",
            "content": format_footer_markdown(snapshot),
            "element_id": FOOTER_ELEMENT_ID
        }]
    })
}

fn build_thinking_panel_element(snapshot: &ImAgentProgressSnapshot) -> serde_json::Value {
    serde_json::json!({
        "tag": "markdown",
        "element_id": THINKING_PANEL_ELEMENT_ID,
        "content": format!("**思考过程**\n\n{}", format_thinking_markdown(snapshot))
    })
}

fn format_thinking_markdown(snapshot: &ImAgentProgressSnapshot) -> String {
    snapshot
        .last_thought
        .as_deref()
        .map(crate::im_gateway::markdown_converter::convert_to_feishu_markdown)
        .unwrap_or_else(|| "暂无思考过程".to_string())
}

fn format_tool_panel_title(snapshot: &ImAgentProgressSnapshot) -> String {
    let Some(tool) = snapshot.latest_tool.as_ref() else {
        return "工具执行状态：暂无工具调用".to_string();
    };
    let status = match tool.success {
        Some(true) => "完成",
        Some(false) => "失败",
        None => "执行中",
    };
    let duration = tool
        .duration_ms
        .map(|value| format!(" · {}ms", value))
        .unwrap_or_default();
    format!(
        "工具执行状态：{} · {}{} · {}次",
        truncate_str(&tool.tool_name, 24),
        status,
        duration,
        snapshot.tool_calls.len()
    )
}

fn format_status_panel_title(snapshot: &ImAgentProgressSnapshot) -> String {
    if snapshot_has_external_runner(snapshot) {
        let runner_id = external_runner_id(snapshot);
        let model = external_runner_model_label(snapshot);
        let title = format!("Runner：{} · 模型：{}", truncate_str(&runner_id, 24), model);
        return if let Some(notice) = snapshot.activity_notice.as_deref() {
            format!("{title} · {notice}")
        } else {
            title
        };
    }
    let token_title = match snapshot.status.as_ref() {
        Some(status) => match (status.total_tokens_used, status.last_response_tokens) {
            (Some(total), Some(last)) => format!(
                "Token：累计 {} · 最近 {}",
                bifrost_agent::format_status_metric_count(total),
                bifrost_agent::format_status_metric_count(last)
            ),
            (Some(total), None) => {
                format!(
                    "Token：累计 {}",
                    bifrost_agent::format_status_metric_count(total)
                )
            }
            (None, Some(last)) => {
                format!(
                    "Token：最近 {}",
                    bifrost_agent::format_status_metric_count(last)
                )
            }
            (None, None) => "Token：统计中".to_string(),
        },
        None => match snapshot.context.as_ref() {
            Some(context) => match (context.total_tokens_used, context.last_response_tokens) {
                (Some(total), Some(last)) => format!(
                    "Token：累计 {} · 最近 {}",
                    bifrost_agent::format_status_metric_count(total),
                    bifrost_agent::format_status_metric_count(last)
                ),
                (Some(total), None) => {
                    format!(
                        "Token：累计 {}",
                        bifrost_agent::format_status_metric_count(total)
                    )
                }
                (None, Some(last)) => {
                    format!(
                        "Token：最近 {}",
                        bifrost_agent::format_status_metric_count(last)
                    )
                }
                (None, None) => "Token：统计中".to_string(),
            },
            None => "Token：统计中".to_string(),
        },
    };
    let token_title = snapshot
        .status
        .as_ref()
        .and_then(internal_runner_model_label)
        .map(|model| format!("模型：{model} · {token_title}"))
        .unwrap_or(token_title);
    if let Some(notice) = snapshot.activity_notice.as_deref() {
        format!("{token_title} · {notice}")
    } else {
        token_title
    }
}

fn snapshot_has_external_runner(snapshot: &ImAgentProgressSnapshot) -> bool {
    snapshot.runner.is_some()
        || snapshot
            .status
            .as_ref()
            .is_some_and(is_external_runner_status)
}

fn is_external_runner_status(status: &bifrost_agent::ActiveTurnStatus) -> bool {
    status
        .runner_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || status
            .runner_type
            .as_deref()
            .is_some_and(|value| value.trim() != "bifrost_agent")
        || status.agent_type.as_deref() == Some("External Runner Agent")
}

fn external_runner_id(snapshot: &ImAgentProgressSnapshot) -> String {
    snapshot
        .runner
        .as_ref()
        .map(|runner| runner.runner_id.as_str())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            snapshot
                .status
                .as_ref()
                .and_then(|status| status.runner_id.as_deref())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("external")
        .to_string()
}

fn external_runner_adapter(snapshot: &ImAgentProgressSnapshot) -> String {
    snapshot
        .runner
        .as_ref()
        .map(|runner| runner.adapter.as_str())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            snapshot
                .status
                .as_ref()
                .and_then(|status| status.runner_type.as_deref())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("external")
        .to_string()
}

fn external_runner_model_label(snapshot: &ImAgentProgressSnapshot) -> String {
    if let Some(status_model) = snapshot
        .status
        .as_ref()
        .and_then(internal_runner_model_label)
    {
        return status_model;
    }
    let runner = snapshot.runner.as_ref();
    let runner_model = runner
        .and_then(|runner| runner.model.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let runner_source = runner
        .and_then(|runner| runner.model_source.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match runner_model {
        Some(model) => match runner_source {
            Some(source) => format!("{}（{}）", truncate_one_line(model, 48), source),
            None => truncate_one_line(model, 48),
        },
        None if runner.is_some_and(|runner| {
            runner.adapter.trim() == "codex" || runner.runner_id.trim() == "codex"
        }) =>
        {
            "Codex 默认模型（未显式配置）".to_string()
        }
        None if runner.is_some_and(|runner| {
            runner.adapter.trim() == "traex" || runner.runner_id.trim() == "traex"
        }) =>
        {
            "Trae 默认模型（未显式配置）".to_string()
        }
        None => "默认模型（未显式配置）".to_string(),
    }
}

fn external_runner_token_usage_line(snapshot: &ImAgentProgressSnapshot) -> Option<String> {
    let usage = snapshot.runner.as_ref()?.token_usage.as_ref()?;
    let total = usage
        .total_tokens
        .map(bifrost_agent::format_status_metric_count)
        .unwrap_or_else(|| "N/A".to_string());
    let input = usage
        .input_tokens
        .map(bifrost_agent::format_status_metric_count)
        .unwrap_or_else(|| "N/A".to_string());
    let output = usage
        .output_tokens
        .map(bifrost_agent::format_status_metric_count)
        .unwrap_or_else(|| "N/A".to_string());
    let mut parts = vec![format!("输入 {input}"), format!("输出 {output}")];
    if let Some(cached) = usage.cached_input_tokens {
        parts.push(format!(
            "缓存输入 {}",
            bifrost_agent::format_status_metric_count(cached)
        ));
    }
    if let Some(reasoning) = usage.reasoning_output_tokens {
        parts.push(format!(
            "推理输出 {}",
            bifrost_agent::format_status_metric_count(reasoning)
        ));
    }
    Some(format!("Token：总计 {total} · {}", parts.join(" · ")))
}

fn external_runner_context_usage_line(snapshot: &ImAgentProgressSnapshot) -> Option<String> {
    let usage = snapshot.runner.as_ref()?.token_usage.as_ref()?;
    let input = usage.input_tokens?;
    Some(format!(
        "Context：最近输入 {} / N/A",
        bifrost_agent::format_status_metric_count(input)
    ))
}

fn external_runner_state_line(snapshot: &ImAgentProgressSnapshot) -> Option<String> {
    let state = snapshot
        .status
        .as_ref()
        .map(|status| status.state.trim())
        .filter(|value| !value.is_empty() && *value != "running")?;
    Some(format!("当前状态：{}", truncate_one_line(state, 48)))
}

fn internal_runner_model_label(status: &bifrost_agent::ActiveTurnStatus) -> Option<String> {
    status
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|model| bifrost_agent::format_model_ref(Some(model), status.model_provider.as_deref()))
}

fn external_runner_work_dir(snapshot: &ImAgentProgressSnapshot) -> Option<&str> {
    snapshot
        .runner
        .as_ref()
        .and_then(|runner| runner.work_dir.as_deref())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            snapshot
                .status
                .as_ref()
                .and_then(|status| status.work_dir.as_deref())
        })
}

fn external_runner_conversation_ref(snapshot: &ImAgentProgressSnapshot) -> String {
    let thread_id = snapshot
        .runner
        .as_ref()
        .and_then(|runner| runner.external_thread_id.as_deref())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            snapshot
                .status
                .as_ref()
                .and_then(|status| status.external_thread_id.as_deref())
        });
    let conversation_id = snapshot
        .runner
        .as_ref()
        .and_then(|runner| runner.external_conversation_id.as_deref())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            snapshot
                .status
                .as_ref()
                .and_then(|status| status.external_conversation_id.as_deref())
        });
    bifrost_agent::format_conversation_ref(thread_id, conversation_id)
}

fn format_external_tool_line(snapshot: &ImAgentProgressSnapshot) -> Option<String> {
    let tool = snapshot.latest_tool.as_ref()?;
    let status = match tool.success {
        Some(true) => "完成",
        Some(false) => "失败",
        None => "执行中",
    };
    Some(format!(
        "最新工具：`{}` · {}",
        truncate_one_line(&tool.tool_name, 32),
        status
    ))
}

fn format_tool_details_markdown(
    logs: &[ToolCallLog],
    latest: Option<&ProgressToolSummary>,
) -> String {
    let mut sections = Vec::new();
    if let Some(tool) = latest {
        sections.push(format_tool_summary_markdown(Some(tool)));
    }
    if logs.is_empty() {
        if sections.is_empty() {
            return "暂无工具调用详情".to_string();
        }
        return sections.join("\n\n");
    }
    let details = logs
        .iter()
        .rev()
        .take(10)
        .rev()
        .map(|log| {
            let icon = if log.success { "OK" } else { "ERR" };
            format!(
                "- {} `{}`\n```\n{}\n```",
                icon,
                log.tool_name,
                truncate_str(&log.result, 500)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    sections.push(details);
    sections.join("\n\n")
}

fn format_footer_markdown(snapshot: &ImAgentProgressSnapshot) -> String {
    let phase = match snapshot.phase {
        ImProgressPhase::Running => "运行中",
        ImProgressPhase::Finished => "已完成",
        ImProgressPhase::Failed => "失败",
    };
    let queue_text = if snapshot.queue_items.is_empty() {
        "无排队消息".to_string()
    } else {
        format!("{} 条排队消息", snapshot.queue_items.len())
    };
    let guide_text = if snapshot.guide_pending {
        "有待处理引导消息"
    } else {
        "无待处理引导消息"
    };
    if snapshot_has_external_runner(snapshot) {
        let prefix = snapshot
            .activity_notice
            .as_deref()
            .map(|notice| format!("提示：{notice}\n"))
            .unwrap_or_default();
        let tool_line = format_external_tool_line(snapshot)
            .map(|line| format!("\n{line}"))
            .unwrap_or_default();
        let token_line = external_runner_token_usage_line(snapshot)
            .map(|line| format!("\n{line}"))
            .unwrap_or_default();
        let context_line = external_runner_context_usage_line(snapshot)
            .map(|line| format!("\n{line}"))
            .unwrap_or_default();
        let state_line = external_runner_state_line(snapshot)
            .map(|line| format!("\n{line}"))
            .unwrap_or_default();
        return format!(
            "{}状态：{}{}\nRunner：`{}` · Adapter：`{}`\n模型：{}{}{}\n外部会话：{}\n队列：{} · 引导：{}\n工作路径：`{}`{}",
            prefix,
            phase,
            state_line,
            external_runner_id(snapshot),
            external_runner_adapter(snapshot),
            external_runner_model_label(snapshot),
            token_line,
            context_line,
            external_runner_conversation_ref(snapshot),
            queue_text,
            guide_text,
            external_runner_work_dir(snapshot).unwrap_or("N/A"),
            tool_line
        );
    }
    match &snapshot.status {
        Some(status) => {
            let token_text = status
                .total_tokens_used
                .map(bifrost_agent::format_status_metric_count)
                .unwrap_or_else(|| "N/A".to_string());
            let last_token_text = status
                .last_response_tokens
                .map(bifrost_agent::format_status_metric_count)
                .unwrap_or_else(|| "N/A".to_string());
            let context_text = match (status.context_window_tokens, status.context_usage_percent) {
                (Some(window), Some(percent)) => format!(
                    "~{} / {} ({percent:.1}%)",
                    bifrost_agent::format_status_metric_count(
                        status.estimated_context_tokens.into()
                    ),
                    bifrost_agent::format_status_metric_count(window.into())
                ),
                _ => format!(
                    "~{} / N/A",
                    bifrost_agent::format_status_metric_count(
                        status.estimated_context_tokens.into()
                    )
                ),
            };
            let model_line = internal_runner_model_label(status)
                .map(|model| format!("\n模型：{model}"))
                .unwrap_or_default();
            format!(
                "{}状态：{} · Loop {}/{}（已完成 {}）{}\nContext：{}\nToken：累计 {}，最近 {}\n压缩：{} 次 · 队列：{} · 引导：{}\n工作路径：`{}`",
                snapshot
                    .activity_notice
                    .as_deref()
                    .map(|notice| format!("提示：{notice}\n"))
                    .unwrap_or_default(),
                phase,
                status.current_loop_iteration,
                status.max_loop_iterations,
                status.completed_loop_iterations,
                model_line,
                context_text,
                token_text,
                last_token_text,
                status.compaction_count,
                queue_text,
                guide_text,
                status.work_dir.as_deref().unwrap_or("N/A")
            )
        }
        None => {
            let prefix = snapshot
                .activity_notice
                .as_deref()
                .map(|notice| format!("提示：{notice}\n"))
                .unwrap_or_default();
            if let Some(context) = snapshot.context.as_ref() {
                let token_text = context
                    .total_tokens_used
                    .map(bifrost_agent::format_status_metric_count)
                    .unwrap_or_else(|| "N/A".to_string());
                let last_token_text = context
                    .last_response_tokens
                    .map(bifrost_agent::format_status_metric_count)
                    .unwrap_or_else(|| "N/A".to_string());
                let context_text =
                    match (context.context_window_tokens, context.context_usage_percent) {
                        (Some(window), Some(percent)) => format!(
                            "~{} / {} ({percent:.1}%)",
                            bifrost_agent::format_status_metric_count(
                                context.estimated_context_tokens.into()
                            ),
                            bifrost_agent::format_status_metric_count(window.into())
                        ),
                        _ => format!(
                            "~{} / N/A",
                            bifrost_agent::format_status_metric_count(
                                context.estimated_context_tokens.into()
                            )
                        ),
                    };
                format!(
                    "{}状态：{}\nContext：{}\nToken：累计 {}，最近 {}\n压缩：{} 次 · 队列：{} · 引导：{}",
                    prefix,
                    phase,
                    context_text,
                    token_text,
                    last_token_text,
                    context.compaction_count,
                    queue_text,
                    guide_text
                )
            } else {
                format!(
                    "{}状态：{} · 队列：{} · 引导：{}",
                    prefix, phase, queue_text, guide_text
                )
            }
        }
    }
}

fn truncate_str(input: &str, max_chars: usize) -> String {
    let mut iter = input.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn truncate_one_line(input: &str, max_chars: usize) -> String {
    let compact = input.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_str(&compact, max_chars)
}

#[cfg(test)]
mod tests;
