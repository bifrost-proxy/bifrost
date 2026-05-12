use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use base64::Engine as _;
use colored::Colorize;
use serde_json::{json, Value};
use tracing::debug;

use bifrost_core::{text::truncate_chars_with_suffix, Result};

const IM_GATEWAY_API_PREFIX: &str = "/_bifrost/api/im-gateway";

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
            let body = parse_provider_add_args(name, &args[2..])?;
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
        _ => {
            eprintln!("Usage: bifrost im provider <list|add|update|delete|status>");
            Ok(())
        }
    }
}

fn parse_provider_add_args(name: &str, args: &[String]) -> Result<Value> {
    let mut body = json!({
        "id": name,
    });

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--type" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["provider_type"] = json!(v);
                }
            }
            "--app-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["app_id"] = json!(v);
                }
            }
            "--secret" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    let resolved = resolve_secret(v)
                        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
                    body["app_secret"] = json!(resolved);
                }
            }
            "--base-url" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["base_url"] = json!(v);
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
            "--owner-open-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["owner_open_id"] = json!(v);
                }
            }
            "--enable-long-connection" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["event_connection_enabled"] = json!(v.parse::<bool>().unwrap_or(false));
                }
            }
            _ => {}
        }
        i += 1;
    }

    Ok(body)
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
                i += 1;
                if let Some(v) = args.get(i) {
                    body["base_url"] = json!(v);
                }
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
    });

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--provider" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["provider_id"] = json!(v);
                }
            }
            "--receive-id-type" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["receive_id_type"] = json!(v);
                }
            }
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
            "--msg-type" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["default_msg_type"] = json!(v);
                }
            }
            _ => {}
        }
        i += 1;
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
    let provider_id = match send_args.provider.clone() {
        Some(provider) => provider,
        None => select_provider_interactively(host, port)?,
    };

    let body = build_send_body(&provider_id, &send_args)?;

    let url = api_url(host, port, "/messages/send");
    let resp = http_post(&url, &body)?;

    let message_id = resp["message_id"].as_str().unwrap_or("unknown");
    println!(
        "{} Message sent via provider '{}' to {} (message_id: {})",
        "✓".bright_green(),
        provider_id,
        send_args
            .target
            .as_deref()
            .unwrap_or("__owner__")
            .bright_white(),
        message_id.dimmed()
    );
    Ok(())
}

#[derive(Debug, Default)]
struct ImSendArgs {
    provider: Option<String>,
    target: Option<String>,
    card_file: Option<String>,
    card_json: Option<String>,
    card_title: Option<String>,
    card_text: Option<String>,
    card_image_file: Option<String>,
    card_image_key: Option<String>,
    card_image_alt: Option<String>,
    image_file: Option<String>,
    image_key: Option<String>,
    image_type: Option<String>,
    text: Option<String>,
}

fn parse_send_args(args: &[String]) -> Result<ImSendArgs> {
    let mut parsed = ImSendArgs::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--provider" => {
                i += 1;
                parsed.provider = args.get(i).cloned();
            }
            "--target" => {
                i += 1;
                parsed.target = args.get(i).cloned();
            }
            "--card-file" => {
                i += 1;
                parsed.card_file = args.get(i).cloned();
            }
            "--card-json" => {
                i += 1;
                parsed.card_json = args.get(i).cloned();
            }
            "--card-title" => {
                i += 1;
                parsed.card_title = args.get(i).cloned();
            }
            "--card-text" => {
                i += 1;
                parsed.card_text = args.get(i).cloned();
            }
            "--card-image-file" => {
                i += 1;
                parsed.card_image_file = args.get(i).cloned();
            }
            "--card-image-key" => {
                i += 1;
                parsed.card_image_key = args.get(i).cloned();
            }
            "--card-image-alt" => {
                i += 1;
                parsed.card_image_alt = args.get(i).cloned();
            }
            "--image-file" => {
                i += 1;
                parsed.image_file = args.get(i).cloned();
            }
            "--image-key" => {
                i += 1;
                parsed.image_key = args.get(i).cloned();
            }
            "--image-type" => {
                i += 1;
                parsed.image_type = args.get(i).cloned();
            }
            "--text" => {
                i += 1;
                parsed.text = args.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }

    Ok(parsed)
}

fn build_send_body(provider_id: &str, args: &ImSendArgs) -> Result<Value> {
    let target_id = args.target.as_deref().unwrap_or("__owner__");
    let mut body = json!({
        "provider_id": provider_id,
        "target_id": target_id,
    });

    if let Some(file_path) = &args.card_file {
        let content = fs::read_to_string(Path::new(file_path)).map_err(|e| {
            bifrost_core::BifrostError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to read card file '{}': {}", file_path, e),
            ))
        })?;
        let card: Value = serde_json::from_str(&content)
            .map_err(|e| bifrost_core::BifrostError::Parse(format!("invalid card JSON: {}", e)))?;
        body["content"] = card;
        body["msg_type"] = json!("interactive");
    } else if let Some(json_str) = &args.card_json {
        let card: Value = serde_json::from_str(json_str)
            .map_err(|e| bifrost_core::BifrostError::Parse(format!("invalid card JSON: {}", e)))?;
        body["content"] = card;
        body["msg_type"] = json!("interactive");
    } else if args.card_title.is_some()
        || args.card_text.is_some()
        || args.card_image_file.is_some()
        || args.card_image_key.is_some()
        || args.card_image_alt.is_some()
    {
        body["rich_card"] = build_rich_card_payload(args)?;
        body["msg_type"] = json!("interactive");
    } else if args.image_file.is_some() || args.image_key.is_some() {
        body["image"] = build_image_payload(
            args.image_file.as_deref(),
            args.image_key.as_deref(),
            args.image_type.as_deref(),
        )?;
        body["msg_type"] = json!("image");
    } else if let Some(text_content) = &args.text {
        body["content"] = json!(text_content);
        body["msg_type"] = json!("text");
    } else {
        return Err(bifrost_core::BifrostError::Config(
            "one of --card-file, --card-json, --card-text, --card-image-file, --card-image-key, --image-file, --image-key, or --text is required".to_string(),
        ));
    }

    Ok(body)
}

fn build_rich_card_payload(args: &ImSendArgs) -> Result<Value> {
    let mut rich_card = json!({});
    if let Some(title) = &args.card_title {
        rich_card["title"] = json!(title);
    }
    if let Some(text) = &args.card_text {
        rich_card["text"] = json!(text);
    }
    if let Some(image_key) = &args.card_image_key {
        rich_card["image_key"] = json!(image_key);
    }
    if args.card_image_file.is_some() {
        rich_card["image"] = build_image_payload(
            args.card_image_file.as_deref(),
            None,
            args.image_type.as_deref(),
        )?;
    }
    if let Some(alt) = &args.card_image_alt {
        rich_card["image_alt"] = json!(alt);
    }
    Ok(rich_card)
}

fn build_image_payload(
    image_file: Option<&str>,
    image_key: Option<&str>,
    image_type: Option<&str>,
) -> Result<Value> {
    let mut image = json!({
        "image_type": image_type.unwrap_or("message")
    });

    if let Some(image_key) = image_key.filter(|value| !value.trim().is_empty()) {
        image["image_key"] = json!(image_key);
        return Ok(image);
    }

    let file_path = image_file.ok_or_else(|| {
        bifrost_core::BifrostError::Config("image file or image key is required".to_string())
    })?;
    let bytes = fs::read(Path::new(file_path)).map_err(|e| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read image file '{}': {}", file_path, e),
        ))
    })?;
    let file_name = Path::new(file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bifrost-image");
    image["file_name"] = json!(file_name);
    image["data_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
    if let Some(mime_type) = guess_image_mime_type(file_name) {
        image["mime_type"] = json!(mime_type);
    }

    Ok(image)
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

// ─── Schedule ────────────────────────────────────────────────────────────────

fn handle_im_schedule(host: &str, port: u16, args: &[String]) -> Result<()> {
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

fn parse_schedule_add_args(name: &str, args: &[String]) -> Result<Value> {
    let mut body = json!({
        "name": name,
    });
    let mut trigger = json!({});
    let mut script = json!({});

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    body["target_id"] = json!(v);
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
                    script["script_file"] = json!(v);
                }
            }
            "--script" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    script["script_text"] = json!(v);
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

    Ok(body)
}

fn parse_schedule_update_args(args: &[String]) -> Result<Value> {
    let mut body = json!({});
    let mut trigger = json!({});

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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
            _ => {}
        }
        i += 1;
    }

    if !trigger.as_object().is_none_or(|t| t.is_empty()) {
        body["trigger"] = trigger;
    }

    Ok(body)
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
        let target = s["target_id"].as_str().unwrap_or("-");
        let trigger_type = s["trigger"]["type"].as_str().unwrap_or("-");
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
    println!("    bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx");
    println!("    bifrost im target add oncall --receive-id-type chat_id --receive-id oc_xxx");
    println!("    bifrost im send --provider feishu-main --text 'hello owner'");
    println!("    bifrost im send --provider feishu-main --image-file ./alert.png");
    println!("    bifrost im send --provider feishu-main --card-title 'Deploy' --card-text 'done' --card-image-file ./chart.png");
    println!("    bifrost im send --target oncall --card-file ./card.json");
    println!("    bifrost im route add deploy --provider feishu-main --event message.receive --regex '^/deploy' --script-file ./deploy.sh");
    println!("    bifrost im schedule add health --target oncall --cron '*/5 * * * *' --script-file ./check.sh");
    println!("    bifrost im messages list --provider feishu-main --direction inbound");
    println!("    bifrost im messages clear feishu-main");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_secret_missing_env_returns_error() {
        let key = format!(
            "BIFROST_TEST_MISSING_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let error = resolve_secret(&format!("env:{key}")).expect_err("missing env must fail");
        assert!(matches!(error, ResolveSecretError::Missing(missing) if missing == key));
    }

    #[test]
    fn resolve_secret_missing_file_returns_io_error() {
        let path = std::env::temp_dir().join(format!(
            "missing-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let error =
            resolve_secret(&format!("file:{}", path.display())).expect_err("missing file fails");
        assert!(matches!(error, ResolveSecretError::Io { .. }));
    }

    #[test]
    fn im_send_defaults_to_owner_and_uses_content_field() {
        let args = parse_send_args(&[
            "--provider".into(),
            "feishu-main".into(),
            "--text".into(),
            "hello".into(),
        ])
        .expect("parse send args");

        let body = build_send_body("feishu-main", &args).expect("build send body");

        assert_eq!(body["provider_id"], "feishu-main");
        assert_eq!(body["target_id"], "__owner__");
        assert_eq!(body["msg_type"], "text");
        assert_eq!(body["content"], "hello");
        assert!(body.get("text").is_none());
    }

    #[test]
    fn im_send_keeps_explicit_target_and_card_content() {
        let args = parse_send_args(&[
            "--provider".into(),
            "feishu-main".into(),
            "--target".into(),
            "oncall".into(),
            "--card-json".into(),
            r#"{"config":{},"elements":[]}"#.into(),
        ])
        .expect("parse send args");

        let body = build_send_body("feishu-main", &args).expect("build send body");

        assert_eq!(body["provider_id"], "feishu-main");
        assert_eq!(body["target_id"], "oncall");
        assert_eq!(body["msg_type"], "interactive");
        assert!(body["content"]["elements"].is_array());
        assert!(body.get("card").is_none());
    }

    #[test]
    fn im_send_builds_image_key_payload() {
        let args = parse_send_args(&[
            "--provider".into(),
            "feishu-main".into(),
            "--image-key".into(),
            "img_v3_key".into(),
        ])
        .expect("parse send args");

        let body = build_send_body("feishu-main", &args).expect("build send body");

        assert_eq!(body["provider_id"], "feishu-main");
        assert_eq!(body["target_id"], "__owner__");
        assert_eq!(body["msg_type"], "image");
        assert_eq!(body["image"]["image_key"], "img_v3_key");
        assert_eq!(body["image"]["image_type"], "message");
    }

    #[test]
    fn im_send_builds_rich_card_payload() {
        let args = parse_send_args(&[
            "--provider".into(),
            "feishu-main".into(),
            "--card-title".into(),
            "Deploy report".into(),
            "--card-text".into(),
            "**Done**".into(),
            "--card-image-key".into(),
            "img_v3_chart".into(),
        ])
        .expect("parse send args");

        let body = build_send_body("feishu-main", &args).expect("build send body");

        assert_eq!(body["provider_id"], "feishu-main");
        assert_eq!(body["target_id"], "__owner__");
        assert_eq!(body["msg_type"], "interactive");
        assert_eq!(body["rich_card"]["title"], "Deploy report");
        assert_eq!(body["rich_card"]["text"], "**Done**");
        assert_eq!(body["rich_card"]["image_key"], "img_v3_chart");
    }

    #[test]
    fn enabled_provider_choices_only_returns_enabled_providers() {
        let providers = enabled_provider_choices(&json!([
            {"id":"disabled","display_name":"Disabled","enabled":false},
            {"id":"feishu-main","display_name":"Feishu Main","enabled":true}
        ]));

        assert_eq!(
            providers,
            vec![("feishu-main".to_string(), "Feishu Main".to_string())]
        );
    }

    #[test]
    fn ensure_provider_in_body_preserves_explicit_provider() {
        let mut body = json!({"provider_id":"feishu-main"});

        ensure_provider_value(&mut body, "feishu-other");

        assert_eq!(body["provider_id"], "feishu-main");
    }

    #[test]
    fn choose_provider_from_reader_returns_selected_provider() {
        let providers = vec![
            ("feishu-a".to_string(), "Feishu A".to_string()),
            ("feishu-b".to_string(), "Feishu B".to_string()),
        ];
        let mut output = Vec::new();

        let selected = choose_provider_from_reader(&providers, io::Cursor::new("2\n"), &mut output)
            .expect("selection should work");

        assert_eq!(selected, "feishu-b");
        let prompt = String::from_utf8(output).expect("prompt utf8");
        assert!(prompt.contains("Select IM provider"));
        assert!(prompt.contains("Feishu B"));
    }
}
