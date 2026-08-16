use super::*;

const RESUME_TEXT_INSTRUCTION: &str =
    "发送 `/resume <id>` 选择；也可以使用至少 8 位的唯一 id 前缀。";

pub(super) enum FeishuChoiceDelivery {
    Sent,
    Fallback(String),
}

pub(super) fn local_session_choice_markdown(text_reply: &str) -> String {
    text_reply
        .strip_suffix(RESUME_TEXT_INSTRUCTION)
        .map(|summary| format!("{summary}点击下方按钮选择要恢复的 session。"))
        .unwrap_or_else(|| text_reply.to_string())
}

pub(super) fn local_session_choice_options(
    sessions: &[crate::im_gateway::external_cli::LocalExternalSession],
) -> Vec<crate::im_gateway::feishu::card_action::FeishuChoiceCardOption> {
    sessions
        .iter()
        .enumerate()
        .filter_map(|(index, session)| {
            let title = truncate_str(session.title.trim(), 36);
            let option = crate::im_gateway::feishu::card_action::FeishuChoiceCardOption {
                label: format!("{}. {} · {}", index + 1, title, session.datetime),
                command: format!("/resume {}", session.id),
            };
            crate::im_gateway::feishu::card_action::is_allowed_choice_command(&option.command)
                .then_some(option)
        })
        .collect()
}

pub(super) fn model_choice_options(
    models: &[crate::im_gateway::external_cli::ExternalCliModelInfo],
) -> Vec<crate::im_gateway::feishu::card_action::FeishuChoiceCardOption> {
    let mut options = vec![
        crate::im_gateway::feishu::card_action::FeishuChoiceCardOption {
            label: "恢复 Runner 默认模型".to_string(),
            command: "/model clear".to_string(),
        },
    ];
    options.extend(models.iter().filter_map(|model| {
        let option = crate::im_gateway::feishu::card_action::FeishuChoiceCardOption {
            label: model_choice_label(model),
            command: format!("/model {}", model.slug),
        };
        crate::im_gateway::feishu::card_action::is_allowed_choice_command(&option.command)
            .then_some(option)
    }));
    options.truncate(41);
    options
}

pub(super) fn effort_choice_options(
    levels: &[crate::im_gateway::external_cli::ExternalCliReasoningLevelInfo],
) -> Vec<crate::im_gateway::feishu::card_action::FeishuChoiceCardOption> {
    let mut options = vec![
        crate::im_gateway::feishu::card_action::FeishuChoiceCardOption {
            label: "恢复 Runner 默认强度".to_string(),
            command: "/effort clear".to_string(),
        },
    ];
    options.extend(levels.iter().filter_map(|level| {
        let option = crate::im_gateway::feishu::card_action::FeishuChoiceCardOption {
            label: level.effort.clone(),
            command: format!("/effort {}", level.effort),
        };
        crate::im_gateway::feishu::card_action::is_allowed_choice_command(&option.command)
            .then_some(option)
    }));
    options
}

fn model_choice_label(model: &crate::im_gateway::external_cli::ExternalCliModelInfo) -> String {
    let display_name = model
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match display_name {
        Some(label) if label != model.slug => {
            format!("{} · {}", truncate_str(label, 32), model.slug)
        }
        _ => model.slug.clone(),
    }
}

pub(super) async fn send_feishu_resume_choice(adapter: &str, ctx: &ImModelCommandContext<'_>) {
    let discovery_adapter = adapter.to_string();
    let discovered = tokio::task::spawn_blocking(move || {
        crate::im_gateway::external_cli::discover_local_sessions(&discovery_adapter, Some(20))
    })
    .await;
    let reply = match discovered {
        Ok(Ok(sessions)) => {
            let reply =
                crate::im_gateway::external_cli::format_local_session_list(adapter, &sessions);
            let card_markdown = local_session_choice_markdown(&reply);
            if send_feishu_choice_card(
                ctx.client,
                ctx.provider,
                ctx.event,
                &card_markdown,
                local_session_choice_options(&sessions),
                ctx.message_log_store,
            )
            .await
            {
                return;
            }
            reply
        }
        Ok(Err(error)) => format!("❌ {error}"),
        Err(error) => format!("❌ 读取本地 session 的后台任务失败：{error}"),
    };
    send_agent_reply(
        ctx.client,
        ctx.provider,
        ctx.event,
        &reply,
        ctx.message_log_store,
    )
    .await;
}

pub(super) async fn send_feishu_model_choice(
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
    status: &str,
    adapter_label: &str,
    ctx: &ImModelCommandContext<'_>,
) -> FeishuChoiceDelivery {
    let models = match crate::im_gateway::external_cli::load_external_cli_model_catalog(
        &effective.settings.adapter,
        &effective.settings.adapter_config,
        None,
    )
    .await
    {
        Ok(models) => models,
        Err(error) => {
            return FeishuChoiceDelivery::Fallback(format!(
                "{status}\n\n无法获取 {adapter_label} 模型列表：{error}"
            ));
        }
    };
    let card_markdown = format!("{status}\n\n请选择模型：");
    if send_feishu_choice_card(
        ctx.client,
        ctx.provider,
        ctx.event,
        &card_markdown,
        model_choice_options(&models),
        ctx.message_log_store,
    )
    .await
    {
        return FeishuChoiceDelivery::Sent;
    }
    FeishuChoiceDelivery::Fallback(format!(
        "{status}\n\n{}",
        crate::im_gateway::external_cli::format_external_cli_model_catalog(
            &effective.settings.adapter,
            &models,
        )
    ))
}

pub(super) async fn send_feishu_effort_choice(
    effective: &crate::im_gateway::external_cli::ExternalCliEffectiveConfig,
    resolved_model: &crate::im_gateway::external_cli::ExternalCliResolvedModelConfig,
    model_catalog: &[crate::im_gateway::external_cli::ExternalCliModelInfo],
    status: &str,
    ctx: &ImModelCommandContext<'_>,
) -> FeishuChoiceDelivery {
    let levels = crate::im_gateway::external_cli::external_cli_effort_options_for_model(
        &effective.settings.adapter,
        resolved_model.model.as_deref(),
        model_catalog,
    );
    let card_markdown = format!("{status}\n\n请选择 Reasoning Effort：");
    if send_feishu_choice_card(
        ctx.client,
        ctx.provider,
        ctx.event,
        &card_markdown,
        effort_choice_options(&levels),
        ctx.message_log_store,
    )
    .await
    {
        return FeishuChoiceDelivery::Sent;
    }
    FeishuChoiceDelivery::Fallback(format!(
        "{status}\n\n{}",
        crate::im_gateway::external_cli::format_external_cli_effort_catalog_for_model(
            &effective.settings.adapter,
            resolved_model.model.as_deref(),
            model_catalog,
        )
    ))
}

pub(super) async fn send_feishu_choice_card(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    markdown: &str,
    options: Vec<crate::im_gateway::feishu::card_action::FeishuChoiceCardOption>,
    message_log_store: &Arc<ImMessageLogStore>,
) -> bool {
    if provider.provider_type != ImProviderType::Feishu || client.feishu().is_none() {
        return false;
    }
    let Some(chat_id) = event
        .source
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(user_id) = event
        .source
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    if options.is_empty() {
        return false;
    }
    let chat_type = if event.source.chat_type.as_deref() == Some("group") {
        "group"
    } else {
        "p2p"
    };
    let binding = crate::im_gateway::feishu::card_action::FeishuChoiceCardBinding {
        provider_id: provider.id.clone(),
        chat_id: chat_id.to_string(),
        chat_type: chat_type.to_string(),
        user_id: user_id.to_string(),
    };
    let card = crate::im_gateway::feishu::card_action::build_feishu_choice_card(
        markdown,
        &binding,
        &options,
        now_ms(),
    );
    let Some(target) = build_agent_reply_target(
        provider,
        event,
        "__agent_choice__",
        "Agent Choice",
        "interactive",
    ) else {
        return false;
    };
    let result = client
        .send_reply_card(
            provider,
            &target,
            event.source.message_id.as_deref(),
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;
    let (status, message_id, error) = match &result {
        Ok(result) => (MessageStatus::Success, result.message_id.clone(), None),
        Err(error) => (MessageStatus::Failed, None, Some(error.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(target.receive_id.clone()),
        target_name: Some(target.display_name.clone()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(markdown, 200)),
        content: Some(markdown.to_string()),
        trigger: Some("agent_choice".to_string()),
        error,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(error) = message_log_store.add(log) {
        error!(error = %error, "failed to store Feishu choice card outbound message log");
    }

    match result {
        Ok(_) => true,
        Err(error) => {
            warn!(
                provider_id = %provider.id,
                event_id = %event.event_id,
                error = %error,
                "failed to send Feishu choice card; falling back to Markdown reply"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvVarGuard {
        key: &'static str,
        old_value: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let old_value = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old_value }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.old_value.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn choice_event(provider: &ImProviderConfig) -> ImEvent {
        ImEvent {
            event_id: "evt-choice-card".to_string(),
            provider_id: provider.id.clone(),
            provider_type: provider.provider_type,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("oc_choice".to_string()),
                chat_type: Some("group".to_string()),
                user_id: Some("ou_owner".to_string()),
                message_id: Some("om_source".to_string()),
                ..Default::default()
            },
            message: None,
            received_at: now_ms(),
            raw_digest: None,
        }
    }

    fn choice_command_event(provider: &ImProviderConfig, command: &str) -> ImEvent {
        let mut event = choice_event(provider);
        event.event_id = format!("evt-choice-{}", uuid_short());
        event.message = Some(crate::im_gateway::types::ImEventMessage {
            text: command.to_string(),
            ..Default::default()
        });
        event
    }

    #[test]
    fn choice_option_builders_preserve_commands_and_limits() {
        let card_markdown = local_session_choice_markdown(
            "最近 1 个 Codex 本地 session：\n\n1. id / title / datetime\n\n发送 `/resume <id>` 选择；也可以使用至少 8 位的唯一 id 前缀。",
        );
        assert!(card_markdown.contains("点击下方按钮"));
        assert!(!card_markdown.contains("发送 `/resume <id>`"));

        let sessions = vec![crate::im_gateway::external_cli::LocalExternalSession {
            id: "01234567-89ab".to_string(),
            title: "A title that can be displayed on the button".to_string(),
            datetime: "2026-08-17T10:00:00Z".to_string(),
            updated_at_millis: 1,
        }];
        let resume = local_session_choice_options(&sessions);
        assert_eq!(resume.len(), 1);
        assert_eq!(resume[0].command, "/resume 01234567-89ab");
        assert!(resume[0].label.contains("2026-08-17T10:00:00Z"));

        let models = (0..45)
            .map(
                |index| crate::im_gateway::external_cli::ExternalCliModelInfo {
                    slug: format!("model-{index}"),
                    display_name: Some(format!("Display {index}")),
                    ..Default::default()
                },
            )
            .collect::<Vec<_>>();
        let mut models = models;
        models.insert(
            0,
            crate::im_gateway::external_cli::ExternalCliModelInfo {
                slug: "invalid model".to_string(),
                display_name: Some("Must be filtered".to_string()),
                ..Default::default()
            },
        );
        let model = model_choice_options(&models);
        assert_eq!(model.len(), 41);
        assert_eq!(model[0].command, "/model clear");
        assert_eq!(model[1].command, "/model model-0");
        assert_eq!(model[40].command, "/model model-39");

        let effort = effort_choice_options(&[
            crate::im_gateway::external_cli::ExternalCliReasoningLevelInfo {
                effort: "low".to_string(),
                description: None,
            },
            crate::im_gateway::external_cli::ExternalCliReasoningLevelInfo {
                effort: "high".to_string(),
                description: None,
            },
            crate::im_gateway::external_cli::ExternalCliReasoningLevelInfo {
                effort: "invalid effort".to_string(),
                description: None,
            },
        ]);
        assert_eq!(effort.len(), 3);
        assert_eq!(effort[0].command, "/effort clear");
        assert_eq!(effort[1].command, "/effort low");
        assert_eq!(effort[2].command, "/effort high");
    }

    #[tokio::test]
    async fn send_choice_card_covers_success_failure_and_input_guards() {
        let _lock = crate::im_gateway::external_cli::local_session_test_env_lock()
            .lock()
            .await;
        let temp = tempfile::tempdir().unwrap();
        let dry_run_file = temp.path().join("feishu/card.jsonl");
        let _dry_run_guard = EnvVarGuard::set("BIFROST_FEISHU_DRY_RUN_FILE", &dry_run_file);
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        provider.id = "feishu-choice-unit".to_string();
        let client =
            ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
        let event = choice_event(&provider);
        let message_log_store = Arc::new(ImMessageLogStore::new(temp.path()));
        let options = vec![
            crate::im_gateway::feishu::card_action::FeishuChoiceCardOption {
                label: "Model".to_string(),
                command: "/model sonnet".to_string(),
            },
        ];

        assert!(
            send_feishu_choice_card(
                &client,
                &provider,
                &event,
                "请选择模型",
                options.clone(),
                &message_log_store,
            )
            .await
        );
        let row: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(&dry_run_file)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(row["kind"], "card");
        assert_eq!(row["receiveId"], "oc_choice");
        assert_eq!(row["sourceMessageId"], "om_source");
        assert_eq!(
            row["card"]["body"]["elements"][1]["columns"][0]["elements"][0]["behaviors"][0]
                ["value"]["chatType"],
            "group"
        );
        let logs = message_log_store.list();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status, MessageStatus::Success);
        assert_eq!(logs[0].msg_type.as_deref(), Some("interactive"));

        let mut invalid_event = event.clone();
        invalid_event.source.chat_id = None;
        assert!(
            !send_feishu_choice_card(
                &client,
                &provider,
                &invalid_event,
                "missing chat",
                options.clone(),
                &message_log_store,
            )
            .await
        );
        invalid_event.source.chat_id = Some("oc_choice".to_string());
        invalid_event.source.user_id = None;
        assert!(
            !send_feishu_choice_card(
                &client,
                &provider,
                &invalid_event,
                "missing user",
                options.clone(),
                &message_log_store,
            )
            .await
        );
        assert!(
            !send_feishu_choice_card(
                &client,
                &provider,
                &event,
                "empty",
                Vec::new(),
                &message_log_store,
            )
            .await
        );
        let weixin = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
        assert!(
            !send_feishu_choice_card(
                &weixin,
                &provider,
                &event,
                "wrong client",
                options.clone(),
                &message_log_store,
            )
            .await
        );

        drop(_dry_run_guard);
        let _failing_guard = EnvVarGuard::set("BIFROST_FEISHU_DRY_RUN_FILE", temp.path());
        assert!(
            !send_feishu_choice_card(
                &client,
                &provider,
                &event,
                "fallback",
                options,
                &message_log_store,
            )
            .await
        );
        let logs = message_log_store.list();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].status, MessageStatus::Failed);
        assert!(logs[0]
            .error
            .as_deref()
            .is_some_and(|error| { error.contains("open Feishu dry-run file") }));
    }

    #[tokio::test]
    async fn feishu_resume_model_and_effort_commands_send_choice_cards() {
        let _local_session_lock = crate::im_gateway::external_cli::local_session_test_env_lock()
            .lock()
            .await;
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let codex_home = tempfile::tempdir().expect("codex home");
        let dry_run_file = temp_dir.path().join("feishu-choice.jsonl");
        let _data_guard =
            crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
        let _home_guard = EnvVarGuard::set("CODEX_HOME", codex_home.path());
        let _dry_run_guard = EnvVarGuard::set("BIFROST_FEISHU_DRY_RUN_FILE", &dry_run_file);
        let id = "eeeeeeee-0000-0000-0000-000000000007";
        let session_path = codex_home.path().join("sessions/choice.jsonl");
        std::fs::create_dir_all(session_path.parent().expect("parent")).expect("create sessions");
        std::fs::write(
            &session_path,
            format!(
                "{{\"timestamp\":\"2099-08-17T05:06:07Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"/tmp\",\"originator\":\"unit\"}}}}\n\
                 {{\"timestamp\":\"2099-08-17T05:06:08Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"Choice session\"}}}}\n"
            ),
        )
        .expect("write session");

        let service = ImGatewayService::new(temp_dir.path());
        let mut provider = crate::handlers::im_gateway::tests::test_provider();
        provider.id = "feishu-choice-unit".to_string();
        let client =
            ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
        let codex_config = service.agent_config_store.load();
        let session_key = build_session_key(&provider.id, Some("ou_owner"));

        let resume = choice_command_event(&provider, "/resume");
        assert!(
            handle_idle_im_command(
                "/resume",
                &session_key,
                &codex_config,
                IdleImCommandContext {
                    client: &client,
                    provider: &provider,
                    provider_store: &service.provider_store,
                    group_context_store: &service.group_context_store,
                    external_cli_config_store: &service.external_cli_config_store,
                    event: &resume,
                    message_log_store: &service.message_log_store,
                    agent_session_manager: &service.agent_session_manager,
                    queue_manager: &service.queue_manager,
                },
            )
            .await
        );

        let claude_config = crate::im_gateway::agent::ImAgentConfig {
            runner: Some(bifrost_agent::AgentRunnerMode::Custom(
                crate::im_gateway::external_cli::DEFAULT_CLAUDE_CODE_RUNNER_ID.to_string(),
            )),
            ..codex_config.clone()
        };
        for command in ["/model", "/effort"] {
            let event = choice_command_event(&provider, command);
            assert!(
                handle_idle_im_command(
                    command,
                    &session_key,
                    &claude_config,
                    IdleImCommandContext {
                        client: &client,
                        provider: &provider,
                        provider_store: &service.provider_store,
                        group_context_store: &service.group_context_store,
                        external_cli_config_store: &service.external_cli_config_store,
                        event: &event,
                        message_log_store: &service.message_log_store,
                        agent_session_manager: &service.agent_session_manager,
                        queue_manager: &service.queue_manager,
                    },
                )
                .await
            );
        }

        let rows = std::fs::read_to_string(&dry_run_file)
            .expect("choice card dry-run")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        let serialized = rows
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(serialized.contains(&format!("/resume {id}")));
        assert!(serialized.contains("/model clear"));
        assert!(serialized.contains("/model sonnet"));
        assert!(serialized.contains("/effort clear"));
        assert!(serialized.contains("/effort high"));
        assert_eq!(
            service
                .message_log_store
                .list()
                .iter()
                .filter(|log| log.trigger.as_deref() == Some("agent_choice"))
                .count(),
            3
        );

        drop(_dry_run_guard);
        let _failing_guard = EnvVarGuard::set("BIFROST_FEISHU_DRY_RUN_FILE", temp_dir.path());
        for (command, expected_catalog) in [
            ("/model", "Claude Code 可用模型:"),
            ("/effort", "Claude Code 可用 Reasoning Effort"),
        ] {
            let event = choice_command_event(&provider, command);
            assert!(
                handle_idle_im_command(
                    command,
                    &session_key,
                    &claude_config,
                    IdleImCommandContext {
                        client: &client,
                        provider: &provider,
                        provider_store: &service.provider_store,
                        group_context_store: &service.group_context_store,
                        external_cli_config_store: &service.external_cli_config_store,
                        event: &event,
                        message_log_store: &service.message_log_store,
                        agent_session_manager: &service.agent_session_manager,
                        queue_manager: &service.queue_manager,
                    },
                )
                .await
            );
            assert!(service.message_log_store.list().iter().any(|log| {
                log.event_id.as_deref() == Some(event.event_id.as_str())
                    && log.trigger.as_deref() == Some("agent")
                    && log
                        .content
                        .as_deref()
                        .is_some_and(|content| content.contains(expected_catalog))
            }));
        }

        let mut weixin_provider = provider.clone();
        weixin_provider.provider_type = ImProviderType::Weixin;
        let weixin = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
        for command in ["/model", "/effort"] {
            let event = choice_command_event(&weixin_provider, command);
            assert!(
                handle_idle_im_command(
                    command,
                    &session_key,
                    &claude_config,
                    IdleImCommandContext {
                        client: &weixin,
                        provider: &weixin_provider,
                        provider_store: &service.provider_store,
                        group_context_store: &service.group_context_store,
                        external_cli_config_store: &service.external_cli_config_store,
                        event: &event,
                        message_log_store: &service.message_log_store,
                        agent_session_manager: &service.agent_session_manager,
                        queue_manager: &service.queue_manager,
                    },
                )
                .await
            );
        }
        let logs = service.message_log_store.list();
        assert!(logs.iter().any(|log| {
            log.content
                .as_deref()
                .is_some_and(|content| content.contains("当前 Claude Code Runner"))
        }));
    }
}
