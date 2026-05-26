use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bifrost_agent::tools::ToolHandler;
use bifrost_agent::types::ToolResult;
use serde::Deserialize;
use serde_json::Value;

use super::schedule_store::ImScheduleStore;
use super::scheduler::ImScheduler;
use super::target_store::ImTargetStore;
use super::types::{ImMessageChannelBinding, ImSchedule, ScheduleTaskType};

#[derive(Clone, Debug, Default)]
pub struct ScheduleToolContext {
    pub message_channel: Option<ImMessageChannelBinding>,
}

pub fn register_schedule_tools(
    registry: &mut bifrost_agent::ToolRegistry,
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
    target_store: Arc<ImTargetStore>,
    context: ScheduleToolContext,
) {
    registry.register(Arc::new(ScheduleListTool::new(schedule_store.clone())));
    registry.register(Arc::new(ScheduleCreateTool::new(
        schedule_store.clone(),
        scheduler.clone(),
        target_store.clone(),
        context.clone(),
    )));
    registry.register(Arc::new(ScheduleUpdateTool::new(
        schedule_store.clone(),
        scheduler.clone(),
        target_store,
        context,
    )));
    registry.register(Arc::new(ScheduleDeleteTool::new(schedule_store, scheduler)));
}

struct ScheduleListTool {
    schedule_store: Arc<ImScheduleStore>,
}

impl ScheduleListTool {
    fn new(schedule_store: Arc<ImScheduleStore>) -> Self {
        Self { schedule_store }
    }
}

#[derive(Debug, Deserialize)]
struct ScheduleListArgs {
    enabled: Option<bool>,
    task_type: Option<ScheduleTaskType>,
}

#[async_trait]
impl ToolHandler for ScheduleListTool {
    fn name(&self) -> &str {
        "schedule_list"
    }

    fn description(&self) -> &str {
        "List configured scheduled tasks. Use this before changing schedules when the user asks about existing cron or interval tasks."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "enabled": {"type": "boolean", "description": "Optional filter for enabled schedules"},
                "task_type": {"type": "string", "enum": ["script", "agent"], "description": "Optional filter by scheduled task type"}
            }
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args = parse_args::<ScheduleListArgs>(arguments).unwrap_or(ScheduleListArgs {
            enabled: None,
            task_type: None,
        });
        let schedules: Vec<_> = self
            .schedule_store
            .list()
            .into_iter()
            .filter(|schedule| {
                args.enabled
                    .is_none_or(|enabled| schedule.enabled == enabled)
            })
            .filter(|schedule| {
                args.task_type
                    .is_none_or(|task_type| schedule.task_type == task_type)
            })
            .collect();
        json_tool_result(true, &schedules)
    }
}

struct ScheduleCreateTool {
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
    target_store: Arc<ImTargetStore>,
    context: ScheduleToolContext,
}

impl ScheduleCreateTool {
    fn new(
        schedule_store: Arc<ImScheduleStore>,
        scheduler: Arc<ImScheduler>,
        target_store: Arc<ImTargetStore>,
        context: ScheduleToolContext,
    ) -> Self {
        Self {
            schedule_store,
            scheduler,
            target_store,
            context,
        }
    }
}

#[async_trait]
impl ToolHandler for ScheduleCreateTool {
    fn name(&self) -> &str {
        "schedule_create"
    }

    fn description(&self) -> &str {
        "Create a scheduled task. Supports script schedules and agent schedules with a preset prompt. Use task_type=\"agent\" with agent.prompt for Agent tasks. Schedules send results to message_channel."
    }

    fn parameters_schema(&self) -> Value {
        schedule_write_schema(true)
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let mut schedule = match serde_json::from_str::<ImSchedule>(arguments) {
            Ok(schedule) => schedule,
            Err(error) => return error_tool_result(format!("invalid schedule: {error}")),
        };
        if let Err(error) = normalize_schedule_with_context(
            &mut schedule,
            Some(&self.target_store),
            Some(&self.context),
        ) {
            return error_tool_result(error);
        }
        match self.schedule_store.add(schedule.clone()) {
            Ok(()) => {
                self.scheduler.notify_reschedule();
                json_tool_result(true, &schedule)
            }
            Err(error) => error_tool_result(error.to_string()),
        }
    }
}

struct ScheduleUpdateTool {
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
    target_store: Arc<ImTargetStore>,
    context: ScheduleToolContext,
}

impl ScheduleUpdateTool {
    fn new(
        schedule_store: Arc<ImScheduleStore>,
        scheduler: Arc<ImScheduler>,
        target_store: Arc<ImTargetStore>,
        context: ScheduleToolContext,
    ) -> Self {
        Self {
            schedule_store,
            scheduler,
            target_store,
            context,
        }
    }
}

#[async_trait]
impl ToolHandler for ScheduleUpdateTool {
    fn name(&self) -> &str {
        "schedule_update"
    }

    fn description(&self) -> &str {
        "Update an existing scheduled task by id. Fields are patched at the top level; nested trigger/script/agent objects replace the existing object."
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = schedule_write_schema(false);
        if let Some(required) = schema.get_mut("required") {
            *required = serde_json::json!(["id"]);
        }
        schema
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args_value = match serde_json::from_str::<Value>(arguments) {
            Ok(value) => value,
            Err(error) => return error_tool_result(format!("invalid arguments: {error}")),
        };
        let Some(id) = args_value.get("id").and_then(Value::as_str) else {
            return error_tool_result("id is required".to_string());
        };
        let Some(existing) = self.schedule_store.get(id) else {
            return error_tool_result(format!("schedule '{id}' not found"));
        };

        let mut merged = match serde_json::to_value(existing) {
            Ok(value) => value,
            Err(error) => return error_tool_result(format!("serialize schedule: {error}")),
        };
        merge_patch_value(&mut merged, &args_value);
        if let Some(patch) = args_value.get("patch") {
            merge_patch_value(&mut merged, patch);
        }
        let mut schedule = match serde_json::from_value::<ImSchedule>(merged) {
            Ok(schedule) => schedule,
            Err(error) => return error_tool_result(format!("invalid schedule patch: {error}")),
        };
        if let Err(error) = normalize_schedule_with_context(
            &mut schedule,
            Some(&self.target_store),
            Some(&self.context),
        ) {
            return error_tool_result(error);
        }
        match self.schedule_store.update(schedule.clone()) {
            Ok(()) => {
                self.scheduler.notify_reschedule();
                json_tool_result(true, &schedule)
            }
            Err(error) => error_tool_result(error.to_string()),
        }
    }
}

struct ScheduleDeleteTool {
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
}

impl ScheduleDeleteTool {
    fn new(schedule_store: Arc<ImScheduleStore>, scheduler: Arc<ImScheduler>) -> Self {
        Self {
            schedule_store,
            scheduler,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScheduleDeleteArgs {
    id: String,
}

#[async_trait]
impl ToolHandler for ScheduleDeleteTool {
    fn name(&self) -> &str {
        "schedule_delete"
    }

    fn description(&self) -> &str {
        "Delete a scheduled task by id."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Schedule id to delete"}
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args = match parse_args::<ScheduleDeleteArgs>(arguments) {
            Ok(args) => args,
            Err(error) => return error_tool_result(error),
        };
        match self.schedule_store.delete(&args.id) {
            Ok(()) => {
                self.scheduler.notify_reschedule();
                json_tool_result(true, &serde_json::json!({"deleted": args.id}))
            }
            Err(error) => error_tool_result(error.to_string()),
        }
    }
}

fn schedule_write_schema(create: bool) -> Value {
    let mut required = vec!["name", "trigger"];
    if create {
        required.push("task_type");
    }
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {"type": "string", "description": "Stable schedule id. Omit on create to generate one."},
            "name": {"type": "string"},
            "enabled": {"type": "boolean"},
            "message_channel": {
                "type": "object",
                "description": "IM channel binding used for notifications and send_msg defaults. If omitted by an injected agent tool, the current IM source/default channel is used.",
                "properties": {
                    "provider_id": {"type": "string"},
                    "target_id": {"type": "string"},
                    "target_mode": {"type": "string", "enum": ["source_thread", "source_user", "owner", "configured_target"]}
                },
                "required": ["provider_id", "target_id", "target_mode"]
            },
            "trigger": {
                "type": "object",
                "description": "Trigger object. Use type=cron with expr/timezone, or type=interval with every_ms.",
                "properties": {
                    "type": {"type": "string", "description": "cron or interval"},
                    "expr": {"type": "string"},
                    "timezone": {"type": "string"},
                    "every_ms": {"type": "integer", "minimum": 1}
                },
                "required": ["type"]
            },
            "task_type": {"type": "string", "enum": ["script", "agent"]},
            "script": {
                "type": "object",
                "properties": {
                    "script_text": {"type": "string"},
                    "script_file": {"type": "string"},
                    "cwd": {"type": "string"},
                    "env": {"type": "object", "additionalProperties": {"type": "string"}}
                }
            },
            "agent": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Preset prompt sent to the agent"},
                    "runner_id": {"type": "string", "description": "Optional runner id. Use bifrost_agent or omit for the built-in Bifrost Agent; use a configured external runner id such as a ChatGPT Web Runner to run externally."},
                    "initial_prompt": {"type": "string", "description": "Optional first-round/persona prompt prepended as runner instructions for external runners."},
                    "session_key": {"type": "string"},
                    "work_dir": {"type": "string"},
                    "system_prompt": {"type": "string"}
                }
            },
            "timeout_ms": {"type": "integer", "minimum": 1},
            "max_output_bytes": {"type": "integer", "minimum": 1},
            "patch": {"type": "object", "description": "Optional patch object for schedule_update"}
        },
        "required": required
    })
}

pub fn normalize_schedule(schedule: &mut ImSchedule) -> Result<(), String> {
    normalize_schedule_with_context(schedule, None, None)
}

pub fn normalize_schedule_with_context(
    schedule: &mut ImSchedule,
    target_store: Option<&ImTargetStore>,
    context: Option<&ScheduleToolContext>,
) -> Result<(), String> {
    let now = now_ms();
    if schedule.id.trim().is_empty() {
        schedule.id = uuid::Uuid::new_v4().as_simple().to_string();
    }
    schedule.infer_task_type();
    bind_schedule_message_channel(schedule, target_store, context)?;
    schedule.validate_for_save()?;
    if schedule.created_at == 0 {
        schedule.created_at = now;
    }
    schedule.updated_at = now;
    schedule.next_run_at = ImScheduler::compute_next_run_for_schedule(schedule, now);
    Ok(())
}

fn bind_schedule_message_channel(
    schedule: &mut ImSchedule,
    _target_store: Option<&ImTargetStore>,
    context: Option<&ScheduleToolContext>,
) -> Result<(), String> {
    if let Some(channel) = schedule.message_channel.as_mut() {
        channel.provider_id = channel.provider_id.trim().to_string();
        channel.target_id = channel.target_id.trim().to_string();
        if channel.provider_id.is_empty() || channel.target_id.is_empty() {
            return Err(
                "message_channel.provider_id and message_channel.target_id are required"
                    .to_string(),
            );
        }
        return Ok(());
    }

    if let Some(channel) = context.and_then(|ctx| ctx.message_channel.clone()) {
        schedule.message_channel = Some(channel);
        return Ok(());
    }

    if matches!(
        schedule.task_type,
        ScheduleTaskType::Agent | ScheduleTaskType::Script
    ) {
        return Err(
            "schedule requires message_channel unless an agent turn provides an IM source/default channel"
                .to_string(),
        );
    }

    Ok(())
}

fn merge_patch_value(target: &mut Value, patch: &Value) {
    let (Some(target_obj), Some(patch_obj)) = (target.as_object_mut(), patch.as_object()) else {
        return;
    };
    for (key, value) in patch_obj {
        if key == "patch" {
            continue;
        }
        target_obj.insert(key.clone(), value.clone());
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T, String> {
    if arguments.trim().is_empty() {
        serde_json::from_str("{}").map_err(|error| format!("invalid arguments: {error}"))
    } else {
        serde_json::from_str(arguments).map_err(|error| format!("invalid arguments: {error}"))
    }
}

fn json_tool_result<T: serde::Serialize>(success: bool, value: &T) -> ToolResult {
    ToolResult {
        success,
        output: serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string()),
        runtime_events: Vec::new(),
    }
}

fn error_tool_result(error: String) -> ToolResult {
    ToolResult {
        success: false,
        output: error,
        runtime_events: Vec::new(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use bifrost_agent::tools::ToolHandler;

    use super::*;

    #[tokio::test]
    async fn schedule_tools_create_update_list_delete_agent_schedule() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(ImScheduleStore::new(temp_dir.path()));
        let scheduler = Arc::new(ImScheduler::new());
        let target_store = Arc::new(ImTargetStore::new(temp_dir.path()));
        let context = ScheduleToolContext {
            message_channel: Some(ImMessageChannelBinding {
                provider_id: "weixin-test".to_string(),
                target_id: "owner-user".to_string(),
                target_mode: crate::im_gateway::types::MessageTargetMode::Owner,
            }),
        };
        let create = ScheduleCreateTool::new(
            store.clone(),
            scheduler.clone(),
            target_store.clone(),
            context.clone(),
        );
        let update =
            ScheduleUpdateTool::new(store.clone(), scheduler.clone(), target_store, context);
        let list = ScheduleListTool::new(store.clone());
        let delete = ScheduleDeleteTool::new(store.clone(), scheduler);

        let created = create
            .execute(
                r#"{
                    "id":"daily-agent",
                    "name":"Daily Agent",
                    "enabled":true,
                    "task_type":"agent",
                    "trigger":{"type":"interval","every_ms":60000},
                    "agent":{"prompt":"Summarize yesterday's traffic"}
                }"#,
                Path::new("."),
            )
            .await;
        assert!(created.success, "{}", created.output);
        assert_eq!(
            store.get("daily-agent").expect("created").task_type,
            ScheduleTaskType::Agent
        );
        assert_eq!(
            store
                .get("daily-agent")
                .expect("created")
                .message_channel
                .expect("channel")
                .provider_id,
            "weixin-test"
        );

        let updated = update
            .execute(
                r#"{"id":"daily-agent","enabled":false,"agent":{"prompt":"Updated prompt"}}"#,
                Path::new("."),
            )
            .await;
        assert!(updated.success, "{}", updated.output);
        let stored = store.get("daily-agent").expect("updated");
        assert!(!stored.enabled);
        assert_eq!(stored.agent.expect("agent").prompt, "Updated prompt");

        let listed = list
            .execute(r#"{"task_type":"agent"}"#, Path::new("."))
            .await;
        assert!(listed.success);
        assert!(listed.output.contains("daily-agent"));

        let deleted = delete
            .execute(r#"{"id":"daily-agent"}"#, Path::new("."))
            .await;
        assert!(deleted.success, "{}", deleted.output);
        assert!(store.get("daily-agent").is_none());
    }

    #[test]
    fn schedule_write_schema_avoids_combinators_for_model_compatibility() {
        let schema = schedule_write_schema(true);
        let trigger = schema
            .pointer("/properties/trigger")
            .expect("trigger schema");
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("oneOf").is_none());
        assert!(schema.get("allOf").is_none());
        assert!(trigger.get("anyOf").is_none());
        assert!(trigger.get("oneOf").is_none());
        assert!(trigger.get("allOf").is_none());
    }
}
