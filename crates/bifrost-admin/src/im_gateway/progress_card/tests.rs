use super::*;
use bifrost_agent::PlanStepStatus;

#[test]
fn assistant_stream_fragments_are_coalesced_and_terminal_duplicate_is_removed() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "inspect branch");
    for fragment in [
        "我\n", "先\n", "按\n", "仓\n", "库\n", "规\n", "范\n", "检\n", "查",
    ] {
        snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
            content: fragment.to_string(),
        });
    }
    snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
        content: "我先按仓库规范检查".to_string(),
    });

    let running_card = build_feishu_progress_card(&snapshot, true);
    let running_serialized = serde_json::to_string(&running_card).unwrap();
    assert!(running_serialized.contains("我先按仓库规范检查"));
    assert!(!running_serialized.contains("我\\n先\\n按"));
    assert!(running_serialized.contains(PROCESS_PANEL_ELEMENT_ID));
    assert_eq!(snapshot.last_thought.as_deref(), Some("我先按仓库规范检查"));
    assert_eq!(snapshot.timeline.len(), 1);
    assert_eq!(snapshot.timeline[0].kind, ProgressTimelineKind::Thinking);
    assert!(snapshot.output.is_empty());

    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "我先按仓库规范检查".to_string(),
    });

    let finished_card = build_feishu_progress_card(&snapshot, false);
    let finished_body = serde_json::to_string(&finished_card["body"]).unwrap();
    assert_eq!(finished_body.matches("我先按仓库规范检查").count(), 1);
    assert!(!finished_body.contains(PROCESS_PANEL_ELEMENT_ID));
    assert!(snapshot.timeline.is_empty());
}

#[test]
fn assistant_stream_keeps_repeated_tokens_and_word_boundaries() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "inspect branch");
    for fragment in ["哈", "哈", " ", "done"] {
        snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
            content: fragment.to_string(),
        });
    }

    assert_eq!(snapshot.last_thought.as_deref(), Some("哈哈 done"));
    assert!(!assistant_texts_equivalent("foo bar", "foobar"));
    assert!(assistant_texts_equivalent("我\n先\n检查", "我先检查"));
}

#[test]
fn progress_snapshot_tracks_tool_plan_queue_and_final_output() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "initial task");
    assert_eq!(snapshot.title.as_deref(), Some("initial task"));
    snapshot.apply_event(AgentTurnProgressEvent::TitleUpdated {
        title: "Updated title".to_string(),
    });
    assert_eq!(snapshot.title.as_deref(), Some("Updated title"));
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "I will inspect the workspace.".to_string(),
    });
    assert_eq!(
        snapshot.last_thought.as_deref(),
        Some("I will inspect the workspace.")
    );
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "shell".to_string(),
        arguments: "{\"cmd\":\"ls\"}".to_string(),
    });
    assert_eq!(
        snapshot
            .latest_tool
            .as_ref()
            .map(|tool| tool.tool_name.as_str()),
        Some("shell")
    );

    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "shell".to_string(),
            arguments: "{\"cmd\":\"ls\"}".to_string(),
            result: "Cargo.toml".to_string(),
            success: true,
        },
        duration_ms: 42,
    });
    snapshot.apply_event(AgentTurnProgressEvent::PlanUpdated {
        title: Some("Build".to_string()),
        steps: vec![PlanStep {
            step: "Run tests".to_string(),
            status: PlanStepStatus::InProgress,
        }],
    });
    snapshot.update_queue_state(
        vec![QueueItem {
            seq: 1,
            message: "next".to_string(),
            images: Vec::new(),
        }],
        true,
        Some("已收到引导：prioritize logs".to_string()),
    );
    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "done".to_string(),
    });

    assert_eq!(snapshot.phase, ImProgressPhase::Finished);
    assert_eq!(snapshot.output, "done");
    assert_eq!(snapshot.plan_steps.len(), 1);
    assert_eq!(snapshot.tool_calls.len(), 1);
    assert_eq!(snapshot.timeline.len(), 2);
    assert_eq!(snapshot.timeline[0].kind, ProgressTimelineKind::Thinking);
    assert_eq!(snapshot.timeline[1].kind, ProgressTimelineKind::Tool);
    assert!(snapshot.timeline[1].completed);
    assert_eq!(snapshot.queue_items.len(), 1);
    assert!(snapshot.guide_pending);
    assert_eq!(
        snapshot.activity_notice.as_deref(),
        Some("已收到引导：prioritize logs")
    );
}

#[test]
fn external_runner_todo_list_plan_renders_in_feishu_progress_card() {
    let event = crate::im_gateway::external_cli::parse_progress_events(
        r#"{"type":"item.updated","item":{"id":"item_0","type":"todo_list","items":[{"text":"inspect output","completed":true},{"text":"map parser","completed":false}]}}"#,
    )
    .pop()
    .expect("todo list event");
    let agent_event = crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
        "s1",
        "codex",
        crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
            Some("Codex"),
            None,
            None,
            None,
            None,
            None,
        ),
        &event,
    )
    .expect("agent plan event");

    let mut snapshot = ImAgentProgressSnapshot::new("s1", "runner task");
    snapshot.apply_event(agent_event);
    assert_eq!(snapshot.title.as_deref(), Some("runner task"));
    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();

    assert!(serialized.contains("任务计划"));
    assert!(serialized.contains("inspect output"));
    assert!(serialized.contains("map parser"));
    assert!(serialized.contains("✅"));
    assert!(serialized.contains("⏳"));
}

#[test]
fn assistant_final_is_pipeline_content_until_turn_finished() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "review task");
    snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
        content: "我先看分支差异。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "git diff --stat".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: "git diff --stat".to_string(),
            result: "56 files changed".to_string(),
            success: true,
        },
        duration_ms: 10,
    });
    snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
        content: "接下来逐个模块检查。".to_string(),
    });

    assert!(snapshot.output.is_empty());
    assert_eq!(snapshot.timeline.len(), 3);
    assert_eq!(snapshot.timeline[0].kind, ProgressTimelineKind::Thinking);
    assert_eq!(snapshot.timeline[1].kind, ProgressTimelineKind::Tool);
    assert_eq!(snapshot.timeline[2].kind, ProgressTimelineKind::Thinking);

    let running_card = build_feishu_progress_card(&snapshot, true);
    let running_serialized = serde_json::to_string(&running_card).unwrap();
    assert!(running_serialized.contains("我先看分支差异"));
    assert!(running_serialized.contains("接下来逐个模块检查"));
    assert!(!running_serialized.contains("最终结论"));
    assert!(!running_serialized.contains("Loop"));
    assert!(!running_serialized.contains("Pipeline"));

    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "最终结论：未发现阻塞问题。".to_string(),
    });
    assert_eq!(snapshot.output, "最终结论：未发现阻塞问题。");
}

#[test]
fn traex_model_messages_stay_visible_while_machine_statuses_are_hidden() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "检查 Traex 版本");
    for state in [
        "turn started",
        "model rerouted: Test-O-New-Thinking -> claude_46_opus (HighRiskCyberActivity)",
        "tool_calls",
        "waiting_on_session",
        "model_request",
        "model_response",
    ] {
        let mut status = active_status(0);
        status.state = state.to_string();
        snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(status)));
    }
    snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
        content: "检查当前 Traex 版本并与最新可用版本对比。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: "traex --version 2>&1 | head -5".to_string(),
            result: "traecli 0.200.9".to_string(),
            success: true,
        },
        duration_ms: 88,
    });
    let final_output = "**结论：** 当前 Traex 版本为 `0.200.9`，已是最新版本，无需更新。";
    snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
        content: final_output.to_string(),
    });

    let running_card = build_feishu_progress_card(&snapshot, true);
    let running_serialized = serde_json::to_string(&running_card).unwrap();
    assert!(running_serialized.contains("检查当前 Traex 版本并与最新可用版本对比"));
    assert!(running_serialized.contains("当前 Traex 版本为"));
    assert!(running_serialized.contains("已完成：exec_command"));
    assert!(!running_serialized.contains("状态：tool_calls"));
    assert!(!running_serialized.contains("状态：waiting_on_session"));
    assert!(!running_serialized.contains("状态：model_request"));
    assert!(!running_serialized.contains("状态：model_response"));
    assert!(!running_serialized.contains("model rerouted"));

    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: final_output.to_string(),
    });

    assert_eq!(snapshot.output, final_output);
    assert_eq!(
        snapshot.last_thought.as_deref(),
        Some("检查当前 Traex 版本并与最新可用版本对比。")
    );
    assert_eq!(snapshot.timeline.len(), 2);
    assert_eq!(snapshot.timeline[0].kind, ProgressTimelineKind::Thinking);
    assert_eq!(snapshot.timeline[1].kind, ProgressTimelineKind::Tool);

    let finished_card = build_feishu_progress_card(&snapshot, false);
    let finished_serialized = serde_json::to_string(&finished_card).unwrap();
    let process_serialized = serde_json::to_string(
        finished_card["body"]["elements"]
            .as_array()
            .and_then(|elements| {
                elements
                    .iter()
                    .find(|element| element["element_id"] == PROCESS_PANEL_ELEMENT_ID)
            })
            .expect("process element"),
    )
    .unwrap();
    assert!(finished_serialized.contains("检查当前 Traex 版本并与最新可用版本对比"));
    assert!(finished_serialized.contains("已完成：exec_command"));
    assert!(finished_serialized.contains("当前 Traex 版本为"));
    assert!(!process_serialized.contains("当前 Traex 版本为"));
    assert!(!finished_serialized.contains("状态：tool_calls"));
    assert!(!finished_serialized.contains("model rerouted"));
}

#[test]
fn duplicate_running_tool_started_updates_existing_pipeline_item() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "review task");
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "git diff --stat".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "git diff --stat".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: "git diff --stat".to_string(),
            result: "large output".repeat(200),
            success: true,
        },
        duration_ms: 42,
    });

    assert_eq!(snapshot.timeline.len(), 1);
    assert!(snapshot.timeline[0].completed);
    assert!(snapshot.timeline[0].detail.len() < 1200);
}

#[test]
fn noisy_runner_statuses_are_hidden_from_process_timeline() {
    assert!(!is_human_readable_progress_status(
        "019ea1ba-28b8-7670-befe-a979605ce0bf"
    ));
    assert!(!is_human_readable_progress_status("turn started"));
    assert!(!is_human_readable_progress_status("tool_calls"));
    assert!(!is_human_readable_progress_status("waiting_on_session"));
    assert!(!is_human_readable_progress_status("model_request"));
    assert!(!is_human_readable_progress_status("model_response"));
    assert!(!is_human_readable_progress_status("custom_machine_state"));
    assert!(!is_human_readable_progress_status("token_usage_updated"));
    assert!(!is_human_readable_progress_status("token usage updated"));
    assert!(!is_human_readable_progress_status("token-usage-update"));
    assert!(!is_human_readable_progress_status(
        "model rerouted: Test-O-New-Thinking -> claude_46_opus"
    ));
    assert!(is_human_readable_progress_status("正在读取当前分支差异"));
}

#[test]
fn machine_status_events_do_not_flood_process_card() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "run task");
    for state in [
        "tool_calls",
        "waiting_on_session",
        "model_request",
        "model_response",
        "custom_machine_state",
        "token_usage_updated",
        "token usage updated",
    ] {
        let mut status = active_status(0);
        status.state = state.to_string();
        snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(status)));
    }
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "我会先检查失败用例。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "cargo test -p bifrost-admin progress_card".to_string(),
    });

    assert_eq!(snapshot.timeline.len(), 2);
    assert_eq!(snapshot.timeline[0].kind, ProgressTimelineKind::Thinking);
    assert_eq!(snapshot.timeline[1].kind, ProgressTimelineKind::Tool);

    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();
    assert!(serialized.contains("我会先检查失败用例"));
    assert!(serialized.contains("正在运行：exec_command"));
    assert!(!serialized.contains("状态：tool_calls"));
    assert!(!serialized.contains("状态：waiting_on_session"));
    assert!(!serialized.contains("状态：model_request"));
    assert!(!serialized.contains("状态：model_response"));
    assert!(!serialized.contains("custom_machine_state"));
    assert!(!serialized.contains("token_usage_updated"));
}

#[test]
fn progress_prose_linebreak_normalizer_collapses_stream_token_lines() {
    let input = "启动\n检查\n发现\n当前\n工作\n区\n已有\n一处\n用户\n改动\n:\n`e2e-tests/tests/test_rule_share\n_confirm_browser.sh`\n\n- 保留列表\n- 第二项\n\n    indented\n    code\n\n```text\n保留\n代码块\n```";
    let normalized = normalize_progress_prose_linebreaks(input);

    assert!(normalized.contains(
        "启动检查发现当前工作区已有一处用户改动:`e2e-tests/tests/test_rule_share_confirm_browser.sh`"
    ));
    assert!(normalized.contains("\n- 保留列表\n- 第二项\n"));
    assert!(normalized.contains("\n    indented\n    code\n"));
    assert!(normalized.contains("```text\n保留\n代码块\n```"));
    assert!(!normalized.contains("启动\n检查"));
    assert!(!normalized.contains("工作\n区"));
}

#[test]
fn progress_prose_linebreak_normalizer_keeps_ascii_words_readable() {
    let normalized =
        normalize_progress_prose_linebreaks("I will inspect\ncurrent workspace\nbefore editing.");

    assert_eq!(
        normalized,
        "I will inspect current workspace before editing."
    );
}

#[test]
fn feishu_progress_card_collapses_fragmented_thinking_lines() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "run task");
    snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(active_status(0))));
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "启动\n检查\n发现\n当前\n工作\n区\n已有\n一处\n用户\n改动\n:\n`e2e-tests/tests/test_rule_share\n_confirm_browser.sh`".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "git status --short --branch".to_string(),
    });

    let card = build_feishu_progress_card(&snapshot, true);
    let process_element = card["body"]["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_id"] == PROCESS_PANEL_ELEMENT_ID)
        })
        .expect("process element");
    let process_content = process_element["elements"][0]["content"].as_str().unwrap();

    assert!(
        process_content.contains(
            "启动检查发现当前工作区已有一处用户改动:`e2e-tests/tests/test_rule_share_confirm_browser.sh`"
        ),
        "fragmented thinking should be rendered as a readable sentence: {process_content}"
    );
    assert!(!process_content.contains("启动\n检查"));
    assert!(!process_content.contains("工作\n区"));
    assert!(!process_content.contains("test_rule_share\n_confirm"));
}

#[test]
fn feishu_progress_card_file_change_tool_expands_with_detail() {
    let event = crate::im_gateway::external_cli::parse_progress_events(
        r#"{"type":"item.completed","item":{"id":"item_file_1","type":"file_change","status":"completed","files":[{"path":"src/main.rs","action":"modified","summary":"updated startup text","diff":"-old\n+new"}]}}"#,
    )
    .pop()
    .expect("file change event");
    let agent_event = crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
        "s1",
        "codex",
        crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
            Some("Codex"),
            None,
            None,
            None,
            None,
            None,
        ),
        &event,
    )
    .expect("tool event");

    let mut snapshot = ImAgentProgressSnapshot::new("s1", "edit task");
    snapshot.apply_event(agent_event);
    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();

    assert!(serialized.contains("file_change"));
    assert!(serialized.contains("src/main.rs"));
    assert!(serialized.contains("updated startup text"));
    assert!(serialized.contains("-old"));
    assert!(serialized.contains("+new"));
    assert!(!serialized.contains("暂无工具详情"));
}

#[test]
fn consecutive_process_tools_are_grouped_by_default() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "review task");
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "我先读取关键文件。".to_string(),
    });
    for index in 0..3 {
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "exec_command".to_string(),
                arguments: format!(r#"{{"cmd":"git diff -- file{index}.rs"}}"#),
                result: "ok".to_string(),
                success: true,
            },
            duration_ms: 20 + index,
        });
    }

    let card = build_feishu_progress_card(&snapshot, true);
    let process_element = card["body"]["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_id"] == PROCESS_PANEL_ELEMENT_ID)
        })
        .expect("process element");
    let process_elements = process_element["elements"].as_array().unwrap();
    assert_eq!(process_elements.len(), 2);
    assert_eq!(process_elements[1]["element_id"], "ap_tg_1");
    assert_eq!(process_elements[1]["expanded"], false);
    assert_eq!(
        process_elements[1]["header"]["title"]["content"],
        "已运行 3 条命令"
    );
    let grouped_tools = process_elements[1]["elements"].as_array().unwrap();
    assert_eq!(grouped_tools.len(), 3);
    assert_eq!(grouped_tools[0]["element_id"], "ap_t_1");
    assert_eq!(grouped_tools[0]["expanded"], false);
    assert_eq!(grouped_tools[1]["element_id"], "ap_t_2");
    assert_eq!(grouped_tools[2]["element_id"], "ap_t_3");
    let serialized = serde_json::to_string(&card).unwrap();
    assert!(serialized.contains("已完成：exec_command · 20ms"));
    assert!(!serialized.contains("ok ok ok"));
}

#[test]
fn process_timeline_keeps_latest_thirty_tool_calls_with_omission_notice() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "long task");
    for index in 0..35 {
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "exec_command".to_string(),
                arguments: format!(r#"{{"cmd":"echo tool-{index}"}}"#),
                result: format!("result-{index}"),
                success: true,
            },
            duration_ms: 10 + index,
        });
    }

    let card = build_feishu_progress_card(&snapshot, true);
    let process_element = card["body"]["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_id"] == PROCESS_PANEL_ELEMENT_ID)
        })
        .expect("process element");
    assert_eq!(
        process_element["header"]["title"]["content"],
        "执行过程：已运行 35 条命令"
    );
    let process_elements = process_element["elements"].as_array().unwrap();
    assert_eq!(process_elements[0]["element_id"], PROCESS_LOG_ELEMENT_ID);
    assert!(process_elements[0]["content"]
        .as_str()
        .unwrap()
        .contains("已省略前面 5 次工具调用，仅显示最新 30 次。"));
    assert_eq!(process_elements[1]["element_id"], "ap_tg_5");

    let grouped_tools = process_elements[1]["elements"].as_array().unwrap();
    assert_eq!(grouped_tools.len(), 30);
    assert_eq!(grouped_tools[0]["element_id"], "ap_t_5");
    assert_eq!(grouped_tools[29]["element_id"], "ap_t_34");

    let serialized = serde_json::to_string(&card).unwrap();
    assert!(!serialized.contains("tool-0"));
    assert!(!serialized.contains("result-4"));
    assert!(serialized.contains("tool-5"));
    assert!(serialized.contains("result-34"));
}

#[test]
fn feishu_progress_card_expands_process_while_running_and_collapses_after_finish() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "stream task");
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "我会先检查代码路径，然后运行测试。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: r#"{"cmd":"cargo test -p bifrost-admin progress_card"}"#.to_string(),
    });

    let running_card = build_feishu_progress_card(&snapshot, true);
    let running_serialized = serde_json::to_string(&running_card).unwrap();
    assert!(running_serialized.contains(PROCESS_PANEL_ELEMENT_ID));
    assert!(running_serialized.contains(r#""expanded":true"#));
    assert!(running_serialized.contains("我会先检查代码路径"));
    assert!(!running_serialized.contains("1. 我会先检查代码路径"));
    assert!(running_serialized.contains("正在运行：exec_command"));
    assert!(!running_serialized.contains("Loop"));
    assert!(!running_serialized.contains("[模型]"));
    assert!(!running_serialized.contains("工具摘要"));

    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: r#"{"cmd":"cargo test -p bifrost-admin progress_card"}"#.to_string(),
            result: "test result: ok".to_string(),
            success: true,
        },
        duration_ms: 123,
    });
    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "最终结论：测试通过。".to_string(),
    });

    let finished_card = build_feishu_progress_card(&snapshot, false);
    let elements = finished_card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements[0]["element_id"], STATUS_PANEL_ELEMENT_ID);
    assert_eq!(elements[1]["element_id"], PROCESS_PANEL_ELEMENT_ID);
    assert_eq!(elements[1]["expanded"], false);
    assert_eq!(elements.last().unwrap()["element_id"], OUTPUT_ELEMENT_ID);

    let finished_serialized = serde_json::to_string(&finished_card).unwrap();
    assert!(finished_serialized.contains("最终结论：测试通过。"));
    assert!(finished_serialized.contains("执行过程：已运行 1 条命令"));
    assert!(finished_serialized.contains("已完成：exec_command"));
    assert!(finished_serialized.contains("test result: ok"));
    assert!(!finished_serialized.contains("Pipeline"));
    assert!(!finished_serialized.contains("Loop"));
    assert!(!finished_serialized.contains("工具摘要"));
    let process_elements = elements[1]["elements"].as_array().unwrap();
    assert_eq!(process_elements[1]["element_id"], "ap_t_1");
    assert_eq!(process_elements[1]["expanded"], false);
    assert!(process_elements[1]["header"]["title"]["content"]
        .as_str()
        .unwrap()
        .contains("已完成：exec_command"));
}

#[test]
fn feishu_progress_card_process_element_ids_stay_within_feishu_limits() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "many tools");
    for index in 0..18 {
        snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
            content: format!("Loop {} thinking", index + 1),
        });
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "exec_command".to_string(),
                arguments: format!(r#"{{"cmd":"echo {index}"}}"#),
                result: "ok".to_string(),
                success: true,
            },
            duration_ms: 10,
        });
    }

    let card = build_feishu_progress_card(&snapshot, true);
    let mut element_ids = Vec::new();
    collect_element_ids(&card, &mut element_ids);
    assert!(
        element_ids.iter().any(|id| id == "ap_t_35"),
        "test must cover two-digit process tool ids: {element_ids:?}"
    );
    assert!(
        element_ids.iter().any(|id| id == "ap_td_35"),
        "test must cover two-digit process tool detail ids: {element_ids:?}"
    );
    for element_id in element_ids {
        assert_feishu_element_id_is_valid(&element_id);
    }
}

#[test]
fn external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(ProgressRunnerSummary {
        runner_id: "codex".to_string(),
        adapter: "codex".to_string(),
        model: Some("gpt-test".to_string()),
        model_source: Some("runner 配置".to_string()),
        reasoning_effort: Some("high".to_string()),
        reasoning_summary: Some("auto".to_string()),
        reasoning_source: Some("runner 配置".to_string()),
        token_usage: Some(ProgressRunnerTokenUsage {
            input_tokens: Some(1_200),
            cached_input_tokens: Some(300),
            output_tokens: Some(80),
            reasoning_output_tokens: Some(20),
            total_tokens: Some(1_280),
        }),
        work_dir: Some("/tmp/bifrost-codex".to_string()),
        external_thread_id: Some("thread-123".to_string()),
        external_conversation_id: None,
    });
    snapshot.latest_tool = Some(ProgressToolSummary {
        tool_name: "exec_command".to_string(),
        arguments: Some("{\"command\":\"pwd\"}".to_string()),
        success: Some(true),
        result_preview: Some("/tmp/bifrost-codex".to_string()),
        duration_ms: Some(12),
    });
    snapshot.queue_items.push(QueueItem {
        seq: 1,
        message: "queued message".to_string(),
        images: Vec::new(),
    });
    snapshot.guide_pending = true;
    snapshot.phase = ImProgressPhase::Finished;

    let title = format_status_panel_title(&snapshot);
    let footer = format_footer_markdown(&snapshot);

    assert!(title.contains("Runner：codex"));
    assert!(title.contains("模型：gpt-test（runner 配置）"));
    assert!(title.contains("思考：high"));
    assert!(footer.contains("状态：已完成"));
    assert!(footer.contains("Runner：`codex` · Adapter：`codex`"));
    assert!(footer.contains("模型：gpt-test（runner 配置）"));
    assert!(footer.contains("思考：high · 摘要：auto"));
    assert!(footer.contains("Token：总计 1.3K · 输入 1.2K · 输出 80"));
    assert!(footer.contains("Context：最近输入 1.2K / N/A"));
    assert!(footer.contains("缓存输入 300"));
    assert!(footer.contains("推理输出 20"));
    assert!(footer.contains("外部会话：Codex threadId=thread-123"));
    assert!(footer.contains("队列：1 条排队消息 · 引导：有待处理引导消息"));
    assert!(footer.contains("工作路径：`/tmp/bifrost-codex`"));
    assert!(footer.contains("最新工具：`exec_command` · 完成"));
    assert!(!footer.contains("Loop"));
    assert!(!footer.contains("压缩"));
    assert!(!footer.contains("工作路径：`N/A`"));
    assert!(!footer.contains("外部会话：N/A"));
}

#[test]
fn external_runner_footer_hides_machine_status_line() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(ProgressRunnerSummary {
        runner_id: "codex".to_string(),
        adapter: "codex".to_string(),
        model: None,
        model_source: None,
        reasoning_effort: None,
        reasoning_summary: None,
        reasoning_source: None,
        token_usage: None,
        work_dir: Some("/tmp/bifrost-codex".to_string()),
        external_thread_id: None,
        external_conversation_id: None,
    });
    let mut machine_status = active_status(0);
    machine_status.state = "model_request".to_string();
    snapshot.status = Some(machine_status);

    assert_eq!(external_runner_state_line(&snapshot), None);

    let footer = format_footer_markdown(&snapshot);
    assert!(footer.contains("Runner：`codex` · Adapter：`codex`"));
    assert!(!footer.contains("当前状态：model_request"));
}

#[test]
fn progress_footer_formats_elapsed_duration_without_milliseconds() {
    assert_eq!(format_progress_elapsed_duration(0), "0 秒");
    assert_eq!(format_progress_elapsed_duration(12), "12 秒");
    assert_eq!(format_progress_elapsed_duration(65), "1 分 05 秒");
    assert_eq!(format_progress_elapsed_duration(3_600), "1 小时");
    assert_eq!(format_progress_elapsed_duration(3_780), "1 小时 03 分");
    assert_eq!(format_progress_elapsed_duration(90_000), "1 天 1 小时");
}

#[test]
fn token_usage_status_refresh_updates_footer_elapsed_without_process_noise() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(ProgressRunnerSummary {
        runner_id: "codex".to_string(),
        adapter: "codex".to_string(),
        model: None,
        model_source: None,
        reasoning_effort: None,
        reasoning_summary: None,
        reasoning_source: None,
        token_usage: Some(ProgressRunnerTokenUsage {
            input_tokens: Some(1_000),
            cached_input_tokens: None,
            output_tokens: Some(50),
            reasoning_output_tokens: None,
            total_tokens: Some(1_050),
        }),
        work_dir: Some("/tmp/bifrost-codex".to_string()),
        external_thread_id: Some("thread-123".to_string()),
        external_conversation_id: None,
    });
    let mut started = active_status(0);
    started.state = "running".to_string();
    started.started_at = 1_800_000_000;
    started.updated_at = 1_800_000_000;
    started.runner_type = Some("codex".to_string());
    started.runner_id = Some("codex".to_string());
    snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(started)));
    assert!(format_footer_markdown(&snapshot).contains("耗时：0 秒"));

    let mut usage_update = active_status(0);
    usage_update.state = "token usage updated".to_string();
    usage_update.started_at = 1_800_000_065;
    usage_update.updated_at = 1_800_000_065;
    usage_update.runner_type = Some("codex".to_string());
    usage_update.runner_id = Some("codex".to_string());
    snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(usage_update)));

    let footer = format_footer_markdown(&snapshot);
    assert!(footer.contains("耗时：1 分 05 秒"));
    assert!(footer.contains("Token：总计 1.1K · 输入 1K · 输出 50"));
    assert!(!footer.contains("ms"));
    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();
    assert!(!serialized.contains("状态：token usage updated"));
    assert!(!serialized.contains("当前状态：token usage updated"));
}

#[test]
fn codex_usage_progress_event_refreshes_status_without_timeline_noise() {
    let event = crate::im_gateway::external_cli::parse_progress_events(
        r#"{"type":"token_usage.updated","usage":{"input_tokens":1200,"output_tokens":80,"total_tokens":1280}}"#,
    )
    .pop()
    .expect("usage progress event");
    let agent_event = crate::im_gateway::external_cli::external_progress_to_agent_turn_event(
        "s1",
        "codex",
        crate::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
            Some("codex"),
            None,
            None,
            None,
            None,
            None,
        ),
        &event,
    )
    .expect("status event");

    let mut snapshot = ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.apply_event(agent_event);

    assert!(snapshot.timeline.is_empty());
    assert!(snapshot.status.is_some());
    let serialized = serde_json::to_string(&build_feishu_progress_card(&snapshot, true)).unwrap();
    assert!(!serialized.contains("token_usage"));
    assert!(!serialized.contains("token usage updated"));
}

fn active_status(compaction_count: u32) -> ActiveTurnStatus {
    ActiveTurnStatus {
        session_key: "s1".to_string(),
        state: "model_response".to_string(),
        started_at: 1,
        updated_at: 2,
        current_loop_iteration: 2,
        completed_loop_iterations: 1,
        max_loop_iterations: 1000,
        last_response_tokens: Some(100),
        total_tokens_used: Some(1_000),
        estimated_context_tokens: 2_000,
        context_window_tokens: Some(10_000),
        context_usage_percent: Some(20.0),
        compaction_count,
        history_version: 7,
        work_dir: Some("/tmp/bifrost-work".to_string()),
        message_count: 9,
        local_tool_count: 12,
        mcp_tool_count: 5,
        pending_guide_messages: Vec::new(),
        user_turn_count: 2,
        agent_type: Some("External Runner Agent".to_string()),
        runner_type: Some("codex".to_string()),
        runner_id: Some("Codex".to_string()),
        model: Some("gpt-5".to_string()),
        model_provider: Some("openai".to_string()),
        model_reasoning_effort: Some("high".to_string()),
        model_reasoning_summary: Some("auto".to_string()),
        external_conversation_id: None,
        external_thread_id: None,
        turn_timing: None,
        turn_id: None,
    }
}

#[test]
fn runner_type_alone_marks_status_as_external() {
    let mut status = active_status(0);
    status.runner_id = None;
    status.agent_type = None;
    status.runner_type = Some("codex".to_string());
    assert!(is_external_runner_status(&status));
}

fn context_snapshot(compaction_count: u32) -> AgentContextSnapshot {
    AgentContextSnapshot {
        estimated_context_tokens: 1_200,
        context_window_tokens: Some(10_000),
        context_usage_percent: Some(12.0),
        compaction_count,
        history_version: 8,
        message_count: 4,
        user_turn_count: 2,
        last_response_tokens: Some(77),
        total_tokens_used: Some(1_077),
    }
}

#[test]
fn progress_snapshot_uses_context_when_status_is_not_available() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "compact task");
    snapshot.apply_event(AgentTurnProgressEvent::ContextUpdated {
        context: context_snapshot(3),
    });

    let title = format_status_panel_title(&snapshot);
    let footer = format_footer_markdown(&snapshot);
    assert!(title.contains("Token：累计 1.1K · 最近 77"));
    assert!(footer.contains("压缩：3 次"));
    assert!(footer.contains("Context：~1.2K / 10K (12.0%)"));
}

#[test]
fn card_metric_count_uses_readable_kmb_units() {
    assert_eq!(bifrost_agent::format_status_metric_count(0), "0");
    assert_eq!(bifrost_agent::format_status_metric_count(999), "999");
    assert_eq!(bifrost_agent::format_status_metric_count(1_000), "1K");
    assert_eq!(bifrost_agent::format_status_metric_count(9_999), "10K");
    assert_eq!(bifrost_agent::format_status_metric_count(19_333), "19.3K");
    assert_eq!(bifrost_agent::format_status_metric_count(38_634), "38.6K");
    assert_eq!(bifrost_agent::format_status_metric_count(250_000), "250K");
    assert_eq!(bifrost_agent::format_status_metric_count(999_950), "1M");
    assert_eq!(bifrost_agent::format_status_metric_count(1_000_000), "1M");
    assert_eq!(bifrost_agent::format_status_metric_count(1_234_567), "1.2M");
    assert_eq!(
        bifrost_agent::format_status_metric_count(1_280_000_000),
        "1.3B"
    );
}

#[test]
fn feishu_progress_card_uses_json_2_streaming_and_stable_elements() {
    let snapshot = ImAgentProgressSnapshot::new("s1", "initial task");
    let card = build_feishu_progress_card(&snapshot, true);
    assert_eq!(card["schema"], "2.0");
    assert_eq!(card["config"]["streaming_mode"], true);
    let body = card["body"]["elements"].as_array().unwrap();
    let serialized = serde_json::to_string(body).unwrap();
    assert!(serialized.contains("处理中..."));
    assert!(!serialized.contains("最终输出"));
    for id in [
        OUTPUT_ELEMENT_ID,
        STATUS_PANEL_ELEMENT_ID,
        FOOTER_ELEMENT_ID,
    ] {
        assert!(serialized.contains(id), "missing element id {id}");
    }
    for id in [
        PLAN_PANEL_ELEMENT_ID,
        PLAN_ELEMENT_ID,
        TOOL_PANEL_ELEMENT_ID,
        TOOL_LOG_ELEMENT_ID,
        THINKING_PANEL_ELEMENT_ID,
        PROCESS_PANEL_ELEMENT_ID,
        PROCESS_LOG_ELEMENT_ID,
    ] {
        assert!(!serialized.contains(id), "unexpected empty module id {id}");
    }
    assert_eq!(card["header"]["title"]["content"], "initial task");

    let mut populated = snapshot.clone();
    populated.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "Inspecting files before running tests.\nThen checking card layout.".to_string(),
    });
    populated.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "shell".to_string(),
        arguments: "{}".to_string(),
    });
    populated.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "shell".to_string(),
            arguments: "{}".to_string(),
            result: "tests passed".to_string(),
            success: true,
        },
        duration_ms: 42,
    });
    populated.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "Now I will write the final summary.".to_string(),
    });
    populated.apply_event(AgentTurnProgressEvent::PlanUpdated {
        title: Some("Build".to_string()),
        steps: vec![PlanStep {
            step: "Run tests".to_string(),
            status: PlanStepStatus::InProgress,
        }],
    });
    populated.update_queue_state(
        vec![QueueItem {
            seq: 7,
            message: "queued".to_string(),
            images: Vec::new(),
        }],
        true,
        Some("已收到引导：rerun failed path".to_string()),
    );
    let populated_card = build_feishu_progress_card(&populated, true);
    let populated_body = populated_card["body"]["elements"].as_array().unwrap();
    let populated_serialized = serde_json::to_string(populated_body).unwrap();
    for id in [
        PLAN_ELEMENT_ID,
        PROCESS_PANEL_ELEMENT_ID,
        PROCESS_LOG_ELEMENT_ID,
        "ap_t_1",
        "已收到引导：rerun failed path",
    ] {
        assert!(
            populated_serialized.contains(id),
            "missing populated module id {id}"
        );
    }
    for id in [
        TOOL_PANEL_ELEMENT_ID,
        TOOL_LOG_ELEMENT_ID,
        THINKING_PANEL_ELEMENT_ID,
    ] {
        assert!(
            !populated_serialized.contains(id),
            "process timeline should replace legacy module id {id}"
        );
    }
    assert_eq!(populated_card["header"]["title"]["content"], "Build");
    assert_eq!(populated_body[0]["element_id"], STATUS_PANEL_ELEMENT_ID);
    assert_eq!(
        populated_body[1]["header"]["title"]["content"],
        "任务计划：Run tests"
    );
    assert_eq!(
        populated_body.last().unwrap()["element_id"],
        OUTPUT_ELEMENT_ID
    );
    let process_element = populated_body
        .iter()
        .find(|element| element["element_id"] == PROCESS_PANEL_ELEMENT_ID)
        .unwrap();
    assert_eq!(process_element["tag"], "collapsible_panel");
    assert_eq!(process_element["expanded"], true);
    let process_content = process_element["elements"][0]["content"].as_str().unwrap();
    assert!(
        process_content.contains("Inspecting files before running tests."),
        "process content should show the latest thought: {process_content}"
    );
    assert!(!process_content.contains("Loop"));
    assert!(!process_content.contains("[模型]"));
    let process_elements = process_element["elements"].as_array().unwrap();
    assert_eq!(process_elements[1]["element_id"], "ap_t_1");
    assert_eq!(process_elements[1]["tag"], "collapsible_panel");
    assert_eq!(process_elements[1]["expanded"], false);
    assert!(process_elements[1]["header"]["title"]["content"]
        .as_str()
        .unwrap()
        .contains("已完成：shell"));
    assert!(process_elements[1]["elements"][0]["content"]
        .as_str()
        .unwrap()
        .contains("tests passed"));
    assert!(process_elements[2]["content"]
        .as_str()
        .unwrap()
        .contains("Now I will write the final summary."));
}

#[test]
fn progress_update_uuid_stays_short_and_avoids_card_id() {
    let long_card_id = format!("card_{}", "x".repeat(120));
    let mut handle = FeishuProgressCardHandle {
        card_id: long_card_id.clone(),
        message_id: Some("om_1".to_string()),
        sequence: 1,
        generation: 3,
        rendered_title: "title".to_string(),
        rendered_has_plan: false,
        rendered_has_tool: false,
        rendered_has_thinking: false,
        rendered_has_process: false,
        rendered_phase: ImProgressPhase::Running,
        rendered_output_hash: 0,
        rendered_plan_hash: None,
        rendered_tool_hash: None,
        rendered_status_hash: 0,
        rendered_thinking_hash: None,
        rendered_process_hash: None,
    };

    let (_, update_uuid) = handle.next_sequence();
    assert!(update_uuid.len() <= 50, "uuid too long: {update_uuid}");
    assert!(
        !update_uuid.contains(&long_card_id),
        "uuid must not include card_id"
    );
}

fn collect_element_ids(value: &serde_json::Value, element_ids: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(element_id) = map.get("element_id").and_then(|value| value.as_str()) {
                element_ids.push(element_id.to_string());
            }
            for child in map.values() {
                collect_element_ids(child, element_ids);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_element_ids(item, element_ids);
            }
        }
        _ => {}
    }
}

fn assert_feishu_element_id_is_valid(element_id: &str) {
    assert!(
        !element_id.is_empty(),
        "element_id must not be empty: {element_id:?}"
    );
    assert!(
        element_id.len() <= 20,
        "element_id exceeds Feishu 20 character limit: {element_id}"
    );
    assert!(
        element_id
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic()),
        "element_id must start with an alphabetic character: {element_id}"
    );
    assert!(
        element_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "element_id can only contain alphabets, numbers, and underscores: {element_id}"
    );
}

struct MockFeishuProgressServer {
    base_url: String,
    card_counter: Arc<std::sync::atomic::AtomicUsize>,
    message_counter: Arc<std::sync::atomic::AtomicUsize>,
    recall_counter: Arc<std::sync::atomic::AtomicUsize>,
    card_update_counter: Arc<std::sync::atomic::AtomicUsize>,
    settings_update_counter: Arc<std::sync::atomic::AtomicUsize>,
    card_create_payloads: Arc<std::sync::Mutex<Vec<String>>>,
    card_update_payloads: Arc<std::sync::Mutex<Vec<String>>>,
}

async fn spawn_mock_feishu_progress_server() -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(None, None, None, None).await
}

async fn spawn_mock_feishu_progress_server_with_send_failure(
    fail_message_send_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(fail_message_send_number, None, None, None)
        .await
}

async fn spawn_mock_feishu_progress_server_with_invalid_card_id_send_failure(
    fail_invalid_card_id_send_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        None,
        None,
        fail_invalid_card_id_send_number,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_card_update_failure(
    fail_card_update_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(None, fail_card_update_number, None, None).await
}

async fn spawn_mock_feishu_progress_server_with_card_update_and_create_failure(
    fail_card_update_number: Option<usize>,
    fail_card_create_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        fail_card_update_number,
        fail_card_create_number,
        None,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_failures(
    fail_message_send_number: Option<usize>,
    fail_card_update_number: Option<usize>,
    fail_card_create_number: Option<usize>,
    fail_invalid_card_id_send_number: Option<usize>,
) -> MockFeishuProgressServer {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let card_counter = Arc::new(AtomicUsize::new(0));
    let message_counter = Arc::new(AtomicUsize::new(0));
    let recall_counter = Arc::new(AtomicUsize::new(0));
    let card_update_counter = Arc::new(AtomicUsize::new(0));
    let settings_update_counter = Arc::new(AtomicUsize::new(0));
    let card_create_payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let card_update_payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock feishu server");
    let port = listener.local_addr().expect("mock local addr").port();
    let card_counter_for_server = Arc::clone(&card_counter);
    let message_counter_for_server = Arc::clone(&message_counter);
    let recall_counter_for_server = Arc::clone(&recall_counter);
    let card_update_counter_for_server = Arc::clone(&card_update_counter);
    let settings_update_counter_for_server = Arc::clone(&settings_update_counter);
    let card_create_payloads_for_server = Arc::clone(&card_create_payloads);
    let card_update_payloads_for_server = Arc::clone(&card_update_payloads);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            let card_counter = Arc::clone(&card_counter_for_server);
            let message_counter = Arc::clone(&message_counter_for_server);
            let recall_counter = Arc::clone(&recall_counter_for_server);
            let card_update_counter = Arc::clone(&card_update_counter_for_server);
            let settings_update_counter = Arc::clone(&settings_update_counter_for_server);
            let card_create_payloads = Arc::clone(&card_create_payloads_for_server);
            let card_update_payloads = Arc::clone(&card_update_payloads_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let card_counter = Arc::clone(&card_counter);
                    let message_counter = Arc::clone(&message_counter);
                    let recall_counter = Arc::clone(&recall_counter);
                    let card_update_counter = Arc::clone(&card_update_counter);
                    let settings_update_counter = Arc::clone(&settings_update_counter);
                    let card_create_payloads = Arc::clone(&card_create_payloads);
                    let card_update_payloads = Arc::clone(&card_update_payloads);
                    async move {
                        let method = req.method().clone();
                        let path = req.uri().path().to_string();
                        let body = req
                            .into_body()
                            .collect()
                            .await
                            .expect("collect request body")
                            .to_bytes();
                        if method == Method::POST
                            && path == "/open-apis/auth/v3/tenant_access_token/internal"
                        {
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from_static(
                                        br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                    )))
                                    .unwrap(),
                            );
                        }
                        if method == Method::POST && path == "/open-apis/cardkit/v1/cards" {
                            let idx = card_counter.fetch_add(1, Ordering::SeqCst) + 1;
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("create card json");
                            let data = body["data"].as_str().unwrap_or_default().to_string();
                            card_create_payloads
                                .lock()
                                .expect("create payloads lock")
                                .push(data);
                            if fail_card_create_number == Some(idx) {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(
                                            br#"{"code":300305,"msg":"ErrMsg: element exceeds the limit"}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from(format!(
                                        r#"{{"code":0,"data":{{"card_id":"card_{idx}"}}}}"#
                                    ))))
                                    .unwrap(),
                            );
                        }
                        if method == Method::PUT
                            && path.starts_with("/open-apis/cardkit/v1/cards/")
                            && !path.contains("/elements/")
                        {
                            let idx = card_update_counter.fetch_add(1, Ordering::SeqCst) + 1;
                            if fail_card_update_number == Some(idx) {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(
                                            br#"{"code":300305,"msg":"ErrMsg: element exceeds the limit"}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("update card json");
                            let data = body["card"]["data"].as_str().unwrap_or_default();
                            card_update_payloads
                                .lock()
                                .expect("update payloads lock")
                                .push(data.to_string());
                            assert!(data.contains("streaming_mode"));
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                    .unwrap(),
                            );
                        }
                        if method == Method::PATCH
                            && path.starts_with("/open-apis/cardkit/v1/cards/")
                            && path.ends_with("/settings")
                        {
                            settings_update_counter.fetch_add(1, Ordering::SeqCst);
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("update settings json");
                            let settings = body["settings"].as_str().unwrap_or_default();
                            assert!(settings.contains("\"streaming_mode\":false"));
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                    .unwrap(),
                            );
                        }
                        if method == Method::POST && path == "/open-apis/im/v1/messages" {
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("send card json");
                            let content = body["content"].as_str().unwrap_or_default();
                            assert!(content.contains("card_"));
                            let idx = message_counter.fetch_add(1, Ordering::SeqCst) + 1;
                            if fail_message_send_number == Some(idx) {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                                        .body(Full::new(Bytes::from_static(
                                            br#"{"code":99991664,"msg":"send denied"}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            if fail_invalid_card_id_send_number == Some(idx) {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(
                                            br#"{"code":230099,"msg":"Failed to create card content, ext=ErrCode: 11310; ErrMsg: cardid is invalid;"}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from(format!(
                                        r#"{{"code":0,"data":{{"message_id":"om_{idx}"}}}}"#
                                    ))))
                                    .unwrap(),
                            );
                        }
                        if method == Method::DELETE
                            && path.starts_with("/open-apis/im/v1/messages/")
                        {
                            recall_counter.fetch_add(1, Ordering::SeqCst);
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                    .unwrap(),
                            );
                        }
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(Full::new(Bytes::from_static(b"{}")))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    MockFeishuProgressServer {
        base_url: format!("http://127.0.0.1:{port}/open-apis"),
        card_counter,
        message_counter,
        recall_counter,
        card_update_counter,
        settings_update_counter,
        card_create_payloads,
        card_update_payloads,
    }
}

fn mock_feishu_provider(base_url: &str) -> ImProviderConfig {
    ImProviderConfig {
        id: "feishu-main".to_string(),
        provider_type: super::super::types::ImProviderType::Feishu,
        display_name: "Feishu Main".to_string(),
        enabled: true,
        base_url: Some(base_url.to_string()),
        app_id: Some("cli_xxx".to_string()),
        secret_ref: Some("secret".to_string()),
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn mock_progress_target() -> ImTarget {
    ImTarget {
        id: "progress".to_string(),
        provider_id: "feishu-main".to_string(),
        display_name: "Progress".to_string(),
        receive_id_type: "open_id".to_string(),
        receive_id: "ou_owner".to_string(),
        default_msg_type: "interactive".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn queue_state_update_rolls_over_card_and_freezes_previous_snapshot() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");
    {
        let mut session = session.lock().await;
        session
            .snapshot
            .apply_event(AgentTurnProgressEvent::TitleUpdated {
                title: "Investigate logs".to_string(),
            });
        session
            .snapshot
            .apply_event(AgentTurnProgressEvent::PlanUpdated {
                title: Some("Debug".to_string()),
                steps: vec![PlanStep {
                    step: "Check runtime logs".to_string(),
                    status: PlanStepStatus::InProgress,
                }],
            });
        session
            .snapshot
            .apply_event(AgentTurnProgressEvent::ToolStarted {
                tool_name: "exec_command".to_string(),
                arguments: "{\"cmd\":\"tail logs\"}".to_string(),
            });
    }

    assert!(
        registry
            .update_queue_state(
                "s1",
                vec![QueueItem {
                    seq: 2,
                    message: "follow-up".to_string(),
                    images: Vec::new(),
                }],
                true,
                Some("已收到引导：follow-up".to_string()),
            )
            .await
    );

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(session.snapshot().title.as_deref(), Some("Debug"));
    assert_eq!(session.snapshot().plan_steps.len(), 1);
    assert_eq!(
        session
            .snapshot()
            .latest_tool
            .as_ref()
            .map(|tool| tool.tool_name.as_str()),
        Some("exec_command")
    );
    assert_eq!(session.snapshot().queue_items.len(), 1);
    assert!(session.snapshot().guide_pending);
    assert_eq!(
        session.snapshot().activity_notice.as_deref(),
        Some("已收到引导：follow-up")
    );
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn progress_event_rolls_over_when_feishu_card_entity_exceeds_limit() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_card_update_failure(Some(1)).await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");

    registry
        .apply_event(
            "s1",
            AgentTurnProgressEvent::PlanUpdated {
                title: Some("Investigate".to_string()),
                steps: vec![PlanStep {
                    step: "Read logs".to_string(),
                    status: PlanStepStatus::InProgress,
                }],
            },
        )
        .await;

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(session.snapshot().title.as_deref(), Some("Investigate"));
    assert_eq!(session.snapshot().plan_steps.len(), 1);
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn progress_event_uses_compact_card_when_rollover_create_also_exceeds_limit() {
    use std::sync::atomic::Ordering;

    const OVERSIZED_MARKER: &str = "OVERSIZED_TOOL_OUTPUT_MARKER";

    let server =
        spawn_mock_feishu_progress_server_with_card_update_and_create_failure(Some(1), Some(2))
            .await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");

    registry
        .apply_event(
            "s1",
            AgentTurnProgressEvent::ToolFinished {
                log: ToolCallLog {
                    tool_name: "exec_command".to_string(),
                    arguments: "{\"cmd\":\"generate huge output\"}".to_string(),
                    result: format!("{OVERSIZED_MARKER}\n{}", "large output\n".repeat(100)),
                    success: true,
                },
                duration_ms: 99,
            },
        )
        .await;

    let message_info = {
        let session = session.lock().await;
        assert!(session.compact_card_mode);
        session.message_info().expect("message info")
    };
    assert_eq!(message_info.card_id, "card_3");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);

    let create_payloads = server
        .card_create_payloads
        .lock()
        .expect("create payloads")
        .clone();
    assert_eq!(create_payloads.len(), 3);
    assert!(
        create_payloads[1].contains(OVERSIZED_MARKER),
        "full rollover create must still carry the oversized process detail before fallback"
    );
    assert!(create_payloads[2].contains("精简状态卡"));
    assert!(create_payloads[2].contains("Agent 仍在运行"));
    assert!(
        !create_payloads[2].contains(OVERSIZED_MARKER),
        "compact fallback must drop oversized process details"
    );
    assert!(!create_payloads[2].contains(PROCESS_PANEL_ELEMENT_ID));

    registry
        .apply_event(
            "s1",
            AgentTurnProgressEvent::AssistantDelta {
                content: "继续检查恢复后的进度卡片。".to_string(),
            },
        )
        .await;

    let session = session.lock().await;
    assert!(session.compact_card_mode);
    assert_eq!(
        session.message_info().expect("message info").card_id,
        "card_3"
    );
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 3);
    let update_payloads = server
        .card_update_payloads
        .lock()
        .expect("update payloads")
        .clone();
    let compact_update = update_payloads.last().expect("compact update payload");
    assert!(compact_update.contains("精简状态卡"));
    assert!(compact_update.contains("继续检查恢复后的进度卡片"));
    assert!(!compact_update.contains(OVERSIZED_MARKER));
}

#[tokio::test(flavor = "current_thread")]
async fn finish_rolls_over_when_final_feishu_card_entity_exceeds_limit() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_card_update_failure(Some(1)).await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");

    let message_info = registry
        .finish(
            "s1",
            Some("最终结论：继续在新卡片显示。".to_string()),
            false,
        )
        .await
        .expect("finish message info");

    let session = session.lock().await;
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(
        session
            .message_info()
            .expect("session message info")
            .card_id,
        "card_2"
    );
    assert_eq!(session.snapshot().phase, ImProgressPhase::Finished);
    assert_eq!(session.snapshot().output, "最终结论：继续在新卡片显示。");
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn queue_state_rollover_sends_new_card_without_recall() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");

    assert!(
        registry
            .update_queue_state(
                "s1",
                vec![QueueItem {
                    seq: 1,
                    message: "queued after rollover".to_string(),
                    images: Vec::new(),
                }],
                false,
                Some("消息已排队：queued after rollover".to_string()),
            )
            .await
    );

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn queue_state_rollover_without_message_id_still_freezes_by_card_id() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");
    {
        let mut session = session.lock().await;
        session.handle.as_mut().expect("progress handle").message_id = None;
    }

    assert!(
        registry
            .update_queue_state(
                "s1",
                Vec::new(),
                true,
                Some("已收到引导：no old message id".to_string()),
            )
            .await
    );

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn queue_state_rollover_send_failure_keeps_previous_running_handle() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_send_failure(Some(2)).await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");

    assert!(
        !registry
            .update_queue_state(
                "s1",
                vec![QueueItem {
                    seq: 1,
                    message: "queued after send failure".to_string(),
                    images: Vec::new(),
                }],
                false,
                Some("消息已排队：queued after send failure".to_string()),
            )
            .await
    );

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_1");
    assert_eq!(message_info.message_id.as_deref(), Some("om_1"));
    assert_eq!(session.snapshot().queue_items.len(), 1);
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn finished_card_queue_state_update_does_not_rollover_or_freeze() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "finished turn",
        )
        .await
        .expect("start progress card");
    {
        let mut session = session.lock().await;
        session.snapshot.phase = ImProgressPhase::Finished;
    }

    assert!(
        !registry
            .update_queue_state(
                "s1",
                vec![QueueItem {
                    seq: 1,
                    message: "late message".to_string(),
                    images: Vec::new(),
                }],
                true,
                Some("已收到引导：late message".to_string()),
            )
            .await
    );

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_1");
    assert_eq!(message_info.message_id.as_deref(), Some("om_1"));
    assert!(session.snapshot().queue_items.is_empty());
    assert!(!session.snapshot().guide_pending);
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn rollover_existing_after_finished_card_returns_false_without_freezing_history() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "finished turn",
        )
        .await
        .expect("start progress card");
    {
        let mut session = session.lock().await;
        session.snapshot.phase = ImProgressPhase::Finished;
    }

    assert!(
        !registry
            .rollover_existing("s1", "new independent turn")
            .await
    );

    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_1");
    assert_eq!(message_info.message_id.as_deref(), Some("om_1"));
    assert_eq!(session.snapshot().title.as_deref(), Some("finished turn"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn start_feishu_after_finished_card_sends_new_card_without_recalling_history() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let first_session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "finished turn",
        )
        .await
        .expect("start progress card");
    {
        let mut session = first_session.lock().await;
        session.snapshot.phase = ImProgressPhase::Finished;
    }

    assert!(
        !registry
            .rollover_existing("s1", "new independent turn")
            .await
    );
    let second_session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "new independent turn",
        )
        .await
        .expect("start second progress card");

    let session = second_session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(
        session.snapshot().title.as_deref(),
        Some("new independent turn")
    );
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn start_feishu_after_finished_card_recovers_from_invalid_card_id_send() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_invalid_card_id_send_failure(Some(2)).await;
    let registry = ImAgentProgressRegistry::new();
    let first_session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "finished turn",
        )
        .await
        .expect("start first progress card");

    registry
        .finish("s1", Some("first turn done".to_string()), false)
        .await
        .expect("finish first progress card");

    assert!(
        !registry
            .rollover_existing("s1", "next turn after finish")
            .await
    );
    let second_session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "next turn after finish",
        )
        .await
        .expect("start second progress card after invalid card_id retry");

    let first_session = first_session.lock().await;
    assert_eq!(
        first_session
            .message_info()
            .expect("first message info")
            .card_id,
        "card_1"
    );
    drop(first_session);

    let second_session = second_session.lock().await;
    let message_info = second_session.message_info().expect("second message info");
    assert_eq!(message_info.card_id, "card_3");
    assert_eq!(message_info.message_id.as_deref(), Some("om_3"));
    assert_eq!(
        second_session.snapshot().title.as_deref(),
        Some("next turn after finish")
    );
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn rollover_existing_freezes_old_card_and_sends_new_card() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await
        .expect("start progress card");

    assert!(registry.rollover_existing("s1", "second turn").await);
    let session = session.lock().await;
    let message_info = session.message_info().expect("message info");
    assert_eq!(message_info.card_id, "card_2");
    assert_eq!(message_info.message_id.as_deref(), Some("om_2"));
    assert_eq!(session.snapshot().title.as_deref(), Some("second turn"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}
#[tokio::test(flavor = "current_thread")]
async fn recall_message_sends_delete_with_tenant_token() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::sync::{Arc, Mutex};

    let seen_delete = Arc::new(Mutex::new(false));
    let seen_delete_for_server = Arc::clone(&seen_delete);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock feishu server");
    let port = listener.local_addr().expect("mock local addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            let seen_delete = Arc::clone(&seen_delete_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let seen_delete = Arc::clone(&seen_delete);
                    async move {
                        let method = req.method().clone();
                        let path = req.uri().path().to_string();
                        let auth = req
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string);
                        let _body = req
                            .into_body()
                            .collect()
                            .await
                            .expect("collect request body")
                            .to_bytes();
                        if method == Method::POST
                            && path == "/open-apis/auth/v3/tenant_access_token/internal"
                        {
                            return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from_static(
                                            br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                        )))
                                        .unwrap(),
                                );
                        }
                        if method == Method::DELETE && path == "/open-apis/im/v1/messages/om_old" {
                            assert_eq!(auth.as_deref(), Some("Bearer tenant-token"));
                            *seen_delete.lock().expect("lock seen delete") = true;
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from_static(br#"{"code":0}"#)))
                                    .unwrap(),
                            );
                        }
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(Full::new(Bytes::from_static(b"{}")))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    let provider = FeishuProvider::new();
    let config = ImProviderConfig {
        id: "feishu-main".to_string(),
        provider_type: super::super::types::ImProviderType::Feishu,
        display_name: "Feishu Main".to_string(),
        enabled: true,
        base_url: Some(format!("http://127.0.0.1:{port}/open-apis")),
        app_id: Some("cli_xxx".to_string()),
        secret_ref: Some("secret".to_string()),
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    };

    provider
        .recall_message(&config, "om_old")
        .await
        .expect("recall message should succeed");
    assert!(*seen_delete.lock().expect("lock seen delete"));
}
