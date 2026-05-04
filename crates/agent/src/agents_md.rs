//! AGENTS.md discovery and user instruction assembly.
//!
//! Aligned with Codex's agents_md.rs behavior:
//!
//! 1. Determine the project root by walking upwards from cwd until a
//!    `project_root_markers` entry is found. Default marker: `.git`.
//!    If no marker is found, only cwd is considered.
//! 2. Collect every AGENTS.md from project root down to cwd (inclusive),
//!    checking `AGENTS.override.md` first in each directory.
//! 3. All files share a single global byte budget (`project_doc_max_bytes`).
//! 4. Global instructions from agent home dir are loaded separately.

use crate::config::AgentConfig;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Default filename scanned for AGENTS.md instructions.
pub const DEFAULT_AGENTS_MD_FILENAME: &str = "AGENTS.md";
/// Preferred local override for AGENTS.md instructions.
pub const LOCAL_AGENTS_MD_FILENAME: &str = "AGENTS.override.md";

/// Default project root marker (matching Codex's default: `.git` only).
const DEFAULT_PROJECT_ROOT_MARKERS: &[&str] = &[".git"];

/// When both config instructions and AGENTS.md docs are present, they are
/// concatenated with this separator (matching Codex).
const AGENTS_MD_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";

/// Manages AGENTS.md discovery and loading.
pub struct AgentsMdManager {
    project_doc_max_bytes: usize,
    fallback_filenames: Vec<String>,
}

impl AgentsMdManager {
    pub fn new(config: &AgentConfig) -> Self {
        let fallback_filenames = config
            .project_doc_fallback_filenames
            .clone()
            .unwrap_or_default();

        Self {
            project_doc_max_bytes: config.get_project_doc_max_bytes(),
            fallback_filenames,
        }
    }

    /// Load global instructions from agent home directory.
    /// Checks `AGENTS.override.md` first, then `AGENTS.md` (matching Codex).
    pub fn load_global_instructions(home_dir: &Path) -> Option<String> {
        for candidate in [LOCAL_AGENTS_MD_FILENAME, DEFAULT_AGENTS_MD_FILENAME] {
            let path = home_dir.join(candidate);
            if let Ok(contents) = std::fs::read_to_string(&path) {
                let trimmed = contents.trim();
                if !trimmed.is_empty() {
                    debug!(path = %path.display(), "loaded global AGENTS.md from home dir");
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    /// Combine configured user instructions and AGENTS.md content into a
    /// single model-visible instruction string (matching Codex's
    /// `user_instructions` method).
    ///
    /// Called once at session creation time. The result is stored on the
    /// session and reused across turns.
    pub fn user_instructions(
        &self,
        work_dir: &Path,
        home_dir: Option<&Path>,
        config_instructions: Option<&str>,
    ) -> Option<String> {
        let agents_md_docs = self.read_agents_md(work_dir);

        let mut output = String::new();

        // 1. Global instructions from home dir
        if let Some(home) = home_dir {
            if let Some(global) = Self::load_global_instructions(home) {
                output.push_str(&global);
            }
        }

        // 2. Config-level instructions
        if let Some(instructions) = config_instructions {
            if !output.is_empty() {
                output.push_str(AGENTS_MD_SEPARATOR);
            }
            output.push_str(instructions);
        }

        // 3. Project AGENTS.md docs
        if let Some(docs) = agents_md_docs {
            if !output.is_empty() {
                output.push_str(AGENTS_MD_SEPARATOR);
            }
            output.push_str(&docs);
        }

        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }

    /// Read and concatenate all AGENTS.md files from project root to cwd,
    /// respecting the global byte budget (matching Codex's `read_agents_md`).
    fn read_agents_md(&self, work_dir: &Path) -> Option<String> {
        if self.project_doc_max_bytes == 0 {
            return None;
        }

        let paths = self.discover_files(work_dir);
        if paths.is_empty() {
            return None;
        }

        let mut remaining = self.project_doc_max_bytes;
        let mut parts: Vec<String> = Vec::new();

        for path in paths {
            if remaining == 0 {
                break;
            }

            if !path.is_file() {
                continue;
            }

            let mut data = match std::fs::read(&path) {
                Ok(data) => data,
                Err(_) => continue,
            };

            if data.is_empty() {
                continue;
            }

            if data.len() > remaining {
                warn!(
                    path = %path.display(),
                    size = data.len(),
                    remaining = remaining,
                    "AGENTS.md exceeds remaining budget, truncating"
                );
                data.truncate(remaining);
            }

            let consumed = data.len();
            let text = String::from_utf8_lossy(&data).to_string();
            if !text.trim().is_empty() {
                parts.push(text);
                remaining = remaining.saturating_sub(consumed);
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Discover AGENTS.md file paths from project root to cwd.
    ///
    /// Matching Codex's approach:
    /// - Walk from cwd upward to project root, collecting dirs
    /// - Reverse to get root→cwd order
    /// - In each dir, check `AGENTS.override.md` first, then `AGENTS.md`,
    ///   then configured fallback filenames
    fn discover_files(&self, work_dir: &Path) -> Vec<PathBuf> {
        let project_root = self.find_project_root(work_dir);

        // Collect directories from cwd up to project root (matching Codex's approach)
        let search_dirs: Vec<PathBuf> = if let Some(ref root) = project_root {
            let mut dirs = Vec::new();
            let mut cursor = work_dir.to_path_buf();
            loop {
                dirs.push(cursor.clone());
                if cursor == *root {
                    break;
                }
                match cursor.parent() {
                    Some(parent) => cursor = parent.to_path_buf(),
                    None => break,
                }
            }
            dirs.reverse(); // root → cwd order
            dirs
        } else {
            vec![work_dir.to_path_buf()]
        };

        let candidate_filenames = self.candidate_filenames();
        let mut found: Vec<PathBuf> = Vec::new();

        for dir in search_dirs {
            for name in &candidate_filenames {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    found.push(candidate);
                    break; // first match in this dir wins
                }
            }
        }

        found
    }

    /// Build the ordered list of candidate filenames for each directory.
    /// Matching Codex: `AGENTS.override.md` → `AGENTS.md` → configured fallbacks.
    fn candidate_filenames(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::with_capacity(2 + self.fallback_filenames.len());
        names.push(LOCAL_AGENTS_MD_FILENAME);
        names.push(DEFAULT_AGENTS_MD_FILENAME);
        for candidate in &self.fallback_filenames {
            let candidate = candidate.as_str();
            if candidate.is_empty() {
                continue;
            }
            if !names.contains(&candidate) {
                names.push(candidate);
            }
        }
        names
    }

    /// Find project root by looking for marker files/dirs.
    /// Matching Codex: default marker is `.git` only.
    fn find_project_root(&self, start: &Path) -> Option<PathBuf> {
        let mut current = start.to_path_buf();
        loop {
            for marker in DEFAULT_PROJECT_ROOT_MARKERS {
                if current.join(marker).exists() {
                    return Some(current);
                }
            }
            if !current.pop() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_agents_md_manager_new() {
        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        assert_eq!(manager.project_doc_max_bytes, 32768);
        // Default fallback_filenames is empty (configured fallbacks only)
        assert!(manager.fallback_filenames.is_empty());
    }

    #[test]
    fn test_candidate_filenames_order() {
        // With no configured fallbacks, order is: override → AGENTS.md
        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let names = manager.candidate_filenames();
        assert_eq!(names, vec!["AGENTS.override.md", "AGENTS.md"]);
    }

    #[test]
    fn test_candidate_filenames_with_configured_fallbacks() {
        let config = AgentConfig {
            project_doc_fallback_filenames: Some(vec![
                "CLAUDE.md".to_string(),
                "CODING_GUIDELINES.md".to_string(),
            ]),
            ..Default::default()
        };
        let manager = AgentsMdManager::new(&config);
        let names = manager.candidate_filenames();
        assert_eq!(
            names,
            vec![
                "AGENTS.override.md",
                "AGENTS.md",
                "CLAUDE.md",
                "CODING_GUIDELINES.md"
            ]
        );
    }

    #[test]
    fn test_discover_files_in_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create a project root marker (.git only, matching Codex default)
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "# Project Instructions").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let files = manager.discover_files(root);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0], root.join("AGENTS.md"));
    }

    #[test]
    fn test_discover_files_override_takes_priority() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "base").unwrap();
        fs::write(root.join("AGENTS.override.md"), "override").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let files = manager.discover_files(root);

        // Override should win over AGENTS.md in same directory
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], root.join("AGENTS.override.md"));
    }

    #[test]
    fn test_user_instructions_basic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "Hello from AGENTS.md").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.user_instructions(root, None, None);

        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("Hello from AGENTS.md"));
    }

    #[test]
    fn test_user_instructions_with_config_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "Project doc").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager
            .user_instructions(root, None, Some("User config instructions"))
            .unwrap();

        assert!(instructions.contains("User config instructions"));
        assert!(instructions.contains("Project doc"));
        assert!(instructions.contains("--- project-doc ---"));
    }

    #[test]
    fn test_user_instructions_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.user_instructions(dir.path(), None, None);
        assert!(instructions.is_none());
    }

    #[test]
    fn test_global_budget_shared_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sub = root.join("src");
        fs::create_dir_all(&sub).unwrap();

        fs::create_dir(root.join(".git")).unwrap();
        // 600 bytes each, budget = 1000
        fs::write(root.join("AGENTS.md"), "x".repeat(600)).unwrap();
        fs::write(sub.join("AGENTS.md"), "y".repeat(600)).unwrap();

        let config = AgentConfig {
            project_doc_max_bytes: Some(1000),
            ..Default::default()
        };
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.user_instructions(&sub, None, None).unwrap();

        // First file (600 bytes) consumed, second file truncated to 400 bytes
        assert!(instructions.contains("xxx"));
        assert!(instructions.contains("yyy"));
        // Total content should be bounded by budget
        let total_xy: usize = instructions
            .chars()
            .filter(|c| *c == 'x' || *c == 'y')
            .count();
        assert!(total_xy <= 1000);
    }

    #[test]
    fn test_truncate_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let large_content = "x".repeat(50000);
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), &large_content).unwrap();

        let config = AgentConfig {
            project_doc_max_bytes: Some(1000),
            ..Default::default()
        };
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.user_instructions(root, None, None).unwrap();

        // Content should be truncated to budget
        assert!(instructions.len() <= 1000);
    }

    #[test]
    fn test_find_project_root_git_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sub = root.join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        // .git is the only default marker (matching Codex)
        fs::create_dir(root.join(".git")).unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let found = manager.find_project_root(&sub);

        assert_eq!(found, Some(root.to_path_buf()));
    }

    #[test]
    fn test_no_project_root_without_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Cargo.toml alone should NOT be treated as project root (Codex uses .git only)
        fs::write(root.join("Cargo.toml"), "").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let found = manager.find_project_root(root);

        // Should not find root based on Cargo.toml alone
        // (unless there's a .git somewhere above in the real filesystem)
        // This test verifies the marker list change
        assert!(found.is_none() || found.unwrap() != root.to_path_buf());
    }

    #[test]
    fn test_fallback_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "Claude instructions").unwrap();

        let config = AgentConfig {
            project_doc_fallback_filenames: Some(vec!["CLAUDE.md".to_string()]),
            ..Default::default()
        };
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.user_instructions(root, None, None).unwrap();

        assert!(instructions.contains("Claude instructions"));
    }

    #[test]
    fn test_zero_budget_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "content").unwrap();

        let config = AgentConfig {
            project_doc_max_bytes: Some(0),
            ..Default::default()
        };
        let manager = AgentsMdManager::new(&config);
        // Project docs should be None when budget is 0
        let instructions = manager.user_instructions(root, None, None);
        assert!(instructions.is_none());
    }

    #[test]
    fn test_global_instructions_from_home() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();

        fs::write(home.path().join("AGENTS.md"), "Global instructions").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager
            .user_instructions(work.path(), Some(home.path()), None)
            .unwrap();

        assert!(instructions.contains("Global instructions"));
    }

    #[test]
    fn test_override_preferred_in_home_dir() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();

        fs::write(home.path().join("AGENTS.md"), "base global").unwrap();
        fs::write(home.path().join("AGENTS.override.md"), "override global").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager
            .user_instructions(work.path(), Some(home.path()), None)
            .unwrap();

        // Override should be used, not base
        assert!(instructions.contains("override global"));
        assert!(!instructions.contains("base global"));
    }
}
