//! System prompt builder for the agent.
//!
//! Priority:
//! 1. Per-turn override prompt
//! 2. User-configured instructions (from config)
//! 3. Built-in prompt with environment info and tool guidance
//!
//! Additional context appended:
//! - AGENTS.md instructions
//! - Available skills descriptions

use crate::agents_md::AgentsMdManager;
use crate::config::{agent_home_dir, AgentConfig};
use crate::skills::SkillsManager;

/// Build the system prompt for the agent.
///
/// Priority:
/// 1. `override_prompt` (per-turn override)
/// 2. Built-in prompt + config instructions + AGENTS.md + skills
pub fn build_system_prompt(
    config: &AgentConfig,
    override_prompt: Option<&str>,
    work_dir_override: Option<&str>,
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

    // Append user instructions from config
    if let Some(ref instructions) = config.instructions {
        prompt.push_str("\n\n## User Instructions\n\n");
        prompt.push_str(instructions);
    }

    // Append AGENTS.md instructions
    let agents_md_manager = AgentsMdManager::new(config);
    let home_dir = agent_home_dir();
    if let Some(agents_instructions) =
        agents_md_manager.load_instructions(&work_dir, Some(&home_dir))
    {
        prompt.push_str("\n\n## Project Instructions\n\n");
        prompt.push_str(&agents_instructions);
    }

    // Append skills descriptions
    let skills_manager = SkillsManager::new(config.skills.clone());
    let skills = skills_manager.load_skills(&work_dir, Some(&home_dir));
    let skills_text = skills_manager.build_skills_instructions(&skills);
    if !skills_text.is_empty() {
        prompt.push_str(&skills_text);
    }

    prompt
}
