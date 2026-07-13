use super::scheduler::ImScheduler;
use super::types::{ImSchedule, ScheduleTaskType};

pub fn normalize_schedule(schedule: &mut ImSchedule) -> Result<(), String> {
    let now = now_ms();
    if schedule.id.trim().is_empty() {
        schedule.id = uuid::Uuid::new_v4().as_simple().to_string();
    }
    schedule.infer_task_type();
    normalize_message_channel(schedule)?;
    schedule.validate_for_save()?;
    if schedule.created_at == 0 {
        schedule.created_at = now;
    }
    schedule.updated_at = now;
    schedule.next_run_at = ImScheduler::compute_next_run_for_schedule(schedule, now);
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
