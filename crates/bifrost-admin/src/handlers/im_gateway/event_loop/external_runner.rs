use super::*;

const WEIXIN_COMPANION_COALESCE_WINDOW_MS: u64 = 3_000;

struct WeixinCompanionInput {
    event: ImEvent,
    message_text: String,
    images: Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
    files: Vec<crate::im_gateway::external_cli::ExternalCliFileInput>,
    has_attachment: bool,
}

fn enforce_weixin_companion_attachment_budgets(input: &mut ExternalCliChatInput) {
    enforce_weixin_companion_attachment_budgets_with_limits(
        input,
        MAX_AGENT_ATTACHMENTS_PER_MESSAGE,
        MAX_AGENT_REPLY_IMAGE_BYTES,
        MAX_FEISHU_REFERENCED_FILE_BYTES,
        MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES,
    );
}

fn enforce_weixin_companion_attachment_budgets_with_limits(
    input: &mut ExternalCliChatInput,
    max_attachments: usize,
    max_image_bytes: u64,
    max_file_bytes: u64,
    max_total_bytes: u64,
) {
    let attachment_count = input.images.len().saturating_add(input.files.len());
    let mut retained_images = Vec::new();
    let mut total_bytes = 0u64;
    for image in std::mem::take(&mut input.images) {
        if retained_images.len() >= max_attachments {
            break;
        }
        let label = image.name.as_deref().unwrap_or("image");
        let decoded_size =
            match preloaded_payload_size(Some(&image.data), "图片", label, max_image_bytes) {
                Ok(Some(size)) => size,
                Ok(None) => 0,
                Err(problem) => {
                    warn!(image = %label, problem, "skipping invalid coalesced Weixin image");
                    continue;
                }
            };
        if referenced_file_budget_exceeded_with_limit(total_bytes, decoded_size, max_total_bytes) {
            warn!(
                image = %label,
                "skipping coalesced Weixin image because the batch exceeds its byte budget"
            );
            continue;
        }
        total_bytes = total_bytes.saturating_add(decoded_size);
        retained_images.push(image);
    }

    let mut retained_files = Vec::new();
    for file in std::mem::take(&mut input.files) {
        if retained_images.len().saturating_add(retained_files.len()) >= max_attachments {
            break;
        }
        let label = file.name.as_deref().unwrap_or("attachment");
        let decoded_size =
            match preloaded_payload_size(Some(&file.data), "文件", label, max_file_bytes) {
                Ok(Some(size)) => size,
                Ok(None) => 0,
                Err(problem) => {
                    warn!(file = %label, problem, "skipping invalid coalesced Weixin file");
                    continue;
                }
            };
        if referenced_file_budget_exceeded_with_limit(total_bytes, decoded_size, max_total_bytes) {
            warn!(
                file = %label,
                "skipping coalesced Weixin file because the batch exceeds its byte budget"
            );
            continue;
        }
        total_bytes = total_bytes.saturating_add(decoded_size);
        retained_files.push(file);
    }
    if attachment_count > max_attachments {
        warn!(
            attachment_count,
            max_attachments,
            "too many attachments across coalesced Weixin events; truncating runner input"
        );
    }
    input.images = retained_images;
    input.files = retained_files;
}

fn merge_weixin_companion_batch(
    input: &mut ExternalCliChatInput,
    initial_has_meaningful_text: bool,
    initial_has_attachment: bool,
    companions: Vec<WeixinCompanionInput>,
) -> Vec<WeixinCompanionInput> {
    if !initial_has_attachment && !companions.iter().any(|item| item.has_attachment) {
        return companions;
    }

    let mut text_parts = Vec::new();
    if initial_has_meaningful_text && !input.message_text.trim().is_empty() {
        text_parts.push(input.message_text.trim().to_string());
    }
    let mut deferred = Vec::new();
    for companion in companions {
        if companion.message_text.trim_start().starts_with('/') {
            deferred.push(companion);
            continue;
        }
        if !companion.message_text.trim().is_empty() {
            text_parts.push(companion.message_text.trim().to_string());
        }
        input.images.extend(companion.images);
        input.files.extend(companion.files);
    }

    enforce_weixin_companion_attachment_budgets(input);

    if !text_parts.is_empty() {
        input.message_text = text_parts.join("\n\n");
    } else if !input.images.is_empty() {
        input.message_text = IMAGE_ONLY_AGENT_PROMPT.to_string();
    } else if !input.files.is_empty() {
        input.message_text = format!("[附件消息: {} 个]", input.files.len());
    }
    deferred
}

async fn finish_progress_task(mut task: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

fn event_has_attachments(event: &ImEvent) -> bool {
    event
        .message
        .as_ref()
        .is_some_and(|message| !message.images.is_empty() || !message.files.is_empty())
}

fn weixin_companion_remaining_ms(first_received_at: u64, current_time_ms: u64) -> u64 {
    WEIXIN_COMPANION_COALESCE_WINDOW_MS
        .saturating_sub(current_time_ms.saturating_sub(first_received_at))
}

async fn collect_weixin_companion_events(
    rx: &mut mpsc::UnboundedReceiver<ImEvent>,
    first_received_at: u64,
) -> Vec<ImEvent> {
    let remaining_ms = weixin_companion_remaining_ms(first_received_at, now_ms());
    if remaining_ms == 0 {
        return Vec::new();
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(remaining_ms);
    let mut events = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        events.push(event);
    }
    events
}

#[cfg(test)]
// These focused tests stay next to the companion-window helpers so their
// timing contract remains reviewable without moving the production pipeline.
#[allow(clippy::items_after_test_module)]
mod weixin_companion_tests {
    use super::*;

    fn weixin_event(
        event_id: &str,
        text: &str,
        files: Vec<crate::im_gateway::types::ImFileAttachment>,
    ) -> ImEvent {
        ImEvent {
            event_id: event_id.to_string(),
            provider_id: "weixin-main".to_string(),
            provider_type: ImProviderType::Weixin,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("weixin-user".to_string()),
                chat_type: Some("p2p".to_string()),
                user_id: Some("weixin-user".to_string()),
                user_name: Some("Weixin User".to_string()),
                sender_type: Some("user".to_string()),
                message_id: Some(format!("message-{event_id}")),
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: text.to_string(),
                files,
                raw_type: Some("text".to_string()),
                ..Default::default()
            }),
            received_at: now_ms(),
            raw_digest: None,
        }
    }

    fn inline_file(name: &str, data_base64: &str) -> crate::im_gateway::types::ImFileAttachment {
        crate::im_gateway::types::ImFileAttachment {
            file_key: format!("file-{name}"),
            name: Some(name.to_string()),
            mime_type: Some("text/plain".to_string()),
            size_bytes: None,
            data_base64: Some(data_base64.to_string()),
            ..Default::default()
        }
    }

    fn input(message_text: &str) -> ExternalCliChatInput {
        ExternalCliChatInput {
            message_text: message_text.to_string(),
            images: Vec::new(),
            files: Vec::new(),
            session_key: "weixin-session".to_string(),
            adapter_override: None,
            instructions_override: None,
            delivery_override: None,
            runner_id_override: None,
            runner_selected: false,
            group_turn_id: None,
            reset_group_context: false,
            thread_anchor_message_id: None,
            thread_fallback_message: None,
        }
    }

    fn image(name: &str) -> crate::im_gateway::external_cli::ExternalCliImageInput {
        crate::im_gateway::external_cli::ExternalCliImageInput {
            mime_type: "image/png".to_string(),
            data: "aW1hZ2U=".to_string(),
            name: Some(name.to_string()),
        }
    }

    fn file(name: &str) -> crate::im_gateway::external_cli::ExternalCliFileInput {
        crate::im_gateway::external_cli::ExternalCliFileInput {
            mime_type: "text/plain".to_string(),
            data: "ZmlsZQ==".to_string(),
            name: Some(name.to_string()),
        }
    }

    fn companion(
        message_text: &str,
        images: Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
        files: Vec<crate::im_gateway::external_cli::ExternalCliFileInput>,
    ) -> WeixinCompanionInput {
        let has_attachment = !images.is_empty() || !files.is_empty();
        WeixinCompanionInput {
            event: ImEvent {
                event_id: format!("event-{message_text}"),
                provider_id: "weixin".to_string(),
                provider_type: ImProviderType::Weixin,
                event_type: "message.receive".to_string(),
                source: crate::im_gateway::types::ImEventSource {
                    chat_id: Some("weixin-user".to_string()),
                    chat_type: Some("p2p".to_string()),
                    user_id: Some("weixin-user".to_string()),
                    user_name: None,
                    sender_type: Some("user".to_string()),
                    message_id: Some(format!("message-{message_text}")),
                },
                message: None,
                received_at: now_ms(),
                raw_digest: None,
            },
            message_text: message_text.to_string(),
            images,
            files,
            has_attachment,
        }
    }

    #[test]
    fn attachment_first_then_text_becomes_one_agent_input() {
        let mut input = input(IMAGE_ONLY_AGENT_PROMPT);
        input.images.push(image("first.png"));

        let deferred = merge_weixin_companion_batch(
            &mut input,
            false,
            true,
            vec![companion("这张图里是什么？", Vec::new(), Vec::new())],
        );

        assert!(deferred.is_empty());
        assert_eq!(input.message_text, "这张图里是什么？");
        assert_eq!(input.images.len(), 1);
    }

    #[test]
    fn text_first_then_attachment_becomes_one_agent_input() {
        let mut input = input("帮我总结这个文件");

        let deferred = merge_weixin_companion_batch(
            &mut input,
            true,
            false,
            vec![companion("", Vec::new(), vec![file("notes.md")])],
        );

        assert!(deferred.is_empty());
        assert_eq!(input.message_text, "帮我总结这个文件");
        assert_eq!(input.files.len(), 1);
        assert_eq!(input.files[0].name.as_deref(), Some("notes.md"));
    }

    #[test]
    fn adjacent_plain_text_is_deferred_instead_of_semantically_merged() {
        let mut input = input("第一件事");

        let deferred = merge_weixin_companion_batch(
            &mut input,
            true,
            false,
            vec![companion("第二件事", Vec::new(), Vec::new())],
        );

        assert_eq!(input.message_text, "第一件事");
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].message_text, "第二件事");
    }

    #[test]
    fn slash_command_is_deferred_while_attachment_companions_merge() {
        let mut input = input("看看附件");

        let deferred = merge_weixin_companion_batch(
            &mut input,
            true,
            false,
            vec![
                companion("", vec![image("diagram.png")], Vec::new()),
                companion("/pwd", Vec::new(), Vec::new()),
            ],
        );

        assert_eq!(input.message_text, "看看附件");
        assert_eq!(input.images.len(), 1);
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].message_text, "/pwd");
    }

    #[test]
    fn merges_multiple_images_files_and_text_fragments_in_arrival_order() {
        let mut input = input("说明");
        input.images.push(image("initial.png"));

        let deferred = merge_weixin_companion_batch(
            &mut input,
            true,
            true,
            vec![
                companion("第一段", vec![image("second.png")], Vec::new()),
                companion("第二段", Vec::new(), vec![file("report.md")]),
            ],
        );

        assert!(deferred.is_empty());
        assert_eq!(input.message_text, "说明\n\n第一段\n\n第二段");
        assert_eq!(input.images.len(), 2);
        assert_eq!(input.files.len(), 1);
    }

    #[test]
    fn mixed_media_uses_one_shared_count_and_byte_budget() {
        let mut mixed_input = input("mixed");
        mixed_input.images = (0..4).map(|index| image(&format!("{index}.png"))).collect();
        mixed_input.files = (0..4).map(|index| file(&format!("{index}.txt"))).collect();

        enforce_weixin_companion_attachment_budgets_with_limits(&mut mixed_input, 6, 10, 10, 100);

        assert_eq!(mixed_input.images.len() + mixed_input.files.len(), 6);

        let mut byte_limited = input("byte-limited");
        byte_limited.images.push(image("first.png"));
        byte_limited.files.push(file("second.txt"));
        enforce_weixin_companion_attachment_budgets_with_limits(&mut byte_limited, 6, 10, 10, 7);
        assert_eq!(byte_limited.images.len(), 1);
        assert!(byte_limited.files.is_empty());
    }

    #[test]
    fn attachment_only_batches_keep_specific_agent_prompts() {
        let mut image_input = input("");
        image_input.images.push(image("only.png"));
        assert!(merge_weixin_companion_batch(&mut image_input, false, true, Vec::new()).is_empty());
        assert_eq!(image_input.message_text, IMAGE_ONLY_AGENT_PROMPT);

        let mut file_input = input("");
        file_input.files.push(file("only.txt"));
        assert!(merge_weixin_companion_batch(&mut file_input, false, true, Vec::new()).is_empty());
        assert_eq!(file_input.message_text, "[附件消息: 1 个]");
    }

    #[test]
    fn companion_window_uses_first_arrival_and_expires_at_three_seconds() {
        assert_eq!(weixin_companion_remaining_ms(10_000, 10_000), 3_000);
        assert_eq!(weixin_companion_remaining_ms(10_000, 12_999), 1);
        assert_eq!(weixin_companion_remaining_ms(10_000, 13_000), 0);
        assert_eq!(weixin_companion_remaining_ms(10_000, 13_500), 0);
    }

    #[tokio::test]
    async fn expired_companion_collection_does_not_consume_later_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(ImEvent {
            event_id: "later".to_string(),
            provider_id: "weixin".to_string(),
            provider_type: ImProviderType::Weixin,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource::default(),
            message: None,
            received_at: now_ms(),
            raw_digest: None,
        })
        .unwrap();

        let events = collect_weixin_companion_events(
            &mut rx,
            now_ms().saturating_sub(WEIXIN_COMPANION_COALESCE_WINDOW_MS),
        )
        .await;

        assert!(events.is_empty());
        assert_eq!(rx.try_recv().unwrap().event_id, "later");
    }

    #[tokio::test]
    async fn coalesce_resolves_inline_files_ignores_empty_events_and_defers_slash() {
        let temp = tempfile::tempdir().expect("weixin coalesce data dir");
        let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
        let client =
            ImProviderClient::Weixin(Arc::clone(service.connection_manager.weixin_provider()));
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        provider.id = "weixin-main".to_string();
        provider.provider_type = ImProviderType::Weixin;
        let initial_event = weixin_event(
            "initial-file",
            "",
            vec![inline_file("initial.txt", "aW5pdA==")],
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(ImEvent {
            event_id: "missing-message".to_string(),
            provider_id: provider.id.clone(),
            provider_type: ImProviderType::Weixin,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource::default(),
            message: None,
            received_at: now_ms(),
            raw_digest: None,
        })
        .unwrap();
        tx.send(weixin_event("empty", "   ", Vec::new())).unwrap();
        tx.send(weixin_event("caption", "请总结这些附件", Vec::new()))
            .unwrap();
        tx.send(weixin_event(
            "second-file",
            "",
            vec![inline_file("second.txt", "c2Vjb25k")],
        ))
        .unwrap();
        tx.send(weixin_event("slash", "/pwd", Vec::new())).unwrap();
        drop(tx);

        // Block only the event-store file path. Removing the whole admin
        // directory races with other stores that keep files open on Windows.
        // Coalescing persistence is best-effort and must still deliver input.
        let admin_path = temp.path().join("admin");
        std::fs::create_dir_all(&admin_path).expect("create event-store directory");
        std::fs::create_dir(admin_path.join("im_gateway_events.json"))
            .expect("block event-store file path");

        let mut input = input(IMAGE_ONLY_AGENT_PROMPT);
        let mut ctx = ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &initial_event,
            message_log_store: &service.message_log_store,
            agent_config_store: &service.agent_config_store,
            external_cli_config_store: &service.external_cli_config_store,
            agent_session_manager: &service.agent_session_manager,
            queue_manager: &service.queue_manager,
            progress_registry: &service.progress_registry,
            event_store: &service.event_store,
            group_context_store: &service.group_context_store,
        };

        let deferred = coalesce_weixin_companion_events(&mut ctx, &mut input).await;

        assert_eq!(input.message_text, "请总结这些附件");
        assert_eq!(input.files.len(), 2);
        assert_eq!(
            input
                .files
                .iter()
                .filter_map(|file| file.name.as_deref())
                .collect::<Vec<_>>(),
            vec!["initial.txt", "second.txt"]
        );
        assert_eq!(deferred.len(), 1);
        assert_eq!(
            deferred[0]
                .message
                .as_ref()
                .map(|message| message.text.as_str()),
            Some("/pwd")
        );
    }

    #[tokio::test]
    async fn coalesce_skips_non_weixin_and_resolves_slash_attachments_without_waiting() {
        let temp = tempfile::tempdir().expect("weixin coalesce slash data dir");
        let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
        let client =
            ImProviderClient::Weixin(Arc::clone(service.connection_manager.weixin_provider()));
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        let slash_event = weixin_event(
            "slash-file",
            "/help",
            vec![inline_file("slash.txt", "c2xhc2g=")],
        );
        let (_tx, mut rx) = mpsc::unbounded_channel();
        let mut input = input("/help");
        {
            let mut ctx = ExternalCliChatContext {
                rx: &mut rx,
                client: &client,
                provider: &provider,
                provider_store: &service.provider_store,
                event: &slash_event,
                message_log_store: &service.message_log_store,
                agent_config_store: &service.agent_config_store,
                external_cli_config_store: &service.external_cli_config_store,
                agent_session_manager: &service.agent_session_manager,
                queue_manager: &service.queue_manager,
                progress_registry: &service.progress_registry,
                event_store: &service.event_store,
                group_context_store: &service.group_context_store,
            };

            assert!(coalesce_weixin_companion_events(&mut ctx, &mut input)
                .await
                .is_empty());
        }
        assert!(input.files.is_empty());

        provider.id = "weixin-main".to_string();
        provider.provider_type = ImProviderType::Weixin;
        let mut ctx = ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &slash_event,
            message_log_store: &service.message_log_store,
            agent_config_store: &service.agent_config_store,
            external_cli_config_store: &service.external_cli_config_store,
            agent_session_manager: &service.agent_session_manager,
            queue_manager: &service.queue_manager,
            progress_registry: &service.progress_registry,
            event_store: &service.event_store,
            group_context_store: &service.group_context_store,
        };
        assert!(coalesce_weixin_companion_events(&mut ctx, &mut input)
            .await
            .is_empty());
        assert_eq!(input.files.len(), 1);
        assert_eq!(input.files[0].name.as_deref(), Some("slash.txt"));

        // A slash command whose attachment was already hydrated must not
        // download it again or enter the companion wait window.
        assert!(coalesce_weixin_companion_events(&mut ctx, &mut input)
            .await
            .is_empty());
        assert_eq!(input.files.len(), 1);
    }

    #[tokio::test]
    async fn attachment_resolution_handles_missing_message_and_empty_companion_window() {
        let temp = tempfile::tempdir().expect("weixin empty companion data dir");
        let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
        let client =
            ImProviderClient::Weixin(Arc::clone(service.connection_manager.weixin_provider()));
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        provider.id = "weixin-main".to_string();
        provider.provider_type = ImProviderType::Weixin;
        let mut event = weixin_event("missing-message", "", Vec::new());
        event.message = None;

        let (images, files) = resolve_weixin_event_attachments(&client, &provider, &event).await;
        assert!(images.is_empty());
        assert!(files.is_empty());

        event.received_at = now_ms().saturating_sub(WEIXIN_COMPANION_COALESCE_WINDOW_MS);
        let (_tx, mut rx) = mpsc::unbounded_channel();
        let mut input = input("无需聚合");
        let mut ctx = ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &event,
            message_log_store: &service.message_log_store,
            agent_config_store: &service.agent_config_store,
            external_cli_config_store: &service.external_cli_config_store,
            agent_session_manager: &service.agent_session_manager,
            queue_manager: &service.queue_manager,
            progress_registry: &service.progress_registry,
            event_store: &service.event_store,
            group_context_store: &service.group_context_store,
        };
        assert!(coalesce_weixin_companion_events(&mut ctx, &mut input)
            .await
            .is_empty());
        assert_eq!(input.message_text, "无需聚合");
    }

    #[tokio::test]
    async fn coalesce_defers_adjacent_plain_text_when_no_attachment_arrives() {
        let temp = tempfile::tempdir().expect("weixin plain text companion data dir");
        let service = crate::handlers::im_gateway::ImGatewayService::new(temp.path());
        let client =
            ImProviderClient::Weixin(Arc::clone(service.connection_manager.weixin_provider()));
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        provider.id = "weixin-main".to_string();
        provider.provider_type = ImProviderType::Weixin;
        let initial_event = weixin_event("first-text", "第一件事", Vec::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(weixin_event("second-text", "第二件事", Vec::new()))
            .unwrap();
        drop(tx);
        let mut input = input("第一件事");
        let mut ctx = ExternalCliChatContext {
            rx: &mut rx,
            client: &client,
            provider: &provider,
            provider_store: &service.provider_store,
            event: &initial_event,
            message_log_store: &service.message_log_store,
            agent_config_store: &service.agent_config_store,
            external_cli_config_store: &service.external_cli_config_store,
            agent_session_manager: &service.agent_session_manager,
            queue_manager: &service.queue_manager,
            progress_registry: &service.progress_registry,
            event_store: &service.event_store,
            group_context_store: &service.group_context_store,
        };

        let deferred = coalesce_weixin_companion_events(&mut ctx, &mut input).await;

        assert_eq!(input.message_text, "第一件事");
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].event_id, "second-text");
    }
}

async fn resolve_weixin_event_attachments(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> (
    Vec<crate::im_gateway::external_cli::ExternalCliImageInput>,
    Vec<crate::im_gateway::external_cli::ExternalCliFileInput>,
) {
    let Some(message) = event.message.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let (images, files) = tokio::join!(
        resolve_event_images(client, provider, event, &message.images),
        resolve_event_files(client, provider, event, &message.files),
    );
    (external_cli_images_from_chat_images(images), files)
}

async fn coalesce_weixin_companion_events(
    ctx: &mut ExternalCliChatContext<'_>,
    input: &mut ExternalCliChatInput,
) -> Vec<ImEvent> {
    if ctx.provider.provider_type != ImProviderType::Weixin {
        return Vec::new();
    }

    if input.message_text.trim_start().starts_with('/') {
        if input.images.is_empty() && input.files.is_empty() {
            let (images, files) =
                resolve_weixin_event_attachments(ctx.client, ctx.provider, ctx.event).await;
            input.images = images;
            input.files = files;
        }
        return Vec::new();
    }

    // Start the first media download immediately, but keep polling the session
    // mailbox for the whole companion window. This is important when Weixin's
    // CDN download/decryption takes longer than the caption/media event gap.
    let initial_download = resolve_weixin_event_attachments(ctx.client, ctx.provider, ctx.event);
    let companion_collection = collect_weixin_companion_events(ctx.rx, ctx.event.received_at);
    let ((initial_images, initial_files), companion_events) =
        tokio::join!(initial_download, companion_collection);
    if input.images.is_empty() && input.files.is_empty() {
        input.images = initial_images;
        input.files = initial_files;
    }

    let mut companions = Vec::new();
    for next_event in companion_events {
        let Some(message) = next_event.message.as_ref() else {
            continue;
        };
        if message.text.trim().is_empty() && message.images.is_empty() && message.files.is_empty() {
            continue;
        }
        let message_text = if message.text.trim().is_empty() {
            String::new()
        } else {
            agent_message_text_with_reference(
                message,
                &next_event.provider_id,
                next_event.source.user_id.as_deref(),
                next_event.source.message_id.as_deref(),
                ctx.message_log_store,
            )
        };
        let has_attachment = !message.images.is_empty() || !message.files.is_empty();
        let (images, files) = if message_text.trim_start().starts_with('/') {
            (Vec::new(), Vec::new())
        } else {
            resolve_weixin_event_attachments(ctx.client, ctx.provider, &next_event).await
        };
        companions.push(WeixinCompanionInput {
            event: next_event,
            message_text,
            images,
            files,
            has_attachment,
        });
    }

    if companions.is_empty() {
        return Vec::new();
    }
    let initial_has_meaningful_text = ctx
        .event
        .message
        .as_ref()
        .is_some_and(|message| !message.text.trim().is_empty() || message.reply_to.is_some());
    let initial_has_attachment = event_has_attachments(ctx.event);
    let batch_has_attachment =
        initial_has_attachment || companions.iter().any(|item| item.has_attachment);
    if batch_has_attachment {
        for companion in companions
            .iter()
            .filter(|item| !item.message_text.trim_start().starts_with('/'))
        {
            if let Err(error) = ctx.event_store.add(companion.event.clone()) {
                error!(error = %error, "failed to store coalesced Weixin companion event");
            }
            acknowledge_and_log_inbound_event(
                ctx.client,
                ctx.provider,
                &companion.event,
                ctx.message_log_store,
            )
            .await;
        }
    }
    let companion_count = companions.len();
    let deferred = merge_weixin_companion_batch(
        input,
        initial_has_meaningful_text,
        initial_has_attachment,
        companions,
    );
    let merged = companion_count.saturating_sub(deferred.len());
    if merged > 0 {
        info!(
            session_key = %input.session_key,
            merged_companion_count = merged,
            image_count = input.images.len(),
            file_count = input.files.len(),
            "coalesced adjacent Weixin text and attachments before Agent dispatch"
        );
    }
    deferred.into_iter().map(|item| item.event).collect()
}

pub(super) struct AbortTaskOnDrop(pub(super) tokio::task::AbortHandle);

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn external_cli_images_from_chat_images(
    images: Vec<bifrost_agent::ChatImageInput>,
) -> Vec<crate::im_gateway::external_cli::ExternalCliImageInput> {
    images
        .into_iter()
        .map(
            |image| crate::im_gateway::external_cli::ExternalCliImageInput {
                mime_type: image.mime_type,
                data: image.data,
                name: None,
            },
        )
        .collect()
}

pub(super) fn event_for_queue_item(
    base: &ImEvent,
    context: Option<&crate::im_gateway::queue_manager::QueueItemContext>,
) -> ImEvent {
    let mut event = base.clone();
    let Some(context) = context else {
        return event;
    };
    if !context.event_id.is_empty() {
        event.event_id = context.event_id.clone();
    }
    event.source.message_id = context.message_id.clone();
    event.source.user_id = context.user_id.clone();
    event.source.user_name = context.user_name.clone();
    event
}

pub(super) fn apply_session_bound_work_dir(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    session_work_dir: Option<&str>,
    fallback: Option<std::path::PathBuf>,
) {
    let runner_work_dir = request.work_dir.take();
    request.work_dir = session_work_dir
        .map(std::path::PathBuf::from)
        .or(runner_work_dir)
        .or(fallback);
}

pub(in crate::handlers::im_gateway) fn resolve_external_cli_delivery_mode(
    progress_presentation: crate::im_gateway::types::ImProgressPresentation,
    settings: &crate::im_gateway::external_cli::ExternalCliAgentSettings,
    sources: &std::collections::BTreeMap<String, String>,
    input_override: Option<crate::im_gateway::external_cli::ExternalCliDeliveryMode>,
) -> crate::im_gateway::external_cli::ExternalCliDeliveryMode {
    if let Some(delivery_mode) = input_override {
        return delivery_mode;
    }
    if progress_presentation != crate::im_gateway::types::ImProgressPresentation::TextOnly
        && is_im_progress_card_external_adapter(&settings.adapter)
        && sources.get("deliveryMode").map(String::as_str) != Some("channel")
    {
        return crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard;
    }
    settings.delivery_mode
}

pub(super) fn is_im_progress_card_external_adapter(adapter: &str) -> bool {
    matches!(
        adapter,
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    )
}

pub(super) fn should_create_traex_checkpoint(
    run_succeeded: bool,
    adapter: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) -> bool {
    run_succeeded
        && adapter == crate::im_gateway::external_cli::TRAEX_ADAPTER
        && crate::im_gateway::external_cli::thread_derivation_capability_for_request(request)
            .fork_completed
}

pub(super) fn take_thread_derivation_anchor(anchor: &mut Option<String>) -> Option<String> {
    anchor.take()
}

fn apply_ready_thread_anchor(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    anchor: crate::im_gateway::group_context::FeishuMessageAnchor,
) -> bool {
    let capability =
        crate::im_gateway::external_cli::thread_derivation_capability_for_request(request);
    let can_fork = if anchor.status == "active_ready" {
        capability.fork_active
    } else {
        capability.fork_completed
    };
    if !can_fork {
        return false;
    }
    let Some(source_thread_id) = anchor.checkpoint_thread_id.or(anchor.external_thread_id) else {
        return false;
    };
    crate::im_gateway::external_cli::apply_thread_derivation_to_run_request(
        request,
        &crate::im_gateway::external_cli::ExternalCliThreadDerivation {
            source_thread_id,
            last_turn_id: (anchor.status != "active_ready")
                .then_some(anchor.external_turn_id)
                .flatten(),
        },
    );
    true
}

pub(super) fn finalize_current_feishu_thread_binding(
    store: &ImGroupContextStore,
    provider_id: &str,
    event: &ImEvent,
    state: &str,
) {
    if let (Some(chat_id), Some((thread_id, _))) = (
        event.source.chat_id.as_deref(),
        crate::im_gateway::group_context::feishu_thread_parts(event),
    ) {
        if let Err(error) = store.update_feishu_thread_binding_state(
            provider_id,
            chat_id,
            thread_id,
            state,
            now_ms(),
        ) {
            warn!(thread_id, state, error = %error, "failed to finalize Feishu topic binding");
        }
    }
}

fn apply_thread_fallback(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    fallback_message: Option<&str>,
) {
    if let Some(fallback) = fallback_message {
        request.message = fallback.to_string();
    }
}

pub(super) async fn apply_thread_anchor_to_request(
    store: &ImGroupContextStore,
    session_manager: &ImAgentSessionManager,
    provider_id: &str,
    anchor_message_id: &str,
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    fallback_message: Option<&str>,
) {
    let anchor = match store.feishu_message_anchor(provider_id, anchor_message_id) {
        Ok(Some(anchor)) => anchor,
        Ok(None) => {
            apply_thread_fallback(request, fallback_message);
            return;
        }
        Err(error) => {
            warn!(message_id = anchor_message_id, error = %error, "failed to load Feishu source anchor");
            apply_thread_fallback(request, fallback_message);
            return;
        }
    };
    if anchor.is_derivable() && matches!(anchor.status.as_str(), "ready" | "active_ready") {
        if !apply_ready_thread_anchor(request, anchor) {
            apply_thread_fallback(request, fallback_message);
        }
        return;
    }
    if anchor.status != "pending" {
        apply_thread_fallback(request, fallback_message);
        return;
    }

    let wait_started = std::time::Instant::now();
    loop {
        if wait_started.elapsed() > std::time::Duration::from_secs(3600) {
            warn!(
                message_id = anchor_message_id,
                "timed out waiting for Feishu source anchor"
            );
            break;
        }
        match store.feishu_message_anchor(provider_id, anchor_message_id) {
            Ok(Some(updated))
                if updated.is_derivable()
                    && matches!(updated.status.as_str(), "ready" | "active_ready") =>
            {
                if apply_ready_thread_anchor(request, updated) {
                    return;
                }
                break;
            }
            Ok(Some(updated)) if updated.status == "pending" => {}
            Ok(Some(_)) | Ok(None) => break,
            Err(error) => {
                warn!(message_id = anchor_message_id, error = %error, "failed to poll Feishu source anchor");
                break;
            }
        }
        if !session_manager.is_session_active(&anchor.source_session_key) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    apply_thread_fallback(request, fallback_message);
}

pub(in crate::handlers::im_gateway) fn finalize_live_guide_group_turns(
    queue_manager: &SessionQueueManager,
    group_context_store: &ImGroupContextStore,
    session_key: &str,
    result: Result<(), &str>,
) {
    for turn_id in queue_manager.take_live_guide_turns(session_key) {
        let status_result = match result {
            Ok(()) => group_context_store.mark_turn_completed(&turn_id, now_ms()),
            Err(error) => group_context_store.mark_turn_failed(&turn_id, error, now_ms()),
        };
        if let Err(error) = status_result {
            warn!(turn_id = %turn_id, error = %error, "failed to finalize live guide group turn");
        }
    }
}

pub(super) struct ExternalRunnerProgressFinishContext<'a> {
    pub(super) progress_registry: &'a Arc<ImAgentProgressRegistry>,
    pub(super) client: &'a ImProviderClient,
    pub(super) provider: &'a ImProviderConfig,
    pub(super) message_log_store: &'a Arc<ImMessageLogStore>,
    pub(super) group_context_store: &'a Arc<ImGroupContextStore>,
    pub(super) event: &'a ImEvent,
}

pub(super) struct ExternalRunnerProgressFinish<'a> {
    pub(super) session_key: &'a str,
    pub(super) final_text: &'a str,
    pub(super) failed: bool,
    pub(super) work_dir: Option<&'a std::path::Path>,
    pub(super) anchor: Option<crate::im_gateway::group_context::FeishuMessageAnchor>,
}

pub(super) async fn finish_external_runner_progress_and_notify(
    ctx: ExternalRunnerProgressFinishContext<'_>,
    finish: ExternalRunnerProgressFinish<'_>,
) {
    let weixin_channel_run_id = ctx
        .progress_registry
        .weixin_channel_run_id(finish.session_key)
        .await;
    let rendered_final_text = ctx
        .progress_registry
        .render_markdown_images(finish.session_key, finish.final_text, finish.work_dir)
        .await;
    let progress_message = ctx
        .progress_registry
        .finish(
            finish.session_key,
            Some(rendered_final_text.clone()),
            finish.failed,
        )
        .await;

    if let Some(anchor) = finish.anchor.as_ref() {
        for message_info in ctx
            .progress_registry
            .message_infos(finish.session_key)
            .await
        {
            if let Some(message_id) = message_info.message_id {
                let mut card_anchor = anchor.clone();
                card_anchor.message_id = message_id;
                let _ = ctx
                    .group_context_store
                    .upsert_feishu_message_anchor(&card_anchor, now_ms());
            }
        }
    }

    let terminal_message_id = send_external_runner_terminal_reply_from_work_dir(
        ctx.client,
        ctx.provider,
        ctx.event,
        ExternalRunnerTerminalReply {
            text: &rendered_final_text,
            failed: finish.failed,
            progress_message_id: progress_message
                .as_ref()
                .and_then(|message| message.message_id.as_deref()),
            work_dir: finish.work_dir,
        },
        ctx.message_log_store,
    )
    .await;
    if let (Some(message_id), Some(mut anchor)) = (terminal_message_id, finish.anchor) {
        anchor.message_id = message_id;
        let _ = ctx
            .group_context_store
            .upsert_feishu_message_anchor(&anchor, now_ms());
    }
    if let (Some(weixin), Some(channel_run_id), Some(target)) = (
        ctx.client.weixin(),
        weixin_channel_run_id,
        build_agent_reply_target(
            ctx.provider,
            ctx.event,
            "__agent_reply__",
            "Agent Reply",
            "interactive",
        ),
    ) {
        weixin.end_channel_run(ctx.provider, &target, &channel_run_id);
    }
}

pub(super) async fn run_external_cli_agent_chat(
    mut ctx: ExternalCliChatContext<'_>,
    mut input: ExternalCliChatInput,
) {
    let deferred_weixin_events = coalesce_weixin_companion_events(&mut ctx, &mut input).await;
    let mut current_group_turn_id = input.group_turn_id.clone();
    let source_anchor = input
        .thread_anchor_message_id
        .as_deref()
        .and_then(|message_id| {
            ctx.group_context_store
                .feishu_message_anchor(&ctx.provider.id, message_id)
                .ok()
                .flatten()
        });
    let config = ctx.external_cli_config_store.load();
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        Some(&ctx.provider.id),
        source_anchor
            .as_ref()
            .map(|anchor| anchor.runner_id.as_str())
            .or(input.runner_id_override.as_deref()),
    );
    let mut settings = effective.settings;
    if let Some(adapter) = source_anchor
        .as_ref()
        .map(|anchor| anchor.adapter.as_str())
        .or(input.adapter_override.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        settings.adapter = adapter.to_string();
    }
    if let Some(instructions) = input
        .instructions_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        settings.instructions = match settings.instructions.take() {
            Some(existing) if !existing.trim().is_empty() => {
                Some(format!("{}\n\n{}", existing.trim(), instructions))
            }
            _ => Some(instructions.to_string()),
        };
    }

    for deferred_event in deferred_weixin_events {
        handle_concurrent_event_during_chat(
            &deferred_event,
            ctx.provider,
            &input.session_key,
            ctx.queue_manager,
            ctx.client,
            ctx.message_log_store,
            ctx.agent_session_manager,
            ctx.progress_registry,
            ctx.agent_config_store,
            ctx.provider_store,
            ctx.event_store,
            ctx.group_context_store,
            ctx.external_cli_config_store,
            busy_default_mode_for_external_adapter(&settings.adapter),
        )
        .await;
    }

    // Intercept /clear and /reset after resolving the effective runner so the
    // reset only clears the current adapter/runner rather than every agent that
    // happens to share the same IM session key.
    let trimmed_msg = input.message_text.trim();
    if trimmed_msg == "/help" {
        let response = build_im_startup_help_for_runner(
            &ImHelpRunnerKind::External {
                adapter: settings.adapter.clone(),
            },
            ctx.provider.provider_type,
        );
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            &response,
            ctx.message_log_store,
        )
        .await;
        finalize_current_feishu_thread_binding(
            ctx.group_context_store,
            &ctx.provider.id,
            ctx.event,
            "ready",
        );
        return;
    }

    if trimmed_msg == "/clear" || trimmed_msg == "/reset" {
        let _ = request_agent_stop(ctx.agent_session_manager, &input.session_key).await;
        if let Some(mut session) = ctx
            .agent_session_manager
            .try_take_session(&input.session_key)
        {
            session.clear();
            ctx.agent_session_manager.return_session(session);
        } else {
            ctx.agent_session_manager.clear_session(&input.session_key);
        }
        ctx.queue_manager.clear_session(&input.session_key);
        if settings.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
            crate::im_gateway::chatgpt_web::clear_session_conversation(&input.session_key).await;
        }
        clear_persisted_agent_session_state(
            &input.session_key,
            Some(&settings.adapter),
            Some(&effective.runner_id),
        );
        if input.reset_group_context {
            if let Err(error) = ctx.group_context_store.advance_context_baseline(ctx.event) {
                warn!(
                    provider_id = %ctx.provider.id,
                    session_key = %input.session_key,
                    error = %error,
                    "session reset succeeded but group context baseline could not be advanced"
                );
                send_agent_reply(
                    ctx.client,
                    ctx.provider,
                    ctx.event,
                    &format!("会话已重置，但群上下文基线更新失败：{error}"),
                    ctx.message_log_store,
                )
                .await;
                finalize_current_feishu_thread_binding(
                    ctx.group_context_store,
                    &ctx.provider.id,
                    ctx.event,
                    "ready",
                );
                return;
            }
        }
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "会话已重置，下一条消息将开始新的对话。",
            ctx.message_log_store,
        )
        .await;
        finalize_current_feishu_thread_binding(
            ctx.group_context_store,
            &ctx.provider.id,
            ctx.event,
            "ready",
        );
        return;
    }

    if !settings.enabled && !input.runner_selected {
        if let Some(turn_id) = current_group_turn_id.take() {
            if let Err(error) = ctx.group_context_store.release_turn(
                &turn_id,
                "Runner is not enabled for this IM channel",
                now_ms(),
            ) {
                warn!(turn_id = %turn_id, error = %error, "failed to release group turn for disabled runner");
            }
        }
        send_agent_reply(
            ctx.client,
            ctx.provider,
            ctx.event,
            "Runner is not enabled for this IM channel.",
            ctx.message_log_store,
        )
        .await;
        finalize_current_feishu_thread_binding(
            ctx.group_context_store,
            &ctx.provider.id,
            ctx.event,
            "failed",
        );
        return;
    }

    let persisted_state = crate::im_gateway::session_state::load_session_state(
        &input.session_key,
        &settings.adapter,
        Some(&effective.runner_id),
    );
    let delivery_mode = resolve_external_cli_delivery_mode(
        ctx.client
            .channel_capabilities(ctx.provider)
            .interaction
            .progress,
        &settings,
        &effective.sources,
        input.delivery_override,
    );
    let mut resolved_model_config =
        crate::im_gateway::external_cli::resolve_external_cli_model_config(
            &settings.adapter,
            &settings.adapter_config,
        );
    crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_model_config(
        &settings.adapter,
        persisted_state.as_ref(),
        &mut resolved_model_config,
    );
    let mut status_context =
        status_context_from_external_runner(&effective.runner_id, &settings.adapter);
    if let Some(model) = resolved_model_config.model.clone() {
        status_context.model = Some(model);
    }
    status_context.model_provider = resolved_model_config
        .model_provider
        .clone()
        .or_else(|| resolved_model_config.model_source.clone());
    status_context.model_reasoning_effort = resolved_model_config.reasoning_effort.clone();
    status_context.model_reasoning_summary = resolved_model_config.reasoning_summary.clone();

    let provider_agent_config =
        effective_agent_config_for_provider(&ctx.agent_config_store.load(), ctx.provider);
    let mut session_work_dir = ctx
        .group_context_store
        .work_dir_by_session(&input.session_key)
        .ok()
        .flatten()
        .map(|path| path.display().to_string())
        .or_else(|| provider_agent_config.work_dir.clone());
    let Some(mut session) = ctx
        .agent_session_manager
        .try_take_session_with_work_dir(&input.session_key, session_work_dir.clone())
    else {
        handle_busy_message(
            &input.message_text,
            &input.session_key,
            BusyMessageContext {
                queue_manager: ctx.queue_manager,
                client: ctx.client,
                provider: ctx.provider,
                event: ctx.event,
                message_log_store: ctx.message_log_store,
                agent_session_manager: ctx.agent_session_manager,
                progress_registry: ctx.progress_registry,
                external_cli_config_store: ctx.external_cli_config_store,
                agent_config: &provider_agent_config,
                group_context_store: ctx.group_context_store,
                group_turn_id: input.group_turn_id.as_deref(),
                default_mode: busy_default_mode_for_external_adapter(&settings.adapter),
                status_context,
                default_work_dir: session_work_dir.or_else(|| {
                    Some(
                        provider_agent_config
                            .resolve_work_dir()
                            .display()
                            .to_string(),
                    )
                }),
            },
        )
        .await;
        finalize_current_feishu_thread_binding(
            ctx.group_context_store,
            &ctx.provider.id,
            ctx.event,
            "ready",
        );
        return;
    };

    let guide_channel = ctx
        .queue_manager
        .get_or_create_guide_channel(&input.session_key);
    restore_session_from_persisted_history(
        &mut session,
        &input.session_key,
        &settings.adapter,
        Some(&effective.runner_id),
        provider_agent_config
            .history
            .as_ref()
            .and_then(|h| h.max_bytes),
    );
    let mut current_message = input.message_text;
    let thread_fallback_message = input.thread_fallback_message.clone();
    let mut thread_anchor_pending = input.thread_anchor_message_id.clone();
    let mut current_images = input.images;
    let mut current_files = input.files;
    let mut current_event = ctx.event.clone();
    let mut recorder = session.recorder.take();
    let mut runner_metadata = persisted_state
        .as_ref()
        .map(crate::im_gateway::session_state::metadata_from_state)
        .unwrap_or_default();

    loop {
        if let Some(command) = parse_im_cwd_command(&current_message) {
            let reply = match command {
                Ok(path) => apply_im_cwd_switch_to_session(
                    ctx.provider_store,
                    ctx.group_context_store,
                    &ctx.provider.id,
                    &input.session_key,
                    &mut session,
                    &path,
                ),
                Err(reason) => format_im_cwd_error(&reason),
            };
            if session.work_dir.is_some() {
                session_work_dir = session.work_dir.clone();
            }
            send_agent_reply(
                ctx.client,
                ctx.provider,
                &current_event,
                &reply,
                ctx.message_log_store,
            )
            .await;
            let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
            if let Some(unconsumed) =
                bifrost_agent::session::combine_guide_messages(unconsumed_guides)
            {
                if !unconsumed.trim().is_empty() {
                    let _ = ctx.queue_manager.push_queue(&input.session_key, unconsumed);
                }
            }
            match ctx.queue_manager.pop_queue_item(&input.session_key) {
                Some(next_item) => {
                    current_group_turn_id = next_item
                        .context
                        .as_ref()
                        .and_then(|context| context.group_turn_id.clone());
                    current_event = event_for_queue_item(ctx.event, next_item.context.as_ref());
                    current_message = next_item.message;
                    current_images = external_cli_images_from_chat_images(next_item.images);
                    current_files = next_item.files;
                    continue;
                }
                None => break,
            }
        }

        let mut request = crate::im_gateway::external_cli::run_request_from_settings(
            current_message.clone(),
            Some(ctx.provider.id.clone()),
            Some(input.session_key.clone()),
            &settings,
        );
        request.instructions =
            crate::im_gateway::external_cli::compose_external_cli_message_instructions(
                session.user_turn_count() == 0,
                provider_agent_config.base_instructions.as_deref(),
                provider_agent_config.developer_instructions.as_deref(),
                provider_agent_config.user_instructions.as_deref(),
                settings.instructions.as_deref(),
            );
        let latest_persisted_state = crate::im_gateway::session_state::load_session_state(
            &input.session_key,
            &settings.adapter,
            Some(&effective.runner_id),
        );
        crate::im_gateway::external_cli::apply_external_cli_session_overrides_to_run_request(
            &mut request,
            latest_persisted_state.as_ref(),
        );
        request.images = std::mem::take(&mut current_images);
        request.files = std::mem::take(&mut current_files);
        apply_external_cli_resume_metadata(&mut request, &runner_metadata);
        let current_thread_anchor = take_thread_derivation_anchor(&mut thread_anchor_pending);
        if let Some(anchor_message_id) = current_thread_anchor.as_deref() {
            apply_thread_anchor_to_request(
                ctx.group_context_store,
                ctx.agent_session_manager,
                &ctx.provider.id,
                anchor_message_id,
                &mut request,
                thread_fallback_message.as_deref(),
            )
            .await;
        }
        apply_session_bound_work_dir(
            &mut request,
            session_work_dir.as_deref(),
            effective_agent_work_dir_for_provider(&ctx.agent_config_store.load(), ctx.provider),
        );
        // Graceful fallback: if the configured work_dir doesn't exist on disk,
        // clear it so the runner uses a default directory instead of failing.
        if let Some(ref work_dir) = request.work_dir {
            if !work_dir.exists() {
                tracing::warn!(
                    work_dir = %work_dir.display(),
                    provider_id = %ctx.provider.id,
                    "agent work_dir does not exist, falling back to default"
                );
                request.work_dir = None;
            }
        }
        if request.allow_work_dirs.is_empty() {
            if let Some(work_dir) = request.work_dir.as_ref() {
                request.allow_work_dirs = vec![work_dir.display().to_string()];
            }
        }
        session.remember_runner_model_config(
            resolved_model_config.model.clone(),
            resolved_model_config
                .model_provider
                .clone()
                .or_else(|| resolved_model_config.model_source.clone()),
            resolved_model_config.reasoning_effort.clone(),
            resolved_model_config.reasoning_summary.clone(),
        );
        ensure_external_cli_session_recorder(
            &mut session,
            &mut recorder,
            &input.session_key,
            ctx.provider,
            &effective.runner_id,
            &request,
        );
        apply_external_cli_session_attachment_base_dir(&mut request, recorder.as_ref());
        record_external_cli_input(
            &mut session,
            &mut recorder,
            &input.session_key,
            &effective.runner_id,
            &request,
        );
        emit_external_cli_timeline_changed(
            ctx.agent_session_manager,
            recorder.as_ref(),
            &input.session_key,
            "im_turn_started",
        );
        let mut progress_enabled = false;
        let mut progress_runner_metadata = std::collections::BTreeMap::new();
        let mut progress_tx_for_finish = None;
        let mut progress_task = None;
        if matches!(
            delivery_mode,
            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
        ) {
            if let Some(progress_target) = build_agent_reply_target(
                ctx.provider,
                &current_event,
                "__agent_progress__",
                "Agent Progress",
                "interactive",
            ) {
                let presentation = ctx
                    .client
                    .channel_capabilities(ctx.provider)
                    .interaction
                    .progress;
                let progress_result: bifrost_core::Result<()> = match presentation {
                    crate::im_gateway::types::ImProgressPresentation::MutableCard => {
                        if let Some(feishu) = ctx.client.feishu() {
                            let is_feishu_thread =
                                crate::im_gateway::group_context::feishu_thread_parts(
                                    &current_event,
                                )
                                .is_some();
                            if ctx
                                .progress_registry
                                .rollover_existing_replying_to(
                                    &input.session_key,
                                    &current_message,
                                    current_event.source.message_id.as_deref(),
                                )
                                .await
                            {
                                Ok(())
                            } else if is_feishu_thread {
                                ctx.progress_registry
                                    .start_feishu_replying_in_thread(
                                        &input.session_key,
                                        feishu,
                                        ctx.provider.clone(),
                                        progress_target.clone(),
                                        &current_message,
                                        current_event.source.message_id.as_deref(),
                                    )
                                    .await
                                    .map(|_| ())
                            } else {
                                ctx.progress_registry
                                    .start_feishu_replying_to(
                                        &input.session_key,
                                        feishu,
                                        ctx.provider.clone(),
                                        progress_target.clone(),
                                        &current_message,
                                        current_event.source.message_id.as_deref(),
                                    )
                                    .await
                                    .map(|_| ())
                            }
                        } else {
                            Err(bifrost_core::BifrostError::Config(
                                "mutable progress provider unavailable".to_string(),
                            ))
                        }
                    }
                    crate::im_gateway::types::ImProgressPresentation::StructuredEvents => {
                        if let Some(weixin) = ctx.client.weixin() {
                            ctx.progress_registry
                                .start_weixin(
                                    &input.session_key,
                                    weixin,
                                    ctx.provider.clone(),
                                    progress_target.clone(),
                                )
                                .await;
                            Ok(())
                        } else {
                            Err(bifrost_core::BifrostError::Config(
                                "structured progress provider unavailable".to_string(),
                            ))
                        }
                    }
                    crate::im_gateway::types::ImProgressPresentation::TextOnly => {
                        Err(bifrost_core::BifrostError::Config(
                            "provider has no native progress presentation".to_string(),
                        ))
                    }
                };
                match progress_result {
                    Ok(_) => {
                        if presentation
                            == crate::im_gateway::types::ImProgressPresentation::MutableCard
                        {
                            if let Some(chat_id) = current_event.source.chat_id.as_deref() {
                                for message_info in ctx
                                    .progress_registry
                                    .message_infos(&input.session_key)
                                    .await
                                {
                                    if let Some(message_id) = message_info.message_id {
                                        let _ = ctx.group_context_store.upsert_feishu_message_anchor(
                                        &crate::im_gateway::group_context::FeishuMessageAnchor {
                                            provider_id: ctx.provider.id.clone(),
                                            chat_id: chat_id.to_string(),
                                            message_id,
                                            source_session_key: input.session_key.clone(),
                                            run_id: None,
                                            runner_id: effective.runner_id.clone(),
                                            adapter: settings.adapter.clone(),
                                            transport: crate::im_gateway::external_cli::resolved_transport_name_for_request(&request)
                                                .unwrap_or("exec")
                                                .to_string(),
                                            external_thread_id: None,
                                            external_turn_id: None,
                                            checkpoint_thread_id: None,
                                            status: "pending".to_string(),
                                        },
                                        now_ms(),
                                    );
                                    }
                                }
                            }
                        }
                        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<
                            bifrost_agent::AgentTurnProgressEvent,
                        >();
                        progress_tx_for_finish = Some(progress_tx.clone());
                        let progress_registry = Arc::clone(ctx.progress_registry);
                        let session_key_for_progress = input.session_key.clone();
                        progress_task = Some(tokio::spawn(async move {
                            super::super::agent_chat_progress::run_progress_event_coalescer(
                                progress_registry,
                                session_key_for_progress,
                                &mut progress_rx,
                            )
                            .await;
                        }));
                        progress_enabled = true;
                        let runner_summary = external_cli_progress_runner_summary(
                            &effective.runner_id,
                            &settings.adapter,
                            &request,
                            None,
                        );
                        let _ = ctx
                            .progress_registry
                            .update_runner_summary(&input.session_key, runner_summary)
                            .await;
                    }
                    Err(error) => {
                        warn!(
                            session_key = %input.session_key,
                            error = %error,
                            "failed to start external runner progress card; final reply will be sent when the run finishes"
                        );
                    }
                }
            }
        }
        let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
            crate::im_gateway::external_cli::default_runs_root(),
        );
        let (external_progress_tx, mut external_progress_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let request_for_progress = request.clone();
        // Keep the runner control loop independently polled while this task
        // handles a default Guide message (or a legacy inbound /g). Awaiting the
        // guide acknowledgement inline otherwise stalls `run_with_progress`,
        // which is also responsible for forwarding that guide to the worker.
        let request_for_run = request.clone();
        let mut run_task = tokio::spawn(async move {
            runtime
                .run_with_progress(request_for_run, Some(external_progress_tx))
                .await
        });
        let _run_task_guard = AbortTaskOnDrop(run_task.abort_handle());
        let result = loop {
            tokio::select! {
                result = &mut run_task => break result
                    .map_err(|error| format!("external runner task failed: {error}"))
                    .and_then(|result| result),
                Some(progress_event) = external_progress_rx.recv() => {
                    if let Some(recorder) = recorder.as_mut() {
                        if let Some(end_index) = super::super::chat_gateway::record_external_cli_progress_event_to_timeline(
                            recorder,
                            &input.session_key,
                            "im",
                            &effective.runner_id,
                            &settings.adapter,
                            &progress_event,
                        ) {
                            ctx.agent_session_manager.emit_timeline_changed(
                                &input.session_key,
                                &recorder.file_path().display().to_string(),
                                Some(end_index),
                                "im_progress",
                            );
                        }
                    }
                    if progress_enabled {
                        let metadata_changed = crate::im_gateway::external_cli::merge_external_cli_progress_metadata(
                            &settings.adapter,
                            &progress_event,
                            &mut progress_runner_metadata,
                        );
                        if settings.adapter == "codex" {
                            if let (Some(chat_id), Some(thread_id), Some(turn_id)) = (
                                current_event.source.chat_id.as_deref(),
                                progress_runner_metadata.get("threadId").cloned(),
                                progress_runner_metadata.get("turnId").cloned(),
                            ) {
                                for message_info in ctx
                                    .progress_registry
                                    .message_infos(&input.session_key)
                                    .await
                                {
                                    if let Some(message_id) = message_info.message_id {
                                        let _ = ctx.group_context_store.upsert_feishu_message_anchor(
                                            &crate::im_gateway::group_context::FeishuMessageAnchor {
                                                provider_id: ctx.provider.id.clone(),
                                                chat_id: chat_id.to_string(),
                                                message_id,
                                                source_session_key: input.session_key.clone(),
                                                run_id: None,
                                                runner_id: effective.runner_id.clone(),
                                                adapter: settings.adapter.clone(),
                                                transport: crate::im_gateway::external_cli::resolved_transport_name_for_request(&request)
                                                    .unwrap_or("exec")
                                                    .to_string(),
                                                external_thread_id: Some(thread_id.clone()),
                                                external_turn_id: Some(turn_id.clone()),
                                                checkpoint_thread_id: None,
                                                status: "active_ready".to_string(),
                                            },
                                            now_ms(),
                                        );
                                    }
                                }
                            }
                        }
                        if metadata_changed {
                            let runner_summary = external_cli_progress_runner_summary(
                                &effective.runner_id,
                                &settings.adapter,
                                &request_for_progress,
                                Some(&progress_runner_metadata),
                            );
                            let _ = ctx
                                .progress_registry
                                .update_runner_summary(&input.session_key, runner_summary)
                                .await;
                        }
                    }
                    if progress_enabled {
                        if let (Some(progress_tx), Some(agent_event)) = (
                            progress_tx_for_finish.as_ref(),
                            crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
                                &input.session_key,
                                &settings.adapter,
                                crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                                    Some(&effective.runner_id),
                                    resolved_model_config.model.as_deref(),
                                    resolved_model_config
                                        .model_provider
                                        .as_deref()
                                        .or(resolved_model_config.model_source.as_deref()),
                                    resolved_model_config.reasoning_effort.as_deref(),
                                    resolved_model_config.reasoning_summary.as_deref(),
                                    request_for_progress.work_dir.as_deref(),
                                ),
                                &progress_event,
                            ),
                        ) {
                            let _ = progress_tx.send(agent_event);
                        }
                    }
                }
                Some(next_event) = ctx.rx.recv() => {
                    maybe_stop_external_cli_for_event(&next_event, &input.session_key).await;
                    handle_concurrent_event_during_chat(
                        &next_event,
                        ctx.provider,
                        &input.session_key,
                        ctx.queue_manager,
                        ctx.client,
                        ctx.message_log_store,
                        ctx.agent_session_manager,
                        ctx.progress_registry,
                        ctx.agent_config_store,
                        ctx.provider_store,
                        ctx.event_store,
                        ctx.group_context_store,
                        ctx.external_cli_config_store,
                        busy_default_mode_for_external_adapter(&settings.adapter),
                    ).await;
                }
            }
        };
        while let Ok(progress_event) = external_progress_rx.try_recv() {
            if let Some(recorder) = recorder.as_mut() {
                if let Some(end_index) =
                    super::super::chat_gateway::record_external_cli_progress_event_to_timeline(
                        recorder,
                        &input.session_key,
                        "im",
                        &effective.runner_id,
                        &settings.adapter,
                        &progress_event,
                    )
                {
                    ctx.agent_session_manager.emit_timeline_changed(
                        &input.session_key,
                        &recorder.file_path().display().to_string(),
                        Some(end_index),
                        "im_progress",
                    );
                }
            }
            if progress_enabled {
                if let (Some(progress_tx), Some(agent_event)) = (
                    progress_tx_for_finish.as_ref(),
                    crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
                        &input.session_key,
                        &settings.adapter,
                        crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                            Some(&effective.runner_id),
                            resolved_model_config.model.as_deref(),
                            resolved_model_config
                                .model_provider
                                .as_deref()
                                .or(resolved_model_config.model_source.as_deref()),
                            resolved_model_config.reasoning_effort.as_deref(),
                            resolved_model_config.reasoning_summary.as_deref(),
                            request_for_progress.work_dir.as_deref(),
                        ),
                        &progress_event,
                    ),
                ) {
                    let _ = progress_tx.send(agent_event);
                }
            }
        }
        match result {
            Ok(mut result) => {
                let run_succeeded = matches!(
                    result.status,
                    crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
                );
                if !run_succeeded {
                    let failure_reply = external_cli_non_success_reply(&result);
                    result.response = failure_reply.clone();
                    result.responses = vec![failure_reply];
                }
                if let Some(turn_id) = current_group_turn_id.take() {
                    let status_result = if run_succeeded {
                        ctx.group_context_store
                            .mark_turn_completed(&turn_id, now_ms())
                    } else {
                        ctx.group_context_store.mark_turn_failed(
                            &turn_id,
                            &result.response,
                            now_ms(),
                        )
                    };
                    if let Err(error) = status_result {
                        warn!(turn_id = %turn_id, error = %error, "failed to finalize group turn");
                    }
                }
                finalize_live_guide_group_turns(
                    ctx.queue_manager,
                    ctx.group_context_store,
                    &input.session_key,
                    if run_succeeded {
                        Ok(())
                    } else {
                        Err(result.response.as_str())
                    },
                );
                remember_external_cli_result_metadata(&mut runner_metadata, &result.metadata);
                let traex_checkpoint_thread_id = if should_create_traex_checkpoint(
                    run_succeeded,
                    &settings.adapter,
                    &request_for_progress,
                ) {
                    if let Some(source_thread_id) = result.metadata.get("threadId") {
                        let mut checkpoint_request = request_for_progress.clone();
                        checkpoint_request.message.clear();
                        checkpoint_request.operation = "checkpoint_fork".to_string();
                        checkpoint_request.params = serde_json::json!({
                            "threadId": source_thread_id,
                        });
                        match crate::im_gateway::external_cli::ExternalCliRuntime::new(
                            crate::im_gateway::external_cli::default_runs_root(),
                        )
                        .run(checkpoint_request)
                        .await
                        {
                            Ok(checkpoint) => checkpoint.metadata.get("threadId").cloned(),
                            Err(error) => {
                                warn!(error = %error, "failed to create Traex checkpoint fork");
                                None
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let anchor_status = if !run_succeeded
                    || (settings.adapter == crate::im_gateway::external_cli::TRAEX_ADAPTER
                        && traex_checkpoint_thread_id.is_none())
                {
                    "failed"
                } else {
                    "ready"
                };
                if let (Some(chat_id), Some((thread_id, _))) = (
                    current_event.source.chat_id.as_deref(),
                    crate::im_gateway::group_context::feishu_thread_parts(&current_event),
                ) {
                    let _ = ctx.group_context_store.update_feishu_thread_binding_state(
                        &ctx.provider.id,
                        chat_id,
                        thread_id,
                        anchor_status,
                        now_ms(),
                    );
                }
                let terminal_anchor = current_event.source.chat_id.as_deref().map(|chat_id| {
                    crate::im_gateway::group_context::FeishuMessageAnchor {
                        provider_id: ctx.provider.id.clone(),
                        chat_id: chat_id.to_string(),
                        message_id: String::new(),
                        source_session_key: input.session_key.clone(),
                        run_id: Some(result.run_id.clone()),
                        runner_id: effective.runner_id.clone(),
                        adapter: settings.adapter.clone(),
                        transport:
                            crate::im_gateway::external_cli::resolved_transport_name_for_request(
                                &request_for_progress,
                            )
                            .unwrap_or("exec")
                            .to_string(),
                        external_thread_id: result.metadata.get("threadId").cloned(),
                        external_turn_id: result.metadata.get("turnId").cloned(),
                        checkpoint_thread_id: traex_checkpoint_thread_id.clone(),
                        status: anchor_status.to_string(),
                    }
                });
                if let Some(chat_id) = current_event.source.chat_id.as_deref() {
                    for message_info in ctx
                        .progress_registry
                        .message_infos(&input.session_key)
                        .await
                    {
                        if let Some(message_id) = message_info.message_id {
                            let thread_id = result.metadata.get("threadId").cloned();
                            let turn_id = result.metadata.get("turnId").cloned();
                            let _ = ctx.group_context_store.upsert_feishu_message_anchor(
                                &crate::im_gateway::group_context::FeishuMessageAnchor {
                                    provider_id: ctx.provider.id.clone(),
                                    chat_id: chat_id.to_string(),
                                    message_id,
                                    source_session_key: input.session_key.clone(),
                                    run_id: Some(result.run_id.clone()),
                                    runner_id: effective.runner_id.clone(),
                                    adapter: settings.adapter.clone(),
                                    transport: crate::im_gateway::external_cli::resolved_transport_name_for_request(&request_for_progress)
                                        .unwrap_or("exec")
                                        .to_string(),
                                    external_thread_id: thread_id.clone(),
                                    external_turn_id: turn_id,
                                    checkpoint_thread_id: traex_checkpoint_thread_id.clone(),
                                    status: anchor_status.to_string(),
                                },
                                now_ms(),
                            );
                        }
                    }
                }
                record_external_cli_result(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &result,
                );
                emit_external_cli_timeline_changed(
                    ctx.agent_session_manager,
                    recorder.as_ref(),
                    &input.session_key,
                    "im_turn_finished",
                );
                remember_session_state_values(
                    &input.session_key,
                    &settings.adapter,
                    Some(&effective.runner_id),
                    session.external_conversation_id.clone(),
                    session.external_thread_id.clone(),
                    recorder
                        .as_ref()
                        .map(|recorder| recorder.file_path().display().to_string()),
                    session.work_dir.clone(),
                );
                if progress_enabled {
                    let runner_summary = external_cli_progress_runner_summary(
                        &effective.runner_id,
                        &settings.adapter,
                        &request_for_progress,
                        Some(&result.metadata),
                    );
                    let _ = ctx
                        .progress_registry
                        .update_runner_summary(&input.session_key, runner_summary)
                        .await;
                    if let Some(progress_tx) = progress_tx_for_finish.take() {
                        let event = if run_succeeded {
                            bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                                content: result.response.clone(),
                            }
                        } else {
                            bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                                error: result.response.clone(),
                            }
                        };
                        let _ = progress_tx.send(event);
                        drop(progress_tx);
                    }
                    if let Some(task) = progress_task.take() {
                        finish_progress_task(task).await;
                    }
                    finish_external_runner_progress_and_notify(
                        ExternalRunnerProgressFinishContext {
                            progress_registry: ctx.progress_registry,
                            client: ctx.client,
                            provider: ctx.provider,
                            message_log_store: ctx.message_log_store,
                            group_context_store: ctx.group_context_store,
                            event: &current_event,
                        },
                        ExternalRunnerProgressFinish {
                            session_key: &input.session_key,
                            final_text: &result.response,
                            failed: !run_succeeded,
                            work_dir: request.work_dir.as_deref(),
                            anchor: terminal_anchor.clone(),
                        },
                    )
                    .await;
                } else if !matches!(
                    delivery_mode,
                    crate::im_gateway::external_cli::ExternalCliDeliveryMode::NoIm
                ) {
                    // Send individual response messages separately when there
                    // are multiple (e.g. ChatGPT thinking + answer).
                    let responses_to_send: Vec<String> = if result.responses.len() > 1 {
                        result
                            .responses
                            .iter()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    } else {
                        vec![result.response.trim().to_string()]
                    };
                    for (idx, reply_text) in responses_to_send.iter().enumerate() {
                        let reply = if matches!(
                            delivery_mode,
                            crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
                        ) && idx == responses_to_send.len() - 1
                        {
                            // Only append run_id to the last message
                            format!("{}\n\n_run: `{}`_", reply_text, result.run_id)
                        } else {
                            reply_text.clone()
                        };
                        let message_id = send_agent_reply_from_work_dir(
                            ctx.client,
                            ctx.provider,
                            &current_event,
                            &reply,
                            ctx.message_log_store,
                            request.work_dir.as_deref(),
                        )
                        .await;
                        if let (Some(message_id), Some(mut anchor)) =
                            (message_id, terminal_anchor.clone())
                        {
                            anchor.message_id = message_id;
                            let _ = ctx
                                .group_context_store
                                .upsert_feishu_message_anchor(&anchor, now_ms());
                        }
                    }
                }
            }
            Err(error) => {
                if let (Some(chat_id), Some((thread_id, _))) = (
                    current_event.source.chat_id.as_deref(),
                    crate::im_gateway::group_context::feishu_thread_parts(&current_event),
                ) {
                    let _ = ctx.group_context_store.update_feishu_thread_binding_state(
                        &ctx.provider.id,
                        chat_id,
                        thread_id,
                        "failed",
                        now_ms(),
                    );
                }
                if let Some(turn_id) = current_group_turn_id.take() {
                    if let Err(status_error) =
                        ctx.group_context_store
                            .mark_turn_failed(&turn_id, &error, now_ms())
                    {
                        warn!(turn_id = %turn_id, error = %status_error, "failed to mark group turn failed");
                    }
                }
                finalize_live_guide_group_turns(
                    ctx.queue_manager,
                    ctx.group_context_store,
                    &input.session_key,
                    Err(error.as_str()),
                );
                // Extract diagnostic screenshot path if present.
                let (clean_error, screenshot_path) = extract_diagnostic_screenshot_path(&error);
                let reply = format!("Runner failed: {}", truncate_str(&clean_error, 300));
                record_external_cli_failure(
                    &mut session,
                    &mut recorder,
                    &input.session_key,
                    &request,
                    &error,
                    &reply,
                );
                emit_external_cli_timeline_changed(
                    ctx.agent_session_manager,
                    recorder.as_ref(),
                    &input.session_key,
                    "im_turn_failed",
                );
                if progress_enabled {
                    if let Some(progress_tx) = progress_tx_for_finish.take() {
                        let _ =
                            progress_tx.send(bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                                error: reply.clone(),
                            });
                        drop(progress_tx);
                    }
                    if let Some(task) = progress_task.take() {
                        finish_progress_task(task).await;
                    }
                    finish_external_runner_progress_and_notify(
                        ExternalRunnerProgressFinishContext {
                            progress_registry: ctx.progress_registry,
                            client: ctx.client,
                            provider: ctx.provider,
                            message_log_store: ctx.message_log_store,
                            group_context_store: ctx.group_context_store,
                            event: &current_event,
                        },
                        ExternalRunnerProgressFinish {
                            session_key: &input.session_key,
                            final_text: &reply,
                            failed: true,
                            work_dir: request.work_dir.as_deref(),
                            anchor: None,
                        },
                    )
                    .await;
                } else {
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        &current_event,
                        &reply,
                        ctx.message_log_store,
                    )
                    .await;
                }
                // Send diagnostic screenshot via IM if available.
                if let Some(path) = screenshot_path {
                    if let Some(target) = build_agent_reply_target(
                        ctx.provider,
                        &current_event,
                        "__diag_screenshot__",
                        "Diagnostic Screenshot",
                        "image",
                    ) {
                        let images = vec![AgentReplyLocalImage {
                            alt: "diagnostic screenshot".to_string(),
                            path,
                        }];
                        send_agent_reply_images(
                            ctx.client,
                            ctx.provider,
                            &current_event,
                            &target,
                            &images,
                            ctx.message_log_store,
                        )
                        .await;
                    }
                }
            }
        };

        let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
        if let Some(unconsumed) = bifrost_agent::session::combine_guide_messages(unconsumed_guides)
        {
            if !unconsumed.trim().is_empty() {
                let _ = ctx.queue_manager.push_queue(&input.session_key, unconsumed);
            }
        }
        match ctx.queue_manager.pop_queue_item(&input.session_key) {
            Some(next_item) => {
                current_group_turn_id = next_item
                    .context
                    .as_ref()
                    .and_then(|context| context.group_turn_id.clone());
                current_event = event_for_queue_item(ctx.event, next_item.context.as_ref());
                if matches!(
                    delivery_mode,
                    crate::im_gateway::external_cli::ExternalCliDeliveryMode::ProgressCard
                ) {
                    let remaining = ctx.queue_manager.queue_status(&input.session_key).len();
                    send_agent_reply(
                        ctx.client,
                        ctx.provider,
                        &current_event,
                        &format!("开始处理排队消息，当前剩余 {remaining} 条。"),
                        ctx.message_log_store,
                    )
                    .await;
                }
                current_message = next_item.message;
                current_images = external_cli_images_from_chat_images(next_item.images);
                current_files = next_item.files;
            }
            None => {
                // This is the runner's last mailbox-consumption boundary. Close
                // the receiver before the remaining session/progress cleanup so
                // a same-session event cannot be accepted by a mailbox that no
                // longer has a consumer. Events already accepted are drained by
                // the task wrapper and replayed through the provider loop;
                // later sends are buffered by SessionMailboxRegistry and replayed
                // after the matching generation completes.
                close_session_mailbox(ctx.rx);
                break;
            }
        };
    }
    if recorder.is_some() && !session.history_cleared {
        session.recorder = recorder;
    }
    remember_session_state_from_agent_session(
        &session,
        &settings.adapter,
        Some(&effective.runner_id),
    );
    ctx.queue_manager.clear_session(&input.session_key);
    ctx.agent_session_manager.return_session(session);
}

pub(super) fn external_cli_non_success_reply(
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) -> String {
    match result.status {
        crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut => format!(
            "Runner failed: external CLI timed out after {} seconds.\n\n_run: `{}`_",
            std::cmp::max(1, result.duration_ms / 1000),
            result.run_id
        ),
        crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped => format!(
            "Runner stopped before completion.\n\n_run: `{}`_",
            result.run_id
        ),
        crate::im_gateway::external_cli::ExternalCliRunStatus::Failed => {
            let detail = if result.response.trim().is_empty() {
                match result.exit_code {
                    Some(code) => format!("exit_code={code}"),
                    None => "external CLI exited unsuccessfully".to_string(),
                }
            } else {
                truncate_str(&result.response, 260)
            };
            format!("Runner failed: {detail}\n\n_run: `{}`_", result.run_id)
        }
        crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded => result.response.clone(),
    }
}

pub(in crate::handlers::im_gateway) fn apply_external_cli_resume_metadata(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    metadata: &std::collections::BTreeMap<String, String>,
) {
    if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        if request
            .params
            .get("conversationId")
            .or_else(|| request.params.get("conversation_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            return;
        }
        let Some(conversation_id) = metadata
            .get("conversationId")
            .or_else(|| metadata.get("conversation_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        if !request.params.is_object() {
            request.params = serde_json::json!({});
        }
        if let Some(params) = request.params.as_object_mut() {
            params.insert(
                "conversationId".to_string(),
                serde_json::Value::String(conversation_id.to_string()),
            );
        }
        return;
    }
    if !matches!(
        request.adapter.as_str(),
        "codex"
            | crate::im_gateway::external_cli::TRAEX_ADAPTER
            | crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER
    ) {
        return;
    }
    if request
        .params
        .get("threadId")
        .or_else(|| request.params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return;
    }
    let Some(thread_id) = metadata
        .get("threadId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.insert(
            "threadId".to_string(),
            serde_json::Value::String(thread_id.to_string()),
        );
    }
}

pub(super) fn remember_external_cli_result_metadata(
    metadata: &mut std::collections::BTreeMap<String, String>,
    result_metadata: &std::collections::BTreeMap<String, String>,
) {
    if let Some(thread_id) = result_metadata
        .get("threadId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("threadId".to_string(), thread_id.to_string());
    }
    if let Some(conversation_id) = result_metadata
        .get("conversationId")
        .or_else(|| result_metadata.get("conversation_id"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("conversationId".to_string(), conversation_id.to_string());
    }
}

pub(super) fn ensure_external_cli_session_recorder(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    provider: &ImProviderConfig,
    runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    let adapter_label = external_cli_adapter_label(&request.adapter);
    if session.source == "unknown" {
        session.source = request.adapter.clone();
    }
    session.mark_external_runner_runtime(runner_id, &request.adapter);
    sync_external_cli_active_status(session);
    if session.title.is_none() {
        session.title = Some(format!(
            "{}: {}",
            adapter_label,
            truncate_str(request.message.trim(), 48)
        ));
    }
    if session.work_dir.is_none() {
        session.work_dir = request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string());
    }

    if recorder.is_none() {
        let data_dir = bifrost_agent::config::agent_home_dir();
        match ConversationRecorder::open_or_create(&data_dir, session_key, None) {
            Ok((mut rec, created)) => {
                if created {
                    if let Err(error) = rec.record_session_start(
                        session_key,
                        serde_json::json!({
                            "source": request.adapter,
                            "runtime": request.runtime,
                            "adapter": request.adapter,
                            "runner_id": runner_id,
                            "provider_id": provider.id,
                            "provider_type": format!("{:?}", provider.provider_type).to_lowercase(),
                            "work_dir": request.work_dir.as_ref().map(|path| path.display().to_string()),
                        }),
                    ) {
                        warn!(error = %error, "failed to record external cli session start");
                    }
                    if let Some(title) = session.title.as_deref() {
                        if let Err(error) = rec.record_title_updated(session_key, title) {
                            warn!(error = %error, "failed to record external cli session title");
                        }
                    }
                }
                *recorder = Some(rec);
            }
            Err(error) => {
                warn!(session_key = %session_key, error = %error, "failed to open the canonical external cli session history");
            }
        }
    }
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) =
            rec.record_run_state(session_key, "running", Some("im"), Some(runner_id))
        {
            warn!(error = %error, "failed to record external cli running state");
        }
    }
}

pub(super) fn apply_external_cli_session_attachment_base_dir(
    request: &mut crate::im_gateway::external_cli::ExternalCliRunRequest,
    recorder: Option<&ConversationRecorder>,
) {
    let Some(recorder) = recorder else {
        return;
    };
    let Some(session_dir) = recorder.file_path().parent() else {
        return;
    };
    let Some(session_stem) = recorder.file_path().file_stem() else {
        return;
    };
    if !request.params.is_object() {
        request.params = serde_json::json!({});
    }
    if let Some(params) = request.params.as_object_mut() {
        params.remove("attachment_base_dir");
        params.insert(
            "attachmentBaseDir".to_string(),
            serde_json::Value::String(
                session_dir
                    .join("attachments")
                    .join(session_stem)
                    .display()
                    .to_string(),
            ),
        );
        params.insert(
            "historyPath".to_string(),
            serde_json::Value::String(recorder.file_path().display().to_string()),
        );
    }
}

pub(super) fn external_cli_request_chat_images(
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) -> Vec<bifrost_agent::ChatImageInput> {
    request
        .images
        .iter()
        .filter(|image| !image.data.trim().is_empty())
        .map(|image| bifrost_agent::ChatImageInput {
            mime_type: image.mime_type.clone(),
            data: image.data.clone(),
        })
        .collect()
}

pub(super) fn record_external_cli_input(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    _runner_id: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
) {
    let images = external_cli_request_chat_images(request);
    append_session_message(
        session,
        bifrost_agent::ChatMessage::user_with_images(&request.message, &images),
    );
    sync_external_cli_active_status(session);
    if let Some(rec) = recorder.as_mut() {
        if let Err(error) =
            rec.record_user_message_with_images(session_key, &request.message, &images)
        {
            warn!(error = %error, "failed to record external cli user message");
        }
    }
}

pub(super) fn emit_external_cli_timeline_changed(
    agent_session_manager: &std::sync::Arc<bifrost_agent::AgentSessionManager>,
    recorder: Option<&ConversationRecorder>,
    session_key: &str,
    reason: &str,
) {
    let Some(recorder) = recorder else {
        return;
    };
    agent_session_manager.emit_timeline_changed(
        session_key,
        &recorder.file_path().display().to_string(),
        recorder.event_count(),
        reason,
    );
}

pub(super) fn record_external_cli_result(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    result: &crate::im_gateway::external_cli::ExternalCliRunResult,
) {
    session.remember_external_conversation_ref(
        result
            .metadata
            .get("conversationId")
            .or_else(|| result.metadata.get("conversation_id"))
            .cloned(),
        result
            .metadata
            .get("threadId")
            .or_else(|| result.metadata.get("thread_id"))
            .cloned(),
    );
    sync_external_cli_active_status(session);
    append_session_message(
        session,
        bifrost_agent::ChatMessage::assistant(&result.response),
    );
    sync_external_cli_active_status(session);
    if let Some(rec) = recorder.as_mut() {
        let run_state = if matches!(
            result.status,
            crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
        ) {
            "completed"
        } else {
            "failed"
        };
        if let Err(error) =
            rec.record_run_state(session_key, run_state, Some("im"), Some(&result.adapter))
        {
            warn!(error = %error, "failed to record external cli run state");
        }
        if let Err(error) = rec.record_assistant_message(session_key, &result.response) {
            warn!(error = %error, "failed to record external cli assistant message");
        }
    }
}

pub(super) fn record_external_cli_failure(
    session: &mut bifrost_agent::session::AgentSession,
    recorder: &mut Option<ConversationRecorder>,
    session_key: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    _error: &str,
    reply: &str,
) {
    append_session_message(session, bifrost_agent::ChatMessage::assistant(reply));
    if let Some(rec) = recorder.as_mut() {
        if let Err(record_error) =
            rec.record_run_state(session_key, "failed", Some("im"), Some(&request.adapter))
        {
            warn!(error = %record_error, "failed to record external cli failure state");
        }
        if let Err(record_error) = rec.record_assistant_message(session_key, reply) {
            warn!(error = %record_error, "failed to record external cli failure message");
        }
    }
}

pub(super) fn append_session_message(
    session: &mut bifrost_agent::session::AgentSession,
    message: bifrost_agent::ChatMessage,
) {
    session.history.push(message);
    session.last_active_at = now_ms() / 1000;
    session.history_version = session.history_version.saturating_add(1);
}

pub(super) fn sync_external_cli_active_status(session: &bifrost_agent::session::AgentSession) {
    let Some(handle) = session.active_turn_status.as_ref() else {
        return;
    };
    let Ok(mut status) = handle.lock() else {
        return;
    };
    status.agent_type = session.agent_type.clone();
    status.runner_type = session.runner_type.clone();
    status.runner_id = session.runner_id.clone();
    status.model = session.model.clone();
    status.model_provider = session.model_provider.clone();
    status.model_reasoning_effort = session.model_reasoning_effort.clone();
    status.model_reasoning_summary = session.model_reasoning_summary.clone();
    status.external_conversation_id = session.external_conversation_id.clone();
    status.external_thread_id = session.external_thread_id.clone();
    status.user_turn_count = session.user_turn_count();
    status.message_count = session.history.len();
    status.work_dir = session.work_dir.clone();
    status.history_version = session.history_version;
    status.compaction_count = session.compaction_count;
    status.total_tokens_used = session.total_tokens_used;
    status.estimated_context_tokens = session.effective_token_count();
}

pub(super) fn external_cli_adapter_label(adapter: &str) -> &'static str {
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        "ChatGPT Web"
    } else {
        "Runner"
    }
}

pub(in crate::handlers::im_gateway) fn external_cli_progress_runner_summary(
    runner_id: &str,
    adapter: &str,
    request: &crate::im_gateway::external_cli::ExternalCliRunRequest,
    metadata: Option<&std::collections::BTreeMap<String, String>>,
) -> crate::im_gateway::progress_card::ProgressRunnerSummary {
    let resolved_model_config = crate::im_gateway::external_cli::resolve_external_cli_model_config(
        &request.adapter,
        &request.adapter_config,
    );
    let configured_model = resolved_model_config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let metadata_model = metadata
        .and_then(|metadata| metadata.get("model"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let model = configured_model.or(metadata_model).map(str::to_string);
    let model_source = if configured_model.is_some() {
        resolved_model_config
            .model_provider
            .clone()
            .or_else(|| resolved_model_config.model_source.clone())
            .map(|value| format_runner_model_source(&value))
    } else {
        metadata
            .and_then(|metadata| metadata.get("modelSource"))
            .map(String::as_str)
            .map(format_runner_model_source)
            .filter(|value| !value.trim().is_empty())
    };
    let external_thread_id = metadata.and_then(|metadata| {
        metadata
            .get("threadId")
            .or_else(|| metadata.get("thread_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let external_conversation_id = metadata.and_then(|metadata| {
        metadata
            .get("conversationId")
            .or_else(|| metadata.get("conversation_id"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    crate::im_gateway::progress_card::ProgressRunnerSummary {
        runner_id: runner_id.trim().to_string(),
        adapter: adapter.trim().to_string(),
        model,
        model_source,
        reasoning_effort: resolved_model_config.reasoning_effort.or_else(|| {
            metadata.and_then(|metadata| metadata.get("modelReasoningEffort").cloned())
        }),
        reasoning_summary: resolved_model_config.reasoning_summary.or_else(|| {
            metadata.and_then(|metadata| metadata.get("modelReasoningSummary").cloned())
        }),
        reasoning_source: resolved_model_config
            .reasoning_source
            .map(|value| format_runner_model_source(&value)),
        token_usage: metadata.and_then(external_cli_token_usage_from_metadata),
        weekly_usage: metadata.and_then(external_cli_weekly_usage_from_metadata),
        work_dir: request
            .work_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        external_thread_id,
        external_conversation_id,
    }
}

pub(super) fn external_cli_weekly_usage_from_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
) -> Option<crate::im_gateway::progress_card::ProgressRunnerWeeklyUsage> {
    Some(
        crate::im_gateway::progress_card::ProgressRunnerWeeklyUsage {
            used_percent: metadata_u64(metadata, "codexWeeklyUsedPercent")?.min(100),
            window_minutes: metadata_u64(metadata, "codexWeeklyWindowMinutes")?,
            resets_at: metadata_u64(metadata, "codexWeeklyResetsAt"),
        },
    )
}

pub(super) fn external_cli_token_usage_from_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
) -> Option<crate::im_gateway::progress_card::ProgressRunnerTokenUsage> {
    let usage = crate::im_gateway::progress_card::ProgressRunnerTokenUsage {
        input_tokens: metadata_u64(metadata, "usageInputTokens"),
        cached_input_tokens: metadata_u64(metadata, "usageCachedInputTokens"),
        output_tokens: metadata_u64(metadata, "usageOutputTokens"),
        reasoning_output_tokens: metadata_u64(metadata, "usageReasoningOutputTokens"),
        total_tokens: metadata_u64(metadata, "usageTotalTokens"),
    };
    (usage.input_tokens.is_some()
        || usage.cached_input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.reasoning_output_tokens.is_some()
        || usage.total_tokens.is_some())
    .then_some(usage)
}

pub(super) fn metadata_u64(
    metadata: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Option<u64> {
    metadata.get(key)?.trim().parse().ok()
}

pub(super) fn format_runner_model_source(source: &str) -> String {
    match source.trim() {
        "runner config" => "runner 配置".to_string(),
        "codex default" => "Codex 默认".to_string(),
        "trae default" => "Trae 默认".to_string(),
        "codex config" => "Codex 配置".to_string(),
        "trae config" => "Trae 配置".to_string(),
        value => value.to_string(),
    }
}

pub(super) async fn maybe_stop_external_cli_for_event(event: &ImEvent, active_session_key: &str) {
    let Some(message) = event.message.as_ref() else {
        return;
    };
    let msg_text = agent_message_text(message);
    if msg_text.trim() != "/stop" {
        return;
    }
    let session_key = session_key_for_event(event);
    if session_key != active_session_key {
        return;
    }
    if let Err(error) = crate::im_gateway::external_cli::request_session_stop(
        crate::im_gateway::external_cli::default_runs_root(),
        active_session_key,
    )
    .await
    {
        debug!(session_key = %active_session_key, error = %error, "external cli session stop was not applied");
    }
}

pub(super) fn session_key_for_event(event: &ImEvent) -> String {
    if crate::im_gateway::group_context::is_feishu_group_event(event) {
        let chat_id = event.source.chat_id.as_deref().unwrap_or_default();
        if let Some((thread_id, _)) = crate::im_gateway::group_context::feishu_thread_parts(event) {
            crate::im_gateway::group_context::build_group_thread_session_key(
                &event.provider_id,
                chat_id,
                thread_id,
            )
        } else {
            crate::im_gateway::group_context::build_group_session_key(&event.provider_id, chat_id)
        }
    } else {
        build_session_key(&event.provider_id, event.source.user_id.as_deref())
    }
}
