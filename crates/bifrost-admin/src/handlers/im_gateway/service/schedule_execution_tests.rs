use super::*;

fn scheduled_script(id: &str, policy: crate::im_gateway::types::ConcurrencyPolicy) -> ImSchedule {
    ImSchedule {
        id: id.to_string(),
        name: format!("Schedule {id}"),
        enabled: true,
        idempotency_key: None,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: "missing-provider".to_string(),
            target_id: "owner".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::Owner,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Script,
        script: crate::im_gateway::types::TaskScript {
            script_text: Some("echo ok".to_string()),
            ..Default::default()
        },
        agent: None,
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: policy,
        retry: Default::default(),
        next_run_at: Some(1),
        last_run_at: None,
        last_run_status: None,
        consecutive_failures: 0,
        created_at: 1,
        updated_at: 1,
    }
}

fn register_pending_task(service: &ImGatewayService, schedule_id: &str) {
    let handle = tokio::spawn(async {
        std::future::pending::<()>().await;
    });
    service.scheduler.register_running(schedule_id, handle);
}

#[tokio::test]
async fn due_schedule_concurrency_policies_advance_without_parallel_execution() {
    for (policy, expected_runs, expected_queued) in [
        (
            crate::im_gateway::types::ConcurrencyPolicy::Forbid,
            1,
            false,
        ),
        (
            crate::im_gateway::types::ConcurrencyPolicy::SkipIfRunning,
            0,
            false,
        ),
        (
            crate::im_gateway::types::ConcurrencyPolicy::QueueOne,
            0,
            true,
        ),
    ] {
        let temp_dir = tempfile::tempdir().unwrap();
        let service = Arc::new(ImGatewayService::new(temp_dir.path()));
        let schedule = scheduled_script("concurrent", policy);
        service.schedule_store.add(schedule.clone()).unwrap();
        register_pending_task(&service, &schedule.id);

        service.process_due_schedule(schedule.clone(), 10_000);

        let persisted = service.schedule_store.get(&schedule.id).unwrap();
        if policy == crate::im_gateway::types::ConcurrencyPolicy::Forbid {
            assert!(persisted.last_run_at.unwrap_or_default() > 10_000);
            assert_eq!(
                persisted.last_run_status,
                Some(crate::im_gateway::types::TaskRunStatus::Rejected)
            );
        } else {
            assert_eq!(persisted.last_run_at, Some(10_000));
        }
        assert_eq!(persisted.next_run_at, Some(70_000));
        assert_eq!(
            service.run_store.list_by_schedule(&schedule.id).len(),
            expected_runs
        );
        assert_eq!(service.scheduler.take_queued(&schedule.id), expected_queued);
        service.scheduler.stop_all();
    }
}

#[tokio::test]
async fn due_schedule_executes_once_and_queued_tick_requires_latest_enabled_schedule() {
    let temp_dir = tempfile::tempdir().unwrap();
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let schedule = scheduled_script(
        "execute-once",
        crate::im_gateway::types::ConcurrencyPolicy::SkipIfRunning,
    );
    service.schedule_store.add(schedule.clone()).unwrap();

    service.process_due_schedule(schedule.clone(), 10_000);
    for _ in 0..100 {
        if service.scheduler.running_count() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(service.scheduler.running_count(), 0);
    let runs = service.run_store.list_by_schedule(&schedule.id);
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].status,
        crate::im_gateway::types::TaskRunStatus::Failed
    );

    service.scheduler.remove_completed(&schedule.id);
    register_pending_task(&service, &schedule.id);
    assert!(service.scheduler.queue_one(&schedule.id));
    let mut disabled = schedule.clone();
    disabled.enabled = false;
    service.schedule_store.update(disabled).unwrap();
    assert!(queued_enabled_schedule(&service, &schedule.id).is_none());
    assert!(!service.scheduler.take_queued(&schedule.id));
    service.scheduler.stop_all();
}
