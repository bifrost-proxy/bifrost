use super::*;

#[test]
fn app_server_retryable_errors_remain_non_terminal() {
    let retrying = progress_event_from_app_server_frame(&serde_json::json!({
        "method": "error",
        "params": {
            "error": {"message": "Reconnecting... 2/5"},
            "willRetry": true
        }
    }))
    .unwrap();
    assert_eq!(retrying.event_type, ExternalCliProgressEventType::Status);
    assert_eq!(retrying.title.as_deref(), Some("Codex reconnecting"));

    let terminal = progress_event_from_app_server_frame(&serde_json::json!({
        "method": "error",
        "params": {"error": {"message": "request failed"}, "willRetry": false}
    }))
    .unwrap();
    assert_eq!(terminal.event_type, ExternalCliProgressEventType::RunFailed);
    assert_eq!(terminal.content, "request failed");
}
