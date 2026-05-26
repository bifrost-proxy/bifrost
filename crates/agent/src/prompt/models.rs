//! Model-specific prompt selection and personality variants.
//!
//! Aligned with Codex's multi-model prompt system where different model families
//! (GPT-5.1, GPT-5.2, etc.) receive different base instructions templates and
//! personality configurations.
//!
//! # Architecture
//!
//! The [`ModelMessages`] struct selects the appropriate instructions template
//! and personality variant based on the model slug. This allows the agent to
//! optimize its system prompt for each model's strengths and expectations.
//!
//! Personality variants control tone and style:
//! - **Default**: Concise, direct, and friendly (matches Codex's default).
//! - **Friendly**: More conversational, warm, collaborative.
//! - **Pragmatic**: Terse, action-oriented, minimal commentary.

/// Personality variant for the model's tone and communication style.
///
/// Corresponds to Codex's `personality` configuration that selects between
/// different instruction sets for the same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersonalityVariant {
    /// Concise, direct, and friendly. Standard operating mode.
    #[default]
    Default,
    /// More conversational, warm, and collaborative. Good for pairing sessions.
    Friendly,
    /// Terse, action-oriented, minimal commentary. Good for batch operations.
    Pragmatic,
}

impl PersonalityVariant {
    /// Parse a personality variant from a string slug.
    pub fn from_slug(slug: &str) -> Self {
        match slug.to_lowercase().as_str() {
            "friendly" | "warm" | "collaborative" => Self::Friendly,
            "pragmatic" | "terse" | "minimal" => Self::Pragmatic,
            _ => Self::Default,
        }
    }

    /// Get the personality instruction text for this variant.
    pub fn instructions(&self) -> &'static str {
        match self {
            Self::Default => DEFAULT_PERSONALITY,
            Self::Friendly => FRIENDLY_PERSONALITY,
            Self::Pragmatic => PRAGMATIC_PERSONALITY,
        }
    }
}

/// Model family classification for prompt template selection.
///
/// Different model families may have different capabilities, context windows,
/// and optimal instruction formats. This enum allows the prompt system to
/// select the best template for each family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelFamily {
    /// Default model family (generic instructions).
    #[default]
    Default,
    /// GPT-5.1 family (advanced reasoning, apply_patch native).
    Gpt5_1,
    /// GPT-5.2 family (enhanced coding, stronger tool use).
    Gpt5_2,
    /// Claude family (Anthropic models).
    Claude,
    /// O-series models (reasoning-focused).
    OSeries,
}

impl ModelFamily {
    /// Detect model family from a model slug or identifier.
    ///
    /// Uses prefix matching to classify models into families.
    pub fn from_slug(slug: &str) -> Self {
        let lower = slug.to_lowercase();
        if lower.contains("gpt-5.2") || lower.contains("gpt_5_2") {
            Self::Gpt5_2
        } else if lower.contains("gpt-5.1") || lower.contains("gpt_5_1") {
            Self::Gpt5_1
        } else if lower.contains("claude") {
            Self::Claude
        } else if lower.starts_with("o1") || lower.starts_with("o3") || lower.starts_with("o4") {
            Self::OSeries
        } else {
            Self::Default
        }
    }
}

/// Model-specific message configuration.
///
/// Selects the appropriate instructions template and personality variant
/// based on the model being used. This matches Codex's pattern of loading
/// different prompt files per model (e.g., `gpt_5_1_prompt.md` vs
/// `prompt_with_apply_patch_instructions.md`).
#[derive(Debug, Clone)]
pub struct ModelMessages {
    /// The detected model family.
    pub family: ModelFamily,
    /// The personality variant in use.
    pub personality: PersonalityVariant,
    /// Whether apply_patch format instructions should be included.
    pub include_apply_patch_instructions: bool,
    /// Whether final answer style guidelines should be included.
    pub include_final_answer_guidelines: bool,
}

impl ModelMessages {
    /// Create model messages configuration from a model slug and optional personality.
    pub fn new(model_slug: &str, personality: Option<&str>) -> Self {
        let family = ModelFamily::from_slug(model_slug);
        let personality = personality
            .map(PersonalityVariant::from_slug)
            .unwrap_or_default();

        Self {
            family,
            personality,
            // GPT-5.1+ models have native apply_patch; all models benefit from instructions
            include_apply_patch_instructions: true,
            // GPT-5.1+ models benefit from structured output guidelines
            include_final_answer_guidelines: matches!(
                family,
                ModelFamily::Gpt5_1 | ModelFamily::Gpt5_2
            ),
        }
    }

    /// Get the personality instructions text.
    pub fn personality_instructions(&self) -> &'static str {
        self.personality.instructions()
    }

    /// Get model-family-specific additional instructions (if any).
    pub fn family_instructions(&self) -> Option<&'static str> {
        match self.family {
            ModelFamily::Gpt5_1 | ModelFamily::Gpt5_2 => Some(ADVANCED_MODEL_INSTRUCTIONS),
            ModelFamily::OSeries => Some(O_SERIES_INSTRUCTIONS),
            _ => None,
        }
    }

    /// Get the apply_patch format instructions (if enabled).
    pub fn apply_patch_instructions(&self) -> Option<&'static str> {
        if self.include_apply_patch_instructions {
            Some(APPLY_PATCH_FORMAT_INSTRUCTIONS)
        } else {
            None
        }
    }

    /// Get the final answer style guidelines (if enabled).
    pub fn final_answer_guidelines(&self) -> Option<&'static str> {
        if self.include_final_answer_guidelines {
            Some(FINAL_ANSWER_STYLE_GUIDELINES)
        } else {
            None
        }
    }
}

impl Default for ModelMessages {
    fn default() -> Self {
        Self {
            family: ModelFamily::Default,
            personality: PersonalityVariant::Default,
            include_apply_patch_instructions: true,
            include_final_answer_guidelines: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Personality text constants
// ---------------------------------------------------------------------------

const DEFAULT_PERSONALITY: &str = "\
Your default personality and tone is concise, direct, and friendly. You communicate \
efficiently, always keeping the user clearly informed about ongoing actions without \
unnecessary detail. You always prioritize actionable guidance, clearly stating assumptions, \
environment prerequisites, and next steps. Unless explicitly asked, you avoid excessively \
verbose explanations about your work.";

const FRIENDLY_PERSONALITY: &str = "\
Your personality is warm, collaborative, and encouraging. You communicate like a supportive \
pair-programming partner who celebrates progress, asks thoughtful questions, and explains \
reasoning in an accessible way. You balance being helpful with being concise, and you adapt \
your communication style to match the user's energy and preferences.";

const PRAGMATIC_PERSONALITY: &str = "\
Your personality is terse and action-oriented. You minimize commentary and focus exclusively \
on delivering results. You communicate in short, factual statements. You skip preambles, \
avoid restating the problem, and go straight to implementation. Only speak when you have \
actionable information to share.";

// ---------------------------------------------------------------------------
// Model-family-specific instructions
// ---------------------------------------------------------------------------

const ADVANCED_MODEL_INSTRUCTIONS: &str = "\
## Autonomy and Persistence
Persist until the task is fully handled end-to-end within the current turn whenever feasible: \
do not stop at analysis or partial fixes; carry changes through implementation, verification, \
and a clear explanation of outcomes unless the user explicitly pauses or redirects you.

Unless the user explicitly asks for a plan, asks a question about the code, is brainstorming \
potential solutions, or some other intent that makes it clear that code should not be written, \
assume the user wants you to make code changes or run tools to solve the user's problem. In \
these cases, it's bad to output your proposed solution in a message, you should go ahead and \
actually implement the change. If you encounter challenges or blockers, you should attempt to \
resolve them yourself.";

const O_SERIES_INSTRUCTIONS: &str = "\
## Reasoning-First Approach
You are a reasoning-focused model. Take time to think through problems carefully before acting. \
Break complex tasks into clear logical steps and verify each step before proceeding. When \
uncertain, state your assumptions explicitly and validate them with evidence from the codebase.";

// ---------------------------------------------------------------------------
// apply_patch format instructions
// ---------------------------------------------------------------------------

/// Complete apply_patch tool format syntax description.
///
/// Matches Codex's `prompt_with_apply_patch_instructions.md` format specification.
pub const APPLY_PATCH_FORMAT_INSTRUCTIONS: &str = r#"## `apply_patch`

Use the `apply_patch` tool to edit files. Your patch language is a stripped-down, file-oriented diff format:

```
*** Begin Patch
[ one or more file sections ]
*** End Patch
```

Each file operation starts with one of three headers:

- `*** Add File: <path>` - create a new file. Every following line is a `+` line (the initial contents).
- `*** Delete File: <path>` - remove an existing file. Nothing follows.
- `*** Update File: <path>` - patch an existing file in place (optionally with a rename).

May be immediately followed by `*** Move to: <new path>` if you want to rename the file.
Then one or more "hunks", each introduced by `@@` (optionally followed by a hunk header).

Within a hunk each line starts with:
- ` ` (space) for context lines (unchanged)
- `-` for lines to remove
- `+` for lines to add

Context rules:
- By default, show 3 lines of code immediately above and 3 lines immediately below each change.
- If 3 lines of context is insufficient to uniquely identify the snippet, use the `@@` operator to indicate the class or function:

```
@@ class BaseClass
[3 lines of pre-context]
- [old_code]
+ [new_code]
[3 lines of post-context]
```

- If a code block is repeated, use multiple `@@` statements:

```
@@ class BaseClass
@@   def method():
[3 lines of pre-context]
- [old_code]
+ [new_code]
[3 lines of post-context]
```

The full grammar:
```
Patch     := Begin { FileOp } End
Begin     := "*** Begin Patch" NEWLINE
End       := "*** End Patch" NEWLINE
FileOp    := AddFile | DeleteFile | UpdateFile
AddFile   := "*** Add File: " path NEWLINE { "+" line NEWLINE }
DeleteFile:= "*** Delete File: " path NEWLINE
UpdateFile:= "*** Update File: " path NEWLINE [ MoveTo ] { Hunk }
MoveTo    := "*** Move to: " newPath NEWLINE
Hunk      := "@@" [ header ] NEWLINE { HunkLine } [ "*** End of File" NEWLINE ]
HunkLine  := (" " | "-" | "+") text NEWLINE
```

A full example:

```
*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
*** Move to: src/main.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch
```

Important rules:
- You must include a header with your intended action (Add/Delete/Update)
- You must prefix new lines with `+` even when creating a new file
- File references can only be relative, NEVER ABSOLUTE"#;

// ---------------------------------------------------------------------------
// Final answer style guidelines
// ---------------------------------------------------------------------------

/// Output formatting guidelines for structured final answers.
///
/// Matches Codex's "Final answer structure and style guidelines" section.
pub const FINAL_ANSWER_STYLE_GUIDELINES: &str = r#"## Final answer structure and style guidelines

You are producing plain text that will later be styled by the CLI. Follow these rules exactly. Formatting should make results easy to scan, but not feel mechanical.

**Section Headers**
- Use only when they improve clarity — they are not mandatory for every answer.
- Choose descriptive names that fit the content.
- Keep headers short (1–3 words) and in `**Title Case**`.
- Leave no blank line before the first bullet under a header.

**Bullets**
- Use `-` followed by a space for every bullet.
- Merge related points when possible; avoid a bullet for every trivial detail.
- Keep bullets to one line unless breaking for clarity is unavoidable.
- Group into short lists (4–6 bullets) ordered by importance.

**Monospace**
- Wrap all commands, file paths, env vars, and code identifiers in backticks.
- Apply to inline examples and to bullet keywords if the keyword itself is a literal file/command.

**File References**
- Use inline code to make file paths clickable.
- Each reference should have a standalone path.
- Accepted: absolute, workspace-relative, or bare filename/suffix.
- Line/column (1-based, optional): `:line[:column]` or `#Lline[Ccolumn]`.
- Do not use URIs like `file://`, `vscode://`, or `https://`.

**Structure**
- Place related bullets together; don't mix unrelated concepts in the same section.
- Order sections from general → specific → supporting info.
- Match structure to complexity: multi-part results use headers and grouped bullets; simple results use minimal formatting.

**Tone**
- Keep the voice collaborative and natural, like a coding partner handing off work.
- Be concise and factual — no filler or conversational commentary.
- Use present tense and active voice.
- Keep descriptions self-contained; don't refer to "above" or "below".

**Verbosity**
- Brevity is very important as a default. Be very concise (no more than 10 lines), but relax this for tasks where additional detail is important.
- For casual greetings or one-off conversational messages, respond naturally without section headers or bullet formatting."#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_family_detection() {
        assert_eq!(ModelFamily::from_slug("gpt-5.1-turbo"), ModelFamily::Gpt5_1);
        assert_eq!(
            ModelFamily::from_slug("gpt-5.2-preview"),
            ModelFamily::Gpt5_2
        );
        assert_eq!(ModelFamily::from_slug("claude-3-opus"), ModelFamily::Claude);
        assert_eq!(ModelFamily::from_slug("o1-preview"), ModelFamily::OSeries);
        assert_eq!(ModelFamily::from_slug("o3-mini"), ModelFamily::OSeries);
        assert_eq!(
            ModelFamily::from_slug("unknown-model"),
            ModelFamily::Default
        );
    }

    #[test]
    fn test_personality_variant_parsing() {
        assert_eq!(
            PersonalityVariant::from_slug("friendly"),
            PersonalityVariant::Friendly
        );
        assert_eq!(
            PersonalityVariant::from_slug("warm"),
            PersonalityVariant::Friendly
        );
        assert_eq!(
            PersonalityVariant::from_slug("pragmatic"),
            PersonalityVariant::Pragmatic
        );
        assert_eq!(
            PersonalityVariant::from_slug("terse"),
            PersonalityVariant::Pragmatic
        );
        assert_eq!(
            PersonalityVariant::from_slug("default"),
            PersonalityVariant::Default
        );
        assert_eq!(
            PersonalityVariant::from_slug("unknown"),
            PersonalityVariant::Default
        );
    }

    #[test]
    fn test_model_messages_construction() {
        let messages = ModelMessages::new("gpt-5.1-turbo", Some("friendly"));
        assert_eq!(messages.family, ModelFamily::Gpt5_1);
        assert_eq!(messages.personality, PersonalityVariant::Friendly);
        assert!(messages.include_apply_patch_instructions);
        assert!(messages.include_final_answer_guidelines);
    }

    #[test]
    fn test_model_messages_default_model() {
        let messages = ModelMessages::new("some-unknown-model", None);
        assert_eq!(messages.family, ModelFamily::Default);
        assert_eq!(messages.personality, PersonalityVariant::Default);
        assert!(messages.include_apply_patch_instructions);
        assert!(!messages.include_final_answer_guidelines);
    }

    #[test]
    fn test_personality_instructions_not_empty() {
        assert!(!PersonalityVariant::Default.instructions().is_empty());
        assert!(!PersonalityVariant::Friendly.instructions().is_empty());
        assert!(!PersonalityVariant::Pragmatic.instructions().is_empty());
    }

    #[test]
    fn test_apply_patch_instructions_contain_grammar() {
        assert!(APPLY_PATCH_FORMAT_INSTRUCTIONS.contains("*** Begin Patch"));
        assert!(APPLY_PATCH_FORMAT_INSTRUCTIONS.contains("*** End Patch"));
        assert!(APPLY_PATCH_FORMAT_INSTRUCTIONS.contains("*** Add File:"));
        assert!(APPLY_PATCH_FORMAT_INSTRUCTIONS.contains("*** Delete File:"));
        assert!(APPLY_PATCH_FORMAT_INSTRUCTIONS.contains("*** Update File:"));
        assert!(APPLY_PATCH_FORMAT_INSTRUCTIONS.contains("*** Move to:"));
    }

    #[test]
    fn test_final_answer_guidelines_contain_sections() {
        assert!(FINAL_ANSWER_STYLE_GUIDELINES.contains("Section Headers"));
        assert!(FINAL_ANSWER_STYLE_GUIDELINES.contains("Bullets"));
        assert!(FINAL_ANSWER_STYLE_GUIDELINES.contains("Monospace"));
        assert!(FINAL_ANSWER_STYLE_GUIDELINES.contains("File References"));
        assert!(FINAL_ANSWER_STYLE_GUIDELINES.contains("Tone"));
        assert!(FINAL_ANSWER_STYLE_GUIDELINES.contains("Verbosity"));
    }

    #[test]
    fn test_advanced_model_has_family_instructions() {
        let messages = ModelMessages::new("gpt-5.1-turbo", None);
        assert!(messages.family_instructions().is_some());
        assert!(messages.family_instructions().unwrap().contains("Autonomy"));
    }

    #[test]
    fn test_default_model_no_family_instructions() {
        let messages = ModelMessages::new("some-model", None);
        assert!(messages.family_instructions().is_none());
    }

    #[test]
    fn test_o_series_has_reasoning_instructions() {
        let messages = ModelMessages::new("o3-mini", None);
        assert!(messages.family_instructions().is_some());
        assert!(messages
            .family_instructions()
            .unwrap()
            .contains("Reasoning"));
    }
}
