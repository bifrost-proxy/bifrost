use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::im_gateway::types::*;

pub struct ImScheduler {
    running_tasks: RwLock<HashMap<String, RunningTask>>,
    notify: Arc<Notify>,
}

struct RunningTask {
    #[allow(dead_code)]
    schedule_id: String,
    task_handle: tokio::task::JoinHandle<()>,
}

impl Default for ImScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl ImScheduler {
    pub fn new() -> Self {
        Self {
            running_tasks: RwLock::new(HashMap::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Compute the next run time for a given schedule trigger.
    ///
    /// For `Interval` triggers: `next_run = last_run_at + every_ms`.
    /// If `last_run_at` is None, returns `now_ms + every_ms`.
    ///
    /// For `Cron` triggers: simplified implementation that returns
    /// `now_ms + 60_000` as a placeholder. A full cron parser should be
    /// integrated in a future iteration.
    pub fn compute_next_run(trigger: &ScheduleTrigger, now_ms: u64) -> Option<u64> {
        match trigger {
            ScheduleTrigger::Interval { every_ms } => {
                if *every_ms == 0 {
                    return None;
                }
                Some(now_ms + every_ms)
            }
            ScheduleTrigger::Cron { expr, .. } => {
                if expr.is_empty() {
                    return None;
                }
                // Simplified cron: parse basic interval-like patterns.
                // Full cron parsing requires a dedicated crate; for now, we attempt
                // to handle `*/N * * * *` (every N minutes) as a common pattern.
                if let Some(next) = Self::parse_simple_cron(expr, now_ms) {
                    Some(next)
                } else {
                    // Fallback: schedule 60s from now for unrecognized cron expressions
                    debug!(
                        cron_expr = %expr,
                        "unrecognized cron expression, defaulting to 60s interval"
                    );
                    Some(now_ms + 60_000)
                }
            }
        }
    }

    /// Attempt to parse simple cron patterns like `*/N * * * *` (every N minutes).
    fn parse_simple_cron(expr: &str, now_ms: u64) -> Option<u64> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() < 5 {
            return None;
        }

        let minute_part = parts[0];

        // Handle `*/N` pattern (every N minutes)
        if let Some(interval_str) = minute_part.strip_prefix("*/") {
            if let Ok(minutes) = interval_str.parse::<u64>() {
                if minutes > 0 && minutes <= 60 {
                    let interval_ms = minutes * 60 * 1000;
                    // Align to next interval boundary
                    let current_minute_ms = now_ms % interval_ms;
                    let next = now_ms + (interval_ms - current_minute_ms);
                    return Some(next);
                }
            }
        }

        // Handle specific minute `N * * * *` (run at minute N of each hour)
        if let Ok(minute) = minute_part.parse::<u64>() {
            if minute < 60 {
                let hour_ms: u64 = 60 * 60 * 1000;
                let minute_ms = minute * 60 * 1000;
                let current_hour_offset = now_ms % hour_ms;
                if current_hour_offset < minute_ms {
                    return Some(now_ms + (minute_ms - current_hour_offset));
                } else {
                    return Some(now_ms + (hour_ms - current_hour_offset) + minute_ms);
                }
            }
        }

        None
    }

    /// Check which schedules are due for execution at the given time.
    pub fn get_due_schedules(&self, schedules: &[ImSchedule], now_ms: u64) -> Vec<ImSchedule> {
        schedules
            .iter()
            .filter(|s| {
                if !s.enabled {
                    return false;
                }
                match s.next_run_at {
                    Some(next_run) => next_run <= now_ms,
                    None => false,
                }
            })
            .cloned()
            .collect()
    }

    /// Register a running task handle for a schedule.
    pub fn register_running(&self, schedule_id: &str, handle: tokio::task::JoinHandle<()>) {
        let mut tasks = self.running_tasks.write();
        tasks.insert(
            schedule_id.to_string(),
            RunningTask {
                schedule_id: schedule_id.to_string(),
                task_handle: handle,
            },
        );
        info!(schedule_id = %schedule_id, "registered running task");
    }

    /// Check whether a specific schedule has a running task.
    pub fn is_running(&self, schedule_id: &str) -> bool {
        let tasks = self.running_tasks.read();
        if let Some(task) = tasks.get(schedule_id) {
            !task.task_handle.is_finished()
        } else {
            false
        }
    }

    /// Remove a completed task from the running registry.
    pub fn remove_completed(&self, schedule_id: &str) {
        let mut tasks = self.running_tasks.write();
        if let Some(task) = tasks.get(schedule_id) {
            if task.task_handle.is_finished() {
                tasks.remove(schedule_id);
                debug!(schedule_id = %schedule_id, "removed completed task");
            }
        }
    }

    /// Notify the scheduler loop to re-check schedules immediately.
    pub fn notify_reschedule(&self) {
        self.notify.notify_one();
    }

    /// Get a clone of the notify handle for use in the scheduler loop.
    pub fn notify_handle(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Stop all running tasks (e.g., on shutdown).
    pub fn stop_all(&self) {
        let mut tasks = self.running_tasks.write();
        for (id, task) in tasks.drain() {
            warn!(schedule_id = %id, "aborting running task on shutdown");
            task.task_handle.abort();
        }
    }

    /// Get count of currently running tasks.
    pub fn running_count(&self) -> usize {
        let tasks = self.running_tasks.read();
        tasks
            .values()
            .filter(|t| !t.task_handle.is_finished())
            .count()
    }

    /// Compute next_run_at for a schedule based on its trigger and current time,
    /// taking into account last_run_at for interval schedules.
    pub fn compute_next_run_for_schedule(schedule: &ImSchedule, now_ms: u64) -> Option<u64> {
        match &schedule.trigger {
            ScheduleTrigger::Interval { every_ms } => {
                if *every_ms == 0 {
                    return None;
                }
                // Use last_run_at if available, otherwise compute from now
                let base = schedule.last_run_at.unwrap_or(now_ms);
                let next = base + every_ms;
                // If already past due, schedule for now + interval
                if next <= now_ms {
                    Some(now_ms + every_ms)
                } else {
                    Some(next)
                }
            }
            ScheduleTrigger::Cron { .. } => Self::compute_next_run(&schedule.trigger, now_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interval_next_run() {
        let trigger = ScheduleTrigger::Interval { every_ms: 5000 };
        let next = ImScheduler::compute_next_run(&trigger, 100_000);
        assert_eq!(next, Some(105_000));
    }

    #[test]
    fn test_interval_zero_returns_none() {
        let trigger = ScheduleTrigger::Interval { every_ms: 0 };
        let next = ImScheduler::compute_next_run(&trigger, 100_000);
        assert_eq!(next, None);
    }

    #[test]
    fn test_cron_every_5_minutes() {
        let trigger = ScheduleTrigger::Cron {
            expr: "*/5 * * * *".to_string(),
            timezone: "UTC".to_string(),
        };
        let now = 1_710_000_000_000u64; // some timestamp
        let next = ImScheduler::compute_next_run(&trigger, now);
        assert!(next.is_some());
        let next_val = next.unwrap();
        assert!(next_val > now);
        // Should be within 5 minutes
        assert!(next_val - now <= 5 * 60 * 1000);
    }

    #[test]
    fn test_cron_empty_returns_none() {
        let trigger = ScheduleTrigger::Cron {
            expr: "".to_string(),
            timezone: "UTC".to_string(),
        };
        let next = ImScheduler::compute_next_run(&trigger, 100_000);
        assert_eq!(next, None);
    }

    #[test]
    fn test_get_due_schedules() {
        let scheduler = ImScheduler::new();
        let now = 1_710_000_100_000u64;

        let schedules = vec![
            ImSchedule {
                id: "s1".to_string(),
                name: "due".to_string(),
                enabled: true,
                message_channel: Some(ImMessageChannelBinding {
                    provider_id: "p1".to_string(),
                    target_id: "owner".to_string(),
                    target_mode: MessageTargetMode::Owner,
                }),
                task_type: ScheduleTaskType::Script,
                trigger: ScheduleTrigger::Interval { every_ms: 5000 },
                script: TaskScript {
                    script_text: Some("echo ok".to_string()),
                    script_file: None,
                    cwd: None,
                    env: BTreeMap::new(),
                },
                agent: None,
                timeout_ms: 30000,
                max_output_bytes: 4096,
                concurrency_policy: ConcurrencyPolicy::SkipIfRunning,
                retry: RetryPolicy::default(),
                next_run_at: Some(now - 1000), // past due
                last_run_at: None,
                created_at: 0,
                updated_at: 0,
            },
            ImSchedule {
                id: "s2".to_string(),
                name: "not due".to_string(),
                enabled: true,
                message_channel: Some(ImMessageChannelBinding {
                    provider_id: "p1".to_string(),
                    target_id: "owner".to_string(),
                    target_mode: MessageTargetMode::Owner,
                }),
                task_type: ScheduleTaskType::Script,
                trigger: ScheduleTrigger::Interval { every_ms: 5000 },
                script: TaskScript {
                    script_text: Some("echo ok".to_string()),
                    script_file: None,
                    cwd: None,
                    env: BTreeMap::new(),
                },
                agent: None,
                timeout_ms: 30000,
                max_output_bytes: 4096,
                concurrency_policy: ConcurrencyPolicy::SkipIfRunning,
                retry: RetryPolicy::default(),
                next_run_at: Some(now + 5000), // future
                last_run_at: None,
                created_at: 0,
                updated_at: 0,
            },
            ImSchedule {
                id: "s3".to_string(),
                name: "disabled".to_string(),
                enabled: false,
                message_channel: Some(ImMessageChannelBinding {
                    provider_id: "p1".to_string(),
                    target_id: "owner".to_string(),
                    target_mode: MessageTargetMode::Owner,
                }),
                task_type: ScheduleTaskType::Script,
                trigger: ScheduleTrigger::Interval { every_ms: 5000 },
                script: TaskScript {
                    script_text: Some("echo ok".to_string()),
                    script_file: None,
                    cwd: None,
                    env: BTreeMap::new(),
                },
                agent: None,
                timeout_ms: 30000,
                max_output_bytes: 4096,
                concurrency_policy: ConcurrencyPolicy::SkipIfRunning,
                retry: RetryPolicy::default(),
                next_run_at: Some(now - 1000), // past due but disabled
                last_run_at: None,
                created_at: 0,
                updated_at: 0,
            },
        ];

        let due = scheduler.get_due_schedules(&schedules, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "s1");
    }

    #[test]
    fn test_is_running_false_when_empty() {
        let scheduler = ImScheduler::new();
        assert!(!scheduler.is_running("nonexistent"));
    }

    use std::collections::BTreeMap;
}
