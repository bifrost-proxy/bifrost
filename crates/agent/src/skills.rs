//! Skill system: discovery, parsing, and instruction building.
//!
//! Skills are loaded from (highest priority first):
//! - `.agents/skills/` in the project directory (repo scope)
//! - `~/.bifrost/agent/skills/` (user scope, for user-created skills)
//! - `~/.bifrost/agent/skills/.system/` (system scope, built-in embedded skills)
//!
//! Each skill is a directory containing a SKILL.md file with YAML frontmatter.

use crate::config::{agent_home_dir, user_home_dir, SkillsConfig};
use include_dir::{Dir, DirEntry};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

pub const SKILLS_DIR_NAME: &str = "skills";
pub const AGENTS_DIR_NAME: &str = ".agents";
pub const SKILL_FILENAME: &str = "SKILL.md";

/// Embedded system skills directory (compiled into the binary).
const SYSTEM_SKILLS_DIR: Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/assets/samples");

/// Marker filename to detect whether system skills need reinstallation.
const SYSTEM_SKILLS_MARKER_FILENAME: &str = ".bifrost-system-skills.marker";
/// Salt for fingerprint versioning (bump to force reinstall).
const SYSTEM_SKILLS_MARKER_SALT: &str = "v1";

/// Maximum BFS traversal depth from a skills root directory.
const MAX_SCAN_DEPTH: usize = 6;
/// Maximum number of directories to visit per root during BFS scan.
const MAX_DIRS_PER_ROOT: usize = 2000;
/// Maximum size for a SKILL.md file (2 MiB). Larger files are skipped.
const MAX_SKILL_MD_BYTES: u64 = 2 * 1024 * 1024;
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

/// Where the skill was discovered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SkillScope {
    Repo,
    User,
    Global,
    System,
}

/// YAML frontmatter fields in SKILL.md.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    short_description: Option<String>,
}

// ---------------------------------------------------------------------------
// SkillsManager
// ---------------------------------------------------------------------------

/// Manages skill discovery and loading.
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
    fn effective_user_home(&self) -> Option<PathBuf> {
        self.user_home_override
            .clone()
            .or_else(|| Some(user_home_dir()))
    }

    /// Load all skills from the working directory hierarchy and user home.
    ///
    /// Loading order (first occurrence wins on name collision):
    /// 1. Repo scope: `<work_dir>/.agents/skills/`
    /// 2. User scope: `~/.bifrost/agent/skills/` (skips `.system` hidden dir)
    /// 3. Global scope: `~/.agents/skills/` (cross-agent shared directory)
    /// 4. System scope: `~/.bifrost/agent/skills/.system/`
    pub fn load_skills(&self, work_dir: &Path) -> Vec<SkillMetadata> {
        let mut skills = Vec::new();
        let agent_home = self.effective_agent_home();

        // 1. Load from project's .agents/skills/ (highest priority)
        let repo_skills_dir = work_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME);
        if repo_skills_dir.is_dir() {
            let repo_skills = self.scan_skills_dir(&repo_skills_dir, SkillScope::Repo);
            debug!(count = repo_skills.len(), dir = %repo_skills_dir.display(), "loaded repo skills");
            skills.extend(repo_skills);
        }

        // 2. Load from user scope: ~/.bifrost/agent/skills/ (BFS skips .system)
        let user_skills_dir = agent_home.join(SKILLS_DIR_NAME);
        if user_skills_dir.is_dir() {
            let user_skills = self.scan_skills_dir(&user_skills_dir, SkillScope::User);
            debug!(count = user_skills.len(), dir = %user_skills_dir.display(), "loaded user skills");
            skills.extend(user_skills);
        }

        // 3. Load from global scope: ~/.agents/skills/ (cross-agent shared)
        if let Some(user_home) = self.effective_user_home() {
            let global_skills_dir = user_home.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME);
            if global_skills_dir.is_dir() {
                let global_skills = self.scan_skills_dir(&global_skills_dir, SkillScope::Global);
                debug!(count = global_skills.len(), dir = %global_skills_dir.display(), "loaded global shared skills");
                skills.extend(global_skills);
            }
        }

        // 4. Load from system scope: ~/.bifrost/agent/skills/.system/ (lowest priority)
        let system_dir = agent_home.join(SKILLS_DIR_NAME).join(".system");
        if system_dir.is_dir() {
            let system_skills = self.scan_skills_dir(&system_dir, SkillScope::System);
            debug!(count = system_skills.len(), dir = %system_dir.display(), "loaded system skills");
            skills.extend(system_skills);
        }

        // Deduplicate by name (first occurrence wins — repo > user > global > system)
        let mut seen = std::collections::HashSet::new();
        skills.retain(|s| seen.insert(s.name.clone()));

        // Filter by config
        self.filter_skills(&mut skills);

        skills
    }

    /// Scan a skills directory for SKILL.md files using BFS traversal.
    /// Matches Codex behavior: max depth 6, max 2000 directories, skip hidden dirs.
    fn scan_skills_dir(&self, dir: &Path, scope: SkillScope) -> Vec<SkillMetadata> {
        let mut skills = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        let mut visited = std::collections::HashSet::new();
        visited.insert(dir.to_path_buf());
        queue.push_back((dir.to_path_buf(), 0usize));

        while let Some((current_dir, depth)) = queue.pop_front() {
            let entries = match std::fs::read_dir(&current_dir) {
                Ok(entries) => entries,
                Err(e) => {
                    warn!(dir = %current_dir.display(), error = %e, "failed to read skills directory");
                    continue;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name.to_string(),
                    None => continue,
                };

                // Skip hidden directories (., .git, .drafts, .history, .system, etc.)
                if file_name.starts_with('.') {
                    continue;
                }

                if !path.is_dir() {
                    continue;
                }

                // If this directory contains SKILL.md, parse it as a skill.
                let skill_file = path.join(SKILL_FILENAME);
                if skill_file.is_file() {
                    // Respect .disabled marker written by SkillStore.enable().
                    if path.join(".disabled").exists() {
                        debug!(dir = %path.display(), "skill disabled via .disabled marker, skipping");
                        continue;
                    }
                    if let Some(skill) = self.parse_skill_md(&skill_file, scope.clone()) {
                        skills.push(skill);
                    }
                }

                // Enqueue for deeper scan if within depth limit.
                if depth < MAX_SCAN_DEPTH
                    && visited.len() < MAX_DIRS_PER_ROOT
                    && visited.insert(path.clone())
                {
                    queue.push_back((path, depth + 1));
                }
            }
        }

        skills
    }

    /// Parse a SKILL.md file (YAML frontmatter + body).
    fn parse_skill_md(&self, path: &Path, scope: SkillScope) -> Option<SkillMetadata> {
        // Guard against OOM from unexpectedly large files.
        if let Err(e) = bifrost_core::text::check_file_size(path, MAX_SKILL_MD_BYTES) {
            warn!(path = %path.display(), error = %e, "SKILL.md too large, skipping");
            return None;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read SKILL.md");
                return None;
            }
        };

        let (frontmatter, body) = parse_frontmatter(&content);

        // Derive skill name from directory name if not in frontmatter
        let dir_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let name = sanitize_single_line(
            &frontmatter
                .as_ref()
                .and_then(|f| f.name.clone())
                .unwrap_or_else(|| dir_name.clone()),
        );

        let description = sanitize_single_line(
            &frontmatter
                .as_ref()
                .and_then(|f| f.description.clone())
                .unwrap_or_else(|| format!("Skill: {}", name)),
        );

        let short_description = frontmatter
            .as_ref()
            .and_then(|f| f.short_description.clone())
            .map(|s| sanitize_single_line(&s))
            .filter(|s| !s.is_empty());

        // Validate field lengths (aligned with Codex limits).
        if name.chars().count() > MAX_NAME_LEN {
            warn!(path = %path.display(), "skill name too long ({} chars, max {}), skipping", name.chars().count(), MAX_NAME_LEN);
            return None;
        }
        if description.chars().count() > MAX_DESCRIPTION_LEN {
            warn!(path = %path.display(), "skill description too long ({} chars, max {}), skipping", description.chars().count(), MAX_DESCRIPTION_LEN);
            return None;
        }

        Some(SkillMetadata {
            name,
            description,
            short_description,
            prompt_content: body.to_string(),
            path: path.to_path_buf(),
            scope,
        })
    }

    /// Filter skills based on config enable/disable settings.
    fn filter_skills(&self, skills: &mut Vec<SkillMetadata>) {
        if let Some(ref config) = self.config {
            let disabled: Vec<&str> = config
                .config
                .iter()
                .filter(|e| !e.enabled)
                .map(|e| e.name.as_str())
                .collect();

            if !disabled.is_empty() {
                skills.retain(|s| !disabled.contains(&s.name.as_str()));
            }
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

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

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
---
This is the body content.
"#;
        let (fm, body) = parse_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm.name.as_deref(), Some("my-skill"));
        assert_eq!(fm.description.as_deref(), Some("A test skill"));
        assert!(body.contains("This is the body content."));
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

        // Create skill without name in frontmatter
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
}
