//! Core types for the prompt building system.
//!
//! Aligned with Codex's architecture:
//! - `BaseInstructions`: System-level instructions (text field only, extensible)
//! - `EnvironmentContext`: Runtime environment info (cwd, shell, date, timezone)
//! - `UserInstructions`: User-provided instructions from AGENTS.md and config

use std::path::PathBuf;

/// Base instructions for the model. Corresponds to the `instructions` field in the Responses API.
///
/// Priority order for resolution:
/// 1. Per-turn override (system_prompt_override)
/// 2. Config-level instructions
/// 3. Default template (loaded from external markdown file)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BaseInstructions {
    pub text: String,
}

impl BaseInstructions {
    /// Create new base instructions with the given text.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Check if instructions are empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Environment context for the current session.
///
/// Rendered as structured XML in the prompt to make it machine-parseable
/// and diff-friendly across turns.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentContext {
    /// Current working directory.
    pub cwd: Option<PathBuf>,
    /// Shell name (e.g., "zsh", "bash").
    pub shell: String,
    /// Operating system (e.g., "macos", "linux").
    pub os: String,
    /// Architecture (e.g., "aarch64", "x86_64").
    pub arch: String,
    /// Current date in ISO format (YYYY-MM-DD).
    pub current_date: Option<String>,
    /// Timezone offset (e.g., "+08:00", "-05:00").
    pub timezone: Option<String>,
}

impl EnvironmentContext {
    /// Create new environment context.
    pub fn new(
        cwd: Option<PathBuf>,
        shell: impl Into<String>,
        os: impl Into<String>,
        arch: impl Into<String>,
    ) -> Self {
        Self {
            cwd,
            shell: shell.into(),
            os: os.into(),
            arch: arch.into(),
            current_date: None,
            timezone: None,
        }
    }

    /// Set the current date.
    pub fn with_date(mut self, date: impl Into<String>) -> Self {
        self.current_date = Some(date.into());
        self
    }

    /// Set the timezone.
    pub fn with_timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }

    /// Create environment context from the current system state.
    pub fn from_system(work_dir: Option<&std::path::Path>) -> Self {
        let cwd = work_dir.map(|p| p.to_path_buf());
        let shell = detect_shell();
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();
        let current_date = Some(current_date_iso());

        Self {
            cwd,
            shell,
            os,
            arch,
            current_date,
            timezone: None,
        }
    }
}

/// User instructions loaded from AGENTS.md and config.
///
/// Rendered as a separate developer message to distinguish from
/// system-level base instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInstructions {
    /// The instruction text content.
    pub text: String,
    /// The directory from which the instructions were loaded (for context).
    pub directory: Option<String>,
}

impl UserInstructions {
    /// Create new user instructions.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            directory: None,
        }
    }

    /// Set the source directory.
    pub fn with_directory(mut self, dir: impl Into<String>) -> Self {
        self.directory = Some(dir.into());
        self
    }

    /// Check if instructions are empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

// ---------------------------------------------------------------------------
// XML tag markers
// ---------------------------------------------------------------------------

/// XML tag markers for environment context section.
pub const ENVIRONMENT_CONTEXT_OPEN_TAG: &str = "<environment_context>";
pub const ENVIRONMENT_CONTEXT_CLOSE_TAG: &str = "</environment_context>";

/// XML tag markers for skills instructions section.
pub const SKILLS_INSTRUCTIONS_OPEN_TAG: &str = "<skills_instructions>";
pub const SKILLS_INSTRUCTIONS_CLOSE_TAG: &str = "</skills_instructions>";

/// XML tag markers for user instructions section.
pub const USER_INSTRUCTIONS_OPEN_TAG: &str = "<user_instructions>";
pub const USER_INSTRUCTIONS_CLOSE_TAG: &str = "</user_instructions>";

/// XML tag markers for goal context section.
pub const GOAL_CONTEXT_OPEN_TAG: &str = "<goal_context>";
pub const GOAL_CONTEXT_CLOSE_TAG: &str = "</goal_context>";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Detect the current shell from the SHELL environment variable.
pub(crate) fn detect_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|s| s.rsplit('/').next().map(|n| n.to_string()))
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "zsh".to_string()
            } else {
                "bash".to_string()
            }
        })
}

/// Get the current date in ISO-8601 format (YYYY-MM-DD) using std::time.
fn current_date_iso() -> String {
    // Use UNIX timestamp to compute date (no external crate needed).
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Days since epoch
    let days = secs / 86400;
    // Compute year/month/day from days since 1970-01-01 (civil date algorithm)
    let (y, m, d) = days_to_civil(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert days since Unix epoch to (year, month, day).
/// Uses the algorithm from http://howardhinnant.github.io/date_algorithms.html
fn days_to_civil(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_instructions_default() {
        let instructions = BaseInstructions::default();
        assert!(instructions.is_empty());
    }

    #[test]
    fn test_base_instructions_new() {
        let instructions = BaseInstructions::new("hello");
        assert_eq!(instructions.text, "hello");
        assert!(!instructions.is_empty());
    }

    #[test]
    fn test_environment_context_from_system() {
        let ctx = EnvironmentContext::from_system(None);
        assert!(!ctx.shell.is_empty());
        assert!(!ctx.os.is_empty());
        assert!(!ctx.arch.is_empty());
        assert!(ctx.current_date.is_some());
        // Verify date format YYYY-MM-DD
        let date = ctx.current_date.unwrap();
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
    }

    #[test]
    fn test_environment_context_builder() {
        let ctx = EnvironmentContext::new(Some(PathBuf::from("/tmp")), "zsh", "macos", "aarch64")
            .with_date("2026-01-15")
            .with_timezone("+08:00");

        assert_eq!(ctx.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(ctx.shell, "zsh");
        assert_eq!(ctx.current_date, Some("2026-01-15".to_string()));
        assert_eq!(ctx.timezone, Some("+08:00".to_string()));
    }

    #[test]
    fn test_user_instructions_builder() {
        let instructions =
            UserInstructions::new("Test instructions").with_directory("/home/user/project");
        assert_eq!(instructions.text, "Test instructions");
        assert_eq!(
            instructions.directory,
            Some("/home/user/project".to_string())
        );
    }

    #[test]
    fn test_days_to_civil_epoch() {
        let (y, m, d) = days_to_civil(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_civil_known_date() {
        // 2026-01-01 is day 20454 from epoch
        let (y, m, d) = days_to_civil(20454);
        assert_eq!((y, m, d), (2026, 1, 1));
    }
}
