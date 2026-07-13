use super::*;
use crate::im_gateway::types::ImProviderType;
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::sync::{Mutex, OnceLock};

mod busy_message_mode_tests;

static IM_GATEWAY_TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) struct EnvGuard {
    _guard: crate::test_env::BifrostDataDirGuard,
}

impl EnvGuard {
    pub(super) fn set_data_dir(data_dir: &std::path::Path) -> Self {
        Self {
            _guard: crate::test_env::BifrostDataDirGuard::set(data_dir),
        }
    }
}

async fn spawn_im_gateway_http(
    service: SharedImGatewayService,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind IM Gateway test server");
    let address = listener.local_addr().expect("IM Gateway server address");
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept IM Gateway request");
        let io = TokioIo::new(stream);
        let handler = service_fn(move |request: Request<Incoming>| {
            let service = service.clone();
            async move {
                let path = request.uri().path().to_string();
                Ok::<_, std::convert::Infallible>(
                    handle_im_gateway(request, Some(service), &path).await,
                )
            }
        });
        let _ = http1::Builder::new()
            .keep_alive(false)
            .serve_connection(io, handler)
            .await;
    });
    (address, handle)
}

struct EnvVarGuard {
    key: &'static str,
    old_value: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let old_value = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, old_value }
    }

    fn remove(key: &'static str) -> Self {
        let old_value = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, old_value }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.old_value.as_deref() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

pub(super) fn fake_external_runner_sleep_command() -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "Start-Sleep -Seconds 30; [Console]::Out.WriteLine('{\"type\":\"assistant_final\",\"content\":\"too late\"}')".to_string(),
            ],
        )
    } else {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "sleep 30; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"too late\"}'"
                    .to_string(),
            ],
        )
    }
}

pub(super) fn fake_external_runner_workdir_command() -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd.exe".to_string(),
            vec![
                "/D".to_string(),
                "/C".to_string(),
                "if exist expected.marker (echo {\"type\":\"assistant_final\",\"content\":\"WORKDIR_OK\"}) else (echo {\"type\":\"assistant_final\",\"content\":\"WORKDIR_MISMATCH\"})".to_string(),
            ],
        )
    } else {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "cat >/dev/null; if [ -f ./expected.marker ]; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"WORKDIR_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"WORKDIR_MISMATCH\"}'; fi".to_string(),
            ],
        )
    }
}

pub(super) fn fake_external_runner_override_command() -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd.exe".to_string(),
            vec![
                "/D".to_string(),
                "/C".to_string(),
                "if \"%MODEL_OVERRIDE%\"==\"gpt-schedule\" (if \"%BASE_ENV%\"==\"runner\" (echo OVERRIDE_OK) else (echo OVERRIDE_MISSING)) else (echo OVERRIDE_MISSING)".to_string(),
            ],
        )
    } else {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "cat >/dev/null; if [ \"$MODEL_OVERRIDE\" = \"gpt-schedule\" ] && [ \"$BASE_ENV\" = \"runner\" ]; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"OVERRIDE_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"OVERRIDE_MISSING\"}'; fi".to_string(),
            ],
        )
    }
}

#[test]
pub(super) fn chatgpt_web_startup_auth_runners_include_all_web_runners() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let service = ImGatewayService::new(temp_dir.path());

    let mut runners = std::collections::BTreeMap::new();
    runners.insert(
        "default".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings::default(),
    );
    runners.insert(
        "web-disabled".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
            enabled: false,
            ..Default::default()
        },
    );
    runners.insert(
        "web-enabled".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
            enabled: true,
            ..Default::default()
        },
    );
    runners.insert(
        "codex".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            adapter: "codex".to_string(),
            enabled: true,
            ..Default::default()
        },
    );
    service
        .external_cli_config_store
        .save(crate::im_gateway::external_cli::ExternalCliGatewayConfig {
            version: 1,
            default_runner_id: "default".to_string(),
            runners,
            channels: std::collections::BTreeMap::new(),
        })
        .unwrap();

    let runner_ids = service
        .chatgpt_web_startup_auth_runners()
        .into_iter()
        .map(|runner| runner.runner_id)
        .collect::<Vec<_>>();

    assert_eq!(runner_ids, vec!["web-disabled", "web-enabled"]);
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn chatgpt_web_startup_auth_dry_run_reports_login_prompt() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let _dry_run_guard = EnvVarGuard::set(
        crate::im_gateway::chatgpt_web::STARTUP_AUTH_DRY_RUN_ENV,
        "1",
    );
    let settings = crate::im_gateway::external_cli::ExternalCliAgentSettings {
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        ..Default::default()
    };

    let status = crate::im_gateway::chatgpt_web::ensure_startup_auth_ready("web", &settings)
        .await
        .unwrap();

    assert_eq!(status.runner_id, "web");
    assert!(!status.logged_in);
    assert!(!status.opened_login);
    assert!(status.dry_run);
    assert!(std::path::Path::new(&status.state_path)
        .ends_with(std::path::Path::new("chatgpt_web").join("auth_state.json")));
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("would open login browser"));
}

#[test]
pub(super) fn online_notification_message_uses_provider_work_dir_override() {
    let mut provider = test_provider();
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: None,
        work_dir: Some("/custom/im-provider-workdir".to_string()),
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });

    let message = build_online_notification_message_with_device_name(&provider, "eden-macbook");

    assert!(message.starts_with("**Bifrost is online**"));
    assert!(message.contains("- **Provider**: Feishu Main (`feishu-main`)"));
    assert!(message.contains("- **Device**: eden-macbook"));
    assert!(message.contains("- **Workspace**: `/custom/im-provider-workdir`"));
    assert!(message.contains("- **Runner Type**: `external`"));
    assert!(message.contains("- **Runner ID**: `N/A`"));
    assert!(message.contains("- **Model**: `N/A`"));
    assert!(message.contains("- **Reasoning Effort**: `N/A`"));
    assert!(message.contains("- **Reasoning Summary**: `N/A`"));
    assert!(message.contains("- **Bound Session**: `N/A`"));
    assert!(message.contains("- **Completed User Turns**: 0"));
    assert!(message.contains("- **Status**: Ready"));
    assert!(message.contains("可用命令:"));
    assert!(message.contains("IM 通道命令（所有 Runner）:"));
    assert!(!message.contains("/remember <text>"));
}

#[test]
pub(super) fn online_notification_message_falls_back_to_process_work_dir() {
    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let provider = test_provider();

    let message = build_online_notification_message_with_device_name(&provider, "eden-macbook");

    assert!(message.starts_with("**Bifrost is online**"));
    assert!(message.contains("- **Device**: eden-macbook"));
    assert!(message.contains("- **Workspace**: `"));
    assert!(message.contains(&cwd));
}

#[test]
pub(super) fn online_notification_message_includes_runner_context_and_turns() {
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou_owner".to_string());
    let context = OnlineNotificationAgentContext {
        runner_type: "chatgpt_web".to_string(),
        runner_id: Some("web-main".to_string()),
        model: Some("gpt-5".to_string()),
        model_provider: Some("chatgpt_web".to_string()),
        model_reasoning_effort: Some("high".to_string()),
        model_reasoning_summary: Some("auto".to_string()),
        session_key: "feishu-main:ou_owner".to_string(),
        user_turn_count: 7,
    };

    let message =
        build_online_notification_message_with_context(&provider, "eden-macbook", Some(&context));

    assert!(message.contains("- **Runner Type**: `chatgpt_web`"));
    assert!(message.contains("- **Runner ID**: `web-main`"));
    assert!(message.contains("- **Model**: `gpt-5（chatgpt_web）`"));
    assert!(message.contains("- **Reasoning Effort**: `high`"));
    assert!(message.contains("- **Reasoning Summary**: `auto`"));
    assert!(message.contains("- **Bound Session**: `feishu-main:ou_owner`"));
    assert!(message.contains("- **Completed User Turns**: 7"));
    assert!(message.contains("IM 通道命令（所有 Runner）:"));
    assert!(!message.contains("/remember"));
    assert!(!message.contains("/goal"));
    assert!(!message.contains("/g <引导内容>"));
}

#[test]
pub(super) fn online_notification_context_resolves_external_runner_adapter_and_turns() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou_owner".to_string());
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom(
            "web-main".to_string(),
        )),
        work_dir: Some("/custom/im-provider-workdir".to_string()),
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });
    let base_config = bifrost_agent::config::AgentConfig::default();
    let external_config = crate::im_gateway::external_cli::ExternalCliGatewayConfig {
        version: 1,
        default_runner_id: "codex-default".to_string(),
        runners: std::collections::BTreeMap::from([(
            "web-main".to_string(),
            crate::im_gateway::external_cli::ExternalCliAgentSettings {
                adapter: "chatgpt_web".to_string(),
                enabled: true,
                ..Default::default()
            },
        )]),
        channels: std::collections::BTreeMap::new(),
    };
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let mut session = manager
        .try_take_session_with_work_dir(
            "feishu-main:ou_owner",
            Some("/custom/im-provider-workdir".to_string()),
        )
        .expect("session should be available");
    session
        .history
        .push(bifrost_agent::ChatMessage::user("first"));
    session
        .history
        .push(bifrost_agent::ChatMessage::assistant("answer"));
    session
        .history
        .push(bifrost_agent::ChatMessage::user("second"));
    manager.return_session(session);

    let context = build_online_notification_agent_context(
        &provider,
        &base_config,
        &external_config,
        &manager,
    );

    assert_eq!(context.runner_type, "chatgpt_web");
    assert_eq!(context.runner_id.as_deref(), Some("web-main"));
    assert_eq!(context.session_key, "feishu-main:ou_owner");
    assert_eq!(context.user_turn_count, 2);
}

#[test]
pub(super) fn online_notification_context_resolves_claude_code_settings_effort() {
    let _env_lock = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let home = tempfile::tempdir().expect("home dir");
    let claude_home = home.path().join(".claude");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    std::fs::write(
        claude_home.join("settings.json"),
        r#"{
          "model": "opus",
          "effortLevel": "low"
        }"#,
    )
    .expect("write claude settings");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let _home_guard = EnvVarGuard::set("HOME", &home.path().display().to_string());
    let _claude_config_guard = EnvVarGuard::remove("CLAUDE_CONFIG_DIR");
    let _anthropic_model_guard = EnvVarGuard::remove("ANTHROPIC_MODEL");
    let _claude_effort_guard = EnvVarGuard::remove("CLAUDE_CODE_EFFORT_LEVEL");
    let mut provider = test_provider();
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom(
            "Claude-Code".to_string(),
        )),
        work_dir: None,
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });
    let base_config = bifrost_agent::config::AgentConfig::default();
    let external_config = crate::im_gateway::external_cli::ExternalCliGatewayConfig {
        version: 1,
        default_runner_id: "Codex".to_string(),
        runners: std::collections::BTreeMap::from([(
            "Claude-Code".to_string(),
            crate::im_gateway::external_cli::ExternalCliAgentSettings {
                adapter: crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER.to_string(),
                enabled: true,
                ..Default::default()
            },
        )]),
        channels: std::collections::BTreeMap::new(),
    };
    let manager = bifrost_agent::AgentSessionManager::new(3600);

    let context = build_online_notification_agent_context(
        &provider,
        &base_config,
        &external_config,
        &manager,
    );
    let message =
        build_online_notification_message_with_context(&provider, "eden-macbook", Some(&context));

    assert_eq!(
        context.runner_type,
        crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    );
    assert_eq!(context.runner_id.as_deref(), Some("Claude-Code"));
    assert_eq!(context.model.as_deref(), Some("opus"));
    assert_eq!(context.model_provider.as_deref(), Some("claude settings"));
    assert_eq!(context.model_reasoning_effort.as_deref(), Some("low"));
    assert!(message.contains("- **Model**: `opus（claude settings）`"));
    assert!(message.contains("- **Reasoning Effort**: `low`"));
    assert!(message.contains("Claude Code Runner 命令:"));
    assert!(message.contains("/models"));
    assert!(message.contains("/efforts"));
    assert!(!message.contains("/remember"));
}

#[test]
pub(super) fn outbound_log_msg_type_marks_feishu_text_as_interactive() {
    let provider = test_provider();
    assert_eq!(outbound_log_msg_type(&provider, "text"), "interactive");
    assert_eq!(outbound_log_msg_type(&provider, "image"), "image");

    let mut weixin = test_provider();
    weixin.provider_type = ImProviderType::Weixin;
    assert_eq!(outbound_log_msg_type(&weixin, "text"), "text");
}

#[test]
pub(super) fn agent_reply_image_path_resolution_uses_work_dir_and_skips_remote_or_image_key() {
    let base = std::path::Path::new("/tmp/im-agent-workdir");

    assert_eq!(
        resolve_agent_reply_image_path("./chart.png", Some(base)).as_deref(),
        Some(std::path::Path::new("/tmp/im-agent-workdir/./chart.png"))
    );
    assert_eq!(
        resolve_agent_reply_image_path("/tmp/chart.png", Some(base)).as_deref(),
        Some(std::path::Path::new("/tmp/chart.png"))
    );
    assert_eq!(
        resolve_agent_reply_image_path("file:///tmp/chart.png", Some(base)).as_deref(),
        Some(std::path::Path::new("/tmp/chart.png"))
    );
    assert!(resolve_agent_reply_image_path("https://example.com/chart.png", Some(base)).is_none());
    assert!(resolve_agent_reply_image_path("img_v3_chart", Some(base)).is_none());
    assert!(resolve_agent_reply_image_path("./chart.png", None).is_none());
}

#[test]
pub(super) fn markdown_image_destination_strips_wrappers_and_title() {
    assert_eq!(markdown_image_destination("<./chart.png>"), "./chart.png");
    assert_eq!(
        markdown_image_destination("./chart.png \"Chart title\""),
        "./chart.png"
    );
    assert_eq!(
        markdown_image_destination("./chart.png 'Chart title'"),
        "./chart.png"
    );
}

#[test]
pub(super) fn agent_reply_target_uses_weixin_sender_instead_of_owner() {
    let mut provider = test_provider();
    provider.provider_type = ImProviderType::Weixin;
    provider.owner_open_id = Some("owner@im.wechat".to_string());
    let event = ImEvent {
        event_id: "evt-1".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Weixin,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("sender@im.wechat".to_string()),
            user_id: Some("sender@im.wechat".to_string()),
            message_id: Some("msg-1".to_string()),
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };

    let target = agent_reply_target_ref(&provider, &event).expect("reply target");
    assert_eq!(
        (target.receive_id_type.as_str(), target.receive_id.as_str()),
        ("open_id", "sender@im.wechat")
    );
}

#[test]
pub(super) fn local_image_fallback_removes_markdown_image_syntax() {
    let fallback = local_image_fallback_markdown("chart", "./missing.png");

    assert_eq!(fallback, "[chart 未能上传]");
    assert!(!fallback.contains("!["));
    assert!(!fallback.contains("./missing.png"));

    let fallback = local_image_fallback_markdown(" ", "/tmp/chart.png");
    assert_eq!(fallback, "[图片 未能上传]");
}

#[test]
pub(super) fn local_markdown_image_candidate_filters_remote_and_existing_keys() {
    assert!(is_local_markdown_image_candidate("./chart.png"));
    assert!(is_local_markdown_image_candidate("/tmp/chart.png"));
    assert!(is_local_markdown_image_candidate("file:///tmp/chart.png"));
    assert!(!is_local_markdown_image_candidate(
        "https://example.com/chart.png"
    ));
    assert!(!is_local_markdown_image_candidate(
        "http://example.com/chart.png"
    ));
    assert!(!is_local_markdown_image_candidate("img_v3_chart"));
    assert!(!is_local_markdown_image_candidate(" "));
}

#[test]
pub(super) fn agent_reply_collects_and_strips_generated_local_images() {
    let base = std::path::Path::new("/tmp/im-agent-workdir");
    let markdown = concat!(
        "生成好了\n",
        "![cat one](./cat-1.png)\n",
        "![cat two](/tmp/cat-2.jpg \"cute\")\n",
        "```md\n",
        "![skip](./inside-code.png)\n",
        "```\n",
        "![remote](https://example.com/cat.png)\n",
        "![feishu](img_v3_existing)\n",
    );

    let images = collect_agent_reply_local_images(markdown, Some(base));

    assert_eq!(images.len(), 2);
    assert_eq!(images[0].alt, "cat one");
    assert_eq!(
        images[0].path.as_path(),
        std::path::Path::new("/tmp/im-agent-workdir/./cat-1.png")
    );
    assert_eq!(images[1].alt, "cat two");
    assert_eq!(
        images[1].path.as_path(),
        std::path::Path::new("/tmp/cat-2.jpg")
    );

    let stripped = strip_agent_reply_local_images(markdown, Some(base));
    assert!(!stripped.contains("./cat-1.png"));
    assert!(!stripped.contains("/tmp/cat-2.jpg"));
    assert!(stripped.contains("![skip](./inside-code.png)"));
    assert!(stripped.contains("![remote](https://example.com/cat.png)"));
    assert!(stripped.contains("![feishu](img_v3_existing)"));
}

#[test]
pub(super) fn agent_reply_dedupes_generated_local_images() {
    let base = std::path::Path::new("/tmp/im-agent-workdir");
    let markdown = "![cat](./cat.png)\n![same cat](./cat.png)\n";

    let images = collect_agent_reply_local_images(markdown, Some(base));

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].alt, "cat");
}

#[test]
pub(super) fn agent_reply_prepare_text_and_images_splits_markdown_local_images() {
    let base = std::path::Path::new("/tmp/im-agent-workdir");
    let markdown = concat!(
        "生成好了\n",
        "![cat one](./cat-1.png)\n",
        "保留远端 favicon ![remote](https://example.com/favicon.png)\n",
    );

    let (text, images) = prepare_agent_reply_text_and_images(markdown, Some(base));

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].alt, "cat one");
    assert!(!text.contains("./cat-1.png"));
    assert!(text.contains("![remote](https://example.com/favicon.png)"));
}

#[test]
pub(super) fn agent_chat_message_text_prefers_trimmed_text_and_uses_image_prompt_fallback() {
    let text_message = crate::im_gateway::types::ImEventMessage {
        text: "  请分析这张图  ".to_string(),
        mentions: Vec::new(),
        images: vec![crate::im_gateway::types::ImImageAttachment {
            file_key: "img-v3-1".to_string(),
            source: crate::im_gateway::types::ImImageSource::UploadedImage,
            mime_type: Some("image/png".to_string()),
            data_base64: Some("AA==".to_string()),
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        }],
        reply_to: None,
        raw_type: Some("text".to_string()),
    };
    assert_eq!(agent_message_text(&text_message), "请分析这张图");

    let image_only_message = crate::im_gateway::types::ImEventMessage {
        text: " \n\t ".to_string(),
        mentions: Vec::new(),
        images: vec![crate::im_gateway::types::ImImageAttachment {
            file_key: "img-v3-2".to_string(),
            source: crate::im_gateway::types::ImImageSource::MessageResource,
            mime_type: None,
            data_base64: None,
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        }],
        reply_to: None,
        raw_type: Some("image".to_string()),
    };
    assert_eq!(
        agent_message_text(&image_only_message),
        IMAGE_ONLY_AGENT_PROMPT
    );

    let empty_message = crate::im_gateway::types::ImEventMessage {
        text: "   ".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        reply_to: None,
        raw_type: None,
    };
    assert!(agent_message_text(&empty_message).is_empty());
}

#[test]
pub(super) fn agent_chat_message_text_includes_resolved_reply_context_and_preserves_commands() {
    let temp = tempfile::tempdir().expect("temp message store");
    let store = ImMessageLogStore::new(temp.path());
    store
        .add(ImMessageLog {
            id: "quoted-log".to_string(),
            provider_id: "weixin-main".to_string(),
            direction: MessageDirection::Outbound,
            status: MessageStatus::Success,
            timestamp: 1_000,
            target_id: Some("peer-a".to_string()),
            target_name: None,
            message_id: Some("quoted-message-id".to_string()),
            msg_type: Some("text".to_string()),
            content_preview: Some("原回复".to_string()),
            content: Some("原回复 https://example.com/article".to_string()),
            trigger: Some("agent".to_string()),
            error: None,
            sender_open_id: None,
            event_id: None,
            reaction_added: None,
        })
        .expect("add quoted message");
    let reply_to = Some(crate::im_gateway::types::ImMessageReference {
        message_id: Some("quoted-message-id".to_string()),
        created_at_ms: Some(1_000),
        text: None,
    });
    let message = crate::im_gateway::types::ImEventMessage {
        text: "这个链接对应哪篇文章？".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        reply_to: reply_to.clone(),
        raw_type: Some("text".to_string()),
    };

    let prompt =
        agent_message_text_with_reference(&message, "weixin-main", Some("peer-a"), None, &store);
    assert!(prompt.contains("【引用消息（仅作为上下文）】"));
    assert!(prompt.contains("原回复 https://example.com/article"));
    assert!(prompt.ends_with("【当前消息】\n这个链接对应哪篇文章？"));

    let command = crate::im_gateway::types::ImEventMessage {
        text: "/g 只处理当前回合".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        reply_to,
        raw_type: Some("text".to_string()),
    };
    assert_eq!(
        agent_message_text_with_reference(&command, "weixin-main", Some("peer-a"), None, &store),
        "/g 只处理当前回合"
    );
}

#[test]
pub(super) fn agent_chat_message_text_limits_reply_context_and_ignores_missing_reference() {
    let temp = tempfile::tempdir().expect("temp message store");
    let store = ImMessageLogStore::new(temp.path());
    let long_quote = "引".repeat(MAX_QUOTED_AGENT_CONTEXT_CHARS + 200);
    let message = crate::im_gateway::types::ImEventMessage {
        text: "继续解释".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        reply_to: Some(crate::im_gateway::types::ImMessageReference {
            message_id: None,
            created_at_ms: None,
            text: Some(long_quote),
        }),
        raw_type: Some("text".to_string()),
    };
    let prompt =
        agent_message_text_with_reference(&message, "weixin-main", Some("peer-a"), None, &store);
    let quoted = prompt
        .split("\n\n【当前消息】")
        .next()
        .expect("quoted section");
    assert!(quoted.ends_with("..."));
    assert!(quoted.chars().count() <= MAX_QUOTED_AGENT_CONTEXT_CHARS + 32);

    let missing = crate::im_gateway::types::ImEventMessage {
        text: "仍然处理当前消息".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        reply_to: Some(crate::im_gateway::types::ImMessageReference {
            message_id: Some("missing".to_string()),
            created_at_ms: None,
            text: None,
        }),
        raw_type: Some("text".to_string()),
    };
    assert_eq!(
        agent_message_text_with_reference(&missing, "weixin-main", Some("peer-a"), None, &store),
        "仍然处理当前消息"
    );
}

#[test]
pub(super) fn inbound_message_preview_summarizes_image_only_and_truncates_text() {
    let image_message = crate::im_gateway::types::ImEventMessage {
        text: String::new(),
        mentions: Vec::new(),
        images: vec![
            crate::im_gateway::types::ImImageAttachment {
                file_key: "img-v3-1".to_string(),
                source: crate::im_gateway::types::ImImageSource::UploadedImage,
                mime_type: Some("image/png".to_string()),
                data_base64: Some("AA==".to_string()),
                download_url: None,
                encrypted_query_param: None,
                aes_key: None,
            },
            crate::im_gateway::types::ImImageAttachment {
                file_key: "img-v3-2".to_string(),
                source: crate::im_gateway::types::ImImageSource::MessageResource,
                mime_type: None,
                data_base64: None,
                download_url: None,
                encrypted_query_param: None,
                aes_key: None,
            },
        ],
        reply_to: None,
        raw_type: Some("image".to_string()),
    };
    assert_eq!(inbound_message_preview(&image_message), "[图片消息: 2 张]");

    let long_text = "中".repeat(240);
    let text_message = crate::im_gateway::types::ImEventMessage {
        text: long_text,
        mentions: Vec::new(),
        images: Vec::new(),
        reply_to: None,
        raw_type: Some("text".to_string()),
    };
    let preview = inbound_message_preview(&text_message);
    assert_eq!(preview.chars().count(), 203);
    assert!(preview.ends_with("..."));
}

#[test]
pub(super) fn progress_events_flush_immediately_only_for_visible_chat_updates() {
    let status = bifrost_agent::ActiveTurnStatus {
        session_key: "s1".to_string(),
        state: "running".to_string(),
        started_at: 1,
        updated_at: 2,
        current_loop_iteration: 1,
        completed_loop_iterations: 0,
        max_loop_iterations: 1000,
        last_response_tokens: None,
        total_tokens_used: None,
        estimated_context_tokens: 0,
        context_window_tokens: None,
        context_usage_percent: None,
        compaction_count: 0,
        history_version: 0,
        work_dir: None,
        message_count: 0,
        local_tool_count: 0,
        mcp_tool_count: 0,
        pending_guide_messages: Vec::new(),
        user_turn_count: 0,
        agent_type: Some("External Runner Agent".to_string()),
        runner_type: Some("codex".to_string()),
        runner_id: Some("Codex".to_string()),
        model: None,
        model_provider: None,
        model_reasoning_effort: None,
        model_reasoning_summary: None,
        external_conversation_id: None,
        external_thread_id: None,
        turn_timing: None,
        turn_id: None,
    };
    assert!(!progress_event_needs_immediate_flush(
        &bifrost_agent::AgentTurnProgressEvent::Status(Box::new(status))
    ));

    for event in [
        bifrost_agent::AgentTurnProgressEvent::AssistantDelta {
            content: "thinking".to_string(),
        },
        bifrost_agent::AgentTurnProgressEvent::AssistantFinal {
            content: "answer".to_string(),
        },
        bifrost_agent::AgentTurnProgressEvent::TurnFinished {
            content: "done".to_string(),
        },
        bifrost_agent::AgentTurnProgressEvent::TurnFailed {
            error: "boom".to_string(),
        },
    ] {
        assert!(
            progress_event_needs_immediate_flush(&event),
            "event should refresh visible chat progress immediately: {event:?}"
        );
    }
}

#[test]
pub(super) fn agent_reply_collects_remote_attachment_images_but_skips_favicons() {
    let markdown = concat!(
        "![download](https://files.oaiusercontent.com/file-1.png)\n",
        "[![](https://www.google.com/s2/favicons?domain=https://www.reuters.com&sz=32)Reuters](https://www.reuters.com)\n",
        "```md\n",
        "![skip](https://files.oaiusercontent.com/inside-code.png)\n",
        "```\n",
    );

    let images = collect_agent_reply_remote_image_links(markdown);

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].alt, "download");
    assert_eq!(images[0].url, "https://files.oaiusercontent.com/file-1.png");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_downloads_remote_markdown_image_to_local_attachment() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind image server");
    let port = listener.local_addr().expect("image server addr").port();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let io = TokioIo::new(stream);
        let service = service_fn(move |_req: Request<Incoming>| async move {
            Ok::<_, hyper::Error>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "image/png")
                    .body(Full::new(Bytes::from_static(b"remote png bytes")))
                    .unwrap(),
            )
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });
    let markdown = format!("图片如下：\n![chart](http://127.0.0.1:{port}/chart.png)\n正文保留。");

    let (text, images, attachments) =
        prepare_agent_reply_text_and_images_with_downloads(&markdown, None).await;

    assert_eq!(images.len(), 1);
    assert!(attachments.is_empty());
    assert_eq!(images[0].alt, "chart");
    assert!(images[0].path.exists());
    assert_eq!(
        std::fs::read(&images[0].path).expect("read downloaded image"),
        b"remote png bytes"
    );
    assert!(!text.contains("http://127.0.0.1"));
    assert!(text.contains("正文保留"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_download_link_with_image_content_type_uses_image_channel() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind image download server");
    let port = listener.local_addr().expect("image download addr").port();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let io = TokioIo::new(stream);
        let service = service_fn(move |_req: Request<Incoming>| async move {
            Ok::<_, hyper::Error>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "image/png")
                    .body(Full::new(Bytes::from_static(b"download image bytes")))
                    .unwrap(),
            )
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });
    let markdown = format!("这是下载链接：[下载图片](http://127.0.0.1:{port}/download)\n正文。");

    let (text, images, attachments) =
        prepare_agent_reply_text_and_images_with_downloads(&markdown, None).await;

    assert_eq!(images.len(), 1);
    assert!(attachments.is_empty());
    assert_eq!(images[0].alt, "下载图片");
    assert_eq!(
        std::fs::read(&images[0].path).expect("read downloaded image link"),
        b"download image bytes"
    );
    assert!(!text.contains("http://127.0.0.1"));
    assert!(text.contains("正文"));
}

#[test]
pub(super) fn agent_reply_collects_remote_file_attachments_from_explicit_links() {
    let markdown = concat!(
        "[报告附件](https://files.oaiusercontent.com/report.pdf)\n",
        "[普通新闻](https://example.com/news/story)\n",
        "[图片下载](https://files.oaiusercontent.com/render.png)\n",
        "```md\n",
        "[跳过附件](https://files.oaiusercontent.com/inside.pdf)\n",
        "```\n",
    );

    let attachments = collect_agent_reply_remote_attachment_links(markdown);

    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].label, "报告附件");
    assert_eq!(attachments[1].label, "图片下载");
}

#[test]
pub(super) fn agent_reply_target_uses_feishu_chat_id_for_event_channel() {
    let mut provider = test_provider();
    provider.owner_open_id = Some("owner-ou".to_string());
    let event = ImEvent {
        event_id: "evt-1".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-1".to_string()),
            user_id: Some("sender-ou".to_string()),
            message_id: Some("msg-1".to_string()),
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };

    let target = agent_reply_target_ref(&provider, &event).expect("reply target");
    assert_eq!(
        (target.receive_id_type.as_str(), target.receive_id.as_str()),
        ("chat_id", "chat-1")
    );

    let progress_target = build_agent_reply_target(
        &provider,
        &event,
        "__agent_progress__",
        "Agent Progress",
        "interactive",
    )
    .expect("progress target");
    assert_eq!(progress_target.receive_id_type, "chat_id");
    assert_eq!(progress_target.receive_id, "chat-1");

    let plan_target = build_agent_reply_target(
        &provider,
        &event,
        "__plan_card__",
        "Plan Card",
        "interactive",
    )
    .expect("plan target");
    assert_eq!(plan_target.receive_id_type, "chat_id");
    assert_eq!(plan_target.receive_id, "chat-1");
}

#[test]
pub(super) fn agent_reply_target_uses_feishu_open_id_without_chat_id() {
    let mut provider = test_provider();
    provider.owner_open_id = Some("owner-ou".to_string());
    let event = ImEvent {
        event_id: "evt-1".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: None,
            user_id: Some("sender-ou".to_string()),
            message_id: Some("msg-1".to_string()),
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };

    let target = agent_reply_target_ref(&provider, &event).expect("reply target");
    assert_eq!(
        (target.receive_id_type.as_str(), target.receive_id.as_str()),
        ("open_id", "sender-ou")
    );
}

pub(super) fn test_provider() -> ImProviderConfig {
    ImProviderConfig {
        id: "feishu-main".to_string(),
        provider_type: ImProviderType::Feishu,
        display_name: "Feishu Main".to_string(),
        enabled: true,
        base_url: None,
        app_id: Some("cli_xxx".to_string()),
        secret_ref: None,
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn debug_mock_inbound_maps_reply_reference_into_event() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.owner_open_id = Some("owner-open-id".to_string());
    service
        .provider_store
        .add(provider)
        .expect("provider should be saved");
    let (address, server) = spawn_im_gateway_http(service).await;

    let response = reqwest::Client::new()
        .post(format!(
            "http://{address}/api/im-gateway/debug/mock-inbound"
        ))
        .header("connection", "close")
        .json(&serde_json::json!({
            "providerId": "feishu-main",
            "text": "quoted follow-up",
            "userId": "sender-open-id",
            "chatId": "chat-id",
            "messageId": "message-id",
            "eventId": "event-id",
            "replyTo": {
                "messageId": "quoted-message-id",
                "createdAtMs": 1234,
                "text": "quoted original"
            }
        }))
        .send()
        .await
        .expect("send mock inbound request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("mock inbound response");
    assert_eq!(body["eventId"], "event-id");
    assert_eq!(body["messageId"], "message-id");
    server.await.expect("mock inbound server task");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn text_message_send_uses_provider_client_and_records_failure() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "weixin-main".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.display_name = "Weixin Main".to_string();
    service
        .provider_store
        .add(provider)
        .expect("provider should be saved");
    service
        .target_store
        .add(ImTarget {
            id: "weixin-owner".to_string(),
            provider_id: "weixin-main".to_string(),
            display_name: "Weixin Owner".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "weixin-user".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 1,
            updated_at: 1,
        })
        .expect("target should be saved");
    let (address, server) = spawn_im_gateway_http(service.clone()).await;

    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "weixin-main",
            "target_id": "weixin-owner",
            "msg_type": "text",
            "content": "migration smoke test"
        }))
        .send()
        .await
        .expect("send outbound message request");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    server.await.expect("message send server task");
    let logs = service.message_log_store.list();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].content.as_deref(), Some("migration smoke test"));
    assert_eq!(logs[0].status, MessageStatus::Failed);
}

#[test]
pub(super) fn feishu_codex_like_external_runner_defaults_to_progress_card_without_channel_override()
{
    let provider = test_provider();
    let settings = crate::im_gateway::external_cli::ExternalCliAgentSettings {
        adapter: crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string(),
        delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        ..Default::default()
    };
    let runner_sources =
        std::collections::BTreeMap::from([("deliveryMode".to_string(), "runner".to_string())]);

    assert_eq!(
        resolve_external_cli_delivery_mode(&provider, &settings, &runner_sources, None),
        crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
    );

    let codex_settings = crate::im_gateway::external_cli::ExternalCliAgentSettings {
        adapter: "codex".to_string(),
        delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        ..Default::default()
    };
    assert_eq!(
        resolve_external_cli_delivery_mode(&provider, &codex_settings, &runner_sources, None),
        crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
    );

    let channel_sources =
        std::collections::BTreeMap::from([("deliveryMode".to_string(), "channel".to_string())]);
    assert_eq!(
        resolve_external_cli_delivery_mode(&provider, &settings, &channel_sources, None),
        crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply
    );
    assert_eq!(
        resolve_external_cli_delivery_mode(
            &provider,
            &settings,
            &runner_sources,
            Some(crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm),
        ),
        crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm
    );

    let mut weixin_provider = test_provider();
    weixin_provider.provider_type = ImProviderType::Weixin;
    assert_eq!(
        resolve_external_cli_delivery_mode(&weixin_provider, &settings, &runner_sources, None),
        crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply
    );
}

mod schedule_agent_tests;

#[test]
pub(super) fn send_message_request_resolves_owner_target_from_provider() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou-owner".to_string());
    service
        .provider_store
        .add(provider)
        .expect("provider should be saved");

    let body = SendMessageRequest {
        provider_id: Some("feishu-main".to_string()),
        target_id: Some("__owner__".to_string()),
        msg_type: "text".to_string(),
        content: serde_json::json!("hello"),
        text: None,
        card: None,
        image: None,
        rich_card: None,
    };

    let resolved =
        resolve_send_message_request(&service, &body).expect("owner target should resolve");

    assert_eq!(resolved.provider.id, "feishu-main");
    assert_eq!(resolved.target.provider_id, "feishu-main");
    assert_eq!(resolved.target.receive_id_type, "open_id");
    assert_eq!(resolved.target.receive_id, "ou-owner");
    assert_eq!(resolved.log_target_id, "__owner__");
    assert_eq!(resolved.log_target_name, "Owner");
    assert_eq!(resolved.content, serde_json::json!("hello"));
}

#[test]
pub(super) fn send_message_request_rejects_owner_without_provider() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    let body = SendMessageRequest {
        provider_id: None,
        target_id: Some("__owner__".to_string()),
        msg_type: "text".to_string(),
        content: serde_json::json!("hello"),
        text: None,
        card: None,
        image: None,
        rich_card: None,
    };

    let error = resolve_send_message_request(&service, &body)
        .expect_err("owner send should require provider");

    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("provider_id is required"));
}

#[test]
pub(super) fn send_message_request_accepts_image_key_payload() {
    let body = SendMessageRequest {
        provider_id: Some("feishu-main".to_string()),
        target_id: Some("__owner__".to_string()),
        msg_type: "image".to_string(),
        content: serde_json::Value::Null,
        text: None,
        card: None,
        image: Some(SendImageRequest {
            image_key: Some("img_v3_key".to_string()),
            data_base64: None,
            file_name: None,
            mime_type: None,
            image_type: default_feishu_image_type(),
        }),
        rich_card: None,
    };

    let content = normalized_send_content(&body).expect("image content");
    assert_eq!(content["image_key"], "img_v3_key");
    assert_eq!(content["image_type"], "message");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn rich_card_builder_uses_image_key_and_markdown() {
    let provider = test_provider();
    let feishu = crate::im_gateway::feishu::FeishuProvider::new();
    let rich_card = SendRichCardRequest {
        title: Some("Deploy report".to_string()),
        text: Some("**Done** with chart".to_string()),
        image_key: Some("img_v3_chart".to_string()),
        image: None,
        image_alt: Some("Chart".to_string()),
    };

    let card = build_rich_card_content(&feishu, &provider, &rich_card)
        .await
        .expect("rich card");

    assert_eq!(card["header"]["title"]["content"], "Deploy report");
    assert_eq!(card["elements"][0]["tag"], "img");
    assert_eq!(card["elements"][0]["img_key"], "img_v3_chart");
    assert_eq!(card["elements"][1]["tag"], "markdown");
    assert_eq!(card["elements"][1]["content"], "**Done** with chart");
}

mod provider_agent_tests;

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_provider_external_cli_runner_bypasses_disabled_default_flag() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    base_config.runner = Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()));
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    #[cfg(windows)]
    let mock_traex = temp_dir.path().join("mock-traex.cmd");
    #[cfg(not(windows))]
    let mock_traex = temp_dir.path().join("mock-traex");
    #[cfg(unix)]
    {
        std::fs::write(
            &mock_traex,
            "#!/usr/bin/env sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"thread-effort-override\"}'\nprintf '%s\\n' '{\"type\":\"assistant_final\",\"content\":\"EXTERNAL_RUNNER_OK\"}'\nprintf '%s\\n' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\n",
        )
        .expect("write mock traex");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&mock_traex)
            .expect("mock metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&mock_traex, permissions).expect("chmod mock traex");
    }
    #[cfg(windows)]
    {
        std::fs::write(
            &mock_traex,
            "@echo off\r\nmore >nul\r\necho {\"type\":\"thread.started\",\"thread_id\":\"thread-effort-override\"}\r\necho {\"type\":\"assistant_final\",\"content\":\"EXTERNAL_RUNNER_OK\"}\r\necho {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\r\n",
        )
        .expect("write mock traex");
    }
    let runner = external_cli_config
        .runners
        .get_mut(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID)
        .expect("default codex runner");
    runner.enabled = false;
    runner.adapter = crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string();
    runner.inject_bifrost_tools = false;
    runner.adapter_config = crate::im_gateway::external_cli::ExternalCliAdapterConfig {
        executable: Some(mock_traex.display().to_string()),
        reasoning_effort: Some("xhigh".to_string()),
        ..Default::default()
    };
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external cli config");

    let mut provider = test_provider();
    provider.id = "external-runner-provider".to_string();
    provider.owner_open_id = Some("owner-open-id".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .route_store
        .add(crate::im_gateway::types::ImRoute {
            id: "agent-chat-external-runner".to_string(),
            provider_id: provider.id.clone(),
            name: "Agent Chat external runner".to_string(),
            enabled: true,
            event_type: crate::im_gateway::types::ImEventType::MessageReceive,
            matcher: crate::im_gateway::types::ImEventMatcher {
                chat_ids: Vec::new(),
                user_ids: vec!["owner-open-id".to_string()],
                keyword: None,
                regex: None,
            },
            action: crate::im_gateway::types::ImRouteAction::AgentChat {
                system_prompt: None,
                model: None,
                reply_target: crate::im_gateway::types::ReplyTarget::OriginalChat,
            },
            timeout_ms: 30_000,
            max_output_bytes: 1_048_576,
            created_at: now_ms(),
            updated_at: now_ms(),
        })
        .expect("add AgentChat route");
    let session_key = build_session_key(&provider.id, Some("owner-open-id"));
    crate::im_gateway::session_state::upsert_session_state(
        &session_key,
        crate::im_gateway::external_cli::TRAEX_ADAPTER,
        Some(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID),
        |state| {
            state.reasoning_effort_override = Some("high".to_string());
            state.reasoning_effort_override_source = Some("session slash command".to_string());
        },
    )
    .expect("persist session effort override");

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
    ));

    tx.send(ImEvent {
        event_id: "evt-external-runner".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: None,
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "run external cli".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            reply_to: None,
            raw_type: Some("text".to_string()),
        }),
        received_at: now_ms(),
        raw_digest: None,
    })
    .expect("send IM event");
    drop(tx);

    tokio::time::timeout(std::time::Duration::from_secs(60), handle)
        .await
        .expect("event loop timed out")
        .expect("event loop task panicked");

    let runs_root = crate::im_gateway::external_cli::default_runs_root();
    let mut found = false;
    let mut runtime_snapshot = None;
    for entry in std::fs::read_dir(runs_root).expect("runs dir") {
        let run_dir = entry.expect("run dir").path();
        let result_path = run_dir.join("result.json");
        if !result_path.exists() {
            continue;
        }
        let result = std::fs::read_to_string(result_path).expect("result json");
        if result.contains("EXTERNAL_RUNNER_OK") {
            runtime_snapshot = Some(
                std::fs::read_to_string(run_dir.join("runtime_snapshot.json"))
                    .expect("runtime snapshot"),
            );
            found = true;
            break;
        }
    }
    assert!(
        found,
        "external runner should execute even when defaults.enabled is false"
    );
    let runtime_snapshot: serde_json::Value =
        serde_json::from_str(&runtime_snapshot.expect("runtime snapshot for matching run"))
            .expect("runtime snapshot json");
    let args = runtime_snapshot["args"].as_array().expect("args array");
    let joined_args = args
        .iter()
        .filter_map(|arg| arg.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !joined_args.contains("xhigh"),
        "runner config effort must be overridden by session slash command: {args:?}"
    );
    assert!(
        joined_args.contains("model_reasoning_effort=\"high\""),
        "session effort override must reach external runner args: {args:?}"
    );

    let detail = service
        .agent_session_manager
        .get_session_detail(&session_key)
        .expect("external runner session detail should be visible in WebUI");
    assert_eq!(
        detail.source,
        crate::im_gateway::external_cli::TRAEX_ADAPTER
    );
    assert_eq!(detail.message_count, 2);
    assert_eq!(detail.messages[0].role, "user");
    assert_eq!(detail.messages[0].content, "run external cli");
    assert_eq!(detail.messages[1].role, "assistant");
    assert_eq!(detail.messages[1].content, "EXTERNAL_RUNNER_OK");

    let files = bifrost_agent::persistence::list_conversations(
        &bifrost_agent::config::agent_home_dir(),
        Some(&session_key),
    );
    assert_eq!(
        files.len(),
        1,
        "external runner should persist one session file"
    );
    let events = bifrost_agent::persistence::load_conversation_events(&files[0])
        .expect("load external runner session events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "session_start"
            && event
                .content
                .get("adapter")
                .and_then(|value| value.as_str())
                == Some(crate::im_gateway::external_cli::TRAEX_ADAPTER)));
    assert!(events.iter().any(|event| event.event_type == "user_message"
        && event
            .content
            .get("message")
            .and_then(|value| value.as_str())
            == Some("run external cli")));
    assert!(
        !events.iter().any(|event| event.event_type == "tool_call"
            && event
                .content
                .get("tool_name")
                .and_then(|value| value.as_str())
                == Some("mock")),
        "external runner wrapper calls should not be shown as user-visible tools"
    );
    assert!(events
        .iter()
        .any(|event| event.event_type == "assistant_message"
            && event
                .content
                .get("message")
                .and_then(|value| value.as_str())
                == Some("EXTERNAL_RUNNER_OK")));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_external_cli_route_processes_image_only_message() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    #[cfg(windows)]
    let mock_runner = temp_dir.path().join("mock-image-runner.cmd");
    #[cfg(not(windows))]
    let mock_runner = temp_dir.path().join("mock-image-runner");
    #[cfg(unix)]
    {
        std::fs::write(
            &mock_runner,
            "#!/usr/bin/env sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"assistant_final\",\"content\":\"IMAGE_ROUTE_OK\"}'\n",
        )
        .expect("write mock runner");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&mock_runner)
            .expect("mock metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&mock_runner, permissions).expect("chmod mock runner");
    }
    #[cfg(windows)]
    {
        std::fs::write(
            &mock_runner,
            "@echo off\r\nmore >nul\r\necho {\"type\":\"assistant_final\",\"content\":\"IMAGE_ROUTE_OK\"}\r\n",
        )
        .expect("write mock runner");
    }

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    let runner = external_cli_config
        .runners
        .get_mut(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID)
        .expect("default runner");
    runner.enabled = true;
    runner.adapter = "mock".to_string();
    runner.inject_bifrost_tools = false;
    runner.delivery_mode = crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm;
    runner.adapter_config = crate::im_gateway::external_cli::ExternalCliAdapterConfig {
        executable: Some(mock_runner.display().to_string()),
        timeout_secs: Some(10),
        ..Default::default()
    };
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external cli config");

    let mut provider = test_provider();
    provider.id = "external-route-image-provider".to_string();
    provider.owner_open_id = Some("owner-open-id".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .route_store
        .add(crate::im_gateway::types::ImRoute {
            id: "external-route-image".to_string(),
            provider_id: provider.id.clone(),
            name: "External route image".to_string(),
            enabled: true,
            event_type: crate::im_gateway::types::ImEventType::MessageReceive,
            matcher: crate::im_gateway::types::ImEventMatcher {
                chat_ids: Vec::new(),
                user_ids: vec!["owner-open-id".to_string()],
                keyword: None,
                regex: None,
            },
            action: crate::im_gateway::types::ImRouteAction::ExternalCliAgentChat {
                adapter: None,
                instructions: None,
                reply_target: crate::im_gateway::types::ReplyTarget::OriginalChat,
                delivery_mode: Some(crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm),
            },
            timeout_ms: 30_000,
            max_output_bytes: 1_048_576,
            created_at: now_ms(),
            updated_at: now_ms(),
        })
        .expect("add external route");

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
    ));

    tx.send(ImEvent {
        event_id: "evt-external-route-image".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: Some("om-route-image".to_string()),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: String::new(),
            mentions: Vec::new(),
            images: vec![
                crate::im_gateway::types::ImImageAttachment {
                    file_key: "img-route-1".to_string(),
                    source: crate::im_gateway::types::ImImageSource::MessageResource,
                    mime_type: Some("image/png".to_string()),
                    data_base64: Some("b25l".to_string()),
                    download_url: None,
                    encrypted_query_param: None,
                    aes_key: None,
                },
                crate::im_gateway::types::ImImageAttachment {
                    file_key: "img-route-2".to_string(),
                    source: crate::im_gateway::types::ImImageSource::MessageResource,
                    mime_type: Some("image/jpeg".to_string()),
                    data_base64: Some("dHdv".to_string()),
                    download_url: None,
                    encrypted_query_param: None,
                    aes_key: None,
                },
            ],
            reply_to: None,
            raw_type: Some("image".to_string()),
        }),
        received_at: now_ms(),
        raw_digest: None,
    })
    .expect("send IM image event");
    drop(tx);

    tokio::time::timeout(std::time::Duration::from_secs(60), handle)
        .await
        .expect("event loop timed out")
        .expect("event loop task panicked");

    let runs_root = crate::im_gateway::external_cli::default_runs_root();
    let mut attachments = None;
    for entry in std::fs::read_dir(runs_root).expect("runs dir") {
        let run_dir = entry.expect("run dir").path();
        let result_path = run_dir.join("result.json");
        if !result_path.exists() {
            continue;
        }
        let result = std::fs::read_to_string(result_path).expect("result json");
        if result.contains("IMAGE_ROUTE_OK") {
            let result: crate::im_gateway::external_cli::ExternalCliRunResult =
                serde_json::from_str(&result).expect("result value");
            attachments = result.metadata.get("attachments.images").cloned();
            break;
        }
    }
    let attachments: Vec<crate::im_gateway::external_cli::ExternalCliSavedImageAttachment> =
        serde_json::from_str(&attachments.expect("attachments metadata"))
            .expect("attachments metadata json");
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].mime_type, "image/png");
    assert_eq!(attachments[1].mime_type, "image/jpeg");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_external_cli_session_records_runner_failure() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    base_config.runner = Some(bifrost_agent::AgentRunnerMode::Custom(
        "broken-runner".to_string(),
    ));
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "broken-runner".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: true,
            adapter: "mock".to_string(),
            inject_bifrost_tools: false,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("/definitely/missing/bifrost-runner".to_string()),
                timeout_secs: Some(1),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external cli config");

    let mut provider = test_provider();
    provider.id = "external-runner-failure-provider".to_string();
    provider.owner_open_id = Some("owner-open-id".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.external_cli_config_store),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
    ));

    tx.send(ImEvent {
        event_id: "evt-external-runner-failure".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: None,
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "trigger broken external cli".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            reply_to: None,
            raw_type: Some("text".to_string()),
        }),
        received_at: now_ms(),
        raw_digest: None,
    })
    .expect("send IM event");
    drop(tx);

    tokio::time::timeout(std::time::Duration::from_secs(60), handle)
        .await
        .expect("event loop timed out")
        .expect("event loop task panicked");

    let session_key = build_session_key(&provider.id, Some("owner-open-id"));
    let detail = service
        .agent_session_manager
        .get_session_detail(&session_key)
        .expect("failed external runner session detail should be visible in WebUI");
    assert_eq!(detail.message_count, 2);
    assert_eq!(detail.messages[0].content, "trigger broken external cli");
    assert!(detail.messages[1].content.starts_with("Runner failed:"));

    let files = bifrost_agent::persistence::list_conversations(
        &bifrost_agent::config::agent_home_dir(),
        Some(&session_key),
    );
    assert_eq!(
        files.len(),
        1,
        "failed external runner should persist one session file"
    );
    let events = bifrost_agent::persistence::load_conversation_events(&files[0])
        .expect("load failed external runner session events");
    assert!(events.iter().any(|event| {
        event.event_type == bifrost_agent::persistence::event_types::RUN_STATE_CHANGED
            && event.content.get("state").and_then(|value| value.as_str()) == Some("failed")
    }));
    assert!(
        !events.iter().any(|event| event.event_type == "tool_result"
            && event
                .content
                .get("result")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value.contains("spawn external cli failed"))),
        "external runner failures should be recorded as run state and assistant message, not wrapper tool results"
    );
    assert!(events
        .iter()
        .any(|event| event.event_type == "assistant_message"
            && event
                .content
                .get("message")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value.starts_with("Runner failed:"))));
}

#[test]
pub(super) fn external_help_and_runner_switch_use_external_configuration() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.id = "runner-switch-provider".to_string();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");

    let mut config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    config.runners.insert(
        "custom-runner".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            adapter: crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER.to_string(),
            enabled: true,
            ..Default::default()
        },
    );
    let agent_config = crate::im_gateway::agent::ImAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom(
            "custom-runner".to_string(),
        )),
        ..Default::default()
    };
    assert_eq!(
        im_help_runner_kind_for_agent_config(&agent_config, &config, Some(&provider.id)),
        ImHelpRunnerKind::External {
            adapter: crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER.to_string()
        }
    );

    let selection = resolve_im_runner_selection(&config, "custom-runner").expect("selection");
    let mut session = bifrost_agent::AgentSession::new("runner-switch-session");
    let reply = apply_im_runner_switch_to_session(
        &service.provider_store,
        &provider.id,
        "runner-switch-session",
        &mut session,
        &selection,
    );
    assert!(reply.contains("custom-runner"));
    assert_eq!(session.runner_id.as_deref(), Some("custom-runner"));
    assert_eq!(
        session.runner_type.as_deref(),
        Some(crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER)
    );
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn concurrent_external_events_cover_active_and_queued_sessions() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.id = "concurrent-provider".to_string();
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom("web".to_string())),
        work_dir: None,
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    let mut external_config = service.external_cli_config_store.load();
    external_config.runners.insert(
        "web".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
            enabled: true,
            ..Default::default()
        },
    );
    service
        .external_cli_config_store
        .save(external_config)
        .expect("save external config");
    let client = ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider()));
    let event_for = |event_id: &str, user_id: &str, text: &str| ImEvent {
        event_id: event_id.to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some(user_id.to_string()),
            message_id: Some(format!("message-{event_id}")),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: text.to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            reply_to: None,
            raw_type: Some("text".to_string()),
        }),
        received_at: now_ms(),
        raw_digest: None,
    };

    let active_session_key = build_session_key(&provider.id, Some("owner-open-id"));
    let active_session = service
        .agent_session_manager
        .take_session(&active_session_key);
    let active_event = event_for("active", "owner-open-id", "queue active message");
    handle_concurrent_event_during_chat(
        &active_event,
        &provider,
        &active_session_key,
        &service.queue_manager,
        &client,
        &service.message_log_store,
        &service.agent_session_manager,
        &service.progress_registry,
        &service.agent_config_store,
        &service.provider_store,
        &service.event_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    assert_eq!(
        service
            .queue_manager
            .queue_status(&active_session_key)
            .len(),
        1
    );
    service.agent_session_manager.return_session(active_session);

    let other_session_key = build_session_key(&provider.id, Some("other-owner"));
    let other_session = service
        .agent_session_manager
        .take_session(&other_session_key);
    let other_event = event_for("other-active", "other-owner", "queue other active message");
    handle_concurrent_event_during_chat(
        &other_event,
        &provider,
        "some-other-active-session",
        &service.queue_manager,
        &client,
        &service.message_log_store,
        &service.agent_session_manager,
        &service.progress_registry,
        &service.agent_config_store,
        &service.provider_store,
        &service.event_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    assert_eq!(
        service.queue_manager.queue_status(&other_session_key).len(),
        1
    );
    service.agent_session_manager.return_session(other_session);

    let inactive_event = event_for("inactive", "inactive-owner", "queue inactive message");
    let inactive_session_key = build_session_key(&provider.id, Some("inactive-owner"));
    handle_concurrent_event_during_chat(
        &inactive_event,
        &provider,
        "some-other-active-session",
        &service.queue_manager,
        &client,
        &service.message_log_store,
        &service.agent_session_manager,
        &service.progress_registry,
        &service.agent_config_store,
        &service.provider_store,
        &service.event_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    assert_eq!(
        service
            .queue_manager
            .queue_status(&inactive_session_key)
            .len(),
        1
    );

    let mut restricted_provider = provider.clone();
    restricted_provider.owner_open_id = Some("owner-open-id".to_string());
    service
        .provider_store
        .update(restricted_provider.clone())
        .expect("restrict provider owner");
    let unauthorized = event_for("unauthorized", "not-owner", "ignored");
    handle_concurrent_event_during_chat(
        &unauthorized,
        &restricted_provider,
        &active_session_key,
        &service.queue_manager,
        &client,
        &service.message_log_store,
        &service.agent_session_manager,
        &service.progress_registry,
        &service.agent_config_store,
        &service.provider_store,
        &service.event_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    assert!(service
        .queue_manager
        .queue_status(&build_session_key(&provider.id, Some("not-owner")))
        .is_empty());
}
