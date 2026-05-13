use crate::research::provider::{ResearchFetchRequest, ResearchSearchRequest};
use crate::research::store::{item_from_input, KnowledgeItem, KnowledgeItemInput};
use crate::research::ResearchRuntime;
use crate::session_status::{AgentTurnProgressEvent, AgentTurnProgressSender};
use crate::tools::ToolHandler;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

fn json_output<T: serde::Serialize>(value: &T) -> ToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(output) => ToolResult {
            success: true,
            output,
        },
        Err(error) => ToolResult {
            success: false,
            output: format!("serialize result: {error}"),
        },
    }
}

fn error_output(error: impl std::fmt::Display) -> ToolResult {
    ToolResult {
        success: false,
        output: error.to_string(),
    }
}

pub struct ResearchSearchTool {
    runtime: Arc<ResearchRuntime>,
}

impl ResearchSearchTool {
    pub fn new(runtime: Arc<ResearchRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl ToolHandler for ResearchSearchTool {
    fn name(&self) -> &str {
        "research_search"
    }

    fn description(&self) -> &str {
        "Search web and configured research providers. Results are grouped by provider completion event so fast providers can be consumed before slower sources finish."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "sources": {"type": "array", "items": {"type": "string", "enum": ["web", "wechat"]}},
                "provider_ids": {"type": "array", "items": {"type": "string"}},
                "freshness": {"type": "string", "enum": ["day", "week", "month", "year", "any"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50},
                "fetch_content": {"type": "boolean"},
                "language": {"type": "string"}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        self.execute_search(arguments, None).await
    }

    async fn execute_with_progress(
        &self,
        arguments: &str,
        _work_dir: &Path,
        progress_sender: Option<AgentTurnProgressSender>,
    ) -> ToolResult {
        self.execute_search(arguments, progress_sender).await
    }
}

impl ResearchSearchTool {
    async fn execute_search(
        &self,
        arguments: &str,
        progress_sender: Option<AgentTurnProgressSender>,
    ) -> ToolResult {
        let args: ResearchSearchRequest = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => return error_output(format!("invalid arguments: {error}")),
        };
        let mut rx = match self.runtime.clone().search_stream_channel(args).await {
            Ok(rx) => rx,
            Err(error) => return error_output(error),
        };
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            if let Some(sender) = &progress_sender {
                let message = provider_event_progress_message(&event);
                let _ = sender.send(AgentTurnProgressEvent::ToolProgress {
                    tool_name: self.name().to_string(),
                    message,
                });
            }
            events.push(event);
        }
        json_output(&events)
    }
}

fn provider_event_progress_message(
    event: &crate::research::provider::ResearchSearchProviderEvent,
) -> String {
    if let Some(error) = &event.error {
        return format!("provider={} failed: {}", event.provider_id, error);
    }
    let mut message = format!(
        "provider={} results={}",
        event.provider_id,
        event.results.len()
    );
    let titles = event
        .results
        .iter()
        .take(3)
        .map(|item| item.title.trim())
        .filter(|title| !title.is_empty())
        .collect::<Vec<_>>();
    if !titles.is_empty() {
        message.push_str(" titles=");
        message.push_str(&titles.join(" | "));
    }
    message
}

pub struct ResearchFetchTool {
    runtime: Arc<ResearchRuntime>,
}

impl ResearchFetchTool {
    pub fn new(runtime: Arc<ResearchRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl ToolHandler for ResearchFetchTool {
    fn name(&self) -> &str {
        "research_fetch"
    }

    fn description(&self) -> &str {
        "Fetch an article URL as markdown under the configured research fetch policy. Blocks localhost and private IPs by default."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string"},
                "format": {"type": "string", "enum": ["markdown"]},
                "max_bytes": {"type": "integer", "minimum": 1}
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: ResearchFetchRequest = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => return error_output(format!("invalid arguments: {error}")),
        };
        match self.runtime.fetch(args).await {
            Ok(response) => json_output(&response),
            Err(error) => error_output(error),
        }
    }
}

pub struct KnowledgeSaveTool {
    runtime: Arc<ResearchRuntime>,
}

impl KnowledgeSaveTool {
    pub fn new(runtime: Arc<ResearchRuntime>) -> Self {
        Self { runtime }
    }
}

#[derive(Deserialize)]
struct KnowledgeSaveArgs {
    items: Vec<KnowledgeSaveInput>,
}

#[derive(Deserialize)]
struct KnowledgeSaveInput {
    url: String,
    title: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    content_markdown: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[async_trait]
impl ToolHandler for KnowledgeSaveTool {
    fn name(&self) -> &str {
        "knowledge_save"
    }

    fn description(&self) -> &str {
        "Save research results or fetched documents into the local SQLite/FTS knowledge store."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "url": {"type": "string"},
                            "title": {"type": "string"},
                            "source": {"type": "string"},
                            "provider": {"type": "string"},
                            "query": {"type": "string"},
                            "author": {"type": "string"},
                            "published_at": {"type": "string"},
                            "content_markdown": {"type": "string"},
                            "summary": {"type": "string"},
                            "tags": {"type": "array", "items": {"type": "string"}}
                        },
                        "required": ["url", "title"]
                    }
                }
            },
            "required": ["items"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: KnowledgeSaveArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => return error_output(format!("invalid arguments: {error}")),
        };
        let items: Vec<KnowledgeItem> = args
            .items
            .into_iter()
            .map(|item| {
                item_from_input(KnowledgeItemInput {
                    source: item.source.unwrap_or_else(|| "manual".to_string()),
                    provider: item.provider.unwrap_or_else(|| "agent".to_string()),
                    query: item.query,
                    title: item.title,
                    url: item.url,
                    author: item.author,
                    published_at: item.published_at,
                    content_markdown: item.content_markdown,
                    summary: item.summary,
                    tags: item.tags,
                })
            })
            .collect();
        match self.runtime.save_knowledge(&items) {
            Ok(report) => json_output(&report),
            Err(error) => error_output(error),
        }
    }
}

pub struct KnowledgeSearchTool {
    runtime: Arc<ResearchRuntime>,
}

impl KnowledgeSearchTool {
    pub fn new(runtime: Arc<ResearchRuntime>) -> Self {
        Self { runtime }
    }
}

#[derive(Deserialize)]
struct KnowledgeSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    since_days: Option<u32>,
}

#[async_trait]
impl ToolHandler for KnowledgeSearchTool {
    fn name(&self) -> &str {
        "knowledge_search"
    }

    fn description(&self) -> &str {
        "Search the local Research Pack SQLite/FTS knowledge store."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50},
                "since_days": {"type": "integer", "minimum": 1}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: KnowledgeSearchArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => return error_output(format!("invalid arguments: {error}")),
        };
        match self
            .runtime
            .search_knowledge(&args.query, args.limit.unwrap_or(10), args.since_days)
        {
            Ok(results) => json_output(&serde_json::json!({ "results": results })),
            Err(error) => error_output(error),
        }
    }
}

pub struct ResearchDigestTool {
    runtime: Arc<ResearchRuntime>,
}

impl ResearchDigestTool {
    pub fn new(runtime: Arc<ResearchRuntime>) -> Self {
        Self { runtime }
    }
}

#[derive(Deserialize)]
struct ResearchDigestArgs {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    query: Option<String>,
}

#[async_trait]
impl ToolHandler for ResearchDigestTool {
    fn name(&self) -> &str {
        "research_digest"
    }

    fn description(&self) -> &str {
        "Generate a Markdown research digest from saved Research Pack knowledge items."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "date": {"type": "string"},
                "format": {"type": "string", "enum": ["markdown"]},
                "query": {"type": "string"}
            }
        })
    }

    async fn execute(&self, arguments: &str, _work_dir: &Path) -> ToolResult {
        let args: ResearchDigestArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(error) => return error_output(format!("invalid arguments: {error}")),
        };
        if args
            .format
            .as_deref()
            .is_some_and(|format| format != "markdown")
        {
            return error_output("only markdown digest format is supported");
        }
        match self.runtime.digest(args.task_id, args.date, args.query) {
            Ok(response) => json_output(&response),
            Err(error) => error_output(error),
        }
    }
}
