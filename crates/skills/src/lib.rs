//! Skill creator subsystem.
//!
//! This crate owns skill manifests, disk persistence, validation, registry
//! indexing, execution, packaging, authoring sessions, and scoped tool bridge
//! primitives. It intentionally does not depend on `bifrost-agent`.

pub mod authoring;
pub mod executor;
pub mod model;
pub mod packager;
pub mod registry;
pub mod store;
pub mod tool_bridge;
pub mod validator;

pub use authoring::{AuthoringError, AuthoringState, SkillAuthoringSession};
pub use executor::{ExecutionEvent, SkillExecutor, SkillInvocation, SkillTestReport};
pub use model::{
    Entrypoint, MemoryOp, ScopeRoot, ShellKind, SkillAuthor, SkillManifest, SkillRecord,
    SkillScope, ToolBinding, TriggerRule,
};
pub use packager::SkillPackager;
pub use registry::{default_roots, system_skills_cache_dir, SkillRegistry};
pub use store::{SkillDraft, SkillStore};
pub use tool_bridge::{MemoryToolRequest, MemoryToolResponse, SkillToolBridge};
pub use validator::{SkillValidator, ValidationError, ValidationIssue, ValidationResult};
