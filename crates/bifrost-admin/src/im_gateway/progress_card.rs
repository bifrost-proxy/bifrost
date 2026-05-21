use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::warn;

use bifrost_agent::{
    ActiveTurnStatus, AgentTurnProgressEvent, PlanStep, PlanStepStatus, ToolCallLog,
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
const THINKING_ELEMENT_ID: &str = "agent_thinking";

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
    pub plan_steps: Vec<PlanStep>,
    pub tool_calls: Vec<ToolCallLog>,
    pub latest_tool: Option<ProgressToolSummary>,
    pub status: Option<ActiveTurnStatus>,
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
            plan_steps: Vec::new(),
            tool_calls: Vec::new(),
            latest_tool: None,
            status: None,
            queue_items: Vec::new(),
            guide_pending: false,
            activity_notice: None,
            phase: ImProgressPhase::Running,
        }
    }

    pub fn apply_event(&mut self, event: AgentTurnProgressEvent) {
        match event {
            AgentTurnProgressEvent::Status(status) => {
                self.status = Some(*status);
            }
            AgentTurnProgressEvent::ToolStarted {
                tool_name,
                arguments,
            } => {
                self.latest_tool = Some(ProgressToolSummary {
                    tool_name,
                    arguments: Some(arguments),
                    success: None,
                    result_preview: None,
                    duration_ms: None,
                });
            }
            AgentTurnProgressEvent::ToolFinished { log, duration_ms } => {
                self.latest_tool = Some(ProgressToolSummary {
                    tool_name: log.tool_name.clone(),
                    arguments: Some(log.arguments.clone()),
                    success: Some(log.success),
                    result_preview: Some(truncate_str(&log.result, 160)),
                    duration_ms: Some(duration_ms),
                });
                self.tool_calls.push(log);
            }
            AgentTurnProgressEvent::PlanUpdated { steps, title } => {
                self.plan_steps = steps;
                if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
                    self.title = Some(title);
                }
            }
            AgentTurnProgressEvent::TitleUpdated { title } => {
                if !title.trim().is_empty() {
                    self.title = Some(title);
                }
            }
            AgentTurnProgressEvent::AssistantDelta { content } => {
                if !content.trim().is_empty() {
                    self.last_thought = Some(content);
                }
            }
            AgentTurnProgressEvent::AssistantFinal { content } => {
                if !content.trim().is_empty() {
                    self.output = content;
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
                self.output = format!("Agent 执行失败：{}", truncate_str(&error, 300));
            }
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressToolSummary {
    pub tool_name: String,
    pub arguments: Option<String>,
    pub success: Option<bool>,
    pub result_preview: Option<String>,
    pub duration_ms: Option<u64>,
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
    pub rendered_phase: ImProgressPhase,
    rendered_output_hash: u64,
    rendered_plan_hash: Option<u64>,
    rendered_tool_hash: Option<u64>,
    rendered_status_hash: u64,
    rendered_thinking_hash: Option<u64>,
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
            rendered_has_plan: !self.snapshot.plan_steps.is_empty(),
            rendered_has_tool: has_tool_state(&self.snapshot),
            rendered_has_thinking: has_thinking_state(&self.snapshot),
            rendered_phase: self.snapshot.phase,
            rendered_output_hash: output_hash(&self.snapshot),
            rendered_plan_hash: current_has_plan_hash(&self.snapshot),
            rendered_tool_hash: current_has_tool_hash(&self.snapshot),
            rendered_status_hash: status_hash(&self.snapshot),
            rendered_thinking_hash: current_has_thinking_hash(&self.snapshot),
        });
        Ok(send_result)
    }

    async fn flush_snapshot(&mut self) -> Result<()> {
        let Some(handle) = self.handle.as_mut() else {
            return Ok(());
        };
        let current_title = header_title(&self.snapshot).to_string();
        let current_has_plan = !self.snapshot.plan_steps.is_empty();
        let current_has_tool = has_tool_state(&self.snapshot);
        let current_has_thinking = has_thinking_state(&self.snapshot);
        if handle.rendered_title != current_title
            || handle.rendered_has_plan != current_has_plan
            || handle.rendered_has_tool != current_has_tool
            || handle.rendered_has_thinking != current_has_thinking
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
            handle.rendered_phase = self.snapshot.phase;
            handle.rendered_output_hash = output_hash(&self.snapshot);
            handle.rendered_plan_hash = current_has_plan_hash(&self.snapshot);
            handle.rendered_tool_hash = current_has_tool_hash(&self.snapshot);
            handle.rendered_status_hash = status_hash(&self.snapshot);
            handle.rendered_thinking_hash = current_has_thinking_hash(&self.snapshot);
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
            .update_queue_state_and_flush(queue_items, guide_pending, notice)
            .await;
        match result {
            Ok(()) => true,
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

    pub async fn finish(
        &self,
        session_key: &str,
        output: Option<String>,
        failed: bool,
    ) -> Option<ProgressCardMessageInfo> {
        let (_, session) = self.sessions.remove(session_key)?;
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
}

pub fn build_feishu_progress_card(
    snapshot: &ImAgentProgressSnapshot,
    streaming_mode: bool,
) -> serde_json::Value {
    let mut elements = Vec::new();
    if !snapshot.plan_steps.is_empty() {
        elements.push(build_plan_panel_element(snapshot));
    }
    if has_tool_state(snapshot) {
        elements.push(build_tool_panel_element(snapshot));
    }
    elements.push(build_status_panel_element(snapshot));
    if has_thinking_state(snapshot) {
        elements.push(build_thinking_panel_element(snapshot));
    }
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": format_output_markdown(snapshot),
        "element_id": OUTPUT_ELEMENT_ID
    }));

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

fn has_thinking_state(snapshot: &ImAgentProgressSnapshot) -> bool {
    snapshot
        .last_thought
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn current_has_plan_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if snapshot.plan_steps.is_empty() {
        return None;
    }
    Some(element_hash(&build_plan_panel_element(snapshot)))
}

fn current_has_tool_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if !has_tool_state(snapshot) {
        return None;
    }
    Some(element_hash(&build_tool_panel_element(snapshot)))
}

fn current_has_thinking_hash(snapshot: &ImAgentProgressSnapshot) -> Option<u64> {
    if !has_thinking_state(snapshot) {
        return None;
    }
    Some(element_hash(&build_thinking_panel_element(snapshot)))
}

fn output_hash(snapshot: &ImAgentProgressSnapshot) -> u64 {
    stable_hash(&format_output_markdown(snapshot))
}

fn status_hash(snapshot: &ImAgentProgressSnapshot) -> u64 {
    element_hash(&build_status_panel_element(snapshot))
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
                "content": format_plan_panel_title(&snapshot.plan_steps)
            }
        },
        "elements": [{
            "tag": "markdown",
            "content": format_plan_markdown(&snapshot.plan_steps),
            "element_id": PLAN_ELEMENT_ID
        }]
    })
}

fn format_plan_panel_title(steps: &[PlanStep]) -> String {
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
        text.push_str(&format!("\n参数：`{}`", truncate_str(arguments, 120)));
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
        "tag": "collapsible_panel",
        "element_id": THINKING_PANEL_ELEMENT_ID,
        "expanded": false,
        "background_color": "grey",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": format_thinking_panel_title(snapshot)
            }
        },
        "elements": [{
            "tag": "markdown",
            "content": format_thinking_markdown(snapshot),
            "element_id": THINKING_ELEMENT_ID
        }]
    })
}

fn format_thinking_markdown(snapshot: &ImAgentProgressSnapshot) -> String {
    snapshot
        .last_thought
        .as_deref()
        .map(crate::im_gateway::markdown_converter::convert_to_feishu_markdown)
        .unwrap_or_else(|| "暂无思考过程".to_string())
}

fn format_thinking_panel_title(snapshot: &ImAgentProgressSnapshot) -> String {
    let Some(thought) = snapshot
        .last_thought
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return "思考过程".to_string();
    };
    format!("思考过程：{}", truncate_one_line(thought, 72))
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
        None => "Token：统计中".to_string(),
    };
    if let Some(notice) = snapshot.activity_notice.as_deref() {
        format!("{token_title} · {notice}")
    } else {
        token_title
    }
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
            format!(
                "{}状态：{} · Loop {}/{}（已完成 {}）\nContext：{}\nToken：累计 {}，最近 {}\n压缩：{} 次 · 队列：{} · 引导：{}\n工作路径：`{}`",
                snapshot
                    .activity_notice
                    .as_deref()
                    .map(|notice| format!("提示：{notice}\n"))
                    .unwrap_or_default(),
                phase,
                status.current_loop_iteration,
                status.max_loop_iterations,
                status.completed_loop_iterations,
                context_text,
                token_text,
                last_token_text,
                status.compaction_count,
                queue_text,
                guide_text,
                status.work_dir.as_deref().unwrap_or("N/A")
            )
        }
        None => format!(
            "{}状态：{} · 队列：{} · 引导：{}",
            snapshot
                .activity_notice
                .as_deref()
                .map(|notice| format!("提示：{notice}\n"))
                .unwrap_or_default(),
            phase,
            queue_text,
            guide_text
        ),
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
mod tests {
    use super::*;
    use bifrost_agent::PlanStepStatus;

    #[test]
    fn progress_snapshot_tracks_tool_plan_queue_and_final_output() {
        let mut snapshot = ImAgentProgressSnapshot::new("s1", "initial task");
        assert_eq!(snapshot.title.as_deref(), Some("initial task"));
        snapshot.apply_event(AgentTurnProgressEvent::TitleUpdated {
            title: "Updated title".to_string(),
        });
        assert_eq!(snapshot.title.as_deref(), Some("Updated title"));
        snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
            content: "I will inspect the workspace.".to_string(),
        });
        assert_eq!(
            snapshot.last_thought.as_deref(),
            Some("I will inspect the workspace.")
        );
        snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
            tool_name: "shell".to_string(),
            arguments: "{\"cmd\":\"ls\"}".to_string(),
        });
        assert_eq!(
            snapshot
                .latest_tool
                .as_ref()
                .map(|tool| tool.tool_name.as_str()),
            Some("shell")
        );

        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "shell".to_string(),
                arguments: "{\"cmd\":\"ls\"}".to_string(),
                result: "Cargo.toml".to_string(),
                success: true,
            },
            duration_ms: 42,
        });
        snapshot.apply_event(AgentTurnProgressEvent::PlanUpdated {
            title: Some("Build".to_string()),
            steps: vec![PlanStep {
                step: "Run tests".to_string(),
                status: PlanStepStatus::InProgress,
            }],
        });
        snapshot.update_queue_state(
            vec![QueueItem {
                seq: 1,
                message: "next".to_string(),
            }],
            true,
            Some("已收到引导：prioritize logs".to_string()),
        );
        snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
            content: "done".to_string(),
        });

        assert_eq!(snapshot.phase, ImProgressPhase::Finished);
        assert_eq!(snapshot.output, "done");
        assert_eq!(snapshot.plan_steps.len(), 1);
        assert_eq!(snapshot.tool_calls.len(), 1);
        assert_eq!(snapshot.queue_items.len(), 1);
        assert!(snapshot.guide_pending);
        assert_eq!(
            snapshot.activity_notice.as_deref(),
            Some("已收到引导：prioritize logs")
        );
    }

    #[test]
    fn card_metric_count_uses_readable_kmb_units() {
        assert_eq!(bifrost_agent::format_status_metric_count(0), "0");
        assert_eq!(bifrost_agent::format_status_metric_count(999), "999");
        assert_eq!(bifrost_agent::format_status_metric_count(1_000), "1K");
        assert_eq!(bifrost_agent::format_status_metric_count(9_999), "10K");
        assert_eq!(bifrost_agent::format_status_metric_count(19_333), "19.3K");
        assert_eq!(bifrost_agent::format_status_metric_count(38_634), "38.6K");
        assert_eq!(bifrost_agent::format_status_metric_count(250_000), "250K");
        assert_eq!(bifrost_agent::format_status_metric_count(999_950), "1M");
        assert_eq!(bifrost_agent::format_status_metric_count(1_000_000), "1M");
        assert_eq!(bifrost_agent::format_status_metric_count(1_234_567), "1.2M");
        assert_eq!(
            bifrost_agent::format_status_metric_count(1_280_000_000),
            "1.3B"
        );
    }

    #[test]
    fn feishu_progress_card_formats_large_token_usage() {
        let mut snapshot = ImAgentProgressSnapshot::new("s1", "token task");
        snapshot.status = Some(ActiveTurnStatus {
            session_key: "s1".to_string(),
            state: "model_response".to_string(),
            started_at: 1,
            updated_at: 2,
            current_loop_iteration: 2,
            completed_loop_iterations: 1,
            max_loop_iterations: 1000,
            last_response_tokens: Some(1_234_567),
            total_tokens_used: Some(1_000_000),
            estimated_context_tokens: 260_000,
            context_window_tokens: Some(1_000_000),
            context_usage_percent: Some(26.0),
            compaction_count: 1,
            history_version: 7,
            work_dir: Some("/tmp/bifrost-work".to_string()),
            message_count: 9,
            local_tool_count: 12,
            mcp_tool_count: 5,
            pending_guide_messages: Vec::new(),
            user_turn_count: 2,
            agent_type: Some("Bifrost Agent".to_string()),
            runner_type: Some("bifrost_agent".to_string()),
            runner_id: None,
            external_conversation_id: None,
            external_thread_id: None,
        });

        let card = build_feishu_progress_card(&snapshot, true);
        let serialized = serde_json::to_string(&card).unwrap();
        assert!(serialized.contains("Token：累计 1M · 最近 1.2M"));
        assert!(serialized.contains("Context：~260K / 1M (26.0%)"));
        assert!(serialized.contains("Token：累计 1M，最近 1.2M"));
        assert!(!serialized.contains("Token：累计 1000000"));
        assert!(!serialized.contains("最近 1234567"));
        assert!(!serialized.contains("Context：~260000 / 1000000"));
    }

    #[test]
    fn feishu_progress_card_uses_json_2_streaming_and_stable_elements() {
        let snapshot = ImAgentProgressSnapshot::new("s1", "initial task");
        let card = build_feishu_progress_card(&snapshot, true);
        assert_eq!(card["schema"], "2.0");
        assert_eq!(card["config"]["streaming_mode"], true);
        let body = card["body"]["elements"].as_array().unwrap();
        let serialized = serde_json::to_string(body).unwrap();
        assert!(serialized.contains("处理中..."));
        assert!(!serialized.contains("最终输出"));
        for id in [
            OUTPUT_ELEMENT_ID,
            STATUS_PANEL_ELEMENT_ID,
            FOOTER_ELEMENT_ID,
        ] {
            assert!(serialized.contains(id), "missing element id {id}");
        }
        for id in [
            PLAN_PANEL_ELEMENT_ID,
            PLAN_ELEMENT_ID,
            TOOL_PANEL_ELEMENT_ID,
            TOOL_LOG_ELEMENT_ID,
            THINKING_PANEL_ELEMENT_ID,
            THINKING_ELEMENT_ID,
        ] {
            assert!(!serialized.contains(id), "unexpected empty module id {id}");
        }
        assert_eq!(card["header"]["title"]["content"], "initial task");

        let mut populated = snapshot.clone();
        populated.apply_event(AgentTurnProgressEvent::AssistantDelta {
            content: "Inspecting files before running tests.".to_string(),
        });
        populated.apply_event(AgentTurnProgressEvent::ToolStarted {
            tool_name: "shell".to_string(),
            arguments: "{}".to_string(),
        });
        populated.apply_event(AgentTurnProgressEvent::PlanUpdated {
            title: Some("Build".to_string()),
            steps: vec![PlanStep {
                step: "Run tests".to_string(),
                status: PlanStepStatus::InProgress,
            }],
        });
        populated.update_queue_state(
            vec![QueueItem {
                seq: 7,
                message: "queued".to_string(),
            }],
            true,
            Some("已收到引导：rerun failed path".to_string()),
        );
        let populated_card = build_feishu_progress_card(&populated, true);
        let populated_body = populated_card["body"]["elements"].as_array().unwrap();
        let populated_serialized = serde_json::to_string(populated_body).unwrap();
        for id in [
            PLAN_ELEMENT_ID,
            TOOL_PANEL_ELEMENT_ID,
            TOOL_LOG_ELEMENT_ID,
            THINKING_PANEL_ELEMENT_ID,
            THINKING_ELEMENT_ID,
            "已收到引导：rerun failed path",
        ] {
            assert!(
                populated_serialized.contains(id),
                "missing populated module id {id}"
            );
        }
        assert_eq!(populated_card["header"]["title"]["content"], "Build");
        assert_eq!(
            populated_body[0]["header"]["title"]["content"],
            "任务计划：Run tests"
        );
        assert_eq!(
            populated_body.last().unwrap()["element_id"],
            OUTPUT_ELEMENT_ID
        );
        let thinking_title = populated_body
            .iter()
            .find(|element| element["element_id"] == THINKING_PANEL_ELEMENT_ID)
            .and_then(|element| element["header"]["title"]["content"].as_str())
            .unwrap();
        assert_eq!(
            thinking_title,
            "思考过程：Inspecting files before running tests."
        );
    }

    #[test]
    fn progress_update_uuid_stays_short_and_avoids_card_id() {
        let long_card_id = format!("card_{}", "x".repeat(120));
        let mut handle = FeishuProgressCardHandle {
            card_id: long_card_id.clone(),
            message_id: Some("om_1".to_string()),
            sequence: 1,
            generation: 3,
            rendered_title: "title".to_string(),
            rendered_has_plan: false,
            rendered_has_tool: false,
            rendered_has_thinking: false,
            rendered_phase: ImProgressPhase::Running,
            rendered_output_hash: 0,
            rendered_plan_hash: None,
            rendered_tool_hash: None,
            rendered_status_hash: 0,
            rendered_thinking_hash: None,
        };

        let (_, update_uuid) = handle.next_sequence();
        assert!(update_uuid.len() <= 50, "uuid too long: {update_uuid}");
        assert!(
            !update_uuid.contains(&long_card_id),
            "uuid must not include card_id"
        );
    }
}
