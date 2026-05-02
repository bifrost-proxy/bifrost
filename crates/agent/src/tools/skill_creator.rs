use super::{ToolHandler, ToolRegistry};
use crate::config::agent_home_dir;
use crate::types::ToolResult;
use async_trait::async_trait;
use bifrost_skills::{
    default_roots, ScopeRoot, SkillDraft, SkillExecutor, SkillInvocation, SkillManifest,
    SkillPackager, SkillRegistry, SkillScope, SkillStore, SkillValidator,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TOOLS: &[(&str, &str)] = &[
    (
        "skill_creator.start",
        "Start a skill authoring session from a short brief.",
    ),
    (
        "skill_creator.interview",
        "Capture or refine skill authoring answers.",
    ),
    (
        "skill_creator.draft",
        "Draft a SKILL.md and manifest for a new skill.",
    ),
    (
        "skill_creator.test",
        "Run a dry skill execution with JSON inputs.",
    ),
    (
        "skill_creator.commit",
        "Commit a drafted skill to the skill store.",
    ),
    ("skill_creator.cancel", "Cancel a skill authoring session."),
    (
        "skill_creator.list_templates",
        "List built-in skill templates.",
    ),
    (
        "skill_creator.import",
        "Import a .skill zip archive from disk.",
    ),
];

pub fn register_skill_creator_tools(registry: &mut ToolRegistry) {
    for (name, description) in TOOLS {
        registry.register(Arc::new(SkillCreatorTool {
            name: (*name).to_string(),
            description: (*description).to_string(),
        }));
    }
}

struct SkillCreatorTool {
    name: String,
    description: String,
}

#[async_trait]
impl ToolHandler for SkillCreatorTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        match self.name.as_str() {
            "skill_creator.start" => serde_json::json!({
                "type": "object",
                "properties": { "brief": { "type": "string" } },
                "required": ["brief"]
            }),
            "skill_creator.interview" => serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "answers": { "type": "object" }
                },
                "required": ["session_id", "answers"]
            }),
            "skill_creator.draft" => serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "overrides": { "type": "object" }
                },
                "required": ["session_id"]
            }),
            "skill_creator.test" => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "scope": { "type": "string", "enum": ["global", "user", "project"] },
                    "inputs": { "type": "object" },
                    "timeout_ms": { "type": "number" }
                },
                "required": ["name"]
            }),
            "skill_creator.commit" => serde_json::json!({
                "type": "object",
                "properties": {
                    "manifest": { "type": "object" },
                    "skill_md": { "type": "string" },
                    "assets": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "content": { "type": "string" }
                            },
                            "required": ["path", "content"]
                        }
                    }
                },
                "required": ["manifest", "skill_md"]
            }),
            "skill_creator.import" => serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            _ => serde_json::json!({"type": "object"}),
        }
    }

    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult {
        let result = match self.name.as_str() {
            "skill_creator.start" => start(arguments),
            "skill_creator.interview" => interview(arguments),
            "skill_creator.draft" => draft(arguments),
            "skill_creator.test" => test(arguments, work_dir).await,
            "skill_creator.commit" => commit(arguments, work_dir),
            "skill_creator.cancel" => cancel(arguments),
            "skill_creator.list_templates" => list_templates(),
            "skill_creator.import" => import(arguments, work_dir),
            _ => Err(format!("unknown skill_creator tool: {}", self.name)),
        };
        match result {
            Ok(value) => ToolResult {
                success: true,
                output: value.to_string(),
            },
            Err(error) => ToolResult {
                success: false,
                output: error,
            },
        }
    }
}

#[derive(Deserialize)]
struct StartArgs {
    brief: String,
}

fn start(arguments: &str) -> Result<serde_json::Value, String> {
    let args: StartArgs = parse(arguments)?;
    Ok(serde_json::json!({
        "session_id": uuid::Uuid::new_v4().to_string(),
        "brief": args.brief,
        "next": "interview"
    }))
}

fn interview(arguments: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = parse(arguments)?;
    Ok(serde_json::json!({
        "next": "draft",
        "captured": value
    }))
}

fn draft(arguments: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = parse(arguments)?;
    Ok(serde_json::json!({
        "skill_md": "---\nname: draft-skill\nversion: 0.1.0\n---\n# Draft Skill\n",
        "manifest": value.get("overrides").cloned().unwrap_or_else(|| serde_json::json!({})),
        "next": "test"
    }))
}

#[derive(Deserialize)]
struct TestArgs {
    name: String,
    scope: Option<SkillScope>,
    inputs: Option<serde_json::Value>,
    timeout_ms: Option<u64>,
}

async fn test(arguments: &str, work_dir: &Path) -> Result<serde_json::Value, String> {
    let args: TestArgs = parse(arguments)?;
    let store = store_for(work_dir);
    let scope = args.scope.unwrap_or(SkillScope::Project);
    let record = store
        .read_one(scope, &args.name)
        .map_err(|error| format!("read skill: {error}"))?;
    let report = SkillExecutor::default()
        .execute(
            &record,
            SkillInvocation {
                input: args.inputs.unwrap_or_else(|| serde_json::json!({})),
                timeout_ms: args.timeout_ms,
            },
        )
        .await?;
    serde_json::to_value(report).map_err(|error| error.to_string())
}

#[derive(Deserialize)]
struct CommitArgs {
    manifest: SkillManifest,
    skill_md: String,
    assets: Option<Vec<AssetArg>>,
}

#[derive(Deserialize)]
struct AssetArg {
    path: PathBuf,
    content: String,
}

fn commit(arguments: &str, work_dir: &Path) -> Result<serde_json::Value, String> {
    let args: CommitArgs = parse(arguments)?;
    let assets = args
        .assets
        .unwrap_or_default()
        .into_iter()
        .map(|asset| (asset.path, asset.content.into_bytes()))
        .collect();
    let store = store_for(work_dir);
    let record = store
        .commit(SkillDraft {
            manifest: args.manifest,
            skill_md: args.skill_md,
            draft_dir: None,
            assets,
        })
        .map_err(|error| format!("commit skill: {error}"))?;
    serde_json::to_value(serde_json::json!({ "record": record })).map_err(|error| error.to_string())
}

fn cancel(arguments: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = parse(arguments)?;
    Ok(serde_json::json!({ "ok": true, "session_id": value.get("session_id") }))
}

fn list_templates() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!([
        { "name": "inline", "description": "Prompt-only skill", "entrypoint_kind": "inline" },
        { "name": "shell", "description": "Shell script skill", "entrypoint_kind": "shell" },
        { "name": "python", "description": "Python script skill", "entrypoint_kind": "python" },
        { "name": "node", "description": "Node.js script skill", "entrypoint_kind": "node" }
    ]))
}

#[derive(Deserialize)]
struct ImportArgs {
    path: PathBuf,
}

fn import(arguments: &str, work_dir: &Path) -> Result<serde_json::Value, String> {
    let args: ImportArgs = parse(arguments)?;
    let bytes = std::fs::read(&args.path).map_err(|error| format!("read archive: {error}"))?;
    let store = store_for(work_dir);
    let record = SkillPackager::import(&store, SkillScope::Project, &bytes)
        .map_err(|error| format!("import skill: {error}"))?;
    serde_json::to_value(serde_json::json!({ "record": record })).map_err(|error| error.to_string())
}

fn store_for(work_dir: &Path) -> SkillStore {
    let roots = default_roots(agent_home_dir(), work_dir.to_path_buf());
    SkillStore::with_validator(roots, SkillValidator::new())
}

pub fn registry_for_work_dir(work_dir: &Path) -> Option<SkillRegistry> {
    let store = Arc::new(SkillStore::new(default_roots(
        agent_home_dir(),
        work_dir.to_path_buf(),
    )));
    SkillRegistry::without_watcher(store).ok()
}

pub fn roots_for_work_dir(work_dir: &Path) -> Vec<ScopeRoot> {
    default_roots(agent_home_dir(), work_dir.to_path_buf())
}

fn parse<T: serde::de::DeserializeOwned>(arguments: &str) -> Result<T, String> {
    serde_json::from_str(arguments).map_err(|error| format!("invalid arguments: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_tool_returns_session_id() {
        let tool = SkillCreatorTool {
            name: "skill_creator.start".into(),
            description: "start".into(),
        };
        let result = tool
            .execute(r#"{"brief":"make weather"}"#, Path::new("."))
            .await;
        assert!(result.success);
        assert!(result.output.contains("session_id"));
    }
}
