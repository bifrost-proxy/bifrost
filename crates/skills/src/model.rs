use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkillManifest {
    /// kebab-case, ^[a-z][a-z0-9-]{0,63}$
    pub name: String,
    /// semver
    pub version: String,
    /// <= 1024 chars; description matching trigger
    pub description: String,
    pub scope: SkillScope,
    pub entrypoint: Entrypoint,
    /// Tools this skill may use.
    pub allowed_tools: Vec<ToolBinding>,
    /// Optional "/xxx" slash command; cannot conflict with built-ins.
    pub slash_command: Option<String>,
    pub triggers: Vec<TriggerRule>,
    pub inputs_schema: Option<serde_json::Value>,
    pub outputs_schema: Option<serde_json::Value>,
    pub metadata: BTreeMap<String, String>,
    pub created_by: SkillAuthor,
    pub created_at_unix: i64,
    pub updated_at_unix: i64,
    /// Stable sha256 of the skill directory excluding manifest.json and .history.
    pub checksum: String,
    pub schema_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    /// Embedded system skills (lowest priority, overridable).
    System,
    /// Global cross-agent skills from `~/.agents/skills/`.
    Global,
    /// User-level skills from `~/.bifrost/agent/skills/`.
    User,
    /// Repository-level skills from `<work_dir>/.agents/skills/`.
    Repo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entrypoint {
    Inline {
        instructions_md: String,
    },
    Shell {
        script: PathBuf,
        shell: ShellKind,
    },
    Python {
        script: PathBuf,
        python: Option<String>,
    },
    Node {
        script: PathBuf,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Bash,
    Sh,
    Zsh,
    PowerShell,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolBinding {
    Registry {
        name: String,
    },
    Mcp {
        server: String,
        tool: String,
    },
    Memory {
        op: MemoryOp,
    },
    Owned {
        name: String,
        description: String,
        input_schema: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOp {
    Read,
    Write,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerRule {
    DescriptionMatch,
    Keyword { any_of: Vec<String> },
    Regex { pattern: String },
    SlashCommand,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillAuthor {
    User { id: String },
    Agent { session_id: String },
    Imported { origin: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkillRecord {
    pub manifest: SkillManifest,
    pub name: String,
    pub version: String,
    pub description: String,
    pub scope: SkillScope,
    pub effective_scope: SkillScope,
    pub shadow_scopes: Vec<SkillScope>,
    pub enabled: bool,
    pub path: PathBuf,
    pub skill_md_path: PathBuf,
    pub checksum: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeRoot {
    pub scope: SkillScope,
    pub path: PathBuf,
}

impl ScopeRoot {
    pub fn new(scope: SkillScope, path: impl Into<PathBuf>) -> Self {
        Self {
            scope,
            path: path.into(),
        }
    }
}

impl SkillScope {
    pub fn priority(&self) -> u8 {
        match self {
            SkillScope::System => 0,
            SkillScope::Global => 1,
            SkillScope::User => 2,
            SkillScope::Repo => 3,
        }
    }
}

impl SkillManifest {
    pub fn minimal_inline(name: &str, description: &str, scope: SkillScope) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            description: description.to_string(),
            scope,
            entrypoint: Entrypoint::Inline {
                instructions_md: String::new(),
            },
            allowed_tools: Vec::new(),
            slash_command: None,
            triggers: vec![TriggerRule::DescriptionMatch],
            inputs_schema: None,
            outputs_schema: None,
            metadata: BTreeMap::new(),
            created_by: SkillAuthor::Agent {
                session_id: "test".to_string(),
            },
            created_at_unix: now,
            updated_at_unix: now,
            checksum: String::new(),
            schema_version: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_with_tagged_enums() {
        let manifest = SkillManifest {
            slash_command: Some("/weather".to_string()),
            allowed_tools: vec![
                ToolBinding::Memory { op: MemoryOp::Read },
                ToolBinding::Owned {
                    name: "forecast".to_string(),
                    description: "lookup forecast".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
            triggers: vec![TriggerRule::SlashCommand],
            ..SkillManifest::minimal_inline("weather-lookup", "weather", SkillScope::Repo)
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("\"kind\":\"owned\""));
        let parsed: SkillManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, manifest);
    }
}
