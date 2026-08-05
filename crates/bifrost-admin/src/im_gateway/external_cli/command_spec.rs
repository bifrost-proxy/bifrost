use super::*;

pub(super) fn build_command_spec(
    request: &ExternalCliRunRequest,
    last_message_path: &Path,
) -> Result<CommandSpec, String> {
    if request.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        return Ok(CommandSpec {
            executable: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
            args: vec![request.operation.clone()],
            env: BTreeMap::new(),
            work_dir: None,
            timeout_secs: request.adapter_config.timeout_secs.or(Some(7200)),
        });
    }
    let config = &request.adapter_config;
    let executable = config
        .executable
        .clone()
        .unwrap_or_else(|| default_executable_for_adapter(&request.adapter).to_string());
    let mut args = config.args.clone();

    if request.adapter == DEFAULT_ADAPTER {
        if args.is_empty() {
            if codex_thread_id_from_params(request).is_some() {
                args = vec![
                    "exec".to_string(),
                    "resume".to_string(),
                    "--json".to_string(),
                ];
            } else {
                args = vec!["exec".to_string(), "--json".to_string()];
            }
        }
        if let Some(work_dir) = request.work_dir.as_ref() {
            ensure_codex_work_dir_arg(&mut args, work_dir);
        }
        ensure_codex_last_message_arg(&mut args, last_message_path);
        if !config.args.is_empty() {
            let is_resume = codex_thread_id_from_params(request).is_some();
            append_codex_config_args(&mut args, config, is_resume);
        }
        if config.args.is_empty() {
            let resume_thread_id = codex_thread_id_from_params(request);
            let is_resume = resume_thread_id.is_some();
            append_codex_config_args(&mut args, config, is_resume);
            if let Some(thread_id) = resume_thread_id {
                args.push(thread_id);
            }
            args.push("-".to_string());
        }
    } else if request.adapter == TRAEX_ADAPTER {
        if args.is_empty() {
            if codex_thread_id_from_params(request).is_some() {
                args = vec![
                    "exec".to_string(),
                    "resume".to_string(),
                    "--json".to_string(),
                ];
            } else {
                args = vec!["exec".to_string(), "--json".to_string()];
            }
        }
        if let Some(work_dir) = request.work_dir.as_ref() {
            ensure_traex_work_dir_arg(&mut args, work_dir);
        }
        ensure_codex_last_message_arg(&mut args, last_message_path);
        if !config.args.is_empty() {
            let is_resume = codex_thread_id_from_params(request).is_some();
            append_traex_config_args(&mut args, config, is_resume);
        }
        if config.args.is_empty() {
            let resume_thread_id = codex_thread_id_from_params(request);
            let is_resume = resume_thread_id.is_some();
            append_traex_config_args(&mut args, config, is_resume);
            if let Some(thread_id) = resume_thread_id {
                args.push(thread_id);
            }
            args.push("-".to_string());
        }
    } else if request.adapter == CLAUDE_CODE_ADAPTER {
        if args.is_empty() {
            let input_format =
                if app_server::resolved_transport(request)? == ExternalCliTransport::StreamJson {
                    "stream-json"
                } else {
                    "text"
                };
            args = vec![
                "-p".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--input-format".to_string(),
                input_format.to_string(),
            ];
            if input_format == "stream-json" {
                args.push("--replay-user-messages".to_string());
            }
        }
        if !config.args.is_empty() {
            append_claude_code_config_args(&mut args, config);
        }
        if config.args.is_empty() {
            append_claude_code_config_args(&mut args, config);
            if let Some(session_id) = claude_code_session_id_from_params(request) {
                args.push("--resume".to_string());
                args.push(session_id);
            }
        }
    } else if config.executable.is_none() && args.is_empty() {
        return Err(format!(
            "adapter '{}' requires explicit adapterConfig.args",
            request.adapter
        ));
    }

    Ok(CommandSpec {
        executable,
        args,
        env: config.env.clone(),
        work_dir: request.work_dir.clone(),
        timeout_secs: config.timeout_secs,
    })
}

fn append_claude_code_config_args(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    remove_overridden_claude_code_args(args, config);
    let mut generated = Vec::new();
    if let Some(model) = config.model.as_deref() {
        generated.push("--model".to_string());
        generated.push(model.to_string());
    }
    let danger_full_access = effective_claude_code_danger_full_access(config);
    if let Some(permission_mode) =
        effective_claude_code_permission_mode(config).filter(|_| !danger_full_access)
    {
        generated.push("--permission-mode".to_string());
        generated.push(permission_mode);
    }
    if danger_full_access {
        generated.push("--dangerously-skip-permissions".to_string());
    }
    if let Some(effort) = config.reasoning_effort.as_deref().map(str::trim) {
        if !effort.is_empty() {
            generated.push("--effort".to_string());
            generated.push(effort.to_string());
        }
    }
    append_repeatable_codex_pairs(&mut generated, "--add-dir", &config.add_dirs);
    insert_codex_args_before_stdin(args, generated);
}

fn default_executable_for_adapter(adapter: &str) -> &str {
    match adapter {
        CLAUDE_CODE_ADAPTER => "claude",
        other => other,
    }
}

fn effective_claude_code_permission_mode(config: &ExternalCliAdapterConfig) -> Option<String> {
    let value = config.permission_mode.as_deref().map(str::trim)?;
    if value.is_empty() || value == "default" {
        return None;
    }
    Some(
        match value {
            "accept_edits" | "accept-edits" => "acceptEdits",
            "bypass_permissions" | "bypass-permissions" => "bypassPermissions",
            "dont_ask" | "dont-ask" => "dontAsk",
            other => other,
        }
        .to_string(),
    )
}

fn effective_claude_code_danger_full_access(config: &ExternalCliAdapterConfig) -> bool {
    config
        .danger_full_access
        .unwrap_or_else(|| effective_claude_code_permission_mode(config).is_none())
}

fn remove_overridden_claude_code_args(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    if config.model.is_some() {
        remove_codex_arg_with_value(args, "--model");
    }
    remove_codex_arg_with_value(args, "--permission-mode");
    if config.reasoning_effort.is_some() {
        remove_codex_arg_with_value(args, "--effort");
    }
}

fn append_traex_config_args(
    args: &mut Vec<String>,
    config: &ExternalCliAdapterConfig,
    is_resume: bool,
) {
    remove_overridden_traex_args(args, config);
    let permission_mode = effective_traex_permission_mode(config);
    let danger_full_access = effective_traex_danger_full_access(config, permission_mode.as_deref());
    if danger_full_access {
        remove_codex_arg_with_value(args, "--sandbox");
    }
    let mut generated = Vec::new();
    append_repeatable_codex_pairs(&mut generated, "--config", &config.config_overrides);
    append_repeatable_codex_pairs(&mut generated, "--enable", &config.enable_features);
    append_repeatable_codex_pairs(&mut generated, "--disable", &config.disable_features);
    if let Some(profile) = config.profile.as_deref() {
        if !is_resume {
            generated.push("--profile".to_string());
            generated.push(profile.to_string());
        }
    }
    if let Some(model) = config.model.as_deref() {
        generated.push("--model".to_string());
        generated.push(model.to_string());
    }
    append_codex_reasoning_config(&mut generated, config);
    if let Some(permission_mode) = permission_mode.filter(|_| !danger_full_access) {
        generated.push("--permission-mode".to_string());
        generated.push(permission_mode);
    }
    if let Some(sandbox) = config.sandbox.as_deref() {
        if !is_resume && !danger_full_access {
            generated.push("--sandbox".to_string());
            generated.push(sandbox.to_string());
        }
    }
    if danger_full_access {
        generated.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    }
    if config.skip_git_repo_check.unwrap_or(false) {
        generated.push("--skip-git-repo-check".to_string());
    }
    if config.ignore_user_config.unwrap_or(false) {
        generated.push("--ignore-user-config".to_string());
    }
    if config.ignore_rules.unwrap_or(false) {
        generated.push("--ignore-rules".to_string());
    }
    if !is_resume {
        append_repeatable_codex_pairs(&mut generated, "--add-dir", &config.add_dirs);
        if let Some(output_schema) = config.output_schema.as_deref().map(str::trim) {
            if !output_schema.is_empty() {
                generated.push("--output-schema".to_string());
                generated.push(output_schema.to_string());
            }
        }
        if let Some(color) = config.color.as_deref().map(str::trim) {
            if !color.is_empty() {
                generated.push("--color".to_string());
                generated.push(color.to_string());
            }
        }
    }
    if config.ephemeral.unwrap_or(false) {
        generated.push("--ephemeral".to_string());
    }
    insert_codex_args_before_stdin(args, generated);
}

fn effective_traex_permission_mode(config: &ExternalCliAdapterConfig) -> Option<String> {
    let configured = config.permission_mode.as_deref().map(str::trim);
    match configured {
        Some(value) if !value.is_empty() && value != "default" => Some(value.to_string()),
        _ => Some("bypass_permissions".to_string()),
    }
}

fn effective_traex_danger_full_access(
    config: &ExternalCliAdapterConfig,
    permission_mode: Option<&str>,
) -> bool {
    config
        .danger_full_access
        .unwrap_or_else(|| permission_mode == Some("bypass_permissions"))
}

fn remove_overridden_traex_args(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    if config.profile.is_some() {
        remove_codex_arg_with_value(args, "--profile");
    }
    if config.model.is_some() {
        remove_codex_arg_with_value(args, "--model");
    }
    remove_codex_arg_with_value(args, "--permission-mode");
    if config.sandbox.is_some() {
        remove_codex_arg_with_value(args, "--sandbox");
    }
    if config.output_schema.is_some() {
        remove_codex_arg_with_value(args, "--output-schema");
    }
    if config.color.is_some() {
        remove_codex_arg_with_value(args, "--color");
    }
    if config.reasoning_effort.is_some() || config.reasoning_summary.is_some() {
        remove_codex_config_with_key(args, "model_reasoning_effort");
        remove_codex_config_with_key(args, "reasoning_effort");
        remove_codex_config_with_key(args, "model_reasoning_summary");
        remove_codex_config_with_key(args, "reasoning_summary");
    }
}

fn append_codex_config_args(
    args: &mut Vec<String>,
    config: &ExternalCliAdapterConfig,
    is_resume: bool,
) {
    remove_overridden_codex_args(args, config);
    let danger_full_access = effective_codex_danger_full_access(config);
    if danger_full_access {
        remove_codex_arg_with_value(args, "--sandbox");
    }
    let mut generated = Vec::new();
    if let Some(profile) = config.profile.as_deref() {
        if !is_resume {
            generated.push("--profile".to_string());
            generated.push(profile.to_string());
        }
    }
    if let Some(profile_v2) = config.profile_v2.as_deref() {
        if !is_resume {
            generated.push("--profile-v2".to_string());
            generated.push(profile_v2.to_string());
        }
    }
    append_repeatable_codex_pairs(&mut generated, "--config", &config.config_overrides);
    append_repeatable_codex_pairs(&mut generated, "--enable", &config.enable_features);
    append_repeatable_codex_pairs(&mut generated, "--disable", &config.disable_features);
    if let Some(model) = config.model.as_deref() {
        generated.push("--model".to_string());
        generated.push(model.to_string());
    }
    if let Some(sandbox) = config.sandbox.as_deref() {
        if !is_resume && !danger_full_access {
            generated.push("--sandbox".to_string());
            generated.push(sandbox.to_string());
        }
    }
    if danger_full_access {
        generated.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    }
    if config.dangerously_bypass_hook_trust.unwrap_or(false) {
        generated.push("--dangerously-bypass-hook-trust".to_string());
    }
    append_codex_approval_config(&mut generated, config);
    append_codex_reasoning_config(&mut generated, config);
    append_codex_legacy_search_config(&mut generated, config);
    if config.strict_config.unwrap_or(false) {
        generated.push("--strict-config".to_string());
    }
    if config.oss.unwrap_or(false) {
        generated.push("--oss".to_string());
    }
    if let Some(local_provider) = config.local_provider.as_deref().map(str::trim) {
        if !local_provider.is_empty() {
            generated.push("--local-provider".to_string());
            generated.push(local_provider.to_string());
        }
    }
    if config.skip_git_repo_check.unwrap_or(false) {
        generated.push("--skip-git-repo-check".to_string());
    }
    if config.ignore_user_config.unwrap_or(false) {
        generated.push("--ignore-user-config".to_string());
    }
    if config.ignore_rules.unwrap_or(false) {
        generated.push("--ignore-rules".to_string());
    }
    if !is_resume {
        append_repeatable_codex_pairs(&mut generated, "--add-dir", &config.add_dirs);
    }
    if let Some(output_schema) = config.output_schema.as_deref().map(str::trim) {
        if !output_schema.is_empty() {
            generated.push("--output-schema".to_string());
            generated.push(output_schema.to_string());
        }
    }
    if let Some(color) = config.color.as_deref().map(str::trim) {
        if !color.is_empty() {
            generated.push("--color".to_string());
            generated.push(color.to_string());
        }
    }
    if config.ephemeral.unwrap_or(false) {
        generated.push("--ephemeral".to_string());
    }
    insert_codex_args_before_stdin(args, generated);
}

fn effective_codex_danger_full_access(config: &ExternalCliAdapterConfig) -> bool {
    config
        .danger_full_access
        .unwrap_or_else(|| config.sandbox.is_none() && config.approval_policy.is_none())
}

fn config_override_key(value: &str) -> Option<&str> {
    value.split_once('=').map(|(key, _)| key.trim())
}

fn remove_overridden_codex_args(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    if config.profile.is_some() {
        remove_codex_arg_with_value(args, "--profile");
    }
    if config.profile_v2.is_some() {
        remove_codex_arg_with_value(args, "--profile-v2");
    }
    if config.model.is_some() {
        remove_codex_arg_with_value(args, "--model");
    }
    if config.sandbox.is_some() {
        remove_codex_arg_with_value(args, "--sandbox");
    }
    if config.local_provider.is_some() {
        remove_codex_arg_with_value(args, "--local-provider");
    }
    if config.output_schema.is_some() {
        remove_codex_arg_with_value(args, "--output-schema");
    }
    if config.color.is_some() {
        remove_codex_arg_with_value(args, "--color");
    }
    if config.reasoning_effort.is_some() || config.reasoning_summary.is_some() {
        remove_codex_config_with_key(args, "model_reasoning_effort");
        remove_codex_config_with_key(args, "reasoning_effort");
        remove_codex_config_with_key(args, "model_reasoning_summary");
        remove_codex_config_with_key(args, "reasoning_summary");
    }
}

fn insert_codex_args_before_stdin(args: &mut Vec<String>, generated: Vec<String>) {
    if generated.is_empty() {
        return;
    }
    let insert_at = if args.last().map(String::as_str) == Some("-") {
        args.len() - 1
    } else {
        args.len()
    };
    args.splice(insert_at..insert_at, generated);
}

fn remove_codex_arg_with_value(args: &mut Vec<String>, flag: &str) {
    let mut index = 0;
    while index < args.len() {
        if args[index] == flag {
            args.remove(index);
            if index < args.len() && !args[index].starts_with('-') {
                args.remove(index);
            }
        } else {
            index += 1;
        }
    }
}

fn remove_codex_config_with_key(args: &mut Vec<String>, key: &str) {
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "--config" && config_override_key(&args[index + 1]) == Some(key) {
            args.remove(index);
            args.remove(index);
        } else {
            index += 1;
        }
    }
}

fn append_repeatable_codex_pairs(args: &mut Vec<String>, flag: &str, values: &[String]) {
    for value in values.iter().map(String::as_str).map(str::trim) {
        if value.is_empty() {
            continue;
        }
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

fn append_codex_approval_config(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    if let Some(policy) = config.approval_policy.as_deref().map(str::trim) {
        if !policy.is_empty() {
            args.push("--config".to_string());
            args.push(format!("approval_policy=\"{policy}\""));
        }
    }
}

fn append_codex_reasoning_config(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    if let Some(effort) = config.reasoning_effort.as_deref().map(str::trim) {
        if !effort.is_empty() {
            args.push("--config".to_string());
            args.push(format!("model_reasoning_effort=\"{effort}\""));
        }
    }
    if let Some(summary) = config.reasoning_summary.as_deref().map(str::trim) {
        if !summary.is_empty() {
            args.push("--config".to_string());
            args.push(format!("model_reasoning_summary=\"{summary}\""));
        }
    }
}

fn append_codex_legacy_search_config(args: &mut Vec<String>, config: &ExternalCliAdapterConfig) {
    if !config.search.unwrap_or(false) {
        return;
    }
    if config
        .enable_features
        .iter()
        .any(|feature| feature.trim() == "web_search")
    {
        return;
    }
    args.push("--enable".to_string());
    args.push("web_search".to_string());
}

pub(super) fn codex_thread_id_from_params(request: &ExternalCliRunRequest) -> Option<String> {
    request
        .params
        .get("threadId")
        .or_else(|| request.params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn claude_code_session_id_from_params(request: &ExternalCliRunRequest) -> Option<String> {
    request
        .params
        .get("sessionId")
        .or_else(|| request.params.get("session_id"))
        .or_else(|| request.params.get("threadId"))
        .or_else(|| request.params.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn ensure_codex_work_dir_arg(args: &mut Vec<String>, work_dir: &Path) {
    if args.iter().any(|arg| arg == "--cd" || arg == "-C") {
        return;
    }
    let insert_at = match args.first().map(String::as_str) {
        Some("exec" | "e") => 1,
        _ => 0,
    };
    args.insert(insert_at, work_dir.display().to_string());
    args.insert(insert_at, "--cd".to_string());
}

fn ensure_traex_work_dir_arg(args: &mut Vec<String>, work_dir: &Path) {
    if args.iter().any(|arg| arg == "--cd" || arg == "-C") {
        return;
    }
    args.insert(0, work_dir.display().to_string());
    args.insert(0, "--cd".to_string());
}

fn ensure_codex_last_message_arg(args: &mut Vec<String>, last_message_path: &Path) {
    if !args.iter().any(|arg| matches!(arg.as_str(), "exec" | "e")) {
        return;
    }
    if args
        .iter()
        .any(|arg| arg == "--output-last-message" || arg == "-o")
    {
        return;
    }
    let insert_at = if args.last().map(String::as_str) == Some("-") {
        args.len() - 1
    } else {
        args.len()
    };
    args.insert(insert_at, last_message_path.display().to_string());
    args.insert(insert_at, "--output-last-message".to_string());
}
