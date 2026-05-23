use super::schedule::{parse_schedule_add_args, parse_schedule_update_args};
use super::*;

#[test]
fn resolve_secret_missing_env_returns_error() {
    let key = format!(
        "BIFROST_TEST_MISSING_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let error = resolve_secret(&format!("env:{key}")).expect_err("missing env must fail");
    assert!(matches!(error, ResolveSecretError::Missing(missing) if missing == key));
}

#[test]
fn resolve_secret_missing_file_returns_io_error() {
    let path = std::env::temp_dir().join(format!(
        "missing-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let error =
        resolve_secret(&format!("file:{}", path.display())).expect_err("missing file fails");
    assert!(matches!(error, ResolveSecretError::Io { .. }));
}

#[test]
fn im_send_defaults_to_owner_and_uses_content_field() {
    let args = parse_send_args(&[
        "--provider".into(),
        "feishu-main".into(),
        "--text".into(),
        "hello".into(),
    ])
    .expect("parse send args");

    let body = build_send_body("feishu-main", &args).expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["target_id"], "__owner__");
    assert_eq!(body["msg_type"], "text");
    assert_eq!(body["content"], "hello");
    assert!(body.get("text").is_none());
}

#[test]
fn im_send_keeps_explicit_target_and_card_content() {
    let args = parse_send_args(&[
        "--provider".into(),
        "feishu-main".into(),
        "--target".into(),
        "oncall".into(),
        "--card-json".into(),
        r#"{"config":{},"elements":[]}"#.into(),
    ])
    .expect("parse send args");

    let body = build_send_body("feishu-main", &args).expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["target_id"], "oncall");
    assert_eq!(body["msg_type"], "interactive");
    assert!(body["content"]["elements"].is_array());
    assert!(body.get("card").is_none());
}

#[test]
fn im_send_builds_image_key_payload() {
    let args = parse_send_args(&[
        "--provider".into(),
        "feishu-main".into(),
        "--image-key".into(),
        "img_v3_key".into(),
    ])
    .expect("parse send args");

    let body = build_send_body("feishu-main", &args).expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["target_id"], "__owner__");
    assert_eq!(body["msg_type"], "image");
    assert_eq!(body["image"]["image_key"], "img_v3_key");
    assert_eq!(body["image"]["image_type"], "message");
}

#[test]
fn im_send_builds_rich_card_payload() {
    let args = parse_send_args(&[
        "--provider".into(),
        "feishu-main".into(),
        "--card-title".into(),
        "Deploy report".into(),
        "--card-text".into(),
        "**Done**".into(),
        "--card-image-key".into(),
        "img_v3_chart".into(),
    ])
    .expect("parse send args");

    let body = build_send_body("feishu-main", &args).expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["target_id"], "__owner__");
    assert_eq!(body["msg_type"], "interactive");
    assert_eq!(body["rich_card"]["title"], "Deploy report");
    assert_eq!(body["rich_card"]["text"], "**Done**");
    assert_eq!(body["rich_card"]["image_key"], "img_v3_chart");
}

#[test]
fn enabled_provider_choices_only_returns_enabled_providers() {
    let providers = enabled_provider_choices(&json!([
        {"id":"disabled","display_name":"Disabled","enabled":false},
        {"id":"feishu-main","display_name":"Feishu Main","enabled":true}
    ]));

    assert_eq!(
        providers,
        vec![("feishu-main".to_string(), "Feishu Main".to_string())]
    );
}

#[test]
fn ensure_provider_in_body_preserves_explicit_provider() {
    let mut body = json!({"provider_id":"feishu-main"});

    ensure_provider_value(&mut body, "feishu-other");

    assert_eq!(body["provider_id"], "feishu-main");
}

#[test]
fn parse_schedule_add_args_supports_agent_prompt() {
    let body = parse_schedule_add_args(
        "daily-agent",
        &[
            "--every".into(),
            "60000".into(),
            "--agent-prompt".into(),
            "Summarize traffic".into(),
            "--agent-session-key".into(),
            "daily".into(),
            "--agent-work-dir".into(),
            "/tmp/project".into(),
        ],
    )
    .expect("parse schedule");

    assert_eq!(body["name"], "daily-agent");
    assert_eq!(body["task_type"], "agent");
    assert_eq!(body["trigger"]["type"], "interval");
    assert_eq!(body["trigger"]["every_ms"], 60000);
    assert_eq!(body["agent"]["prompt"], "Summarize traffic");
    assert_eq!(body["agent"]["session_key"], "daily");
    assert_eq!(body["agent"]["work_dir"], "/tmp/project");
}

#[test]
fn parse_schedule_add_args_supports_agent_codex_flags() {
    let body = parse_schedule_add_args(
        "daily",
        &[
            "--cron".into(),
            "0 9 * * *".into(),
            "--target".into(),
            "oncall".into(),
            "--provider".into(),
            "feishu-alerts".into(),
            "--target-mode".into(),
            "source_thread".into(),
            "--agent-prompt".into(),
            "Summarize traffic".into(),
            "--agent-runner-id".into(),
            "codex".into(),
            "--agent-model".into(),
            "gpt-5".into(),
            "--agent-profile-v2".into(),
            "team".into(),
            "--agent-reasoning-effort".into(),
            "high".into(),
            "--agent-approval-policy".into(),
            "never".into(),
            "--agent-add-dir".into(),
            "/tmp/extra".into(),
            "--agent-config".into(),
            "shell_environment_policy.inherit=all".into(),
            "--agent-enable".into(),
            "web_search".into(),
            "--agent-search".into(),
            "--agent-ephemeral".into(),
            "--agent-timeout-secs".into(),
            "180".into(),
            "--agent-danger-full-access".into(),
            "--agent-bypass-hook-trust".into(),
            "--agent-strict-config".into(),
            "--agent-skip-git-repo-check".into(),
            "--agent-ignore-user-config".into(),
            "--agent-ignore-rules".into(),
            "--agent-oss".into(),
            "--agent-local-provider".into(),
            "ollama".into(),
            "--agent-output-schema".into(),
            "/tmp/schema.json".into(),
            "--agent-color".into(),
            "never".into(),
        ],
    )
    .unwrap();

    assert_eq!(body["task_type"], "agent");
    assert_eq!(body["message_channel"]["provider_id"], "feishu-alerts");
    assert_eq!(body["message_channel"]["target_id"], "oncall");
    assert_eq!(body["message_channel"]["target_mode"], "source_thread");
    assert_eq!(body["agent"]["runner_id"], "codex");
    assert_eq!(body["agent"]["adapter_config"]["model"], "gpt-5");
    assert_eq!(body["agent"]["adapter_config"]["profileV2"], "team");
    assert_eq!(body["agent"]["adapter_config"]["reasoningEffort"], "high");
    assert_eq!(body["agent"]["adapter_config"]["approvalPolicy"], "never");
    assert_eq!(body["agent"]["adapter_config"]["addDirs"][0], "/tmp/extra");
    assert_eq!(
        body["agent"]["adapter_config"]["configOverrides"][0],
        "shell_environment_policy.inherit=all"
    );
    assert_eq!(
        body["agent"]["adapter_config"]["enableFeatures"][0],
        "web_search"
    );
    assert_eq!(body["agent"]["adapter_config"]["search"], true);
    assert_eq!(body["agent"]["adapter_config"]["ephemeral"], true);
    assert_eq!(body["agent"]["adapter_config"]["timeoutSecs"], 180);
    assert_eq!(body["agent"]["adapter_config"]["dangerFullAccess"], true);
    assert_eq!(
        body["agent"]["adapter_config"]["dangerouslyBypassHookTrust"],
        true
    );
    assert_eq!(body["agent"]["adapter_config"]["strictConfig"], true);
    assert_eq!(body["agent"]["adapter_config"]["skipGitRepoCheck"], true);
    assert_eq!(body["agent"]["adapter_config"]["ignoreUserConfig"], true);
    assert_eq!(body["agent"]["adapter_config"]["ignoreRules"], true);
    assert_eq!(body["agent"]["adapter_config"]["oss"], true);
    assert_eq!(body["agent"]["adapter_config"]["localProvider"], "ollama");
    assert_eq!(
        body["agent"]["adapter_config"]["outputSchema"],
        "/tmp/schema.json"
    );
    assert_eq!(body["agent"]["adapter_config"]["color"], "never");
}

#[test]
fn parse_schedule_add_args_supports_agent_enable_web_search_without_legacy_search() {
    let body = parse_schedule_add_args(
        "daily",
        &[
            "--agent-prompt".into(),
            "Summarize traffic".into(),
            "--agent-enable".into(),
            "web_search".into(),
        ],
    )
    .unwrap();

    assert_eq!(body["task_type"], "agent");
    assert_eq!(
        body["agent"]["adapter_config"]["enableFeatures"][0],
        "web_search"
    );
    assert!(body["agent"]["adapter_config"].get("search").is_none());
}

#[test]
fn parse_schedule_update_args_supports_agent_codex_flags() {
    let body = parse_schedule_update_args(&[
        "--target".into(),
        "__owner__".into(),
        "--provider".into(),
        "feishu-alerts".into(),
        "--target-mode".into(),
        "owner".into(),
        "--agent-prompt".into(),
        "Updated summary".into(),
        "--agent-runner-id".into(),
        "codex".into(),
        "--agent-model".into(),
        "gpt-5".into(),
        "--agent-reasoning-summary".into(),
        "auto".into(),
        "--agent-disable".into(),
        "legacy_mode".into(),
        "--agent-skip-git-repo-check".into(),
        "--agent-dangerously-bypass-hook-trust".into(),
    ])
    .unwrap();

    assert_eq!(body["task_type"], "agent");
    assert_eq!(body["message_channel"]["provider_id"], "feishu-alerts");
    assert_eq!(body["message_channel"]["target_id"], "__owner__");
    assert_eq!(body["message_channel"]["target_mode"], "owner");
    assert_eq!(body["agent"]["prompt"], "Updated summary");
    assert_eq!(body["agent"]["runner_id"], "codex");
    assert_eq!(body["agent"]["adapter_config"]["model"], "gpt-5");
    assert_eq!(body["agent"]["adapter_config"]["reasoningSummary"], "auto");
    assert_eq!(
        body["agent"]["adapter_config"]["disableFeatures"][0],
        "legacy_mode"
    );
    assert_eq!(body["agent"]["adapter_config"]["skipGitRepoCheck"], true);
    assert_eq!(
        body["agent"]["adapter_config"]["dangerouslyBypassHookTrust"],
        true
    );
}

#[test]
fn parse_schedule_update_args_supports_adapter_config_only() {
    let body = parse_schedule_update_args(&[
        "--agent-model".into(),
        "gpt-5".into(),
        "--agent-reasoning-effort".into(),
        "high".into(),
    ])
    .unwrap();

    assert_eq!(body["task_type"], "agent");
    assert_eq!(body["agent"]["adapter_config"]["model"], "gpt-5");
    assert_eq!(body["agent"]["adapter_config"]["reasoningEffort"], "high");
}

#[test]
fn parse_schedule_update_args_can_switch_to_script() {
    let body = parse_schedule_update_args(&[
        "--script".into(),
        "echo ok".into(),
        "--enabled".into(),
        "true".into(),
    ])
    .expect("parse schedule update");

    assert_eq!(body["task_type"], "script");
    assert_eq!(body["script"]["script_text"], "echo ok");
    assert_eq!(body["enabled"], true);
}

#[test]
fn choose_provider_from_reader_returns_selected_provider() {
    let providers = vec![
        ("feishu-a".to_string(), "Feishu A".to_string()),
        ("feishu-b".to_string(), "Feishu B".to_string()),
    ];
    let mut output = Vec::new();

    let selected = choose_provider_from_reader(&providers, io::Cursor::new("2\n"), &mut output)
        .expect("selection should work");

    assert_eq!(selected, "feishu-b");
    let prompt = String::from_utf8(output).expect("prompt utf8");
    assert!(prompt.contains("Select IM provider"));
    assert!(prompt.contains("Feishu B"));
}
