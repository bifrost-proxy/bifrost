//! Focused E2E coverage for IM Gateway progress-card rendering.

use crate::TestCase;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
        "im_gateway_file_change_progress_card_renderer",
        "Validate a completed File Change tool expands to file paths and line statistics instead of the empty-detail fallback",
        "admin",
        || async move {
            use bifrost_admin::im_gateway::progress_card::{
                build_feishu_progress_card, ImAgentProgressSnapshot,
            };

            let event = bifrost_admin::im_gateway::external_cli::parse_progress_events(
                r#"{"type":"item.completed","item":{"id":"item_file_1","type":"fileChange","status":"completed","changes":[{"path":"/workspace/project/target/demo.txt","kind":{"type":"add"},"diff":"first\nsecond\nthird\n"}]}}"#,
            )
            .pop()
            .ok_or_else(|| "file change event was not parsed".to_string())?;
            let agent_event =
                bifrost_admin::im_gateway::external_cli::external_progress_to_agent_turn_event(
                    "provider:owner",
                    "codex",
                    bifrost_admin::im_gateway::external_cli::ExternalCliProgressStatusContext::new(
                        Some("Codex"),
                        None,
                        None,
                        None,
                        None,
                        Some(std::path::Path::new("/workspace/project")),
                    ),
                    &event,
                )
                .ok_or_else(|| "file change event was not mapped".to_string())?;

            let mut snapshot =
                ImAgentProgressSnapshot::new("provider:owner", "render file change");
            snapshot.apply_event(agent_event);
            snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                content: "file updated".to_string(),
            });

            let card = build_feishu_progress_card(&snapshot, true);
            let body = card["body"]["elements"].to_string();
            for needle in ["文件变更", "target/demo.txt", "新增 3 行", "已执行 1 个步骤"] {
                if !body.contains(needle) {
                    return Err(format!("file change card body missing {needle}: {body}"));
                }
            }
            if body.contains("暂无工具详情") {
                return Err(format!(
                    "file change detail unexpectedly used empty fallback: {body}"
                ));
            }
            if body.contains("/workspace/project") {
                return Err(format!("file change card leaked workspace prefix: {body}"));
            }
            if !body.contains("  first\\n  second\\n  third") {
                return Err(format!("file change detail lines were not aligned: {body}"));
            }
            let process_element = card["body"]["elements"]
                .as_array()
                .and_then(|elements| {
                    elements
                        .iter()
                        .find(|element| element["element_id"] == "agent_process_panel")
                })
                .ok_or_else(|| "file change card missing process element".to_string())?;
            let tool_element = process_element["elements"]
                .as_array()
                .and_then(|elements| {
                    elements.iter().find(|element| {
                        element["element_id"]
                            .as_str()
                            .is_some_and(|id| id.starts_with("ap_t_"))
                            && element.to_string().contains("文件变更")
                    })
                })
                .ok_or_else(|| "file change card missing expandable tool element".to_string())?;
            if tool_element["tag"] != "collapsible_panel" || tool_element["expanded"] != false {
                return Err(format!(
                    "file change tool should be collapsed and expandable: {tool_element}"
                ));
            }
            Ok(())
        },
        ),
        TestCase::standalone(
        "im_gateway_progress_card_budget_and_codex_resources",
        "Validate a long progress card stays inside the local byte budget while its top status exposes Codex session tokens, weekly balance, and elapsed time",
        "admin",
        || async move {
            use bifrost_admin::im_gateway::progress_card::{
                build_feishu_progress_card, ImAgentProgressSnapshot, ProgressRunnerSummary,
                ProgressRunnerTokenUsage, ProgressRunnerWeeklyUsage,
            };

            let mut snapshot =
                ImAgentProgressSnapshot::new("provider:owner", "render a long Codex task");
            snapshot.runner = Some(ProgressRunnerSummary {
                runner_id: "codex".to_string(),
                adapter: "codex".to_string(),
                model: Some("gpt-test".to_string()),
                model_source: Some("runner config".to_string()),
                reasoning_effort: None,
                reasoning_summary: None,
                reasoning_source: None,
                token_usage: Some(ProgressRunnerTokenUsage {
                    input_tokens: Some(10_000),
                    cached_input_tokens: Some(2_000),
                    output_tokens: Some(2_345),
                    reasoning_output_tokens: Some(345),
                    total_tokens: Some(12_345),
                }),
                weekly_usage: Some(ProgressRunnerWeeklyUsage {
                    used_percent: 63,
                    window_minutes: 10_080,
                    resets_at: Some(1_800_000_000),
                }),
                work_dir: Some("/workspace/project".to_string()),
                external_thread_id: Some("thread-resource-e2e".to_string()),
                external_conversation_id: None,
            });
            let mut status = bifrost_agent::ActiveTurnStatus::new("provider:owner");
            status.state = "running".to_string();
            status.runner_type = Some("codex".to_string());
            status.runner_id = Some("codex".to_string());
            status.started_at = 1_800_000_000;
            status.updated_at = 1_800_000_125;
            snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::Status(Box::new(status)));
            for index in 0..40 {
                snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::ToolFinished {
                    log: bifrost_agent::ToolCallLog {
                        tool_name: "exec_command".to_string(),
                        arguments: format!(
                            "MARKER_{index}_{}",
                            "input".repeat(220)
                        ),
                        result: format!(
                            "{}_{}",
                            if index == 39 { "LATEST_MARKER" } else { "OLD_MARKER" },
                            "output".repeat(520)
                        ),
                        success: true,
                    },
                    duration_ms: 10,
                });
            }

            let card = build_feishu_progress_card(&snapshot, true);
            let bytes = serde_json::to_vec(&card).map_err(|error| error.to_string())?;
            if bytes.len() > 24 * 1024 {
                return Err(format!("progress card exceeded 24KB budget: {} bytes", bytes.len()));
            }
            let serialized = String::from_utf8(bytes).map_err(|error| error.to_string())?;
            for needle in [
                "本次：12.3K Token",
                "周余额：37%",
                "耗时：2 分 05 秒",
                "Codex 周额度：剩余 37%",
                "LATEST_MARKER",
                "已省略前面",
            ] {
                if !serialized.contains(needle) {
                    return Err(format!("resource-aware card missing {needle}: {serialized}"));
                }
            }
            if serialized.contains("MARKER_0_") {
                return Err("oldest process record was not removed from the card view".to_string());
            }
            Ok(())
        },
        ),
    ]
}
