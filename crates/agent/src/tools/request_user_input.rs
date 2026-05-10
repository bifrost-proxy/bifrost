//! request_user_input schema and validation.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;

pub struct RequestUserInputTool;

#[derive(Debug, Deserialize)]
struct RequestUserInputArgs {
    questions: Vec<RequestUserInputQuestion>,
}

#[derive(Debug, Deserialize)]
struct RequestUserInputQuestion {
    id: String,
    header: String,
    question: String,
    options: Vec<RequestUserInputOption>,
}

#[derive(Debug, Deserialize)]
struct RequestUserInputOption {
    label: String,
    description: String,
}

#[async_trait]
impl ToolHandler for RequestUserInputTool {
    fn name(&self) -> &str {
        "request_user_input"
    }

    fn description(&self) -> &str {
        "Request user input for one to three short questions and wait for the response. Bifrost currently validates the request but has no interactive wait channel in this runtime."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to show the user. Prefer 1 and do not exceed 3",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable identifier for mapping answers (snake_case)."
                            },
                            "header": {
                                "type": "string",
                                "description": "Short header label shown in the UI (12 or fewer chars)."
                            },
                            "question": {
                                "type": "string",
                                "description": "Single-sentence prompt shown to the user."
                            },
                            "options": {
                                "type": "array",
                                "description": "Provide 2-3 mutually exclusive choices. Put the recommended option first and suffix its label with (Recommended).",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "User-facing label (1-5 words)."
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "One short sentence explaining impact/tradeoff if selected."
                                        }
                                    },
                                    "required": ["label", "description"]
                                }
                            }
                        },
                        "required": ["id", "header", "question", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: RequestUserInputArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {error}"),
                };
            }
        };

        if args.questions.is_empty() || args.questions.len() > 3 {
            return ToolResult {
                success: false,
                output: "request_user_input requires one to three questions".to_string(),
            };
        }

        for question in &args.questions {
            if !is_snake_case_id(&question.id) {
                return ToolResult {
                    success: false,
                    output: format!(
                        "request_user_input question id must be snake_case: {}",
                        question.id
                    ),
                };
            }
            if question.header.chars().count() > 12 {
                return ToolResult {
                    success: false,
                    output: format!(
                        "request_user_input header must be 12 or fewer characters: {}",
                        question.header
                    ),
                };
            }
            if question.question.trim().is_empty() {
                return ToolResult {
                    success: false,
                    output: format!("request_user_input question is empty: {}", question.id),
                };
            }
            if !(2..=3).contains(&question.options.len()) {
                return ToolResult {
                    success: false,
                    output: format!(
                        "request_user_input question {} requires 2-3 options",
                        question.id
                    ),
                };
            }
            for option in &question.options {
                if option.label.trim().is_empty() || option.description.trim().is_empty() {
                    return ToolResult {
                        success: false,
                        output: format!(
                            "request_user_input question {} has an empty option label or description",
                            question.id
                        ),
                    };
                }
            }
        }

        ToolResult {
            success: false,
            output: "request_user_input is unavailable in this Bifrost runtime: no interactive user-input channel is attached to the agent turn".to_string(),
        }
    }
}

fn is_snake_case_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !value.starts_with('_')
        && !value.ends_with('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_user_input_validates_questions() {
        let result = RequestUserInputTool
            .execute(r#"{"questions":[]}"#, Path::new("/tmp"))
            .await;
        assert!(!result.success);
        assert!(result.output.contains("one to three"));
    }

    #[tokio::test]
    async fn request_user_input_returns_unavailable_after_valid_request() {
        let result = RequestUserInputTool
            .execute(
                r#"{"questions":[{"id":"pick_mode","header":"Mode","question":"Choose mode?","options":[{"label":"A (Recommended)","description":"Use A."},{"label":"B","description":"Use B."}]}]}"#,
                Path::new("/tmp"),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("unavailable"));
    }
}
