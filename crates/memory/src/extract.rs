use crate::types::{MemoryKind, MemoryScope};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 抽取模型输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractRequest {
    pub session_key: String,
    pub turn: u64,
    pub user_message: String,
    pub assistant_message: String,
    pub project_path: Option<String>,
    pub user_id: Option<String>,
    pub extract_model: Option<String>,
}

/// 抽取出的候选记忆。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryCandidate {
    pub content: String,
    pub kind: MemoryKind,
    pub tags: Vec<String>,
    pub confidence: f32,
    pub scope_hint: Option<MemoryScope>,
}

/// 长期记忆抽取器。
#[async_trait]
pub trait MemoryExtractor: Send + Sync {
    /// 从一次 turn 中抽取候选记忆。
    async fn extract(&self, request: ExtractRequest) -> Result<Vec<MemoryCandidate>, String>;
}

/// LLM 抽取器骨架。
///
/// 具体 agent 侧会把 prompt 和 `AgentClient::chat_completion` 适配进来；
/// 该类型负责持有 prompt 文本，避免 prompt 硬编码在 session loop。
#[derive(Debug, Clone)]
pub struct LlmMemoryExtractor {
    prompt: String,
}

impl LlmMemoryExtractor {
    /// 使用仓库内置 prompt 创建抽取器。
    pub fn new() -> Self {
        Self {
            prompt: include_str!("prompts/extract.md").to_string(),
        }
    }

    /// 返回抽取 prompt。
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// 解析模型返回的严格 JSON 数组。
    pub fn parse_candidates(&self, raw: &str) -> Result<Vec<MemoryCandidate>, String> {
        serde_json::from_str(raw).map_err(|error| format!("parse memory candidates: {error}"))
    }
}

impl Default for LlmMemoryExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryExtractor for LlmMemoryExtractor {
    async fn extract(&self, _request: ExtractRequest) -> Result<Vec<MemoryCandidate>, String> {
        Err("LlmMemoryExtractor must be connected to AgentClient by bifrost-agent".to_string())
    }
}

/// consolidation 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationRequest {
    pub model: Option<String>,
    pub max_items: usize,
}

/// consolidation 结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsolidationReport {
    pub processed: usize,
    pub merged: usize,
    pub skipped_reason: Option<String>,
}

/// 长期记忆合并器扩展点。
#[async_trait]
pub trait MemoryConsolidator: Send + Sync {
    /// 合并或压缩候选记忆。
    async fn consolidate(
        &self,
        request: ConsolidationRequest,
    ) -> Result<ConsolidationReport, String>;
}

/// 本期默认不执行真实 LLM consolidation，只消费配置并记录可观测结果。
pub struct NoopMemoryConsolidator;

#[async_trait]
impl MemoryConsolidator for NoopMemoryConsolidator {
    async fn consolidate(
        &self,
        request: ConsolidationRequest,
    ) -> Result<ConsolidationReport, String> {
        tracing::info!(
            target: "memory",
            model = request.model.as_deref().unwrap_or(""),
            max_items = request.max_items,
            "memory consolidation skipped in v1"
        );
        Ok(ConsolidationReport {
            processed: 0,
            merged: 0,
            skipped_reason: Some("consolidation is a v1 extension point".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_json_candidates() {
        let extractor = LlmMemoryExtractor::new();
        let parsed = extractor
            .parse_candidates(
                r#"[{"content":"用户偏好中文","kind":"preference","tags":["lang"],"confidence":0.9,"scope_hint":{"type":"user","value":"u1"}}]"#,
            )
            .expect("parse candidates");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, MemoryKind::Preference);
    }

    #[test]
    fn rejects_invalid_json_candidates() {
        let extractor = LlmMemoryExtractor::new();
        assert!(extractor.parse_candidates("not json").is_err());
    }
}
