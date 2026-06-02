use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::{WorkflowDocument, WorkflowScheduleState, WorkflowStore};

static WORKFLOW_SCHEDULER: Lazy<Mutex<Option<WorkflowSchedulerHandle>>> =
    Lazy::new(|| Mutex::new(None));
static RUNNING_SCHEDULED_WORKFLOWS: Lazy<Mutex<BTreeSet<String>>> =
    Lazy::new(|| Mutex::new(BTreeSet::new()));

struct WorkflowSchedulerHandle {
    root: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

pub async fn ensure_workflow_scheduler_started() {
    let store = WorkflowStore::default();
    let root = store.root_dir();
    let mut current = WORKFLOW_SCHEDULER.lock();
    if let Some(active) = current.as_ref() {
        if active.root == root && !active.handle.is_finished() {
            return;
        }
    }
    if let Some(active) = current.take() {
        active.handle.abort();
    }
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = tick_workflow_scheduler(&store).await {
                warn!(error = %error, "AI Workflow scheduler tick failed");
            }
        }
    });
    *current = Some(WorkflowSchedulerHandle { root, handle });
}

async fn tick_workflow_scheduler(store: &WorkflowStore) -> Result<(), String> {
    let now = now_ms();
    for workflow in store.list().map_err(|error| error.to_string())? {
        let document = match store.get(&workflow.id) {
            Ok(document) => document,
            Err(error) => {
                warn!(workflow_id = %workflow.id, error = %error, "failed to load Workflow for schedule tick");
                continue;
            }
        };
        for (trigger_index, trigger) in document.spec.triggers.iter().enumerate() {
            let Some(schedule) = parse_schedule_trigger(trigger) else {
                continue;
            };
            if !schedule.enabled {
                continue;
            }
            let existing_state = store
                .get_schedule_state(&document.metadata.id, trigger_index)
                .map_err(|error| error.to_string())?;
            let mut state = match existing_state {
                Some(state) => state,
                None => {
                    let state = WorkflowScheduleState {
                        workflow_id: document.metadata.id.clone(),
                        trigger_index,
                        next_run_at_ms: compute_next_schedule_run(&schedule, now),
                        updated_at_ms: now,
                        ..Default::default()
                    };
                    store
                        .save_schedule_state(&state)
                        .map_err(|error| error.to_string())?;
                    state
                }
            };
            if state.next_run_at_ms.is_none() {
                state.next_run_at_ms = compute_next_schedule_run(&schedule, now);
                state.updated_at_ms = now;
                store
                    .save_schedule_state(&state)
                    .map_err(|error| error.to_string())?;
                continue;
            }
            if state.next_run_at_ms.is_some_and(|next| next <= now) {
                if let Err(error) = spawn_scheduled_workflow_run(
                    store.clone(),
                    document.clone(),
                    schedule.clone(),
                    state,
                ) {
                    debug!(workflow_id = %document.metadata.id, trigger_index, error = %error, "AI Workflow schedule tick skipped trigger");
                }
            }
        }
    }
    Ok(())
}

fn spawn_scheduled_workflow_run(
    store: WorkflowStore,
    workflow: WorkflowDocument,
    schedule: WorkflowScheduleTrigger,
    mut state: WorkflowScheduleState,
) -> Result<JoinHandle<()>, String> {
    let key = format!("{}#{}", workflow.metadata.id, state.trigger_index);
    {
        let mut running = RUNNING_SCHEDULED_WORKFLOWS.lock();
        if !running.insert(key.clone()) {
            debug!(workflow_id = %workflow.metadata.id, trigger_index = state.trigger_index, "AI Workflow schedule already running");
            return Err("AI Workflow schedule is already running".to_string());
        }
    }
    let handle = tokio::spawn(async move {
        let now = now_ms();
        state.last_run_at_ms = Some(now);
        state.updated_at_ms = now;
        state.next_run_at_ms = compute_next_schedule_run(&schedule, now);
        if let Err(error) = store.save_schedule_state(&state) {
            warn!(workflow_id = %workflow.metadata.id, error = %error, "failed to persist scheduled Workflow running state");
        }
        let run_result = store
            .create_run_async(&workflow.metadata.id, schedule.inputs.clone())
            .await;
        let finished = now_ms();
        match run_result {
            Ok(run) => {
                info!(workflow_id = %workflow.metadata.id, run_id = %run.id, status = %run.status, "scheduled AI Workflow run completed");
                state.last_run_id = Some(run.id);
                state.last_status = Some(run.status);
                state.last_error = None;
            }
            Err(error) => {
                warn!(workflow_id = %workflow.metadata.id, error = %error, "scheduled AI Workflow run failed");
                state.last_status = Some("failed".to_string());
                state.last_error = Some(error);
            }
        }
        state.updated_at_ms = finished;
        if let Err(error) = store.save_schedule_state(&state) {
            warn!(workflow_id = %workflow.metadata.id, error = %error, "failed to persist scheduled Workflow result state");
        }
        RUNNING_SCHEDULED_WORKFLOWS.lock().remove(&key);
    });
    Ok(handle)
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowScheduleTrigger {
    pub(crate) enabled: bool,
    cron: Option<String>,
    every_ms: Option<u64>,
    pub(crate) inputs: Value,
}

pub(crate) fn parse_schedule_trigger(trigger: &Value) -> Option<WorkflowScheduleTrigger> {
    let trigger_type = trigger.get("type").and_then(Value::as_str)?;
    if trigger_type != "schedule" && trigger_type != "cron" && trigger_type != "interval" {
        return None;
    }
    Some(WorkflowScheduleTrigger {
        enabled: trigger
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        cron: trigger
            .get("cron")
            .or_else(|| trigger.get("expr"))
            .and_then(Value::as_str)
            .map(str::to_string),
        every_ms: trigger
            .get("everyMs")
            .or_else(|| trigger.get("every_ms"))
            .and_then(Value::as_u64),
        inputs: trigger.get("inputs").cloned().unwrap_or_else(|| json!({})),
    })
}

pub(crate) fn compute_next_schedule_run(
    trigger: &WorkflowScheduleTrigger,
    now_ms: u64,
) -> Option<u64> {
    if let Some(every_ms) = trigger.every_ms.filter(|value| *value > 0) {
        return Some(now_ms + every_ms);
    }
    trigger
        .cron
        .as_deref()
        .and_then(|cron| compute_next_simple_cron(cron, now_ms))
}

fn compute_next_simple_cron(expr: &str, now_ms: u64) -> Option<u64> {
    let parts = expr.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    let minute_part = parts[0];
    let hour_part = parts[1];
    if let Some(interval) = minute_part
        .strip_prefix("*/")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= 60)
    {
        let interval_ms = interval * 60_000;
        let offset = now_ms % interval_ms;
        return Some(now_ms + interval_ms.saturating_sub(offset).max(1_000));
    }
    let minute = minute_part
        .parse::<u64>()
        .ok()
        .filter(|value| *value < 60)?;
    if hour_part == "*" {
        let hour_ms = 60 * 60_000;
        let minute_ms = minute * 60_000;
        let current_hour_offset = now_ms % hour_ms;
        return Some(if current_hour_offset < minute_ms {
            now_ms + (minute_ms - current_hour_offset)
        } else {
            now_ms + (hour_ms - current_hour_offset) + minute_ms
        });
    }
    let hour = hour_part.parse::<u64>().ok().filter(|value| *value < 24)?;
    let day_ms = 24 * 60 * 60_000;
    let target_offset = (hour * 60 + minute) * 60_000;
    let current_day_offset = now_ms % day_ms;
    Some(if current_day_offset < target_offset {
        now_ms + (target_offset - current_day_offset)
    } else {
        now_ms + (day_ms - current_day_offset) + target_offset
    })
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
