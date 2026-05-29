//! Turn Timing metrics for Bifrost Agent.
//!
//! Tracks TTFT (Time to First Token) and TTFM (Time to First Message) for
//! each turn, providing observability into model response latency. Modeled
//! after Codex's `turn_timing.rs`.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// TurnTimingState
// ---------------------------------------------------------------------------

/// Thread-safe timing state for a single turn.
///
/// Tracks when the turn started and when the first token/message arrived,
/// enabling TTFT and TTFM calculations.
#[derive(Debug, Default)]
pub struct TurnTimingState {
    inner: Mutex<TurnTimingInner>,
}

#[derive(Debug, Default)]
struct TurnTimingInner {
    /// Monotonic instant when the turn started (model request sent).
    started_at: Option<Instant>,
    /// Unix timestamp (seconds) when the turn started.
    started_at_unix_secs: Option<i64>,
    /// When the first streaming token was received.
    first_token_at: Option<Instant>,
    /// When the first complete message was received.
    first_message_at: Option<Instant>,
}

impl TurnTimingState {
    /// Mark the turn as started. Resets first_token and first_message timestamps.
    /// Returns the start time as unix milliseconds.
    pub fn mark_turn_started(&self, started_at: Instant) -> i64 {
        let unix_ms = now_unix_timestamp_ms();
        if let Ok(mut inner) = self.inner.lock() {
            inner.started_at = Some(started_at);
            inner.started_at_unix_secs = Some(unix_ms / 1000);
            inner.first_token_at = None;
            inner.first_message_at = None;
        }
        unix_ms
    }

    /// Returns the unix seconds timestamp of when the turn started.
    pub fn started_at_unix_secs(&self) -> Option<i64> {
        self.inner.lock().ok()?.started_at_unix_secs
    }

    /// Record that the first token has arrived. Returns the TTFT duration if
    /// this is the first call (subsequent calls return `None`).
    pub fn record_first_token(&self) -> Option<Duration> {
        let mut inner = self.inner.lock().ok()?;
        if inner.first_token_at.is_some() {
            return None;
        }
        let started_at = inner.started_at?;
        let now = Instant::now();
        inner.first_token_at = Some(now);
        Some(now.duration_since(started_at))
    }

    /// Record that the first complete message has arrived. Returns the TTFM
    /// duration if this is the first call.
    pub fn record_first_message(&self) -> Option<Duration> {
        let mut inner = self.inner.lock().ok()?;
        if inner.first_message_at.is_some() {
            return None;
        }
        let started_at = inner.started_at?;
        let now = Instant::now();
        inner.first_message_at = Some(now);
        Some(now.duration_since(started_at))
    }

    /// Get TTFT in milliseconds (if recorded).
    pub fn ttft_ms(&self) -> Option<i64> {
        let inner = self.inner.lock().ok()?;
        let started = inner.started_at?;
        let first_token = inner.first_token_at?;
        let duration = first_token.duration_since(started);
        Some(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
    }

    /// Get TTFM in milliseconds (if recorded).
    pub fn ttfm_ms(&self) -> Option<i64> {
        let inner = self.inner.lock().ok()?;
        let started = inner.started_at?;
        let first_message = inner.first_message_at?;
        let duration = first_message.duration_since(started);
        Some(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
    }

    /// Get the total turn duration so far in milliseconds.
    pub fn elapsed_ms(&self) -> Option<i64> {
        let inner = self.inner.lock().ok()?;
        let started = inner.started_at?;
        Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX))
    }

    /// Get completed-at unix seconds and duration in milliseconds.
    pub fn completed_at_and_duration_ms(&self) -> (Option<i64>, Option<i64>) {
        let completed_at = Some(now_unix_timestamp_ms() / 1000);
        let duration_ms = self.inner.lock().ok().and_then(|inner| {
            inner
                .started_at
                .map(|s| i64::try_from(s.elapsed().as_millis()).unwrap_or(i64::MAX))
        });
        (completed_at, duration_ms)
    }
}

/// Summary of timing metrics for a completed turn.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TurnTimingSummary {
    /// Time to first token in milliseconds.
    pub ttft_ms: Option<i64>,
    /// Time to first message in milliseconds.
    pub ttfm_ms: Option<i64>,
    /// Total turn duration in milliseconds.
    pub total_ms: Option<i64>,
    /// Turn start time (unix seconds).
    pub started_at_unix_secs: Option<i64>,
}

impl TurnTimingSummary {
    /// Build a summary from the current timing state.
    pub fn from_state(state: &TurnTimingState) -> Self {
        Self {
            ttft_ms: state.ttft_ms(),
            ttfm_ms: state.ttfm_ms(),
            total_ms: state.elapsed_ms(),
            started_at_unix_secs: state.started_at_unix_secs(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_unix_timestamp_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn ttft_recorded_only_once() {
        let state = TurnTimingState::default();
        state.mark_turn_started(Instant::now());
        sleep(Duration::from_millis(1));

        let first = state.record_first_token();
        assert!(first.is_some());
        assert!(first.unwrap().as_millis() >= 1);

        // Second call should return None.
        let second = state.record_first_token();
        assert!(second.is_none());
    }

    #[test]
    fn ttfm_recorded_only_once() {
        let state = TurnTimingState::default();
        state.mark_turn_started(Instant::now());
        sleep(Duration::from_millis(1));

        let first = state.record_first_message();
        assert!(first.is_some());

        let second = state.record_first_message();
        assert!(second.is_none());
    }

    #[test]
    fn no_ttft_before_start() {
        let state = TurnTimingState::default();
        assert!(state.record_first_token().is_none());
        assert!(state.ttft_ms().is_none());
    }

    #[test]
    fn summary_from_state() {
        let state = TurnTimingState::default();
        state.mark_turn_started(Instant::now());
        sleep(Duration::from_millis(2));
        state.record_first_token();
        state.record_first_message();

        let summary = TurnTimingSummary::from_state(&state);
        assert!(summary.ttft_ms.is_some());
        assert!(summary.ttfm_ms.is_some());
        assert!(summary.started_at_unix_secs.is_some());
    }
}
