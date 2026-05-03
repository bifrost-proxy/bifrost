use crate::model::{ScopeRoot, SkillManifest, SkillRecord, SkillScope};
use crate::validator::{stable_checksum, SkillValidator, ValidationError};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::warn;

pub const SKILL_MD: &str = "SKILL.md";
pub const MANIFEST_JSON: &str = "manifest.json";

/// Maximum BFS traversal depth from a skill root directory.
const MAX_SCAN_DEPTH: usize = 6;
/// Maximum number of directories to visit per root during BFS scan.
const MAX_DIRS_PER_ROOT: usize = 2000;
/// Maximum allowed length for a skill name (characters).
const MAX_NAME_LEN: usize = 64;
/// Maximum allowed length for a skill description (characters).
const MAX_DESCRIPTION_LEN: usize = 1024;

#[derive(Clone, Debug)]
pub struct SkillDraft {
    pub manifest: SkillManifest,
    pub skill_md: String,
    pub draft_dir: Option<PathBuf>,
    pub assets: Vec<(PathBuf, Vec<u8>)>,
}

#[derive(Clone, Debug)]
pub struct SkillStore {
    roots: Vec<ScopeRoot>,
    validator: SkillValidator,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("scope root not found: {0:?}")]
    MissingRoot(SkillScope),
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("validation: {0:?}")]
    Validation(Vec<crate::validator::ValidationIssue>),
}

impl From<ValidationError> for StoreError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.issues)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl SkillStore {
    pub fn new(roots: Vec<ScopeRoot>) -> Self {
        Self {
            roots,
            validator: SkillValidator::new(),
        }
    }

    pub fn with_validator(roots: Vec<ScopeRoot>, validator: SkillValidator) -> Self {
        Self { roots, validator }
    }

    pub fn roots(&self) -> &[ScopeRoot] {
        &self.roots
    }

    pub fn read_all(&self) -> Result<Vec<SkillRecord>> {
        let mut records = Vec::new();
        for root in &self.roots {
            if !root.path.is_dir() {
                continue;
            }
            // BFS scan with depth limit, matching Codex traversal behavior.
            let mut queue = std::collections::VecDeque::new();
            let mut visited = std::collections::HashSet::new();
            visited.insert(root.path.clone());
            queue.push_back((root.path.clone(), 0usize));

            while let Some((dir, depth)) = queue.pop_front() {
                let entries = match fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(e) => {
                        warn!(dir = %dir.display(), error = %e, "failed to read skills dir");
                        continue;
                    }
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let file_name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(name) => name.to_string(),
                        None => continue,
                    };

                    // Skip hidden directories (., .git, .drafts, .history, etc.)
                    if file_name.starts_with('.') {
                        continue;
                    }

                    if !path.is_dir() {
                        continue;
                    }

                    // If this directory contains SKILL.md, parse it as a skill.
                    if path.join(SKILL_MD).is_file() {
                        if let Ok(record) = self.read_record_dir(root.scope.clone(), &path) {
                            records.push(record);
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
        }
        Ok(apply_effective_scopes(records))
    }

    pub fn read_one(&self, scope: SkillScope, name: &str) -> Result<SkillRecord> {
        let root = self.root_for(&scope)?;
        self.read_record_dir(scope, &root.join(name))
    }

    pub fn commit(&self, draft: SkillDraft) -> Result<SkillRecord> {
        let root = self.root_for(&draft.manifest.scope)?;
        fs::create_dir_all(&root)?;
        let drafts_root = root.join(".drafts");
        fs::create_dir_all(&drafts_root)?;
        let session_id = format!("{}-{}", draft.manifest.name, Utc::now().timestamp_millis());
        let draft_dir = draft
            .draft_dir
            .unwrap_or_else(|| drafts_root.join(session_id).join(&draft.manifest.name));
        if draft_dir.exists() {
            fs::remove_dir_all(&draft_dir)?;
        }
        fs::create_dir_all(&draft_dir)?;

        // Update SKILL.md frontmatter with manifest fields to ensure consistency.
        let skill_md = Self::update_frontmatter(&draft.skill_md, &draft.manifest);
        fs::write(draft_dir.join(SKILL_MD), &skill_md)?;

        for (relative, bytes) in draft.assets {
            ensure_relative(&relative)?;
            let target = draft_dir.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, bytes)?;
        }
        let mut manifest = draft.manifest;
        self.validator.validate_or_err(
            &manifest,
            Some(&draft_dir),
            self.existing_slash_in_scope(&manifest),
        )?;
        manifest.updated_at_unix = Utc::now().timestamp();
        manifest.checksum = stable_checksum(&draft_dir)?;
        fs::write(
            draft_dir.join(MANIFEST_JSON),
            serde_json::to_vec_pretty(&manifest)?,
        )?;

        let final_dir = root.join(&manifest.name);
        let archived = if final_dir.exists() {
            let archive = self.archive_dir(&final_dir)?;
            fs::remove_dir_all(&final_dir)?;
            Some(archive)
        } else {
            None
        };
        fs::rename(&draft_dir, &final_dir)?;
        if let Some(archive) = archived {
            let history = final_dir.join(".history");
            fs::create_dir_all(&history)?;
            let target = history.join(
                archive
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("previous.tar.zst")),
            );
            fs::rename(archive, target)?;
        }
        self.read_record_dir(manifest.scope.clone(), &final_dir)
    }

    /// Update SKILL.md frontmatter with values from manifest.
    /// This ensures the SKILL.md remains the single source of truth.
    fn update_frontmatter(skill_md: &str, manifest: &SkillManifest) -> String {
        // Parse existing frontmatter
        let (_fm, body) = if let Some((existing, body)) = Self::split_frontmatter(skill_md) {
            (existing, body)
        } else {
            (SkillFrontmatter::default(), skill_md)
        };

        // Merge manifest fields (manifest takes precedence)
        let merged = SkillFrontmatter {
            name: Some(manifest.name.clone()),
            version: Some(manifest.version.clone()),
            description: Some(manifest.description.clone()),
            slash_command: manifest.slash_command.clone(),
        };

        // Reconstruct SKILL.md
        let mut output = String::from("---\n");
        // Use serde_yaml to serialize
        if let Ok(yaml) = serde_yaml::to_string(&merged) {
            // serde_yaml adds a leading "---\n" we need to strip
            let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
            output.push_str(yaml);
        }
        output.push_str("---\n");
        output.push_str(body);
        output
    }

    /// Split SKILL.md into frontmatter and body.
    fn split_frontmatter(content: &str) -> Option<(SkillFrontmatter, &str)> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return None;
        }
        let after_first = trimmed[3..].trim_start_matches(['\r', '\n']);
        let end_pos = after_first.find("\n---")?;
        let yaml_str = &after_first[..end_pos];
        let body_start = end_pos + 4;
        let body = after_first[body_start..].trim_start_matches(['\r', '\n']);
        serde_yaml::from_str::<SkillFrontmatter>(yaml_str)
            .ok()
            .map(|fm| (fm, body))
    }

    pub fn delete(&self, scope: SkillScope, name: &str) -> Result<()> {
        let root = self.root_for(&scope)?;
        let dir = root.join(name);
        if !dir.exists() {
            return Err(StoreError::NotFound(name.to_string()));
        }
        self.archive_dir(&dir)?;
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    pub fn enable(&self, scope: SkillScope, name: &str, enabled: bool) -> Result<()> {
        let root = self.root_for(&scope)?;
        let dir = root.join(name);
        let marker = dir.join(".disabled");
        if enabled {
            if marker.exists() {
                fs::remove_file(marker)?;
            }
        } else {
            fs::write(marker, b"disabled")?;
        }
        Ok(())
    }

    pub fn verify_checksum(&self, scope: SkillScope, name: &str) -> Result<bool> {
        let root = self.root_for(&scope)?;
        let dir = root.join(name);
        // Only meaningful if manifest.json exists (created by commit()).
        let manifest_path = dir.join(MANIFEST_JSON);
        if !manifest_path.exists() {
            warn!(
                scope = ?scope,
                name,
                path = %manifest_path.display(),
                "skill checksum manifest missing"
            );
            return Ok(false);
        }
        let manifest: SkillManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        Ok(stable_checksum(&dir)? == manifest.checksum)
    }

    /// Maximum size for a SKILL.md file we will read (2 MiB).
    const MAX_SKILL_MD_BYTES: u64 = 2 * 1024 * 1024;

    pub fn skill_md(&self, scope: SkillScope, name: &str) -> Result<String> {
        let root = self.root_for(&scope)?;
        let path = root.join(name).join(SKILL_MD);
        // Guard against OOM from unexpectedly large files.
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > Self::MAX_SKILL_MD_BYTES {
                return Err(StoreError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "SKILL.md too large ({} bytes, limit {} bytes)",
                        meta.len(),
                        Self::MAX_SKILL_MD_BYTES
                    ),
                )));
            }
        }
        Ok(fs::read_to_string(path)?)
    }

    fn read_record_dir(&self, scope: SkillScope, dir: &Path) -> Result<SkillRecord> {
        // SKILL.md frontmatter is the single source of truth (Codex-compatible).
        let manifest = manifest_from_skill_md(dir, &scope)?;
        let enabled = !dir.join(".disabled").exists();
        Ok(SkillRecord {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            scope: scope.clone(),
            effective_scope: scope,
            shadow_scopes: Vec::new(),
            enabled,
            path: dir.to_path_buf(),
            skill_md_path: dir.join(SKILL_MD),
            checksum: manifest.checksum.clone(),
            manifest,
        })
    }

    pub fn read_records_for_name(&self, name: &str) -> Result<Vec<SkillRecord>> {
        let mut records = Vec::new();
        for root in &self.roots {
            let path = root.path.join(name);
            if path.is_dir() {
                records.push(self.read_record_dir(root.scope.clone(), &path)?);
            }
        }
        Ok(apply_effective_scopes(records))
    }

    fn root_for(&self, scope: &SkillScope) -> Result<PathBuf> {
        self.roots
            .iter()
            .find(|root| &root.scope == scope)
            .map(|root| root.path.clone())
            .ok_or_else(|| StoreError::MissingRoot(scope.clone()))
    }

    fn existing_slash_in_scope(&self, manifest: &SkillManifest) -> Vec<String> {
        self.read_all()
            .unwrap_or_default()
            .into_iter()
            .filter(|record| record.scope == manifest.scope && record.name != manifest.name)
            .filter_map(|record| record.manifest.slash_command)
            .collect()
    }

    fn archive_dir(&self, dir: &Path) -> Result<PathBuf> {
        let history = dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".history");
        fs::create_dir_all(&history)?;
        let name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("skill");
        let archive = history.join(format!("{}-{}.tar.zst", name, Utc::now().timestamp()));
        let file = fs::File::create(&archive)?;
        let encoder = zstd::Encoder::new(file, 0)?;
        let mut tar = tar::Builder::new(encoder);
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() == ".history" {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                tar.append_dir_all(entry.file_name(), entry.path())?;
            } else if file_type.is_file() {
                tar.append_path_with_name(entry.path(), entry.file_name())?;
            }
        }
        let encoder = tar.into_inner()?;
        encoder.finish()?;
        Ok(archive)
    }
}

/// YAML frontmatter struct used in SKILL.md files (Codex-compatible standard).
#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct SkillFrontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slash_command: Option<String>,
}

/// Build a `SkillManifest` from the SKILL.md frontmatter (Codex-compatible).
/// This is the primary skill discovery mechanism.
fn manifest_from_skill_md(dir: &Path, scope: &SkillScope) -> Result<SkillManifest> {
    use crate::model::{Entrypoint, SkillAuthor, TriggerRule};

    let skill_md_path = dir.join(SKILL_MD);
    let content = fs::read_to_string(&skill_md_path)?;

    let fm = parse_frontmatter(&content).ok_or_else(|| {
        StoreError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no valid frontmatter in {}", skill_md_path.display()),
        ))
    })?;

    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let name = sanitize_single_line(&fm.name.unwrap_or_else(|| dir_name.clone()));
    let description =
        sanitize_single_line(&fm.description.unwrap_or_else(|| format!("Skill: {}", name)));
    let version = fm.version.unwrap_or_else(|| "0.1.0".to_string());

    // Validate field lengths (aligned with Codex limits).
    if name.chars().count() > MAX_NAME_LEN {
        return Err(StoreError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "skill name too long ({} chars, max {}): {}",
                name.chars().count(),
                MAX_NAME_LEN,
                skill_md_path.display()
            ),
        )));
    }
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(StoreError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "skill description too long ({} chars, max {}): {}",
                description.chars().count(),
                MAX_DESCRIPTION_LEN,
                skill_md_path.display()
            ),
        )));
    }

    let mut triggers = vec![TriggerRule::DescriptionMatch];
    if fm.slash_command.is_some() {
        triggers.push(TriggerRule::SlashCommand);
    }

    Ok(SkillManifest {
        name,
        version,
        description,
        scope: scope.clone(),
        entrypoint: Entrypoint::Inline {
            instructions_md: String::new(),
        },
        allowed_tools: Vec::new(),
        slash_command: fm.slash_command,
        triggers,
        inputs_schema: None,
        outputs_schema: None,
        metadata: BTreeMap::new(),
        env: BTreeMap::new(),
        created_by: SkillAuthor::Imported {
            origin: "skill-md".to_string(),
        },
        created_at_unix: 0,
        updated_at_unix: 0,
        checksum: String::new(),
        schema_version: 1,
    })
}

/// Parse YAML frontmatter from a SKILL.md file.
/// Frontmatter is delimited by `---` lines at the start of the file.
fn parse_frontmatter(content: &str) -> Option<SkillFrontmatter> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = trimmed[3..].trim_start_matches(['\r', '\n']);
    let end_pos = after_first.find("\n---")?;
    let yaml_str = &after_first[..end_pos];
    match serde_yaml::from_str::<SkillFrontmatter>(yaml_str) {
        Ok(fm) => Some(fm),
        Err(e) => {
            warn!(error = %e, "failed to parse SKILL.md frontmatter");
            None
        }
    }
}

/// Collapse all whitespace (newlines, tabs, runs of spaces) into a single space.
/// Matches Codex's `sanitize_single_line` behavior.
fn sanitize_single_line(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ensure_relative(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(StoreError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "asset path escapes skill directory",
        )));
    }
    Ok(())
}

pub fn apply_effective_scopes(mut records: Vec<SkillRecord>) -> Vec<SkillRecord> {
    records.sort_by_key(|record| (record.name.clone(), record.scope.priority()));
    let mut grouped: std::collections::BTreeMap<String, Vec<SkillRecord>> =
        std::collections::BTreeMap::new();
    for record in records {
        grouped.entry(record.name.clone()).or_default().push(record);
    }
    let mut effective = Vec::new();
    for (_, mut group) in grouped {
        group.sort_by_key(|record| record.scope.priority());
        let mut winner = group.pop().expect("non-empty group");
        winner.effective_scope = winner.scope.clone();
        winner.shadow_scopes = group.into_iter().map(|record| record.scope).collect();
        effective.push(winner);
    }
    effective
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entrypoint, SkillManifest, SkillScope};
    use tempfile::tempdir;

    #[test]
    fn commit_archives_previous_and_verifies_checksum() {
        let dir = tempdir().unwrap();
        let store = SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, dir.path())]);
        let manifest = SkillManifest::minimal_inline("demo-skill", "demo", SkillScope::Repo);
        let record = store
            .commit(SkillDraft {
                manifest: manifest.clone(),
                skill_md: "---\nname: demo-skill\n---\n# Demo".to_string(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        assert_eq!(record.name, "demo-skill");
        assert!(store
            .verify_checksum(SkillScope::Repo, "demo-skill")
            .unwrap());
        let mut next = manifest;
        next.entrypoint = Entrypoint::Inline {
            instructions_md: "changed".into(),
        };
        store
            .commit(SkillDraft {
                manifest: next,
                skill_md: "---\nname: demo-skill\n---\n# Demo 2".to_string(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        assert!(dir.path().join("demo-skill/.history").is_dir());
    }

    #[test]
    fn verify_checksum_missing_manifest_returns_false() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("missing-manifest");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join(SKILL_MD),
            "---\nname: missing-manifest\n---\n# Missing",
        )
        .unwrap();
        let store = SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, dir.path())]);

        assert!(!store
            .verify_checksum(SkillScope::Repo, "missing-manifest")
            .unwrap());
    }

    #[test]
    fn scope_overlay_prefers_repo() {
        let mut user = SkillRecord {
            manifest: SkillManifest::minimal_inline("same", "user", SkillScope::User),
            name: "same".into(),
            version: "0.1.0".into(),
            description: "user".into(),
            scope: SkillScope::User,
            effective_scope: SkillScope::User,
            shadow_scopes: Vec::new(),
            enabled: true,
            path: "g".into(),
            skill_md_path: "g/SKILL.md".into(),
            checksum: String::new(),
        };
        let mut repo = user.clone();
        repo.scope = SkillScope::Repo;
        repo.manifest.scope = SkillScope::Repo;
        user.manifest.scope = SkillScope::User;
        let records = apply_effective_scopes(vec![user, repo]);
        assert_eq!(records[0].effective_scope, SkillScope::Repo);
        assert_eq!(records[0].shadow_scopes, vec![SkillScope::User]);
    }
}
