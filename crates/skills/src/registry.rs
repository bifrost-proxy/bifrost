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
        let roots: Vec<PathBuf> = store
            .roots()
            .iter()
            .map(|root| std::fs::canonicalize(&root.path).unwrap_or_else(|_| root.path.clone()))
            .collect();
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
        let normalized_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        for root in roots {
            if let Ok(rel) = normalized_path.strip_prefix(root) {
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
}
