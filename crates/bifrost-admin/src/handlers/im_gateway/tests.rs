use super::*;
use crate::im_gateway::types::ImProviderType;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
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
    assert!(message.contains("- **Runner Type**: `bifrost_agent`"));
    assert!(message.contains("- **Runner ID**: `N/A`"));
    assert!(message.contains("- **Model**: `N/A`"));
    assert!(message.contains("- **Reasoning Effort**: `N/A`"));
    assert!(message.contains("- **Reasoning Summary**: `N/A`"));
    assert!(message.contains("- **Bound Session**: `N/A`"));
    assert!(message.contains("- **Completed User Turns**: 0"));
    assert!(message.contains("- **Status**: Ready"));
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
          "model": "sonnet",
          "effortLevel": "low",
          "env": {
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-opus-4-7",
            "CLAUDE_CODE_EFFORT_LEVEL": "high"
          }
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
    assert_eq!(context.model, None);
    assert_eq!(context.model_provider, None);
    assert_eq!(context.model_reasoning_effort.as_deref(), Some("high"));
    assert!(message.contains("- **Model**: `N/A`"));
    assert!(message.contains("- **Reasoning Effort**: `high`"));
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
        raw_type: None,
    };
    assert!(agent_message_text(&empty_message).is_empty());
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
        raw_type: Some("image".to_string()),
    };
    assert_eq!(inbound_message_preview(&image_message), "[图片消息: 2 张]");

    let long_text = "中".repeat(240);
    let text_message = crate::im_gateway::types::ImEventMessage {
        text: long_text,
        mentions: Vec::new(),
        images: Vec::new(),
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
        agent_type: Some("Bifrost Agent".to_string()),
        runner_type: Some("bifrost_agent".to_string()),
        runner_id: None,
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

#[test]
pub(super) fn start_notice_is_plain_weixin_only_without_progress_card() {
    let mut provider = test_provider();
    provider.provider_type = ImProviderType::Weixin;

    assert!(should_send_plain_im_task_start_notice(&provider, false));
    assert!(!should_send_plain_im_task_start_notice(&provider, true));

    provider.provider_type = ImProviderType::Feishu;
    assert!(!should_send_plain_im_task_start_notice(&provider, false));
}

pub(super) struct TestChatCompletionMock {
    port: u16,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl TestChatCompletionMock {
    pub(super) async fn start() -> Self {
        Self::start_with_content("IM_PROVIDER_CONFIG_OK").await
    }

    pub(super) async fn start_with_content(content: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock chat server");
        let port = listener.local_addr().expect("mock local addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_server = Arc::clone(&requests);
        let content = content.to_string();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let requests = Arc::clone(&requests_for_server);
                let content = content.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let requests = Arc::clone(&requests);
                        let content = content.clone();
                        async move {
                            let body_bytes = req
                                .into_body()
                                .collect()
                                .await
                                .map(|body| body.to_bytes())
                                .unwrap_or_else(|_| Bytes::new());
                            let body: serde_json::Value =
                                serde_json::from_slice(&body_bytes).unwrap_or_default();
                            requests.lock().expect("requests lock").push(body);
                            let response = serde_json::json!({
                                "choices": [{
                                    "message": {
                                        "role": "assistant",
                                        "content": content
                                    },
                                    "finish_reason": "stop"
                                }],
                                "usage": {
                                    "prompt_tokens": 10,
                                    "completion_tokens": 4,
                                    "total_tokens": 14
                                }
                            });
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("Content-Type", "application/json")
                                    .body(Full::new(Bytes::from(response.to_string())))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        Self { port, requests }
    }

    pub(super) fn url(&self) -> String {
        format!("http://127.0.0.1:{}/chat/completions", self.port)
    }
}

pub(super) fn request_messages_contain(body: &serde_json::Value, needle: &str) -> bool {
    body.get("messages")
        .and_then(|messages| messages.as_array())
        .map(|messages| {
            messages.iter().any(|message| {
                let Some(content) = message.get("content") else {
                    return false;
                };
                if let Some(text) = content.as_str() {
                    return text.contains(needle);
                }
                content
                    .as_array()
                    .map(|parts| {
                        parts.iter().any(|part| {
                            part.get("text")
                                .and_then(|value| value.as_str())
                                .map(|text| text.contains(needle))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub(super) fn request_contains_image_url(body: &serde_json::Value) -> bool {
    request_image_url_count(body) > 0
}

pub(super) fn request_image_url_count(body: &serde_json::Value) -> usize {
    body.get("messages")
        .and_then(|messages| messages.as_array())
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    message
                        .get("content")
                        .and_then(|content| content.as_array())
                        .map(|parts| {
                            parts
                                .iter()
                                .filter(|part| {
                                    part.get("type").and_then(|value| value.as_str())
                                        == Some("image_url")
                                        && part
                                            .pointer("/image_url/url")
                                            .and_then(|value| value.as_str())
                                            .is_some_and(|url| {
                                                url.starts_with("data:image/png;base64,")
                                            })
                                })
                                .count()
                        })
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

pub(super) fn request_message_role(body: &serde_json::Value, idx: usize) -> Option<&str> {
    body.get("messages")?
        .as_array()?
        .get(idx)?
        .get("role")?
        .as_str()
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
pub(super) async fn im_event_loop_uses_provider_agent_config_for_agent_chat() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let _worker_env_lock = crate::test_env::agent_worker_env_lock().lock().await;
    let _force_worker_guard = EnvVarGuard::remove("BIFROST_FORCE_AGENT_WORKER");
    let mock = TestChatCompletionMock::start().await;
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    base_config.model = Some("mock-model".to_string());
    base_config.model_provider = Some("mock".to_string());
    base_config.work_dir = Some(std::env::current_dir().unwrap().display().to_string());
    base_config.base_instructions = Some("GLOBAL_BASE_SHOULD_NOT_APPEAR".to_string());
    base_config.developer_instructions = Some("GLOBAL_DEV_SHOULD_NOT_APPEAR".to_string());
    base_config.user_instructions = Some("GLOBAL_USER_SHOULD_NOT_APPEAR".to_string());
    base_config.max_turn_iterations = Some(1);
    base_config.model_providers.insert(
        "mock".to_string(),
        bifrost_agent::config::ModelProviderConfig {
            name: Some("Mock".to_string()),
            base_url: Some(mock.url()),
            wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
            env_key: None,
            api_key: None,
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer test".to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    let mut provider = test_provider();
    provider.id = "new-im-provider-config".to_string();
    provider.owner_open_id = Some("owner-open-id".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    let mut provider_in_store = provider.clone();
    provider_in_store.agent_config = Some(ImProviderAgentConfig {
        runner: None,
        work_dir: Some(std::env::current_dir().unwrap().display().to_string()),
        base_instructions: Some("IM_PROVIDER_BASE_OK: answer IM_PROVIDER_CONFIG_OK".to_string()),
        developer_instructions: Some("IM_PROVIDER_DEV_OK".to_string()),
        user_instructions: Some("IM_PROVIDER_USER_OK".to_string()),
    });
    service
        .provider_store
        .add(provider_in_store)
        .expect("add current provider config to store");

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
        Arc::clone(&service.agent_client),
        Arc::clone(&service.agent_tools),
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
        event_id: "evt-im-provider-agent-config".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: None,
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "IM_PROVIDER_CHAT_MARKER 请只回复 IM_PROVIDER_CONFIG_OK".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
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

    let requests = mock.requests.lock().expect("requests lock");
    let request = requests.first().expect("mock received chat request");
    assert_eq!(request_message_role(request, 0), Some("system"));
    assert_eq!(request_message_role(request, 1), Some("developer"));
    assert_eq!(request_message_role(request, 2), Some("user"));
    assert!(request_messages_contain(request, "IM_PROVIDER_BASE_OK"));
    assert!(request_messages_contain(request, "IM_PROVIDER_DEV_OK"));
    assert!(request_messages_contain(request, "IM_PROVIDER_USER_OK"));
    assert!(request_messages_contain(request, "IM_PROVIDER_CHAT_MARKER"));
    assert!(!request_messages_contain(
        request,
        "GLOBAL_BASE_SHOULD_NOT_APPEAR"
    ));
    assert!(!request_messages_contain(
        request,
        "GLOBAL_DEV_SHOULD_NOT_APPEAR"
    ));
    assert!(!request_messages_contain(
        request,
        "GLOBAL_USER_SHOULD_NOT_APPEAR"
    ));
}

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
    let runner = external_cli_config
        .runners
        .get_mut(crate::im_gateway::external_cli::DEFAULT_CODEX_RUNNER_ID)
        .expect("default codex runner");
    runner.enabled = false;
    runner.adapter = "mock".to_string();
    runner.inject_bifrost_tools = false;
    runner.adapter_config =
        crate::im_gateway::external_cli::ExternalCliAdapterConfig {
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "cat >/dev/null; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"EXTERNAL_RUNNER_OK\"}'".to_string(),
            ],
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
        Arc::clone(&service.agent_client),
        Arc::clone(&service.agent_tools),
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
    for entry in std::fs::read_dir(runs_root).expect("runs dir") {
        let result_path = entry.expect("run dir").path().join("result.json");
        if !result_path.exists() {
            continue;
        }
        let result = std::fs::read_to_string(result_path).expect("result json");
        if result.contains("EXTERNAL_RUNNER_OK") {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "external runner should execute even when defaults.enabled is false"
    );

    let session_key = build_session_key(&provider.id, Some("owner-open-id"));
    let detail = service
        .agent_session_manager
        .get_session_detail(&session_key)
        .expect("external runner session detail should be visible in WebUI");
    assert_eq!(detail.source, "mock");
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
                == Some("mock")));
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
        Arc::clone(&service.agent_client),
        Arc::clone(&service.agent_tools),
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

#[tokio::test(flavor = "current_thread")]
pub(super) async fn agent_chat_final_reply_sends_local_markdown_images_as_im_images() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let _worker_env_lock = crate::test_env::agent_worker_env_lock().lock().await;
    let _force_worker_guard = EnvVarGuard::remove("BIFROST_FORCE_AGENT_WORKER");
    let image_path = temp_dir.path().join("chatgpt-web-image-1.png");
    std::fs::write(&image_path, b"fake png bytes").expect("write image");
    let response = format!(
        "已生成图片，正在发送原图。\n\n![ChatGPT 生成图片 1]({})\n\n正文继续。",
        image_path.display()
    );
    let mock = TestChatCompletionMock::start_with_content(&response).await;
    let service = ImGatewayService::new(temp_dir.path());

    let mut agent_config = service.agent_config_store.load();
    agent_config.enabled = true;
    agent_config.model = Some("mock-model".to_string());
    agent_config.model_provider = Some("mock".to_string());
    agent_config.work_dir = Some(temp_dir.path().display().to_string());
    agent_config.max_turn_iterations = Some(1);
    agent_config.model_providers.insert(
        "mock".to_string(),
        bifrost_agent::config::ModelProviderConfig {
            name: Some("Mock".to_string()),
            base_url: Some(mock.url()),
            wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
            env_key: None,
            api_key: None,
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer test".to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );

    let mut provider = test_provider();
    provider.id = "weixin-image-reply-provider".to_string();
    provider.provider_type = ImProviderType::Weixin;
    provider.owner_open_id = Some("owner@im.wechat".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    provider.secret_ref = Some("test-token".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");

    let event = ImEvent {
        event_id: "evt-weixin-generated-image".to_string(),
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

    process_agent_chat(
        &ImProviderClient::Weixin(Arc::clone(service.connection_manager.weixin_provider())),
        &provider,
        &service.provider_store,
        &event,
        &service.agent_client,
        &agent_config,
        &service.agent_tools,
        &service.schedule_store,
        &service.scheduler,
        &service.target_store,
        &service.connection_manager,
        &service.agent_session_manager,
        &service.progress_registry,
        &service.queue_manager,
        "weixin:image-reply-test",
        "生成图片",
        &[],
        None,
        None,
        &service.message_log_store,
        None,
    )
    .await;

    let logs = service.message_log_store.list_by_provider(&provider.id);
    assert!(logs.iter().any(|log| {
        log.msg_type.as_deref() == Some("image")
            && log
                .content_preview
                .as_deref()
                .is_some_and(|preview| preview.contains("ChatGPT 生成图片 1"))
    }));
    assert!(logs.iter().any(|log| {
        log.msg_type.as_deref() == Some("interactive")
            && log
                .content_preview
                .as_deref()
                .is_some_and(|preview| !preview.contains("chatgpt-web-image-1.png"))
    }));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_forwards_image_attachment_to_agent_chat() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let _worker_env_lock = crate::test_env::agent_worker_env_lock().lock().await;
    let _force_worker_guard = EnvVarGuard::remove("BIFROST_FORCE_AGENT_WORKER");
    let mock = TestChatCompletionMock::start().await;
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    base_config.model = Some("mock-vision-model".to_string());
    base_config.model_provider = Some("mock".to_string());
    base_config.work_dir = Some(std::env::current_dir().unwrap().display().to_string());
    base_config.max_turn_iterations = Some(1);
    base_config.model_providers.insert(
        "mock".to_string(),
        bifrost_agent::config::ModelProviderConfig {
            name: Some("Mock".to_string()),
            base_url: Some(mock.url()),
            wire_api: Some(bifrost_agent::config::ModelWireApi::ChatCompletions),
            env_key: None,
            api_key: None,
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer test".to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    let mut provider = test_provider();
    provider.id = "image-provider".to_string();
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
        Arc::clone(&service.agent_client),
        Arc::clone(&service.agent_tools),
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
        event_id: "evt-im-image-agent-chat".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: Some("om-image".to_string()),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "".to_string(),
            mentions: Vec::new(),
            images: (0..7)
                .map(|idx| crate::im_gateway::types::ImImageAttachment {
                    file_key: format!("img-unit-{idx}"),
                    source: crate::im_gateway::types::ImImageSource::MessageResource,
                    mime_type: Some("image/png".to_string()),
                    data_base64: Some("iVBORw0KGgo=".to_string()),
                    download_url: None,
                    encrypted_query_param: None,
                    aes_key: None,
                })
                .collect(),
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

    let requests = mock.requests.lock().expect("requests lock");
    if let Some(request) = requests.first() {
        assert!(request_messages_contain(request, IMAGE_ONLY_AGENT_PROMPT));
        assert!(request_contains_image_url(request));
        assert_eq!(
            request_image_url_count(request),
            MAX_AGENT_IMAGES_PER_MESSAGE
        );
    }
}
