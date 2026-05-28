//! Auto-compact window tracking for status/debug telemetry.
//!
//! Bifrost's runtime compaction trigger follows Codex's current total-token
//! threshold. This window still tracks a per-compaction-cycle prefill baseline
//! for diagnostics:
//! - Each compaction cycle creates a new "window" with an ordinal
//! - The window tracks a prefill baseline (initial input tokens)
//! - Debug/status consumers can inspect `active_context - prefill_baseline`
//!
//! Key design: the prefill baseline is established per window from either:
//! 1. Server-observed `input_tokens` from the first API response (preferred)
//! 2. Local character-based estimate (fallback)
//!
//! Estimates may be refreshed until a server observation is recorded. Once a
//! server observation exists, it cannot be overwritten by estimates.

use crate::token_counting::TokenSource;

/// Read-only snapshot of the auto-compact window state.
///
/// Used by external code to inspect window state without holding a mutable
/// reference to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCompactWindowSnapshot {
    /// Sequential window number (starts at 1, increments on each compaction).
    pub ordinal: u64,
    /// The prefill input token baseline for this window.
    /// `None` means no baseline has been established yet.
    pub prefill_input_tokens: Option<u64>,
}

/// Internal representation of the prefill baseline source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefillSource {
    ServerObserved(u64),
    Estimated(u64),
}

/// Tracks auto-compaction window telemetry for a session.
///
/// The window is not the compaction trigger; `compact::should_compact()`
/// compares Codex-style total effective context tokens to the configured
/// threshold. This state is retained for progress/debug surfaces.
///
/// Lifecycle:
/// 1. Window starts at ordinal=1 with no prefill
/// 2. First API response → `ensure_server_observed_prefill_from_usage()` locks baseline
/// 3. Context grows beyond threshold relative to baseline → compaction triggers
/// 4. After compaction → `start_next_window()` increments ordinal and clears prefill
/// 5. Repeat from step 2
#[derive(Debug, Clone)]
pub struct AutoCompactWindow {
    /// Sequential window number. Starts at 1 and increments after each compaction.
    ordinal: u64,
    /// Absolute input-token baseline for the current compaction window.
    /// `body_after_prefix` is computed as `active_context - prefill_baseline`.
    prefill_input_tokens: Option<PrefillSource>,
}

impl AutoCompactWindow {
    /// Create a new window starting at ordinal 1 with no prefill baseline.
    pub fn new() -> Self {
        Self {
            ordinal: 1,
            prefill_input_tokens: None,
        }
    }

    /// Clear the prefill baseline without advancing the window.
    /// Used after history replacement when the old baseline is no longer valid.
    pub fn clear_prefill(&mut self) {
        self.prefill_input_tokens = None;
    }

    /// Advance to the next compaction window.
    ///
    /// Increments the ordinal and clears the prefill baseline so the new window
    /// starts fresh. Called after a successful compaction.
    pub fn start_next_window(&mut self) {
        self.ordinal = self.ordinal.saturating_add(1);
        self.clear_prefill();
    }

    /// Record the server-observed input token count as the prefill baseline.
    ///
    /// Called when the first API response in this window returns `usage.input_tokens`.
    /// Once a server observation is locked in, subsequent calls (whether server or
    /// estimated) do not overwrite it.
    pub fn ensure_server_observed_prefill_from_usage(&mut self, input_tokens: u64) {
        if matches!(
            self.prefill_input_tokens,
            Some(PrefillSource::ServerObserved(_))
        ) {
            return;
        }
        self.prefill_input_tokens = Some(PrefillSource::ServerObserved(input_tokens));
    }

    /// Set an estimated prefill baseline.
    ///
    /// Only takes effect if no server-observed baseline has been recorded yet.
    /// Estimates may be refreshed until server usage becomes available.
    /// This is the fallback path used when no API response is available.
    pub fn set_estimated_prefill(&mut self, tokens: u64) {
        if matches!(
            self.prefill_input_tokens,
            Some(PrefillSource::ServerObserved(_))
        ) {
            return;
        }
        self.prefill_input_tokens = Some(PrefillSource::Estimated(tokens));
    }

    /// Get the current window ordinal.
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// Get the prefill baseline as a `TokenSource`.
    pub fn prefill_token_source(&self) -> Option<TokenSource> {
        match self.prefill_input_tokens {
            Some(PrefillSource::ServerObserved(t)) => Some(TokenSource::ServerObserved(t)),
            Some(PrefillSource::Estimated(t)) => Some(TokenSource::Estimated(t)),
            None => None,
        }
    }

    /// Get the prefill baseline token count (regardless of source).
    pub fn prefill_tokens(&self) -> Option<u64> {
        match self.prefill_input_tokens {
            Some(PrefillSource::ServerObserved(t)) | Some(PrefillSource::Estimated(t)) => Some(t),
            None => None,
        }
    }

    /// Compute the "body after prefix" — the growth since the window started.
    ///
    /// This is the value compared against the compaction threshold:
    /// `active_context_tokens - prefill_baseline`
    ///
    /// If no baseline is set, returns the full active context (conservative).
    pub fn body_after_prefix(&self, active_context_tokens: u64) -> u64 {
        match self.prefill_tokens() {
            Some(baseline) => active_context_tokens.saturating_sub(baseline),
            None => active_context_tokens,
        }
    }

    /// Create a read-only snapshot of the current window state.
    pub fn snapshot(&self) -> AutoCompactWindowSnapshot {
        AutoCompactWindowSnapshot {
            ordinal: self.ordinal,
            prefill_input_tokens: self.prefill_tokens(),
        }
    }
}

impl Default for AutoCompactWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_window_has_ordinal_1_and_no_prefill() {
        let window = AutoCompactWindow::new();
        assert_eq!(
            window.snapshot(),
            AutoCompactWindowSnapshot {
                ordinal: 1,
                prefill_input_tokens: None,
            }
        );
    }

    #[test]
    fn test_set_estimated_prefill() {
        let mut window = AutoCompactWindow::new();
        window.set_estimated_prefill(150);
        assert_eq!(
            window.snapshot(),
            AutoCompactWindowSnapshot {
                ordinal: 1,
                prefill_input_tokens: Some(150),
            }
        );
    }

    #[test]
    fn test_server_observed_overrides_estimated() {
        let mut window = AutoCompactWindow::new();
        window.set_estimated_prefill(150);
        window.ensure_server_observed_prefill_from_usage(120);
        assert_eq!(
            window.snapshot(),
            AutoCompactWindowSnapshot {
                ordinal: 1,
                prefill_input_tokens: Some(120),
            }
        );
    }

    #[test]
    fn test_server_observed_locks_once() {
        let mut window = AutoCompactWindow::new();
        window.ensure_server_observed_prefill_from_usage(120);
        // Second server observation should NOT overwrite
        window.ensure_server_observed_prefill_from_usage(130);
        assert_eq!(window.prefill_tokens(), Some(120));
        // Estimated should NOT overwrite server observed
        window.set_estimated_prefill(90);
        assert_eq!(window.prefill_tokens(), Some(120));
    }

    #[test]
    fn test_start_next_window_increments_ordinal_and_clears_prefill() {
        let mut window = AutoCompactWindow::new();
        window.ensure_server_observed_prefill_from_usage(100);
        window.start_next_window();
        assert_eq!(
            window.snapshot(),
            AutoCompactWindowSnapshot {
                ordinal: 2,
                prefill_input_tokens: None,
            }
        );
    }

    #[test]
    fn test_body_after_prefix_with_baseline() {
        let mut window = AutoCompactWindow::new();
        window.ensure_server_observed_prefill_from_usage(1000);
        // 1500 active - 1000 prefill = 500 body
        assert_eq!(window.body_after_prefix(1500), 500);
    }

    #[test]
    fn test_body_after_prefix_without_baseline() {
        let window = AutoCompactWindow::new();
        // No baseline set, so full active context is returned
        assert_eq!(window.body_after_prefix(1500), 1500);
    }

    #[test]
    fn test_body_after_prefix_saturating_sub() {
        let mut window = AutoCompactWindow::new();
        window.ensure_server_observed_prefill_from_usage(2000);
        // active < prefill should return 0, not underflow
        assert_eq!(window.body_after_prefix(500), 0);
    }

    #[test]
    fn test_clear_prefill() {
        let mut window = AutoCompactWindow::new();
        window.ensure_server_observed_prefill_from_usage(100);
        window.clear_prefill();
        assert_eq!(window.prefill_tokens(), None);
        // After clear, can set estimated again
        window.set_estimated_prefill(200);
        assert_eq!(window.prefill_tokens(), Some(200));
    }

    #[test]
    fn test_multiple_window_cycles() {
        let mut window = AutoCompactWindow::new();

        // Window 1
        window.set_estimated_prefill(100);
        window.ensure_server_observed_prefill_from_usage(80);
        assert_eq!(window.ordinal(), 1);
        assert_eq!(window.prefill_tokens(), Some(80));

        // Compaction happens -> advance to window 2
        window.start_next_window();
        assert_eq!(window.ordinal(), 2);
        assert_eq!(window.prefill_tokens(), None);

        // Window 2
        window.ensure_server_observed_prefill_from_usage(50);
        assert_eq!(window.prefill_tokens(), Some(50));

        // Another compaction -> window 3
        window.start_next_window();
        assert_eq!(window.ordinal(), 3);
        assert_eq!(window.prefill_tokens(), None);
    }

    #[test]
    fn test_prefill_token_source() {
        let mut window = AutoCompactWindow::new();

        assert_eq!(window.prefill_token_source(), None);

        window.set_estimated_prefill(100);
        assert_eq!(
            window.prefill_token_source(),
            Some(TokenSource::Estimated(100))
        );

        window.ensure_server_observed_prefill_from_usage(80);
        assert_eq!(
            window.prefill_token_source(),
            Some(TokenSource::ServerObserved(80))
        );
    }
}
