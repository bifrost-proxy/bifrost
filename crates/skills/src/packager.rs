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
                let mut parsed: crate::model::SkillManifest = serde_json::from_slice(&bytes)?;
                parsed.scope = scope.clone();
                manifest = Some(parsed);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScopeRoot, SkillManifest};
    use tempfile::tempdir;

    #[test]
    fn package_import_round_trip() {
        let source = tempdir().unwrap();
        let target = tempdir().unwrap();
        let source_store =
            SkillStore::new(vec![ScopeRoot::new(SkillScope::Project, source.path())]);
        let target_store =
            SkillStore::new(vec![ScopeRoot::new(SkillScope::Project, target.path())]);
        source_store
            .commit(SkillDraft {
                manifest: SkillManifest::minimal_inline("pkg-skill", "pkg", SkillScope::Project),
                skill_md: "---\nname: pkg-skill\n---\n# Pkg".into(),
                draft_dir: None,
                assets: vec![("assets/example.txt".into(), b"hello".to_vec())],
            })
            .unwrap();
        let archive =
            SkillPackager::package(&source_store, SkillScope::Project, "pkg-skill").unwrap();
        let imported = SkillPackager::import(&target_store, SkillScope::Project, &archive).unwrap();
        assert_eq!(imported.name, "pkg-skill");
        assert!(target.path().join("pkg-skill/assets/example.txt").is_file());
    }
}
