use std::path::{Path, PathBuf};

const MAX_ALIAS_NAME_LEN: usize = 64;

pub fn process_alias_executable(
    executable: &Path,
    _alias_dir: &Path,
    alias_name: &str,
) -> std::result::Result<PathBuf, String> {
    validate_alias_name(alias_name)?;
    Ok(executable.to_path_buf())
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

#[cfg(test)]
mod tests {
    use super::process_alias_executable;

    #[test]
    fn process_alias_executable_returns_original_executable() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("bifrost");
        std::fs::write(&executable, b"bin").unwrap();

        let alias =
            process_alias_executable(&executable, &temp.path().join("aliases"), "bifrost-agent")
                .unwrap();

        assert_eq!(alias, executable);
        assert!(!temp.path().join("aliases").exists());
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
