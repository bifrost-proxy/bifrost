use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};
use qrcode::{render::unicode, QrCode};
use serde_json::{json, Value};
use tracing::debug;

mod schedule;

use schedule::handle_im_schedule;

use bifrost_core::{text::truncate_chars_with_suffix, Result};

const IM_GATEWAY_API_PREFIX: &str = "/_bifrost/api/im-gateway";
const DEFAULT_IM_PROVIDER_ID: &str = "feishu-main";
const DEFAULT_BUILTIN_RUNNERS_HINT: &str = "codex, traex, Claude Code";

pub fn handle_im_command(host: &str, port: u16, args: &[String]) -> Result<()> {
    if args.is_empty() {
        print_im_help();
        return Ok(());
    }

    match args[0].as_str() {
        "provider" => handle_im_provider(host, port, &args[1..]),
        "target" => handle_im_target(host, port, &args[1..]),
        "send" => handle_im_send(host, port, &args[1..]),
        "route" => handle_im_route(host, port, &args[1..]),
        "schedule" => handle_im_schedule(host, port, &args[1..]),
        "history" => handle_im_history(host, port, &args[1..]),
        "messages" => handle_im_messages(host, port, &args[1..]),
        "help" | "--help" | "-h" => {
            print_im_help();
            Ok(())
        }
        other => {
            eprintln!(
                "{} Unknown im subcommand: {}",
                "error:".bright_red().bold(),
                other
            );
            print_im_help();
            Ok(())
        }
    }
}

// ─── Provider ────────────────────────────────────────────────────────────────

fn handle_im_provider(host: &str, port: u16, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str());
    match sub {
        Some("list") => {
            let url = api_url(host, port, "/providers");
            let resp = http_get(&url)?;
            print_provider_list(&resp);
            Ok(())
        }
        Some("add") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("provider name required".to_string())
            })?;
            let mut add_args = parse_provider_add_args(name, &args[2..])?;
            if add_args.should_use_feishu_setup() {
                return handle_feishu_provider_setup(host, port, &add_args);
            }
            if add_args.should_use_weixin_setup() {
                return handle_weixin_provider_setup(host, port, &add_args);
            }
            if add_args.should_require_runner() {
                let runner_id =
                    resolve_provider_setup_runner(host, port, add_args.runner.as_deref())?;
                add_args.runner = Some(runner_id);
            }
            let body = add_args.into_create_body();
            let url = api_url(host, port, "/providers");
            let resp = http_post(&url, &body)?;
            println!(
                "{} Provider '{}' created.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("update") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("provider name required".to_string())
            })?;
            let body = parse_provider_update_args(&args[2..])?;
            let url = api_url(host, port, &format!("/providers/{}", name));
            let resp = http_patch(&url, &body)?;
            println!(
                "{} Provider '{}' updated.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("delete") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("provider name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/providers/{}", name));
            http_delete(&url)?;
            println!("{} Provider '{}' deleted.", "✓".bright_green(), name);
            Ok(())
        }
        Some("status") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("provider name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/providers/{}/status", name));
            let resp = http_get(&url)?;
            print_provider_status(name, &resp);
            Ok(())
        }
        Some("capabilities") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("provider name required".to_string())
            })?;
            let format = args
                .windows(2)
                .find(|pair| pair[0] == "--format")
                .map(|pair| pair[1].as_str())
                .unwrap_or("human");
            if !matches!(format, "human" | "json" | "json-pretty") {
                return Err(bifrost_core::BifrostError::Config(
                    "--format must be one of: human, json, json-pretty".to_string(),
                ));
            }
            let url = api_url(host, port, &format!("/providers/{name}/capabilities"));
            let resp = http_get(&url)?;
            print_provider_capabilities(&resp, format)?;
            Ok(())
        }
        Some("menu") => handle_im_provider_menu(host, port, &args[1..]),
        _ => {
            eprintln!(
                "Usage: bifrost im provider <list|add|update|delete|status|capabilities|menu>"
            );
            Ok(())
        }
    }
}

fn handle_im_provider_menu(host: &str, port: u16, args: &[String]) -> Result<()> {
    let provider_id = args.first().ok_or_else(|| {
        bifrost_core::BifrostError::Config(
            "usage: bifrost im provider menu <provider> <preview|status|sync> [--publish]"
                .to_string(),
        )
    })?;
    let action = args.get(1).map(String::as_str).ok_or_else(|| {
        bifrost_core::BifrostError::Config(
            "menu action required: preview, status, or sync".to_string(),
        )
    })?;
    let publish = args.iter().skip(2).any(|arg| arg == "--publish");
    if args.iter().skip(2).any(|arg| arg != "--publish") || (publish && action != "sync") {
        return Err(bifrost_core::BifrostError::Config(
            "--publish is only valid with 'menu <provider> sync'".to_string(),
        ));
    }
    let path = format!("/providers/{provider_id}/feishu/menu/{action}");
    let url = api_url(host, port, &path);
    let response = match action {
        "preview" | "status" => http_get(&url)?,
        "sync" => {
            let (status, response) = http_post_with_status(&url, &json!({"publish": publish}))?;
            if !(200..300).contains(&status) {
                let detail = response["message"]
                    .as_str()
                    .or_else(|| response["error"].as_str())
                    .unwrap_or("unknown menu sync error");
                return Err(bifrost_core::BifrostError::Network(format!(
                    "Feishu menu sync failed with HTTP {status}: {detail}"
                )));
            }
            response
        }
        _ => {
            return Err(bifrost_core::BifrostError::Config(
                "menu action must be one of: preview, status, sync".to_string(),
            ));
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(im_json_error)?
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct ProviderAddArgs {
    id: String,
    provider_type: Option<String>,
    app_id: Option<String>,
    app_secret: Option<String>,
    display_name: Option<String>,
    enabled: Option<bool>,
    owner_open_id: Option<String>,
    event_connection_enabled: Option<bool>,
    runner: Option<String>,
    brand: Option<String>,
}

impl ProviderAddArgs {
    fn should_use_feishu_setup(&self) -> bool {
        self.provider_type.as_deref() == Some("feishu")
            && self.app_id.as_deref().is_none_or(str::is_empty)
            && self.app_secret.as_deref().is_none_or(str::is_empty)
    }

    fn should_use_weixin_setup(&self) -> bool {
        matches!(
            self.provider_type.as_deref(),
            Some("weixin") | Some("wechat")
        ) && self.app_id.as_deref().is_none_or(str::is_empty)
            && self.app_secret.as_deref().is_none_or(str::is_empty)
    }

    fn should_require_runner(&self) -> bool {
        matches!(
            self.provider_type.as_deref(),
            Some("feishu") | Some("weixin") | Some("wechat")
        )
    }

    fn into_create_body(self) -> Value {
        let mut body = json!({
            "id": self.id,
        });

        if let Some(value) = self.provider_type {
            body["provider_type"] = json!(value);
        }
        if let Some(value) = self.app_id {
            body["app_id"] = json!(value);
        }
        if let Some(value) = self.app_secret {
            body["app_secret"] = json!(value);
        }
        if let Some(value) = self.display_name {
            body["display_name"] = json!(value);
        }
        if let Some(value) = self.enabled {
            body["enabled"] = json!(value);
        }
        if let Some(value) = self.owner_open_id {
            body["owner_open_id"] = json!(value);
        }
        if let Some(value) = self.event_connection_enabled {
            body["event_connection_enabled"] = json!(value);
        }
        if let Some(value) = self.runner {
            body["agent_config"] = json!({
                "runner": value,
            });
        }

        body
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunnerChoice {
    id: String,
    adapter: String,
    enabled: bool,
}

fn parse_provider_add_args(name: &str, args: &[String]) -> Result<ProviderAddArgs> {
    let mut parsed = ProviderAddArgs {
        id: name.to_string(),
        provider_type: None,
        app_id: None,
        app_secret: None,
        display_name: None,
        enabled: None,
        owner_open_id: None,
        event_connection_enabled: None,
        runner: None,
        brand: None,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--type" => {
                i += 1;
                parsed.provider_type = args.get(i).cloned();
            }
            "--app-id" => {
                i += 1;
                parsed.app_id = args.get(i).cloned();
            }
            "--secret" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    let resolved = resolve_secret(v)
                        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
                    parsed.app_secret = Some(resolved);
                }
            }
            "--base-url" => {
                return Err(bifrost_core::BifrostError::Config(
                    "base_url is managed by system and cannot be set via CLI".to_string(),
                ));
            }
            "--display-name" => {
                i += 1;
                parsed.display_name = args.get(i).cloned();
            }
            "--enabled" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    parsed.enabled = Some(v.parse::<bool>().unwrap_or(true));
                }
            }
            "--owner-open-id" => {
                i += 1;
                parsed.owner_open_id = args.get(i).cloned();
            }
            "--enable-long-connection" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    parsed.event_connection_enabled = Some(v.parse::<bool>().unwrap_or(false));
                }
            }
            "--runner" | "--agent-runner" | "--agent-runner-id" => {
                i += 1;
                parsed.runner = args.get(i).cloned();
            }
            "--brand" => {
                i += 1;
                parsed.brand = args.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }

    Ok(parsed)
}

fn build_setup_provider_body(
    args: &ProviderAddArgs,
    provider_type: &str,
    runner_id: &str,
) -> Value {
    let mut body = json!({
        "id": args.id,
        "provider_type": provider_type,
        "enabled": args.enabled.unwrap_or(true),
        "event_connection_enabled": args.event_connection_enabled.unwrap_or(true),
        "event_types": ["message.receive"],
        "agent_config": {
            "runner": runner_id,
        },
    });

    if let Some(display_name) = args.display_name.as_deref() {
        body["display_name"] = json!(display_name);
    }
    if let Some(owner_open_id) = args.owner_open_id.as_deref() {
        body["owner_open_id"] = json!(owner_open_id);
    }

    body
}

fn handle_feishu_provider_setup(host: &str, port: u16, args: &ProviderAddArgs) -> Result<()> {
    let runner_id = resolve_provider_setup_runner(host, port, args.runner.as_deref())?;
    println!(
        "{} Starting Feishu provider setup for '{}'.",
        "●".bright_cyan(),
        args.id.bright_white().bold()
    );

    let provider_body = build_setup_provider_body(args, "feishu", &runner_id);
    let start_body = json!({
        "brand": args.brand.as_deref().unwrap_or("feishu"),
        "provider": provider_body,
    });
    let start_url = api_url(host, port, "/providers/feishu-setup/start");
    let start = http_post(&start_url, &start_body)?;
    let session_id = required_string(&start, "session_id")?.to_string();
    let verification_url = required_string(&start, "verification_url")?.to_string();
    let mut interval_seconds = setup_poll_interval_seconds(start["interval_seconds"].as_u64(), 5);
    let expires_at = start["expires_at"].as_i64().unwrap_or_default();

    println!("{} Open this URL to continue:", "→".bright_cyan());
    println!("  {}", verification_url.bright_white().bold());
    print_terminal_qr_code(&verification_url);
    println!(
        "{} Waiting for Feishu setup confirmation. Press Ctrl+C to stop waiting.",
        "…".bright_yellow()
    );

    let confirmed = loop {
        thread::sleep(Duration::from_secs(interval_seconds));
        let status_url = api_url(
            host,
            port,
            &format!("/providers/feishu-setup/{}/status", session_id),
        );
        let status = http_get(&status_url)?;
        match status["status"].as_str().unwrap_or("pending") {
            "confirmed" => break status,
            "expired" => {
                return Err(bifrost_core::BifrostError::Config(
                    "Feishu setup session expired before confirmation".to_string(),
                ));
            }
            "pending" => {
                interval_seconds = setup_poll_interval_seconds(
                    status["interval_seconds"].as_u64(),
                    interval_seconds,
                );
                let remaining = setup_remaining_seconds(&status, expires_at);
                if let Some(remaining) = remaining {
                    println!(
                        "{} Still waiting for setup confirmation ({}s remaining).",
                        "…".bright_yellow(),
                        remaining
                    );
                } else {
                    println!(
                        "{}",
                        "… Still waiting for setup confirmation.".bright_yellow()
                    );
                }
            }
            other => {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "unexpected Feishu setup status: {other}"
                )));
            }
        }
    };

    let app_id = confirmed["app_id"].as_str().unwrap_or("-");
    println!(
        "{} Feishu setup confirmed for app {}.",
        "✓".bright_green(),
        app_id.bright_white()
    );

    let provider_id = if let Some(provider_id) = confirmed["provider_id"].as_str() {
        provider_id.to_string()
    } else {
        let create_url = api_url(
            host,
            port,
            &format!("/providers/feishu-setup/{}/provider", session_id),
        );
        let resp = http_post(&create_url, &provider_body)?;
        resp["provider"]["id"]
            .as_str()
            .or_else(|| resp["id"].as_str())
            .unwrap_or(&args.id)
            .to_string()
    };

    let connect_url = api_url(host, port, &format!("/providers/{}/connect", provider_id));
    http_post(&connect_url, &json!({}))?;
    println!(
        "{} Provider '{}' created and connected with runner '{}'.",
        "✓".bright_green(),
        provider_id,
        runner_id
    );

    Ok(())
}

fn handle_weixin_provider_setup(host: &str, port: u16, args: &ProviderAddArgs) -> Result<()> {
    let runner_id = resolve_provider_setup_runner(host, port, args.runner.as_deref())?;
    println!(
        "{} Starting Weixin provider setup for '{}'.",
        "●".bright_cyan(),
        args.id.bright_white().bold()
    );

    let provider_body = build_setup_provider_body(args, "weixin", &runner_id);
    let create_url = api_url(host, port, "/providers");
    let resp = http_post(&create_url, &provider_body)?;
    let provider_id = resp["id"].as_str().unwrap_or(&args.id).to_string();

    let start_url = api_url(
        host,
        port,
        &format!("/providers/{}/weixin-login/start", provider_id),
    );
    let start = http_post(&start_url, &json!({}))?;
    let scan_url = required_string(&start, "scan_url")?.to_string();
    let expires_in_seconds = start["expires_in_seconds"].as_u64().unwrap_or(120);
    let mut interval_seconds = setup_poll_interval_seconds(start["interval_seconds"].as_u64(), 2);
    let expires_at =
        chrono::Utc::now().timestamp_millis() + (expires_in_seconds as i64).saturating_mul(1000);

    println!("{} Scan this QR code to continue:", "→".bright_cyan());
    print_terminal_qr_code(&scan_url);
    println!(
        "{} Waiting for Weixin QR login confirmation. Press Ctrl+C to stop waiting.",
        "…".bright_yellow()
    );

    loop {
        thread::sleep(Duration::from_secs(interval_seconds));
        let status_url = api_url(
            host,
            port,
            &format!("/providers/{}/weixin-login/status", provider_id),
        );
        let status = http_get(&status_url)?;
        match status["status"].as_str().unwrap_or("pending") {
            "confirmed" | "authorized" => break,
            "expired" => {
                return Err(bifrost_core::BifrostError::Config(
                    "Weixin QR login expired before confirmation".to_string(),
                ));
            }
            "pending" | "idle" => {
                interval_seconds = setup_poll_interval_seconds(
                    status["interval_seconds"].as_u64(),
                    interval_seconds,
                );
                if let Some(remaining) = setup_remaining_seconds(&status, expires_at) {
                    println!(
                        "{} Still waiting for QR login confirmation ({}s remaining).",
                        "…".bright_yellow(),
                        remaining
                    );
                } else {
                    println!(
                        "{}",
                        "… Still waiting for QR login confirmation.".bright_yellow()
                    );
                }
            }
            other => {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "unexpected Weixin login status: {other}"
                )));
            }
        }
    }

    let connect_url = api_url(host, port, &format!("/providers/{}/connect", provider_id));
    http_post(&connect_url, &json!({}))?;
    println!(
        "{} Provider '{}' created and connected with runner '{}'.",
        "✓".bright_green(),
        provider_id,
        runner_id
    );

    Ok(())
}

fn resolve_provider_setup_runner(
    host: &str,
    port: u16,
    requested_runner: Option<&str>,
) -> Result<String> {
    let runners = load_runner_choices(host, port)?;
    resolve_runner_choice(requested_runner, &runners)
}

fn load_runner_choices(host: &str, port: u16) -> Result<Vec<RunnerChoice>> {
    let url = api_url(host, port, "/chat/config");
    let config = http_get(&url)?;
    Ok(runner_choices_from_config(&config))
}

fn runner_choices_from_config(config: &Value) -> Vec<RunnerChoice> {
    let mut runners: Vec<_> = config
        .get("runners")
        .and_then(|value| value.as_object())
        .into_iter()
        .flat_map(|runners| runners.iter())
        .map(|(id, settings)| RunnerChoice {
            id: id.to_string(),
            adapter: settings["adapter"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            enabled: settings["enabled"].as_bool().unwrap_or(false),
        })
        .collect();
    runners.sort_by(|a, b| a.id.cmp(&b.id));
    runners
}

fn resolve_runner_choice(
    requested_runner: Option<&str>,
    runners: &[RunnerChoice],
) -> Result<String> {
    resolve_runner_choice_with_terminal(requested_runner, runners, io::stdin().is_terminal())
}

fn resolve_runner_choice_with_terminal(
    requested_runner: Option<&str>,
    runners: &[RunnerChoice],
    stdin_is_terminal: bool,
) -> Result<String> {
    let enabled: Vec<_> = runners.iter().filter(|runner| runner.enabled).collect();
    if enabled.is_empty() {
        return Err(bifrost_core::BifrostError::Config(format!(
            "no enabled runners found. Default built-in runners include: {DEFAULT_BUILTIN_RUNNERS_HINT}"
        )));
    }

    if let Some(requested_runner) = requested_runner
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(runner) = find_runner_choice(requested_runner, &enabled) {
            return Ok(runner.id.clone());
        }
        return Err(bifrost_core::BifrostError::Config(format!(
            "unknown or disabled runner '{}'. Available runners: {}. Default built-in runners include: {}",
            requested_runner,
            format_runner_choices(&enabled),
            DEFAULT_BUILTIN_RUNNERS_HINT
        )));
    }

    if !stdin_is_terminal {
        return Err(bifrost_core::BifrostError::Config(format!(
            "--runner is required when stdin is not interactive. Available runners: {}. Default built-in runners include: {}",
            format_runner_choices(&enabled),
            DEFAULT_BUILTIN_RUNNERS_HINT
        )));
    }

    let labels: Vec<_> = enabled
        .iter()
        .map(|runner| format!("{} ({})", runner.id, runner.adapter))
        .collect();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select runner")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|error| {
            bifrost_core::BifrostError::Config(format!(
                "unable to select runner interactively: {error}"
            ))
        })?;
    Ok(enabled[selection].id.clone())
}

fn find_runner_choice<'a>(
    requested_runner: &str,
    runners: &'a [&'a RunnerChoice],
) -> Option<&'a RunnerChoice> {
    runners
        .iter()
        .copied()
        .find(|runner| runner.id == requested_runner)
        .or_else(|| {
            runners
                .iter()
                .copied()
                .find(|runner| runner.id.eq_ignore_ascii_case(requested_runner))
        })
        .or_else(|| match requested_runner.to_ascii_lowercase().as_str() {
            "codex" => runners
                .iter()
                .copied()
                .find(|runner| runner.id.eq_ignore_ascii_case("codex")),
            "traex" | "trae" => runners
                .iter()
                .copied()
                .find(|runner| runner.id.eq_ignore_ascii_case("traex")),
            "claude_code" | "claude-code" | "claude" | "claude code" => {
                runners.iter().copied().find(|runner| {
                    runner.id.eq_ignore_ascii_case("Claude-Code")
                        || runner.id.eq_ignore_ascii_case("Claude Code")
                })
            }
            _ => None,
        })
}

fn format_runner_choices(runners: &[&RunnerChoice]) -> String {
    runners
        .iter()
        .map(|runner| format!("{} ({})", runner.id, runner.adapter))
        .collect::<Vec<_>>()
        .join(", ")
}

fn setup_remaining_seconds(status: &Value, fallback_expires_at: i64) -> Option<i64> {
    let expires_at = status["expires_at"].as_i64().unwrap_or(fallback_expires_at);
    if expires_at <= 0 {
        return None;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    Some((expires_at - now_ms).max(0) / 1000)
}

fn setup_poll_interval_seconds(value: Option<u64>, default: u64) -> u64 {
    let value = value.unwrap_or(default).min(60);
    if cfg!(test) {
        value
    } else {
        value.max(1)
    }
}

fn print_terminal_qr_code(value: &str) {
    match render_terminal_qr_code(value) {
        Ok(image) => {
            println!();
            println!("{image}");
        }
        Err(error) => {
            println!(
                "{} Failed to render QR code: {}",
                "!".bright_yellow(),
                error
            );
        }
    }
}

fn render_terminal_qr_code(value: &str) -> std::result::Result<String, qrcode::types::QrError> {
    let code = QrCode::new(value.as_bytes())?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .module_dimensions(1, 1)
        .build())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| bifrost_core::BifrostError::Parse(format!("response missing '{field}'")))
}

fn parse_provider_update_args(args: &[String]) -> Result<Value> {
    let mut body = json!({});

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--display-name" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["display_name"] = json!(v);
                }
            }
            "--enable-long-connection" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["event_connection_enabled"] = json!(v.parse::<bool>().unwrap_or(false));
                }
            }
            "--enabled" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["enabled"] = json!(v.parse::<bool>().unwrap_or(true));
                }
            }
            "--base-url" => {
                return Err(bifrost_core::BifrostError::Config(
                    "base_url is managed by system and cannot be set via CLI".to_string(),
                ));
            }
            _ => {}
        }
        i += 1;
    }

    Ok(body)
}

fn print_provider_list(resp: &Value) {
    let empty = vec![];
    let providers = resp.as_array().unwrap_or(&empty);
    if providers.is_empty() {
        println!("{}", "No IM providers configured.".dimmed());
        return;
    }

    println!(
        "{:<20} {:<10} {:<10} {:<15} {}",
        "ID".bold(),
        "TYPE".bold(),
        "ENABLED".bold(),
        "CONNECTION".bold(),
        "APP_ID".bold()
    );
    println!("{}", "─".repeat(72));

    for p in providers {
        let id = p["id"].as_str().unwrap_or("-");
        let ptype = p["provider_type"].as_str().unwrap_or("-");
        let enabled = p["enabled"].as_bool().unwrap_or(false);
        let conn = if p["event_connection_enabled"].as_bool().unwrap_or(false) {
            "long-conn"
        } else {
            "webhook"
        };
        let app_id = p["app_id"].as_str().unwrap_or("-");
        let masked_app_id = if app_id.chars().count() > 8 {
            format!("{}***", truncate_chars_with_suffix(app_id, 8, ""))
        } else {
            app_id.to_string()
        };

        let enabled_str = if enabled {
            "yes".bright_green().to_string()
        } else {
            "no".bright_red().to_string()
        };

        println!(
            "{:<20} {:<10} {:<10} {:<15} {}",
            id, ptype, enabled_str, conn, masked_app_id
        );
    }
}

fn print_provider_status(name: &str, resp: &Value) {
    println!(
        "{} Provider: {}",
        "●".bright_cyan(),
        name.bright_white().bold()
    );
    let state = resp["state"].as_str().unwrap_or("unknown");
    let state_colored = match state {
        "connected" => state.bright_green().to_string(),
        "connecting" | "reconnecting" => state.bright_yellow().to_string(),
        "disconnected" | "failed" => state.bright_red().to_string(),
        _ => state.dimmed().to_string(),
    };
    println!("  State:           {}", state_colored);

    if let Some(ts) = resp["last_connected_at"].as_i64() {
        println!("  Last Connected:  {}", format_timestamp(ts));
    }
    if let Some(ts) = resp["last_event_at"].as_i64() {
        println!("  Last Event:      {}", format_timestamp(ts));
    }
    if let Some(count) = resp["reconnect_count"].as_u64() {
        println!("  Reconnects:      {}", count);
    }
    if let Some(err) = resp["last_error"].as_str() {
        println!("  Last Error:      {}", err.bright_red());
    }
}

fn print_provider_capabilities(resp: &Value, format: &str) -> Result<()> {
    if format == "json" {
        let rendered = serde_json::to_string(resp).map_err(im_json_error)?;
        println!("{rendered}");
        return Ok(());
    }
    if format == "json-pretty" {
        let rendered = serde_json::to_string_pretty(resp).map_err(im_json_error)?;
        println!("{rendered}");
        return Ok(());
    }
    let provider_id = resp["provider_id"].as_str().unwrap_or("unknown");
    let provider_type = resp["provider_type"].as_str().unwrap_or("unknown");
    println!("Provider '{provider_id}' ({provider_type})");
    println!("  Destinations: {}", string_array(&resp["destinations"]));
    println!(
        "  Receive ID types: {}",
        string_array(&resp["receive_id_types"])
    );
    let requires_context = resp["requires_context"].as_bool().unwrap_or(false);
    println!("  Requires inbound context: {requires_context}");
    if let Some(parts) = resp["parts"].as_object() {
        println!("  Content parts:");
        for (kind, capability) in parts {
            let mut detail = capability["support"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            if let Some(delivered_as) = capability["delivered_as"].as_str() {
                detail.push_str(&format!(" → {delivered_as}"));
            }
            if let Some(max_bytes) = capability["max_bytes"].as_u64() {
                detail.push_str(&format!(" (max {max_bytes} bytes)"));
            }
            println!("    {kind}: {detail}");
            if let Some(reason) = capability["reason"].as_str() {
                println!("      {reason}");
            }
        }
    }
    Ok(())
}
#[rustfmt::skip] fn string_array(value: &Value) -> String { value.as_array().map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default() }
// ─── Target ──────────────────────────────────────────────────────────────────

fn handle_im_target(host: &str, port: u16, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str());
    match sub {
        Some("list") => {
            let url = api_url(host, port, "/targets");
            let resp = http_get(&url)?;
            print_target_list(&resp);
            Ok(())
        }
        Some("add") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("target name required".to_string())
            })?;
            let mut body = parse_target_add_args(name, &args[2..])?;
            ensure_provider_in_body(host, port, &mut body)?;
            let url = api_url(host, port, "/targets");
            let resp = http_post(&url, &body)?;
            println!(
                "{} Target '{}' created.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("update") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("target name required".to_string())
            })?;
            let body = parse_target_update_args(&args[2..])?;
            let url = api_url(host, port, &format!("/targets/{}", name));
            let resp = http_patch(&url, &body)?;
            println!(
                "{} Target '{}' updated.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("delete") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("target name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/targets/{}", name));
            http_delete(&url)?;
            println!("{} Target '{}' deleted.", "✓".bright_green(), name);
            Ok(())
        }
        _ => {
            eprintln!("Usage: bifrost im target <list|add|update|delete>");
            Ok(())
        }
    }
}

fn parse_target_add_args(name: &str, args: &[String]) -> Result<Value> {
    let mut body = json!({
        "id": name,
        "display_name": name,
        "default_msg_type": "text",
        "enabled": true,
    });

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--provider" => {
                i += 1;
                body["provider_id"] = json!(required_send_arg(args, i, "--provider")?);
            }
            "--receive-id-type" => {
                i += 1;
                body["receive_id_type"] = json!(required_send_arg(args, i, "--receive-id-type")?);
            }
            "--receive-id" => {
                i += 1;
                body["receive_id"] = json!(required_send_arg(args, i, "--receive-id")?);
            }
            "--display-name" => {
                i += 1;
                body["display_name"] = json!(required_send_arg(args, i, "--display-name")?);
            }
            "--msg-type" => {
                i += 1;
                body["default_msg_type"] = json!(required_send_arg(args, i, "--msg-type")?);
            }
            value => {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "unknown im target add option '{value}'"
                )))
            }
        }
        i += 1;
    }

    if body["receive_id_type"].as_str().is_none() || body["receive_id"].as_str().is_none() {
        return Err(bifrost_core::BifrostError::Config(
            "--receive-id-type and --receive-id are required".to_string(),
        ));
    }

    Ok(body)
}

fn parse_target_update_args(args: &[String]) -> Result<Value> {
    let mut body = json!({});

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--receive-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["receive_id"] = json!(v);
                }
            }
            "--display-name" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["display_name"] = json!(v);
                }
            }
            "--enabled" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["enabled"] = json!(v.parse::<bool>().unwrap_or(true));
                }
            }
            _ => {}
        }
        i += 1;
    }

    Ok(body)
}

fn print_target_list(resp: &Value) {
    let empty = vec![];
    let targets = resp.as_array().unwrap_or(&empty);
    if targets.is_empty() {
        println!("{}", "No IM targets configured.".dimmed());
        return;
    }

    println!(
        "{:<20} {:<15} {:<12} {:<10} {}",
        "ID".bold(),
        "PROVIDER".bold(),
        "ID_TYPE".bold(),
        "ENABLED".bold(),
        "RECEIVE_ID".bold()
    );
    println!("{}", "─".repeat(75));

    for t in targets {
        let id = t["id"].as_str().unwrap_or("-");
        let provider = t["provider_id"].as_str().unwrap_or("-");
        let id_type = t["receive_id_type"].as_str().unwrap_or("-");
        let enabled = t["enabled"].as_bool().unwrap_or(false);
        let receive_id = t["receive_id"].as_str().unwrap_or("-");
        let masked_id = if receive_id.chars().count() > 10 {
            format!("{}***", truncate_chars_with_suffix(receive_id, 10, ""))
        } else {
            receive_id.to_string()
        };

        let enabled_str = if enabled {
            "yes".bright_green().to_string()
        } else {
            "no".bright_red().to_string()
        };

        println!(
            "{:<20} {:<15} {:<12} {:<10} {}",
            id, provider, id_type, enabled_str, masked_id
        );
    }
}

// ─── Send ────────────────────────────────────────────────────────────────────

fn handle_im_send(host: &str, port: u16, args: &[String]) -> Result<()> {
    let send_args = parse_send_args(args)?;
    if send_args.help {
        print_im_send_help();
        return Ok(());
    }
    let provider_id = resolve_send_provider_id(host, port, &send_args)?;

    let capabilities_path = format!("/providers/{provider_id}/capabilities");
    let capabilities_url = api_url(host, port, &capabilities_path);
    let capabilities = http_get(&capabilities_url)?;
    let parts = prepare_send_parts(host, port, &provider_id, &send_args, &capabilities)?;
    let body = build_send_body(&provider_id, &send_args, parts)?;

    let url = api_url(host, port, "/messages/send");
    let (_http_status, resp) = http_post_with_status(&url, &body)?;
    print_send_response(&resp, &send_args.output_format)?;
    (resp["status"].as_str() == Some("success"))
        .then_some(())
        .ok_or_else(|| {
            bifrost_core::BifrostError::Config(format!(
                "IM send completed with status '{}'",
                resp["status"].as_str().unwrap_or("failed")
            ))
        })
}
#[rustfmt::skip] enum ImSendPartArg { Text(String), Markdown(String), MarkdownFile(String), ImageFile(String), ImageKey(String), File(String), FileKey(String), CardFile(String), CardJson(String) }
#[derive(Default)]
#[rustfmt::skip] struct ImSendArgs { provider: Option<String>, bot_id: Option<String>, bot_name: Option<String>, target: Option<String>, chat_id: Option<String>, receive_id_type: Option<String>, receive_id: Option<String>, owner: bool, parts: Vec<ImSendPartArg>, card_title: Option<String>, card_text: Option<String>, card_image_file: Option<String>, card_image_key: Option<String>, card_image_alt: Option<String>, image_type: Option<String>, idempotency_key: Option<String>, output_format: String, help: bool }
impl std::fmt::Debug for ImSendArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImSendArgs")
            .field("provider", &self.provider)
            .field("bot_id", &self.bot_id)
            .field("bot_name", &self.bot_name)
            .field(
                "destination",
                &self.target.as_ref().or(self.chat_id.as_ref()),
            )
            .field("part_count", &self.parts.len())
            .field("output_format", &self.output_format)
            .finish_non_exhaustive()
    }
}

fn parse_send_args(args: &[String]) -> Result<ImSendArgs> {
    let mut parsed = ImSendArgs {
        output_format: "human".to_string(),
        ..ImSendArgs::default()
    };
    let mut positional_provider = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => parsed.help = true,
            "--provider" => parsed.provider = Some(next_send_arg(args, &mut i)?),
            "--bot-id" => parsed.bot_id = Some(next_send_arg(args, &mut i)?),
            "--bot-name" => parsed.bot_name = Some(next_send_arg(args, &mut i)?),
            "--target" => parsed.target = Some(next_send_arg(args, &mut i)?),
            "--owner" => parsed.owner = true,
            "--chat-id" => parsed.chat_id = Some(next_send_arg(args, &mut i)?),
            "--receive-id-type" => parsed.receive_id_type = Some(next_send_arg(args, &mut i)?),
            "--receive-id" => parsed.receive_id = Some(next_send_arg(args, &mut i)?),
            "--text" => parsed
                .parts
                .push(ImSendPartArg::Text(next_send_arg(args, &mut i)?)),
            "--markdown" => parsed
                .parts
                .push(ImSendPartArg::Markdown(next_send_arg(args, &mut i)?)),
            "--markdown-file" => parsed
                .parts
                .push(ImSendPartArg::MarkdownFile(next_send_arg(args, &mut i)?)),
            "--image" | "--image-file" => parsed
                .parts
                .push(ImSendPartArg::ImageFile(next_send_arg(args, &mut i)?)),
            "--image-key" => parsed
                .parts
                .push(ImSendPartArg::ImageKey(next_send_arg(args, &mut i)?)),
            "--file" => parsed
                .parts
                .push(ImSendPartArg::File(next_send_arg(args, &mut i)?)),
            "--file-key" => parsed
                .parts
                .push(ImSendPartArg::FileKey(next_send_arg(args, &mut i)?)),
            "--card-file" => parsed
                .parts
                .push(ImSendPartArg::CardFile(next_send_arg(args, &mut i)?)),
            "--card-json" => parsed
                .parts
                .push(ImSendPartArg::CardJson(next_send_arg(args, &mut i)?)),
            "--card-title" => parsed.card_title = Some(next_send_arg(args, &mut i)?),
            "--card-text" => parsed.card_text = Some(next_send_arg(args, &mut i)?),
            "--card-image-file" => parsed.card_image_file = Some(next_send_arg(args, &mut i)?),
            "--card-image-key" => parsed.card_image_key = Some(next_send_arg(args, &mut i)?),
            "--card-image-alt" => parsed.card_image_alt = Some(next_send_arg(args, &mut i)?),
            "--image-type" => parsed.image_type = Some(next_send_arg(args, &mut i)?),
            "--idempotency-key" => parsed.idempotency_key = Some(next_send_arg(args, &mut i)?),
            "--format" => parsed.output_format = next_send_arg(args, &mut i)?,
            value if value.starts_with('-') => {
                return send_config_error(format!("unknown im send option '{value}'"))
            }
            value => {
                if positional_provider.replace(value.to_string()).is_some() {
                    return send_config_error("only one provider positional argument is allowed");
                }
            }
        }
        i += 1;
    }

    if parsed.help {
        return Ok(parsed);
    }
    if parsed.provider.is_some() && positional_provider.is_some() {
        return send_config_error(
            "provider positional argument and --provider are mutually exclusive",
        );
    }
    parsed.provider = parsed.provider.or(positional_provider);
    if parsed.provider.is_some() && (parsed.bot_id.is_some() || parsed.bot_name.is_some()) {
        return send_config_error(
            "PROVIDER/--provider and --bot-id/--bot-name are mutually exclusive",
        );
    }
    let destination_count = usize::from(parsed.owner)
        + usize::from(parsed.target.is_some())
        + usize::from(parsed.chat_id.is_some())
        + usize::from(parsed.receive_id.is_some() || parsed.receive_id_type.is_some());
    if destination_count > 1 {
        return send_config_error(
            "--owner, --target, --chat-id, and --receive-id are mutually exclusive",
        );
    }
    if parsed.receive_id.is_some() != parsed.receive_id_type.is_some() {
        return send_config_error("--receive-id and --receive-id-type must be provided together");
    }
    if !matches!(
        parsed.output_format.as_str(),
        "human" | "json" | "json-pretty"
    ) {
        return send_config_error("--format must be one of: human, json, json-pretty");
    }
    let has_rich_card = parsed.card_title.is_some()
        || parsed.card_text.is_some()
        || parsed.card_image_file.is_some()
        || parsed.card_image_key.is_some()
        || parsed.card_image_alt.is_some();
    if parsed.parts.is_empty() && !has_rich_card {
        return send_config_error("at least one of --text, --markdown, --markdown-file, --image, --file, --card-file, or --card-json is required");
    }
    Ok(parsed)
}
fn resolve_send_provider_id(host: &str, port: u16, args: &ImSendArgs) -> Result<String> {
    if let Some(provider_id) = &args.provider {
        return Ok(provider_id.clone());
    }
    if args.bot_id.is_some() || args.bot_name.is_some() {
        let mut body = json!({});
        if let Some(bot_id) = &args.bot_id {
            body["bot_id"] = json!(bot_id);
        }
        if let Some(bot_name) = &args.bot_name {
            body["bot_name"] = json!(bot_name);
        }
        let response = http_post(&api_url(host, port, "/providers/resolve"), &body)?;
        return resolved_provider_id(&response);
    }
    select_provider_interactively(host, port)
}
#[rustfmt::skip] fn required_send_arg<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str> { args.get(index).map(String::as_str).filter(|value| !value.is_empty()).ok_or_else(|| bifrost_core::BifrostError::Config(format!("{flag} requires a non-empty value"))) }
fn next_send_arg(args: &[String], index: &mut usize) -> Result<String> {
    let flag = args[*index].as_str();
    *index += 1;
    required_send_arg(args, *index, flag).map(str::to_string)
}
#[rustfmt::skip] fn send_config_error<T>(message: impl Into<String>) -> Result<T> { Err(bifrost_core::BifrostError::Config(message.into())) }
#[rustfmt::skip] fn resolved_provider_id(response: &Value) -> Result<String> { response["provider_id"].as_str().filter(|value| !value.trim().is_empty()).map(str::to_string).ok_or_else(|| bifrost_core::BifrostError::Parse("provider resolve response is missing provider_id".to_string())) }
fn build_send_body(provider_id: &str, args: &ImSendArgs, parts: Vec<Value>) -> Result<Value> {
    let destination = if let Some(target_id) = &args.target {
        json!({ "mode": "target", "target_id": target_id })
    } else if let Some(chat_id) = &args.chat_id {
        json!({ "mode": "direct", "receive_id_type": "chat_id", "receive_id": chat_id })
    } else if let (Some(receive_id_type), Some(receive_id)) =
        (&args.receive_id_type, &args.receive_id)
    {
        json!({ "mode": "direct", "receive_id_type": receive_id_type, "receive_id": receive_id })
    } else {
        json!({ "mode": "owner" })
    };
    let mut body =
        json!({ "provider_id": provider_id, "destination": destination, "parts": parts });
    if let Some(key) = &args.idempotency_key {
        body["idempotency_key"] = json!(key);
    }
    Ok(body)
}
fn prepare_send_parts(
    host: &str,
    port: u16,
    provider_id: &str,
    args: &ImSendArgs,
    capabilities: &Value,
) -> Result<Vec<Value>> {
    let mut parts = Vec::new();
    for part in &args.parts {
        let kind = match part {
            ImSendPartArg::Text(_) => "text",
            ImSendPartArg::Markdown(_) | ImSendPartArg::MarkdownFile(_) => "markdown",
            ImSendPartArg::ImageFile(_) | ImSendPartArg::ImageKey(_) => "image",
            ImSendPartArg::File(_) | ImSendPartArg::FileKey(_) => "file",
            ImSendPartArg::CardFile(_) | ImSendPartArg::CardJson(_) => "native_card",
        };
        ensure_send_capability(capabilities, kind)?;
        let value = match part {
            ImSendPartArg::Text(text) => scalar_send_part("text", "text", text.as_str()),
            ImSendPartArg::Markdown(text) => scalar_send_part("markdown", "text", text.as_str()),
            ImSendPartArg::MarkdownFile(path) => {
                scalar_send_part("markdown", "text", read_text_send_file(path, "Markdown")?)
            }
            ImSendPartArg::ImageFile(path) => scalar_send_part(
                "image",
                "image_key",
                upload_send_key(host, port, provider_id, "image", path, args, capabilities)?,
            ),
            ImSendPartArg::ImageKey(key) => scalar_send_part("image", "image_key", key.as_str()),
            ImSendPartArg::File(path) => {
                let key =
                    upload_send_key(host, port, provider_id, "file", path, args, capabilities)?;
                file_send_part(key, send_file_name(path))
            }
            ImSendPartArg::FileKey(key) => scalar_send_part("file", "file_key", key.as_str()),
            ImSendPartArg::CardFile(path) => {
                native_card_send_part(parse_card_json(&read_text_send_file(path, "card JSON")?)?)
            }
            ImSendPartArg::CardJson(card) => native_card_send_part(parse_card_json(card)?),
        };
        parts.push(value);
    }
    if args.card_title.is_some()
        || args.card_text.is_some()
        || args.card_image_file.is_some()
        || args.card_image_key.is_some()
        || args.card_image_alt.is_some()
    {
        ensure_send_capability(capabilities, "native_card")?;
        let image_key = if let Some(path) = &args.card_image_file {
            ensure_send_capability(capabilities, "image")?;
            Some(upload_send_key(
                host,
                port,
                provider_id,
                "image",
                path,
                args,
                capabilities,
            )?)
        } else {
            args.card_image_key.as_ref().map(|key| json!(key))
        };
        let mut elements = Vec::new();
        if let Some(image_key) = image_key {
            elements.push(card_image_element(
                image_key,
                args.card_image_alt.as_deref().unwrap_or("image"),
            ));
        }
        if let Some(text) = &args.card_text {
            elements.push(json!({ "tag": "markdown", "content": text }));
        }
        let mut card = json!({ "config": { "wide_screen_mode": true }, "elements": elements });
        if let Some(title) = &args.card_title {
            card["header"] = card_header(title);
        }
        parts.push(json!({ "type": "native_card", "card": card }));
    }

    Ok(parts)
}
fn scalar_send_part(kind: &str, field: &str, value: impl Into<Value>) -> Value {
    let mut part = json!({ "type": kind });
    part[field] = value.into();
    part
}
fn file_send_part(key: Value, file_name: &str) -> Value {
    json!({ "type": "file", "file_key": key, "file_name": file_name })
}

fn native_card_send_part(card: Value) -> Value {
    json!({ "type": "native_card", "card": card })
}
fn card_image_element(image_key: Value, alt: &str) -> Value {
    json!({ "tag": "img", "img_key": image_key, "alt": { "tag": "plain_text", "content": alt } })
}

fn card_header(title: &str) -> Value {
    json!({ "template": "blue", "title": { "tag": "plain_text", "content": title } })
}
fn send_file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
}
fn upload_send_key(
    host: &str,
    port: u16,
    provider_id: &str,
    kind: &str,
    path: &str,
    args: &ImSendArgs,
    capabilities: &Value,
) -> Result<Value> {
    let (image_type, fallback) = if kind == "image" {
        (args.image_type.as_deref(), 10 * 1024 * 1024)
    } else {
        (None, 30 * 1024 * 1024)
    };
    Ok(upload_send_file(
        host,
        port,
        provider_id,
        kind,
        path,
        image_type,
        send_capability_max_bytes(capabilities, kind, fallback),
    )?["key"]
        .clone())
}
fn ensure_send_capability(capabilities: &Value, kind: &str) -> Result<()> {
    let capability = capabilities
        .get("parts")
        .and_then(|parts| parts.get(kind))
        .ok_or_else(|| {
            bifrost_core::BifrostError::Config(format!(
                "provider does not declare the '{kind}' send capability"
            ))
        })?;
    if capability["support"].as_str() == Some("unsupported") {
        return Err(bifrost_core::BifrostError::Config(
            capability["reason"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("provider does not support {kind}")),
        ));
    }
    Ok(())
}

fn send_capability_max_bytes(capabilities: &Value, kind: &str, fallback: u64) -> u64 {
    capabilities["parts"][kind]["max_bytes"]
        .as_u64()
        .unwrap_or(fallback)
}

fn read_text_send_file(path: &str, label: &str) -> Result<String> {
    fs::read_to_string(Path::new(path)).map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("failed to read {label} file '{path}': {error}"),
        ))
    })
}
fn parse_card_json(content: &str) -> Result<Value> {
    let card: Value = serde_json::from_str(content).map_err(|error| {
        bifrost_core::BifrostError::Parse(format!("invalid card JSON: {error}"))
    })?;
    if !card.is_object() {
        return Err(bifrost_core::BifrostError::Config(
            "card JSON must be an object".to_string(),
        ));
    }
    Ok(card)
}
fn upload_send_file(
    host: &str,
    port: u16,
    provider_id: &str,
    kind: &str,
    path: &str,
    image_type: Option<&str>,
    max_bytes: u64,
) -> Result<Value> {
    let path_ref = Path::new(path);
    let metadata = fs::metadata(path_ref).map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("failed to inspect {kind} file '{path}': {error}"),
        ))
    })?;
    if !metadata.is_file() {
        return send_config_error(format!("{kind} path '{path}' is not a regular file"));
    }
    if metadata.len() == 0 || metadata.len() > max_bytes {
        return send_config_error(format!(
            "{kind} file must be between 1 and {max_bytes} bytes"
        ));
    }
    let bytes = fs::read(path_ref).map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("failed to read {kind} file '{path}': {error}"),
        ))
    })?;
    let file_name = path_ref
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            bifrost_core::BifrostError::Config("file name must be valid UTF-8".to_string())
        })?;
    let mime_type = upload_mime_type(kind, file_name);
    let mut url = upload_send_url(host, port, provider_id, kind, file_name, mime_type);
    if let Some(image_type) = image_type {
        url.push_str("&image_type=");
        url.push_str(&urlencoding::encode(image_type));
    }
    http_post_bytes(&url, &bytes, mime_type)
}
#[rustfmt::skip] fn upload_mime_type(kind: &str, file_name: &str) -> &'static str { if kind == "image" { guess_image_mime_type(file_name).unwrap_or("application/octet-stream") } else { "application/octet-stream" } }
#[rustfmt::skip] fn upload_send_url(host: &str, port: u16, provider_id: &str, kind: &str, file_name: &str, mime_type: &str) -> String { format!("{}?provider_id={}&kind={}&file_name={}&mime_type={}", api_url(host, port, "/messages/upload"), urlencoding::encode(provider_id), urlencoding::encode(kind), urlencoding::encode(file_name), urlencoding::encode(mime_type)) }
fn print_send_response(response: &Value, format: &str) -> Result<()> {
    let rendered = match format {
        "json" => serde_json::to_string(response).map_err(im_json_error)?,
        "json-pretty" => serde_json::to_string_pretty(response).map_err(im_json_error)?,
        _ => format_send_response_human(response),
    };
    println!("{rendered}");
    Ok(())
}
#[rustfmt::skip]
fn format_send_response_human(response: &Value) -> String {
    let status = response["status"].as_str().unwrap_or("failed");
    let glyph = if status == "success" { "✓" } else { "!" };
    let bundle = response["bundle_id"].as_str().unwrap_or("unknown");
    let provider = response["provider_id"].as_str().unwrap_or("unknown");
    let destination = response["destination"].as_str().unwrap_or("unknown");
    let mut lines = vec![format!("{glyph} IM bundle {bundle} via '{provider}' to {destination}: {status}")];
    for receipt in response["receipts"].as_array().into_iter().flatten() {
        let index = receipt["index"].as_u64().unwrap_or(0) + 1;
        let receipt_status = receipt["status"].as_str().unwrap_or("failed");
        let glyph = if receipt_status == "success" { "✓" } else { "✗" };
        let requested = receipt["requested_kind"].as_str().unwrap_or("unknown");
        let delivered = receipt["delivered_kind"].as_str().unwrap_or("unknown");
        let suffix = receipt["message_id"].as_str().map(|value| format!(" ({value})")).unwrap_or_default();
        lines.push(format!("  {glyph} part {index} {requested} → {delivered}{suffix}"));
        if let Some(warning) = receipt["warning"].as_str() {
            lines.push(format!("    warning: {warning}"));
        }
        if let Some(error) = receipt["error"].as_str() {
            lines.push(format!("    error: {error}"));
        }
    }
    lines.join("\n")
}
#[rustfmt::skip] fn im_json_error(error: serde_json::Error) -> bifrost_core::BifrostError { bifrost_core::BifrostError::Parse(format!("failed to format response: {error}")) }
fn print_im_send_help() {
    println!("bifrost im send - send ordered content parts through an IM provider");
    println!();
    println!("USAGE:");
    println!("    bifrost im send [PROVIDER] [DESTINATION] <CONTENT>...");
    println!();
    println!("DESTINATION (choose at most one):");
    println!("    --owner                         Send to provider owner (default)");
    println!("    --target <ALIAS>                Send to a configured target");
    println!("    --chat-id <ID>                  Send directly to a Feishu group chat");
    println!("    --receive-id-type <TYPE> --receive-id <ID>");
    println!();
    println!("CONTENT (repeatable, sent in argument order):");
    println!("    --text <TEXT>");
    println!("    --markdown <MARKDOWN> | --markdown-file <PATH>");
    println!("    --image <PATH> | --image-file <PATH> | --image-key <KEY>");
    println!("    --file <PATH> | --file-key <KEY>");
    println!("    --card-file <PATH> | --card-json <JSON>");
    println!("    --card-title <TITLE> [--card-text <MARKDOWN>]");
    println!("    [--card-image-file <PATH> | --card-image-key <KEY>] [--card-image-alt <TEXT>]");
    println!(
        "    Video-capable providers accept video paths through --file; there is no --video flag"
    );
    println!();
    println!("OPTIONS:");
    println!("    --provider <ID>                 Compatibility form for PROVIDER");
    println!("    --bot-id <APP_ID>               Resolve Feishu provider by bot App ID");
    println!("    --bot-name <NAME>               Resolve exact name; reject ambiguity");
    println!("    --idempotency-key <KEY>         Stable bundle idempotency key");
    println!("    --format human|json|json-pretty");
}

fn guess_image_mime_type(file_name: &str) -> Option<&'static str> {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn select_provider_interactively(host: &str, port: u16) -> Result<String> {
    let url = api_url(host, port, "/providers");
    let resp = http_get(&url)?;
    let providers = enabled_provider_choices(&resp);

    if providers.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "no enabled IM providers found; create one with `bifrost im provider add`".to_string(),
        ));
    }

    if providers.len() == 1 {
        return Ok(providers[0].0.clone());
    }

    if !io::stdin().is_terminal() {
        return Err(bifrost_core::BifrostError::Config(
            "--provider is required when stdin is not interactive".to_string(),
        ));
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    choose_provider_from_reader(&providers, stdin.lock(), stdout.lock())
}

fn ensure_provider_in_body(host: &str, port: u16, body: &mut Value) -> Result<()> {
    if body
        .get("provider_id")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }

    let provider_id = select_provider_interactively(host, port)?;
    ensure_provider_value(body, &provider_id);
    Ok(())
}

fn ensure_provider_value(body: &mut Value, provider_id: &str) {
    if body
        .get("provider_id")
        .and_then(|value| value.as_str())
        .is_none_or(|value| value.trim().is_empty())
    {
        body["provider_id"] = json!(provider_id);
    }
}

fn enabled_provider_choices(resp: &Value) -> Vec<(String, String)> {
    resp.as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["enabled"].as_bool().unwrap_or(false))
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            let display = p["display_name"].as_str().unwrap_or(id);
            Some((id.to_string(), display.to_string()))
        })
        .collect()
}

fn choose_provider_from_reader<R, W>(
    providers: &[(String, String)],
    mut reader: R,
    mut writer: W,
) -> Result<String>
where
    R: io::BufRead,
    W: Write,
{
    writeln!(writer, "Select IM provider:")?;
    for (idx, (id, display)) in providers.iter().enumerate() {
        writeln!(writer, "  {}) {} ({})", idx + 1, display, id)?;
    }
    write!(writer, "Provider [1-{}]: ", providers.len())?;
    writer.flush()?;

    let mut input = String::new();
    reader.read_line(&mut input)?;
    let choice = input.trim().parse::<usize>().map_err(|_| {
        bifrost_core::BifrostError::Config("invalid provider selection".to_string())
    })?;
    let Some((provider_id, _)) = providers.get(choice.saturating_sub(1)) else {
        return Err(bifrost_core::BifrostError::Config(
            "provider selection out of range".to_string(),
        ));
    };

    Ok(provider_id.clone())
}

// ─── Route ───────────────────────────────────────────────────────────────────

fn handle_im_route(host: &str, port: u16, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str());
    match sub {
        Some("list") => {
            let url = api_url(host, port, "/routes");
            let resp = http_get(&url)?;
            print_route_list(&resp);
            Ok(())
        }
        Some("add") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("route name required".to_string())
            })?;
            let mut body = parse_route_add_args(name, &args[2..])?;
            ensure_provider_in_body(host, port, &mut body)?;
            let url = api_url(host, port, "/routes");
            let resp = http_post(&url, &body)?;
            println!(
                "{} Route '{}' created.",
                "✓".bright_green(),
                resp["id"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        Some("pause") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("route name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/routes/{}/pause", name));
            http_post(&url, &json!({}))?;
            println!("{} Route '{}' paused.", "✓".bright_green(), name);
            Ok(())
        }
        Some("resume") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("route name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/routes/{}/resume", name));
            http_post(&url, &json!({}))?;
            println!("{} Route '{}' resumed.", "✓".bright_green(), name);
            Ok(())
        }
        Some("delete") => {
            let name = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config("route name required".to_string())
            })?;
            let url = api_url(host, port, &format!("/routes/{}", name));
            http_delete(&url)?;
            println!("{} Route '{}' deleted.", "✓".bright_green(), name);
            Ok(())
        }
        _ => {
            eprintln!("Usage: bifrost im route <list|add|pause|resume|delete>");
            Ok(())
        }
    }
}

fn parse_route_add_args(name: &str, args: &[String]) -> Result<Value> {
    let mut body = json!({
        "name": name,
    });
    let mut matcher = json!({});
    let mut action = json!({});

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--provider" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["provider_id"] = json!(v);
                }
            }
            "--event" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["event_type"] = json!(v);
                }
            }
            "--chat-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    matcher["chat_ids"] = json!([v]);
                }
            }
            "--user-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    matcher["user_ids"] = json!([v]);
                }
            }
            "--keyword" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    matcher["keyword"] = json!(v);
                }
            }
            "--regex" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    matcher["regex"] = json!(v);
                }
            }
            "--script-file" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    action["script_file"] = json!(v);
                    action["type"] = json!("script");
                }
            }
            "--script" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    action["script_text"] = json!(v);
                    action["type"] = json!("script");
                }
            }
            "--reply" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    action["reply_mode"] = json!(v);
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

    if !matcher.as_object().is_none_or(|m| m.is_empty()) {
        body["matcher"] = matcher;
    }
    if !action.as_object().is_none_or(|a| a.is_empty()) {
        body["action"] = action;
    }

    Ok(body)
}

fn print_route_list(resp: &Value) {
    let empty = vec![];
    let routes = resp.as_array().unwrap_or(&empty);
    if routes.is_empty() {
        println!("{}", "No IM routes configured.".dimmed());
        return;
    }

    println!(
        "{:<20} {:<15} {:<18} {:<10} {}",
        "NAME".bold(),
        "PROVIDER".bold(),
        "EVENT".bold(),
        "ENABLED".bold(),
        "MATCHER".bold()
    );
    println!("{}", "─".repeat(80));

    for r in routes {
        let name = r["name"].as_str().unwrap_or("-");
        let provider = r["provider_id"].as_str().unwrap_or("-");
        let event = r["event_type"].as_str().unwrap_or("-");
        let enabled = r["enabled"].as_bool().unwrap_or(false);
        let matcher_summary = summarize_matcher(&r["matcher"]);

        let enabled_str = if enabled {
            "yes".bright_green().to_string()
        } else {
            "no".bright_red().to_string()
        };

        println!(
            "{:<20} {:<15} {:<18} {:<10} {}",
            name, provider, event, enabled_str, matcher_summary
        );
    }
}

fn summarize_matcher(matcher: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(regex) = matcher["regex"].as_str() {
        parts.push(format!("regex:{}", regex));
    }
    if let Some(keyword) = matcher["keyword"].as_str() {
        parts.push(format!("kw:{}", keyword));
    }
    if let Some(chats) = matcher["chat_ids"].as_array() {
        if !chats.is_empty() {
            parts.push(format!("chats:{}", chats.len()));
        }
    }
    if parts.is_empty() {
        "*".to_string()
    } else {
        parts.join(", ")
    }
}

// ─── History ─────────────────────────────────────────────────────────────────

fn handle_im_history(host: &str, port: u16, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str());
    match sub {
        Some("events") => {
            let url = api_url(host, port, "/history/events");
            let resp = http_get(&url)?;
            print_events(&resp);
            Ok(())
        }
        Some("runs") => {
            let url = api_url(host, port, "/history/runs");
            let resp = http_get(&url)?;
            print_task_runs(&resp);
            Ok(())
        }
        _ => {
            eprintln!("Usage: bifrost im history <events|runs>");
            Ok(())
        }
    }
}

fn print_events(resp: &Value) {
    let empty = vec![];
    let events = resp.as_array().unwrap_or(&empty);
    if events.is_empty() {
        println!("{}", "No recent events.".dimmed());
        return;
    }

    println!(
        "{:<12} {:<15} {:<20} {:<15} {}",
        "EVENT_ID".bold(),
        "PROVIDER".bold(),
        "EVENT_TYPE".bold(),
        "SOURCE".bold(),
        "TIME".bold()
    );
    println!("{}", "─".repeat(75));

    for e in events.iter().take(50) {
        let event_id = e["event_id"]
            .as_str()
            .map(|s| truncate_chars_with_suffix(s, 10, ""))
            .unwrap_or_else(|| "-".to_string());
        let provider = e["provider_id"].as_str().unwrap_or("-");
        let event_type = e["event_type"].as_str().unwrap_or("-");
        let source = e["source"]["chat_id"]
            .as_str()
            .or_else(|| e["source"]["user_id"].as_str())
            .unwrap_or("-");
        let time = e["received_at"]
            .as_i64()
            .map(format_timestamp)
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<12} {:<15} {:<20} {:<15} {}",
            event_id, provider, event_type, source, time
        );
    }
}

fn print_task_runs(resp: &Value) {
    let empty = vec![];
    let runs = resp.as_array().unwrap_or(&empty);
    if runs.is_empty() {
        println!("{}", "No task runs found.".dimmed());
        return;
    }

    println!(
        "{:<12} {:<12} {:<10} {:<10} {:<8} {}",
        "RUN_ID".bold(),
        "TRIGGER".bold(),
        "STATUS".bold(),
        "DURATION".bold(),
        "EXIT".bold(),
        "TIME".bold()
    );
    println!("{}", "─".repeat(72));

    for r in runs.iter().take(50) {
        let run_id = r["run_id"]
            .as_str()
            .map(|s| truncate_chars_with_suffix(s, 10, ""))
            .unwrap_or_else(|| "-".to_string());
        let trigger = r["trigger_source"].as_str().unwrap_or("-");
        let status = r["status"].as_str().unwrap_or("-");
        let duration = r["duration_ms"]
            .as_u64()
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_else(|| "-".to_string());
        let exit_code = r["exit_code"]
            .as_i64()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        let time = r["started_at"]
            .as_i64()
            .map(format_timestamp)
            .unwrap_or_else(|| "-".to_string());

        let status_colored = match status {
            "success" | "completed" => status.bright_green().to_string(),
            "running" => status.bright_yellow().to_string(),
            "failed" | "error" => status.bright_red().to_string(),
            _ => status.dimmed().to_string(),
        };

        println!(
            "{:<12} {:<12} {:<10} {:<10} {:<8} {}",
            run_id, trigger, status_colored, duration, exit_code, time
        );
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn api_url(host: &str, port: u16, path: &str) -> String {
    format!("http://{}:{}{}{}", host, port, IM_GATEWAY_API_PREFIX, path)
}

fn http_get(url: &str) -> Result<Value> {
    debug!(url = %url, "im: GET");
    let resp = bifrost_core::direct_ureq_agent()
        .get(url)
        .call()
        .map_err(|e| bifrost_core::BifrostError::Network(format!("HTTP GET failed: {}", e)))?;
    let body_str = resp.into_string().map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to read response: {}", e))
    })?;
    let body: Value = serde_json::from_str(&body_str).map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to parse response: {}", e))
    })?;
    Ok(body)
}

fn http_post(url: &str, body: &Value) -> Result<Value> {
    debug!(url = %url, "im: POST");
    let resp = bifrost_core::direct_ureq_agent()
        .post(url)
        .send_json(body)
        .map_err(|e| bifrost_core::BifrostError::Network(format!("HTTP POST failed: {}", e)))?;
    let body_str = resp.into_string().map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to read response: {}", e))
    })?;
    let resp_body: Value = serde_json::from_str(&body_str).map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to parse response: {}", e))
    })?;
    Ok(resp_body)
}

fn http_post_with_status(url: &str, body: &Value) -> Result<(u16, Value)> {
    debug!(url = %url, "im: POST");
    let result = bifrost_core::direct_ureq_agent().post(url).send_json(body);
    let (status, response) = match result {
        Ok(response) => (response.status(), response),
        Err(ureq::Error::Status(status, response)) => (status, response),
        Err(error) => return Err(im_network_error("HTTP POST failed", error)),
    };
    read_im_json_response(response).map(|body| (status, body))
}

fn http_post_bytes(url: &str, bytes: &[u8], content_type: &str) -> Result<Value> {
    debug!(url = %url, bytes = bytes.len(), "im: POST binary upload");
    let request = bifrost_core::direct_ureq_agent()
        .post(url)
        .set("Content-Type", content_type);
    let result = request.send_bytes(bytes);
    match result {
        Ok(response) => read_im_json_response(response),
        Err(ureq::Error::Status(status, response)) => upload_status_error(status, response),
        Err(error) => Err(im_network_error("IM upload failed", error)),
    }
}
#[rustfmt::skip] fn upload_status_error(status: u16, response: ureq::Response) -> Result<Value> { let body = read_im_json_response(response).unwrap_or_else(|_| json!({})); let message = body["error"].as_str().unwrap_or("unknown error"); Err(bifrost_core::BifrostError::Network(format!("IM upload failed with HTTP {status}: {message}"))) }
#[rustfmt::skip] fn im_network_error(context: &str, error: ureq::Error) -> bifrost_core::BifrostError { bifrost_core::BifrostError::Network(format!("{context}: {error}")) }
#[rustfmt::skip] fn read_im_json_response(response: ureq::Response) -> Result<Value> { let body = response.into_string().map_err(|error| bifrost_core::BifrostError::Parse(format!("failed to read response: {error}")))?; serde_json::from_str(&body).map_err(|error| bifrost_core::BifrostError::Parse(format!("failed to parse response: {error}"))) }
fn http_patch(url: &str, body: &Value) -> Result<Value> {
    debug!(url = %url, "im: PATCH");
    let resp = bifrost_core::direct_ureq_agent()
        .request("PATCH", url)
        .send_json(body)
        .map_err(|e| bifrost_core::BifrostError::Network(format!("HTTP PATCH failed: {}", e)))?;
    let body_str = resp.into_string().map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to read response: {}", e))
    })?;
    let resp_body: Value = serde_json::from_str(&body_str).map_err(|e| {
        bifrost_core::BifrostError::Parse(format!("failed to parse response: {}", e))
    })?;
    Ok(resp_body)
}

fn http_delete(url: &str) -> Result<Value> {
    debug!(url = %url, "im: DELETE");
    let resp = bifrost_core::direct_ureq_agent()
        .delete(url)
        .call()
        .map_err(|e| bifrost_core::BifrostError::Network(format!("HTTP DELETE failed: {}", e)))?;
    let text = resp.into_string().unwrap_or_default();
    if text.is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(&text)
            .map_err(|_| bifrost_core::BifrostError::Parse("invalid response".to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
enum ResolveSecretError {
    #[error("environment variable '{0}' not set")]
    Missing(String),
    #[error("failed to read secret file '{path}': {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

fn resolve_secret(value: &str) -> std::result::Result<String, ResolveSecretError> {
    if let Some(env_key) = value.strip_prefix("env:") {
        std::env::var(env_key).map_err(|_| ResolveSecretError::Missing(env_key.to_string()))
    } else if let Some(file_path) = value.strip_prefix("file:") {
        fs::read_to_string(file_path)
            .map(|s| s.trim().to_string())
            .map_err(|source| ResolveSecretError::Io {
                path: file_path.to_string(),
                source,
            })
    } else {
        Ok(value.to_string())
    }
}

fn format_timestamp(ts: i64) -> String {
    let secs = if ts > 1_000_000_000_000 {
        ts / 1000
    } else {
        ts
    };
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ts.to_string())
}

// ─── Messages (message log) ─────────────────────────────────────────────────

fn handle_im_messages(host: &str, port: u16, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str());
    match sub {
        Some("list") => {
            let mut provider: Option<&str> = None;
            let mut direction: Option<&str> = None;
            let mut source: Option<&str> = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--provider" => {
                        provider = args.get(i + 1).map(|s| s.as_str());
                        i += 2;
                    }
                    "--direction" => {
                        direction = args.get(i + 1).map(|s| s.as_str());
                        i += 2;
                    }
                    "--source" => {
                        source = args.get(i + 1).map(|s| s.as_str());
                        i += 2;
                    }
                    _ => i += 1,
                }
            }

            let selected_provider;
            let pid = match provider {
                Some(provider) => provider,
                None => {
                    selected_provider = select_provider_interactively(host, port)?;
                    selected_provider.as_str()
                }
            };

            let mut query_parts = Vec::new();
            if let Some(d) = direction {
                query_parts.push(format!("direction={}", d));
            }
            if let Some(s) = source {
                query_parts.push(format!("source={}", s));
            }
            let query_str = if query_parts.is_empty() {
                String::new()
            } else {
                format!("?{}", query_parts.join("&"))
            };

            let url = api_url(
                host,
                port,
                &format!("/providers/{}/messages{}", pid, query_str),
            );
            let resp = http_get(&url)?;
            print_message_logs(&resp);
            Ok(())
        }
        Some("clear") => {
            let provider = args.get(1).ok_or_else(|| {
                bifrost_core::BifrostError::Config(
                    "provider id required: bifrost im messages clear <provider-id>".to_string(),
                )
            })?;
            let url = api_url(host, port, &format!("/providers/{}/messages", provider));
            http_delete(&url)?;
            println!(
                "{} Messages cleared for provider '{}'.",
                "✓".bright_green(),
                provider
            );
            Ok(())
        }
        _ => {
            eprintln!(
                "Usage: bifrost im messages <list|clear> --provider <provider-id> [--direction inbound|outbound] [--source user|bot]"
            );
            Ok(())
        }
    }
}

fn print_message_logs(resp: &Value) {
    let empty = vec![];
    let messages = resp.as_array().unwrap_or(&empty);
    if messages.is_empty() {
        println!("{}", "No message logs found.".dimmed());
        return;
    }

    println!(
        "{:<10} {:<10} {:<10} {:<20} {:<20} {}",
        "ID".bold(),
        "DIR".bold(),
        "STATUS".bold(),
        "TARGET/SENDER".bold(),
        "CONTENT".bold(),
        "TIME".bold(),
    );
    println!("{}", "─".repeat(90));

    for m in messages.iter().take(100) {
        let id = m["id"].as_str().unwrap_or("-");
        let dir = m["direction"].as_str().unwrap_or("-");
        let status = m["status"].as_str().unwrap_or("-");
        let target_or_sender = if dir == "inbound" {
            m["sender_open_id"]
                .as_str()
                .map(|s| {
                    if s.len() > 18 {
                        truncate_chars_with_suffix(s, 15, "...")
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string())
        } else {
            m["target_name"]
                .as_str()
                .unwrap_or(m["target_id"].as_str().unwrap_or("-"))
                .to_string()
        };
        let content = m["content_preview"]
            .as_str()
            .map(|s| {
                if s.len() > 18 {
                    truncate_chars_with_suffix(s, 15, "...")
                } else {
                    s.to_string()
                }
            })
            .unwrap_or_else(|| "-".to_string());
        let time = m["timestamp"]
            .as_i64()
            .map(format_timestamp)
            .unwrap_or_else(|| "-".to_string());

        let dir_colored = match dir {
            "inbound" => "← IN".bright_cyan().to_string(),
            "outbound" => "→ OUT".bright_yellow().to_string(),
            _ => dir.to_string(),
        };
        let status_colored = match status {
            "success" => "✓".bright_green().to_string(),
            "failed" => "✗".bright_red().to_string(),
            "rejected" => "⊘".bright_red().to_string(),
            _ => status.to_string(),
        };

        println!(
            "{:<10} {:<10} {:<10} {:<20} {:<20} {}",
            id, dir_colored, status_colored, target_or_sender, content, time
        );
    }

    println!("{}", format!("Total: {} messages", messages.len()).dimmed());
}

fn print_im_help() {
    println!(
        "{}",
        "bifrost im - IM Gateway management commands"
            .bright_white()
            .bold()
    );
    println!();
    println!("{}", "USAGE:".bright_yellow());
    println!("    bifrost im <SUBCOMMAND>");
    println!();
    println!("{}", "SUBCOMMANDS:".bright_yellow());
    println!("    provider    Manage IM providers (feishu, wechat, webhook)");
    println!("    target      Manage message targets (groups, users)");
    println!("    send        Send a message to a target");
    println!("    route       Manage event routes (message → script)");
    println!("    schedule    Manage scheduled tasks");
    println!("    history     View event and task run history");
    println!("    messages    View and manage message logs (inbound/outbound)");
    println!();
    println!("{}", "EXAMPLES:".bright_yellow());
    println!("    bifrost im provider list");
    println!("    bifrost im provider add feishu-main --type feishu --runner traex");
    println!("    bifrost im provider menu feishu-main preview");
    println!("    bifrost im provider menu feishu-main sync --publish");
    println!("    bifrost im provider add weixin-main --type weixin --runner codex");
    println!("    bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --runner 'Claude Code'");
    println!("    bifrost im provider capabilities feishu-main --format json-pretty");
    println!("    bifrost im target add oncall --provider feishu-main --receive-id-type chat_id --receive-id oc_xxx");
    println!("    bifrost im send weixin-main --text 'hello owner'");
    println!("    bifrost im send feishu-main --markdown-file ./report.md");
    println!("    bifrost im send feishu-main --target oncall --card-file ./card.json");
    println!("    bifrost im send feishu-main --chat-id oc_xxx --markdown '**done**' --image ./chart.png --file ./report.pdf");
    println!("    bifrost im route add deploy --provider feishu-main --event message.receive --regex '^/deploy' --script-file ./deploy.sh");
    println!("    bifrost im schedule add health --target oncall --cron '*/5 * * * *' --script-file ./check.sh");
    println!("    bifrost im schedule add agent-daily --target oncall --cron '0 9 * * *' --agent-prompt 'Summarize traffic' --agent-runner-id codex --agent-model gpt-5 --agent-reasoning-effort high");
    println!("    bifrost im messages list --provider feishu-main --direction inbound");
    println!("    bifrost im messages clear feishu-main");
}

#[cfg(test)]
mod tests;
