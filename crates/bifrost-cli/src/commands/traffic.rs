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
    pub listener_port: Option<u16>,
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
    /// Batch mode: when non-empty the CLI calls `/api/traffic/batch` once.
    /// Mutually exclusive with `id` (clap enforces it).
    pub ids: Vec<String>,
    /// Batch mode: optional per-body cap forwarded as `max_body=...`.
    pub max_body: Option<usize>,
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
    #[serde(default)]
    lp: u16,
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
    #[serde(default)]
    listener_port: u16,
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
    listener_port: u16,
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
            listener_port: r.lp,
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
            listener_port: r.listener_port,
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
    // Batch mode: when `--ids a,b,c` is provided we hit /api/traffic/batch once
    // and stream the ndjson response (or aggregate to JSON when --format json*).
    if !options.ids.is_empty() {
        return run_traffic_batch_get(&options);
    }

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

/// CLI batch handler: fetch many traffic records in a single HTTP round-trip via
/// `GET /_bifrost/api/traffic/batch?ids=a,b,c[&include=...][&max_body=N]`.
///
/// Output format selection:
///   - `ndjson`           -> stream server lines unchanged (one JSON object per line).
///   - `json` / `json-pretty` -> aggregate into `{"results":[...]}` and pretty-print.
///   - `table` / `compact` -> not meaningful for batch detail; falls back to `ndjson`.
fn run_traffic_batch_get(options: &TrafficGetOptions) -> Result<()> {
    // Build the query string. Reuse `urlencoding` to escape ids that may contain
    // `/`, `+`, `%`, etc. (record ids in the wild may be UUID-like or seq-based).
    let mut query = String::new();
    let joined_ids = options
        .ids
        .iter()
        .map(|s| urlencoding::encode(s).into_owned())
        .collect::<Vec<_>>()
        .join(",");
    query.push_str("ids=");
    query.push_str(&joined_ids);

    let mut include_tokens: Vec<&str> = Vec::new();
    if options.request_body {
        include_tokens.push("request-body");
    }
    if options.response_body {
        include_tokens.push("response-body");
    }
    // Headers are not exposed as separate CLI flags in this iteration; bodies
    // are the primary use case. Users wanting headers can call the single-id
    // endpoint (which always returns headers).
    if !include_tokens.is_empty() {
        query.push_str("&include=");
        query.push_str(&urlencoding::encode(&include_tokens.join(",")));
    }
    if let Some(mb) = options.max_body {
        query.push_str("&max_body=");
        query.push_str(&mb.to_string());
    }

    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic/batch?{}",
        options.port, query
    );

    // Bodies can be large; pick a generous timeout proportional to id count.
    let timeout = Duration::from_secs(10 + (options.ids.len() as u64).min(120));
    let resp = direct_agent(timeout)
        .get(&url)
        .call()
        .map_err(|e| network_request_error(&url, &e))?;

    if resp.status() != 200 {
        let status = resp.status();
        let body = resp.into_string().unwrap_or_default();
        return Err(BifrostError::Network(format!(
            "/api/traffic/batch returned status {}: {}",
            status, body
        )));
    }

    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("read batch response: {}", e)))?;

    match options.format {
        OutputFormat::JsonPretty => {
            // Aggregate ndjson lines into a single JSON object for easy scripting.
            let mut results = Vec::new();
            for line in body.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let v: Value = serde_json::from_str(line)
                    .map_err(|e| BifrostError::Parse(format!("batch line parse: {}", e)))?;
                results.push(v);
            }
            let envelope = serde_json::json!({ "results": results });
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        }
        OutputFormat::Json => {
            let mut results = Vec::new();
            for line in body.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let v: Value = serde_json::from_str(line)
                    .map_err(|e| BifrostError::Parse(format!("batch line parse: {}", e)))?;
                results.push(v);
            }
            let envelope = serde_json::json!({ "results": results });
            println!("{}", envelope);
        }
        _ => {
            // ndjson / table / compact -> emit server payload as-is (already ndjson).
            // Trailing newline is preserved by the server; ensure exactly one.
            print!("{}", body);
            if !body.ends_with("\n") {
                println!();
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
        OutputFormat::Ndjson => {
            println!("{}", body.trim());
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

    let Some(modulus) = 10u64.checked_pow(suffix.len() as u32) else {
        return Err(BifrostError::Parse(format!(
            "Traffic ID suffix too long: {}",
            suffix
        )));
    };

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
    if let Some(v) = options.listener_port {
        params.push(("listener_port".to_string(), v.to_string()));
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
                        "\x1b[90m{}\x1b[0m {}{}\x1b[0m {} :{} \x1b[36m{}\x1b[0m{} \x1b[90m#{}\x1b[0m",
                        short_start_time(&r.start_time),
                        status_color,
                        status,
                        r.method,
                        r.listener_port,
                        r.host,
                        r.path,
                        r.seq
                    );
                } else {
                    println!(
                        "{} {} {} :{} {}{} #{}",
                        short_start_time(&r.start_time),
                        status,
                        r.method,
                        r.listener_port,
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
                    "\x1b[1;37m{:12}  {:>6}  {:>6}  {:>5}  {:7}  {:28}  {:50}  {:>10}  {:>8}  {:>10}\x1b[0m",
                    "START", "STATUS", "METHOD", "PORT", "PROTO", "HOST", "PATH", "SIZE", "TIME", "SEQ"
                )
            } else {
                format!(
                    "{:12}  {:>6}  {:>6}  {:>5}  {:7}  {:28}  {:50}  {:>10}  {:>8}  {:>10}",
                    "START",
                    "STATUS",
                    "METHOD",
                    "PORT",
                    "PROTO",
                    "HOST",
                    "PATH",
                    "SIZE",
                    "TIME",
                    "SEQ"
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
                        "\x1b[90m{:12}\x1b[0m  {}{}  {}  {:>5}  {:7}  {:28}  {:50}  {:>10}  {:>8}  \x1b[90m{:>10}\x1b[0m",
                        start,
                        status_color,
                        status_display,
                        method_display,
                        r.listener_port,
                        proto,
                        host,
                        path,
                        size,
                        time,
                        seq
                    );
                } else {
                    println!(
                        "{:12}  {}  {}  {:>5}  {:7}  {:28}  {:50}  {:>10}  {:>8}  {:>10}",
                        start,
                        status_display,
                        method_display,
                        r.listener_port,
                        proto,
                        host,
                        path,
                        size,
                        time,
                        seq
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
        OutputFormat::Ndjson => {
            println!(
                "{}",
                serde_json::to_string(record)
                    .map_err(|e| BifrostError::Parse(format!("serialize traffic json: {}", e)))?
            );
            return Ok(());
        }
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

pub struct TrafficAuthStatusOptions {
    pub port: u16,
    pub id: String,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct AuthSummaryView {
    pub host: String,
    pub has_jwt: bool,
    pub has_cookie: bool,
    pub jwt_exp_ms: Option<i64>,
    pub jwt_user_id: Option<String>,
    pub cookie_exp_ms: Option<i64>,
    pub valid_at_ms: Option<i64>,
    pub valid: Option<bool>,
}

pub fn run_traffic_auth_status(options: TrafficAuthStatusOptions) -> Result<()> {
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic/{}/auth-status",
        options.port,
        urlencoding::encode(&options.id)
    );

    let resp = match direct_agent(Duration::from_secs(10)).get(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => {
            return Err(BifrostError::NotFound(format!(
                "Traffic record '{}' not found",
                options.id
            )));
        }
        Err(e) => return Err(network_request_error(&url, &e)),
    };

    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read response: {}", e)))?;

    let summary: AuthSummaryView =
        serde_json::from_str(&body).map_err(|e| BifrostError::Parse(e.to_string()))?;

    match options.format {
        OutputFormat::Json | OutputFormat::JsonPretty => {
            let s = if matches!(options.format, OutputFormat::JsonPretty) {
                serde_json::to_string_pretty(&summary).unwrap()
            } else {
                serde_json::to_string(&summary).unwrap()
            };
            println!("{s}");
        }
        OutputFormat::Table | OutputFormat::Compact => {
            print_auth_summary_human(&summary);
        }
        OutputFormat::Ndjson => {
            let s = serde_json::to_string(&summary).unwrap();
            println!("{s}");
        }
    }
    Ok(())
}

fn print_auth_summary_human(s: &AuthSummaryView) {
    println!(
        "host: {}",
        if s.host.is_empty() {
            "(unknown)"
        } else {
            &s.host
        }
    );
    let now = s.valid_at_ms.unwrap_or(0);

    if s.has_jwt {
        let user = s
            .jwt_user_id
            .as_deref()
            .map(|u| format!("user_id={u}, "))
            .unwrap_or_default();
        let (exp_str, jwt_valid) = match s.jwt_exp_ms {
            Some(exp) => (format_ts_iso(exp), Some(exp > now)),
            None => ("unknown".to_string(), None),
        };
        let status = match jwt_valid {
            Some(true) => "valid",
            Some(false) => "EXPIRED",
            None => "no-exp",
        };
        println!("jwt: present ({user}exp={exp_str}, {status})");
    } else {
        println!("jwt: absent");
    }

    if s.has_cookie {
        let (exp_str, ck_valid) = match s.cookie_exp_ms {
            Some(exp) => (format_ts_iso(exp), Some(exp > now)),
            None => ("session-only".to_string(), None),
        };
        let status = match ck_valid {
            Some(true) => "valid",
            Some(false) => "EXPIRED",
            None => "session",
        };
        println!("cookie: present (exp={exp_str}, {status})");
    } else {
        println!("cookie: absent");
    }

    let overall = match s.valid {
        Some(true) => {
            let earliest = [s.jwt_exp_ms, s.cookie_exp_ms].into_iter().flatten().min();
            match earliest {
                Some(exp) if exp > now => format!("valid for {}", format_duration_short(exp - now)),
                _ => "valid".to_string(),
            }
        }
        Some(false) => "EXPIRED".to_string(),
        None => "unknown (no expiry info captured)".to_string(),
    };
    println!("overall: {overall}");
}

fn format_ts_iso(ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    match Utc.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        None => format!("{ms}ms"),
    }
}

fn format_duration_short(ms: i64) -> String {
    if ms <= 0 {
        return "0s".to_string();
    }
    let mut secs = ms / 1000;
    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3_600;
    secs %= 3_600;
    let mins = secs / 60;
    secs %= 60;
    let mut out = String::new();
    if days > 0 {
        out.push_str(&format!("{days}d"));
    }
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if mins > 0 {
        out.push_str(&format!("{mins}m"));
    }
    if out.is_empty() || (secs > 0 && days == 0) {
        out.push_str(&format!("{secs}s"));
    }
    out
}

#[cfg(test)]
mod auth_status_tests {
    use super::*;

    #[test]
    fn format_duration_short_handles_hours_and_minutes() {
        assert_eq!(format_duration_short(0), "0s");
        assert_eq!(format_duration_short(45_000), "45s");
        assert_eq!(format_duration_short(3_600_000 + 60_000 * 23), "1h23m");
        assert_eq!(format_duration_short(86_400_000 * 2), "2d");
    }

    #[test]
    fn print_auth_summary_handles_all_states() {
        // Just ensure no panic on representative inputs.
        let s1 = AuthSummaryView {
            host: "h".into(),
            has_jwt: true,
            has_cookie: true,
            jwt_exp_ms: Some(9_999_999_999_000),
            jwt_user_id: Some("u1".into()),
            cookie_exp_ms: Some(9_999_999_999_000),
            valid_at_ms: Some(1_700_000_000_000),
            valid: Some(true),
        };
        print_auth_summary_human(&s1);
        let s2 = AuthSummaryView {
            host: "h".into(),
            has_jwt: true,
            has_cookie: false,
            jwt_exp_ms: Some(1),
            jwt_user_id: None,
            cookie_exp_ms: None,
            valid_at_ms: Some(1_700_000_000_000),
            valid: Some(false),
        };
        print_auth_summary_human(&s2);
        let s3 = AuthSummaryView {
            host: "h".into(),
            has_jwt: false,
            has_cookie: false,
            jwt_exp_ms: None,
            jwt_user_id: None,
            cookie_exp_ms: None,
            valid_at_ms: Some(1_700_000_000_000),
            valid: None,
        };
        print_auth_summary_human(&s3);
    }
}

// =============================================================================
// P2-6: traffic export / replay CLI commands
// =============================================================================

use std::fs;
use std::path::PathBuf;

pub struct TrafficExportOptions {
    pub port: u16,
    pub id: String,
    pub as_format: String,
    pub output: Option<PathBuf>,
}

pub struct TrafficReplayOptions {
    pub port: u16,
    pub id: String,
    pub patch: Vec<String>,
    pub patch_json: Option<String>,
    pub refresh_auth: bool,
    pub timeout: String,
    pub format: OutputFormat,
}

fn resolve_traffic_id(port: u16, id: &str) -> Result<String> {
    // 数字后缀模式：纯数字按 sequence suffix 查；否则按 ID 原样
    if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
        let r = find_id_by_sequence_suffix(port, id)?;
        Ok(r.matched_id)
    } else {
        Ok(id.to_string())
    }
}

pub fn run_traffic_export(opts: TrafficExportOptions) -> Result<()> {
    let id = resolve_traffic_id(opts.port, &opts.id)?;
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic/{}/export?format={}",
        opts.port, id, opts.as_format
    );
    let agent = direct_agent(Duration::from_secs(15));
    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| network_request_error(&url, &e))?;
    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read export body: {e}")))?;
    if let Some(path) = opts.output {
        fs::write(&path, &body).map_err(BifrostError::Io)?;
        eprintln!("wrote {}", path.display());
    } else {
        print!("{}", body);
        if !body.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

/// 解析 "/a/b=value" 简写为 RFC 6902 replace op；如以 `+` 开头 (`/a/b+=v`) 解释为 add，
/// 以 `-` 开头 (`-/a/b`) 解释为 remove。
pub(crate) fn parse_patch_shorthand(input: &str) -> Result<serde_json::Value> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix('-') {
        let path = rest.trim();
        if !path.starts_with('/') {
            return Err(BifrostError::Parse(format!(
                "patch remove path must start with '/': {input}"
            )));
        }
        return Ok(serde_json::json!({"op": "remove", "path": path}));
    }
    let (lhs, rhs) = trimmed.split_once('=').ok_or_else(|| {
        BifrostError::Parse(format!(
            "patch shorthand must be '/path=value' or '-/path': {input}"
        ))
    })?;
    let (op, path) = if let Some(p) = lhs.strip_suffix('+') {
        ("add", p)
    } else {
        ("replace", lhs)
    };
    if !path.starts_with('/') {
        return Err(BifrostError::Parse(format!(
            "patch path must start with '/': {input}"
        )));
    }
    let value = parse_patch_rhs(rhs);
    Ok(serde_json::json!({"op": op, "path": path, "value": value}))
}

fn parse_patch_rhs(rhs: &str) -> serde_json::Value {
    let t = rhs.trim();
    if t == "null" {
        return serde_json::Value::Null;
    }
    if t == "true" {
        return serde_json::Value::Bool(true);
    }
    if t == "false" {
        return serde_json::Value::Bool(false);
    }
    if let Ok(n) = t.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(f) = t.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return serde_json::Value::Number(n);
        }
    }
    // JSON 字面量：{...}、[...]、"..."
    if (t.starts_with('{') && t.ends_with('}'))
        || (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with('"') && t.ends_with('"'))
    {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            return v;
        }
    }
    serde_json::Value::String(t.to_string())
}

fn parse_timeout(spec: &str) -> Result<u64> {
    let s = spec.trim();
    if let Some(rest) = s.strip_suffix("ms") {
        rest.trim()
            .parse::<u64>()
            .map_err(|_| BifrostError::Parse(format!("invalid timeout: {spec}")))
    } else if let Some(rest) = s.strip_suffix('s') {
        let v: f64 = rest
            .trim()
            .parse()
            .map_err(|_| BifrostError::Parse(format!("invalid timeout: {spec}")))?;
        Ok((v * 1000.0) as u64)
    } else {
        s.parse::<u64>()
            .map_err(|_| BifrostError::Parse(format!("invalid timeout: {spec}")))
    }
}

pub fn run_traffic_replay(opts: TrafficReplayOptions) -> Result<()> {
    let id = resolve_traffic_id(opts.port, &opts.id)?;

    // 拼接 patch_json
    let mut patch_ops: Vec<serde_json::Value> = Vec::new();
    if let Some(raw) = opts.patch_json.as_ref() {
        let v: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| BifrostError::Parse(format!("--patch-json invalid: {e}")))?;
        match v {
            serde_json::Value::Array(arr) => patch_ops.extend(arr),
            other => patch_ops.push(other),
        }
    }
    for p in &opts.patch {
        patch_ops.push(parse_patch_shorthand(p)?);
    }

    let timeout_ms = parse_timeout(&opts.timeout)?;
    let body_json = serde_json::json!({
        "patch_json": patch_ops,
        "refresh_auth_from": if opts.refresh_auth { Some("latest") } else { None },
        "timeout_ms": timeout_ms,
    });

    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/traffic/{}/replay",
        opts.port, id
    );
    let agent = direct_agent(Duration::from_millis(timeout_ms.saturating_add(15_000)));
    let resp = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&serde_json::to_string(&body_json).unwrap())
        .map_err(|e| network_request_error(&url, &e))?;
    let body = resp
        .into_string()
        .map_err(|e| BifrostError::Network(format!("Failed to read replay body: {e}")))?;
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| BifrostError::Parse(format!("Invalid JSON response: {e}")))?;

    match opts.format {
        OutputFormat::Json => {
            println!("{}", parsed);
        }
        OutputFormat::JsonPretty => {
            println!("{}", serde_json::to_string_pretty(&parsed).unwrap());
        }
        _ => {
            // human / table / compact: human-readable summary
            render_replay_human(&parsed);
        }
    }
    if let Some(err) = replay_failure_message(&parsed) {
        return Err(BifrostError::Network(format!("Replay failed: {err}")));
    }
    Ok(())
}

fn replay_failure_message(parsed: &serde_json::Value) -> Option<String> {
    if parsed.get("success").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    Some(
        parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string(),
    )
}

fn render_replay_human(parsed: &serde_json::Value) {
    if let Some(err) = replay_failure_message(parsed) {
        println!("replay failed: {err}");
        return;
    }
    let data = match parsed.get("data") {
        Some(d) => d,
        None => {
            println!("(no data)");
            return;
        }
    };
    let status = data.get("status").and_then(|v| v.as_u64()).unwrap_or(0);
    let duration_ms = data
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    println!("HTTP {status}  ({duration_ms} ms)");
    if let Some(headers) = data.get("headers").and_then(|v| v.as_array()) {
        for entry in headers {
            if let Some(arr) = entry.as_array() {
                if arr.len() == 2 {
                    let k = arr[0].as_str().unwrap_or("");
                    let v = arr[1].as_str().unwrap_or("");
                    println!("{k}: {v}");
                }
            }
        }
    }
    if let Some(body_b64) = data.get("body_b64").and_then(|v| v.as_str()) {
        use base64::Engine as _;
        match base64::engine::general_purpose::STANDARD.decode(body_b64) {
            Ok(bytes) => match std::str::from_utf8(&bytes) {
                Ok(text) => {
                    println!();
                    println!("{}", text);
                }
                Err(_) => println!("\n<binary {} bytes>", bytes.len()),
            },
            Err(e) => println!("\n<base64 decode error: {e}>"),
        }
    }
}

#[cfg(test)]
mod tests_export_replay {
    use super::*;

    #[test]
    fn parse_patch_shorthand_replace_string() {
        let v = parse_patch_shorthand("/a/b=hello").unwrap();
        assert_eq!(v["op"], "replace");
        assert_eq!(v["path"], "/a/b");
        assert_eq!(v["value"], "hello");
    }

    #[test]
    fn parse_patch_shorthand_replace_number() {
        let v = parse_patch_shorthand("/x=42").unwrap();
        assert_eq!(v["op"], "replace");
        assert_eq!(v["value"], 42);
    }

    #[test]
    fn parse_patch_shorthand_replace_bool_null_float() {
        assert_eq!(parse_patch_shorthand("/a=true").unwrap()["value"], true);
        assert_eq!(parse_patch_shorthand("/a=false").unwrap()["value"], false);
        assert!(parse_patch_shorthand("/a=null").unwrap()["value"].is_null());
        let v = parse_patch_shorthand("/a=2.5").unwrap();
        assert!((v["value"].as_f64().unwrap() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn parse_patch_shorthand_add_with_plus_suffix() {
        let v = parse_patch_shorthand("/list/-+=1").unwrap();
        assert_eq!(v["op"], "add");
        assert_eq!(v["path"], "/list/-");
        assert_eq!(v["value"], 1);
    }

    #[test]
    fn parse_patch_shorthand_remove_with_minus_prefix() {
        let v = parse_patch_shorthand("-/a/b").unwrap();
        assert_eq!(v["op"], "remove");
        assert_eq!(v["path"], "/a/b");
        assert!(v.get("value").is_none());
    }

    #[test]
    fn parse_patch_shorthand_json_literal() {
        let v = parse_patch_shorthand(r#"/x={"k":1}"#).unwrap();
        assert_eq!(v["op"], "replace");
        assert_eq!(v["value"], serde_json::json!({"k":1}));
    }

    #[test]
    fn parse_patch_shorthand_invalid_no_slash() {
        let err = parse_patch_shorthand("nope=1").unwrap_err();
        assert!(err.to_string().contains("path must start with '/'"));
    }

    #[test]
    fn parse_patch_shorthand_invalid_no_equals() {
        let err = parse_patch_shorthand("/just-a-path").unwrap_err();
        assert!(err.to_string().contains("must be"));
    }

    #[test]
    fn parse_timeout_seconds_and_ms() {
        assert_eq!(parse_timeout("30s").unwrap(), 30_000);
        assert_eq!(parse_timeout("1500ms").unwrap(), 1500);
        assert_eq!(parse_timeout("500").unwrap(), 500);
        assert_eq!(parse_timeout("1.5s").unwrap(), 1500);
        assert!(parse_timeout("abc").is_err());
    }

    #[test]
    fn replay_failure_message_detects_business_failure() {
        let parsed = serde_json::json!({
            "success": false,
            "error": "request body is not valid JSON"
        });

        assert_eq!(
            replay_failure_message(&parsed).as_deref(),
            Some("request body is not valid JSON")
        );
    }

    #[test]
    fn replay_failure_message_ignores_success_response() {
        let parsed = serde_json::json!({
            "success": true,
            "data": {
                "status": 200
            }
        });

        assert!(replay_failure_message(&parsed).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_traffic_list_query, format_duration, format_size, parse_traffic_list_rows,
        print_body, print_headers_section, render_traffic_detail_value, short_start_time,
        truncate_str, TrafficListOptions, TrafficRow,
    };
    use crate::commands::OutputFormat;
    use serde_json::json;

    #[test]
    fn build_traffic_list_query_includes_listener_port_filter() {
        let query = build_traffic_list_query(&TrafficListOptions {
            port: 9900,
            limit: 50,
            cursor: None,
            direction: "backward".to_string(),
            method: None,
            status: None,
            status_min: None,
            status_max: None,
            protocol: None,
            host: None,
            url: None,
            path: None,
            content_type: None,
            client_ip: None,
            client_app: None,
            listener_port: Some(50831),
            has_rule_hit: None,
            is_websocket: None,
            is_sse: None,
            is_tunnel: None,
            format: OutputFormat::Json,
            no_color: true,
        });

        assert!(query.contains("limit=50"));
        assert!(query.contains("listener_port=50831"));
    }

    #[test]
    fn short_start_time_drops_date_prefix_when_present() {
        assert_eq!(short_start_time("2024-01-01 12:34:56"), "12:34:56");
        assert_eq!(short_start_time("12:34:56"), "12:34:56");
    }

    #[test]
    fn truncate_str_truncates_and_adds_ellipsis() {
        assert_eq!(truncate_str("short", 10), "short");
        assert_eq!(truncate_str("abcdefghij", 5), "ab...");
    }

    #[test]
    fn format_size_formats_bytes_kb_and_mb() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(999), "999B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
    }

    #[test]
    fn format_duration_formats_ms_and_seconds_and_zero() {
        assert_eq!(format_duration(0), "...");
        assert_eq!(format_duration(50), "50ms");
        assert_eq!(format_duration(1500), "1.50s");
    }

    #[test]
    fn parse_traffic_list_rows_supports_compact_format() {
        let body = json!({
            "records": [{
                "id": "r1",
                "seq": 42,
                "m": "GET",
                "h": "example.com",
                "p": "/",
                "s": 200,
                "res_sz": 1234,
                "dur": 50,
                "lp": 8080,
                "proto": "HTTP/1.1",
                "st": "2024-01-01 12:00:00",
            }],
            "next_cursor": null,
            "prev_cursor": null,
            "has_more": false,
            "total": 1,
            "server_sequence": 42,
        })
        .to_string();

        let (rows, meta) = parse_traffic_list_rows(&body).unwrap();
        assert_eq!(rows.len(), 1);
        let row: &TrafficRow = &rows[0];
        assert_eq!(row.id, "r1");
        assert_eq!(row.seq, 42);
        assert_eq!(row.status, 200);
        assert_eq!(row.method, "GET");
        assert_eq!(row.host, "example.com");
        assert_eq!(row.path, "/");
        assert_eq!(meta.total, Some(1));
        assert_eq!(meta.server_sequence, Some(42));
    }

    #[test]
    fn parse_traffic_list_rows_supports_legacy_format() {
        let body = json!({
            "total": 1,
            "offset": 0,
            "limit": 10,
            "records": [{
                "id": "r1",
                "sequence": 7,
                "method": "POST",
                "host": "example.org",
                "path": "/api",
                "status": 201,
                "response_size": 2048,
                "duration_ms": 123,
                "listener_port": 9000,
                "protocol": "HTTP/2",
                "start_time": "2024-01-01 13:00:00",
            }],
        })
        .to_string();

        let (rows, meta) = parse_traffic_list_rows(&body).unwrap();
        assert_eq!(rows.len(), 1);
        let row: &TrafficRow = &rows[0];
        assert_eq!(row.id, "r1");
        assert_eq!(row.seq, 7);
        assert_eq!(row.method, "POST");
        assert_eq!(row.listener_port, 9000);
        assert_eq!(meta.total, Some(1));
        assert_eq!(meta.offset, Some(0));
        assert_eq!(meta.limit, Some(10));
    }

    #[test]
    fn render_traffic_detail_value_json_and_pretty_do_not_error() {
        let record = json!({
            "id": "r1",
            "method": "GET",
            "url": "https://example.com/",
            "status": 200,
            "protocol": "HTTP/1.1",
            "duration_ms": 100,
            "request_size": 512,
            "response_size": 1024,
        });

        render_traffic_detail_value(&record, OutputFormat::Json, true).unwrap();
        render_traffic_detail_value(&record, OutputFormat::JsonPretty, true).unwrap();
    }

    #[test]
    fn render_traffic_detail_value_table_renders_headers_and_bodies() {
        let record = json!({
            "id": "r1",
            "method": "GET",
            "url": "https://example.com/",
            "status": 200,
            "protocol": "HTTP/1.1",
            "duration_ms": 100,
            "request_size": 256,
            "response_size": 512,
            "host": "example.com",
            "client_ip": "127.0.0.1",
            "client_app": "cli",
            "has_rule_hit": true,
            "matched_rules": [{
                "protocol": "reqH",
                "value": "X-Test: 1",
                "rule_name": "test.rule",
            }],
            "request_headers": [["Content-Type", "application/json"]],
            "original_request_headers": [["Content-Type", "text/plain"]],
            "response_headers": [["Content-Type", "application/json"]],
            "original_response_headers": [["Content-Type", "text/plain"]],
            "request_body": "hello",
            "response_body": "world",
            "timing": {
                "dns_ms": 10,
                "connect_ms": 20,
            },
        });

        render_traffic_detail_value(&record, OutputFormat::Table, true).unwrap();
        render_traffic_detail_value(&record, OutputFormat::Compact, true).unwrap();
    }

    #[test]
    fn print_headers_section_without_original_shows_headers() {
        let record = json!({
            "request_headers": [["X-Test", "1"], ["Content-Type", "text/plain"]],
        });

        print_headers_section(
            &record,
            "request_headers",
            "original_request_headers",
            "Request Headers",
            false,
        );
    }

    #[test]
    fn print_headers_section_with_original_shows_add_change_and_remove_markers() {
        let record = json!({
            "response_headers": [["X-New", "2"], ["Content-Type", "application/json"]],
            "original_response_headers": [["X-Old", "1"], ["Content-Type", "text/plain"]],
        });

        print_headers_section(
            &record,
            "response_headers",
            "original_response_headers",
            "Response Headers",
            true,
        );
    }

    #[test]
    fn print_body_handles_long_string_and_truncates() {
        let long = "a".repeat(2500);
        let value = json!(long);
        print_body(&value, false);
    }

    #[test]
    fn print_body_handles_large_json_structure() {
        let mut items = Vec::new();
        for i in 0..60 {
            items.push(json!({"idx": i}));
        }
        let value = json!(items);
        print_body(&value, false);
    }
}
