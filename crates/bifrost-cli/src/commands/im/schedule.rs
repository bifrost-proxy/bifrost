use super::*;

// ─── Schedule ────────────────────────────────────────────────────────────────

pub(super) fn handle_im_schedule(host: &str, port: u16, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str());
    match sub {
        Some("list") => {
            let url = api_url(host, port, "/schedules");
            let resp = http_get(&url)?;
            print_schedule_list(&resp);
            Ok(())
        }
        Some("add") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let body = parse_schedule_add_args(name, &args[2..])?;
            let url = api_url(host, port, "/schedules");
            let resp = http_post(&url, &body)?;
            println!(
                "{} Schedule '{}' created.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("update") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let body = parse_schedule_update_args(&args[2..])?;
            let url = api_url(host, port, &format!("/schedules/{}", name));
            let resp = http_patch(&url, &body)?;
            println!(
                "{} Schedule '{}' updated.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("pause") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/schedules/{}/pause", name));
            http_post(&url, &json!({}))?;
            println!("{} Schedule '{}' paused.", "✓".bright_green(), name);
            Ok(())
        }
        Some("resume") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/schedules/{}/resume", name));
            http_post(&url, &json!({}))?;
            println!("{} Schedule '{}' resumed.", "✓".bright_green(), name);
            Ok(())
        }
        Some("run") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/schedules/{}/run", name));
            let resp = http_post(&url, &json!({}))?;
            let run_id = resp["run_id"].as_str().unwrap_or("unknown");
            println!(
                "{} Schedule '{}' triggered (run_id: {})",
                "✓".bright_green(),
                name,
                run_id.dimmed()
            );
            Ok(())
        }
        Some("logs") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/schedules/{}/runs", name));
            let resp = http_get(&url)?;
            print_task_runs(&resp);
            Ok(())
        }
        Some("delete") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("schedule name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/schedules/{}", name));
            http_delete(&url)?;
            println!("{} Schedule '{}' deleted.", "✓".bright_green(), name);
            Ok(())
        }
        _ => {
            eprintln!("Usage: bifrost im schedule <list|add|update|pause|resume|run|logs|delete>");
            Ok(())
        }
    }
}

pub(super) fn parse_schedule_add_args(name: &str, args: &[String]) -> Result<Value> {
    let mut body = json!({
        "name": name,
    });
    let mut trigger = json!({});
    let mut script = json!({});
    let mut agent = json!({});
    let mut agent_adapter_config = json!({});
    let mut provider_id: Option<String> = None;
    let mut target_id: Option<String> = None;
    let mut target_mode: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    target_id = Some(v.to_string());
                }
            }
            "--provider" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    provider_id = Some(v.to_string());
                }
            }
            "--target-mode" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    target_mode = Some(v.to_string());
                }
            }
            "--cron" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    trigger["type"] = json!("cron");
                    trigger["expr"] = json!(v);
                }
            }
            "--every" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Ok(ms) = v.parse::<u64>() {
                        trigger["type"] = json!("interval");
                        trigger["every_ms"] = json!(ms);
                    }
                }
            }
            "--timezone" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    trigger["timezone"] = json!(v);
                }
            }
            "--script-file" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("script");
                    script["script_file"] = json!(v);
                }
            }
            "--script" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("script");
                    script["script_text"] = json!(v);
                }
            }
            "--agent-prompt" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("agent");
                    agent["prompt"] = json!(v);
                }
            }
            "--agent-prompt-file" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("agent");
                    agent["prompt"] = json!(std::fs::read_to_string(v).map_err(|e| {
                        bifrost_core::BifrostError::Config(format!(
                            "read agent prompt file '{v}': {e}"
                        ))
                    })?);
                }
            }
            "--agent-session-key" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["session_key"] = json!(v);
                }
            }
            "--agent-work-dir" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["work_dir"] = json!(v);
                }
            }
            "--agent-system-prompt" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["system_prompt"] = json!(v);
                }
            }
            "--agent-runner-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["runner_id"] = json!(v);
                }
            }
            "--agent-model" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["model"] = json!(v);
                }
            }
            "--agent-profile" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["profile"] = json!(v);
                }
            }
            "--agent-profile-v2" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["profileV2"] = json!(v);
                }
            }
            "--agent-sandbox" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["sandbox"] = json!(v);
                }
            }
            "--agent-reasoning-effort" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["reasoningEffort"] = json!(v);
                }
            }
            "--agent-reasoning-summary" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["reasoningSummary"] = json!(v);
                }
            }
            "--agent-approval-policy" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["approvalPolicy"] = json!(v);
                }
            }
            "--agent-search" => {
                agent_adapter_config["search"] = json!(true);
            }
            "--agent-ephemeral" => {
                agent_adapter_config["ephemeral"] = json!(true);
            }
            "--agent-timeout-secs" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Ok(n) = v.parse::<u64>() {
                        agent_adapter_config["timeoutSecs"] = json!(n);
                    }
                }
            }
            "--agent-danger-full-access" => {
                agent_adapter_config["dangerFullAccess"] = json!(true);
            }
            "--agent-dangerously-bypass-hook-trust" | "--agent-bypass-hook-trust" => {
                agent_adapter_config["dangerouslyBypassHookTrust"] = json!(true);
            }
            "--agent-strict-config" => {
                agent_adapter_config["strictConfig"] = json!(true);
            }
            "--agent-skip-git-repo-check" => {
                agent_adapter_config["skipGitRepoCheck"] = json!(true);
            }
            "--agent-ignore-user-config" => {
                agent_adapter_config["ignoreUserConfig"] = json!(true);
            }
            "--agent-ignore-rules" => {
                agent_adapter_config["ignoreRules"] = json!(true);
            }
            "--agent-oss" => {
                agent_adapter_config["oss"] = json!(true);
            }
            "--agent-local-provider" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["localProvider"] = json!(v);
                }
            }
            "--agent-output-schema" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["outputSchema"] = json!(v);
                }
            }
            "--agent-color" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["color"] = json!(v);
                }
            }
            "--agent-add-dir" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["addDirs"], v);
                }
            }
            "--agent-config" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["configOverrides"], v);
                }
            }
            "--agent-enable" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["enableFeatures"], v);
                }
            }
            "--agent-disable" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["disableFeatures"], v);
                }
            }
            "--cwd" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    script["cwd"] = json!(v);
                }
            }
            "--timeout-ms" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Ok(n) = v.parse::<u64>() {
                        body["timeout_ms"] = json!(n);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    if !trigger.as_object().is_none_or(|t| t.is_empty()) {
        body["trigger"] = trigger;
    }
    if !script.as_object().is_none_or(|s| s.is_empty()) {
        body["script"] = script;
    }
    apply_schedule_message_channel_args(
        &mut body,
        provider_id.as_deref(),
        target_id.as_deref(),
        target_mode.as_deref(),
    );
    let has_agent_adapter_config = !agent_adapter_config
        .as_object()
        .is_none_or(|config| config.is_empty());
    if !agent.as_object().is_none_or(|a| a.is_empty()) || has_agent_adapter_config {
        if has_agent_adapter_config {
            agent["adapter_config"] = agent_adapter_config;
        }
        body["agent"] = agent;
        body["task_type"] = json!("agent");
    }

    Ok(body)
}

pub(super) fn parse_schedule_update_args(args: &[String]) -> Result<Value> {
    let mut body = json!({});
    let mut trigger = json!({});
    let mut script = json!({});
    let mut agent = json!({});
    let mut agent_adapter_config = json!({});
    let mut provider_id: Option<String> = None;
    let mut target_id: Option<String> = None;
    let mut target_mode: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    target_id = Some(v.to_string());
                }
            }
            "--provider" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    provider_id = Some(v.to_string());
                }
            }
            "--target-mode" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    target_mode = Some(v.to_string());
                }
            }
            "--cron" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    trigger["type"] = json!("cron");
                    trigger["expr"] = json!(v);
                }
            }
            "--timeout-ms" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Ok(n) = v.parse::<u64>() {
                        body["timeout_ms"] = json!(n);
                    }
                }
            }
            "--enabled" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["enabled"] = json!(v.parse::<bool>().unwrap_or(true));
                }
            }
            "--script-file" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("script");
                    script["script_file"] = json!(v);
                }
            }
            "--script" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("script");
                    script["script_text"] = json!(v);
                }
            }
            "--agent-prompt" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("agent");
                    agent["prompt"] = json!(v);
                }
            }
            "--agent-prompt-file" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["task_type"] = json!("agent");
                    agent["prompt"] = json!(std::fs::read_to_string(v).map_err(|e| {
                        bifrost_core::BifrostError::Config(format!(
                            "read agent prompt file '{v}': {e}"
                        ))
                    })?);
                }
            }
            "--agent-session-key" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["session_key"] = json!(v);
                }
            }
            "--agent-work-dir" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["work_dir"] = json!(v);
                }
            }
            "--agent-system-prompt" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["system_prompt"] = json!(v);
                }
            }
            "--agent-runner-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent["runner_id"] = json!(v);
                }
            }
            "--agent-model" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["model"] = json!(v);
                }
            }
            "--agent-profile" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["profile"] = json!(v);
                }
            }
            "--agent-profile-v2" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["profileV2"] = json!(v);
                }
            }
            "--agent-sandbox" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["sandbox"] = json!(v);
                }
            }
            "--agent-reasoning-effort" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["reasoningEffort"] = json!(v);
                }
            }
            "--agent-reasoning-summary" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["reasoningSummary"] = json!(v);
                }
            }
            "--agent-approval-policy" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["approvalPolicy"] = json!(v);
                }
            }
            "--agent-search" => {
                agent_adapter_config["search"] = json!(true);
            }
            "--agent-ephemeral" => {
                agent_adapter_config["ephemeral"] = json!(true);
            }
            "--agent-timeout-secs" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if let Ok(n) = v.parse::<u64>() {
                        agent_adapter_config["timeoutSecs"] = json!(n);
                    }
                }
            }
            "--agent-danger-full-access" => {
                agent_adapter_config["dangerFullAccess"] = json!(true);
            }
            "--agent-dangerously-bypass-hook-trust" | "--agent-bypass-hook-trust" => {
                agent_adapter_config["dangerouslyBypassHookTrust"] = json!(true);
            }
            "--agent-strict-config" => {
                agent_adapter_config["strictConfig"] = json!(true);
            }
            "--agent-skip-git-repo-check" => {
                agent_adapter_config["skipGitRepoCheck"] = json!(true);
            }
            "--agent-ignore-user-config" => {
                agent_adapter_config["ignoreUserConfig"] = json!(true);
            }
            "--agent-ignore-rules" => {
                agent_adapter_config["ignoreRules"] = json!(true);
            }
            "--agent-oss" => {
                agent_adapter_config["oss"] = json!(true);
            }
            "--agent-local-provider" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["localProvider"] = json!(v);
                }
            }
            "--agent-output-schema" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["outputSchema"] = json!(v);
                }
            }
            "--agent-color" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    agent_adapter_config["color"] = json!(v);
                }
            }
            "--agent-add-dir" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["addDirs"], v);
                }
            }
            "--agent-config" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["configOverrides"], v);
                }
            }
            "--agent-enable" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["enableFeatures"], v);
                }
            }
            "--agent-disable" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    push_json_array_string(&mut agent_adapter_config["disableFeatures"], v);
                }
            }
            _ => {}
        }
        i += 1;
    }

    if !trigger.as_object().is_none_or(|t| t.is_empty()) {
        body["trigger"] = trigger;
    }
    if !script.as_object().is_none_or(|s| s.is_empty()) {
        body["script"] = script;
    }
    apply_schedule_message_channel_args(
        &mut body,
        provider_id.as_deref(),
        target_id.as_deref(),
        target_mode.as_deref(),
    );
    let has_agent_adapter_config = !agent_adapter_config
        .as_object()
        .is_none_or(|config| config.is_empty());
    if !agent.as_object().is_none_or(|a| a.is_empty()) || has_agent_adapter_config {
        if has_agent_adapter_config {
            agent["adapter_config"] = agent_adapter_config;
        }
        body["agent"] = agent;
        body["task_type"] = json!("agent");
    }

    Ok(body)
}

fn apply_schedule_message_channel_args(
    body: &mut Value,
    provider_id: Option<&str>,
    target_id: Option<&str>,
    target_mode: Option<&str>,
) {
    let Some(target_id) = target_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let provider_id = provider_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_IM_PROVIDER_ID);
    let target_mode = target_mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("configured_target");
    body["message_channel"] = json!({
        "provider_id": provider_id,
        "target_id": target_id,
        "target_mode": target_mode
    });
}

fn push_json_array_string(value: &mut Value, item: &str) {
    if !value.is_array() {
        *value = json!([]);
    }
    if let Some(values) = value.as_array_mut() {
        values.push(json!(item));
    }
}

fn print_schedule_list(resp: &Value) {
    let empty = vec![];
    let schedules = resp.as_array().unwrap_or(&empty);
    if schedules.is_empty() {
        println!("{}", "No IM schedules configured.".dimmed());
        return;
    }

    println!(
        "{:<20} {:<20} {:<10} {:<10} {}",
        "NAME".bold(),
        "TARGET".bold(),
        "TYPE".bold(),
        "ENABLED".bold(),
        "SCHEDULE".bold()
    );
    println!("{}", "─".repeat(75));

    for s in schedules {
        let name = s["name"].as_str().unwrap_or("-");
        let target = s["message_channel"]["target_id"]
            .as_str()
            .or_else(|| s["target_id"].as_str())
            .unwrap_or("-");
        let trigger_type = s["task_type"]
            .as_str()
            .unwrap_or_else(|| s["trigger"]["type"].as_str().unwrap_or("-"));
        let enabled = s["enabled"].as_bool().unwrap_or(false);
        let schedule_expr = s["trigger"]["expr"]
            .as_str()
            .or_else(|| s["trigger"]["every_ms"].as_u64().map(|_| "interval"))
            .unwrap_or("-");

        let enabled_str = if enabled {
            "yes".bright_green().to_string()
        } else {
            "no".bright_red().to_string()
        };

        println!(
            "{:<20} {:<20} {:<10} {:<10} {}",
            name, target, trigger_type, enabled_str, schedule_expr
        );
    }
}
