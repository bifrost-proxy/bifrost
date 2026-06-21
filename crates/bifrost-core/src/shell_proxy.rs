use std::path::{Path, PathBuf};

use crate::{BifrostError, Result};

const BACKUP_FILE_NAME: &str = "shell_proxy_backup.json";

const START_MARKER: &str = "# >>> Bifrost proxy start >>>";
const END_MARKER: &str = "# <<< Bifrost proxy end <<<";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellType {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
    Unknown,
}

impl ShellType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShellType::Bash => "bash",
            ShellType::Zsh => "zsh",
            ShellType::Fish => "fish",
            ShellType::PowerShell => "powershell",
            ShellType::Cmd => "cmd",
            ShellType::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShellProxyBackupFile {
    pub path: String,
    pub original_content: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShellProxyBackup {
    pub shell_type: String,
    #[serde(default)]
    pub files: Vec<ShellProxyBackupFile>,
    #[serde(default)]
    pub config_path: Option<String>,
    #[serde(default)]
    pub original_content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ShellProxyStatus {
    pub shell_type: ShellType,
    pub config_paths: Vec<PathBuf>,
    pub has_persistent_config: bool,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
}

pub struct ShellProxyManager {
    data_dir: PathBuf,
    shell_type: ShellType,
    config_paths: Vec<PathBuf>,
}

impl ShellProxyManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let shell_type = Self::detect_shell();
        let config_paths = Self::get_config_paths(shell_type);
        Self {
            data_dir,
            shell_type,
            config_paths,
        }
    }

    pub fn detect_shell() -> ShellType {
        if let Ok(shell) = std::env::var("SHELL") {
            if shell.contains("zsh") {
                return ShellType::Zsh;
            } else if shell.contains("bash") {
                return ShellType::Bash;
            } else if shell.contains("fish") {
                return ShellType::Fish;
            }
        }

        if std::env::var("PSModulePath").is_ok() || std::env::var("PROFILE").is_ok() {
            return ShellType::PowerShell;
        }

        if cfg!(windows) && std::env::var("COMSPEC").is_ok() {
            return ShellType::Cmd;
        }

        ShellType::Unknown
    }

    fn get_config_paths(shell_type: ShellType) -> Vec<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        let Some(home) = home else {
            return Vec::new();
        };

        match shell_type {
            ShellType::Bash => vec![home.join(".bashrc"), home.join(".bash_profile")],
            ShellType::Zsh => vec![home.join(".zshrc"), home.join(".zprofile")],
            ShellType::Fish => vec![home.join(".config").join("fish").join("config.fish")],
            ShellType::PowerShell | ShellType::Cmd | ShellType::Unknown => Vec::new(),
        }
    }

    pub fn shell_type(&self) -> ShellType {
        self.shell_type
    }

    pub fn config_paths(&self) -> &[PathBuf] {
        &self.config_paths
    }

    pub fn enable_temporary(host: &str, port: u16, bypass: &str) -> String {
        let shell_type = Self::detect_shell();
        let proxy_url = format!("http://{}:{}", host, port);

        match shell_type {
            ShellType::Bash | ShellType::Zsh => format!(
                r#"export HTTP_PROXY={proxy}
export HTTPS_PROXY={proxy}
export ALL_PROXY={proxy}
export NO_PROXY={bypass}
export http_proxy={proxy}
export https_proxy={proxy}
export all_proxy={proxy}
export no_proxy={bypass}"#,
                proxy = proxy_url,
                bypass = bypass
            ),
            ShellType::Fish => format!(
                r#"set -x HTTP_PROXY {proxy}
set -x HTTPS_PROXY {proxy}
set -x ALL_PROXY {proxy}
set -x NO_PROXY {bypass}
set -x http_proxy {proxy}
set -x https_proxy {proxy}
set -x all_proxy {proxy}
set -x no_proxy {bypass}"#,
                proxy = proxy_url,
                bypass = bypass
            ),
            ShellType::PowerShell => format!(
                r#"$env:HTTP_PROXY = "{proxy}"
$env:HTTPS_PROXY = "{proxy}"
$env:ALL_PROXY = "{proxy}"
$env:NO_PROXY = "{bypass}"
$env:http_proxy = "{proxy}"
$env:https_proxy = "{proxy}"
$env:all_proxy = "{proxy}"
$env:no_proxy = "{bypass}""#,
                proxy = proxy_url,
                bypass = bypass
            ),
            ShellType::Cmd => format!(
                r#"set HTTP_PROXY={proxy}
set HTTPS_PROXY={proxy}
set ALL_PROXY={proxy}
set NO_PROXY={bypass}"#,
                proxy = proxy_url,
                bypass = bypass
            ),
            ShellType::Unknown => format!(
                r#"# Unknown shell type
# Use these environment variables:
HTTP_PROXY={proxy}
HTTPS_PROXY={proxy}
ALL_PROXY={proxy}
NO_PROXY={bypass}"#,
                proxy = proxy_url,
                bypass = bypass
            ),
        }
    }

    pub fn disable_temporary() -> String {
        let shell_type = Self::detect_shell();

        match shell_type {
            ShellType::Bash | ShellType::Zsh => r#"unset HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY http_proxy https_proxy all_proxy no_proxy"#.to_string(),
            ShellType::Fish => r#"set -e HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY http_proxy https_proxy all_proxy no_proxy"#.to_string(),
            ShellType::PowerShell => r#"Remove-Item Env:\HTTP_PROXY, Env:\HTTPS_PROXY, Env:\ALL_PROXY, Env:\NO_PROXY, Env:\http_proxy, Env:\https_proxy, Env:\all_proxy, Env:\no_proxy -ErrorAction SilentlyContinue"#.to_string(),
            ShellType::Cmd => r#"set HTTP_PROXY=
set HTTPS_PROXY=
set ALL_PROXY=
set NO_PROXY=" "#.to_string(),
            ShellType::Unknown => r#"# Unknown shell type
# Unset these environment variables:
unset HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY"#.to_string(),
        }
    }

    pub fn status(&self) -> ShellProxyStatus {
        let has_persistent_config = self.config_paths.iter().any(|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|content| content.contains(START_MARKER))
                .unwrap_or(false)
        });

        let http_proxy = std::env::var("HTTP_PROXY")
            .ok()
            .or_else(|| std::env::var("http_proxy").ok());
        let https_proxy = std::env::var("HTTPS_PROXY")
            .ok()
            .or_else(|| std::env::var("https_proxy").ok());

        ShellProxyStatus {
            shell_type: self.shell_type,
            config_paths: self.config_paths.clone(),
            has_persistent_config,
            http_proxy,
            https_proxy,
        }
    }

    pub fn enable_persistent(&mut self, host: &str, port: u16, bypass: &str) -> Result<()> {
        if self.config_paths.is_empty() {
            return Err(BifrostError::Config(
                "Could not determine shell config file path".to_string(),
            ));
        }

        let proxy_url = format!("http://{}:{}", host, port);
        let config_block = self.generate_config_block(&proxy_url, bypass);

        for config_path in &self.config_paths {
            let content = if config_path.exists() {
                std::fs::read_to_string(config_path)?
            } else {
                String::new()
            };
            let new_content = self.replace_or_add_config_block(&content, &config_block);
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(config_path, new_content)?;
        }

        Ok(())
    }

    pub fn disable_persistent(&mut self) -> Result<()> {
        if self.config_paths.is_empty() {
            return Ok(());
        }

        for config_path in &self.config_paths {
            if !config_path.exists() {
                continue;
            }
            let content = std::fs::read_to_string(config_path)?;
            let new_content = self.remove_config_block(&content);
            if new_content != content {
                self.write_or_remove_empty_config(config_path, new_content)?;
            }
        }

        Ok(())
    }

    pub fn restore(&mut self) -> Result<()> {
        let backup = self
            .load_backup()
            .ok()
            .map(|backup| self.normalize_backup(backup));
        let mut paths = self.config_paths.clone();
        if let Some(backup) = backup {
            for file in backup.files {
                paths.push(PathBuf::from(file.path));
            }
        }
        paths.sort();
        paths.dedup();

        self.remove_bifrost_blocks_from_paths(paths)?;

        self.remove_backup();

        Ok(())
    }

    pub fn recover_from_crash(data_dir: &Path) -> Result<()> {
        let mut manager = Self::new(data_dir.to_path_buf());
        if manager.backup_file_path().exists() {
            manager.restore()?;
        }
        Ok(())
    }

    fn generate_config_block(&self, proxy_url: &str, bypass: &str) -> String {
        match self.shell_type {
            ShellType::Bash | ShellType::Zsh => format!(
                "{}\nexport HTTP_PROXY={}\nexport HTTPS_PROXY={}\nexport ALL_PROXY={}\nexport NO_PROXY={}\nexport http_proxy={}\nexport https_proxy={}\nexport all_proxy={}\nexport no_proxy={}\n{}",
                START_MARKER, proxy_url, proxy_url, proxy_url, bypass, proxy_url, proxy_url, proxy_url, bypass, END_MARKER
            ),
            ShellType::Fish => format!(
                "{}\nset -x HTTP_PROXY {}\nset -x HTTPS_PROXY {}\nset -x ALL_PROXY {}\nset -x NO_PROXY {}\nset -x http_proxy {}\nset -x https_proxy {}\nset -x all_proxy {}\nset -x no_proxy {}\n{}",
                START_MARKER, proxy_url, proxy_url, proxy_url, bypass, proxy_url, proxy_url, proxy_url, bypass, END_MARKER
            ),
            _ => String::new(),
        }
    }

    fn replace_or_add_config_block(&self, content: &str, new_block: &str) -> String {
        let has_start = content.find(START_MARKER);
        let has_end = content.find(END_MARKER);

        match (has_start, has_end) {
            (Some(start), Some(end)) if start < end => {
                let before = &content[..start];
                let after = &content[end + END_MARKER.len()..];
                format!("{}{}{}", before, new_block, after)
            }
            _ => {
                if content.is_empty() {
                    new_block.to_string()
                } else {
                    let trimmed = content.trim_end();
                    format!("{}\n\n{}", trimmed, new_block)
                }
            }
        }
    }

    fn remove_config_block(&self, content: &str) -> String {
        let has_start = content.find(START_MARKER);
        let has_end = content.find(END_MARKER);

        match (has_start, has_end) {
            (Some(start), Some(end)) if start < end => {
                let before = &content[..start];
                let after = &content[end + END_MARKER.len()..];
                format!("{}{}", before.trim_end(), after)
            }
            _ => content.to_string(),
        }
    }

    fn backup_file_path(&self) -> PathBuf {
        self.data_dir.join(BACKUP_FILE_NAME)
    }

    #[cfg(test)]
    fn save_backup(&self, files: Vec<ShellProxyBackupFile>) -> Result<()> {
        let backup = ShellProxyBackup {
            shell_type: self.shell_type.as_str().to_string(),
            files,
            config_path: None,
            original_content: None,
        };

        let content = serde_json::to_string_pretty(&backup)?;

        if let Some(parent) = self.backup_file_path().parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(self.backup_file_path(), content)?;

        Ok(())
    }

    fn load_backup(&self) -> Result<ShellProxyBackup> {
        let content = std::fs::read_to_string(self.backup_file_path())?;
        let backup: ShellProxyBackup = serde_json::from_str(&content)?;
        Ok(backup)
    }

    fn normalize_backup(&self, mut backup: ShellProxyBackup) -> ShellProxyBackup {
        if backup.files.is_empty() {
            if let Some(config_path) = backup.config_path.take() {
                backup.files.push(ShellProxyBackupFile {
                    path: config_path,
                    original_content: backup.original_content.take(),
                });
            }
        }
        backup
    }

    fn remove_backup(&self) {
        let _ = std::fs::remove_file(self.backup_file_path());
    }

    fn remove_bifrost_blocks_from_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        for config_path in paths {
            if !config_path.exists() {
                continue;
            }
            let content = std::fs::read_to_string(&config_path)?;
            let new_content = self.remove_config_block(&content);
            if new_content != content {
                self.write_or_remove_empty_config(&config_path, new_content)?;
            }
        }
        Ok(())
    }

    fn write_or_remove_empty_config(&self, config_path: &Path, content: String) -> Result<()> {
        if content.trim().is_empty() {
            let _ = std::fs::remove_file(config_path);
            return Ok(());
        }
        std::fs::write(config_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a manager with explicit shell type and config paths, bypassing the
    /// environment-dependent `detect_shell` / `get_config_paths` so the
    /// file-mutation logic can be tested deterministically.
    fn manager_with(
        data_dir: PathBuf,
        shell_type: ShellType,
        config_paths: Vec<PathBuf>,
    ) -> ShellProxyManager {
        ShellProxyManager {
            data_dir,
            shell_type,
            config_paths,
        }
    }

    #[test]
    fn shell_type_as_str_covers_all_variants() {
        assert_eq!(ShellType::Bash.as_str(), "bash");
        assert_eq!(ShellType::Zsh.as_str(), "zsh");
        assert_eq!(ShellType::Fish.as_str(), "fish");
        assert_eq!(ShellType::PowerShell.as_str(), "powershell");
        assert_eq!(ShellType::Cmd.as_str(), "cmd");
        assert_eq!(ShellType::Unknown.as_str(), "unknown");
    }

    #[test]
    fn get_config_paths_maps_each_shell() {
        // Note: get_config_paths relies on HOME; assert structure, not exact path.
        let bash = ShellProxyManager::get_config_paths(ShellType::Bash);
        if !bash.is_empty() {
            assert!(bash.iter().any(|p| p.ends_with(".bashrc")));
        }
        let zsh = ShellProxyManager::get_config_paths(ShellType::Zsh);
        if !zsh.is_empty() {
            assert!(zsh.iter().any(|p| p.ends_with(".zshrc")));
        }
        let fish = ShellProxyManager::get_config_paths(ShellType::Fish);
        if !fish.is_empty() {
            assert!(fish.iter().any(|p| p.ends_with("config.fish")));
        }
        assert!(ShellProxyManager::get_config_paths(ShellType::PowerShell).is_empty());
        assert!(ShellProxyManager::get_config_paths(ShellType::Cmd).is_empty());
        assert!(ShellProxyManager::get_config_paths(ShellType::Unknown).is_empty());
    }

    #[test]
    fn detect_shell_returns_a_variant() {
        // Whatever the environment, it returns a defined variant without panicking.
        let _ = ShellProxyManager::detect_shell();
    }

    #[test]
    fn enable_temporary_renders_for_current_shell() {
        let out = ShellProxyManager::enable_temporary("127.0.0.1", 7890, "localhost");
        // Every rendering mentions the proxy host:port and bypass somewhere.
        assert!(out.contains("127.0.0.1:7890"));
        assert!(out.contains("localhost"));
    }

    #[test]
    fn disable_temporary_is_nonempty() {
        let out = ShellProxyManager::disable_temporary();
        assert!(!out.is_empty());
    }

    #[test]
    fn new_builds_manager() {
        let tmp = TempDir::new().unwrap();
        let mgr = ShellProxyManager::new(tmp.path().to_path_buf());
        // Accessors work.
        let _ = mgr.shell_type();
        let _ = mgr.config_paths();
    }

    #[test]
    fn accessors_reflect_construction() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join(".bashrc");
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, vec![cfg.clone()]);
        assert_eq!(mgr.shell_type(), ShellType::Bash);
        assert_eq!(mgr.config_paths(), &[cfg]);
    }

    #[test]
    fn enable_persistent_errors_without_config_paths() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Unknown, Vec::new());
        let err = mgr
            .enable_persistent("127.0.0.1", 7890, "localhost")
            .unwrap_err();
        assert!(matches!(err, BifrostError::Config(_)));
    }

    #[test]
    fn enable_then_disable_persistent_round_trip() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        std::fs::write(&rc, "# existing user config\nalias ll='ls -la'\n").unwrap();
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Zsh, vec![rc.clone()]);

        mgr.enable_persistent("127.0.0.1", 7890, "localhost")
            .unwrap();
        let after_enable = std::fs::read_to_string(&rc).unwrap();
        assert!(after_enable.contains(START_MARKER));
        assert!(after_enable.contains(END_MARKER));
        assert!(after_enable.contains("HTTP_PROXY=http://127.0.0.1:7890"));
        // Existing user content is preserved.
        assert!(after_enable.contains("alias ll='ls -la'"));
        // Persistent proxy management is marker-block based and does not rely
        // on whole-file backups that could overwrite concurrent user edits.
        assert!(!tmp.path().join(BACKUP_FILE_NAME).exists());

        mgr.disable_persistent().unwrap();
        let after_disable = std::fs::read_to_string(&rc).unwrap();
        assert!(!after_disable.contains(START_MARKER));
        assert!(after_disable.contains("alias ll='ls -la'"));
    }

    #[test]
    fn enable_persistent_replaces_existing_block() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".bashrc");
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, vec![rc.clone()]);

        mgr.enable_persistent("127.0.0.1", 1111, "localhost")
            .unwrap();
        mgr.enable_persistent("127.0.0.1", 2222, "localhost")
            .unwrap();
        let content = std::fs::read_to_string(&rc).unwrap();
        // Only the latest block remains; no duplicate markers.
        assert_eq!(content.matches(START_MARKER).count(), 1);
        assert!(content.contains(":2222"));
        assert!(!content.contains(":1111"));
    }

    #[test]
    fn disable_persistent_is_noop_without_config_paths() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Unknown, Vec::new());
        mgr.disable_persistent().unwrap();
    }

    #[test]
    fn disable_persistent_without_marker_does_not_write_backup() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        std::fs::write(&rc, "# user config\nalias gl='git pull'\n").unwrap();
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Zsh, vec![rc.clone()]);

        mgr.disable_persistent().unwrap();

        assert_eq!(
            std::fs::read_to_string(&rc).unwrap(),
            "# user config\nalias gl='git pull'\n"
        );
        assert!(!tmp.path().join(BACKUP_FILE_NAME).exists());
    }

    #[test]
    fn disable_persistent_only_removes_bifrost_blocks() {
        let tmp = TempDir::new().unwrap();
        let zshrc = tmp.path().join(".zshrc");
        let zprofile = tmp.path().join(".zprofile");
        std::fs::write(&zshrc, "# user config\n").unwrap();
        std::fs::write(
            &zprofile,
            format!(
                "before\n{}\nproxy block\n{}\nafter\n",
                START_MARKER, END_MARKER
            ),
        )
        .unwrap();
        let mut mgr = manager_with(
            tmp.path().to_path_buf(),
            ShellType::Zsh,
            vec![zshrc.clone(), zprofile.clone()],
        );

        mgr.disable_persistent().unwrap();

        assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), "# user config\n");
        assert_eq!(
            std::fs::read_to_string(&zprofile).unwrap(),
            "before\nafter\n"
        );
        assert!(!tmp.path().join(BACKUP_FILE_NAME).exists());
    }

    #[test]
    fn status_detects_persistent_config_and_env() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".bashrc");
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, vec![rc.clone()]);
        // Before enable: no persistent config.
        let before = mgr.status();
        assert!(!before.has_persistent_config);
        assert_eq!(before.shell_type, ShellType::Bash);

        mgr.enable_persistent("127.0.0.1", 7890, "localhost")
            .unwrap();
        let after = mgr.status();
        assert!(after.has_persistent_config);
        assert_eq!(after.config_paths, vec![rc]);
    }

    #[test]
    fn restore_removes_bifrost_block_without_overwriting_user_changes() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        std::fs::write(&rc, "ORIGINAL\n").unwrap();
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Zsh, vec![rc.clone()]);

        mgr.enable_persistent("127.0.0.1", 7890, "localhost")
            .unwrap();
        assert_ne!(std::fs::read_to_string(&rc).unwrap(), "ORIGINAL\n");
        let edited = std::fs::read_to_string(&rc).unwrap() + "\n# user edited while proxy active\n";
        std::fs::write(&rc, edited).unwrap();

        mgr.restore().unwrap();
        let restored = std::fs::read_to_string(&rc).unwrap();
        assert!(!restored.contains(START_MARKER));
        assert!(!restored.contains(END_MARKER));
        assert!(restored.contains("ORIGINAL"));
        assert!(restored.contains("# user edited while proxy active"));
        assert!(!tmp.path().join(BACKUP_FILE_NAME).exists());
    }

    #[test]
    fn restore_removes_file_when_no_original_content() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".bashrc"); // does not exist yet
        let mut mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, vec![rc.clone()]);

        // Enable creates the file with only the managed Bifrost block.
        mgr.enable_persistent("127.0.0.1", 7890, "localhost")
            .unwrap();
        assert!(rc.exists());

        mgr.restore().unwrap();
        // Removing the managed block leaves the file empty, so it is removed.
        assert!(!rc.exists());
    }

    #[test]
    fn recover_from_crash_uses_legacy_backup_paths_without_overwriting_content() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Zsh, vec![rc.clone()]);
        mgr.save_backup(vec![ShellProxyBackupFile {
            path: rc.to_string_lossy().to_string(),
            original_content: Some("STALE BACKUP\n".to_string()),
        }])
        .unwrap();
        std::fs::write(
            &rc,
            format!(
                "USER BEFORE\n{}\nproxy block\n{}\nUSER AFTER\n",
                START_MARKER, END_MARKER
            ),
        )
        .unwrap();

        ShellProxyManager::recover_from_crash(tmp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&rc).unwrap(),
            "USER BEFORE\nUSER AFTER\n"
        );
        assert!(!tmp.path().join(BACKUP_FILE_NAME).exists());
    }

    #[test]
    fn recover_from_crash_noop_without_backup() {
        let tmp = TempDir::new().unwrap();
        // No backup file present.
        ShellProxyManager::recover_from_crash(tmp.path()).unwrap();
    }

    #[test]
    fn generate_config_block_empty_for_non_posix_shells() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::PowerShell, Vec::new());
        assert!(mgr.generate_config_block("http://x", "y").is_empty());
    }

    #[test]
    fn replace_or_add_handles_empty_and_appended() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, Vec::new());
        // Empty content -> returns block as-is.
        assert_eq!(mgr.replace_or_add_config_block("", "BLOCK"), "BLOCK");
        // Non-empty content with no markers -> appended after a blank line.
        let out = mgr.replace_or_add_config_block("line1\n", "BLOCK");
        assert_eq!(out, "line1\n\nBLOCK");
    }

    #[test]
    fn remove_config_block_no_markers_is_identity() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, Vec::new());
        assert_eq!(mgr.remove_config_block("nothing here"), "nothing here");
    }

    #[test]
    fn normalize_backup_migrates_legacy_single_file_fields() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, Vec::new());
        let legacy = ShellProxyBackup {
            shell_type: "bash".to_string(),
            files: Vec::new(),
            config_path: Some("/home/u/.bashrc".to_string()),
            original_content: Some("OLD".to_string()),
        };
        let normalized = mgr.normalize_backup(legacy);
        assert_eq!(normalized.files.len(), 1);
        assert_eq!(normalized.files[0].path, "/home/u/.bashrc");
        assert_eq!(normalized.files[0].original_content.as_deref(), Some("OLD"));
    }

    #[test]
    fn load_backup_errors_when_missing() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Bash, Vec::new());
        assert!(mgr.load_backup().is_err());
    }

    #[test]
    fn save_and_load_backup_round_trip() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_with(tmp.path().to_path_buf(), ShellType::Zsh, Vec::new());
        let files = vec![ShellProxyBackupFile {
            path: "/p/.zshrc".to_string(),
            original_content: Some("X".to_string()),
        }];
        mgr.save_backup(files).unwrap();
        let loaded = mgr.load_backup().unwrap();
        assert_eq!(loaded.shell_type, "zsh");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].path, "/p/.zshrc");
    }
}
