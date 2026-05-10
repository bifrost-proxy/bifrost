//! Skill system: discovery, parsing, and instruction building.
//!
//! This module adapts `bifrost_skills::SkillStore` for prompt rendering.
//!
//! Skill discovery routes through `SkillStore`, which enforces:
//! - Scope priority (Repo > User > Global > System) via `apply_effective_scopes`.
//! - `.disabled` marker filtering.
//! - Hidden directory skipping (including `.history`, `.drafts`, `.git`).
//! - Manifest / frontmatter validation at commit time.
//!
//! `SkillRegistry` (which layers slash-command indexing and filesystem watcher
//! on top of `SkillStore`) is deliberately **not** used in this read-only
//! prompt-build path; it is wired by session / admin paths that need slash
//! resolution and hot-reload.
//!
//! The surface exposed by this module is intentionally small:
//! `SkillsManager`, `SkillMetadata`, `SkillScope` re-export, and
//! `install_system_skills`.
//! Embedded system skills are still bootstrapped from `src/assets/samples/`
//! via `include_dir!` — that concern is orthogonal to the loader and remains
//! in this module because it is agent-crate-specific packaging.

pub mod mentions;
pub mod render;

use crate::config::{agent_home_dir, user_home_dir, SkillsConfig};
use bifrost_skills::{ScopeRoot, SkillStore};
use include_dir::{Dir, DirEntry};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

pub use self::mentions::{
    build_skill_injections, extract_skill_mentions, resolve_mentioned_skills, SkillMentions,
};
pub use self::render::{
    default_skill_metadata_budget, render_skill_lines, SkillMetadataBudget, SkillRenderReport,
};

pub const SKILLS_DIR_NAME: &str = "skills";
pub const AGENTS_DIR_NAME: &str = ".agents";
pub const SKILL_FILENAME: &str = "SKILL.md";

/// Re-export the canonical `SkillScope` so the agent crate stays in lockstep
/// with `bifrost_skills`.
pub use bifrost_skills::SkillScope;

/// Embedded system skills directory (compiled into the binary).
const SYSTEM_SKILLS_DIR: Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/assets/samples");

/// Marker filename to detect whether system skills need reinstallation.
const SYSTEM_SKILLS_MARKER_FILENAME: &str = ".bifrost-system-skills.marker";
/// Salt for fingerprint versioning (bump to force reinstall).
const SYSTEM_SKILLS_MARKER_SALT: &str = "v1";

/// Maximum allowed length for a skill name (characters).
const MAX_NAME_LEN: usize = 64;
/// Maximum allowed length for a skill description (characters).
const MAX_DESCRIPTION_LEN: usize = 1024;

// ---------------------------------------------------------------------------
// System Skills Installation
// ---------------------------------------------------------------------------

/// Returns the on-disk cache path for embedded system skills.
///
/// Layout: `~/.bifrost/agent/skills/.system/`
pub fn system_skills_dir() -> PathBuf {
    agent_home_dir().join(SKILLS_DIR_NAME).join(".system")
}

/// Returns the user skills directory.
///
/// Layout: `~/.bifrost/agent/skills/`
pub fn user_skills_dir() -> PathBuf {
    agent_home_dir().join(SKILLS_DIR_NAME)
}

/// Installs embedded system skills to `~/.bifrost/agent/skills/.system/`.
///
/// Uses a fingerprint marker file to skip reinstallation when the embedded
/// content hasn't changed. Call this at agent startup.
pub fn install_system_skills() {
    let dest = system_skills_dir();

    // Check marker to skip if unchanged.
    let marker_path = dest.join(SYSTEM_SKILLS_MARKER_FILENAME);
    let expected = embedded_fingerprint();
    if dest.is_dir() {
        if let Ok(existing) = std::fs::read_to_string(&marker_path) {
            if existing.trim() == expected {
                debug!("system skills up-to-date, skipping install");
                return;
            }
        }
    }

    // Clean and rewrite.
    if dest.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dest) {
            warn!(error = %e, "failed to remove old system skills dir");
            return;
        }
    }
    if let Err(e) = write_embedded_dir(&SYSTEM_SKILLS_DIR, &dest) {
        warn!(error = %e, "failed to install system skills");
        return;
    }
    if let Err(e) = std::fs::write(&marker_path, format!("{expected}\n")) {
        warn!(error = %e, "failed to write system skills marker");
    }
    debug!(path = %dest.display(), "installed system skills");
}

fn embedded_fingerprint() -> String {
    let mut items = Vec::new();
    collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
    items.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    let mut hasher = DefaultHasher::new();
    SYSTEM_SKILLS_MARKER_SALT.hash(&mut hasher);
    for (path, contents_hash) in items {
        path.hash(&mut hasher);
        contents_hash.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

fn collect_fingerprint_items(dir: &Dir<'_>, items: &mut Vec<(String, Option<u64>)>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(subdir) => {
                items.push((subdir.path().to_string_lossy().to_string(), None));
                collect_fingerprint_items(subdir, items);
            }
            DirEntry::File(file) => {
                let mut file_hasher = DefaultHasher::new();
                file.contents().hash(&mut file_hasher);
                items.push((
                    file.path().to_string_lossy().to_string(),
                    Some(file_hasher.finish()),
                ));
            }
        }
    }
}

fn write_embedded_dir(dir: &Dir<'_>, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(subdir) => {
                let subdir_dest = dest.join(subdir.path());
                std::fs::create_dir_all(&subdir_dest)?;
                write_embedded_dir(subdir, dest)?;
            }
            DirEntry::File(file) => {
                let path = dest.join(file.path());
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, file.contents())?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Skill types
// ---------------------------------------------------------------------------

/// Metadata and content of a discovered skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    pub prompt_content: String,
    pub path: PathBuf,
    pub scope: SkillScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<SkillInterface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<SkillDependencies>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<SkillPolicy>,
}

/// Visual and UX metadata for skill presentation in IDE/UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillInterface {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
    pub icon_small: Option<PathBuf>,
    pub icon_large: Option<PathBuf>,
    pub brand_color: Option<String>,
    pub default_prompt: Option<String>,
}

/// Declared tool dependencies for a skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDependencies {
    pub tools: Vec<SkillToolDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillToolDependency {
    pub r#type: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Policy controlling skill invocation behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SkillPolicy {
    /// Whether this skill can be triggered implicitly by description matching.
    /// Defaults to true when None.
    pub allow_implicit_invocation: Option<bool>,
    /// Product restrictions (empty = available in all products).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub products: Vec<String>,
}

/// Structured outcome from skill loading, matching Codex's `SkillLoadOutcome`.
#[derive(Debug, Clone, Default)]
pub struct SkillLoadOutcome {
    pub skills: Vec<SkillMetadata>,
    pub errors: Vec<SkillLoadError>,
    pub disabled_paths: HashSet<PathBuf>,
    pub skill_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SkillLoadError {
    pub path: PathBuf,
    pub message: String,
}

impl SkillLoadOutcome {
    /// Skills allowed for implicit invocation (enabled + policy allows).
    pub fn allowed_skills_for_implicit_invocation(&self) -> Vec<&SkillMetadata> {
        self.skills
            .iter()
            .filter(|skill| {
                !self.disabled_paths.contains(&skill.path)
                    && skill
                        .policy
                        .as_ref()
                        .and_then(|p| p.allow_implicit_invocation)
                        .unwrap_or(true)
            })
            .collect()
    }

    /// Collect all environment variable dependencies declared by active skills.
    ///
    /// Returns a map of env var name → list of skill names that require it.
    /// Matches Codex's dependency resolution behavior in `core/skills.rs`.
    pub fn collect_env_dependencies(&self) -> HashMap<String, Vec<String>> {
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        for skill in &self.skills {
            if self.disabled_paths.contains(&skill.path) {
                continue;
            }
            if let Some(ref skill_deps) = skill.dependencies {
                for tool_dep in &skill_deps.tools {
                    if tool_dep.r#type == "env" {
                        deps.entry(tool_dep.value.clone())
                            .or_default()
                            .push(skill.name.clone());
                    }
                }
            }
        }
        deps
    }

    /// Check which declared env dependencies are missing from the environment.
    ///
    /// Returns tuples of (env_var_name, requiring_skill_names) for vars not set.
    pub fn missing_env_dependencies(&self) -> Vec<(String, Vec<String>)> {
        self.collect_env_dependencies()
            .into_iter()
            .filter(|(var, _)| std::env::var(var).is_err())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Implicit Skill Invocation Detection
// ---------------------------------------------------------------------------

/// Result of implicit skill invocation detection.
#[derive(Debug, Clone)]
pub struct ImplicitSkillMatch {
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub match_reason: ImplicitMatchReason,
}

/// Reason an implicit skill invocation was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplicitMatchReason {
    /// A command executed matched a script inside the skill's directory.
    ScriptInSkillDir,
    /// The command path is within a skill's directory tree.
    CommandInSkillDir,
}

/// Detect whether a command execution implicitly invokes a skill.
///
/// Matches Codex's `detect_implicit_skill_invocation_for_command` behavior.
/// Checks if the command path falls within any skill's directory.
pub fn detect_implicit_skill_invocation(
    command: &str,
    outcome: &SkillLoadOutcome,
) -> Option<ImplicitSkillMatch> {
    let command_path = Path::new(command.split_whitespace().next()?);

    for skill in &outcome.skills {
        if outcome.disabled_paths.contains(&skill.path) {
            continue;
        }
        // Check if command is within the skill's directory
        if let Some(skill_dir) = skill.path.parent() {
            if command_path.starts_with(skill_dir) {
                let reason = if command_path
                    .parent()
                    .and_then(|p| p.file_name())
                    .is_some_and(|n| n == "scripts")
                {
                    ImplicitMatchReason::ScriptInSkillDir
                } else {
                    ImplicitMatchReason::CommandInSkillDir
                };
                return Some(ImplicitSkillMatch {
                    skill_name: skill.name.clone(),
                    skill_path: skill.path.clone(),
                    match_reason: reason,
                });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// SkillsManager
// ---------------------------------------------------------------------------

/// Manages skill discovery and loading.
///
/// Internally delegates to `bifrost_skills::SkillStore` so that scope
/// priority, `.disabled` filtering, and manifest validation live in a single
/// implementation shared with `/skill`, `SkillRegistry`, and the admin HTTP
/// surface.
pub struct SkillsManager {
    config: Option<SkillsConfig>,
    /// Override for the agent home directory (for testing isolation).
    agent_home_override: Option<PathBuf>,
    /// Override for the user home directory (for testing isolation).
    user_home_override: Option<PathBuf>,
}

impl SkillsManager {
    pub fn new(config: Option<SkillsConfig>) -> Self {
        Self {
            config,
            agent_home_override: None,
            user_home_override: None,
        }
    }

    /// Create a SkillsManager with explicit overrides for agent/user home directories.
    ///
    /// Primarily used by tests (unit + E2E) to isolate filesystem side effects.
    pub fn with_overrides(
        config: Option<SkillsConfig>,
        agent_home: PathBuf,
        user_home: PathBuf,
    ) -> Self {
        Self {
            config,
            agent_home_override: Some(agent_home),
            user_home_override: Some(user_home),
        }
    }

    /// Resolve the effective agent home directory.
    fn effective_agent_home(&self) -> PathBuf {
        self.agent_home_override
            .clone()
            .unwrap_or_else(agent_home_dir)
    }

    /// Resolve the effective user home directory.
    fn effective_user_home(&self) -> PathBuf {
        self.user_home_override
            .clone()
            .unwrap_or_else(user_home_dir)
    }

    /// Build the four `ScopeRoot`s this manager reads from, in scope-priority
    /// order (System → Global → User → Repo).
    fn scope_roots(&self, work_dir: &Path) -> Vec<ScopeRoot> {
        let agent_home = self.effective_agent_home();
        let user_home = self.effective_user_home();
        vec![
            // System: <agent_home>/skills/.system/
            ScopeRoot::new(
                SkillScope::System,
                agent_home.join(SKILLS_DIR_NAME).join(".system"),
            ),
            // Global: <user_home>/.agents/skills/
            ScopeRoot::new(
                SkillScope::Global,
                user_home.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME),
            ),
            // User: <agent_home>/skills/
            ScopeRoot::new(SkillScope::User, agent_home.join(SKILLS_DIR_NAME)),
            // Repo: <work_dir>/.agents/skills/  (highest priority)
            ScopeRoot::new(
                SkillScope::Repo,
                work_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME),
            ),
        ]
    }

    /// Load all skills from the working directory hierarchy and user home.
    ///
    /// Discovery is delegated to `SkillStore::read_all`, which:
    /// 1. Performs a depth-limited BFS over each scope root.
    /// 2. Skips hidden directories (`.history`, `.drafts`, `.git`, `.system`
    ///    when not directly configured as a root, etc.).
    /// 3. Respects `.disabled` markers (written by `SkillStore::enable(false)`).
    /// 4. Applies the scope-priority overlay so a Repo skill shadows a
    ///    same-named User/Global/System skill.
    ///
    /// Each surviving `SkillRecord` is then projected into `SkillMetadata`
    /// (which carries the raw SKILL.md body used for prompt injection), and
    /// finally filtered by the user's config-level enable/disable list.
    ///
    /// Unlike `SkillRegistry::init`, this path does **not** spawn a filesystem
    /// watcher and does **not** build a slash-command index; it is cheap to
    /// call repeatedly from prompt construction.
    pub fn load_skills(&self, work_dir: &Path) -> Vec<SkillMetadata> {
        let roots = self.scope_roots(work_dir);
        let store = SkillStore::new(roots);
        let records = match store.read_all() {
            Ok(records) => records,
            Err(error) => {
                warn!(error = %error, "failed to read skills via SkillStore, returning empty list");
                return Vec::new();
            }
        };

        let mut skills: Vec<SkillMetadata> = records
            .into_iter()
            .filter(|record| record.enabled)
            .filter_map(|record| project_record(&record))
            .collect();

        debug!(
            count = skills.len(),
            work_dir = %work_dir.display(),
            "loaded skills via SkillStore"
        );

        // Apply user-configured enable/disable overlay.
        self.filter_skills(&mut skills);
        skills
    }

    /// Load all skills and return a structured outcome with errors and metadata.
    pub fn load_skills_with_outcome(&self, work_dir: &Path) -> SkillLoadOutcome {
        let roots = self.scope_roots(work_dir);
        let root_paths: Vec<PathBuf> = roots.iter().map(|r| r.path.clone()).collect();
        let store = SkillStore::new(roots);

        let records = match store.read_all() {
            Ok(records) => records,
            Err(error) => {
                warn!(error = %error, "failed to read skills via SkillStore");
                return SkillLoadOutcome {
                    skills: Vec::new(),
                    errors: vec![SkillLoadError {
                        path: PathBuf::from("<store>"),
                        message: error.to_string(),
                    }],
                    disabled_paths: HashSet::new(),
                    skill_roots: root_paths,
                };
            }
        };

        let mut disabled_paths = HashSet::new();
        let mut skills = Vec::new();

        for record in records {
            if !record.enabled {
                disabled_paths.insert(record.skill_md_path.clone());
                continue;
            }
            if let Some(meta) = project_record(&record) {
                skills.push(meta);
            }
        }

        // Apply user-configured disable overlay
        if let Some(config) = self.config.as_ref() {
            let disabled: HashSet<&str> = config
                .config
                .iter()
                .filter(|e| !e.enabled)
                .map(|e| e.name.as_str())
                .collect();
            if !disabled.is_empty() {
                skills.retain(|s| !disabled.contains(s.name.as_str()));
            }
        }

        debug!(
            count = skills.len(),
            work_dir = %work_dir.display(),
            "loaded skills with outcome"
        );

        SkillLoadOutcome {
            skills,
            errors: Vec::new(),
            disabled_paths,
            skill_roots: root_paths,
        }
    }

    /// Filter skills based on config enable/disable settings.
    fn filter_skills(&self, skills: &mut Vec<SkillMetadata>) {
        let Some(config) = self.config.as_ref() else {
            return;
        };
        let disabled: HashSet<&str> = config
            .config
            .iter()
            .filter(|e| !e.enabled)
            .map(|e| e.name.as_str())
            .collect();
        if !disabled.is_empty() {
            skills.retain(|s| !disabled.contains(s.name.as_str()));
        }
    }

    /// Build the skills instruction text for the system prompt.
    ///
    /// Uses progressive disclosure: only outputs name + description + file path.
    /// The model can read the SKILL.md via `read_file` when it decides to use a skill.
    pub fn build_skills_instructions(&self, skills: &[SkillMetadata]) -> String {
        let (text, _report) = self.build_skills_instructions_budgeted(skills, None);
        text
    }

    /// Build the skills instruction text for the system prompt using progressive disclosure.
    ///
    /// Instead of injecting the full SKILL.md body, only outputs name + description + file path.
    /// The model can read the SKILL.md via `read_file` when it decides to use a skill.
    /// Descriptions are truncated proportionally when the budget is tight.
    pub fn build_skills_instructions_budgeted(
        &self,
        skills: &[SkillMetadata],
        budget: Option<SkillMetadataBudget>,
    ) -> (String, Option<SkillRenderReport>) {
        if skills.is_empty() {
            return (String::new(), None);
        }

        let include = self
            .config
            .as_ref()
            .map(|c| c.include_instructions)
            .unwrap_or(true);

        if !include {
            return (String::new(), None);
        }

        let budget = budget.unwrap_or(SkillMetadataBudget::Characters(
            render::default_char_budget(),
        ));
        let (skill_lines, report) = render_skill_lines(skills, budget);

        let mut output = String::from("\n## Skills\n");
        output.push_str(render::SKILLS_INTRO);
        output.push_str("\n### Available skills\n");
        for line in &skill_lines {
            output.push_str(line);
            output.push('\n');
        }
        output.push_str("\n### How to use skills\n");
        output.push_str(render::SKILLS_HOW_TO_USE);
        output.push('\n');

        (output, Some(report))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collapse all whitespace (newlines, tabs, runs of spaces) into a single space.
/// Matches Codex's `sanitize_single_line` behavior.
fn sanitize_single_line(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Project a `SkillRecord` (from `bifrost_skills`) into the `SkillMetadata`
/// shape consumed by prompt injection and the admin API.
///
/// - `name` / `description` come from the validated manifest and are sanitized
///   to a single line.
/// - `prompt_content` is the body of SKILL.md (minus YAML frontmatter). Read
///   fresh from disk so edits to instructions propagate without rebuilding the
///   manifest.
/// - `short_description` is recovered from the SKILL.md frontmatter when
///   present. It is optional and not carried on `SkillRecord`.
/// - `scope` uses `effective_scope` so the caller sees the winning scope after
///   the override overlay (e.g. a Repo skill shadowing a System skill reports
///   `Repo`).
fn project_record(record: &bifrost_skills::SkillRecord) -> Option<SkillMetadata> {
    let name = sanitize_single_line(&record.name);
    let description = sanitize_single_line(&record.description);

    // Length guards match the manifest limits used by prompt rendering.
    if name.chars().count() > MAX_NAME_LEN {
        warn!(
            path = %record.skill_md_path.display(),
            "skill name too long ({} chars, max {}), skipping",
            name.chars().count(),
            MAX_NAME_LEN,
        );
        return None;
    }
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        warn!(
            path = %record.skill_md_path.display(),
            "skill description too long ({} chars, max {}), skipping",
            description.chars().count(),
            MAX_DESCRIPTION_LEN,
        );
        return None;
    }

    // Read SKILL.md body for prompt injection.
    let (short_description, prompt_content) = match std::fs::read_to_string(&record.skill_md_path) {
        Ok(content) => {
            let (frontmatter, body) = parse_frontmatter(&content);
            let short = frontmatter
                .and_then(|fm| fm.short_description)
                .map(|s| sanitize_single_line(&s))
                .filter(|s| !s.is_empty());
            (short, body.to_string())
        }
        Err(e) => {
            warn!(
                path = %record.skill_md_path.display(),
                error = %e,
                "failed to read SKILL.md, using empty prompt body"
            );
            (None, String::new())
        }
    };

    Some(SkillMetadata {
        name,
        description,
        short_description,
        prompt_content,
        path: record.skill_md_path.clone(),
        scope: record.effective_scope.clone(),
        interface: None,
        dependencies: None,
        policy: None,
    })
}

// ---------------------------------------------------------------------------
// Frontmatter parsing (retained for `short_description` extraction)
// ---------------------------------------------------------------------------

/// Minimal YAML frontmatter shape. The authoritative manifest (name, description,
/// scope, etc.) lives in `manifest.json`; we only reach into SKILL.md frontmatter
/// here to surface `short_description`, which is an agent-prompt concern not
/// tracked by `SkillManifest`.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    short_description: Option<String>,
}

/// Parse YAML frontmatter from a markdown file.
/// Frontmatter is delimited by `---` lines at the start of the file.
fn parse_frontmatter(content: &str) -> (Option<SkillFrontmatter>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }

    // Find end delimiter
    let after_first = &trimmed[3..];
    let after_first = after_first.trim_start_matches(['\r', '\n']);

    if let Some(end_pos) = after_first.find("\n---") {
        let yaml_str = &after_first[..end_pos];
        let body_start = end_pos + 4; // skip "\n---"
        let body = after_first[body_start..].trim_start_matches(['\r', '\n']);

        match serde_yaml::from_str::<SkillFrontmatter>(yaml_str) {
            Ok(fm) => (Some(fm), body),
            Err(e) => {
                warn!(error = %e, "failed to parse SKILL.md frontmatter");
                (None, content)
            }
        }
    } else {
        (None, content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = r#"---
name: my-skill
description: A test skill
short_description: short
---
This is the body content.
"#;
        let (fm, body) = parse_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm.short_description.as_deref(), Some("short"));
        assert!(body.contains("This is the body content."));
    }

    #[test]
    fn test_parse_frontmatter_without_short_description() {
        // Frontmatter without short_description is still valid; body is returned verbatim.
        let content = r#"---
name: plain-skill
description: No short form
---
Body only.
"#;
        let (fm, body) = parse_frontmatter(content);
        let fm = fm.unwrap();
        assert!(fm.short_description.is_none());
        assert!(body.contains("Body only."));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just regular markdown content.";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_skills_manager_load_skills() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create .agents/skills/test-skill/SKILL.md
        let skill_dir = root
            .join(AGENTS_DIR_NAME)
            .join(SKILLS_DIR_NAME)
            .join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join(SKILL_FILENAME),
            r#"---
name: test-skill
description: A test skill for testing
---
Do something useful.
"#,
        )
        .unwrap();

        // Use with_overrides to isolate (agent_home + user_home with no user/system skills)
        let agent_home = dir.path().join("fake-agent-home");
        let user_home = dir.path().join("fake-user-home");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();
        let manager = SkillsManager::with_overrides(None, agent_home, user_home);
        let skills = manager.load_skills(root);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
        assert_eq!(skills[0].description, "A test skill for testing");
        assert!(skills[0].prompt_content.contains("Do something useful."));
        assert_eq!(skills[0].scope, SkillScope::Repo);
    }

    #[test]
    fn test_skills_manager_filter_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create two skills
        for name in &["skill-a", "skill-b"] {
            let skill_dir = root.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME).join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join(SKILL_FILENAME),
                format!(
                    "---\nname: {}\ndescription: Skill {}\n---\nContent",
                    name, name
                ),
            )
            .unwrap();
        }

        use crate::config::{SkillConfigEntry, SkillsConfig};
        let skills_config = SkillsConfig {
            include_instructions: true,
            config: vec![SkillConfigEntry {
                name: "skill-b".to_string(),
                enabled: false,
            }],
        };

        let agent_home = dir.path().join("fake-agent-home");
        let user_home = dir.path().join("fake-user-home");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();
        let manager = SkillsManager::with_overrides(Some(skills_config), agent_home, user_home);
        let skills = manager.load_skills(root);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "skill-a");
    }

    #[test]
    fn test_build_skills_instructions() {
        let skills = vec![SkillMetadata {
            name: "test-skill".to_string(),
            description: "Does testing".to_string(),
            short_description: None,
            prompt_content: "Prompt body".to_string(),
            path: PathBuf::from("/tmp/test"),
            scope: SkillScope::Repo,
            interface: None,
            dependencies: None,
            policy: None,
        }];

        let manager = SkillsManager::new(None);
        let instructions = manager.build_skills_instructions(&skills);

        assert!(instructions.contains("test-skill"));
        assert!(instructions.contains("Does testing"));
        assert!(instructions.contains("/tmp/test"));
        assert!(
            !instructions.contains("Prompt body"),
            "base skill prompt uses progressive disclosure and must not eagerly inject SKILL.md body"
        );
    }

    #[test]
    fn test_build_skills_instructions_empty() {
        let manager = SkillsManager::new(None);
        let instructions = manager.build_skills_instructions(&[]);
        assert!(instructions.is_empty());
    }

    #[test]
    fn test_skill_name_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create skill without name in frontmatter — SkillStore falls back to
        // the directory slug as the skill name.
        let skill_dir = root
            .join(AGENTS_DIR_NAME)
            .join(SKILLS_DIR_NAME)
            .join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join(SKILL_FILENAME),
            "---\ndescription: No name field\n---\nBody",
        )
        .unwrap();

        let agent_home = dir.path().join("fake-agent-home");
        let user_home = dir.path().join("fake-user-home");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();
        let manager = SkillsManager::with_overrides(None, agent_home, user_home);
        let skills = manager.load_skills(root);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill"); // derived from dir name
    }

    #[test]
    fn test_system_skills_embedded() {
        // Verify that the embedded system skills directory contains skill-creator
        let mut items = Vec::new();
        collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
        let paths: Vec<String> = items.into_iter().map(|(path, _)| path).collect();
        assert!(
            paths.iter().any(|p| p.contains("skill-creator/SKILL.md")),
            "embedded system skills should contain skill-creator/SKILL.md"
        );
    }

    #[test]
    fn test_install_and_load_system_skills() {
        let dir = tempfile::tempdir().unwrap();
        let agent_home = dir.path().join("agent");
        let user_home = dir.path().join("user-home");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();

        // Manually install to the test dir
        let system_dir = agent_home.join(SKILLS_DIR_NAME).join(".system");
        write_embedded_dir(&SYSTEM_SKILLS_DIR, &system_dir).unwrap();

        let manager = SkillsManager::with_overrides(None, agent_home, user_home);
        let work_dir = dir.path().join("project");
        fs::create_dir_all(&work_dir).unwrap();
        let skills = manager.load_skills(&work_dir);

        assert!(
            skills.iter().any(|s| s.name == "skill-creator"),
            "system skills should include skill-creator"
        );
        let sc = skills.iter().find(|s| s.name == "skill-creator").unwrap();
        assert_eq!(sc.scope, SkillScope::System);
    }

    #[test]
    fn test_repo_skill_overrides_system() {
        let dir = tempfile::tempdir().unwrap();
        let agent_home = dir.path().join("agent");
        let user_home = dir.path().join("user-home");
        let work_dir = dir.path().join("project");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();
        fs::create_dir_all(&work_dir).unwrap();

        // Install system skills
        let system_dir = agent_home.join(SKILLS_DIR_NAME).join(".system");
        write_embedded_dir(&SYSTEM_SKILLS_DIR, &system_dir).unwrap();

        // Create repo skill with same name "skill-creator"
        let repo_skill_dir = work_dir
            .join(AGENTS_DIR_NAME)
            .join(SKILLS_DIR_NAME)
            .join("skill-creator");
        fs::create_dir_all(&repo_skill_dir).unwrap();
        fs::write(
            repo_skill_dir.join(SKILL_FILENAME),
            "---\nname: skill-creator\ndescription: My custom override\n---\nOverridden body",
        )
        .unwrap();

        let manager = SkillsManager::with_overrides(None, agent_home, user_home);
        let skills = manager.load_skills(&work_dir);

        let sc = skills.iter().find(|s| s.name == "skill-creator").unwrap();
        assert_eq!(sc.scope, SkillScope::Repo);
        assert!(sc.prompt_content.contains("Overridden body"));
    }

    #[test]
    fn test_disabled_marker_skips_skill() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create a skill
        let skill_dir = root
            .join(AGENTS_DIR_NAME)
            .join(SKILLS_DIR_NAME)
            .join("disabled-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join(SKILL_FILENAME),
            "---\nname: disabled-skill\ndescription: Should be skipped\n---\nBody",
        )
        .unwrap();

        // Write .disabled marker (as SkillStore.enable(false) does)
        fs::write(skill_dir.join(".disabled"), b"disabled").unwrap();

        let agent_home = dir.path().join("fake-agent-home");
        let user_home = dir.path().join("fake-user-home");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();
        let manager = SkillsManager::with_overrides(None, agent_home, user_home);
        let skills = manager.load_skills(root);

        assert!(
            !skills.iter().any(|s| s.name == "disabled-skill"),
            "disabled skill should not be loaded"
        );
    }

    #[test]
    fn test_scope_roots_layout_and_order() {
        // Guardrail: `scope_roots` ordering and path layout are part of the
        // contract with the E2E suite. Break this and `skill_loading_*` fails.
        let dir = tempfile::tempdir().unwrap();
        let agent_home = dir.path().join("agent");
        let user_home = dir.path().join("user-home");
        let work_dir = dir.path().join("project");

        let manager = SkillsManager::with_overrides(None, agent_home.clone(), user_home.clone());
        let roots = manager.scope_roots(&work_dir);

        assert_eq!(roots.len(), 4);
        assert_eq!(roots[0].scope, SkillScope::System);
        assert_eq!(
            roots[0].path,
            agent_home.join(SKILLS_DIR_NAME).join(".system")
        );
        assert_eq!(roots[1].scope, SkillScope::Global);
        assert_eq!(
            roots[1].path,
            user_home.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME)
        );
        assert_eq!(roots[2].scope, SkillScope::User);
        assert_eq!(roots[2].path, agent_home.join(SKILLS_DIR_NAME));
        assert_eq!(roots[3].scope, SkillScope::Repo);
        assert_eq!(
            roots[3].path,
            work_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME)
        );
    }

    #[test]
    fn test_filter_skills_removes_disabled_entries() {
        // Unit test for `filter_skills` independent of disk IO.
        use crate::config::{SkillConfigEntry, SkillsConfig};
        let manager = SkillsManager::new(Some(SkillsConfig {
            include_instructions: true,
            config: vec![
                SkillConfigEntry {
                    name: "keep".to_string(),
                    enabled: true,
                },
                SkillConfigEntry {
                    name: "drop".to_string(),
                    enabled: false,
                },
            ],
        }));

        let mut skills = vec![
            SkillMetadata {
                name: "keep".to_string(),
                description: "k".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/dev/null"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: None,
                policy: None,
            },
            SkillMetadata {
                name: "drop".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/dev/null"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: None,
                policy: None,
            },
        ];
        manager.filter_skills(&mut skills);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "keep");
    }

    #[test]
    fn test_load_skills_with_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let skill_dir = root
            .join(AGENTS_DIR_NAME)
            .join(SKILLS_DIR_NAME)
            .join("outcome-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join(SKILL_FILENAME),
            "---\nname: outcome-skill\ndescription: Test outcome\n---\nBody",
        )
        .unwrap();

        let agent_home = dir.path().join("fake-agent-home");
        let user_home = dir.path().join("fake-user-home");
        fs::create_dir_all(&agent_home).unwrap();
        fs::create_dir_all(&user_home).unwrap();
        let manager = SkillsManager::with_overrides(None, agent_home, user_home);
        let outcome = manager.load_skills_with_outcome(root);

        assert_eq!(outcome.skills.len(), 1);
        assert_eq!(outcome.skills[0].name, "outcome-skill");
        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.skill_roots.len(), 4);
    }

    #[test]
    fn test_skill_load_outcome_implicit_invocation_filter() {
        let outcome = SkillLoadOutcome {
            skills: vec![
                SkillMetadata {
                    name: "allowed".to_string(),
                    description: "d".into(),
                    short_description: None,
                    prompt_content: String::new(),
                    path: PathBuf::from("/a"),
                    scope: SkillScope::Repo,
                    interface: None,
                    dependencies: None,
                    policy: None,
                },
                SkillMetadata {
                    name: "blocked".to_string(),
                    description: "d".into(),
                    short_description: None,
                    prompt_content: String::new(),
                    path: PathBuf::from("/b"),
                    scope: SkillScope::Repo,
                    interface: None,
                    dependencies: None,
                    policy: Some(SkillPolicy {
                        allow_implicit_invocation: Some(false),
                        products: Vec::new(),
                    }),
                },
            ],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            skill_roots: Vec::new(),
        };

        let allowed = outcome.allowed_skills_for_implicit_invocation();
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].name, "allowed");
    }

    #[test]
    fn test_budgeted_instructions_returns_report() {
        let skills = vec![SkillMetadata {
            name: "test".to_string(),
            description: "A skill".to_string(),
            short_description: None,
            prompt_content: String::new(),
            path: PathBuf::from("/test/SKILL.md"),
            scope: SkillScope::Repo,
            interface: None,
            dependencies: None,
            policy: None,
        }];

        let manager = SkillsManager::new(None);
        let (text, report) = manager.build_skills_instructions_budgeted(&skills, None);
        assert!(text.contains("test"));
        assert!(text.contains("A skill"));
        let report = report.unwrap();
        assert_eq!(report.total_count, 1);
        assert_eq!(report.included_count, 1);
    }

    #[test]
    fn test_collect_env_dependencies() {
        let outcome = SkillLoadOutcome {
            skills: vec![
                SkillMetadata {
                    name: "api-skill".to_string(),
                    description: "needs key".into(),
                    short_description: None,
                    prompt_content: String::new(),
                    path: PathBuf::from("/a/SKILL.md"),
                    scope: SkillScope::Repo,
                    interface: None,
                    dependencies: Some(SkillDependencies {
                        tools: vec![SkillToolDependency {
                            r#type: "env".to_string(),
                            value: "API_KEY".to_string(),
                            description: None,
                            transport: None,
                            command: None,
                            url: None,
                        }],
                    }),
                    policy: None,
                },
                SkillMetadata {
                    name: "other-skill".to_string(),
                    description: "also needs key".into(),
                    short_description: None,
                    prompt_content: String::new(),
                    path: PathBuf::from("/b/SKILL.md"),
                    scope: SkillScope::Repo,
                    interface: None,
                    dependencies: Some(SkillDependencies {
                        tools: vec![SkillToolDependency {
                            r#type: "env".to_string(),
                            value: "API_KEY".to_string(),
                            description: None,
                            transport: None,
                            command: None,
                            url: None,
                        }],
                    }),
                    policy: None,
                },
            ],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            skill_roots: Vec::new(),
        };

        let deps = outcome.collect_env_dependencies();
        assert_eq!(deps.len(), 1);
        let skills = deps.get("API_KEY").unwrap();
        assert_eq!(skills.len(), 2);
        assert!(skills.contains(&"api-skill".to_string()));
        assert!(skills.contains(&"other-skill".to_string()));
    }

    #[test]
    fn test_collect_env_dependencies_skips_disabled() {
        let mut disabled = HashSet::new();
        disabled.insert(PathBuf::from("/a/SKILL.md"));

        let outcome = SkillLoadOutcome {
            skills: vec![SkillMetadata {
                name: "disabled-skill".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/a/SKILL.md"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: Some(SkillDependencies {
                    tools: vec![SkillToolDependency {
                        r#type: "env".to_string(),
                        value: "SECRET".to_string(),
                        description: None,
                        transport: None,
                        command: None,
                        url: None,
                    }],
                }),
                policy: None,
            }],
            errors: Vec::new(),
            disabled_paths: disabled,
            skill_roots: Vec::new(),
        };

        let deps = outcome.collect_env_dependencies();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_detect_implicit_invocation_script_in_skill_dir() {
        let outcome = SkillLoadOutcome {
            skills: vec![SkillMetadata {
                name: "my-skill".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/project/.agents/skills/my-skill/SKILL.md"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: None,
                policy: None,
            }],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            skill_roots: Vec::new(),
        };

        // Command within the skill's scripts/ dir
        let result = detect_implicit_skill_invocation(
            "/project/.agents/skills/my-skill/scripts/run.sh",
            &outcome,
        );
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.skill_name, "my-skill");
        assert_eq!(m.match_reason, ImplicitMatchReason::ScriptInSkillDir);
    }

    #[test]
    fn test_detect_implicit_invocation_command_in_skill_dir() {
        let outcome = SkillLoadOutcome {
            skills: vec![SkillMetadata {
                name: "my-skill".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/project/.agents/skills/my-skill/SKILL.md"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: None,
                policy: None,
            }],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            skill_roots: Vec::new(),
        };

        // Command directly in skill dir (not scripts/)
        let result = detect_implicit_skill_invocation(
            "/project/.agents/skills/my-skill/tool --arg",
            &outcome,
        );
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.skill_name, "my-skill");
        assert_eq!(m.match_reason, ImplicitMatchReason::CommandInSkillDir);
    }

    #[test]
    fn test_detect_implicit_invocation_no_match() {
        let outcome = SkillLoadOutcome {
            skills: vec![SkillMetadata {
                name: "my-skill".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/project/.agents/skills/my-skill/SKILL.md"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: None,
                policy: None,
            }],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            skill_roots: Vec::new(),
        };

        // Unrelated command
        let result = detect_implicit_skill_invocation("ls -la /tmp", &outcome);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_implicit_invocation_skips_disabled() {
        let mut disabled = HashSet::new();
        disabled.insert(PathBuf::from("/project/.agents/skills/my-skill/SKILL.md"));

        let outcome = SkillLoadOutcome {
            skills: vec![SkillMetadata {
                name: "my-skill".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/project/.agents/skills/my-skill/SKILL.md"),
                scope: SkillScope::Repo,
                interface: None,
                dependencies: None,
                policy: None,
            }],
            errors: Vec::new(),
            disabled_paths: disabled,
            skill_roots: Vec::new(),
        };

        let result = detect_implicit_skill_invocation(
            "/project/.agents/skills/my-skill/scripts/run.sh",
            &outcome,
        );
        assert!(result.is_none());
    }
}
