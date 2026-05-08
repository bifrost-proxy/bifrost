use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleSetRef {
    LocalRule { name: String },
    GroupRule { group_id: String, name: String },
    RuleFile { path: String },
    InlineRule { content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryPortBindRequest {
    pub port: u16,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub rule_refs: Vec<RuleSetRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryPortUpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub rule_refs: Option<Vec<RuleSetRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryPortStatus {
    Running,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryPortBinding {
    pub port: u16,
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: TemporaryPortStatus,
    pub rule_refs: Vec<RuleSetRef>,
    #[serde(default)]
    pub missing_refs: Vec<RuleSetRef>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryPortRuleItem {
    pub name: String,
    pub rule_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryPortActiveSummary {
    pub port: u16,
    pub total: usize,
    pub rules: Vec<TemporaryPortRuleItem>,
    pub merged_content: String,
}

#[derive(Debug, Clone)]
pub struct TemporaryPortError {
    pub status: hyper::StatusCode,
    pub message: String,
}

impl TemporaryPortError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: hyper::StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: hyper::StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: hyper::StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: hyper::StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait TemporaryPortManager: Send + Sync {
    async fn bind(
        &self,
        req: TemporaryPortBindRequest,
    ) -> Result<TemporaryPortBinding, TemporaryPortError>;

    async fn update(
        &self,
        port: u16,
        req: TemporaryPortUpdateRequest,
    ) -> Result<TemporaryPortBinding, TemporaryPortError>;

    async fn destroy(&self, port: u16) -> Result<TemporaryPortBinding, TemporaryPortError>;

    async fn list(&self) -> Vec<TemporaryPortBinding>;

    async fn show(&self, port: u16) -> Result<TemporaryPortBinding, TemporaryPortError>;

    async fn active_summary(
        &self,
        port: u16,
    ) -> Result<TemporaryPortActiveSummary, TemporaryPortError>;
}

pub type SharedTemporaryPortManager = Arc<dyn TemporaryPortManager>;
