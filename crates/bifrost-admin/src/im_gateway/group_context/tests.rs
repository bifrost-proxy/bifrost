use super::*;
use crate::im_gateway::types::{ImEventSource, ImImageAttachment, ImImageSource, ImProviderType};

fn group_event(
    event_id: &str,
    chat_id: &str,
    sender: &str,
    text: &str,
    mentions: Vec<ImMention>,
    received_at: u64,
) -> ImEvent {
    ImEvent {
        event_id: event_id.to_string(),
        provider_id: "feishu-main".to_string(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: ImEventSource {
            chat_id: Some(chat_id.to_string()),
            chat_type: Some("group".to_string()),
            user_id: Some(sender.to_string()),
            user_name: None,
            sender_type: Some("user".to_string()),
            message_id: Some(event_id.to_string()),
        },
        message: Some(ImEventMessage {
            text: text.to_string(),
            mentions,
            images: Vec::new(),
            raw_type: Some("text".to_string()),
            raw_content: Some(serde_json::json!({"text": text})),
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

fn bot_mention() -> ImMention {
    ImMention {
        key: "@_user_1".to_string(),
        open_id: Some("ou_bot".to_string()),
        name: Some("Bifrost".to_string()),
        tenant_key: None,
        is_bot: false,
    }
}

#[test]
fn group_trigger_classifier_only_accepts_current_bot_or_slash() {
    let bot = FeishuBotIdentity {
        open_id: "ou_bot".to_string(),
        name: Some("Bifrost".to_string()),
    };
    let ambient = group_event("m1", "c1", "u1", "hello", Vec::new(), 1);
    assert_eq!(
        classify_group_message(ambient.message.as_ref().unwrap(), Some(&bot), false),
        GroupMessageDisposition::Ambient
    );

    let other_mention = ImMention {
        key: "@_user_1".to_string(),
        open_id: Some("ou_other".to_string()),
        name: Some("Other".to_string()),
        tenant_key: None,
        is_bot: false,
    };
    let other = group_event("m2", "c1", "u1", "@_user_1 hello", vec![other_mention], 2);
    assert_eq!(
        classify_group_message(other.message.as_ref().unwrap(), Some(&bot), false),
        GroupMessageDisposition::Ambient
    );

    let mentioned = group_event(
        "m3",
        "c1",
        "u1",
        "@_user_1 inspect this",
        vec![bot_mention()],
        3,
    );
    assert_eq!(
        classify_group_message(mentioned.message.as_ref().unwrap(), Some(&bot), false),
        GroupMessageDisposition::AgentTrigger {
            kind: GroupTriggerKind::Mention,
            active_request: "inspect this".to_string(),
            command_prefix: None,
        }
    );

    let slash = group_event("m4", "c1", "u1", "/status", Vec::new(), 4);
    assert_eq!(
        classify_group_message(slash.message.as_ref().unwrap(), None, false),
        GroupMessageDisposition::SystemCommand {
            command: "/status".to_string(),
            reset_context: false,
        }
    );
    let cwd = group_event("m5", "c1", "u1", "/cwd /tmp", Vec::new(), 5);
    assert_eq!(
        classify_group_message(cwd.message.as_ref().unwrap(), None, false),
        GroupMessageDisposition::SystemCommand {
            command: "/cwd /tmp".to_string(),
            reset_context: false,
        }
    );
}

#[test]
fn image_only_bot_mention_triggers_agent_instead_of_help() {
    let bot = FeishuBotIdentity {
        open_id: "ou_bot".to_string(),
        name: Some("Bifrost".to_string()),
    };
    let mut event = group_event("image", "c1", "u1", "@_user_1", vec![bot_mention()], 1);
    event.message.as_mut().unwrap().images = vec![ImImageAttachment {
        file_key: "img-1".to_string(),
        source: ImImageSource::MessageResource,
        mime_type: Some("image/png".to_string()),
        data_base64: None,
        download_url: None,
        encrypted_query_param: None,
        aes_key: None,
    }];

    assert_eq!(
        classify_group_message(event.message.as_ref().unwrap(), Some(&bot), false),
        GroupMessageDisposition::AgentTrigger {
            kind: GroupTriggerKind::Mention,
            active_request: "请理解这张图片，并根据图片内容回答。".to_string(),
            command_prefix: None,
        }
    );
}

#[test]
fn mention_only_reply_uses_quoted_message_instead_of_help() {
    let bot = FeishuBotIdentity {
        open_id: "ou_bot".to_string(),
        name: Some("Bifrost".to_string()),
    };
    let mut event = group_event("reply", "c1", "u1", "@_user_1", vec![bot_mention()], 1);
    event.message.as_mut().unwrap().parent_id = Some("quoted".to_string());

    assert_eq!(
        classify_group_message(event.message.as_ref().unwrap(), Some(&bot), false),
        GroupMessageDisposition::AgentTrigger {
            kind: GroupTriggerKind::Mention,
            active_request: String::new(),
            command_prefix: None,
        }
    );

    event.message.as_mut().unwrap().parent_id = None;
    assert_eq!(
        classify_group_message(event.message.as_ref().unwrap(), Some(&bot), false),
        GroupMessageDisposition::SystemCommand {
            command: "/help".to_string(),
            reset_context: false,
        }
    );
}

#[test]
fn mention_rendering_replaces_longer_placeholder_first() {
    let mentions = vec![
        ImMention {
            key: "@_user_1".to_string(),
            open_id: Some("ou_one".to_string()),
            name: Some("One".to_string()),
            tenant_key: None,
            is_bot: false,
        },
        ImMention {
            key: "@_user_10".to_string(),
            open_id: Some("ou_ten".to_string()),
            name: Some("Ten".to_string()),
            tenant_key: None,
            is_bot: false,
        },
    ];

    assert_eq!(
        render_message_mentions("@_user_10 and @_user_1", &mentions),
        "<at id=ou_ten>Ten</at> and <at id=ou_one>One</at>"
    );
}

#[test]
fn chat_name_lookup_backoff_throttles_failures_and_clears_on_success() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let event = group_event("m1", "c1", "u1", "hello", Vec::new(), 1);
    store.record_event(&event, "event").unwrap();

    assert!(store.begin_chat_name_lookup("feishu-main", "c1", 1_000));
    assert!(!store.begin_chat_name_lookup("feishu-main", "c1", 1_001));
    assert!(store.begin_chat_name_lookup("feishu-main", "c1", 61_000));
    assert!(store
        .set_chat_name("feishu-main", "c1", "Engineering", 61_001)
        .unwrap());
    assert!(store.begin_chat_name_lookup("feishu-main", "c1", 61_002));
}

#[test]
fn slash_classification_matches_direct_message_command_boundaries() {
    let model_fallbacks = [
        "/help extra",
        "/clear extra",
        "/reset extra",
        "/CWD /tmp",
        "/rq 1",
    ];
    for (index, text) in model_fallbacks.into_iter().enumerate() {
        let event = group_event(
            &format!("model-{index}"),
            "c1",
            "u1",
            text,
            Vec::new(),
            index as u64,
        );
        assert_eq!(
            classify_group_message(event.message.as_ref().unwrap(), None, false),
            GroupMessageDisposition::AgentTrigger {
                kind: GroupTriggerKind::Slash,
                active_request: text.to_string(),
                command_prefix: None,
            },
            "{text} should follow the direct-message model fallback"
        );
    }

    let busy_remove = group_event("busy-rq", "c1", "u1", "/rq 1", Vec::new(), 9);
    assert_eq!(
        classify_group_message(busy_remove.message.as_ref().unwrap(), None, true),
        GroupMessageDisposition::SystemCommand {
            command: "/rq 1".to_string(),
            reset_context: false,
        }
    );

    for text in ["/Runner Codex", "/models extra", "/effort invalid"] {
        let event = group_event(text, "c1", "u1", text, Vec::new(), 10);
        assert_eq!(
            classify_group_message(event.message.as_ref().unwrap(), None, false),
            GroupMessageDisposition::SystemCommand {
                command: text.to_string(),
                reset_context: false,
            },
            "{text} should use the direct-message command/error path"
        );
    }
}

#[test]
fn same_group_multiple_bots_only_trigger_the_matching_provider_identity() {
    let bot_a = FeishuBotIdentity {
        open_id: "ou_bot_a".to_string(),
        name: Some("Shared Bot Name".to_string()),
    };
    let bot_b = FeishuBotIdentity {
        open_id: "ou_bot_b".to_string(),
        name: Some("Shared Bot Name".to_string()),
    };
    let mention_b = ImMention {
        key: "@_user_1".to_string(),
        open_id: Some("ou_bot_b".to_string()),
        name: Some("Shared Bot Name".to_string()),
        tenant_key: None,
        is_bot: true,
    };
    let event = group_event(
        "multi-bot",
        "shared-chat",
        "u1",
        "@_user_1 only bot b should answer",
        vec![mention_b],
        11,
    );

    assert_eq!(
        classify_group_message(event.message.as_ref().unwrap(), Some(&bot_a), false),
        GroupMessageDisposition::Ambient
    );
    assert_eq!(
        classify_group_message(event.message.as_ref().unwrap(), Some(&bot_b), false),
        GroupMessageDisposition::AgentTrigger {
            kind: GroupTriggerKind::Mention,
            active_request: "only bot b should answer".to_string(),
            command_prefix: None,
        }
    );
    assert_ne!(
        build_group_session_key("provider-a", "shared-chat"),
        build_group_session_key("provider-b", "shared-chat")
    );
}

#[test]
fn feishu_sender_at_uses_empty_label_fallback_and_escapes_markup() {
    assert_eq!(
        feishu_sender_at("ou_alice", Some("Alice")),
        "<at id=ou_alice>Alice</at>"
    );
    assert_eq!(
        feishu_sender_at("ou_<unsafe>", Some("A&B")),
        "<at id=ou_&lt;unsafe&gt;>A&amp;B</at>"
    );
    assert_eq!(
        feishu_sender_at("ou_unknown", None),
        "<at id=ou_unknown></at>"
    );
}

#[test]
fn group_store_freezes_non_overlapping_incremental_turns() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let first = group_event("m1", "c1", "u1", "first context", Vec::new(), 1);
    let second = group_event("m2", "c1", "u2", "second context", Vec::new(), 2);
    let trigger = group_event("m3", "c1", "u1", "@_user_1 do it", vec![bot_mention()], 3);
    store.record_event(&first, "websocket").unwrap();
    store.record_event(&second, "websocket").unwrap();
    store.record_event(&trigger, "websocket").unwrap();
    assert!(store
        .set_chat_name("feishu-main", "c1", "发布讨论群", 3)
        .unwrap());
    let turn = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "do it")
        .unwrap();
    assert_eq!(turn.message_count, 3);
    assert!(turn.prompt.contains("first context"));
    assert!(turn.prompt.contains("second context"));
    assert!(turn.prompt.contains("群名称：发布讨论群"));
    assert!(turn.prompt.contains("群 ID：c1"));
    assert!(turn.prompt.contains("<at id=u1></at>：do it"));
    assert_eq!(turn.prompt.matches("do it").count(), 1);
    for internal_field in [
        "provider_id",
        "session_key",
        "message_id",
        "sender_open_id",
        "attachment_count",
    ] {
        assert!(!turn.prompt.contains(internal_field), "{internal_field}");
    }

    let status = group_event("m4", "c1", "u2", "@_all /status", Vec::new(), 4);
    let mut next_context = group_event("m5", "c1", "u2", "new context", Vec::new(), 5);
    next_context.message.as_mut().unwrap().images.push(
        crate::im_gateway::types::ImImageAttachment {
            file_key: "img_1".to_string(),
            source: Default::default(),
            mime_type: None,
            data_base64: None,
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        },
    );
    let next_trigger = group_event(
        "m6",
        "c1",
        "u1",
        "@_user_1 continue",
        vec![bot_mention()],
        6,
    );
    store.record_event(&status, "websocket").unwrap();
    store.record_event(&next_context, "websocket").unwrap();
    store.record_event(&next_trigger, "websocket").unwrap();
    let next = store
        .prepare_turn(&next_trigger, GroupTriggerKind::Mention, "continue")
        .unwrap();
    assert_eq!(next.message_count, 3);
    assert!(!next.prompt.contains("first context"));
    assert!(!next.prompt.contains("/status"));
    assert!(next.prompt.contains("new context"));
    assert!(next.prompt.contains("new context [附件 1 个]"));
    assert!(next.prompt.contains("<at id=u1></at>：continue"));
    assert!(!next.prompt.contains("群名称："));
    assert!(!next.prompt.contains("群 ID："));
}

#[test]
fn quoted_message_before_cursor_is_loaded_as_the_main_input() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let quoted = group_event(
        "quoted-old",
        "c1",
        "u2",
        "请检查这个发布方案",
        Vec::new(),
        1,
    );
    let first_trigger = group_event(
        "first-trigger",
        "c1",
        "u1",
        "@_user_1 first",
        vec![bot_mention()],
        2,
    );
    store.record_event(&quoted, "event").unwrap();
    store.record_event(&first_trigger, "event").unwrap();
    store
        .prepare_turn(&first_trigger, GroupTriggerKind::Mention, "first")
        .unwrap();

    let mut reply = group_event(
        "reply-trigger",
        "c1",
        "u1",
        "@_user_1",
        vec![bot_mention()],
        3,
    );
    reply.message.as_mut().unwrap().parent_id = Some("quoted-old".to_string());
    store.record_event(&reply, "event").unwrap();
    let turn = store
        .prepare_turn(&reply, GroupTriggerKind::Mention, "")
        .unwrap();

    assert_eq!(turn.message_count, 1);
    assert!(turn.prompt.contains("本轮主要处理对象（来自被引用消息）"));
    assert!(turn.prompt.contains("<at id=u2></at>：请检查这个发布方案"));
    assert_eq!(turn.prompt.matches("请检查这个发布方案").count(), 1);
    assert!(turn
        .prompt
        .contains("当前用户未附加文字；请直接理解并回应上述被引用消息"));
    assert!(!turn.prompt.contains("/help"));
}

#[test]
fn quoted_message_in_current_range_is_not_duplicated_as_background() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let quoted = group_event("quoted", "c1", "u2", "需要处理的内容", Vec::new(), 1);
    let background = group_event("background", "c1", "u3", "其他背景", Vec::new(), 2);
    let mut reply = group_event(
        "reply",
        "c1",
        "u1",
        "@_user_1 请执行它",
        vec![bot_mention()],
        3,
    );
    reply.message.as_mut().unwrap().parent_id = Some("quoted".to_string());
    for event in [&quoted, &background, &reply] {
        store.record_event(event, "event").unwrap();
    }

    let turn = store
        .prepare_turn(&reply, GroupTriggerKind::Mention, "请执行它")
        .unwrap();
    assert!(turn.prompt.contains("其他背景"));
    assert_eq!(turn.prompt.matches("需要处理的内容").count(), 1);
    assert!(turn
        .prompt
        .contains("当前用户指令：\n<at id=u1></at>：请执行它"));
}

#[test]
fn quoted_message_uses_immediate_parent_instead_of_thread_root() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let root = group_event("root", "c1", "u2", "话题根消息", Vec::new(), 1);
    let parent = group_event("parent", "c1", "u3", "直接引用的上一层消息", Vec::new(), 2);
    let mut reply = group_event("reply", "c1", "u1", "@_user_1", vec![bot_mention()], 3);
    let message = reply.message.as_mut().unwrap();
    message.root_id = Some("root".to_string());
    message.parent_id = Some("parent".to_string());
    message.thread_id = Some("thread".to_string());
    for event in [&root, &parent, &reply] {
        store.record_event(event, "event").unwrap();
    }

    let turn = store
        .prepare_turn(&reply, GroupTriggerKind::Mention, "")
        .unwrap();
    assert!(turn
        .prompt
        .contains("本轮主要处理对象（来自被引用消息）：\n<at id=u3></at>：直接引用的上一层消息"));
    assert!(!turn
        .prompt
        .contains("本轮主要处理对象（来自被引用消息）：\n<at id=u2></at>：话题根消息"));
}

#[test]
fn quoted_message_lookup_never_crosses_chat_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let foreign = group_event(
        "foreign-message",
        "other-chat",
        "u2",
        "另一个群的秘密内容",
        Vec::new(),
        1,
    );
    let mut reply = group_event("reply", "c1", "u1", "@_user_1", vec![bot_mention()], 2);
    reply.message.as_mut().unwrap().parent_id = Some("foreign-message".to_string());
    store.record_event(&foreign, "event").unwrap();
    store.record_event(&reply, "event").unwrap();

    let turn = store
        .prepare_turn(&reply, GroupTriggerKind::Mention, "")
        .unwrap();
    assert!(!turn.prompt.contains("另一个群的秘密内容"));
    assert!(turn.prompt.contains("不在当前机器人的本地群消息账本中"));
    assert!(turn.prompt.contains("请用户重新发送或补充内容"));
    assert!(!turn.prompt.contains("foreign-message"));
    assert!(turn.quoted_message_missing);
}

#[test]
fn group_store_deduplicates_messages_and_triggers() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let trigger = group_event("m1", "c1", "u1", "@_user_1 run", vec![bot_mention()], 1);
    assert_eq!(store.record_event(&trigger, "websocket").unwrap(), 1);
    assert_eq!(store.record_event(&trigger, "websocket").unwrap(), 1);
    assert_eq!(store.message_count("feishu-main", "c1").unwrap(), 1);
    let first = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "run")
        .unwrap();
    assert!(!first.duplicate);
    let duplicate = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "run")
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.turn_id, first.turn_id);
}

#[test]
fn released_undispatched_turn_is_included_in_next_trigger() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let first = group_event(
        "m1",
        "c1",
        "u1",
        "context before disabled run",
        Vec::new(),
        1,
    );
    let trigger = group_event(
        "m2",
        "c1",
        "u1",
        "@_user_1 first try",
        vec![bot_mention()],
        2,
    );
    store.record_event(&first, "websocket").unwrap();
    store.record_event(&trigger, "websocket").unwrap();
    let prepared = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "first try")
        .unwrap();
    assert!(store
        .release_turn(&prepared.turn_id, "agent disabled", 3)
        .unwrap());

    let retry = group_event("m3", "c1", "u1", "@_user_1 retry", vec![bot_mention()], 4);
    store.record_event(&retry, "websocket").unwrap();
    let retry_turn = store
        .prepare_turn(&retry, GroupTriggerKind::Mention, "retry")
        .unwrap();
    assert_eq!(retry_turn.message_count, 3);
    assert!(retry_turn.prompt.contains("context before disabled run"));
    assert!(retry_turn.prompt.contains("first try"));
}

#[test]
fn group_work_directories_are_isolated_by_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    for (message_id, chat_id) in [("m1", "c1"), ("m2", "c2")] {
        let event = group_event(message_id, chat_id, "u1", "ambient", Vec::new(), 1);
        store.record_event(&event, "websocket").unwrap();
    }
    let first = build_group_session_key("feishu-main", "c1");
    let second = build_group_session_key("feishu-main", "c2");
    assert!(store
        .set_work_dir_by_session(&first, "/workspace/one")
        .unwrap());
    assert!(store
        .set_work_dir_by_session(&second, "/workspace/two")
        .unwrap());
    assert_eq!(
        store.work_dir_by_session(&first).unwrap(),
        Some(PathBuf::from("/workspace/one"))
    );
    assert_eq!(
        store.work_dir_by_session(&second).unwrap(),
        Some(PathBuf::from("/workspace/two"))
    );
    assert!(store.set_runner_id_by_session(&first, "codex-a").unwrap());
    assert!(store.set_runner_id_by_session(&second, "codex-b").unwrap());
    assert_eq!(
        store.runner_id_by_session(&first).unwrap().as_deref(),
        Some("codex-a")
    );
    assert_eq!(
        store.runner_id_by_session(&second).unwrap().as_deref(),
        Some("codex-b")
    );
}

#[test]
fn group_store_turn_lifecycle_and_baseline_are_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    assert_eq!(
        store.file_path(),
        temp.path().join("admin").join(STORE_FILENAME)
    );
    assert_eq!(store.chat_name("missing", "missing").unwrap(), None);
    assert_eq!(store.work_dir_by_session("missing").unwrap(), None);
    assert_eq!(store.runner_id_by_session("missing").unwrap(), None);
    assert!(!store
        .set_work_dir_by_session("missing", "/tmp/missing")
        .unwrap());
    assert!(!store
        .set_runner_id_by_session("missing", "missing-runner")
        .unwrap());
    assert!(!store.set_chat_name("missing", "missing", " ", 1).unwrap());
    assert!(store.mark_turn_dispatched("missing", 1).is_err());
    assert!(store.mark_turn_failed("missing", "boom", 1).is_err());
    assert!(store.mark_turn_completed("missing", 1).is_err());
    assert!(store.release_turn("missing", "boom", 1).is_err());

    let ambient = group_event("m1", "c1", "u1", "ambient", Vec::new(), 1);
    let first_trigger = group_event("m2", "c1", "u1", "/g inspect", Vec::new(), 2);
    store.record_event(&ambient, "event").unwrap();
    store.record_event(&first_trigger, "event").unwrap();
    let first = store
        .prepare_turn(&first_trigger, GroupTriggerKind::Guide, "inspect")
        .unwrap();
    assert_eq!(
        first.delivery_message(Some("/g")),
        format!("/g {}", first.prompt)
    );
    assert_eq!(first.delivery_message(None), first.prompt);
    store.mark_turn_dispatched(&first.turn_id, 3).unwrap();
    store.mark_turn_failed(&first.turn_id, "retry", 4).unwrap();

    let second_trigger = group_event("m3", "c1", "u1", "/q continue", Vec::new(), 5);
    store.record_event(&second_trigger, "event").unwrap();
    let second = store
        .prepare_turn(&second_trigger, GroupTriggerKind::Queue, "continue")
        .unwrap();
    assert!(!store.release_turn(&first.turn_id, "superseded", 6).unwrap());
    assert!(store
        .release_turn(&second.turn_id, "agent disabled", 7)
        .unwrap());

    let final_trigger = group_event("m4", "c1", "u1", "/custom", Vec::new(), 8);
    store.record_event(&final_trigger, "event").unwrap();
    let final_turn = store
        .prepare_turn(&final_trigger, GroupTriggerKind::Slash, "/custom")
        .unwrap();
    store.mark_turn_completed(&final_turn.turn_id, 9).unwrap();

    let reset = group_event("m5", "c1", "u1", "/clear", Vec::new(), 10);
    store.record_event(&reset, "event").unwrap();
    store.advance_context_baseline(&reset).unwrap();
    let after_reset = group_event("m6", "c1", "u1", "@_user_1 after", vec![bot_mention()], 11);
    store.record_event(&after_reset, "event").unwrap();
    let after = store
        .prepare_turn(&after_reset, GroupTriggerKind::Mention, "after")
        .unwrap();
    assert_eq!(after.message_count, 1);
    assert!(!after.prompt.contains("/clear"));
}

#[test]
fn group_store_rejects_stale_and_oversized_ranges_without_advancing_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let old = group_event("old", "stale", "u1", "old", Vec::new(), 1);
    let trigger = group_event("trigger", "stale", "u1", "run", Vec::new(), 2);
    store.record_event(&old, "event").unwrap();
    store.record_event(&trigger, "event").unwrap();
    store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "run")
        .unwrap();
    let stale_error = store
        .prepare_turn(&old, GroupTriggerKind::Mention, "old")
        .unwrap_err();
    assert!(stale_error.contains("is not after cursor"));

    for index in 0..=MAX_INLINE_GROUP_MESSAGES {
        let event = group_event(
            &format!("many-{index}"),
            "many",
            "u1",
            "context",
            Vec::new(),
            index as u64 + 10,
        );
        store.record_event(&event, "event").unwrap();
    }
    let too_many = group_event("many-trigger", "many", "u1", "run", Vec::new(), 1_000);
    store.record_event(&too_many, "event").unwrap();
    let count_error = store
        .prepare_turn(&too_many, GroupTriggerKind::Mention, "run")
        .unwrap_err();
    assert!(count_error.contains("超过当前单次上限"));

    let huge = "x".repeat(MAX_INLINE_GROUP_CONTEXT_BYTES + 1);
    let huge_context = group_event("huge", "bytes", "u1", &huge, Vec::new(), 2_000);
    let huge_trigger = group_event("huge-trigger", "bytes", "u1", "run", Vec::new(), 2_001);
    store.record_event(&huge_context, "event").unwrap();
    store.record_event(&huge_trigger, "event").unwrap();
    let bytes_error = store
        .prepare_turn(&huge_trigger, GroupTriggerKind::Mention, "run")
        .unwrap_err();
    assert!(bytes_error.contains("字节"));
}

#[test]
fn prepare_turn_propagates_group_range_query_errors() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let trigger = group_event("broken-range", "c1", "u1", "run", Vec::new(), 1);
    store.record_event(&trigger, "event").unwrap();
    {
        let connection = store.connection.lock();
        connection
            .execute_batch(
                "DROP TABLE im_group_messages;
                 CREATE TABLE im_group_messages (
                    seq INTEGER PRIMARY KEY,
                    provider_id TEXT NOT NULL,
                    chat_id TEXT NOT NULL,
                    message_id TEXT NOT NULL
                 );
                 INSERT INTO im_group_messages (seq, provider_id, chat_id, message_id)
                 VALUES (1, 'feishu-main', 'c1', 'broken-range');",
            )
            .unwrap();
    }

    let error = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "run")
        .unwrap_err();
    assert!(
        error.contains("prepare group context range query"),
        "{error}"
    );
}

#[test]
fn prepare_turn_rejects_a_trigger_missing_from_the_selected_chat_range() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let trigger = group_event("moved-trigger", "c1", "u1", "run", Vec::new(), 1);
    store.record_event(&trigger, "event").unwrap();
    store
        .connection
        .lock()
        .execute(
            "UPDATE im_group_messages SET chat_id = 'other-chat' WHERE message_id = 'moved-trigger'",
            [],
        )
        .unwrap();

    let error = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "run")
        .unwrap_err();
    assert_eq!(error, "trigger message missing from selected group context");
}

#[test]
fn schema_init_reports_non_duplicate_runner_migration_errors() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE VIEW im_group_bindings AS SELECT 1 AS runner_id;")
        .unwrap();

    let error = init_schema(&connection).unwrap_err();
    assert!(error.contains("migrate group runner binding"), "{error}");
}

#[test]
fn group_prompt_renders_mentions_attachments_and_empty_content_safely() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let mut attachment = group_event("a1", "render", "u1", "", Vec::new(), 1);
    attachment
        .message
        .as_mut()
        .unwrap()
        .images
        .push(crate::im_gateway::types::ImImageAttachment {
            file_key: "img-1".to_string(),
            source: crate::im_gateway::types::ImImageSource::MessageResource,
            mime_type: None,
            data_base64: None,
            download_url: None,
            encrypted_query_param: None,
            aes_key: None,
        });
    let empty = group_event("a2", "render", "u2", "", Vec::new(), 2);
    let mentions = vec![
        ImMention {
            key: "".to_string(),
            open_id: Some("ignored".to_string()),
            name: None,
            tenant_key: None,
            is_bot: false,
        },
        ImMention {
            key: "@missing".to_string(),
            open_id: None,
            name: Some("Missing".to_string()),
            tenant_key: None,
            is_bot: false,
        },
        ImMention {
            key: "@alice".to_string(),
            open_id: Some("ou_alice".to_string()),
            name: Some("Alice".to_string()),
            tenant_key: None,
            is_bot: false,
        },
    ];
    let mention_context = group_event("a3", "render", "u3", "@missing and @alice", mentions, 3);
    let trigger = group_event("a4", "render", "u4", "run", Vec::new(), 4);
    for event in [&attachment, &empty, &mention_context, &trigger] {
        store.record_event(event, "event").unwrap();
    }
    let turn = store
        .prepare_turn(&trigger, GroupTriggerKind::Mention, "run")
        .unwrap();
    assert!(turn.prompt.contains("[附件 1 个]"));
    assert!(!turn.prompt.contains("<at id=u2></at>："));
    assert!(turn
        .prompt
        .contains("@missing and <at id=ou_alice>Alice</at>"));

    let bot_by_name = FeishuBotIdentity {
        open_id: "ou_bot".to_string(),
        name: Some("Bifrost".to_string()),
    };
    let name_only_mention = ImMention {
        key: "@name".to_string(),
        open_id: None,
        name: Some("Bifrost".to_string()),
        tenant_key: None,
        is_bot: false,
    };
    let name_only = group_event(
        "name-only",
        "render",
        "u1",
        "@name",
        vec![name_only_mention],
        5,
    );
    assert_eq!(
        classify_group_message(
            name_only.message.as_ref().unwrap(),
            Some(&bot_by_name),
            false
        ),
        GroupMessageDisposition::SystemCommand {
            command: "/help".to_string(),
            reset_context: false,
        }
    );
    assert!(matches!(
        classify_group_message(
            group_event("guide", "render", "u1", "/g go", Vec::new(), 6)
                .message
                .as_ref()
                .unwrap(),
            None,
            false
        ),
        GroupMessageDisposition::AgentTrigger {
            kind: GroupTriggerKind::Guide,
            ..
        }
    ));
    assert!(matches!(
        classify_group_message(
            group_event("queue", "render", "u1", "/q later", Vec::new(), 7)
                .message
                .as_ref()
                .unwrap(),
            None,
            false
        ),
        GroupMessageDisposition::AgentTrigger {
            kind: GroupTriggerKind::Queue,
            ..
        }
    ));
}

#[test]
fn group_store_rejects_non_group_and_missing_message_events() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let mut direct = group_event("direct", "c1", "u1", "hi", Vec::new(), 1);
    direct.source.chat_type = Some("p2p".to_string());
    assert!(store.record_event(&direct, "event").is_err());

    let mut missing = group_event("missing", "c1", "u1", "hi", Vec::new(), 2);
    missing.message = None;
    assert!(store.record_event(&missing, "event").is_err());

    let mut event_id_fallback = group_event("fallback", "c1", "u1", "hi", Vec::new(), 3);
    event_id_fallback.source.message_id = None;
    assert_eq!(store.record_event(&event_id_fallback, "event").unwrap(), 1);
}
