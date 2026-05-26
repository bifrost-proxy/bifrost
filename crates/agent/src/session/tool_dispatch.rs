//! Tool dispatch helpers for the session turn loop.

use crate::authorization::ToolOrchestrator;
use crate::mcp::McpManager;
use crate::tools::{ToolHandler, ToolRegistry};
use crate::types::ToolResult;
use std::path::Path;
use std::sync::Arc;

pub(super) async fn run_local_handler(
    orchestrator: &ToolOrchestrator,
    name: &str,
    arguments: &str,
    work_dir: &Path,
    handler: Arc<dyn ToolHandler>,
) -> ToolResult {
    orchestrator
        .run_local(name, work_dir, || async {
            handler.execute(arguments, work_dir).await
        })
        .await
}

pub(super) async fn run_local_registry(
    orchestrator: &ToolOrchestrator,
    tools: &ToolRegistry,
    name: &str,
    arguments: &str,
    work_dir: &Path,
) -> ToolResult {
    orchestrator
        .run_local(name, work_dir, || async {
            tools.execute(name, arguments, work_dir).await
        })
        .await
}

pub(super) async fn run_mcp(
    orchestrator: &ToolOrchestrator,
    mcp: &mut McpManager,
    name: &str,
    arguments: &str,
) -> ToolResult {
    orchestrator
        .run_mcp(name, || async {
            match mcp.call_tool(name, arguments).await {
                Ok(result) => result,
                Err(error) => ToolResult {
                    success: false,
                    output: format!("MCP tool error: {error}"),
                    runtime_events: Vec::new(),
                },
            }
        })
        .await
}
