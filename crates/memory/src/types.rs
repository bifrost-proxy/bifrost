use serde::{Deserialize, Serialize};
use std::fmt;

/// 长期记忆记录的稳定 ID，使用 UUIDv7 以便按创建时间排序。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(String);

impl MemoryId {
    /// 生成新的 UUIDv7 记忆 ID。
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }

    /// 从已存在字符串构造记忆 ID。
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 返回底层字符串。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// 记忆作用域。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum MemoryScope {
    /// 全局记忆。
    Global,
    /// 用户级记忆。
    User(String),
    /// 项目级记忆，值为 path hash。
    Project(String),
    /// session 级记忆。
    Session(String),
}

impl MemoryScope {
    /// 返回 scope 类型字符串。
    pub fn scope_type(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::User(_) => "user",
            Self::Project(_) => "project",
            Self::Session(_) => "session",
        }
    }

    /// 返回 scope 值。
    pub fn scope_value(&self) -> Option<&str> {
        match self {
            Self::Global => None,
            Self::User(value) | Self::Project(value) | Self::Session(value) => Some(value),
        }
    }

    /// 返回用于索引的稳定 scope key。
    pub fn scope_kind(&self) -> String {
        match self {
            Self::Global => "global:*".to_string(),
            Self::User(value) => format!("user:{value}"),
            Self::Project(value) => format!("project:{value}"),
            Self::Session(value) => format!("session:{value}"),
        }
    }

    /// 返回召回排序使用的具体程度。
    pub fn specificity(&self) -> u8 {
        match self {
            Self::Session(_) => 4,
            Self::Project(_) => 3,
            Self::User(_) => 2,
            Self::Global => 1,
        }
    }
}

/// 记忆类型。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Preference,
    Rule,
    Skill,
    TaskContext,
    Other,
}

impl MemoryKind {
    /// 返回数据库中的稳定字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Preference => "preference",
            Self::Rule => "rule",
            Self::Skill => "skill",
            Self::TaskContext => "task_context",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for MemoryKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fact" => Ok(Self::Fact),
            "preference" => Ok(Self::Preference),
            "rule" => Ok(Self::Rule),
            "skill" => Ok(Self::Skill),
            "task_context" => Ok(Self::TaskContext),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown memory kind: {other}")),
        }
    }
}

/// 记忆来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemorySource {
    /// 自动抽取。
    AutoExtract { session_key: String, turn: u64 },
    /// 用户显式创建。
    UserExplicit,
    /// JSONL 导入。
    Import,
    /// 初始种子数据。
    Seed,
}

/// 已入库的长期记忆记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub content: String,
    pub source: MemorySource,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub confidence: f32,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_used_at: Option<u64>,
    pub use_count: u32,
    pub expires_at: Option<u64>,
    pub dedupe_hash: String,
}

/// 新建记忆记录输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewMemoryRecord {
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub content: String,
    pub source: MemorySource,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub confidence: f32,
    pub expires_at: Option<u64>,
}

/// 更新记忆记录的 patch。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryPatch {
    pub scope: Option<MemoryScope>,
    pub kind: Option<MemoryKind>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub pinned: Option<bool>,
    pub confidence: Option<f32>,
    pub expires_at: Option<Option<u64>>,
}

/// 记忆搜索条件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchQuery {
    pub query: Option<String>,
    pub scopes: Vec<MemoryScope>,
    pub kind: Option<MemoryKind>,
    pub tag: Option<String>,
    pub include_deleted: bool,
    pub limit: usize,
    pub offset: usize,
}

/// 记忆统计信息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total: u64,
    pub by_scope: Vec<(String, u64)>,
    pub by_kind: Vec<(String, u64)>,
    pub written_last_7_days: u64,
    pub recalled_last_7_days: u64,
}
