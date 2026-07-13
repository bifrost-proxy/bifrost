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

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(label: &str) -> String {
        format!("{label}-{}", uuid::Uuid::new_v4())
    }

    fn handle(
        run_id: &str,
        guide_tx: mpsc::UnboundedSender<LiveGuideCommand>,
    ) -> ActiveGuideHandle {
        ActiveGuideHandle {
            run_id: run_id.to_string(),
            thread_id: Some("thread-live-guide".to_string()),
            turn_id: Some("turn-live-guide".to_string()),
            guide_tx,
        }
    }

    #[tokio::test]
    async fn request_session_guide_covers_success_and_closed_channels() {
        let session_key = unique("live-guide-session");
        let run_id = unique("live-guide-run");
        ACTIVE_SESSIONS.insert(session_key.clone(), run_id.clone());

        let (guide_tx, mut guide_rx) = mpsc::unbounded_channel();
        assert!(register_session(
            &session_key,
            &run_id,
            handle(&run_id, guide_tx)
        ));
        let response_task = tokio::spawn(async move {
            let command = guide_rx.recv().await.expect("guide command");
            let _ = command.ack_tx.send(accepted_guide(
                command.guide_id,
                Some("thread-accepted".to_string()),
                None,
            ));
        });
        let accepted =
            request_session_guide(&session_key, unique("guide"), "focus tests".to_string()).await;
        response_task.await.unwrap();
        assert!(accepted.accepted);
        assert_eq!(accepted.thread_id.as_deref(), Some("thread-accepted"));

        let (guide_tx, mut guide_rx) = mpsc::unbounded_channel();
        assert!(register_session(
            &session_key,
            &run_id,
            handle(&run_id, guide_tx)
        ));
        let response_task = tokio::spawn(async move {
            drop(guide_rx.recv().await.expect("guide command"));
        });
        let response_closed =
            request_session_guide(&session_key, unique("guide"), "drop response".to_string()).await;
        response_task.await.unwrap();
        assert_eq!(
            response_closed.reason.as_deref(),
            Some("live guide response channel is closed")
        );

        let (guide_tx, guide_rx) = mpsc::unbounded_channel();
        drop(guide_rx);
        assert!(register_session(
            &session_key,
            &run_id,
            handle(&run_id, guide_tx)
        ));
        let channel_closed =
            request_session_guide(&session_key, unique("guide"), "closed channel".to_string())
                .await;
        assert_eq!(
            channel_closed.reason.as_deref(),
            Some("live guide channel is closed")
        );

        remove_session(&session_key, &run_id);
        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[tokio::test]
    async fn request_session_guide_rejects_missing_or_stale_session() {
        let missing_session = unique("missing-live-guide");
        let missing = request_session_guide(
            &missing_session,
            unique("guide"),
            "no active runner".to_string(),
        )
        .await;
        assert_eq!(
            missing.reason.as_deref(),
            Some("active runner does not expose a live guide channel")
        );

        let session_key = unique("stale-live-guide-session");
        let stale_run_id = unique("stale-live-guide-run");
        let replacement_run_id = unique("replacement-live-guide-run");
        let (guide_tx, _guide_rx) = mpsc::unbounded_channel();
        ACTIVE_SESSIONS.insert(session_key.clone(), stale_run_id.clone());
        assert!(register_session(
            &session_key,
            &stale_run_id,
            handle(&stale_run_id, guide_tx)
        ));
        ACTIVE_SESSIONS.insert(session_key.clone(), replacement_run_id);

        let stale =
            request_session_guide(&session_key, unique("guide"), "stale runner".to_string()).await;
        assert_eq!(
            stale.reason.as_deref(),
            Some("active runner does not expose a live guide channel")
        );
        assert!(active_handle(&session_key).is_none());
        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[tokio::test]
    async fn request_session_guide_times_out_unacknowledged_command() {
        let session_key = unique("timeout-live-guide-session");
        let run_id = unique("timeout-live-guide-run");
        ACTIVE_SESSIONS.insert(session_key.clone(), run_id.clone());
        let (guide_tx, mut guide_rx) = mpsc::unbounded_channel();
        assert!(register_session(
            &session_key,
            &run_id,
            handle(&run_id, guide_tx)
        ));
        let hold_ack = tokio::spawn(async move {
            let command = guide_rx.recv().await.expect("guide command");
            sleep(Duration::from_secs(GUIDE_ACK_TIMEOUT_SECS + 1)).await;
            drop(command);
        });

        let timed_out = request_session_guide(
            &session_key,
            unique("guide"),
            "never acknowledged".to_string(),
        )
        .await;
        assert_eq!(
            timed_out.reason.as_deref(),
            Some("live guide acknowledgement timed out")
        );
        hold_ack.abort();
        remove_session(&session_key, &run_id);
        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[test]
    fn registration_requires_active_run_ownership() {
        let session_key = unique("unowned-live-guide-session");
        let run_id = unique("unowned-live-guide-run");
        let (guide_tx, _guide_rx) = mpsc::unbounded_channel();
        assert!(!register_session(
            &session_key,
            &run_id,
            handle(&run_id, guide_tx)
        ));
        assert!(!active_session_is_owned_by(&session_key, &run_id));
        assert!(active_handle(&session_key).is_none());

        let rejected = rejected_guide(
            "guide-rejected".to_string(),
            None,
            None,
            "reason".to_string(),
        );
        assert!(!rejected.accepted);
        assert_eq!(rejected.reason.as_deref(), Some("reason"));
    }
}
