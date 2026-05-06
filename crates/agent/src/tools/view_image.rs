//! Local image viewing tool compatible with Codex's `view_image` schema.

use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

pub struct ViewImageTool;

#[derive(Debug, Deserialize)]
struct ViewImageArgs {
    path: String,
    #[serde(default)]
    detail: Option<String>,
}

#[async_trait]
impl ToolHandler for ViewImageTool {
    fn name(&self) -> &str {
        "view_image"
    }

    fn description(&self) -> &str {
        "View a local image from the filesystem. Returns a data URL for model consumption."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local filesystem path to an image file"
                },
                "detail": {
                    "type": "string",
                    "description": "Optional detail override. The only supported value is `original`; omit this field for default resized behavior."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let args: ViewImageArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("invalid arguments: {error}"),
                };
            }
        };

        if let Some(detail) = args.detail.as_deref() {
            if detail != "original" {
                return ToolResult {
                    success: false,
                    output: format!(
                        "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `{detail}`"
                    ),
                };
            }
        }

        let path = resolve_path(&args.path, work_dir);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("failed to stat image: {error}"),
                };
            }
        };
        if !metadata.is_file() {
            return ToolResult {
                success: false,
                output: format!("path is not a file: {}", path.display()),
            };
        }
        if metadata.len() > MAX_IMAGE_BYTES {
            return ToolResult {
                success: false,
                output: format!(
                    "image is too large: {} bytes exceeds {} bytes",
                    metadata.len(),
                    MAX_IMAGE_BYTES
                ),
            };
        }

        let Some(mime) = mime_from_path(&path) else {
            return ToolResult {
                success: false,
                output: format!("unsupported image type: {}", path.display()),
            };
        };

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return ToolResult {
                    success: false,
                    output: format!("failed to read image: {error}"),
                };
            }
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let output = serde_json::json!({
            "image_url": format!("data:{mime};base64,{encoded}"),
            "detail": args.detail
        });
        ToolResult {
            success: true,
            output: output.to_string(),
        }
    }
}

fn resolve_path(path: &str, work_dir: &Path) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        work_dir.join(path)
    }
}

fn mime_from_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("svg") => Some("image/svg+xml"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn view_image_returns_data_url() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tiny.png");
        std::fs::write(
            &path,
            base64::engine::general_purpose::STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=")
                .unwrap(),
        )
        .unwrap();

        let result = ViewImageTool
            .execute(r#"{"path":"tiny.png","detail":"original"}"#, dir.path())
            .await;
        assert!(result.success, "{}", result.output);
        let value: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert!(value["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert_eq!(value["detail"], "original");
    }

    #[tokio::test]
    async fn view_image_rejects_missing_file() {
        let dir = tempdir().unwrap();
        let result = ViewImageTool
            .execute(r#"{"path":"missing.png"}"#, dir.path())
            .await;
        assert!(!result.success);
        assert!(result.output.contains("failed to stat image"));
    }
}
