//! System prompt builder for the agent.
//!
//! Priority:
//! 1. Per-turn override prompt
//! 2. Built-in prompt with environment info and tool guidance
//!
//! Additional context appended:
//! - Pre-loaded user instructions (AGENTS.md + config instructions, loaded once at session creation)
//! - Available skills descriptions

use crate::config::AgentConfig;
use crate::skills::SkillsManager;
use bifrost_skills::SkillRegistry;

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

pub fn build_system_prompt_with_skill_registry(
    config: &AgentConfig,
    override_prompt: Option<&str>,
    work_dir_override: Option<&str>,
    registry: Option<&SkillRegistry>,
    user_instructions: Option<&str>,
) -> String {
    if let Some(custom) = override_prompt {
        return custom.to_string();
    }

    let work_dir = work_dir_override
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.resolve_work_dir());
    let os_info = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    // Detect current shell
    let shell = std::env::var("SHELL")
        .ok()
        .and_then(|s| s.rsplit('/').next().map(|n| n.to_string()))
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "zsh".to_string()
            } else {
                "bash".to_string()
            }
        });

    let mut prompt = format!(
        r#"You are Bifrost Agent, an AI coding assistant with the ability to execute shell commands and manage files.

## Environment
- Operating System: {os_info} ({arch})
- Working Directory: {work_dir}
- Shell: {shell}

## Available Tools

### shell
Execute any shell command. The command runs in the working directory via `{shell} -lc`.
Use for: running scripts, installing packages, compilation, git operations, searching code (`rg` for grep, `rg --files` for find).

### write_file
Write content to a file. Creates parent directories automatically. Overwrites existing files.
Use for: creating new files, writing complete file content.

### read_file
Read file contents with optional line offset and limit.
Use for: inspecting files before modifying, reading specific sections of large files.

### apply_patch
Apply a precise edit to a file by replacing exact text (search-and-replace).
Use for: modifying existing files without rewriting the entire content. More precise than write_file for edits.
**Prefer this over write_file when modifying existing files.**

### list_directory
List files and directories in a given path.
Use for: exploring project structure, finding files.

### switch_workdir
Switch the working directory of the current session. Clears conversation history and reloads project configuration.
Use for: switching to a different project directory when the user explicitly requests it.

### update_plan
Update the task plan (TODO/checklist) to track progress on multi-step tasks.
Each step has a status: `pending`, `in_progress`, or `completed`.
The plan is displayed to the user as a progress card. Using the tool helps demonstrate that you've understood the task and convey how you're approaching it.

A good plan should break the task into meaningful, logically ordered steps that are easy to verify as you go.
Plans are not for padding out simple work with filler steps or stating the obvious. The content of your plan should not involve doing anything that you aren't capable of doing. Do not use plans for simple or single-step queries that you can just do or answer immediately.

To create a new plan, call `update_plan` with a short list of 1-sentence steps (no more than 5-7 words each) with a `status` for each step (`pending`, `in_progress`, or `completed`).

When steps have been completed, use `update_plan` to mark each finished step as `completed` and the next step you are working on as `in_progress`. There should always be exactly one `in_progress` step until everything is done. You can mark multiple items as complete in a single `update_plan` call.

If all steps are complete, ensure you call `update_plan` to mark all steps as `completed`.

Do not repeat the full contents of the plan after an `update_plan` call — the harness already displays it. Instead, summarize the change made and highlight any important context or next step.

Before running a command, consider whether or not you have completed the previous step, and make sure to mark it as completed before moving on to the next step. It may be the case that you complete all steps in your plan after a single pass of implementation. If this is the case, you can simply mark all the planned steps as completed. Sometimes, you may need to change plans in the middle of a task: call `update_plan` with the updated plan and make sure to provide an `explanation` of the rationale when doing so.

Use a plan when:
- The task is non-trivial and will require multiple actions over a long time horizon.
- There are logical phases or dependencies where sequencing matters.
- The work has ambiguity that benefits from outlining high-level goals.
- You want intermediate checkpoints for feedback and validation.
- When the user asked you to do more than one thing in a single prompt.
- The user has asked you to use the plan tool (aka "TODOs").
- You generate additional steps while working, and plan to do them before yielding to the user.

High-quality plan examples:
- 1. Add CLI entry with file args  2. Parse Markdown via CommonMark library  3. Apply semantic HTML template  4. Handle code blocks, images, links  5. Add error handling for invalid files
- 1. Define CSS variables for colors  2. Add toggle with localStorage state  3. Refactor components to use variables  4. Verify all views for readability  5. Add smooth theme-change transition

Low-quality plan examples (avoid these):
- 1. Create CLI tool  2. Add Markdown parser  3. Convert to HTML
- 1. Add dark mode toggle  2. Save preference  3. Make styles look good

If you need to write a plan, only write high quality plans, not low quality ones.

## Guidelines
- Always read a file before modifying it (use `read_file` or `shell` with `cat`).
- For file modifications, prefer `apply_patch` over `write_file` to make precise edits.
- Verify your work by checking command output or reading modified files.
- If a task requires multiple steps, plan and execute them sequentially.
- When writing scripts, set proper permissions (`chmod +x`) before executing.
- Report results clearly and concisely.
- If a command fails, analyze the error and try to fix it.
- For searching code, use `rg` (ripgrep) or `rg --files` instead of `grep`/`find`.
- For coding tasks, write clean and well-structured code."#,
        work_dir = work_dir.display(),
        shell = shell,
    );

    // Append pre-loaded user instructions (AGENTS.md + config instructions,
    // resolved once at session creation, matching Codex's behavior).
    if let Some(instructions) = user_instructions {
        prompt.push_str("\n\n## Project Instructions\n\n");
        prompt.push_str(instructions);
    }

    // Append skills instructions (loaded via SkillsManager from all scopes).
    let skills_manager = SkillsManager::new(config.skills.clone());
    let skills = skills_manager.load_skills(&work_dir);
    let skills_text = skills_manager.build_skills_instructions(&skills);
    if !skills_text.is_empty() {
        prompt.push_str(&skills_text);
    }

    if let Some(registry) = registry {
        let digest = build_skill_registry_digest(registry);
        if !digest.is_empty() {
            prompt.push_str(&digest);
        }
    }

    prompt
}

const SKILL_DIGEST_CHAR_LIMIT: usize = 4 * 1024;

fn build_skill_registry_digest(registry: &SkillRegistry) -> String {
    let mut skills = registry.enabled();
    if skills.is_empty() {
        return String::new();
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    let mut output = String::from("\n\n## Available Skills\n\n");
    let mut hidden = 0usize;
    for (index, skill) in skills.iter().enumerate() {
        let description = skill
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let line = format!("- {}: {}\n", skill.name, description);
        if output.len() + line.len() > SKILL_DIGEST_CHAR_LIMIT {
            hidden = skills.len() - index;
            break;
        }
        output.push_str(&line);
    }
    if hidden > 0 {
        let more = format!("- ... ({hidden} more hidden)\n");
        if output.len() + more.len() > SKILL_DIGEST_CHAR_LIMIT {
            output.truncate(SKILL_DIGEST_CHAR_LIMIT.saturating_sub(more.len()));
        }
        output.push_str(&more);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_skills::{ScopeRoot, SkillDraft, SkillManifest, SkillScope, SkillStore};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn system_prompt_includes_bounded_skill_registry_digest() {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        for name in ["alpha", "beta", "gamma"] {
            let manifest = SkillManifest::minimal_inline(name, name, SkillScope::Repo);
            store
                .commit(SkillDraft {
                    manifest,
                    skill_md: format!("---\nname: {name}\ndescription: {name}\n---\n# {name}"),
                    draft_dir: None,
                    assets: Vec::new(),
                })
                .unwrap();
        }
        let registry = SkillRegistry::without_watcher(store).unwrap();

        let prompt = build_system_prompt_with_skill_registry(
            &AgentConfig::default(),
            None,
            Some(dir.path().to_str().unwrap()),
            Some(&registry),
            None, // no user instructions in this test
        );

        assert!(prompt.contains("## Available Skills"));
        assert!(prompt.contains("- alpha: alpha"));
        assert!(prompt.contains("- beta: beta"));
        assert!(prompt.contains("- gamma: gamma"));
    }
}
