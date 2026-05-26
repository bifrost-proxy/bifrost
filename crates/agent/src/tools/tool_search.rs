//! Deferred tool search.

use crate::tools::ToolHandler;
use crate::types::{ToolDefinition, ToolResult};
use async_trait::async_trait;
use bm25::{Document, Language, SearchEngine, SearchEngineBuilder};
use serde::Deserialize;
use std::path::Path;

pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";
pub const TOOL_SEARCH_DEFAULT_LIMIT: usize = 8;

#[derive(Debug, Clone)]
pub struct ToolSearchEntry {
    pub search_text: String,
    pub definition: ToolDefinition,
    pub source: String,
}

impl ToolSearchEntry {
    pub fn new(definition: ToolDefinition, source: impl Into<String>) -> Self {
        let source = source.into();
        let search_text = build_search_text(&definition, &source);
        Self {
            search_text,
            definition,
            source,
        }
    }
}

pub struct ToolSearchTool {
    entries: Vec<ToolSearchEntry>,
    search_engine: SearchEngine<usize>,
}

impl ToolSearchTool {
    pub fn new(entries: Vec<ToolSearchEntry>) -> Self {
        let documents = entries
            .iter()
            .map(|entry| entry.search_text.clone())
            .enumerate()
            .map(|(idx, search_text)| Document::new(idx, search_text))
            .collect::<Vec<_>>();
        let search_engine =
            SearchEngineBuilder::<usize>::with_documents(Language::English, documents).build();
        Self {
            entries,
            search_engine,
        }
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
        TOOL_SEARCH_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Searches over deferred tool metadata and exposes matching tools for the next model call."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for deferred tools."
                },
                "limit": {
                    "type": "integer",
                    "description": format!("Maximum number of tools to return (defaults to {TOOL_SEARCH_DEFAULT_LIMIT}).")
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: ToolSearchArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {error}"),
                    runtime_events: Vec::new(),
                };
            }
        };

        let query = args.query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return ToolResult {
                success: false,
                output: "tool_search query must not be empty".to_string(),
                runtime_events: Vec::new(),
            };
        }

        let limit = match args.limit {
            Some(0) => {
                return ToolResult {
                    success: false,
                    output: "limit must be greater than zero".to_string(),
                    runtime_events: Vec::new(),
                };
            }
            Some(limit) => limit.min(50),
            None => TOOL_SEARCH_DEFAULT_LIMIT,
        };
        let mut result_ids = self
            .search_engine
            .search(&query, limit)
            .into_iter()
            .map(|result| result.document.id)
            .collect::<Vec<_>>();
        if result_ids.is_empty() {
            result_ids = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.search_text.to_ascii_lowercase().contains(&query))
                .take(limit)
                .map(|(idx, _)| idx)
                .collect();
        }
        let tools = result_ids
            .into_iter()
            .filter_map(|id| self.entries.get(id))
            .map(|entry| entry.definition.clone())
            .collect::<Vec<_>>();
        let output = serde_json::json!({
            "tools": tools
        });
        ToolResult {
            success: true,
            output: output.to_string(),
            runtime_events: Vec::new(),
        }
    }
}

pub fn tool_search_definition(deferred_entries: &[ToolSearchEntry]) -> ToolDefinition {
    let mut sources = deferred_entries
        .iter()
        .map(|entry| entry.source.as_str())
        .filter(|source| !source.trim().is_empty())
        .collect::<Vec<_>>();
    sources.sort_unstable();
    sources.dedup();
    let source_descriptions = if sources.is_empty() {
        "None currently enabled.".to_string()
    } else {
        sources
            .into_iter()
            .map(|source| format!("- {source}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    ToolDefinition::function(
        TOOL_SEARCH_TOOL_NAME.to_string(),
        format!(
            "# Tool discovery\n\nSearches over deferred tool metadata and exposes matching tools for the next model call.\n\nYou have access to tools from the following sources:\n{source_descriptions}\nSome of the tools may not have been provided to you upfront, and you should use this tool (`{TOOL_SEARCH_TOOL_NAME}`) to search for the required tools."
        ),
        Some(ToolSearchTool::new(Vec::new()).parameters_schema()),
    )
}

pub fn parse_loadable_tool_definitions(output: &str) -> Vec<ToolDefinition> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|value| value.get("tools").cloned())
        .and_then(|tools| serde_json::from_value::<Vec<ToolDefinition>>(tools).ok())
        .unwrap_or_default()
}

fn build_search_text(definition: &ToolDefinition, source: &str) -> String {
    let mut parts = vec![definition.name().to_string(), source.to_string()];
    if let Some(function) = definition.function.as_ref() {
        if !function.description.trim().is_empty() {
            parts.push(function.description.clone());
        }
        if let Some(parameters) = function.parameters.as_ref() {
            parts.push(parameters.to_string());
        }
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tool_search_returns_loadable_deferred_tool_definitions() {
        let definition = ToolDefinition::function(
            "mcp_server__exec_command".to_string(),
            "Runs a command".to_string(),
            Some(serde_json::json!({"type":"object"})),
        );
        let tool = ToolSearchTool::new(vec![ToolSearchEntry::new(
            definition.clone(),
            "MCP server: test",
        )]);
        let result = tool
            .execute(r#"{"query":"exec","limit":3}"#, Path::new("/tmp"))
            .await;
        assert!(result.success);
        assert!(result.output.contains("exec_command"));
        let tools = parse_loadable_tool_definitions(&result.output);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), definition.name());
    }

    #[tokio::test]
    async fn tool_search_rejects_zero_limit() {
        let tool = ToolSearchTool::new(Vec::new());
        let result = tool
            .execute(r#"{"query":"exec","limit":0}"#, Path::new("/tmp"))
            .await;
        assert!(!result.success);
        assert!(result.output.contains("greater than zero"));
    }
}
