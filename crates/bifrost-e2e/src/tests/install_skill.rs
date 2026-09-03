use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use bifrost_cli::commands::handle_install_skill;

use crate::TestCase;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "install_skill_single_tool_to_temp_dir",
            "Install skill to a single supported target in a temp directory",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_single")?;
                handle_install_skill(
                    Some("claude-code".to_string()),
                    Some(tmp.clone()),
                    false,
                    true,
                )
                .map_err(|e| format!("handle_install_skill failed: {e}"))?;

                assert_installed_skill(&tmp.join("SKILL.md"))?;
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

                handle_install_skill(
                    Some("universal".to_string()),
                    Some(tmp.clone()),
                    false,
                    true,
                )
                .map_err(|e| format!("handle_install_skill failed: {e}"))?;

                let new_content = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read overwritten file: {e}"))?;
                if new_content == "old content that should be replaced" {
                    return Err("File was NOT overwritten — still contains old content".to_string());
                }
                assert_skill_content(&new_content)?;

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_has_standard_frontmatter",
            "Installed SKILL.md should contain standard YAML frontmatter",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_fm")?;

                handle_install_skill(
                    Some("universal".to_string()),
                    Some(tmp.clone()),
                    false,
                    true,
                )
                .map_err(|e| format!("handle_install_skill failed: {e}"))?;

                let target = tmp.join("SKILL.md");
                let content =
                    fs::read_to_string(&target).map_err(|e| format!("Failed to read file: {e}"))?;
                let normalized = content.replace("\r\n", "\n");
                if !normalized.starts_with("---\n") {
                    return Err("SKILL.md should start with YAML frontmatter (---)".to_string());
                }
                if !content.contains("name:") {
                    return Err("SKILL.md frontmatter should contain 'name'".to_string());
                }
                if !content.contains("description:") {
                    return Err("SKILL.md frontmatter should contain 'description'".to_string());
                }

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_legacy_targets_rejected",
            "Legacy vendor-specific skill targets are rejected",
            "install_skill",
            || async move {
                let tmp = tempdir("install_skill_legacy")?;
                for legacy in ["codex", "trae", "cursor", "github-copilot"] {
                    let result = handle_install_skill(
                        Some(legacy.to_string()),
                        Some(tmp.clone()),
                        false,
                        true,
                    );
                    match result {
                        Ok(()) => {
                            cleanup_dir(&tmp);
                            return Err(format!(
                                "Expected legacy target '{legacy}' to be rejected"
                            ));
                        }
                        Err(e) if e.to_string().contains("Unknown tool") => {}
                        Err(e) => {
                            cleanup_dir(&tmp);
                            return Err(format!(
                                "Legacy target '{legacy}' returned unexpected error: {e}"
                            ));
                        }
                    }
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
                        if !msg.contains("Unknown tool") || !msg.contains("nonexistent-tool") {
                            cleanup_dir(&tmp);
                            return Err(format!("Unexpected error message: {msg}"));
                        }
                        cleanup_dir(&tmp);
                        Ok(())
                    }
                }
            },
        ),
        TestCase::standalone(
            "install_skill_all_targets_to_temp_dir",
            "Default install writes the skill for both supported targets",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_all")?;

                handle_install_skill(None, Some(tmp.clone()), false, true)
                    .map_err(|e| format!("handle_install_skill (all) failed: {e}"))?;

                assert_installed_skill(&tmp.join("SKILL.md"))?;
                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_cwd_claude_mode",
            "Project-local Claude install writes .claude/skills/bifrost",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_cwd_claude")?;
                with_temp_cwd(&tmp, || {
                    handle_install_skill(Some("claude-code".to_string()), None, true, true)
                        .map_err(|e| format!("handle_install_skill --cwd failed: {e}"))
                })?;

                let target = tmp
                    .join(".claude")
                    .join("skills")
                    .join("bifrost")
                    .join("SKILL.md");
                assert_installed_skill(&target)?;

                cleanup_dir(&tmp);
                Ok(())
            },
        ),
        TestCase::standalone(
            "install_skill_cwd_universal_mode",
            "Project-local universal install writes .agents/skills/bifrost",
            "install_skill",
            || async move {
                std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");
                let tmp = tempdir("install_skill_cwd_universal")?;
                with_temp_cwd(&tmp, || {
                    handle_install_skill(Some("universal".to_string()), None, true, true)
                        .map_err(|e| format!("handle_install_skill universal --cwd failed: {e}"))
                })?;

                let target = tmp
                    .join(".agents")
                    .join("skills")
                    .join("bifrost")
                    .join("SKILL.md");
                assert_installed_skill(&target)?;
                let client_skill = fs::read_to_string(&target)
                    .map_err(|e| format!("Failed to read Client skill: {e}"))?;
                assert_client_skill_content(&client_skill)?;

                let remote_target = tmp
                    .join(".agents")
                    .join("skills")
                    .join("bifrost-remote")
                    .join("SKILL.md");
                assert_installed_skill(&remote_target)?;
                let remote_skill = fs::read_to_string(&remote_target)
                    .map_err(|e| format!("Failed to read Remote Invoke skill: {e}"))?;
                assert_remote_skill_content(&remote_skill)?;

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

fn assert_installed_skill(target: &PathBuf) -> Result<(), String> {
    if !target.exists() {
        return Err(format!("Expected file not found: {}", target.display()));
    }
    let content = fs::read_to_string(target)
        .map_err(|e| format!("Failed to read installed file {}: {e}", target.display()))?;
    assert_skill_content(&content)
}

fn assert_skill_content(content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("Installed skill is empty".to_string());
    }
    if !content.contains("bifrost") && !content.contains("Bifrost") {
        return Err("Installed skill does not contain expected bifrost content".to_string());
    }
    Ok(())
}

fn assert_client_skill_content(content: &str) -> Result<(), String> {
    for required in [
        "bifrost client target add",
        "bifrost client target login",
        "bifrost client --target devbox traffic list",
        "bifrost client --target devbox rule list",
        "不得改走本机数据目录或自动降级到 Remote Invoke",
    ] {
        if !content.contains(required) {
            return Err(format!(
                "Installed bifrost skill is missing Client guidance: {required}"
            ));
        }
    }
    Ok(())
}

fn assert_remote_skill_content(content: &str) -> Result<(), String> {
    for required in [
        "先选择正确的远程模式",
        "通用 `bifrost` skill 的 `bifrost client`",
        "不得自动改用 `remote exec`",
    ] {
        if !content.contains(required) {
            return Err(format!(
                "Installed bifrost-remote skill is missing mode boundary: {required}"
            ));
        }
    }
    Ok(())
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
