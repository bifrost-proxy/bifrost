use super::*;

/// Verify that sending an existing absolute directory path directly switches
/// the session work_dir without invoking the model. This supports the IM/WebUI
/// direct path switch shortcut while avoiding command parsing for `/tmp/...`.
#[tokio::test]
async fn absolute_directory_input_switches_without_model_call() {
    let first_work_dir = tempfile::tempdir().expect("first work dir");
    let second_work_dir = tempfile::tempdir().expect("second work dir");
    let second_work_dir_string = second_work_dir.path().to_string_lossy().to_string();
    let mut session = AgentSession::new_with_work_dir(
        "no-auto-switch",
        Some(first_work_dir.path().to_string_lossy().to_string()),
    );
    let config = provider_config_for_responses(vec![chat_text_response(
        "That path exists, but I won't switch unless you ask me to.",
        1,
    )])
    .await;

    let result = run_turn(
        &AgentClient::new(),
        &config,
        &mut session,
        &ToolRegistry::with_defaults(),
        &second_work_dir_string,
        None,
    )
    .await
    .expect("run turn");

    assert_eq!(
        result.work_dir_switched.as_deref(),
        Some(second_work_dir_string.as_str())
    );
    assert!(
        result.response.contains("已切换工作目录到:"),
        "{}",
        result.response
    );
    assert!(
        result.tool_calls_log.is_empty(),
        "direct path switch must not call the model/tools"
    );
    assert_eq!(
        session.work_dir.as_deref(),
        Some(second_work_dir_string).as_deref()
    );
}

#[tokio::test]
async fn non_directory_path_like_input_does_not_switch() {
    let first_work_dir = tempfile::tempdir().expect("first work dir");
    let second_work_dir = tempfile::tempdir().expect("second work dir");
    let second_work_dir_string = second_work_dir.path().to_string_lossy().to_string();
    let mut session = AgentSession::new_with_work_dir(
        "no-switch-for-path-like-input",
        Some(first_work_dir.path().to_string_lossy().to_string()),
    );
    let config = provider_config_for_responses(vec![chat_text_response(
        "That looks like a path-like request, but it is not a direct switch.",
        1,
    )])
    .await;

    let result = run_turn(
        &AgentClient::new(),
        &config,
        &mut session,
        &ToolRegistry::with_defaults(),
        &format!("{second_work_dir_string} --with-extra-args"),
        None,
    )
    .await
    .expect("run turn");

    assert!(result.work_dir_switched.is_none());
    assert_eq!(
        session.work_dir.as_deref(),
        Some(first_work_dir.path().to_string_lossy().to_string()).as_deref()
    );
}
