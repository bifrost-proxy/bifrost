use crate::model::{Entrypoint, ShellKind, SkillManifest, ToolBinding, TriggerRule};
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, thiserror::Error)]
#[error("skill validation failed")]
pub struct ValidationError {
    pub issues: Vec<ValidationIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationIssue {
    pub code: String,
    pub message: String,
    pub field: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationResult {
    pub ok: bool,
    pub errors: Vec<ValidationIssue>,
    pub warnings: Vec<ValidationIssue>,
}

#[derive(Clone, Debug)]
pub struct SkillValidator {
    builtin_slash_commands: BTreeSet<String>,
    registry_tools: BTreeSet<String>,
    mcp_tools: BTreeSet<(String, String)>,
}

impl Default for SkillValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillValidator {
    pub fn new() -> Self {
        Self {
            builtin_slash_commands: [
                "/clear",
                "/reset",
                "/undo",
                "/compact",
                "/status",
                "/resume",
                "/remember",
                "/memories",
                "/forget",
                "/skill",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            registry_tools: BTreeSet::new(),
            mcp_tools: BTreeSet::new(),
        }
    }

    pub fn with_registry_tools(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.registry_tools = names.into_iter().collect();
        self
    }

    pub fn with_mcp_tools(mut self, tools: impl IntoIterator<Item = (String, String)>) -> Self {
        self.mcp_tools = tools.into_iter().collect();
        self
    }

    pub fn validate_manifest(
        &self,
        manifest: &SkillManifest,
        skill_dir: Option<&Path>,
        existing_slash: impl IntoIterator<Item = String>,
    ) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_name(&manifest.name, &mut errors);
        if Version::parse(&manifest.version).is_err() {
            errors.push(issue(
                "invalid_version",
                "version must be valid semver",
                "version",
            ));
        }
        if manifest.description.is_empty() || manifest.description.len() > 1024 {
            errors.push(issue(
                "invalid_description",
                "description length must be 1..=1024",
                "description",
            ));
        }
        self.validate_slash(manifest, existing_slash, &mut errors);
        self.validate_triggers(manifest, &mut errors);
        self.validate_tools(manifest, &mut errors);
        self.validate_schema(
            manifest.inputs_schema.as_ref(),
            "inputs_schema",
            &mut errors,
        );
        self.validate_schema(
            manifest.outputs_schema.as_ref(),
            "outputs_schema",
            &mut errors,
        );
        if let Some(dir) = skill_dir {
            self.validate_entrypoint(manifest, dir, &mut errors);
        }
        ValidationResult {
            ok: errors.is_empty(),
            errors,
            warnings: Vec::new(),
        }
    }

    pub fn validate_or_err(
        &self,
        manifest: &SkillManifest,
        skill_dir: Option<&Path>,
        existing_slash: impl IntoIterator<Item = String>,
    ) -> Result<(), ValidationError> {
        let result = self.validate_manifest(manifest, skill_dir, existing_slash);
        if result.ok {
            Ok(())
        } else {
            Err(ValidationError {
                issues: result.errors,
            })
        }
    }

    fn validate_name(&self, name: &str, errors: &mut Vec<ValidationIssue>) {
        let re = Regex::new(r"^[a-z][a-z0-9-]{0,63}$").expect("valid regex");
        if !re.is_match(name) {
            errors.push(issue(
                "invalid_name",
                "name must match ^[a-z][a-z0-9-]{0,63}$",
                "name",
            ));
        }
        if ["system", "agent", "memory", "tools", "admin"].contains(&name) {
            errors.push(issue("reserved_name", "name is reserved", "name"));
        }
    }

    fn validate_slash(
        &self,
        manifest: &SkillManifest,
        existing_slash: impl IntoIterator<Item = String>,
        errors: &mut Vec<ValidationIssue>,
    ) {
        let Some(command) = manifest.slash_command.as_deref() else {
            return;
        };
        let re = Regex::new(r"^/[a-z][a-z0-9-]{0,31}$").expect("valid regex");
        if !re.is_match(command) {
            errors.push(issue(
                "invalid_slash_command",
                "slash_command must match ^/[a-z][a-z0-9-]{0,31}$",
                "slash_command",
            ));
        }
        if self.builtin_slash_commands.contains(command) {
            errors.push(issue(
                "slash_command_conflict",
                "slash_command conflicts with a built-in command",
                "slash_command",
            ));
        }
        if existing_slash.into_iter().any(|item| item == command) {
            errors.push(issue(
                "slash_command_conflict",
                "slash_command already exists in this scope",
                "slash_command",
            ));
        }
    }

    fn validate_triggers(&self, manifest: &SkillManifest, errors: &mut Vec<ValidationIssue>) {
        if manifest.triggers.is_empty() {
            errors.push(issue(
                "missing_triggers",
                "at least one trigger is required",
                "triggers",
            ));
        }
        for trigger in &manifest.triggers {
            match trigger {
                TriggerRule::SlashCommand if manifest.slash_command.is_none() => {
                    errors.push(issue(
                        "slash_trigger_without_command",
                        "slash_command trigger requires slash_command",
                        "triggers",
                    ))
                }
                TriggerRule::Keyword { any_of } if any_of.is_empty() => errors.push(issue(
                    "empty_keyword_trigger",
                    "keyword trigger requires at least one keyword",
                    "triggers",
                )),
                TriggerRule::Regex { pattern } if Regex::new(pattern).is_err() => {
                    errors.push(issue(
                        "invalid_regex_trigger",
                        "regex trigger is invalid",
                        "triggers",
                    ))
                }
                _ => {}
            }
        }
    }

    fn validate_tools(&self, manifest: &SkillManifest, errors: &mut Vec<ValidationIssue>) {
        for tool in &manifest.allowed_tools {
            match tool {
                ToolBinding::Registry { name }
                    if !self.registry_tools.is_empty() && !self.registry_tools.contains(name) =>
                {
                    errors.push(issue(
                        "unknown_registry_tool",
                        "registry tool is not available",
                        "allowed_tools",
                    ));
                }
                ToolBinding::Mcp { server, tool }
                    if !self.mcp_tools.is_empty()
                        && !self.mcp_tools.contains(&(server.clone(), tool.clone())) =>
                {
                    errors.push(issue(
                        "unknown_mcp_tool",
                        "mcp tool is not available",
                        "allowed_tools",
                    ));
                }
                ToolBinding::Owned { input_schema, .. } => {
                    self.validate_schema(Some(input_schema), "allowed_tools.input_schema", errors);
                }
                _ => {}
            }
        }
    }

    fn validate_schema(
        &self,
        schema: Option<&serde_json::Value>,
        field: &str,
        errors: &mut Vec<ValidationIssue>,
    ) {
        let Some(schema) = schema else {
            return;
        };
        if !schema.is_object() && !schema.is_boolean() {
            errors.push(issue(
                "invalid_json_schema",
                "schema must be a JSON object or boolean schema",
                field,
            ));
        }
    }

    fn validate_entrypoint(
        &self,
        manifest: &SkillManifest,
        skill_dir: &Path,
        errors: &mut Vec<ValidationIssue>,
    ) {
        match &manifest.entrypoint {
            Entrypoint::Inline { .. } => {}
            Entrypoint::Shell { script, shell } => {
                self.validate_relative_existing_path(
                    script,
                    skill_dir,
                    "entrypoint.script",
                    errors,
                );
                if matches!(shell, ShellKind::PowerShell) && !cfg!(target_os = "windows") {
                    errors.push(issue(
                        "unsupported_shell",
                        "powershell shell entrypoint is only supported on Windows",
                        "entrypoint.shell",
                    ));
                }
            }
            Entrypoint::Python { script, .. } | Entrypoint::Node { script } => {
                self.validate_relative_existing_path(
                    script,
                    skill_dir,
                    "entrypoint.script",
                    errors,
                );
            }
        }
    }

    fn validate_relative_existing_path(
        &self,
        path: &Path,
        skill_dir: &Path,
        field: &str,
        errors: &mut Vec<ValidationIssue>,
    ) {
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            errors.push(issue(
                "path_escape",
                "entrypoint path must stay inside the skill directory",
                field,
            ));
            return;
        }
        let full = skill_dir.join(path);
        if !full.is_file() {
            errors.push(issue(
                "missing_entrypoint",
                "entrypoint script does not exist",
                field,
            ));
        }
    }
}

pub fn stable_checksum(skill_dir: &Path) -> Result<String, std::io::Error> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(skill_dir).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(skill_dir).unwrap_or(path);
        if rel == Path::new("manifest.json")
            || rel.components().any(|c| c.as_os_str() == ".history")
        {
            continue;
        }
        files.push(path.to_path_buf());
    }
    files.sort();
    let mut hasher = Sha256::new();
    for file in files {
        let rel = file.strip_prefix(skill_dir).unwrap_or(&file);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0]);
        const MAX_SKILL_FILE_BYTES: u64 = 64 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(&file) {
            if meta.len() > MAX_SKILL_FILE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("skill file too large: {}", file.display()),
                ));
            }
        }
        hasher.update(std::fs::read(file)?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn issue(code: &str, message: &str, field: &str) -> ValidationIssue {
    ValidationIssue {
        code: code.to_string(),
        message: message.to_string(),
        field: Some(field.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillAuthor, SkillManifest, SkillScope};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn manifest() -> SkillManifest {
        SkillManifest {
            name: "weather-lookup".to_string(),
            version: "0.1.0".to_string(),
            description: "Fetch weather".to_string(),
            scope: SkillScope::Repo,
            entrypoint: Entrypoint::Inline {
                instructions_md: "Do weather".to_string(),
            },
            allowed_tools: Vec::new(),
            slash_command: Some("/weather".to_string()),
            triggers: vec![TriggerRule::SlashCommand],
            inputs_schema: Some(serde_json::json!({"type":"object"})),
            outputs_schema: None,
            metadata: BTreeMap::new(),
            created_by: SkillAuthor::User { id: "u".into() },
            created_at_unix: 1,
            updated_at_unix: 1,
            checksum: String::new(),
            schema_version: 1,
        }
    }

    #[test]
    fn validates_positive_manifest() {
        let result = SkillValidator::new().validate_manifest(&manifest(), None, Vec::new());
        assert!(result.ok, "{:?}", result.errors);
    }

    #[test]
    fn rejects_invalid_name_and_builtin_slash() {
        let mut m = manifest();
        m.name = "BadName".to_string();
        m.slash_command = Some("/clear".to_string());
        let result = SkillValidator::new().validate_manifest(&m, None, Vec::new());
        let codes: Vec<_> = result.errors.iter().map(|e| e.code.as_str()).collect();
        assert!(codes.contains(&"invalid_name"));
        assert!(codes.contains(&"slash_command_conflict"));
    }

    #[test]
    fn rejects_path_escape_entrypoint() {
        let dir = tempdir().unwrap();
        let mut m = manifest();
        m.entrypoint = Entrypoint::Shell {
            script: "../run.sh".into(),
            shell: ShellKind::Sh,
        };
        let result = SkillValidator::new().validate_manifest(&m, Some(dir.path()), Vec::new());
        assert!(result.errors.iter().any(|e| e.code == "path_escape"));
    }

    #[test]
    fn checksum_is_stable_and_excludes_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "a").unwrap();
        std::fs::write(dir.path().join("manifest.json"), "first").unwrap();
        let a = stable_checksum(dir.path()).unwrap();
        std::fs::write(dir.path().join("manifest.json"), "second").unwrap();
        let b = stable_checksum(dir.path()).unwrap();
        assert_eq!(a, b);
    }
}
