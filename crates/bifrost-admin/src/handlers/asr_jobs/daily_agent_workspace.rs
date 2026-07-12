// ─── Daily Agent Workspace ───────────────────────────────────────────────────

const DAILY_AGENT_TERMS_REFERENCE_BEGIN: &str = "<!-- BIFROST_DAILY_AGENT_TERMS_BEGIN -->";
const DAILY_AGENT_TERMS_REFERENCE_END: &str = "<!-- BIFROST_DAILY_AGENT_TERMS_END -->";

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

fn is_daily_source_markdown_path(path: &Path) -> bool {
    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    path.is_file()
        && filename.ends_with(".md")
        && !filename.starts_with('.')
        && filename != "AGENTS.md"
        && is_valid_date_format(filename.trim_end_matches(".md"))
}

fn sync_daily_source_to_agent_work_dir(
    task: &AsrDirectoryTask,
    source_path: &Path,
) -> Result<(), String> {
    let Some(filename) = source_path.file_name() else {
        return Ok(());
    };
    let target_path = daily_agent_input_dir(task).join(filename);
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create agent source dir {}: {e}", parent.display()))?;
    }
    if daily_agent_source_copy_is_current(source_path, &target_path)? {
        return Ok(());
    }
    std::fs::copy(source_path, &target_path).map_err(|e| {
        format!(
            "copy daily source {} to {}: {e}",
            source_path.display(),
            target_path.display()
        )
    })?;
    Ok(())
}

fn daily_agent_source_copy_is_current(source_path: &Path, target_path: &Path) -> Result<bool, String> {
    if !target_path.exists() {
        return Ok(false);
    }
    let source_meta = std::fs::metadata(source_path)
        .map_err(|e| format!("read daily source metadata {}: {e}", source_path.display()))?;
    let target_meta = std::fs::metadata(target_path)
        .map_err(|e| format!("read agent source metadata {}: {e}", target_path.display()))?;
    if source_meta.len() != target_meta.len() {
        return Ok(false);
    }
    let source = std::fs::read(source_path)
        .map_err(|e| format!("read daily source {}: {e}", source_path.display()))?;
    let target = std::fs::read(target_path)
        .map_err(|e| format!("read agent source {}: {e}", target_path.display()))?;
    Ok(source == target)
}

fn sync_daily_sources_to_agent_workspaces(task: &AsrDirectoryTask) -> Result<(), String> {
    let daily_dir = daily_dir_for_task(&task.id);
    if !daily_dir.exists() {
        return Ok(());
    }
    let sources: Vec<PathBuf> = std::fs::read_dir(&daily_dir)
        .map_err(|e| format!("read daily source dir {}: {e}", daily_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_daily_source_markdown_path(path))
        .collect();

    for agent in normalized_daily_agents(&task.daily_agent) {
        let agent_task = task_for_daily_agent(task, &agent);
        for source_path in &sources {
            sync_daily_source_to_agent_work_dir(&agent_task, source_path)?;
        }
    }

    Ok(())
}

fn sync_daily_agent_plan_sources_to_work_dir(
    task: &AsrDirectoryTask,
    plan: &AsrDailyAgentChangePlan,
) -> Result<(), String> {
    for entry in &plan.entries {
        let source_path = Path::new(&entry.source_path);
        if source_path.exists() {
            sync_daily_source_to_agent_work_dir(task, source_path)?;
        }
    }
    Ok(())
}

fn sync_daily_agent_dependency_outputs(
    task: &AsrDirectoryTask,
    agent: &AsrDailyAgentItem,
    agents_by_id: &HashMap<String, AsrDailyAgentItem>,
    requested_date: Option<&str>,
) -> Result<Vec<String>, String> {
    let downstream_task = task_for_daily_agent(task, agent);
    let downstream_work_dir = daily_agent_work_dir(&downstream_task);
    let mut copied = Vec::new();

    for dependency in agent
        .dependencies
        .iter()
        .filter(|dependency| dependency.include_output)
    {
        let Some(upstream_agent) = agents_by_id.get(&dependency.agent_id) else {
            return Err(format!(
                "Daily Agent '{}' dependency '{}' is not configured",
                agent.id, dependency.agent_id
            ));
        };
        let upstream_task = task_for_daily_agent(task, upstream_agent);
        let source_dir = daily_agent_output_dir(&upstream_task);
        if !source_dir.exists() {
            continue;
        }
        let target_dir = daily_agent_upstream_input_dir(&downstream_task, &dependency.agent_id);
        std::fs::create_dir_all(&target_dir).map_err(|error| {
            format!(
                "create Daily Agent upstream input dir {}: {error}",
                target_dir.display()
            )
        })?;

        let mut entries = std::fs::read_dir(&source_dir)
            .map_err(|error| {
                format!(
                    "read Daily Agent dependency output dir {}: {error}",
                    source_dir.display()
                )
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        entries.sort();

        for source_path in entries {
            let Some(filename) = source_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(date) = filename.strip_suffix("-report.md") else {
                continue;
            };
            if !source_path.is_file()
                || !is_valid_date_format(date)
                || requested_date.is_some_and(|requested| requested != date)
            {
                continue;
            }
            let target_path = target_dir.join(filename);
            std::fs::copy(&source_path, &target_path).map_err(|error| {
                format!(
                    "copy Daily Agent dependency output {} to {}: {error}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
            copied.push(
                target_path
                    .strip_prefix(&downstream_work_dir)
                    .unwrap_or(&target_path)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    Ok(copied)
}

fn migrate_daily_agent_instructions_content(task: &AsrDirectoryTask, content: &str) -> String {
    if should_replace_legacy_generic_research_instructions(
        &task.daily_agent.agent_id,
        &task.daily_agent.instructions_source,
        content,
    ) {
        return daily_agent_instruction_content(task);
    }
    let report_dir = format!("./output/{}/", task.daily_agent.output_dir);
    content
        .replace(
            "原始按日转写文件位于当前目录，命名通常为 `YYYY-MM-DD.md`。",
            "原始按日转写文件副本位于当前目录的 `input/`，命名通常为 `input/YYYY-MM-DD.md`。",
        )
        .replace(
            "每日报告输出到当前目录下的 `report/` 文件夹，命名为 `YYYY-MM-DD-report.md`。",
            &format!(
                "每日报告输出到当前目录下的 `{}` 文件夹，命名为 `YYYY-MM-DD-report.md`。",
                report_dir.trim_end_matches('/')
            ),
        )
        .replace(
            "- 源文件是当前目录根部的 `YYYY-MM-DD.md`。",
            "- 源文件是当前目录下的 `input/YYYY-MM-DD.md`。",
        )
        .replace(
            "- 源文件是当前目录根部 `YYYY-MM-DD.md`。",
            "- 源文件是当前目录下的 `input/YYYY-MM-DD.md`。",
        )
        .replace(
            "- 默认输出目录是 `./report/`。",
            &format!("- 默认输出目录是 `{report_dir}`。"),
        )
        .replace(
            "- 默认输出目录是 `./tomorrow_todo/`。",
            &format!("- 默认输出目录是 `{report_dir}`。"),
        )
}

fn should_replace_legacy_generic_research_instructions(
    agent_id: &str,
    source: &AsrDailyAgentInstructionsSource,
    content: &str,
) -> bool {
    source == &AsrDailyAgentInstructionsSource::Default
        && matches!(
            agent_id,
            DEFAULT_RESEARCH_SEED_AGENT_ID | DEFAULT_RESEARCH_DISPATCHER_AGENT_ID
        )
        && content.trim_start().starts_with("# 全天候私人助理整理指南")
}

fn daily_agent_terms_reference_block() -> String {
    format!(
        "{DAILY_AGENT_TERMS_REFERENCE_BEGIN}\n## 专有名词配置\n\n运行前必须读取当前工作目录根目录下的 `{DAILY_AGENT_TERMS_FILENAME}`，并把其中的专有名词、缩写、项目名、人名或固定表达作为本次报告的优先参考。该路径是相对于当前 Agent 工作目录的相对文件路径。\n{DAILY_AGENT_TERMS_REFERENCE_END}\n"
    )
}

fn replace_managed_daily_agent_terms_reference(content: &str, replacement: Option<&str>) -> String {
    let Some(begin) = content.find(DAILY_AGENT_TERMS_REFERENCE_BEGIN) else {
        if let Some(replacement) = replacement {
            let mut next = content.trim_end().to_string();
            if !next.is_empty() {
                next.push_str("\n\n");
            }
            next.push_str(replacement);
            return next;
        }
        return content.to_string();
    };
    let Some(end_from_begin) = content[begin..].find(DAILY_AGENT_TERMS_REFERENCE_END) else {
        return content.to_string();
    };
    let end = begin + end_from_begin + DAILY_AGENT_TERMS_REFERENCE_END.len();
    let mut next = String::new();
    next.push_str(content[..begin].trim_end());
    if let Some(replacement) = replacement {
        if !next.is_empty() {
            next.push_str("\n\n");
        }
        next.push_str(replacement.trim_end());
    }
    let suffix = content[end..].trim_start_matches(['\r', '\n']);
    if !suffix.is_empty() {
        if !next.is_empty() {
            next.push_str("\n\n");
        }
        next.push_str(suffix);
    }
    next
}

fn ensure_daily_agent_terms_reference(content: &str, has_terms: bool) -> String {
    if has_terms {
        let block = daily_agent_terms_reference_block();
        replace_managed_daily_agent_terms_reference(content, Some(&block))
    } else {
        replace_managed_daily_agent_terms_reference(content, None)
    }
}

fn sync_daily_agent_terms_file(task: &AsrDirectoryTask) -> Result<bool, String> {
    let terms_path = daily_agent_terms_path(task);
    let Some(content) = normalize_daily_agent_terminology(task.daily_agent.terminology.clone())
    else {
        if terms_path.exists() {
            std::fs::remove_file(&terms_path)
                .map_err(|e| format!("remove Daily Agent terms {}: {e}", terms_path.display()))?;
        }
        return Ok(false);
    };
    let content = if content.ends_with('\n') {
        content
    } else {
        format!("{content}\n")
    };
    if let Ok(existing) = std::fs::read_to_string(&terms_path) {
        if existing == content {
            return Ok(true);
        }
    }
    std::fs::write(&terms_path, content.as_bytes())
        .map_err(|e| format!("write Daily Agent terms {}: {e}", terms_path.display()))?;
    Ok(true)
}

fn ensure_asr_daily_workspace(
    task: &AsrDirectoryTask,
) -> Result<AsrDailyWorkspaceStatus, String> {
    let daily_dir = daily_dir_for_task(&task.id);
    let legacy_task = task_for_daily_agent(task, &daily_agent_item_from_legacy(&task.daily_agent));
    let report_dir = daily_agent_output_dir(&legacy_task);
    let agents_path = daily_agent_instructions_path(&legacy_task);
    let gitignore_path = daily_dir.join(".gitignore");

    // Create directories
    std::fs::create_dir_all(&daily_dir).map_err(|e| format!("create daily dir: {e}"))?;
    std::fs::create_dir_all(daily_agent_instructions_dir(&task.id))
        .map_err(|e| format!("create agents dir: {e}"))?;

    for agent in normalized_daily_agents(&task.daily_agent) {
        let agent_task = task_for_daily_agent(task, &agent);
        let work_dir = daily_agent_work_dir(&agent_task);
        std::fs::create_dir_all(&work_dir).map_err(|e| format!("create agent work dir: {e}"))?;
        std::fs::create_dir_all(daily_agent_input_dir(&agent_task))
            .map_err(|e| format!("create agent input dir: {e}"))?;
        std::fs::create_dir_all(daily_agent_output_root_dir(&agent_task))
            .map_err(|e| format!("create agent output root dir: {e}"))?;
        let output_dir = daily_agent_output_dir(&agent_task);
        std::fs::create_dir_all(&output_dir).map_err(|e| format!("create output dir: {e}"))?;
        let has_terms = sync_daily_agent_terms_file(&agent_task)?;
        let instructions_path = daily_agent_instructions_path(&agent_task);
        if !instructions_path.exists() {
            let content = if agent_task.daily_agent.instructions_source
                == AsrDailyAgentInstructionsSource::Custom
            {
                agent_task.daily_agent.instructions.clone().unwrap_or_default()
            } else {
                daily_agent_instruction_content(&agent_task)
            };
            let content = ensure_daily_agent_terms_reference(&content, has_terms);
            std::fs::write(&instructions_path, content.as_bytes()).map_err(|e| {
                format!("write Daily Agent instructions {}: {e}", instructions_path.display())
            })?;
        } else if let Ok(content) = std::fs::read_to_string(&instructions_path) {
            let migrated = migrate_daily_agent_instructions_content(&agent_task, &content);
            let referenced = ensure_daily_agent_terms_reference(&migrated, has_terms);
            if referenced != content {
                std::fs::write(&instructions_path, referenced.as_bytes()).map_err(|e| {
                    format!(
                        "migrate Daily Agent instructions {}: {e}",
                        instructions_path.display()
                    )
                })?;
            }
        }
    }

    sync_daily_sources_to_agent_workspaces(task)?;

    let agents_exists = agents_path.exists();

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
