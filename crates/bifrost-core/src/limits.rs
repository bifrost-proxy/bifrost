use crate::BifrostError;
use std::path::Path;

pub const MAX_RULE_FILE_BYTES: u64 = 256 * 1024 * 1024;

pub fn ensure_file_size_within_limit(path: &Path, limit: u64) -> Result<(), BifrostError> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > limit {
            return Err(BifrostError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "file too large ({} bytes, limit {} bytes)",
                    meta.len(),
                    limit
                ),
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_file_size_within_limit_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rule.bifrost");
        std::fs::write(&path, b"abcd").unwrap();

        let error = ensure_file_size_within_limit(&path, 3).unwrap_err();

        assert!(error.to_string().contains("file too large"));
    }

    #[test]
    fn ensure_file_size_within_limit_accepts_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rule.bifrost");
        std::fs::write(&path, b"abcd").unwrap();
        assert!(ensure_file_size_within_limit(&path, 1024).is_ok());
    }

    #[test]
    fn ensure_file_size_within_limit_ok_when_path_missing() {
        // metadata() fails -> the function treats it as within limit (Ok).
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.bifrost");
        assert!(ensure_file_size_within_limit(&missing, 1).is_ok());
    }
}
