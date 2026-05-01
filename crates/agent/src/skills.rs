//! Skill system: discovery, parsing, and instruction building.
//!
//! Skills are loaded from:
//! - `.agents/skills/` in the project directory (repo scope)
//! - `~/.agents/skills/` (user/global scope, aligned with Codex)
//! - `~/.bifrost/agent/skills/` (legacy user scope, for backward compatibility)
//!
//! Each skill is a directory containing a SKILL.md file with YAML frontmatter.

use crate::config::SkillsConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

pub const SKILLS_DIR_NAME: &str = "skills";
pub const AGENTS_DIR_NAME: &str = ".agents";
pub const SKILL_FILENAME: &str = "SKILL.md";

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
}

impl SkillsManager {
    pub fn new(config: Option<SkillsConfig>) -> Self {
        Self { config }
    }

    /// Load all skills from the working directory hierarchy and user home.
    pub fn load_skills(&self, work_dir: &Path, home_dir: Option<&Path>) -> Vec<SkillMetadata> {
        let mut skills = Vec::new();

        // 1. Load from project's .agents/skills/
        let repo_skills_dir = work_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME);
        if repo_skills_dir.is_dir() {
            let repo_skills = self.scan_skills_dir(&repo_skills_dir, SkillScope::Repo);
            debug!(count = repo_skills.len(), dir = %repo_skills_dir.display(), "loaded repo skills");
            skills.extend(repo_skills);
        }

        // 2. Load from user home ~/.agents/skills/ (Codex-compatible global scope)
        if let Some(user_home) = home_dir_path() {
            let global_skills_dir = user_home.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME);
            if global_skills_dir.is_dir() {
                let global_skills = self.scan_skills_dir(&global_skills_dir, SkillScope::User);
                debug!(count = global_skills.len(), dir = %global_skills_dir.display(), "loaded global user skills");
                skills.extend(global_skills);
            }
        }

        // 3. Load from agent home dir skills/ (legacy: ~/.bifrost/agent/skills/)
        if let Some(home) = home_dir {
            let user_skills_dir = home.join(SKILLS_DIR_NAME);
            if user_skills_dir.is_dir() {
                let user_skills = self.scan_skills_dir(&user_skills_dir, SkillScope::User);
                debug!(count = user_skills.len(), dir = %user_skills_dir.display(), "loaded legacy user skills");
                skills.extend(user_skills);
            }
        }

        // Deduplicate by name (first occurrence wins — repo > global > legacy)
        let mut seen = std::collections::HashSet::new();
        skills.retain(|s| seen.insert(s.name.clone()));

        // Filter by config
        self.filter_skills(&mut skills);

        skills
    }

    /// Scan a skills directory for SKILL.md files.
    fn scan_skills_dir(&self, dir: &Path, scope: SkillScope) -> Vec<SkillMetadata> {
        let mut skills = Vec::new();

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(dir = %dir.display(), error = %e, "failed to read skills directory");
                return skills;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_file = path.join(SKILL_FILENAME);
                if skill_file.is_file() {
                    if let Some(skill) = self.parse_skill_md(&skill_file, scope.clone()) {
                        skills.push(skill);
                    }
                }
            }
        }

        skills
    }

    /// Parse a SKILL.md file (YAML frontmatter + body).
    fn parse_skill_md(&self, path: &Path, scope: SkillScope) -> Option<SkillMetadata> {
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

        let name = frontmatter
            .as_ref()
            .and_then(|f| f.name.clone())
            .unwrap_or_else(|| dir_name.clone());

        let description = frontmatter
            .as_ref()
            .and_then(|f| f.description.clone())
            .unwrap_or_else(|| format!("Skill: {}", name));

        let short_description = frontmatter
            .as_ref()
            .and_then(|f| f.short_description.clone());

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

/// Get user home directory.
fn home_dir_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
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

        // Isolate HOME so global ~/.agents/skills/ is not loaded
        std::env::set_var("HOME", root.to_str().unwrap());

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

        let manager = SkillsManager::new(None);
        let skills = manager.load_skills(root, None);

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

        // Isolate HOME so global ~/.agents/skills/ is not loaded
        std::env::set_var("HOME", root.to_str().unwrap());

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

        let manager = SkillsManager::new(Some(skills_config));
        let skills = manager.load_skills(root, None);

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

        // Isolate HOME so global ~/.agents/skills/ is not loaded
        std::env::set_var("HOME", root.to_str().unwrap());

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

        let manager = SkillsManager::new(None);
        let skills = manager.load_skills(root, None);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill"); // derived from dir name
    }
}
