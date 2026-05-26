//! Compaction hooks: extensible pre/post-compact lifecycle callbacks.
//!
//! Aligned with Codex's hook runtime for compaction:
//! - `PreCompactHookOutcome`: whether to proceed with or abort compaction
//! - `PostCompactHookOutcome`: whether to proceed or stop after compaction
//! - `CompactionHook` trait: async callbacks for pre/post compaction phases
//!
//! The hook system allows external code to:
//! - Intercept compaction before it begins (e.g., to save state, check conditions)
//! - React after compaction completes (e.g., invalidate caches, notify clients)
//! - Abort compaction if conditions are not met

use crate::compact::{CompactionPhase, CompactionReason, CompactionTrigger};

// ---------------------------------------------------------------------------
// Hook outcomes (matching Codex's PreCompactHookOutcome / PostCompactHookOutcome)
// ---------------------------------------------------------------------------

/// Outcome of a pre-compact hook invocation.
///
/// Determines whether the compaction should proceed or be aborted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreCompactOutcome {
    /// Compaction should proceed normally.
    Continue,
    /// Compaction should be stopped. The optional reason is logged.
    Stopped { reason: Option<String> },
}

/// Outcome of a post-compact hook invocation.
///
/// Determines whether post-compaction processing should continue or stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostCompactOutcome {
    /// Post-compaction processing continues normally.
    Continue,
    /// Post-compaction processing should stop (e.g., turn is aborted).
    Stopped,
}

// ---------------------------------------------------------------------------
// Compaction context (passed to hooks)
// ---------------------------------------------------------------------------

/// Context provided to compaction hooks with information about the compaction.
#[derive(Debug, Clone)]
pub struct CompactionContext {
    /// What triggered this compaction (Auto / Manual).
    pub trigger: CompactionTrigger,
    /// Why the compaction was triggered.
    pub reason: CompactionReason,
    /// Phase of the turn when compaction occurs.
    pub phase: CompactionPhase,
    /// Current active context token count before compaction.
    pub active_context_tokens: u64,
    /// Session key for identification.
    pub session_key: String,
    /// Compaction count before this operation.
    pub compaction_count: u32,
}

/// Result provided to post-compact hooks with information about the outcome.
#[derive(Debug, Clone)]
pub struct CompactionHookResult {
    /// Token count before compaction.
    pub pre_tokens: u32,
    /// Token count after compaction.
    pub post_tokens: u32,
    /// Tokens saved by compaction.
    pub tokens_saved: u32,
    /// Duration of the compaction in milliseconds.
    pub duration_ms: u64,
    /// Whether compaction was successful.
    pub success: bool,
}

// ---------------------------------------------------------------------------
// CompactionHook trait
// ---------------------------------------------------------------------------

/// Trait for implementing compaction lifecycle hooks.
///
/// Implementors can intercept the compaction process at two points:
/// - Before compaction starts (`pre_compact`)
/// - After compaction completes (`post_compact`)
///
/// Hooks are executed in registration order. If any pre-compact hook returns
/// `Stopped`, subsequent hooks are skipped and compaction is aborted.
#[async_trait::async_trait]
pub trait CompactionHook: Send + Sync {
    /// Called before compaction begins.
    ///
    /// Return `PreCompactOutcome::Continue` to allow compaction to proceed,
    /// or `PreCompactOutcome::Stopped` to abort it.
    async fn pre_compact(&self, context: &CompactionContext) -> PreCompactOutcome;

    /// Called after compaction completes (successfully or not).
    ///
    /// Return `PostCompactOutcome::Continue` to allow normal post-compaction
    /// processing, or `PostCompactOutcome::Stopped` to signal abort.
    async fn post_compact(&self, result: &CompactionHookResult) -> PostCompactOutcome;
}

// ---------------------------------------------------------------------------
// Hook registry
// ---------------------------------------------------------------------------

/// Registry holding compaction hooks that are invoked during the compaction lifecycle.
#[derive(Default)]
pub struct CompactionHookRegistry {
    hooks: Vec<Box<dyn CompactionHook>>,
}

impl CompactionHookRegistry {
    /// Create an empty hook registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a new compaction hook.
    pub fn register(&mut self, hook: Box<dyn CompactionHook>) {
        self.hooks.push(hook);
    }

    /// Returns the number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Returns true if no hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Run all pre-compact hooks in order.
    ///
    /// If any hook returns `Stopped`, execution halts and the stop outcome is returned.
    pub async fn run_pre_compact(&self, context: &CompactionContext) -> PreCompactOutcome {
        for hook in &self.hooks {
            let outcome = hook.pre_compact(context).await;
            if matches!(outcome, PreCompactOutcome::Stopped { .. }) {
                return outcome;
            }
        }
        PreCompactOutcome::Continue
    }

    /// Run all post-compact hooks in order.
    ///
    /// If any hook returns `Stopped`, execution halts and `Stopped` is returned.
    pub async fn run_post_compact(&self, result: &CompactionHookResult) -> PostCompactOutcome {
        for hook in &self.hooks {
            let outcome = hook.post_compact(result).await;
            if matches!(outcome, PostCompactOutcome::Stopped) {
                return outcome;
            }
        }
        PostCompactOutcome::Continue
    }
}

impl std::fmt::Debug for CompactionHookRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactionHookRegistry")
            .field("hook_count", &self.hooks.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Compaction analytics event
// ---------------------------------------------------------------------------

/// Structured analytics event emitted for each compaction attempt.
///
/// Mirrors Codex's `CodexCompactionEvent` for observability.
#[derive(Debug, Clone)]
pub struct CompactionAnalytics {
    /// What triggered the compaction.
    pub trigger: CompactionTrigger,
    /// Why the compaction was triggered.
    pub reason: CompactionReason,
    /// Phase of the turn when compaction occurred.
    pub phase: CompactionPhase,
    /// Active context tokens before compaction.
    pub active_context_tokens_before: u64,
    /// Active context tokens after compaction (None if compaction failed/aborted).
    pub active_context_tokens_after: Option<u64>,
    /// Wall-clock duration of the compaction in milliseconds.
    pub duration_ms: u64,
    /// Implementation strategy used (always Memento for local compaction).
    pub implementation: CompactionImplementation,
    /// Final status of the compaction attempt.
    pub status: CompactionStatus,
    /// Error message if compaction failed.
    pub error: Option<String>,
    /// Session key for correlation.
    pub session_key: String,
    /// Window ordinal at the time of compaction.
    pub window_ordinal: u64,
}

/// Implementation strategy for compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionImplementation {
    /// Local summarization via model call (Memento strategy).
    Local,
}

/// Final status of a compaction attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStatus {
    /// Compaction completed successfully.
    Completed,
    /// Compaction was interrupted (hook stopped, user cancelled).
    Interrupted,
    /// Compaction failed with an error.
    Failed,
}

// ---------------------------------------------------------------------------
// Reference context item tracking
// ---------------------------------------------------------------------------

/// Tracks the identity and content hash of a context fragment.
///
/// Used to detect whether context items have changed since last injection,
/// avoiding unnecessary full rebuilds of initial context on each turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFragmentId {
    /// Unique identifier for this context fragment.
    pub id: String,
    /// Content hash (used for change detection).
    pub content_hash: u64,
}

impl ContextFragmentId {
    /// Create a new fragment ID with a computed hash.
    pub fn new(id: impl Into<String>, content: &str) -> Self {
        Self {
            id: id.into(),
            content_hash: Self::compute_hash(content),
        }
    }

    /// Check if the content has changed by comparing hashes.
    pub fn has_content_changed(&self, new_content: &str) -> bool {
        self.content_hash != Self::compute_hash(new_content)
    }

    fn compute_hash(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }
}

/// Tracks the last-injected context state for incremental updates.
///
/// After compaction or at the start of a window, we track what context was
/// injected so we can detect changes and only re-inject when necessary.
#[derive(Debug, Clone, Default)]
pub struct ReferenceContextState {
    /// Fragments that were injected in the last context build.
    fragments: Vec<ContextFragmentId>,
    /// Generation counter — incremented when context is rebuilt.
    generation: u64,
}

impl ReferenceContextState {
    /// Create a new empty reference context state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the current set of context fragments.
    pub fn update_fragments(&mut self, fragments: Vec<ContextFragmentId>) {
        self.fragments = fragments;
        self.generation = self.generation.saturating_add(1);
    }

    /// Check if any fragment has changed compared to new content.
    ///
    /// Returns true if the fragment set differs in count or any individual
    /// fragment has a different content hash.
    pub fn has_context_changed(&self, new_fragments: &[ContextFragmentId]) -> bool {
        if self.fragments.len() != new_fragments.len() {
            return true;
        }
        self.fragments
            .iter()
            .zip(new_fragments.iter())
            .any(|(old, new)| old.id != new.id || old.content_hash != new.content_hash)
    }

    /// Get the current generation number.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Advance the generation counter (e.g., after compaction invalidates cache).
    pub fn advance_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }

    /// Clear all tracked fragments and advance generation.
    pub fn invalidate(&mut self) {
        self.fragments.clear();
        self.advance_generation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- PreCompactOutcome / PostCompactOutcome tests --

    #[test]
    fn test_pre_compact_outcome_variants() {
        let continue_outcome = PreCompactOutcome::Continue;
        assert_eq!(continue_outcome, PreCompactOutcome::Continue);

        let stopped_outcome = PreCompactOutcome::Stopped {
            reason: Some("test reason".to_string()),
        };
        if let PreCompactOutcome::Stopped { reason } = stopped_outcome {
            assert_eq!(reason, Some("test reason".to_string()));
        } else {
            panic!("expected Stopped variant");
        }
    }

    #[test]
    fn test_post_compact_outcome_variants() {
        assert_eq!(PostCompactOutcome::Continue, PostCompactOutcome::Continue);
        assert_eq!(PostCompactOutcome::Stopped, PostCompactOutcome::Stopped);
    }

    // -- CompactionHookRegistry tests --

    struct NoopHook;

    #[async_trait::async_trait]
    impl CompactionHook for NoopHook {
        async fn pre_compact(&self, _context: &CompactionContext) -> PreCompactOutcome {
            PreCompactOutcome::Continue
        }
        async fn post_compact(&self, _result: &CompactionHookResult) -> PostCompactOutcome {
            PostCompactOutcome::Continue
        }
    }

    struct BlockingHook;

    #[async_trait::async_trait]
    impl CompactionHook for BlockingHook {
        async fn pre_compact(&self, _context: &CompactionContext) -> PreCompactOutcome {
            PreCompactOutcome::Stopped {
                reason: Some("blocked by policy".to_string()),
            }
        }
        async fn post_compact(&self, _result: &CompactionHookResult) -> PostCompactOutcome {
            PostCompactOutcome::Stopped
        }
    }

    fn test_context() -> CompactionContext {
        CompactionContext {
            trigger: CompactionTrigger::Auto,
            reason: CompactionReason::ContextLimit,
            phase: CompactionPhase::PreTurn,
            active_context_tokens: 5000,
            session_key: "test-session".to_string(),
            compaction_count: 0,
        }
    }

    fn test_result() -> CompactionHookResult {
        CompactionHookResult {
            pre_tokens: 5000,
            post_tokens: 2000,
            tokens_saved: 3000,
            duration_ms: 1500,
            success: true,
        }
    }

    #[tokio::test]
    async fn test_empty_registry_returns_continue() {
        let registry = CompactionHookRegistry::new();
        assert!(registry.is_empty());
        let outcome = registry.run_pre_compact(&test_context()).await;
        assert_eq!(outcome, PreCompactOutcome::Continue);
        let outcome = registry.run_post_compact(&test_result()).await;
        assert_eq!(outcome, PostCompactOutcome::Continue);
    }

    #[tokio::test]
    async fn test_noop_hook_allows_compaction() {
        let mut registry = CompactionHookRegistry::new();
        registry.register(Box::new(NoopHook));
        assert_eq!(registry.len(), 1);

        let outcome = registry.run_pre_compact(&test_context()).await;
        assert_eq!(outcome, PreCompactOutcome::Continue);
    }

    #[tokio::test]
    async fn test_blocking_hook_stops_compaction() {
        let mut registry = CompactionHookRegistry::new();
        registry.register(Box::new(BlockingHook));

        let outcome = registry.run_pre_compact(&test_context()).await;
        assert!(matches!(outcome, PreCompactOutcome::Stopped { .. }));
    }

    #[tokio::test]
    async fn test_multiple_hooks_first_blocker_wins() {
        let mut registry = CompactionHookRegistry::new();
        registry.register(Box::new(NoopHook));
        registry.register(Box::new(BlockingHook));
        registry.register(Box::new(NoopHook));

        let outcome = registry.run_pre_compact(&test_context()).await;
        if let PreCompactOutcome::Stopped { reason } = outcome {
            assert_eq!(reason, Some("blocked by policy".to_string()));
        } else {
            panic!("expected Stopped outcome");
        }
    }

    // -- ContextFragmentId tests --

    #[test]
    fn test_context_fragment_id_same_content() {
        let frag = ContextFragmentId::new("agents-md", "# Instructions\nDo this.");
        assert!(!frag.has_content_changed("# Instructions\nDo this."));
    }

    #[test]
    fn test_context_fragment_id_different_content() {
        let frag = ContextFragmentId::new("agents-md", "# Instructions\nDo this.");
        assert!(frag.has_content_changed("# Instructions\nDo that."));
    }

    // -- ReferenceContextState tests --

    #[test]
    fn test_reference_context_state_empty_no_change() {
        let state = ReferenceContextState::new();
        assert!(!state.has_context_changed(&[]));
    }

    #[test]
    fn test_reference_context_state_detects_addition() {
        let state = ReferenceContextState::new();
        let new_frags = vec![ContextFragmentId::new("a", "content")];
        assert!(state.has_context_changed(&new_frags));
    }

    #[test]
    fn test_reference_context_state_detects_content_change() {
        let mut state = ReferenceContextState::new();
        let frags = vec![ContextFragmentId::new("a", "old content")];
        state.update_fragments(frags);

        let new_frags = vec![ContextFragmentId::new("a", "new content")];
        assert!(state.has_context_changed(&new_frags));
    }

    #[test]
    fn test_reference_context_state_no_change() {
        let mut state = ReferenceContextState::new();
        let frags = vec![ContextFragmentId::new("a", "same")];
        state.update_fragments(frags.clone());
        assert!(!state.has_context_changed(&frags));
    }

    #[test]
    fn test_reference_context_state_invalidate() {
        let mut state = ReferenceContextState::new();
        state.update_fragments(vec![ContextFragmentId::new("a", "content")]);
        let gen_before = state.generation();
        state.invalidate();
        assert!(state.generation() > gen_before);
        assert!(!state.has_context_changed(&[]));
    }

    #[test]
    fn test_compaction_analytics_debug() {
        let analytics = CompactionAnalytics {
            trigger: CompactionTrigger::Auto,
            reason: CompactionReason::ContextLimit,
            phase: CompactionPhase::MidTurn,
            active_context_tokens_before: 10000,
            active_context_tokens_after: Some(3000),
            duration_ms: 2500,
            implementation: CompactionImplementation::Local,
            status: CompactionStatus::Completed,
            error: None,
            session_key: "sess-1".to_string(),
            window_ordinal: 2,
        };
        // Ensure Debug is implemented
        let debug_str = format!("{analytics:?}");
        assert!(debug_str.contains("CompactionAnalytics"));
        assert!(debug_str.contains("Auto"));
    }
}
