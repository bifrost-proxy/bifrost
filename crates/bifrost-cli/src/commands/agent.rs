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
        AgentCommands::Guide {
            message,
            session,
            json,
        } => handle_agent_guide(host, port, &session, &message, json),
        AgentCommands::ExternalRunnerWorker => {
            bifrost_admin::im_gateway::external_cli::run_worker_stdio()
                .map_err(bifrost_core::BifrostError::Config)
        }
    }
}

fn handle_agent_guide(
    host: &str,
    port: u16,
    session: &str,
    message: &str,
    raw_json: bool,
) -> Result<()> {
    let session = session.trim();
    let message = message.trim();
    if session.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "session cannot be empty".to_string(),
        ));
    }
    if message.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "guide message cannot be empty".to_string(),
        ));
    }
    let url = format!(
        "http://{}:{}{}/sessions/{}/guide",
        host,
        port,
        CHAT_GATEWAY_API,
        urlencoding::encode(session),
    );
    let response = bifrost_core::direct_ureq_agent()
        .post(&url)
        .send_json(json!({"message": message}))
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "failed to guide active agent session '{session}': {error}"
            ))
        })?;
    let value: Value = response.into_json().map_err(|error| {
        bifrost_core::BifrostError::Parse(format!("failed to parse guide response: {error}"))
    })?;
    if raw_json {
        println!(
            "{}",
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
        );
        return Ok(());
    }
    let delivery = value
        .get("delivery")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match delivery {
        "steered" => {
            if let Some(turn_id) = value.get("turnId").and_then(Value::as_str) {
                println!(
                    "{} Guided active turn {} (session={})",
                    "✓".bright_green(),
                    turn_id.bright_cyan(),
                    session,
                );
            } else {
                let thread_id = value
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                println!(
                    "{} Redirected active runner session {} (session={})",
                    "✓".bright_green(),
                    thread_id.bright_cyan(),
                    session,
                );
            }
        }
        "queued" => {
            let reason = value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("active turn could not accept guidance");
            println!(
                "{} Active turn could not be steered; queued for the next turn ({})",
                "→".bright_yellow(),
                reason,
            );
        }
        _ => {
            return Err(bifrost_core::BifrostError::Config(format!(
                "unexpected guide delivery response: {value}"
            )));
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex, OnceLock};
    use std::thread;

    fn agent_run_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn spawn_simple_http_server(body: String, content_type: &str) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ct = content_type.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let mut received = Vec::new();
            let mut header_end = None;
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
                if let Some(pos) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = Some(pos + 4);
                    break;
                }
            }
            if let Some(header_end) = header_end {
                let headers = String::from_utf8_lossy(&received[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while received.len().saturating_sub(header_end) < content_length {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    received.extend_from_slice(&buf[..n]);
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                ct,
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (port, handle)
    }

    fn spawn_recording_http_server(body: &'static str) -> (u16, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buf = [0u8; 1024];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0, "request ended before headers");
                received.extend_from_slice(&buf[..n]);
                if let Some(pos) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            let headers = String::from_utf8_lossy(&received[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or_default();
            while received.len().saturating_sub(header_end) < content_length {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body,
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(received).unwrap()
        });
        (port, handle)
    }

    #[test]
    fn handle_agent_run_rejects_empty_message() {
        let opts = AgentRunOptions {
            message: "   ",
            runner: Some("test-runner".to_string()),
            session: None,
            new_conversation: false,
            output_dir: None,
            raw_json: false,
        };

        let result = handle_agent_run("127.0.0.1", 0, opts);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("message cannot be empty"));
    }

    #[test]
    fn handle_agent_guide_posts_encoded_session_and_message() {
        let (port, handle) = spawn_recording_http_server(
            r#"{"delivery":"steered","turnId":"turn-1","sessionKey":"team/a"}"#,
        );

        handle_agent_guide("127.0.0.1", port, "team/a", "focus on tests", true).unwrap();
        let request = handle.join().unwrap();

        assert!(request
            .starts_with("POST /_bifrost/api/im-gateway/chat/sessions/team%2Fa/guide HTTP/1.1"));
        assert!(request.contains(r#"{"message":"focus on tests"}"#));
    }

    #[test]
    fn handle_agent_guide_accepts_session_redirect_without_turn_id() {
        let (port, handle) = spawn_recording_http_server(
            r#"{"delivery":"steered","threadId":"claude-session","sessionKey":"team/claude"}"#,
        );

        handle_agent_guide("127.0.0.1", port, "team/claude", "focus on tests", false).unwrap();
        let request = handle.join().unwrap();

        assert!(request.starts_with(
            "POST /_bifrost/api/im-gateway/chat/sessions/team%2Fclaude/guide HTTP/1.1"
        ));
    }

    #[test]
    fn handle_agent_guide_rejects_empty_inputs_before_network() {
        assert!(handle_agent_guide("127.0.0.1", 0, " ", "guide", true).is_err());
        assert!(handle_agent_guide("127.0.0.1", 0, "session", " ", true).is_err());
    }

    #[test]
    fn handle_agent_run_stream_success_without_images() {
        let _guard = agent_run_test_lock();
        let body_lines = [
            r#"{"eventType":"run_started"}"#,
            r#"{"eventType":"status","title":"working","content":"step1"}"#,
            r#"{"eventType":"assistant_delta"}"#,
            r#"{"eventType":"run_finished","response":"Hello from agent","status":"succeeded"}"#,
        ];
        let body = body_lines.join("\n") + "\n";
        let (port, handle) = spawn_simple_http_server(body, "application/x-ndjson");

        let opts = AgentRunOptions {
            message: "Hello",
            runner: Some("test-runner".to_string()),
            session: Some("test-session".to_string()),
            new_conversation: false,
            output_dir: None,
            raw_json: false,
        };

        let result = handle_agent_run("127.0.0.1", port, opts);
        handle.join().unwrap();
        assert!(result.is_ok(), "agent run failed: {result:?}");
    }

    #[test]
    fn handle_agent_run_stream_failed_propagates_error() {
        let _guard = agent_run_test_lock();
        let body_lines = [r#"{"eventType":"run_failed","error":"something went wrong"}"#];
        let body = body_lines.join("\n") + "\n";
        let (port, handle) = spawn_simple_http_server(body, "application/x-ndjson");

        let opts = AgentRunOptions {
            message: "Hello",
            runner: Some("test-runner".to_string()),
            session: None,
            new_conversation: false,
            output_dir: None,
            raw_json: false,
        };

        let result = handle_agent_run("127.0.0.1", port, opts);
        handle.join().unwrap();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("agent run failed"));
    }

    #[test]
    fn clear_status_line_does_not_panic() {
        clear_status_line(true);
        clear_status_line(false);
    }

    #[test]
    fn select_runner_interactively_uses_single_enabled_runner_without_tty() {
        let config_body = r#"{
            "runners": {
                "chatgpt-web": {"enabled": true, "adapter": "test-adapter"}
            }
        }"#
        .to_string();
        let (port, handle) = spawn_simple_http_server(config_body, "application/json");

        let runner_id = select_runner_interactively("127.0.0.1", port).unwrap();
        handle.join().unwrap();
        assert_eq!(runner_id, "chatgpt-web");
    }
}
