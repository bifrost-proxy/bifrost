use super::*;

const GUIDE_STARTUP_WAIT_MS: u64 = 10_000;
const GUIDE_ACK_TIMEOUT_SECS: u64 = 8;

#[derive(Clone)]
pub(super) struct ActiveGuideHandle {
    pub(super) run_id: String,
    pub(super) thread_id: Option<String>,
    pub(super) turn_id: Option<String>,
    pub(super) guide_tx: mpsc::UnboundedSender<LiveGuideCommand>,
}

pub(super) struct LiveGuideCommand {
    pub(super) guide_id: String,
    pub(super) message: String,
    pub(super) ack_tx: oneshot::Sender<ExternalCliGuideResult>,
}

static ACTIVE_GUIDE_SESSIONS: once_cell::sync::Lazy<dashmap::DashMap<String, ActiveGuideHandle>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

pub(super) async fn request_session_guide(
    session_key: &str,
    guide_id: String,
    message: String,
) -> ExternalCliGuideResult {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(GUIDE_STARTUP_WAIT_MS);
    let handle = loop {
        if let Some(handle) = ACTIVE_GUIDE_SESSIONS
            .get(session_key)
            .map(|entry| entry.clone())
        {
            if active_session_is_owned_by(session_key, &handle.run_id) {
                break Some(handle);
            }
            remove_session(session_key, &handle.run_id);
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
        sleep(Duration::from_millis(25)).await;
    };
    let Some(handle) = handle else {
        return rejected_guide(
            guide_id,
            None,
            None,
            "active runner does not expose a live guide channel".to_string(),
        );
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    if handle
        .guide_tx
        .send(LiveGuideCommand {
            guide_id: guide_id.clone(),
            message,
            ack_tx,
        })
        .is_err()
    {
        return rejected_guide(
            guide_id,
            handle.thread_id,
            handle.turn_id,
            "live guide channel is closed".to_string(),
        );
    }
    match timeout(Duration::from_secs(GUIDE_ACK_TIMEOUT_SECS), ack_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => rejected_guide(
            guide_id,
            handle.thread_id,
            handle.turn_id,
            "live guide response channel is closed".to_string(),
        ),
        Err(_) => rejected_guide(
            guide_id,
            handle.thread_id,
            handle.turn_id,
            "live guide acknowledgement timed out".to_string(),
        ),
    }
}

pub(super) fn register_session(session_key: &str, run_id: &str, handle: ActiveGuideHandle) -> bool {
    if !active_session_is_owned_by(session_key, run_id) {
        return false;
    }
    ACTIVE_GUIDE_SESSIONS.insert(session_key.to_string(), handle);
    let still_owns_session = active_session_is_owned_by(session_key, run_id);
    if !still_owns_session {
        remove_session(session_key, run_id);
    }
    still_owns_session
}

pub(super) fn active_session_is_owned_by(session_key: &str, run_id: &str) -> bool {
    ACTIVE_SESSIONS
        .get(session_key)
        .is_some_and(|entry| entry.value() == run_id)
}

pub(super) fn remove_session(session_key: &str, run_id: &str) {
    ACTIVE_GUIDE_SESSIONS.remove_if(session_key, |_, handle| handle.run_id == run_id);
}

pub(super) fn rejected_guide(
    guide_id: String,
    thread_id: Option<String>,
    turn_id: Option<String>,
    reason: String,
) -> ExternalCliGuideResult {
    ExternalCliGuideResult {
        guide_id,
        accepted: false,
        thread_id,
        turn_id,
        reason: Some(reason),
    }
}

pub(super) fn accepted_guide(
    guide_id: String,
    thread_id: Option<String>,
    turn_id: Option<String>,
) -> ExternalCliGuideResult {
    ExternalCliGuideResult {
        guide_id,
        accepted: true,
        thread_id,
        turn_id,
        reason: None,
    }
}

#[cfg(test)]
pub(super) fn active_handle(session_key: &str) -> Option<ActiveGuideHandle> {
    ACTIVE_GUIDE_SESSIONS
        .get(session_key)
        .map(|entry| entry.clone())
}
