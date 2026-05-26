//! Precise token counting with dual-track source attribution.
//!
//! Aligned with Codex's token tracking approach:
//! - `TokenSource::ServerObserved`: Real token count from API response `usage.input_tokens`
//! - `TokenSource::Estimated`: Fallback character-based estimation (chars / 4)
//!
//! The dual-track system ensures we use the most accurate data available while
//! maintaining a consistent interface for compaction threshold decisions.

/// Source of a token count measurement.
///
/// Mirrors Codex's approach where server-observed counts from the API response
/// are preferred over local character-based estimates. Once a server observation
/// is recorded, it supersedes any prior estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// Token count observed from the API response (`usage.input_tokens`).
    /// This is the authoritative measurement from the model provider.
    ServerObserved(u64),
    /// Token count estimated locally via character-based heuristic (chars / 4).
    /// Used as a fallback when no API response has been received yet.
    Estimated(u64),
    /// Token count combines server-observed history with locally estimated
    /// messages appended after the last model response.
    Mixed {
        server_observed: u64,
        estimated_total: u64,
    },
}

impl TokenSource {
    /// Extract the raw token count regardless of source.
    pub fn tokens(&self) -> u64 {
        match self {
            Self::ServerObserved(t) | Self::Estimated(t) => *t,
            Self::Mixed {
                estimated_total, ..
            } => *estimated_total,
        }
    }

    /// Returns true if the source is a server-observed measurement.
    pub fn is_server_observed(&self) -> bool {
        matches!(self, Self::ServerObserved(_))
    }

    /// Returns true if the source is a local estimate.
    pub fn is_estimated(&self) -> bool {
        matches!(self, Self::Estimated(_))
    }

    pub fn is_mixed(&self) -> bool {
        matches!(self, Self::Mixed { .. })
    }
}

/// Token usage snapshot combining input context tokens and their source.
///
/// This is the primary interface for compaction decisions. The `source` field
/// indicates whether the measurement came from the API or is a local estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsageSnapshot {
    /// The active context token count (input tokens for the next request).
    pub active_context_tokens: u64,
    /// How the token count was determined.
    pub source: TokenSource,
}

impl TokenUsageSnapshot {
    /// Create a snapshot from a server-observed value.
    pub fn from_server(tokens: u64) -> Self {
        Self {
            active_context_tokens: tokens,
            source: TokenSource::ServerObserved(tokens),
        }
    }

    /// Create a snapshot from a local estimate.
    pub fn from_estimate(tokens: u64) -> Self {
        Self {
            active_context_tokens: tokens,
            source: TokenSource::Estimated(tokens),
        }
    }

    pub fn from_mixed(server_observed: u64, estimated_total: u64) -> Self {
        Self {
            active_context_tokens: estimated_total,
            source: TokenSource::Mixed {
                server_observed,
                estimated_total,
            },
        }
    }
}

/// Approximate token count from text using character-based heuristic.
///
/// Uses the standard 4 bytes per token estimate, matching the existing
/// `approx_token_count` in `compact.rs`.
pub fn approx_token_count_from_text(text: &str) -> u64 {
    const APPROX_BYTES_PER_TOKEN: u64 = 4;
    let len = text.len() as u64;
    len.saturating_add(APPROX_BYTES_PER_TOKEN.saturating_sub(1)) / APPROX_BYTES_PER_TOKEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_source_server_observed() {
        let source = TokenSource::ServerObserved(150);
        assert_eq!(source.tokens(), 150);
        assert!(source.is_server_observed());
        assert!(!source.is_estimated());
    }

    #[test]
    fn test_token_source_estimated() {
        let source = TokenSource::Estimated(200);
        assert_eq!(source.tokens(), 200);
        assert!(!source.is_server_observed());
        assert!(source.is_estimated());
    }

    #[test]
    fn test_token_source_mixed() {
        let source = TokenSource::Mixed {
            server_observed: 100,
            estimated_total: 140,
        };
        assert_eq!(source.tokens(), 140);
        assert!(!source.is_server_observed());
        assert!(!source.is_estimated());
        assert!(source.is_mixed());
    }

    #[test]
    fn test_token_usage_snapshot_from_server() {
        let snapshot = TokenUsageSnapshot::from_server(500);
        assert_eq!(snapshot.active_context_tokens, 500);
        assert!(snapshot.source.is_server_observed());
    }

    #[test]
    fn test_token_usage_snapshot_from_estimate() {
        let snapshot = TokenUsageSnapshot::from_estimate(400);
        assert_eq!(snapshot.active_context_tokens, 400);
        assert!(snapshot.source.is_estimated());
    }

    #[test]
    fn test_approx_token_count_from_text() {
        assert_eq!(approx_token_count_from_text(""), 0);
        assert_eq!(approx_token_count_from_text("abcd"), 1);
        assert_eq!(approx_token_count_from_text("abcde"), 2);
        assert_eq!(approx_token_count_from_text("abcdefgh"), 2);
        // 12 chars -> 3 tokens
        assert_eq!(approx_token_count_from_text("abcdefghijkl"), 3);
    }
}
