use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
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
    queued: bool,
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
    pub fn compute_next_run(trigger: &ScheduleTrigger, now_ms: u64) -> Option<u64> {
        Self::compute_next_run_result(trigger, now_ms)
            .ok()
            .flatten()
    }

    pub fn compute_next_run_result(
        trigger: &ScheduleTrigger,
        now_ms: u64,
    ) -> Result<Option<u64>, String> {
        match trigger {
            ScheduleTrigger::Interval { every_ms } => {
                if *every_ms == 0 {
                    return Err("interval every_ms must be greater than 0".to_string());
                }
                Ok(now_ms.checked_add(*every_ms))
            }
            ScheduleTrigger::Cron { expr, timezone } => {
                let expression = normalize_cron_expression(expr)?;
                let schedule = cron::Schedule::from_str(&expression)
                    .map_err(|error| format!("invalid cron expression '{expr}': {error}"))?;
                let timezone = Tz::from_str(timezone.trim())
                    .map_err(|_| format!("invalid IANA timezone '{timezone}'"))?;
                let now_ms = i64::try_from(now_ms)
                    .map_err(|_| format!("timestamp out of range: {now_ms}"))?;
                let now = Utc
                    .timestamp_millis_opt(now_ms)
                    .single()
                    .ok_or_else(|| format!("timestamp out of range: {now_ms}"))?
                    .with_timezone(&timezone);
                Ok(schedule
                    .after(&now)
                    .next()
                    .and_then(|next| u64::try_from(next.timestamp_millis()).ok()))
            }
        }
    }

    pub fn preview_next_runs(
        trigger: &ScheduleTrigger,
        now_ms: u64,
        count: usize,
    ) -> Result<Vec<u64>, String> {
        match trigger {
            ScheduleTrigger::Interval { every_ms } => {
                if *every_ms == 0 {
                    return Err("interval every_ms must be greater than 0".to_string());
                }
                Ok((1..=count)
                    .filter_map(|step| {
                        every_ms
                            .checked_mul(step as u64)
                            .and_then(|offset| now_ms.checked_add(offset))
                    })
                    .collect())
            }
            ScheduleTrigger::Cron { expr, timezone } => {
                let expression = normalize_cron_expression(expr)?;
                let schedule = cron::Schedule::from_str(&expression)
                    .map_err(|error| format!("invalid cron expression '{expr}': {error}"))?;
                let timezone = Tz::from_str(timezone.trim())
                    .map_err(|_| format!("invalid IANA timezone '{timezone}'"))?;
                let now = Utc
                    .timestamp_millis_opt(now_ms as i64)
                    .single()
                    .ok_or_else(|| format!("timestamp out of range: {now_ms}"))?
                    .with_timezone(&timezone);
                Ok(schedule
                    .after(&now)
                    .take(count)
                    .filter_map(|next| u64::try_from(next.timestamp_millis()).ok())
                    .collect())
            }
        }
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
                queued: false,
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

    /// Keep at most one pending tick while a schedule is already running.
    pub fn queue_one(&self, schedule_id: &str) -> bool {
        let mut tasks = self.running_tasks.write();
        let Some(task) = tasks.get_mut(schedule_id) else {
            return false;
        };
        if task.task_handle.is_finished() || task.queued {
            return false;
        }
        task.queued = true;
        true
    }

    /// Consume the single pending tick after the active execution finishes.
    pub fn take_queued(&self, schedule_id: &str) -> bool {
        let mut tasks = self.running_tasks.write();
        let Some(task) = tasks.get_mut(schedule_id) else {
            return false;
        };
        let queued = task.queued;
        task.queued = false;
        queued
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

fn normalize_cron_expression(expr: &str) -> Result<String, String> {
    let fields = expr.split_whitespace().collect::<Vec<_>>();
    match fields.len() {
        // Five-field input follows conventional Unix cron numbering where
        // Sunday is 0 or 7. The `cron` crate uses Quartz numbering where
        // Sunday is 1, so translate numeric day-of-week fields while keeping
        // names and wildcard expressions unchanged.
        5 => {
            let day_of_week = normalize_unix_day_of_week(fields[4])?;
            Ok(format!(
                "0 {} {} {} {} {}",
                fields[0], fields[1], fields[2], fields[3], day_of_week
            ))
        }
        6 | 7 => Ok(fields.join(" ")),
        _ => Err(format!(
            "invalid cron expression '{expr}': expected 5, 6, or 7 fields"
        )),
    }
}

fn normalize_unix_day_of_week(value: &str) -> Result<String, String> {
    if value == "*" || value.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return Ok(value.to_string());
    }
    value
        .split(',')
        .map(|part| {
            let (base, step) = part
                .split_once('/')
                .map(|(base, step)| (base, Some(step)))
                .unwrap_or((part, None));
            let normalized = if base == "*" {
                "*".to_string()
            } else if let Some((start, end)) = base.split_once('-') {
                let start = parse_unix_day_of_week(start)?;
                let end = parse_unix_day_of_week(end)?;
                if start <= end {
                    format!("{start}-{end}")
                } else {
                    format!("{start}-7,1-{end}")
                }
            } else {
                parse_unix_day_of_week(base)?.to_string()
            };
            Ok(match step {
                Some(step) => format!("{normalized}/{step}"),
                None => normalized,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join(","))
}

fn parse_unix_day_of_week(value: &str) -> Result<u8, String> {
    let value = value
        .parse::<u8>()
        .map_err(|_| format!("invalid Unix day-of-week value '{value}'"))?;
    match value {
        0 | 7 => Ok(1),
        1..=6 => Ok(value + 1),
        _ => Err(format!("invalid Unix day-of-week value '{value}'")),
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
    fn cron_uses_iana_timezone_for_five_field_expression() {
        let trigger = ScheduleTrigger::Cron {
            expr: "0 9 * * 1-5".to_string(),
            timezone: "Asia/Shanghai".to_string(),
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-21T00:30:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let expected = chrono::DateTime::parse_from_rfc3339("2026-08-21T01:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;

        assert_eq!(ImScheduler::compute_next_run(&trigger, now), Some(expected));
    }

    #[test]
    fn cron_rejects_invalid_expression_and_timezone() {
        let invalid_expression = ScheduleTrigger::Cron {
            expr: "not a cron".to_string(),
            timezone: "UTC".to_string(),
        };
        assert!(ImScheduler::compute_next_run_result(&invalid_expression, 100_000).is_err());

        let invalid_timezone = ScheduleTrigger::Cron {
            expr: "0 9 * * *".to_string(),
            timezone: "Mars/Olympus".to_string(),
        };
        assert!(ImScheduler::compute_next_run_result(&invalid_timezone, 100_000).is_err());
    }

    #[test]
    fn cron_rejects_timestamp_larger_than_i64() {
        let trigger = ScheduleTrigger::Cron {
            expr: "0 * * * *".to_string(),
            timezone: "UTC".to_string(),
        };
        assert_eq!(
            ImScheduler::compute_next_run_result(&trigger, u64::MAX),
            Err(format!("timestamp out of range: {}", u64::MAX))
        );
    }

    #[test]
    fn cron_preview_returns_three_ordered_runs() {
        let trigger = ScheduleTrigger::Cron {
            expr: "*/15 * * * *".to_string(),
            timezone: "UTC".to_string(),
        };
        let runs = ImScheduler::preview_next_runs(&trigger, 1_710_000_000_000, 3).unwrap();
        assert_eq!(runs.len(), 3);
        assert!(runs.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn schedule_preview_and_cron_normalization_cover_boundary_forms() {
        assert!(ImScheduler::preview_next_runs(
            &ScheduleTrigger::Interval { every_ms: 0 },
            100_000,
            3,
        )
        .is_err());

        assert_eq!(
            normalize_cron_expression("0 0 9 * * 1").unwrap(),
            "0 0 9 * * 1"
        );
        assert_eq!(
            normalize_cron_expression("0 0 9 * * 1 2026").unwrap(),
            "0 0 9 * * 1 2026"
        );
        assert_eq!(normalize_unix_day_of_week("*/2").unwrap(), "*/2");
        assert_eq!(normalize_unix_day_of_week("6-1").unwrap(), "7-7,1-2");
        assert_eq!(normalize_unix_day_of_week("1").unwrap(), "2");
        assert_eq!(normalize_unix_day_of_week("0").unwrap(), "1");
        assert!(normalize_unix_day_of_week("8").is_err());
    }

    #[tokio::test]
    async fn queue_one_only_keeps_one_pending_tick() {
        let scheduler = ImScheduler::new();
        assert!(!scheduler.queue_one("missing"));
        assert!(!scheduler.take_queued("missing"));
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        scheduler.register_running("scheduled", handle);

        assert!(scheduler.queue_one("scheduled"));
        assert!(!scheduler.queue_one("scheduled"));
        assert!(scheduler.take_queued("scheduled"));
        assert!(!scheduler.take_queued("scheduled"));
        scheduler.stop_all();
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
                idempotency_key: None,
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
                last_run_status: None,
                consecutive_failures: 0,
                created_at: 0,
                updated_at: 0,
            },
            ImSchedule {
                id: "s2".to_string(),
                name: "not due".to_string(),
                enabled: true,
                idempotency_key: None,
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
                last_run_status: None,
                consecutive_failures: 0,
                created_at: 0,
                updated_at: 0,
            },
            ImSchedule {
                id: "s3".to_string(),
                name: "disabled".to_string(),
                enabled: false,
                idempotency_key: None,
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
                last_run_status: None,
                consecutive_failures: 0,
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
