//! Lightweight local tool search.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub struct ToolSearchTool {
    tools: Vec<ToolSummary>,
}

impl ToolSearchTool {
    pub fn new(tools: Vec<ToolSummary>) -> Self {
        Self { tools }
    }
}

#[derive(Debug, Deserialize)]
struct ToolSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search available Bifrost agent tools by name or description."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for local tools."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of tools to return."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: ToolSearchArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {error}"),
                };
            }
        };

        let query = args.query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return ToolResult {
                success: false,
                output: "tool_search query must not be empty".to_string(),
            };
        }

        let limit = args.limit.unwrap_or(8).clamp(1, 50);
        let mut scored = self
            .tools
            .iter()
            .filter_map(|tool| {
                let haystack = format!(
                    "{}\n{}",
                    tool.name.to_ascii_lowercase(),
                    tool.description.to_ascii_lowercase()
                );
                score_match(&haystack, &tool.name, &query).map(|score| (score, tool))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.name.cmp(&right.name))
        });

        let tools = scored
            .into_iter()
            .take(limit)
            .map(|(_, tool)| tool)
            .collect::<Vec<_>>();
        let output = serde_json::json!({
            "tools": tools
        });
        ToolResult {
            success: true,
            output: output.to_string(),
        }
    }
}

fn score_match(haystack: &str, name: &str, query: &str) -> Option<u32> {
    if name.eq_ignore_ascii_case(query) {
        Some(100)
    } else if name.to_ascii_lowercase().contains(query) {
        Some(75)
    } else if haystack.contains(query) {
        Some(50)
    } else {
        let terms = query.split_whitespace().collect::<Vec<_>>();
        (!terms.is_empty() && terms.iter().all(|term| haystack.contains(term))).then_some(25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tool_search_finds_registered_tool() {
        let tool = ToolSearchTool::new(vec![ToolSummary {
            name: "exec_command".to_string(),
            description: "Runs a command".to_string(),
            parameters: serde_json::json!({"type":"object"}),
        }]);
        let result = tool
            .execute(r#"{"query":"exec","limit":3}"#, Path::new("/tmp"))
            .await;
        assert!(result.success);
        assert!(result.output.contains("exec_command"));
    }
}
