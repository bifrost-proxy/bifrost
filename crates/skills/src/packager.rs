use crate::model::SkillScope;
use crate::store::{SkillDraft, SkillStore, StoreError, MANIFEST_JSON, SKILL_MD};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;

#[derive(Clone, Debug)]
pub struct SkillPackager;

impl SkillPackager {
    pub fn package(
        store: &SkillStore,
        scope: SkillScope,
        name: &str,
    ) -> Result<Vec<u8>, StoreError> {
        let record = store.read_one(scope, name)?;
        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for entry in walkdir::WalkDir::new(&record.path) {
            let entry = entry.map_err(|error| StoreError::Io(error.into()))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&record.path).unwrap_or(path);
            if rel.components().any(|c| c.as_os_str() == ".history") {
                continue;
            }
            zip.start_file(rel.to_string_lossy(), options)
                .map_err(io_error)?;
            zip.write_all(&fs::read(path)?)?;
        }
        Ok(zip.finish().map_err(io_error)?.into_inner())
    }

    pub fn import(
        store: &SkillStore,
        scope: SkillScope,
        archive: &[u8],
    ) -> Result<crate::model::SkillRecord, StoreError> {
        let temp = tempfile::tempdir()?;
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(io_error)?;
        let mut assets = Vec::new();
        let mut manifest = None;
        let mut skill_md = None;
        for index in 0..zip.len() {
            let mut file = zip.by_index(index).map_err(io_error)?;
            if !file.is_file() {
                continue;
            }
            let enclosed = file.enclosed_name().ok_or_else(|| {
                StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "zip path escape",
                ))
            })?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            if enclosed == Path::new(MANIFEST_JSON) {
                manifest = Some(parse_manifest_with_scope_default(&bytes, &scope)?);
            } else if enclosed == Path::new(SKILL_MD) {
                skill_md = Some(String::from_utf8(bytes).map_err(|error| {
                    StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
                })?);
            } else {
                assets.push((enclosed, bytes));
            }
        }
        let manifest = manifest.ok_or_else(|| StoreError::NotFound(MANIFEST_JSON.to_string()))?;
        let skill_md = skill_md.ok_or_else(|| StoreError::NotFound(SKILL_MD.to_string()))?;
        store.commit(SkillDraft {
            manifest,
            skill_md,
            draft_dir: Some(temp.path().join("import")),
            assets,
        })
    }
}

fn io_error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

fn parse_manifest_with_scope_default(
    bytes: &[u8],
    default_scope: &SkillScope,
) -> Result<crate::model::SkillManifest, StoreError> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let scope_is_valid = value
        .get("scope")
        .and_then(|scope| serde_json::from_value::<SkillScope>(scope.clone()).ok())
        .is_some();
    if !scope_is_valid {
        let object = value.as_object_mut().ok_or_else(|| {
            StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "manifest.json must be an object",
            ))
        })?;
        object.insert("scope".to_string(), serde_json::to_value(default_scope)?);
    }
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScopeRoot, SkillManifest};
    use tempfile::tempdir;

    #[test]
    fn import_preserves_manifest_scope_when_valid() {
        let repo = tempdir().unwrap();
        let user = tempdir().unwrap();
        let store = SkillStore::new(vec![
            ScopeRoot::new(SkillScope::User, user.path()),
            ScopeRoot::new(SkillScope::Repo, repo.path()),
        ]);
        let manifest = SkillManifest::minimal_inline("repo-skill", "repo", SkillScope::Repo);
        store
            .commit(SkillDraft {
                manifest,
                skill_md: "---\nname: repo-skill\n---\n# Repo".to_string(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();

        let archive = SkillPackager::package(&store, SkillScope::Repo, "repo-skill").unwrap();
        let imported = SkillPackager::import(&store, SkillScope::User, &archive).unwrap();

        assert_eq!(imported.manifest.scope, SkillScope::Repo);
        assert_eq!(imported.scope, SkillScope::Repo);
    }
    #[test]
    fn package_import_round_trip() {
        let source = tempdir().unwrap();
        let target = tempdir().unwrap();
        let source_store = SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, source.path())]);
        let target_store = SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, target.path())]);
        source_store
            .commit(SkillDraft {
                manifest: SkillManifest::minimal_inline("pkg-skill", "pkg", SkillScope::Repo),
                skill_md: "---\nname: pkg-skill\n---\n# Pkg".into(),
                draft_dir: None,
                assets: vec![("assets/example.txt".into(), b"hello".to_vec())],
            })
            .unwrap();
        let archive = SkillPackager::package(&source_store, SkillScope::Repo, "pkg-skill").unwrap();
        let imported = SkillPackager::import(&target_store, SkillScope::Repo, &archive).unwrap();
        assert_eq!(imported.name, "pkg-skill");
        assert!(target.path().join("pkg-skill/assets/example.txt").is_file());
    }
}
