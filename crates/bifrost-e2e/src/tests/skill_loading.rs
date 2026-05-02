//! End-to-end tests for skill loading consistency across all entry points.
//!
//! Validates that "management" (SkillStore/SkillRegistry) and "consumption"
//! (SkillsManager) agree on what skills are visible, enabled, and their
//! priority ordering.

use crate::runner::TestCase;
use bifrost_agent::skills::{
    SkillScope as AgentSkillScope, SkillsManager, AGENTS_DIR_NAME, SKILLS_DIR_NAME, SKILL_FILENAME,
};
use bifrost_skills::{
    default_roots, ScopeRoot, SkillDraft, SkillManifest, SkillRegistry, SkillScope, SkillStore,
    TriggerRule,
};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        // ---- Scope loading tests ----
        TestCase::standalone(
            "skill_loading_repo_scope_visible_in_both_systems",
            "Repo-scope skill is visible via both SkillsManager and SkillStore/SkillRegistry",
            "skill_loading",
            || async move {
                let env = TestEnv::new("repo_scope")?;

                // Create a skill in repo scope
                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "repo-tool",
                    "A repo-scoped tool",
                    "Repo tool instructions.",
                )?;

                // Also commit via SkillStore so SkillRegistry can see it
                let store = env.make_store();
                commit_inline(&store, "repo-tool", "A repo-scoped tool", SkillScope::Repo)?;

                // Verify: SkillsManager
                let manager_skills = env.load_via_manager();
                assert_contains(&manager_skills, "repo-tool", "SkillsManager")?;

                // Verify: SkillRegistry
                let registry = SkillRegistry::without_watcher(Arc::new(store))
                    .map_err(|e| format!("registry init: {e}"))?;
                let registry_skills = registry.list();
                assert_record_contains(&registry_skills, "repo-tool", "SkillRegistry")?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_user_scope_visible_in_both_systems",
            "User-scope skill is visible via both SkillsManager and SkillStore/SkillRegistry",
            "skill_loading",
            || async move {
                let env = TestEnv::new("user_scope")?;

                // Create skill in user scope (agent_home/skills/)
                env.create_skill_md(
                    &env.user_skills_dir(),
                    "user-tool",
                    "A user-scoped tool",
                    "User tool instructions.",
                )?;

                // Also commit via SkillStore
                let store = env.make_store();
                commit_inline(&store, "user-tool", "A user-scoped tool", SkillScope::User)?;

                // Verify: SkillsManager
                let manager_skills = env.load_via_manager();
                assert_contains(&manager_skills, "user-tool", "SkillsManager")?;

                // Verify: SkillRegistry
                let registry = SkillRegistry::without_watcher(Arc::new(store))
                    .map_err(|e| format!("registry init: {e}"))?;
                let registry_skills = registry.list();
                assert_record_contains(&registry_skills, "user-tool", "SkillRegistry")?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_global_scope_visible_in_both_systems",
            "Global-scope skill (~/.agents/skills/) is visible via both systems",
            "skill_loading",
            || async move {
                let env = TestEnv::new("global_scope")?;

                // Create skill in global scope (user_home/.agents/skills/)
                env.create_skill_md(
                    &env.global_skills_dir(),
                    "global-tool",
                    "A global-scoped tool",
                    "Global tool instructions.",
                )?;

                // Also commit via SkillStore
                let store = env.make_store();
                commit_inline(
                    &store,
                    "global-tool",
                    "A global-scoped tool",
                    SkillScope::Global,
                )?;

                // Verify: SkillsManager
                let manager_skills = env.load_via_manager();
                assert_contains(&manager_skills, "global-tool", "SkillsManager")?;

                // Verify: SkillRegistry
                let registry = SkillRegistry::without_watcher(Arc::new(store))
                    .map_err(|e| format!("registry init: {e}"))?;
                let registry_skills = registry.list();
                assert_record_contains(&registry_skills, "global-tool", "SkillRegistry")?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_system_scope_visible_in_both_systems",
            "System-scope skill is visible via both SkillsManager and SkillStore/SkillRegistry",
            "skill_loading",
            || async move {
                let env = TestEnv::new("system_scope")?;

                // Create skill in system scope (agent_home/skills/.system/)
                env.create_skill_md(
                    &env.system_skills_dir(),
                    "sys-tool",
                    "A system-scoped tool",
                    "System tool instructions.",
                )?;

                // Also commit via SkillStore
                let store = env.make_store();
                commit_inline(
                    &store,
                    "sys-tool",
                    "A system-scoped tool",
                    SkillScope::System,
                )?;

                // Verify: SkillsManager
                let manager_skills = env.load_via_manager();
                assert_contains(&manager_skills, "sys-tool", "SkillsManager")?;

                // Verify: SkillRegistry
                let registry = SkillRegistry::without_watcher(Arc::new(store))
                    .map_err(|e| format!("registry init: {e}"))?;
                let registry_skills = registry.list();
                assert_record_contains(&registry_skills, "sys-tool", "SkillRegistry")?;

                Ok(())
            },
        ),
        // ---- Priority / override tests ----
        TestCase::standalone(
            "skill_loading_repo_overrides_user_global_system",
            "Repo-scope skill overrides same-name skills in user/global/system scope",
            "skill_loading",
            || async move {
                let env = TestEnv::new("priority")?;

                // Create same-name skill in all four scopes with different descriptions
                env.create_skill_md(
                    &env.system_skills_dir(),
                    "multi-tool",
                    "System version",
                    "System body.",
                )?;
                env.create_skill_md(
                    &env.global_skills_dir(),
                    "multi-tool",
                    "Global version",
                    "Global body.",
                )?;
                env.create_skill_md(
                    &env.user_skills_dir(),
                    "multi-tool",
                    "User version",
                    "User body.",
                )?;
                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "multi-tool",
                    "Repo version",
                    "Repo body.",
                )?;

                // SkillsManager: repo should win
                let manager_skills = env.load_via_manager();
                let skill = manager_skills
                    .iter()
                    .find(|s| s.name == "multi-tool")
                    .ok_or("multi-tool not found in SkillsManager")?;
                if skill.description != "Repo version" {
                    return Err(format!(
                        "SkillsManager: expected 'Repo version', got '{}'",
                        skill.description
                    ));
                }
                if skill.scope != AgentSkillScope::Repo {
                    return Err(format!(
                        "SkillsManager: expected Repo scope, got {:?}",
                        skill.scope
                    ));
                }
                // Should be deduplicated — only one entry
                let count = manager_skills
                    .iter()
                    .filter(|s| s.name == "multi-tool")
                    .count();
                if count != 1 {
                    return Err(format!(
                        "SkillsManager: expected 1 multi-tool, found {}",
                        count
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_user_overrides_global_and_system",
            "User-scope skill overrides global and system when no repo skill exists",
            "skill_loading",
            || async move {
                let env = TestEnv::new("user_override")?;

                env.create_skill_md(
                    &env.system_skills_dir(),
                    "shared-tool",
                    "System version",
                    "System body.",
                )?;
                env.create_skill_md(
                    &env.global_skills_dir(),
                    "shared-tool",
                    "Global version",
                    "Global body.",
                )?;
                env.create_skill_md(
                    &env.user_skills_dir(),
                    "shared-tool",
                    "User version",
                    "User body.",
                )?;

                let manager_skills = env.load_via_manager();
                let skill = manager_skills
                    .iter()
                    .find(|s| s.name == "shared-tool")
                    .ok_or("shared-tool not found")?;
                if skill.description != "User version" {
                    return Err(format!(
                        "Expected 'User version', got '{}'",
                        skill.description
                    ));
                }

                Ok(())
            },
        ),
        // ---- Enable/Disable consistency tests ----
        TestCase::standalone(
            "skill_loading_disable_via_store_hides_from_manager",
            "Disabling a skill via SkillStore.enable(false) hides it from SkillsManager",
            "skill_loading",
            || async move {
                let env = TestEnv::new("disable_store")?;

                // Create skill in repo scope
                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "toggle-tool",
                    "Toggle tool",
                    "Toggle body.",
                )?;

                // Before disabling: visible
                let before = env.load_via_manager();
                assert_contains(&before, "toggle-tool", "SkillsManager (before disable)")?;

                // Disable via SkillStore mechanism (.disabled marker)
                let disabled_marker = env
                    .repo_skills_dir()
                    .join("toggle-tool")
                    .join(".disabled");
                fs::write(&disabled_marker, b"disabled")
                    .map_err(|e| format!("write .disabled: {e}"))?;

                // After disabling: hidden from SkillsManager
                let after = env.load_via_manager();
                if after.iter().any(|s| s.name == "toggle-tool") {
                    return Err(
                        "SkillsManager still shows disabled skill after .disabled marker written"
                            .to_string(),
                    );
                }

                // Re-enable by removing marker
                fs::remove_file(&disabled_marker)
                    .map_err(|e| format!("remove .disabled: {e}"))?;

                let re_enabled = env.load_via_manager();
                assert_contains(
                    &re_enabled,
                    "toggle-tool",
                    "SkillsManager (after re-enable)",
                )?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_disable_via_store_marks_disabled_in_registry",
            "Disabling a skill via SkillStore.enable(false) marks it disabled in SkillRegistry",
            "skill_loading",
            || async move {
                let env = TestEnv::new("disable_registry")?;

                let store = env.make_store();
                commit_inline(&store, "reg-tool", "Reg tool", SkillScope::Repo)?;

                // Initially enabled
                let registry = SkillRegistry::without_watcher(Arc::new(store.clone()))
                    .map_err(|e| format!("registry: {e}"))?;
                let enabled = registry.enabled();
                assert_record_contains(&enabled, "reg-tool", "SkillRegistry.enabled()")?;

                // Disable
                store
                    .enable(SkillScope::Repo, "reg-tool", false)
                    .map_err(|e| format!("store.enable(false): {e}"))?;

                // Reload registry
                let registry2 = SkillRegistry::without_watcher(Arc::new(store.clone()))
                    .map_err(|e| format!("registry2: {e}"))?;
                let enabled2 = registry2.enabled();
                if enabled2.iter().any(|r| r.name == "reg-tool") {
                    return Err(
                        "SkillRegistry.enabled() still contains disabled skill".to_string()
                    );
                }
                // But still listed (just not enabled)
                let all = registry2.list();
                assert_record_contains(&all, "reg-tool", "SkillRegistry.list() after disable")?;

                // Re-enable
                store
                    .enable(SkillScope::Repo, "reg-tool", true)
                    .map_err(|e| format!("store.enable(true): {e}"))?;
                let registry3 = SkillRegistry::without_watcher(Arc::new(store.clone()))
                    .map_err(|e| format!("registry3: {e}"))?;
                let enabled3 = registry3.enabled();
                assert_record_contains(&enabled3, "reg-tool", "SkillRegistry.enabled() re-enable")?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_disable_consistent_across_manager_and_store",
            "Full cycle: disable via SkillStore hides from both SkillsManager and SkillRegistry.enabled()",
            "skill_loading",
            || async move {
                let env = TestEnv::new("disable_full")?;

                // Create SKILL.md for SkillsManager and commit for SkillStore
                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "full-tool",
                    "Full tool",
                    "Full tool body.",
                )?;
                let store = env.make_store();
                commit_inline(&store, "full-tool", "Full tool", SkillScope::Repo)?;

                // Both systems see it
                let mgr = env.load_via_manager();
                assert_contains(&mgr, "full-tool", "SkillsManager initial")?;
                let reg = SkillRegistry::without_watcher(Arc::new(store.clone()))
                    .map_err(|e| format!("registry: {e}"))?;
                assert_record_contains(&reg.enabled(), "full-tool", "SkillRegistry initial")?;

                // Disable via store
                store
                    .enable(SkillScope::Repo, "full-tool", false)
                    .map_err(|e| format!("disable: {e}"))?;

                // Both systems hide it
                let mgr2 = env.load_via_manager();
                if mgr2.iter().any(|s| s.name == "full-tool") {
                    return Err("SkillsManager still shows full-tool after disable".to_string());
                }
                let reg2 = SkillRegistry::without_watcher(Arc::new(store.clone()))
                    .map_err(|e| format!("registry2: {e}"))?;
                if reg2.enabled().iter().any(|r| r.name == "full-tool") {
                    return Err(
                        "SkillRegistry.enabled() still shows full-tool after disable".to_string(),
                    );
                }

                // Re-enable — both see it again
                store
                    .enable(SkillScope::Repo, "full-tool", true)
                    .map_err(|e| format!("re-enable: {e}"))?;
                let mgr3 = env.load_via_manager();
                assert_contains(&mgr3, "full-tool", "SkillsManager after re-enable")?;
                let reg3 = SkillRegistry::without_watcher(Arc::new(store.clone()))
                    .map_err(|e| format!("registry3: {e}"))?;
                assert_record_contains(
                    &reg3.enabled(),
                    "full-tool",
                    "SkillRegistry after re-enable",
                )?;

                Ok(())
            },
        ),
        // ---- Prompt injection tests ----
        TestCase::standalone(
            "skill_loading_enabled_skill_appears_in_prompt",
            "Enabled skill content is injected into system prompt via build_skills_instructions",
            "skill_loading",
            || async move {
                let env = TestEnv::new("prompt_inject")?;

                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "prompt-tool",
                    "Prompt tool",
                    "Use this tool to do amazing things.",
                )?;

                let manager = env.make_manager();
                let skills = manager.load_skills(&env.work_dir);
                let instructions = manager.build_skills_instructions(&skills);

                if !instructions.contains("prompt-tool") {
                    return Err("Prompt does not contain skill name".to_string());
                }
                if !instructions.contains("Prompt tool") {
                    return Err("Prompt does not contain skill description".to_string());
                }
                if !instructions.contains("Use this tool to do amazing things.") {
                    return Err("Prompt does not contain skill body".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_disabled_skill_excluded_from_prompt",
            "Disabled skill content is NOT injected into system prompt",
            "skill_loading",
            || async move {
                let env = TestEnv::new("prompt_exclude")?;

                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "hidden-tool",
                    "Hidden tool",
                    "This should NOT appear.",
                )?;

                // Disable it
                let disabled_marker = env
                    .repo_skills_dir()
                    .join("hidden-tool")
                    .join(".disabled");
                fs::write(&disabled_marker, b"disabled")
                    .map_err(|e| format!("write .disabled: {e}"))?;

                let manager = env.make_manager();
                let skills = manager.load_skills(&env.work_dir);
                let instructions = manager.build_skills_instructions(&skills);

                if instructions.contains("hidden-tool") {
                    return Err("Disabled skill name appeared in prompt".to_string());
                }
                if instructions.contains("This should NOT appear.") {
                    return Err("Disabled skill body appeared in prompt".to_string());
                }

                Ok(())
            },
        ),
        // ---- Slash command resolution tests ----
        TestCase::standalone(
            "skill_loading_slash_command_resolves_via_registry",
            "Skill with slash_command resolves via SkillRegistry",
            "skill_loading",
            || async move {
                let env = TestEnv::new("slash_resolve")?;

                let store = env.make_store();
                let mut manifest =
                    make_manifest("weather-cmd", "Weather lookup", SkillScope::Repo);
                manifest.slash_command = Some("/weather".to_string());
                manifest.triggers = vec![TriggerRule::SlashCommand];
                store
                    .commit(SkillDraft {
                        manifest,
                        skill_md: "---\nname: weather-cmd\nslash_command: /weather\n---\n# Weather"
                            .into(),
                        draft_dir: None,
                        assets: Vec::new(),
                    })
                    .map_err(|e| format!("commit: {e}"))?;

                let registry = SkillRegistry::without_watcher(Arc::new(store))
                    .map_err(|e| format!("registry: {e}"))?;
                let resolved = registry
                    .resolve_slash("/weather")
                    .ok_or("slash /weather did not resolve")?;
                if resolved.name != "weather-cmd" {
                    return Err(format!("Unexpected resolved name: {}", resolved.name));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "skill_loading_disabled_slash_command_still_resolves",
            "Disabled skill slash command still resolves in SkillRegistry.list but not in enabled()",
            "skill_loading",
            || async move {
                let env = TestEnv::new("slash_disabled")?;

                let store = env.make_store();
                let mut manifest =
                    make_manifest("slash-tool", "Slash tool", SkillScope::Repo);
                manifest.slash_command = Some("/slashtest".to_string());
                manifest.triggers = vec![TriggerRule::SlashCommand];
                store
                    .commit(SkillDraft {
                        manifest,
                        skill_md:
                            "---\nname: slash-tool\nslash_command: /slashtest\n---\n# Slash"
                                .into(),
                        draft_dir: None,
                        assets: Vec::new(),
                    })
                    .map_err(|e| format!("commit: {e}"))?;

                store
                    .enable(SkillScope::Repo, "slash-tool", false)
                    .map_err(|e| format!("disable: {e}"))?;

                let registry = SkillRegistry::without_watcher(Arc::new(store))
                    .map_err(|e| format!("registry: {e}"))?;

                // list() still contains it
                assert_record_contains(
                    &registry.list(),
                    "slash-tool",
                    "SkillRegistry.list() after disable",
                )?;

                // enabled() does not contain it
                if registry.enabled().iter().any(|r| r.name == "slash-tool") {
                    return Err("SkillRegistry.enabled() still has disabled slash skill".to_string());
                }

                Ok(())
            },
        ),
        // ---- default_roots path alignment tests ----
        TestCase::standalone(
            "skill_loading_default_roots_has_four_scopes",
            "default_roots() returns roots for System, Global, User, Repo",
            "skill_loading",
            || async move {
                let user_home =
                    std::path::PathBuf::from("/tmp/bifrost-e2e-skill-loading/fake-home");
                let work_dir =
                    std::path::PathBuf::from("/tmp/bifrost-e2e-skill-loading/fake-project");
                let roots = default_roots(user_home.clone(), work_dir.clone());

                if roots.len() != 4 {
                    return Err(format!("Expected 4 roots, got {}", roots.len()));
                }

                // Check scopes
                let scopes: Vec<_> = roots.iter().map(|r| r.scope.clone()).collect();
                if !scopes.contains(&SkillScope::System) {
                    return Err("Missing System scope".to_string());
                }
                if !scopes.contains(&SkillScope::Global) {
                    return Err("Missing Global scope".to_string());
                }
                if !scopes.contains(&SkillScope::User) {
                    return Err("Missing User scope".to_string());
                }
                if !scopes.contains(&SkillScope::Repo) {
                    return Err("Missing Repo scope".to_string());
                }

                // Check paths
                let system_root = roots.iter().find(|r| r.scope == SkillScope::System).unwrap();
                let expected_system = user_home.join(".bifrost/agent/skills/.system");
                if system_root.path != expected_system {
                    return Err(format!(
                        "System path mismatch: {:?} vs {:?}",
                        system_root.path, expected_system
                    ));
                }

                let global_root = roots.iter().find(|r| r.scope == SkillScope::Global).unwrap();
                let expected_global = user_home.join(".agents/skills");
                if global_root.path != expected_global {
                    return Err(format!(
                        "Global path mismatch: {:?} vs {:?}",
                        global_root.path, expected_global
                    ));
                }

                let user_root = roots.iter().find(|r| r.scope == SkillScope::User).unwrap();
                let expected_user = user_home.join(".bifrost/agent/skills");
                if user_root.path != expected_user {
                    return Err(format!(
                        "User path mismatch: {:?} vs {:?}",
                        user_root.path, expected_user
                    ));
                }

                let repo_root = roots.iter().find(|r| r.scope == SkillScope::Repo).unwrap();
                let expected_repo = work_dir.join(".agents/skills");
                if repo_root.path != expected_repo {
                    return Err(format!(
                        "Repo path mismatch: {:?} vs {:?}",
                        repo_root.path, expected_repo
                    ));
                }

                Ok(())
            },
        ),
        // ---- Hidden directory filtering tests ----
        TestCase::standalone(
            "skill_loading_hidden_dirs_are_skipped",
            "Hidden directories (.git, .drafts, etc.) are not loaded as skills",
            "skill_loading",
            || async move {
                let env = TestEnv::new("hidden_dirs")?;

                // Create a normal skill
                env.create_skill_md(
                    &env.repo_skills_dir(),
                    "visible-tool",
                    "Visible tool",
                    "Visible body.",
                )?;

                // Create a hidden dir with SKILL.md (should be skipped)
                let hidden_dir = env.repo_skills_dir().join(".hidden-tool");
                fs::create_dir_all(&hidden_dir).map_err(|e| format!("mkdir hidden: {e}"))?;
                fs::write(
                    hidden_dir.join(SKILL_FILENAME),
                    "---\nname: hidden-tool\ndescription: Should not appear\n---\nHidden body.",
                )
                .map_err(|e| format!("write hidden skill: {e}"))?;

                let manager_skills = env.load_via_manager();
                assert_contains(&manager_skills, "visible-tool", "SkillsManager")?;

                if manager_skills.iter().any(|s| s.name == "hidden-tool") {
                    return Err(
                        "SkillsManager loaded skill from hidden directory .hidden-tool".to_string(),
                    );
                }

                Ok(())
            },
        ),
        // ---- Nested skill discovery test ----
        TestCase::standalone(
            "skill_loading_nested_skills_discovered",
            "Skills nested in subdirectories (depth > 1) are discovered via BFS",
            "skill_loading",
            || async move {
                let env = TestEnv::new("nested")?;

                // Create skill at depth 2: .agents/skills/category/nested-tool/SKILL.md
                let nested_dir = env
                    .repo_skills_dir()
                    .join("category")
                    .join("nested-tool");
                fs::create_dir_all(&nested_dir).map_err(|e| format!("mkdir nested: {e}"))?;
                fs::write(
                    nested_dir.join(SKILL_FILENAME),
                    "---\nname: nested-tool\ndescription: Nested skill\n---\nNested body.",
                )
                .map_err(|e| format!("write nested skill: {e}"))?;

                let manager_skills = env.load_via_manager();
                assert_contains(&manager_skills, "nested-tool", "SkillsManager (nested)")?;

                Ok(())
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// Test environment helper
// ---------------------------------------------------------------------------

struct TestEnv {
    _temp: tempfile::TempDir,
    work_dir: std::path::PathBuf,
    agent_home: std::path::PathBuf,
    user_home: std::path::PathBuf,
}

impl TestEnv {
    fn new(suffix: &str) -> Result<Self, String> {
        let temp = tempfile::tempdir().map_err(|e| format!("create temp dir: {e}"))?;
        let work_dir = temp.path().join("project");
        let agent_home = temp.path().join("agent-home");
        let user_home = temp.path().join("user-home");
        fs::create_dir_all(&work_dir).map_err(|e| format!("create work_dir ({suffix}): {e}"))?;
        fs::create_dir_all(&agent_home)
            .map_err(|e| format!("create agent_home ({suffix}): {e}"))?;
        fs::create_dir_all(&user_home).map_err(|e| format!("create user_home ({suffix}): {e}"))?;
        Ok(Self {
            _temp: temp,
            work_dir,
            agent_home,
            user_home,
        })
    }

    /// Repo scope: <work_dir>/.agents/skills/
    fn repo_skills_dir(&self) -> std::path::PathBuf {
        self.work_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME)
    }

    /// User scope: <agent_home>/skills/
    fn user_skills_dir(&self) -> std::path::PathBuf {
        self.agent_home.join(SKILLS_DIR_NAME)
    }

    /// Global scope: <user_home>/.agents/skills/
    fn global_skills_dir(&self) -> std::path::PathBuf {
        self.user_home.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME)
    }

    /// System scope: <agent_home>/skills/.system/
    fn system_skills_dir(&self) -> std::path::PathBuf {
        self.agent_home.join(SKILLS_DIR_NAME).join(".system")
    }

    fn create_skill_md(
        &self,
        parent: &std::path::Path,
        name: &str,
        description: &str,
        body: &str,
    ) -> Result<(), String> {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).map_err(|e| format!("create skill dir {name}: {e}"))?;
        fs::write(
            dir.join(SKILL_FILENAME),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
        )
        .map_err(|e| format!("write SKILL.md for {name}: {e}"))?;
        Ok(())
    }

    fn make_manager(&self) -> SkillsManager {
        SkillsManager::with_overrides(None, self.agent_home.clone(), self.user_home.clone())
    }

    fn load_via_manager(&self) -> Vec<bifrost_agent::SkillMetadata> {
        self.make_manager().load_skills(&self.work_dir)
    }

    fn make_store(&self) -> SkillStore {
        SkillStore::new(vec![
            ScopeRoot::new(SkillScope::System, self.system_skills_dir()),
            ScopeRoot::new(SkillScope::Global, self.global_skills_dir()),
            ScopeRoot::new(SkillScope::User, self.user_skills_dir()),
            ScopeRoot::new(SkillScope::Repo, self.repo_skills_dir()),
        ])
    }
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

fn assert_contains(
    skills: &[bifrost_agent::SkillMetadata],
    name: &str,
    context: &str,
) -> Result<(), String> {
    if !skills.iter().any(|s| s.name == name) {
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        Err(format!(
            "{context}: skill '{name}' not found. Available: {names:?}"
        ))
    } else {
        Ok(())
    }
}

fn assert_record_contains(
    records: &[bifrost_skills::SkillRecord],
    name: &str,
    context: &str,
) -> Result<(), String> {
    if !records.iter().any(|r| r.name == name) {
        let names: Vec<_> = records.iter().map(|r| r.name.as_str()).collect();
        Err(format!(
            "{context}: skill '{name}' not found. Available: {names:?}"
        ))
    } else {
        Ok(())
    }
}

fn make_manifest(name: &str, description: &str, scope: SkillScope) -> SkillManifest {
    SkillManifest {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        description: description.to_string(),
        scope,
        entrypoint: bifrost_skills::Entrypoint::Inline {
            instructions_md: format!("{description} instructions"),
        },
        allowed_tools: Vec::new(),
        slash_command: None,
        triggers: vec![TriggerRule::DescriptionMatch],
        inputs_schema: None,
        outputs_schema: None,
        metadata: BTreeMap::new(),
        created_by: bifrost_skills::SkillAuthor::Agent {
            session_id: "e2e".to_string(),
        },
        created_at_unix: 1,
        updated_at_unix: 1,
        checksum: String::new(),
        schema_version: 1,
    }
}

fn commit_inline(
    store: &SkillStore,
    name: &str,
    description: &str,
    scope: SkillScope,
) -> Result<(), String> {
    let manifest = make_manifest(name, description, scope);
    store
        .commit(SkillDraft {
            manifest,
            skill_md: format!("---\nname: {name}\ndescription: {description}\n---\n# {name}"),
            draft_dir: None,
            assets: Vec::new(),
        })
        .map_err(|e| format!("commit {name}: {e}"))?;
    Ok(())
}
