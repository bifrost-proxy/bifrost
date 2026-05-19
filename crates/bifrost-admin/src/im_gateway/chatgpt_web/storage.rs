use std::path::Path;

use serde_json::Value;

use super::AuthState;

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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).write(true).truncate(true).mode(0o600);
        let mut file = options
            .open(path)
            .await
            .map_err(|error| format!("open {} failed: {error}", path.display()))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, content)
            .await
            .map_err(|error| format!("write {} failed: {error}", path.display()))?;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| format!("chmod {} failed: {error}", path.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(path, content)
            .await
            .map_err(|error| format!("write {} failed: {error}", path.display()))
    }
}

pub(super) async fn write_redacted_json(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("serialize {} failed: {error}", path.display()))?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| format!("write {} failed: {error}", path.display()))
}
