//! `bifrost capture wait` — block until the next traffic record matches a
//! filter, then print the captured record as a single JSON line.
//!
//! See `design/capture-wait.md` for the full contract.

use std::time::Duration;

use bifrost_core::direct_ureq_agent_builder;
use serde::{Deserialize, Serialize};

const MAX_TIMEOUT_SECS: u64 = 600;
pub const EXIT_TIMEOUT: i32 = 124;

#[derive(Debug, Clone)]
pub enum CaptureOutputFormat {
    Human,
    Json,
}

impl CaptureOutputFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid --format value {other:?}, expected human|json"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureWaitOptions {
    pub host: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub timeout: Duration,
    pub open: Option<String>,
    pub format: CaptureOutputFormat,
    pub port: u16,
}

#[derive(Debug, Serialize)]
struct CaptureWaitApiRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    host_contains: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_contains: Option<&'a str>,
    timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
struct CaptureWaitApiResponse {
    matched: bool,
    #[serde(default)]
    record: Option<serde_json::Value>,
    #[serde(default)]
    waited_ms: u64,
    #[serde(default)]
    scanned_count: usize,
}

/// Parse a human duration: `30s`, `2m`, `1h`, `500ms`, bare digits (seconds).
pub fn parse_duration(raw: &str) -> Result<Duration, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("duration is empty".to_string());
    }
    let (number_part, unit) = if let Some(stripped) = trimmed.strip_suffix("ms") {
        (stripped, "ms")
    } else if let Some(stripped) = trimmed.strip_suffix('s') {
        (stripped, "s")
    } else if let Some(stripped) = trimmed.strip_suffix('m') {
        (stripped, "m")
    } else if let Some(stripped) = trimmed.strip_suffix('h') {
        (stripped, "h")
    } else {
        (trimmed, "s")
    };
    let n: u64 = number_part
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration {raw:?}, expected like 30s/2m/1h/500ms"))?;
    let dur = match unit {
        "ms" => Duration::from_millis(n),
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n * 60),
        "h" => Duration::from_secs(n * 3_600),
        _ => unreachable!(),
    };
    if dur.as_secs() > MAX_TIMEOUT_SECS {
        return Err(format!(
            "duration {raw:?} exceeds max {MAX_TIMEOUT_SECS}s ceiling"
        ));
    }
    Ok(dur)
}

/// Open `url` via the OS default launcher. Failures only log a warning.
fn try_open_url(url: &str) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("warning: opener exited with status {status}, url={url}"),
        Err(e) => eprintln!("warning: failed to launch opener for {url}: {e}"),
    }
}

pub fn run_capture_wait(options: CaptureWaitOptions) -> i32 {
    let timeout_secs = options.timeout.as_secs();
    let timeout_ms = options.timeout.as_millis() as u64;
    if matches!(options.format, CaptureOutputFormat::Human) {
        let host = options.host.as_deref().unwrap_or("*");
        let method = options.method.as_deref().unwrap_or("*");
        let path = options.path.as_deref().unwrap_or("*");
        eprintln!("Waiting for {method} {host} {path} (timeout {timeout_secs}s)...");
    }
    if let Some(url) = options.open.as_deref() {
        if matches!(options.format, CaptureOutputFormat::Human) {
            eprintln!("Opened {url}");
        }
        try_open_url(url);
    }

    let body = CaptureWaitApiRequest {
        host_contains: options.host.as_deref(),
        method: options.method.as_deref(),
        path_contains: options.path.as_deref(),
        timeout_ms,
    };

    // Add a generous read timeout so ureq does not cut the wait early.
    let read_timeout = options.timeout.saturating_add(Duration::from_secs(15));
    let agent = direct_ureq_agent_builder()
        .redirects(0)
        .timeout_read(read_timeout)
        .timeout_connect(Duration::from_secs(5))
        .build();

    let url = super::client::api_url(options.port, "/capture/wait");
    let started = std::time::Instant::now();
    let response = super::client::authenticated_request(&agent, "POST", &url)
        .send_json(serde_json::to_value(&body).unwrap());
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let resp = match response {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "error: capture wait request failed (is bifrost running on port {}?): {}",
                options.port, e
            );
            return 1;
        }
    };

    let text = match resp.into_string() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: failed to read response: {e}");
            return 1;
        }
    };

    let parsed: CaptureWaitApiResponse = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: invalid response: {e}: {text}");
            return 1;
        }
    };

    let waited_ms = if parsed.waited_ms == 0 {
        elapsed_ms
    } else {
        parsed.waited_ms
    };

    match options.format {
        CaptureOutputFormat::Json => {
            println!("{text}");
            if parsed.matched {
                0
            } else {
                EXIT_TIMEOUT
            }
        }
        CaptureOutputFormat::Human => {
            if parsed.matched {
                let record = parsed.record.unwrap_or(serde_json::Value::Null);
                let id = record
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown>");
                let host = record.get("host").and_then(|v| v.as_str()).unwrap_or("");
                let method = record.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let path = record.get("path").and_then(|v| v.as_str()).unwrap_or("");
                println!(
                    "[{:.1}s] Captured request id={id} host={host} method={method} path={path}",
                    waited_ms as f64 / 1000.0
                );
                0
            } else {
                eprintln!(
                    "Timed out after {:.1}s, scanned {} records",
                    waited_ms as f64 / 1000.0,
                    parsed.scanned_count
                );
                EXIT_TIMEOUT
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
    }

    #[test]
    fn parse_duration_minutes_and_hours() {
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        // 10m == 600s == MAX_TIMEOUT_SECS, accepted at the ceiling.
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        // 1h exceeds the 600s ceiling and must be rejected so the server-side
        // cap (MAX_TIMEOUT_MS = 600_000) is not silently clamped down.
        assert!(parse_duration("1h").is_err());
    }

    #[test]
    fn parse_duration_milliseconds() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    }

    #[test]
    fn parse_duration_rejects_bad_input() {
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("99h").is_err()); // exceeds max
    }

    #[test]
    fn parse_format() {
        assert!(matches!(
            CaptureOutputFormat::parse("human").unwrap(),
            CaptureOutputFormat::Human
        ));
        assert!(matches!(
            CaptureOutputFormat::parse("JSON").unwrap(),
            CaptureOutputFormat::Json
        ));
        assert!(CaptureOutputFormat::parse("yaml").is_err());
    }
}
