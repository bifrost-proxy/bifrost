//! System prompt builder for the agent.
//!
//! # Architecture
//!
//! The prompt is assembled from multiple independent sections:
//!
//! 1. **Base Instructions** - System-level instructions loaded from external markdown template.
//!    Priority: `override > config.base_instructions > default template`
//!
//! 2. **Environment Context** - Runtime info (cwd, shell, OS, arch, date) rendered as XML.
//!
//! 3. **Developer Instructions** - Config-level developer instructions, rendered as XML block.
//!
//! 4. **User Instructions** - AGENTS.md + config-level user instructions, rendered as XML block.
//!
//! 5. **Skills Instructions** - Budget-aware skill list with progressive disclosure.
//!
//! Each section is independently testable and can be incrementally updated across turns.
//!
//! # Message Roles
//!
//! Bifrost keeps base instructions separate from developer and contextual user
//! fragments. Responses requests map base instructions to the wire-level
//! `instructions` field; Chat Completions fallback flattens the same prefix into
//! compatible message roles.

pub mod fragments;
pub mod models;
mod render;
mod types;

use std::path::Path;

use bifrost_skills::SkillRegistry;

use crate::config::AgentConfig;
use crate::skills::{SkillMetadata, SkillMetadataBudget, SkillsManager};
use crate::types::ChatMessage;

pub use types::{
    BaseInstructions, EnvironmentContext, UserInstructions, ENVIRONMENT_CONTEXT_CLOSE_TAG,
    ENVIRONMENT_CONTEXT_OPEN_TAG, GOAL_CONTEXT_CLOSE_TAG, GOAL_CONTEXT_OPEN_TAG,
    SKILLS_INSTRUCTIONS_CLOSE_TAG, SKILLS_INSTRUCTIONS_OPEN_TAG, USER_INSTRUCTIONS_CLOSE_TAG,
    USER_INSTRUCTIONS_OPEN_TAG,
};

// Re-export fragment system types for convenient access.
pub use fragments::{
    ContextFragment, ContextualFragment, FragmentRegistration, FragmentRegistrationProxy,
    FragmentRegistry, GoalContext, PromptSlot,
};

// Re-export model-specific types.
pub use models::{ModelFamily, ModelMessages, PersonalityVariant};

/// Default base instructions template loaded from external markdown file.
const BASE_INSTRUCTIONS_DEFAULT: &str = include_str!("../prompts/base_instructions/default.md");

/// Return the built-in base instructions shown in WebUI when no override is configured.
pub fn default_base_instructions() -> &'static str {
    BASE_INSTRUCTIONS_DEFAULT
}

#[derive(Debug, Clone)]
pub struct PromptMessages {
    pub prefix: Vec<ChatMessage>,
    pub base_instructions: String,
}

/// Build the system prompt for the agent.
///
/// Priority:
/// 1. `override_prompt` (per-turn override)
/// 2. Built-in prompt + pre-loaded user instructions + skills
pub fn build_system_prompt(
    config: &AgentConfig,
    override_prompt: Option<&str>,
    work_dir_override: Option<&str>,
    user_instructions: Option<&str>,
) -> String {
    build_system_prompt_with_skill_registry(
        config,
        override_prompt,
        work_dir_override,
        None,
        user_instructions,
    )
}

pub fn resolve_base_instructions_text(
    config: &AgentConfig,
    override_prompt: Option<&str>,
) -> String {
    if let Some(custom) = override_prompt {
        return custom.to_string();
    }
    resolve_base_instructions(config).text
}

pub fn build_prompt_messages_with_skill_registry(
    config: &AgentConfig,
    base_instructions: &str,
    work_dir_override: Option<&str>,
    registry: Option<&SkillRegistry>,
    user_instructions: Option<&str>,
) -> PromptMessages {
    use crate::prompt::fragments::{ContextFragment, FragmentRegistry, PromptSlot};
    use crate::prompt::models::ModelMessages;

    let work_dir = work_dir_override
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.resolve_work_dir());

    let mut frag_registry = FragmentRegistry::new();

    // Model-specific instructions → DeveloperPolicy slot
    let model_msgs = ModelMessages::new(config.model.as_deref().unwrap_or("default"), None);
    if let Some(family_instructions) = model_msgs
        .family_instructions()
        .filter(|text| !base_instructions.contains(*text))
    {
        frag_registry.register(ContextFragment::new(
            "model_family_instructions",
            PromptSlot::DeveloperPolicy,
            95,
            family_instructions,
        ));
    }
    let personality_instructions = model_msgs.personality_instructions();
    if !base_instructions.contains(personality_instructions) {
        frag_registry.register(ContextFragment::new(
            "personality_instructions",
            PromptSlot::DeveloperPolicy,
            90,
            personality_instructions,
        ));
    }
    if let Some(patch_instructions) = model_msgs.apply_patch_instructions() {
        frag_registry.register(ContextFragment::new(
            "apply_patch_instructions",
            PromptSlot::DeveloperPolicy,
            85,
            patch_instructions,
        ));
    }
    if let Some(final_guidelines) = model_msgs.final_answer_guidelines() {
        frag_registry.register(ContextFragment::new(
            "final_answer_guidelines",
            PromptSlot::DeveloperPolicy,
            80,
            final_guidelines,
        ));
    }

    // Developer instructions → DeveloperPolicy slot
    let developer_section =
        build_developer_instructions_section(config.developer_instructions.as_deref());
    if !developer_section.is_empty() {
        frag_registry.register(ContextFragment::new(
            "developer_instructions",
            PromptSlot::DeveloperPolicy,
            100,
            developer_section,
        ));
    }

    // Skills → DeveloperCapabilities slot
    let skills_section = build_skills_section(config, &work_dir, registry);
    if !skills_section.is_empty() {
        frag_registry.register(ContextFragment::new(
            "skills",
            PromptSlot::DeveloperCapabilities,
            80,
            skills_section,
        ));
    }

    // User instructions → ContextualUser slot (higher priority = rendered first)
    let user_section = build_user_instructions_section(user_instructions, &work_dir);
    if !user_section.is_empty() {
        frag_registry.register(ContextFragment::new(
            "user_instructions",
            PromptSlot::ContextualUser,
            100,
            user_section,
        ));
    }

    // Environment context → ContextualUser slot
    let env_context = EnvironmentContext::from_system(Some(&work_dir));
    let env_section = render::render_environment_context(&env_context);
    if !env_section.is_empty() {
        frag_registry.register(ContextFragment::new(
            "environment",
            PromptSlot::ContextualUser,
            90,
            env_section,
        ));
    }

    // Assemble: base_instructions as system message + fragment-based messages
    let mut prefix = Vec::new();
    if !base_instructions.trim().is_empty() {
        prefix.push(ChatMessage::system(base_instructions));
    }
    prefix.extend(frag_registry.assemble());

    PromptMessages {
        prefix,
        base_instructions: base_instructions.to_string(),
    }
}

/// Build the system prompt with an optional skill registry.
///
/// This is the main entry point for prompt assembly.
pub fn build_system_prompt_with_skill_registry(
    config: &AgentConfig,
    override_prompt: Option<&str>,
    work_dir_override: Option<&str>,
    registry: Option<&SkillRegistry>,
    user_instructions: Option<&str>,
) -> String {
    // Step 1: Resolve work directory
    let work_dir = work_dir_override
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.resolve_work_dir());

    // Step 2: Build base instructions. The override replaces only base
    // instructions; environment, skills, developer and user fragments remain.
    let base_instructions =
        BaseInstructions::new(resolve_base_instructions_text(config, override_prompt));

    // Step 3: Build environment context
    let env_context = EnvironmentContext::from_system(Some(&work_dir));
    let env_section = render::render_environment_context(&env_context);

    // Step 4: Build skills section (budget-aware)
    let skills_section = build_skills_section(config, &work_dir, registry);

    // Step 5: Build developer and user instructions sections
    let developer_section =
        build_developer_instructions_section(config.developer_instructions.as_deref());
    let user_section = build_user_instructions_section(user_instructions, &work_dir);

    // Step 6: Assemble final prompt
    assemble_prompt(
        &base_instructions,
        &env_section,
        &skills_section,
        &developer_section,
        &user_section,
    )
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Resolve base instructions with 2-tier priority:
/// 1. Config-level `base_instructions` override
/// 2. Default template from external markdown
fn resolve_base_instructions(config: &AgentConfig) -> BaseInstructions {
    if let Some(ref instructions) = config.base_instructions {
        return BaseInstructions::new(instructions);
    }
    BaseInstructions::new(BASE_INSTRUCTIONS_DEFAULT)
}

/// Build the skills instructions section with budget-aware rendering.
fn build_skills_section(
    config: &AgentConfig,
    work_dir: &Path,
    registry: Option<&SkillRegistry>,
) -> String {
    // Use registry if provided, otherwise load from SkillsManager
    let skills: Vec<SkillMetadata> = if let Some(reg) = registry {
        reg.enabled()
            .into_iter()
            .map(|skill| SkillMetadata {
                name: skill.name.clone(),
                description: skill.description.clone(),
                short_description: None,
                prompt_content: String::new(),
                path: skill.path.clone(),
                scope: skill.scope,
                interface: None,
                dependencies: None,
                policy: None,
            })
            .collect()
    } else {
        let manager = SkillsManager::new(config.skills.clone());
        manager.load_skills(work_dir)
    };

    if skills.is_empty() {
        return String::new();
    }

    // Compute budget from config's context window
    let budget = SkillMetadataBudget::from_context_window(config.model_context_window);

    // Render with budget
    let (skill_body, _report) = render_skill_lines_with_intro(&skills, budget);
    render::render_skills_section(&skill_body)
}

/// Build the developer instructions section.
fn build_developer_instructions_section(developer_instructions: Option<&str>) -> String {
    let text = match developer_instructions {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => return String::new(),
    };

    format!("<developer_instructions>\n{text}\n</developer_instructions>")
}

/// Build the user instructions section.
fn build_user_instructions_section(user_instructions: Option<&str>, work_dir: &Path) -> String {
    let text = match user_instructions {
        Some(t) if !t.trim().is_empty() => t,
        _ => return String::new(),
    };

    let instructions = UserInstructions::new(text).with_directory(work_dir.display().to_string());
    render::render_user_instructions(&instructions)
}

/// Assemble the final prompt from all sections.
fn assemble_prompt(
    base_instructions: &BaseInstructions,
    env_section: &str,
    skills_section: &str,
    developer_section: &str,
    user_section: &str,
) -> String {
    let mut parts = Vec::new();

    // Part 1: Base instructions
    parts.push(base_instructions.text.clone());

    // Part 2: Environment context
    if !env_section.is_empty() {
        parts.push(String::new()); // blank line
        parts.push(env_section.to_string());
    }

    // Part 3: Skills
    if !skills_section.is_empty() {
        parts.push(String::new());
        parts.push(skills_section.to_string());
    }

    // Part 4: Developer instructions
    if !developer_section.is_empty() {
        parts.push(String::new());
        parts.push(developer_section.to_string());
    }

    // Part 5: User instructions
    if !user_section.is_empty() {
        parts.push(String::new());
        parts.push(user_section.to_string());
    }

    parts.join("\n")
}

/// Render skill lines with introductory text (matching Codex's `render_available_skills_body`).
fn render_skill_lines_with_intro(
    skills: &[SkillMetadata],
    budget: SkillMetadataBudget,
) -> (String, crate::skills::SkillRenderReport) {
    use crate::skills::render_skill_lines;

    let (lines, report) = render_skill_lines(skills, budget);

    // Build full section with intro
    let intro = crate::skills::render::SKILLS_INTRO;
    let how_to_use = crate::skills::render::SKILLS_HOW_TO_USE;

    let mut body = String::new();
    body.push_str(intro);
    body.push_str("\n\n### Available skills\n");
    for line in &lines {
        body.push_str(line);
        body.push('\n');
    }
    body.push_str("\n### How to use skills\n");
    body.push_str(how_to_use);

    (body, report)
}

// ---------------------------------------------------------------------------
// SkillMetadataBudget extension
// ---------------------------------------------------------------------------

impl SkillMetadataBudget {
    /// Create a budget from an optional context window size.
    ///
    /// When context window is known, uses 2% of it as the token budget.
    /// Falls back to a default character budget otherwise.
    pub fn from_context_window(context_window: Option<i64>) -> Self {
        crate::skills::default_skill_metadata_budget(context_window)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_base_instructions_default() {
        let config = AgentConfig::default();
        let instructions = resolve_base_instructions(&config);
        assert!(instructions.text.contains("Bifrost Agent"));
    }

    #[test]
    fn test_default_base_instructions_describe_terminal_tool_selection() {
        let config = AgentConfig::default();
        let instructions = resolve_base_instructions(&config);
        assert!(instructions.text.contains("exec_command"));
        assert!(instructions.text.contains("tty=true"));
        assert!(!instructions.text.contains("shell_pty"));
        assert!(instructions.text.contains("write_stdin"));
        assert!(!instructions.text.contains("`shell`"));
        assert!(instructions
            .text
            .contains("Use `exec_command` for terminal commands"));
        assert!(instructions
            .text
            .contains("foreground job that is still running"));
        assert!(instructions
            .text
            .contains("Do not infer persistence only from command names"));
    }

    #[test]
    fn test_resolve_base_instructions_override() {
        let config = AgentConfig {
            base_instructions: Some("Custom instructions".to_string()),
            ..AgentConfig::default()
        };
        let instructions = resolve_base_instructions(&config);
        assert_eq!(instructions.text, "Custom instructions");
    }

    #[test]
    fn test_build_developer_instructions_section() {
        let result = build_developer_instructions_section(Some("  Follow policy  "));
        assert!(result.contains("<developer_instructions>"));
        assert!(result.contains("Follow policy"));
    }

    #[test]
    fn test_build_user_instructions_section_empty() {
        let result = build_user_instructions_section(None, Path::new("/tmp"));
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_user_instructions_section_with_content() {
        let result =
            build_user_instructions_section(Some("Follow code style"), Path::new("/home/project"));
        assert!(result.contains("<user_instructions>"));
        assert!(result.contains("Follow code style"));
        assert!(result.contains("<directory>/home/project</directory>"));
    }

    #[test]
    fn test_assemble_prompt_full() {
        let base = BaseInstructions::new("System prompt");
        let env = "<environment_context>\n  <os>macos</os>\n</environment_context>";
        let skills = "<skills_instructions>\n- skill: desc\n</skills_instructions>";
        let developer = "<developer_instructions>\nDev rules\n</developer_instructions>";
        let user = "<user_instructions>\n  <content>Rules</content>\n</user_instructions>";

        let result = assemble_prompt(&base, env, skills, developer, user);
        assert!(result.starts_with("System prompt"));
        assert!(result.contains("<environment_context>"));
        assert!(result.contains("<skills_instructions>"));
        assert!(result.contains("<developer_instructions>"));
        assert!(result.contains("<user_instructions>"));
    }

    #[test]
    fn test_build_system_prompt_override() {
        let config = AgentConfig::default();
        let result = build_system_prompt(
            &config,
            Some("Override prompt"),
            None,
            Some("User instructions"),
        );
        assert!(result.starts_with("Override prompt"));
        assert!(result.contains("<environment_context>"));
        assert!(result.contains("<user_instructions>"));
    }

    #[test]
    fn test_build_prompt_messages_codex_style_roles() {
        let config = AgentConfig {
            developer_instructions: Some("Developer rules".to_string()),
            ..AgentConfig::default()
        };
        let result = build_prompt_messages_with_skill_registry(
            &config,
            "Base prompt",
            Some("/tmp"),
            None,
            Some("User rules"),
        );

        assert_eq!(result.prefix[0].role, "system");
        assert_eq!(result.prefix[0].content.as_deref(), Some("Base prompt"));
        assert!(result
            .prefix
            .iter()
            .any(|message| message.role == "developer"
                && message
                    .content
                    .as_deref()
                    .is_some_and(|content| content.contains("Developer rules"))));
        assert!(result.prefix.iter().any(|message| message.role == "user"
            && message
                .content
                .as_deref()
                .is_some_and(|content| content.contains("User rules"))));
    }

    #[test]
    fn test_skill_metadata_budget_from_context() {
        let budget = SkillMetadataBudget::from_context_window(Some(100_000));
        assert!(matches!(budget, SkillMetadataBudget::Tokens(_)));

        let budget = SkillMetadataBudget::from_context_window(None);
        assert!(matches!(budget, SkillMetadataBudget::Characters(_)));
    }
}
