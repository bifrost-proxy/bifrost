use crate::model::{SkillRecord, SkillScope};
use crate::store::{SkillStore, StoreError};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tracing::warn;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("notify: {0}")]
    Notify(#[from] notify::Error),
    #[error("slash command conflict: {0}")]
    SlashConflict(String),
}

#[derive(Default)]
struct Inner {
    by_name: HashMap<String, SkillRecord>,
    by_slash: HashMap<String, String>,
    enabled: HashSet<String>,
}

pub struct SkillRegistry {
    inner: Arc<RwLock<Inner>>,
    store: Arc<SkillStore>,
    watcher: RwLock<Option<RecommendedWatcher>>,
}

impl SkillRegistry {
    pub fn init(store: Arc<SkillStore>) -> Result<Self, RegistryError> {
        let registry = Self {
            inner: Arc::new(RwLock::new(Inner::default())),
            store,
            watcher: RwLock::new(None),
        };
        registry.reload_all()?;
        registry.start_watcher()?;
        Ok(registry)
    }

    pub fn without_watcher(store: Arc<SkillStore>) -> Result<Self, RegistryError> {
        let registry = Self {
            inner: Arc::new(RwLock::new(Inner::default())),
            store,
            watcher: RwLock::new(None),
        };
        registry.reload_all()?;
        Ok(registry)
    }

    pub fn store(&self) -> Arc<SkillStore> {
        Arc::clone(&self.store)
    }

    pub fn list(&self) -> Vec<SkillRecord> {
        self.inner.read().by_name.values().cloned().collect()
    }

    pub fn enabled(&self) -> Vec<SkillRecord> {
        let inner = self.inner.read();
        inner
            .enabled
            .iter()
            .filter_map(|name| inner.by_name.get(name).cloned())
            .collect()
    }

    pub fn resolve_slash(&self, cmd: &str) -> Option<SkillRecord> {
        let inner = self.inner.read();
        inner
            .by_slash
            .get(cmd)
            .and_then(|name| inner.by_name.get(name))
            .cloned()
    }

    /// List all registered slash commands from skills.
    /// Returns `(command, Option<description>)` pairs sorted by command name.
    pub fn list_slash_commands(&self) -> Vec<(String, Option<String>)> {
        let inner = self.inner.read();
        let mut cmds: Vec<_> = inner
            .by_slash
            .iter()
            .map(|(cmd, name)| {
                let desc = inner.by_name.get(name).map(|r| r.description.clone());
                (cmd.clone(), desc)
            })
            .collect();
        cmds.sort_by(|a, b| a.0.cmp(&b.0));
        cmds
    }

    pub fn get(&self, name: &str) -> Option<SkillRecord> {
        self.inner.read().by_name.get(name).cloned()
    }

    pub fn reload_one(&self, name: &str) -> Result<(), RegistryError> {
        Self::reload_one_into(&self.inner, &self.store, name)
    }

    pub fn reload_all(&self) -> Result<(), RegistryError> {
        let records = self.store.read_all()?;
        let mut by_name = HashMap::new();
        let mut by_slash = HashMap::new();
        let mut enabled = HashSet::new();
        for record in records {
            if let Some(command) = record.manifest.slash_command.clone() {
                if let Some(existing) = by_slash.insert(command.clone(), record.name.clone()) {
                    return Err(RegistryError::SlashConflict(format!(
                        "{command} used by {existing} and {}",
                        record.name
                    )));
                }
            }
            if record.enabled {
                enabled.insert(record.name.clone());
            }
            by_name.insert(record.name.clone(), record);
        }
        *self.inner.write() = Inner {
            by_name,
            by_slash,
            enabled,
        };
        Ok(())
    }

    fn start_watcher(&self) -> Result<(), RegistryError> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                let _ = tx.send(result);
            },
            Config::default().with_poll_interval(Duration::from_millis(500)),
        )?;
        for root in self.store.roots() {
            if let Err(error) = std::fs::create_dir_all(&root.path) {
                warn!(path = %root.path.display(), error = %error, "failed to create skill root");
                continue;
            }
            watcher.watch(&root.path, RecursiveMode::Recursive)?;
        }
        let inner = Arc::clone(&self.inner);
        let store = Arc::clone(&self.store);
        let mut roots = Vec::new();
        for root in store.roots() {
            roots.push(root.path.clone());
            if let Ok(canonical) = std::fs::canonicalize(&root.path) {
                if !roots.contains(&canonical) {
                    roots.push(canonical);
                }
            }
        }
        std::thread::Builder::new()
            .name("skill-registry-watch".to_string())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    match event {
                        Ok(event) => {
                            if !matches!(
                                event.kind,
                                EventKind::Create(_)
                                    | EventKind::Modify(_)
                                    | EventKind::Remove(_)
                                    | EventKind::Any
                            ) {
                                continue;
                            }
                            for slug in slugs_from_event_paths(&roots, &event.paths) {
                                if let Err(error) = Self::reload_one_into(&inner, &store, &slug) {
                                    warn!(skill = %slug, error = %error, "skill hot reload failed");
                                }
                            }
                        }
                        Err(error) => {
                            warn!(error = %error, "skill watcher event failed");
                        }
                    }
                }
            })
            .expect("start skill watcher thread");
        *self.watcher.write() = Some(watcher);
        Ok(())
    }

    fn reload_one_into(
        inner: &RwLock<Inner>,
        store: &SkillStore,
        name: &str,
    ) -> Result<(), RegistryError> {
        let records = store.read_records_for_name(name)?;
        let replacement = records.into_iter().find(|record| record.name == name);
        let mut inner = inner.write();
        let old_slash = inner
            .by_name
            .get(name)
            .and_then(|old| old.manifest.slash_command.clone());
        if let Some(command) = old_slash {
            inner.by_slash.remove(&command);
        }
        inner.by_name.remove(name);
        inner.enabled.remove(name);
        if let Some(record) = replacement {
            if let Some(command) = record.manifest.slash_command.clone() {
                if let Some(existing) = inner.by_slash.get(&command) {
                    if existing != name {
                        return Err(RegistryError::SlashConflict(format!(
                            "{command} used by {existing} and {name}"
                        )));
                    }
                }
                inner.by_slash.insert(command, record.name.clone());
            }
            if record.enabled {
                inner.enabled.insert(record.name.clone());
            }
            inner.by_name.insert(record.name.clone(), record);
        }
        Ok(())
    }
}

fn slugs_from_event_paths(roots: &[PathBuf], paths: &[PathBuf]) -> HashSet<String> {
    let mut slugs = HashSet::new();
    for path in paths {
        let mut path_candidates = vec![path.clone()];
        if let Ok(canonical) = std::fs::canonicalize(path) {
            if !path_candidates.contains(&canonical) {
                path_candidates.push(canonical);
            }
        }
        for root in roots {
            for candidate in &path_candidates {
                if let Ok(rel) = candidate.strip_prefix(root) {
                    if let Some(first) = rel.components().next() {
                        if let Some(slug) = first.as_os_str().to_str() {
                            if !slug.is_empty() && slug != ".history" {
                                slugs.insert(slug.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    slugs
}

impl std::fmt::Debug for SkillRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillRegistry")
            .field("skills", &self.inner.read().by_name.len())
            .finish()
    }
}

/// Build default skill roots aligned with Bifrost data directory layout.
///
/// - System: `~/.bifrost/agent/skills/.system` (embedded/cached system skills)
/// - Global: `~/.agents/skills/` (cross-agent shared, Codex-compatible)
/// - User: `~/.bifrost/agent/skills/` (user-created skills)
/// - Repo: `<work_dir>/.agents/skills/`
pub fn default_roots(user_home: PathBuf, work_dir: PathBuf) -> Vec<crate::model::ScopeRoot> {
    vec![
        // System skills (lowest priority): cached under user's bifrost data.
        crate::model::ScopeRoot::new(SkillScope::System, system_skills_cache_dir(&user_home)),
        // Global skills: ~/.agents/skills/ (cross-agent shared, Codex-compatible).
        crate::model::ScopeRoot::new(SkillScope::Global, user_home.join(".agents/skills")),
        // User skills: ~/.bifrost/agent/skills/ (user-created, bifrost-specific).
        crate::model::ScopeRoot::new(SkillScope::User, user_home.join(".bifrost/agent/skills")),
        // Repo skills: <work_dir>/.agents/skills/ (highest priority).
        crate::model::ScopeRoot::new(SkillScope::Repo, work_dir.join(".agents/skills")),
    ]
}

/// Return the cache directory for embedded system skills.
///
/// Layout: `~/.bifrost/agent/skills/.system/`
/// System skills are installed here at startup and have the lowest priority,
/// allowing user and repo skills to override them.
pub fn system_skills_cache_dir(user_home: &Path) -> PathBuf {
    user_home
        .join(".bifrost")
        .join("agent")
        .join("skills")
        .join(".system")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScopeRoot, SkillManifest, SkillScope, TriggerRule};
    use crate::store::{SkillDraft, SkillStore};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[test]
    fn resolves_enabled_slash() {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        let mut manifest = SkillManifest::minimal_inline("weather", "weather", SkillScope::Repo);
        manifest.slash_command = Some("/weather".into());
        manifest.triggers = vec![TriggerRule::SlashCommand];
        store
            .commit(SkillDraft {
                manifest,
                skill_md: "---\nname: weather\nslash_command: /weather\n---\n# Weather".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        let registry = SkillRegistry::without_watcher(store).unwrap();
        assert_eq!(registry.resolve_slash("/weather").unwrap().name, "weather");
        assert_eq!(registry.enabled().len(), 1);
    }

    #[test]
    fn watcher_reloads_one_slug_and_removes_deleted_slug() {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        commit_skill(&store, "weather", "weather");
        commit_skill(&store, "notes", "notes");
        let registry = SkillRegistry::init(Arc::clone(&store)).unwrap();
        assert_eq!(registry.get("notes").unwrap().description, "notes");
        std::thread::sleep(Duration::from_millis(300));

        fs::write(
            dir.path().join("weather").join("SKILL.md"),
            "---\nname: weather\ndescription: updated weather\n---\n# Weather",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        wait_for(Duration::from_secs(2), || {
            registry
                .get("weather")
                .is_some_and(|record| record.description == "updated weather")
        });
        assert_eq!(registry.get("notes").unwrap().description, "notes");

        fs::remove_dir_all(dir.path().join("weather")).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        wait_for(Duration::from_secs(2), || registry.get("weather").is_none());
        assert!(registry.get("notes").is_some());
    }

    fn commit_skill(store: &SkillStore, name: &str, description: &str) {
        let manifest = SkillManifest::minimal_inline(name, description, SkillScope::Repo);
        store
            .commit(SkillDraft {
                manifest,
                skill_md: format!("---\nname: {name}\ndescription: {description}\n---\n# {name}"),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
    }

    #[test]
    fn reload_all_detects_slash_conflict_between_skills() {
        let dir = tempdir().unwrap();
        let root = ScopeRoot::new(SkillScope::Repo, dir.path());
        // two skills with same slash command in filesystem, bypassing validator
        for name in ["first", "second"] {
            let skill_dir = dir.path().join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\nslash_command: /dup\n---\n# {name}\n",),
            )
            .unwrap();
        }
        let store = Arc::new(SkillStore::new(vec![root]));
        let err = SkillRegistry::without_watcher(store).unwrap_err();
        assert!(matches!(err, RegistryError::SlashConflict(msg) if msg.contains("/dup")));
    }

    #[test]
    fn slugs_from_event_paths_extracts_slug_and_ignores_history() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // normal skill file
        let skill_dir = root.join("skill-one");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, "# Skill").unwrap();

        let slugs = slugs_from_event_paths(
            std::slice::from_ref(&root),
            std::slice::from_ref(&skill_file),
        );
        assert!(slugs.contains("skill-one"));

        // .history should be ignored
        let history_dir = root.join(".history");
        fs::create_dir_all(&history_dir).unwrap();
        let history_file = history_dir.join("old-skill");
        fs::write(&history_file, "old").unwrap();
        let slugs = slugs_from_event_paths(&[root], &[history_file]);
        assert!(slugs.is_empty());
    }

    #[test]
    fn list_slash_commands_returns_sorted_commands() {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        let mut manifest1 = SkillManifest::minimal_inline("a-skill", "aaa", SkillScope::Repo);
        manifest1.slash_command = Some("/b".into());
        manifest1.triggers = vec![TriggerRule::SlashCommand];
        store
            .commit(SkillDraft {
                manifest: manifest1,
                skill_md: "---\nname: a-skill\nslash_command: /b\n---\n# A".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        let mut manifest2 = SkillManifest::minimal_inline("z-skill", "zzz", SkillScope::Repo);
        manifest2.slash_command = Some("/a".into());
        manifest2.triggers = vec![TriggerRule::SlashCommand];
        store
            .commit(SkillDraft {
                manifest: manifest2,
                skill_md: "---\nname: z-skill\nslash_command: /a\n---\n# Z".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        let registry = SkillRegistry::without_watcher(store).unwrap();
        let cmds = registry.list_slash_commands();
        let cmds_only: Vec<_> = cmds.iter().map(|(cmd, _)| cmd.as_str()).collect();
        assert_eq!(cmds_only, vec!["/a", "/b"]);
    }

    #[test]
    fn default_roots_and_system_skills_cache_dir_build_expected_paths() {
        let user_home = PathBuf::from("/home/testuser");
        let work_dir = PathBuf::from("/repo/workdir");
        let roots = default_roots(user_home.clone(), work_dir.clone());
        assert_eq!(roots.len(), 4);
        assert_eq!(roots[0].scope, SkillScope::System);
        assert_eq!(roots[0].path, system_skills_cache_dir(&user_home));
        assert_eq!(roots[1].scope, SkillScope::Global);
        assert!(roots[1].path.ends_with(".agents/skills"));
        assert_eq!(roots[2].scope, SkillScope::User);
        assert!(roots[2].path.ends_with(".bifrost/agent/skills"));
        assert_eq!(roots[3].scope, SkillScope::Repo);
        assert!(roots[3].path.starts_with(&work_dir));
    }

    fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if predicate() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(predicate());
    }

    fn make_store(dir: &Path) -> Arc<SkillStore> {
        Arc::new(SkillStore::new(vec![ScopeRoot::new(SkillScope::Repo, dir)]))
    }

    #[test]
    fn list_get_and_store_accessor() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        commit_skill(&store, "alpha", "alpha desc");
        commit_skill(&store, "beta", "beta desc");
        let registry = SkillRegistry::without_watcher(Arc::clone(&store)).unwrap();
        assert_eq!(registry.list().len(), 2);
        assert_eq!(registry.get("alpha").unwrap().description, "alpha desc");
        assert!(registry.get("missing").is_none());
        // store() returns a clone of the underlying Arc.
        assert_eq!(registry.store().roots().len(), 1);
    }

    #[test]
    fn list_slash_commands_sorted() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        for (n, s) in [("zeta", "/zeta"), ("alpha", "/alpha")] {
            let mut m = SkillManifest::minimal_inline(n, n, SkillScope::Repo);
            m.slash_command = Some(s.to_string());
            m.triggers = vec![TriggerRule::SlashCommand];
            store
                .commit(SkillDraft {
                    manifest: m,
                    skill_md: format!("---\nname: {n}\nslash_command: {s}\n---\n# {n}"),
                    draft_dir: None,
                    assets: Vec::new(),
                })
                .unwrap();
        }
        let registry = SkillRegistry::without_watcher(store).unwrap();
        let cmds = registry.list_slash_commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].0, "/alpha");
        assert_eq!(cmds[1].0, "/zeta");
        assert!(cmds[0].1.is_some());
    }

    #[test]
    fn reload_one_picks_up_new_and_removes_deleted() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        commit_skill(&store, "gamma", "gamma");
        let registry = SkillRegistry::without_watcher(Arc::clone(&store)).unwrap();
        assert!(registry.get("gamma").is_some());

        // Add a new skill directly, then reload just that slug.
        commit_skill(&store, "delta", "delta");
        registry.reload_one("delta").unwrap();
        assert!(registry.get("delta").is_some());

        // Remove gamma's directory and reload it -> dropped from registry.
        fs::remove_dir_all(dir.path().join("gamma")).unwrap();
        registry.reload_one("gamma").unwrap();
        assert!(registry.get("gamma").is_none());
    }

    #[test]
    fn reload_all_detects_slash_conflict() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        // Write two skills directly to disk that both declare the same slash
        // command. commit() would reject the second one, so we bypass it and
        // let reload_all() surface the conflict.
        for n in ["one", "two"] {
            let skill_dir = dir.path().join(n);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {n}\nslash_command: /dup\n---\n# {n}"),
            )
            .unwrap();
        }
        let err = SkillRegistry::without_watcher(store).unwrap_err();
        assert!(matches!(err, RegistryError::SlashConflict(_)));
    }

    #[test]
    fn debug_impl_reports_skill_count() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        commit_skill(&store, "solo", "solo");
        let registry = SkillRegistry::without_watcher(store).unwrap();
        let dbg = format!("{registry:?}");
        assert!(dbg.contains("SkillRegistry"));
        assert!(dbg.contains("skills: 1"));
    }

    #[test]
    fn slugs_from_event_paths_extracts_and_filters() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let skill_dir = root.join("myskill");
        fs::create_dir_all(&skill_dir).unwrap();
        let file = skill_dir.join("SKILL.md");
        fs::write(&file, "x").unwrap();
        let roots = vec![std::fs::canonicalize(&root).unwrap()];

        let slugs = slugs_from_event_paths(&roots, &[file]);
        assert!(slugs.contains("myskill"));

        // .history paths are filtered out.
        let hist = skill_dir.join(".history");
        fs::create_dir_all(&hist).unwrap();
        let hist_file = hist.join("old.tar.zst");
        fs::write(&hist_file, "x").unwrap();
        let slugs2 = slugs_from_event_paths(&roots, &[hist_file]);
        assert!(slugs2.contains("myskill"));

        // A path outside any root yields nothing.
        let outside = slugs_from_event_paths(&roots, &[PathBuf::from("/tmp/elsewhere/x")]);
        assert!(outside.is_empty());
    }

    #[test]
    fn default_roots_and_system_cache_layout() {
        let home = PathBuf::from("/home/u");
        let work = PathBuf::from("/work/repo");
        let roots = default_roots(home.clone(), work.clone());
        assert_eq!(roots.len(), 4);
        assert_eq!(roots[0].scope, SkillScope::System);
        assert_eq!(roots[3].scope, SkillScope::Repo);
        assert_eq!(roots[3].path, work.join(".agents/skills"));
        assert_eq!(
            system_skills_cache_dir(&home),
            home.join(".bifrost/agent/skills/.system")
        );
    }
}
