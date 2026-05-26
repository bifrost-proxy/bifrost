// ─── Daily Agent Workspace ───────────────────────────────────────────────────

/// Read workspace status without creating directories or files (for GET endpoint).
fn read_workspace_status(task: &AsrDirectoryTask) -> AsrDailyWorkspaceStatus {
    let daily_dir = daily_dir_for_task(&task.id);
    let report_dir = daily_dir.join("report");
    let agents_path = daily_dir.join("AGENTS.md");

    let agents_exists = agents_path.exists();
    let git_initialized = daily_dir.join(".git").exists();

    let report_count = std::fs::read_dir(&report_dir)
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
        .unwrap_or(0);

    AsrDailyWorkspaceStatus {
        daily_dir: daily_dir.to_string_lossy().to_string(),
        report_dir: report_dir.to_string_lossy().to_string(),
        agents_path: agents_path.to_string_lossy().to_string(),
        agents_exists,
        git_available: true, // Assume available; actual check deferred to ensure_*
        git_initialized,
        git_error: None,
        report_count,
    }
}

fn ensure_asr_daily_workspace(
    task: &AsrDirectoryTask,
) -> Result<AsrDailyWorkspaceStatus, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let report_dir = daily_dir.join("report");
    let agents_path = daily_dir.join("AGENTS.md");
    let gitignore_path = daily_dir.join(".gitignore");

    // Create directories
    std::fs::create_dir_all(&daily_dir).map_err(|e| format!("create daily dir: {e}"))?;
    std::fs::create_dir_all(&report_dir).map_err(|e| format!("create report dir: {e}"))?;

    // Write AGENTS.md if not exists
    let agents_exists = if agents_path.exists() {
        true
    } else {
        let content =
            if task.daily_agent.instructions_source == AsrDailyAgentInstructionsSource::Custom {
                task.daily_agent.instructions.clone().unwrap_or_default()
            } else {
                DEFAULT_ASR_DAILY_AGENTS_MD
                    .replace("{{task_name}}", &task.name)
                    .replace("{{daily_dir}}", ".")
                    .replace("{{report_dir}}", "./report/")
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
    let report_count = std::fs::read_dir(&report_dir)
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
        .unwrap_or(0);

    let status = AsrDailyWorkspaceStatus {
        daily_dir: daily_dir.to_string_lossy().to_string(),
        report_dir: report_dir.to_string_lossy().to_string(),
        agents_path: agents_path.to_string_lossy().to_string(),
        agents_exists,
        git_available,
        git_initialized,
        git_error,
        report_count,
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

    // git add *.md report/ .gitignore (track daily source files too)
    let _ = std::process::Command::new("git")
        .args(["add", "*.md", "report/", ".gitignore"])
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
