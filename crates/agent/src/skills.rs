//! Skill system: discovery, parsing, and instruction building.
//!
//! This module is a thin compatibility layer over `bifrost_skills::SkillStore`.
//! Historically, the agent crate shipped its own walkdir-based loader that
//! lived in parallel with the canonical skill store in the `bifrost-skills`
//! crate. The two implementations drifted (duplicated scope semantics,
//! diverging disabled-marker handling, and no shared validation).
//!
//! As of the 2026/05 refactor, all discovery routes through `SkillStore`,
//! which already enforces:
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
//! The surface exposed by this module (`SkillsManager`, `SkillMetadata`,
//! `SkillScope` re-export, `install_system_skills`) is preserved verbatim so
//! existing callers (`prompt.rs`, `bifrost-e2e/tests/skill_loading.rs`,
//! `bifrost-admin`) continue to compile without modification.
//!
//! Embedded system skills are still bootstrapped from `src/assets/samples/`
//! via `include_dir!` — that concern is orthogonal to the loader and remains
//! in this module because it is agent-crate-specific packaging.

use crate::config::{agent_home_dir, user_home_dir, SkillsConfig};
use bifrost_skills::{ScopeRoot, SkillStore};
use include_dir::{Dir, DirEntry};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

pub const SKILLS_DIR_NAME: &str = "skills";
pub const AGENTS_DIR_NAME: &str = ".agents";
pub const SKILL_FILENAME: &str = "SKILL.md";

/// Re-export the canonical `SkillScope` so the agent crate stays in lockstep
/// with `bifrost_skills`. The variant set (`Repo`, `User`, `Global`, `System`)
/// and priority ordering are identical to the legacy agent-local enum.
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
    /// Includes both the skill description and the full prompt content (body of SKILL.md),
    /// matching Codex's behavior of injecting skill prompts into system instructions.
    pub fn build_skills_instructions(&self, skills: &[SkillMetadata]) -> String {
        if skills.is_empty() {
            return String::new();
        }

        let include = self
            .config
            .as_ref()
            .map(|c| c.include_instructions)
            .unwrap_or(true);

        if !include {
            return String::new();
        }

        let mut output = String::from("\n## Available Skills\n\n");
        output.push_str(
            "The following skills are available. Each provides specialized capabilities.\n\n",
        );

        for skill in skills {
            output.push_str(&format!("### {}\n", skill.name));
            output.push_str(&format!("{}\n", skill.description));
            // Inject skill prompt content (the body of SKILL.md)
            if !skill.prompt_content.is_empty() {
                output.push_str(&format!("\n{}\n", skill.prompt_content));
            }
            output.push('\n');
        }

        output
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
///   to a single line (matches legacy behavior).
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

    // Length guards (kept consistent with legacy behavior / Codex limits).
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
        }];

        let manager = SkillsManager::new(None);
        let instructions = manager.build_skills_instructions(&skills);

        assert!(instructions.contains("test-skill"));
        assert!(instructions.contains("Does testing"));
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
            },
            SkillMetadata {
                name: "drop".to_string(),
                description: "d".into(),
                short_description: None,
                prompt_content: String::new(),
                path: PathBuf::from("/dev/null"),
                scope: SkillScope::Repo,
            },
        ];
        manager.filter_skills(&mut skills);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "keep");
    }
}
