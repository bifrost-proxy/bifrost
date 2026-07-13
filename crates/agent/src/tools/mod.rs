//! Shared goal and plan state used by external-runner progress surfaces.

pub mod goal;
pub mod update_plan;

use crate::types::{ToolDefinition, ToolResult};
use async_trait::async_trait;
use std::path::Path;

/// Minimal handler contract retained for typed plan events.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult;

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            self.name().to_string(),
            self.description().to_string(),
            Some(self.parameters_schema()),
        )
    }
}
