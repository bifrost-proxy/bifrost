use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bifrost_agent::tools::ToolHandler;
use bifrost_agent::types::ToolResult;
use serde::Deserialize;
use serde_json::Value;

use super::schedule_store::ImScheduleStore;
use super::scheduler::ImScheduler;
use super::types::{ImSchedule, ScheduleTaskType};

pub fn register_schedule_tools(
    registry: &mut bifrost_agent::ToolRegistry,
    schedule_store: Arc<ImScheduleStore>,
    scheduler: Arc<ImScheduler>,
) {
    registry.register(Arc::new(ScheduleListTool::new(schedule_store.clone())));
    registry.register(Arc::new(ScheduleCreateTool::new(
        schedule_store.clone(),
        scheduler.clone(),
    )));
    registry.register(Arc::new(ScheduleUpdateTool::new(
        schedule_store.clone(),
        scheduler.clone(),
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
}

impl ScheduleCreateTool {
    fn new(schedule_store: Arc<ImScheduleStore>, scheduler: Arc<ImScheduler>) -> Self {
        Self {
            schedule_store,
            scheduler,
        }
    }
}

#[async_trait]
impl ToolHandler for ScheduleCreateTool {
    fn name(&self) -> &str {
        "schedule_create"
    }

    fn description(&self) -> &str {
        "Create a scheduled task. Supports script schedules and agent schedules with a preset prompt. Use task_type=\"agent\" with agent.prompt for Agent tasks."
    }

    fn parameters_schema(&self) -> Value {
        schedule_write_schema(true)
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let mut schedule = match serde_json::from_str::<ImSchedule>(arguments) {
            Ok(schedule) => schedule,
            Err(error) => return error_tool_result(format!("invalid schedule: {error}")),
        };
        if let Err(error) = normalize_schedule(&mut schedule) {
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
}

impl ScheduleUpdateTool {
    fn new(schedule_store: Arc<ImScheduleStore>, scheduler: Arc<ImScheduler>) -> Self {
        Self {
            schedule_store,
            scheduler,
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
        if let Err(error) = normalize_schedule(&mut schedule) {
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
            "target_id": {"type": "string", "description": "Required for script schedules; optional for agent schedules."},
            "trigger": {
                "type": "object",
                "oneOf": [
                    {
                        "properties": {
                            "type": {"const": "cron"},
                            "expr": {"type": "string"},
                            "timezone": {"type": "string"}
                        },
                        "required": ["type", "expr"]
                    },
                    {
                        "properties": {
                            "type": {"const": "interval"},
                            "every_ms": {"type": "integer", "minimum": 1}
                        },
                        "required": ["type", "every_ms"]
                    }
                ]
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
    let now = now_ms();
    if schedule.id.trim().is_empty() {
        schedule.id = uuid::Uuid::new_v4().as_simple().to_string();
    }
    schedule.infer_task_type();
    schedule.validate_for_save()?;
    if schedule.created_at == 0 {
        schedule.created_at = now;
    }
    schedule.updated_at = now;
    schedule.next_run_at = ImScheduler::compute_next_run_for_schedule(schedule, now);
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
    }
}

fn error_tool_result(error: String) -> ToolResult {
    ToolResult {
        success: false,
        output: error,
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
        let create = ScheduleCreateTool::new(store.clone(), scheduler.clone());
        let update = ScheduleUpdateTool::new(store.clone(), scheduler.clone());
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
}
