//! Bifrost Agent 的长期记忆系统核心。
//!
//! 该 crate 只提供可复用的存储、脱敏、召回和抽取抽象，不依赖
//! `bifrost-agent` 或 `bifrost-admin`，从而保证记忆系统可以被
//! CLI、Admin API、IM Gateway 和未来同步层共同消费。

pub mod extract;
pub mod recall;
pub mod redact;
pub mod schema;
pub mod store;
pub mod types;

pub use extract::{
    ConsolidationReport, ConsolidationRequest, ExtractRequest, LlmMemoryExtractor, MemoryCandidate,
    MemoryConsolidator, MemoryExtractor, NoopMemoryConsolidator,
};
pub use recall::{DefaultMemoryRecaller, MemoryRecaller, RecallContext};
pub use redact::Redactor;
pub use store::{GcPolicy, GcReport, ImportReport, MemoryStore, SqliteMemoryStore};
pub use types::{
    MemoryId, MemoryKind, MemoryPatch, MemoryRecord, MemoryScope, MemorySearchQuery, MemorySource,
    MemoryStats, NewMemoryRecord,
};
