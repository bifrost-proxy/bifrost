use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;

use colored::Colorize;
use serde_json::{json, Value};
use tracing::debug;

use bifrost_core::Result;

use crate::cli::AgentCommands;

const CHAT_GATEWAY_API: &str = "/_bifrost/api/im-gateway/chat";

pub fn handle_agent_command(
    host: &str,
    port: u16,
    action: crate::cli::AgentCommands,
) -> Result<()> {
    match action {
        AgentCommands::Run {
            message,
            runner,
            session,
            new,
            output_dir,
            json,
        } => handle_agent_run(
            host,
            port,
            AgentRunOptions {
                message: &message,
                runner,
                session,
                new_conversation: new,
                output_dir,
                raw_json: json,
            },
        ),
    }
}

struct AgentRunOptions<'a> {
    message: &'a str,
    runner: Option<String>,
    session: Option<String>,
    new_conversation: bool,
    output_dir: Option<PathBuf>,
    raw_json: bool,
}

fn handle_agent_run(host: &str, port: u16, options: AgentRunOptions<'_>) -> Result<()> {
    let AgentRunOptions {
        message,
        runner,
        session,
        new_conversation,
        output_dir,
        raw_json,
    } = options;

    if message.trim().is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "message cannot be empty".to_string(),
        ));
    }

    // Resolve runner: explicit --runner, or interactive selection
    let runner_id = match runner {
        Some(id) => id,
        None => select_runner_interactively(host, port)?,
    };

    // Determine session key
    let session_key = if new_conversation {
        None
    } else {
        Some(session.unwrap_or_else(|| format!("cli-{}", runner_id)))
    };

    // Build request body. Server expects camelCase (ExternalCliRunRequest is
    // tagged with `#[serde(rename_all = "camelCase")]`); using snake_case keys
    // would be silently dropped and cause the request to fall back to the
    // default runner (codex) instead of the user-selected one.
    let mut body = json!({
        "message": message,
        "operation": "ask",
        "runnerId": runner_id,
    });
    if let Some(ref session_key) = session_key {
        body["sessionKey"] = json!(session_key);
    }

    // Execute the run via streaming endpoint
    let url = format!("http://{}:{}{}/stream", host, port, CHAT_GATEWAY_API);
    debug!(url = %url, runner_id = %runner_id, "agent run: POST stream");

    if !raw_json {
        eprintln!(
            "{} Running agent '{}' ...",
            "⏳".dimmed(),
            runner_id.bright_cyan()
        );
    }

    let resp = bifrost_core::direct_ureq_agent()
        .post(&url)
        .send_json(&body)
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "Failed to reach Bifrost at {}:{} — is the proxy running? ({})",
                host, port, e
            ))
        })?;

    // Read NDJSON stream
    let reader = io::BufReader::new(resp.into_reader());
    let mut final_response = String::new();
    let mut final_json: Option<Value> = None;
    let mut run_status = String::new();
    let mut run_error: Option<String> = None;

    for line in reader.lines() {
        let line = line.map_err(|e| {
            bifrost_core::BifrostError::Network(format!("stream read error: {}", e))
        })?;
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let event: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = event
            .get("eventType")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if raw_json {
            println!("{}", serde_json::to_string(&event).unwrap());
            io::stdout().flush().ok();
        }

        match event_type {
            "run_started" => {
                debug!("agent run: started");
            }
            "status" => {
                let content = event.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let title = event.get("title").and_then(|v| v.as_str());
                if !raw_json {
                    if let Some(title) = title {
                        eprint!(
                            "\r{} {} {}",
                            "⏳".dimmed(),
                            title.dimmed(),
                            content.dimmed()
                        );
                    } else {
                        eprint!("\r{} {}", "⏳".dimmed(), content.dimmed());
                    }
                }
            }
            "assistant_delta" => {
                clear_status_line(raw_json);
            }
            "run_finished" => {
                clear_status_line(raw_json);
                final_response = event
                    .get("response")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                run_status = event
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                final_json = Some(event);
            }
            "run_failed" => {
                clear_status_line(raw_json);
                run_error = Some(
                    event
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error")
                        .to_string(),
                );
                final_json = Some(event);
            }
            _ => {}
        }
    }

    // Handle errors
    if let Some(error) = run_error {
        if !raw_json {
            eprintln!("{} Agent run failed: {}", "✗".bright_red(), error);
        }
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent run failed: {}",
            error
        )));
    }

    // Output raw JSON if requested
    if raw_json {
        return Ok(());
    }

    // Check for images in the response JSON
    let generated_images = final_json
        .as_ref()
        .and_then(|v| v.get("generatedImages"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if !generated_images.is_empty() {
        let out_dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
        fs::create_dir_all(&out_dir).ok();

        eprintln!(
            "{} {} image(s) generated",
            "✓".bright_green(),
            generated_images.len()
        );
        println!();
        for (i, img) in generated_images.iter().enumerate() {
            let url = img
                .as_str()
                .or_else(|| img.get("url").and_then(|v| v.as_str()));
            if let Some(url) = url {
                let filename = format!("agent-image-{}.png", i + 1);
                let filepath = out_dir.join(&filename);

                // Try to download the image
                match download_image(url, &filepath) {
                    Ok(()) => {
                        println!("![image-{}]({})", i + 1, filepath.display());
                    }
                    Err(e) => {
                        debug!(error = %e, url = %url, "failed to download image");
                        // Fallback: just print the URL
                        println!("![image-{}]({})", i + 1, url);
                    }
                }
            }
        }
    } else if !final_response.is_empty() {
        // Print text response as Markdown
        println!("{}", final_response);
    } else {
        eprintln!("{} Agent returned empty response.", "⚠".bright_yellow());
    }

    // Print run metadata
    let status_colored = match run_status.as_str() {
        "succeeded" => run_status.bright_green(),
        "failed" => run_status.bright_red(),
        "stopped" => run_status.bright_yellow(),
        _ => run_status.dimmed(),
    };
    eprintln!();
    eprintln!(
        "{} runner={} status={}",
        "─".repeat(40).dimmed(),
        runner_id.bright_cyan(),
        status_colored
    );

    Ok(())
}

fn clear_status_line(raw_json: bool) {
    if !raw_json {
        eprint!("\r{}\r", " ".repeat(80));
    }
}

// ─── Runner selection ─────────────────────────────────────────────────────────

fn select_runner_interactively(host: &str, port: u16) -> Result<String> {
    let url = format!("http://{}:{}{}/config", host, port, CHAT_GATEWAY_API);
    debug!(url = %url, "agent: fetching chat-gateway config");

    let resp = bifrost_core::direct_ureq_agent()
        .get(&url)
        .call()
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!(
                "Failed to reach Bifrost at {}:{} — is the proxy running? ({})",
                host, port, e
            ))
        })?;

    let body_str = resp.into_string().map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to read config response: {}", e))
    })?;
    let config: Value = serde_json::from_str(&body_str)
        .map_err(|e| bifrost_core::BifrostError::Parse(format!("failed to parse config: {}", e)))?;

    let runners = config
        .get("runners")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(id, settings)| {
                    let enabled = settings
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let adapter = settings
                        .get("adapter")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    (id.clone(), adapter.to_string(), enabled)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if runners.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "no runners configured; configure one via the Bifrost WebUI or API".to_string(),
        ));
    }

    // Filter to enabled runners for auto-selection
    let enabled_runners: Vec<_> = runners.iter().filter(|(_, _, enabled)| *enabled).collect();

    if enabled_runners.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "no enabled runners found; enable a runner via WebUI or API".to_string(),
        ));
    }

    // If only one enabled runner, use it directly
    if enabled_runners.len() == 1 {
        let (id, adapter, _) = enabled_runners[0];
        eprintln!(
            "{} Using runner '{}' (adapter: {})",
            "→".dimmed(),
            id.bright_cyan(),
            adapter.dimmed()
        );
        return Ok(id.clone());
    }

    // Interactive selection
    if !io::stdin().is_terminal() {
        return Err(bifrost_core::BifrostError::Config(
            "--runner is required when stdin is not interactive and multiple runners exist"
                .to_string(),
        ));
    }

    eprintln!("{}", "Select runner:".bright_white().bold());
    for (idx, (id, adapter, _)) in enabled_runners.iter().enumerate() {
        eprintln!(
            "  {}) {} ({})",
            (idx + 1).to_string().bright_cyan(),
            id.bright_white(),
            adapter.dimmed()
        );
    }
    eprint!("Runner [1-{}]: ", enabled_runners.len());
    io::stderr().flush().ok();

    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .map_err(|e| bifrost_core::BifrostError::Config(format!("failed to read input: {}", e)))?;
    let choice = input
        .trim()
        .parse::<usize>()
        .map_err(|_| bifrost_core::BifrostError::Config("invalid selection".to_string()))?;

    let Some((id, _, _)) = enabled_runners.get(choice.saturating_sub(1)) else {
        return Err(bifrost_core::BifrostError::Config(
            "selection out of range".to_string(),
        ));
    };

    Ok(id.clone())
}

// ─── Image download ─────────────────────────────────────────────────────────

fn download_image(url: &str, dest: &PathBuf) -> Result<()> {
    let resp = bifrost_core::direct_ureq_agent()
        .get(url)
        .call()
        .map_err(|e| {
            bifrost_core::BifrostError::Network(format!("image download failed: {}", e))
        })?;

    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .map_err(bifrost_core::BifrostError::Io)?;
    fs::write(dest, &bytes).map_err(bifrost_core::BifrostError::Io)?;
    Ok(())
}
