//! Memory guard utilities for the Bifrost Agent memory system.
//!
//! Provides:
//! - Secret/credential redaction before writing memories
//! - No-op gate: determines if extracted memories have enough signal
//! - Developer message filtering for Phase 1 extraction input
//! - Rate limit guard to skip memory work when API quota is low
//! - Usage tracking types for memory consumption metrics

use tracing::debug;

// ───────────────── Secret Redaction ─────────────────

/// Redact potential secrets from text before writing to memory files.
/// Replaces detected secret patterns with `[REDACTED]`.
///
/// This is a best-effort heuristic — it catches common patterns but
/// is not a comprehensive secret scanner.
pub fn redact_secrets(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_pem_block = false;

    for line in text.lines() {
        let lower = line.to_lowercase();

        // Handle PEM block redaction
        if lower.contains("-----begin") {
            in_pem_block = true;
            result.push_str("[REDACTED]");
            result.push('\n');
            continue;
        }
        if in_pem_block {
            if lower.contains("-----end") {
                in_pem_block = false;
            }
            continue;
        }

        // Check for standalone token patterns (sk-xxx, ghp_xxx, etc.)
        let redacted_line = redact_standalone_tokens(line);
        let redacted_line = redact_bearer_tokens(&redacted_line);
        let redacted_line = redact_key_value_pairs(&redacted_line);

        result.push_str(&redacted_line);
        result.push('\n');
    }

    // Remove trailing newline if original didn't have one
    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Redact standalone token patterns like sk-abc123, ghp_xxxx, xoxb-xxxx, AKIA...
fn redact_standalone_tokens(line: &str) -> String {
    let token_prefixes = &[
        "sk-", "pk-", "ghp_", "gho_", "ghs_", "ghr_", "xoxb-", "xoxp-",
    ];
    let mut result = line.to_string();

    for prefix in token_prefixes {
        while let Some(start) = result.to_lowercase().find(&prefix.to_lowercase()) {
            // Find the end of the token (non-whitespace, non-comma, non-quote sequence)
            let token_start = start;
            let rest = &result[start..];
            let token_end = rest
                .find(|c: char| c.is_whitespace() || c == ',' || c == '"' || c == '\'')
                .map(|i| start + i)
                .unwrap_or(result.len());
            result.replace_range(token_start..token_end, "[REDACTED]");
        }
    }

    // Redact AWS access key IDs (AKIA followed by 16 alphanumeric chars)
    let mut i = 0;
    while i < result.len() {
        if result[i..].starts_with("AKIA") && result[i..].len() >= 20 {
            let candidate = &result[i..i + 20];
            if candidate.chars().all(|c| c.is_ascii_alphanumeric()) {
                result.replace_range(i..i + 20, "[REDACTED]");
            }
        }
        i += 1;
    }

    result
}

/// Redact Bearer token values.
fn redact_bearer_tokens(line: &str) -> String {
    let lower = line.to_lowercase();
    if let Some(pos) = lower.find("bearer ") {
        let value_start = pos + 7; // "bearer " length
        let rest = &line[value_start..];
        let value_end = rest
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .map(|i| value_start + i)
            .unwrap_or(line.len());
        if value_end > value_start {
            let mut result = line.to_string();
            result.replace_range(value_start..value_end, "[REDACTED]");
            return result;
        }
    }
    line.to_string()
}

/// Redact key=value style secrets (api_key=xxx, password=xxx, etc.).
fn redact_key_value_pairs(line: &str) -> String {
    let kv_patterns = &[
        "api_key",
        "apikey",
        "api-key",
        "secret",
        "token",
        "password",
        "passwd",
        "credential",
    ];

    let lower = line.to_lowercase();
    let mut result = line.to_string();

    for pattern in kv_patterns {
        if let Some(key_pos) = lower.find(pattern) {
            let after_key = key_pos + pattern.len();
            let remaining = &line[after_key..];
            // Look for = or : separator
            let trimmed = remaining.trim_start();
            let skip = remaining.len() - trimmed.len();
            if trimmed.starts_with('=') || trimmed.starts_with(':') {
                let sep_pos = after_key + skip + 1;
                let value_part = &line[sep_pos..];
                let value_trimmed = value_part.trim_start();
                let value_skip = value_part.len() - value_trimmed.len();
                let value_start = sep_pos + value_skip;

                // Strip surrounding quotes if present
                let (actual_start, quote_char) =
                    if value_trimmed.starts_with('"') || value_trimmed.starts_with('\'') {
                        (value_start + 1, Some(value_trimmed.chars().next().unwrap()))
                    } else {
                        (value_start, None)
                    };

                let rest = &line[actual_start..];
                let value_end = if let Some(q) = quote_char {
                    rest.find(q).map(|i| actual_start + i).unwrap_or(line.len())
                } else {
                    rest.find(|c: char| {
                        c.is_whitespace() || c == ',' || c == ';' || c == '"' || c == '\''
                    })
                    .map(|i| actual_start + i)
                    .unwrap_or(line.len())
                };

                if value_end > actual_start {
                    result.replace_range(actual_start..value_end, "[REDACTED]");
                    break; // Only redact first match per line to avoid index shifting
                }
            }
        }
    }

    result
}

// ───────────────── No-op Gate ─────────────────

/// Maximum length of a memory that's considered "too short to be useful".
const MIN_MEMORY_LENGTH: usize = 10;

/// Check if the extracted memories pass the no-op gate.
/// Returns `true` if the memories should be written (i.e., there's enough signal).
/// Returns `false` if all memories are empty/trivial and should be skipped.
pub fn passes_noop_gate(memories: &[String]) -> bool {
    let meaningful: Vec<_> = memories
        .iter()
        .filter(|m| !m.trim().is_empty() && m.trim().len() >= MIN_MEMORY_LENGTH)
        .collect();

    if meaningful.is_empty() {
        debug!(
            total = memories.len(),
            meaningful = meaningful.len(),
            "no-op gate filtered out all memories — nothing to write"
        );
        return false;
    }

    true
}

// ───────────────── Developer Message Filtering ─────────────────

/// Filter out developer/system instruction content from conversation text
/// before sending to the extraction model.
///
/// Removes:
/// - Lines that look like AGENTS.md instructions
/// - Content wrapped in `<skill>` tags
/// - Lines starting with common system prefixes
pub fn filter_developer_content(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_skill_block = false;

    for line in text.lines() {
        let trimmed = line.trim();

        // Handle <skill> blocks
        if trimmed.starts_with("<skill>") || trimmed.starts_with("<skill ") {
            in_skill_block = true;
            continue;
        }
        if trimmed.starts_with("</skill>") {
            in_skill_block = false;
            continue;
        }
        if in_skill_block {
            continue;
        }

        // Skip AGENTS.md markers
        if trimmed.starts_with("# AGENTS.md")
            || trimmed.starts_with("## Rules:")
            || trimmed.contains("AGENTS.md")
        {
            continue;
        }

        // Skip common system instruction prefixes
        if trimmed.starts_with("<system-reminder>")
            || trimmed.starts_with("</system-reminder>")
            || trimmed.starts_with("<always_applied_workspace_rules")
            || trimmed.starts_with("</always_applied_workspace_rules>")
        {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // Remove trailing newline if original didn't have one
    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

// ───────────────── Rate Limit Guard ─────────────────

/// Default minimum percentage of rate limit budget required to proceed.
const DEFAULT_MIN_REQUIRED_PERCENT: u64 = 20;

/// Check if memory operations should proceed based on rate limit budget.
///
/// `remaining_percent`: current API rate limit remaining percentage (0-100).
/// `min_required_percent`: minimum percentage required to proceed.
///
/// Returns `true` if there's enough budget to proceed with memory operations.
pub fn rate_limit_ok(remaining_percent: Option<u64>, min_required_percent: Option<i64>) -> bool {
    let remaining = match remaining_percent {
        Some(r) => r,
        None => {
            // No rate limit info available — proceed optimistically
            return true;
        }
    };

    let min_required = min_required_percent
        .map(|p| p.clamp(0, 100) as u64)
        .unwrap_or(DEFAULT_MIN_REQUIRED_PERCENT);

    if remaining < min_required {
        debug!(
            remaining_percent = remaining,
            min_required_percent = min_required,
            "rate limit guard: insufficient budget for memory operations"
        );
        return false;
    }

    true
}

// ───────────────── Usage Tracking ─────────────────

/// Tracks how often a memory has been referenced/used.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MemoryUsageRecord {
    /// Number of times this memory was cited in model responses.
    pub usage_count: u64,
    /// Unix timestamp of last usage.
    pub last_usage_unix: u64,
    /// Whether this memory has been selected for Phase 2 consolidation.
    pub selected_for_phase2: bool,
}

/// Memory pollution state — tracks if external context has tainted the session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MemoryPollutionState {
    /// Normal memory generation.
    #[default]
    Clean,
    /// External context (MCP, web search) detected — memories from this session
    /// should be treated with lower confidence.
    Polluted,
}

/// Indicators that suggest external context injection.
const POLLUTION_INDICATORS: &[&str] = &[
    "mcp_",
    "tool_call",
    "WebSearch",
    "WebFetch",
    "external_api",
    "SearchCodebase",
    "<invoke",
    "mcp://",
];

/// Check if a session should be marked as polluted based on external context indicators.
pub fn detect_pollution(messages: &[&str]) -> MemoryPollutionState {
    for msg in messages {
        let lower = msg.to_lowercase();
        for indicator in POLLUTION_INDICATORS {
            if lower.contains(&indicator.to_lowercase()) {
                debug!(indicator, "pollution detected in session messages");
                return MemoryPollutionState::Polluted;
            }
        }
    }
    MemoryPollutionState::Clean
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── redact_secrets tests ───

    #[test]
    fn redact_secrets_replaces_api_keys() {
        let input = "my api_key=super_secret_value here";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("super_secret_value"));
    }

    #[test]
    fn redact_secrets_preserves_normal_text() {
        let input = "This is a normal line with no secrets at all.";
        let output = redact_secrets(input);
        assert_eq!(output, input);
    }

    #[test]
    fn redact_secrets_handles_bearer_tokens() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("eyJhbGciOiJIUzI1NiJ9.test"));
    }

    #[test]
    fn redact_secrets_handles_github_tokens() {
        let input = "export GITHUB_TOKEN=ghp_abc123def456ghi789";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("ghp_abc123def456ghi789"));
    }

    #[test]
    fn redact_secrets_handles_pem_blocks() {
        let input =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("MIIEpAIBAAKCAQEA"));
    }

    #[test]
    fn redact_secrets_handles_sk_tokens() {
        let input = "Using key sk-proj-abc123xyz for requests";
        let output = redact_secrets(input);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("sk-proj-abc123xyz"));
    }

    // ─── passes_noop_gate tests ───

    #[test]
    fn passes_noop_gate_returns_false_for_empty_memories() {
        let memories: Vec<String> = vec![];
        assert!(!passes_noop_gate(&memories));
    }

    #[test]
    fn passes_noop_gate_returns_false_for_trivial_memories() {
        let memories = vec!["hi".to_string(), "ok".to_string(), "".to_string()];
        assert!(!passes_noop_gate(&memories));
    }

    #[test]
    fn passes_noop_gate_returns_true_for_meaningful_memories() {
        let memories = vec![
            "User prefers dark mode for all UI components".to_string(),
            "Project uses Rust with tokio async runtime".to_string(),
        ];
        assert!(passes_noop_gate(&memories));
    }

    // ─── filter_developer_content tests ───

    #[test]
    fn filter_developer_content_removes_skill_tags() {
        let input = "Hello\n<skill>\nsome internal stuff\n</skill>\nWorld";
        let output = filter_developer_content(input);
        assert!(!output.contains("<skill>"));
        assert!(!output.contains("some internal stuff"));
        assert!(!output.contains("</skill>"));
        assert!(output.contains("Hello"));
        assert!(output.contains("World"));
    }

    #[test]
    fn filter_developer_content_preserves_normal_content() {
        let input = "This is a normal conversation.\nWith multiple lines.\nNothing special.";
        let output = filter_developer_content(input);
        assert_eq!(output, input);
    }

    #[test]
    fn filter_developer_content_removes_agents_md_references() {
        let input = "Normal line\n# AGENTS.md rules\nAnother normal line";
        let output = filter_developer_content(input);
        assert!(!output.contains("AGENTS.md"));
        assert!(output.contains("Normal line"));
        assert!(output.contains("Another normal line"));
    }

    // ─── rate_limit_ok tests ───

    #[test]
    fn rate_limit_ok_with_sufficient_budget() {
        assert!(rate_limit_ok(Some(50), Some(20)));
        assert!(rate_limit_ok(Some(80), None));
    }

    #[test]
    fn rate_limit_ok_with_insufficient_budget() {
        assert!(!rate_limit_ok(Some(10), Some(20)));
        assert!(!rate_limit_ok(Some(5), None)); // default threshold is 20
    }

    #[test]
    fn rate_limit_ok_with_no_info_returns_true() {
        assert!(rate_limit_ok(None, None));
        assert!(rate_limit_ok(None, Some(50)));
    }

    // ─── detect_pollution tests ───

    #[test]
    fn detect_pollution_detects_mcp_content() {
        let messages = vec!["User called mcp_Chrome_DevTools_MCP_click to interact with page"];
        assert_eq!(detect_pollution(&messages), MemoryPollutionState::Polluted);
    }

    #[test]
    fn detect_pollution_returns_clean_for_normal_messages() {
        let messages = vec![
            "How do I write a function in Rust?",
            "Here is the implementation...",
        ];
        assert_eq!(detect_pollution(&messages), MemoryPollutionState::Clean);
    }

    #[test]
    fn detect_pollution_detects_web_search() {
        let messages = vec!["Results from WebSearch: latest Rust docs"];
        assert_eq!(detect_pollution(&messages), MemoryPollutionState::Polluted);
    }
}
