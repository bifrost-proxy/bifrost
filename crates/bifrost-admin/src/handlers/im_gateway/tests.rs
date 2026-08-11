use super::*;
use crate::im_gateway::types::ImProviderType;
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::sync::OnceLock;

struct DefaultMethodProvider;

#[async_trait::async_trait]
impl crate::im_gateway::provider::ImProvider for DefaultMethodProvider {
    fn provider_type(&self) -> ImProviderType {
        ImProviderType::Webhook
    }

    fn send_capabilities(
        &self,
        _config: &ImProviderConfig,
    ) -> crate::im_gateway::types::ImSendCapabilities {
        crate::im_gateway::types::ImSendCapabilities {
            provider_id: "test".to_string(),
            provider_type: ImProviderType::Webhook,
            destinations: Vec::new(),
            receive_id_types: Vec::new(),
            parts: Default::default(),
            requires_context: false,
        }
    }

    async fn validate_config(
        &self,
        _config: &ImProviderConfig,
    ) -> bifrost_core::Result<crate::im_gateway::types::ProviderValidation> {
        unreachable!("not used")
    }

    async fn connect_events(
        &self,
        _config: &ImProviderConfig,
        _sink: crate::im_gateway::provider::EventSink,
    ) -> bifrost_core::Result<crate::im_gateway::types::ConnectionHandle> {
        unreachable!("not used")
    }

    async fn send_card(
        &self,
        _config: &ImProviderConfig,
        _target: &ImTarget,
        _card: serde_json::Value,
        _opts: crate::im_gateway::types::SendOptions,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        Err(bifrost_core::BifrostError::Config("card".to_string()))
    }

    async fn send_text(
        &self,
        _config: &ImProviderConfig,
        _target: &ImTarget,
        _text: &str,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        Err(bifrost_core::BifrostError::Config("text".to_string()))
    }

    async fn upload_image(
        &self,
        _config: &ImProviderConfig,
        _image_type: &str,
        _file_name: &str,
        _bytes: Vec<u8>,
        _mime_type: Option<&str>,
    ) -> bifrost_core::Result<crate::im_gateway::types::UploadedImage> {
        unreachable!("not used")
    }

    async fn send_image(
        &self,
        _config: &ImProviderConfig,
        _target: &ImTarget,
        _image_key: &str,
        _uuid: Option<&str>,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        unreachable!("not used")
    }
}

mod busy_message_mode_tests;

static IM_GATEWAY_TEST_ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

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
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .blocking_lock();
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
            ..Default::default()
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

#[tokio::test]
pub(super) async fn group_reply_assets_use_the_session_work_dir_before_provider_default() {
    let temp = tempfile::tempdir().unwrap();
    let group_dir = temp.path().join("group-project");
    let provider_dir = temp.path().join("provider-project");
    std::fs::create_dir_all(&group_dir).unwrap();
    std::fs::create_dir_all(&provider_dir).unwrap();
    std::fs::write(group_dir.join("report.txt"), "group report").unwrap();
    let mut provider = test_provider();
    provider.agent_config = Some(crate::im_gateway::types::ImProviderAgentConfig {
        runner: None,
        work_dir: Some(provider_dir.display().to_string()),
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });

    let base_dir = agent_reply_base_dir(&provider, Some(&group_dir)).unwrap();
    assert_eq!(base_dir, group_dir);
    let (_text, images, attachments, notices) = prepare_agent_reply_text_and_images_with_downloads(
        "[报告附件](./report.txt)",
        Some(&base_dir),
    )
    .await;
    assert!(images.is_empty());
    assert_eq!(attachments.len(), 1);
    assert!(notices.is_empty());
    assert_eq!(attachments[0].path, group_dir.join("report.txt"));

    assert_eq!(agent_reply_base_dir(&provider, None), Some(provider_dir));
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
        files: Vec::new(),
        reply_to: None,
        raw_type: Some("text".to_string()),
        ..Default::default()
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
        files: Vec::new(),
        reply_to: None,
        raw_type: Some("image".to_string()),
        ..Default::default()
    };
    assert_eq!(
        agent_message_text(&image_only_message),
        IMAGE_ONLY_AGENT_PROMPT
    );

    let empty_message = crate::im_gateway::types::ImEventMessage {
        text: "   ".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        files: Vec::new(),
        reply_to: None,
        raw_type: None,
        ..Default::default()
    };
    assert!(agent_message_text(&empty_message).is_empty());

    let file_only_message = crate::im_gateway::types::ImEventMessage {
        text: "   ".to_string(),
        mentions: Vec::new(),
        images: Vec::new(),
        files: vec![
            crate::im_gateway::types::ImFileAttachment {
                file_key: "file-v3-1".to_string(),
                name: Some("需求.md".to_string()),
                mime_type: Some("text/markdown".to_string()),
                size_bytes: Some(12),
                data_base64: None,
                download_url: None,
            },
            crate::im_gateway::types::ImFileAttachment {
                file_key: "file-v3-2".to_string(),
                name: Some("日志.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                size_bytes: Some(20),
                data_base64: None,
                download_url: None,
            },
        ],
        raw_type: Some("file".to_string()),
        ..Default::default()
    };
    assert_eq!(agent_message_text(&file_only_message), "[附件消息: 2 个]");
}

#[tokio::test]
pub(super) async fn resolve_event_files_handles_inline_limits_and_missing_message_id() {
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let provider = test_provider();
    let event = ImEvent {
        event_id: "evt-files-inline".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("oc_engineering".to_string()),
            chat_type: Some("group".to_string()),
            user_id: Some("ou_sender".to_string()),
            user_name: Some("Alice".to_string()),
            sender_type: Some("user".to_string()),
            message_id: None,
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };
    let mut files = Vec::new();
    for index in 0..(MAX_AGENT_ATTACHMENTS_PER_MESSAGE + 1) {
        files.push(crate::im_gateway::types::ImFileAttachment {
            file_key: format!("file-{index}"),
            name: Some(format!("file-{index}.txt")),
            mime_type: if index == 0 {
                None
            } else {
                Some("text/plain".to_string())
            },
            size_bytes: None,
            data_base64: if index < MAX_AGENT_ATTACHMENTS_PER_MESSAGE - 1 {
                Some(format!("ZmlsZS0{index}="))
            } else {
                None
            },
            download_url: None,
        });
    }

    let resolved = resolve_event_files(&client, &provider, &event, &files).await;

    assert_eq!(resolved.len(), MAX_AGENT_ATTACHMENTS_PER_MESSAGE - 1);
    assert_eq!(resolved[0].mime_type, "application/octet-stream");
    assert_eq!(resolved[0].name.as_deref(), Some("file-0.txt"));
    assert_eq!(resolved[0].data, "ZmlsZS00="); // inline base64 is preserved until runner save time
    assert_eq!(resolved[1].mime_type, "text/plain");
}

#[tokio::test]
pub(super) async fn resolve_event_files_download_errors_are_not_returned_to_runner() {
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let provider = test_provider();
    let event = ImEvent {
        event_id: "evt-files-download".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("oc_engineering".to_string()),
            chat_type: Some("group".to_string()),
            user_id: Some("ou_sender".to_string()),
            user_name: Some("Alice".to_string()),
            sender_type: Some("user".to_string()),
            message_id: Some("om_file".to_string()),
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };
    let files = vec![crate::im_gateway::types::ImFileAttachment {
        file_key: "file-needs-download".to_string(),
        name: Some("remote.txt".to_string()),
        mime_type: Some("text/plain".to_string()),
        size_bytes: Some(5),
        data_base64: None,
        download_url: None,
    }];

    let resolved = resolve_event_files(&client, &provider, &event, &files).await;

    assert!(resolved.is_empty());
}

#[tokio::test]
pub(super) async fn resolve_event_files_downloads_message_resources() {
    use http_body_util::Full;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock feishu file server");
    let port = listener.local_addr().expect("mock local addr").port();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<hyper::body::Incoming>| async move {
                    let method = req.method().clone();
                    let path = req.uri().path().to_string();
                    if method == Method::POST
                        && path == "/open-apis/auth/v3/tenant_access_token/internal"
                    {
                        return Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(bytes::Bytes::from_static(
                                    br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                )))
                                .unwrap(),
                        );
                    }
                    if method == Method::GET
                        && path == "/open-apis/im/v1/messages/om_file/resources/file-v3"
                    {
                        return Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "text/markdown; charset=utf-8")
                                .body(Full::new(bytes::Bytes::from_static(b"# Report")))
                                .unwrap(),
                        );
                    }
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Full::new(bytes::Bytes::from_static(b"not found")))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = test_provider();
    provider.base_url = Some(format!("http://127.0.0.1:{port}/open-apis"));
    let event = ImEvent {
        event_id: "evt-files-download-success".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            message_id: Some("om_file".to_string()),
            ..Default::default()
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };
    let files = vec![crate::im_gateway::types::ImFileAttachment {
        file_key: "file-v3".to_string(),
        name: Some("report.md".to_string()),
        mime_type: None,
        size_bytes: Some(8),
        data_base64: None,
        download_url: None,
    }];

    let resolved = resolve_event_files(&client, &provider, &event, &files).await;

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].mime_type, "text/markdown");
    assert_eq!(resolved[0].data, "IyBSZXBvcnQ=");
    assert_eq!(resolved[0].name.as_deref(), Some("report.md"));
}

#[tokio::test]
pub(super) async fn busy_queue_command_preserves_event_files() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    let provider = test_provider();
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let agent_config = service.agent_config_store.load();
    let event = ImEvent {
        event_id: "evt-busy-q-file".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: Some("om_busy_q_file".to_string()),
            ..Default::default()
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "/q please inspect the attachment".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            files: vec![crate::im_gateway::types::ImFileAttachment {
                file_key: "inline-q-file".to_string(),
                name: Some("queued.md".to_string()),
                mime_type: Some("text/markdown".to_string()),
                size_bytes: Some(8),
                data_base64: Some("IyBRdWV1ZWQ=".to_string()),
                download_url: None,
            }],
            raw_type: Some("file".to_string()),
            ..Default::default()
        }),
        received_at: now_ms(),
        raw_digest: None,
    };
    let session_key = build_session_key(&provider.id, Some("owner-open-id"));

    handle_busy_message(
        "/q please inspect the attachment",
        &session_key,
        BusyMessageContext {
            queue_manager: &service.queue_manager,
            client: &client,
            provider: &provider,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_session_manager: &service.agent_session_manager,
            progress_registry: &service.progress_registry,
            external_cli_config_store: &service.external_cli_config_store,
            agent_config: &agent_config,
            group_context_store: &service.group_context_store,
            group_turn_id: None,
            default_mode: BusyMessageDefaultMode::Queue,
            status_context: Default::default(),
            default_work_dir: None,
        },
    )
    .await;

    let queue = service.queue_manager.queue_status(&session_key);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].message, "please inspect the attachment");
    assert_eq!(queue[0].files.len(), 1);
    assert_eq!(queue[0].files[0].mime_type, "text/markdown");
    assert_eq!(queue[0].files[0].name.as_deref(), Some("queued.md"));
}

#[tokio::test]
pub(super) async fn codex_fast_commands_cover_busy_idle_modes_and_rejections() {
    async fn invoke_busy_fast_command(
        service: &ImGatewayService,
        client: &ImProviderClient,
        provider: &ImProviderConfig,
        agent_config: &crate::im_gateway::agent::ImAgentConfig,
        session_key: &str,
        command: &str,
    ) {
        let event = ImEvent {
            event_id: format!("evt-{}", uuid_short()),
            provider_id: provider.id.clone(),
            provider_type: provider.provider_type,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("fast-chat".to_string()),
                user_id: Some("fast-user".to_string()),
                message_id: Some(format!("om-{}", uuid_short())),
                ..Default::default()
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: command.to_string(),
                ..Default::default()
            }),
            received_at: now_ms(),
            raw_digest: None,
        };
        handle_busy_message(
            command,
            session_key,
            BusyMessageContext {
                queue_manager: &service.queue_manager,
                client,
                provider,
                event: &event,
                message_log_store: &service.message_log_store,
                agent_session_manager: &service.agent_session_manager,
                progress_registry: &service.progress_registry,
                external_cli_config_store: &service.external_cli_config_store,
                agent_config,
                group_context_store: &service.group_context_store,
                group_turn_id: None,
                default_mode: BusyMessageDefaultMode::Queue,
                status_context: Default::default(),
                default_work_dir: None,
            },
        )
        .await;
    }

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.id = "weixin-fast".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.secret_ref = None;
    let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let codex_agent_config = service.agent_config_store.load();
    let session_key = build_session_key(&provider.id, Some("fast-user"));

    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast status",
    )
    .await;
    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast off",
    )
    .await;
    let state =
        crate::im_gateway::session_state::load_session_state(&session_key, "codex", Some("Codex"))
            .expect("fast off should persist session state");
    assert_eq!(
        state.service_tier_override.as_deref(),
        Some(crate::im_gateway::external_cli::CODEX_STANDARD_SERVICE_TIER)
    );

    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast",
    )
    .await;
    let state =
        crate::im_gateway::session_state::load_session_state(&session_key, "codex", Some("Codex"))
            .expect("fast toggle should persist session state");
    assert_eq!(
        state.service_tier_override.as_deref(),
        Some(crate::im_gateway::external_cli::CODEX_FAST_SERVICE_TIER)
    );

    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast on",
    )
    .await;
    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast",
    )
    .await;
    let state =
        crate::im_gateway::session_state::load_session_state(&session_key, "codex", Some("Codex"))
            .expect("second fast toggle should persist session state");
    assert_eq!(
        state.service_tier_override.as_deref(),
        Some(crate::im_gateway::external_cli::CODEX_STANDARD_SERVICE_TIER)
    );
    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast unsupported",
    )
    .await;

    crate::im_gateway::session_state::upsert_session_state(
        &session_key,
        "codex",
        Some("Codex"),
        |state| {
            state.service_tier_override = Some("fast".to_string());
            state.service_tier_override_source = None;
        },
    )
    .expect("seed source-free service tier");
    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &codex_agent_config,
        &session_key,
        "/fast status",
    )
    .await;

    let no_runner_config = crate::im_gateway::agent::ImAgentConfig {
        runner: None,
        ..codex_agent_config.clone()
    };
    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &no_runner_config,
        "no-runner-session",
        "/fast on",
    )
    .await;

    let traex_config = crate::im_gateway::agent::ImAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom("Traex".to_string())),
        ..codex_agent_config.clone()
    };
    invoke_busy_fast_command(
        &service,
        &client,
        &provider,
        &traex_config,
        "traex-session",
        "/fast on",
    )
    .await;

    let idle_event = ImEvent {
        event_id: "evt-fast-idle".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("fast-chat".to_string()),
            user_id: Some("fast-user".to_string()),
            message_id: Some("om-fast-idle".to_string()),
            ..Default::default()
        },
        message: None,
        received_at: now_ms(),
        raw_digest: None,
    };
    assert!(
        handle_idle_im_command(
            "/fast off",
            "idle-fast-session",
            &codex_agent_config,
            IdleImCommandContext {
                client: &client,
                provider: &provider,
                provider_store: &service.provider_store,
                group_context_store: &service.group_context_store,
                external_cli_config_store: &service.external_cli_config_store,
                event: &idle_event,
                message_log_store: &service.message_log_store,
                agent_session_manager: &service.agent_session_manager,
                queue_manager: &service.queue_manager,
            },
        )
        .await
    );

    let replies = service.message_log_store.list();
    assert!(replies.iter().any(|log| {
        log.content.as_deref().is_some_and(|content| {
            content.contains("未显式设置 service tier") && content.contains("Codex 自身默认模式")
        })
    }));
    assert!(replies.iter().any(|log| {
        log.content
            .as_deref()
            .is_some_and(|content| content.contains("使用快速模式"))
    }));
    assert!(replies.iter().any(|log| {
        log.content
            .as_deref()
            .is_some_and(|content| content.contains("用法: /fast"))
    }));
    assert!(replies.iter().any(|log| {
        log.content
            .as_deref()
            .is_some_and(|content| content.contains("仅支持 Codex Runner"))
    }));
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        files: Vec::new(),
        reply_to: None,
        raw_type: Some("image".to_string()),
        ..Default::default()
    };
    assert_eq!(inbound_message_preview(&image_message), "[图片消息: 2 张]");

    let long_text = "中".repeat(240);
    let text_message = crate::im_gateway::types::ImEventMessage {
        text: long_text,
        mentions: Vec::new(),
        images: Vec::new(),
        files: Vec::new(),
        reply_to: None,
        raw_type: Some("text".to_string()),
        ..Default::default()
    };
    let preview = inbound_message_preview(&text_message);
    assert_eq!(preview.chars().count(), 203);
    assert!(preview.ends_with("..."));

    let file_message = crate::im_gateway::types::ImEventMessage {
        text: String::new(),
        mentions: Vec::new(),
        images: Vec::new(),
        files: vec![crate::im_gateway::types::ImFileAttachment {
            file_key: "file-v3-1".to_string(),
            name: Some("需求.md".to_string()),
            mime_type: Some("text/markdown".to_string()),
            size_bytes: Some(12),
            data_base64: None,
            download_url: None,
        }],
        raw_type: Some("file".to_string()),
        ..Default::default()
    };
    assert_eq!(inbound_message_preview(&file_message), "[附件消息: 1 个]");
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

    let (text, images, attachments, notices) =
        prepare_agent_reply_text_and_images_with_downloads(&markdown, None).await;

    assert_eq!(images.len(), 1);
    assert!(attachments.is_empty());
    assert!(notices.is_empty());
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

    let (text, images, attachments, notices) =
        prepare_agent_reply_text_and_images_with_downloads(&markdown, None).await;

    assert_eq!(images.len(), 1);
    assert!(attachments.is_empty());
    assert!(notices.is_empty());
    assert_eq!(images[0].alt, "下载图片");
    assert_eq!(
        std::fs::read(&images[0].path).expect("read downloaded image link"),
        b"download image bytes"
    );
    assert!(!text.contains("http://127.0.0.1"));
    assert!(text.contains("正文"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_rejects_remote_file_above_feishu_upload_limit_before_body_read() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind oversized attachment server");
    let port = listener
        .local_addr()
        .expect("attachment server addr")
        .port();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        // Drain the request before closing the socket. On Windows, dropping a
        // socket with unread inbound bytes resets the connection and can hide
        // the response headers from reqwest.
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_AGENT_REPLY_ATTACHMENT_BYTES + 1
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
    let attachment = AgentReplyRemoteAttachment {
        label: "oversized file".to_string(),
        url: format!("http://127.0.0.1:{port}/oversized.bin"),
    };

    let error = download_agent_reply_remote_attachment(&attachment)
        .await
        .expect_err("oversized remote file must be rejected before buffering its body");

    assert_eq!(MAX_AGENT_REPLY_ATTACHMENT_BYTES, 30 * 1024 * 1024);
    assert!(
        error.to_string().contains("飞书上传文件 30 MiB 上限"),
        "unexpected oversized attachment error: {error}"
    );
    server.await.expect("oversized attachment server");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_remote_file_stream_limit_and_empty_body_are_non_panicking_errors() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind streamed attachment server");
    let port = listener.local_addr().expect("streamed server addr").port();
    let server = tokio::spawn(async move {
        for body in [
            "3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
            "0\r\n\r\n",
            "10\r\nabc",
        ] {
            let (mut stream, _) = listener.accept().await.expect("accept download");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{body}"
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write chunked response");
        }
    });

    let attachment = |name: &str| AgentReplyRemoteAttachment {
        label: name.to_string(),
        url: format!("http://127.0.0.1:{port}/{name}.bin"),
    };
    let oversized = download_remote_attachment_with_limit(&attachment("streamed"), 4)
        .await
        .expect_err("chunked response must enforce the cumulative limit");
    assert!(oversized.to_string().contains("0 MiB 上限"));
    let empty = download_remote_attachment_with_limit(&attachment("empty"), 4)
        .await
        .expect_err("empty response must be rejected");
    assert!(empty.to_string().contains("empty body"));
    let truncated = download_remote_attachment_with_limit(&attachment("truncated"), 64)
        .await
        .expect_err("truncated chunked response must return a body read error");
    assert!(truncated.to_string().contains("body failed"));
    server.await.expect("streamed attachment server");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_remote_file_download_failure_returns_non_blocking_notice() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failed attachment download server");
    let port = listener.local_addr().expect("failed download addr").port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept failed download");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await;
        let body = r#"{"error":"unavailable"}"#;
        let response = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write failed download response");
    });
    let markdown = format!("[最终报告](http://127.0.0.1:{port}/report.pdf)\n结论正文");

    let (text, images, attachments, notices) =
        prepare_agent_reply_text_and_images_with_downloads(&markdown, None).await;

    server.await.expect("failed attachment download server");
    assert_eq!(text, markdown);
    assert!(images.is_empty());
    assert!(attachments.is_empty());
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("最终报告"));
    assert!(notices[0].contains("503"));
    assert!(notices[0].contains("任务结论已正常发布"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_attachment_notice_send_failure_is_logged_without_failing_task() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let temp = tempfile::tempdir().expect("temp failed notice store");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failed notice server");
    let address = listener.local_addr().expect("failed notice address");
    let server = tokio::spawn(async move {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept Feishu request");
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await.expect("read request");
            let (status, body) = if index == 0 {
                (
                    "200 OK",
                    r#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                )
            } else {
                ("500 Internal Server Error", r#"{"code":230001}"#)
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write Feishu response");
        }
    });

    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = test_provider();
    provider.base_url = Some(format!("http://{address}/open-apis"));
    let event = ImEvent {
        event_id: "evt-failed-attachment-notice".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-failed-attachment-notice".to_string()),
            message_id: Some("msg-failed-attachment-notice".to_string()),
            ..Default::default()
        },
        message: None,
        received_at: now_ms(),
        raw_digest: None,
    };
    let target = ImTarget {
        id: "failed-attachment-notice".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Failed notice".to_string(),
        receive_id_type: "chat_id".to_string(),
        receive_id: "chat-failed-attachment-notice".to_string(),
        default_msg_type: "interactive".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));

    send_agent_reply_attachments(
        &client,
        &provider,
        &event,
        &target,
        &[],
        &["模拟附件发送失败".to_string()],
        &message_log_store,
    )
    .await;
    server.await.expect("failed notice server");

    let notice = message_log_store
        .list()
        .into_iter()
        .find(|log| log.trigger.as_deref() == Some("agent_attachment_notice"))
        .expect("failed notice log");
    assert_eq!(notice.status, MessageStatus::Failed);
    assert!(notice
        .error
        .as_deref()
        .is_some_and(|value| !value.is_empty()));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_reply_attachment_failures_are_logged_and_reported_without_failing_task() {
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let temp = tempfile::tempdir().expect("temp attachment notice store");
    let empty_path = temp.path().join("empty.txt");
    std::fs::File::create(&empty_path).expect("create empty attachment");
    let oversized_path = temp.path().join("oversized.bin");
    let mut oversized = std::fs::File::create(&oversized_path).expect("create oversized file");
    oversized.write_all(b"x").expect("seed oversized file");
    oversized
        .set_len(MAX_AGENT_REPLY_ATTACHMENT_BYTES + 1)
        .expect("extend oversized sparse file");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind attachment notice server");
    let address = listener.local_addr().expect("attachment notice address");
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept Feishu request");
            let mut request = vec![0u8; 8192];
            let length = stream
                .read(&mut request)
                .await
                .expect("read Feishu request");
            let request = String::from_utf8_lossy(&request[..length]);
            let body = if request.contains("/auth/v3/tenant_access_token/internal") {
                r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
            } else {
                assert!(request.contains("/im/v1/messages/msg-attachment-limit/reply"));
                assert!(request.contains("interactive"));
                r#"{"code":0,"data":{"message_id":"notice-message"}}"#
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write Feishu response");
        }
    });

    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = test_provider();
    provider.base_url = Some(format!("http://{address}/open-apis"));
    let event = ImEvent {
        event_id: "evt-attachment-limit".to_string(),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-attachment-limit".to_string()),
            user_id: Some("sender-attachment-limit".to_string()),
            message_id: Some("msg-attachment-limit".to_string()),
            ..Default::default()
        },
        message: None,
        received_at: now_ms(),
        raw_digest: None,
    };
    let target = ImTarget {
        id: "target-attachment-limit".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Attachment limit".to_string(),
        receive_id_type: "chat_id".to_string(),
        receive_id: "chat-attachment-limit".to_string(),
        default_msg_type: "interactive".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    let attachments = vec![
        AgentReplyLocalAttachment {
            label: "empty report".to_string(),
            path: empty_path,
            mime_type: Some("text/plain".to_string()),
        },
        AgentReplyLocalAttachment {
            label: String::new(),
            path: oversized_path,
            mime_type: Some("application/octet-stream".to_string()),
        },
    ];
    let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));

    send_agent_reply_attachments(
        &client,
        &provider,
        &event,
        &target,
        &attachments,
        &["远程附件下载失败；任务结论已正常发布。".to_string()],
        &message_log_store,
    )
    .await;
    server.await.expect("attachment notice server");

    let logs = message_log_store.list();
    assert_eq!(
        logs.iter()
            .filter(|log| log.msg_type.as_deref() == Some("file"))
            .count(),
        2
    );
    assert!(logs
        .iter()
        .filter(|log| log.msg_type.as_deref() == Some("file"))
        .all(|log| log.status == MessageStatus::Failed));
    let notice = logs
        .iter()
        .find(|log| log.trigger.as_deref() == Some("agent_attachment_notice"))
        .expect("attachment failure notice log");
    assert_eq!(notice.status, MessageStatus::Success);
    let content = notice.content.as_deref().expect("notice content");
    assert!(content.contains("附件发送提示（不影响任务结论）"));
    assert!(content.contains("飞书不允许上传空文件"));
    assert!(content.contains("飞书上传文件 30 MiB 上限"));
    assert!(content.contains("远程附件下载失败"));
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
pub(super) fn agent_reply_collects_local_and_remote_archive_attachments() {
    let temp = tempfile::tempdir().expect("temp dir");
    let local_archive = temp.path().join("agent-result.tar.gz");
    std::fs::write(&local_archive, b"archive bytes").expect("write archive");
    let markdown = format!(
        "[本地打包结果]({})\n[远程打包结果](https://example.com/download?filename=runner-output.tar.zst)",
        local_archive.display()
    );
    let mut images = Vec::new();
    let mut local_attachments = Vec::new();
    collect_agent_reply_local_attachment_links(
        &markdown,
        Some(temp.path()),
        &mut images,
        &mut local_attachments,
    );
    let remote_attachments = collect_agent_reply_remote_attachment_links(&markdown);

    assert!(images.is_empty());
    assert_eq!(local_attachments.len(), 1);
    assert_eq!(local_attachments[0].path, local_archive);
    assert_eq!(remote_attachments.len(), 1);
    assert_eq!(
        remote_attachments[0].url,
        "https://example.com/download?filename=runner-output.tar.zst"
    );
    assert_eq!(
        attachment_extension_from_path("bundle.tar.gz"),
        Some("tar.gz")
    );
    for (path, extension) in [
        ("bundle.tar.bz2", "tar.bz2"),
        ("bundle.tar.xz", "tar.xz"),
        ("bundle.tar.zst", "tar.zst"),
        ("bundle.tar", "tar"),
        ("bundle.tgz", "tgz"),
        ("bundle.tbz", "tbz"),
        ("bundle.tbz2", "tbz2"),
        ("bundle.txz", "txz"),
        ("bundle.tzst", "tzst"),
        ("bundle.gz", "gz"),
        ("bundle.bz2", "bz2"),
        ("bundle.xz", "xz"),
        ("bundle.zst", "zst"),
        ("bundle.7z", "7z"),
        ("bundle.rar", "rar"),
    ] {
        assert_eq!(attachment_extension_from_path(path), Some(extension));
    }
    for (content_type, extension) in [
        ("application/x-tar", "tar"),
        ("application/gzip", "gz"),
        ("application/x-gzip", "gz"),
        ("application/x-bzip2", "bz2"),
        ("application/x-xz", "xz"),
        ("application/zstd", "zst"),
        ("application/x-zstd", "zst"),
        ("application/x-7z-compressed", "7z"),
        ("application/vnd.rar", "rar"),
        ("application/x-rar-compressed", "rar"),
    ] {
        assert_eq!(extension_from_content_type(content_type), Some(extension));
    }
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
            ..Default::default()
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
            ..Default::default()
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

#[test]
pub(super) fn provider_resolve_by_feishu_bot_id_and_name_is_exact_and_unambiguous() {
    let primary = test_provider();
    let mut duplicate_name = test_provider();
    duplicate_name.id = "feishu-secondary".to_string();
    duplicate_name.app_id = Some("cli_secondary".to_string());
    let mut disabled = test_provider();
    disabled.id = "feishu-disabled".to_string();
    disabled.app_id = Some("cli_disabled".to_string());
    disabled.display_name = "Disabled Bot".to_string();
    disabled.enabled = false;
    let mut weixin = test_provider();
    weixin.id = "weixin-main".to_string();
    weixin.provider_type = ImProviderType::Weixin;
    weixin.app_id = Some("cli_weixin".to_string());

    let providers = vec![primary, duplicate_name, disabled, weixin];
    let resolved = resolve_feishu_provider_by_bot(
        providers.clone(),
        Some(" cli_secondary "),
        Some(" Feishu Main "),
    )
    .expect("bot ID and name should resolve the same provider");
    assert_eq!(resolved.id, "feishu-secondary");

    let (status, message) =
        resolve_feishu_provider_by_bot(providers.clone(), None, Some("Feishu Main"))
            .expect_err("duplicate display names must be ambiguous");
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(message.contains("multiple enabled Feishu providers"));

    let (status, _) = resolve_feishu_provider_by_bot(providers.clone(), None, None)
        .expect_err("empty selector must fail");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = resolve_feishu_provider_by_bot(providers, Some("cli_disabled"), None)
        .expect_err("disabled providers must not resolve");
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn provider_resolve_endpoint_covers_success_and_rejection_matrix() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let primary = test_provider();
    service
        .provider_store
        .add(primary.clone())
        .expect("save primary provider");
    let mut duplicate_name = primary;
    duplicate_name.id = "feishu-secondary".to_string();
    duplicate_name.app_id = Some("cli_secondary".to_string());
    service
        .provider_store
        .add(duplicate_name)
        .expect("save duplicate-name provider");
    let client = reqwest::Client::new();

    let (address, server) = spawn_im_gateway_http(Arc::clone(&service)).await;
    let response = client
        .post(format!("http://{address}/api/im-gateway/providers/resolve"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "bot_id": "cli_xxx",
            "bot_name": "Feishu Main"
        }))
        .send()
        .await
        .expect("resolve provider");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("resolve JSON");
    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["provider_type"], "feishu");
    assert_eq!(body["display_name"], "Feishu Main");
    assert!(body.get("app_id").is_none());
    server.await.expect("resolve success server");

    for (method, body, expected) in [
        (
            reqwest::Method::POST,
            r#"{"bot_name":"Feishu Main"}"#,
            reqwest::StatusCode::CONFLICT,
        ),
        (
            reqwest::Method::POST,
            r#"{"bot_id":"missing"}"#,
            reqwest::StatusCode::NOT_FOUND,
        ),
        (
            reqwest::Method::POST,
            r#"{}"#,
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            reqwest::Method::POST,
            r#"{"bot_id":42}"#,
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            reqwest::Method::POST,
            r#"{"bot_name":42}"#,
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            reqwest::Method::POST,
            r#"{"bot_id":"broken""#,
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            reqwest::Method::GET,
            r#"{}"#,
            reqwest::StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let (address, server) = spawn_im_gateway_http(Arc::clone(&service)).await;
        let response = client
            .request(
                method,
                format!("http://{address}/api/im-gateway/providers/resolve"),
            )
            .header("connection", "close")
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .expect("resolve rejection request");
        assert_eq!(response.status(), expected);
        server.await.expect("resolve rejection server");
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
        .add(provider.clone())
        .expect("provider should be saved");
    service
        .connection_manager
        .weixin_provider()
        .store_context_for_test(&provider, "weixin-user", "test-context-token")
        .expect("context token should be persisted");
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
            "content": "migration smoke test",
            "idempotency_key": "migration-smoke-test"
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

#[tokio::test(flavor = "current_thread")]
pub(super) async fn idempotent_weixin_send_commits_successful_provider_ack() {
    use http_body_util::BodyExt;
    use sha2::Digest;

    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Weixin provider");
    let provider_address = provider_listener.local_addr().unwrap();
    let provider_server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (stream, _) = provider_listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            let handler = service_fn(move |request: Request<Incoming>| async move {
                assert_eq!(request.uri().path(), "/ilink/bot/sendmessage");
                let body = request.into_body().collect().await.unwrap().to_bytes();
                let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let client_id = payload["msg"]["client_id"].as_str().unwrap();
                if request_index == 0 {
                    assert_eq!(client_id.len(), 45);
                } else {
                    assert!(client_id.starts_with("bifrost-weixin-"));
                }
                let response = format!(
                    r#"{{"ret":0,"message_id":"provider-message-{}"}}"#,
                    request_index + 1
                );
                Ok::<_, std::convert::Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Full::new(Bytes::from(response)))
                        .unwrap(),
                )
            });
            http1::Builder::new()
                .keep_alive(false)
                .serve_connection(io, handler)
                .await
                .unwrap();
        }
    });

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "weixin-success".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.base_url = Some(format!("http://{provider_address}"));
    provider.app_id = Some("bot@im.bot".to_string());
    provider.secret_ref = Some("bot-token".to_string());
    provider.owner_open_id = Some("owner-user".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("provider should be saved");
    service
        .connection_manager
        .weixin_provider()
        .store_context_for_test(&provider, "owner-user", "context-token")
        .expect("context should persist");

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "weixin-success",
            "target_id": "__owner__",
            "msg_type": "text",
            "content": "exactly once",
            "idempotency_key": "successful-send"
        }))
        .send()
        .await
        .expect("send successful message");
    let status = response.status();
    let response_text = response.text().await.expect("send response text");
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "unexpected send response: {response_text}"
    );
    let body: serde_json::Value = serde_json::from_str(&response_text).expect("send response");
    assert_eq!(body["message_id"], "provider-message-1");
    server.await.expect("message server task");

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "weixin-success",
            "target_id": "__owner__",
            "msg_type": "interactive",
            "card": {
                "header": {"title": {"tag": "plain_text", "content": "Daily"}},
                "elements": [{"tag": "markdown", "content": "Summary"}]
            }
        }))
        .send()
        .await
        .expect("send non-idempotent interactive message");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("interactive send response");
    assert_eq!(body["message_id"], "provider-message-2");
    server.await.expect("interactive message server task");

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "weixin-success",
            "target_id": "__owner__",
            "msg_type": "text",
            "content": "non-idempotent text"
        }))
        .send()
        .await
        .expect("send non-idempotent text message");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("text send response");
    assert_eq!(body["message_id"], "provider-message-3");
    server.await.expect("text message server task");
    provider_server.await.expect("provider server task");

    let logs = service.message_log_store.list();
    assert_eq!(logs.len(), 3);
    assert!(logs.iter().all(|log| log.status == MessageStatus::Success));
    assert!(matches!(
        service
            .outbox_store
            .begin(
                "successful-send",
                "weixin-success",
                "__owner__",
                "text",
                &format!(
                    "{:x}",
                    sha2::Sha256::digest(
                        serde_json::to_vec(&serde_json::json!("exactly once")).unwrap()
                    )
                )
            )
            .unwrap(),
        crate::im_gateway::ImOutboxBegin::Replay { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn provider_status_reports_weixin_send_readiness_and_missing_provider() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "weixin-status".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.owner_open_id = Some("owner-user".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("provider should be saved");

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/weixin-status/status"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("query provider status");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("status body");
    assert_eq!(body["send_ready"], false);
    assert_eq!(
        body["send_ready_reason"],
        "awaiting an inbound message context token"
    );
    server.await.expect("provider status server task");

    service.connection_manager.set_status_for_test(
        "weixin-status",
        crate::im_gateway::types::ConnectionStatus {
            state: crate::im_gateway::types::ConnectionState::Connected,
            last_connected_at: Some(1),
            last_event_at: Some(2),
            reconnect_count: 0,
            last_error: None,
        },
    );
    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/weixin-status/status"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("query connected provider without send context");
    let body: serde_json::Value = response.json().await.expect("status body");
    assert_eq!(body["state"], "connected");
    assert_eq!(body["send_ready"], false);
    assert_eq!(
        body["send_ready_reason"],
        "awaiting an inbound message context token"
    );
    server
        .await
        .expect("connected not-ready provider status server task");

    service
        .connection_manager
        .weixin_provider()
        .store_context_for_test(&provider, "owner-user", "context-token")
        .expect("context should persist");
    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/weixin-status/status"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("query send-ready provider status");
    let body: serde_json::Value = response.json().await.expect("status body");
    assert_eq!(body["send_ready"], true);
    assert!(body.get("send_ready_reason").is_none());
    server.await.expect("provider status server task");

    service.connection_manager.set_status_for_test(
        "status-without-provider",
        crate::im_gateway::types::ConnectionStatus {
            state: crate::im_gateway::types::ConnectionState::Connected,
            last_connected_at: Some(3),
            last_event_at: None,
            reconnect_count: 0,
            last_error: None,
        },
    );
    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/status-without-provider/status"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("query status without provider config");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("status body");
    assert_eq!(body["state"], "connected");
    assert!(body.get("send_ready").is_none());
    server.await.expect("status without provider server task");

    let (address, server) = spawn_im_gateway_http(service).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/missing/status"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("query missing provider status");
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    server.await.expect("missing provider status server task");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn idempotent_message_send_rejects_oversized_key_and_not_ready_weixin() {
    use sha2::Digest;

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "weixin-not-ready".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.owner_open_id = Some("owner-user".to_string());
    service
        .provider_store
        .add(provider)
        .expect("provider should be saved");

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "weixin-not-ready",
            "target_id": "__owner__",
            "msg_type": "text",
            "content": "hello",
            "idempotency_key": "x".repeat(513)
        }))
        .send()
        .await
        .expect("send oversized idempotency key");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    server.await.expect("oversized key server task");

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "weixin-not-ready",
            "target_id": "__owner__",
            "msg_type": "text",
            "content": "hello",
            "idempotency_key": "not-ready-once"
        }))
        .send()
        .await
        .expect("send before context is ready");
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    server.await.expect("not-ready server task");

    assert!(matches!(
        service
            .outbox_store
            .begin(
                "not-ready-once",
                "weixin-not-ready",
                "__owner__",
                "text",
                &format!(
                    "{:x}",
                    sha2::Sha256::digest(serde_json::to_vec(&serde_json::json!("hello")).unwrap())
                )
            )
            .unwrap(),
        crate::im_gateway::ImOutboxBegin::Send { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn sent_outbox_message_is_replayed_without_contacting_provider() {
    use sha2::Digest;

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.owner_open_id = Some("owner-user".to_string());
    service
        .provider_store
        .add(provider)
        .expect("provider should be saved");

    let payload = serde_json::json!("already sent");
    let payload_sha256 = format!(
        "{:x}",
        sha2::Sha256::digest(serde_json::to_vec(&payload).unwrap())
    );
    service
        .outbox_store
        .begin(
            "daily-replay",
            "feishu-main",
            "__owner__",
            "text",
            &payload_sha256,
        )
        .unwrap();
    service
        .outbox_store
        .mark_sent("daily-replay", Some("message-already-sent"))
        .unwrap();

    let (address, server) = spawn_im_gateway_http(service.clone()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "feishu-main",
            "target_id": "__owner__",
            "msg_type": "text",
            "content": "different payload",
            "idempotency_key": "daily-replay"
        }))
        .send()
        .await
        .expect("reject reused key with different payload");
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    server.await.expect("conflict server task");

    let (address, server) = spawn_im_gateway_http(service).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/im-gateway/messages/send"))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id": "feishu-main",
            "target_id": "__owner__",
            "msg_type": "text",
            "content": "already sent",
            "idempotency_key": "daily-replay"
        }))
        .send()
        .await
        .expect("replay sent message");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("replay body");
    assert_eq!(body["message_id"], "message-already-sent");
    assert_eq!(body["request_id"], "idempotent-replay");
    server.await.expect("replay server task");
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
        idempotency_key: None,
        destination: None,
        parts: Vec::new(),
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
        idempotency_key: None,
        destination: None,
        parts: Vec::new(),
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
        idempotency_key: None,
        destination: None,
        parts: Vec::new(),
    };

    let content = normalized_send_content(&body).expect("image content");
    assert_eq!(content["image_key"], "img_v3_key");
    assert_eq!(content["image_type"], "message");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn rich_card_builder_uses_image_key_and_markdown() {
    let provider = test_provider();
    let feishu = ImProviderClient::Feishu(std::sync::Arc::new(
        crate::im_gateway::feishu::FeishuProvider::new(),
    ));
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

#[test]
pub(super) fn outbound_capabilities_distinguish_feishu_and_weixin() {
    let feishu_config = test_provider();
    let feishu = ImProviderClient::Feishu(std::sync::Arc::new(
        crate::im_gateway::feishu::FeishuProvider::new(),
    ));
    let feishu_caps = feishu.send_capabilities(&feishu_config);
    assert_eq!(feishu_caps.destinations, ["owner", "target", "direct"]);
    assert_eq!(
        feishu_caps.part("file").expect("file capability").support,
        crate::im_gateway::types::ImSendSupportLevel::Native
    );
    assert_eq!(
        feishu_caps
            .part("native_card")
            .expect("card capability")
            .support,
        crate::im_gateway::types::ImSendSupportLevel::Native
    );

    let mut weixin_config = test_provider();
    weixin_config.id = "weixin-main".to_string();
    weixin_config.provider_type = ImProviderType::Weixin;
    let weixin = ImProviderClient::Weixin(std::sync::Arc::new(WeixinProvider::new()));
    let weixin_caps = weixin.send_capabilities(&weixin_config);
    assert!(weixin_caps.requires_context);
    assert_eq!(
        weixin_caps
            .part("markdown")
            .expect("markdown capability")
            .support,
        crate::im_gateway::types::ImSendSupportLevel::Degraded
    );
    assert_eq!(
        weixin_caps.part("file").expect("file capability").support,
        crate::im_gateway::types::ImSendSupportLevel::Unsupported
    );
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn outbound_capabilities_endpoint_and_unsupported_dispatch_are_explicit() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = std::sync::Arc::new(ImGatewayService::new(temp_dir.path()));
    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("save provider");

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/feishu-main/capabilities"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("get capabilities");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("capabilities JSON");
    assert_eq!(body["provider_id"], "feishu-main");
    assert_eq!(body["parts"]["file"]["support"], "native");
    server.await.expect("capabilities server");

    let unsupported = ImProviderClient::Unsupported(ImProviderType::Webhook);
    let target = ImTarget {
        id: "owner".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Owner".to_string(),
        receive_id_type: "open_id".to_string(),
        receive_id: "ou_owner".to_string(),
        default_msg_type: "text".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    let concrete_feishu = crate::im_gateway::feishu::FeishuProvider::new();
    assert!(crate::im_gateway::provider::ImProvider::upload_file(
        &concrete_feishu,
        &provider,
        "a.txt",
        vec![1],
        Some("text/plain"),
    )
    .await
    .is_err());
    assert!(crate::im_gateway::provider::ImProvider::send_file(
        &concrete_feishu,
        &provider,
        &target,
        "file-key",
        Some("uuid"),
    )
    .await
    .is_err());
    let concrete_weixin = WeixinProvider::new();
    assert!(crate::im_gateway::provider::ImProvider::send_native_card(
        &concrete_weixin,
        &provider,
        &target,
        serde_json::json!({"elements": []}),
        crate::im_gateway::types::SendOptions::default(),
    )
    .await
    .is_err());
    let weixin_client = ImProviderClient::Weixin(std::sync::Arc::new(WeixinProvider::new()));
    assert!(weixin_client
        .send_native_card(
            &provider,
            &target,
            serde_json::json!({"elements": []}),
            crate::im_gateway::types::SendOptions::default(),
        )
        .await
        .is_err());
    let default_provider = DefaultMethodProvider;
    assert!(crate::im_gateway::provider::ImProvider::upload_file(
        &default_provider,
        &provider,
        "a.txt",
        vec![1],
        Some("text/plain"),
    )
    .await
    .is_err());
    assert!(crate::im_gateway::provider::ImProvider::send_file(
        &default_provider,
        &provider,
        &target,
        "file-key",
        Some("uuid"),
    )
    .await
    .is_err());
    assert!(crate::im_gateway::provider::ImProvider::send_native_card(
        &default_provider,
        &provider,
        &target,
        serde_json::json!({"elements": []}),
        crate::im_gateway::types::SendOptions::default(),
    )
    .await
    .is_err());
    assert!(unsupported.send_capabilities(&provider).parts.is_empty());
    assert!(unsupported
        .send_text(&provider, &target, "hello")
        .await
        .is_err());
    assert!(unsupported
        .send_text_with_uuid(&provider, &target, "hello", Some("uuid"))
        .await
        .is_err());
    assert!(unsupported
        .upload_image(&provider, "message", "a.png", vec![1], Some("image/png"))
        .await
        .is_err());
    assert!(unsupported
        .send_image(&provider, &target, "img", Some("uuid"))
        .await
        .is_err());
    assert!(unsupported
        .upload_file(&provider, "a.txt", vec![1], Some("text/plain"))
        .await
        .is_err());
    assert!(unsupported
        .send_file(&provider, &target, "file", Some("uuid"))
        .await
        .is_err());
    assert!(unsupported
        .send_native_card(
            &provider,
            &target,
            serde_json::json!({"elements": []}),
            crate::im_gateway::types::SendOptions::default(),
        )
        .await
        .is_err());
    assert!(unsupported
        .create_feishu_group_chat(&provider, "group", "ou_owner", "uuid")
        .await
        .is_err());
    assert!(unsupported
        .send_card(
            &provider,
            &target,
            serde_json::json!({"elements": []}),
            crate::im_gateway::types::SendOptions::default(),
        )
        .await
        .is_err());
    assert!(unsupported
        .add_reaction(&provider, "om_message", "THUMBSUP")
        .await
        .is_err());
    let image = crate::im_gateway::types::ImImageAttachment {
        file_key: "img".to_string(),
        source: Default::default(),
        mime_type: None,
        data_base64: None,
        download_url: None,
        encrypted_query_param: None,
        aes_key: None,
    };
    assert!(unsupported
        .download_message_image_resource(&provider, "om_message", &image)
        .await
        .is_err());
    let file = crate::im_gateway::types::ImFileAttachment {
        file_key: "file".to_string(),
        name: None,
        mime_type: None,
        size_bytes: None,
        data_base64: None,
        download_url: None,
    };
    assert!(unsupported
        .download_message_file_resource(&provider, "om_message", &file)
        .await
        .is_err());

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let missing = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/im-gateway/providers/missing/capabilities"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("missing capabilities request");
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
    server.await.expect("missing capabilities server");

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let method = reqwest::Client::new()
        .post(format!(
            "http://{address}/api/im-gateway/providers/feishu-main/capabilities"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("capabilities method request");
    assert_eq!(method.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    server.await.expect("capabilities method server");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn outbound_target_validation_and_upload_error_matrix() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = std::sync::Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut primary_provider = test_provider();
    primary_provider.owner_open_id = Some("ou_owner".to_string());
    service
        .provider_store
        .add(primary_provider)
        .expect("save provider");
    let mut disabled_provider = test_provider();
    disabled_provider.id = "feishu-disabled".to_string();
    disabled_provider.enabled = false;
    service
        .provider_store
        .add(disabled_provider)
        .expect("save disabled provider");
    let mut weixin_provider = test_provider();
    weixin_provider.id = "weixin-upload".to_string();
    weixin_provider.provider_type = ImProviderType::Weixin;
    service
        .provider_store
        .add(weixin_provider)
        .expect("save Weixin provider");
    let mut webhook_provider = test_provider();
    webhook_provider.id = "webhook-upload".to_string();
    webhook_provider.provider_type = ImProviderType::Webhook;
    service
        .provider_store
        .add(webhook_provider)
        .expect("save webhook provider");
    let http = reqwest::Client::new();

    for (target, expected) in [
        (
            serde_json::json!({
                "id": "",
                "provider_id": "feishu-main",
                "display_name": "Bad",
                "receive_id_type": "chat_id",
                "receive_id": "oc_group",
                "enabled": true
            }),
            "target id is required",
        ),
        (
            serde_json::json!({
                "id": "bad-display",
                "provider_id": "feishu-main",
                "display_name": " ",
                "receive_id_type": "chat_id",
                "receive_id": "oc_group",
                "enabled": true
            }),
            "display_name is required",
        ),
        (
            serde_json::json!({
                "id": "missing-receive",
                "provider_id": "feishu-main",
                "display_name": "Missing Receive",
                "receive_id_type": "chat_id",
                "receive_id": " ",
                "enabled": true
            }),
            "receive_id is required",
        ),
        (
            serde_json::json!({
                "id": "missing-provider",
                "provider_id": "does-not-exist",
                "display_name": "Missing Provider",
                "receive_id_type": "chat_id",
                "receive_id": "oc_group",
                "enabled": true
            }),
            "Provider 'does-not-exist' not found",
        ),
        (
            serde_json::json!({
                "id": "bad-type",
                "provider_id": "feishu-main",
                "display_name": "Bad Type",
                "receive_id_type": "phone",
                "receive_id": "123",
                "enabled": true
            }),
            "is not supported",
        ),
    ] {
        let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
        let response = http
            .post(format!("http://{address}/api/im-gateway/targets"))
            .header("connection", "close")
            .json(&target)
            .send()
            .await
            .expect("post invalid target");
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        assert!(response
            .text()
            .await
            .expect("target error body")
            .contains(expected));
        server.await.expect("target server");
    }

    let valid_target = serde_json::json!({
        "id": "valid-target",
        "provider_id": "feishu-main",
        "display_name": "Valid Target",
        "receive_id_type": "chat_id",
        "receive_id": "oc_group",
        "enabled": true
    });
    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = http
        .post(format!("http://{address}/api/im-gateway/targets"))
        .header("connection", "close")
        .json(&valid_target)
        .send()
        .await
        .expect("post valid target");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    server.await.expect("valid target server");

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = http
        .patch(format!(
            "http://{address}/api/im-gateway/targets/valid-target"
        ))
        .header("connection", "close")
        .json(&serde_json::json!({"receive_id_type":"phone"}))
        .send()
        .await
        .expect("patch invalid target");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    server.await.expect("invalid target patch server");

    for (query, body, expected_status) in [
        ("", vec![1], reqwest::StatusCode::BAD_REQUEST),
        (
            "?provider_id=feishu-main",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=feishu-main&kind=image",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=missing&kind=image&file_name=a.png",
            vec![1],
            reqwest::StatusCode::NOT_FOUND,
        ),
        (
            "?provider_id=feishu-main&kind=archive&file_name=a.zip",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=feishu-main&kind=image&file_name=..%2Fa.png",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=feishu-main&kind=image&file_name=a.png",
            Vec::new(),
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=feishu-disabled&kind=image&file_name=a.png",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=weixin-upload&kind=file&file_name=a.txt",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
        (
            "?provider_id=webhook-upload&kind=image&file_name=a.png",
            vec![1],
            reqwest::StatusCode::BAD_REQUEST,
        ),
    ] {
        let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
        let response = http
            .post(format!(
                "http://{address}/api/im-gateway/messages/upload{query}"
            ))
            .header("connection", "close")
            .body(body)
            .send()
            .await
            .expect("post invalid upload");
        assert_eq!(response.status(), expected_status);
        server.await.expect("upload server");
    }

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = http
        .get(format!(
            "http://{address}/api/im-gateway/messages/upload?provider_id=feishu-main&kind=image&file_name=a.png"
        ))
        .header("connection", "close")
        .send()
        .await
        .expect("get upload endpoint");
    assert_eq!(response.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    server.await.expect("upload method server");

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = http
        .post(format!(
            "http://{address}/api/im-gateway/messages/upload?provider_id=feishu-main&kind=image&file_name=huge.png"
        ))
        .header("connection", "close")
        .header("content-length", (10 * 1024 * 1024 + 1).to_string())
        .body(vec![0_u8])
        .send()
        .await
        .expect("post oversized upload");
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    server.await.expect("oversized upload server");

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = http
        .post(format!(
            "http://{address}/api/im-gateway/messages/upload?provider_id=feishu-main&kind=image&file_name=a.png"
        ))
        .header("connection", "close")
        .body(vec![1_u8])
        .send()
        .await
        .expect("post provider-failed upload");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    server.await.expect("provider-failed upload server");

    for (data_base64, expected_status) in [
        ("%%%", reqwest::StatusCode::BAD_REQUEST),
        ("AQ==", reqwest::StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
        let response = http
            .post(format!("http://{address}/api/im-gateway/messages/send"))
            .header("connection", "close")
            .json(&serde_json::json!({
                "provider_id": "feishu-main",
                "target_id": "__owner__",
                "msg_type": "image",
                "image": {"data_base64": data_base64, "file_name": "a.png"}
            }))
            .send()
            .await
            .expect("send inline image failure");
        assert_eq!(response.status(), expected_status);
        server.await.expect("inline image failure server");
    }
}

#[tokio::test(flavor = "current_thread")]
#[cfg(not(windows))]
pub(super) async fn outbound_chunked_upload_enforces_streaming_size_limit() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = std::sync::Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou_owner".to_string());
    service.provider_store.add(provider).expect("save provider");

    let (address, server) = spawn_im_gateway_http(service).await;
    let stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect chunked upload client");
    let body = vec![b'x'; 10 * 1024 * 1024 + 1];
    let headers = format!(
        "POST /api/im-gateway/messages/upload?provider_id=feishu-main&kind=image&file_name=huge.png HTTP/1.1\r\nHost: {address}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        body.len()
    );
    let (mut reader, mut writer) = stream.into_split();
    let upload = tokio::spawn(async move {
        writer
            .write_all(headers.as_bytes())
            .await
            .expect("write chunked upload headers");
        let _ = writer.write_all(&body).await;
        let _ = writer.write_all(b"\r\n0\r\n\r\n").await;
    });

    let mut response = Vec::new();
    let read_result = reader.read_to_end(&mut response).await;
    upload.await.expect("chunked upload writer");
    if let Err(error) = read_result {
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
            ) && response.starts_with(b"HTTP/1.1 413"),
            "read chunked upload response: {error}"
        );
        server.await.expect("chunked upload server");
        return;
    }
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 413"),
        "unexpected response: {}",
        response.lines().next().unwrap_or_default()
    );
    server.await.expect("chunked upload server");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn outbound_bundle_validation_destination_and_provider_defaults_cover_edges() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let mut feishu = test_provider();
    feishu.owner_open_id = Some("ou_owner".to_string());
    service
        .provider_store
        .add(feishu.clone())
        .expect("save Feishu provider");

    let request =
        |value| serde_json::from_value::<SendMessageRequest>(value).expect("valid request shape");
    for (body, expected) in [
        (
            request(serde_json::json!({
                "provider_id": "feishu-main",
                "destination": {"mode": "owner"},
                "parts": []
            })),
            StatusCode::BAD_REQUEST,
        ),
        (
            request(serde_json::json!({
                "provider_id": "feishu-main",
                "destination": {"mode": "owner"},
                "parts": (0..17).map(|index| serde_json::json!({"type":"text","text":format!("part-{index}")})).collect::<Vec<_>>()
            })),
            StatusCode::BAD_REQUEST,
        ),
        (
            request(serde_json::json!({
                "provider_id": "feishu-main",
                "destination": {"mode": "owner"},
                "parts": [{"type":"text","text":" "}]
            })),
            StatusCode::BAD_REQUEST,
        ),
        (
            request(serde_json::json!({
                "destination": {"mode": "owner"},
                "parts": [{"type":"text","text":"hello"}]
            })),
            StatusCode::BAD_REQUEST,
        ),
        (
            request(serde_json::json!({
                "provider_id": "missing",
                "destination": {"mode": "owner"},
                "parts": [{"type":"text","text":"hello"}]
            })),
            StatusCode::NOT_FOUND,
        ),
        (
            request(serde_json::json!({
                "provider_id": "feishu-main",
                "destination": {"mode": "owner"},
                "parts": [{"type":"text","text":"hello"}],
                "idempotency_key": "x".repeat(481)
            })),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            handle_message_bundle_send(&service, body).await.status(),
            expected
        );
    }
    let mut disabled = feishu.clone();
    disabled.id = "feishu-disabled-bundle".to_string();
    disabled.enabled = false;
    service
        .provider_store
        .add(disabled)
        .expect("save disabled bundle provider");
    assert_eq!(
        handle_message_bundle_send(
            &service,
            request(serde_json::json!({
                "provider_id": "feishu-disabled-bundle",
                "destination": {"mode":"owner"},
                "parts": [{"type":"text","text":"hello"}]
            }))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let owner = request(serde_json::json!({
        "provider_id": "feishu-main",
        "parts": [{"type":"text","text":"hello"}]
    }));
    let (_, _, _, summary) =
        resolve_bundle_destination(&service, &feishu, &owner).expect("legacy owner default");
    assert_eq!(summary, "owner");

    let direct_bad_type = request(serde_json::json!({
        "destination": {"mode":"direct","receive_id_type":"phone","receive_id":"123"},
        "parts": [{"type":"text","text":"hello"}]
    }));
    assert_eq!(
        resolve_bundle_destination(&service, &feishu, &direct_bad_type)
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let direct_empty = request(serde_json::json!({
        "destination": {"mode":"direct","receive_id_type":"chat_id","receive_id":" "},
        "parts": [{"type":"text","text":"hello"}]
    }));
    assert_eq!(
        resolve_bundle_destination(&service, &feishu, &direct_empty)
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let direct_short = request(serde_json::json!({
        "destination": {"mode":"direct","receive_id_type":"chat_id","receive_id":"short"},
        "parts": [{"type":"text","text":"hello"}]
    }));
    let (_, _, _, summary) =
        resolve_bundle_destination(&service, &feishu, &direct_short).expect("short direct ID");
    assert_eq!(summary, "direct:chat_id:short");

    let target_missing = request(serde_json::json!({
        "destination": {"mode":"target","target_id":"missing"},
        "parts": [{"type":"text","text":"hello"}]
    }));
    assert_eq!(
        resolve_bundle_destination(&service, &feishu, &target_missing)
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    service
        .target_store
        .add(ImTarget {
            id: "owned-target".to_string(),
            provider_id: feishu.id.clone(),
            display_name: "Owned Target".to_string(),
            receive_id_type: "chat_id".to_string(),
            receive_id: "oc_owned".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("save owned target");
    service
        .target_store
        .add(ImTarget {
            id: "disabled-target".to_string(),
            provider_id: feishu.id.clone(),
            display_name: "Disabled Target".to_string(),
            receive_id_type: "chat_id".to_string(),
            receive_id: "oc_disabled".to_string(),
            default_msg_type: "text".to_string(),
            enabled: false,
            created_at: 0,
            updated_at: 0,
        })
        .expect("save disabled target");
    assert_eq!(
        handle_message_bundle_send(
            &service,
            request(serde_json::json!({
                "provider_id": "feishu-main",
                "destination": {"mode":"target","target_id":"disabled-target"},
                "parts": [{"type":"text","text":"hello"}]
            }))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let target_ok = request(serde_json::json!({
        "destination": {"mode":"target","target_id":"owned-target"},
        "parts": [{"type":"text","text":"hello"}]
    }));
    let (_, id, name, summary) =
        resolve_bundle_destination(&service, &feishu, &target_ok).expect("owned target");
    assert_eq!(
        (id.as_str(), name.as_str(), summary.as_str()),
        ("owned-target", "Owned Target", "target:owned-target")
    );
    let target_legacy = request(serde_json::json!({
        "target_id": "owned-target",
        "parts": [{"type":"text","text":"hello"}]
    }));
    assert_eq!(
        resolve_bundle_destination(&service, &feishu, &target_legacy)
            .expect("legacy target destination")
            .3,
        "target:owned-target"
    );
    let mut other_provider = feishu.clone();
    other_provider.id = "feishu-other".to_string();
    assert_eq!(
        resolve_bundle_destination(&service, &other_provider, &target_ok)
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );

    let mut no_owner = feishu.clone();
    no_owner.owner_open_id = Some(" ".to_string());
    assert_eq!(
        resolve_bundle_destination(&service, &no_owner, &owner)
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );

    let mut weixin = test_provider();
    weixin.id = "weixin-no-context".to_string();
    weixin.provider_type = ImProviderType::Weixin;
    weixin.owner_open_id = Some("wx-owner".to_string());
    service
        .provider_store
        .add(weixin.clone())
        .expect("save Weixin provider");
    let not_ready = request(serde_json::json!({
        "provider_id": "weixin-no-context",
        "destination": {"mode":"owner"},
        "parts": [{"type":"markdown","text":"**hello**"}]
    }));
    assert_eq!(
        handle_message_bundle_send(&service, not_ready)
            .await
            .status(),
        StatusCode::BAD_GATEWAY
    );

    service
        .connection_manager
        .weixin_provider()
        .store_context_for_test(&weixin, "wx-owner", "context-token")
        .expect("store Weixin context");
    let mut failing_weixin = weixin.clone();
    failing_weixin.id = "weixin-failing".to_string();
    failing_weixin.base_url = Some("http://127.0.0.1:9".to_string());
    service
        .provider_store
        .add(failing_weixin.clone())
        .expect("save failing Weixin provider");
    service
        .connection_manager
        .weixin_provider()
        .store_context_for_test(&failing_weixin, "wx-owner", "context-token")
        .expect("store failing Weixin context");
    let provider_failure = request(serde_json::json!({
        "provider_id": "weixin-failing",
        "destination": {"mode":"owner"},
        "parts": [{"type":"markdown","text":"**hello**"}]
    }));
    assert_eq!(
        handle_message_bundle_send(&service, provider_failure)
            .await
            .status(),
        StatusCode::BAD_GATEWAY
    );

    let mut webhook = test_provider();
    webhook.id = "webhook-main".to_string();
    webhook.provider_type = ImProviderType::Webhook;
    webhook.owner_open_id = Some("hook-owner".to_string());
    service
        .provider_store
        .add(webhook)
        .expect("save webhook provider");
    let unsupported = request(serde_json::json!({
        "provider_id": "webhook-main",
        "destination": {"mode":"owner"},
        "parts": [{"type":"text","text":"hello"}]
    }));
    assert_eq!(
        handle_message_bundle_send(&service, unsupported)
            .await
            .status(),
        StatusCode::BAD_GATEWAY
    );

    let weixin_provider = WeixinProvider::new();
    let target = ImTarget {
        id: "wx-owner".to_string(),
        provider_id: weixin.id.clone(),
        display_name: "Owner".to_string(),
        receive_id_type: "open_id".to_string(),
        receive_id: "wx-owner".to_string(),
        default_msg_type: "text".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    assert!(crate::im_gateway::provider::ImProvider::upload_file(
        &weixin_provider,
        &weixin,
        "a.txt",
        vec![1],
        Some("text/plain")
    )
    .await
    .is_err());
    assert!(crate::im_gateway::provider::ImProvider::send_file(
        &weixin_provider,
        &weixin,
        &target,
        "file-key",
        None
    )
    .await
    .is_err());
    assert!(
        crate::im_gateway::provider::ImProvider::send_text_with_uuid(
            &weixin_provider,
            &weixin,
            &target,
            "hello",
            None
        )
        .await
        .is_err()
    );

    let legacy_owner_missing = request(serde_json::json!({
        "provider_id": "feishu-main",
        "target_id": "owner",
        "msg_type": "text",
        "content": "hello"
    }));
    let mut provider_without_owner = feishu.clone();
    provider_without_owner.id = "feishu-no-owner".to_string();
    provider_without_owner.owner_open_id = None;
    service
        .provider_store
        .add(provider_without_owner)
        .expect("save ownerless provider");
    let mut legacy_owner_missing = legacy_owner_missing;
    legacy_owner_missing.provider_id = Some("feishu-no-owner".to_string());
    assert_eq!(
        resolve_send_message_request(&service, &legacy_owner_missing)
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );

    let feishu_client = ImProviderClient::Feishu(std::sync::Arc::new(
        crate::im_gateway::feishu::FeishuProvider::new(),
    ));
    let image_body = request(serde_json::json!({
        "msg_type": "image",
        "image": {"image_key": "img_existing"}
    }));
    assert_eq!(
        prepare_outbound_content(
            &feishu_client,
            &feishu,
            &image_body,
            serde_json::Value::Null
        )
        .await
        .expect("prepare image")["image_key"],
        "img_existing"
    );
    let rich_body = request(serde_json::json!({
        "msg_type": "interactive",
        "rich_card": {"text": "**hello**"}
    }));
    assert!(
        prepare_outbound_content(&feishu_client, &feishu, &rich_body, serde_json::Value::Null)
            .await
            .expect("prepare rich card")["elements"]
            .is_array()
    );
    assert!(resolve_image_key(&feishu_client, &feishu, None)
        .await
        .is_err());
    let rich_with_invalid_image = SendRichCardRequest {
        title: None,
        text: None,
        image_key: None,
        image: Some(SendImageRequest {
            image_key: None,
            data_base64: Some("%%%".to_string()),
            file_name: None,
            mime_type: None,
            image_type: default_feishu_image_type(),
        }),
        image_alt: None,
    };
    assert!(
        build_rich_card_content(&feishu_client, &feishu, &rich_with_invalid_image)
            .await
            .is_err()
    );
    let unsupported_client = ImProviderClient::Unsupported(ImProviderType::Webhook);
    let plain_rich = SendRichCardRequest {
        title: None,
        text: Some("plain markdown".to_string()),
        image_key: None,
        image: None,
        image_alt: None,
    };
    assert!(
        build_rich_card_content(&unsupported_client, &feishu, &plain_rich)
            .await
            .expect("plain rich card")["elements"][0]["content"]
            .as_str()
            .is_some_and(|value| value.contains("plain markdown"))
    );
}

#[test]
pub(super) fn outbound_bundle_part_validation_rejects_empty_and_unsafe_payloads() {
    assert!(validate_send_part(&SendPartRequest::Text {
        text: "  ".to_string()
    })
    .is_err());
    assert!(validate_send_part(&SendPartRequest::Image {
        image_key: String::new()
    })
    .is_err());
    assert!(validate_send_part(&SendPartRequest::File {
        file_key: "file-key".to_string(),
        file_name: Some("../secret.txt".to_string()),
    })
    .is_err());
    assert!(validate_send_part(&SendPartRequest::File {
        file_key: "file-key".to_string(),
        file_name: Some("..\\secret.txt".to_string()),
    })
    .is_err());
    assert!(validate_send_part(&SendPartRequest::File {
        file_key: " ".to_string(),
        file_name: None,
    })
    .is_err());
    assert!(validate_send_part(&SendPartRequest::NativeCard {
        card: serde_json::json!([])
    })
    .is_err());
    assert!(validate_send_part(&SendPartRequest::Markdown {
        text: "**ok**".to_string()
    })
    .is_ok());
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn outbound_weixin_bundle_degrades_markdown_and_reports_unsupported_file() {
    use http_body_util::BodyExt;

    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Weixin provider");
    let provider_address = provider_listener.local_addr().expect("provider address");
    let provider_server = tokio::spawn(async move {
        let (stream, _) = provider_listener
            .accept()
            .await
            .expect("accept Weixin send");
        let io = TokioIo::new(stream);
        let handler = service_fn(move |request: Request<Incoming>| async move {
            assert_eq!(request.uri().path(), "/ilink/bot/sendmessage");
            let body = request.into_body().collect().await?.to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&body).expect("Weixin JSON");
            assert_eq!(
                body["msg"]["item_list"][0]["text_item"]["text"],
                "**report**"
            );
            Ok::<_, hyper::Error>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Full::new(Bytes::from_static(
                        br#"{"ret":0,"message_id":"wx-markdown"}"#,
                    )))
                    .expect("Weixin response"),
            )
        });
        http1::Builder::new()
            .keep_alive(false)
            .serve_connection(io, handler)
            .await
            .expect("serve Weixin send");
    });

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = std::sync::Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "weixin-main".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.display_name = "Weixin Main".to_string();
    provider.base_url = Some(format!("http://{provider_address}"));
    provider.app_id = Some("bot@im.bot".to_string());
    provider.secret_ref = Some("bot-token".to_string());
    provider.owner_open_id = Some("wx-owner".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("save Weixin provider");
    service
        .connection_manager
        .weixin_provider()
        .store_context_for_test(&provider, "wx-owner", "context-token")
        .expect("store Weixin context");

    let (address, server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reqwest::Client::new()
            .post(format!("http://{address}/api/im-gateway/messages/send"))
            .header("connection", "close")
            .json(&serde_json::json!({
                "provider_id": "weixin-main",
                "destination": { "mode": "owner" },
                "parts": [
                    { "type": "markdown", "text": "**report**" },
                    { "type": "file", "file_key": "unsupported-file" }
                ],
                "idempotency_key": "weixin-partial"
            }))
            .send(),
    )
    .await
    .expect("Weixin bundle request timed out")
    .expect("send Weixin bundle");
    assert_eq!(response.status(), reqwest::StatusCode::MULTI_STATUS);
    let body: serde_json::Value = response.json().await.expect("bundle response JSON");
    assert_eq!(body["status"], "partial_success");
    assert_eq!(body["receipts"][0]["status"], "success");
    assert_eq!(body["receipts"][0]["requested_kind"], "markdown");
    assert_eq!(body["receipts"][0]["delivered_kind"], "text");
    assert!(body["receipts"][0]["warning"]
        .as_str()
        .is_some_and(|value| value.contains("plain text")));
    assert_eq!(body["receipts"][1]["status"], "failed");
    assert!(body["receipts"][1]["error"]
        .as_str()
        .is_some_and(|value| value.contains("not verified")));

    server.await.expect("gateway server");
    tokio::time::timeout(std::time::Duration::from_secs(5), provider_server)
        .await
        .expect("Weixin provider fixture did not receive the send")
        .expect("Weixin provider server");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn outbound_bundle_uploads_binary_assets_and_sends_ordered_feishu_parts() {
    use http_body_util::BodyExt;

    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    let provider_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(
        String,
        Option<serde_json::Value>,
        usize,
    )>::new()));
    let outbox_failure_path =
        std::sync::Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Feishu provider");
    let provider_address = listener.local_addr().expect("fake provider address");
    let captured = std::sync::Arc::clone(&provider_requests);
    let failure_path = std::sync::Arc::clone(&outbox_failure_path);
    let provider_server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let io = TokioIo::new(stream);
            let captured = std::sync::Arc::clone(&captured);
            let failure_path = std::sync::Arc::clone(&failure_path);
            tokio::spawn(async move {
                let handler = service_fn(move |request: Request<Incoming>| {
                    let captured = std::sync::Arc::clone(&captured);
                    let failure_path = std::sync::Arc::clone(&failure_path);
                    async move {
                        let path_and_query = request
                            .uri()
                            .path_and_query()
                            .map(|value| value.as_str().to_string())
                            .unwrap_or_default();
                        let path = request.uri().path().to_string();
                        let bytes = request.into_body().collect().await?.to_bytes();
                        let json_body = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
                        captured.lock().expect("capture provider request").push((
                            path_and_query,
                            json_body,
                            bytes.len(),
                        ));
                        if path == "/open-apis/im/v1/messages" {
                            if let Some(admin_dir) =
                                failure_path.lock().expect("outbox failure path").take()
                            {
                                std::fs::create_dir(admin_dir.join("im_gateway_outbox.json.tmp"))
                                    .expect("block outbox temporary file");
                            }
                        }
                        let response = match path.as_str() {
                            "/open-apis/auth/v3/tenant_access_token/internal" => {
                                serde_json::json!({
                                    "code": 0,
                                    "tenant_access_token": "tenant-token",
                                    "expire": 7200
                                })
                            }
                            "/open-apis/im/v1/images" => serde_json::json!({
                                "code": 0,
                                "data": { "image_key": "img_uploaded" }
                            }),
                            "/open-apis/im/v1/files" => serde_json::json!({
                                "code": 0,
                                "data": { "file_key": "file_uploaded" }
                            }),
                            "/open-apis/im/v1/messages" => serde_json::json!({
                                "code": 0,
                                "data": { "message_id": format!("om_{}", bytes.len()) }
                            }),
                            _ => serde_json::json!({ "code": 404, "msg": "not found" }),
                        };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(response.to_string())))
                                .expect("fake Feishu response"),
                        )
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, handler).await;
            });
        }
    });

    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let _loopback_guard = EnvVarGuard::set("BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL", "1");
    let service = std::sync::Arc::new(ImGatewayService::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou_owner".to_string());
    provider.secret_ref = Some("test-secret".to_string());
    provider.base_url = Some(format!("http://{provider_address}/open-apis"));
    service
        .provider_store
        .add(provider)
        .expect("save fake Feishu provider");

    let http = reqwest::Client::new();
    let (upload_address, upload_server) =
        spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let image_upload = http
        .post(format!(
            "http://{upload_address}/api/im-gateway/messages/upload?provider_id=feishu-main&kind=image&file_name=chart.png&mime_type=image%2Fpng"
        ))
        .header("connection", "close")
        .body(Vec::from(&b"PNG-DATA"[..]))
        .send()
        .await
        .expect("upload image through gateway");
    let image_status = image_upload.status();
    let image_body = image_upload.text().await.expect("image upload body");
    assert_eq!(
        image_status,
        reqwest::StatusCode::OK,
        "unexpected image upload response: {image_body}"
    );
    let image_upload: serde_json::Value =
        serde_json::from_str(&image_body).expect("image upload JSON");
    assert_eq!(image_upload["key"], "img_uploaded");
    upload_server.await.expect("image upload server");

    let (upload_address, upload_server) =
        spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let file_upload = http
        .post(format!(
            "http://{upload_address}/api/im-gateway/messages/upload?provider_id=feishu-main&kind=file&file_name=report.pdf&mime_type=application%2Fpdf"
        ))
        .header("connection", "close")
        .body(Vec::from(&b"PDF-DATA"[..]))
        .send()
        .await
        .expect("upload file through gateway");
    let file_status = file_upload.status();
    let file_body = file_upload.text().await.expect("file upload body");
    assert_eq!(
        file_status,
        reqwest::StatusCode::OK,
        "unexpected file upload response: {file_body}"
    );
    let file_upload: serde_json::Value =
        serde_json::from_str(&file_body).expect("file upload JSON");
    assert_eq!(file_upload["key"], "file_uploaded");
    upload_server.await.expect("file upload server");

    let bundle_payload = serde_json::json!({
        "provider_id": "feishu-main",
        "destination": {
            "mode": "direct",
            "receive_id_type": "chat_id",
            "receive_id": "oc_engineering"
        },
        "parts": [
            { "type": "text", "text": "first" },
            { "type": "markdown", "text": "**second**" },
            { "type": "image", "image_key": "img_uploaded" },
            { "type": "file", "file_key": "file_uploaded", "file_name": "report.pdf" },
            {
                "type": "native_card",
                "card": {
                    "header": {
                        "title": { "tag": "plain_text", "content": "Final card" }
                    },
                    "elements": []
                }
            }
        ],
        "idempotency_key": "ordered-bundle"
    });
    let (send_address, send_server) = spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let send_response = http
        .post(format!(
            "http://{send_address}/api/im-gateway/messages/send"
        ))
        .header("connection", "close")
        .json(&bundle_payload)
        .send()
        .await
        .expect("send ordered bundle through gateway");
    let send_status = send_response.status();
    let send_body = send_response.text().await.expect("send response body");
    assert_eq!(
        send_status,
        reqwest::StatusCode::OK,
        "unexpected bundle response: {send_body}"
    );
    let send_response: serde_json::Value =
        serde_json::from_str(&send_body).expect("send response JSON");
    assert_eq!(send_response["status"], "success");
    assert_eq!(send_response["destination"], "direct:chat_id:oc_engin***");
    assert_eq!(send_response["receipts"].as_array().map(Vec::len), Some(5));
    send_server.await.expect("bundle send server");

    let (replay_address, replay_server) =
        spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let replay = http
        .post(format!(
            "http://{replay_address}/api/im-gateway/messages/send"
        ))
        .header("connection", "close")
        .json(&bundle_payload)
        .send()
        .await
        .expect("replay ordered bundle");
    assert_eq!(replay.status(), reqwest::StatusCode::OK);
    let replay: serde_json::Value = replay.json().await.expect("replay JSON");
    assert!(replay["receipts"]
        .as_array()
        .expect("replay receipts")
        .iter()
        .all(|receipt| receipt["request_id"] == "idempotent-replay"));
    replay_server.await.expect("replay server");

    let mut conflicting_payload = bundle_payload.clone();
    conflicting_payload["parts"][0]["text"] = serde_json::json!("changed");
    let (conflict_address, conflict_server) =
        spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let conflict = http
        .post(format!(
            "http://{conflict_address}/api/im-gateway/messages/send"
        ))
        .header("connection", "close")
        .json(&conflicting_payload)
        .send()
        .await
        .expect("conflicting ordered bundle");
    assert_eq!(conflict.status(), reqwest::StatusCode::MULTI_STATUS);
    let conflict: serde_json::Value = conflict.json().await.expect("conflict JSON");
    assert_eq!(conflict["status"], "partial_success");
    assert_eq!(conflict["receipts"][0]["status"], "failed");
    conflict_server.await.expect("conflict server");

    *outbox_failure_path.lock().expect("set outbox failure path") =
        Some(temp_dir.path().join("admin"));
    let (failure_address, failure_server) =
        spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let failure = http
        .post(format!(
            "http://{failure_address}/api/im-gateway/messages/send"
        ))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id":"feishu-main",
            "destination":{"mode":"direct","receive_id_type":"chat_id","receive_id":"oc_engineering"},
            "parts":[{"type":"text","text":"ack then fail commit"}],
            "idempotency_key":"outbox-commit-failure"
        }))
        .send()
        .await
        .expect("send outbox commit failure bundle");
    assert_eq!(failure.status(), reqwest::StatusCode::BAD_GATEWAY);
    let failure: serde_json::Value = failure.json().await.expect("outbox failure JSON");
    assert_eq!(failure["receipts"][0]["status"], "failed");
    assert!(failure["receipts"][0]["error"]
        .as_str()
        .is_some_and(|value| value.contains("outbox commit failed")));
    failure_server.await.expect("outbox failure server");

    {
        let captured = provider_requests.lock().expect("provider captures");
        let uploads: Vec<_> = captured
            .iter()
            .filter(|(uri, _, _)| uri.ends_with("/im/v1/images") || uri.ends_with("/im/v1/files"))
            .collect();
        assert_eq!(uploads.len(), 2);
        assert!(uploads.iter().all(|(_, _, body_len)| *body_len > 8));
        let messages: Vec<_> = captured
            .iter()
            .filter(|(uri, _, _)| uri.starts_with("/open-apis/im/v1/messages?"))
            .collect();
        assert_eq!(messages.len(), 6);
        assert!(messages
            .iter()
            .all(|(uri, _, _)| uri.contains("receive_id_type=chat_id")));
        let stable_uuids: std::collections::HashSet<_> = messages
            .iter()
            .map(|(_, body, _)| {
                body.as_ref()
                    .and_then(|value| value["uuid"].as_str())
                    .expect("stable UUID on every bundle part")
            })
            .collect();
        assert_eq!(stable_uuids.len(), 6);
        let message_types: Vec<_> = messages
            .iter()
            .map(|(_, body, _)| {
                body.as_ref().expect("message JSON")["msg_type"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            message_types,
            [
                "interactive",
                "interactive",
                "image",
                "file",
                "interactive",
                "interactive"
            ]
        );
        let final_content = messages[4].1.as_ref().expect("card message JSON")["content"]
            .as_str()
            .expect("serialized card content");
        let final_card: serde_json::Value = serde_json::from_str(final_content).expect("card JSON");
        assert_eq!(final_card["header"]["title"]["content"], "Final card");
    }

    let (legacy_address, legacy_server) =
        spawn_im_gateway_http(std::sync::Arc::clone(&service)).await;
    let legacy_image = http
        .post(format!(
            "http://{legacy_address}/api/im-gateway/messages/send"
        ))
        .header("connection", "close")
        .json(&serde_json::json!({
            "provider_id":"feishu-main",
            "target_id":"__owner__",
            "msg_type":"image",
            "image":{"image_key":"img_uploaded"}
        }))
        .send()
        .await
        .expect("send legacy image-key message");
    assert_eq!(legacy_image.status(), reqwest::StatusCode::OK);
    legacy_server.await.expect("legacy image server");

    provider_server.abort();
}

mod provider_agent_tests;
mod status_query_tests;
mod thread_query_tests;

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
        Arc::clone(&service.group_context_store),
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
            ..Default::default()
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "run external cli".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            files: Vec::new(),
            reply_to: None,
            raw_type: Some("text".to_string()),
            ..Default::default()
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
    assert_eq!(
        runtime_snapshot["reasoningEffort"], "high",
        "session effort override must be reflected without archiving raw CLI args"
    );
    assert!(runtime_snapshot.get("args").is_none());

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
        Arc::clone(&service.group_context_store),
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
            ..Default::default()
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
            files: Vec::new(),
            reply_to: None,
            raw_type: Some("image".to_string()),
            ..Default::default()
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
        Arc::clone(&service.group_context_store),
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
            ..Default::default()
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "trigger broken external cli".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            files: Vec::new(),
            reply_to: None,
            raw_type: Some("text".to_string()),
            ..Default::default()
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
        &service.group_context_store,
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
    let event_for = |event_id: &str,
                     user_id: &str,
                     text: &str,
                     files: Vec<crate::im_gateway::types::ImFileAttachment>|
     -> ImEvent {
        ImEvent {
            event_id: event_id.to_string(),
            provider_id: provider.id.clone(),
            provider_type: ImProviderType::Feishu,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("chat-id".to_string()),
                user_id: Some(user_id.to_string()),
                message_id: Some(format!("message-{event_id}")),
                ..Default::default()
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: text.to_string(),
                mentions: Vec::new(),
                images: Vec::new(),
                files,
                reply_to: None,
                raw_type: Some("text".to_string()),
                ..Default::default()
            }),
            received_at: now_ms(),
            raw_digest: None,
        }
    };

    let active_session_key = build_session_key(&provider.id, Some("owner-open-id"));
    let active_session = service
        .agent_session_manager
        .take_session(&active_session_key);
    let mut active_event = event_for(
        "active",
        "owner-open-id",
        "queue active message",
        Vec::new(),
    );
    let active_message = active_event.message.as_mut().expect("active message");
    active_message.reply_to = Some(crate::im_gateway::types::ImMessageReference {
        message_id: Some("quoted-active-message".to_string()),
        created_at_ms: None,
        text: Some("quoted active context".to_string()),
    });
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
        &service.group_context_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    let active_queue = service.queue_manager.queue_status(&active_session_key);
    assert_eq!(active_queue.len(), 1);
    assert!(active_queue[0]
        .message
        .contains("【引用消息（仅作为上下文）】\nquoted active context"));
    assert!(active_queue[0]
        .message
        .ends_with("【当前消息】\nqueue active message"));
    service.agent_session_manager.return_session(active_session);

    let other_session_key = build_session_key(&provider.id, Some("other-owner"));
    let other_session = service
        .agent_session_manager
        .take_session(&other_session_key);
    let other_event = event_for(
        "other-active",
        "other-owner",
        "queue other active message",
        Vec::new(),
    );
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
        &service.group_context_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    assert_eq!(
        service.queue_manager.queue_status(&other_session_key).len(),
        1
    );
    service.agent_session_manager.return_session(other_session);

    let inactive_event = event_for(
        "inactive",
        "inactive-owner",
        "queue inactive message",
        Vec::new(),
    );
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
        &service.group_context_store,
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
    let file_only_event = event_for(
        "inactive-file",
        "inactive-file-owner",
        " ",
        vec![crate::im_gateway::types::ImFileAttachment {
            file_key: "inline-file".to_string(),
            name: Some("inline.md".to_string()),
            mime_type: Some("text/markdown".to_string()),
            size_bytes: Some(8),
            data_base64: Some("IyBSZXBvcnQ=".to_string()),
            download_url: None,
        }],
    );
    let file_session_key = build_session_key(&provider.id, Some("inactive-file-owner"));
    handle_concurrent_event_during_chat(
        &file_only_event,
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
        &service.group_context_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    let file_queue = service.queue_manager.queue_status(&file_session_key);
    assert_eq!(file_queue.len(), 1);
    assert_eq!(file_queue[0].message, "[附件消息: 1 个]");
    assert_eq!(file_queue[0].files.len(), 1);
    assert_eq!(file_queue[0].files[0].mime_type, "text/markdown");

    let mut group_file_event = event_for(
        "inactive-group-file",
        "group-file-owner",
        "@_user_1 inspect attachment",
        vec![crate::im_gateway::types::ImFileAttachment {
            file_key: "group-inline-file".to_string(),
            name: Some("group.md".to_string()),
            mime_type: Some("text/markdown".to_string()),
            size_bytes: Some(8),
            data_base64: Some("IyBHcm91cA==".to_string()),
            download_url: None,
        }],
    );
    group_file_event.source.chat_id = Some("chat-group-file".to_string());
    group_file_event.source.chat_type = Some("group".to_string());
    let group_message = group_file_event.message.as_mut().unwrap();
    group_message.raw_type = Some("file".to_string());
    group_message.mentions = vec![crate::im_gateway::types::ImMention {
        key: "@_user_1".to_string(),
        open_id: Some("ou_bot".to_string()),
        name: Some("Bifrost".to_string()),
        tenant_key: None,
        is_bot: true,
    }];
    group_message.raw_content = Some(serde_json::json!({
        "text": "@_user_1 inspect attachment",
        "_bifrost_debug_chat_name": "Engineering Files"
    }));
    let group_file_session_key =
        crate::im_gateway::group_context::build_group_session_key(&provider.id, "chat-group-file");
    handle_concurrent_event_during_chat(
        &group_file_event,
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
        &service.group_context_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    let group_file_queue = service.queue_manager.queue_status(&group_file_session_key);
    assert_eq!(group_file_queue.len(), 1);
    assert_eq!(group_file_queue[0].files.len(), 1);
    assert_eq!(
        group_file_queue[0].files[0].name.as_deref(),
        Some("group.md")
    );
    let group_turn_id = group_file_queue[0]
        .context
        .as_ref()
        .and_then(|context| context.group_turn_id.as_deref());
    assert!(group_turn_id.is_some());

    let mut restricted_provider = provider.clone();
    restricted_provider.owner_open_id = Some("owner-open-id".to_string());
    service
        .provider_store
        .update(restricted_provider.clone())
        .expect("restrict provider owner");
    let unauthorized = event_for("unauthorized", "not-owner", "ignored", Vec::new());
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
        &service.group_context_store,
        &service.external_cli_config_store,
        BusyMessageDefaultMode::Queue,
    )
    .await;
    assert!(service
        .queue_manager
        .queue_status(&build_session_key(&provider.id, Some("not-owner")))
        .is_empty());
}
