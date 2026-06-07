use super::*;
use bifrost_agent::PlanStepStatus;

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
    assert!(running_serialized.contains("**Loop 1**"));
    assert!(running_serialized.contains("**Loop 2**"));
    assert!(running_serialized.contains("我先看分支差异"));
    assert!(running_serialized.contains("接下来逐个模块检查"));
    assert!(!running_serialized.contains("最终结论"));

    snapshot.apply_event(AgentTurnProgressEvent::TurnFinished {
        content: "最终结论：未发现阻塞问题。".to_string(),
    });
    assert_eq!(snapshot.output, "最终结论：未发现阻塞问题。");
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
    assert!(running_serialized.contains("**Loop 1**"));
    assert!(running_serialized.contains("[模型]"));
    assert!(running_serialized.contains("工具摘要"));
    assert!(running_serialized.contains("正在运行 `exec_command`"));

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
    assert!(finished_serialized.contains("Pipeline 过程：1 轮 Loop · 已运行 1 条命令"));
    assert!(finished_serialized.contains("Loop 1 工具摘要：exec_command · 已运行"));
    assert!(finished_serialized.contains("test result: ok"));
    let process_elements = elements[1]["elements"].as_array().unwrap();
    assert_eq!(process_elements[1]["element_id"], "agent_process_tool_1");
    assert_eq!(process_elements[1]["expanded"], false);
    assert!(process_elements[1]["header"]["title"]["content"]
        .as_str()
        .unwrap()
        .contains("工具摘要"));
}

#[test]
fn external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(ProgressRunnerSummary {
        runner_id: "codex".to_string(),
        adapter: "codex".to_string(),
        model: Some("gpt-test".to_string()),
        model_source: Some("runner 配置".to_string()),
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
    });
    snapshot.guide_pending = true;
    snapshot.phase = ImProgressPhase::Finished;

    let title = format_status_panel_title(&snapshot);
    let footer = format_footer_markdown(&snapshot);

    assert!(title.contains("Runner：codex"));
    assert!(title.contains("模型：gpt-test（runner 配置）"));
    assert!(footer.contains("状态：已完成"));
    assert!(footer.contains("Runner：`codex` · Adapter：`codex`"));
    assert!(footer.contains("模型：gpt-test（runner 配置）"));
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
        agent_type: Some("Bifrost Agent".to_string()),
        runner_type: Some("bifrost_agent".to_string()),
        runner_id: None,
        model: Some("gpt-5".to_string()),
        model_provider: Some("openai".to_string()),
        external_conversation_id: None,
        external_thread_id: None,
        turn_timing: None,
        turn_id: None,
    }
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

fn compaction_progress(compaction_count: u32) -> bifrost_agent::AgentCompactionProgress {
    bifrost_agent::AgentCompactionProgress {
        trigger: "auto".to_string(),
        reason: "context_limit".to_string(),
        phase: "mid_turn".to_string(),
        pre_tokens: 4_000,
        post_tokens: Some(1_200),
        tokens_saved: Some(2_800),
        messages_removed: Some(5),
        duration_ms: Some(42),
        compaction_count,
        history_version: 8,
        context: context_snapshot(compaction_count),
    }
}

#[test]
fn progress_snapshot_updates_status_from_compaction_context() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "compact task");
    snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(active_status(0))));
    snapshot.apply_event(AgentTurnProgressEvent::CompactionFinished {
        progress: compaction_progress(2),
    });

    let status = snapshot.status.as_ref().expect("status should be kept");
    assert_eq!(status.compaction_count, 2);
    assert_eq!(status.history_version, 8);
    assert_eq!(status.estimated_context_tokens, 1_200);
    assert_eq!(status.last_response_tokens, Some(77));
    assert_eq!(status.total_tokens_used, Some(1_077));
    assert_eq!(snapshot.context.as_ref().unwrap().compaction_count, 2);

    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();
    assert!(serialized.contains("压缩：2 次"));
    assert!(serialized.contains("Token：累计 1.1K · 最近 77"));
    assert!(serialized.contains("Token：累计 1.1K，最近 77"));
    assert!(!serialized.contains("压缩：0 次"));
}

#[test]
fn internal_agent_status_panel_displays_model() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "model task");
    snapshot.apply_event(AgentTurnProgressEvent::Status(Box::new(active_status(0))));

    let title = format_status_panel_title(&snapshot);
    let footer = format_footer_markdown(&snapshot);

    assert!(title.contains("模型：gpt-5（openai）"));
    assert!(footer.contains("模型：gpt-5（openai）"));
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
fn feishu_progress_card_formats_large_token_usage() {
    let mut snapshot = ImAgentProgressSnapshot::new("s1", "token task");
    let mut status = active_status(1);
    status.last_response_tokens = Some(1_234_567);
    status.total_tokens_used = Some(1_000_000);
    status.estimated_context_tokens = 260_000;
    status.context_window_tokens = Some(1_000_000);
    status.context_usage_percent = Some(26.0);
    snapshot.status = Some(status);

    let card = build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).unwrap();
    assert!(serialized.contains("Token：累计 1M · 最近 1.2M"));
    assert!(serialized.contains("Context：~260K / 1M (26.0%)"));
    assert!(serialized.contains("Token：累计 1M，最近 1.2M"));
    assert!(!serialized.contains("Token：累计 1000000"));
    assert!(!serialized.contains("最近 1234567"));
    assert!(!serialized.contains("Context：~260000 / 1000000"));
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
        "agent_process_tool_1",
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
    assert!(process_content.contains("**Loop 1**"));
    assert!(process_content.contains("[模型]"));
    assert!(
        process_content.contains("Inspecting files before running tests."),
        "process content should show the latest thought: {process_content}"
    );
    let process_elements = process_element["elements"].as_array().unwrap();
    assert_eq!(process_elements[1]["element_id"], "agent_process_tool_1");
    assert_eq!(process_elements[1]["tag"], "collapsible_panel");
    assert_eq!(process_elements[1]["expanded"], false);
    assert!(process_elements[1]["header"]["title"]["content"]
        .as_str()
        .unwrap()
        .contains("Loop 1 工具摘要"));
    assert!(process_elements[1]["elements"][0]["content"]
        .as_str()
        .unwrap()
        .contains("tests passed"));
    assert!(process_elements[2]["content"]
        .as_str()
        .unwrap()
        .contains("**Loop 2**"));
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

struct MockFeishuProgressServer {
    base_url: String,
    card_counter: Arc<std::sync::atomic::AtomicUsize>,
    message_counter: Arc<std::sync::atomic::AtomicUsize>,
    recall_counter: Arc<std::sync::atomic::AtomicUsize>,
    card_update_counter: Arc<std::sync::atomic::AtomicUsize>,
    settings_update_counter: Arc<std::sync::atomic::AtomicUsize>,
}

async fn spawn_mock_feishu_progress_server() -> MockFeishuProgressServer {
    spawn_mock_feishu_progress_server_with_send_failure(None).await
}

async fn spawn_mock_feishu_progress_server_with_send_failure(
    fail_message_send_number: Option<usize>,
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock feishu server");
    let port = listener.local_addr().expect("mock local addr").port();
    let card_counter_for_server = Arc::clone(&card_counter);
    let message_counter_for_server = Arc::clone(&message_counter);
    let recall_counter_for_server = Arc::clone(&recall_counter);
    let card_update_counter_for_server = Arc::clone(&card_update_counter);
    let settings_update_counter_for_server = Arc::clone(&settings_update_counter);
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
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let card_counter = Arc::clone(&card_counter);
                    let message_counter = Arc::clone(&message_counter);
                    let recall_counter = Arc::clone(&recall_counter);
                    let card_update_counter = Arc::clone(&card_update_counter);
                    let settings_update_counter = Arc::clone(&settings_update_counter);
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
                            card_update_counter.fetch_add(1, Ordering::SeqCst);
                            let body: serde_json::Value =
                                serde_json::from_slice(&body).expect("update card json");
                            let data = body["card"]["data"].as_str().unwrap_or_default();
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
