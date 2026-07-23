use super::*;

#[tokio::test]
async fn abort_task_on_drop_cancels_spawned_runner_task() {
    let task = tokio::spawn(std::future::pending::<()>());
    let guard = AbortTaskOnDrop(task.abort_handle());

    drop(guard);

    let error = task.await.expect_err("task should be cancelled by guard");
    assert!(error.is_cancelled());
}

#[test]
fn event_dedup_evict_expired_handles_large_ttl() {
    let mut dedup = EventDedup::new();
    dedup.ttl = std::time::Duration::MAX;

    dedup
        .window
        .push_back(("event-1".to_string(), Instant::now()));
    dedup.evict_expired();

    assert_eq!(dedup.window.len(), 1);
}

#[tokio::test]
async fn completion_replay_removes_unconsumed_event_from_dedup_window() {
    let event = group_test_event("provider", "recovered-message", "/status", false, 1);
    let session_key =
        crate::im_gateway::group_context::build_group_session_key("provider", "oc_group");
    let mut registry = SessionMailboxRegistry::new();
    let (sender, _receiver) = mpsc::unbounded_channel();
    let generation = registry.reserve_generation();
    let pending_task = tokio::spawn(std::future::pending::<()>());
    registry.register(
        session_key.clone(),
        generation,
        sender,
        pending_task.abort_handle(),
    );
    let dispatch_result = registry.dispatch(event.clone());
    assert!(dispatch_result.delivered);
    assert!(dispatch_result.unrouted_event.is_none());

    let mut dedup = EventDedup::new();
    dedup.record("recovered-message");
    let mut recovered = VecDeque::new();
    recover_session_completion(
        &mut registry,
        &mut dedup,
        &mut recovered,
        SessionTaskCompletion {
            session_key,
            generation,
            recovered_events: VecDeque::from([event]),
        },
    );

    assert!(!dedup.contains("recovered-message"));
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered.front().unwrap().source.message_id.as_deref(),
        Some("recovered-message")
    );
}

#[test]
fn external_cli_images_from_chat_images_preserves_payloads() {
    let images = external_cli_images_from_chat_images(vec![
        bifrost_agent::ChatImageInput {
            mime_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
        },
        bifrost_agent::ChatImageInput {
            mime_type: "image/jpeg".to_string(),
            data: "dHdv".to_string(),
        },
    ]);

    assert_eq!(images.len(), 2);
    assert_eq!(images[0].mime_type, "image/png");
    assert_eq!(images[0].data, "aGVsbG8=");
    assert!(images[0].name.is_none());
    assert_eq!(images[1].mime_type, "image/jpeg");
    assert_eq!(images[1].data, "dHdv");
    assert!(images[1].name.is_none());
}
