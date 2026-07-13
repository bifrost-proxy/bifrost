//! Focused E2E coverage for IM Gateway progress-card rendering.

use crate::TestCase;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![TestCase::standalone(
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
    )]
}
