//! Memory extensions framework and safety utilities for the Bifrost Agent memory system.
//!
//! Extensions allow third-party or user-defined memory augmentation through
//! a structured directory convention:
//!   `$memory_root/extensions/<name>/instructions.md`
//!   `$memory_root/extensions/<name>/resources/*.md`
//!
//! Also provides:
//! - Symlink safety guard for memory clear operations
//! - Extension resource pruning (retention-based cleanup)

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tracing::{info, warn};

use crate::config::MemoriesConfig;

/// Default retention period for extension resources (days).
const EXTENSION_RESOURCE_RETENTION_DAYS: u64 = 30;

/// Information about a discovered memory extension.
#[derive(Debug, Clone)]
pub struct ExtensionInfo {
    /// Extension name (directory name)
    pub name: String,
    /// Path to the extension root directory
    pub root: PathBuf,
    /// Content of instructions.md (if exists)
    pub instructions: Option<String>,
    /// Paths to resource files
    pub resource_files: Vec<PathBuf>,
}

/// Discover all memory extensions under `$memory_root/extensions/`.
///
/// Each valid extension is a subdirectory containing at least an
/// `instructions.md` file. Resource files are collected from the
/// `resources/` subdirectory (only `.md` files, hidden files skipped).
///
/// Results are sorted by extension name for deterministic ordering.
pub fn discover_extensions(memory_root: &Path) -> Vec<ExtensionInfo> {
    let extensions_dir = memory_root.join("extensions");
    let entries = match fs::read_dir(&extensions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            warn!(
                "failed reading memory extensions dir {}: {err}",
                extensions_dir.display()
            );
            return Vec::new();
        }
    };

    let mut extensions: Vec<ExtensionInfo> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip hidden directories
        if name.starts_with('.') {
            continue;
        }

        let instructions_path = path.join("instructions.md");
        let instructions = match fs::read_to_string(&instructions_path) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                warn!(
                    "failed reading extension instructions {}: {err}",
                    instructions_path.display()
                );
                None
            }
        };

        let resource_files = collect_resource_files(&path.join("resources"));

        extensions.push(ExtensionInfo {
            name,
            root: path,
            instructions,
            resource_files,
        });
    }

    extensions.sort_by(|a, b| a.name.cmp(&b.name));
    extensions
}

/// Collect `.md` resource files from a resources directory, skipping hidden files.
fn collect_resource_files(resources_dir: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(resources_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            if name.starts_with('.') || !name.ends_with(".md") {
                return None;
            }
            Some(path)
        })
        .collect();

    files.sort();
    files
}

/// Build a formatted context string from all discovered extensions,
/// suitable for injection into Phase 2 consolidation prompts.
///
/// Returns an empty string if no extensions are found or none have instructions.
pub fn build_extensions_context(memory_root: &Path) -> String {
    let extensions = discover_extensions(memory_root);
    if extensions.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();

    for ext in &extensions {
        let Some(instructions) = &ext.instructions else {
            continue;
        };

        let mut section = format!("### Extension: {}\n\n{}", ext.name, instructions.trim());

        if !ext.resource_files.is_empty() {
            section.push_str("\n\nResources:");
            for resource in &ext.resource_files {
                if let Some(name) = resource.file_name().and_then(|n| n.to_str()) {
                    section.push_str(&format!("\n- {name}"));
                }
            }
        }

        parts.push(section);
    }

    if parts.is_empty() {
        return String::new();
    }

    parts.join("\n\n")
}

/// Prune expired extension resources based on file modification time.
///
/// Walks `extensions/*/resources/` directories and removes `.md` files whose
/// modification time is older than `retention_days` (defaults to 30 days).
///
/// Returns the number of files removed.
pub fn prune_expired_resources(
    memory_root: &Path,
    retention_days: Option<u64>,
) -> Result<usize, String> {
    let retention = Duration::from_secs(
        retention_days.unwrap_or(EXTENSION_RESOURCE_RETENTION_DAYS) * 24 * 60 * 60,
    );
    let now = SystemTime::now();
    let cutoff = now.checked_sub(retention).unwrap_or(SystemTime::UNIX_EPOCH);

    let extensions_dir = memory_root.join("extensions");
    let entries = match fs::read_dir(&extensions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(format!(
                "failed reading extensions dir {}: {err}",
                extensions_dir.display()
            ))
        }
    };

    let mut removed_count = 0usize;

    for entry in entries.flatten() {
        let ext_path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }

        let resources_dir = ext_path.join("resources");
        let resource_entries = match fs::read_dir(&resources_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for res_entry in resource_entries.flatten() {
            let res_path = res_entry.path();
            let res_ft = match res_entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !res_ft.is_file() {
                continue;
            }
            let Some(name) = res_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".md") {
                continue;
            }

            let modified = match fs::metadata(&res_path).and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };

            if modified < cutoff {
                match fs::remove_file(&res_path) {
                    Ok(()) => {
                        info!("pruned expired extension resource: {}", res_path.display());
                        removed_count += 1;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => {
                        warn!(
                            "failed pruning extension resource {}: {err}",
                            res_path.display()
                        );
                    }
                }
            }
        }
    }

    Ok(removed_count)
}

/// Ensure the extensions directory layout exists under the memory root.
pub fn ensure_extensions_layout(memory_root: &Path) -> Result<(), String> {
    let extensions_dir = memory_root.join("extensions");
    fs::create_dir_all(&extensions_dir)
        .map_err(|err| format!("create extensions dir {}: {err}", extensions_dir.display()))
}

// ---------------------------------------------------------------------------
// Pluggable Extension Trait & Registry
// ---------------------------------------------------------------------------

/// Specification for a tool contributed by a memory extension.
///
/// Mirrors the structure used by the Codex extension API (`ToolExecutor`)
/// but simplified for Bifrost's architecture where tools are described
/// declaratively rather than via trait-object executors.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// Unique tool name (must not collide with built-in tools).
    pub name: String,
    /// Human-readable description shown in the system prompt.
    pub description: String,
    /// JSON Schema string describing the tool's parameters.
    pub parameters_schema: Option<String>,
}

/// A prompt fragment contributed by a memory extension to the system prompt.
#[derive(Debug, Clone)]
pub struct PromptFragment {
    /// The text to inject.
    pub content: String,
    /// Ordering priority — lower values appear earlier in the prompt.
    pub priority: i32,
}

/// Trait for pluggable memory extensions.
///
/// Modeled after Codex's `ContextContributor` + `ToolContributor` +
/// `ConfigContributor` + `ThreadLifecycleContributor` but unified into a
/// single trait suitable for Bifrost's simpler runtime model.
///
/// Extensions are registered into a [`MemoryExtensionRegistry`] and invoked
/// collectively during prompt building, tool enumeration, and config changes.
#[async_trait]
pub trait MemoryExtension: Send + Sync {
    /// Unique name identifying this extension (e.g. `"code-search"`).
    fn name(&self) -> &str;

    /// Contribute prompt context fragments for the current thread.
    ///
    /// Called during system-prompt assembly. Return `None` if this extension
    /// has nothing to add for the current session/thread state.
    async fn contribute_context(&self) -> Option<PromptFragment>;

    /// Contribute additional tools available to the agent.
    ///
    /// Called during tool enumeration. Return an empty vec if no tools.
    async fn contribute_tools(&self) -> Vec<ToolSpec>;

    /// React to a configuration change.
    ///
    /// Called whenever the agent's `MemoriesConfig` is updated (e.g. via
    /// YAML reload or runtime API). Extensions can cache relevant settings.
    fn on_config_change(&self, config: &MemoriesConfig);

    /// Lifecycle hook: called when a new session/thread starts.
    ///
    /// Extensions can perform initialization (load caches, open handles).
    /// Default implementation is a no-op.
    async fn on_thread_start(&self) {
        // no-op by default
    }

    /// Lifecycle hook: called when a session/thread ends.
    ///
    /// Extensions can perform cleanup (flush caches, close handles).
    /// Default implementation is a no-op.
    async fn on_thread_end(&self) {
        // no-op by default
    }
}

/// Registry holding all active memory extensions.
///
/// Extensions are registered at agent startup and queried during prompt
/// assembly and tool enumeration. The registry itself is `Send + Sync` so
/// it can be shared across async tasks via `Arc`.
pub struct MemoryExtensionRegistry {
    extensions: Vec<Box<dyn MemoryExtension>>,
}

impl MemoryExtensionRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    /// Register an extension. Extensions are invoked in registration order.
    pub fn register(&mut self, extension: Box<dyn MemoryExtension>) {
        info!(
            extension_name = extension.name(),
            "registered memory extension"
        );
        self.extensions.push(extension);
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Whether the registry has no extensions.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    /// Collect prompt context fragments from all extensions.
    ///
    /// Fragments are returned sorted by priority (ascending).
    pub async fn collect_context(&self) -> Vec<PromptFragment> {
        let mut fragments: Vec<PromptFragment> = Vec::new();
        for ext in &self.extensions {
            if let Some(fragment) = ext.contribute_context().await {
                fragments.push(fragment);
            }
        }
        fragments.sort_by_key(|f| f.priority);
        fragments
    }

    /// Collect tools from all extensions.
    pub async fn collect_tools(&self) -> Vec<ToolSpec> {
        let mut tools: Vec<ToolSpec> = Vec::new();
        for ext in &self.extensions {
            let mut ext_tools = ext.contribute_tools().await;
            tools.append(&mut ext_tools);
        }
        tools
    }

    /// Notify all extensions of a configuration change.
    pub fn notify_config_change(&self, config: &MemoriesConfig) {
        for ext in &self.extensions {
            ext.on_config_change(config);
        }
    }

    /// Invoke `on_thread_start` on all extensions.
    pub async fn notify_thread_start(&self) {
        for ext in &self.extensions {
            ext.on_thread_start().await;
        }
    }

    /// Invoke `on_thread_end` on all extensions.
    pub async fn notify_thread_end(&self) {
        for ext in &self.extensions {
            ext.on_thread_end().await;
        }
    }

    /// Get extension names in registration order.
    pub fn extension_names(&self) -> Vec<&str> {
        self.extensions.iter().map(|e| e.name()).collect()
    }
}

impl Default for MemoryExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A file-system-based extension that reads instructions and resources from
/// the standard `$memory_root/extensions/<name>/` directory layout.
///
/// This bridges the existing directory-based discovery (`discover_extensions`)
/// into the pluggable trait system, allowing mixed use of directory-based and
/// programmatic extensions.
pub struct FsBasedExtension {
    info: ExtensionInfo,
}

impl FsBasedExtension {
    /// Wrap an already-discovered [`ExtensionInfo`] into a trait-object extension.
    pub fn from_info(info: ExtensionInfo) -> Self {
        Self { info }
    }
}

#[async_trait]
impl MemoryExtension for FsBasedExtension {
    fn name(&self) -> &str {
        &self.info.name
    }

    async fn contribute_context(&self) -> Option<PromptFragment> {
        let instructions = self.info.instructions.as_ref()?;
        let mut content = format!(
            "### Extension: {}\n\n{}",
            self.info.name,
            instructions.trim()
        );

        if !self.info.resource_files.is_empty() {
            content.push_str("\n\nResources:");
            for resource in &self.info.resource_files {
                if let Some(name) = resource.file_name().and_then(|n| n.to_str()) {
                    content.push_str(&format!("\n- {name}"));
                }
            }
        }

        Some(PromptFragment {
            content,
            priority: 100, // FS-based extensions get default priority
        })
    }

    async fn contribute_tools(&self) -> Vec<ToolSpec> {
        Vec::new() // FS-based extensions don't contribute tools
    }

    fn on_config_change(&self, _config: &MemoriesConfig) {
        // FS-based extensions are stateless w.r.t. config
    }
}

/// Build a [`MemoryExtensionRegistry`] pre-populated with all file-system
/// extensions discovered under `$memory_root/extensions/`.
///
/// This is the recommended way to initialize the registry at agent startup,
/// after which programmatic extensions can be registered via
/// [`MemoryExtensionRegistry::register`].
pub fn build_registry_from_fs(memory_root: &Path) -> MemoryExtensionRegistry {
    let discovered = discover_extensions(memory_root);
    let mut registry = MemoryExtensionRegistry::new();

    for info in discovered {
        registry.register(Box::new(FsBasedExtension::from_info(info)));
    }

    registry
}

/// Safety check: refuse to clear a memory root if it is a symlink.
///
/// Uses `symlink_metadata()` to detect symlinks without following them.
/// Returns `Err` with explanation if the path is a symlink.
/// Returns `Ok(())` if the path does not exist or is a regular directory.
pub fn check_symlink_safety(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "refusing to operate on symlinked path: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to check symlink status of {}: {err}",
            path.display()
        )),
    }
}

/// Clear all contents of a memory root directory, with symlink safety guard.
///
/// Refuses to operate on symlinked directories. Preserves the root directory
/// itself and phase-2 lock files (`.phase2.lock`).
/// After clearing, recreates the standard directory structure via
/// `ensure_extensions_layout`.
pub fn clear_memory_root_contents(memory_root: &Path) -> Result<(), String> {
    check_symlink_safety(memory_root)?;

    // Ensure root exists
    fs::create_dir_all(memory_root)
        .map_err(|err| format!("create memory root {}: {err}", memory_root.display()))?;

    let entries = fs::read_dir(memory_root)
        .map_err(|err| format!("read memory root {}: {err}", memory_root.display()))?;

    // Files to preserve during clear
    const PRESERVED_FILES: &[&str] = &[".phase2.lock"];

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if PRESERVED_FILES.contains(&name.as_str()) {
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(err) => {
                warn!("failed to get file type for {}: {err}", path.display());
                continue;
            }
        };

        let result = if file_type.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };

        if let Err(err) = result {
            warn!("failed to remove {}: {err}", path.display());
        }
    }

    // Recreate extensions layout
    ensure_extensions_layout(memory_root)?;

    info!("cleared memory root contents: {}", memory_root.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discover_extensions_empty_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        fs::create_dir_all(memory_root.join("extensions")).unwrap();

        let extensions = discover_extensions(&memory_root);
        assert!(extensions.is_empty());
    }

    #[test]
    fn discover_extensions_nonexistent_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        // Don't create anything

        let extensions = discover_extensions(&memory_root);
        assert!(extensions.is_empty());
    }

    #[test]
    fn discover_extensions_finds_extension_with_instructions_and_resources() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        let ext_dir = memory_root.join("extensions").join("my-ext");
        let resources_dir = ext_dir.join("resources");
        fs::create_dir_all(&resources_dir).unwrap();

        fs::write(ext_dir.join("instructions.md"), "Do something useful").unwrap();
        fs::write(resources_dir.join("note1.md"), "resource 1").unwrap();
        fs::write(resources_dir.join("note2.md"), "resource 2").unwrap();
        // Hidden file should be skipped
        fs::write(resources_dir.join(".hidden.md"), "hidden").unwrap();
        // Non-md file should be skipped
        fs::write(resources_dir.join("data.txt"), "text").unwrap();

        let extensions = discover_extensions(&memory_root);
        assert_eq!(extensions.len(), 1);

        let ext = &extensions[0];
        assert_eq!(ext.name, "my-ext");
        assert_eq!(ext.instructions.as_deref(), Some("Do something useful"));
        assert_eq!(ext.resource_files.len(), 2);
    }

    #[test]
    fn discover_extensions_skips_hidden_dirs() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        let hidden_ext = memory_root.join("extensions").join(".hidden-ext");
        fs::create_dir_all(&hidden_ext).unwrap();
        fs::write(hidden_ext.join("instructions.md"), "should be skipped").unwrap();

        let visible_ext = memory_root.join("extensions").join("visible");
        fs::create_dir_all(&visible_ext).unwrap();
        fs::write(visible_ext.join("instructions.md"), "visible ext").unwrap();

        let extensions = discover_extensions(&memory_root);
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].name, "visible");
    }

    #[test]
    fn build_extensions_context_empty_returns_empty_string() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        fs::create_dir_all(memory_root.join("extensions")).unwrap();

        let ctx = build_extensions_context(&memory_root);
        assert!(ctx.is_empty());
    }

    #[test]
    fn build_extensions_context_formats_correctly() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        let ext_dir = memory_root.join("extensions").join("tooling");
        let resources_dir = ext_dir.join("resources");
        fs::create_dir_all(&resources_dir).unwrap();

        fs::write(ext_dir.join("instructions.md"), "Use these tools wisely.").unwrap();
        fs::write(resources_dir.join("ref.md"), "reference").unwrap();

        let ctx = build_extensions_context(&memory_root);
        assert!(ctx.contains("### Extension: tooling"));
        assert!(ctx.contains("Use these tools wisely."));
        assert!(ctx.contains("Resources:"));
        assert!(ctx.contains("- ref.md"));
    }

    #[test]
    fn prune_expired_resources_removes_old_files() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        let ext_dir = memory_root.join("extensions").join("test-ext");
        let resources_dir = ext_dir.join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        fs::write(ext_dir.join("instructions.md"), "ext").unwrap();

        let old_file = resources_dir.join("old.md");
        fs::write(&old_file, "old content").unwrap();

        // Set modification time to 60 days ago
        let sixty_days_ago = filetime::FileTime::from_system_time(
            SystemTime::now() - Duration::from_secs(60 * 24 * 60 * 60),
        );
        filetime::set_file_mtime(&old_file, sixty_days_ago).unwrap();

        let new_file = resources_dir.join("new.md");
        fs::write(&new_file, "new content").unwrap();

        let removed = prune_expired_resources(&memory_root, Some(30)).unwrap();
        assert_eq!(removed, 1);
        assert!(!old_file.exists());
        assert!(new_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_safety_rejects_symlinks() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("important.txt"), "keep me").unwrap();

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let result = check_symlink_safety(&link);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("refusing to operate on symlinked path"));

        // Target should remain intact
        assert!(target.join("important.txt").exists());
    }

    #[test]
    fn symlink_safety_accepts_regular_dir() {
        let dir = tempdir().unwrap();
        let regular = dir.path().join("regular");
        fs::create_dir_all(&regular).unwrap();

        assert!(check_symlink_safety(&regular).is_ok());
    }

    #[test]
    fn symlink_safety_accepts_nonexistent_path() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist");

        assert!(check_symlink_safety(&nonexistent).is_ok());
    }

    #[test]
    fn clear_memory_root_contents_removes_files_and_dirs() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        fs::create_dir_all(memory_root.join("rollout_summaries")).unwrap();
        fs::write(memory_root.join("MEMORY.md"), "old memory").unwrap();
        fs::write(
            memory_root.join("rollout_summaries").join("rollout.md"),
            "old rollout",
        )
        .unwrap();

        clear_memory_root_contents(&memory_root).unwrap();

        // Root should still exist
        assert!(memory_root.exists());
        // Old contents should be gone
        assert!(!memory_root.join("MEMORY.md").exists());
        assert!(!memory_root.join("rollout_summaries").exists());
        // Extensions dir should be recreated
        assert!(memory_root.join("extensions").exists());
    }

    #[test]
    fn clear_memory_root_contents_preserves_lock_files() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        fs::create_dir_all(&memory_root).unwrap();
        fs::write(memory_root.join(".phase2.lock"), "").unwrap();
        fs::write(memory_root.join(".phase2_state.json"), "{}").unwrap();
        fs::write(memory_root.join("MEMORY.md"), "to be cleared").unwrap();

        clear_memory_root_contents(&memory_root).unwrap();

        assert!(memory_root.join(".phase2.lock").exists());
        assert!(!memory_root.join(".phase2_state.json").exists());
        assert!(!memory_root.join("MEMORY.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn clear_memory_root_contents_rejects_symlinked_root() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("outside");
        fs::create_dir_all(&target).unwrap();
        let target_file = target.join("keep.txt");
        fs::write(&target_file, "keep\n").unwrap();

        let link = dir.path().join("memory");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let result = clear_memory_root_contents(&link);
        assert!(result.is_err());
        // Target file should remain intact
        assert!(target_file.exists());
    }

    #[test]
    fn ensure_extensions_layout_creates_dir() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");

        ensure_extensions_layout(&memory_root).unwrap();
        assert!(memory_root.join("extensions").is_dir());
    }

    // --- Tests for pluggable extension trait & registry ---

    /// A minimal test extension for unit testing.
    struct TestExtension {
        ext_name: &'static str,
        context: Option<&'static str>,
        tools: Vec<ToolSpec>,
    }

    impl TestExtension {
        fn new(name: &'static str) -> Self {
            Self {
                ext_name: name,
                context: None,
                tools: Vec::new(),
            }
        }

        fn with_context(mut self, ctx: &'static str) -> Self {
            self.context = Some(ctx);
            self
        }

        fn with_tool(mut self, name: &str, desc: &str) -> Self {
            self.tools.push(ToolSpec {
                name: name.to_string(),
                description: desc.to_string(),
                parameters_schema: None,
            });
            self
        }
    }

    #[async_trait]
    impl MemoryExtension for TestExtension {
        fn name(&self) -> &str {
            self.ext_name
        }

        async fn contribute_context(&self) -> Option<PromptFragment> {
            self.context.map(|c| PromptFragment {
                content: c.to_string(),
                priority: 50,
            })
        }

        async fn contribute_tools(&self) -> Vec<ToolSpec> {
            self.tools.clone()
        }

        fn on_config_change(&self, _config: &MemoriesConfig) {}
    }

    #[test]
    fn registry_new_is_empty() {
        let registry = MemoryExtensionRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registry_register_increments_count() {
        let mut registry = MemoryExtensionRegistry::new();
        registry.register(Box::new(TestExtension::new("ext-a")));
        registry.register(Box::new(TestExtension::new("ext-b")));

        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
        assert_eq!(registry.extension_names(), vec!["ext-a", "ext-b"]);
    }

    #[tokio::test]
    async fn registry_collect_context_gathers_fragments() {
        let mut registry = MemoryExtensionRegistry::new();
        registry.register(Box::new(TestExtension::new("a").with_context("context A")));
        registry.register(Box::new(TestExtension::new("b"))); // no context
        registry.register(Box::new(TestExtension::new("c").with_context("context C")));

        let fragments = registry.collect_context().await;
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].content, "context A");
        assert_eq!(fragments[1].content, "context C");
    }

    #[tokio::test]
    async fn registry_collect_tools_aggregates_from_all_extensions() {
        let mut registry = MemoryExtensionRegistry::new();
        registry.register(Box::new(
            TestExtension::new("a").with_tool("tool1", "desc1"),
        ));
        registry.register(Box::new(
            TestExtension::new("b")
                .with_tool("tool2", "desc2")
                .with_tool("tool3", "desc3"),
        ));

        let tools = registry.collect_tools().await;
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "tool1");
        assert_eq!(tools[1].name, "tool2");
        assert_eq!(tools[2].name, "tool3");
    }

    #[tokio::test]
    async fn fs_based_extension_contributes_context_from_info() {
        let info = ExtensionInfo {
            name: "test-fs".to_string(),
            root: PathBuf::from("/tmp/fake"),
            instructions: Some("Use this wisely.".to_string()),
            resource_files: vec![PathBuf::from("/tmp/fake/resources/ref.md")],
        };

        let ext = FsBasedExtension::from_info(info);
        assert_eq!(ext.name(), "test-fs");

        let fragment = ext.contribute_context().await.unwrap();
        assert!(fragment.content.contains("### Extension: test-fs"));
        assert!(fragment.content.contains("Use this wisely."));
        assert!(fragment.content.contains("- ref.md"));
        assert_eq!(fragment.priority, 100);
    }

    #[tokio::test]
    async fn fs_based_extension_no_instructions_returns_none() {
        let info = ExtensionInfo {
            name: "no-instructions".to_string(),
            root: PathBuf::from("/tmp/fake"),
            instructions: None,
            resource_files: Vec::new(),
        };

        let ext = FsBasedExtension::from_info(info);
        assert!(ext.contribute_context().await.is_none());
    }

    #[test]
    fn build_registry_from_fs_populates_from_discovered_extensions() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        let ext_a = memory_root.join("extensions").join("alpha");
        let ext_b = memory_root.join("extensions").join("beta");
        fs::create_dir_all(&ext_a).unwrap();
        fs::create_dir_all(&ext_b).unwrap();
        fs::write(ext_a.join("instructions.md"), "Alpha instructions").unwrap();
        fs::write(ext_b.join("instructions.md"), "Beta instructions").unwrap();

        let registry = build_registry_from_fs(&memory_root);
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.extension_names(), vec!["alpha", "beta"]);
    }

    #[test]
    fn build_registry_from_fs_empty_dir_returns_empty_registry() {
        let dir = tempdir().unwrap();
        let memory_root = dir.path().join("memory");
        fs::create_dir_all(memory_root.join("extensions")).unwrap();

        let registry = build_registry_from_fs(&memory_root);
        assert!(registry.is_empty());
    }
}
