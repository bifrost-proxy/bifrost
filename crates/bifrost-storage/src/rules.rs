use std::fs;
use std::path::PathBuf;

use bifrost_core::bifrost_file::{
    BifrostFileParser, BifrostFileWriter, RuleFileMeta as BifrostRuleFileMeta,
    RuleFileOptions as BifrostRuleFileOptions, RuleSyncMeta as BifrostRuleSyncMeta,
    RuleSyncStatus as BifrostRuleSyncStatus,
};
use bifrost_core::limits::{ensure_file_size_within_limit, MAX_RULE_FILE_BYTES};
use bifrost_core::{normalize_rule_content, BifrostError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SHARE_ENV_STATE_FILE: &str = ".share_env_state.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSyncStatus {
    #[default]
    LocalOnly,
    Synced,
    Modified,
}

impl From<RuleSyncStatus> for BifrostRuleSyncStatus {
    fn from(value: RuleSyncStatus) -> Self {
        match value {
            RuleSyncStatus::LocalOnly => Self::LocalOnly,
            RuleSyncStatus::Synced => Self::Synced,
            RuleSyncStatus::Modified => Self::Modified,
        }
    }
}

impl From<BifrostRuleSyncStatus> for RuleSyncStatus {
    fn from(value: BifrostRuleSyncStatus) -> Self {
        match value {
            BifrostRuleSyncStatus::LocalOnly => Self::LocalOnly,
            BifrostRuleSyncStatus::Synced => Self::Synced,
            BifrostRuleSyncStatus::Modified => Self::Modified,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSyncMetadata {
    #[serde(default)]
    pub rule_id: String,
    #[serde(default)]
    pub status: RuleSyncStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_updated_at: Option<String>,
}

impl Default for RuleSyncMetadata {
    fn default() -> Self {
        Self {
            rule_id: generate_rule_id(),
            status: RuleSyncStatus::LocalOnly,
            last_synced_at: None,
            last_synced_content_hash: None,
            remote_id: None,
            remote_user_id: None,
            remote_created_at: None,
            remote_updated_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleFile {
    pub name: String,
    pub content: String,
    pub enabled: bool,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_timestamp")]
    pub created_at: String,
    #[serde(default = "default_timestamp")]
    pub updated_at: String,
    #[serde(default)]
    pub sync: RuleSyncMetadata,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl RuleFile {
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            name: name.into(),
            content: content.into(),
            enabled: true,
            sort_order: 0,
            description: None,
            group: None,
            version: "1.0.0".to_string(),
            created_at: now.clone(),
            updated_at: now,
            sync: RuleSyncMetadata::default(),
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_sort_order(mut self, sort_order: i32) -> Self {
        self.sort_order = sort_order;
        self
    }

    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    pub fn with_group(mut self, group: Option<String>) -> Self {
        self.group = group;
        self
    }

    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn touch_local_change(&mut self) {
        self.touch();
        if self.sync.remote_id.is_some() {
            self.sync.status = RuleSyncStatus::Modified;
        } else {
            self.sync.status = RuleSyncStatus::LocalOnly;
            self.sync.last_synced_at = None;
            self.sync.last_synced_content_hash = None;
            self.sync.remote_id = None;
            self.sync.remote_user_id = None;
            self.sync.remote_created_at = None;
            self.sync.remote_updated_at = None;
        }
    }

    pub fn mark_synced(
        &mut self,
        remote_id: impl Into<String>,
        remote_user_id: impl Into<String>,
        remote_created_at: impl Into<String>,
        remote_updated_at: impl Into<String>,
    ) {
        self.sync = RuleSyncMetadata {
            rule_id: self.sync.rule_id.clone(),
            status: RuleSyncStatus::Synced,
            last_synced_at: Some(chrono::Utc::now().to_rfc3339()),
            last_synced_content_hash: Some(content_hash(&self.content)),
            remote_id: Some(remote_id.into()),
            remote_user_id: Some(remote_user_id.into()),
            remote_created_at: Some(remote_created_at.into()),
            remote_updated_at: Some(remote_updated_at.into()),
        };
    }

    fn to_bifrost_meta(&self) -> BifrostRuleFileMeta {
        BifrostRuleFileMeta {
            name: self.name.clone(),
            enabled: self.enabled,
            sort_order: self.sort_order,
            version: self.version.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            description: self.description.clone(),
            group: self.group.clone(),
            sync: BifrostRuleSyncMeta {
                rule_id: self.sync.rule_id.clone(),
                status: self.sync.status.into(),
                last_synced_at: self.sync.last_synced_at.clone(),
                last_synced_content_hash: self.sync.last_synced_content_hash.clone(),
                remote_id: self.sync.remote_id.clone(),
                remote_user_id: self.sync.remote_user_id.clone(),
                remote_created_at: self.sync.remote_created_at.clone(),
                remote_updated_at: self.sync.remote_updated_at.clone(),
            },
        }
    }

    fn from_bifrost(meta: BifrostRuleFileMeta, content: String) -> Self {
        Self {
            name: meta.name,
            content,
            enabled: meta.enabled,
            sort_order: meta.sort_order,
            description: meta.description,
            group: meta.group,
            version: meta.version,
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            sync: RuleSyncMetadata {
                rule_id: meta.sync.rule_id,
                status: meta.sync.status.into(),
                last_synced_at: meta.sync.last_synced_at,
                last_synced_content_hash: meta.sync.last_synced_content_hash,
                remote_id: meta.sync.remote_id,
                remote_user_id: meta.sync.remote_user_id,
                remote_created_at: meta.sync.remote_created_at,
                remote_updated_at: meta.sync.remote_updated_at,
            },
        }
    }
}

pub fn rule_reference_key(rule: &RuleFile) -> String {
    match rule
        .group
        .as_deref()
        .filter(|group| !group.trim().is_empty())
    {
        Some(group) => format!("{}/{}", group.trim(), rule.name),
        None => rule.name.clone(),
    }
}

pub fn build_rule_reference_catalog<'a>(
    rules: impl IntoIterator<Item = &'a RuleFile>,
) -> std::collections::HashMap<String, String> {
    rules
        .into_iter()
        .map(|rule| (rule_reference_key(rule), rule.content.clone()))
        .collect()
}

fn generate_rule_id() -> String {
    format!("rl_{}", Uuid::new_v4().simple())
}

pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleSummary {
    pub name: String,
    pub enabled: bool,
    pub sort_order: i32,
    pub rule_count: usize,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareEnvState {
    pub active: bool,
    pub imported_rule_name: String,
    pub requested_name: String,
    pub content_hash: String,
    pub enabled_rule_names: Vec<String>,
    pub entered_at: String,
    #[serde(default)]
    pub exit_token: String,
}

impl From<&RuleFile> for RuleSummary {
    fn from(rule: &RuleFile) -> Self {
        let rule_count = rule
            .content
            .lines()
            .filter(|l| {
                let trimmed = l.trim();
                !trimmed.is_empty() && !trimmed.starts_with('#')
            })
            .count();

        Self {
            name: rule.name.clone(),
            enabled: rule.enabled,
            sort_order: rule.sort_order,
            rule_count,
            description: rule.description.clone(),
            created_at: rule.created_at.clone(),
            updated_at: rule.updated_at.clone(),
        }
    }
}

#[derive(Deserialize)]
struct RuleSummaryWrapper {
    meta: BifrostRuleFileMeta,
    #[serde(default)]
    options: BifrostRuleFileOptions,
}

#[derive(Clone)]
pub struct RulesStorage {
    base_dir: PathBuf,
}

impl RulesStorage {
    fn encode_rule_name(name: &str) -> String {
        urlencoding::encode(name).into_owned()
    }

    pub fn new() -> Result<Self> {
        let base_dir = crate::data_dir().join("rules");
        Self::with_dir(base_dir)
    }

    pub fn with_dir(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { base_dir: dir })
    }

    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    fn rule_path(&self, name: &str) -> PathBuf {
        self.base_dir
            .join(format!("{}.bifrost", Self::encode_rule_name(name)))
    }

    fn legacy_rule_path(&self, name: &str) -> PathBuf {
        self.base_dir
            .join(format!("{}.json", Self::encode_rule_name(name)))
    }

    fn raw_rule_path(&self, name: &str) -> PathBuf {
        self.base_dir.join(format!("{}.bifrost", name))
    }

    fn raw_legacy_rule_path(&self, name: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", name))
    }

    fn share_env_state_path(&self) -> PathBuf {
        self.base_dir.join(SHARE_ENV_STATE_FILE)
    }

    pub fn load(&self, name: &str) -> Result<RuleFile> {
        let bifrost_path = self.rule_path(name);
        let legacy_path = self.legacy_rule_path(name);
        let raw_bifrost_path = self.raw_rule_path(name);
        let raw_legacy_path = self.raw_legacy_rule_path(name);

        if bifrost_path.exists() {
            ensure_file_size_within_limit(&bifrost_path, MAX_RULE_FILE_BYTES)?;
            let content = fs::read_to_string(&bifrost_path)?;
            let file = BifrostFileParser::parse_rules(&content)
                .map_err(|e| BifrostError::Parse(format!("Failed to parse rule file: {}", e)))?;
            let normalized_content = normalize_rule_content(&file.content);
            let mut rule = RuleFile::from_bifrost(file.meta, normalized_content);
            let mut should_resave = rule.content != file.content;
            should_resave |= ensure_sync_metadata(&mut rule);
            if should_resave {
                self.save(&rule)?;
            }
            Ok(rule)
        } else if raw_bifrost_path.exists() {
            ensure_file_size_within_limit(&raw_bifrost_path, MAX_RULE_FILE_BYTES)?;
            let content = fs::read_to_string(&raw_bifrost_path)?;
            let file = BifrostFileParser::parse_rules(&content)
                .map_err(|e| BifrostError::Parse(format!("Failed to parse rule file: {}", e)))?;
            let normalized_content = normalize_rule_content(&file.content);
            let mut rule = RuleFile::from_bifrost(file.meta, normalized_content);
            let mut should_resave = rule.content != file.content;
            should_resave |= ensure_sync_metadata(&mut rule);
            if should_resave {
                self.save(&rule)?;
            }
            Ok(rule)
        } else if legacy_path.exists() {
            #[derive(Deserialize)]
            struct LegacyRuleFile {
                name: String,
                content: String,
                enabled: bool,
            }
            ensure_file_size_within_limit(&legacy_path, MAX_RULE_FILE_BYTES)?;
            let content = fs::read_to_string(&legacy_path)?;
            let legacy: LegacyRuleFile = serde_json::from_str(&content).map_err(|e| {
                BifrostError::Parse(format!("Failed to parse legacy rule file: {}", e))
            })?;

            let rule = RuleFile::new(legacy.name, legacy.content).with_enabled(legacy.enabled);
            self.save(&rule)?;
            fs::remove_file(&legacy_path)?;
            Ok(rule)
        } else if raw_legacy_path.exists() {
            #[derive(Deserialize)]
            struct LegacyRuleFile {
                name: String,
                content: String,
                enabled: bool,
            }
            ensure_file_size_within_limit(&raw_legacy_path, MAX_RULE_FILE_BYTES)?;
            let content = fs::read_to_string(&raw_legacy_path)?;
            let legacy: LegacyRuleFile = serde_json::from_str(&content).map_err(|e| {
                BifrostError::Parse(format!("Failed to parse legacy rule file: {}", e))
            })?;

            let rule = RuleFile::new(legacy.name, legacy.content).with_enabled(legacy.enabled);
            self.save(&rule)?;
            fs::remove_file(&raw_legacy_path)?;
            Ok(rule)
        } else {
            Err(BifrostError::NotFound(format!("Rule '{}' not found", name)))
        }
    }

    pub fn save(&self, rule: &RuleFile) -> Result<()> {
        let path = self.rule_path(&rule.name);
        let meta = rule.to_bifrost_meta();
        let normalized_content = normalize_rule_content(&rule.content);
        let content = BifrostFileWriter::write_rules(&meta, &normalized_content);
        if path.exists() {
            ensure_file_size_within_limit(&path, MAX_RULE_FILE_BYTES)?;
            if fs::read_to_string(&path)? == content {
                return Ok(());
            }
        }
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path.extension().and_then(|s| s.to_str());
            if ext != Some("bifrost") {
                continue;
            }

            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let decoded = urlencoding::decode(stem)
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| stem.to_string());
                if !names.contains(&decoded) {
                    names.push(decoded);
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let bifrost_path = self.rule_path(name);
        let legacy_path = self.legacy_rule_path(name);
        let raw_bifrost_path = self.raw_rule_path(name);
        let raw_legacy_path = self.raw_legacy_rule_path(name);

        let exists = bifrost_path.exists()
            || legacy_path.exists()
            || raw_bifrost_path.exists()
            || raw_legacy_path.exists();
        if !exists {
            return Err(BifrostError::NotFound(format!("Rule '{}' not found", name)));
        }

        if bifrost_path.exists() {
            fs::remove_file(&bifrost_path)?;
        }
        if legacy_path.exists() {
            fs::remove_file(&legacy_path)?;
        }
        if raw_bifrost_path.exists() {
            fs::remove_file(&raw_bifrost_path)?;
        }
        if raw_legacy_path.exists() {
            fs::remove_file(&raw_legacy_path)?;
        }
        Ok(())
    }

    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        if !self.exists(old) {
            return Err(BifrostError::NotFound(format!("Rule '{}' not found", old)));
        }
        if self.exists(new) {
            return Err(BifrostError::Config(format!(
                "Rule '{}' already exists",
                new
            )));
        }

        let mut rule = self.load(old)?;
        rule.name = new.to_string();
        rule.touch_local_change();
        self.save(&rule)?;
        self.delete(old)?;
        Ok(())
    }

    pub fn exists(&self, name: &str) -> bool {
        self.rule_path(name).exists()
            || self.legacy_rule_path(name).exists()
            || self.raw_rule_path(name).exists()
            || self.raw_legacy_rule_path(name).exists()
    }

    pub fn load_all(&self) -> Result<Vec<RuleFile>> {
        let names = self.list()?;
        let mut rules = Vec::new();
        for name in names {
            match self.load(&name) {
                Ok(rule) => rules.push(rule),
                Err(e) => {
                    tracing::warn!(name = %name, error = %e, "Failed to load rule file, skipping");
                }
            }
        }
        rules.sort_by_key(|r| r.sort_order);
        Ok(rules)
    }

    pub fn load_enabled(&self) -> Result<Vec<RuleFile>> {
        let rules = self.load_all()?;
        Ok(rules.into_iter().filter(|r| r.enabled).collect())
    }

    pub fn load_all_with_subdirs(&self) -> Result<Vec<RuleFile>> {
        self.load_all_with_subdirs_filtered(None)
    }

    pub fn load_all_with_subdirs_filtered(
        &self,
        valid_subdirs: Option<&std::collections::HashSet<String>>,
    ) -> Result<Vec<RuleFile>> {
        let mut all_rules = self.load_all()?;

        if let Ok(entries) = fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let dir_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");

                    if let Some(valid) = valid_subdirs {
                        if !valid.contains(dir_name) {
                            tracing::warn!(
                                target: "bifrost_storage::rules",
                                subdir = %dir_name,
                                "skipping subdirectory not in valid group list"
                            );
                            continue;
                        }
                    }

                    if let Ok(sub_storage) = RulesStorage::with_dir(path.clone()) {
                        match sub_storage.load_all() {
                            Ok(mut sub_rules) => {
                                for rule in &mut sub_rules {
                                    if rule.group.as_deref().unwrap_or_default().trim().is_empty() {
                                        rule.group = Some(dir_name.to_string());
                                    }
                                }
                                if !sub_rules.is_empty() {
                                    tracing::debug!(
                                        target: "bifrost_storage::rules",
                                        subdir = %dir_name,
                                        count = sub_rules.len(),
                                        "loaded rules from subdirectory"
                                    );
                                }
                                all_rules.extend(sub_rules);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    target: "bifrost_storage::rules",
                                    subdir = %path.display(),
                                    error = %e,
                                    "failed to load rules from subdirectory"
                                );
                            }
                        }
                    }
                }
            }
        }

        all_rules.sort_by_key(|r| r.sort_order);
        Ok(all_rules)
    }

    pub fn load_enabled_with_subdirs(&self) -> Result<Vec<RuleFile>> {
        self.load_enabled_with_subdirs_filtered(None)
    }

    pub fn load_enabled_with_subdirs_filtered(
        &self,
        valid_subdirs: Option<&std::collections::HashSet<String>>,
    ) -> Result<Vec<RuleFile>> {
        Ok(self
            .load_all_with_subdirs_filtered(valid_subdirs)?
            .into_iter()
            .filter(|r| r.enabled)
            .collect())
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        let mut rule = self.load(name)?;
        rule.enabled = enabled;
        rule.touch_local_change();
        self.save(&rule)
    }

    pub fn set_sort_order(&self, name: &str, sort_order: i32) -> Result<()> {
        let mut rule = self.load(name)?;
        rule.sort_order = sort_order;
        rule.touch_local_change();
        self.save(&rule)
    }

    pub fn update_content(&self, name: &str, content: String) -> Result<()> {
        let mut rule = self.load(name)?;
        rule.content = content;
        rule.touch_local_change();
        self.save(&rule)
    }

    pub fn list_summaries(&self) -> Result<Vec<RuleSummary>> {
        let names = self.list()?;
        let mut summaries = Vec::new();

        for name in names {
            match self.load_summary(&name) {
                Ok(summary) => summaries.push(summary),
                Err(e) => {
                    tracing::warn!(name = %name, error = %e, "Failed to load rule summary, skipping");
                }
            }
        }

        summaries.sort_by_key(|r| r.sort_order);
        Ok(summaries)
    }

    pub fn reorder(&self, order: &[String]) -> Result<()> {
        for (i, name) in order.iter().enumerate() {
            if self.exists(name) {
                self.set_sort_order(name, i as i32)?;
            }
        }
        Ok(())
    }

    pub fn load_share_env_state(&self) -> Result<Option<ShareEnvState>> {
        let path = self.share_env_state_path();
        if !path.exists() {
            return Ok(None);
        }
        ensure_file_size_within_limit(&path, MAX_RULE_FILE_BYTES)?;
        let content = fs::read_to_string(&path)?;
        let state: ShareEnvState = serde_json::from_str(&content)
            .map_err(|e| BifrostError::Parse(format!("Failed to parse share env state: {}", e)))?;
        Ok(Some(state))
    }

    pub fn save_share_env_state(&self, state: &ShareEnvState) -> Result<()> {
        let path = self.share_env_state_path();
        let content = serde_json::to_string_pretty(state).map_err(|e| {
            BifrostError::Config(format!("Failed to serialize share env state: {}", e))
        })?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn clear_share_env_state(&self) -> Result<()> {
        let path = self.share_env_state_path();
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn ensure_sync_metadata(rule: &mut RuleFile) -> bool {
    let mut changed = false;
    if rule.sync.rule_id.trim().is_empty() {
        rule.sync.rule_id = generate_rule_id();
        changed = true;
    }
    if rule.sync.status == RuleSyncStatus::Synced
        && rule.sync.last_synced_content_hash.is_none()
        && rule.sync.remote_id.is_some()
    {
        rule.sync.last_synced_content_hash = Some(content_hash(&rule.content));
        changed = true;
    }
    changed
}

impl RulesStorage {
    fn load_summary(&self, name: &str) -> Result<RuleSummary> {
        let bifrost_path = self.rule_path(name);
        let legacy_path = self.legacy_rule_path(name);

        if bifrost_path.exists() {
            ensure_file_size_within_limit(&bifrost_path, MAX_RULE_FILE_BYTES)?;
            let content = fs::read_to_string(&bifrost_path)?;
            let raw = BifrostFileParser::parse_raw(&content)
                .map_err(|e| BifrostError::Parse(format!("Failed to parse rule file: {}", e)))?;

            let parsed: RuleSummaryWrapper = toml::from_str(&raw.meta_raw)
                .map_err(|e| BifrostError::Parse(format!("Failed to parse rule meta: {}", e)))?;

            Ok(RuleSummary {
                name: parsed.meta.name,
                enabled: parsed.meta.enabled,
                sort_order: parsed.meta.sort_order,
                rule_count: parsed.options.rule_count,
                description: parsed.meta.description,
                created_at: parsed.meta.created_at,
                updated_at: parsed.meta.updated_at,
            })
        } else if legacy_path.exists() {
            #[derive(Deserialize)]
            struct LegacyRuleFile {
                name: String,
                content: String,
                enabled: bool,
            }

            ensure_file_size_within_limit(&legacy_path, MAX_RULE_FILE_BYTES)?;
            let content = fs::read_to_string(&legacy_path)?;
            let legacy: LegacyRuleFile = serde_json::from_str(&content).map_err(|e| {
                BifrostError::Parse(format!("Failed to parse legacy rule file: {}", e))
            })?;

            Ok(RuleSummary::from(
                &RuleFile::new(legacy.name, legacy.content).with_enabled(legacy.enabled),
            ))
        } else {
            Err(BifrostError::NotFound(format!("Rule '{}' not found", name)))
        }
    }
}

impl Default for RulesStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create default RulesStorage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, RulesStorage) {
        let temp_dir = TempDir::new().unwrap();
        let storage = RulesStorage::with_dir(temp_dir.path().to_path_buf()).unwrap();
        (temp_dir, storage)
    }

    #[test]
    fn test_save_and_load() {
        let (_temp_dir, storage) = setup();
        let rule = RuleFile::new("test", "example.com resHeaders://x-test=1");
        storage.save(&rule).unwrap();

        let loaded = storage.load("test").unwrap();
        assert_eq!(loaded.name, "test");
        assert_eq!(loaded.content, "example.com resHeaders://x-test=1");
        assert!(loaded.enabled);
    }

    #[test]
    fn test_save_creates_bifrost_file() {
        let (temp_dir, storage) = setup();
        let rule = RuleFile::new("test", "example.com proxy://localhost");
        storage.save(&rule).unwrap();

        let file_path = temp_dir.path().join("test.bifrost");
        assert!(file_path.exists());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.starts_with("01 rules"));
        assert!(content.contains("[meta]"));
        assert!(content.contains("name = \"test\""));
        assert!(content.contains("---"));
    }

    #[test]
    fn test_load_not_found() {
        let (_temp_dir, storage) = setup();
        let result = storage.load("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_list() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("rule1", "content1")).unwrap();
        storage.save(&RuleFile::new("rule2", "content2")).unwrap();
        storage.save(&RuleFile::new("rule3", "content3")).unwrap();

        let names = storage.list().unwrap();
        assert_eq!(names, vec!["rule1", "rule2", "rule3"]);
    }

    #[test]
    fn test_list_empty() {
        let (_temp_dir, storage) = setup();
        let names = storage.list().unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn test_list_ignores_non_bifrost_files() {
        let (temp_dir, storage) = setup();
        storage.save(&RuleFile::new("valid", "content")).unwrap();
        fs::write(temp_dir.path().join(".group_cache"), "{\"cached\":true}").unwrap();
        fs::write(
            temp_dir.path().join(SHARE_ENV_STATE_FILE),
            r#"{"active":true,"imported_rule_name":"share/demo","requested_name":"demo","content_hash":"abc","enabled_rule_names":[],"entered_at":"2026-06-22T00:00:00Z"}"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("legacy.json"),
            r#"{"name":"legacy","content":"legacy.example.com host://127.0.0.1:4000","enabled":true}"#,
        )
        .unwrap();

        let names = storage.list().unwrap();
        assert_eq!(names, vec!["valid"]);
    }

    #[test]
    fn test_delete() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("test", "content")).unwrap();
        assert!(storage.exists("test"));

        storage.delete("test").unwrap();
        assert!(!storage.exists("test"));
    }

    #[test]
    fn test_delete_not_found() {
        let (_temp_dir, storage) = setup();
        let result = storage.delete("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_rename() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("old_name", "content")).unwrap();

        storage.rename("old_name", "new_name").unwrap();

        assert!(!storage.exists("old_name"));
        assert!(storage.exists("new_name"));
        let rule = storage.load("new_name").unwrap();
        assert_eq!(rule.name, "new_name");
        assert_eq!(rule.content, "content");
    }

    #[test]
    fn test_rename_not_found() {
        let (_temp_dir, storage) = setup();
        let result = storage.rename("nonexistent", "new_name");
        assert!(result.is_err());
    }

    #[test]
    fn test_rename_target_exists() {
        let (_temp_dir, storage) = setup();
        storage
            .save(&RuleFile::new("old_name", "content1"))
            .unwrap();
        storage
            .save(&RuleFile::new("new_name", "content2"))
            .unwrap();

        let result = storage.rename("old_name", "new_name");
        assert!(result.is_err());
    }

    #[test]
    fn test_exists() {
        let (_temp_dir, storage) = setup();
        assert!(!storage.exists("test"));

        storage.save(&RuleFile::new("test", "content")).unwrap();
        assert!(storage.exists("test"));
    }

    #[test]
    fn test_set_enabled() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("test", "content")).unwrap();

        storage.set_enabled("test", false).unwrap();
        let rule = storage.load("test").unwrap();
        assert!(!rule.enabled);

        storage.set_enabled("test", true).unwrap();
        let rule = storage.load("test").unwrap();
        assert!(rule.enabled);
    }

    #[test]
    fn test_share_env_state_roundtrip_and_clear() {
        let (_temp_dir, storage) = setup();
        let state = ShareEnvState {
            active: true,
            imported_rule_name: "share/demo".to_string(),
            requested_name: "demo".to_string(),
            content_hash: "hash".to_string(),
            enabled_rule_names: vec!["a".to_string(), "b".to_string()],
            entered_at: "2026-06-22T00:00:00Z".to_string(),
            exit_token: "exit-token".to_string(),
        };

        storage.save_share_env_state(&state).unwrap();
        assert_eq!(storage.load_share_env_state().unwrap(), Some(state));

        storage.clear_share_env_state().unwrap();
        assert_eq!(storage.load_share_env_state().unwrap(), None);
    }

    #[test]
    fn test_load_enabled() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("rule1", "content1")).unwrap();
        storage
            .save(&RuleFile::new("rule2", "content2").with_enabled(false))
            .unwrap();
        storage.save(&RuleFile::new("rule3", "content3")).unwrap();

        let enabled = storage.load_enabled().unwrap();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.iter().all(|r| r.enabled));
    }

    #[test]
    fn test_overwrite() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("test", "content1")).unwrap();
        storage.save(&RuleFile::new("test", "content2")).unwrap();

        let loaded = storage.load("test").unwrap();
        assert_eq!(loaded.content, "content2");
    }

    #[test]
    fn test_sort_order() {
        let (_temp_dir, storage) = setup();
        storage
            .save(&RuleFile::new("rule1", "c1").with_sort_order(2))
            .unwrap();
        storage
            .save(&RuleFile::new("rule2", "c2").with_sort_order(0))
            .unwrap();
        storage
            .save(&RuleFile::new("rule3", "c3").with_sort_order(1))
            .unwrap();

        let rules = storage.load_all().unwrap();
        assert_eq!(rules[0].name, "rule2");
        assert_eq!(rules[1].name, "rule3");
        assert_eq!(rules[2].name, "rule1");
    }

    #[test]
    fn test_reorder() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("a", "ca")).unwrap();
        storage.save(&RuleFile::new("b", "cb")).unwrap();
        storage.save(&RuleFile::new("c", "cc")).unwrap();

        storage
            .reorder(&["c".to_string(), "a".to_string(), "b".to_string()])
            .unwrap();

        let rules = storage.load_all().unwrap();
        assert_eq!(rules[0].name, "c");
        assert_eq!(rules[1].name, "a");
        assert_eq!(rules[2].name, "b");
    }

    #[test]
    fn test_list_summaries() {
        let (_temp_dir, storage) = setup();
        storage
            .save(
                &RuleFile::new("test", "rule1\nrule2\n# comment\n")
                    .with_description(Some("Test rules".to_string())),
            )
            .unwrap();

        let summaries = storage.list_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "test");
        assert_eq!(summaries[0].rule_count, 2);
        assert_eq!(summaries[0].description, Some("Test rules".to_string()));
    }

    #[test]
    fn test_load_all_ignores_non_bifrost_files() {
        let (temp_dir, storage) = setup();
        storage
            .save(&RuleFile::new("valid", "example.com host://127.0.0.1:3000"))
            .unwrap();
        fs::write(temp_dir.path().join(".group_cache"), "{\"cached\":true}").unwrap();

        let legacy_rule = r#"{"name":"legacy","content":"broken.example.com host://127.0.0.1:4000","enabled":true}"#;
        fs::write(temp_dir.path().join("legacy.json"), legacy_rule).unwrap();

        let rules = storage.load_all().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "valid");
    }

    #[test]
    fn test_list_summaries_ignores_non_bifrost_files() {
        let (temp_dir, storage) = setup();
        storage
            .save(&RuleFile::new("valid", "example.com host://127.0.0.1:3000"))
            .unwrap();
        fs::write(temp_dir.path().join(".group_cache"), "{\"cached\":true}").unwrap();

        let legacy_rule = r#"{"name":"legacy","content":"broken.example.com host://127.0.0.1:4000","enabled":true}"#;
        fs::write(temp_dir.path().join("legacy.json"), legacy_rule).unwrap();

        let summaries = storage.list_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "valid");
    }

    #[test]
    fn test_load_enabled_with_subdirs_keeps_group_directories() {
        let (temp_dir, storage) = setup();
        storage
            .save(&RuleFile::new(
                "local",
                "local.example.com host://127.0.0.1:3000",
            ))
            .unwrap();

        let group_dir = temp_dir.path().join("my-group");
        let group_storage = RulesStorage::with_dir(group_dir).unwrap();
        group_storage
            .save(&RuleFile::new(
                "group",
                "group.example.com host://127.0.0.1:4000",
            ))
            .unwrap();

        let enabled = storage.load_enabled_with_subdirs().unwrap();
        assert_eq!(enabled.len(), 2);
        assert_eq!(enabled[0].name, "local");
        assert_eq!(enabled[1].name, "group");
    }

    #[test]
    fn test_load_all_with_subdirs_keeps_disabled_reference_rules() {
        let (temp_dir, storage) = setup();
        storage
            .save(&RuleFile::new("enabled", "@disabled-ref"))
            .unwrap();
        storage
            .save(
                &RuleFile::new("disabled-ref", "ref.example.com statusCode://204")
                    .with_enabled(false),
            )
            .unwrap();

        let group_dir = temp_dir.path().join("my-group");
        let group_storage = RulesStorage::with_dir(group_dir).unwrap();
        group_storage
            .save(
                &RuleFile::new("group-disabled", "group.example.com statusCode://205")
                    .with_enabled(false),
            )
            .unwrap();

        let all_rules = storage.load_all_with_subdirs().unwrap();
        let mut names = all_rules
            .iter()
            .map(|rule| rule.name.clone())
            .collect::<Vec<_>>();
        names.sort();
        let mut reference_names = all_rules
            .into_iter()
            .map(|rule| rule_reference_key(&rule))
            .collect::<Vec<_>>();
        reference_names.sort();
        assert_eq!(names, vec!["disabled-ref", "enabled", "group-disabled"]);
        assert_eq!(
            reference_names,
            vec!["disabled-ref", "enabled", "my-group/group-disabled"]
        );

        let enabled = storage.load_enabled_with_subdirs().unwrap();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "enabled");
    }

    #[test]
    fn test_load_all_with_subdirs_filtered_respects_valid_subdirs() {
        use std::collections::HashSet;

        let (temp_dir, storage) = setup();

        storage
            .save(&RuleFile::new(
                "local",
                "local.example.com host://127.0.0.1:3000",
            ))
            .unwrap();

        let group_a_dir = temp_dir.path().join("group-a");
        let group_a_storage = RulesStorage::with_dir(group_a_dir).unwrap();
        group_a_storage
            .save(&RuleFile::new(
                "group-a-rule",
                "group-a.example.com host://127.0.0.1:4000",
            ))
            .unwrap();

        let group_b_dir = temp_dir.path().join("group-b");
        let group_b_storage = RulesStorage::with_dir(group_b_dir).unwrap();
        group_b_storage
            .save(&RuleFile::new(
                "group-b-rule",
                "group-b.example.com host://127.0.0.1:5000",
            ))
            .unwrap();

        let mut valid = HashSet::new();
        valid.insert("group-a".to_string());

        let all = storage
            .load_all_with_subdirs_filtered(Some(&valid))
            .unwrap();

        let names: Vec<_> = all.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"local"));
        assert!(names.contains(&"group-a-rule"));
        assert!(!names.contains(&"group-b-rule"));

        let group_a_rule = all.iter().find(|r| r.name == "group-a-rule").unwrap();
        assert_eq!(group_a_rule.group.as_deref(), Some("group-a"));
    }

    #[test]
    fn test_synced_rule_becomes_modified_after_local_change() {
        let (_temp_dir, storage) = setup();
        let mut rule = RuleFile::new("demo", "example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "remote-1",
            "user-1",
            "2026-03-20T09:00:00Z",
            "2026-03-20T10:00:00Z",
        );
        storage.save(&rule).unwrap();

        storage
            .update_content("demo", "example.com host://127.0.0.1:4000".to_string())
            .unwrap();

        let updated = storage.load("demo").unwrap();
        assert_eq!(updated.sync.status, RuleSyncStatus::Modified);
    }

    #[test]
    fn test_delete_synced_rule_removes_from_disk() {
        let (_temp_dir, storage) = setup();
        let mut rule = RuleFile::new("synced-rule", "example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "remote-1",
            "user-1",
            "2026-03-20T09:00:00Z",
            "2026-03-20T10:00:00Z",
        );
        storage.save(&rule).unwrap();

        assert!(storage.exists("synced-rule"));
        let loaded = storage.load("synced-rule").unwrap();
        assert!(loaded.sync.remote_id.is_some());

        storage.delete("synced-rule").unwrap();
        assert!(!storage.exists("synced-rule"));
    }

    #[test]
    fn test_delete_synced_rule_excluded_from_enabled_list() {
        let (_temp_dir, storage) = setup();
        let mut rule = RuleFile::new("enabled-synced", "example.com host://127.0.0.1:3000");
        rule.mark_synced(
            "remote-1",
            "user-1",
            "2026-03-20T09:00:00Z",
            "2026-03-20T10:00:00Z",
        );
        storage.save(&rule).unwrap();
        storage
            .save(&RuleFile::new("other-rule", "other.com host://127.0.0.1"))
            .unwrap();

        let enabled_before = storage.load_enabled().unwrap();
        assert_eq!(enabled_before.len(), 2);

        storage.delete("enabled-synced").unwrap();

        let enabled_after = storage.load_enabled().unwrap();
        assert_eq!(enabled_after.len(), 1);
        assert_eq!(enabled_after[0].name, "other-rule");
    }

    #[test]
    fn test_rule_file_builders_and_touch() {
        let mut rule = RuleFile::new("r", "c")
            .with_enabled(false)
            .with_sort_order(7)
            .with_description(Some("desc".to_string()))
            .with_group(Some("grp".to_string()));
        assert!(!rule.enabled);
        assert_eq!(rule.sort_order, 7);
        assert_eq!(rule.description, Some("desc".to_string()));
        assert_eq!(rule.group, Some("grp".to_string()));

        let before = rule.updated_at.clone();
        std::thread::sleep(std::time::Duration::from_millis(5));
        rule.touch();
        assert_ne!(rule.updated_at, before);
    }

    #[test]
    fn test_touch_local_change_resets_when_not_remote() {
        let mut rule = RuleFile::new("r", "c");
        // Pretend it had been marked synced previously but with no remote_id.
        rule.sync.status = RuleSyncStatus::Synced;
        rule.sync.last_synced_at = Some("x".to_string());
        rule.touch_local_change();
        assert_eq!(rule.sync.status, RuleSyncStatus::LocalOnly);
        assert!(rule.sync.last_synced_at.is_none());
    }

    #[test]
    fn test_touch_local_change_marks_modified_when_remote() {
        let mut rule = RuleFile::new("r", "c");
        rule.mark_synced("rid", "uid", "ca", "ua");
        rule.touch_local_change();
        assert_eq!(rule.sync.status, RuleSyncStatus::Modified);
        // remote_id is preserved.
        assert_eq!(rule.sync.remote_id, Some("rid".to_string()));
    }

    #[test]
    fn test_sync_status_conversions_roundtrip() {
        use bifrost_core::bifrost_file::RuleSyncStatus as B;
        for status in [
            RuleSyncStatus::LocalOnly,
            RuleSyncStatus::Synced,
            RuleSyncStatus::Modified,
        ] {
            let to_b: B = status.into();
            let back: RuleSyncStatus = to_b.into();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn test_content_hash_and_reference_key() {
        let h = content_hash("hello");
        assert!(h.starts_with("sha256:"));

        let plain = RuleFile::new("name", "c");
        assert_eq!(rule_reference_key(&plain), "name");

        let grouped = RuleFile::new("name", "c").with_group(Some(" grp ".to_string()));
        assert_eq!(rule_reference_key(&grouped), "grp/name");

        let blank_group = RuleFile::new("name", "c").with_group(Some("   ".to_string()));
        assert_eq!(rule_reference_key(&blank_group), "name");
    }

    #[test]
    fn test_build_rule_reference_catalog() {
        let rules = [
            RuleFile::new("a", "ca"),
            RuleFile::new("b", "cb").with_group(Some("g".to_string())),
        ];
        let catalog = build_rule_reference_catalog(rules.iter());
        assert_eq!(catalog.get("a"), Some(&"ca".to_string()));
        assert_eq!(catalog.get("g/b"), Some(&"cb".to_string()));
    }

    #[test]
    fn test_set_sort_order_and_update_content() {
        let (_temp_dir, storage) = setup();
        storage.save(&RuleFile::new("r", "old")).unwrap();

        storage.set_sort_order("r", 42).unwrap();
        assert_eq!(storage.load("r").unwrap().sort_order, 42);

        storage
            .update_content("r", "new content".to_string())
            .unwrap();
        assert_eq!(storage.load("r").unwrap().content, "new content");
    }

    #[test]
    fn test_load_legacy_json_migrates_to_bifrost() {
        let (temp_dir, storage) = setup();
        // Write a legacy raw .json (unencoded name) directly.
        fs::write(
            temp_dir.path().join("legacyrule.json"),
            r#"{"name":"legacyrule","content":"legacy.example.com host://127.0.0.1","enabled":false}"#,
        )
        .unwrap();

        let rule = storage.load("legacyrule").unwrap();
        assert_eq!(rule.name, "legacyrule");
        assert!(!rule.enabled);
        // After migration, the .bifrost file exists and the .json is removed.
        assert!(temp_dir.path().join("legacyrule.bifrost").exists());
        assert!(!temp_dir.path().join("legacyrule.json").exists());
    }

    #[test]
    fn test_default_rules_storage_smoke() {
        // Exercise the From<&RuleFile> for RuleSummary conversion directly.
        let rule = RuleFile::new("s", "line1\n\n# comment\nline2")
            .with_sort_order(3)
            .with_description(Some("d".to_string()));
        let summary = RuleSummary::from(&rule);
        assert_eq!(summary.name, "s");
        assert_eq!(summary.rule_count, 2);
        assert_eq!(summary.sort_order, 3);
        assert_eq!(summary.description, Some("d".to_string()));
    }

    #[test]
    fn test_ensure_sync_metadata_fills_empty_rule_id() {
        let mut rule = RuleFile::new("r", "c");
        rule.sync.rule_id = "".to_string();
        let changed = ensure_sync_metadata(&mut rule);
        assert!(changed);
        assert!(rule.sync.rule_id.starts_with("rl_"));
    }
}
