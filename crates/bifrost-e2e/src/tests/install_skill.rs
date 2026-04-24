use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use bifrost_cli::commands::handle_install_skill;

use crate::TestCase;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "install_skill_single_tool_to_temp_dir",
            "Install skill to a single tool (claude-code) in a temp directory",
            "install_skill",
            || async move {
                // Avoid flaky external network dependency in CI.
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_single")?;
                handle_install_skill(
                    Some("claude-code".to_string()),
                    Some(tmp.clone()),
                    false,
                    true,
                )
                .map_err(|e| format!("handle_install_skill failed: {e}"))?;

                let target = tmp.join("SKILL.md");
                if !target.exists() {
                    return Err(format!("Expected file not found: {}", target.display()));
                }

                let content = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read installed file: {e}"))?;

                if content.trim().is_empty() {
                    return Err("Installed file is empty".to_string());
                }

                if !content.contains("bifrost") && !content.contains("Bifrost") {
                    return Err(
                        "Installed file does not contain expected bifrost content".to_string()
                    );
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_overwrite_existing",
            "Install skill overwrites existing file with latest content",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_overwrite")?;

                let target = tmp.join("SKILL.md");
                fs::write(&target, "old content that should be replaced")
                    .map_err(|e| format!("Failed to write seed file: {e}"))?;

                handle_install_skill(Some("codex".to_string()), Some(tmp.clone()), false, true)
                    .map_err(|e| format!("handle_install_skill failed: {e}"))?;

                let new_content = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read overwritten file: {e}"))?;

                if new_content == "old content that should be replaced" {
                    return Err("File was NOT overwritten — still contains old content".to_string());
                }

                if new_content.trim().is_empty() {
                    return Err("Overwritten file is empty".to_string());
                }

                if !new_content.contains("bifrost") && !new_content.contains("Bifrost") {
                    return Err(
                        "Overwritten file does not contain expected bifrost content".to_string()
                    );
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_has_standard_frontmatter",
            "Installed SKILL.md should contain standard YAML frontmatter (name + description)",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_fm")?;

                handle_install_skill(Some("trae".to_string()), Some(tmp.clone()), false, true)
                    .map_err(|e| format!("handle_install_skill failed: {e}"))?;

                let target = tmp.join("SKILL.md");
                if !target.exists() {
                    return Err(format!("Expected file not found: {}", target.display()));
                }

                let content =
                    fs::read_to_string(&target).map_err(|e| format!("Failed to read file: {e}"))?;

                let normalized = content.replace("\r\n", "\n");
                if !normalized.starts_with("---\n") {
                    return Err("SKILL.md should start with YAML frontmatter (---)".to_string());
                }

                if !content.contains("name:") {
                    return Err(
                        "SKILL.md frontmatter should contain 'name' field for skill discovery"
                            .to_string(),
                    );
                }

                if !content.contains("description:") {
                    return Err(
                        "SKILL.md frontmatter should contain 'description' field for skill discovery"
                            .to_string(),
                    );
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_unknown_tool_error",
            "Unknown tool name returns a clear error",
            "install_skill",
            || async move {
                let tmp = tempdir("install_skill_unknown")?;

                let result = handle_install_skill(
                    Some("nonexistent-tool".to_string()),
                    Some(tmp.clone()),
                    false,
                    true,
                );

                match result {
                    Ok(()) => {
                        cleanup_dir(&tmp);
                        Err("Expected error for unknown tool, but got Ok".to_string())
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if !msg.contains("Unknown tool") {
                            cleanup_dir(&tmp);
                            return Err(format!(
                                "Error message should mention 'Unknown tool', got: {msg}"
                            ));
                        }
                        if !msg.contains("nonexistent-tool") {
                            cleanup_dir(&tmp);
                            return Err(format!(
                                "Error message should mention the invalid tool name, got: {msg}"
                            ));
                        }
                        cleanup_dir(&tmp);
                        Ok(())
                    }
                }
            },
        ),
        TestCase::standalone(
            "install_skill_all_tools_to_temp_dir",
            "Install skill to all tools writes SKILL.md for each",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_all")?;

                handle_install_skill(None, Some(tmp.clone()), false, true)
                    .map_err(|e| format!("handle_install_skill (all) failed: {e}"))?;

                let target = tmp.join("SKILL.md");
                if !target.exists() {
                    return Err(format!("Expected SKILL.md not found: {}", target.display()));
                }

                let content = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;

                if content.trim().is_empty() {
                    return Err("SKILL.md is empty".to_string());
                }

                if !content.contains("bifrost") && !content.contains("Bifrost") {
                    return Err("SKILL.md does not contain bifrost content".to_string());
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_cwd_mode",
            "Install skill with --cwd installs to project-local skills/bifrost/ directories",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_cwd")?;
                with_temp_cwd(&tmp, || {
                    handle_install_skill(Some("claude-code".to_string()), None, true, true)
                        .map_err(|e| format!("handle_install_skill --cwd failed: {e}"))
                })?;

                let target = tmp
                    .join(".claude")
                    .join("skills")
                    .join("bifrost")
                    .join("SKILL.md");
                if !target.exists() {
                    return Err(format!(
                        "Expected project-local file not found: {}",
                        target.display()
                    ));
                }

                let content = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read project-local file: {e}"))?;

                if content.trim().is_empty() {
                    return Err("Project-local installed file is empty".to_string());
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_cwd_codex_installs_legacy_and_universal_dirs",
            "Install skill with --cwd for codex writes both .codex and .agents skill directories",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_cwd_codex")?;
                with_temp_cwd(&tmp, || {
                    handle_install_skill(Some("codex".to_string()), None, true, true)
                        .map_err(|e| format!("handle_install_skill codex --cwd failed: {e}"))
                })?;

                let targets = [
                    tmp.join(".codex")
                        .join("skills")
                        .join("bifrost")
                        .join("SKILL.md"),
                    tmp.join(".agents")
                        .join("skills")
                        .join("bifrost")
                        .join("SKILL.md"),
                ];

                for target in &targets {
                    if !target.exists() {
                        return Err(format!(
                            "Expected project-local file not found: {}",
                            target.display()
                        ));
                    }

                    let content = fs::read_to_string(target)
                        .map_err(|e| format!("Failed to read project-local file: {e}"))?;
                    if content.trim().is_empty() {
                        return Err(format!("Installed file is empty: {}", target.display()));
                    }
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_github_copilot_cwd_mode",
            "Install skill with --cwd for GitHub Copilot writes to .github/skills/bifrost",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_cwd_copilot")?;
                with_temp_cwd(&tmp, || {
                    handle_install_skill(Some("github-copilot".to_string()), None, true, true)
                        .map_err(|e| {
                            format!("handle_install_skill github-copilot --cwd failed: {e}")
                        })
                })?;

                let target = tmp
                    .join(".github")
                    .join("skills")
                    .join("bifrost")
                    .join("SKILL.md");
                if !target.exists() {
                    return Err(format!(
                        "Expected GitHub Copilot project-local file not found: {}",
                        target.display()
                    ));
                }

                let content = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read GitHub Copilot file: {e}"))?;
                if content.trim().is_empty() {
                    return Err("GitHub Copilot installed file is empty".to_string());
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_dir_and_cwd_conflict",
            "--dir and --cwd are mutually exclusive",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_conflict")?;

                let result = handle_install_skill(
                    Some("claude-code".to_string()),
                    Some(tmp.clone()),
                    true,
                    true,
                );

                match result {
                    Ok(()) => {
                        cleanup_dir(&tmp);
                        Err("Expected error for --dir + --cwd conflict, but got Ok".to_string())
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if !msg.contains("mutually exclusive") {
                            cleanup_dir(&tmp);
                            return Err(format!(
                                "Error should mention 'mutually exclusive', got: {msg}"
                            ));
                        }
                        cleanup_dir(&tmp);
                        Ok(())
                    }
                }
            },
        ),
    ]
}

fn tempdir(prefix: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir()
        .join("bifrost-e2e-install-skill")
        .join(prefix);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .map_err(|e| format!("Failed to clean temp dir {}: {e}", dir.display()))?;
    }
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create temp dir {}: {e}", dir.display()))?;
    Ok(dir)
}

fn cwd_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_temp_cwd<F>(tmp: &PathBuf, op: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let _guard = cwd_test_lock()
        .lock()
        .map_err(|_| "Failed to acquire cwd test lock".to_string())?;
    let original_dir = std::env::current_dir().map_err(|e| format!("Failed to get cwd: {e}"))?;

    std::env::set_current_dir(tmp).map_err(|e| format!("Failed to set cwd: {e}"))?;
    let result = op();
    std::env::set_current_dir(&original_dir).map_err(|e| format!("Failed to restore cwd: {e}"))?;
    result
}

fn cleanup_dir(dir: &PathBuf) {
    let _ = fs::remove_dir_all(dir);
}
