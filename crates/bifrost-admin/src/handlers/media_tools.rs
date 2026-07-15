use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tokio::process::Command;

/// Build a command for an ASR media tool without assuming that a desktop
/// process inherited the user's interactive-shell PATH.
pub(crate) fn command(tool: &str) -> Command {
    let path = std::env::var_os("PATH");
    let fallback_dirs = common_media_tool_dirs();
    let resolved = resolve_media_tool(tool, path.as_deref(), &fallback_dirs);
    if resolved != Path::new(tool) {
        tracing::debug!(
            tool,
            resolved = %resolved.display(),
            "resolved ASR media tool executable"
        );
    }
    Command::new(resolved)
}

fn common_media_tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/local/bin",
            "/home/linuxbrew/.linuxbrew/bin",
            "/usr/bin",
            "/bin",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    dirs
}

fn resolve_media_tool(tool: &str, path: Option<&OsStr>, fallback_dirs: &[PathBuf]) -> PathBuf {
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            if let Some(candidate) = executable_in_dir(&dir, tool) {
                return candidate;
            }
        }
    }

    for dir in fallback_dirs {
        if let Some(candidate) = executable_in_dir(dir, tool) {
            return candidate;
        }
    }

    PathBuf::from(tool)
}

fn executable_in_dir(dir: &Path, tool: &str) -> Option<PathBuf> {
    let candidate = dir.join(tool);
    if is_executable_file(&candidate) {
        return Some(candidate);
    }

    #[cfg(windows)]
    {
        let candidate = dir.join(format!("{tool}.exe"));
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }

    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_executable(path: &Path) {
        std::fs::write(path, b"test executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }
    }

    #[test]
    fn media_tool_prefers_path_before_fallback_directories() {
        let path_dir = TempDir::new().unwrap();
        let fallback_dir = TempDir::new().unwrap();
        let path_tool = path_dir.path().join("ffmpeg");
        let fallback_tool = fallback_dir.path().join("ffmpeg");
        write_executable(&path_tool);
        write_executable(&fallback_tool);

        let path = std::env::join_paths([path_dir.path()]).unwrap();
        let resolved = resolve_media_tool(
            "ffmpeg",
            Some(path.as_os_str()),
            &[fallback_dir.path().to_path_buf()],
        );

        assert_eq!(resolved, path_tool);
    }

    #[test]
    fn media_tool_uses_fallback_when_path_is_sanitized() {
        let empty_path_dir = TempDir::new().unwrap();
        let fallback_dir = TempDir::new().unwrap();
        let fallback_tool = fallback_dir.path().join("ffprobe");
        write_executable(&fallback_tool);

        let path = std::env::join_paths([empty_path_dir.path()]).unwrap();
        let resolved = resolve_media_tool(
            "ffprobe",
            Some(path.as_os_str()),
            &[fallback_dir.path().to_path_buf()],
        );

        assert_eq!(resolved, fallback_tool);
    }

    #[test]
    fn media_tool_ignores_non_executable_candidates() {
        let fallback_dir = TempDir::new().unwrap();
        let candidate = fallback_dir.path().join("ffmpeg");
        std::fs::write(&candidate, b"not executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&candidate).unwrap().permissions();
            permissions.set_mode(0o644);
            std::fs::set_permissions(&candidate, permissions).unwrap();
        }

        let resolved = resolve_media_tool("ffmpeg", None, &[fallback_dir.path().to_path_buf()]);

        #[cfg(unix)]
        assert_eq!(resolved, PathBuf::from("ffmpeg"));
        #[cfg(not(unix))]
        assert_eq!(resolved, candidate);
    }

    #[test]
    fn media_tool_returns_bare_name_when_unresolved() {
        let empty_dir = TempDir::new().unwrap();

        let resolved = resolve_media_tool(
            "missing-media-tool",
            None,
            &[empty_dir.path().to_path_buf()],
        );

        assert_eq!(resolved, PathBuf::from("missing-media-tool"));
    }
}
