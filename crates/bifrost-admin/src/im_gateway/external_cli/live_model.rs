use super::*;

const MODEL_STARTUP_WAIT_MS: u64 = 10_000;
const MODEL_ACK_TIMEOUT_SECS: u64 = 8;

#[derive(Clone)]
pub(super) struct ActiveModelHandle {
    pub(super) run_id: String,
    pub(super) thread_id: Option<String>,
    pub(super) model_tx: mpsc::UnboundedSender<LiveModelCommand>,
}

pub(super) struct LiveModelCommand {
    pub(super) update_id: String,
    pub(super) model: Option<String>,
    pub(super) ack_tx: oneshot::Sender<ExternalCliModelUpdateResult>,
}

static ACTIVE_MODEL_SESSIONS: once_cell::sync::Lazy<dashmap::DashMap<String, ActiveModelHandle>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

pub(super) async fn request_session_model_update(
    session_key: &str,
    update_id: String,
    model: Option<String>,
) -> ExternalCliModelUpdateResult {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(MODEL_STARTUP_WAIT_MS);
    let handle = loop {
        if let Some(handle) = ACTIVE_MODEL_SESSIONS
            .get(session_key)
            .map(|entry| entry.clone())
        {
            if live_guide::active_session_is_owned_by(session_key, &handle.run_id) {
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
        return rejected_model_update(
            update_id,
            model,
            None,
            "active runner does not expose a live model channel".to_string(),
        );
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    if handle
        .model_tx
        .send(LiveModelCommand {
            update_id: update_id.clone(),
            model: model.clone(),
            ack_tx,
        })
        .is_err()
    {
        return rejected_model_update(
            update_id,
            model,
            handle.thread_id,
            "live model channel is closed".to_string(),
        );
    }
    match timeout(Duration::from_secs(MODEL_ACK_TIMEOUT_SECS), ack_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => rejected_model_update(
            update_id,
            model,
            handle.thread_id,
            "live model response channel is closed".to_string(),
        ),
        Err(_) => rejected_model_update(
            update_id,
            model,
            handle.thread_id,
            "live model acknowledgement timed out".to_string(),
        ),
    }
}

pub(super) fn register_session(session_key: &str, run_id: &str, handle: ActiveModelHandle) -> bool {
    if !live_guide::active_session_is_owned_by(session_key, run_id) {
        return false;
    }
    ACTIVE_MODEL_SESSIONS.insert(session_key.to_string(), handle);
    let still_owns_session = live_guide::active_session_is_owned_by(session_key, run_id);
    if !still_owns_session {
        remove_session(session_key, run_id);
    }
    still_owns_session
}

pub(super) fn remove_session(session_key: &str, run_id: &str) {
    ACTIVE_MODEL_SESSIONS.remove_if(session_key, |_, handle| handle.run_id == run_id);
}

pub(super) fn rejected_model_update(
    update_id: String,
    model: Option<String>,
    thread_id: Option<String>,
    reason: String,
) -> ExternalCliModelUpdateResult {
    ExternalCliModelUpdateResult {
        update_id,
        model,
        accepted: false,
        thread_id,
        reason: Some(reason),
    }
}

pub(super) fn accepted_model_update(
    update_id: String,
    model: Option<String>,
    thread_id: Option<String>,
) -> ExternalCliModelUpdateResult {
    ExternalCliModelUpdateResult {
        update_id,
        model,
        accepted: true,
        thread_id,
        reason: None,
    }
}

#[cfg(test)]
pub(super) fn active_handle(session_key: &str) -> Option<ActiveModelHandle> {
    ACTIVE_MODEL_SESSIONS
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
        model_tx: mpsc::UnboundedSender<LiveModelCommand>,
    ) -> ActiveModelHandle {
        ActiveModelHandle {
            run_id: run_id.to_string(),
            thread_id: Some("thread-model".to_string()),
            model_tx,
        }
    }

    #[tokio::test]
    async fn live_model_update_round_trips_and_rejects_closed_channel() {
        let session_key = format!("live-model-{}", uuid::Uuid::new_v4());
        let run_id = format!("live-model-run-{}", uuid::Uuid::new_v4());
        ACTIVE_SESSIONS.insert(session_key.clone(), run_id.clone());

        let (model_tx, mut model_rx) = mpsc::unbounded_channel();
        assert!(register_session(
            &session_key,
            &run_id,
            ActiveModelHandle {
                run_id: run_id.clone(),
                thread_id: Some("thread-model".to_string()),
                model_tx,
            },
        ));
        assert!(active_handle(&session_key).is_some());
        let response = tokio::spawn(async move {
            let command = model_rx.recv().await.expect("model command");
            let _ = command.ack_tx.send(accepted_model_update(
                command.update_id,
                command.model,
                Some("thread-model".to_string()),
            ));
        });
        let accepted = request_session_model_update(
            &session_key,
            "update-1".to_string(),
            Some("gpt-5.3-codex".to_string()),
        )
        .await;
        response.await.unwrap();
        assert!(accepted.accepted);
        assert_eq!(accepted.model.as_deref(), Some("gpt-5.3-codex"));

        let (model_tx, model_rx) = mpsc::unbounded_channel();
        drop(model_rx);
        assert!(register_session(
            &session_key,
            &run_id,
            ActiveModelHandle {
                run_id: run_id.clone(),
                thread_id: Some("thread-model".to_string()),
                model_tx,
            },
        ));
        let rejected =
            request_session_model_update(&session_key, "update-2".to_string(), None).await;
        assert!(!rejected.accepted);
        assert_eq!(
            rejected.reason.as_deref(),
            Some("live model channel is closed")
        );

        remove_session(&session_key, &run_id);
        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[tokio::test]
    async fn live_model_update_rejects_closed_response_missing_and_stale_sessions() {
        let session_key = unique("live-model-response");
        let run_id = unique("live-model-run");
        ACTIVE_SESSIONS.insert(session_key.clone(), run_id.clone());
        let (model_tx, mut model_rx) = mpsc::unbounded_channel();
        assert!(register_session(
            &session_key,
            &run_id,
            handle(&run_id, model_tx)
        ));
        let drop_response = tokio::spawn(async move {
            drop(model_rx.recv().await.expect("model command"));
        });
        let response_closed = request_session_model_update(
            &session_key,
            unique("update"),
            Some("gpt-test".to_string()),
        )
        .await;
        drop_response.await.unwrap();
        assert_eq!(
            response_closed.reason.as_deref(),
            Some("live model response channel is closed")
        );
        remove_session(&session_key, &run_id);
        ACTIVE_SESSIONS.remove(&session_key);

        let missing =
            request_session_model_update(&unique("missing-model"), unique("update"), None).await;
        assert_eq!(
            missing.reason.as_deref(),
            Some("active runner does not expose a live model channel")
        );

        let stale_session = unique("stale-model");
        let stale_run = unique("stale-run");
        let replacement_run = unique("replacement-run");
        let (model_tx, _model_rx) = mpsc::unbounded_channel();
        ACTIVE_SESSIONS.insert(stale_session.clone(), stale_run.clone());
        assert!(register_session(
            &stale_session,
            &stale_run,
            handle(&stale_run, model_tx)
        ));
        ACTIVE_SESSIONS.insert(stale_session.clone(), replacement_run);
        let stale = request_session_model_update(&stale_session, unique("update"), None).await;
        assert_eq!(
            stale.reason.as_deref(),
            Some("active runner does not expose a live model channel")
        );
        assert!(active_handle(&stale_session).is_none());
        ACTIVE_SESSIONS.remove(&stale_session);
    }

    #[tokio::test]
    async fn live_model_update_times_out_unacknowledged_command() {
        let session_key = unique("timeout-model");
        let run_id = unique("timeout-run");
        ACTIVE_SESSIONS.insert(session_key.clone(), run_id.clone());
        let (model_tx, mut model_rx) = mpsc::unbounded_channel();
        assert!(register_session(
            &session_key,
            &run_id,
            handle(&run_id, model_tx)
        ));
        let hold_ack = tokio::spawn(async move {
            let command = model_rx.recv().await.expect("model command");
            sleep(Duration::from_secs(MODEL_ACK_TIMEOUT_SECS + 1)).await;
            drop(command);
        });
        let timed_out = request_session_model_update(&session_key, unique("update"), None).await;
        assert_eq!(
            timed_out.reason.as_deref(),
            Some("live model acknowledgement timed out")
        );
        hold_ack.abort();
        remove_session(&session_key, &run_id);
        ACTIVE_SESSIONS.remove(&session_key);
    }

    #[test]
    fn live_model_registration_requires_active_run_ownership() {
        let session_key = unique("unowned-model");
        let run_id = unique("unowned-run");
        let (model_tx, _model_rx) = mpsc::unbounded_channel();
        assert!(!register_session(
            &session_key,
            &run_id,
            handle(&run_id, model_tx)
        ));
        assert!(active_handle(&session_key).is_none());
    }
}
