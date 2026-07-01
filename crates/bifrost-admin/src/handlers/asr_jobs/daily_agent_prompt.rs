// ─── Daily Agent Prompt ──────────────────────────────────────────────────────

fn build_daily_agent_prompt(
    task: &AsrDirectoryTask,
    plan: &AsrDailyAgentChangePlan,
    adapter: &str,
    chatgpt_first_turn: bool,
) -> Result<String, String> {
    let work_dir = daily_agent_work_dir(task);
    let instructions_file = daily_agent_instructions_path(task)
        .strip_prefix(&work_dir)
        .unwrap_or_else(|_| Path::new("AGENTS.md"))
        .to_string_lossy()
        .to_string();
    let changed_entries: Vec<_> = plan
        .entries
        .iter()
        .filter(|e| e.change_kind != DailyAgentChangeKind::Unchanged)
        .collect();

    let is_file_capable = adapter != "chatgpt_web";
    let is_chatgpt_web = adapter == "chatgpt_web";
    let terms_file = daily_agent_terms_path(task)
        .strip_prefix(&work_dir)
        .unwrap_or_else(|_| Path::new(DAILY_AGENT_TERMS_FILENAME))
        .to_string_lossy()
        .to_string();
    let terms_content = read_daily_agent_terms_content(task);

    let mut prompt = String::new();
    if is_chatgpt_web {
        if let Some(terms_content) = &terms_content {
            prompt.push_str("## 专有名词配置（每次运行动态注入）\n\n```markdown\n");
            prompt.push_str(terms_content.trim());
            prompt.push_str("\n```\n\n---\n\n");
        }
    }

    if is_file_capable {
        if terms_content.is_some() {
            prompt.push_str(&format!(
                "请根据当前目录 `{instructions_file}` 和专有名词文件 `{terms_file}`，检查并处理以下变更文件：\n\n"
            ));
        } else {
            prompt.push_str(&format!(
                "请根据当前目录 `{instructions_file}`，检查并处理以下变更文件：\n\n"
            ));
        }
    } else {
        prompt.push_str("请根据以下 AGENTS.md 指令，对变更文件进行分析整理，直接以 Markdown 格式输出报告内容：\n\n");
    }

    for entry in &changed_entries {
        if is_file_capable {
            let source_path = daily_agent_source_copy_path(task, &entry.date)
                .strip_prefix(&work_dir)
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| entry.source_path.clone());
            let report_path = PathBuf::from(&entry.report_target)
                .strip_prefix(&work_dir)
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| entry.report_target.clone());
            prompt.push_str(&format!(
                "- source={}, change_kind={:?}, source_sha256={}, report={}\n",
                source_path,
                entry.change_kind,
                &entry.source_sha256[..8],
                report_path
            ));
        } else {
            prompt.push_str(&format!(
                "- {}.md: change_kind={:?}\n",
                entry.date, entry.change_kind,
            ));
        }
    }
    if is_file_capable {
        prompt.push_str("\n只刷新这些日期对应的 report。不要修改原始 YYYY-MM-DD.md。\n");
    } else {
        prompt.push_str(
            "\n请直接输出 Markdown 格式的报告。不需要创建文件，不需要代码块包裹，直接输出内容即可。\n",
        );
    }

    if is_chatgpt_web {
        // 每条消息都自带完成任务所需的全部信息：AGENTS.md 指令 + 已有日报完整内容 + 变更文件完整内容。
        let _ = chatgpt_first_turn;
        let response_contract = chatgpt_web_daily_agent_contract(
            &task.daily_agent.agent_id,
            &task.daily_agent.output_dir,
        );
        prompt.push_str(
            "\n本条消息已附带 AGENTS.md 指令、已有输出的完整内容，以及变更文件的完整内容。请在已有输出的基础上合并本轮新增或变更的内容，输出完整的最新正文。\n",
        );

        if response_contract == ChatGptWebDailyAgentContract::TomorrowTodo {
            prompt.push_str("\n## Tomorrow ToDo 日期规则（优先级高于已有输出基线）\n");
            for entry in &changed_entries {
                let target_date = tomorrow_todo_target_date(&entry.date);
                prompt.push_str(&format!(
                    "- 源转录日期 `{}` 的明日待办目标日期是 `{}`；最终标题必须是 `# 明日 To Do List - {}`。\n",
                    entry.date, target_date, target_date
                ));
                prompt.push_str(&format!(
                    "- 如果已有输出标题仍是 `# 明日 To Do List - {}`，必须替换为 `# 明日 To Do List - {}`，不要沿用旧标题。\n",
                    entry.date, target_date
                ));
            }
        }

        let agents_path = daily_agent_instructions_path(task);
        if let Ok(agents_content) = std::fs::read_to_string(&agents_path) {
            prompt.push_str("\n---\n## AGENTS.md 内容：\n\n```markdown\n");
            prompt.push_str(&agents_content);
            prompt.push_str("\n```\n");
        }

        prompt.push_str("\n---\n## 已有输出完整内容（如存在，作为合并基线，请在此基础上更新）：\n");
        for entry in &changed_entries {
            if let Ok(report_content) = std::fs::read_to_string(&entry.report_target) {
                prompt.push_str(&format!(
                    "\n### {}-report.md:\n\n```markdown\n{}\n```\n",
                    entry.date, report_content
                ));
            }
        }

        prompt.push_str("\n---\n## 变更文件完整内容（每次均为全量原文）：\n");
        for entry in &changed_entries {
            if let Ok(file_content) = std::fs::read_to_string(&entry.source_path) {
                // 始终发送完整文件内容；对于追加变更，原有内容与新增内容一并附上，
                // 仅额外标注新增部分的起始位置。
                let content_to_include = if entry.change_kind == DailyAgentChangeKind::Appended {
                    if let Some(offset) = entry.append_offset {
                        let offset = offset as usize;
                        if offset < file_content.len() && file_content.is_char_boundary(offset) {
                            format!(
                                "{}\n[以下为本次新增内容，从字节 {} 开始]\n{}",
                                &file_content[..offset],
                                offset,
                                &file_content[offset..]
                            )
                        } else {
                            file_content
                        }
                    } else {
                        file_content
                    }
                } else {
                    file_content
                };
                prompt.push_str(&format!(
                    "\n### {}.md ({:?}):\n\n```markdown\n{}\n```\n",
                    entry.date, entry.change_kind, content_to_include
                ));
            }
        }
    }

    Ok(prompt)
}

fn read_daily_agent_terms_content(task: &AsrDirectoryTask) -> Option<String> {
    let terms_path = daily_agent_terms_path(task);
    if let Ok(content) = std::fs::read_to_string(&terms_path) {
        return normalize_daily_agent_terminology(Some(content));
    }
    normalize_daily_agent_terminology(task.daily_agent.terminology.clone())
}
