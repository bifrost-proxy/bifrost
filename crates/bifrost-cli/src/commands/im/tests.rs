use super::schedule::{parse_schedule_add_args, parse_schedule_update_args};
use super::*;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn mock_server_host_port(server: &MockServer) -> (String, u16) {
    let url = server.uri();
    let rest = url.strip_prefix("http://").expect("wiremock uses http URL");
    let (host, port) = rest.split_once(':').expect("host:port");
    (
        host.to_string(),
        port.parse::<u16>().expect("mock server port"),
    )
}

fn chat_config_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "runners": {
            "Codex": {
                "adapter": "codex",
                "enabled": true
            },
            "Traex": {
                "adapter": "traex",
                "enabled": true
            },
            "Claude-Code": {
                "adapter": "claude_code",
                "enabled": true
            }
        }
    }))
}

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
fn parse_provider_add_args_uses_feishu_setup_without_credentials() {
    let args = parse_provider_add_args("feishu-main", &["--type".into(), "feishu".into()])
        .expect("parse provider add args");

    assert!(args.should_use_feishu_setup());

    let body = build_setup_provider_body(&args, "feishu", "traex");
    assert_eq!(body["id"], "feishu-main");
    assert_eq!(body["provider_type"], "feishu");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["event_connection_enabled"], true);
    assert_eq!(body["event_types"][0], "message.receive");
    assert_eq!(body["agent_config"]["runner"], "traex");
}

#[test]
fn parse_provider_add_args_allows_feishu_setup_runner_override() {
    let args = parse_provider_add_args(
        "feishu-main",
        &[
            "--type".into(),
            "feishu".into(),
            "--display-name".into(),
            "Main Feishu".into(),
            "--runner".into(),
            "codex".into(),
        ],
    )
    .expect("parse provider add args");

    assert!(args.should_use_feishu_setup());

    let body = build_setup_provider_body(&args, "feishu", "codex");
    assert_eq!(body["display_name"], "Main Feishu");
    assert_eq!(body["agent_config"]["runner"], "codex");
}

#[test]
fn parse_provider_add_args_with_credentials_uses_direct_create_body() {
    let args = parse_provider_add_args(
        "feishu-main",
        &[
            "--type".into(),
            "feishu".into(),
            "--app-id".into(),
            "cli_xxx".into(),
            "--secret".into(),
            "secret".into(),
            "--runner".into(),
            "claude-code".into(),
        ],
    )
    .expect("parse provider add args");

    assert!(!args.should_use_feishu_setup());
    let body = args.into_create_body();
    assert_eq!(body["id"], "feishu-main");
    assert_eq!(body["provider_type"], "feishu");
    assert_eq!(body["app_id"], "cli_xxx");
    assert_eq!(body["app_secret"], "secret");
    assert_eq!(body["agent_config"]["runner"], "claude-code");
}

#[test]
fn parse_provider_add_args_uses_weixin_setup_without_credentials() {
    let args = parse_provider_add_args("weixin-main", &["--type".into(), "weixin".into()])
        .expect("parse provider add args");

    assert!(args.should_use_weixin_setup());
    assert!(args.should_require_runner());

    let body = build_setup_provider_body(&args, "weixin", "Claude Code");
    assert_eq!(body["id"], "weixin-main");
    assert_eq!(body["provider_type"], "weixin");
    assert_eq!(body["agent_config"]["runner"], "Claude Code");
}

#[tokio::test]
async fn im_provider_add_feishu_setup_uses_admin_api_flow_and_runner() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);

    Mock::given(method("GET"))
        .and(path("/_bifrost/api/im-gateway/chat/config"))
        .respond_with(chat_config_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-setup/start",
        ))
        .and(body_partial_json(serde_json::json!({
            "brand": "feishu"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "session_id": "setup-session",
            "verification_url": "https://open.feishu.cn/setup/mock",
            "interval_seconds": 0,
            "expires_at": chrono::Utc::now().timestamp_millis() + 60_000
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-setup/setup-session/status",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "confirmed",
            "app_id": "cli_mock_app"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-setup/setup-session/provider",
        ))
        .and(body_partial_json(serde_json::json!({
            "id": "feishu-main",
            "provider_type": "feishu",
            "display_name": "Main Feishu",
            "agent_config": {
                "runner": "Traex"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "provider": {
                "id": "feishu-main"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-main/connect",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    handle_im_command(
        &host,
        port,
        &[
            "provider".into(),
            "add".into(),
            "feishu-main".into(),
            "--type".into(),
            "feishu".into(),
            "--display-name".into(),
            "Main Feishu".into(),
            "--runner".into(),
            "trae".into(),
        ],
    )
    .expect("feishu setup flow should complete");
}

#[tokio::test]
async fn im_provider_add_weixin_setup_uses_admin_api_flow_and_runner() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);

    Mock::given(method("GET"))
        .and(path("/_bifrost/api/im-gateway/chat/config"))
        .respond_with(chat_config_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/providers"))
        .and(body_partial_json(serde_json::json!({
            "id": "weixin-main",
            "provider_type": "weixin",
            "agent_config": {
                "runner": "Claude-Code"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "weixin-main"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/weixin-main/weixin-login/start",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "scan_url": "https://ilinkai.weixin.qq.com/qrcode/mock",
            "interval_seconds": 0,
            "expires_in_seconds": 60
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/weixin-main/weixin-login/status",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "confirmed"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/weixin-main/connect",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    handle_im_command(
        &host,
        port,
        &[
            "provider".into(),
            "add".into(),
            "weixin-main".into(),
            "--type".into(),
            "weixin".into(),
            "--runner".into(),
            "claude-code".into(),
        ],
    )
    .expect("weixin setup flow should complete");
}

#[test]
fn resolve_runner_choice_accepts_aliases_and_enabled_runners() {
    let runners = vec![
        RunnerChoice {
            id: "Claude-Code".into(),
            adapter: "claude_code".into(),
            enabled: true,
        },
        RunnerChoice {
            id: "codex".into(),
            adapter: "codex".into(),
            enabled: true,
        },
        RunnerChoice {
            id: "traex".into(),
            adapter: "traex".into(),
            enabled: true,
        },
    ];

    assert_eq!(
        resolve_runner_choice(Some("claude-code"), &runners).unwrap(),
        "Claude-Code"
    );
    assert_eq!(
        resolve_runner_choice(Some("claude code"), &runners).unwrap(),
        "Claude-Code"
    );
    assert_eq!(
        resolve_runner_choice(Some("trae"), &runners).unwrap(),
        "traex"
    );
    assert_eq!(
        resolve_runner_choice(Some("codex"), &runners).unwrap(),
        "codex"
    );
}

#[test]
fn resolve_runner_choice_rejects_missing_runner_with_available_list() {
    let runners = vec![RunnerChoice {
        id: "traex".into(),
        adapter: "traex".into(),
        enabled: true,
    }];

    let error =
        resolve_runner_choice(Some("missing"), &runners).expect_err("missing runner should fail");

    assert!(error.to_string().contains("Available runners: traex"));
    assert!(error.to_string().contains("codex, traex, Claude Code"));
}

#[test]
fn resolve_runner_choice_requires_runner_when_stdin_is_not_interactive() {
    let runners = vec![RunnerChoice {
        id: "traex".into(),
        adapter: "traex".into(),
        enabled: true,
    }];

    let error = resolve_runner_choice(None, &runners)
        .expect_err("non-interactive provider setup must require --runner");

    assert!(error.to_string().contains("--runner is required"));
    assert!(error.to_string().contains("Available runners: traex"));
    assert!(error.to_string().contains("codex, traex, Claude Code"));
}

#[test]
fn terminal_qr_code_renders_with_square_terminal_ratio() {
    let image = render_terminal_qr_code("https://open.feishu.cn/page/launcher?user_code=TEST")
        .expect("qr renders");
    let lines: Vec<_> = image
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let height = lines.len();
    let width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .expect("qr has width");

    assert!(height > 0);
    assert!(width > 0);
    let estimated_visual_height = height * 2;
    let visual_ratio = width as f64 / estimated_visual_height as f64;
    assert!(
        (0.75..=1.35).contains(&visual_ratio),
        "terminal QR should be close to square: width={width}, height={height}, visual_ratio={visual_ratio:.2}"
    );
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

#[test]
fn ensure_provider_value_sets_when_missing() {
    let mut body = json!({});
    ensure_provider_value(&mut body, "feishu-main");
    assert_eq!(body["provider_id"], "feishu-main");
}

#[test]
fn parse_provider_add_args_parses_common_flags() {
    let args = parse_provider_add_args(
        "feishu-main",
        &[
            "--type".into(),
            "feishu".into(),
            "--app-id".into(),
            "cli_xxx".into(),
            "--secret".into(),
            "plain-secret".into(),
            "--display-name".into(),
            "Main".into(),
            "--enabled".into(),
            "false".into(),
            "--owner-open-id".into(),
            "ou_xxx".into(),
            "--enable-long-connection".into(),
            "true".into(),
        ],
    )
    .expect("parse provider add args");
    let body = args.into_create_body();

    assert_eq!(body["id"], "feishu-main");
    assert_eq!(body["provider_type"], "feishu");
    assert_eq!(body["app_id"], "cli_xxx");
    assert_eq!(body["app_secret"], "plain-secret");
    assert_eq!(body["display_name"], "Main");
    assert_eq!(body["enabled"], false); // parsed from explicit flag
    assert_eq!(body["owner_open_id"], "ou_xxx");
    assert_eq!(body["event_connection_enabled"], true);
}

#[test]
fn parse_provider_add_args_rejects_base_url() {
    let err = parse_provider_add_args(
        "feishu-main",
        &[
            "--type".into(),
            "feishu".into(),
            "--base-url".into(),
            "https://open.feishu.cn".into(),
        ],
    )
    .expect_err("base_url should be rejected");

    match err {
        bifrost_core::BifrostError::Config(msg) => {
            assert!(msg.contains("base_url is managed by system and cannot be set via CLI"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn parse_provider_update_args_parses_flags() {
    let body = parse_provider_update_args(&[
        "--display-name".into(),
        "New Name".into(),
        "--enable-long-connection".into(),
        "true".into(),
        "--enabled".into(),
        "false".into(),
    ])
    .expect("parse provider update args");

    assert_eq!(body["display_name"], "New Name");
    assert_eq!(body["event_connection_enabled"], true);
    assert_eq!(body["enabled"], false);
}

#[test]
fn parse_provider_update_args_rejects_base_url() {
    let err = parse_provider_update_args(&["--base-url".into(), "https://example.com".into()])
        .expect_err("base_url should be rejected");

    match err {
        bifrost_core::BifrostError::Config(msg) => {
            assert!(msg.contains("base_url is managed by system and cannot be set via CLI"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn parse_target_add_args_parses_flags() {
    let body = parse_target_add_args(
        "oncall",
        &[
            "--provider".into(),
            "feishu-main".into(),
            "--receive-id-type".into(),
            "chat_id".into(),
            "--receive-id".into(),
            "oc_xxx".into(),
            "--display-name".into(),
            "Oncall".into(),
            "--msg-type".into(),
            "text".into(),
        ],
    )
    .expect("parse target add args");

    assert_eq!(body["id"], "oncall");
    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["receive_id_type"], "chat_id");
    assert_eq!(body["receive_id"], "oc_xxx");
    assert_eq!(body["display_name"], "Oncall");
    assert_eq!(body["default_msg_type"], "text");
}

#[test]
fn parse_target_update_args_parses_flags() {
    let body = parse_target_update_args(&[
        "--receive-id".into(),
        "oc_new".into(),
        "--display-name".into(),
        "New".into(),
        "--enabled".into(),
        "false".into(),
    ])
    .expect("parse target update args");

    assert_eq!(body["receive_id"], "oc_new");
    assert_eq!(body["display_name"], "New");
    assert_eq!(body["enabled"], false);
}

#[test]
fn parse_route_add_args_builds_matcher_and_action() {
    let body = parse_route_add_args(
        "deploy",
        &[
            "--provider".into(),
            "feishu-main".into(),
            "--event".into(),
            "message.receive".into(),
            "--chat-id".into(),
            "oc_xxx".into(),
            "--user-id".into(),
            "ou_xxx".into(),
            "--keyword".into(),
            "deploy".into(),
            "--regex".into(),
            "^/deploy".into(),
            "--script-file".into(),
            "./deploy.sh".into(),
            "--reply".into(),
            "thread".into(),
            "--timeout-ms".into(),
            "5000".into(),
        ],
    )
    .expect("parse route add args");

    assert_eq!(body["name"], "deploy");
    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["event_type"], "message.receive");
    assert_eq!(body["matcher"]["chat_ids"][0], "oc_xxx");
    assert_eq!(body["matcher"]["user_ids"][0], "ou_xxx");
    assert_eq!(body["matcher"]["keyword"], "deploy");
    assert_eq!(body["matcher"]["regex"], "^/deploy");
    assert_eq!(body["action"]["script_file"], "./deploy.sh");
    assert_eq!(body["action"]["type"], "script");
    assert_eq!(body["action"]["reply_mode"], "thread");
    assert_eq!(body["timeout_ms"], 5000);
}

#[test]
fn summarize_matcher_returns_star_for_empty_and_describes_fields() {
    let empty = summarize_matcher(&json!({}));
    assert_eq!(empty, "*");

    let matcher = json!({
        "regex": "^/deploy",
        "keyword": "deploy",
        "chat_ids": ["oc_xxx", "oc_yyy"],
    });
    let summary = summarize_matcher(&matcher);
    assert!(summary.contains("regex:^/deploy"));
    assert!(summary.contains("kw:deploy"));
    assert!(summary.contains("chats:2"));
}

#[test]
fn api_url_builds_expected_prefix() {
    let url = api_url("127.0.0.1", 9900, "/providers");
    assert_eq!(
        url,
        "http://127.0.0.1:9900/_bifrost/api/im-gateway/providers"
    );
}

#[test]
fn guess_image_mime_type_recognizes_common_extensions() {
    assert_eq!(guess_image_mime_type("photo.png"), Some("image/png"));
    assert_eq!(guess_image_mime_type("photo.jpg"), Some("image/jpeg"));
    assert_eq!(guess_image_mime_type("photo.jpeg"), Some("image/jpeg"));
    assert_eq!(guess_image_mime_type("anim.gif"), Some("image/gif"));
    assert_eq!(guess_image_mime_type("img.webp"), Some("image/webp"));
    assert_eq!(guess_image_mime_type("file.txt"), None);
}

#[test]
fn format_timestamp_handles_seconds_and_milliseconds() {
    // 2020-01-01 00:00:00 UTC
    let secs = 1_577_836_800; // seconds
    let ms = secs * 1000; // milliseconds

    let from_secs = format_timestamp(secs);
    let from_ms = format_timestamp(ms);

    assert_eq!(from_secs, from_ms);
    assert!(from_secs.starts_with("2020-01-01"));
}

#[test]
fn handle_im_command_help_and_empty_args_do_not_error() {
    handle_im_command("127.0.0.1", 9900, &[]).expect("empty args should show help");
    handle_im_command("127.0.0.1", 9900, &["help".into()])
        .expect("help subcommand should print help");
}

#[test]
fn build_image_payload_reads_file_and_sets_mime_and_base64() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("image.png");
    std::fs::write(&path, [1u8, 2, 3]).unwrap();

    let payload = build_image_payload(Some(path.to_str().unwrap()), None, Some("avatar"))
        .expect("build image payload");

    assert_eq!(payload["image_type"], "avatar");
    assert_eq!(payload["file_name"], "image.png");
    assert_eq!(payload["mime_type"], "image/png");
    // 0x01 0x02 0x03 -> AQID in base64
    assert_eq!(payload["data_base64"], "AQID");
}

#[test]
fn build_image_payload_requires_file_or_key() {
    let err = build_image_payload(None, None, None).unwrap_err();
    match err {
        bifrost_core::BifrostError::Config(msg) => {
            assert!(msg.contains("image file or image key is required"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn resolve_secret_allows_plain_and_env_and_file_sources() {
    // plain value
    assert_eq!(resolve_secret("plain").unwrap(), "plain".to_string());

    // env:KEY
    std::env::set_var("BIFROST_TEST_SECRET", "from-env");
    assert_eq!(
        resolve_secret("env:BIFROST_TEST_SECRET").unwrap(),
        "from-env".to_string()
    );
    std::env::remove_var("BIFROST_TEST_SECRET");

    // file:path
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("secret.txt");
    std::fs::write(&path, "from-file\n").unwrap();
    let value = resolve_secret(&format!("file:{}", path.display())).unwrap();
    assert_eq!(value, "from-file".to_string());
}

#[test]
fn print_route_list_and_history_helpers_handle_empty_inputs() {
    let empty = json!([]);
    print_route_list(&empty);
    print_events(&empty);
    print_task_runs(&empty);
    print_message_logs(&empty);
}

#[test]
fn format_events_and_runs_render_basic_fields() {
    let events = json!([
        {
            "event_id": "abcdef123456",
            "provider_id": "feishu-main",
            "event_type": "message.receive",
            "source": {"chat_id": "oc_xxx"},
            "received_at": 1_577_836_800_000i64
        }
    ]);
    print_events(&events);

    let runs = json!([
        {
            "run_id": "run123",
            "trigger_source": "manual",
            "status": "success",
            "duration_ms": 1500u64,
            "exit_code": 0,
            "started_at": 1_577_836_800_000i64
        }
    ]);
    print_task_runs(&runs);
}

#[test]
fn print_message_logs_formats_inbound_and_outbound() {
    let messages = json!([
        {
            "id": "1",
            "direction": "inbound",
            "status": "success",
            "sender_open_id": "sender-1234567890",
            "content_preview": "hello from remote",
            "timestamp": 1_577_836_800_000i64
        },
        {
            "id": "2",
            "direction": "outbound",
            "status": "failed",
            "target_name": "oncall",
            "content_preview": "deployment failed",
            "timestamp": 1_577_836_800_000i64
        }
    ]);

    print_message_logs(&messages);
}

#[test]
fn handle_im_command_unknown_subcommand_falls_back_to_help() {
    handle_im_command("127.0.0.1", 9900, &["unknown".into()])
        .expect("unknown subcommand should not error");
}
