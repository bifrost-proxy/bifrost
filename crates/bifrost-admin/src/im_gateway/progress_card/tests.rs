use super::*;
use crate::im_gateway::provider::ImProvider;
use bifrost_agent::{PlanStepStatus, SubAgentProgress, SubAgentStatus};

#[test]
fn subagent_progress_upserts_by_agent_identity_and_renders_task_state_and_duration() {
    let mut snapshot = ImAgentProgressSnapshot::new("subagents", "coordinate review");
    snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
        progress: SubAgentProgress {
            id: "spawn-call-1".to_string(),
            agent_id: None,
            label: Some("reviewer".to_string()),
            task: "Review authentication handlers".to_string(),
            phase: "dispatching".to_string(),
            status: SubAgentStatus::Running,
            detail: None,
            started_at_ms: Some(1_000),
            updated_at_ms: 1_000,
            duration_ms: None,
        },
    });
    snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
        progress: SubAgentProgress {
            id: "spawn-call-1".to_string(),
            agent_id: Some("agent-7".to_string()),
            label: Some("reviewer".to_string()),
            task: "Review authentication handlers".to_string(),
            phase: "working".to_string(),
            status: SubAgentStatus::Running,
            detail: Some("Inspecting route guards".to_string()),
            started_at_ms: None,
            updated_at_ms: 3_500,
            duration_ms: None,
        },
    });
    snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
        progress: SubAgentProgress {
            id: "wait-call-2".to_string(),
            agent_id: Some("agent-7".to_string()),
            label: None,
            task: String::new(),
            phase: "waiting".to_string(),
            status: SubAgentStatus::Completed,
            detail: Some("Review complete".to_string()),
            started_at_ms: None,
            updated_at_ms: 5_200,
            duration_ms: None,
        },
    });

    assert_eq!(snapshot.timeline.len(), 1);
    let item = &snapshot.timeline[0];
    assert_eq!(item.kind, ProgressTimelineKind::SubAgent);
    assert_eq!(item.agent_id.as_deref(), Some("agent-7"));
    assert_eq!(item.subagent_status, Some(SubAgentStatus::Completed));
    assert_eq!(item.duration_ms, Some(4_200));
    assert!(item.detail.contains("Review authentication handlers"));
    assert!(item.detail.contains("Review complete"));

    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();
    assert!(serialized.contains("子 Agent"));
    assert!(serialized.contains("Review authentication handlers"));
    assert!(serialized.contains("Agent ID"));
    assert!(serialized.contains("agent-7"));
    assert!(serialized.contains("已完成"));
    assert!(serialized.contains("4 秒"));
}

#[test]
fn subagent_progress_covers_status_labels_defaults_and_short_duration() {
    let statuses = [
        (SubAgentStatus::Pending, "待启动"),
        (SubAgentStatus::Running, "执行中"),
        (SubAgentStatus::Completed, "已完成"),
        (SubAgentStatus::Failed, "失败"),
        (SubAgentStatus::Interrupted, "已中断"),
        (SubAgentStatus::Unknown, "状态未知"),
    ];
    for (status, label) in statuses {
        assert_eq!(subagent_status_label(status), label);
    }
    assert_eq!(format_subagent_duration(450), "450ms");

    let mut snapshot = ImAgentProgressSnapshot::new("subagent-defaults", "coordinate");
    snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
        progress: SubAgentProgress {
            id: "pending-1".to_string(),
            agent_id: None,
            label: None,
            task: String::new(),
            phase: "dispatching".to_string(),
            status: SubAgentStatus::Pending,
            detail: None,
            started_at_ms: None,
            updated_at_ms: 0,
            duration_ms: None,
        },
    });
    assert_eq!(snapshot.timeline.len(), 1);
    assert!(snapshot.timeline[0].started_at_ms.is_some());
    assert_eq!(snapshot.timeline[0].success, None);

    snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
        progress: SubAgentProgress {
            id: "pending-1".to_string(),
            agent_id: None,
            label: None,
            task: String::new(),
            phase: "finished".to_string(),
            status: SubAgentStatus::Failed,
            detail: None,
            started_at_ms: None,
            updated_at_ms: snapshot.timeline[0].started_at_ms.unwrap() + 450,
            duration_ms: None,
        },
    });
    assert_eq!(snapshot.timeline[0].duration_ms, Some(450));
    assert_eq!(snapshot.timeline[0].success, Some(false));
}

#[test]
fn subagent_budget_trims_old_terminal_entries_but_preserves_running_and_latest_five() {
    let mut snapshot = ImAgentProgressSnapshot::new("subagent-budget", "coordinate reviews");
    for index in 0..7 {
        snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
            progress: SubAgentProgress {
                id: format!("done-{index}"),
                agent_id: Some(format!("agent-{index}")),
                label: Some("reviewer".to_string()),
                task: format!("Review module {index}"),
                phase: "finished".to_string(),
                status: SubAgentStatus::Completed,
                detail: Some("done".to_string()),
                started_at_ms: Some(1_000),
                updated_at_ms: 2_000,
                duration_ms: Some(1_000),
            },
        });
    }
    snapshot.apply_event(AgentTurnProgressEvent::SubAgentUpdated {
        progress: SubAgentProgress {
            id: "running".to_string(),
            agent_id: Some("agent-running".to_string()),
            label: Some("tester".to_string()),
            task: "Run tests".to_string(),
            phase: "working".to_string(),
            status: SubAgentStatus::Running,
            detail: None,
            started_at_ms: Some(1_000),
            updated_at_ms: 2_000,
            duration_ms: None,
        },
    });

    let first = oldest_budget_removable_subagent_range(&snapshot.timeline).unwrap();
    assert_eq!(first, 0..1);
    snapshot.timeline.drain(first);
    let second = oldest_budget_removable_subagent_range(&snapshot.timeline).unwrap();
    assert_eq!(second, 0..1);
    snapshot.timeline.drain(second);
    assert!(oldest_budget_removable_subagent_range(&snapshot.timeline).is_none());
    assert!(snapshot.timeline.iter().any(|item| {
        item.kind == ProgressTimelineKind::SubAgent
            && item.subagent_status == Some(SubAgentStatus::Running)
    }));
}

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
            files: Vec::new(),
            context: None,
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
    assert!(snapshot.timeline[0].detail.len() < 3200);
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

    assert!(serialized.contains("文件变更"));
    assert!(serialized.contains("src/main.rs"));
    assert!(serialized.contains("updated startup text"));
    assert!(serialized.contains("-old"));
    assert!(serialized.contains("+new"));
    assert!(!serialized.contains("暂无工具详情"));
}

#[test]
fn process_tool_group_title_distinguishes_running_and_failed_steps() {
    let mut running = ImAgentProgressSnapshot::new("s1", "running task");
    running.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "cargo test".to_string(),
    });
    let running_items = running.timeline.iter().enumerate().collect::<Vec<_>>();
    assert_eq!(
        format_process_tool_group_title(&running_items),
        "正在执行 1 个步骤"
    );

    let mut failed = ImAgentProgressSnapshot::new("s2", "failed task");
    failed.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: "cargo test".to_string(),
            result: "failed".to_string(),
            success: false,
        },
        duration_ms: 0,
    });
    let failed_items = failed.timeline.iter().enumerate().collect::<Vec<_>>();
    assert_eq!(
        format_process_tool_group_title(&failed_items),
        "已执行 1 个步骤，1 个失败"
    );
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
        "已执行 3 个步骤"
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
        snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
            content: format!("THINKING_ROUND_{index}"),
        });
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: format!("tool_{index}"),
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
        "执行过程：共 70 步 · 工具 35 次"
    );
    let process_elements = process_element["elements"].as_array().unwrap();
    assert_eq!(process_elements[0]["element_id"], PROCESS_LOG_ELEMENT_ID);
    assert!(process_elements[0]["content"]
        .as_str()
        .unwrap()
        .contains("已省略前面 5 次工具调用，仅显示最新 30 次。"));

    let serialized = serde_json::to_string(&card).unwrap();
    assert!(!serialized.contains("tool-0"));
    assert!(!serialized.contains("result-4"));
    assert!(!serialized.contains("THINKING_ROUND_0"));
    assert!(serialized.contains("THINKING_ROUND_5"));
    assert!(serialized.contains("步骤：`tool_5` · 完成"));
    assert!(!serialized.contains("result-5"));
    assert!(serialized.contains("ap_t_61"));
    assert!(serialized.contains("result-34"));
    assert_eq!(
        oldest_budget_removable_timeline_range(&snapshot.timeline),
        Some(0..10)
    );
}

#[test]
fn process_tool_detail_caps_input_and_output_previews_at_three_hundred_chars() {
    let input_visible = "INPUT_VISIBLE";
    let input_hidden = "INPUT_HIDDEN";
    let output_visible = "OUTPUT_VISIBLE";
    let output_hidden = "OUTPUT_HIDDEN";
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "long tool detail");
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: format!("{input_visible}{}{input_hidden}", "a".repeat(300)),
            result: format!("{output_visible}{}{output_hidden}", "b".repeat(300)),
            success: true,
        },
        duration_ms: 12,
    });

    let serialized = serde_json::to_string(&build_feishu_progress_card(&snapshot, true)).unwrap();
    assert!(serialized.contains(input_visible));
    assert!(serialized.contains(output_visible));
    assert!(!serialized.contains(input_hidden));
    assert!(!serialized.contains(output_hidden));
    assert_eq!(
        truncate_str_within_limit(&"中".repeat(301), PROCESS_TOOL_INPUT_PREVIEW_CHARS)
            .chars()
            .count(),
        300
    );
    assert_eq!(
        truncate_str_within_limit(&"x".repeat(301), PROCESS_TOOL_OUTPUT_PREVIEW_CHARS)
            .chars()
            .count(),
        300
    );
    let fallback_tool_panel = format_tool_details_markdown(
        &[ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: String::new(),
            result: format!("FALLBACK_VISIBLE{}FALLBACK_HIDDEN", "x".repeat(300)),
            success: true,
        }],
        None,
    );
    assert!(fallback_tool_panel.contains("FALLBACK_VISIBLE"));
    assert!(!fallback_tool_panel.contains("FALLBACK_HIDDEN"));
}

#[test]
fn budgeted_card_drops_oldest_process_items_and_keeps_latest() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "budget tool history");
    for index in 0..8 {
        if index < 3 {
            snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
                content: format!("OLD_THINKING_MARKER_{index}\n{}", "t".repeat(600)),
            });
        }
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "exec_command".to_string(),
                arguments: format!(r#"{{"cmd":"tool-{index}"}}"#),
                result: format!("RESULT_MARKER_{index}\n{}", "x".repeat(2600)),
                success: true,
            },
            duration_ms: 10 + index,
        });
    }

    let rendered = build_budgeted_feishu_progress_card(
        &snapshot,
        true,
        FeishuCardBudget {
            max_bytes: 5_000,
            max_components: 180,
        },
    );
    let serialized = serde_json::to_string(&rendered.card).unwrap();
    assert!(!rendered.compact);
    assert!(rendered.omitted_timeline_items > 0);
    assert!(serialized.len() <= 5_000);
    assert!(serialized.contains("RESULT_MARKER_7"));
    assert!(!serialized.contains("RESULT_MARKER_0"));
    assert!(!serialized.contains("OLD_THINKING_MARKER_0"));
    assert!(serialized.contains("已省略前面"));
    assert!(serialized.contains("执行过程：共 11 步 · 工具 8 次"));
    assert!(serialized.contains("保留最近执行脉络"));
}

#[test]
fn budget_removal_without_tools_drops_status_before_preserving_five_thinking_items() {
    let statuses = vec![
        ProgressTimelineItem::status("old status".to_string()),
        ProgressTimelineItem::status("latest status".to_string()),
    ];
    assert_eq!(
        oldest_budget_removable_timeline_range(&statuses),
        Some(0..1)
    );
    assert_eq!(
        oldest_budget_removable_timeline_range(&statuses[1..]),
        Some(0..1)
    );

    let thinking = (0..6)
        .map(|index| ProgressTimelineItem::thinking(format!("thinking-{index}")))
        .collect::<Vec<_>>();
    assert_eq!(
        oldest_budget_removable_timeline_range(&thinking),
        Some(0..1)
    );
    assert_eq!(oldest_budget_removable_timeline_range(&thinking[1..]), None);
}

#[test]
fn budget_removal_tool_boundaries_and_step_statuses_cover_all_states() {
    let running =
        ProgressTimelineItem::tool_started("running_tool".to_string(), "RUNNING_INPUT".to_string());
    assert_eq!(
        format_process_tool_step_line(&running),
        "步骤：`running_tool` · 执行中"
    );
    assert_eq!(
        oldest_budget_removable_timeline_range(std::slice::from_ref(&running)),
        None
    );

    let failed = ProgressTimelineItem::tool_finished(
        &ToolCallLog {
            tool_name: "failed_tool".to_string(),
            arguments: "FAILED_INPUT".to_string(),
            result: "FAILED_OUTPUT".to_string(),
            success: false,
        },
        12,
    );
    assert_eq!(
        format_process_tool_step_line(&failed),
        "步骤：`failed_tool` · 失败 · 12ms"
    );

    let consecutive_tools = (0..7)
        .map(|index| {
            ProgressTimelineItem::tool_finished(
                &ToolCallLog {
                    tool_name: format!("tool_{index}"),
                    arguments: String::new(),
                    result: String::new(),
                    success: true,
                },
                10,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        oldest_budget_removable_timeline_range(&consecutive_tools),
        Some(0..2)
    );
}

#[test]
fn old_tools_render_as_steps_while_latest_five_keep_expandable_details() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "balanced tool history");
    for index in 0..8 {
        snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
            content: format!("THINKING_ROUND_{index}"),
        });
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: format!("tool_{index}"),
                arguments: format!("TOOL_INPUT_{index}"),
                result: format!("TOOL_OUTPUT_{index}"),
                success: true,
            },
            duration_ms: 10 + index,
        });
    }

    let serialized = serde_json::to_string(&build_feishu_progress_card(&snapshot, true)).unwrap();
    for index in 0..3 {
        assert!(serialized.contains(&format!("步骤：`tool_{index}` · 完成")));
        assert!(!serialized.contains(&format!("TOOL_INPUT_{index}")));
        assert!(!serialized.contains(&format!("TOOL_OUTPUT_{index}")));
    }
    for index in 3..8 {
        assert!(serialized.contains(&format!("TOOL_INPUT_{index}")));
        assert!(serialized.contains(&format!("TOOL_OUTPUT_{index}")));
    }
    assert_eq!(
        serialized.matches(r#""tag":"collapsible_panel""#).count(),
        7
    );
}

#[test]
fn budgeted_card_removes_old_execution_segments_before_preserving_latest_five_rounds() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "thinking-first budget");
    for index in 0..8 {
        snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
            content: format!("THINKING_ROUND_{index}_{}", "思考".repeat(240)),
        });
        snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: format!("tool_{index}"),
                arguments: format!("TOOL_INPUT_{index}_{}", "i".repeat(500)),
                result: format!("TOOL_OUTPUT_{index}_{}", "o".repeat(500)),
                success: true,
            },
            duration_ms: 10,
        });
    }

    let rendered = build_budgeted_feishu_progress_card(
        &snapshot,
        true,
        FeishuCardBudget {
            max_bytes: 16 * 1024,
            max_components: 180,
        },
    );
    let serialized = serde_json::to_string(&rendered.card).unwrap();

    assert!(!rendered.compact);
    assert!(rendered.omitted_timeline_items > 0);
    for index in 3..8 {
        assert!(
            serialized.contains(&format!("THINKING_ROUND_{index}")),
            "latest five thinking rounds must remain visible: {serialized}"
        );
    }
    assert!(serialized.contains("TOOL_OUTPUT_7"));
    assert!(serialized.contains("TOOL_OUTPUT_3"));
    assert!(!serialized.contains("THINKING_ROUND_0"));
    assert!(!serialized.contains("TOOL_OUTPUT_0"));
}

#[test]
fn budgeted_card_counts_utf8_json_bytes_and_falls_back_to_compact_output() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "large final output");
    snapshot.output = "中\"文\\内容".repeat(4000);
    snapshot.phase = ImProgressPhase::Finished;

    let rendered = build_budgeted_feishu_progress_card(
        &snapshot,
        false,
        FeishuCardBudget {
            max_bytes: 8 * 1024,
            max_components: 180,
        },
    );
    let serialized = serde_json::to_vec(&rendered.card).unwrap();
    assert!(rendered.compact);
    assert!(serialized.len() < snapshot.output.len());
    assert!(rendered.card.to_string().contains("精简状态卡"));
}

#[test]
fn feishu_card_limit_errors_are_classified_by_official_codes() {
    let size_error = BifrostError::Network(
        "feishu update card entity failed: code=200860, msg=Card content exceeds limit".to_string(),
    );
    let component_error = BifrostError::Network(
        "feishu update card entity failed: code=300305, msg=element exceeds the limit".to_string(),
    );
    let ordinary_error = BifrostError::Network(
        "feishu update card entity failed: code=10002, msg=invalid request".to_string(),
    );
    let non_network_error = BifrostError::Config("feishu card config".to_string());

    assert_eq!(
        feishu_card_limit_kind(&size_error),
        Some(FeishuCardLimitKind::ContentSize)
    );
    assert_eq!(
        feishu_card_limit_kind(&component_error),
        Some(FeishuCardLimitKind::ComponentCount)
    );
    assert_eq!(feishu_card_limit_kind(&ordinary_error), None);
    assert_eq!(feishu_card_limit_kind(&non_network_error), None);
}

#[test]
fn feishu_progress_card_collapses_process_with_current_round_summary() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "stream task");
    snapshot.apply_event(AgentTurnProgressEvent::AssistantFinal {
        content: "上一轮先读取配置。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "read_file".to_string(),
            arguments: "config.toml".to_string(),
            result: "ok".to_string(),
            success: true,
        },
        duration_ms: 10,
    });
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "我会先检查代码路径，然后运行测试。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "view_image".to_string(),
            arguments: "failure.png".to_string(),
            result: "opened".to_string(),
            success: true,
        },
        duration_ms: 20,
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "read_file".to_string(),
            arguments: "missing.rs".to_string(),
            result: "not found".to_string(),
            success: false,
        },
        duration_ms: 21,
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: r#"{"cmd":"cargo test -p bifrost-admin progress_card"}"#.to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: r#"{"cmd":"cargo clippy -p bifrost-admin"}"#.to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "web_search".to_string(),
        arguments: "CardKit theme".to_string(),
    });

    let running_card = build_feishu_progress_card(&snapshot, true);
    let running_serialized = serde_json::to_string(&running_card).unwrap();
    assert!(running_serialized.contains(PROCESS_PANEL_ELEMENT_ID));
    let elements = running_card["body"]["elements"].as_array().unwrap();
    let summary = elements
        .iter()
        .find(|element| element["element_id"] == PROCESS_SUMMARY_ELEMENT_ID)
        .expect("process summary");
    let summary_content = summary["content"].as_str().unwrap();
    assert!(summary_content.contains("我会先检查代码路径，然后运行测试。"));
    assert!(summary_content.contains("当前工具：`exec_command` ×2、`web_search`"));
    assert!(summary_content.contains("本轮工具：成功 1 · 失败 1 · 执行中 3"));
    assert!(!summary_content.contains("上一轮先读取配置"));
    let process = elements
        .iter()
        .find(|element| element["element_id"] == PROCESS_PANEL_ELEMENT_ID)
        .expect("process panel");
    assert_eq!(process["expanded"], false);
    assert_eq!(
        process["header"]["title"]["content"],
        "执行过程：共 8 步 · 工具 6 次"
    );
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
    assert_eq!(elements[1]["element_id"], PROCESS_SUMMARY_ELEMENT_ID);
    assert_eq!(elements[2]["element_id"], PROCESS_PANEL_ELEMENT_ID);
    assert_eq!(elements[2]["expanded"], false);
    assert_eq!(elements.last().unwrap()["element_id"], OUTPUT_ELEMENT_ID);
    assert_eq!(elements.last().unwrap()["tag"], "collapsible_panel");
    assert_eq!(elements.last().unwrap()["expanded"], false);
    assert_eq!(
        elements.last().unwrap()["header"]["title"]["content"],
        "最终结论"
    );
    assert_eq!(
        elements.last().unwrap()["elements"][0]["element_id"],
        OUTPUT_CONTENT_ELEMENT_ID
    );

    let finished_serialized = serde_json::to_string(&finished_card).unwrap();
    assert!(finished_serialized.contains("最终结论：测试通过。"));
    assert!(finished_serialized.contains("执行过程：共 8 步 · 工具 6 次"));
    assert!(finished_serialized.contains("已完成：exec_command"));
    assert!(finished_serialized.contains("test result: ok"));
    assert!(!finished_serialized.contains("Pipeline"));
    assert!(!finished_serialized.contains("Loop"));
    assert!(!finished_serialized.contains("工具摘要"));
    assert!(elements[2].to_string().contains("已完成：exec_command"));
}

#[test]
fn external_runner_status_title_exposes_session_id_lifecycle() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "runner task");
    snapshot.runner = Some(ProgressRunnerSummary {
        runner_id: "claude-code".to_string(),
        adapter: "claude-code".to_string(),
        ..ProgressRunnerSummary::default()
    });

    assert!(format_status_panel_title(&snapshot).contains("Session：获取中"));

    snapshot.runner.as_mut().unwrap().external_thread_id = Some("session-live-123".to_string());
    assert!(format_status_panel_title(&snapshot).contains("Session：session-live-123"));

    snapshot.runner.as_mut().unwrap().external_thread_id = None;
    snapshot.phase = ImProgressPhase::Finished;
    assert!(format_status_panel_title(&snapshot).contains("Session：未提供"));
}

#[test]
fn process_summary_limits_running_tool_types_without_public_explanation() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "parallel tools");
    for tool_name in ["exec_command", "view_image", "web_search", "read_file"] {
        snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
            tool_name: tool_name.to_string(),
            arguments: tool_name.to_string(),
        });
    }

    let summary = format_process_summary_markdown(&snapshot);
    assert!(summary.contains("等待模型输出下一步说明。"));
    assert!(summary.contains("`exec_command`、`view_image`、`web_search`，等 1 个"));
    assert!(!summary.contains("`read_file`"));
    assert!(summary.contains("成功 0 · 失败 0 · 执行中 4"));
}

#[test]
fn generated_feishu_cards_use_theme_adaptive_colors() {
    fn assert_theme_safe(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    if key == "background_color" || key == "text_color" {
                        assert_eq!(child, "default", "non-adaptive {key}: {child}");
                    }
                    assert_theme_safe(child);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    assert_theme_safe(item);
                }
            }
            serde_json::Value::String(text) => {
                let lower = text.to_ascii_lowercase();
                assert!(!lower.contains("rgba("));
                assert!(!lower.contains("rgb("));
                assert!(!lower.contains("<font color='black'"));
                assert!(!lower.contains("<font color='white'"));
                assert!(!lower.contains("<font color=\"black\""));
                assert!(!lower.contains("<font color=\"white\""));
            }
            _ => {}
        }
    }

    let mut snapshot = ImAgentProgressSnapshot::new("s1", "theme task");
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "检查主题。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolStarted {
        tool_name: "exec_command".to_string(),
        arguments: "cargo test".to_string(),
    });

    let standard = build_feishu_progress_card(&snapshot, true);
    let compact = build_feishu_compact_progress_card(&snapshot, true);
    let mut legacy_tool_snapshot = ImAgentProgressSnapshot::new("s2", "legacy tool theme");
    legacy_tool_snapshot.latest_tool = Some(ProgressToolSummary {
        tool_name: "exec_command".to_string(),
        arguments: Some("cargo test".to_string()),
        success: None,
        result_preview: None,
        duration_ms: None,
    });
    let legacy_tool = build_feishu_progress_card(&legacy_tool_snapshot, true);
    assert_theme_safe(&standard);
    assert_theme_safe(&compact);
    assert_theme_safe(&legacy_tool);

    for card in [&standard, &compact, &legacy_tool] {
        for panel in card["body"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|element| element["tag"] == "collapsible_panel")
        {
            assert_eq!(panel["background_color"], "default");
            assert_eq!(panel["header"]["title"]["text_color"], "default");
        }
    }
}

#[test]
fn terminal_progress_card_collapses_status_plan_process_and_conclusion() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "terminal layout");
    snapshot.apply_event(AgentTurnProgressEvent::PlanUpdated {
        title: Some("Implement terminal layout".to_string()),
        steps: vec![PlanStep {
            step: "Verify collapsed panels".to_string(),
            status: PlanStepStatus::Completed,
        }],
    });
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "先检查终态布局。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: "cargo test".to_string(),
            result: "ok".to_string(),
            success: true,
        },
        duration_ms: 15,
    });

    let running_card = build_feishu_progress_card(&snapshot, true);
    let running_elements = running_card["body"]["elements"].as_array().unwrap();
    assert_eq!(running_elements[0]["expanded"], false);
    assert_eq!(running_elements[1]["element_id"], PLAN_PANEL_ELEMENT_ID);
    assert_eq!(running_elements[1]["expanded"], true);
    assert_eq!(
        running_elements[2]["element_id"],
        PROCESS_SUMMARY_ELEMENT_ID
    );
    assert_eq!(running_elements[3]["element_id"], PROCESS_PANEL_ELEMENT_ID);
    assert_eq!(running_elements[3]["expanded"], false);
    assert_eq!(running_elements[4]["element_id"], OUTPUT_ELEMENT_ID);
    assert_eq!(running_elements[4]["tag"], "markdown");

    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "TERMINAL_COLLAPSED_CONCLUSION".to_string(),
    });

    let card = build_feishu_progress_card(&snapshot, false);
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(
        elements
            .iter()
            .map(|element| element["element_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            STATUS_PANEL_ELEMENT_ID,
            PLAN_PANEL_ELEMENT_ID,
            PROCESS_SUMMARY_ELEMENT_ID,
            PROCESS_PANEL_ELEMENT_ID,
            OUTPUT_ELEMENT_ID,
        ]
    );
    for element in elements
        .iter()
        .filter(|element| element["element_id"] != PROCESS_SUMMARY_ELEMENT_ID)
    {
        assert_eq!(
            element["tag"], "collapsible_panel",
            "terminal section must be collapsible: {element}"
        );
        assert_eq!(
            element["expanded"], false,
            "terminal section must default to collapsed: {element}"
        );
    }
    assert_eq!(elements[4]["header"]["title"]["content"], "最终结论");
    assert!(serde_json::to_string(&elements[4])
        .unwrap()
        .contains("TERMINAL_COLLAPSED_CONCLUSION"));
}

#[test]
fn failed_progress_card_collapses_failure_conclusion_in_standard_and_compact_cards() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "failed layout");
    snapshot.apply_event(AgentTurnProgressEvent::PlanUpdated {
        title: Some("Investigate failure".to_string()),
        steps: vec![PlanStep {
            step: "Reproduce failure".to_string(),
            status: PlanStepStatus::InProgress,
        }],
    });
    snapshot.apply_event(AgentTurnProgressEvent::AssistantDelta {
        content: "先复现失败路径。".to_string(),
    });
    snapshot.apply_event(AgentTurnProgressEvent::ToolFinished {
        log: ToolCallLog {
            tool_name: "exec_command".to_string(),
            arguments: "cargo test".to_string(),
            result: "failed".to_string(),
            success: false,
        },
        duration_ms: 20,
    });
    snapshot.apply_event(AgentTurnProgressEvent::TurnFailed {
        error: "TERMINAL_COLLAPSED_FAILURE".to_string(),
    });

    let standard = build_feishu_progress_card(&snapshot, false);
    let standard_elements = standard["body"]["elements"].as_array().unwrap();
    for element_id in [
        STATUS_PANEL_ELEMENT_ID,
        PLAN_PANEL_ELEMENT_ID,
        PROCESS_PANEL_ELEMENT_ID,
        OUTPUT_ELEMENT_ID,
    ] {
        let element = standard_elements
            .iter()
            .find(|element| element["element_id"] == element_id)
            .expect("terminal section");
        assert_eq!(element["tag"], "collapsible_panel");
        assert_eq!(element["expanded"], false);
    }

    for card in [
        standard,
        build_feishu_compact_progress_card(&snapshot, false),
    ] {
        let output = card["body"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|element| element["element_id"] == OUTPUT_ELEMENT_ID)
            .expect("output element");
        assert_eq!(output["tag"], "collapsible_panel");
        assert_eq!(output["expanded"], false);
        assert_eq!(output["header"]["title"]["content"], "失败结论");
        assert!(serde_json::to_string(output)
            .unwrap()
            .contains("TERMINAL_COLLAPSED_FAILURE"));
    }
}

#[test]
fn progress_output_content_updates_target_the_visible_phase_specific_markdown_element() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "output patch target");
    assert_eq!(output_content_element_id(&snapshot), OUTPUT_ELEMENT_ID);

    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "done".to_string(),
    });
    assert_eq!(
        output_content_element_id(&snapshot),
        OUTPUT_CONTENT_ELEMENT_ID
    );

    snapshot.apply_event(AgentTurnProgressEvent::TurnFailed {
        error: "failed".to_string(),
    });
    assert_eq!(
        output_content_element_id(&snapshot),
        OUTPUT_CONTENT_ELEMENT_ID
    );
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
        weekly_usage: Some(ProgressRunnerWeeklyUsage {
            used_percent: 63,
            window_minutes: 10_080,
            resets_at: Some(1_800_000_000),
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
        files: Vec::new(),
        context: None,
    });
    snapshot.guide_pending = true;
    snapshot.phase = ImProgressPhase::Finished;

    let title = format_status_panel_title(&snapshot);
    let footer = format_footer_markdown(&snapshot);

    assert!(title.contains("Runner：codex"));
    assert!(title.contains("模型：gpt-test（runner 配置）"));
    assert!(title.contains("思考：high"));
    assert!(title.contains("本次：1.3K Token"));
    assert!(title.contains("周余额：37%"));
    assert!(footer.contains("状态：已完成"));
    assert!(footer.contains("Runner：`codex` · Adapter：`codex` · Session ID：`thread-123`"));
    assert!(footer.contains("模型：gpt-test（runner 配置）"));
    assert!(footer.contains("思考：high · 摘要：auto"));
    assert!(footer.contains("Token：总计 1.3K · 输入 1.2K · 输出 80"));
    assert!(footer.contains("Context：最近输入 1.2K / N/A"));
    assert!(footer.contains("缓存输入 300"));
    assert!(footer.contains("推理输出 20"));
    assert!(footer.contains("Codex 周额度：剩余 37%（已用 63%） · 重置："));
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
        weekly_usage: None,
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
    assert!(!footer.contains("Session ID："));
    assert!(!footer.contains("当前状态：model_request"));
}

#[test]
fn external_runner_footer_bounds_and_escapes_session_id() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(ProgressRunnerSummary {
        runner_id: "codex".to_string(),
        adapter: "codex".to_string(),
        external_thread_id: Some(format!("unsafe`{}", "x".repeat(100))),
        ..ProgressRunnerSummary::default()
    });

    let footer = format_footer_markdown(&snapshot);

    assert!(footer.contains("Session ID：unsafe\\`"));
    assert!(!footer.contains("Session ID：`unsafe`"));
    assert!(!footer.contains(&"x".repeat(81)));
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
        weekly_usage: None,
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
    assert!(card.get("header").is_none());

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
            files: Vec::new(),
            context: None,
        }],
        true,
        Some("已收到引导：rerun failed path".to_string()),
    );
    let populated_card = build_feishu_progress_card(&populated, true);
    let populated_body = populated_card["body"]["elements"].as_array().unwrap();
    let populated_serialized = serde_json::to_string(populated_body).unwrap();
    for id in [
        PLAN_ELEMENT_ID,
        PROCESS_SUMMARY_ELEMENT_ID,
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
    assert!(populated_card.get("header").is_none());
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
    assert_eq!(process_element["expanded"], false);
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

pub(crate) struct MockFeishuProgressServer {
    pub(crate) base_url: String,
    card_counter: Arc<std::sync::atomic::AtomicUsize>,
    message_counter: Arc<std::sync::atomic::AtomicUsize>,
    recall_counter: Arc<std::sync::atomic::AtomicUsize>,
    card_update_counter: Arc<std::sync::atomic::AtomicUsize>,
    settings_update_counter: Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) card_create_payloads: Arc<std::sync::Mutex<Vec<String>>>,
    pub(crate) card_update_payloads: Arc<std::sync::Mutex<Vec<String>>>,
    pub(crate) message_paths: Arc<std::sync::Mutex<Vec<String>>>,
    pub(crate) message_payloads: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

pub(crate) async fn spawn_mock_feishu_progress_server() -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(None, Vec::new(), Vec::new(), None).await
}

pub(crate) async fn spawn_mock_feishu_progress_server_with_send_failure(
    fail_message_send_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        fail_message_send_number,
        Vec::new(),
        Vec::new(),
        None,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_invalid_card_id_send_failure(
    fail_invalid_card_id_send_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        Vec::new(),
        Vec::new(),
        fail_invalid_card_id_send_number,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_card_update_failure(
    fail_card_update_number: Option<usize>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        fail_card_update_number
            .into_iter()
            .map(|number| (number, 300305))
            .collect(),
        Vec::new(),
        None,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_card_update_limit(
    fail_card_update_number: Option<usize>,
    limit_error_code: i64,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        fail_card_update_number
            .into_iter()
            .map(|number| (number, limit_error_code))
            .collect(),
        Vec::new(),
        None,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_card_update_limits(
    fail_card_update_numbers: Vec<usize>,
    limit_error_code: i64,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        fail_card_update_numbers
            .into_iter()
            .map(|number| (number, limit_error_code))
            .collect(),
        Vec::new(),
        None,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_card_update_codes(
    fail_card_update_codes: Vec<(usize, i64)>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(None, fail_card_update_codes, Vec::new(), None)
        .await
}

async fn spawn_mock_feishu_progress_server_with_card_create_limit(
    fail_card_create_number: Option<usize>,
    limit_error_code: i64,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(
        None,
        Vec::new(),
        fail_card_create_number
            .into_iter()
            .map(|number| (number, limit_error_code))
            .collect(),
        None,
    )
    .await
}

async fn spawn_mock_feishu_progress_server_with_card_create_codes(
    fail_card_create_codes: Vec<(usize, i64)>,
) -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_failures(None, Vec::new(), fail_card_create_codes, None)
        .await
}

async fn spawn_mock_feishu_progress_server_with_failures(
    fail_message_send_number: Option<usize>,
    fail_card_update_codes: Vec<(usize, i64)>,
    fail_card_create_codes: Vec<(usize, i64)>,
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
    let message_paths = Arc::new(std::sync::Mutex::new(Vec::new()));
    let message_payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let fail_card_update_codes = Arc::new(
        fail_card_update_codes
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>(),
    );
    let fail_card_create_codes = Arc::new(
        fail_card_create_codes
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>(),
    );
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
    let message_paths_for_server = Arc::clone(&message_paths);
    let message_payloads_for_server = Arc::clone(&message_payloads);
    let fail_card_update_codes_for_server = Arc::clone(&fail_card_update_codes);
    let fail_card_create_codes_for_server = Arc::clone(&fail_card_create_codes);
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
            let message_paths = Arc::clone(&message_paths_for_server);
            let message_payloads = Arc::clone(&message_payloads_for_server);
            let fail_card_update_codes = Arc::clone(&fail_card_update_codes_for_server);
            let fail_card_create_codes = Arc::clone(&fail_card_create_codes_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let card_counter = Arc::clone(&card_counter);
                    let message_counter = Arc::clone(&message_counter);
                    let recall_counter = Arc::clone(&recall_counter);
                    let card_update_counter = Arc::clone(&card_update_counter);
                    let settings_update_counter = Arc::clone(&settings_update_counter);
                    let card_create_payloads = Arc::clone(&card_create_payloads);
                    let card_update_payloads = Arc::clone(&card_update_payloads);
                    let message_paths = Arc::clone(&message_paths);
                    let message_payloads = Arc::clone(&message_payloads);
                    let fail_card_update_codes = Arc::clone(&fail_card_update_codes);
                    let fail_card_create_codes = Arc::clone(&fail_card_create_codes);
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
                            if let Some(error_code) = fail_card_create_codes.get(&idx) {
                                let message = if *error_code == 200860 {
                                    "Card content exceeds limit"
                                } else {
                                    "element exceeds the limit"
                                };
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from(format!(
                                            r#"{{"code":{error_code},"msg":"{message}"}}"#
                                        ))))
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
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("update card json");
                            let data = body["card"]["data"].as_str().unwrap_or_default();
                            card_update_payloads
                                .lock()
                                .expect("update payloads lock")
                                .push(data.to_string());
                            if let Some(error_code) = fail_card_update_codes.get(&idx) {
                                let message = if *error_code == 200860 {
                                    "Card content exceeds limit"
                                } else {
                                    "element exceeds the limit"
                                };
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(Bytes::from(format!(
                                            r#"{{"code":{error_code},"msg":"{message}"}}"#
                                        ))))
                                        .unwrap(),
                                );
                            }
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
                        if method == Method::POST
                            && (path == "/open-apis/im/v1/messages"
                                || (path.starts_with("/open-apis/im/v1/messages/")
                                    && path.ends_with("/reply")))
                        {
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("send card json");
                            message_paths
                                .lock()
                                .expect("message paths lock")
                                .push(path.clone());
                            message_payloads
                                .lock()
                                .expect("message payloads lock")
                                .push(body.clone());
                            let content = body["content"].as_str().unwrap_or_default();
                            assert!(!content.is_empty());
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
        message_paths,
        message_payloads,
    }
}

pub(crate) fn mock_feishu_provider(base_url: &str) -> ImProviderConfig {
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

pub(crate) fn mock_progress_target() -> ImTarget {
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
async fn initial_content_size_limit_retries_creation_with_reduced_budget() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_card_create_limit(Some(1), 200860).await;
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
        .expect("retry initial progress card creation");

    let session = session.lock().await;
    assert_eq!(
        session.message_info().expect("message info").card_id,
        "card_2"
    );
    assert_eq!(session.card_budget, FEISHU_CARD_RETRY_BUDGET);
    assert!(!session.compact_card_mode);
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    let create_payloads = server
        .card_create_payloads
        .lock()
        .expect("create payloads")
        .clone();
    assert_eq!(create_payloads.len(), 2);
    assert!(create_payloads
        .iter()
        .all(|payload| !payload.contains("精简状态卡")));
}

#[tokio::test(flavor = "current_thread")]
async fn initial_limits_fall_back_to_compact_card_before_sending_message() {
    use std::sync::atomic::Ordering;

    let server =
        spawn_mock_feishu_progress_server_with_card_create_codes(vec![(1, 200860), (2, 300305)])
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
        .expect("compact initial card should recover from both limits");

    let session = session.lock().await;
    assert!(session.compact_card_mode);
    assert_eq!(
        session.message_info().expect("message info").card_id,
        "card_3"
    );
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    let payloads = server
        .card_create_payloads
        .lock()
        .expect("create payloads")
        .clone();
    assert_eq!(payloads.len(), 3);
    assert!(payloads[2].contains("精简状态卡"));
}

#[tokio::test(flavor = "current_thread")]
async fn initial_reduced_card_non_limit_error_is_not_hidden() {
    use std::sync::atomic::Ordering;

    let server =
        spawn_mock_feishu_progress_server_with_card_create_codes(vec![(1, 200860), (2, 10002)])
            .await;
    let registry = ImAgentProgressRegistry::new();
    let result = registry
        .start_feishu(
            "s1",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
        )
        .await;

    let error = match result {
        Ok(_) => panic!("ordinary create error must be returned"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("code=10002"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn already_compact_reduced_initial_card_does_not_retry_forever() {
    use std::sync::atomic::Ordering;

    let server =
        spawn_mock_feishu_progress_server_with_card_create_codes(vec![(2, 200860), (3, 300305)])
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
        .expect("start normal progress card");

    let mut session = session.lock().await;
    session.snapshot.output = "large final output".repeat(10_000);
    session.snapshot.phase = ImProgressPhase::Finished;
    let result = session.create_initial_card_entity().await;

    assert!(result.is_err());
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn failed_turn_rollover_restores_previous_card_state() {
    let server = spawn_mock_feishu_progress_server_with_card_create_codes(vec![(2, 10002)]).await;
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
        .expect("start first card");

    let mut session = session.lock().await;
    let previous = session.message_info().expect("previous message");
    let error = session
        .rollover_turn("second turn")
        .await
        .expect_err("ordinary create failure must abort rollover");

    assert!(error.to_string().contains("code=10002"));
    let restored = session.message_info().expect("restored message");
    assert_eq!(restored.card_id, previous.card_id);
    assert_eq!(restored.message_id, previous.message_id);
    assert_eq!(session.snapshot().title.as_deref(), Some("first turn"));
    assert!(!session.compact_card_mode);
    assert_eq!(session.card_budget, FEISHU_CARD_STANDARD_BUDGET);
}

#[tokio::test(flavor = "current_thread")]
async fn proactive_progress_card_uses_direct_send_without_reply_message_id() {
    let server = spawn_mock_feishu_progress_server().await;
    let registry = ImAgentProgressRegistry::new();
    registry
        .start_feishu(
            "web-bound-session",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "message from WebUI",
        )
        .await
        .expect("start proactive progress card");

    assert_eq!(
        server
            .message_paths
            .lock()
            .expect("message paths")
            .as_slice(),
        ["/open-apis/im/v1/messages"]
    );
    let payloads = server.message_payloads.lock().expect("message payloads");
    assert_eq!(payloads[0]["receive_id"], "ou_owner");
    assert_eq!(payloads[0]["msg_type"], "interactive");
}

#[tokio::test(flavor = "current_thread")]
async fn local_budget_can_switch_existing_card_to_compact_mode() {
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

    let mut session = session.lock().await;
    session.snapshot.output = "large final output".repeat(10_000);
    session.snapshot.phase = ImProgressPhase::Finished;
    session
        .flush_snapshot()
        .await
        .expect("compact local update");

    assert!(session.compact_card_mode);
    let payloads = server
        .card_update_payloads
        .lock()
        .expect("update payloads")
        .clone();
    assert_eq!(payloads.len(), 1);
    assert!(payloads[0].contains("精简状态卡"));
}

#[tokio::test(flavor = "current_thread")]
async fn limit_recovery_requires_an_existing_card_handle() {
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

    let mut session = session.lock().await;
    session.handle = None;
    let error = session
        .replace_current_card_after_limit(false)
        .await
        .expect_err("missing handle must not be synthesized");

    assert!(error.to_string().contains("handle missing"));
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
                    files: Vec::new(),
                    context: None
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
async fn component_limit_retries_same_card_with_reduced_budget() {
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
    assert_eq!(message_info.card_id, "card_1");
    assert_eq!(message_info.message_id.as_deref(), Some("om_1"));
    assert_eq!(session.snapshot().title.as_deref(), Some("Investigate"));
    assert_eq!(session.snapshot().plan_steps.len(), 1);
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn content_size_limit_retries_same_card_with_less_old_history() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_card_update_limit(Some(1), 200860).await;
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
        for index in 0..30 {
            session
                .snapshot
                .apply_event(AgentTurnProgressEvent::AssistantFinal {
                    content: format!("THINKING_HISTORY_{index}_{}", "思考".repeat(100)),
                });
            session
                .snapshot
                .apply_event(AgentTurnProgressEvent::ToolFinished {
                    log: ToolCallLog {
                        tool_name: "exec_command".to_string(),
                        arguments: format!(r#"{{"cmd":"history-{index}"}}"#),
                        result: format!("HISTORY_MARKER_{index}\n{}", "x".repeat(2600)),
                        success: true,
                    },
                    duration_ms: index + 1,
                });
        }
    }

    registry
        .apply_event(
            "s1",
            AgentTurnProgressEvent::ToolFinished {
                log: ToolCallLog {
                    tool_name: "exec_command".to_string(),
                    arguments: r#"{"cmd":"latest"}"#.to_string(),
                    result: format!("LATEST_HISTORY_MARKER\n{}", "y".repeat(2600)),
                    success: true,
                },
                duration_ms: 99,
            },
        )
        .await;

    let session = session.lock().await;
    assert_eq!(
        session.message_info().expect("message info").card_id,
        "card_1"
    );
    assert!(!session.compact_card_mode);
    assert_eq!(session.card_budget, FEISHU_CARD_RETRY_BUDGET);
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
    let payloads = server
        .card_update_payloads
        .lock()
        .expect("update payloads")
        .clone();
    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].contains("LATEST_HISTORY_MARKER"));
    assert!(payloads[1].contains("LATEST_HISTORY_MARKER"));
    assert!(payloads[1].contains("已省略前面"));
    assert!(payloads[1].len() < payloads[0].len());
    assert!((0..30).any(|index| {
        let marker = format!("THINKING_HISTORY_{index}");
        payloads[0].contains(&marker) && !payloads[1].contains(&marker)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn progress_event_uses_compact_card_after_two_limit_rejections() {
    use std::sync::atomic::Ordering;

    const OVERSIZED_MARKER: &str = "OVERSIZED_TOOL_OUTPUT_MARKER";

    let server =
        spawn_mock_feishu_progress_server_with_card_update_limits(vec![1, 2], 300305).await;
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
    assert_eq!(message_info.card_id, "card_1");
    assert_eq!(message_info.message_id.as_deref(), Some("om_1"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 0);
    assert_eq!(server.recall_counter.load(Ordering::SeqCst), 0);

    let update_payloads = server
        .card_update_payloads
        .lock()
        .expect("update payloads")
        .clone();
    assert_eq!(update_payloads.len(), 3);
    assert!(
        update_payloads[0].contains(OVERSIZED_MARKER),
        "first full update must carry the process detail before fallback"
    );
    assert!(update_payloads[2].contains("精简状态卡"));
    assert!(update_payloads[2].contains("Agent 仍在运行"));
    assert!(
        !update_payloads[2].contains(OVERSIZED_MARKER),
        "compact fallback must drop oversized process details"
    );
    assert!(!update_payloads[2].contains(PROCESS_PANEL_ELEMENT_ID));

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
        "card_1"
    );
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 4);
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
async fn progress_event_rolls_over_only_after_compact_card_limit_rejection() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_card_update_codes(vec![
        (1, 300305),
        (2, 300305),
        (3, 300305),
    ])
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
                    arguments: r#"{"cmd":"large"}"#.to_string(),
                    result: "x".repeat(20_000),
                    success: true,
                },
                duration_ms: 42,
            },
        )
        .await;

    let session = session.lock().await;
    assert_eq!(
        session.message_info().expect("message info").card_id,
        "card_2"
    );
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 4);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn compact_card_non_limit_failure_does_not_create_duplicate_message() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_card_update_codes(vec![
        (1, 300305),
        (2, 300305),
        (3, 10002),
    ])
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
                    arguments: r#"{"cmd":"large"}"#.to_string(),
                    result: "x".repeat(20_000),
                    success: true,
                },
                duration_ms: 42,
            },
        )
        .await;

    let session = session.lock().await;
    assert_eq!(
        session.message_info().expect("message info").card_id,
        "card_1"
    );
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 3);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn reduced_card_non_limit_failure_stops_before_compact_retry() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_failures(
        None,
        vec![(1, 300305), (2, 10002)],
        Vec::new(),
        None,
    )
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

    let mut session = session.lock().await;
    session
        .snapshot
        .apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "exec_command".to_string(),
                arguments: r#"{"cmd":"large"}"#.to_string(),
                result: "x".repeat(20_000),
                success: true,
            },
            duration_ms: 42,
        });
    let error = session
        .flush_snapshot_with_limit_rollover("freeze")
        .await
        .expect_err("ordinary reduced-card failure must be returned");

    assert!(error.to_string().contains("code=10002"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn rollover_creation_failure_preserves_the_full_limit_error_chain() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_failures(
        None,
        vec![(1, 300305), (2, 300305), (3, 300305)],
        vec![(2, 10002)],
        None,
    )
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

    let mut session = session.lock().await;
    session
        .snapshot
        .apply_event(AgentTurnProgressEvent::ToolFinished {
            log: ToolCallLog {
                tool_name: "exec_command".to_string(),
                arguments: r#"{"cmd":"large"}"#.to_string(),
                result: "x".repeat(20_000),
                success: true,
            },
            duration_ms: 42,
        });
    let error = session
        .flush_snapshot_with_limit_rollover("freeze")
        .await
        .expect_err("failed rollover creation must preserve recovery context");
    let message = error.to_string();

    assert!(message.contains("progress card limit recovery failed"));
    assert!(message.contains("initial="));
    assert!(message.contains("reduced="));
    assert!(message.contains("compact="));
    assert!(message.contains("rollover failed"));
    assert!(message.contains("code=10002"));
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn finish_retries_same_card_when_final_update_hits_limit() {
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
    assert_eq!(message_info.card_id, "card_1");
    assert_eq!(message_info.message_id.as_deref(), Some("om_1"));
    assert_eq!(
        session
            .message_info()
            .expect("session message info")
            .card_id,
        "card_1"
    );
    assert_eq!(session.snapshot().phase, ImProgressPhase::Finished);
    assert_eq!(session.snapshot().output, "最终结论：继续在新卡片显示。");
    assert_eq!(server.card_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.message_counter.load(Ordering::SeqCst), 1);
    assert_eq!(server.card_update_counter.load(Ordering::SeqCst), 2);
    assert_eq!(server.settings_update_counter.load(Ordering::SeqCst), 1);
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
                    files: Vec::new(),
                    context: None
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
                    files: Vec::new(),
                    context: None
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
                    files: Vec::new(),
                    context: None
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
async fn feishu_message_cards_use_native_reply_paths_and_strip_root_headers() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server().await;
    let feishu = Arc::new(FeishuProvider::new());
    let provider = mock_feishu_provider(&server.base_url);
    feishu
        .reply_card(
            &provider,
            "om_trigger_plain",
            serde_json::json!({
                "schema": "2.0",
                "header": { "title": { "tag": "plain_text", "content": "remove me" } },
                "body": { "elements": [{ "tag": "markdown", "content": "hello" }] }
            }),
            Some("reply_plain_uuid"),
        )
        .await
        .expect("reply with ordinary card");
    feishu
        .send_card(
            &provider,
            &mock_progress_target(),
            serde_json::json!({
                "schema": "2.0",
                "header": { "title": { "tag": "plain_text", "content": "remove me too" } },
                "body": { "elements": [{ "tag": "markdown", "content": "proactive" }] }
            }),
            super::super::types::SendOptions::default(),
        )
        .await
        .expect("send proactive ordinary card");

    let registry = ImAgentProgressRegistry::new();
    registry
        .start_feishu_replying_to(
            "s-reply",
            Arc::clone(&feishu),
            provider,
            mock_progress_target(),
            "first turn",
            Some("om_trigger_progress_1"),
        )
        .await
        .expect("start replied progress card");
    assert!(
        registry
            .rollover_existing_replying_to("s-reply", "second turn", Some("om_trigger_progress_2"),)
            .await
    );

    assert_eq!(server.message_counter.load(Ordering::SeqCst), 4);
    let paths = server.message_paths.lock().expect("message paths").clone();
    assert_eq!(
        paths,
        vec![
            "/open-apis/im/v1/messages/om_trigger_plain/reply",
            "/open-apis/im/v1/messages",
            "/open-apis/im/v1/messages/om_trigger_progress_1/reply",
            "/open-apis/im/v1/messages/om_trigger_progress_2/reply",
        ]
    );

    let payloads = server
        .message_payloads
        .lock()
        .expect("message payloads")
        .clone();
    assert_eq!(payloads[0]["msg_type"], "interactive");
    assert_eq!(payloads[0]["uuid"], "reply_plain_uuid");
    assert!(payloads[0].get("receive_id").is_none());
    let ordinary_card: serde_json::Value = serde_json::from_str(
        payloads[0]["content"]
            .as_str()
            .expect("ordinary card content"),
    )
    .expect("ordinary card json");
    assert!(ordinary_card.get("header").is_none());
    let proactive_card: serde_json::Value = serde_json::from_str(
        payloads[1]["content"]
            .as_str()
            .expect("proactive card content"),
    )
    .expect("proactive card json");
    assert!(proactive_card.get("header").is_none());

    let created_cards = server
        .card_create_payloads
        .lock()
        .expect("created cards")
        .clone();
    assert_eq!(created_cards.len(), 2);
    for created_card in created_cards {
        let card: serde_json::Value =
            serde_json::from_str(&created_card).expect("created progress card json");
        assert!(card.get("header").is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn progress_card_falls_back_to_direct_send_when_native_reply_fails() {
    use std::sync::atomic::Ordering;

    let server = spawn_mock_feishu_progress_server_with_send_failure(Some(1)).await;
    let registry = ImAgentProgressRegistry::new();
    let session = registry
        .start_feishu_replying_to(
            "s-reply-fallback",
            Arc::new(FeishuProvider::new()),
            mock_feishu_provider(&server.base_url),
            mock_progress_target(),
            "first turn",
            Some("om_trigger"),
        )
        .await
        .expect("fallback direct progress-card send");

    assert_eq!(server.message_counter.load(Ordering::SeqCst), 2);
    assert_eq!(
        server
            .message_paths
            .lock()
            .expect("message paths")
            .as_slice(),
        [
            "/open-apis/im/v1/messages/om_trigger/reply",
            "/open-apis/im/v1/messages",
        ]
    );
    assert_eq!(
        session
            .lock()
            .await
            .message_info()
            .and_then(|info| info.message_id),
        Some("om_2".to_string())
    );
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
