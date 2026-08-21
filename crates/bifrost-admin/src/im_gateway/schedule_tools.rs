use super::scheduler::ImScheduler;
use super::types::{ImSchedule, ScheduleTaskType};

pub fn normalize_schedule(schedule: &mut ImSchedule) -> Result<(), String> {
    let now = now_ms();
    schedule.name = schedule.name.trim().to_string();
    schedule.idempotency_key = schedule
        .idempotency_key
        .take()
        .map(|value| value.trim().to_string());
    if schedule.id.trim().is_empty() {
        schedule.id = uuid::Uuid::new_v4().as_simple().to_string();
    }
    schedule.infer_task_type();
    normalize_message_channel(schedule)?;
    schedule.validate_for_save()?;
    ImScheduler::compute_next_run_result(&schedule.trigger, now)?;
    if schedule.created_at == 0 {
        schedule.created_at = now;
    }
    schedule.updated_at = now;
    schedule.next_run_at = if schedule.enabled {
        ImScheduler::compute_next_run_for_schedule(schedule, now)
    } else {
        None
    };
    Ok(())
}

pub fn validate_schedule_destination(
    schedule: &ImSchedule,
    provider_owner_ready: impl Fn(&str) -> Option<bool>,
    configured_target: impl Fn(&str) -> Option<(String, bool)>,
) -> Result<(), String> {
    let channel = schedule
        .message_channel
        .as_ref()
        .ok_or_else(|| "schedule requires message_channel".to_string())?;
    let Some(owner_ready) = provider_owner_ready(&channel.provider_id) else {
        return Err(format!("Provider '{}' not found", channel.provider_id));
    };
    if channel.target_mode == crate::im_gateway::types::MessageTargetMode::Owner
        || (channel.target_mode == crate::im_gateway::types::MessageTargetMode::ConfiguredTarget
            && matches!(channel.target_id.as_str(), "owner" | "__owner__"))
    {
        if !owner_ready {
            return Err(format!(
                "Provider '{}' does not have an owner destination",
                channel.provider_id
            ));
        }
    } else if channel.target_mode == crate::im_gateway::types::MessageTargetMode::ConfiguredTarget {
        let (provider_id, target_enabled) = configured_target(&channel.target_id)
            .ok_or_else(|| format!("Target '{}' not found", channel.target_id))?;
        if provider_id != channel.provider_id {
            return Err(format!(
                "Target '{}' belongs to provider '{}', not '{}'",
                channel.target_id, provider_id, channel.provider_id
            ));
        }
        if !target_enabled {
            return Err(format!("Target '{}' is disabled", channel.target_id));
        }
    }
    Ok(())
}

fn normalize_message_channel(schedule: &mut ImSchedule) -> Result<(), String> {
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

    if matches!(
        schedule.task_type,
        ScheduleTaskType::Agent | ScheduleTaskType::Script
    ) {
        return Err("schedule requires message_channel".to_string());
    }

    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::types::{ImMessageChannelBinding, MessageTargetMode, ScheduleTrigger};

    fn schedule() -> ImSchedule {
        serde_json::from_value(serde_json::json!({
            "name": " agent schedule ",
            "message_channel": {
                "provider_id": " provider-a ",
                "target_id": " target-a ",
                "target_mode": "configured_target"
            },
            "trigger": {"type": "cron", "expr": "0 9 * * 1-5", "timezone": "Asia/Shanghai"},
            "task_type": "agent",
            "agent": {"prompt": "summarize"},
            "idempotency_key": " request-1 "
        }))
        .unwrap()
    }

    #[test]
    fn normalize_schedule_trims_identity_and_computes_next_run() {
        let mut schedule = schedule();
        normalize_schedule(&mut schedule).unwrap();
        assert_eq!(schedule.name, "agent schedule");
        assert_eq!(schedule.idempotency_key.as_deref(), Some("request-1"));
        assert_eq!(
            schedule.message_channel.as_ref().unwrap().provider_id,
            "provider-a"
        );
        assert!(schedule.next_run_at.is_some());
    }

    #[test]
    fn validate_destination_rejects_cross_provider_target() {
        let mut schedule = schedule();
        normalize_schedule(&mut schedule).unwrap();
        let error = validate_schedule_destination(
            &schedule,
            |provider| (provider == "provider-a").then_some(true),
            |target| (target == "target-a").then(|| ("provider-b".to_string(), true)),
        )
        .unwrap_err();
        assert!(error.contains("belongs to provider 'provider-b'"));

        schedule.message_channel = Some(ImMessageChannelBinding {
            provider_id: "provider-a".to_string(),
            target_id: "owner".to_string(),
            target_mode: MessageTargetMode::Owner,
        });
        schedule.trigger = ScheduleTrigger::Interval { every_ms: 10 };
        validate_schedule_destination(&schedule, |_| Some(true), |_| None).unwrap();
    }

    #[test]
    fn validate_destination_rejects_disabled_configured_target() {
        let mut schedule = schedule();
        normalize_schedule(&mut schedule).unwrap();
        let error = validate_schedule_destination(
            &schedule,
            |_| Some(true),
            |_| Some(("provider-a".to_string(), false)),
        )
        .unwrap_err();
        assert_eq!(error, "Target 'target-a' is disabled");
    }

    #[test]
    fn normalize_disabled_schedule_and_destination_errors_are_explicit() {
        let mut schedule = schedule();
        schedule.enabled = false;
        normalize_schedule(&mut schedule).unwrap();
        assert_eq!(schedule.next_run_at, None);

        let provider_error = validate_schedule_destination(&schedule, |_| None, |_| None)
            .expect_err("unknown provider must fail closed");
        assert_eq!(provider_error, "Provider 'provider-a' not found");

        schedule.message_channel = Some(ImMessageChannelBinding {
            provider_id: "provider-a".to_string(),
            target_id: "owner".to_string(),
            target_mode: MessageTargetMode::Owner,
        });
        let owner_error = validate_schedule_destination(&schedule, |_| Some(false), |_| None)
            .expect_err("provider without owner destination must fail closed");
        assert_eq!(
            owner_error,
            "Provider 'provider-a' does not have an owner destination"
        );
    }
}
