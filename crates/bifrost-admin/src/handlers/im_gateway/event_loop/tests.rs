use super::*;

#[tokio::test]
async fn initial_external_attachments_are_deferred_for_weixin_and_empty_without_message() {
    let temp = tempfile::tempdir().expect("initial attachment data dir");
    let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
    let client = ImProviderClient::Weixin(Arc::clone(service.connection_manager.weixin_provider()));
    let mut provider = crate::handlers::im_gateway::tests::test_provider();
    provider.id = "weixin-main".to_string();
    provider.provider_type = ImProviderType::Weixin;
    let mut event = group_test_event("weixin-initial", "message", "caption", false, 1);

    let (images, files) =
        resolve_initial_external_cli_attachments(&client, &provider, &event).await;
    assert!(images.is_empty());
    assert!(files.is_empty());

    provider.provider_type = ImProviderType::Feishu;
    event.message = None;
    let (images, files) =
        resolve_initial_external_cli_attachments(&client, &provider, &event).await;
    assert!(images.is_empty());
    assert!(files.is_empty());
}

#[test]
fn queued_event_restores_the_triggering_reply_target() {
    let base = group_test_event("feishu-queue", "m1", "first", false, 1);
    let context = crate::im_gateway::queue_manager::QueueItemContext {
        event_id: "event-m2".to_string(),
        message_id: Some("m2".to_string()),
        user_id: Some("ou_second".to_string()),
        user_name: Some("Bob".to_string()),
        group_turn_id: Some("turn-m2".to_string()),
    };

    let queued = event_for_queue_item(&base, Some(&context));
    assert_eq!(queued.event_id, "event-m2");
    assert_eq!(queued.source.message_id.as_deref(), Some("m2"));
    assert_eq!(queued.source.user_id.as_deref(), Some("ou_second"));
    assert_eq!(queued.source.user_name.as_deref(), Some("Bob"));
    assert_eq!(queued.source.chat_id, base.source.chat_id);

    let unchanged = event_for_queue_item(&base, None);
    assert_eq!(unchanged.event_id, base.event_id);
    let empty_event_id = crate::im_gateway::queue_manager::QueueItemContext {
        event_id: String::new(),
        ..context
    };
    let unchanged_id = event_for_queue_item(&base, Some(&empty_event_id));
    assert_eq!(unchanged_id.event_id, base.event_id);
}

#[test]
fn group_session_work_dir_overrides_runner_and_provider_defaults() {
    let mut request: crate::im_gateway::external_cli::ExternalCliRunRequest =
        serde_json::from_value(serde_json::json!({
            "message": "inspect",
            "workDir": "/runner/default"
        }))
        .unwrap();

    apply_session_bound_work_dir(
        &mut request,
        Some("/group/bound"),
        Some(std::path::PathBuf::from("/provider/default")),
    );

    assert_eq!(
        request.work_dir.as_deref(),
        Some(std::path::Path::new("/group/bound"))
    );

    let mut runner_request = recorder_test_request("runner-workdir");
    runner_request.work_dir = Some(std::path::PathBuf::from("/runner/default"));
    apply_session_bound_work_dir(
        &mut runner_request,
        None,
        Some(std::path::PathBuf::from("/provider/default")),
    );
    assert_eq!(
        runner_request.work_dir.as_deref(),
        Some(std::path::Path::new("/runner/default"))
    );

    let mut provider_request = recorder_test_request("provider-workdir");
    apply_session_bound_work_dir(
        &mut provider_request,
        None,
        Some(std::path::PathBuf::from("/provider/default")),
    );
    assert_eq!(
        provider_request.work_dir.as_deref(),
        Some(std::path::Path::new("/provider/default"))
    );
}

#[test]
fn referenced_attachments_are_prepended_before_current_message_attachments() {
    let mut event = group_test_event("feishu-order", "trigger", "inspect", true, 1);
    let message = event.message.as_mut().unwrap();
    message
        .images
        .push(crate::im_gateway::types::ImImageAttachment {
            file_key: "current-image".to_string(),
            source: Default::default(),
            mime_type: None,
            data_base64: None,
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        });
    message
        .files
        .push(crate::im_gateway::types::ImFileAttachment {
            file_key: "current-file".to_string(),
            name: Some("current.txt".to_string()),
            mime_type: None,
            size_bytes: None,
            data_base64: None,
            download_url: None,
            ..Default::default()
        });
    let dispatch = PreparedInboundDispatch {
        message_text: "inspect".to_string(),
        session_key: "session".to_string(),
        group_turn_id: None,
        reset_group_context: false,
        direct_reply: None,
        thread_anchor_message_id: None,
        thread_fallback_message: None,
        referenced_images: vec![crate::im_gateway::types::ImImageAttachment {
            file_key: "quoted-image".to_string(),
            source: Default::default(),
            mime_type: None,
            data_base64: None,
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        }],
        referenced_files: vec![crate::im_gateway::types::ImFileAttachment {
            file_key: "quoted-file".to_string(),
            name: Some("quoted.txt".to_string()),
            mime_type: None,
            size_bytes: None,
            data_base64: None,
            download_url: None,
            ..Default::default()
        }],
        attachment_notices: Vec::new(),
    };

    prepend_referenced_attachments(&mut event, &dispatch);

    let message = event.message.as_ref().unwrap();
    assert_eq!(
        message
            .images
            .iter()
            .map(|image| image.file_key.as_str())
            .collect::<Vec<_>>(),
        ["quoted-image", "current-image"]
    );
    assert_eq!(
        message
            .files
            .iter()
            .map(|file| file.file_key.as_str())
            .collect::<Vec<_>>(),
        ["quoted-file", "current-file"]
    );

    let mut event_without_message = event.clone();
    event_without_message.message = None;
    prepend_referenced_attachments(&mut event_without_message, &dispatch);
    assert!(event_without_message.message.is_none());
    assert!(referenced_attachment_prompt_note(1, 1).contains("1 张图片和 1 个文件"));
    assert_eq!(
        attachment_notice_message(&["文件过大；已跳过".to_string()]),
        "附件处理提示（不影响任务继续执行）：\n- 文件过大；已跳过"
    );
}

#[tokio::test]
async fn referenced_attachment_hydration_truncates_preloaded_payloads_and_checks_file_size() {
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let provider = recorder_test_provider();
    let images = (0..=MAX_AGENT_ATTACHMENTS_PER_MESSAGE)
        .map(|index| crate::im_gateway::types::ImImageAttachment {
            file_key: format!("image-{index}"),
            source: Default::default(),
            mime_type: Some("image/png".to_string()),
            data_base64: Some("aW1hZ2U=".to_string()),
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        })
        .collect();
    let files = (0..=MAX_AGENT_ATTACHMENTS_PER_MESSAGE)
        .map(|index| crate::im_gateway::types::ImFileAttachment {
            file_key: format!("file-{index}"),
            name: Some(format!("file-{index}.txt")),
            mime_type: Some("text/plain".to_string()),
            size_bytes: Some(4),
            data_base64: Some("ZmlsZQ==".to_string()),
            download_url: None,
            ..Default::default()
        })
        .collect();
    let (images, files, notices) = hydrate_referenced_group_attachments(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-preloaded".to_string(),
            images,
            files,
        },
    )
    .await;
    assert_eq!(images.len(), MAX_AGENT_ATTACHMENTS_PER_MESSAGE);
    assert_eq!(files.len(), MAX_AGENT_ATTACHMENTS_PER_MESSAGE);
    assert_eq!(notices.len(), 2);

    let (images, files, notices) = hydrate_referenced_group_attachments(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-oversize".to_string(),
            images: Vec::new(),
            files: vec![crate::im_gateway::types::ImFileAttachment {
                file_key: "oversize".to_string(),
                name: Some("oversize.bin".to_string()),
                mime_type: None,
                size_bytes: Some(MAX_FEISHU_REFERENCED_FILE_BYTES + 1),
                data_base64: None,
                download_url: None,
                ..Default::default()
            }],
        },
    )
    .await;
    assert!(images.is_empty());
    assert!(files.is_empty());
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("100 MiB 上限"));

    let files = (0..3)
        .map(|index| crate::im_gateway::types::ImFileAttachment {
            file_key: format!("budget-{index}"),
            name: Some(format!("budget-{index}.bin")),
            mime_type: Some("application/octet-stream".to_string()),
            size_bytes: Some(MAX_FEISHU_REFERENCED_FILE_BYTES),
            data_base64: Some("AA==".to_string()),
            download_url: None,
            ..Default::default()
        })
        .collect();
    let (_, files, notices) = hydrate_referenced_group_attachments(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-total-budget".to_string(),
            images: Vec::new(),
            files,
        },
    )
    .await;
    assert_eq!(files.len(), 3);
    assert!(notices.is_empty());
    assert!(files.iter().all(|file| file.size_bytes == Some(1)));
    assert!(!referenced_file_budget_exceeded(
        2 * MAX_FEISHU_REFERENCED_FILE_BYTES,
        50 * 1024 * 1024
    ));
    assert!(referenced_file_budget_exceeded(
        2 * MAX_FEISHU_REFERENCED_FILE_BYTES,
        50 * 1024 * 1024 + 1
    ));

    let (_, files, notices) = hydrate_referenced_group_attachments(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-invalid-base64".to_string(),
            images: vec![crate::im_gateway::types::ImImageAttachment {
                file_key: "invalid-image".to_string(),
                source: Default::default(),
                mime_type: Some("image/png".to_string()),
                data_base64: Some("not base64".to_string()),
                download_url: None,
                encrypted_query_param: None,
                aes_key: None,
            }],
            files: vec![crate::im_gateway::types::ImFileAttachment {
                file_key: "invalid-file".to_string(),
                name: Some("invalid.bin".to_string()),
                mime_type: None,
                size_bytes: None,
                data_base64: Some("also not base64".to_string()),
                download_url: None,
                ..Default::default()
            }],
        },
    )
    .await;
    assert!(files.is_empty());
    assert_eq!(notices.len(), 2);
    assert!(notices
        .iter()
        .all(|notice| notice.contains("不是有效 Base64")));
}

async fn spawn_oversized_referenced_resource_server() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = vec![0u8; 8192];
                let Ok(length) = stream.read(&mut request).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&request[..length]);
                let response = if request.contains("/auth/v3/tenant_access_token/internal") {
                    let body = r#"{"code":0,"tenant_access_token":"token","expire":7200}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    )
                } else if request.contains("/resources/oversized-image") {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_AGENT_REPLY_IMAGE_BYTES + 1
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_FEISHU_REFERENCED_FILE_BYTES + 1
                    )
                };
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}/open-apis"), task)
}

#[tokio::test]
async fn referenced_attachment_hydration_rejects_oversized_downloaded_payloads() {
    let (base_url, server) = spawn_oversized_referenced_resource_server().await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.base_url = Some(base_url);

    let (images, files, notices) = hydrate_referenced_group_attachments(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-oversized-image".to_string(),
            images: vec![crate::im_gateway::types::ImImageAttachment {
                file_key: "oversized-image".to_string(),
                source: Default::default(),
                mime_type: None,
                data_base64: None,
                download_url: None,
                encrypted_query_param: None,
                aes_key: None,
            }],
            files: Vec::new(),
        },
    )
    .await;
    assert!(images.is_empty());
    assert!(files.is_empty());
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("超过 10 MiB 上限"));

    let (images, files, notices) = hydrate_referenced_group_attachments(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-oversized-file".to_string(),
            images: Vec::new(),
            files: vec![crate::im_gateway::types::ImFileAttachment {
                file_key: "oversized-file".to_string(),
                name: Some("oversized.bin".to_string()),
                mime_type: None,
                size_bytes: None,
                data_base64: None,
                download_url: None,
                ..Default::default()
            }],
        },
    )
    .await;
    assert!(images.is_empty());
    assert!(files.is_empty());
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("100 MiB 上限"));
    server.abort();
}

async fn spawn_referenced_resource_branch_server() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = vec![0u8; 8192];
                let Ok(length) = stream.read(&mut request).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&request[..length]);
                let (status, content_type, body): (&str, &str, &[u8]) =
                    if request.contains("/auth/v3/tenant_access_token/internal") {
                        (
                            "200 OK",
                            "application/json",
                            br#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                        )
                    } else if request.contains("fail") {
                        ("500 Internal Server Error", "text/plain", b"failed")
                    } else if request.contains("image-ok") {
                        ("200 OK", "image/png", b"img4")
                    } else if request.contains("image-too-big") {
                        ("200 OK", "image/png", b"image")
                    } else if request.contains("file-too-big") {
                        ("200 OK", "application/octet-stream", b"12345")
                    } else {
                        ("200 OK", "application/octet-stream", b"abc")
                    };
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes()).await;
                let _ = stream.write_all(body).await;
            });
        }
    });
    (format!("http://{address}/open-apis"), task)
}

#[tokio::test]
async fn referenced_attachment_hydration_covers_small_limit_error_matrix() {
    let (base_url, server) = spawn_referenced_resource_branch_server().await;
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let mut provider = recorder_test_provider();
    provider.base_url = Some(base_url);
    let limits: ReferencedAttachmentLimits = [6, 4, 4, 5];

    let image = |key: &str, data_base64: Option<&str>| ImImageAttachment {
        file_key: key.to_string(),
        source: Default::default(),
        mime_type: None,
        data_base64: data_base64.map(ToString::to_string),
        download_url: None,
        encrypted_query_param: None,
        aes_key: None,
    };
    let file = |key: &str, size_bytes: Option<u64>, data_base64: Option<&str>| ImFileAttachment {
        file_key: key.to_string(),
        name: Some(format!("{key}.bin")),
        mime_type: None,
        size_bytes,
        data_base64: data_base64.map(ToString::to_string),
        download_url: None,
        ..Default::default()
    };

    let (images, files, notices) = hydrate_referenced_group_attachments_with_limits(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-small-limits".to_string(),
            images: vec![
                image("preloaded-ok", Some("aW1nNA==")),
                image("preloaded-too-big", Some("aW1hZ2U=")),
                image("preloaded-invalid", Some("?")),
                image("image-ok", None),
                image("image-too-big", None),
                image("image-fail", None),
            ],
            files: vec![
                file("preloaded-file", None, Some("YWJj")),
                file("budget-preflight", Some(3), None),
                file("metadata-too-big", Some(5), None),
                file("invalid-file", None, Some("?")),
                file("file-ok", None, None),
                file("file-too-big", None, None),
            ],
        },
        limits,
    )
    .await;

    assert_eq!(
        images
            .iter()
            .map(|image| image.file_key.as_str())
            .collect::<Vec<_>>(),
        ["preloaded-ok", "image-ok"]
    );
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_key, "preloaded-file");
    assert!(notices
        .iter()
        .any(|notice| notice.contains("不是有效 Base64")));
    assert!(notices.iter().any(|notice| notice.contains("下载失败")));
    assert!(notices.iter().any(|notice| notice.contains("附件总量")));
    assert!(notices
        .iter()
        .any(|notice| notice.contains("超过 0 MiB 上限")));

    let (_, files, notices) = hydrate_referenced_group_attachments_with_limits(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-post-download-budget".to_string(),
            images: Vec::new(),
            files: vec![
                file("file-first", None, None),
                file("file-second", None, None),
                file("file-fail", None, None),
            ],
        },
        limits,
    )
    .await;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_key, "file-first");
    assert!(notices.iter().any(|notice| notice.contains("附件总量")));
    assert!(notices.iter().any(|notice| notice.contains("下载失败")));

    let truncation_limits: ReferencedAttachmentLimits = [1, limits[1], limits[2], limits[3]];
    let (_, _, notices) = hydrate_referenced_group_attachments_with_limits(
        &client,
        &provider,
        crate::im_gateway::group_context::ReferencedGroupAttachments {
            message_id: "quoted-count-limits".to_string(),
            images: vec![image("first", Some("aW1nNA==")), image("second", None)],
            files: vec![
                file("first", None, Some("YWJj")),
                file("second", None, None),
            ],
        },
        truncation_limits,
    )
    .await;
    assert_eq!(notices.len(), 2);
    assert!(notices[0].contains("最多处理 1 张"));
    assert!(notices[1].contains("最多处理 1 个"));
    server.abort();
}

async fn spawn_group_lookup_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    |request: hyper::Request<hyper::body::Incoming>| async move {
                        let body = if request
                            .uri()
                            .path()
                            .ends_with("/auth/v3/tenant_access_token/internal")
                        {
                            r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                        } else if request.uri().path().ends_with("/bot/v3/info") {
                            r#"{"code":0,"bot":{"open_id":"ou_bot","app_name":"Bifrost"}}"#
                        } else {
                            r#"{"code":0,"data":{"name":"API Engineering"}}"#
                        };
                        Ok::<_, hyper::Error>(
                            hyper::Response::builder()
                                .header("Content-Type", "application/json")
                                .body(http_body_util::Full::new(bytes::Bytes::from_static(
                                    body.as_bytes(),
                                )))
                                .unwrap(),
                        )
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}"), task)
}

async fn spawn_reference_routing_server(
    referenced_chat_id: &'static str,
) -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let message_reads = Arc::new(AtomicUsize::new(0));
    let server_reads = Arc::clone(&message_reads);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let message_reads = Arc::clone(&server_reads);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let message_reads = Arc::clone(&message_reads);
                        async move {
                            let path = request.uri().path();
                            let body = if path.ends_with("/auth/v3/tenant_access_token/internal") {
                                r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                                    .to_string()
                            } else if path.ends_with("/bot/v3/info") {
                                r#"{"code":0,"bot":{"open_id":"ou_bot","app_name":"Bifrost"}}"#
                                    .to_string()
                            } else if path.contains("/im/v1/messages/") {
                                message_reads.fetch_add(1, Ordering::SeqCst);
                                format!(
                                    r#"{{"code":0,"data":{{"items":[{{"message_id":"om_parent","chat_id":"{referenced_chat_id}","msg_type":"text","sender":{{"id":"ou_author","sender_type":"user"}},"mentions":[{{"key":"@_user_1","id":{{"open_id":"ou_alice"}},"name":"Alice"}}],"body":{{"content":"{{\"text\":\"@_user_1 quoted content\"}}"}},"create_time":"1"}}]}}}}"#
                                )
                            } else {
                                r#"{"code":0,"data":{"name":"Engineering"}}"#.to_string()
                            };
                            Ok::<_, hyper::Error>(
                                hyper::Response::builder()
                                    .header("Content-Type", "application/json")
                                    .body(http_body_util::Full::new(bytes::Bytes::from(body)))
                                    .unwrap(),
                            )
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}"), message_reads, task)
}

async fn spawn_referenced_file_server() -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let message_reads = Arc::new(AtomicUsize::new(0));
    let resource_reads = Arc::new(AtomicUsize::new(0));
    let server_message_reads = Arc::clone(&message_reads);
    let server_reads = Arc::clone(&resource_reads);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let message_reads = Arc::clone(&server_message_reads);
            let resource_reads = Arc::clone(&server_reads);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let message_reads = Arc::clone(&message_reads);
                        let resource_reads = Arc::clone(&resource_reads);
                        async move {
                            let path = request.uri().path();
                            let (content_type, body) = if path
                                .ends_with("/auth/v3/tenant_access_token/internal")
                            {
                                (
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                                    ),
                                )
                            } else if path.ends_with(
                                "/im/v1/messages/om_quoted_file/resources/file_v3_quoted",
                            ) {
                                resource_reads.fetch_add(1, Ordering::SeqCst);
                                (
                                    "text/markdown; charset=utf-8",
                                    bytes::Bytes::from_static(b"# quoted attachment"),
                                )
                            } else if path.ends_with("/im/v1/messages/om_quoted_file") {
                                message_reads.fetch_add(1, Ordering::SeqCst);
                                (
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"data":{"items":[{"message_id":"om_quoted_file","chat_id":"oc_group","msg_type":"file","sender":{"id":"ou_author","sender_type":"user"},"body":{"content":"{\"file_key\":\"file_v3_quoted\",\"file_name\":\"quoted.md\",\"mime_type\":\"text/markdown\",\"file_size\":19}"},"create_time":"1"}]}}"#,
                                    ),
                                )
                            } else if path.ends_with("/bot/v3/info") {
                                (
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"bot":{"open_id":"ou_bot","app_name":"Bifrost"}}"#,
                                    ),
                                )
                            } else {
                                (
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"data":{"name":"Engineering"}}"#,
                                    ),
                                )
                            };
                            Ok::<_, hyper::Error>(
                                hyper::Response::builder()
                                    .header("Content-Type", content_type)
                                    .body(http_body_util::Full::new(body))
                                    .unwrap(),
                            )
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (
        format!("http://{address}"),
        message_reads,
        resource_reads,
        task,
    )
}

async fn spawn_referenced_image_server(
    fail_resource: bool,
) -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let message_reads = Arc::new(AtomicUsize::new(0));
    let resource_reads = Arc::new(AtomicUsize::new(0));
    let server_message_reads = Arc::clone(&message_reads);
    let server_resource_reads = Arc::clone(&resource_reads);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let message_reads = Arc::clone(&server_message_reads);
            let resource_reads = Arc::clone(&server_resource_reads);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let message_reads = Arc::clone(&message_reads);
                        let resource_reads = Arc::clone(&resource_reads);
                        async move {
                            let path = request.uri().path();
                            let (status, content_type, body) = if path
                                .ends_with("/auth/v3/tenant_access_token/internal")
                            {
                                (
                                    hyper::StatusCode::OK,
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                                    ),
                                )
                            } else if path.ends_with(
                                "/im/v1/messages/om_quoted_image/resources/img_v3_quoted",
                            ) {
                                resource_reads.fetch_add(1, Ordering::SeqCst);
                                if fail_resource {
                                    (
                                        hyper::StatusCode::INTERNAL_SERVER_ERROR,
                                        "application/json",
                                        bytes::Bytes::from_static(br#"{"code":230001}"#),
                                    )
                                } else {
                                    (
                                        hyper::StatusCode::OK,
                                        "image/png",
                                        bytes::Bytes::from_static(b"quoted image bytes"),
                                    )
                                }
                            } else if path.ends_with("/im/v1/messages/om_quoted_image") {
                                message_reads.fetch_add(1, Ordering::SeqCst);
                                (
                                    hyper::StatusCode::OK,
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"data":{"items":[{"message_id":"om_quoted_image","chat_id":"oc_group","msg_type":"image","sender":{"id":"ou_author","sender_type":"user"},"body":{"content":"{\"image_key\":\"img_v3_quoted\"}"},"create_time":"1"}]}}"#,
                                    ),
                                )
                            } else if path.ends_with("/bot/v3/info") {
                                (
                                    hyper::StatusCode::OK,
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"bot":{"open_id":"ou_bot","app_name":"Bifrost"}}"#,
                                    ),
                                )
                            } else {
                                (
                                    hyper::StatusCode::OK,
                                    "application/json",
                                    bytes::Bytes::from_static(
                                        br#"{"code":0,"data":{"name":"Engineering"}}"#,
                                    ),
                                )
                            };
                            Ok::<_, hyper::Error>(
                                hyper::Response::builder()
                                    .status(status)
                                    .header("Content-Type", content_type)
                                    .body(http_body_util::Full::new(body))
                                    .unwrap(),
                            )
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (
        format!("http://{address}"),
        message_reads,
        resource_reads,
        task,
    )
}

async fn spawn_new_group_event_loop_server() -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let creates = Arc::new(AtomicUsize::new(0));
    let server_creates = Arc::clone(&creates);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let creates = Arc::clone(&server_creates);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let creates = Arc::clone(&creates);
                        async move {
                            let path = request.uri().path();
                            let body = if path.ends_with("/auth/v3/tenant_access_token/internal") {
                                r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                            } else if path.ends_with("/im/v1/chats") {
                                creates.fetch_add(1, Ordering::SeqCst);
                                r#"{"code":0,"data":{"chat_id":"oc_new_loop","name":"事件循环群"}}"#
                            } else {
                                r#"{"code":0,"data":{"message_id":"om_sent"}}"#
                            };
                            Ok::<_, hyper::Error>(
                                hyper::Response::builder()
                                    .header("Content-Type", "application/json")
                                    .body(http_body_util::Full::new(bytes::Bytes::from_static(
                                        body.as_bytes(),
                                    )))
                                    .unwrap(),
                            )
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}"), creates, task)
}

fn group_test_event(
    provider_id: &str,
    message_id: &str,
    text: &str,
    mention_bot: bool,
    received_at: u64,
) -> ImEvent {
    ImEvent {
        event_id: format!("event-{message_id}"),
        provider_id: provider_id.to_string(),
        provider_type: crate::im_gateway::types::ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("oc_group".to_string()),
            chat_type: Some("group".to_string()),
            user_id: Some("ou_sender".to_string()),
            user_name: Some("Alice".to_string()),
            sender_type: Some("user".to_string()),
            message_id: Some(message_id.to_string()),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: text.to_string(),
            mentions: mention_bot
                .then(|| crate::im_gateway::types::ImMention {
                    key: "@_user_1".to_string(),
                    open_id: Some("ou_bot".to_string()),
                    name: Some("Bifrost".to_string()),
                    tenant_key: None,
                    is_bot: true,
                })
                .into_iter()
                .collect(),
            images: Vec::new(),
            files: Vec::new(),
            reply_to: None,
            raw_type: Some("text".to_string()),
            raw_content: Some(serde_json::json!({
                "text": text,
                "_bifrost_debug_chat_name": "Engineering"
            })),
            create_time: Some(received_at),
            update_time: None,
            root_id: None,
            parent_id: None,
            thread_id: None,
        }),
        received_at,
        raw_digest: None,
    }
}

#[path = "tests/group_flow_tests.rs"]
mod group_flow_tests;

#[path = "tests/recorder_tests.rs"]
mod recorder_tests;

#[path = "tests/recovery_tests.rs"]
mod recovery_tests;

pub(super) fn recorder_test_provider() -> ImProviderConfig {
    ImProviderConfig {
        id: "feishu-recorder".to_string(),
        provider_type: crate::im_gateway::types::ImProviderType::Feishu,
        display_name: "Feishu Recorder".to_string(),
        enabled: true,
        base_url: None,
        app_id: Some("app".to_string()),
        secret_ref: Some("secret".to_string()),
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

pub(super) fn recorder_test_request(
    session_key: &str,
) -> crate::im_gateway::external_cli::ExternalCliRunRequest {
    crate::im_gateway::external_cli::ExternalCliRunRequest {
        message: "record this turn".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("feishu-recorder".to_string()),
        runner_id: Some("codex".to_string()),
        session_key: Some(session_key.to_string()),
        runtime: "external_cli".to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    }
}
