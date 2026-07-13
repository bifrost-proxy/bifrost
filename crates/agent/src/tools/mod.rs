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

#[cfg(test)]
mod tests {
    use super::*;

    struct ExampleHandler;

    #[async_trait]
    impl ToolHandler for ExampleHandler {
        fn name(&self) -> &str {
            "example"
        }

        fn description(&self) -> &str {
            "example handler"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _arguments: &str, _work_dir: &Path) -> ToolResult {
            ToolResult {
                success: true,
                output: "ok".to_string(),
                runtime_events: Vec::new(),
            }
        }
    }

    #[test]
    fn handler_definition_uses_handler_metadata() {
        let definition = ExampleHandler.definition();
        let function = definition.function.expect("function definition");
        assert_eq!(function.name, "example");
        assert_eq!(function.description, "example handler");
        assert_eq!(
            function.parameters,
            Some(serde_json::json!({"type": "object"}))
        );
    }
}
