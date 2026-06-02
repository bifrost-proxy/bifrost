// ─── Daily Agent Workspace ───────────────────────────────────────────────────

/// Read workspace status without creating directories or files (for GET endpoint).
fn read_workspace_status(task: &AsrDirectoryTask) -> AsrDailyWorkspaceStatus {
    let daily_dir = daily_dir_for_task(&task.id);
    let legacy_task = task_for_daily_agent(task, &daily_agent_item_from_legacy(&task.daily_agent));
    let report_dir = daily_agent_output_dir(&legacy_task);
    let agents_path = daily_agent_instructions_path(&legacy_task);

    let agents_exists = agents_path.exists();
    let git_initialized = daily_dir.join(".git").exists();

    let agents = build_workspace_agent_statuses(task);
    let report_count = agents.iter().map(|agent| agent.report_count).sum();

    AsrDailyWorkspaceStatus {
        daily_dir: daily_dir.to_string_lossy().to_string(),
        report_dir: report_dir.to_string_lossy().to_string(),
        agents_path: agents_path.to_string_lossy().to_string(),
        agents_exists,
        git_available: true, // Assume available; actual check deferred to ensure_*
        git_initialized,
        git_error: None,
        report_count,
        agents,
    }
}

fn count_markdown_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "md")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn build_workspace_agent_statuses(task: &AsrDirectoryTask) -> Vec<AsrDailyWorkspaceAgentStatus> {
    normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .map(|agent| {
            let agent_task = task_for_daily_agent(task, &agent);
            let report_dir = daily_agent_output_dir(&agent_task);
            let instructions_path = daily_agent_instructions_path(&agent_task);
            AsrDailyWorkspaceAgentStatus {
                agent_id: agent.id,
                name: agent.name,
                output_dir: agent.output_dir,
                report_dir: report_dir.to_string_lossy().to_string(),
                instructions_path: instructions_path.to_string_lossy().to_string(),
                instructions_exists: instructions_path.exists(),
                report_count: count_markdown_files(&report_dir),
            }
        })
        .collect()
}

fn ensure_asr_daily_workspace(
    task: &AsrDirectoryTask,
) -> Result<AsrDailyWorkspaceStatus, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let legacy_task = task_for_daily_agent(task, &daily_agent_item_from_legacy(&task.daily_agent));
    let report_dir = daily_agent_output_dir(&legacy_task);
    let agents_path = daily_agent_instructions_path(&legacy_task);
    let instructions_dir = daily_agent_instructions_dir(&task.id);
    let gitignore_path = daily_dir.join(".gitignore");

    // Create directories
    std::fs::create_dir_all(&daily_dir).map_err(|e| format!("create daily dir: {e}"))?;
    std::fs::create_dir_all(&instructions_dir).map_err(|e| format!("create agents dir: {e}"))?;

    for agent in normalized_daily_agents(&task.daily_agent) {
        let agent_task = task_for_daily_agent(task, &agent);
        let output_dir = daily_agent_output_dir(&agent_task);
        std::fs::create_dir_all(&output_dir).map_err(|e| format!("create output dir: {e}"))?;
        let instructions_path = daily_agent_instructions_path(&agent_task);
        if !instructions_path.exists() {
            let content = if agent_task.daily_agent.instructions_source
                == AsrDailyAgentInstructionsSource::Custom
            {
                agent_task.daily_agent.instructions.clone().unwrap_or_default()
            } else {
                daily_agent_instruction_content(&agent_task)
            };
            std::fs::write(&instructions_path, content.as_bytes()).map_err(|e| {
                format!("write Daily Agent instructions {}: {e}", instructions_path.display())
            })?;
        }
    }

    // Write AGENTS.md if not exists
    let agents_exists = if agents_path.exists() {
        true
    } else {
        let content =
            if task.daily_agent.instructions_source == AsrDailyAgentInstructionsSource::Custom {
                task.daily_agent.instructions.clone().unwrap_or_default()
            } else {
                daily_agent_instruction_content(&legacy_task)
            };
        std::fs::write(&agents_path, content.as_bytes())
            .map_err(|e| format!("write AGENTS.md: {e}"))?;
        true
    };

    // Write .gitignore if not exists
    if !gitignore_path.exists() {
        let _ = std::fs::write(&gitignore_path, ".DS_Store\n");
    }

    // Git init (best-effort)
    let (git_available, git_initialized, git_error) = try_git_init(&daily_dir);

    // Count reports
    let agents = build_workspace_agent_statuses(task);
    let report_count = agents.iter().map(|agent| agent.report_count).sum();

    let status = AsrDailyWorkspaceStatus {
        daily_dir: daily_dir.to_string_lossy().to_string(),
        report_dir: report_dir.to_string_lossy().to_string(),
        agents_path: agents_path.to_string_lossy().to_string(),
        agents_exists,
        git_available,
        git_initialized,
        git_error,
        report_count,
        agents,
    };

    tracing::info!(
        task_id = %task.id,
        daily_dir = %status.daily_dir,
        git_initialized = status.git_initialized,
        "initialized ASR daily agent workspace"
    );

    Ok(status)
}

fn try_git_init(daily_dir: &Path) -> (bool, bool, Option<String>) {
    // Check if already initialized (no need to run git --version)
    if daily_dir.join(".git").exists() {
        return (true, true, None);
    }

    // Try git init (implicitly checks if git is available)
    let result = std::process::Command::new("git")
        .arg("init")
        .current_dir(daily_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => (true, true, None),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            tracing::warn!(daily_dir = %daily_dir.display(), error = %stderr, "git init failed");
            (true, false, Some(stderr))
        }
        Err(e) => {
            let is_not_found = e.kind() == std::io::ErrorKind::NotFound;
            if is_not_found {
                (false, false, Some("git executable not found".to_string()))
            } else {
                tracing::warn!(daily_dir = %daily_dir.display(), error = %e, "git init failed");
                (true, false, Some(e.to_string()))
            }
        }
    }
}

fn try_git_commit(daily_dir: &Path, message: &str) -> Option<String> {
    if !daily_dir.join(".git").exists() {
        return None;
    }

    // git add daily sources, per-agent instructions/output directories, and .gitignore.
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(daily_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // git commit
    let result = std::process::Command::new("git")
        .args(["commit", "-m", message, "--allow-empty-message"])
        .current_dir(daily_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => {
            tracing::debug!(daily_dir = %daily_dir.display(), "git commit succeeded");
            // Capture the commit hash
            let hash_output = std::process::Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .current_dir(daily_dir)
                .output();
            hash_output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // "nothing to commit" is not a real error
            if !stderr.contains("nothing to commit") {
                tracing::warn!(daily_dir = %daily_dir.display(), error = %stderr, "git commit failed");
            }
            None
        }
        Err(e) => {
            tracing::warn!(daily_dir = %daily_dir.display(), error = %e, "git commit failed");
            None
        }
    }
}
