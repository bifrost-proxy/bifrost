//! AGENTS.md loading and discovery system.
//!
//! Discovers and loads AGENTS.md files from:
//! 1. Agent home directory (`~/.bifrost-agent/AGENTS.md`)
//! 2. Project root directory hierarchy (from root to cwd)
//! 3. Local override file (`AGENTS.override.md`)

use crate::config::AgentConfig;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

pub const DEFAULT_AGENTS_MD_FILENAME: &str = "AGENTS.md";
pub const LOCAL_AGENTS_MD_FILENAME: &str = "AGENTS.override.md";
pub const AGENTS_MD_MAX_BYTES: usize = 32 * 1024; // 32 KiB

/// Markers used to detect project root.
const PROJECT_ROOT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    ".hg",
    "Makefile",
];

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
            .unwrap_or_else(|| {
                vec![
                    DEFAULT_AGENTS_MD_FILENAME.to_string(),
                    "agents.md".to_string(),
                    "CLAUDE.md".to_string(),
                    "CODING_GUIDELINES.md".to_string(),
                ]
            });

        Self {
            project_doc_max_bytes: config.get_project_doc_max_bytes(),
            fallback_filenames,
        }
    }

    /// Load all AGENTS.md files from home and project hierarchy.
    /// Returns concatenated content with separators, or None if nothing found.
    pub fn load_instructions(&self, work_dir: &Path, home_dir: Option<&Path>) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();

        // 1. Load from agent home directory
        if let Some(home) = home_dir {
            if let Some(content) = self.load_file_from_dir(home) {
                debug!(path = %home.display(), "loaded AGENTS.md from home dir");
                parts.push(format!(
                    "# Instructions from {}\n\n{}",
                    home.display(),
                    content
                ));
            }
        }

        // 2. Load from project hierarchy (root → cwd)
        let discovered = self.discover_files(work_dir);
        for path in &discovered {
            if let Some(content) = self.read_and_truncate(path) {
                debug!(path = %path.display(), "loaded project AGENTS.md");
                parts.push(format!(
                    "# Instructions from {}\n\n{}",
                    path.display(),
                    content
                ));
            }
        }

        // 3. Load local override
        let override_path = work_dir.join(LOCAL_AGENTS_MD_FILENAME);
        if let Some(content) = self.read_and_truncate(&override_path) {
            debug!(path = %override_path.display(), "loaded AGENTS.override.md");
            parts.push(format!("# Local override instructions\n\n{}", content));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n---\n\n"))
        }
    }

    /// Discover AGENTS.md files from project root to cwd.
    fn discover_files(&self, work_dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();

        let project_root = self.find_project_root(work_dir);
        let start = project_root.as_deref().unwrap_or(work_dir);

        // Walk from project root to work_dir, collecting any AGENTS.md found
        let mut current = start.to_path_buf();
        let work_dir_canonical = work_dir.to_path_buf();

        loop {
            if let Some(found) = self.find_doc_in_dir(&current) {
                // Avoid duplicates
                if !files.contains(&found) {
                    files.push(found);
                }
            }

            if current == work_dir_canonical {
                break;
            }

            // Move towards work_dir
            // If work_dir is a subdirectory of current, go one level deeper
            if let Ok(relative) = work_dir_canonical.strip_prefix(&current) {
                if let Some(next_component) = relative.components().next() {
                    current = current.join(next_component);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        files
    }

    /// Find project root by looking for marker files/dirs.
    fn find_project_root(&self, start: &Path) -> Option<PathBuf> {
        let mut current = start.to_path_buf();
        loop {
            for marker in PROJECT_ROOT_MARKERS {
                if current.join(marker).exists() {
                    return Some(current);
                }
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// Look for a doc file in a directory using fallback filenames.
    fn find_doc_in_dir(&self, dir: &Path) -> Option<PathBuf> {
        for filename in &self.fallback_filenames {
            let path = dir.join(filename);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    /// Load a doc file from a directory using fallback filenames.
    fn load_file_from_dir(&self, dir: &Path) -> Option<String> {
        let path = self.find_doc_in_dir(dir)?;
        self.read_and_truncate(&path)
    }

    /// Read a file and truncate to max bytes if needed.
    /// Uses Vec<u8> truncate + from_utf8_lossy to avoid UTF-8 boundary panics
    /// (matching Codex's approach in agents_md.rs).
    fn read_and_truncate(&self, path: &Path) -> Option<String> {
        match std::fs::read(path) {
            Ok(data) => {
                if data.is_empty() {
                    return None;
                }
                let data = if data.len() > self.project_doc_max_bytes {
                    warn!(
                        path = %path.display(),
                        size = data.len(),
                        max = self.project_doc_max_bytes,
                        "AGENTS.md exceeds max size, truncating"
                    );
                    let mut truncated = data;
                    truncated.truncate(self.project_doc_max_bytes);
                    truncated
                } else {
                    data
                };
                Some(String::from_utf8_lossy(&data).into_owned())
            }
            Err(_) => None,
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
        assert!(manager
            .fallback_filenames
            .contains(&"AGENTS.md".to_string()));
    }

    #[test]
    fn test_discover_files_in_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create a project root marker
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        // Create an AGENTS.md
        fs::write(root.join("AGENTS.md"), "# Project Instructions").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let files = manager.discover_files(root);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0], root.join("AGENTS.md"));
    }

    #[test]
    fn test_load_instructions_basic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("AGENTS.md"), "Hello from AGENTS.md").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.load_instructions(root, None);

        assert!(instructions.is_some());
        assert!(instructions.unwrap().contains("Hello from AGENTS.md"));
    }

    #[test]
    fn test_load_instructions_with_override() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("AGENTS.md"), "Base instructions").unwrap();
        fs::write(root.join("AGENTS.override.md"), "Override instructions").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.load_instructions(root, None).unwrap();

        assert!(instructions.contains("Base instructions"));
        assert!(instructions.contains("Override instructions"));
    }

    #[test]
    fn test_load_instructions_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.load_instructions(dir.path(), None);
        assert!(instructions.is_none());
    }

    #[test]
    fn test_truncate_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let large_content = "x".repeat(50000);
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("AGENTS.md"), &large_content).unwrap();

        let config = AgentConfig {
            project_doc_max_bytes: Some(1000),
            ..Default::default()
        };
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.load_instructions(root, None).unwrap();

        // The content should be truncated (plus some header text)
        assert!(instructions.len() < 2000);
    }

    #[test]
    fn test_find_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sub = root.join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let found = manager.find_project_root(&sub);

        assert_eq!(found, Some(root.to_path_buf()));
    }

    #[test]
    fn test_fallback_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("CLAUDE.md"), "Claude instructions").unwrap();

        let config = AgentConfig::default();
        let manager = AgentsMdManager::new(&config);
        let instructions = manager.load_instructions(root, None).unwrap();

        assert!(instructions.contains("Claude instructions"));
    }
}
