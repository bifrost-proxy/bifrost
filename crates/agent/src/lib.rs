//! Shared state and persistence support for Bifrost external runners.
//!
//! The former built-in model/tool runtime has been removed. This crate keeps
//! the historical package name because IM, WebUI, and external-runner code use
//! its configuration, session metadata, progress events, skills discovery, and
//! JSONL persistence types.

pub mod agents_md;
pub mod config;
pub mod history;
pub mod persistence;
pub mod session;
pub mod session_status;
pub mod skills;
pub mod tools;
pub mod types;

pub use config::{
    AgentConfig, AgentConfigStore, AgentRunnerMode, HistoryConfig, HistoryPersistence,
    ImMessageChannelBinding, MessageTargetMode,
};
pub use session::{
    AgentSession, AgentSessionEvent, AgentSessionManager, SessionDetail, SessionInfo,
};
pub use session_status::{
    format_active_turn_status_text, format_active_turn_status_text_with_context,
    format_context_management_status, format_conversation_ref, format_model_ref,
    format_optional_status_text, format_status_metric_count, snapshot_agent_context,
    ActiveTurnStatus, AgentCompactionProgress, AgentContextSnapshot, AgentTurnProgressEvent,
    AgentTurnProgressSender, StatusRuntimeContext,
};
pub use skills::{install_system_skills, SkillMetadata, SkillScope, SkillsManager};
pub use tools::update_plan::{PlanStep, PlanStepStatus, UpdatePlanArgs};
pub use types::{ChatImageInput, ChatMessage, CollaborationMode, ToolCallLog, TurnResult};
