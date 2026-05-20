use super::*;
use crate::im_gateway::types::ImProviderType;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

pub(super) struct EnvGuard {
    old_data_dir: Option<String>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub(super) fn set_data_dir(data_dir: &std::path::Path) -> Self {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("BIFROST_DATA_DIR test env lock poisoned");
        let old_data_dir = std::env::var("BIFROST_DATA_DIR").ok();
        std::env::set_var("BIFROST_DATA_DIR", data_dir);
        Self {
            old_data_dir,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.old_data_dir.as_deref() {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
    }
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

    let message = build_online_notification_message(&provider);

    assert!(message.starts_with("你好，Bifrost 助手上线了"));
    assert!(message.contains("工作目录：/custom/im-provider-workdir"));
}

#[test]
pub(super) fn online_notification_message_falls_back_to_process_work_dir() {
    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let provider = test_provider();

    let message = build_online_notification_message(&provider);

    assert!(message.starts_with("你好，Bifrost 助手上线了"));
    assert!(message.contains("工作目录："));
    assert!(message.contains(&cwd));
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

    assert_eq!(
        agent_reply_target_id(&provider, &event).as_deref(),
        Some("sender@im.wechat")
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
pub(super) fn agent_reply_target_keeps_feishu_owner_boundary() {
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

    assert_eq!(
        agent_reply_target_id(&provider, &event).as_deref(),
        Some("owner-ou")
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
pub(super) fn schedule_chatgpt_web_initial_prompt_is_sent_as_first_message_only() {
    assert_eq!(
        schedule_external_runner_messages(
            crate::im_gateway::chatgpt_web::ADAPTER_ID,
            Some("INIT_MARKER"),
            false,
            "TASK_MARKER",
        ),
        vec!["INIT_MARKER".to_string(), "TASK_MARKER".to_string()]
    );
    assert_eq!(
        schedule_external_runner_messages(
            crate::im_gateway::chatgpt_web::ADAPTER_ID,
            Some("INIT_MARKER"),
            true,
            "TASK_MARKER",
        ),
        vec!["TASK_MARKER".to_string()]
    );
    assert_eq!(
        schedule_external_runner_messages("mock", Some("INIT_MARKER"), false, "TASK_MARKER"),
        vec!["TASK_MARKER".to_string()]
    );
}

#[test]
pub(super) fn schedule_agent_work_dir_prefers_schedule_then_inherited_default() {
    let mut agent_task = crate::im_gateway::types::ScheduleAgentTask {
        prompt: "TASK".to_string(),
        ..Default::default()
    };

    assert_eq!(
        schedule_agent_effective_work_dir(&agent_task, Some(" /repo/default ")).as_deref(),
        Some("/repo/default")
    );

    agent_task.work_dir = Some(" /repo/schedule ".to_string());
    assert_eq!(
        schedule_agent_effective_work_dir(&agent_task, Some("/repo/default")).as_deref(),
        Some("/repo/schedule")
    );
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_chatgpt_web_session_exists_requires_non_empty_conversation() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let sessions_path = bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join("chatgpt_web")
        .join("sessions.json");
    tokio::fs::create_dir_all(sessions_path.parent().expect("sessions parent"))
        .await
        .expect("create sessions parent");
    tokio::fs::write(
        &sessions_path,
        r#"{"schedule:empty":"  ","schedule:ready":"conv-ready"}"#,
    )
    .await
    .expect("write sessions map");

    assert!(crate::im_gateway::chatgpt_web::session_conversation_exists("schedule:ready").await);
    assert!(!crate::im_gateway::chatgpt_web::session_conversation_exists("schedule:empty").await);
    assert!(!crate::im_gateway::chatgpt_web::session_conversation_exists("schedule:missing").await);
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_agent_can_run_selected_external_runner_with_initial_prompt() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "chatgpt-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "mock".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "input=$(cat); if printf '%s' \"$input\" | grep -q INIT_MARKER && printf '%s' \"$input\" | grep -q TASK_MARKER; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"SCHEDULE_RUNNER_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"SCHEDULE_RUNNER_MISSING_PROMPT\"}'; fi".to_string(),
                ],
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-runner".to_string(),
        name: "Schedule Runner".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("chatgpt-test".to_string()),
            initial_prompt: Some("INIT_MARKER".to_string()),
            session_key: Some("schedule-runner-test".to_string()),
            work_dir: None,
            system_prompt: None,
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-selected-runner".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.runner_id.as_deref(), Some("chatgpt-test"));
    assert_eq!(run.provider_id.as_deref(), Some("feishu-main"));
    assert_eq!(run.target_id.as_deref(), Some("target-main"));
    assert!(run
        .input_preview
        .as_deref()
        .unwrap_or_default()
        .contains("INIT_MARKER"));
    assert!(run
        .input_preview
        .as_deref()
        .unwrap_or_default()
        .contains("TASK_MARKER"));
    assert_eq!(
        run.agent_final_response.as_deref(),
        Some("SCHEDULE_RUNNER_OK")
    );
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_external_runner_executes_from_configured_work_dir() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let work_dir = temp_dir.path().join("runner-workdir");
    std::fs::create_dir_all(&work_dir).expect("create runner workdir");
    let expected_pwd = std::fs::canonicalize(&work_dir)
        .expect("canonical workdir")
        .display()
        .to_string();

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "codex-workdir-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "mock".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "cat >/dev/null; if [ \"$(pwd -P)\" = \"$EXPECTED_PWD\" ]; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"WORKDIR_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"WORKDIR_MISMATCH\"}'; fi".to_string(),
                ],
                env: std::collections::BTreeMap::from([(
                    "EXPECTED_PWD".to_string(),
                    expected_pwd,
                )]),
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-workdir".to_string(),
        name: "Schedule Workdir".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("codex-workdir-test".to_string()),
            initial_prompt: None,
            session_key: Some("schedule-workdir".to_string()),
            work_dir: Some(work_dir.display().to_string()),
            system_prompt: None,
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-workdir".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.agent_final_response.as_deref(), Some("WORKDIR_OK"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_agent_persists_codex_thread_id_for_next_run() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "codex-thread-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "codex".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "cat >/dev/null; printf '%s\n' '{\"type\":\"thread.started\",\"thread_id\":\"thread-schedule-1\"}' '{\"type\":\"assistant_final\",\"content\":\"THREAD_OK\"}'".to_string(),
                ],
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-codex-thread".to_string(),
        name: "Schedule Codex Thread".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("codex-thread-test".to_string()),
            initial_prompt: None,
            session_key: Some("schedule-codex-thread".to_string()),
            work_dir: None,
            system_prompt: None,
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };
    service
        .schedule_store
        .add(schedule.clone())
        .expect("store schedule");

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-codex-thread".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.agent_final_response.as_deref(), Some("THREAD_OK"));
    let stored = service
        .schedule_store
        .get("schedule-codex-thread")
        .expect("stored schedule");
    let conversation_ref = stored
        .agent
        .expect("agent task")
        .conversation_ref
        .expect("conversation ref");
    assert_eq!(conversation_ref.adapter, "codex");
    assert_eq!(
        conversation_ref.thread_id.as_deref(),
        Some("thread-schedule-1")
    );
}

#[test]
pub(super) fn schedule_external_result_extracts_chatgpt_conversation_id() {
    let result = crate::im_gateway::external_cli::ExternalCliRunResult {
        run_id: "run-1".to_string(),
        session_key: Some("schedule:one".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
        exit_code: Some(0),
        response: "ok".to_string(),
        responses: vec!["ok".to_string()],
        started_at: 1,
        finished_at: 2,
        duration_ms: 1,
        artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
            run_dir: "".to_string(),
            prompt: "".to_string(),
            command_snapshot: "".to_string(),
            stdout: "".to_string(),
            stderr: "".to_string(),
            normalized_events: "".to_string(),
            last_message: "".to_string(),
        },
        events: Vec::new(),
        metadata: std::collections::BTreeMap::from([(
            "conversationId".to_string(),
            "conv-schedule-1".to_string(),
        )]),
    };

    let conversation_ref = schedule_conversation_ref_from_external_result(
        crate::im_gateway::chatgpt_web::ADAPTER_ID,
        &result,
    )
    .expect("conversation ref");

    assert_eq!(
        conversation_ref.conversation_id.as_deref(),
        Some("conv-schedule-1")
    );
    assert_eq!(
        conversation_ref.adapter,
        crate::im_gateway::chatgpt_web::ADAPTER_ID
    );
}

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

#[test]
pub(super) fn provider_agent_config_patch_sets_and_clears_overrides() {
    let mut provider = test_provider();

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "agent_config": {
                "runner": "codex",
                "work_dir": " /tmp/bifrost-im ",
                "base_instructions": " Provider prompt "
            }
        }),
    );

    let agent_config = provider.agent_config.as_ref().expect("agent_config");
    assert_eq!(
        agent_config.runner,
        Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()))
    );
    assert_eq!(agent_config.work_dir.as_deref(), Some("/tmp/bifrost-im"));
    assert_eq!(
        agent_config.base_instructions.as_deref(),
        Some("Provider prompt")
    );

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "agent_config": {
                "runner": null,
                "work_dir": null,
                "base_instructions": ""
            }
        }),
    );

    assert!(provider.agent_config.is_none());
}

#[test]
pub(super) fn provider_create_payload_maps_app_secret_without_exposing_it() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "feishu-main",
        "provider_type": "feishu",
        "display_name": "Feishu Main",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should parse");

    assert_eq!(provider.secret_ref.as_deref(), Some("sk_test_secret"));
    assert_eq!(provider.display_name, "Feishu Main");

    let safe = sanitize_provider(&provider);
    assert_eq!(
        safe.get("secret_configured").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(safe.get("secret_ref").is_none());
    assert!(safe.get("app_secret").is_none());
    assert!(!safe.to_string().contains("sk_test_secret"));
}

#[test]
pub(super) fn provider_create_payload_defaults_missing_display_name_to_id() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "feishu-main",
        "provider_type": "feishu",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should default display_name");

    assert_eq!(provider.display_name, "feishu-main");
    assert_eq!(provider.secret_ref.as_deref(), Some("sk_test_secret"));
}

#[test]
pub(super) fn provider_agent_config_overrides_base_agent_config() {
    let base = crate::im_gateway::agent::ImAgentConfig {
        work_dir: Some("/global".to_string()),
        base_instructions: Some("global prompt".to_string()),
        developer_instructions: Some("global developer".to_string()),
        user_instructions: Some("global user".to_string()),
        ..Default::default()
    };

    let mut provider = test_provider();
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string())),
        work_dir: Some("/provider".to_string()),
        base_instructions: Some("provider prompt".to_string()),
        developer_instructions: Some("provider developer".to_string()),
        user_instructions: Some("provider user".to_string()),
    });

    let effective = effective_agent_config_for_provider(&base, &provider);
    assert_eq!(
        effective.runner,
        Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()))
    );
    assert_eq!(effective.work_dir.as_deref(), Some("/provider"));
    assert_eq!(
        effective.base_instructions.as_deref(),
        Some("provider prompt")
    );
    assert_eq!(
        effective.developer_instructions.as_deref(),
        Some("provider developer")
    );
    assert_eq!(
        effective.user_instructions.as_deref(),
        Some("provider user")
    );
}

#[test]
pub(super) fn provider_switch_workdir_persists_provider_agent_override() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let store = Arc::new(ImProviderStore::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "persist-workdir-provider".to_string();
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: None,
        work_dir: Some("/old".to_string()),
        base_instructions: Some("keep provider prompt".to_string()),
        developer_instructions: None,
        user_instructions: None,
    });
    store.add(provider).expect("add provider");

    persist_provider_agent_work_dir(&store, "persist-workdir-provider", " /new/workdir ");

    let updated = store.get("persist-workdir-provider").expect("provider");
    let agent_config = updated.agent_config.expect("agent_config");
    assert_eq!(agent_config.work_dir.as_deref(), Some("/new/workdir"));
    assert_eq!(
        agent_config.base_instructions.as_deref(),
        Some("keep provider prompt")
    );
}

#[test]
pub(super) fn agent_api_status_detail_applies_work_dir_for_fresh_status_session() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);

    let detail = resolve_agent_api_status_detail(
        &manager,
        "status-fresh-workdir",
        Some("/tmp/bifrost-status-workdir".to_string()),
    )
    .expect("requested work_dir should create status detail");

    assert_eq!(
        detail.work_dir.as_deref(),
        Some("/tmp/bifrost-status-workdir")
    );
    assert_eq!(detail.message_count, 0);
}

#[test]
pub(super) fn agent_api_status_detail_overrides_existing_idle_session_work_dir() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let session = manager
        .try_take_session_with_work_dir("status-existing-workdir", Some("/tmp/old".to_string()))
        .expect("initial session should be available");
    manager.return_session(session);

    let detail = resolve_agent_api_status_detail(
        &manager,
        "status-existing-workdir",
        Some("/tmp/new".to_string()),
    )
    .expect("existing status detail should remain available");

    assert_eq!(detail.work_dir.as_deref(), Some("/tmp/new"));
}

#[test]
pub(super) fn agent_api_status_detail_keeps_new_session_text_when_no_work_dir_requested() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);

    let detail = resolve_agent_api_status_detail(&manager, "status-no-workdir", None);

    assert!(detail.is_none());
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_uses_provider_agent_config_for_agent_chat() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
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

    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
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
        .get_mut("codex")
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

    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
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
    assert!(events.iter().any(|event| event.event_type == "tool_call"
        && event
            .content
            .get("tool_name")
            .and_then(|value| value.as_str())
            == Some("mock")));
    assert!(events.iter().any(|event| event.event_type == "tool_result"
        && event
            .content
            .get("success")
            .and_then(|value| value.as_bool())
            == Some(true)));
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

    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
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
    assert!(events.iter().any(|event| event.event_type == "tool_result"
        && event
            .content
            .get("success")
            .and_then(|value| value.as_bool())
            == Some(false)
        && event
            .content
            .get("result")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value.contains("spawn external cli failed"))));
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

    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("event loop timed out")
        .expect("event loop task panicked");

    let requests = mock.requests.lock().expect("requests lock");
    let request = requests.first().expect("mock received chat request");
    assert!(request_messages_contain(request, IMAGE_ONLY_AGENT_PROMPT));
    assert!(request_contains_image_url(request));
    assert_eq!(
        request_image_url_count(request),
        MAX_AGENT_IMAGES_PER_MESSAGE
    );
}
