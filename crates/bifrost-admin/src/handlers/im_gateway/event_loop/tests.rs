use super::*;

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
