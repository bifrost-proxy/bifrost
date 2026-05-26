//! Steer Input mechanism for the Bifrost Agent turn loop.
//!
//! Inspired by Codex's `input_queue` and pending input drain, this module
//! provides a way for external callers (e.g. guide messages, user interjections)
//! to inject messages into a running turn so the model sees them on the next
//! prompt build or immediately within the current streaming response.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// SteerInjectMode
// ---------------------------------------------------------------------------

/// Controls when injected steer input takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerInjectMode {
    /// The input is queued and will be included in the next prompt build
    /// (i.e. the next loop iteration within the same turn).
    NextPromptBuild,

    /// The input is surfaced to the model immediately — useful when streaming
    /// is paused or between tool calls so the model can react mid-turn.
    Immediate,
}

// ---------------------------------------------------------------------------
// SteerInput
// ---------------------------------------------------------------------------

/// A single piece of steered input to inject into the running turn.
#[derive(Debug, Clone)]
pub struct SteerInput {
    /// The content to inject (typically a user or system message body).
    pub content: String,

    /// When this input should take effect.
    pub mode: SteerInjectMode,

    /// Optional label for tracing/debugging.
    pub label: Option<String>,
}

impl SteerInput {
    /// Create a new steer input with `NextPromptBuild` mode.
    pub fn next_prompt(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            mode: SteerInjectMode::NextPromptBuild,
            label: None,
        }
    }

    /// Create a new steer input with `Immediate` mode.
    pub fn immediate(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            mode: SteerInjectMode::Immediate,
            label: None,
        }
    }

    /// Attach a label for diagnostic purposes.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

// ---------------------------------------------------------------------------
// SteerQueue
// ---------------------------------------------------------------------------

/// Thread-safe queue for steer inputs.
///
/// The turn loop drains this queue at appropriate points:
/// - `NextPromptBuild` items are drained before building the next model request.
/// - `Immediate` items are drained between streaming chunks (when the loop
///   checks for pending user input mid-iteration).
#[derive(Debug, Clone, Default)]
pub struct SteerQueue {
    inner: Arc<Mutex<VecDeque<SteerInput>>>,
}

impl SteerQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Push a steer input into the queue.
    pub fn push(&self, input: SteerInput) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.push_back(input);
        }
    }

    /// Drain all queued inputs matching the given mode.
    pub fn drain_by_mode(&self, mode: SteerInjectMode) -> Vec<SteerInput> {
        let mut drained = Vec::new();
        if let Ok(mut queue) = self.inner.lock() {
            let mut remaining = VecDeque::new();
            for item in queue.drain(..) {
                if item.mode == mode {
                    drained.push(item);
                } else {
                    remaining.push_back(item);
                }
            }
            *queue = remaining;
        }
        drained
    }

    /// Returns the number of pending items.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
