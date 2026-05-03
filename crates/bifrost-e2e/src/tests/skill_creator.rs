use crate::runner::TestCase;
use bifrost_skills::{
    Entrypoint, ScopeRoot, SkillDraft, SkillExecutor, SkillInvocation, SkillManifest,
    SkillPackager, SkillRegistry, SkillScope, SkillStore, TriggerRule,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![TestCase::standalone(
        "skill_creator_create_test_invoke_delete_import",
        "Validate skill creator create, test, invoke, delete, package, and import flow",
        "skill_creator",
        || async move {
            let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
            let import_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
            let store = SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, dir.path())]);
            let manifest = manifest("weather-skill");

            let record = store
                .commit(SkillDraft {
                    manifest,
                    skill_md: "---\nname: weather-skill\n---\n# Weather\n".to_string(),
                    draft_dir: None,
                    assets: Vec::new(),
                })
                .map_err(|error| format!("commit skill: {error}"))?;

            let report = SkillExecutor::default()
                .execute(
                    &record,
                    SkillInvocation {
                        input: serde_json::json!({ "city": "Paris" }),
                        timeout_ms: None,
                    },
                )
                .await?;
            if report.exit_code != Some(0) || !report.stdout.contains("city") {
                return Err(format!("unexpected test report: {report:?}"));
            }

            let registry = SkillRegistry::without_watcher(Arc::new(store.clone()))
                .map_err(|error| format!("registry: {error}"))?;
            let invoked = registry
                .resolve_slash("/weather")
                .ok_or_else(|| "slash command did not resolve".to_string())?;
            if invoked.name != "weather-skill" {
                return Err(format!("unexpected slash record: {}", invoked.name));
            }

            let archive = SkillPackager::package(&store, SkillScope::Repo, "weather-skill")
                .map_err(|error| format!("package skill: {error}"))?;
            store
                .delete(SkillScope::Repo, "weather-skill")
                .map_err(|error| format!("delete skill: {error}"))?;
            let registry = SkillRegistry::without_watcher(Arc::new(store.clone()))
                .map_err(|error| format!("registry after delete: {error}"))?;
            if registry.resolve_slash("/weather").is_some() {
                return Err("deleted skill still resolves slash command".to_string());
            }

            let import_store =
                SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, import_dir.path())]);
            let imported = SkillPackager::import(&import_store, SkillScope::Repo, &archive)
                .map_err(|error| format!("import skill: {error}"))?;
            if imported.name != "weather-skill" {
                return Err(format!("unexpected imported skill: {}", imported.name));
            }

            Ok(())
        },
    )]
}

fn manifest(name: &str) -> SkillManifest {
    SkillManifest {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        description: "Weather skill".to_string(),
        scope: SkillScope::Repo,
        entrypoint: Entrypoint::Inline {
            instructions_md: "Return weather for the input city.".to_string(),
        },
        allowed_tools: Vec::new(),
        slash_command: Some("/weather".to_string()),
        triggers: vec![TriggerRule::SlashCommand],
        inputs_schema: Some(serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"]
        })),
        outputs_schema: None,
        metadata: BTreeMap::new(),
        env: BTreeMap::new(),
        created_by: bifrost_skills::SkillAuthor::Agent {
            session_id: "e2e".to_string(),
        },
        created_at_unix: 1,
        updated_at_unix: 1,
        checksum: String::new(),
        schema_version: 1,
    }
}
