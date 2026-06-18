use std::io::{stdout, BufRead, BufReader, IsTerminal};
use std::time::Duration;

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::CrosstermBackend,
    style::{Color as RColor, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, Wrap,
    },
    Frame, Terminal,
};
use serde::Deserialize;

fn direct_agent(timeout: Duration) -> ureq::Agent {
    bifrost_core::direct_ureq_agent_builder()
        .timeout(timeout)
        .build()
}

fn network_request_error(url: &str, e: &ureq::Error) -> String {
    let detail = e.to_string();
    let lower = detail.to_lowercase();
    if lower.contains("connection refused") || lower.contains("connect error") {
        format!(
            "Failed to connect to Bifrost admin API at {}\n\
             Is the proxy server running?\n\n\
             Hint: Start the proxy with: bifrost start\n\n\
             Error: {}",
            url, detail
        )
    } else {
        format!("Request failed: {}: {}", url, detail)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultItem {
    pub record: TrafficSummary,
    pub matches: Vec<MatchLocation>,
    /// Optional bodies payload returned when `--include request_body|response_body|bodies`
    /// is passed and the server's `include` block is honored.
    #[serde(default)]
    pub bodies: Option<BodiesPayloadIn>,
    /// Optional headers payload returned when `--include request_headers|response_headers|headers`
    /// is passed and the server's `include` block is honored.
    #[serde(default)]
    pub headers: Option<HeadersPayloadIn>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BodiesPayloadIn {
    #[serde(default)]
    pub request: Option<BodyChunkIn>,
    #[serde(default)]
    pub response: Option<BodyChunkIn>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BodyChunkIn {
    pub bytes_b64: String,
    pub size: usize,
    pub truncated: bool,
    #[serde(default)]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HeadersPayloadIn {
    #[serde(default)]
    pub request: Vec<(String, String)>,
    #[serde(default)]
    pub response: Vec<(String, String)>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TrafficSummary {
    pub id: String,
    pub seq: u64,
    pub ts: u64,
    pub m: String,
    pub h: String,
    pub p: String,
    pub s: u16,
    ct: Option<String>,
    req_ct: Option<String>,
    pub req_sz: usize,
    pub res_sz: usize,
    pub dur: u64,
    pub proto: String,
    cip: String,
    capp: Option<String>,
    flags: u32,
    fc: usize,
    st: String,
    et: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct MatchLocation {
    pub field: String,
    pub preview: String,
    offset: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct TrafficDetail {
    id: String,
    method: String,
    host: String,
    url: String,
    path: String,
    status: u16,
    protocol: String,
    content_type: Option<String>,
    request_content_type: Option<String>,
    request_size: usize,
    response_size: usize,
    duration_ms: u64,
    client_ip: String,
    client_app: Option<String>,
    #[allow(dead_code)]
    timestamp: u64,
    request_headers: Option<Vec<(String, String)>>,
    response_headers: Option<Vec<(String, String)>>,
    is_websocket: bool,
    is_sse: bool,
    is_tunnel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Table,
    Compact,
    Json,
    JsonPretty,
    Ndjson,
}

#[derive(Debug, Clone, Deserialize)]
struct SseProgressPayload {
    total_searched: usize,
    total_matched: usize,
    #[allow(dead_code)]
    next_cursor: Option<u64>,
    #[allow(dead_code)]
    has_more_hint: bool,
    #[allow(dead_code)]
    iterations: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct SseDonePayload {
    total_searched: usize,
    total_matched: usize,
    next_cursor: Option<u64>,
    has_more: bool,
    #[allow(dead_code)]
    search_id: String,
    #[serde(default)]
    searched_range: SseSearchedRange,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SseSearchedRange {
    oldest_ts_ms: Option<i64>,
    newest_ts_ms: Option<i64>,
    scanned_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct SseErrorPayload {
    message: String,
}

enum SseEvent {
    Result(Box<SearchResultItem>),
    Progress(SseProgressPayload),
    Done(SseDonePayload),
    Error(SseErrorPayload),
}

fn parse_sse_events(reader: impl std::io::Read) -> impl Iterator<Item = SseEvent> {
    let buf = BufReader::new(reader);
    let mut event_name = String::new();
    let mut data_lines: Vec<String> = Vec::new();

    let mut lines_iter = buf.lines();
    std::iter::from_fn(move || loop {
        match lines_iter.next() {
            Some(Ok(line)) => {
                if line.is_empty() {
                    if !event_name.is_empty() && !data_lines.is_empty() {
                        let data_text = data_lines.join("\n");
                        let evt = match event_name.as_str() {
                            "result" => serde_json::from_str::<SearchResultItem>(&data_text)
                                .ok()
                                .map(|r| SseEvent::Result(Box::new(r))),
                            "progress" => serde_json::from_str::<SseProgressPayload>(&data_text)
                                .ok()
                                .map(SseEvent::Progress),
                            "done" => serde_json::from_str::<SseDonePayload>(&data_text)
                                .ok()
                                .map(SseEvent::Done),
                            "error" => serde_json::from_str::<SseErrorPayload>(&data_text)
                                .ok()
                                .map(SseEvent::Error),
                            _ => None,
                        };
                        event_name.clear();
                        data_lines.clear();
                        if let Some(e) = evt {
                            return Some(e);
                        }
                    }
                    event_name.clear();
                    data_lines.clear();
                } else if let Some(rest) = line.strip_prefix("event:") {
                    event_name = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    data_lines.push(rest.trim().to_string());
                }
            }
            Some(Err(_)) => return None,
            None => return None,
        }
    })
}

impl std::str::FromStr for OutputFormat {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "compact" | "c" => Self::Compact,
            "json" | "j" => Self::Json,
            "json-pretty" | "jp" => Self::JsonPretty,
            "ndjson" => Self::Ndjson,
            _ => Self::Table,
        })
    }
}

pub struct SearchOptions {
    pub keyword: String,
    pub port: u16,
    pub limit: usize,
    pub format: OutputFormat,
    pub interactive: bool,
    pub scope_url: bool,
    pub scope_headers: bool,
    pub scope_body: bool,
    pub scope_request_headers: bool,
    pub scope_response_headers: bool,
    pub scope_request_body: bool,
    pub scope_response_body: bool,
    pub filter_status: Option<String>,
    pub filter_method: Option<String>,
    #[allow(dead_code)]
    pub filter_protocol: Option<String>,
    pub filter_content_type: Option<String>,
    pub filter_domain: Option<String>,
    pub filter_host: Option<String>,
    pub filter_path: Option<String>,
    pub filter_listener_port: Option<u16>,
    pub no_color: bool,
    pub max_scan: Option<usize>,
    pub max_results: Option<usize>,
    pub req_json: Vec<String>,
    pub res_json: Vec<String>,
    pub req_header_eq: Vec<String>,
    pub res_header_eq: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub latest: Option<String>,
    pub include_request_body: bool,
    pub include_response_body: bool,
    pub include_request_headers: bool,
    pub include_response_headers: bool,
    pub max_body: Option<usize>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            keyword: String::new(),
            port: 9900,
            limit: 50,
            format: OutputFormat::Table,
            interactive: false,
            scope_url: false,
            scope_headers: false,
            scope_body: false,
            scope_request_headers: false,
            scope_response_headers: false,
            scope_request_body: false,
            scope_response_body: false,
            filter_status: None,
            filter_method: None,
            filter_protocol: None,
            filter_content_type: None,
            filter_domain: None,
            filter_host: None,
            filter_path: None,
            filter_listener_port: None,
            no_color: false,
            max_scan: None,
            max_results: None,
            req_json: Vec::new(),
            res_json: Vec::new(),
            req_header_eq: Vec::new(),
            res_header_eq: Vec::new(),
            since: None,
            until: None,
            latest: None,
            include_request_body: false,
            include_response_body: false,
            include_request_headers: false,
            include_response_headers: false,
            max_body: None,
        }
    }
}

/// Parse `--include` tokens (with aliases / shortcuts) into 4 booleans.
///
/// Accepted tokens (case-insensitive, comma-separated upstream):
///   request-body  | req-body
///   response-body | res-body
///   request-headers  | req-headers
///   response-headers | res-headers
///   bodies  -> both bodies
///   headers -> both headers
///
/// Unknown tokens are silently ignored (CLI is forgiving; admin API validates strictly).
pub fn parse_include_tokens(tokens: &[String]) -> (bool, bool, bool, bool) {
    let mut req_body = false;
    let mut res_body = false;
    let mut req_headers = false;
    let mut res_headers = false;
    for tok in tokens {
        match tok.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "request-body" | "req-body" => req_body = true,
            "response-body" | "res-body" => res_body = true,
            "request-headers" | "req-headers" => req_headers = true,
            "response-headers" | "res-headers" => res_headers = true,
            "bodies" => {
                req_body = true;
                res_body = true;
            }
            "headers" => {
                req_headers = true;
                res_headers = true;
            }
            _ => {}
        }
    }
    (req_body, res_body, req_headers, res_headers)
}

fn check_proxy_running(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/_bifrost/api/metrics", port);
    direct_agent(Duration::from_secs(2))
        .get(&url)
        .call()
        .is_ok()
}

pub fn run_search(options: SearchOptions) -> i32 {
    if !check_proxy_running(options.port) {
        eprintln!(
            "\x1b[31m✗\x1b[0m Bifrost proxy is not running on port {}",
            options.port
        );
        eprintln!(
            "  Start it with: \x1b[36mbifrost start -p {}\x1b[0m",
            options.port
        );
        return 1;
    }

    if options.interactive {
        run_interactive_search(options)
    } else {
        run_simple_search(options)
    }
}

fn run_simple_search(options: SearchOptions) -> i32 {
    let use_color = !options.no_color && std::io::stdout().is_terminal();

    let reader = match start_search_stream(&options, None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("\x1b[31m✗\x1b[0m Search failed: {}", e);
            return 1;
        }
    };

    match options.format {
        OutputFormat::Json | OutputFormat::JsonPretty => stream_json_output(reader, &options),
        OutputFormat::Ndjson => stream_ndjson_output(reader),
        OutputFormat::Table => stream_table_output(reader, &options, use_color),
        OutputFormat::Compact => stream_compact_output(reader, &options, use_color),
    }
}

fn stream_ndjson_output(reader: Box<dyn std::io::Read + Send>) -> i32 {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut error_message: Option<String> = None;
    for event in parse_sse_events(reader) {
        match event {
            SseEvent::Result(item) => {
                let line = search_result_to_json(&item, true);
                let _ = writeln!(out, "{}", line);
            }
            SseEvent::Done(d) => {
                let line = serde_json::json!({
                    "type": "done",
                    "total_matched": d.total_matched,
                    "total_searched": d.total_searched,
                    "has_more": d.has_more,
                    "searched_range": searched_range_to_json(&d.searched_range),
                });
                let _ = writeln!(out, "{}", line);
            }
            SseEvent::Progress(p) => {
                let line = serde_json::json!({
                    "type": "progress",
                    "total_matched": p.total_matched,
                    "total_searched": p.total_searched,
                });
                let _ = writeln!(out, "{}", line);
            }
            SseEvent::Error(err) => {
                error_message = Some(err.message);
                break;
            }
        }
    }
    if let Some(msg) = error_message {
        let line = serde_json::json!({"type": "error", "message": msg});
        let _ = writeln!(out, "{}", line);
        return 1;
    }
    0
}

fn search_result_to_json(item: &SearchResultItem, include_type: bool) -> serde_json::Value {
    let (bodies_json, headers_json) = search_payloads_to_json(item);
    let mut obj = serde_json::json!({
        "id": &item.record.id,
        "seq": item.record.seq,
        "method": &item.record.m,
        "host": &item.record.h,
        "path": &item.record.p,
        "status": item.record.s,
        "protocol": &item.record.proto,
        "request_size": item.record.req_sz,
        "response_size": item.record.res_sz,
        "timestamp": item.record.ts,
        "duration_ms": item.record.dur,
        "matches": item.matches.iter().map(|m| serde_json::json!({
            "field": m.field,
            "preview": m.preview,
        })).collect::<Vec<_>>(),
    });
    if let Some(map) = obj.as_object_mut() {
        if include_type {
            map.insert(
                "type".to_string(),
                serde_json::Value::String("result".to_string()),
            );
        }
        if let Some(v) = bodies_json {
            map.insert("bodies".to_string(), v);
        }
        if let Some(v) = headers_json {
            map.insert("headers".to_string(), v);
        }
    }
    obj
}

fn searched_range_to_json(range: &SseSearchedRange) -> serde_json::Value {
    serde_json::json!({
        "oldest_ts_ms": range.oldest_ts_ms,
        "newest_ts_ms": range.newest_ts_ms,
        "scanned_count": range.scanned_count,
    })
}

fn start_search_stream(
    options: &SearchOptions,
    cursor: Option<u64>,
) -> Result<Box<dyn std::io::Read + Send>, String> {
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/search/stream",
        options.port
    );
    let body = build_search_request_body(options, cursor);

    let response = direct_agent(Duration::from_secs(600))
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| network_request_error(&url, &e))?;

    let ct = response.header("Content-Type").unwrap_or("").to_string();

    if !ct.contains("text/event-stream") {
        return Err(format!(
            "Server returned unexpected content-type: {}. Ensure Bifrost version supports streaming search.",
            ct
        ));
    }

    Ok(Box::new(response.into_reader()))
}

fn stream_table_output(
    reader: Box<dyn std::io::Read + Send>,
    options: &SearchOptions,
    use_color: bool,
) -> i32 {
    let mut printed_header = false;
    let mut total_matched = 0usize;
    let mut total_searched = 0usize;
    let mut has_more = false;
    let mut error_message: Option<String> = None;

    for event in parse_sse_events(reader) {
        match event {
            SseEvent::Result(item) => {
                if !printed_header {
                    println!();
                    let header = if use_color {
                        format!(
                            "\x1b[1;37m{:>10}  {:>6}  {:>6}  {:7}  {:40}  {:46}  {:>10}  {:>8}\x1b[0m",
                            "SEQ", "STATUS", "METHOD", "PROTO", "HOST", "PATH", "SIZE", "TIME"
                        )
                    } else {
                        format!(
                            "{:>10}  {:>6}  {:>6}  {:7}  {:40}  {:46}  {:>10}  {:>8}",
                            "SEQ", "STATUS", "METHOD", "PROTO", "HOST", "PATH", "SIZE", "TIME"
                        )
                    };
                    println!("{}", header);
                    println!("{}", "─".repeat(150));
                    printed_header = true;
                }

                total_matched += 1;
                print_table_row(&item, options, use_color);
            }
            SseEvent::Progress(p) => {
                total_searched = p.total_searched;
                if use_color {
                    eprint!(
                        "\r\x1b[90m  ⏳ Searching... {} records scanned, {} matched\x1b[0m\x1b[K",
                        format_number(p.total_searched),
                        p.total_matched,
                    );
                }
            }
            SseEvent::Done(d) => {
                total_searched = d.total_searched;
                total_matched = d.total_matched;
                has_more = d.has_more;

                if use_color {
                    eprint!("\r\x1b[K");
                }
            }
            SseEvent::Error(error) => {
                if use_color {
                    eprint!("\r\x1b[K");
                }
                error_message = Some(error.message);
                break;
            }
        }
    }

    if let Some(error_message) = error_message {
        eprintln!("\x1b[31m✗\x1b[0m Search failed: {}", error_message);
        return 1;
    }

    if total_matched == 0 {
        if options.format == OutputFormat::Json || options.format == OutputFormat::JsonPretty {
            println!("{{\"results\":[],\"total_matched\":0}}");
        } else {
            println!(
                "\x1b[33m⚠\x1b[0m No results found for '\x1b[1m{}\x1b[0m'",
                options.keyword
            );
            print_search_summary(options, total_searched, total_matched, false, use_color);
        }
        return 0;
    }

    println!();
    print_search_summary(options, total_searched, total_matched, has_more, use_color);

    0
}

fn print_search_summary(
    options: &SearchOptions,
    total_searched: usize,
    total_matched: usize,
    has_more: bool,
    use_color: bool,
) {
    let max_scan = options.max_scan.unwrap_or(0);
    let max_results = options.max_results.unwrap_or(100);

    if use_color {
        if total_matched > 0 {
            println!(
                "\x1b[1;32m✓\x1b[0m Found \x1b[1m{}\x1b[0m matches (scanned {} records, scan range: {}, max results: {})",
                total_matched,
                format_number(total_searched),
                format_number(max_scan),
                format_number(max_results),
            );
        } else {
            println!(
                "  Scanned {} records (scan range: {}, max results: {})",
                format_number(total_searched),
                format_number(max_scan),
                format_number(max_results),
            );
        }
        if has_more {
            println!("\x1b[33m  ⚡ Search stopped early — more data may match.\x1b[0m");
        }
        println!(
            "\x1b[90m  Tip: --max-scan <N>     Broaden scan range (e.g. --max-scan 100000)\x1b[0m"
        );
        println!("\x1b[90m       --max-results <N>  Increase max returned matches (e.g. --max-results 500)\x1b[0m");
    } else {
        if total_matched > 0 {
            println!(
                "Found {} matches (scanned {} records, scan range: {}, max results: {})",
                total_matched,
                format_number(total_searched),
                format_number(max_scan),
                format_number(max_results),
            );
        } else {
            println!(
                "  Scanned {} records (scan range: {}, max results: {})",
                format_number(total_searched),
                format_number(max_scan),
                format_number(max_results),
            );
        }
        if has_more {
            println!("  Search stopped early — more data may match.");
        }
        println!("  Tip: --max-scan <N>     Broaden scan range (e.g. --max-scan 100000)");
        println!(
            "       --max-results <N>  Increase max returned matches (e.g. --max-results 500)"
        );
    }
}

fn print_table_row(item: &SearchResultItem, options: &SearchOptions, use_color: bool) {
    let r = &item.record;

    let status_str = if r.s == 0 {
        "...".to_string()
    } else {
        r.s.to_string()
    };

    let (status_color, status_display) = if use_color {
        match r.s {
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
        match r.m.as_str() {
            "GET" => format!("\x1b[36m{:>6}\x1b[0m", r.m),
            "POST" => format!("\x1b[33m{:>6}\x1b[0m", r.m),
            "PUT" => format!("\x1b[35m{:>6}\x1b[0m", r.m),
            "DELETE" => format!("\x1b[31m{:>6}\x1b[0m", r.m),
            "PATCH" => format!("\x1b[34m{:>6}\x1b[0m", r.m),
            _ => format!("{:>6}", r.m),
        }
    } else {
        format!("{:>6}", r.m)
    };

    let proto = truncate_str(&r.proto, 7);
    let host = highlight_keyword(&truncate_str(&r.h, 40), &options.keyword, use_color);
    let path = highlight_keyword(&truncate_str(&r.p, 46), &options.keyword, use_color);
    let size = format_size(r.res_sz);
    let time = format_duration(r.dur);
    let seq = r.seq.to_string();

    if use_color {
        println!(
            "\x1b[90m{:>10}\x1b[0m  {}{}  {}  {:7}  {}  {}  {:>10}  {:>8}\x1b[0m",
            seq, status_color, status_display, method_display, proto, host, path, size, time
        );
    } else {
        println!(
            "{:>10}  {}  {}  {:7}  {}  {}  {:>10}  {:>8}",
            seq, status_display, method_display, proto, host, path, size, time
        );
    }

    if !item.matches.is_empty() && item.matches.iter().any(|m| m.field != "url") {
        for m in &item.matches {
            if m.field == "url" {
                continue;
            }
            let preview = highlight_keyword(&m.preview, &options.keyword, use_color);
            if use_color {
                println!(
                    "        \x1b[90m└─ \x1b[34m{}\x1b[90m: {}\x1b[0m",
                    m.field, preview
                );
            } else {
                println!("        └─ {}: {}", m.field, preview);
            }
        }
    }
}

fn stream_compact_output(
    reader: Box<dyn std::io::Read + Send>,
    _options: &SearchOptions,
    use_color: bool,
) -> i32 {
    let mut error_message: Option<String> = None;

    for event in parse_sse_events(reader) {
        match event {
            SseEvent::Result(item) => {
                let r = &item.record;
                let status = if r.s == 0 {
                    "...".to_string()
                } else {
                    r.s.to_string()
                };

                if use_color {
                    let status_color = match r.s {
                        0 => "\x1b[90m",
                        200..=299 => "\x1b[32m",
                        300..=399 => "\x1b[33m",
                        400..=499 => "\x1b[31m",
                        500..=599 => "\x1b[1;31m",
                        _ => "\x1b[37m",
                    };
                    println!(
                        "\x1b[90m{:>10}\x1b[0m {}{}\x1b[0m {} \x1b[36m{}\x1b[0m{}",
                        r.seq, status_color, status, r.m, r.h, r.p
                    );
                } else {
                    println!("{:>10} {} {} {}{}", r.seq, status, r.m, r.h, r.p);
                }
            }
            SseEvent::Error(error) => {
                error_message = Some(error.message);
                break;
            }
            SseEvent::Progress(_) | SseEvent::Done(_) => {}
        }
    }

    if let Some(error_message) = error_message {
        eprintln!("\x1b[31m✗\x1b[0m Search failed: {}", error_message);
        return 1;
    }

    0
}

/// Produce the optional bodies/headers JSON fragments for one `SearchResultItem`.
fn search_payloads_to_json(
    item: &SearchResultItem,
) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    let bodies_json = item.bodies.as_ref().map(|b| {
        let req = b.request.as_ref().map(body_chunk_to_json);
        let res = b.response.as_ref().map(body_chunk_to_json);
        let mut obj = serde_json::Map::new();
        if let Some(v) = req {
            obj.insert("request".to_string(), v);
        }
        if let Some(v) = res {
            obj.insert("response".to_string(), v);
        }
        serde_json::Value::Object(obj)
    });

    let headers_json = if let Some(h) = item.headers.as_ref() {
        let hj = serde_json::json!({
            "request": h.request.iter().map(|(k, v)| serde_json::json!([k, v])).collect::<Vec<_>>(),
            "response": h.response.iter().map(|(k, v)| serde_json::json!([k, v])).collect::<Vec<_>>(),
        });
        Some(hj)
    } else {
        None
    };

    (bodies_json, headers_json)
}

fn body_chunk_to_json(chunk: &BodyChunkIn) -> serde_json::Value {
    serde_json::json!({
        "bytes_b64": chunk.bytes_b64.clone(),
        "size": chunk.size,
        "truncated": chunk.truncated,
        "content_type": chunk.content_type.clone(),
    })
}

fn stream_json_output(reader: Box<dyn std::io::Read + Send>, options: &SearchOptions) -> i32 {
    let pretty = options.format == OutputFormat::JsonPretty;
    let mut results = Vec::new();
    let mut total_matched = 0;
    let mut total_searched = 0;
    let mut has_more = false;
    let mut error_message: Option<String> = None;
    let mut searched_range = SseSearchedRange::default();

    for event in parse_sse_events(reader) {
        match event {
            SseEvent::Result(item) => {
                let obj = search_result_to_json(&item, false);
                results.push(obj);
            }
            SseEvent::Done(d) => {
                total_matched = d.total_matched;
                total_searched = d.total_searched;
                has_more = d.has_more;
                searched_range = d.searched_range;
            }
            SseEvent::Progress(progress) => {
                total_matched = progress.total_matched;
                total_searched = progress.total_searched;
            }
            SseEvent::Error(error) => {
                error_message = Some(error.message);
                break;
            }
        }
    }

    if let Some(error_message) = error_message {
        let output = serde_json::json!({
            "error": error_message,
            "results": results,
            "total_matched": total_matched,
            "total_searched": total_searched,
            "has_more": has_more,
            "searched_range": searched_range_to_json(&searched_range),
        });
        if pretty {
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        } else {
            println!("{}", output);
        }
        return 1;
    }

    let output = serde_json::json!({
        "results": results,
        "total_matched": total_matched,
        "total_searched": total_searched,
        "has_more": has_more,
        "searched_range": searched_range_to_json(&searched_range),
    });

    if pretty {
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        println!("{}", output);
    }

    0
}

fn build_search_request_body(options: &SearchOptions, cursor: Option<u64>) -> serde_json::Value {
    let request_headers = options.scope_headers || options.scope_request_headers;
    let response_headers = options.scope_headers || options.scope_response_headers;
    let request_body = options.scope_body || options.scope_request_body;
    let response_body = options.scope_body || options.scope_response_body;
    let mut scope = serde_json::json!({});
    if options.scope_url || request_headers || response_headers || request_body || response_body {
        scope = serde_json::json!({
            "all": false,
            "url": options.scope_url,
            "request_headers": request_headers,
            "response_headers": response_headers,
            "request_body": request_body,
            "response_body": response_body,
        });
    }

    let mut filters = serde_json::json!({});
    let mut conditions = Vec::new();
    if let Some(ref status) = options.filter_status {
        filters["status_ranges"] = serde_json::json!([status]);
    }
    if let Some(ref domain) = options.filter_domain {
        filters["domains"] = serde_json::json!([domain]);
    }
    if let Some(ref ct) = options.filter_content_type {
        filters["content_types"] = serde_json::json!([ct]);
    }
    if let Some(ref proto) = options.filter_protocol {
        filters["protocols"] = serde_json::json!([proto]);
    }
    if let Some(ref method) = options.filter_method {
        conditions.push(serde_json::json!({
            "field": "method",
            "operator": "equals",
            "value": method,
        }));
    }
    if let Some(ref host) = options.filter_host {
        conditions.push(serde_json::json!({
            "field": "host",
            "operator": "contains",
            "value": host,
        }));
    }
    if let Some(ref path) = options.filter_path {
        conditions.push(serde_json::json!({
            "field": "path",
            "operator": "contains",
            "value": path,
        }));
    }
    if let Some(port) = options.filter_listener_port {
        conditions.push(serde_json::json!({
            "field": "listener_port",
            "operator": "equals",
            "value": port.to_string(),
        }));
    }
    for entry in &options.req_json {
        if let Some((path, value)) = split_eq(entry) {
            let field = if path.starts_with('$') {
                format!(
                    "req.body.{}",
                    path.trim_start_matches('$').trim_start_matches('.')
                )
            } else {
                format!("req.body.{}", path)
            };
            conditions.push(serde_json::json!({
                "field": field,
                "operator": "equals",
                "value": value,
            }));
        }
    }
    for entry in &options.res_json {
        if let Some((path, value)) = split_eq(entry) {
            let field = if path.starts_with('$') {
                format!(
                    "res.body.{}",
                    path.trim_start_matches('$').trim_start_matches('.')
                )
            } else {
                format!("res.body.{}", path)
            };
            conditions.push(serde_json::json!({
                "field": field,
                "operator": "equals",
                "value": value,
            }));
        }
    }
    for entry in &options.req_header_eq {
        if let Some((name, value)) = split_eq(entry) {
            conditions.push(serde_json::json!({
                "field": format!("req.header.{}", name),
                "operator": "equals",
                "value": value,
            }));
        }
    }
    for entry in &options.res_header_eq {
        if let Some((name, value)) = split_eq(entry) {
            conditions.push(serde_json::json!({
                "field": format!("res.header.{}", name),
                "operator": "equals",
                "value": value,
            }));
        }
    }
    if !conditions.is_empty() {
        filters["conditions"] = serde_json::Value::Array(conditions);
    }

    let mut body = serde_json::json!({
        "keyword": options.keyword,
        "scope": scope,
        "filters": filters,
        "limit": options.limit,
    });

    if let Some(c) = cursor {
        body["cursor"] = serde_json::json!(c);
    }

    if let Some(ms) = options.max_scan {
        body["max_scan"] = serde_json::json!(ms);
    }

    if let Some(mr) = options.max_results {
        body["max_results"] = serde_json::json!(mr);
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut since_ms: Option<i64> = None;
    let mut until_ms: Option<i64> = None;
    if let Some(ref s) = options.latest {
        if let Some(dur_ms) = parse_duration_ms(s) {
            since_ms = Some(now_ms - dur_ms);
        }
    }
    if let Some(ref s) = options.since {
        if let Some(ts) = parse_time_arg(s, now_ms) {
            since_ms = Some(ts);
        }
    }
    if let Some(ref s) = options.until {
        if let Some(ts) = parse_time_arg(s, now_ms) {
            until_ms = Some(ts);
        }
    }
    if since_ms.is_some() || until_ms.is_some() {
        let mut tr = serde_json::json!({});
        if let Some(v) = since_ms {
            tr["since_ms"] = serde_json::json!(v);
        }
        if let Some(v) = until_ms {
            tr["until_ms"] = serde_json::json!(v);
        }
        body["time_range"] = tr;
    }

    // Emit `include` block when any attach flag is set (or max_body is given).
    // The admin API treats absent include as the default (no extras), so we only
    // add the block when needed to keep the request body small for the common path.
    if options.include_request_body
        || options.include_response_body
        || options.include_request_headers
        || options.include_response_headers
        || options.max_body.is_some()
    {
        let mut include = serde_json::json!({
            "request_body": options.include_request_body,
            "response_body": options.include_response_body,
            "request_headers": options.include_request_headers,
            "response_headers": options.include_response_headers,
        });
        if let Some(mb) = options.max_body {
            include["max_body_bytes"] = serde_json::json!(mb);
        }
        body["include"] = include;
    }

    body
}

fn split_eq(input: &str) -> Option<(&str, &str)> {
    let (lhs, rhs) = input.split_once('=')?;
    let lhs = lhs.trim();
    let rhs = rhs.trim();
    if lhs.is_empty() {
        None
    } else {
        Some((lhs, rhs))
    }
}

/// Parse a relative duration like "30s", "5m", "2h", "1d" into milliseconds.
pub fn parse_duration_ms(input: &str) -> Option<i64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, unit_part) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit() && c != '-' && c != '+' && c != '.')
            .unwrap_or(s.len()),
    );
    let n: f64 = num_part.parse().ok()?;
    let mult: f64 = match unit_part.trim() {
        "ms" => 1.0,
        "s" | "" => 1000.0,
        "m" => 60.0 * 1000.0,
        "h" => 60.0 * 60.0 * 1000.0,
        "d" => 24.0 * 60.0 * 60.0 * 1000.0,
        "w" => 7.0 * 24.0 * 60.0 * 60.0 * 1000.0,
        _ => return None,
    };
    Some((n * mult) as i64)
}

fn parse_time_arg(input: &str, now_ms: i64) -> Option<i64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    if let Some(dur_ms) = parse_duration_ms(s) {
        return Some(now_ms - dur_ms);
    }
    None
}

fn highlight_keyword(text: &str, keyword: &str, use_color: bool) -> String {
    if !use_color || keyword.is_empty() {
        return text.to_string();
    }

    let lower_text = text.to_lowercase();
    let lower_keyword = keyword.to_lowercase();

    if !lower_text.contains(&lower_keyword) {
        return text.to_string();
    }

    let mut result = String::new();
    let mut last_end = 0;

    for (start, _) in lower_text.match_indices(&lower_keyword) {
        let prefix = match text.get(last_end..start) {
            Some(s) => s,
            None => return text.to_string(),
        };
        result.push_str(prefix);
        result.push_str("\x1b[1;33m");
        let end = start + lower_keyword.len();
        let highlighted = match text.get(start..end) {
            Some(s) => s,
            None => return text.to_string(),
        };
        result.push_str(highlighted);
        result.push_str("\x1b[0m");
        last_end = end;
    }
    let rest = match text.get(last_end..) {
        Some(s) => s,
        None => return text.to_string(),
    };
    result.push_str(rest);

    result
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

fn format_number(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

struct InteractiveApp {
    options: SearchOptions,
    results: Vec<SearchResultItem>,
    total_matched: usize,
    total_searched: usize,
    has_more: bool,
    next_cursor: Option<u64>,
    selected_index: usize,
    scroll_offset: usize,
    search_input: String,
    mode: AppMode,
    detail_record: Option<TrafficDetail>,
    detail_scroll: usize,
    detail_tab: usize,
    request_body: Option<String>,
    response_body: Option<String>,
    loading: bool,
    error_message: Option<String>,
    visible_height: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum AppMode {
    List,
    Search,
    Detail,
}

impl InteractiveApp {
    fn new(options: SearchOptions) -> Self {
        let initial_keyword = options.keyword.clone();
        Self {
            options,
            results: Vec::new(),
            total_matched: 0,
            total_searched: 0,
            has_more: false,
            next_cursor: None,
            selected_index: 0,
            scroll_offset: 0,
            search_input: initial_keyword,
            mode: AppMode::List,
            detail_record: None,
            detail_scroll: 0,
            detail_tab: 0,
            request_body: None,
            response_body: None,
            loading: false,
            error_message: None,
            visible_height: 20,
        }
    }

    fn search(&mut self) {
        self.loading = true;
        self.error_message = None;
        self.options.keyword = self.search_input.clone();

        match start_search_stream(&self.options, None) {
            Ok(reader) => {
                self.results.clear();
                self.total_matched = 0;
                self.total_searched = 0;
                self.has_more = false;
                self.next_cursor = None;

                for event in parse_sse_events(reader) {
                    match event {
                        SseEvent::Result(item) => {
                            self.results.push(*item);
                        }
                        SseEvent::Progress(p) => {
                            self.total_searched = p.total_searched;
                            self.total_matched = p.total_matched;
                        }
                        SseEvent::Done(d) => {
                            self.total_matched = d.total_matched;
                            self.total_searched = d.total_searched;
                            self.has_more = d.has_more;
                            self.next_cursor = d.next_cursor;
                        }
                        SseEvent::Error(error) => {
                            self.error_message = Some(error.message);
                            self.results.clear();
                            break;
                        }
                    }
                }

                self.selected_index = 0;
                self.scroll_offset = 0;
            }
            Err(e) => {
                self.error_message = Some(e);
                self.results.clear();
            }
        }
        self.loading = false;
    }

    fn load_more(&mut self) {
        if !self.has_more || self.next_cursor.is_none() {
            return;
        }

        self.loading = true;
        match start_search_stream(&self.options, self.next_cursor) {
            Ok(reader) => {
                for event in parse_sse_events(reader) {
                    match event {
                        SseEvent::Result(item) => {
                            self.results.push(*item);
                        }
                        SseEvent::Progress(p) => {
                            self.total_searched = p.total_searched;
                            self.total_matched = p.total_matched;
                        }
                        SseEvent::Done(d) => {
                            self.total_matched = d.total_matched;
                            self.total_searched = d.total_searched;
                            self.has_more = d.has_more;
                            self.next_cursor = d.next_cursor;
                        }
                        SseEvent::Error(error) => {
                            self.error_message = Some(error.message);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                self.error_message = Some(e);
            }
        }
        self.loading = false;
    }

    fn load_detail(&mut self) {
        if self.results.is_empty() {
            return;
        }

        let id = &self.results[self.selected_index].record.id;
        let url = format!(
            "http://127.0.0.1:{}/_bifrost/api/traffic/{}",
            self.options.port, id
        );

        self.loading = true;
        match direct_agent(Duration::from_secs(5)).get(&url).call() {
            Ok(resp) => {
                if let Ok(detail) = resp.into_json::<TrafficDetail>() {
                    self.detail_record = Some(detail);
                    self.detail_scroll = 0;
                    self.detail_tab = 0;
                    self.mode = AppMode::Detail;

                    self.load_bodies();
                }
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to load detail: {}", e));
            }
        }
        self.loading = false;
    }

    fn load_bodies(&mut self) {
        let id = match &self.detail_record {
            Some(r) => r.id.clone(),
            None => return,
        };

        let req_url = format!(
            "http://127.0.0.1:{}/_bifrost/api/traffic/{}/request-body",
            self.options.port, id
        );
        let res_url = format!(
            "http://127.0.0.1:{}/_bifrost/api/traffic/{}/response-body",
            self.options.port, id
        );

        if let Ok(resp) = direct_agent(Duration::from_secs(5)).get(&req_url).call() {
            if let Ok(body) = resp.into_json::<serde_json::Value>() {
                if let Some(data) = body.get("data") {
                    if !data.is_null() {
                        self.request_body = data.as_str().map(|s| s.to_string());
                    }
                }
            }
        }

        if let Ok(resp) = direct_agent(Duration::from_secs(5)).get(&res_url).call() {
            if let Ok(body) = resp.into_json::<serde_json::Value>() {
                if let Some(data) = body.get("data") {
                    if !data.is_null() {
                        self.response_body = data.as_str().map(|s| s.to_string());
                    }
                }
            }
        }
    }

    fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            }
        }
    }

    fn move_down(&mut self) {
        if self.selected_index < self.results.len().saturating_sub(1) {
            self.selected_index += 1;
            if self.selected_index >= self.scroll_offset + self.visible_height {
                self.scroll_offset = self.selected_index - self.visible_height + 1;
            }

            if self.selected_index >= self.results.len() - 5 && self.has_more {
                self.load_more();
            }
        }
    }

    fn page_up(&mut self) {
        let page_size = self.visible_height.saturating_sub(2);
        self.selected_index = self.selected_index.saturating_sub(page_size);
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
    }

    fn page_down(&mut self) {
        let page_size = self.visible_height.saturating_sub(2);
        let max_index = self.results.len().saturating_sub(1);
        self.selected_index = (self.selected_index + page_size).min(max_index);

        if self.selected_index >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.selected_index.saturating_sub(self.visible_height - 1);
        }

        if self.selected_index >= self.results.len() - 5 && self.has_more {
            self.load_more();
        }
    }

    fn scroll_detail(&mut self, delta: i32) {
        if delta < 0 {
            self.detail_scroll = self.detail_scroll.saturating_sub((-delta) as usize);
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as usize);
        }
    }
}

fn run_interactive_search(options: SearchOptions) -> i32 {
    let mut app = InteractiveApp::new(options);

    if !app.search_input.is_empty() {
        app.search();
    }

    let result = run_tui(&mut app);

    if let Err(e) = result {
        eprintln!("TUI error: {}", e);
        return 1;
    }

    0
}

fn run_tui(app: &mut InteractiveApp) -> std::io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui_loop(&mut terminal, app);

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.backend_mut().execute(Show)?;

    result
}

fn run_tui_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut InteractiveApp,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| draw_ui(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match app.mode {
                    AppMode::List => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(())
                        }
                        KeyCode::Char('/') | KeyCode::Char('s') => {
                            app.mode = AppMode::Search;
                        }
                        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                        KeyCode::PageUp => app.page_up(),
                        KeyCode::PageDown => app.page_down(),
                        KeyCode::Home | KeyCode::Char('g') => {
                            app.selected_index = 0;
                            app.scroll_offset = 0;
                        }
                        KeyCode::End | KeyCode::Char('G') if !app.results.is_empty() => {
                            app.selected_index = app.results.len() - 1;
                            if app.selected_index >= app.visible_height {
                                app.scroll_offset = app.selected_index - app.visible_height + 1;
                            }
                        }
                        KeyCode::Enter => app.load_detail(),
                        KeyCode::Char('r') => app.search(),
                        _ => {}
                    },
                    AppMode::Search => match key.code {
                        KeyCode::Esc => {
                            app.mode = AppMode::List;
                        }
                        KeyCode::Enter => {
                            app.mode = AppMode::List;
                            app.search();
                        }
                        KeyCode::Backspace => {
                            app.search_input.pop();
                        }
                        KeyCode::Char(c) => {
                            app.search_input.push(c);
                        }
                        _ => {}
                    },
                    AppMode::Detail => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            app.mode = AppMode::List;
                            app.detail_record = None;
                            app.request_body = None;
                            app.response_body = None;
                        }
                        KeyCode::Tab => {
                            app.detail_tab = (app.detail_tab + 1) % 4;
                            app.detail_scroll = 0;
                        }
                        KeyCode::BackTab => {
                            app.detail_tab = if app.detail_tab == 0 {
                                3
                            } else {
                                app.detail_tab - 1
                            };
                            app.detail_scroll = 0;
                        }
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_detail(-1),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_detail(1),
                        KeyCode::PageUp => app.scroll_detail(-10),
                        KeyCode::PageDown => app.scroll_detail(10),
                        KeyCode::Home => app.detail_scroll = 0,
                        _ => {}
                    },
                }
            }
        }
    }
}

fn draw_ui(f: &mut Frame, app: &mut InteractiveApp) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(size);

    let list_rows = chunks[1].height.saturating_sub(3) / 2;
    app.visible_height = match app.mode {
        AppMode::Detail => size.height.saturating_sub(6) as usize,
        _ => list_rows.max(1) as usize,
    };

    draw_search_bar(f, app, chunks[0]);

    match app.mode {
        AppMode::Detail => draw_detail_view(f, app, chunks[1]),
        _ => draw_results_list(f, app, chunks[1]),
    }

    draw_status_bar(f, app, chunks[2]);
}

fn draw_search_bar(f: &mut Frame, app: &InteractiveApp, area: Rect) {
    let style = if app.mode == AppMode::Search {
        Style::default().fg(RColor::Yellow)
    } else {
        Style::default().fg(RColor::White)
    };

    let cursor_char = if app.mode == AppMode::Search {
        "│"
    } else {
        ""
    };
    let search_text = format!(" 🔍 {}{}", app.search_input, cursor_char);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Search ")
        .title_style(
            Style::default()
                .fg(RColor::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(style);

    let paragraph = Paragraph::new(search_text)
        .block(block)
        .style(Style::default().fg(RColor::White));

    f.render_widget(paragraph, area);
}

fn draw_results_list(f: &mut Frame, app: &InteractiveApp, area: Rect) {
    if app.loading {
        let loading = Paragraph::new(" Loading...")
            .style(Style::default().fg(RColor::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Results "),
            );
        f.render_widget(loading, area);
        return;
    }

    if let Some(ref err) = app.error_message {
        let error = Paragraph::new(format!(" ✗ {}", err))
            .style(Style::default().fg(RColor::Red))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Error "),
            );
        f.render_widget(error, area);
        return;
    }

    if app.results.is_empty() {
        let empty_msg = if app.search_input.is_empty() {
            " Press / or s to start searching"
        } else {
            " No results found"
        };
        let empty = Paragraph::new(empty_msg)
            .style(Style::default().fg(RColor::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Results "),
            );
        f.render_widget(empty, area);
        return;
    }

    let header_cells = ["SEQ", "STATUS", "METHOD", "HOST", "PATH", "MATCH"]
        .iter()
        .map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(RColor::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        });
    let header = Row::new(header_cells).height(1);

    let visible_results: Vec<_> = app
        .results
        .iter()
        .enumerate()
        .skip(app.scroll_offset)
        .take(app.visible_height)
        .collect();

    let rows = visible_results.iter().map(|(idx, item)| {
        let r = &item.record;
        let is_selected = *idx == app.selected_index;

        let status_str = if r.s == 0 {
            "...".to_string()
        } else {
            r.s.to_string()
        };

        let status_style = match r.s {
            0 => Style::default().fg(RColor::DarkGray),
            200..=299 => Style::default().fg(RColor::Green),
            300..=399 => Style::default().fg(RColor::Yellow),
            400..=499 => Style::default().fg(RColor::Red),
            500..=599 => Style::default()
                .fg(RColor::LightRed)
                .add_modifier(Modifier::BOLD),
            _ => Style::default().fg(RColor::White),
        };

        let method_style = match r.m.as_str() {
            "GET" => Style::default().fg(RColor::Cyan),
            "POST" => Style::default().fg(RColor::Yellow),
            "PUT" => Style::default().fg(RColor::Magenta),
            "DELETE" => Style::default().fg(RColor::Red),
            "PATCH" => Style::default().fg(RColor::Blue),
            _ => Style::default().fg(RColor::White),
        };

        let row_style = if is_selected {
            Style::default().bg(RColor::DarkGray)
        } else {
            Style::default()
        };

        let seq_line = if is_selected {
            Line::from(vec![
                Span::styled("▶", Style::default().fg(RColor::Yellow)),
                Span::styled(
                    format!("{:>9}", r.seq),
                    Style::default()
                        .fg(RColor::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else {
            Line::from(Span::styled(
                format!("{:>10}", r.seq),
                Style::default().fg(RColor::DarkGray),
            ))
        };

        let (match_header, match_preview) = build_match_summary(item, &app.options.keyword);
        let match_cell = Cell::from(Text::from(vec![match_header, match_preview]))
            .style(Style::default().fg(RColor::White));

        Row::new(vec![
            Cell::from(Text::from(vec![seq_line, Line::from("")])),
            Cell::from(status_str).style(status_style),
            Cell::from(r.m.clone()).style(method_style),
            Cell::from(truncate_str(&r.h, 28)),
            Cell::from(truncate_str(&r.p, 40)),
            match_cell,
        ])
        .height(2)
        .style(row_style)
    });

    let title = format!(" Results ({}/{}) ", app.total_matched, app.total_searched);

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(30),
            Constraint::Min(20),
            Constraint::Min(40),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title)
            .title_style(
                Style::default()
                    .fg(RColor::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
    );

    f.render_widget(table, area);

    if app.results.len() > app.visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state =
            ScrollbarState::new(app.results.len()).position(app.selected_index);

        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };

        f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn build_match_summary(item: &SearchResultItem, keyword: &str) -> (Line<'static>, Line<'static>) {
    if item.matches.is_empty() {
        let header = Line::from(Span::styled(
            "NO_MATCH",
            Style::default().fg(RColor::DarkGray),
        ));
        return (header, Line::from(""));
    }

    let mut fields = Vec::<&str>::new();
    for m in &item.matches {
        let f = m.field.as_str();
        if !fields.contains(&f) {
            fields.push(f);
        }
    }

    let primary = item
        .matches
        .iter()
        .find(|m| m.field != "url")
        .unwrap_or(&item.matches[0]);

    let label = match_field_label(&primary.field);
    let extra = fields.len().saturating_sub(1);
    let header_text = if extra > 0 {
        format!("{}+{} ({})", label, extra, item.matches.len())
    } else {
        format!("{} ({})", label, item.matches.len())
    };

    let header = Line::from(vec![
        Span::styled(
            header_text,
            Style::default()
                .fg(RColor::LightCyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            truncate_str(&primary.preview, 28),
            Style::default().fg(RColor::DarkGray),
        ),
    ]);

    let preview_spans = highlight_spans(&truncate_str(&primary.preview, 80), keyword);
    let preview = Line::from(preview_spans);
    (header, preview)
}

fn match_field_label(field: &str) -> &'static str {
    match field {
        "url" => "URL",
        "request_headers" => "REQ_HDR",
        "response_headers" => "RES_HDR",
        "request_body" => "REQ_BODY",
        "response_body" => "RES_BODY",
        "frames" => "FRAMES",
        _ => "MATCH",
    }
}

fn highlight_spans(text: &str, keyword: &str) -> Vec<Span<'static>> {
    if keyword.is_empty() {
        return vec![Span::raw(text.to_string())];
    }

    let lower_text = text.to_lowercase();
    let lower_keyword = keyword.to_lowercase();
    if !lower_text.contains(&lower_keyword) {
        return vec![Span::raw(text.to_string())];
    }

    let mut spans = Vec::new();
    let mut last_end = 0usize;

    for (start, _) in lower_text.match_indices(&lower_keyword) {
        if start > last_end {
            let prefix = match text.get(last_end..start) {
                Some(s) => s,
                None => return vec![Span::raw(text.to_string())],
            };
            spans.push(Span::raw(prefix.to_string()));
        }
        let end = start + lower_keyword.len();
        let highlighted = match text.get(start..end) {
            Some(s) => s,
            None => return vec![Span::raw(text.to_string())],
        };
        spans.push(Span::styled(
            highlighted.to_string(),
            Style::default()
                .fg(RColor::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        last_end = end;
    }

    if last_end < text.len() {
        let rest = match text.get(last_end..) {
            Some(s) => s,
            None => return vec![Span::raw(text.to_string())],
        };
        spans.push(Span::raw(rest.to_string()));
    }

    spans
}

fn draw_detail_view(f: &mut Frame, app: &InteractiveApp, area: Rect) {
    let detail = match &app.detail_record {
        Some(d) => d,
        None => return,
    };

    let tabs_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 3,
    };

    let content_area = Rect {
        x: area.x,
        y: area.y + 3,
        width: area.width,
        height: area.height.saturating_sub(3),
    };

    let tab_titles = [
        " Overview ",
        " Request Headers ",
        " Response Headers ",
        " Body ",
    ];

    let tabs: Vec<Line> = tab_titles
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == app.detail_tab {
                Line::from(Span::styled(
                    *t,
                    Style::default()
                        .fg(RColor::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(*t, Style::default().fg(RColor::DarkGray)))
            }
        })
        .collect();

    let tabs_widget = ratatui::widgets::Tabs::new(tabs)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(format!(" {} ", detail.id))
                .title_style(
                    Style::default()
                        .fg(RColor::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .highlight_style(Style::default().fg(RColor::Yellow))
        .select(app.detail_tab);

    f.render_widget(tabs_widget, tabs_area);

    let content_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let content = match app.detail_tab {
        0 => format_overview(detail),
        1 => format_headers(&detail.request_headers, "Request Headers"),
        2 => format_headers(&detail.response_headers, "Response Headers"),
        3 => format_body(app),
        _ => String::new(),
    };

    let lines: Vec<&str> = content.lines().collect();
    let visible_lines: Vec<Line> = lines
        .iter()
        .skip(app.detail_scroll)
        .take(content_area.height.saturating_sub(2) as usize)
        .map(|s| Line::from(*s))
        .collect();

    let paragraph = Paragraph::new(visible_lines)
        .block(content_block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, content_area);
}

fn format_overview(detail: &TrafficDetail) -> String {
    let status_emoji = match detail.status {
        0 => "⏳",
        200..=299 => "✅",
        300..=399 => "↗️",
        400..=499 => "⚠️",
        500..=599 => "❌",
        _ => "❓",
    };

    format!(
        r#"
  {} {}  {}

  ┌─────────────────────────────────────────────────────────────────────
  │  URL          {}{}
  │  Method       {}
  │  Protocol     {}
  │  Status       {} {}
  │
  │  Request
  │    Size       {}
  │    Type       {}
  │
  │  Response
  │    Size       {}
  │    Type       {}
  │    Duration   {}
  │
  │  Client
  │    IP         {}
  │    App        {}
  │
  │  Flags
  │    WebSocket  {}
  │    SSE        {}
  │    Tunnel     {}
  └─────────────────────────────────────────────────────────────────────
"#,
        status_emoji,
        detail.method,
        detail.url,
        detail.host,
        detail.path,
        detail.method,
        detail.protocol,
        detail.status,
        status_emoji,
        format_size(detail.request_size),
        detail.request_content_type.as_deref().unwrap_or("-"),
        format_size(detail.response_size),
        detail.content_type.as_deref().unwrap_or("-"),
        format_duration(detail.duration_ms),
        detail.client_ip,
        detail.client_app.as_deref().unwrap_or("-"),
        if detail.is_websocket { "Yes" } else { "No" },
        if detail.is_sse { "Yes" } else { "No" },
        if detail.is_tunnel { "Yes" } else { "No" },
    )
}

fn format_headers(headers: &Option<Vec<(String, String)>>, title: &str) -> String {
    match headers {
        Some(h) if !h.is_empty() => {
            let mut result = format!("\n  {}\n  {}\n\n", title, "─".repeat(60));
            for (name, value) in h {
                result.push_str(&format!("  {}: {}\n", name, value));
            }
            result
        }
        _ => format!("\n  No {} available", title.to_lowercase()),
    }
}

fn format_body(app: &InteractiveApp) -> String {
    let mut result = String::new();

    result.push_str("\n  ═══ Request Body ═══\n\n");
    match &app.request_body {
        Some(body) if !body.is_empty() => {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                    for line in pretty.lines() {
                        result.push_str(&format!("  {}\n", line));
                    }
                } else {
                    result.push_str(&format!("  {}\n", body));
                }
            } else {
                for line in body.lines().take(100) {
                    result.push_str(&format!("  {}\n", line));
                }
            }
        }
        _ => result.push_str("  (empty)\n"),
    }

    result.push_str("\n  ═══ Response Body ═══\n\n");
    match &app.response_body {
        Some(body) if !body.is_empty() => {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                    for line in pretty.lines() {
                        result.push_str(&format!("  {}\n", line));
                    }
                } else {
                    result.push_str(&format!("  {}\n", body));
                }
            } else {
                for line in body.lines().take(100) {
                    result.push_str(&format!("  {}\n", line));
                }
            }
        }
        _ => result.push_str("  (empty)\n"),
    }

    result
}

fn draw_status_bar(f: &mut Frame, app: &InteractiveApp, area: Rect) {
    let help_text = match app.mode {
        AppMode::List => " ↑/k ↓/j Navigate │ Enter View │ /,s Search │ r Refresh │ q Quit ",
        AppMode::Search => " Type to search │ Enter Confirm │ Esc Cancel ",
        AppMode::Detail => " Tab Switch │ ↑/k ↓/j Scroll │ Esc/q Back ",
    };

    let status = Paragraph::new(help_text)
        .style(Style::default().fg(RColor::DarkGray).bg(RColor::Black))
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(status, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_traffic_summary() -> TrafficSummary {
        TrafficSummary {
            id: "id-1".to_string(),
            seq: 1,
            ts: 0,
            m: "GET".to_string(),
            h: "example.com".to_string(),
            p: "/path".to_string(),
            s: 200,
            ct: None,
            req_ct: None,
            req_sz: 123,
            res_sz: 456,
            dur: 789,
            proto: "HTTP/1.1".to_string(),
            cip: "127.0.0.1".to_string(),
            capp: None,
            flags: 0,
            fc: 0,
            st: String::new(),
            et: None,
        }
    }

    #[test]
    fn build_search_request_body_includes_basic_conditions() {
        let options = SearchOptions {
            keyword: "token".to_string(),
            filter_method: Some("POST".to_string()),
            filter_host: Some("api.example.com".to_string()),
            filter_path: Some("/v1/chat".to_string()),
            filter_listener_port: Some(50831),
            ..SearchOptions::default()
        };

        let body = build_search_request_body(&options, Some(42));

        assert_eq!(body["keyword"], "token");
        assert_eq!(body["cursor"], 42);
        assert_eq!(body["limit"], 50);
        assert_eq!(
            body["filters"]["conditions"],
            serde_json::json!([
                {
                    "field": "method",
                    "operator": "equals",
                    "value": "POST"
                },
                {
                    "field": "host",
                    "operator": "contains",
                    "value": "api.example.com"
                },
                {
                    "field": "path",
                    "operator": "contains",
                    "value": "/v1/chat"
                },
                {
                    "field": "listener_port",
                    "operator": "equals",
                    "value": "50831"
                }
            ])
        );
    }

    #[test]
    fn build_search_request_body_omits_conditions_when_not_needed() {
        let options = SearchOptions {
            keyword: "token".to_string(),
            ..SearchOptions::default()
        };

        let body = build_search_request_body(&options, None);

        assert!(body["filters"]["conditions"].is_null());
        assert!(body["cursor"].is_null());
    }

    #[test]
    fn build_search_request_body_supports_granular_scope_flags() {
        let options = SearchOptions {
            keyword: "token".to_string(),
            scope_request_headers: true,
            scope_response_body: true,
            ..SearchOptions::default()
        };

        let body = build_search_request_body(&options, None);

        assert_eq!(
            body["scope"],
            serde_json::json!({
                "all": false,
                "url": false,
                "request_headers": true,
                "response_headers": false,
                "request_body": false,
                "response_body": true,
            })
        );
    }

    #[test]
    fn build_search_request_body_keeps_legacy_scope_aliases() {
        let options = SearchOptions {
            keyword: "token".to_string(),
            scope_headers: true,
            scope_body: true,
            ..SearchOptions::default()
        };

        let body = build_search_request_body(&options, None);

        assert_eq!(
            body["scope"],
            serde_json::json!({
                "all": false,
                "url": false,
                "request_headers": true,
                "response_headers": true,
                "request_body": true,
                "response_body": true,
            })
        );
    }

    #[test]
    fn parse_sse_events_supports_error_payload() {
        let sse = b"event: error\ndata: {\"message\":\"search stream failed\"}\n\n";
        let events: Vec<_> = parse_sse_events(&sse[..]).collect();

        assert_eq!(events.len(), 1);
        match &events[0] {
            SseEvent::Error(error) => assert_eq!(error.message, "search stream failed"),
            other => panic!("unexpected event: {:?}", std::mem::discriminant(other)),
        }
    }

    #[test]
    fn parse_sse_events_supports_done_searched_range() {
        let sse = b"event: done\ndata: {\"total_searched\":0,\"total_matched\":0,\"next_cursor\":null,\"has_more\":false,\"search_id\":\"s1\",\"searched_range\":{\"oldest_ts_ms\":null,\"newest_ts_ms\":null,\"scanned_count\":0}}\n\n";
        let events: Vec<_> = parse_sse_events(&sse[..]).collect();

        assert_eq!(events.len(), 1);
        match &events[0] {
            SseEvent::Done(done) => {
                assert_eq!(done.total_searched, 0);
                assert_eq!(done.searched_range.scanned_count, 0);
                assert!(done.searched_range.oldest_ts_ms.is_none());
            }
            other => panic!("unexpected event: {:?}", std::mem::discriminant(other)),
        }
    }

    #[test]
    fn parse_sse_events_ignores_unknown_events() {
        let sse = b"event: unknown\ndata: {\"foo\":1}\n\n";
        let events: Vec<_> = parse_sse_events(&sse[..]).collect();
        assert!(events.is_empty());
    }

    #[test]
    fn search_result_to_json_preserves_include_payloads_for_ndjson() {
        let item = SearchResultItem {
            record: sample_traffic_summary(),
            matches: vec![MatchLocation {
                field: "request_body".to_string(),
                preview: "token".to_string(),
                offset: 5,
            }],
            bodies: Some(BodiesPayloadIn {
                request: Some(BodyChunkIn {
                    bytes_b64: "eyJ0b2tlbiI6dHJ1ZX0=".to_string(),
                    size: 14,
                    truncated: false,
                    content_type: Some("application/json".to_string()),
                }),
                response: None,
            }),
            headers: Some(HeadersPayloadIn {
                request: vec![("X-Trace-Id".to_string(), "abc123".to_string())],
                response: vec![("Content-Type".to_string(), "application/json".to_string())],
            }),
        };

        let value = search_result_to_json(&item, true);

        assert_eq!(value["type"], "result");
        assert_eq!(
            value["bodies"]["request"]["bytes_b64"],
            "eyJ0b2tlbiI6dHJ1ZX0="
        );
        assert_eq!(value["bodies"]["request"]["truncated"], false);
        assert_eq!(value["headers"]["request"][0][0], "X-Trace-Id");
        assert_eq!(value["headers"]["response"][0][1], "application/json");
    }

    #[test]
    fn output_format_from_str_accepts_short_and_long_aliases() {
        use std::str::FromStr;

        assert!(matches!(
            super::OutputFormat::from_str("table").unwrap(),
            super::OutputFormat::Table
        ));
        assert!(matches!(
            super::OutputFormat::from_str("compact").unwrap(),
            super::OutputFormat::Compact
        ));
        assert!(matches!(
            super::OutputFormat::from_str("c").unwrap(),
            super::OutputFormat::Compact
        ));
        assert!(matches!(
            super::OutputFormat::from_str("json").unwrap(),
            super::OutputFormat::Json
        ));
        assert!(matches!(
            super::OutputFormat::from_str("j").unwrap(),
            super::OutputFormat::Json
        ));
        assert!(matches!(
            super::OutputFormat::from_str("json-pretty").unwrap(),
            super::OutputFormat::JsonPretty
        ));
        assert!(matches!(
            super::OutputFormat::from_str("jp").unwrap(),
            super::OutputFormat::JsonPretty
        ));
    }

    #[test]
    fn highlight_keyword_wraps_all_case_insensitive_matches() {
        let text = "Hello Token token";
        let highlighted = super::highlight_keyword(text, "token", true);
        // two highlighted segments
        assert!(highlighted.matches("\u{1b}[1;33m").count() >= 2);
        assert!(highlighted.ends_with("\u{1b}[0m"));
    }

    #[test]
    fn highlight_keyword_returns_original_when_no_match_or_disabled() {
        let text = "hello";
        assert_eq!(super::highlight_keyword(text, "token", true), text);
        assert_eq!(super::highlight_keyword(text, "", true), text);
        assert_eq!(super::highlight_keyword(text, "hello", false), text);
    }

    #[test]
    fn truncate_str_limits_visible_characters_and_appends_ellipsis() {
        let s = "abcdefghijklmnopqrstuvwxyz";
        let truncated = super::truncate_str(s, 10);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.chars().count(), 10);

        // Unicode-safe: do not split surrogate pairs
        let s = "😀😀😀😀😀"; // 5 emojis
        let truncated = super::truncate_str(s, 4);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn format_size_and_duration_and_number_are_human_friendly() {
        assert_eq!(super::format_size(500), "500B");
        assert_eq!(super::format_size(1_024), "1.0KB");
        assert_eq!(super::format_size(1_048_576), "1.0MB");

        assert_eq!(super::format_duration(0), "...");
        assert_eq!(super::format_duration(500), "500ms");
        assert_eq!(super::format_duration(2_500), "2.50s");

        assert_eq!(super::format_number(999), "999");
        assert_eq!(super::format_number(1_500), "1.5K");
        assert_eq!(super::format_number(1_500_000), "1.5M");
    }

    #[test]
    fn match_field_label_maps_known_fields_and_falls_back_to_match() {
        assert_eq!(super::match_field_label("url"), "URL");
        assert_eq!(super::match_field_label("request_headers"), "REQ_HDR");
        assert_eq!(super::match_field_label("response_headers"), "RES_HDR");
        assert_eq!(super::match_field_label("request_body"), "REQ_BODY");
        assert_eq!(super::match_field_label("response_body"), "RES_BODY");
        assert_eq!(super::match_field_label("frames"), "FRAMES");
        assert_eq!(super::match_field_label("other"), "MATCH");
    }

    #[test]
    fn highlight_spans_returns_spans_with_highlight_style() {
        let spans = super::highlight_spans("hello token world", "token");
        assert!(!spans.is_empty());
        let joined: String = spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined, "hello token world");
    }

    #[test]
    fn build_match_summary_returns_no_match_when_matches_empty() {
        let item = SearchResultItem {
            record: sample_traffic_summary(),
            matches: Vec::new(),
            bodies: None,
            headers: None,
        };
        let (header, _preview) = build_match_summary(&item, "token");
        let header_debug = format!("{header:?}");
        assert!(header_debug.contains("NO_MATCH"));
    }

    #[test]
    fn build_match_summary_prefers_non_url_match_and_truncates_preview() {
        let item = SearchResultItem {
            record: sample_traffic_summary(),
            matches: vec![
                MatchLocation {
                    field: "url".to_string(),
                    preview: "https://example.com/ignored".to_string(),
                    offset: 0,
                },
                MatchLocation {
                    field: "request_body".to_string(),
                    preview: "a".repeat(200),
                    offset: 0,
                },
            ],
            bodies: None,
            headers: None,
        };
        let (header, preview) = build_match_summary(&item, "a");
        let header_debug = format!("{header:?}");
        assert!(
            header_debug.contains("REQ_BODY"),
            "header was {header_debug}"
        );
        let preview_debug = format!("{preview:?}");
        assert!(preview_debug.contains("..."));
    }

    #[test]
    fn format_headers_formats_present_headers_and_reports_empty() {
        let headers = Some(vec![(
            "Content-Type".to_string(),
            "application/json".to_string(),
        )]);
        let text = format_headers(&headers, "Headers");
        assert!(text.contains("Headers"));
        assert!(text.contains("Content-Type: application/json"));

        let empty = format_headers(&None, "Headers");
        assert!(empty.to_lowercase().contains("no headers"));
    }

    #[test]
    fn format_body_formats_json_and_plain_text_bodies() {
        let mut app = InteractiveApp::new(SearchOptions::default());
        app.request_body = Some("{\"foo\":1}".to_string());
        app.response_body = Some("line1\nline2".to_string());

        let text = format_body(&app);
        assert!(text.contains("Request Body"));
        assert!(text.contains("\"foo\": 1"));
        assert!(text.contains("Response Body"));
        assert!(text.contains("line1"));
    }

    #[test]
    fn parse_duration_ms_handles_units() {
        assert_eq!(parse_duration_ms("30s"), Some(30_000));
        assert_eq!(parse_duration_ms("5m"), Some(300_000));
        assert_eq!(parse_duration_ms("2h"), Some(2 * 60 * 60 * 1000));
        assert_eq!(parse_duration_ms("1d"), Some(24 * 60 * 60 * 1000));
    }

    #[test]
    fn parse_duration_ms_default_unit_seconds() {
        assert_eq!(parse_duration_ms("45"), Some(45_000));
    }

    #[test]
    fn parse_duration_ms_rejects_unknown_unit() {
        assert!(parse_duration_ms("5y").is_none());
        assert!(parse_duration_ms("").is_none());
        assert!(parse_duration_ms("abc").is_none());
    }

    #[test]
    fn split_eq_basic() {
        assert_eq!(split_eq("name=value"), Some(("name", "value")));
        assert_eq!(split_eq("$.a.b=42"), Some(("$.a.b", "42")));
        assert_eq!(split_eq("=missing"), None);
        assert_eq!(split_eq("missing"), None);
    }

    #[test]
    fn build_search_request_body_adds_jsonpath_and_header_conditions() {
        let options = SearchOptions {
            keyword: String::new(),
            req_json: vec!["$.user.name=alice".to_string()],
            res_json: vec!["$.data.errno=0".to_string()],
            req_header_eq: vec!["X-Trace-Id=abc".to_string()],
            res_header_eq: vec!["Set-Cookie=foo".to_string()],
            ..SearchOptions::default()
        };
        let body = build_search_request_body(&options, None);
        let conds = body["filters"]["conditions"]
            .as_array()
            .expect("conditions array");
        let fields: Vec<&str> = conds
            .iter()
            .map(|c| c["field"].as_str().unwrap_or(""))
            .collect();
        assert!(fields.contains(&"req.body.user.name"));
        assert!(fields.contains(&"res.body.data.errno"));
        assert!(fields.contains(&"req.header.X-Trace-Id"));
        assert!(fields.contains(&"res.header.Set-Cookie"));
    }

    #[test]
    fn build_search_request_body_sets_time_range_from_latest() {
        let options = SearchOptions {
            keyword: String::new(),
            latest: Some("5m".to_string()),
            ..SearchOptions::default()
        };
        let body = build_search_request_body(&options, None);
        let tr = &body["time_range"];
        assert!(tr.is_object(), "expected time_range to be set, got {tr}");
        assert!(tr["since_ms"].is_i64() || tr["since_ms"].is_u64());
    }

    #[test]
    fn parse_include_tokens_basic_aliases() {
        let (rb, sb, rh, sh) = parse_include_tokens(&[
            "request-body".to_string(),
            "response-body".to_string(),
            "request-headers".to_string(),
            "response-headers".to_string(),
        ]);
        assert!(rb && sb && rh && sh);
    }

    #[test]
    fn parse_include_tokens_short_aliases() {
        let (rb, sb, rh, sh) = parse_include_tokens(&[
            "req-body".to_string(),
            "res-body".to_string(),
            "req-headers".to_string(),
            "res-headers".to_string(),
        ]);
        assert!(rb && sb && rh && sh);
    }

    #[test]
    fn parse_include_tokens_shortcuts() {
        let (rb, sb, rh, sh) = parse_include_tokens(&["bodies".to_string()]);
        assert!(rb && sb && !rh && !sh);
        let (rb, sb, rh, sh) = parse_include_tokens(&["headers".to_string()]);
        assert!(!rb && !sb && rh && sh);
    }

    #[test]
    fn parse_include_tokens_ignores_unknown_and_empty() {
        let (rb, sb, rh, sh) =
            parse_include_tokens(&["mystery".to_string(), "".to_string(), "  ".to_string()]);
        assert!(!rb && !sb && !rh && !sh);
    }

    #[test]
    fn build_search_request_body_omits_include_when_all_default() {
        let options = SearchOptions {
            keyword: "x".to_string(),
            ..SearchOptions::default()
        };
        let body = build_search_request_body(&options, None);
        assert!(
            body.get("include").is_none(),
            "include block should be omitted when no flags set"
        );
    }

    #[test]
    fn build_search_request_body_emits_include_block_when_any_flag_set() {
        let options = SearchOptions {
            keyword: "x".to_string(),
            include_response_body: true,
            ..SearchOptions::default()
        };
        let body = build_search_request_body(&options, None);
        let include = &body["include"];
        assert!(include.is_object());
        assert_eq!(include["response_body"], serde_json::json!(true));
        assert_eq!(include["request_body"], serde_json::json!(false));
        assert!(include.get("max_body_bytes").is_none());
    }

    #[test]
    fn build_search_request_body_emits_include_block_for_max_body_only() {
        let options = SearchOptions {
            keyword: "x".to_string(),
            max_body: Some(2048),
            ..SearchOptions::default()
        };
        let body = build_search_request_body(&options, None);
        let include = &body["include"];
        assert!(include.is_object());
        assert_eq!(include["max_body_bytes"], serde_json::json!(2048));
    }
}
