use std::fs;
use std::path::{Path, PathBuf};

const MAX_ALIAS_NAME_LEN: usize = 64;

pub fn process_alias_executable(
    executable: &Path,
    alias_dir: &Path,
    alias_name: &str,
) -> std::result::Result<PathBuf, String> {
    validate_alias_name(alias_name)?;
    let alias_path = alias_dir.join(platform_alias_name(alias_name));
    if executable.file_name() == alias_path.file_name() {
        return Ok(executable.to_path_buf());
    }
    fs::create_dir_all(alias_dir)
        .map_err(|error| format!("create process alias dir {}: {error}", alias_dir.display()))?;
    refresh_alias(executable, &alias_path)?;
    Ok(alias_path)
}

fn validate_alias_name(alias_name: &str) -> std::result::Result<(), String> {
    if alias_name.is_empty() {
        return Err("process alias name is empty".to_string());
    }
    if alias_name.len() > MAX_ALIAS_NAME_LEN {
        return Err(format!(
            "process alias name is too long: {} > {}",
            alias_name.len(),
            MAX_ALIAS_NAME_LEN
        ));
    }
    if alias_name
        .bytes()
        .any(|byte| matches!(byte, b'/' | b'\\' | b':' | 0))
    {
        return Err(format!(
            "process alias name contains a path separator: {alias_name}"
        ));
    }
    Ok(())
}

fn platform_alias_name(alias_name: &str) -> String {
    #[cfg(windows)]
    {
        if alias_name.ends_with(".exe") {
            alias_name.to_string()
        } else {
            format!("{alias_name}.exe")
        }
    }
    #[cfg(not(windows))]
    {
        alias_name.to_string()
    }
}

fn refresh_alias(executable: &Path, alias_path: &Path) -> std::result::Result<(), String> {
    if alias_points_to(alias_path, executable) {
        return Ok(());
    }
    if alias_path.exists() || fs::symlink_metadata(alias_path).is_ok() {
        fs::remove_file(alias_path).map_err(|error| {
            format!(
                "remove stale process alias {}: {error}",
                alias_path.display()
            )
        })?;
    }
    create_alias(executable, alias_path)
}

#[cfg(unix)]
fn create_alias(executable: &Path, alias_path: &Path) -> std::result::Result<(), String> {
    std::os::unix::fs::symlink(executable, alias_path).map_err(|error| {
        format!(
            "create process alias {} -> {}: {error}",
            alias_path.display(),
            executable.display()
        )
    })
}

#[cfg(windows)]
fn create_alias(executable: &Path, alias_path: &Path) -> std::result::Result<(), String> {
    fs::copy(executable, alias_path)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "copy process alias {} -> {}: {error}",
                alias_path.display(),
                executable.display()
            )
        })
}

#[cfg(unix)]
fn alias_points_to(alias_path: &Path, executable: &Path) -> bool {
    fs::read_link(alias_path)
        .map(|target| target == executable)
        .unwrap_or(false)
}

#[cfg(windows)]
fn alias_points_to(alias_path: &Path, executable: &Path) -> bool {
    if !alias_path.is_file() {
        return false;
    }
    let alias_meta = match fs::metadata(alias_path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    let executable_meta = match fs::metadata(executable) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    alias_meta.len() == executable_meta.len()
}

#[cfg(test)]
mod tests {
    use super::process_alias_executable;

    #[test]
    fn process_alias_executable_creates_named_alias() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("bifrost");
        std::fs::write(&executable, b"bin").unwrap();

        let alias =
            process_alias_executable(&executable, &temp.path().join("aliases"), "bifrost-agent")
                .unwrap();

        assert_eq!(alias.file_name().unwrap(), "bifrost-agent");
        assert!(alias.exists() || std::fs::symlink_metadata(&alias).is_ok());
        #[cfg(unix)]
        assert_eq!(std::fs::read_link(alias).unwrap(), executable);
    }

    #[test]
    fn process_alias_executable_rejects_path_names() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("bifrost");
        std::fs::write(&executable, b"bin").unwrap();

        let error =
            process_alias_executable(&executable, temp.path(), "nested/bifrost-agent").unwrap_err();

        assert!(error.contains("path separator"), "{error}");
    }
}
