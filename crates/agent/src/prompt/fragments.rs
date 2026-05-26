//! Context Fragment registration and assembly system.
//!
//! Aligned with Codex's `core/src/context/fragment.rs` architecture.
//!
//! Each fragment represents a discrete piece of context injected into model input.
//! Fragments are categorized by [`PromptSlot`] which determines how they are
//! assembled into the final message sequence:
//!
//! - **DeveloperPolicy**: Aggregated into a single developer message (permissions, sandbox).
//! - **DeveloperCapabilities**: Aggregated with DeveloperPolicy into developer message
//!   (skills, tools descriptions).
//! - **ContextualUser**: Aggregated into a single user message (environment, user instructions).
//! - **SeparateDeveloper**: Each fragment becomes its own developer message (isolation needs).
//! - **GoalContext**: Injected as a user message with goal steering prompts.
//!
//! # Example
//!
//! ```rust,ignore
//! use bifrost_agent::prompt::fragments::{ContextFragment, FragmentRegistry, PromptSlot};
//!
//! let mut registry = FragmentRegistry::new();
//! registry.register(ContextFragment::new(
//!     "permissions",
//!     PromptSlot::DeveloperPolicy,
//!     100,
//!     "You have full access to the filesystem.",
//! ));
//! registry.register(ContextFragment::new(
//!     "environment",
//!     PromptSlot::ContextualUser,
//!     90,
//!     "<environment_context>\n  <os>macos</os>\n</environment_context>",
//! ));
//!
//! let messages = registry.assemble();
//! ```

use std::marker::PhantomData;

use crate::types::ChatMessage;

// ---------------------------------------------------------------------------
// PromptSlot
// ---------------------------------------------------------------------------

/// Determines where a fragment is placed in the assembled prompt message sequence.
///
/// Modeled after Codex's fragment role system:
/// - Developer-role fragments are grouped by policy vs. capabilities.
/// - Contextual user fragments carry runtime environment info.
/// - Separate developer fragments get their own isolated messages.
/// - Goal context is injected as hidden user context for goal steering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptSlot {
    /// Permissions, sandbox mode, approval policy.
    /// Aggregated into a single developer message.
    DeveloperPolicy,

    /// Skills, tools, developer instructions.
    /// Aggregated with DeveloperPolicy into the developer message.
    DeveloperCapabilities,

    /// Environment context, user instructions.
    /// Aggregated into a single user message.
    ContextualUser,

    /// Fragments that must remain isolated as separate developer messages.
    SeparateDeveloper,

    /// Goal steering prompts (continuation, budget limit, objective updated).
    /// Injected as a user message with XML markers.
    GoalContext,
}

// ---------------------------------------------------------------------------
// ContextFragment
// ---------------------------------------------------------------------------

/// A single context fragment with metadata for assembly.
///
/// Each fragment carries:
/// - A unique identifier for deduplication and debugging.
/// - A slot classification for assembly routing.
/// - A priority for ordering within a slot (higher = earlier).
/// - The rendered text body.
/// - Optional XML markers for recognition in conversation history.
#[derive(Debug, Clone)]
pub struct ContextFragment {
    /// Unique identifier for this fragment (e.g., "permissions", "environment").
    pub id: &'static str,

    /// Which slot this fragment belongs to.
    pub slot: PromptSlot,

    /// Priority for ordering within a slot (higher = rendered earlier).
    pub priority: u32,

    /// The rendered text body of this fragment.
    pub body: String,

    /// Optional start marker for history recognition.
    pub start_marker: Option<&'static str>,

    /// Optional end marker for history recognition.
    pub end_marker: Option<&'static str>,
}

impl ContextFragment {
    /// Create a new context fragment.
    pub fn new(id: &'static str, slot: PromptSlot, priority: u32, body: impl Into<String>) -> Self {
        Self {
            id,
            slot,
            priority,
            body: body.into(),
            start_marker: None,
            end_marker: None,
        }
    }

    /// Set XML markers for history recognition.
    pub fn with_markers(mut self, start: &'static str, end: &'static str) -> Self {
        self.start_marker = Some(start);
        self.end_marker = Some(end);
        self
    }

    /// Render the fragment with markers (if present).
    pub fn render(&self) -> String {
        match (self.start_marker, self.end_marker) {
            (Some(start), Some(end)) => format!("{start}\n{}\n{end}", self.body),
            _ => self.body.clone(),
        }
    }

    /// Check if a text matches this fragment's markers.
    pub fn matches_text(&self, text: &str) -> bool {
        let (Some(start), Some(end)) = (self.start_marker, self.end_marker) else {
            return false;
        };
        matches_marked_text(start, end, text)
    }
}

// ---------------------------------------------------------------------------
// FragmentRegistry
// ---------------------------------------------------------------------------

/// Registry for context fragments that handles assembly into chat messages.
///
/// Fragments are registered with their slot classification and priority.
/// During assembly, fragments are grouped by slot and ordered by priority
/// (descending) within each group, then aggregated into the appropriate
/// message roles.
#[derive(Debug, Clone, Default)]
pub struct FragmentRegistry {
    fragments: Vec<ContextFragment>,
}

impl FragmentRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fragment. Replaces any existing fragment with the same id.
    pub fn register(&mut self, fragment: ContextFragment) {
        // Remove existing fragment with same id
        self.fragments.retain(|f| f.id != fragment.id);
        self.fragments.push(fragment);
    }

    /// Remove a fragment by id.
    pub fn remove(&mut self, id: &str) {
        self.fragments.retain(|f| f.id != id);
    }

    /// Check if a fragment with the given id is registered.
    pub fn has(&self, id: &str) -> bool {
        self.fragments.iter().any(|f| f.id == id)
    }

    /// Get all registered fragments.
    pub fn fragments(&self) -> &[ContextFragment] {
        &self.fragments
    }

    /// Assemble registered fragments into a sequence of chat messages.
    ///
    /// Assembly rules:
    /// 1. DeveloperPolicy + DeveloperCapabilities → single developer message
    /// 2. ContextualUser → single user message
    /// 3. SeparateDeveloper → each fragment becomes its own developer message
    /// 4. GoalContext → each fragment becomes its own user message
    ///
    /// Within each group, fragments are ordered by priority (descending).
    pub fn assemble(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // Collect and sort fragments by slot
        let mut developer_policy: Vec<&ContextFragment> = self
            .fragments
            .iter()
            .filter(|f| f.slot == PromptSlot::DeveloperPolicy)
            .collect();
        developer_policy.sort_by_key(|item| std::cmp::Reverse(item.priority));

        let mut developer_capabilities: Vec<&ContextFragment> = self
            .fragments
            .iter()
            .filter(|f| f.slot == PromptSlot::DeveloperCapabilities)
            .collect();
        developer_capabilities.sort_by_key(|item| std::cmp::Reverse(item.priority));

        let mut contextual_user: Vec<&ContextFragment> = self
            .fragments
            .iter()
            .filter(|f| f.slot == PromptSlot::ContextualUser)
            .collect();
        contextual_user.sort_by_key(|item| std::cmp::Reverse(item.priority));

        let mut separate_developer: Vec<&ContextFragment> = self
            .fragments
            .iter()
            .filter(|f| f.slot == PromptSlot::SeparateDeveloper)
            .collect();
        separate_developer.sort_by_key(|item| std::cmp::Reverse(item.priority));

        let mut goal_context: Vec<&ContextFragment> = self
            .fragments
            .iter()
            .filter(|f| f.slot == PromptSlot::GoalContext)
            .collect();
        goal_context.sort_by_key(|item| std::cmp::Reverse(item.priority));

        // 1. Aggregate developer sections into one developer message
        let developer_parts: Vec<String> = developer_policy
            .iter()
            .chain(developer_capabilities.iter())
            .map(|f| f.render())
            .filter(|s| !s.is_empty())
            .collect();
        if !developer_parts.is_empty() {
            messages.push(ChatMessage::developer(&developer_parts.join("\n\n")));
        }

        // 2. Aggregate contextual user sections into one user message
        let user_parts: Vec<String> = contextual_user
            .iter()
            .map(|f| f.render())
            .filter(|s| !s.is_empty())
            .collect();
        if !user_parts.is_empty() {
            messages.push(ChatMessage::user(&user_parts.join("\n\n")));
        }

        // 3. Each separate developer fragment gets its own message
        for fragment in &separate_developer {
            let rendered = fragment.render();
            if !rendered.is_empty() {
                messages.push(ChatMessage::developer(&rendered));
            }
        }

        // 4. Each goal context fragment gets its own user message
        for fragment in &goal_context {
            let rendered = fragment.render();
            if !rendered.is_empty() {
                messages.push(ChatMessage::user(&rendered));
            }
        }

        messages
    }

    /// Check if a text matches any registered fragment's markers.
    ///
    /// Used to identify injected context in conversation history,
    /// matching Codex's `is_contextual_user_fragment` pattern.
    pub fn matches_any(&self, text: &str) -> bool {
        self.fragments.iter().any(|f| f.matches_text(text))
    }
}

// ---------------------------------------------------------------------------
// FragmentRegistration trait (Codex parity)
// ---------------------------------------------------------------------------

/// Type-erased registration for contextual fragments.
///
/// Implementations allow context filtering code to recognize injected
/// fragments without constructing the concrete context payload.
/// Matches Codex's `FragmentRegistration` trait.
pub trait FragmentRegistration: Sync + Send {
    /// Check if a text matches this fragment type's markers.
    fn matches_text(&self, text: &str) -> bool;
}

/// Proxy for type-safe fragment registration.
///
/// Wraps a `ContextualFragment` implementor to provide type-erased matching.
/// Matches Codex's `FragmentRegistrationProxy<T>`.
pub struct FragmentRegistrationProxy<T> {
    _marker: PhantomData<fn() -> T>,
}

impl<T> FragmentRegistrationProxy<T> {
    /// Create a new registration proxy.
    pub const fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T> Default for FragmentRegistrationProxy<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ContextualFragment> FragmentRegistration for FragmentRegistrationProxy<T> {
    fn matches_text(&self, text: &str) -> bool {
        T::matches_text(text)
    }
}

// ---------------------------------------------------------------------------
// ContextualFragment trait (Codex's ContextualUserFragment equivalent)
// ---------------------------------------------------------------------------

/// Context payload that is injected as a message fragment.
///
/// Implementations own the response role and provide the exact fragment body.
/// Marked fragments provide start/end markers for history recognition.
/// Matches Codex's `ContextualUserFragment` trait.
pub trait ContextualFragment: Send + Sync {
    /// The role for messages containing this fragment ("developer", "user", "system").
    fn role() -> &'static str
    where
        Self: Sized;

    /// Instance-level markers (may differ from type-level if dynamic).
    fn markers(&self) -> (&'static str, &'static str);

    /// The body content of this fragment.
    fn body(&self) -> String;

    /// Type-level markers for static matching.
    fn type_markers() -> (&'static str, &'static str)
    where
        Self: Sized;

    /// Check if text matches this fragment type's markers.
    fn matches_text(text: &str) -> bool
    where
        Self: Sized,
    {
        let (start, end) = Self::type_markers();
        matches_marked_text(start, end, text)
    }

    /// Render the fragment with its markers.
    fn render(&self) -> String {
        let (start, end) = self.markers();
        let body = self.body();
        if start.is_empty() && end.is_empty() {
            return body;
        }
        format!("{start}{body}{end}")
    }

    /// Convert into a ChatMessage with the appropriate role.
    fn into_message(self) -> ChatMessage
    where
        Self: Sized,
    {
        let role = Self::role();
        let text = self.render();
        match role {
            "developer" => ChatMessage::developer(&text),
            "user" => ChatMessage::user(&text),
            _ => ChatMessage::system(&text),
        }
    }
}

// ---------------------------------------------------------------------------
// GoalContext fragment
// ---------------------------------------------------------------------------

/// Hidden user-context fragment for runtime-owned goal steering prompts.
///
/// Matches Codex's `GoalContext` struct in `context/goal_context.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalContext {
    prompt: String,
}

impl GoalContext {
    /// Creates goal context around an already-rendered steering prompt.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
        }
    }

    /// Convert into a ChatMessage for injection into the conversation.
    pub fn into_chat_message(self) -> ChatMessage {
        ChatMessage::user(&self.render())
    }
}

impl ContextualFragment for GoalContext {
    fn role() -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<goal_context>", "</goal_context>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.prompt)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a text matches start/end markers (case-insensitive, trimmed).
///
/// Matches Codex's `matches_marked_text` function.
fn matches_marked_text(start_marker: &str, end_marker: &str, text: &str) -> bool {
    if start_marker.is_empty() || end_marker.is_empty() {
        return false;
    }

    let trimmed = text.trim_start();
    let starts_with_marker = trimmed
        .get(..start_marker.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(start_marker));

    let trimmed = trimmed.trim_end();
    let ends_with_marker = trimmed
        .get(trimmed.len().saturating_sub(end_marker.len())..)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(end_marker));

    starts_with_marker && ends_with_marker
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fragment_creation_and_render() {
        let fragment =
            ContextFragment::new("test", PromptSlot::DeveloperPolicy, 100, "Hello world");
        assert_eq!(fragment.render(), "Hello world");

        let fragment_with_markers = ContextFragment::new(
            "test_marked",
            PromptSlot::ContextualUser,
            90,
            "Content here",
        )
        .with_markers("<test>", "</test>");
        assert_eq!(
            fragment_with_markers.render(),
            "<test>\nContent here\n</test>"
        );
    }

    #[test]
    fn test_fragment_matches_text() {
        let fragment =
            ContextFragment::new("env", PromptSlot::ContextualUser, 100, "<cwd>/tmp</cwd>")
                .with_markers("<environment_context>", "</environment_context>");

        assert!(fragment
            .matches_text("<environment_context>\n  <cwd>/tmp</cwd>\n</environment_context>"));
        assert!(!fragment.matches_text("random text"));
    }

    #[test]
    fn test_registry_assemble_developer_message() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "permissions",
            PromptSlot::DeveloperPolicy,
            100,
            "Full access granted.",
        ));
        registry.register(ContextFragment::new(
            "skills",
            PromptSlot::DeveloperCapabilities,
            80,
            "Skills: git, docker",
        ));

        let messages = registry.assemble();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "developer");
        let content = messages[0].content.as_deref().unwrap();
        assert!(content.contains("Full access granted."));
        assert!(content.contains("Skills: git, docker"));
    }

    #[test]
    fn test_registry_assemble_user_message() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "environment",
            PromptSlot::ContextualUser,
            100,
            "<environment_context>\n  <os>macos</os>\n</environment_context>",
        ));
        registry.register(ContextFragment::new(
            "user_instructions",
            PromptSlot::ContextualUser,
            90,
            "Follow code style.",
        ));

        let messages = registry.assemble();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        let content = messages[0].content.as_deref().unwrap();
        assert!(content.contains("<environment_context>"));
        assert!(content.contains("Follow code style."));
    }

    #[test]
    fn test_registry_assemble_separate_developer() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "isolated_1",
            PromptSlot::SeparateDeveloper,
            100,
            "Isolated fragment 1",
        ));
        registry.register(ContextFragment::new(
            "isolated_2",
            PromptSlot::SeparateDeveloper,
            50,
            "Isolated fragment 2",
        ));

        let messages = registry.assemble();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "developer");
        assert_eq!(messages[0].content.as_deref(), Some("Isolated fragment 1"));
        assert_eq!(messages[1].role, "developer");
        assert_eq!(messages[1].content.as_deref(), Some("Isolated fragment 2"));
    }

    #[test]
    fn test_registry_assemble_goal_context() {
        let mut registry = FragmentRegistry::new();
        registry.register(
            ContextFragment::new("goal", PromptSlot::GoalContext, 100, "Continue working.")
                .with_markers("<goal_context>", "</goal_context>"),
        );

        let messages = registry.assemble();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert!(messages[0]
            .content
            .as_deref()
            .unwrap()
            .contains("<goal_context>"));
    }

    #[test]
    fn test_registry_full_assembly_order() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "env",
            PromptSlot::ContextualUser,
            100,
            "env info",
        ));
        registry.register(ContextFragment::new(
            "perms",
            PromptSlot::DeveloperPolicy,
            100,
            "permissions",
        ));
        registry.register(ContextFragment::new(
            "goal",
            PromptSlot::GoalContext,
            100,
            "goal context",
        ));
        registry.register(ContextFragment::new(
            "isolated",
            PromptSlot::SeparateDeveloper,
            100,
            "isolated",
        ));

        let messages = registry.assemble();
        // developer (perms) -> user (env) -> separate developer -> goal user
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "developer");
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[2].role, "developer");
        assert_eq!(messages[3].role, "user");
    }

    #[test]
    fn test_registry_deduplication() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "same_id",
            PromptSlot::DeveloperPolicy,
            100,
            "first version",
        ));
        registry.register(ContextFragment::new(
            "same_id",
            PromptSlot::DeveloperPolicy,
            100,
            "updated version",
        ));

        assert_eq!(registry.fragments().len(), 1);
        assert_eq!(registry.fragments()[0].body, "updated version");
    }

    #[test]
    fn test_registry_remove() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "to_remove",
            PromptSlot::DeveloperPolicy,
            100,
            "will be removed",
        ));
        assert!(registry.has("to_remove"));
        registry.remove("to_remove");
        assert!(!registry.has("to_remove"));
    }

    #[test]
    fn test_matches_marked_text_case_insensitive() {
        assert!(matches_marked_text(
            "<goal_context>",
            "</goal_context>",
            "<GOAL_CONTEXT>\nsome content\n</GOAL_CONTEXT>"
        ));
    }

    #[test]
    fn test_matches_marked_text_with_whitespace() {
        assert!(matches_marked_text(
            "<goal_context>",
            "</goal_context>",
            "  <goal_context>\ncontent\n</goal_context>  "
        ));
    }

    #[test]
    fn test_matches_marked_text_empty_markers() {
        assert!(!matches_marked_text("", "", "any text"));
    }

    #[test]
    fn test_goal_context_fragment() {
        let goal = GoalContext::new("Continue working toward the objective.");
        assert_eq!(GoalContext::role(), "user");
        assert_eq!(
            goal.render(),
            "<goal_context>\nContinue working toward the objective.\n</goal_context>"
        );
    }

    #[test]
    fn test_goal_context_into_message() {
        let goal = GoalContext::new("Test prompt");
        let msg = goal.into_chat_message();
        assert_eq!(msg.role, "user");
        assert!(msg.content.as_deref().unwrap().contains("<goal_context>"));
        assert!(msg.content.as_deref().unwrap().contains("Test prompt"));
    }

    #[test]
    fn test_goal_context_matches_text() {
        assert!(GoalContext::matches_text(
            "<goal_context>\nsome steering\n</goal_context>"
        ));
        assert!(!GoalContext::matches_text("unrelated text"));
    }

    #[test]
    fn test_priority_ordering_within_slot() {
        let mut registry = FragmentRegistry::new();
        registry.register(ContextFragment::new(
            "low_priority",
            PromptSlot::DeveloperPolicy,
            10,
            "low",
        ));
        registry.register(ContextFragment::new(
            "high_priority",
            PromptSlot::DeveloperPolicy,
            100,
            "high",
        ));
        registry.register(ContextFragment::new(
            "mid_priority",
            PromptSlot::DeveloperPolicy,
            50,
            "mid",
        ));

        let messages = registry.assemble();
        let content = messages[0].content.as_deref().unwrap();
        let high_pos = content.find("high").unwrap();
        let mid_pos = content.find("mid").unwrap();
        let low_pos = content.find("low").unwrap();
        assert!(high_pos < mid_pos);
        assert!(mid_pos < low_pos);
    }
}
