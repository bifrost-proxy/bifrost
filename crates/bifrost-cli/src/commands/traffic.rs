use std::io::IsTerminal;
use std::time::Duration;

use bifrost_core::{text::truncate_chars_with_suffix, BifrostError, Result};
use dialoguer::{theme::ColorfulTheme, Input, Select};
use serde::Deserialize;
use serde_json::Value;

use super::OutputFormat;

fn direct_agent(timeout: Duration) -> ureq::Agent {
    bifrost_core::direct_ureq_agent_builder()
        .timeout(timeout)
        .build()
}

fn network_request_error(url: &str, e: &ureq::Error) -> BifrostError {
    let detail = e.to_string();
    let lower = detail.to_lowercase();
    if lower.contains("connection refused") || lower.contains("connect error") {
        BifrostError::Network(format!(
            "Failed to connect to Bifrost admin API at {}\n\
             Is the proxy server running?\n\n\
             Hint: Start the proxy with: bifrost start\n\n\
             Error: {}",
            url, detail
        ))
    } else {
        BifrostError::Network(format!("Request failed: {}: {}", url, detail))
    }
}

pub struct TrafficListOptions {
    pub port: u16,
    pub limit: usize,
    pub cursor: Option<u64>,
    pub direction: String,
    pub method: Option<String>,
    pub status: Option<u16>,
    pub status_min: Option<u16>,
    pub status_max: Option<u16>,
    pub protocol: Option<String>,
    pub host: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>,
    pub content_type: Option<String>,
    pub client_ip: Option<String>,
    pub client_app: Option<String>,
    pub has_rule_hit: Option<bool>,
    pub is_websocket: Option<bool>,
    pub is_sse: Option<bool>,
    pub is_tunnel: Option<bool>,
    pub format: OutputFormat,
    pub no_color: bool,
}

pub struct TrafficGetOptions {
    pub port: u16,
    pub id: Option<String>,
    pub request_body: bool,
    pub response_body: bool,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Deserialize)]
struct TrafficQueryResult {
    records: Vec<TrafficSummaryCompact>,
    next_cursor: Option<u64>,
    prev_cursor: Option<u64>,
    has_more: bool,
    total: usize,
    server_sequence: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct TrafficSummaryCompact {
    id: String,
    seq: u64,
    m: String,
    h: String,
    p: String,
    s: u16,
    res_sz: usize,
    dur: u64,
    proto: String,
    st: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TrafficListLegacyResponse {
    total: usize,
    offset: usize,
    limit: usize,
    records: Vec<TrafficSummaryLegacy>,
}

#[derive(Debug, Clone, Deserialize)]
struct TrafficSummaryLegacy {
    id: String,
    sequence: u64,
    method: String,
    host: String,
    path: String,
    status: u16,
    response_size: usize,
    duration_ms: u64,
    protocol: String,
    start_time: String,
}

struct TrafficRow {
    id: String,
    seq: u64,
    status: u16,
    method: String,
    proto: String,
    host: String,
    path: String,
    res_sz: usize,
    dur: u64,
    start_time: String,
}

impl From<TrafficSummaryCompact> for TrafficRow {
    fn from(r: TrafficSummaryCompact) -> Self {
        Self {
            id: r.id,
            seq: r.seq,
            status: r.s,
            method: r.m,
            proto: r.proto,
            host: r.h,
            path: r.p,
            res_sz: r.res_sz,
            dur: r.dur,
            start_time: r.st,
        }
    }
}

impl From<TrafficSummaryLegacy> for TrafficRow {
    fn from(r: TrafficSummaryLegacy) -> Self {
        Self {
            id: r.id,
            seq: r.sequence,
            status: r.status,
            method: r.method,
            proto: r.protocol,
            host: r.host,
            path: r.path,
            res_sz: r.response_size,
            dur: r.duration_ms,
            start_time: r.start_time,
        }
    }
}

pub fn run_traffic_list(options: TrafficListOptions) -> Result<()> {
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic{}",
        options.port,
        build_traffic_list_query(&options)
    );

    let resp = direct_agent(Duration::from_secs(10))
        .get(&url)
        .call()
        .map_err(|e| network_request_error(&url, &e))?;

    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read response: {}", e)))?;

    render_traffic_list_body(&body, options.format, options.no_color)
}

pub fn run_traffic_clear(port: u16, ids: Option<String>, yes: bool) -> Result<()> {
    let client = super::config::client::ConfigApiClient::new("127.0.0.1", port);

    if let Some(ids_str) = ids {
        let id_list: Vec<String> = ids_str.split(',').map(|s| s.trim().to_string()).collect();
        client
            .delete_traffic_by_ids(&id_list)
            .map_err(BifrostError::Config)?;
        println!("Deleted {} traffic record(s).", id_list.len());
    } else {
        if !yes {
            if !std::io::stdin().is_terminal() {
                return Err(BifrostError::Config(
                    "Use --yes to confirm clearing all traffic records in non-interactive mode"
                        .to_string(),
                ));
            }
            print!("Clear ALL traffic records? This cannot be undone. [y/N] ");
            use std::io::Write;
            std::io::stdout().flush().ok();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).ok();
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Cancelled.");
                return Ok(());
            }
        }
        client.clear_traffic().map_err(BifrostError::Config)?;
        println!("All traffic records cleared.");
    }
    Ok(())
}

pub fn run_traffic_get(options: TrafficGetOptions) -> Result<()> {
    let mut id = options.id.clone();
    if id.is_none() {
        if !std::io::stdin().is_terminal() {
            return Err(BifrostError::Parse(
                "Missing <id> and stdin is not interactive".to_string(),
            ));
        }
        id = Some(select_traffic_id(options.port, None)?);
    }

    let mut id = id.unwrap_or_default();
    if id.is_empty() {
        return Err(BifrostError::Parse("Traffic id is empty".to_string()));
    }

    let mut other_matches: Vec<SeqMatch> = Vec::new();

    if id.chars().all(|c| c.is_ascii_digit()) {
        let result = find_id_by_sequence_suffix(options.port, &id)?;
        id = result.matched_id;
        other_matches = result.others;
    }

    let record = match fetch_traffic_record(options.port, &id) {
        Ok(v) => v,
        Err(FetchTrafficError::NotFound) => {
            if !std::io::stdin().is_terminal() {
                return Err(BifrostError::NotFound(format!(
                    "Traffic record '{}' not found",
                    id
                )));
            }
            id = select_traffic_id(options.port, Some(&id))?;
            fetch_traffic_record(options.port, &id).map_err(|e| match e {
                FetchTrafficError::NotFound => {
                    BifrostError::NotFound(format!("Traffic record '{}' not found", id))
                }
                FetchTrafficError::Other(err) => err,
            })?
        }
        Err(FetchTrafficError::Other(e)) => return Err(e),
    };

    let mut output = record;

    if options.request_body {
        if let Some(v) = fetch_traffic_body(options.port, &id, true) {
            output["request_body"] = v;
        }
    }

    if options.response_body {
        if let Some(v) = fetch_traffic_body(options.port, &id, false) {
            output["response_body"] = v;
        }
    }

    render_traffic_detail_value(&output, options.format, false)?;

    if !other_matches.is_empty() {
        let use_color = std::io::stdout().is_terminal();
        if use_color {
            eprintln!(
                "\n\x1b[90m{} other record(s) also match suffix:\x1b[0m",
                other_matches.len()
            );
            for m in &other_matches {
                eprintln!("\x1b[90m  bifrost traffic get {}\x1b[0m", m.seq);
            }
        } else {
            eprintln!(
                "\n{} other record(s) also match suffix:",
                other_matches.len()
            );
            for m in &other_matches {
                eprintln!("  bifrost traffic get {}", m.seq);
            }
        }
    }

    Ok(())
}

pub fn render_traffic_list_body(body: &str, format: OutputFormat, no_color: bool) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", body.trim());
            Ok(())
        }
        OutputFormat::JsonPretty => {
            let v: Value =
                serde_json::from_str(body).map_err(|e| BifrostError::Parse(e.to_string()))?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            Ok(())
        }
        OutputFormat::Table | OutputFormat::Compact => {
            let (rows, meta) = parse_traffic_list_rows(body)?;
            print_traffic_rows(&rows, meta, format, no_color);
            Ok(())
        }
    }
}

pub fn render_traffic_detail_body(body: &str, format: OutputFormat, no_color: bool) -> Result<()> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| BifrostError::Parse(e.to_string()))?;
    render_traffic_detail_value(&value, format, no_color)
}

enum FetchTrafficError {
    NotFound,
    Other(BifrostError),
}

fn fetch_traffic_record(port: u16, id: &str) -> std::result::Result<Value, FetchTrafficError> {
    let record_url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic/{}",
        port,
        urlencoding::encode(id)
    );

    let resp = match direct_agent(Duration::from_secs(10))
        .get(&record_url)
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => return Err(FetchTrafficError::NotFound),
        Err(e) => {
            return Err(FetchTrafficError::Other(network_request_error(
                &record_url,
                &e,
            )))
        }
    };

    let body = resp.into_string().map_err(|e| {
        FetchTrafficError::Other(BifrostError::Network(format!(
            "Failed to read response: {}",
            e
        )))
    })?;

    serde_json::from_str::<Value>(&body)
        .map_err(|e| FetchTrafficError::Other(BifrostError::Parse(e.to_string())))
}

fn fetch_traffic_body(port: u16, id: &str, is_request: bool) -> Option<Value> {
    let suffix = if is_request {
        "request-body"
    } else {
        "response-body"
    };

    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic/{}/{}",
        port,
        urlencoding::encode(id),
        suffix
    );

    let resp = direct_agent(Duration::from_secs(10))
        .get(&url)
        .call()
        .ok()?;
    let body = resp.into_string().ok()?;
    serde_json::from_str::<Value>(&body).ok()
}

struct SeqMatch {
    seq: u64,
    id: String,
}

struct SeqSuffixResult {
    matched_id: String,
    others: Vec<SeqMatch>,
}

fn find_id_by_sequence_suffix(port: u16, suffix: &str) -> Result<SeqSuffixResult> {
    let server_seq = fetch_server_sequence(port)?;

    let suffix_val: u64 = suffix
        .parse()
        .map_err(|_| BifrostError::Parse(format!("Invalid sequence suffix: {}", suffix)))?;

    let modulus = 10u64.pow(suffix.len() as u32);

    let mut candidates: Vec<u64> = Vec::new();
    let mut candidate = suffix_val;
    while candidate <= server_seq {
        candidates.push(candidate);
        candidate += modulus;
    }
    candidates.reverse();

    let mut all_matches: Vec<SeqMatch> = Vec::new();
    for seq in candidates {
        if let Some(id) = find_id_by_exact_sequence(port, seq)? {
            all_matches.push(SeqMatch { seq, id });
        }
    }

    if all_matches.is_empty() {
        return Err(BifrostError::NotFound(format!(
            "No traffic record with sequence suffix '{}' found",
            suffix
        )));
    }

    let matched_id = all_matches[0].id.clone();
    let others = all_matches.into_iter().skip(1).collect();

    Ok(SeqSuffixResult { matched_id, others })
}

fn fetch_server_sequence(port: u16) -> Result<u64> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/traffic?limit=1", port);
    let resp = direct_agent(Duration::from_secs(10))
        .get(&url)
        .call()
        .map_err(|e| network_request_error(&url, &e))?;
    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read response: {}", e)))?;

    if let Ok(r) = serde_json::from_str::<TrafficQueryResult>(&body) {
        return Ok(r.server_sequence);
    }

    if let Ok(legacy) = serde_json::from_str::<TrafficListLegacyResponse>(&body) {
        if let Some(first) = legacy.records.first() {
            return Ok(first.sequence);
        }
    }

    Err(BifrostError::NotFound(
        "No traffic records available".to_string(),
    ))
}

fn find_id_by_exact_sequence(port: u16, seq: u64) -> Result<Option<String>> {
    let cursor = seq.saturating_add(1);
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic?limit=1&cursor={}&direction=backward",
        port, cursor
    );

    let resp = match direct_agent(Duration::from_secs(10)).get(&url).call() {
        Ok(r) => r,
        Err(e) => return Err(network_request_error(&url, &e)),
    };
    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read response: {}", e)))?;

    let parsed = match serde_json::from_str::<TrafficQueryResult>(&body) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    let first = match parsed.records.into_iter().next() {
        Some(r) => r,
        None => return Ok(None),
    };

    if first.seq == seq {
        Ok(Some(first.id))
    } else {
        Ok(None)
    }
}

fn select_traffic_id(port: u16, hint: Option<&str>) -> Result<String> {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/traffic?limit=100", port);
    let resp = direct_agent(Duration::from_secs(10))
        .get(&url)
        .call()
        .map_err(|e| network_request_error(&url, &e))?;

    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read response: {}", e)))?;

    let (rows, _) = parse_traffic_list_rows(&body)?;

    let mut candidates = rows;
    if let Some(h) = hint {
        let needle = h.to_lowercase();
        candidates.retain(|r| r.id.to_lowercase().contains(&needle));
    }

    if candidates.is_empty() {
        return Err(BifrostError::NotFound(match hint {
            Some(h) => format!("No traffic records matched id '{}'", h),
            None => "No traffic records found".to_string(),
        }));
    }

    let mut items: Vec<String> = candidates
        .iter()
        .map(|r| {
            format!(
                "{}  {:>6}  {:>6}  {}{}  {:>8}  {}",
                truncate_str(&short_start_time(&r.start_time), 12),
                if r.status == 0 {
                    "...".to_string()
                } else {
                    r.status.to_string()
                },
                r.method,
                truncate_str(&r.host, 28),
                truncate_str(&r.path, 36),
                r.seq,
                truncate_str(&r.id, 16),
            )
        })
        .collect();
    items.push("Enter ID manually...".to_string());

    let prompt = match hint {
        Some(h) => format!("No exact match found. Select a candidate (keyword: {})", h),
        None => "Select a Traffic ID".to_string(),
    };

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&items)
        .default(0)
        .interact()
        .map_err(dialoguer_error)?;

    if selection == items.len() - 1 {
        let input: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Enter Traffic ID")
            .interact_text()
            .map_err(dialoguer_error)?;
        if input.trim().is_empty() {
            return Err(BifrostError::Parse("Traffic id is empty".to_string()));
        }
        return Ok(input.trim().to_string());
    }

    Ok(candidates[selection].id.clone())
}

fn dialoguer_error(e: dialoguer::Error) -> BifrostError {
    BifrostError::Io(std::io::Error::other(e))
}

struct TrafficListMeta {
    total: Option<usize>,
    has_more: Option<bool>,
    next_cursor: Option<u64>,
    prev_cursor: Option<u64>,
    server_sequence: Option<u64>,
    offset: Option<usize>,
    limit: Option<usize>,
}

fn parse_traffic_list_rows(body: &str) -> Result<(Vec<TrafficRow>, TrafficListMeta)> {
    if let Ok(r) = serde_json::from_str::<TrafficQueryResult>(body) {
        let rows = r.records.into_iter().map(TrafficRow::from).collect();
        let meta = TrafficListMeta {
            total: Some(r.total),
            has_more: Some(r.has_more),
            next_cursor: r.next_cursor,
            prev_cursor: r.prev_cursor,
            server_sequence: Some(r.server_sequence),
            offset: None,
            limit: None,
        };
        return Ok((rows, meta));
    }

    let legacy: TrafficListLegacyResponse =
        serde_json::from_str(body).map_err(|e| BifrostError::Parse(e.to_string()))?;
    let rows = legacy.records.into_iter().map(TrafficRow::from).collect();
    let meta = TrafficListMeta {
        total: Some(legacy.total),
        has_more: None,
        next_cursor: None,
        prev_cursor: None,
        server_sequence: None,
        offset: Some(legacy.offset),
        limit: Some(legacy.limit),
    };
    Ok((rows, meta))
}

fn build_traffic_list_query(options: &TrafficListOptions) -> String {
    let mut params: Vec<(String, String)> = Vec::new();

    params.push(("limit".to_string(), options.limit.to_string()));
    if let Some(cursor) = options.cursor {
        params.push(("cursor".to_string(), cursor.to_string()));
    }
    if options.direction == "forward" {
        params.push(("direction".to_string(), "forward".to_string()));
    }
    if let Some(ref v) = options.method {
        params.push(("method".to_string(), v.to_string()));
    }
    if let Some(v) = options.status {
        params.push(("status".to_string(), v.to_string()));
    }
    if let Some(v) = options.status_min {
        params.push(("status_min".to_string(), v.to_string()));
    }
    if let Some(v) = options.status_max {
        params.push(("status_max".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.protocol {
        params.push(("protocol".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.host {
        params.push(("host".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.url {
        params.push(("url".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.path {
        params.push(("path".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.content_type {
        params.push(("content_type".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.client_ip {
        params.push(("client_ip".to_string(), v.to_string()));
    }
    if let Some(ref v) = options.client_app {
        params.push(("client_app".to_string(), v.to_string()));
    }
    if let Some(v) = options.has_rule_hit {
        params.push(("has_rule_hit".to_string(), v.to_string()));
    }
    if let Some(v) = options.is_websocket {
        params.push(("is_websocket".to_string(), v.to_string()));
    }
    if let Some(v) = options.is_sse {
        params.push(("is_sse".to_string(), v.to_string()));
    }
    if let Some(v) = options.is_tunnel {
        params.push(("is_tunnel".to_string(), v.to_string()));
    }

    if params.is_empty() {
        return String::new();
    }

    let encoded = params
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(&v)))
        .collect::<Vec<_>>()
        .join("&");

    format!("?{}", encoded)
}

fn print_traffic_rows(
    rows: &[TrafficRow],
    meta: TrafficListMeta,
    format: OutputFormat,
    no_color: bool,
) {
    let use_color = !no_color && std::io::stdout().is_terminal();

    match format {
        OutputFormat::Compact => {
            for r in rows {
                let status = if r.status == 0 {
                    "...".to_string()
                } else {
                    r.status.to_string()
                };

                if use_color {
                    let status_color = match r.status {
                        0 => "\x1b[90m",
                        200..=299 => "\x1b[32m",
                        300..=399 => "\x1b[33m",
                        400..=499 => "\x1b[31m",
                        500..=599 => "\x1b[1;31m",
                        _ => "\x1b[37m",
                    };
                    println!(
                        "\x1b[90m{}\x1b[0m {}{}\x1b[0m {} \x1b[36m{}\x1b[0m{} \x1b[90m#{}\x1b[0m",
                        short_start_time(&r.start_time),
                        status_color,
                        status,
                        r.method,
                        r.host,
                        r.path,
                        r.seq
                    );
                } else {
                    println!(
                        "{} {} {} {}{} #{}",
                        short_start_time(&r.start_time),
                        status,
                        r.method,
                        r.host,
                        r.path,
                        r.seq
                    );
                }
            }
        }
        OutputFormat::Table => {
            println!();
            let header = if use_color {
                format!(
                    "\x1b[1;37m{:12}  {:>6}  {:>6}  {:7}  {:28}  {:50}  {:>10}  {:>8}  {:>10}\x1b[0m",
                    "START", "STATUS", "METHOD", "PROTO", "HOST", "PATH", "SIZE", "TIME", "SEQ"
                )
            } else {
                format!(
                    "{:12}  {:>6}  {:>6}  {:7}  {:28}  {:50}  {:>10}  {:>8}  {:>10}",
                    "START", "STATUS", "METHOD", "PROTO", "HOST", "PATH", "SIZE", "TIME", "SEQ"
                )
            };
            println!("{}", header);
            println!("{}", "─".repeat(155));

            for r in rows {
                let status_str = if r.status == 0 {
                    "...".to_string()
                } else {
                    r.status.to_string()
                };

                let (status_color, status_display) = if use_color {
                    match r.status {
                        0 => ("\x1b[90m", format!("{:>6}", status_str)),
                        200..=299 => ("\x1b[32m", format!("{:>6}", status_str)),
                        300..=399 => ("\x1b[33m", format!("{:>6}", status_str)),
                        400..=499 => ("\x1b[31m", format!("{:>6}", status_str)),
                        500..=599 => ("\x1b[1;31m", format!("{:>6}", status_str)),
                        _ => ("\x1b[37m", format!("{:>6}", status_str)),
                    }
                } else {
                    ("", format!("{:>6}", status_str))
                };

                let method_display = if use_color {
                    match r.method.as_str() {
                        "GET" => format!("\x1b[36m{:>6}\x1b[0m", r.method),
                        "POST" => format!("\x1b[33m{:>6}\x1b[0m", r.method),
                        "PUT" => format!("\x1b[35m{:>6}\x1b[0m", r.method),
                        "DELETE" => format!("\x1b[31m{:>6}\x1b[0m", r.method),
                        "PATCH" => format!("\x1b[34m{:>6}\x1b[0m", r.method),
                        _ => format!("{:>6}", r.method),
                    }
                } else {
                    format!("{:>6}", r.method)
                };

                let proto = truncate_str(&r.proto, 7);
                let host = truncate_str(&r.host, 28);
                let path = truncate_str(&r.path, 50);
                let size = format_size(r.res_sz);
                let time = format_duration(r.dur);
                let start = truncate_str(&short_start_time(&r.start_time), 12);
                let seq = r.seq.to_string();

                if use_color {
                    println!(
                        "\x1b[90m{:12}\x1b[0m  {}{}  {}  {:7}  {:28}  {:50}  {:>10}  {:>8}  \x1b[90m{:>10}\x1b[0m",
                        start,
                        status_color,
                        status_display,
                        method_display,
                        proto,
                        host,
                        path,
                        size,
                        time,
                        seq
                    );
                } else {
                    println!(
                        "{:12}  {}  {}  {:7}  {:28}  {:50}  {:>10}  {:>8}  {:>10}",
                        start, status_display, method_display, proto, host, path, size, time, seq
                    );
                }
            }

            println!();
            if let Some(total) = meta.total {
                if use_color {
                    print!("\x1b[90mTotal: {}\x1b[0m", total);
                } else {
                    print!("Total: {}", total);
                }
                if let Some(offset) = meta.offset {
                    if let Some(limit) = meta.limit {
                        if use_color {
                            print!("\x1b[90m  Offset: {}  Limit: {}\x1b[0m", offset, limit);
                        } else {
                            print!("  Offset: {}  Limit: {}", offset, limit);
                        }
                    }
                }
                if let Some(seq) = meta.server_sequence {
                    if use_color {
                        print!("\x1b[90m  ServerSeq: {}\x1b[0m", seq);
                    } else {
                        print!("  ServerSeq: {}", seq);
                    }
                }
                println!();
            }

            if meta.next_cursor.is_some() || meta.prev_cursor.is_some() {
                let mut parts = Vec::new();
                if let Some(c) = meta.next_cursor {
                    parts.push(format!("next_cursor={}", c));
                }
                if let Some(c) = meta.prev_cursor {
                    parts.push(format!("prev_cursor={}", c));
                }
                if !parts.is_empty() {
                    if use_color {
                        println!("\x1b[90m{}\x1b[0m", parts.join("  "));
                    } else {
                        println!("{}", parts.join("  "));
                    }
                }
            }

            if meta.has_more == Some(true) {
                if use_color {
                    println!("\x1b[90m... more records available. Use --cursor/--direction to paginate.\x1b[0m");
                } else {
                    println!("... more records available. Use --cursor/--direction to paginate.");
                }
            }
        }
        _ => {}
    }
}

fn short_start_time(s: &str) -> String {
    if let Some((_, rest)) = s.split_once(' ') {
        rest.to_string()
    } else {
        s.to_string()
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    let len = s.chars().count();
    if len <= max_len {
        return s.to_string();
    }

    let keep = max_len.saturating_sub(3);
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= keep {
            break;
        }
        out.push(ch);
    }
    if keep < max_len {
        out.push_str("...");
    }
    out
}

fn format_size(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;

    if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

fn format_duration(ms: u64) -> String {
    if ms == 0 {
        "...".to_string()
    } else if ms >= 1000 {
        format!("{:.2}s", ms as f64 / 1000.0)
    } else {
        format!("{}ms", ms)
    }
}

pub fn render_traffic_detail_value(
    record: &Value,
    format: OutputFormat,
    no_color: bool,
) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(record)
                    .map_err(|e| BifrostError::Parse(format!("serialize traffic json: {}", e)))?
            );
            return Ok(());
        }
        OutputFormat::JsonPretty => {
            println!(
                "{}",
                serde_json::to_string_pretty(record).map_err(|e| {
                    BifrostError::Parse(format!("serialize pretty traffic json: {}", e))
                })?
            );
            return Ok(());
        }
        OutputFormat::Table | OutputFormat::Compact => {}
    }

    let use_color = !no_color && std::io::stdout().is_terminal();

    let dim = if use_color { "\x1b[90m" } else { "" };
    let bold = if use_color { "\x1b[1;37m" } else { "" };
    let green = if use_color { "\x1b[32m" } else { "" };
    let yellow = if use_color { "\x1b[33m" } else { "" };
    let red = if use_color { "\x1b[31m" } else { "" };
    let cyan = if use_color { "\x1b[36m" } else { "" };
    let reset = if use_color { "\x1b[0m" } else { "" };

    let s = |key: &str| -> String {
        record
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let u64_val = |key: &str| -> u64 { record.get(key).and_then(|v| v.as_u64()).unwrap_or(0) };
    let usize_val = |key: &str| -> usize { u64_val(key) as usize };
    let bool_val =
        |key: &str| -> bool { record.get(key).and_then(|v| v.as_bool()).unwrap_or(false) };

    let status = u64_val("status") as u16;
    let status_color = match status {
        200..=299 => green,
        300..=399 => yellow,
        400..=499 => red,
        500..=599 => red,
        _ => dim,
    };

    println!();
    println!("{bold}── Request Detail ──{reset}  {dim}{}{reset}", s("id"));
    println!();

    println!(
        "  {bold}URL:{reset}       {} {cyan}{}{reset}",
        s("method"),
        s("url")
    );
    println!("  {bold}Status:{reset}    {status_color}{}{reset}", status);
    println!("  {bold}Protocol:{reset}  {}", s("protocol"));
    println!(
        "  {bold}Duration:{reset}  {}",
        format_duration(u64_val("duration_ms"))
    );
    println!(
        "  {bold}Size:{reset}      req {} / res {}",
        format_size(usize_val("request_size")),
        format_size(usize_val("response_size"))
    );
    if !s("host").is_empty() {
        println!("  {bold}Host:{reset}      {}", s("host"));
    }
    if !s("client_ip").is_empty() {
        println!("  {bold}Client:{reset}    {}", s("client_ip"));
    }
    if let Some(app) = record.get("client_app").and_then(|v| v.as_str()) {
        println!("  {bold}App:{reset}       {}", app);
    }
    if let Some(err) = record.get("error_message").and_then(|v| v.as_str()) {
        println!("  {bold}Error:{reset}     {red}{}{reset}", err);
    }

    if bool_val("has_rule_hit") {
        if let Some(rules) = record.get("matched_rules").and_then(|v| v.as_array()) {
            println!();
            println!("  {bold}Matched Rules ({}):{reset}", rules.len());
            for rule in rules {
                let protocol = rule.get("protocol").and_then(|v| v.as_str()).unwrap_or("?");
                let value = rule.get("value").and_then(|v| v.as_str()).unwrap_or("");
                let rule_name = rule.get("rule_name").and_then(|v| v.as_str()).unwrap_or("");
                let display_value = truncate_chars_with_suffix(value, 57, "...");
                if rule_name.is_empty() {
                    println!(
                        "    {dim}•{reset} {yellow}{}{reset} → {}",
                        protocol, display_value
                    );
                } else {
                    println!(
                        "    {dim}•{reset} {yellow}{}{reset} → {}  {dim}[{}]{reset}",
                        protocol, display_value, rule_name
                    );
                }
            }
        }
    }

    print_headers_section(
        record,
        "request_headers",
        "original_request_headers",
        "Request Headers",
        use_color,
    );

    print_headers_section(
        record,
        "response_headers",
        "original_response_headers",
        "Response Headers",
        use_color,
    );

    if let Some(body) = record.get("request_body") {
        println!();
        println!("  {bold}Request Body:{reset}");
        print_body(body, use_color);
    }

    if let Some(body) = record.get("response_body") {
        println!();
        println!("  {bold}Response Body:{reset}");
        print_body(body, use_color);
    }

    if let Some(timing) = record.get("timing") {
        let fields = [
            ("dns_ms", "DNS"),
            ("connect_ms", "Connect"),
            ("tls_ms", "TLS"),
            ("send_ms", "Send"),
            ("wait_ms", "Wait"),
            ("first_byte_ms", "First Byte"),
            ("receive_ms", "Receive"),
        ];
        let parts: Vec<String> = fields
            .iter()
            .filter_map(|(key, label)| {
                timing
                    .get(*key)
                    .and_then(|v| v.as_u64())
                    .map(|ms| format!("{}: {}ms", label, ms))
            })
            .collect();
        if !parts.is_empty() {
            println!();
            println!("  {bold}Timing:{reset}  {dim}{}{reset}", parts.join("  "));
        }
    }

    println!();
    Ok(())
}

fn print_headers_section(
    record: &Value,
    current_key: &str,
    original_key: &str,
    title: &str,
    use_color: bool,
) {
    let bold = if use_color { "\x1b[1;37m" } else { "" };
    let dim = if use_color { "\x1b[90m" } else { "" };
    let green = if use_color { "\x1b[32m" } else { "" };
    let yellow = if use_color { "\x1b[33m" } else { "" };
    let red = if use_color { "\x1b[31m" } else { "" };
    let reset = if use_color { "\x1b[0m" } else { "" };

    let current_headers = record.get(current_key).and_then(|v| v.as_array());
    let original_headers = record.get(original_key).and_then(|v| v.as_array());
    let has_modifications = original_headers.is_some();

    if let Some(headers) = current_headers {
        println!();
        if has_modifications {
            println!("  {bold}{} (Current):{reset}", title);
        } else {
            println!("  {bold}{}:{reset}", title);
        }
        let mut sorted: Vec<(&str, &str)> = headers
            .iter()
            .filter_map(|h| {
                let arr = h.as_array()?;
                Some((arr.first()?.as_str()?, arr.get(1)?.as_str()?))
            })
            .collect();
        sorted.sort_by_key(|a| a.0.to_lowercase());

        if has_modifications {
            let orig_map: std::collections::HashMap<String, Vec<&str>> =
                if let Some(orig) = original_headers {
                    let mut map: std::collections::HashMap<String, Vec<&str>> =
                        std::collections::HashMap::new();
                    for h in orig {
                        if let Some(arr) = h.as_array() {
                            if let (Some(k), Some(v)) = (
                                arr.first().and_then(|v| v.as_str()),
                                arr.get(1).and_then(|v| v.as_str()),
                            ) {
                                map.entry(k.to_lowercase()).or_default().push(v);
                            }
                        }
                    }
                    map
                } else {
                    std::collections::HashMap::new()
                };

            let mut used_idx: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for (name, value) in &sorted {
                let lower = name.to_lowercase();
                let orig_values = orig_map.get(&lower);
                let idx = used_idx.entry(lower.clone()).or_insert(0);
                let marker = if let Some(values) = orig_values {
                    if *idx < values.len() {
                        let orig_val = values[*idx];
                        *idx += 1;
                        if orig_val != *value {
                            format!("{yellow}~{reset}")
                        } else {
                            " ".to_string()
                        }
                    } else {
                        format!("{green}+{reset}")
                    }
                } else {
                    format!("{green}+{reset}")
                };
                println!("   {} {dim}{}{reset}: {}", marker, name, value);
            }

            let current_key_count: std::collections::HashMap<String, usize> = {
                let mut map = std::collections::HashMap::new();
                for (name, _) in &sorted {
                    *map.entry(name.to_lowercase()).or_insert(0) += 1;
                }
                map
            };
            for (key, values) in &orig_map {
                let cur_count = current_key_count.get(key).copied().unwrap_or(0);
                if cur_count < values.len() {
                    for v in &values[cur_count..] {
                        let display_name = original_headers
                            .and_then(|h| {
                                h.iter().find_map(|entry| {
                                    let arr = entry.as_array()?;
                                    let k = arr.first()?.as_str()?;
                                    if k.to_lowercase() == *key {
                                        Some(k)
                                    } else {
                                        None
                                    }
                                })
                            })
                            .unwrap_or(key);
                        println!(
                            "   {red}-{reset} {dim}{}{reset}: {dim}{}{reset}",
                            display_name, v
                        );
                    }
                }
            }
        } else {
            for (name, value) in &sorted {
                println!("    {dim}{}{reset}: {}", name, value);
            }
        }
    }

    if has_modifications {
        if let Some(orig) = original_headers {
            println!();
            println!("  {bold}{} (Original):{reset}", title);
            let mut sorted: Vec<(&str, &str)> = orig
                .iter()
                .filter_map(|h| {
                    let arr = h.as_array()?;
                    Some((arr.first()?.as_str()?, arr.get(1)?.as_str()?))
                })
                .collect();
            sorted.sort_by_key(|a| a.0.to_lowercase());
            for (name, value) in &sorted {
                println!("    {dim}{}{reset}: {}", name, value);
            }
        }
    }
}

fn print_body(body: &Value, use_color: bool) {
    let dim = if use_color { "\x1b[90m" } else { "" };
    let reset = if use_color { "\x1b[0m" } else { "" };

    if let Some(s) = body.as_str() {
        if s.chars().count() > 2000 {
            println!("    {}", truncate_chars_with_suffix(s, 2000, ""));
            println!("    {dim}... ({} bytes total, truncated){reset}", s.len());
        } else {
            println!("    {}", s);
        }
    } else if body.is_object() || body.is_array() {
        let pretty = serde_json::to_string_pretty(body).unwrap_or_default();
        let lines: Vec<&str> = pretty.lines().collect();
        if lines.len() > 50 {
            for line in &lines[..50] {
                println!("    {}", line);
            }
            println!(
                "    {dim}... ({} lines total, truncated){reset}",
                lines.len()
            );
        } else {
            for line in &lines {
                println!("    {}", line);
            }
        }
    } else {
        println!("    {}", body);
    }
}
