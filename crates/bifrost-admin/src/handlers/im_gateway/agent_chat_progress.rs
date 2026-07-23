use super::*;

pub(super) async fn run_progress_event_coalescer(
    progress_registry: Arc<ImAgentProgressRegistry>,
    session_key: String,
    rx: &mut mpsc::UnboundedReceiver<bifrost_agent::AgentTurnProgressEvent>,
) {
    const STATUS_COALESCE_MS: u64 = 300;
    while let Some(first) = rx.recv().await {
        let mut immediate = progress_event_needs_immediate_flush(&first);
        let mut events = vec![first];
        while let Ok(event) = rx.try_recv() {
            immediate |= progress_event_needs_immediate_flush(&event);
            events.push(event);
        }
        if !immediate {
            let deadline = tokio::time::sleep(std::time::Duration::from_millis(STATUS_COALESCE_MS));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    maybe_event = rx.recv() => {
                        let Some(event) = maybe_event else {
                            break;
                        };
                        let mut batch_is_immediate = progress_event_needs_immediate_flush(&event);
                        events.push(event);
                        while let Ok(event) = rx.try_recv() {
                            let drained_is_immediate = progress_event_needs_immediate_flush(&event);
                            events.push(event);
                            if drained_is_immediate {
                                batch_is_immediate = true;
                                break;
                            }
                        }
                        if batch_is_immediate {
                            break;
                        }
                    }
                }
            }
        }
        progress_registry.apply_events(&session_key, events).await;
    }
}

pub(super) fn progress_event_needs_immediate_flush(
    event: &bifrost_agent::AgentTurnProgressEvent,
) -> bool {
    matches!(
        event,
        bifrost_agent::AgentTurnProgressEvent::ToolStarted { .. }
            | bifrost_agent::AgentTurnProgressEvent::ToolFinished { .. }
            | bifrost_agent::AgentTurnProgressEvent::LongTaskStatus { .. }
            | bifrost_agent::AgentTurnProgressEvent::PlanUpdated { .. }
            | bifrost_agent::AgentTurnProgressEvent::ProposedPlan { .. }
            | bifrost_agent::AgentTurnProgressEvent::TitleUpdated { .. }
            | bifrost_agent::AgentTurnProgressEvent::AssistantDelta { .. }
            | bifrost_agent::AgentTurnProgressEvent::AssistantFinal { .. }
            | bifrost_agent::AgentTurnProgressEvent::TurnFinished { .. }
            | bifrost_agent::AgentTurnProgressEvent::TurnFailed { .. }
    )
}
