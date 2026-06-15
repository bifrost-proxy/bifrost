use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::AuthState;

static SECRET_FILE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) async fn read_auth_state(path: &Path) -> Result<AuthState, String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("read auth state failed: {error}"))?;
    serde_json::from_str(&content).map_err(|error| format!("parse auth state failed: {error}"))
}

pub(super) async fn write_auth_state(path: &Path, state: &AuthState) -> Result<(), String> {
    let content = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("serialize auth state failed: {error}"))?;
    write_secret_file(path, &content).await
}

pub(super) async fn write_secret_file(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("create {} failed: {error}", parent.display()))?;
    }
    let tmp_path = secret_file_tmp_path(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::AsyncWriteExt;
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true).mode(0o600);
        let mut file = options
            .open(&tmp_path)
            .await
            .map_err(|error| format!("open {} failed: {error}", tmp_path.display()))?;
        file.write_all(content)
            .await
            .map_err(|error| format!("write {} failed: {error}", tmp_path.display()))?;
        file.flush()
            .await
            .map_err(|error| format!("flush {} failed: {error}", tmp_path.display()))?;
        drop(file);
        tokio::fs::rename(&tmp_path, path)
            .await
            .map_err(|error| format!("rename {} failed: {error}", path.display()))?;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| format!("chmod {} failed: {error}", path.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(&tmp_path, content)
            .await
            .map_err(|error| format!("write {} failed: {error}", tmp_path.display()))?;
        if path.exists() {
            let _ = tokio::fs::remove_file(path).await;
        }
        tokio::fs::rename(&tmp_path, path)
            .await
            .map_err(|error| format!("rename {} failed: {error}", path.display()))
    }
}

fn secret_file_tmp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("secret");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = SECRET_FILE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        nanos,
        counter
    ))
}

pub(super) async fn write_redacted_json(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("serialize {} failed: {error}", path.display()))?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| format!("write {} failed: {error}", path.display()))
}
