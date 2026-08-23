use super::schedule::{
    handle_im_schedule, parse_schedule_add_args, parse_schedule_update_args, print_schedule_result,
    schedule_output_format,
};
use super::*;
use wiremock::matchers::{body_partial_json, method, path, query_param};
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
    let args = parse_send_args(&["feishu-main".into(), "--text".into(), "hello".into()])
        .expect("parse send args");

    let body = build_send_body(
        "feishu-main",
        &args,
        vec![json!({ "type": "text", "text": "hello" })],
    )
    .expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["destination"]["mode"], "owner");
    assert_eq!(body["parts"][0]["type"], "text");
    assert_eq!(body["parts"][0]["text"], "hello");
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

    let body = build_send_body(
        "feishu-main",
        &args,
        vec![json!({
            "type": "native_card",
            "card": {"config": {}, "elements": []}
        })],
    )
    .expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["destination"]["mode"], "target");
    assert_eq!(body["destination"]["target_id"], "oncall");
    assert_eq!(body["parts"][0]["type"], "native_card");
    assert!(body["parts"][0]["card"]["elements"].is_array());
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

    assert!(matches!(
        args.parts.as_slice(),
        [ImSendPartArg::ImageKey(key)] if key == "img_v3_key"
    ));
    let body = build_send_body(
        "feishu-main",
        &args,
        vec![json!({ "type": "image", "image_key": "img_v3_key" })],
    )
    .expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["destination"]["mode"], "owner");
    assert_eq!(body["parts"][0]["image_key"], "img_v3_key");
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

    let capabilities = json!({
        "parts": {
            "native_card": { "support": "native" },
            "image": { "support": "native" }
        }
    });
    let parts = prepare_send_parts("127.0.0.1", 9900, "feishu-main", &args, &capabilities)
        .expect("prepare rich card");
    let body = build_send_body("feishu-main", &args, parts).expect("build send body");

    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["destination"]["mode"], "owner");
    assert_eq!(body["parts"][0]["type"], "native_card");
    assert_eq!(
        body["parts"][0]["card"]["header"]["title"]["content"],
        "Deploy report"
    );
    assert_eq!(
        body["parts"][0]["card"]["elements"][1]["content"],
        "**Done**"
    );
    assert_eq!(
        body["parts"][0]["card"]["elements"][0]["img_key"],
        "img_v3_chart"
    );
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

    let error = resolve_runner_choice_with_terminal(None, &runners, false)
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
    assert_eq!(body["enabled"], true);
}

#[test]
fn parse_schedule_add_args_supports_safe_schedule_controls() {
    let body = parse_schedule_add_args(
        "daily-agent",
        &[
            "--disabled".into(),
            "--idempotency-key".into(),
            "im:event-123".into(),
            "--concurrency-policy".into(),
            "queue_one".into(),
            "--retry-max".into(),
            "2".into(),
            "--retry-delay-ms".into(),
            "1500".into(),
        ],
    )
    .expect("parse schedule controls");

    assert_eq!(body["enabled"], false);
    assert_eq!(body["idempotency_key"], "im:event-123");
    assert_eq!(body["concurrency_policy"], "queue_one");
    assert_eq!(body["retry"]["max_retries"], 2);
    assert_eq!(body["retry"]["delay_ms"], 1500);
}

#[test]
fn schedule_output_format_rejects_missing_or_invalid_values_before_request() {
    assert!(schedule_output_format(&["--format".into()])
        .unwrap_err()
        .to_string()
        .contains("requires a value"));
    assert!(schedule_output_format(&["--format".into(), "yaml".into()])
        .unwrap_err()
        .to_string()
        .contains("must be one of"));
    assert_eq!(
        schedule_output_format(&["--format".into(), "json".into()]).unwrap(),
        "json"
    );
}

#[tokio::test]
async fn schedule_cli_preview_and_add_use_expected_control_plane_routes() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    let schedule = json!({
        "id": "schedule-1",
        "name": "daily-agent",
        "idempotency_key": "stable-key"
    });
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/schedules/preview"))
        .and(body_partial_json(json!({
            "name": "daily-agent",
            "idempotency_key": "stable-key"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schedule": schedule,
            "upcoming_run_times": [1, 2, 3]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/schedules"))
        .respond_with(ResponseTemplate::new(200).set_body_json(schedule))
        .expect(1)
        .mount(&server)
        .await;

    let common = vec![
        "daily-agent".into(),
        "--every".into(),
        "60000".into(),
        "--agent-prompt".into(),
        "Summarize".into(),
        "--target".into(),
        "owner".into(),
        "--idempotency-key".into(),
        "stable-key".into(),
        "--format".into(),
        "json".into(),
    ];
    let mut preview = vec!["preview".into()];
    preview.extend(common.clone());
    handle_im_schedule(&host, port, &preview).expect("preview schedule");
    let mut add = vec!["add".into()];
    add.extend(common);
    handle_im_schedule(&host, port, &add).expect("add schedule");
}

#[test]
fn schedule_cli_rejects_format_before_connecting() {
    for subcommand in ["preview", "add"] {
        let error = handle_im_schedule(
            "127.0.0.1",
            1,
            &[
                subcommand.into(),
                "daily".into(),
                "--format".into(),
                "yaml".into(),
            ],
        )
        .unwrap_err();
        assert!(error.to_string().contains("--format must be one of"));
    }
}

#[test]
fn schedule_cli_handles_preview_usage_and_all_output_formats_without_network() {
    let missing_name = handle_im_schedule("127.0.0.1", 1, &["preview".into()])
        .expect_err("preview requires a schedule name");
    assert!(missing_name.to_string().contains("schedule name required"));

    handle_im_schedule("127.0.0.1", 1, &[]).expect("usage is informational");
    let value = json!({"id": "schedule-1"});
    print_schedule_result(&value, "json", "created").unwrap();
    print_schedule_result(&value, "json-pretty", "created").unwrap();
    print_schedule_result(&value, "human", "created").unwrap();
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
fn parse_send_args_preserves_order_and_direct_chat_destination() {
    let args = parse_send_args(&[
        "feishu-main".into(),
        "--chat-id".into(),
        "oc_group".into(),
        "--text".into(),
        "first".into(),
        "--image-key".into(),
        "img_key".into(),
        "--markdown".into(),
        "**last**".into(),
    ])
    .expect("parse ordered send parts");

    assert_eq!(args.provider.as_deref(), Some("feishu-main"));
    assert_eq!(args.chat_id.as_deref(), Some("oc_group"));
    assert!(matches!(args.parts[0], ImSendPartArg::Text(ref text) if text == "first"));
    assert!(matches!(args.parts[1], ImSendPartArg::ImageKey(ref key) if key == "img_key"));
    assert!(matches!(args.parts[2], ImSendPartArg::Markdown(ref text) if text == "**last**"));
}

#[test]
fn parse_send_args_accepts_feishu_bot_selectors_without_provider_name() {
    let args = parse_send_args(&[
        "--bot-id".into(),
        "cli_bot".into(),
        "--bot-name".into(),
        "Release Bot".into(),
        "--chat-id".into(),
        "oc_group".into(),
        "--text".into(),
        "hello".into(),
    ])
    .expect("parse bot selectors");

    assert!(args.provider.is_none());
    assert_eq!(args.bot_id.as_deref(), Some("cli_bot"));
    assert_eq!(args.bot_name.as_deref(), Some("Release Bot"));
    assert_eq!(args.chat_id.as_deref(), Some("oc_group"));
}

#[tokio::test]
async fn resolve_send_provider_id_supports_bot_name_and_rejects_invalid_response() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/providers/resolve"))
        .and(body_partial_json(json!({"bot_name": "Release Bot"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "provider_id": "feishu-release"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let by_name = parse_send_args(&[
        "--bot-name".into(),
        "Release Bot".into(),
        "--text".into(),
        "hello".into(),
    ])
    .expect("parse bot name");
    assert_eq!(
        resolve_send_provider_id(&host, port, &by_name).expect("resolve bot name"),
        "feishu-release"
    );

    let invalid = MockServer::start().await;
    let (invalid_host, invalid_port) = mock_server_host_port(&invalid);
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/providers/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"provider_id": "  "})))
        .expect(1)
        .mount(&invalid)
        .await;
    let by_id = parse_send_args(&[
        "--bot-id".into(),
        "cli_bot".into(),
        "--text".into(),
        "hello".into(),
    ])
    .expect("parse bot id");
    assert!(
        resolve_send_provider_id(&invalid_host, invalid_port, &by_id)
            .expect_err("blank provider id must fail")
            .to_string()
            .contains("missing provider_id")
    );

    let explicit = parse_send_args(&["feishu-main".into(), "--text".into(), "hello".into()])
        .expect("parse explicit provider");
    assert_eq!(
        resolve_send_provider_id("127.0.0.1", 1, &explicit).expect("explicit provider"),
        "feishu-main"
    );

    let fallback = MockServer::start().await;
    let (fallback_host, fallback_port) = mock_server_host_port(&fallback);
    Mock::given(method("GET"))
        .and(path("/_bifrost/api/im-gateway/providers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": "only-enabled",
            "display_name": "Only Enabled",
            "enabled": true
        }])))
        .expect(1)
        .mount(&fallback)
        .await;
    let implicit =
        parse_send_args(&["--text".into(), "hello".into()]).expect("parse provider-less send");
    assert_eq!(
        resolve_send_provider_id(&fallback_host, fallback_port, &implicit)
            .expect("select only enabled provider"),
        "only-enabled"
    );
}

#[test]
fn parse_send_args_rejects_unknown_conflicting_and_incomplete_options() {
    let unknown = parse_send_args(&[
        "feishu-main".into(),
        "--text".into(),
        "hello".into(),
        "--typo".into(),
    ])
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown im send option"));

    let provider_conflict = parse_send_args(&[
        "feishu-main".into(),
        "--provider".into(),
        "other".into(),
        "--text".into(),
        "hello".into(),
    ])
    .unwrap_err();
    assert!(provider_conflict.to_string().contains("mutually exclusive"));

    let bot_provider_conflict = parse_send_args(&[
        "feishu-main".into(),
        "--bot-id".into(),
        "cli_bot".into(),
        "--text".into(),
        "hello".into(),
    ])
    .unwrap_err();
    assert!(bot_provider_conflict
        .to_string()
        .contains("mutually exclusive"));

    let destination_conflict = parse_send_args(&[
        "feishu-main".into(),
        "--owner".into(),
        "--target".into(),
        "oncall".into(),
        "--text".into(),
        "hello".into(),
    ])
    .unwrap_err();
    assert!(destination_conflict
        .to_string()
        .contains("mutually exclusive"));

    let incomplete = parse_send_args(&[
        "feishu-main".into(),
        "--receive-id".into(),
        "ou_user".into(),
        "--text".into(),
        "hello".into(),
    ])
    .unwrap_err();
    assert!(incomplete.to_string().contains("must be provided together"));
}

#[test]
fn parse_send_args_help_does_not_require_provider_or_content() {
    let args = parse_send_args(&["--help".into()]).expect("help should parse offline");
    assert!(args.help);
    assert!(args.provider.is_none());
}

#[test]
fn parse_card_json_requires_object() {
    let error = parse_card_json("[]").expect_err("array card must fail");
    assert!(error.to_string().contains("card JSON must be an object"));
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

#[tokio::test]
async fn im_send_full_bundle_uploads_files_and_posts_ordered_payload() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    let temp = tempfile::tempdir().expect("temp send files");
    let markdown = temp.path().join("report.md");
    let image = temp.path().join("chart.png");
    let attachment = temp.path().join("report.bin");
    let card = temp.path().join("card.json");
    std::fs::write(&markdown, "# Report").expect("write markdown");
    std::fs::write(&image, b"PNG-DATA").expect("write image");
    std::fs::write(&attachment, b"FILE-DATA").expect("write file");
    std::fs::write(&card, r#"{"config":{},"elements":[]}"#).expect("write card");

    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/providers/resolve"))
        .and(body_partial_json(json!({"bot_id": "cli_e2e"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "provider_id": "feishu-main",
            "provider_type": "feishu",
            "display_name": "Feishu Main"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-main/capabilities",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "provider_id": "feishu-main",
            "provider_type": "feishu",
            "destinations": ["owner", "target", "direct"],
            "receive_id_types": ["chat_id", "open_id"],
            "requires_context": false,
            "parts": {
                "text": {"support": "native"},
                "markdown": {"support": "native"},
                "image": {"support": "native", "max_bytes": 1024},
                "file": {"support": "native", "max_bytes": 1024},
                "native_card": {"support": "native"}
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/messages/upload"))
        .and(query_param("kind", "image"))
        .and(query_param("provider_id", "feishu-main"))
        .and(query_param("image_type", "avatar"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "kind": "image", "key": "img_uploaded"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/messages/upload"))
        .and(query_param("kind", "file"))
        .and(query_param("provider_id", "feishu-main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "kind": "file", "key": "file_uploaded"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/messages/send"))
        .and(body_partial_json(json!({
            "provider_id": "feishu-main",
            "destination": {
                "mode": "direct",
                "receive_id_type": "open_id",
                "receive_id": "ou_owner"
            },
            "idempotency_key": "bundle-1",
            "parts": [
                {"type": "text", "text": "first"},
                {"type": "markdown", "text": "**second**"},
                {"type": "markdown", "text": "# Report"},
                {"type": "image", "image_key": "img_uploaded"},
                {"type": "image", "image_key": "img_existing"},
                {"type": "file", "file_key": "file_uploaded", "file_name": "report.bin"},
                {"type": "file", "file_key": "file_existing"},
                {"type": "native_card", "card": {"config": {}, "elements": []}},
                {"type": "native_card", "card": {"config": {}, "elements": []}}
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "bundle_id": "bundle-1",
            "provider_id": "feishu-main",
            "destination": "direct:open_id:ou_owner",
            "status": "success",
            "receipts": [{
                "index": 0,
                "requested_kind": "text",
                "delivered_kind": "text",
                "status": "success",
                "message_id": "om_1"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    handle_im_send(
        &host,
        port,
        &[
            "--bot-id".into(),
            "cli_e2e".into(),
            "--receive-id-type".into(),
            "open_id".into(),
            "--receive-id".into(),
            "ou_owner".into(),
            "--text".into(),
            "first".into(),
            "--markdown".into(),
            "**second**".into(),
            "--markdown-file".into(),
            markdown.display().to_string(),
            "--image".into(),
            image.display().to_string(),
            "--image-key".into(),
            "img_existing".into(),
            "--file".into(),
            attachment.display().to_string(),
            "--file-key".into(),
            "file_existing".into(),
            "--card-file".into(),
            card.display().to_string(),
            "--card-json".into(),
            r#"{"config":{},"elements":[]}"#.into(),
            "--image-type".into(),
            "avatar".into(),
            "--idempotency-key".into(),
            "bundle-1".into(),
            "--format".into(),
            "json".into(),
        ],
    )
    .expect("full IM send should succeed");
}

#[tokio::test]
async fn im_send_card_image_and_partial_response_cover_error_path() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    let temp = tempfile::tempdir().expect("temp card image");
    let image = temp.path().join("hero.unknown");
    std::fs::write(&image, b"IMAGE").expect("write card image");
    let capabilities = json!({
        "parts": {
            "native_card": {"support": "native"},
            "image": {"support": "native"}
        }
    });
    Mock::given(method("GET"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-main/capabilities",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(capabilities))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/messages/upload"))
        .and(query_param("kind", "image"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"key": "img_hero"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/_bifrost/api/im-gateway/messages/send"))
        .and(body_partial_json(json!({
            "destination": {"mode": "direct", "receive_id_type": "chat_id", "receive_id": "oc_group"},
            "parts": [{
                "type": "native_card",
                "card": {
                    "header": {"title": {"content": "Deploy"}},
                    "elements": [
                        {"tag": "img", "img_key": "img_hero", "alt": {"content": "hero"}},
                        {"tag": "markdown", "content": "failed"}
                    ]
                }
            }]
        })))
        .respond_with(ResponseTemplate::new(207).set_body_json(json!({
            "bundle_id": "generated",
            "provider_id": "feishu-main",
            "destination": "direct:chat_id:oc_group",
            "status": "partial_success",
            "receipts": [{
                "index": 0,
                "requested_kind": "native_card",
                "delivered_kind": "native_card",
                "status": "failed",
                "warning": "degraded",
                "error": "provider rejected card"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let error = handle_im_send(
        &host,
        port,
        &[
            "feishu-main".into(),
            "--chat-id".into(),
            "oc_group".into(),
            "--card-title".into(),
            "Deploy".into(),
            "--card-text".into(),
            "failed".into(),
            "--card-image-file".into(),
            image.display().to_string(),
            "--card-image-alt".into(),
            "hero".into(),
        ],
    )
    .expect_err("partial send should be surfaced as an error");
    assert!(error.to_string().contains("partial_success"));
}

#[tokio::test]
async fn im_provider_capabilities_command_supports_formats_and_validation() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    Mock::given(method("GET"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-main/capabilities",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "provider_id": "feishu-main",
            "provider_type": "feishu",
            "destinations": ["owner"],
            "receive_id_types": ["open_id"],
            "requires_context": false,
            "parts": {"text": {"support": "native"}}
        })))
        .expect(3)
        .mount(&server)
        .await;

    for format in ["human", "json", "json-pretty"] {
        handle_im_provider(
            &host,
            port,
            &[
                "capabilities".into(),
                "feishu-main".into(),
                "--format".into(),
                format.into(),
            ],
        )
        .expect("print provider capabilities");
    }
    assert!(handle_im_provider(&host, port, &["capabilities".into()])
        .unwrap_err()
        .to_string()
        .contains("provider name required"));
    assert!(handle_im_provider(
        &host,
        port,
        &[
            "capabilities".into(),
            "feishu-main".into(),
            "--format".into(),
            "xml".into(),
        ],
    )
    .unwrap_err()
    .to_string()
    .contains("--format"));
}

#[tokio::test]
async fn im_provider_menu_commands_use_preview_status_and_sync_endpoints() {
    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    let menu_path = |action: &str| {
        format!("/_bifrost/api/im-gateway/providers/feishu-main/feishu/menu/{action}")
    };
    for action in ["preview", "status"] {
        Mock::given(method("GET"))
            .and(path(menu_path(action)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "provider_id": "feishu-main",
                "action": action
            })))
            .expect(1)
            .mount(&server)
            .await;
        handle_im_provider(
            &host,
            port,
            &["menu".into(), "feishu-main".into(), action.into()],
        )
        .expect("menu read command");
    }

    Mock::given(method("POST"))
        .and(path(menu_path("sync")))
        .and(body_partial_json(json!({"publish": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "published": false
        })))
        .expect(1)
        .mount(&server)
        .await;
    handle_im_provider(
        &host,
        port,
        &["menu".into(), "feishu-main".into(), "sync".into()],
    )
    .expect("draft menu sync");

    Mock::given(method("POST"))
        .and(path(menu_path("sync")))
        .and(body_partial_json(json!({"publish": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "published": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    handle_im_provider(
        &host,
        port,
        &[
            "menu".into(),
            "feishu-main".into(),
            "sync".into(),
            "--publish".into(),
        ],
    )
    .expect("published menu sync");
}

#[tokio::test]
async fn im_provider_menu_rejects_invalid_arguments_and_surfaces_sync_errors() {
    assert!(handle_im_provider("127.0.0.1", 1, &["menu".into()])
        .unwrap_err()
        .to_string()
        .contains("usage"));
    assert!(
        handle_im_provider("127.0.0.1", 1, &["menu".into(), "feishu-main".into()])
            .unwrap_err()
            .to_string()
            .contains("menu action required")
    );
    assert!(handle_im_provider(
        "127.0.0.1",
        1,
        &[
            "menu".into(),
            "feishu-main".into(),
            "preview".into(),
            "--publish".into(),
        ]
    )
    .unwrap_err()
    .to_string()
    .contains("only valid"));
    assert!(handle_im_provider(
        "127.0.0.1",
        1,
        &["menu".into(), "feishu-main".into(), "delete".into(),]
    )
    .unwrap_err()
    .to_string()
    .contains("preview, status, sync"));

    let server = MockServer::start().await;
    let (host, port) = mock_server_host_port(&server);
    Mock::given(method("POST"))
        .and(path(
            "/_bifrost/api/im-gateway/providers/feishu-main/feishu/menu/sync",
        ))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "error": "unsupported_app_type",
            "message": "PersonalAgent is not supported"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let error = handle_im_provider(
        &host,
        port,
        &[
            "menu".into(),
            "feishu-main".into(),
            "sync".into(),
            "--publish".into(),
        ],
    )
    .unwrap_err();
    assert!(error.to_string().contains("HTTP 422"));
    assert!(error.to_string().contains("PersonalAgent is not supported"));
}

#[tokio::test]
async fn im_http_helpers_preserve_status_json_and_transport_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/status-json"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error": "conflict"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/upload-error"))
        .respond_with(ResponseTemplate::new(413).set_body_json(json!({"error": "too large"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/invalid-json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .expect(1)
        .mount(&server)
        .await;

    let (status, body) = http_post_with_status(
        &format!("{}/status-json", server.uri()),
        &json!({"hello": "world"}),
    )
    .expect("status response remains readable");
    assert_eq!(status, 409);
    assert_eq!(body["error"], "conflict");

    assert!(http_post_bytes(
        &format!("{}/upload-error", server.uri()),
        b"bytes",
        "application/octet-stream",
    )
    .unwrap_err()
    .to_string()
    .contains("HTTP 413: too large"));
    assert!(
        http_post_with_status(&format!("{}/invalid-json", server.uri()), &json!({}))
            .unwrap_err()
            .to_string()
            .contains("failed to parse response")
    );
    assert!(
        http_post_with_status("http://127.0.0.1:1/unreachable", &json!({}))
            .unwrap_err()
            .to_string()
            .contains("HTTP POST failed")
    );
    assert!(http_post_bytes(
        "http://127.0.0.1:1/unreachable",
        b"bytes",
        "application/octet-stream",
    )
    .unwrap_err()
    .to_string()
    .contains("IM upload failed"));
}

#[test]
fn im_send_helpers_cover_formats_capabilities_and_validation_edges() {
    let capabilities = json!({
        "provider_id": "feishu-main",
        "provider_type": "feishu",
        "destinations": ["owner", "target", "direct"],
        "receive_id_types": ["chat_id"],
        "requires_context": false,
        "parts": {
            "markdown": {"support": "degraded", "delivered_as": "text", "reason": "plain text"},
            "file": {"support": "native", "max_bytes": 32},
            "native_card": {"support": "unsupported", "reason": "not supported"}
        }
    });
    print_provider_capabilities(&capabilities, "human").expect("human capabilities");
    print_provider_capabilities(&capabilities, "json").expect("JSON capabilities");
    print_provider_capabilities(&capabilities, "json-pretty").expect("pretty capabilities");
    assert_eq!(send_capability_max_bytes(&capabilities, "file", 99), 32);
    assert_eq!(send_capability_max_bytes(&capabilities, "image", 99), 99);
    assert!(ensure_send_capability(&capabilities, "native_card")
        .unwrap_err()
        .to_string()
        .contains("not supported"));
    assert!(ensure_send_capability(&capabilities, "image")
        .unwrap_err()
        .to_string()
        .contains("does not declare"));

    for format in ["human", "json", "json-pretty"] {
        print_send_response(
            &json!({
                "bundle_id": "b1",
                "provider_id": "p1",
                "destination": "owner",
                "status": if format == "human" { "failed" } else { "success" },
                "receipts": [{
                    "index": 1,
                    "requested_kind": "markdown",
                    "delivered_kind": "text",
                    "status": "failed",
                    "warning": "plain text",
                    "error": "send failed"
                }]
            }),
            format,
        )
        .expect("print send response");
    }
    print_send_response(
        &json!({
            "bundle_id": "b2",
            "provider_id": "p1",
            "destination": "owner",
            "status": "success"
        }),
        "human",
    )
    .expect("print successful human response");
    print_im_send_help();
    handle_im_send("127.0.0.1", 1, &["--help".into()]).expect("offline send help");
    handle_im_provider("127.0.0.1", 1, &["unknown".into()])
        .expect("unknown provider subcommand prints usage");
    handle_im_target("127.0.0.1", 1, &["unknown".into()])
        .expect("unknown target subcommand prints usage");

    let duplicate_provider =
        parse_send_args(&["one".into(), "two".into(), "--text".into(), "hello".into()])
            .unwrap_err();
    assert!(duplicate_provider.to_string().contains("only one provider"));
    assert!(parse_send_args(&[
        "feishu-main".into(),
        "--format".into(),
        "xml".into(),
        "--text".into(),
        "hello".into(),
    ])
    .unwrap_err()
    .to_string()
    .contains("--format"));
    assert!(parse_send_args(&["feishu-main".into(), "--text".into()])
        .unwrap_err()
        .to_string()
        .contains("non-empty"));
    assert!(parse_send_args(&["feishu-main".into()])
        .unwrap_err()
        .to_string()
        .contains("at least one"));
    assert!(parse_card_json("not-json").is_err());

    let target = parse_send_args(&[
        "feishu-main".into(),
        "--target".into(),
        "oncall".into(),
        "--text".into(),
        "hello".into(),
    ])
    .expect("target args");
    assert_eq!(
        build_send_body(
            "feishu-main",
            &target,
            vec![json!({"type":"text","text":"hello"})]
        )
        .expect("target body")["destination"]["mode"],
        "target"
    );
}

#[test]
fn im_send_file_helpers_reject_missing_directory_empty_and_oversized_files() {
    let temp = tempfile::tempdir().expect("temp upload validation");
    let missing = temp.path().join("missing.bin");
    assert!(read_text_send_file(missing.to_str().unwrap(), "Markdown").is_err());
    assert!(upload_send_file(
        "127.0.0.1",
        1,
        "p",
        "file",
        missing.to_str().unwrap(),
        None,
        10,
    )
    .unwrap_err()
    .to_string()
    .contains("failed to inspect"));
    assert!(upload_send_file(
        "127.0.0.1",
        1,
        "p",
        "file",
        temp.path().to_str().unwrap(),
        None,
        10,
    )
    .unwrap_err()
    .to_string()
    .contains("not a regular file"));
    let empty = temp.path().join("empty.bin");
    std::fs::write(&empty, []).expect("write empty file");
    assert!(upload_send_file(
        "127.0.0.1",
        1,
        "p",
        "file",
        empty.to_str().unwrap(),
        None,
        10,
    )
    .unwrap_err()
    .to_string()
    .contains("between 1 and 10"));
    let large = temp.path().join("large.bin");
    std::fs::write(&large, b"too large").expect("write large file");
    assert!(upload_send_file(
        "127.0.0.1",
        1,
        "p",
        "file",
        large.to_str().unwrap(),
        None,
        2,
    )
    .unwrap_err()
    .to_string()
    .contains("between 1 and 2"));
}

#[test]
fn im_send_parser_and_preflight_cover_every_offline_content_form() {
    let temp = tempfile::tempdir().expect("temp send content");
    let markdown = temp.path().join("body.md");
    let card = temp.path().join("card.json");
    std::fs::write(&markdown, "# status").expect("write markdown");
    std::fs::write(&card, r#"{"config":{},"elements":[]}"#).expect("write card");

    let args = parse_send_args(&[
        "--provider".into(),
        "feishu-main".into(),
        "--receive-id-type".into(),
        "open_id".into(),
        "--receive-id".into(),
        "ou_owner".into(),
        "--text".into(),
        "plain".into(),
        "--markdown".into(),
        "**inline**".into(),
        "--markdown-file".into(),
        markdown.display().to_string(),
        "--image-key".into(),
        "img_key".into(),
        "--file-key".into(),
        "file_key".into(),
        "--card-file".into(),
        card.display().to_string(),
        "--card-json".into(),
        r#"{"elements":[]}"#.into(),
        "--image-type".into(),
        "message".into(),
        "--idempotency-key".into(),
        "stable-key".into(),
        "--format".into(),
        "json-pretty".into(),
    ])
    .expect("parse every offline content form");
    let capabilities = json!({
        "parts": {
            "text": {"support":"native"},
            "markdown": {"support":"native"},
            "image": {"support":"native"},
            "file": {"support":"native"},
            "native_card": {"support":"native"}
        }
    });
    let parts = prepare_send_parts("127.0.0.1", 1, "feishu-main", &args, &capabilities)
        .expect("prepare offline parts");
    assert_eq!(parts.len(), 7);
    assert_eq!(parts[2]["text"], "# status");
    assert_eq!(parts[3]["image_key"], "img_key");
    assert_eq!(parts[4]["file_key"], "file_key");
    let body = build_send_body("feishu-main", &args, parts).expect("build direct body");
    assert_eq!(body["destination"]["receive_id_type"], "open_id");
    assert_eq!(body["destination"]["receive_id"], "ou_owner");
    assert_eq!(body["idempotency_key"], "stable-key");

    let selectors = parse_send_args(&[
        "--bot-id".into(),
        "cli_app".into(),
        "--bot-name".into(),
        "Release bot".into(),
        "--owner".into(),
        "--text".into(),
        "hello".into(),
    ])
    .expect("bot selectors can be intersected");
    assert_eq!(selectors.bot_id.as_deref(), Some("cli_app"));
    assert_eq!(selectors.bot_name.as_deref(), Some("Release bot"));
    let debug = format!("{selectors:?}");
    assert!(debug.contains("part_count: 1"));
    assert!(!debug.contains("hello"));
}

#[test]
fn im_send_parser_rejects_each_validation_boundary() {
    for flag in [
        "--provider",
        "--bot-id",
        "--bot-name",
        "--target",
        "--chat-id",
        "--receive-id-type",
        "--receive-id",
        "--text",
        "--markdown",
        "--markdown-file",
        "--image",
        "--image-key",
        "--file",
        "--file-key",
        "--card-file",
        "--card-json",
        "--card-title",
        "--card-text",
        "--card-image-file",
        "--card-image-key",
        "--card-image-alt",
        "--image-type",
        "--idempotency-key",
        "--format",
    ] {
        let error = parse_send_args(&[flag.into()]).expect_err("missing value must fail");
        assert!(error.to_string().contains("requires a non-empty value"));
    }
    assert!(parse_send_args(&[
        "feishu-main".into(),
        "--text".into(),
        "hello".into(),
        "--format".into(),
        "yaml".into(),
    ])
    .unwrap_err()
    .to_string()
    .contains("--format must be one of"));
    assert!(parse_send_args(&[
        "--bot-name".into(),
        "Release bot".into(),
        "--chat-id".into(),
        "oc_group".into(),
        "--owner".into(),
        "--text".into(),
        "hello".into(),
    ])
    .unwrap_err()
    .to_string()
    .contains("mutually exclusive"));
}

#[test]
fn im_send_preflight_rejects_missing_and_reasonless_unsupported_capabilities() {
    let missing = ensure_send_capability(&json!({"parts": {}}), "image")
        .expect_err("missing capability must fail");
    assert!(missing.to_string().contains("does not declare"));
    let unsupported =
        ensure_send_capability(&json!({"parts":{"file":{"support":"unsupported"}}}), "file")
            .expect_err("unsupported capability must fail");
    assert!(unsupported.to_string().contains("does not support file"));

    assert_eq!(guess_image_mime_type("PIC.PNG"), Some("image/png"));
    assert_eq!(guess_image_mime_type("photo.jpeg"), Some("image/jpeg"));
    assert_eq!(guess_image_mime_type("anim.gif"), Some("image/gif"));
    assert_eq!(guess_image_mime_type("modern.webp"), Some("image/webp"));
    assert_eq!(guess_image_mime_type("unknown.bin"), None);
}

#[tokio::test]
async fn im_send_http_helpers_cover_invalid_json_and_transport_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invalid-json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        http_post_with_status(&format!("{}/invalid-json", server.uri()), &json!({}))
            .unwrap_err()
            .to_string()
            .contains("failed to parse response")
    );

    let unavailable = "http://127.0.0.1:1/unavailable";
    assert!(http_post_with_status(unavailable, &json!({})).is_err());
    assert!(http_post_bytes(unavailable, b"x", "application/octet-stream").is_err());
}

#[test]
fn target_add_requires_ids_rejects_unknown_flags_and_has_defaults() {
    let body = parse_target_add_args(
        "oncall",
        &[
            "--receive-id-type".into(),
            "chat_id".into(),
            "--receive-id".into(),
            "oc_group".into(),
        ],
    )
    .expect("minimal target");
    assert_eq!(body["display_name"], "oncall");
    assert_eq!(body["default_msg_type"], "text");
    assert_eq!(body["enabled"], true);
    let customized = parse_target_add_args(
        "oncall",
        &[
            "--receive-id-type".into(),
            "chat_id".into(),
            "--receive-id".into(),
            "oc_group".into(),
            "--msg-type".into(),
            "interactive".into(),
        ],
    )
    .expect("custom target message type");
    assert_eq!(customized["default_msg_type"], "interactive");
    assert!(parse_target_add_args("bad", &[])
        .unwrap_err()
        .to_string()
        .contains("required"));
    assert!(parse_target_add_args(
        "bad",
        &[
            "--receive-id-type".into(),
            "chat_id".into(),
            "--receive-id".into(),
            "oc_group".into(),
            "--typo".into(),
        ],
    )
    .unwrap_err()
    .to_string()
    .contains("unknown im target add option"));
}
