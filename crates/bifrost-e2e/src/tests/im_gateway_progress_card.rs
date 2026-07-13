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

            let mut snapshot =
                ImAgentProgressSnapshot::new("provider:owner", "render file change");
            snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::ToolFinished {
                log: bifrost_agent::ToolCallLog {
                    tool_name: "fileChange".to_string(),
                    arguments: String::new(),
                    result: "changes:\n- file: src/main.rs (修改 1 行 · 新增 1 行)\n  @@ -1 +1,2 @@\n  -old\n  +new\n  +extra"
                        .to_string(),
                    success: true,
                },
                duration_ms: 0,
            });
            snapshot.apply_event(bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                content: "file updated".to_string(),
            });

            let card = build_feishu_progress_card(&snapshot, true);
            let body = card["body"]["elements"].to_string();
            for needle in ["fileChange", "src/main.rs", "修改 1 行 · 新增 1 行"] {
                if !body.contains(needle) {
                    return Err(format!("file change card body missing {needle}: {body}"));
                }
            }
            if body.contains("暂无工具详情") {
                return Err(format!(
                    "file change detail unexpectedly used empty fallback: {body}"
                ));
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
                            && element.to_string().contains("fileChange")
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
